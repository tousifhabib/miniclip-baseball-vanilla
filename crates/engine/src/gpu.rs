//! Draws a frame's commands with wgpu.
//!
//! Meshes are built the first time a symbol is drawn and kept. Masks use the
//! stencil buffer: a mask's outline raises the stencil value of the pixels it
//! covers, and masked content only draws where the value matches.

use std::collections::HashMap;

use anyhow::{Context, Result};
use bb_format::{SymbolId, SymbolInfo};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::display::Command;
use crate::library::Library;
use crate::math::{ColorTransform, Matrix};
use crate::tess::{Mesh, Paint, Spread, Tessellator, Vertex};

const SAMPLES: u32 = 4;
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
/// Uniform data for one draw is placed at multiples of this, the largest
/// alignment any device asks for.
const ITEM_STRIDE: usize = 256;

const SHADER: &str = r"
struct Globals {
    // Pixels to clip space: clip = pixel * view.xy + view.zw.
    view: vec4<f32>,
    // x: the least half-width a stroke may have, in pixels.
    limits: vec4<f32>,
};

struct Item {
    world_abcd: vec4<f32>,
    world_t: vec4<f32>,
    color_mult: vec4<f32>,
    color_add: vec4<f32>,
    paint_abcd: vec4<f32>,
    // xy: translation. z: the paint's row in the ramp texture.
    paint_t: vec4<f32>,
    // x: 0 solid, 1 linear, 2 radial, 3 image. y: 0 pad, 1 reflect, 2 repeat.
    kind: vec4<u32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> item: Item;
@group(2) @binding(0) var paint_texture: texture_2d<f32>;
@group(2) @binding(1) var paint_sampler: sampler;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) paint: vec2<f32>,
};

@vertex
fn vs(
    @location(0) position: vec2<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) half_width: f32,
    @location(3) color: vec4<f32>,
) -> VertexOut {
    let m = item.world_abcd;
    // A stroke is never thinner than one pixel on screen, however far its
    // shape is scaled down. Fills have no normal, so this leaves them alone.
    let scale = sqrt(abs(m.x * m.w - m.y * m.z));
    let half = max(half_width, globals.limits.x / max(scale, 1e-6));
    let local = position + normal * half;

    let pixel = vec2<f32>(
        m.x * local.x + m.z * local.y + item.world_t.x,
        m.y * local.x + m.w * local.y + item.world_t.y,
    );
    let p = item.paint_abcd;
    var out: VertexOut;
    out.clip = vec4<f32>(pixel * globals.view.xy + globals.view.zw, 0.0, 1.0);
    out.color = color;
    out.paint = vec2<f32>(
        p.x * local.x + p.z * local.y + item.paint_t.x,
        p.y * local.x + p.w * local.y + item.paint_t.y,
    );
    return out;
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    var c = in.color;
    let kind = item.kind.x;
    if kind == 3u {
        // Images are stored multiplied by alpha, so that smoothing does not
        // drag colour in from clear pixels.
        let texel = textureSampleLevel(paint_texture, paint_sampler, in.paint, 0.0);
        let rgb = select(vec3<f32>(0.0), texel.rgb / texel.a, texel.a > 0.0);
        c = vec4<f32>(rgb, texel.a * in.color.a);
    } else if kind != 0u {
        var t = in.paint.x;
        if kind == 2u {
            t = length(in.paint);
        }
        if item.kind.y == 1u {
            t = 1.0 - abs(t - 2.0 * floor(t / 2.0) - 1.0);
        } else if item.kind.y == 2u {
            t = fract(t);
        }
        t = clamp(t, 0.0, 1.0);
        let rows = f32(textureDimensions(paint_texture).y);
        let uv = vec2<f32>(t * (255.0 / 256.0) + 0.5 / 256.0, (item.paint_t.z + 0.5) / rows);
        c = textureSampleLevel(paint_texture, paint_sampler, uv, 0.0);
    }
    c = clamp(c * item.color_mult + item.color_add, vec4<f32>(0.0), vec4<f32>(1.0));
    return vec4<f32>(c.rgb * c.a, c.a);
}
";

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view: [f32; 4],
    limits: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Item {
    world_abcd: [f32; 4],
    world_t: [f32; 4],
    color_mult: [f32; 4],
    color_add: [f32; 4],
    paint_abcd: [f32; 4],
    paint_t: [f32; 4],
    kind: [u32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MeshKey {
    Shape(SymbolId),
    Morph(SymbolId, u16),
    Text(SymbolId),
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    mesh: Mesh,
}

/// How a draw treats the stencil buffer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Draws colour where the stencil matches.
    Content = 0,
    /// Raises the stencil where it matches, drawing no colour.
    MaskWrite = 1,
    /// Lowers the stencil where it matches, drawing no colour.
    MaskClear = 2,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Texture {
    Ramps,
    Image { slot: usize, smooth: bool },
}

struct Step {
    mode: Mode,
    stencil: u32,
    key: MeshKey,
    draw: usize,
    item: usize,
    texture: Texture,
}

struct Targets {
    size: (u32, u32),
    color: wgpu::TextureView,
    stencil: wgpu::TextureView,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    pipelines: [wgpu::RenderPipeline; 3],
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    item_layout: wgpu::BindGroupLayout,
    items: wgpu::Buffer,
    items_bind: wgpu::BindGroup,
    items_capacity: usize,
    texture_layout: wgpu::BindGroupLayout,
    ramp_sampler: wgpu::Sampler,
    smooth_sampler: wgpu::Sampler,
    crisp_sampler: wgpu::Sampler,
    tessellator: Tessellator,
    meshes: HashMap<MeshKey, Option<GpuMesh>>,
    ramps_bind: wgpu::BindGroup,
    ramps_texture: wgpu::Texture,
    ramps_capacity: usize,
    ramps_uploaded: usize,
    image_views: Vec<wgpu::TextureView>,
    image_binds: HashMap<Texture, wgpu::BindGroup>,
    targets: Option<Targets>,
    /// The least width a stroke is drawn at, in pixels of the target.
    pub min_stroke: f32,
    /// Symbols that could not be drawn, each reported once.
    pub problems: Vec<String>,
}

impl Renderer {
    /// Opens the default graphics device, with no window.
    pub fn headless() -> Result<Renderer> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .context("finding a graphics adapter")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .context("opening the graphics device")?;
        Ok(Renderer::new(
            device,
            queue,
            wgpu::TextureFormat::Rgba8Unorm,
        ))
    }

    /// `format` is the format of the textures this will draw to. It should not
    /// be an sRGB format: Flash blends colours as stored, without converting.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Renderer {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shapes"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let uniform_entry = |dynamic: bool, size: usize| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(size as u64),
            },
            count: None,
        };
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[uniform_entry(false, size_of::<Globals>())],
        });
        let item_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("item"),
            entries: &[uniform_entry(true, size_of::<Item>())],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paint texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shapes"),
            bind_group_layouts: &[
                Some(&globals_layout),
                Some(&item_layout),
                Some(&texture_layout),
            ],
            immediate_size: 0,
        });

        let pipeline = |mode: Mode| {
            let (pass_op, write_mask) = match mode {
                Mode::Content => (wgpu::StencilOperation::Keep, wgpu::ColorWrites::ALL),
                Mode::MaskWrite => (
                    wgpu::StencilOperation::IncrementClamp,
                    wgpu::ColorWrites::empty(),
                ),
                Mode::MaskClear => (
                    wgpu::StencilOperation::DecrementClamp,
                    wgpu::ColorWrites::empty(),
                ),
            };
            let face = wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Equal,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op,
            };
            // Colours are multiplied by alpha before blending.
            let blend = wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("shapes"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x2,
                            1 => Float32x2,
                            2 => Float32,
                            3 => Unorm8x4,
                        ],
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: STENCIL_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: wgpu::StencilState {
                        front: face,
                        back: face,
                        read_mask: 0xff,
                        write_mask: 0xff,
                    },
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLES,
                    ..wgpu::MultisampleState::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState {
                            color: blend,
                            alpha: blend,
                        }),
                        write_mask,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = [
            pipeline(Mode::Content),
            pipeline(Mode::MaskWrite),
            pipeline(Mode::MaskClear),
        ];

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let sampler = |filter: wgpu::FilterMode, address: wgpu::AddressMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: address,
                address_mode_v: address,
                mag_filter: filter,
                min_filter: filter,
                ..wgpu::SamplerDescriptor::default()
            })
        };
        let ramp_sampler = sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::ClampToEdge);
        let smooth_sampler = sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::Repeat);
        let crisp_sampler = sampler(wgpu::FilterMode::Nearest, wgpu::AddressMode::Repeat);

        let items_capacity = 1024;
        let (items, items_bind) = item_buffer(&device, &item_layout, items_capacity);
        let ramps_capacity = 64;
        let (ramps_texture, ramps_bind) =
            ramp_texture(&device, &texture_layout, &ramp_sampler, ramps_capacity);

        Renderer {
            device,
            queue,
            format,
            pipelines,
            globals,
            globals_bind,
            item_layout,
            items,
            items_bind,
            items_capacity,
            texture_layout,
            ramp_sampler,
            smooth_sampler,
            crisp_sampler,
            tessellator: Tessellator::default(),
            meshes: HashMap::new(),
            ramps_bind,
            ramps_texture,
            ramps_capacity,
            ramps_uploaded: 0,
            image_views: Vec::new(),
            image_binds: HashMap::new(),
            targets: None,
            min_stroke: 1.0,
            problems: Vec::new(),
        }
    }

    /// Draws `commands` into `target`, a texture of `size` pixels in the
    /// format this renderer was made for. With `scissor`, only the pixels
    /// inside that rectangle (x, y, width, height) are drawn.
    pub fn render(
        &mut self,
        library: &Library,
        commands: &[Command],
        target: &wgpu::TextureView,
        size: (u32, u32),
        background: [f64; 4],
        scissor: Option<[u32; 4]>,
    ) {
        let (steps, items) = self.prepare(library, commands);
        self.upload(&items, size);

        let targets = self.targets.as_ref().expect("made by upload");
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let [r, g, b, a] = background;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.color,
                    depth_slice: None,
                    resolve_target: Some(target),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.stencil,
                    depth_ops: None,
                    stencil_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0),
                        store: wgpu::StoreOp::Discard,
                    }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind, &[]);
            if let Some([x, y, width, height]) = scissor {
                pass.set_scissor_rect(x, y, width, height);
            }

            let mut mode = None;
            let mut key = None;
            let mut texture = None;
            for step in &steps {
                let Some(Some(mesh)) = self.meshes.get(&step.key) else {
                    continue;
                };
                if mode != Some(step.mode) {
                    pass.set_pipeline(&self.pipelines[step.mode as usize]);
                    mode = Some(step.mode);
                }
                if key != Some(step.key) {
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    key = Some(step.key);
                }
                if texture != Some(step.texture) {
                    let bind = match step.texture {
                        Texture::Ramps => &self.ramps_bind,
                        image => &self.image_binds[&image],
                    };
                    pass.set_bind_group(2, bind, &[]);
                    texture = Some(step.texture);
                }
                pass.set_stencil_reference(step.stencil);
                pass.set_bind_group(1, &self.items_bind, &[(step.item * ITEM_STRIDE) as u32]);
                pass.draw_indexed(mesh.mesh.draws[step.draw].indices.clone(), 0, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    /// Draws `commands` to a new image.
    pub fn capture(
        &mut self,
        library: &Library,
        commands: &[Command],
        size: (u32, u32),
        background: [f64; 4],
    ) -> Result<image::RgbaImage> {
        let (width, height) = size;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.render(library, commands, &view, size, background, None);

        // Rows in a copy must be a multiple of 256 bytes long.
        let row = (width as usize * 4).next_multiple_of(256);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture"),
            size: (row * height as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row as u32),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        self.queue.submit([encoder.finish()]);

        buffer.map_async(wgpu::MapMode::Read, .., |result| {
            result.expect("mapping the capture buffer");
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("waiting for the frame to finish")?;
        let data = buffer
            .get_mapped_range(..)
            .context("reading the capture buffer")?;
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for line in data.chunks_exact(row) {
            pixels.extend_from_slice(&line[..width as usize * 4]);
        }
        image::RgbaImage::from_raw(width, height, pixels).context("building the image")
    }

    /// Works out the draws for `commands`, building any meshes they need.
    fn prepare(&mut self, library: &Library, commands: &[Command]) -> (Vec<Step>, Vec<u8>) {
        let mut steps = Vec::new();
        let mut items: Vec<u8> = Vec::new();
        // How many masks are in force, and how the next draws use them.
        let mut masks = 0u32;
        let mut mode = Mode::Content;
        let mut stencil = 0u32;

        for command in commands {
            match command {
                Command::PushMask => {
                    mode = Mode::MaskWrite;
                    stencil = masks;
                    masks += 1;
                }
                Command::ActivateMask => {
                    mode = Mode::Content;
                    stencil = masks;
                }
                Command::DeactivateMask => {
                    mode = Mode::MaskClear;
                    stencil = masks;
                }
                Command::PopMask => {
                    masks = masks.saturating_sub(1);
                    mode = Mode::Content;
                    stencil = masks;
                }
                Command::Draw {
                    symbol,
                    ratio,
                    matrix,
                    color,
                } => {
                    let Some(key) = mesh_key(library, *symbol, *ratio) else {
                        continue;
                    };
                    self.ensure_mesh(library, key);
                    let Some(Some(mesh)) = self.meshes.get(&key) else {
                        continue;
                    };
                    for (index, draw) in mesh.mesh.draws.iter().enumerate() {
                        let item = items.len() / ITEM_STRIDE;
                        let (data, texture) = item_for(&draw.paint, *matrix, *color);
                        items.extend_from_slice(bytemuck::bytes_of(&data));
                        items.resize((item + 1) * ITEM_STRIDE, 0);
                        steps.push(Step {
                            mode,
                            stencil,
                            key,
                            draw: index,
                            item,
                            texture,
                        });
                    }
                }
            }
        }
        (steps, items)
    }

    fn ensure_mesh(&mut self, library: &Library, key: MeshKey) {
        if self.meshes.contains_key(&key) {
            return;
        }
        let built = match key {
            MeshKey::Shape(id) => {
                let symbol = &library.manifest.symbols[&id];
                let origin = match symbol.info {
                    SymbolInfo::Shape { bounds } => (bounds.x_min as f32, bounds.y_min as f32),
                    _ => (0.0, 0.0),
                };
                self.tessellator
                    .svg(&library.dir.join(&symbol.file), origin)
            }
            MeshKey::Morph(id, ratio) => self.tessellator.morph(&library.morphs[&id], ratio),
            MeshKey::Text(id) => match (library.texts.get(&id), library.edit_texts.get(&id)) {
                (Some(text), _) => self.tessellator.text(text, library),
                (None, Some(text)) => self.tessellator.edit_text(text, library),
                (None, None) => Ok(Mesh::default()),
            },
        };
        let mesh = match built {
            Ok(mesh) if !mesh.indices.is_empty() => Some(GpuMesh {
                vertices: self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("vertices"),
                        contents: bytemuck::cast_slice(&mesh.vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                indices: self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("indices"),
                        contents: bytemuck::cast_slice(&mesh.indices),
                        usage: wgpu::BufferUsages::INDEX,
                    }),
                mesh,
            }),
            Ok(_) => None,
            Err(error) => {
                self.problems.push(format!("{error:#}"));
                None
            }
        };
        self.meshes.insert(key, mesh);
    }

    /// Sends this frame's uniforms, and any new ramps and images, to the GPU.
    fn upload(&mut self, items: &[u8], size: (u32, u32)) {
        let (width, height) = size;
        let globals = Globals {
            view: [2.0 / width as f32, -2.0 / height as f32, -1.0, 1.0],
            limits: [self.min_stroke / 2.0, 0.0, 0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let needed = items.len() / ITEM_STRIDE;
        if needed > self.items_capacity {
            self.items_capacity = needed.next_power_of_two();
            (self.items, self.items_bind) =
                item_buffer(&self.device, &self.item_layout, self.items_capacity);
        }
        if !items.is_empty() {
            self.queue.write_buffer(&self.items, 0, items);
        }

        let ramps = &self.tessellator.ramps;
        if ramps.len() > self.ramps_capacity {
            self.ramps_capacity = ramps.len().next_power_of_two();
            (self.ramps_texture, self.ramps_bind) = ramp_texture(
                &self.device,
                &self.texture_layout,
                &self.ramp_sampler,
                self.ramps_capacity,
            );
            self.ramps_uploaded = 0;
        }
        if ramps.len() > self.ramps_uploaded {
            let rows = &ramps[self.ramps_uploaded..];
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.ramps_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: self.ramps_uploaded as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(rows),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256 * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: 256,
                    height: rows.len() as u32,
                    depth_or_array_layers: 1,
                },
            );
            self.ramps_uploaded = ramps.len();
        }

        for image in &self.tessellator.images[self.image_views.len()..] {
            let mut pixels = image.rgba.clone();
            for pixel in pixels.as_chunks_mut::<4>().0 {
                let alpha = u32::from(pixel[3]);
                for channel in &mut pixel[..3] {
                    *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
                }
            }
            let texture = self.device.create_texture_with_data(
                &self.queue,
                &wgpu::TextureDescriptor {
                    label: Some("image"),
                    size: wgpu::Extent3d {
                        width: image.width,
                        height: image.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &pixels,
            );
            self.image_views
                .push(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        }
        for (slot, view) in self.image_views.iter().enumerate() {
            for smooth in [false, true] {
                let key = Texture::Image { slot, smooth };
                if !self.image_binds.contains_key(&key) {
                    let sampler = if smooth {
                        &self.smooth_sampler
                    } else {
                        &self.crisp_sampler
                    };
                    let bind = texture_bind(&self.device, &self.texture_layout, view, sampler);
                    self.image_binds.insert(key, bind);
                }
            }
        }

        if self
            .targets
            .as_ref()
            .is_none_or(|targets| targets.size != size)
        {
            let attachment = |format: wgpu::TextureFormat| {
                self.device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("frame"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: SAMPLES,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };
            self.targets = Some(Targets {
                size,
                color: attachment(self.format),
                stencil: attachment(STENCIL_FORMAT),
            });
        }
    }
}

/// Which mesh draws `symbol`, if it is something with a mesh.
fn mesh_key(library: &Library, symbol: SymbolId, ratio: u16) -> Option<MeshKey> {
    match library.manifest.symbols.get(&symbol)?.info {
        SymbolInfo::Shape { .. } => Some(MeshKey::Shape(symbol)),
        SymbolInfo::MorphShape => Some(MeshKey::Morph(symbol, ratio)),
        SymbolInfo::Text | SymbolInfo::EditText => Some(MeshKey::Text(symbol)),
        _ => None,
    }
}

fn item_for(paint: &Paint, world: Matrix, color: ColorTransform) -> (Item, Texture) {
    let spread_code = |spread: Spread| match spread {
        Spread::Pad => 0,
        Spread::Reflect => 1,
        Spread::Repeat => 2,
    };
    let (kind, spread, matrix, row, texture) = match *paint {
        Paint::Solid => (0, 0, Matrix::IDENTITY, 0.0, Texture::Ramps),
        Paint::Linear {
            ramp,
            matrix,
            spread,
        } => (1, spread_code(spread), matrix, ramp as f32, Texture::Ramps),
        Paint::Radial {
            ramp,
            matrix,
            spread,
        } => (2, spread_code(spread), matrix, ramp as f32, Texture::Ramps),
        Paint::Image {
            image,
            matrix,
            smooth,
        } => (
            3,
            0,
            matrix,
            0.0,
            Texture::Image {
                slot: image,
                smooth,
            },
        ),
    };
    let item = Item {
        world_abcd: [world.a, world.b, world.c, world.d],
        world_t: [world.tx, world.ty, 0.0, 0.0],
        color_mult: color.mult,
        color_add: color.add,
        paint_abcd: [matrix.a, matrix.b, matrix.c, matrix.d],
        paint_t: [matrix.tx, matrix.ty, row, 0.0],
        kind: [kind, spread, 0, 0],
    };
    (item, texture)
}

fn item_buffer(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    capacity: usize,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("items"),
        size: (capacity * ITEM_STRIDE) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("items"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size_of::<Item>() as u64),
            }),
        }],
    });
    (buffer, bind)
}

/// A texture holding one gradient ramp per row.
fn ramp_texture(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    rows: usize,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ramps"),
        size: wgpu::Extent3d {
            width: 256,
            height: rows as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = texture_bind(device, layout, &view, sampler);
    (texture, bind)
}

fn texture_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("paint texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
