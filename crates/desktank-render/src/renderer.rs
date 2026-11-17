//! GPU resources for one grid and one target format: the cell texture, the
//! palette, the uniforms and the pipeline.

use desktank_sim::World;

use crate::grid::{self, DirtyRects};
use crate::layout::Layout;
use crate::palette;
use crate::{Gpu, timer::GpuTimer};

/// Darkening of water at the floor relative to the surface.
const DEPTH_DIM: f32 = 0.5;

/// Size of the `View` uniform block in `tank.wgsl`, in bytes.
const VIEW_BYTES: u64 = 48;

/// Letterbox colour around the grid (sRGB).
const BACKDROP: [u8; 3] = [0x05, 0x0a, 0x12];

#[derive(Debug)]
pub struct Renderer {
    format: wgpu::TextureFormat,
    grid: (u32, u32),
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    view_buffer: wgpu::Buffer,
    cells: wgpu::Texture,
    /// Layout the uniforms were last written for.
    layout: Option<Layout>,
    scratch: Vec<u8>,
}

impl Renderer {
    /// Resources for `world`'s grid, drawing into `format` targets.
    pub fn new(gpu: &Gpu, world: &World, format: wgpu::TextureFormat) -> Self {
        let d = &gpu.device;
        let grid = (side(world.width()), side(world.height()));
        let cells = d.create_texture(&wgpu::TextureDescriptor {
            label: Some("cells"),
            size: wgpu::Extent3d {
                width: grid.0,
                height: grid.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let palette = create_palette(gpu, world);
        let view_buffer = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view"),
            size: VIEW_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bind_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tank"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(VIEW_BYTES),
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
            ],
        });
        let cells_view = cells.create_view(&wgpu::TextureViewDescriptor::default());
        let palette_view = palette.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tank"),
            layout: &bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&cells_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&palette_view),
                },
            ],
        });
        let pipeline = create_pipeline(d, &bind_layout, format);
        Self {
            format,
            grid,
            pipeline,
            bind_group,
            view_buffer,
            cells,
            layout: None,
            scratch: Vec::new(),
        }
    }

    /// Whether these resources fit `world` and `format`.
    pub fn matches(&self, world: &World, format: wgpu::TextureFormat) -> bool {
        self.format == format && self.grid == (side(world.width()), side(world.height()))
    }

    /// Copies the waiting dirty rectangles (or the whole grid) into the cell
    /// texture.
    pub fn upload(&mut self, gpu: &Gpu, world: &World, dirty: &mut DirtyRects) {
        let width = world.width();
        let full = desktank_sim::Rect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: world.height(),
        };
        let (cells, scratch, texture) = (world.cells(), &mut self.scratch, &self.cells);
        dirty.take(|r| {
            let r = r.unwrap_or(full);
            grid::pack(cells, width, r, scratch);
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: side(r.x0),
                        y: side(r.y0),
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                scratch,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(side(r.x1 - r.x0) * 2),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: side(r.x1 - r.x0),
                    height: side(r.y1 - r.y0),
                    depth_or_array_layers: 1,
                },
            );
        });
    }

    /// Records the draw into `target` (`size` pixels).
    pub fn encode(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        surface_row: usize,
        timer: Option<&GpuTimer>,
    ) {
        let layout = Layout::fit(self.grid, size);
        if self.layout != Some(layout) {
            #[allow(clippy::cast_precision_loss)] // pixel and cell coordinates
            let view = [
                self.grid.0 as f32,
                self.grid.1 as f32,
                layout.offset.0 as f32,
                layout.offset.1 as f32,
                layout.scale as f32,
                surface_row as f32,
                DEPTH_DIM,
                if self.format.is_srgb() { 1.0 } else { 0.0 },
            ]
            .into_iter()
            .chain(backdrop());
            let bytes: Vec<u8> = view.flat_map(f32::to_le_bytes).collect();
            gpu.queue.write_buffer(&self.view_buffer, 0, &bytes);
            self.layout = Some(layout);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tank"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Every pixel is written, so there is nothing to load.
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timer.map(GpuTimer::pass_writes),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// The palette texture for `world`'s materials, sRGB-encoded.
fn create_palette(gpu: &Gpu, world: &World) -> wgpu::Texture {
    let d = &gpu.device;
    // The palette holds sRGB-encoded values, read raw: the shader shades in
    // encoded space and encodes for sRGB targets itself, so every target
    // format (WebGL2's sRGB canvas, the Rgba8Unorm captures) gets the same
    // pixels.
    let palette_format = wgpu::TextureFormat::Rgba8Unorm;
    let side = side(palette::SIDE);
    let palette_size = wgpu::Extent3d {
        width: side,
        height: side,
        depth_or_array_layers: 1,
    };
    let palette = d.create_texture(&wgpu::TextureDescriptor {
        label: Some("palette"),
        size: palette_size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: palette_format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        palette.as_image_copy(),
        &palette::build(world.materials()),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(side * 4),
            rows_per_image: None,
        },
        palette_size,
    );
    palette
}

/// The full-screen-triangle pipeline drawing into `format`.
fn create_pipeline(
    d: &wgpu::Device,
    bind_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("tank"),
        source: wgpu::ShaderSource::Wgsl(include_str!("tank.wgsl").into()),
    });
    let pipeline_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("tank"),
        bind_group_layouts: &[Some(bind_layout)],
        immediate_size: 0,
    });
    d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("tank"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// The backdrop, sRGB-encoded (the shader encodes for sRGB targets).
fn backdrop() -> [f32; 4] {
    let [r, g, b] = BACKDROP.map(|c| f32::from(c) / 255.0);
    [r, g, b, 1.0]
}

/// A grid or texture side as `u32`. Sides are at most
/// [`desktank_sim::MAX_SIDE`], so this never saturates in practice.
fn side(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
