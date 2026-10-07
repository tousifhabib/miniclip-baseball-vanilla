//! Plays the extracted game in a window, with sound and a working pointer.
//!
//! Space pauses, the right arrow steps one frame while paused, F1 opens the
//! inspector, Escape quits.

mod inspector;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bb_engine::audio::Audio;
use bb_engine::display::Event;
use bb_engine::gpu::Renderer;
use bb_engine::library::Library;
use bb_engine::math::Matrix;
use bb_engine::stage::Stage;
use clap::Parser;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::inspector::{Action, Info, Inspector};

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
    /// Keep the top timeline on its frame while the clips inside it play, even
    /// where the original has no `stop()`.
    #[arg(long)]
    hold: bool,
    /// Play no sound.
    #[arg(long)]
    mute: bool,
    /// Open with the inspector showing.
    #[arg(long)]
    inspect: bool,
    /// Load every sound, report any that fail, and quit without a window.
    #[arg(long)]
    check_sounds: bool,
    /// Quit after drawing this many frames. For testing.
    #[arg(long)]
    exit_after: Option<u32>,
}

struct View {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    egui: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
}

struct Player {
    args: Args,
    library: Library,
    stage: Stage,
    audio: Option<Audio>,
    inspector: Inspector,
    paused: bool,
    last_redraw: Instant,
    /// Time that has passed but not yet been played.
    owed: Duration,
    /// A running average of the time between redraws.
    frame_time: f32,
    drawn: u32,
    sounds_asked: u32,
    /// Where the pointer is, in window pixels, and whether its button is held.
    cursor: (f32, f32),
    pressed: bool,
    view: Option<View>,
    error: Option<anyhow::Error>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let library = Library::load(&args.dir)?;

    if args.check_sounds {
        let mut audio = Audio::new()?;
        let loaded = audio.load_all(&library);
        println!("Loaded {loaded} sounds.");
        for problem in &audio.problems {
            println!("  problem: {problem}");
        }
        return Ok(());
    }

    let mut stage = Stage::new(args.clip, &library);
    stage.goto(args.frame, &library);
    if args.hold {
        stage.root.playing = false;
    }
    let audio = if args.mute {
        None
    } else {
        // A machine with no sound device can still play, silently.
        Audio::new()
            .inspect_err(|error| eprintln!("Playing without sound: {error:#}"))
            .ok()
    };

    let mut inspector = Inspector::new();
    inspector.open = args.inspect;
    let mut player = Player {
        args,
        library,
        stage,
        audio,
        inspector,
        paused: false,
        last_redraw: Instant::now(),
        owed: Duration::ZERO,
        frame_time: 1.0 / 60.0,
        drawn: 0,
        sounds_asked: 0,
        cursor: (0.0, 0.0),
        pressed: false,
        view: None,
        error: None,
    };
    let event_loop = EventLoop::new().context("starting the window system")?;
    event_loop
        .run_app(&mut player)
        .context("running the window")?;

    println!(
        "Drew {} frames; the game asked for {} sounds.",
        player.drawn, player.sounds_asked
    );
    let audio_problems = player.audio.iter().flat_map(|audio| &audio.problems);
    let draw_problems = player.view.iter().flat_map(|view| &view.renderer.problems);
    for problem in draw_problems.chain(audio_problems) {
        println!("  problem: {problem}");
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

        let egui = egui_winit::State::new(
            egui::Context::default(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        );

        Ok(View {
            renderer: Renderer::new(device, queue, config.format),
            window,
            surface,
            config,
            egui,
            egui_renderer,
        })
    }

    /// How the stage sits in a window of this size: the transform from stage
    /// coordinates to window pixels, and the rectangle the stage covers.
    fn layout(&self, width: u32, height: u32) -> (Matrix, [u32; 4]) {
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
        let rectangle = [
            left as u32,
            top as u32,
            ((stage_width * scale).round() as u32).min(width - left as u32),
            ((stage_height * scale).round() as u32).min(height - top as u32),
        ];
        (base, rectangle)
    }

    /// Tells the stage where the pointer now is.
    fn pointer_changed(&mut self) {
        let Some(view) = &mut self.view else {
            return;
        };
        let (width, height) = (view.config.width, view.config.height);
        let (base, _) = self.layout(width, height);
        let Some(inverse) = base.inverse() else {
            return;
        };
        let (x, y) = inverse.apply(self.cursor.0, self.cursor.1);
        let Some(view) = &mut self.view else {
            return;
        };
        self.stage
            .pointer_changed(x, y, self.pressed, &self.library, &mut view.renderer);
    }

    /// Acts on what the stage says has happened.
    fn handle_events(&mut self) {
        for event in self.stage.take_events() {
            match event {
                Event::Sound(start) => {
                    if let Some(audio) = &mut self.audio {
                        audio.play(&self.library, &start);
                    }
                    self.sounds_asked += 1;
                    self.inspector.note(format!("sound {}", start.sound));
                }
                Event::Button {
                    symbol,
                    path,
                    event,
                } => {
                    self.inspector
                        .note(format!("button {symbol} at {path:?}: {event:?}"));
                }
            }
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::TogglePause => self.paused = !self.paused,
            Action::Step => self.step(),
            Action::SetPlaying(path, playing) => {
                if let Some(clip) = self.stage.clip_mut(&path) {
                    clip.playing = playing;
                }
            }
            Action::SetVisible(path, visible) => {
                if let Some(child) = self.stage.root.child_mut(&path) {
                    child.visible = visible;
                }
            }
            Action::Goto(path, frame) => {
                self.stage.goto_clip(&path, frame, &self.library);
                // Hold it there, or it would play straight on.
                if let Some(clip) = self.stage.clip_mut(&path) {
                    clip.playing = false;
                }
            }
        }
    }

    /// Plays one frame.
    fn step(&mut self) {
        if let Some(view) = &mut self.view {
            self.stage.advance(&self.library, &mut view.renderer);
        }
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let elapsed = now - self.last_redraw;
        self.last_redraw = now;
        self.frame_time += (elapsed.as_secs_f32() - self.frame_time) * 0.1;
        let frame = Duration::from_secs_f64(1.0 / self.library.manifest.stage.frame_rate);
        if !self.paused {
            // After a long stall, skip ahead instead of replaying it all.
            self.owed = (self.owed + elapsed).min(frame * 5);
            while self.owed >= frame {
                self.step();
                self.owed -= frame;
            }
        }
        self.handle_events();

        let Some((width, height)) = self
            .view
            .as_ref()
            .map(|view| (view.config.width, view.config.height))
        else {
            return;
        };
        let (base, scissor) = self.layout(width, height);
        let commands = self.stage.commands(base, &self.library);
        let background = self
            .library
            .manifest
            .stage
            .background
            .map_or([0.0, 0.0, 0.0, 1.0], |c| {
                [c.r, c.g, c.b, c.a].map(|channel| f64::from(channel) / 255.0)
            });
        let Some(view) = &mut self.view else {
            return;
        };

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
            &commands,
            &target,
            (width, height),
            background,
            Some(scissor),
        );

        // The inspector goes on top, in a pass of its own.
        let info = Info {
            stage: &self.stage,
            library: &self.library,
            stats: view.renderer.stats,
            paused: self.paused,
            frames_per_second: 1.0 / self.frame_time.max(1e-6),
            base,
        };
        let input = view.egui.take_egui_input(&view.window);
        let context = view.egui.egui_ctx().clone();
        let mut actions = Vec::new();
        let output = context.run_ui(input, |ui| {
            actions = self.inspector.ui(ui.ctx(), &info);
        });
        view.egui
            .handle_platform_output(&view.window, output.platform_output);
        let jobs = context.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: output.pixels_per_point,
        };
        let (device, queue) = (&view.renderer.device, &view.renderer.queue);
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                view.egui_renderer.update_texture(device, queue, *id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut buffers =
            view.egui_renderer
                .update_buffers(device, queue, &mut encoder, &jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("inspector"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            view.egui_renderer.render(&mut pass, &jobs, &screen);
        }
        buffers.push(encoder.finish());
        queue.submit(buffers);
        for id in &output.textures_delta.free {
            view.egui_renderer.free_texture(id);
        }

        view.renderer.queue.present(texture);
        let cursor = if self.stage.pointer.on_button() {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        };
        view.window.set_cursor(cursor);
        self.drawn += 1;

        for action in actions {
            self.apply(action);
        }
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
        // The inspector sees every event first, and keeps the ones meant for
        // it: a click on its panel must not reach the game underneath.
        let taken = match &mut self.view {
            Some(view) if self.inspector.open => {
                view.egui.on_window_event(&view.window, &event).consumed
            }
            _ => false,
        };

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
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                self.pointer_changed();
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                // A release always goes through, so that a press which began
                // on the game is not left hanging.
                let down = state == ElementState::Pressed;
                if !down || !taken {
                    self.pressed = down;
                    self.pointer_changed();
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
            } if !taken => match key {
                NamedKey::Space => self.paused = !self.paused,
                NamedKey::ArrowRight if self.paused => self.step(),
                NamedKey::F1 => self.inspector.open = !self.inspector.open,
                NamedKey::Escape => event_loop.exit(),
                _ => {}
            },
            _ => {}
        }
    }
}
