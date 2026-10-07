//! Draws a frame's commands with wgpu.
//!
//! Meshes are built the first time a symbol is drawn and kept. Masks use the
//! stencil buffer: a mask's outline raises the stencil value of the pixels it
//! covers, and masked content only draws where the value matches.
//!
//! A blurred object is drawn to a layer of its own, a texture just big enough
//! to hold it. The layer is blurred and then drawn into its parent like any
//! other picture.

use std::collections::HashMap;

use anyhow::{Context, Result};
use bb_format::{SymbolId, SymbolInfo};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::display::{Bounds, Command};
use crate::input::Geometry;
use crate::library::Library;
use crate::math::{ColorTransform, Matrix};
use crate::tess::{Draw, Mesh, Paint, Spread, Tessellator, Vertex};

const SAMPLES: u32 = 4;
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
/// Uniform data for one draw or one layer is placed at multiples of this, the
/// largest alignment any device asks for.
const SLOT: usize = 256;
/// Layer sizes are rounded up to a multiple of this, so that an object which
/// changes size a little from frame to frame can reuse its textures.
const LAYER_STEP: u32 = 64;
const MAX_LAYER: u32 = 4096;
/// A layer's textures are dropped after going unused for this many frames.
const LAYER_LIFETIME: u64 = 300;
/// How many meshes of changing text to keep before clearing them out.
const MAX_FIELD_MESHES: usize = 512;
/// The widest blur the shader will sample, in pixels.
const MAX_BLUR: f32 = 127.0;

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
    // For a blur: xy is one pixel along the blur, in texture coordinates.
    paint_abcd: vec4<f32>,
    // xy: translation. z: the paint's row in the ramp texture.
    // For a blur: x is the width of the box, in pixels.
    paint_t: vec4<f32>,
    // x: 0 solid, 1 linear, 2 radial, 3 image, 4 layer.
    // y: 0 pad, 1 reflect, 2 repeat.
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
    let centre = vec2<f32>(
        m.x * position.x + m.z * position.y + item.world_t.x,
        m.y * position.x + m.w * position.y + item.world_t.y,
    );

    // A stroke is drawn as if by a round pen on the screen: its width is the
    // same in every direction, however its shape is stretched or skewed, and
    // never less than one pixel. So the vertex moves out from the centre
    // line on the screen, not in the shape's own coordinates. A direction
    // that is square to a line stays square to it when turned by the
    // inverse transpose of the transform, which is what this is.
    var pixel = centre;
    let turned = vec2<f32>(
        m.w * normal.x - m.y * normal.y,
        m.x * normal.y - m.z * normal.x,
    );
    if dot(turned, turned) > 0.0 {
        let scale = sqrt(abs(m.x * m.w - m.y * m.z));
        let half = max(half_width * scale, globals.limits.x);
        pixel = centre + normalize(turned) * length(normal) * half;
    }
    // Where the vertex is in the shape, for working out its paint.
    let local = position + normal * half_width;

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
    if kind == 4u {
        // A finished layer: already transformed and multiplied by alpha.
        return textureSampleLevel(paint_texture, paint_sampler, in.paint, 0.0);
    }
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

struct BlurOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One triangle that covers the whole target.
@vertex
fn vs_blur(@builtin(vertex_index) index: u32) -> BlurOut {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: BlurOut;
    out.clip = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return out;
}

// Flash's blur: the average of a row of pixels `width` wide. A width that is
// not a whole odd number gives the two end pixels part weight.
@fragment
fn fs_blur(in: BlurOut) -> @location(0) vec4<f32> {
    let radius = (item.paint_t.x - 1.0) / 2.0;
    let reach = i32(ceil(radius));
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var i = -reach; i <= reach; i++) {
        let weight = clamp(radius + 0.5 - abs(f32(i)), 0.0, 1.0);
        let uv = in.uv + item.paint_abcd.xy * f32(i);
        sum += textureSampleLevel(paint_texture, paint_sampler, uv, 0.0) * weight;
        total += weight;
    }
    return sum / max(total, 1e-6);
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
    /// A text field saying something the game has set. The number stands
    /// for what it says.
    Field(SymbolId, u64),
    /// The square from (0, 0) to (1, 1), for drawing a layer.
    Quad,
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
    Image {
        slot: usize,
        smooth: bool,
    },
    /// The finished picture of a layer.
    Layer(usize),
}

struct Step {
    mode: Mode,
    stencil: u32,
    key: MeshKey,
    draw: usize,
    item: usize,
    texture: Texture,
}

/// One picture being built: the frame itself, or a blurred object.
struct Layer {
    steps: Vec<Step>,
    /// Where the layer sits in the frame, in pixels.
    origin: (i32, i32),
    size: (u32, u32),
    /// One entry per blur to run, in order: the item holding its settings.
    blurs: Vec<usize>,
    /// The textures it draws to. `None` for the frame, and for a layer that
    /// is entirely out of view.
    target: Option<usize>,
}

/// Mask state, which starts afresh inside each layer.
#[derive(Clone, Copy)]
struct Masking {
    /// How many masks are in force.
    depth: u32,
    mode: Mode,
    stencil: u32,
}

impl Masking {
    const NONE: Masking = Masking {
        depth: 0,
        mode: Mode::Content,
        stencil: 0,
    };
}

/// The textures a layer draws to and is blurred between.
struct LayerTarget {
    size: (u32, u32),
    /// Multisampled, resolved into `a`.
    color: wgpu::TextureView,
    stencil: wgpu::TextureView,
    a: wgpu::TextureView,
    a_bind: wgpu::BindGroup,
    b: wgpu::TextureView,
    b_bind: wgpu::BindGroup,
    last_used: u64,
}

struct Targets {
    size: (u32, u32),
    color: wgpu::TextureView,
    stencil: wgpu::TextureView,
}

/// What the last frame took to draw.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub draws: usize,
    pub layers: usize,
    pub meshes: usize,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    pipelines: [wgpu::RenderPipeline; 3],
    blur_pipeline: wgpu::RenderPipeline,
    globals_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    globals_capacity: usize,
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
    layer_targets: Vec<LayerTarget>,
    frame: u64,
    /// The least width a stroke is drawn at, in pixels of the target.
    pub min_stroke: f32,
    pub stats: Stats,
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

        let uniform_entry = |size: usize| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(size as u64),
            },
            count: None,
        };
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[uniform_entry(size_of::<Globals>())],
        });
        let item_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("item"),
            entries: &[uniform_entry(size_of::<Item>())],
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
        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blur"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_blur"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blur"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
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

        let globals_capacity = 16;
        let (globals, globals_bind) =
            slot_buffer::<Globals>(&device, &globals_layout, globals_capacity);
        let items_capacity = 1024;
        let (items, items_bind) = slot_buffer::<Item>(&device, &item_layout, items_capacity);
        let ramps_capacity = 64;
        let (ramps_texture, ramps_bind) =
            ramp_texture(&device, &texture_layout, &ramp_sampler, ramps_capacity);

        Renderer {
            device,
            queue,
            format,
            pipelines,
            blur_pipeline,
            globals_layout,
            globals,
            globals_bind,
            globals_capacity,
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
            layer_targets: Vec::new(),
            frame: 0,
            min_stroke: 1.0,
            stats: Stats::default(),
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
        self.frame += 1;
        // Text that changes often, such as a score, leaves a mesh behind for
        // every value it has shown. Clear them out now and then; the ones
        // still wanted are rebuilt as they are drawn.
        let fields = self
            .meshes
            .keys()
            .filter(|key| matches!(key, MeshKey::Field(..)))
            .count();
        if fields > MAX_FIELD_MESHES {
            self.meshes
                .retain(|key, _| !matches!(key, MeshKey::Field(..)));
        }
        let (mut layers, items) = self.prepare(library, commands, size);
        self.upload(&mut layers, &items, size);
        self.stats = Stats {
            draws: layers.iter().map(|layer| layer.steps.len()).sum(),
            layers: layers.len() - 1,
            meshes: self.meshes.len(),
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        // A layer is made after the layer it belongs to, so going backwards
        // finishes every layer before the one that draws it.
        for (index, layer) in layers.iter().enumerate().rev() {
            let frame = self.targets.as_ref().expect("made by upload");
            let (color, resolve, stencil, clear) = match (index, layer.target) {
                (0, _) => {
                    let [r, g, b, a] = background;
                    let clear = wgpu::Color { r, g, b, a };
                    (&frame.color, target, &frame.stencil, clear)
                }
                (_, Some(slot)) => {
                    let layer = &self.layer_targets[slot];
                    let clear = wgpu::Color::TRANSPARENT;
                    (&layer.color, &layer.a, &layer.stencil, clear)
                }
                (_, None) => continue,
            };
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("layer"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: color,
                        depth_slice: None,
                        resolve_target: Some(resolve),
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear),
                            store: wgpu::StoreOp::Discard,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: stencil,
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
                pass.set_bind_group(0, &self.globals_bind, &[(index * SLOT) as u32]);
                if index == 0
                    && let Some([x, y, width, height]) = scissor
                {
                    pass.set_scissor_rect(x, y, width, height);
                }
                self.draw_steps(&mut pass, &layers, &layer.steps);
            }

            // Each blur reads one of the layer's two textures and writes the
            // other.
            let Some(slot) = layer.target else { continue };
            let textures = &self.layer_targets[slot];
            for (round, &item) in layer.blurs.iter().enumerate() {
                let (source, destination) = if round.is_multiple_of(2) {
                    (&textures.a_bind, &textures.b)
                } else {
                    (&textures.b_bind, &textures.a)
                };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("blur"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: destination,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.blur_pipeline);
                pass.set_bind_group(0, &self.globals_bind, &[(index * SLOT) as u32]);
                pass.set_bind_group(1, &self.items_bind, &[(item * SLOT) as u32]);
                pass.set_bind_group(2, source, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);

        let frame = self.frame;
        self.layer_targets
            .retain(|target| frame - target.last_used < LAYER_LIFETIME);
    }

    fn draw_steps(&self, pass: &mut wgpu::RenderPass, layers: &[Layer], steps: &[Step]) {
        let mut mode = None;
        let mut key = None;
        let mut texture = None;
        for step in steps {
            let Some(Some(mesh)) = self.meshes.get(&step.key) else {
                continue;
            };
            let bind = match step.texture {
                Texture::Ramps => &self.ramps_bind,
                Texture::Layer(index) => {
                    let layer = &layers[index];
                    let Some(slot) = layer.target else { continue };
                    let textures = &self.layer_targets[slot];
                    // An odd number of blurs leaves the picture in the
                    // second texture.
                    if layer.blurs.len().is_multiple_of(2) {
                        &textures.a_bind
                    } else {
                        &textures.b_bind
                    }
                }
                image => &self.image_binds[&image],
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
                pass.set_bind_group(2, bind, &[]);
                texture = Some(step.texture);
            }
            pass.set_stencil_reference(step.stencil);
            pass.set_bind_group(1, &self.items_bind, &[(step.item * SLOT) as u32]);
            pass.draw_indexed(mesh.mesh.draws[step.draw].indices.clone(), 0, 0..1);
        }
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

    /// Sorts `commands` into layers of draws, building any meshes they need.
    /// The first layer is the frame itself.
    fn prepare(
        &mut self,
        library: &Library,
        commands: &[Command],
        size: (u32, u32),
    ) -> (Vec<Layer>, Vec<u8>) {
        let mut items: Vec<u8> = Vec::new();
        let mut push_item = |item: Item| {
            let slot = items.len() / SLOT;
            items.extend_from_slice(bytemuck::bytes_of(&item));
            items.resize((slot + 1) * SLOT, 0);
            slot
        };
        let mut layers = vec![Layer {
            steps: Vec::new(),
            origin: (0, 0),
            size,
            blurs: Vec::new(),
            target: None,
        }];
        // The layers being drawn into, outermost first, each with the mask
        // state to go back to when it ends.
        let mut open: Vec<(usize, Masking)> = Vec::new();
        let mut current = 0;
        let mut masking = Masking::NONE;

        for command in commands {
            match command {
                Command::PushMask => {
                    masking.mode = Mode::MaskWrite;
                    masking.stencil = masking.depth;
                    masking.depth += 1;
                }
                Command::ActivateMask => {
                    masking.mode = Mode::Content;
                    masking.stencil = masking.depth;
                }
                Command::DeactivateMask => {
                    masking.mode = Mode::MaskClear;
                    masking.stencil = masking.depth;
                }
                Command::PopMask => {
                    masking.depth = masking.depth.saturating_sub(1);
                    masking.mode = Mode::Content;
                    masking.stencil = masking.depth;
                }
                Command::BeginBlur {
                    blur_x,
                    blur_y,
                    passes,
                    bounds,
                } => {
                    let (blur_x, blur_y) = (blur_x.min(MAX_BLUR), blur_y.min(MAX_BLUR));
                    // Each pass spreads the picture by half the box's width.
                    let reach = (blur_x.max(blur_y) / 2.0 * f32::from(*passes)).ceil() + 1.0;
                    let parent = &layers[current];
                    let area = layer_area(*bounds, reach, parent.origin, parent.size);
                    let (origin, size) = area.unwrap_or(((0, 0), (0, 0)));
                    let mut blurs = Vec::new();
                    if area.is_some() {
                        for _ in 0..*passes {
                            for (width, step) in [
                                (blur_x, [1.0 / size.0 as f32, 0.0]),
                                (blur_y, [0.0, 1.0 / size.1 as f32]),
                            ] {
                                if width > 1.0 {
                                    blurs.push(push_item(Item {
                                        paint_abcd: [step[0], step[1], 0.0, 0.0],
                                        paint_t: [width, 0.0, 0.0, 0.0],
                                        ..Item::zeroed()
                                    }));
                                }
                            }
                        }
                    }
                    open.push((current, masking));
                    current = layers.len();
                    masking = Masking::NONE;
                    layers.push(Layer {
                        steps: Vec::new(),
                        origin,
                        size,
                        blurs,
                        target: None,
                    });
                }
                Command::EndBlur => {
                    let Some((parent, parent_masking)) = open.pop() else {
                        continue;
                    };
                    let finished = current;
                    (current, masking) = (parent, parent_masking);
                    let layer = &layers[finished];
                    if layer.size == (0, 0) {
                        continue;
                    }
                    self.ensure_mesh(library, MeshKey::Quad, None);
                    // The unit square, stretched over the layer's place.
                    let item = push_item(Item {
                        world_abcd: [layer.size.0 as f32, 0.0, 0.0, layer.size.1 as f32],
                        world_t: [layer.origin.0 as f32, layer.origin.1 as f32, 0.0, 0.0],
                        paint_abcd: [1.0, 0.0, 0.0, 1.0],
                        kind: [4, 0, 0, 0],
                        ..Item::zeroed()
                    });
                    layers[current].steps.push(Step {
                        mode: masking.mode,
                        stencil: masking.stencil,
                        key: MeshKey::Quad,
                        draw: 0,
                        item,
                        texture: Texture::Layer(finished),
                    });
                }
                Command::Draw {
                    symbol,
                    ratio,
                    matrix,
                    color,
                    text,
                } => {
                    if layers[current].size == (0, 0) {
                        continue;
                    }
                    let Some(key) = mesh_key(library, *symbol, *ratio, text.as_deref()) else {
                        continue;
                    };
                    self.ensure_mesh(library, key, text.as_deref());
                    let Some(Some(mesh)) = self.meshes.get(&key) else {
                        continue;
                    };
                    for (index, draw) in mesh.mesh.draws.iter().enumerate() {
                        let (data, texture) = item_for(&draw.paint, *matrix, *color);
                        let item = push_item(data);
                        layers[current].steps.push(Step {
                            mode: masking.mode,
                            stencil: masking.stencil,
                            key,
                            draw: index,
                            item,
                            texture,
                        });
                    }
                }
            }
        }
        (layers, items)
    }

    /// Builds the mesh for `key` if it is not there yet. `text` is what a
    /// [`MeshKey::Field`] says.
    fn ensure_mesh(&mut self, library: &Library, key: MeshKey, text: Option<&str>) {
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
                (None, Some(field)) => self.tessellator.edit_text(field, None, library),
                (None, None) => Ok(Mesh::default()),
            },
            MeshKey::Field(id, _) => match library.edit_texts.get(&id) {
                Some(field) => self.tessellator.edit_text(field, text, library),
                None => Ok(Mesh::default()),
            },
            MeshKey::Quad => {
                let corner = |x: f32, y: f32| Vertex {
                    position: [x, y],
                    normal: [0.0, 0.0],
                    half_width: 0.0,
                    color: [255; 4],
                };
                Ok(Mesh {
                    vertices: vec![
                        corner(0.0, 0.0),
                        corner(1.0, 0.0),
                        corner(1.0, 1.0),
                        corner(0.0, 1.0),
                    ],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    draws: vec![Draw {
                        indices: 0..6,
                        paint: Paint::Solid,
                    }],
                })
            }
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

    /// Sends this frame's uniforms, and any new ramps and images, to the GPU,
    /// and gives every layer textures to draw to.
    fn upload(&mut self, layers: &mut [Layer], items: &[u8], size: (u32, u32)) {
        if layers.len() > self.globals_capacity {
            self.globals_capacity = layers.len().next_power_of_two();
            (self.globals, self.globals_bind) =
                slot_buffer::<Globals>(&self.device, &self.globals_layout, self.globals_capacity);
        }
        let mut globals = vec![0u8; layers.len() * SLOT];
        for (index, layer) in layers.iter().enumerate() {
            let (width, height) = (layer.size.0.max(1) as f32, layer.size.1.max(1) as f32);
            let (x, y) = (layer.origin.0 as f32, layer.origin.1 as f32);
            let data = Globals {
                view: [
                    2.0 / width,
                    -2.0 / height,
                    -1.0 - 2.0 * x / width,
                    1.0 + 2.0 * y / height,
                ],
                limits: [self.min_stroke / 2.0, 0.0, 0.0, 0.0],
            };
            let at = index * SLOT;
            globals[at..at + size_of::<Globals>()].copy_from_slice(bytemuck::bytes_of(&data));
        }
        self.queue.write_buffer(&self.globals, 0, &globals);

        let needed = items.len() / SLOT;
        if needed > self.items_capacity {
            self.items_capacity = needed.next_power_of_two();
            (self.items, self.items_bind) =
                slot_buffer::<Item>(&self.device, &self.item_layout, self.items_capacity);
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
            self.targets = Some(Targets {
                size,
                color: attachment(&self.device, size, self.format, SAMPLES),
                stencil: attachment(&self.device, size, STENCIL_FORMAT, SAMPLES),
            });
        }

        // Give each layer a set of textures of its size that no other layer
        // has taken this frame.
        for layer in layers.iter_mut().skip(1) {
            if layer.size == (0, 0) {
                continue;
            }
            let free = self
                .layer_targets
                .iter()
                .position(|target| target.size == layer.size && target.last_used != self.frame);
            let slot = match free {
                Some(slot) => slot,
                None => {
                    self.layer_targets.push(self.new_layer_target(layer.size));
                    self.layer_targets.len() - 1
                }
            };
            self.layer_targets[slot].last_used = self.frame;
            layer.target = Some(slot);
        }
    }

    fn new_layer_target(&self, size: (u32, u32)) -> LayerTarget {
        let sampled = || {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("layer"),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let (a, b) = (sampled(), sampled());
        let bind = |view: &wgpu::TextureView| {
            texture_bind(&self.device, &self.texture_layout, view, &self.ramp_sampler)
        };
        LayerTarget {
            size,
            color: attachment(&self.device, size, self.format, SAMPLES),
            stencil: attachment(&self.device, size, STENCIL_FORMAT, SAMPLES),
            a_bind: bind(&a),
            b_bind: bind(&b),
            a,
            b,
            last_used: 0,
        }
    }
}

impl Geometry for Renderer {
    fn contains(
        &mut self,
        library: &Library,
        symbol: SymbolId,
        ratio: u16,
        x: f32,
        y: f32,
    ) -> bool {
        let Some(key) = mesh_key(library, symbol, ratio, None) else {
            return false;
        };
        self.ensure_mesh(library, key, None);
        match self.meshes.get(&key) {
            Some(Some(mesh)) => mesh.mesh.contains(x, y),
            _ => false,
        }
    }
}

/// Which mesh draws `symbol`, if it is something with a mesh.
fn mesh_key(
    library: &Library,
    symbol: SymbolId,
    ratio: u16,
    text: Option<&str>,
) -> Option<MeshKey> {
    match (&library.manifest.symbols.get(&symbol)?.info, text) {
        (SymbolInfo::Shape { .. }, _) => Some(MeshKey::Shape(symbol)),
        (SymbolInfo::MorphShape, _) => Some(MeshKey::Morph(symbol, ratio)),
        (SymbolInfo::EditText, Some(text)) => {
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(text, &mut hasher);
            Some(MeshKey::Field(symbol, std::hash::Hasher::finish(&hasher)))
        }
        (SymbolInfo::Text | SymbolInfo::EditText, None) => Some(MeshKey::Text(symbol)),
        _ => None,
    }
}

/// Where a blurred object's layer goes: `bounds` grown by `reach`, cut down
/// to the part that can affect the parent layer, on whole pixels. Returns the
/// top-left corner and the size, or `None` if nothing of it can be seen.
fn layer_area(
    bounds: Bounds,
    reach: f32,
    parent_origin: (i32, i32),
    parent_size: (u32, u32),
) -> Option<((i32, i32), (u32, u32))> {
    let reach_px = reach as i32;
    let left = ((bounds[0] - reach).floor() as i32).max(parent_origin.0 - reach_px);
    let top = ((bounds[1] - reach).floor() as i32).max(parent_origin.1 - reach_px);
    let right =
        ((bounds[2] + reach).ceil() as i32).min(parent_origin.0 + parent_size.0 as i32 + reach_px);
    let bottom =
        ((bounds[3] + reach).ceil() as i32).min(parent_origin.1 + parent_size.1 as i32 + reach_px);
    if right <= left || bottom <= top {
        return None;
    }
    let round = |length: i32| (length as u32).next_multiple_of(LAYER_STEP).min(MAX_LAYER);
    Some(((left, top), (round(right - left), round(bottom - top))))
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

/// A uniform buffer holding `capacity` values of `T`, one per slot, bound so
/// that a draw picks its slot by offset.
fn slot_buffer<T>(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    capacity: usize,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniforms"),
        size: (capacity * SLOT) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("uniforms"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size_of::<T>() as u64),
            }),
        }],
    });
    (buffer, bind)
}

/// A texture to draw into and nothing else.
fn attachment(
    device: &wgpu::Device,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    samples: u32,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("attachment"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layer_covers_its_object_and_the_reach_of_the_blur() {
        let area = layer_area([100.0, 100.0, 150.0, 140.0], 4.0, (0, 0), (590, 400));
        // 96 to 154 across and 96 to 144 down, each rounded up to 64.
        assert_eq!(area, Some(((96, 96), (64, 64))));
    }

    #[test]
    fn a_layer_is_cut_down_to_what_can_reach_the_view() {
        let area = layer_area([-500.0, 10.0, 30.0, 40.0], 4.0, (0, 0), (590, 400));
        assert_eq!(area, Some(((-4, 6), (64, 64))));
    }

    #[test]
    fn an_object_wholly_out_of_view_gets_no_layer() {
        assert_eq!(
            layer_area([700.0, 10.0, 800.0, 40.0], 4.0, (0, 0), (590, 400)),
            None
        );
    }
}
