//! Plays the extracted game in a window.
//!
//! Space pauses, the right arrow steps one frame while paused, Escape quits.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bb_engine::display::{ClipState, commands};
use bb_engine::gpu::Renderer;
use bb_engine::library::Library;
use bb_engine::math::Matrix;
use clap::Parser;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

#[derive(Parser)]
#[command(about = "Plays the extracted game in a window")]
struct Args {
    /// The folder `bb-extract` wrote.
    dir: PathBuf,
    /// Show this clip on its own, with its origin at the centre of the
    /// window, instead of the main timeline.
    #[arg(long)]
    clip: Option<u16>,
    /// Start on this frame.
    #[arg(long, default_value_t = 1)]
    frame: u16,
    /// Keep the top timeline on its frame while the clips inside it play, as
    /// a `stop()` in the original would.
    #[arg(long)]
    hold: bool,
    /// Quit after drawing this many frames. For testing.
    #[arg(long)]
    exit_after: Option<u32>,
}

struct View {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

struct Player {
    args: Args,
    library: Library,
    clip: ClipState,
    paused: bool,
    last_redraw: Instant,
    /// Time that has passed but not yet been played.
    owed: Duration,
    drawn: u32,
    view: Option<View>,
    error: Option<anyhow::Error>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let library = Library::load(&args.dir)?;
    let mut clip = ClipState::new(args.clip, &library);
    clip.goto(args.frame, &library);
    clip.playing = !args.hold;

    let mut player = Player {
        args,
        library,
        clip,
        paused: false,
        last_redraw: Instant::now(),
        owed: Duration::ZERO,
        drawn: 0,
        view: None,
        error: None,
    };
    let event_loop = EventLoop::new().context("starting the window system")?;
    event_loop
        .run_app(&mut player)
        .context("running the window")?;

    println!("Drew {} frames.", player.drawn);
    if let Some(view) = &player.view {
        for problem in &view.renderer.problems {
            println!("  problem: {problem}");
        }
    }
    match player.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

impl Player {
    fn open(&self, event_loop: &ActiveEventLoop) -> Result<View> {
        let stage = &self.library.manifest.stage;
        let attributes = Window::default_attributes()
            .with_title("Miniclip Baseball (engine preview)")
            .with_inner_size(LogicalSize::new(stage.width, stage.height));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("opening the window")?,
        );

        let display = Box::new(event_loop.owned_display_handle());
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(display));
        let surface = instance
            .create_surface(window.clone())
            .context("attaching to the window")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..wgpu::RequestAdapterOptions::default()
        }))
        .context("finding a graphics adapter")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .context("opening the graphics device")?;

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("the window cannot be drawn to")?;
        // Flash blends colours as stored, so prefer a format that does not
        // convert them.
        let formats = surface.get_capabilities(&adapter).formats;
        if let Some(format) = formats.into_iter().find(|format| !format.is_srgb()) {
            config.format = format;
        }
        surface.configure(&device, &config);

        Ok(View {
            renderer: Renderer::new(device, queue, config.format),
            window,
            surface,
            config,
        })
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let frame = Duration::from_secs_f64(1.0 / self.library.manifest.stage.frame_rate);
        if !self.paused {
            // After a long stall, skip ahead instead of replaying it all.
            self.owed = (self.owed + (now - self.last_redraw)).min(frame * 5);
            while self.owed >= frame {
                self.clip.advance(&self.library);
                self.owed -= frame;
            }
        }
        self.last_redraw = now;

        let Some(view) = &mut self.view else {
            return;
        };
        let (width, height) = (view.config.width, view.config.height);
        let stage = &self.library.manifest.stage;
        let (stage_width, stage_height) = (stage.width as f32, stage.height as f32);
        // Fit the stage inside the window, centred, keeping its shape.
        let scale = (width as f32 / stage_width).min(height as f32 / stage_height);
        let (left, top) = (
            ((width as f32 - stage_width * scale) / 2.0).round(),
            ((height as f32 - stage_height * scale) / 2.0).round(),
        );
        let fit = Matrix::translate(left, top).then_inner(Matrix::scale(scale, scale));
        let base = match self.args.clip {
            Some(_) => fit.then_inner(Matrix::translate(stage_width / 2.0, stage_height / 2.0)),
            None => fit,
        };
        // Flash shows nothing outside the stage.
        let scissor = [
            left as u32,
            top as u32,
            ((stage_width * scale).round() as u32).min(width - left as u32),
            ((stage_height * scale).round() as u32).min(height - top as u32),
        ];
        let background = stage.background.map_or([0.0, 0.0, 0.0, 1.0], |c| {
            [c.r, c.g, c.b, c.a].map(|channel| f64::from(channel) / 255.0)
        });

        let texture = match view.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            // The window changed under us: set the surface up again and
            // draw on the next round.
            _ => {
                view.surface.configure(&view.renderer.device, &view.config);
                return;
            }
        };
        let target = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        // One point on screen, which is more than one pixel on a dense display.
        view.renderer.min_stroke = view.window.scale_factor() as f32;
        view.renderer.render(
            &self.library,
            &commands(&self.clip, base),
            &target,
            (width, height),
            background,
            Some(scissor),
        );
        view.renderer.queue.present(texture);
        self.drawn += 1;
    }
}

impl ApplicationHandler for Player {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.view.is_some() {
            return;
        }
        match self.open(event_loop) {
            Ok(view) => {
                view.window.request_redraw();
                self.last_redraw = Instant::now();
                self.view = Some(view);
            }
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(view) = &mut self.view {
                    view.config.width = size.width.max(1);
                    view.config.height = size.height.max(1);
                    view.surface.configure(&view.renderer.device, &view.config);
                    view.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if self
                    .args
                    .exit_after
                    .is_some_and(|limit| self.drawn >= limit)
                {
                    event_loop.exit();
                    return;
                }
                self.redraw();
                if let Some(view) = &self.view {
                    view.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(key),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => match key {
                NamedKey::Space => self.paused = !self.paused,
                NamedKey::ArrowRight if self.paused => self.clip.advance(&self.library),
                NamedKey::Escape => event_loop.exit(),
                _ => {}
            },
            _ => {}
        }
    }
}
