//! The camera's GPU side (D29): the offscreen image the tank draws into
//! while a camera is set, and the post pass that resamples a view of it
//! over the target (`camera.wgsl`).
//!
//! The image has the target's size and format, so the tank's own pipelines
//! draw into it unchanged and a pixel survives the round trip: an sRGB
//! image reads back linear and the sRGB target encodes it again. Only a
//! standard sampled texture and a render pass are used (WebGL2-safe).

use crate::Gpu;
use crate::camera::ViewRect;
use crate::layout::Layout;

/// Size of the shader's `Post` uniform block, in bytes (three vec4s).
const UNIFORM_BYTES: u64 = 48;

/// The post pass: its pipeline and uniform, built when a camera is first
/// set and kept for the next one.
#[derive(Debug)]
pub(crate) struct Post {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    /// Uniform bytes last written.
    written: Vec<u8>,
}

impl Post {
    pub(crate) fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Self {
        let d = &gpu.device;
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("camera"),
            source: wgpu::ShaderSource::Wgsl(include_str!("camera.wgsl").into()),
        });
        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("camera"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("camera post"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_post"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_post"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Every pixel is written.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniform = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera post"),
            size: UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            layout,
            uniform,
            written: Vec::new(),
        }
    }

    pub(crate) fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    /// Writes the uniform for showing `view` of the `layout`-placed tank
    /// over `size` target pixels, with `backdrop` outside the image, unless
    /// it is what is already there.
    pub(crate) fn update(
        &mut self,
        gpu: &Gpu,
        view: ViewRect,
        layout: &Layout,
        size: (u32, u32),
        backdrop: [f32; 4],
    ) {
        let bytes = uniform_bytes(view, layout, size, backdrop);
        if bytes != self.written {
            gpu.queue.write_buffer(&self.uniform, 0, &bytes);
            self.written = bytes;
        }
    }
}

/// The `Post` uniform block: the source pixel under the target's top-left
/// corner and source pixels per output pixel, then the inverse of those
/// and the source size, then the backdrop.
#[allow(clippy::cast_precision_loss)] // pixels and cells, far below 2^24
fn uniform_bytes(view: ViewRect, layout: &Layout, size: (u32, u32), backdrop: [f32; 4]) -> Vec<u8> {
    let scale = layout.scale as f32;
    let step = (
        view.w * scale / size.0 as f32,
        view.h * scale / size.1 as f32,
    );
    let floats = [
        view.x * scale + layout.offset.0 as f32,
        view.y * scale + layout.offset.1 as f32,
        step.0,
        step.1,
        1.0 / step.0,
        1.0 / step.1,
        size.0 as f32,
        size.1 as f32,
        backdrop[0],
        backdrop[1],
        backdrop[2],
        backdrop[3],
    ];
    floats.into_iter().flat_map(f32::to_le_bytes).collect()
}

/// The image the tank draws into while a camera is set, and the bind group
/// the post pass reads it with.
#[derive(Debug)]
pub(crate) struct Offscreen {
    size: (u32, u32),
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

impl Offscreen {
    pub(crate) fn new(
        gpu: &Gpu,
        post: &Post,
        format: wgpu::TextureFormat,
        size: (u32, u32),
    ) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("camera image"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &post.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: post.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        Self {
            size,
            view,
            bind_group,
        }
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        self.size
    }

    /// What the tank draws into.
    pub(crate) fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// What the post pass binds.
    pub(crate) fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floats(bytes: &[u8]) -> Vec<f32> {
        assert_eq!(u64::try_from(bytes.len()), Ok(UNIFORM_BYTES));
        bytes
            .chunks(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    #[test]
    fn the_fit_view_reads_the_image_one_to_one() {
        let target = (2000, 1500);
        let layout = Layout::fit((320, 180), target);
        let view = ViewRect::fit(&layout, target);
        let f = floats(&uniform_bytes(view, &layout, target, [0.0; 4]));
        assert!(f[0].abs() < 1e-3 && f[1].abs() < 1e-3, "{f:?}");
        assert!(
            (f[2] - 1.0).abs() < 1e-6 && (f[3] - 1.0).abs() < 1e-6,
            "{f:?}"
        );
        assert!(
            (f[4] - 1.0).abs() < 1e-6 && (f[5] - 1.0).abs() < 1e-6,
            "{f:?}"
        );
        assert_eq!((f[6], f[7]), (2000.0, 1500.0));
    }

    #[test]
    fn a_zoomed_view_reads_a_smaller_part_of_the_image_and_magnifies_it() {
        let target = (1280, 720);
        let layout = Layout::fit((320, 180), target); // scale 4
        let view = ViewRect {
            x: 100.0,
            y: 50.0,
            w: 128.0,
            h: 72.0,
        };
        let f = floats(&uniform_bytes(view, &layout, target, [0.0; 4]));
        assert_eq!((f[0], f[1]), (400.0, 200.0));
        // 512 source pixels over 1280 output pixels: 2.5 output pixels each.
        assert!((f[2] - 0.4).abs() < 1e-6, "{f:?}");
        assert!((f[4] - 2.5).abs() < 1e-5, "{f:?}");
    }
}
