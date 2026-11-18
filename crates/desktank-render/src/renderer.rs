//! GPU resources for one grid and one target format: the cell texture, the
//! palette, the uniforms, the sprite atlas and instances, and the two
//! pipelines (the grid, then the creature sprites over it).

use desktank_sim::{FieldKind, Part, World};

use crate::grid::{self, DirtyRects};
use crate::hud::{self, BrushPreview, PaletteLayout};
use crate::layout::Layout;
use crate::palette;
use crate::plants::{self, PlantTints};
use crate::sprites::{self, Atlas};
use crate::{Gpu, timer::GpuTimer};

/// Darkening of water at the floor relative to the surface.
const DEPTH_DIM: f32 = 0.5;

/// Size of the `View` uniform block in `tank.wgsl`, in bytes.
const VIEW_BYTES: u64 = 144;
/// Bytes per sprite instance.
const INSTANCE_BYTES: u64 = (sprites::INSTANCE_FLOATS * 4) as u64;

/// Field value shown at full strength by the overlay (light is 0–1).
const FIELD_SHOW_MAX: f32 = 2.0;

/// Letterbox colour around the grid (sRGB).
const BACKDROP: [u8; 3] = [0x05, 0x0a, 0x12];

#[derive(Debug)]
pub struct Renderer {
    format: wgpu::TextureFormat,
    grid: (u32, u32),
    pipeline: wgpu::RenderPipeline,
    sprite_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    /// Where each species' frames are in the atlas texture.
    atlas: Atlas,
    /// Per-instance sprite data, [`sprites::MAX_SPRITES`] slots.
    instances: wgpu::Buffer,
    /// Sprites to draw, and the instance bytes last written.
    sprite_count: u32,
    instance_bytes: Vec<u8>,
    instance_scratch: Vec<f32>,
    view_buffer: wgpu::Buffer,
    cells: wgpu::Texture,
    /// The palette strip image (top-left corner of a fixed-size texture).
    hud: wgpu::Texture,
    /// Nutrient, CO₂ and light per field cell (RGB), for the debug overlay.
    fields: wgpu::Texture,
    fields_size: (u32, u32),
    /// Field version last uploaded.
    fields_version: Option<u64>,
    /// Health tint per plant id (T6b), and its CPU copy.
    plants: wgpu::Texture,
    plant_tints: PlantTints,
    /// Material ids of the root, stem and leaf parts and of water, for the
    /// shader's plant shapes (-1 if missing).
    plant_parts: [f32; 4],
    /// Uniform bytes last written.
    view: Vec<u8>,
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
            // Material, variant and the state's two bytes (a plant id).
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let palette = create_palette(gpu, world);
        let plant_tints = PlantTints::default();
        let plants = create_plants(gpu, &plant_tints);
        let species = create_species(gpu, world);
        let material = |name: &str| world.materials().id(name).map_or(-1.0, |m| f32::from(m.0));
        let [root, stem, leaf] = Part::ALL.map(|p| material(p.material_name()));
        let plant_parts = [root, stem, leaf, material("water")];
        let hud = create_hud(d);
        let (fields, fields_size) = create_fields(d, world);
        let atlas = Atlas::build(world.species());
        let atlas_texture = create_atlas(gpu, &atlas);
        let instances = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprites"),
            size: INSTANCE_BYTES * sprites::MAX_SPRITES as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_buffer = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view"),
            size: VIEW_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = create_bind_layout(d);
        let bind_group = create_bind_group(
            d,
            &bind_layout,
            &view_buffer,
            [
                &cells,
                &palette,
                &hud,
                &fields,
                &atlas_texture,
                &plants,
                &species,
            ],
        );
        let (pipeline, sprite_pipeline) = create_pipelines(d, &bind_layout, format);
        Self {
            format,
            grid,
            pipeline,
            sprite_pipeline,
            bind_group,
            atlas,
            instances,
            sprite_count: 0,
            instance_bytes: Vec::new(),
            instance_scratch: Vec::new(),
            view_buffer,
            cells,
            hud,
            fields,
            fields_size,
            fields_version: None,
            plants,
            plant_tints,
            plant_parts,
            view: Vec::new(),
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
                    bytes_per_row: Some(side((r.x1 - r.x0) * grid::TEXEL_BYTES)),
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

    /// Uploads the rows of the plant tint texture whose plants changed
    /// since the last upload. Returns how many bytes it wrote.
    pub fn upload_plants(&mut self, gpu: &Gpu, world: &World) -> usize {
        let texture = &self.plants;
        let mut bytes = 0;
        self.plant_tints.update(world, |row, texels| {
            bytes += texels.len();
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: side(row),
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(side(texels.len())),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: side(plants::SIDE),
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        });
        bytes
    }

    /// Writes one sprite instance per creature, if they changed since the
    /// last upload.
    pub fn upload_sprites(&mut self, gpu: &Gpu, world: &World) {
        self.sprite_count = self.atlas.instances(world, &mut self.instance_scratch);
        let bytes: Vec<u8> = self
            .instance_scratch
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        if bytes != self.instance_bytes && !bytes.is_empty() {
            gpu.queue.write_buffer(&self.instances, 0, &bytes);
        }
        self.instance_bytes = bytes;
    }

    /// Uploads the fields for the overlay if they changed since the last
    /// upload.
    pub fn upload_fields(&mut self, gpu: &Gpu, world: &World) {
        if self.fields_version == Some(world.fields_version()) {
            return;
        }
        self.fields_version = Some(world.fields_version());
        let [n, c, l] = FieldKind::ALL.map(|k| world.field(k).values());
        let byte = |v: f32| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped
            let b = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            b
        };
        // The cell-upload scratch buffer, reused.
        let texels = &mut self.scratch;
        texels.clear();
        texels.extend((0..n.len()).flat_map(|i| {
            [
                byte(n[i] / FIELD_SHOW_MAX),
                byte(c[i] / FIELD_SHOW_MAX),
                byte(l[i]),
                255,
            ]
        }));
        gpu.queue.write_texture(
            self.fields.as_image_copy(),
            texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.fields_size.0 * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: self.fields_size.0,
                height: self.fields_size.1,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Uploads the palette strip image (`size` palette pixels, RGBA rows).
    pub fn upload_hud(&self, gpu: &Gpu, image: &[u8], size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        gpu.queue.write_texture(
            self.hud.as_image_copy(),
            image,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.0 * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Records the draw into `target` (`size` pixels), with the palette at
    /// `palette`, the brush outline at `cursor` and the field overlay
    /// `field` if given.
    #[allow(clippy::too_many_arguments)] // one frame's inputs
    pub fn encode(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        surface_row: usize,
        daylight: [f32; 3],
        overlay: (
            Option<PaletteLayout>,
            Option<BrushPreview>,
            Option<FieldKind>,
        ),
        timer: Option<&GpuTimer>,
    ) {
        let layout = Layout::fit(self.grid, size);
        let (palette, cursor, field) = overlay;
        // 0 = none; 1, 2, 3 = nutrient, CO₂, light (the texture's channels).
        let field_mode = match field {
            None => 0.0,
            Some(FieldKind::Nutrient) => 1.0,
            Some(FieldKind::Co2) => 2.0,
            Some(FieldKind::Light) => 3.0,
        };
        #[allow(clippy::cast_precision_loss)] // pixel and cell coordinates
        let extra = {
            let hud = palette.map_or([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, field_mode, 0.0], |p| {
                let (w, h) = hud::image_size(p.tabs, p.items);
                [
                    p.origin.0 as f32,
                    p.origin.1 as f32,
                    p.scale as f32,
                    1.0,
                    w as f32,
                    h as f32,
                    field_mode,
                    0.0,
                ]
            });
            let cursor = cursor.map_or([0.0; 4], |c| {
                [c.cell.0 as f32, c.cell.1 as f32, c.radius as f32, 1.0]
            });
            #[allow(clippy::cast_precision_loss)] // target pixels
            let target = [size.0 as f32, size.1 as f32, 0.0, 0.0];
            hud.into_iter()
                .chain(cursor)
                .chain(daylight)
                .chain([1.0])
                .chain(target)
                .chain(self.plant_parts)
        };
        {
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
            .chain(backdrop())
            .chain(extra);
            let bytes: Vec<u8> = view.flat_map(f32::to_le_bytes).collect();
            if bytes != self.view {
                gpu.queue.write_buffer(&self.view_buffer, 0, &bytes);
                self.view = bytes;
            }
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
        if self.sprite_count > 0 {
            pass.set_pipeline(&self.sprite_pipeline);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..6, 0..self.sprite_count);
        }
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

/// The per-plant tint texture, filled from `tints` (untinted at first).
fn create_plants(gpu: &Gpu, tints: &PlantTints) -> wgpu::Texture {
    let n = side(plants::SIDE);
    let size = wgpu::Extent3d {
        width: n,
        height: n,
        depth_or_array_layers: 1,
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("plants"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        texture.as_image_copy(),
        tints.image(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(n * 4),
            rows_per_image: None,
        },
        size,
    );
    texture
}

/// The species texture (T6d): ramps, shapes and sizes per species id.
fn create_species(gpu: &Gpu, world: &World) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width: side(plants::SPECIES_WIDTH),
        height: side(plants::SIDE),
        depth_or_array_layers: 1,
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("plant species"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // sRGB-encoded, read raw like the palette.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        texture.as_image_copy(),
        &plants::species_image(world.plant_species()),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(side(plants::SPECIES_WIDTH * 4)),
            rows_per_image: None,
        },
        size,
    );
    texture
}

/// The field-overlay texture: one RGBA texel per field cell.
fn create_fields(d: &wgpu::Device, world: &World) -> (wgpu::Texture, (u32, u32)) {
    let field = world.field(FieldKind::Nutrient);
    let size = (side(field.width()), side(field.height()));
    let texture = d.create_texture(&wgpu::TextureDescriptor {
        label: Some("fields"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    (texture, size)
}

/// The one bind group layout both pipelines share: the view uniform and
/// seven textures (cells, palette, HUD, fields, sprite atlas, plant tints,
/// plant species).
fn create_bind_layout(d: &wgpu::Device) -> wgpu::BindGroupLayout {
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
    d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("tank"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                // The sprite vertex shader places quads with the view.
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(VIEW_BYTES),
                },
                count: None,
            },
            texture_entry(1),
            texture_entry(2),
            texture_entry(3),
            texture_entry(4),
            texture_entry(5),
            texture_entry(6),
            texture_entry(7),
        ],
    })
}

/// The bind group: `view` at binding 0, then `textures` at 1–7.
fn create_bind_group(
    d: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::Buffer,
    textures: [&wgpu::Texture; 7],
) -> wgpu::BindGroup {
    let views = textures.map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
    let mut entries = vec![wgpu::BindGroupEntry {
        binding: 0,
        resource: view.as_entire_binding(),
    }];
    for (binding, v) in (1..).zip(&views) {
        entries.push(wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(v),
        });
    }
    d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tank"),
        layout,
        entries: &entries,
    })
}

/// The sprite atlas texture, filled from `atlas`.
fn create_atlas(gpu: &Gpu, atlas: &Atlas) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width: atlas.size.0,
        height: atlas.size.1,
        depth_or_array_layers: 1,
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("atlas"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // sRGB-encoded, read raw like the palette.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        texture.as_image_copy(),
        &atlas.image,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(atlas.size.0 * 4),
            rows_per_image: None,
        },
        size,
    );
    texture
}

/// The palette-strip texture, sized for the largest strip.
fn create_hud(d: &wgpu::Device) -> wgpu::Texture {
    d.create_texture(&wgpu::TextureDescriptor {
        label: Some("hud"),
        size: wgpu::Extent3d {
            width: hud::MAX_SIZE.0,
            height: hud::MAX_SIZE.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // sRGB-encoded, read raw like the palette.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// The full-screen-triangle grid pipeline and the instanced sprite
/// pipeline, drawing into `format`.
fn create_pipelines(
    d: &wgpu::Device,
    bind_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::RenderPipeline) {
    let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("tank"),
        source: wgpu::ShaderSource::Wgsl(include_str!("tank.wgsl").into()),
    });
    let pipeline_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("tank"),
        bind_group_layouts: &[Some(bind_layout)],
        immediate_size: 0,
    });
    let targets = [Some(wgpu::ColorTargetState {
        format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let grid = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    });
    // Per instance: top-left cell, size, atlas origin, mirror and turn
    // (vec2 each).
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2
    ];
    let sprites = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sprites"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_sprite"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE_BYTES,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &attributes,
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        // Transparent texels are discarded: no blending needed.
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_sprite"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    });
    (grid, sprites)
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
