//! GPU resources for one grid and one target format: the cell and plant
//! band textures, the palette, the uniforms, the sprite atlas and
//! instances. The grid draws first, then depth-sorted sprites, then any
//! film on the front glass. With a camera (D29) all of that draws into an
//! offscreen image first and a post pass shows a view of it, then the
//! screen-space overlay draws over that.

use std::collections::BTreeMap;

use desktank_sim::{FieldKind, Part, World};

use crate::algae;
use crate::camera::ViewRect;
use crate::card::{self, CardLayout};
use crate::grid::{self, DirtyRects};
use crate::hud::{self, BrushPreview, Magnifier, MagnifierLayout, PaletteLayout};
use crate::layout::Layout;
use crate::palette;
use crate::plants::{self, PlantTints};
use crate::post::{Offscreen, Post};
use crate::ruler;
use crate::sprites::{self, Atlas};
use crate::{Gpu, timer::GpuTimer};

/// The shader's field overlay mode: 0 = none; 1, 2, 3 = nutrient, CO₂, light
/// (the texture's channels); 4 = flow.
fn field_mode(field: Option<crate::FieldOverlay>) -> f32 {
    match field {
        None => 0.0,
        Some(crate::FieldOverlay::Scalar(FieldKind::Nutrient)) => 1.0,
        Some(crate::FieldOverlay::Scalar(FieldKind::Co2)) => 2.0,
        Some(crate::FieldOverlay::Scalar(FieldKind::Light)) => 3.0,
        Some(crate::FieldOverlay::Flow) => 4.0,
    }
}

/// What is drawn over the tank in one frame.
#[derive(Clone, Copy)]
pub struct Overlay {
    /// The interact palette.
    pub palette: Option<PaletteLayout>,
    /// The species card.
    pub card: Option<CardLayout>,
    /// The in-tank inspect card.
    pub inspect: Option<CardLayout>,
    /// The brush outline.
    pub cursor: Option<BrushPreview>,
    /// The magnified lens.
    pub magnifier: Option<Magnifier>,
    /// The field tinted over the tank.
    pub field: Option<crate::FieldOverlay>,
    /// The centimetre ruler on the bottom and left edges: `Some` with the
    /// smallest art pixel in target pixels (0 for none) when shown.
    pub ruler: Option<u32>,
}

/// Darkening of water at the floor relative to the surface.
const DEPTH_DIM: f32 = 0.5;

/// Size of the `View` uniform block in `tank.wgsl`, in bytes (272 base + 16 ripple vec4s).
const VIEW_BYTES: u64 = 272 + (desktank_sim::MAX_DEVICES as u64) * 16;
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
    /// The grid and sprite pipelines, drawn in order: one grid pipeline and
    /// one sprite pipeline, or split passes with [`Gpu::split_grid`].
    pipelines: Pipelines,
    front_film: wgpu::RenderPipeline,
    /// Border and centre reticle drawn over the magnified lens.
    lens_frame: wgpu::RenderPipeline,
    /// The tank shader and pipeline layout, kept to build the camera's
    /// overlay pipeline when a camera is first set.
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    /// The centimetre ruler's strips over the finished tank.
    ruler: wgpu::RenderPipeline,
    /// The layout both bind groups use, kept to rebuild them when the grid
    /// textures change size.
    bind_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    /// Second bind group sharing every texture with `lens_view_buffer` at
    /// binding 0, used only while the magnifier is active.
    lens_bind_group: wgpu::BindGroup,
    /// Where each species' frames are in the atlas texture.
    atlas: Atlas,
    atlas_texture: wgpu::Texture,
    /// Per-instance sprite data, [`sprites::BUFFER_SPRITES`] slots.
    instances: wgpu::Buffer,
    /// Sprites to draw, and the instance bytes last written.
    sprite_count: u32,
    sprite_scratch: Vec<sprites::Sprite>,
    instance_bytes: Vec<u8>,
    instance_scratch: Vec<f32>,
    view_buffer: wgpu::Buffer,
    lens_view_buffer: wgpu::Buffer,
    cells: wgpu::Texture,
    /// Each plant or rigid cell's depth, uploaded with the cell texture.
    bands: wgpu::Texture,
    band_scratch: Vec<u8>,
    algae: AlgaeTextures,
    /// Each plant's base cell when its bands were last uploaded, and the
    /// plants version then: a stem plant's leaf bands follow its base
    /// column, which can move without its leaf cells changing.
    plant_bases: BTreeMap<u16, (usize, usize)>,
    bases_version: Option<u64>,
    /// The material palette, built once.
    palette: wgpu::Texture,
    /// Ramps, shapes and sizes per plant species, built once.
    species: wgpu::Texture,
    /// The palette strip image (top-left corner of a fixed-size texture).
    hud: wgpu::Texture,
    /// Nutrient, CO₂, light and flow speed per field cell (RGBA), for the debug overlay.
    fields: wgpu::Texture,
    /// 2D flow vector `(vx, vy)` and sway lean per field cell (`Rgba8Unorm`,
    /// 128 = 0), for the flow overlay, current-driven surface ripples and
    /// plant sway.
    flow: wgpu::Texture,
    fields_size: (u32, u32),
    /// Field version last uploaded.
    fields_version: Option<u64>,
    /// Current-rebuild counter last uploaded to `flow`.
    flow_version: Option<u64>,
    /// Health tint per plant id (T6b), and its CPU copy.
    plants: wgpu::Texture,
    plant_tints: PlantTints,
    /// Material ids of the root, stem and leaf parts and of water, then of
    /// the runner and floating leaf parts, for the shader's plant shapes
    /// (-1 if missing).
    plant_parts: [f32; 8],
    /// Material ids drawn as stone varieties (`palette::variety_stones`).
    variety_stones: [bool; palette::SIDE],
    /// Whether any cell holds one, as of the last upload that changed cells.
    stone_varieties_visible: bool,
    /// The rectangle of cells covering every plant (none: no plants), so the
    /// split grid's plant passes only draw where a plant is.
    plant_area: Option<desktank_sim::Rect>,
    /// Uniform bytes last written.
    view: Vec<u8>,
    /// Uniform bytes last written for the magnifier pass.
    lens_view: Vec<u8>,
    scratch: Vec<u8>,
    /// The camera's pipelines and buffers, built the first time one is set.
    camera: Option<CameraGpu>,
    /// The image the tank draws into while a camera is set; dropped when
    /// it isn't.
    offscreen: Option<Offscreen>,
}

/// What a camera needs beyond the tank's own resources (D29): the post
/// pass, and the pipeline, uniform and bind group that draw the overlay
/// (cards, palette, brush outline) over it in screen space.
#[derive(Debug)]
struct CameraGpu {
    post: Post,
    overlay_pipeline: wgpu::RenderPipeline,
    overlay_view_buffer: wgpu::Buffer,
    overlay_bind_group: wgpu::BindGroup,
    /// Uniform bytes last written for the overlay pass.
    overlay_view: Vec<u8>,
}

impl Renderer {
    /// Resources for `world`'s grid, drawing into `format` targets.
    pub fn new(gpu: &Gpu, world: &World, format: wgpu::TextureFormat) -> Self {
        let d = &gpu.device;
        let grid = (side(world.width()), side(world.height()));
        // Material, variant and the state's two bytes (a plant id).
        let cells = create_grid_texture(d, "cells", grid, wgpu::TextureFormat::Rgba8Unorm);
        let bands = create_grid_texture(d, "depth bands", grid, wgpu::TextureFormat::R8Unorm);
        let palette = create_palette(gpu, world);
        let plant_tints = PlantTints::default();
        let plants = create_plants(gpu, &plant_tints);
        let species = create_species(gpu, world);
        let material = |name: &str| world.materials().id(name).map_or(-1.0, |m| f32::from(m.0));
        let [root, stem, leaf, runner, float, rhizome] =
            Part::ALL.map(|p| material(p.material_name()));
        let plant_parts = [
            root,
            stem,
            leaf,
            material("water"),
            runner,
            float,
            rhizome,
            material("air"),
        ];
        let hud = create_hud(d);
        let (fields, flow, fields_size) = create_fields(d, world);
        let algae = AlgaeTextures::new(gpu, grid, fields_size, world);
        let atlas = Atlas::build(world.species(), world.device_kinds());
        let atlas_texture = create_atlas(gpu, &atlas);
        let instances = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprites"),
            size: INSTANCE_BYTES * sprites::BUFFER_SPRITES as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_buffer = create_view_buffer(d, "view");
        let lens_view_buffer = create_view_buffer(d, "lens view");
        let bind_layout = create_bind_layout(d);
        let textures = [
            &cells,
            &palette,
            &hud,
            &fields,
            &atlas_texture,
            &plants,
            &species,
            &bands,
            &flow,
            &algae.coat,
            &algae.glass,
            &algae.look_atlas,
        ];
        let bind_group = create_bind_group(d, &bind_layout, &view_buffer, textures);
        let lens_bind_group = create_bind_group(d, &bind_layout, &lens_view_buffer, textures);
        let (pipelines, [lens_frame, front_film, ruler], shader, pipeline_layout) =
            create_pipelines(gpu, &bind_layout, format);
        Self {
            format,
            grid,
            pipelines,
            front_film,
            lens_frame,
            shader,
            pipeline_layout,
            ruler,
            bind_layout,
            bind_group,
            lens_bind_group,
            atlas,
            atlas_texture,
            instances,
            sprite_count: 0,
            sprite_scratch: Vec::new(),
            instance_bytes: Vec::new(),
            instance_scratch: Vec::new(),
            view_buffer,
            lens_view_buffer,
            cells,
            bands,
            band_scratch: Vec::new(),
            algae,
            plant_bases: BTreeMap::new(),
            bases_version: None,
            palette,
            species,
            hud,
            fields,
            flow,
            fields_size,
            fields_version: None,
            flow_version: None,
            plants,
            plant_tints,
            plant_parts,
            variety_stones: palette::variety_stones(world.materials()),
            stone_varieties_visible: false,
            plant_area: None,
            view: Vec::new(),
            lens_view: Vec::new(),
            scratch: Vec::new(),
            camera: None,
            offscreen: None,
        }
    }

    /// Whether these resources fit `world` and `format`.
    pub fn matches(&self, world: &World, format: wgpu::TextureFormat) -> bool {
        self.format == format && self.grid == (side(world.width()), side(world.height()))
    }

    /// The target format these resources draw into.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Forgets what was uploaded for the previous world, so a new world's
    /// cells, plant bases, tints, fields, flow, algae and sprites are all written
    /// afresh. Textures and pipelines stay.
    pub fn reset_world(&mut self) {
        self.plant_bases.clear();
        self.bases_version = None;
        self.fields_version = None;
        self.flow_version = None;
        self.algae.coat_version = None;
        self.algae.glass_version = None;
        self.plant_tints.invalidate();
        self.instance_bytes.clear();
        self.view.clear();
        self.lens_view.clear();
        if let Some(camera) = &mut self.camera {
            camera.overlay_view.clear();
        }
    }

    /// Resizes the grid-dependent textures and bind groups to `world`'s grid,
    /// keeping the compiled pipelines and the static textures, and resets
    /// the per-world state. The caller uploads the whole grid next.
    pub fn resize_grid(&mut self, gpu: &Gpu, world: &World) {
        let d = &gpu.device;
        self.grid = (side(world.width()), side(world.height()));
        self.cells = create_grid_texture(d, "cells", self.grid, wgpu::TextureFormat::Rgba8Unorm);
        self.bands = create_grid_texture(d, "depth bands", self.grid, wgpu::TextureFormat::R8Unorm);
        (self.fields, self.flow, self.fields_size) = create_fields(d, world);
        [self.algae.coat, self.algae.glass] =
            AlgaeTextures::textures(d, self.grid, self.fields_size);
        let textures = [
            &self.cells,
            &self.palette,
            &self.hud,
            &self.fields,
            &self.atlas_texture,
            &self.plants,
            &self.species,
            &self.bands,
            &self.flow,
            &self.algae.coat,
            &self.algae.glass,
            &self.algae.look_atlas,
        ];
        self.bind_group = create_bind_group(d, &self.bind_layout, &self.view_buffer, textures);
        self.lens_bind_group =
            create_bind_group(d, &self.bind_layout, &self.lens_view_buffer, textures);
        if let Some(camera) = &mut self.camera {
            camera.overlay_bind_group =
                create_bind_group(d, &self.bind_layout, &camera.overlay_view_buffer, textures);
        }
        self.algae.set_looks(gpu, world);
        self.reset_world();
    }

    /// Uploads the packed coat and glass planes when their shared version changes.
    pub fn upload_algae(&mut self, gpu: &Gpu, world: &World) {
        self.algae.upload(gpu, world, &mut self.scratch);
    }

    /// Copies the waiting dirty rectangles (or the whole grid) into the cell
    /// texture, and their plant and rigid cells' depth into the band texture.
    pub fn upload(&mut self, gpu: &Gpu, world: &World, dirty: &mut DirtyRects) {
        let width = world.width();
        let full = desktank_sim::Rect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: world.height(),
        };
        let cells = world.cells();
        if !dirty.is_empty() {
            // Cells changed: look for variety stones again (stops at the first).
            let stones = &self.variety_stones;
            self.stone_varieties_visible = cells.iter().any(|c| stones[usize::from(c.material.0)]);
        }
        let (scratch, band_scratch) = (&mut self.scratch, &mut self.band_scratch);
        let (texture, bands) = (&self.cells, &self.bands);
        dirty.take(|r| {
            let r = r.unwrap_or(full);
            grid::pack(cells, width, r, |i| world.rigid_mask(i), scratch);
            write_rect(gpu, texture, r, scratch, grid::TEXEL_BYTES);
            // Unbanded cells never sample this texture, so old bytes can stay.
            if world.render_bands(r, band_scratch) {
                write_rect(gpu, bands, r, band_scratch, 1);
            }
        });
        // Floaters drift without any plant changing, so look every frame.
        self.plant_area = world
            .plants()
            .map(|p| p.bounds)
            .filter(|b| !b.is_empty())
            .reduce(desktank_sim::Rect::union);
        if self.bases_version != Some(world.plants_version()) {
            self.bases_version = Some(world.plants_version());
            for p in world.plants() {
                let old = self.plant_bases.insert(p.id.0, p.base);
                let r = desktank_sim::Rect {
                    x0: p.bounds.x0.min(full.x1),
                    y0: p.bounds.y0.min(full.y1),
                    x1: p.bounds.x1.min(full.x1),
                    y1: p.bounds.y1.min(full.y1),
                };
                if old.is_some_and(|b| b != p.base) && !r.is_empty() {
                    world.render_bands(r, band_scratch);
                    write_rect(gpu, bands, r, band_scratch, 1);
                }
            }
        }
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

    /// Writes one sprite instance per creature, corpse and device, and the
    /// placement `ghost` and whole-plant `plant_ghost`, if they changed since
    /// the last upload.
    pub fn upload_sprites(
        &mut self,
        gpu: &Gpu,
        world: &World,
        ghost: Option<(desktank_sim::Device, bool)>,
        plant_ghost: Option<&desktank_sim::PlantGhost>,
    ) {
        self.sprite_count = self.atlas.instances_with(
            world,
            ghost,
            plant_ghost,
            &mut self.sprite_scratch,
            &mut self.instance_scratch,
        );
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
    /// upload, plus the 2D flow texture if the current was rebuilt.
    pub fn upload_fields(&mut self, gpu: &Gpu, world: &World) {
        if self.fields_version != Some(world.fields_version()) {
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
            let speed = world.ecology().current.speed;
            let current = world.current();
            texels.extend((0..n.len()).flat_map(|i| {
                let [vx, vy] = current[i];
                [
                    byte(n[i] / FIELD_SHOW_MAX),
                    byte(c[i] / FIELD_SHOW_MAX),
                    byte(l[i]),
                    byte((vx * vx + vy * vy).sqrt() / speed),
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
        self.upload_flow(gpu, world);
    }

    /// Uploads the 2D current texture if the current field was rebuilt since
    /// the last upload (used by the flow overlay and surface-current ripples).
    pub fn upload_flow(&mut self, gpu: &Gpu, world: &World) {
        if self.flow_version == Some(world.device_rebuilds()) {
            return;
        }
        self.flow_version = Some(world.device_rebuilds());
        let speed = world.ecology().current.speed;
        let current = world.current();
        let lean = sway_lean(current, self.fields_size.0 as usize, speed);
        // 2D flow direction and sway lean: signed values mapped to 1..=255
        // with 128 = 0.
        let signed_byte = |v: f32| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 1..=255
            let b = (v.clamp(-1.0, 1.0) * 127.0 + 128.0).round() as u8;
            b
        };
        let texels = &mut self.scratch;
        texels.clear();
        texels.extend(current.iter().zip(&lean).flat_map(|(&[vx, vy], &l)| {
            [
                signed_byte(vx / speed),
                signed_byte(vy / speed),
                signed_byte(l),
                128,
            ]
        }));
        gpu.queue.write_texture(
            self.flow.as_image_copy(),
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
        self.write_hud(gpu, image, size, 0);
    }

    /// Uploads the species card image (`size` card pixels, RGBA rows) to the
    /// card's region of the HUD texture, below the palette's.
    pub fn upload_card(&self, gpu: &Gpu, image: &[u8], size: (u32, u32)) {
        self.write_hud(gpu, image, size, card::TEXTURE_Y);
    }

    /// Uploads the inspect card image (`size` card pixels, RGBA rows) to
    /// its region of the HUD texture, below the species card's.
    pub fn upload_inspect(&self, gpu: &Gpu, image: &[u8], size: (u32, u32)) {
        self.write_hud(gpu, image, size, crate::inspect::TEXTURE_Y);
    }

    /// Writes `image` into the HUD texture at column 0, row `y`.
    fn write_hud(&self, gpu: &Gpu, image: &[u8], size: (u32, u32), y: u32) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d { x: 0, y, z: 0 },
                ..self.hud.as_image_copy()
            },
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

    /// Packs the `View` uniform block bytes for one pass (main tank or lens).
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        clippy::cast_precision_loss
    )]
    fn pack_view(
        &self,
        size: (u32, u32),
        offset: (f32, f32),
        scale: f32,
        surface_row: usize,
        daylight: [f32; 3],
        phase: f32,
        sway_on: bool,
        ripples: &[[f32; 4]],
        surface_ripple_threshold: f32,
        overlay: Overlay,
        lens: [f32; 4],
        overlay_water_alpha: Option<f32>,
        oled: bool,
    ) -> Vec<u8> {
        let Overlay {
            palette,
            card,
            inspect,
            cursor,
            field,
            ruler,
            ..
        } = overlay;
        let ruler = ruler.map_or([0.0; 4], |min_px| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let scale = scale.max(1.0).round() as u32;
            ruler::Plan::new(scale, min_px, self.grid).uniform()
        });
        let field_mode = field_mode(field);
        let hud = palette.map_or(
            [
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                field_mode,
                surface_ripple_threshold,
            ],
            |p| {
                let (w, h) = hud::image_size(p.tabs, p.subs, p.items, p.morphs);
                [
                    p.origin.0 as f32,
                    p.origin.1 as f32,
                    p.scale as f32,
                    1.0,
                    w as f32,
                    h as f32,
                    field_mode,
                    surface_ripple_threshold,
                ]
            },
        );
        // Origin, scale and 1 if shown; then size and the texture row of the
        // card's region.
        let region = |c: Option<CardLayout>, row: u32| {
            c.map_or([0.0; 8], |c| {
                [
                    c.origin.0 as f32,
                    c.origin.1 as f32,
                    c.scale as f32,
                    1.0,
                    c.image.0 as f32,
                    c.image.1 as f32,
                    row as f32,
                    0.0,
                ]
            })
        };
        let card = region(card, card::TEXTURE_Y);
        let inspect = region(inspect, crate::inspect::TEXTURE_Y);
        let cursor = cursor.map_or([0.0; 4], |c| {
            [c.cell.0 as f32, c.cell.1 as f32, c.radius as f32, 1.0]
        });
        let target_info = [
            size.0 as f32,
            size.1 as f32,
            phase,
            if sway_on { 1.0 } else { 0.0 },
        ];
        let mut ripple_slots = [[0.0_f32; 4]; desktank_sim::MAX_DEVICES];
        for (slot, &r) in ripple_slots.iter_mut().zip(ripples.iter()) {
            *slot = r;
        }
        [
            self.grid.0 as f32,
            self.grid.1 as f32,
            offset.0,
            offset.1,
            scale,
            surface_row as f32,
            DEPTH_DIM,
            if self.format.is_srgb() { 1.0 } else { 0.0 },
        ]
        .into_iter()
        .chain(backdrop(overlay_water_alpha, oled))
        .chain(hud)
        .chain(card)
        .chain(inspect)
        .chain(cursor)
        .chain(daylight)
        .chain([desktank_sim::daylight::darkness(daylight)])
        .chain(target_info)
        .chain(self.plant_parts)
        .chain(lens)
        .chain([
            f32::from(self.algae.coat_visible),
            f32::from(self.algae.rear_film_visible),
            f32::from(self.algae.front_film_visible && overlay_water_alpha.is_none()),
            f32::from(oled_applies(overlay_water_alpha, oled)),
        ])
        .chain(ruler)
        .chain(ripple_slots.into_iter().flatten())
        .flat_map(f32::to_le_bytes)
        .collect()
    }

    /// Records the draw into `target` (`size` pixels), with the palette at
    /// `palette`, the brush outline at `cursor`, the magnified lens at
    /// `magnifier` and the field overlay `field` if given.
    ///
    /// With a `camera` the tank draws offscreen at `layout`, a post pass
    /// shows that view of it over `target`, and the overlay (palette,
    /// cards, brush outline, lens) draws over that in screen space; without
    /// one the tank and overlay draw straight into `target`, with no
    /// offscreen image.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one frame's inputs and passes
    pub fn encode(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        layout: Layout,
        camera: Option<ViewRect>,
        overlay_water_alpha: Option<f32>,
        surface_row: usize,
        daylight: [f32; 3],
        phase: f32,
        sway_on: bool,
        ripples: &[[f32; 4]],
        surface_ripple_threshold: f32,
        overlay: Overlay,
        oled: bool,
        timer: Option<&GpuTimer>,
    ) {
        let camera = camera.filter(|v| v.is_valid() && size.0 > 0 && size.1 > 0);
        let divisor = if camera.is_some() && gpu.compact_camera {
            (layout.scale / 2).max(1)
        } else {
            1
        };
        let source_size = (size.0.div_ceil(divisor), size.1.div_ceil(divisor));
        let source_layout = Layout {
            scale: layout.scale / divisor,
            offset: (
                layout.offset.0 / i64::from(divisor),
                layout.offset.1 / i64::from(divisor),
            ),
            ..layout
        };
        // With a camera the tank draws without the overlay, which draws in
        // screen space afterwards (the field tint belongs to the tank).
        let tank_overlay = if camera.is_some() {
            Overlay {
                palette: None,
                card: None,
                inspect: None,
                cursor: None,
                magnifier: None,
                ..overlay
            }
        } else {
            overlay
        };
        #[allow(clippy::cast_precision_loss)] // pixels
        let bytes = self.pack_view(
            source_size,
            (source_layout.offset.0 as f32, source_layout.offset.1 as f32),
            source_layout.scale as f32,
            surface_row,
            daylight,
            phase,
            sway_on,
            ripples,
            surface_ripple_threshold,
            tank_overlay,
            [0.0; 4],
            overlay_water_alpha,
            oled,
        );
        if bytes != self.view {
            gpu.queue.write_buffer(&self.view_buffer, 0, &bytes);
            self.view = bytes;
        }

        let lens_layout = overlay.magnifier.and_then(|m| {
            let grid = camera.map_or(layout, |v| v.magnifier_grid(size, &layout, m.cell));
            MagnifierLayout::fit(size, &grid, m)
        });
        if let Some(ml) = lens_layout {
            #[allow(clippy::cast_precision_loss)] // target pixels
            let lens_info = [
                ml.centre.0 as f32,
                ml.centre.1 as f32,
                ml.radius as f32,
                ml.step as f32,
            ];
            #[allow(clippy::cast_precision_loss)] // pixels
            let lens_bytes = self.pack_view(
                size,
                (ml.offset.0 as f32, ml.offset.1 as f32),
                ml.scale as f32,
                surface_row,
                daylight,
                phase,
                sway_on,
                ripples,
                surface_ripple_threshold,
                Overlay {
                    cursor: None,
                    ruler: None,
                    ..overlay
                },
                lens_info,
                overlay_water_alpha,
                oled,
            );
            if lens_bytes != self.lens_view {
                gpu.queue
                    .write_buffer(&self.lens_view_buffer, 0, &lens_bytes);
                self.lens_view = lens_bytes;
            }
        }
        let front_pass = self.algae.front_film_visible
            && (!front_in_layers(self.format, gpu.split_grid) || overlay_water_alpha.is_some());
        let Some(view) = camera else {
            self.offscreen = None;
            let mut pass = begin_pass(encoder, "tank", target, timer.map(GpuTimer::pass_writes));
            self.draw_view(
                &mut pass,
                &self.bind_group,
                layout.scale,
                front_pass,
                PlantPasses::Within(self.plant_pixels(layout, size)),
            );
            if let Some(min_px) = overlay.ruler {
                // Only the edge strips are shaded; the rest of the tank is untouched.
                let strips = ruler::Plan::new(layout.scale, min_px, self.grid).strips(
                    layout.offset,
                    self.grid,
                    layout.scale,
                    size,
                );
                pass.set_pipeline(&self.ruler);
                for (x, y, w, h) in strips.into_iter().flatten() {
                    pass.set_scissor_rect(x, y, w, h);
                    pass.draw(0..3, 0..1);
                }
                pass.set_scissor_rect(0, 0, size.0, size.1);
            }
            if let Some(ml) = lens_layout {
                self.draw_lens(&mut pass, &ml, front_pass);
            }
            return;
        };

        self.build_camera(gpu, source_size);
        let backdrop = camera_backdrop(self.format, overlay_water_alpha, oled);
        if let Some(camera) = self.camera.as_mut() {
            camera
                .post
                .update(gpu, view, &source_layout, size, source_size, backdrop);
        }
        let Some(image) = self.offscreen.as_ref().map(|o| o.view().clone()) else {
            return;
        };
        // The tank at its integer layout, into the image.
        {
            let mut pass = begin_pass(
                encoder,
                "tank image",
                &image,
                timer.map(GpuTimer::begin_writes),
            );
            self.draw_view(
                &mut pass,
                &self.bind_group,
                source_layout.scale,
                front_pass,
                PlantPasses::Within(self.plant_pixels(source_layout, source_size)),
            );
        }
        // Where the view puts the grid, for the overlay's brush outline.
        let (sx, sy) = view.pixels_per_cell(size);
        #[allow(clippy::cast_possible_truncation)] // pixels
        let (scale, offset) = (
            sx as f32,
            (
                (-f64::from(view.x) * sx) as f32,
                (-f64::from(view.y) * sy) as f32,
            ),
        );
        let overlay_bytes = self.pack_view(
            size,
            offset,
            scale,
            surface_row,
            daylight,
            phase,
            sway_on,
            ripples,
            surface_ripple_threshold,
            Overlay {
                magnifier: None,
                ..overlay
            },
            [0.0; 4],
            overlay_water_alpha,
            oled,
        );
        if let Some(camera) = self.camera.as_mut()
            && overlay_bytes != camera.overlay_view
        {
            gpu.queue
                .write_buffer(&camera.overlay_view_buffer, 0, &overlay_bytes);
            camera.overlay_view = overlay_bytes;
        }
        let (Some(camera), Some(offscreen)) = (self.camera.as_ref(), self.offscreen.as_ref())
        else {
            return;
        };
        // The view over the target, then the lens, then the overlay.
        let mut pass = begin_pass(encoder, "camera", target, timer.map(GpuTimer::end_writes));
        pass.set_pipeline(camera.post.pipeline());
        pass.set_bind_group(0, offscreen.bind_group(), &[]);
        pass.draw(0..3, 0..1);
        if let Some(ml) = lens_layout {
            self.draw_lens(&mut pass, &ml, front_pass);
            pass.set_scissor_rect(0, 0, size.0, size.1);
        }
        pass.set_pipeline(&camera.overlay_pipeline);
        pass.set_bind_group(0, &camera.overlay_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Builds the camera's pipelines and uniform on first use, and the
    /// offscreen image for `size` if there isn't one that size.
    fn build_camera(&mut self, gpu: &Gpu, size: (u32, u32)) {
        if self.camera.is_none() {
            let d = &gpu.device;
            let overlay_view_buffer = create_view_buffer(d, "camera overlay view");
            let overlay_bind_group = create_bind_group(
                d,
                &self.bind_layout,
                &overlay_view_buffer,
                self.bound_textures(),
            );
            let overlay_pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("camera overlay"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some("fs_hud"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.camera = Some(CameraGpu {
                post: Post::new(gpu, self.format),
                overlay_pipeline,
                overlay_view_buffer,
                overlay_bind_group,
                overlay_view: Vec::new(),
            });
        }
        if self.offscreen.as_ref().is_none_or(|o| o.size() != size)
            && let Some(camera) = &self.camera
        {
            self.offscreen = Some(Offscreen::new(gpu, &camera.post, self.format, size));
        }
    }

    /// Frees the offscreen image (the camera was cleared). Its pipelines
    /// stay for the next camera.
    pub fn release_camera(&mut self) {
        self.offscreen = None;
    }

    /// The textures every bind group binds, in binding order from 1.
    fn bound_textures(&self) -> [&wgpu::Texture; 12] {
        [
            &self.cells,
            &self.palette,
            &self.hud,
            &self.fields,
            &self.atlas_texture,
            &self.plants,
            &self.species,
            &self.bands,
            &self.flow,
            &self.algae.coat,
            &self.algae.glass,
            &self.algae.look_atlas,
        ]
    }

    /// Draws the magnified lens and its frame, scissored to where they are.
    fn draw_lens(&self, pass: &mut wgpu::RenderPass<'_>, ml: &MagnifierLayout, front_pass: bool) {
        pass.set_scissor_rect(
            ml.lens_origin.0,
            ml.lens_origin.1,
            ml.lens_size.0,
            ml.lens_size.1,
        );
        self.draw_view(
            pass,
            &self.lens_bind_group,
            ml.scale,
            front_pass,
            PlantPasses::Everywhere,
        );
        if ml.frame_size.0 > 0 && ml.frame_size.1 > 0 {
            pass.set_scissor_rect(
                ml.frame_origin.0,
                ml.frame_origin.1,
                ml.frame_size.0,
                ml.frame_size.1,
            );
            pass.set_pipeline(&self.lens_frame);
            pass.draw(0..3, 0..1);
        }
    }

    /// The pixels the plants cover in a view at `layout` into a target of
    /// `size`, with a margin for leaves swaying out of their cells; none if
    /// there are no plants (or none on screen).
    fn plant_pixels(&self, layout: Layout, size: (u32, u32)) -> Option<PlantRect> {
        let area = self.plant_area?;
        let scale = i64::from(layout.scale);
        let to_px = |cell: usize, origin: i64, limit: u32| {
            let cell = i64::try_from(cell).unwrap_or(i64::MAX / 4);
            (origin + cell * scale).clamp(0, i64::from(limit))
        };
        let (x0, y0) = (
            to_px(
                area.x0.saturating_sub(PLANT_MARGIN),
                layout.offset.0,
                size.0,
            ),
            to_px(
                area.y0.saturating_sub(PLANT_MARGIN),
                layout.offset.1,
                size.1,
            ),
        );
        let (x1, y1) = (
            to_px(area.x1 + PLANT_MARGIN, layout.offset.0, size.0),
            to_px(area.y1 + PLANT_MARGIN, layout.offset.1, size.1),
        );
        (x1 > x0 && y1 > y0).then(|| PlantRect {
            x: u32::try_from(x0).unwrap_or(0),
            y: u32::try_from(y0).unwrap_or(0),
            w: u32::try_from(x1 - x0).unwrap_or(0),
            h: u32::try_from(y1 - y0).unwrap_or(0),
            target: size,
        })
    }

    /// Draws the grid and creature sprites with `bind_group`.
    fn draw_view(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        bind_group: &wgpu::BindGroup,
        scale: u32,
        front_pass: bool,
        plants: PlantPasses,
    ) {
        let pipelines = &self.pipelines;
        pass.set_bind_group(0, bind_group, &[]);
        for (i, p) in pipelines.grid.iter().enumerate() {
            // The split grid's plant passes (after the ground) draw nothing
            // but over plant cells: skip them with no plants, and shade only
            // the rectangle round the plants.
            let plant_pass = i > 0;
            let scissor = match (plant_pass, plants) {
                (true, PlantPasses::Within(None)) => continue,
                (true, PlantPasses::Within(Some(r))) => Some(r),
                _ => None,
            };
            if let Some(r) = scissor {
                pass.set_scissor_rect(r.x, r.y, r.w, r.h);
            }
            pass.set_pipeline(p);
            pass.draw(0..3, 0..1);
            if let Some(r) = scissor {
                pass.set_scissor_rect(0, 0, r.target.0, r.target.1);
            }
            if i == 0 {
                if let Some(stone) = &pipelines.stone_varieties
                    && self.stone_varieties_visible
                    && scale >= 2
                {
                    pass.set_pipeline(stone);
                    pass.draw(0..3, 0..1);
                }
                if let Some(film) = &pipelines.ground_film
                    && (self.algae.coat_visible || self.algae.rear_film_visible)
                {
                    pass.set_pipeline(film);
                    pass.draw(0..3, 0..1);
                }
            }
        }
        if self.sprite_count > 0 {
            pass.set_vertex_buffer(0, self.instances.slice(..));
            for p in &pipelines.sprites {
                pass.set_pipeline(p);
                pass.draw(0..6, 0..self.sprite_count);
            }
        }
        if front_pass {
            pass.set_pipeline(&self.front_film);
            pass.draw(0..3, 0..1);
        }
    }
}

/// Cells round the plants that the plant passes still shade: a swaying
/// leaf moves out of its cell, and a plant's cells all lie within its bounds.
const PLANT_MARGIN: usize = 4;

/// A pixel rectangle in a target of `target` pixels.
#[derive(Clone, Copy, Debug)]
struct PlantRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    target: (u32, u32),
}

/// Where the split grid's plant passes draw.
#[derive(Clone, Copy, Debug)]
enum PlantPasses {
    /// Over the whole view (the lens, whose layout differs from the tank's).
    Everywhere,
    /// Within the plants' rectangle, or not at all with no plants.
    Within(Option<PlantRect>),
}

#[derive(Debug)]
struct AlgaeTextures {
    coat: wgpu::Texture,
    glass: wgpu::Texture,
    /// Baked spot and slime patterns and variety colours (`algae.rs`).
    look_atlas: wgpu::Texture,
    coat_version: Option<u64>,
    glass_version: Option<u64>,
    coat_visible: bool,
    rear_film_visible: bool,
    front_film_visible: bool,
}

impl AlgaeTextures {
    fn new(gpu: &Gpu, grid: (u32, u32), fields: (u32, u32), world: &World) -> Self {
        let d = &gpu.device;
        let [coat, glass] = Self::textures(d, grid, fields);
        let look_atlas = create_grid_texture(
            d,
            "algae looks",
            algae::SIZE,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let rect = desktank_sim::Rect {
            x0: 0,
            y0: 0,
            x1: 2 * algae::TILE as usize,
            y1: algae::TILE as usize,
        };
        write_rect(gpu, &look_atlas, rect, &algae::bake_patterns(), 4);
        let mut textures = Self {
            coat,
            glass,
            look_atlas,
            coat_version: None,
            glass_version: None,
            coat_visible: false,
            rear_film_visible: false,
            front_film_visible: false,
        };
        textures.set_looks(gpu, world);
        textures
    }

    /// Uploads `world`'s variety looks into the atlas's colour columns.
    fn set_looks(&mut self, gpu: &Gpu, world: &World) {
        let x0 = 2 * algae::TILE as usize;
        let rect = desktank_sim::Rect {
            x0,
            y0: 0,
            x1: x0 + 2,
            y1: algae::VARIETIES,
        };
        let colours = algae::colours(world.algae_looks());
        write_rect(gpu, &self.look_atlas, rect, &colours, 4);
    }

    fn textures(d: &wgpu::Device, grid: (u32, u32), fields: (u32, u32)) -> [wgpu::Texture; 2] {
        [
            create_grid_texture(d, "algae coat", grid, wgpu::TextureFormat::R8Unorm),
            create_grid_texture(d, "glass film", fields, wgpu::TextureFormat::Rgba8Unorm),
        ]
    }

    fn upload(&mut self, gpu: &Gpu, world: &World, scratch: &mut Vec<u8>) {
        let version = world.coat_version();
        if self.coat_version != Some(version) {
            self.coat_version = Some(version);
            self.coat_visible = world.coated_cells() != 0;
            if self.coat_visible {
                let rect = desktank_sim::Rect {
                    x0: 0,
                    y0: 0,
                    x1: world.width(),
                    y1: world.height(),
                };
                write_rect(gpu, &self.coat, rect, world.coat(), 1);
            }
        }
        let version = world.glass_version();
        if self.glass_version == Some(version) {
            return;
        }
        self.glass_version = Some(version);
        let front = world.front_glass_algae();
        let back = world.back_glass_algae();
        let sides = world.side_glass_algae();
        self.front_film_visible = front.iter().any(|&v| v != 0);
        self.rear_film_visible =
            back.iter().any(|&v| v != 0) || sides.iter().any(|&row| row != [0; 2]);
        // Empty layers are ignored by the shader, so stale texels may stay.
        if self.front_film_visible || self.rear_film_visible {
            let width = world.field(FieldKind::Light).width();
            scratch.clear();
            for ((front, back), &[left, right]) in front
                .chunks_exact(width)
                .zip(back.chunks_exact(width))
                .zip(sides)
            {
                for (&front, &back) in front.iter().zip(back) {
                    scratch.extend([front, back, left, right]);
                }
            }
            let rect = desktank_sim::Rect {
                x0: 0,
                y0: 0,
                x1: width,
                y1: sides.len(),
            };
            write_rect(gpu, &self.glass, rect, scratch, 4);
        }
    }
}

/// A pass over `target`, cleared (every pixel is written, so there is
/// nothing to load).
fn begin_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'static> {
    encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes,
            occlusion_query_set: None,
            multiview_mask: None,
        })
        .forget_lifetime()
}

/// What the camera's post pass shows outside the tank's image, as the
/// target holds it (linear for an sRGB target) and premultiplied: the
/// letterbox colour (black in OLED mode), or nothing in overlay mode, where the letterbox is
/// clear.
fn camera_backdrop(
    format: wgpu::TextureFormat,
    overlay_water_alpha: Option<f32>,
    oled: bool,
) -> [f32; 4] {
    if overlay_water_alpha.is_some() {
        return [0.0; 4];
    }
    let [r, g, b, _] = backdrop(None, oled);
    let [r, g, b] = [r, g, b].map(|c| {
        if format.is_srgb() {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        } else {
            c
        }
    });
    [r, g, b, 1.0]
}

fn create_view_buffer(d: &wgpu::Device, label: &str) -> wgpu::Buffer {
    d.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: VIEW_BYTES,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Writes `texels` (`r`'s cells, row by row, `bytes` each) into `texture`
/// at `r`.
fn write_rect(
    gpu: &Gpu,
    texture: &wgpu::Texture,
    r: desktank_sim::Rect,
    texels: &[u8],
    bytes: usize,
) {
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
        texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(side((r.x1 - r.x0) * bytes)),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: side(r.x1 - r.x0),
            height: side(r.y1 - r.y0),
            depth_or_array_layers: 1,
        },
    );
}

/// A texture of one texel per grid cell, written by [`write_rect`].
fn create_grid_texture(
    d: &wgpu::Device,
    label: &str,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    d.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
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

/// How much of a field row's sway lean carries up to the row above.
const LEAN_CARRY: f32 = 0.8;

/// Per field cell (row-major, `width` across), how far swaying plants lean
/// downstream there, in -1..=1 of the full lean (`tank.wgsl` `sway_dx`).
/// A plant is pushed by the horizontal current along its height, so the
/// lean builds up from the floor through each field row's current
/// (normalized by `speed`) and fades slowly above it: tops above a jet stay
/// bent rather than springing back.
fn sway_lean(current: &[[f32; 2]], width: usize, speed: f32) -> Vec<f32> {
    let mut lean = vec![0.0; current.len()];
    if width == 0 || speed <= 0.0 {
        return lean;
    }
    let height = current.len() / width;
    for x in 0..width {
        let mut acc = 0.0_f32;
        for y in (0..height).rev() {
            let i = y * width + x;
            acc = (acc * LEAN_CARRY + current[i][0] / speed).clamp(-1.0, 1.0);
            lean[i] = acc;
        }
    }
    lean
}

/// The field-overlay textures: RGBA per field cell (`fields`) and RGBA per
/// field cell (`flow`: 2D current vector and sway lean, see [`sway_lean`]).
fn create_fields(d: &wgpu::Device, world: &World) -> (wgpu::Texture, wgpu::Texture, (u32, u32)) {
    let field = world.field(FieldKind::Nutrient);
    let size = (side(field.width()), side(field.height()));
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    };
    let texture = d.create_texture(&wgpu::TextureDescriptor {
        label: Some("fields"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let flow = d.create_texture(&wgpu::TextureDescriptor {
        label: Some("flow"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    (texture, flow, size)
}

/// The shared view and twelve textures.
fn create_bind_layout(d: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture_entry = |binding, filterable| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
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
            texture_entry(1, false),
            texture_entry(2, false),
            texture_entry(3, false),
            texture_entry(4, false),
            texture_entry(5, false),
            texture_entry(6, false),
            texture_entry(7, false),
            texture_entry(8, false),
            texture_entry(9, false),
            texture_entry(10, false),
            texture_entry(11, false),
            texture_entry(12, false),
        ],
    })
}

/// The bind group: `view` at binding 0, then `textures` at 1–12.
fn create_bind_group(
    d: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::Buffer,
    textures: [&wgpu::Texture; 12],
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

/// The HUD texture: the palette strip (top), the species card and the
/// inspect card (below), each sized for the largest.
fn create_hud(d: &wgpu::Device) -> wgpu::Texture {
    d.create_texture(&wgpu::TextureDescriptor {
        label: Some("hud"),
        size: wgpu::Extent3d {
            width: card::HUD_TEXTURE.0,
            height: card::HUD_TEXTURE.1,
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

/// The grid and sprite pipelines, drawn in order: full-screen triangles
/// for the grid, then the instanced sprites.
#[derive(Debug)]
struct Pipelines {
    grid: Vec<wgpu::RenderPipeline>,
    /// The split grid's dragon stone and Seiryu (`fs_stone_varieties`), drawn over what
    /// `fs_ground` leaves of them (it discards those texels) and before the algae and
    /// plants, only at scale 2 and up and while a cell holds one.
    stone_varieties: Option<wgpu::RenderPipeline>,
    /// The split grid's algae over the ground (`fs_ground_film`), drawn after `fs_ground`
    /// only while a coat or a rear or side film shows.
    ground_film: Option<wgpu::RenderPipeline>,
    sprites: Vec<wgpu::RenderPipeline>,
}

/// `fs_ground_film` blends the algae's colour and keeps the ground's alpha.
const FILM_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendState::ALPHA_BLENDING.color,
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// The grid's fragment entry points: `fs_main`, or with the split grid
/// `fs_ground` and then the plant passes, which blend over it (`fs_stone_varieties`
/// and `fs_ground_film` draw between them while they have something to draw).
const GRID_ENTRIES: &[&str] = &["fs_main"];
const SPLIT_GRID_ENTRIES: &[&str] = &["fs_ground", "fs_leaves", "fs_stems"];
/// The sprites' fragment entry points likewise; the split ones each draw
/// the pixels over one kind of plant part.
const SPRITE_ENTRIES: &[&str] = &["fs_sprite"];
const SPLIT_SPRITE_ENTRIES: &[&str] = &["fs_sprite_leaves", "fs_sprite_stems"];
/// The grid and sprite pipelines, plus the lens frame, front film and ruler
/// pipelines, drawing into `format`, one pass each or split ([`Gpu::split_grid`]),
/// and the shader and layout they were built from.
#[allow(clippy::too_many_lines)] // one pipeline per pass, built from one shader module
fn create_pipelines(
    gpu: &Gpu,
    bind_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> (
    Pipelines,
    [wgpu::RenderPipeline; 3],
    wgpu::ShaderModule,
    wgpu::PipelineLayout,
) {
    let d = &gpu.device;
    let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("tank"),
        source: wgpu::ShaderSource::Wgsl(include_str!("tank.wgsl").into()),
    });
    let pipeline_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("tank"),
        bind_group_layouts: &[Some(bind_layout)],
        immediate_size: 0,
    });
    // Per instance: top-left cell, size, atlas origin, mirror and turn,
    // frame size, look and fine atlas origin (vec2 each), then depth and
    // the night glow and pitch rotation with upright body size (vec4s).
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2,
        4 => Float32x2, 5 => Float32x2, 6 => Float32x2, 7 => Float32,
        8 => Float32x4, 9 => Float32x4
    ];
    let instances = [Some(wgpu::VertexBufferLayout {
        array_stride: INSTANCE_BYTES,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &attributes,
    })];
    // The grid is a full-screen triangle (`vs_main`, no buffers); sprites
    // are instanced quads (`vs_sprite`).
    let pipeline = |label: &str, entry: &str, blend, constants: &[(&str, f64)]| {
        let sprite = entry.starts_with("fs_sprite");
        d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(if sprite { "vs_sprite" } else { "vs_main" }),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: if sprite { &instances } else { &[] },
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let (grid_entries, sprite_entries) = if gpu.split_grid {
        (SPLIT_GRID_ENTRIES, SPLIT_SPRITE_ENTRIES)
    } else {
        (GRID_ENTRIES, SPRITE_ENTRIES)
    };
    let constants = [(
        "FRONT_IN_LAYERS",
        f64::from(front_in_layers(format, gpu.split_grid)),
    )];
    let pipelines = Pipelines {
        grid: grid_entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let blend = (i > 0).then_some(wgpu::BlendState::ALPHA_BLENDING);
                pipeline(&format!("tank {entry}"), entry, blend, &constants)
            })
            .collect(),
        stone_varieties: gpu
            .split_grid
            .then(|| pipeline("tank fs_stone_varieties", "fs_stone_varieties", None, &[])),
        ground_film: gpu.split_grid.then(|| {
            pipeline(
                "tank fs_ground_film",
                "fs_ground_film",
                Some(FILM_BLEND),
                &constants,
            )
        }),
        sprites: sprite_entries
            .iter()
            .map(|entry| {
                // Sprites blend only a glow's halo (alpha below 1); every
                // other sprite fragment is opaque or discarded.
                let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
                pipeline(&format!("sprites {entry}"), entry, blend, &constants)
            })
            .collect(),
    };
    let lens_frame = pipeline(
        "lens frame",
        "fs_lens_frame",
        Some(wgpu::BlendState::ALPHA_BLENDING),
        &[],
    );
    let front_film = pipeline(
        "front glass film",
        "fs_front_film",
        Some(wgpu::BlendState::ALPHA_BLENDING),
        &[],
    );
    let ruler = pipeline(
        "ruler",
        "fs_ruler",
        Some(wgpu::BlendState::ALPHA_BLENDING),
        &[],
    );
    (
        pipelines,
        [lens_frame, front_film, ruler],
        shader,
        pipeline_layout,
    )
}

/// Whether the grid and sprites apply the front film before encoding into
/// an sRGB `format` (see `FRONT_IN_LAYERS` in `tank.wgsl`). The split grid
/// has no room for it in `fs_ground`, so it draws the film's own pass.
fn front_in_layers(format: wgpu::TextureFormat, split_grid: bool) -> bool {
    format.is_srgb() && !split_grid
}

/// Whether OLED mode (true black water, air and letterbox) takes effect: only
/// on opaque tanks, never on the translucent click-through overlay.
fn oled_applies(overlay_water_alpha: Option<f32>, oled: bool) -> bool {
    oled && overlay_water_alpha.is_none()
}

/// The sRGB-encoded backdrop (black in OLED mode) and the overlay water
/// opacity (-1 for opaque mode).
fn backdrop(overlay_water_alpha: Option<f32>, oled: bool) -> [f32; 4] {
    let [r, g, b] = if oled_applies(overlay_water_alpha, oled) {
        [0.0; 3]
    } else {
        BACKDROP.map(|c| f32::from(c) / 255.0)
    };
    [r, g, b, overlay_water_alpha.unwrap_or(-1.0)]
}

/// A grid or texture side as `u32`. Sides are at most
/// [`desktank_sim::MAX_SIDE`], so this never saturates in practice.
fn side(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    #[test]
    fn oled_mode_blacks_the_backdrop_only_on_opaque_tanks() {
        assert_eq!(super::backdrop(None, true), [0.0, 0.0, 0.0, -1.0]);
        let normal = super::backdrop(None, false);
        assert!(normal[..3].iter().any(|&c| c > 0.0), "{normal:?}");
        // The translucent overlay ignores OLED mode.
        assert_eq!(super::backdrop(Some(0.5), true), {
            let [r, g, b, _] = normal;
            [r, g, b, 0.5]
        });
        assert!(super::oled_applies(None, true));
        assert!(!super::oled_applies(Some(0.5), true));
        assert!(!super::oled_applies(None, false));
    }

    #[test]
    fn sway_lean_builds_up_through_a_jet_and_fades_above_it() {
        // One column of 6 field rows, a rightward jet in row 3 only.
        let mut current = vec![[0.0, 0.0]; 6];
        current[3] = [0.5, 0.0];
        let lean = super::sway_lean(&current, 1, 1.0);
        assert_eq!(&lean[4..], &[0.0, 0.0], "still below the jet");
        assert!((lean[3] - 0.5).abs() < 1e-6);
        assert!(lean[2] > 0.3 && lean[2] < lean[3], "fading above: {lean:?}");
        assert!(lean[0] > 0.0 && lean[0] < lean[2]);
        // Reversed flow reverses the lean; strong flow saturates at 1.
        current[3] = [-0.5, 0.0];
        assert!(super::sway_lean(&current, 1, 1.0)[2] < -0.3);
        current[3] = [3.0, 0.0];
        assert!((super::sway_lean(&current, 1, 1.0)[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_shader_knows_the_back_band() {
        let back = format!(
            "const BACK_BAND: f32 = {}.0;",
            desktank_sim::DEPTH_BANDS - 1
        );
        assert!(include_str!("tank.wgsl").contains(&back), "{back}");
    }

    #[test]
    fn the_shader_knows_the_glow_alphas() {
        let wgsl = include_str!("tank.wgsl");
        for (name, alpha) in [
            ("ALPHA_EYE", crate::sprites::ALPHA_EYE),
            ("ALPHA_UNLIT", crate::sprites::ALPHA_UNLIT),
        ] {
            let line = format!("const {name}: f32 = {alpha}.0;");
            assert!(wgsl.contains(&line), "{line}");
        }
        assert!(wgsl.contains("@location(8) glow: vec4<f32>"));
    }

    #[test]
    fn the_shader_knows_the_ground_styles() {
        let wgsl = include_str!("tank.wgsl");
        for (name, style) in crate::palette::GroundStyle::ALL {
            let line = format!("const {name}: i32 = {};", style as u8);
            assert!(wgsl.contains(&line), "{line}");
        }
    }
}
