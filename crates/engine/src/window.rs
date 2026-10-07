//! Runs a [`Runner`] in a window, with sound, a working pointer, and the
//! inspector.
//!
//! Space pauses, the right arrow steps one frame while paused, F1 opens the
//! inspector, Escape quits.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::app::Runner;
use crate::gpu::Renderer;
use crate::inspector::{Action, Info, Inspector};
use crate::math::Matrix;

pub struct Options {
    pub title: String,
    /// Open with the inspector showing.
    pub inspect: bool,
    /// Put the top clip's origin at the centre of the window, for looking at
    /// one clip on its own.
    pub centre_origin: bool,
    /// Quit after drawing this many frames. For testing.
    pub exit_after: Option<u32>,
}

/// What a run of the window amounted to.
pub struct Summary {
    pub frames_drawn: u32,
    pub sounds_asked: u32,
    /// Things that could not be drawn or played.
    pub problems: Vec<String>,
}

/// Opens a window and plays until it is closed.
pub fn run(runner: Runner, options: Options) -> Result<Summary> {
    let mut inspector = Inspector::new();
    inspector.open = options.inspect;
    let mut app = App {
        runner,
        options,
        inspector,
        paused: false,
        last_redraw: Instant::now(),
        owed: Duration::ZERO,
        frame_time: 1.0 / 60.0,
        drawn: 0,
        cursor: (0.0, 0.0),
        pressed: false,
        view: None,
        error: None,
    };
    let event_loop = EventLoop::new().context("starting the window system")?;
    event_loop.run_app(&mut app).context("running the window")?;
    if let Some(error) = app.error {
        return Err(error);
    }

    let audio_problems = app.runner.audio.iter().flat_map(|audio| &audio.problems);
    let draw_problems = app.view.iter().flat_map(|view| &view.renderer.problems);
    Ok(Summary {
        frames_drawn: app.drawn,
        sounds_asked: app.runner.sounds_asked,
        problems: draw_problems.chain(audio_problems).cloned().collect(),
    })
}

struct View {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    egui: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
}

struct App {
    runner: Runner,
    options: Options,
    inspector: Inspector,
    paused: bool,
    last_redraw: Instant,
    /// Time that has passed but not yet been played.
    owed: Duration,
    /// A running average of the time between redraws.
    frame_time: f32,
    drawn: u32,
    /// Where the pointer is, in window pixels, and whether its button is held.
    cursor: (f32, f32),
    pressed: bool,
    view: Option<View>,
    error: Option<anyhow::Error>,
}

impl App {
    fn open(&self, event_loop: &ActiveEventLoop) -> Result<View> {
        let stage = &self.runner.library.manifest.stage;
        let attributes = Window::default_attributes()
            .with_title(&self.options.title)
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
        let stage = &self.runner.library.manifest.stage;
        let (stage_width, stage_height) = (stage.width as f32, stage.height as f32);
        // Fit the stage inside the window, centred, keeping its shape.
        let scale = (width as f32 / stage_width).min(height as f32 / stage_height);
        let (left, top) = (
            ((width as f32 - stage_width * scale) / 2.0).round(),
            ((height as f32 - stage_height * scale) / 2.0).round(),
        );
        let fit = Matrix::translate(left, top).then_inner(Matrix::scale(scale, scale));
        let base = if self.options.centre_origin {
            fit.then_inner(Matrix::translate(stage_width / 2.0, stage_height / 2.0))
        } else {
            fit
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
        let Some((width, height)) = self.size() else {
            return;
        };
        let (base, _) = self.layout(width, height);
        let Some(inverse) = base.inverse() else {
            return;
        };
        let (x, y) = inverse.apply(self.cursor.0, self.cursor.1);
        if let Some(view) = &mut self.view {
            self.runner.pointer(x, y, self.pressed, &mut view.renderer);
        }
    }

    fn size(&self) -> Option<(u32, u32)> {
        self.view
            .as_ref()
            .map(|view| (view.config.width, view.config.height))
    }

    fn apply(&mut self, action: Action) {
        let stage = &mut self.runner.stage;
        match action {
            Action::TogglePause => self.paused = !self.paused,
            Action::Step => self.step(),
            Action::SetPlaying(path, playing) => {
                if let Some(clip) = stage.clip_mut(&path) {
                    clip.playing = playing;
                }
            }
            Action::SetVisible(path, visible) => {
                if let Some(child) = stage.root.child_mut(&path) {
                    child.visible = visible;
                }
            }
            Action::Goto(path, frame) => {
                stage.goto_clip(&path, frame, &self.runner.library);
                // Hold it there, or it would play straight on.
                if let Some(clip) = stage.clip_mut(&path) {
                    clip.playing = false;
                }
                self.runner.settle();
            }
        }
    }

    /// Plays one frame.
    fn step(&mut self) {
        if let Some(view) = &mut self.view {
            self.runner.tick(&mut view.renderer);
        }
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let elapsed = now - self.last_redraw;
        self.last_redraw = now;
        self.frame_time += (elapsed.as_secs_f32() - self.frame_time) * 0.1;
        let frame = Duration::from_secs_f64(1.0 / self.runner.library.manifest.stage.frame_rate);
        if !self.paused {
            // After a long stall, skip ahead instead of replaying it all.
            self.owed = (self.owed + elapsed).min(frame * 5);
            while self.owed >= frame {
                self.step();
                self.owed -= frame;
            }
        }
        for note in self.runner.take_notes() {
            self.inspector.note(note);
        }

        let Some((width, height)) = self.size() else {
            return;
        };
        let (base, scissor) = self.layout(width, height);
        let library = &self.runner.library;
        let commands = self.runner.stage.commands(base, library);
        let background = library
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
            library,
            &commands,
            &target,
            (width, height),
            background,
            Some(scissor),
        );

        // The inspector goes on top, in a pass of its own.
        let info = Info {
            stage: &self.runner.stage,
            library,
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
        let cursor = if self.runner.stage.pointer.on_button() {
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

impl ApplicationHandler for App {
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
                    .options
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
