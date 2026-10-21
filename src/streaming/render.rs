use crate::{project::Project, runtime::SpriteState};
use anyhow::{Context, Result, ensure};
use wgpu::util::DeviceExt;
const WIDTH: u32 = 480;
const HEIGHT: u32 = 360;
const ROW: u32 = 2048;
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
}
struct Costume {
    bind: wgpu::BindGroup,
    width: f64,
    height: f64,
    center: [f64; 2],
    resolution: f64,
}
pub(super) struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    target: wgpu::Texture,
    readback: wgpu::Buffer,
    vertices: wgpu::Buffer,
    costumes: Vec<Vec<Costume>>,
    order: Vec<usize>,
}
impl Renderer {
    pub async fn new(project: &Project) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .context("no wgpu graphics adapter available")?;
        eprintln!(
            "Graphics: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend
        );
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Scratch renderer"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: Default::default(),
                },
                None,
            )
            .await?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Costume shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2],
                }],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut costumes = vec![];
        let mut pixel_budget = 0u64;
        for target in &project.targets {
            let mut list = vec![];
            for costume in &target.costumes {
                let bytes = project.assets.get(&costume.md5ext).with_context(|| {
                    format!(
                        "costume {} has no asset; streaming requires an .sb3 archive",
                        costume.md5ext
                    )
                })?;
                let (width, height, rgba) = decode(&costume.md5ext, bytes)?;
                ensure!(
                    width <= device.limits().max_texture_dimension_2d
                        && height <= device.limits().max_texture_dimension_2d,
                    "costume exceeds this adapter's texture dimension limit"
                );
                pixel_budget += width as u64 * height as u64 * 4;
                ensure!(
                    pixel_budget <= 512 * 1024 * 1024,
                    "decoded costumes exceed 512 MiB"
                );
                let texture = device.create_texture_with_data(
                    &queue,
                    &wgpu::TextureDescriptor {
                        label: Some(&costume.name),
                        size: wgpu::Extent3d {
                            width,
                            height,
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
                    &rgba,
                );
                let view = texture.create_view(&Default::default());
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                });
                list.push(Costume {
                    bind,
                    width: width as f64,
                    height: height as f64,
                    center: [costume.rotation_center_x, costume.rotation_center_y],
                    resolution: costume.bitmap_resolution,
                });
            }
            costumes.push(list);
        }
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Scratch stage"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("portable capture readback"),
            size: ROW as u64 * HEIGHT as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (project.targets.len().max(1) * 6 * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut order: Vec<_> = (0..project.targets.len()).collect();
        order.sort_by_key(|i| {
            (
                !project.targets[*i].is_stage,
                project.targets[*i].layer_order,
            )
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            target,
            readback,
            vertices,
            costumes,
            order,
        })
    }
    pub fn capture(&mut self, states: &[SpriteState]) -> Result<Vec<u8>> {
        let mut vertices = Vec::with_capacity(states.len() * 6);
        let mut draws = vec![];
        for &id in &self.order {
            let state = &states[id];
            if !state.is_stage && !state.visible {
                continue;
            }
            let Some(costume) = self.costumes[id].get(state.costume) else {
                continue;
            };
            let start = vertices.len() as u32;
            let scale = if state.is_stage {
                1.0
            } else {
                state.size / 100.0
            } / costume.resolution;
            let angle = if state.is_stage || state.rotation_style != "all around" {
                0.0
            } else {
                (90.0 - state.direction).to_radians()
            };
            let flip = if state.rotation_style == "left-right" && state.direction < 0.0 {
                -1.0
            } else {
                1.0
            };
            for [u, v] in [[0., 0.], [1., 0.], [0., 1.], [0., 1.], [1., 0.], [1., 1.]] {
                let x = (u * costume.width - costume.center[0]) * scale * flip;
                let y = (costume.center[1] - v * costume.height) * scale;
                let (sin, cos) = angle.sin_cos();
                let px = if state.is_stage { 0.0 } else { state.x } + x * cos - y * sin;
                let py = if state.is_stage { 0.0 } else { state.y } + x * sin + y * cos;
                vertices.push(Vertex {
                    position: [(px / 240.0) as f32, (py / 180.0) as f32],
                    uv: [u as f32, v as f32],
                });
            }
            draws.push((id, state.costume, start));
        }
        if !vertices.is_empty() {
            self.queue
                .write_buffer(&self.vertices, 0, bytemuck::cast_slice(&vertices));
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let view = self.target.create_view(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            for (id, costume, start) in draws {
                pass.set_bind_group(0, &self.costumes[id][costume].bind, &[]);
                pass.draw(start..start + 6, 0..1);
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: Default::default(),
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ROW),
                    rows_per_image: Some(HEIGHT),
                },
            },
            wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv().context("GPU readback callback lost")??;
        let mapped = slice.get_mapped_range();
        let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
        for row in mapped.chunks(ROW as usize) {
            rgba.extend_from_slice(&row[..(WIDTH * 4) as usize]);
        }
        drop(mapped);
        self.readback.unmap();
        Ok(rgba)
    }
}
fn decode(name: &str, data: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    if name.to_ascii_lowercase().ends_with(".svg") {
        let mut options = resvg::usvg::Options::default();
        options.image_href_resolver.resolve_string = Box::new(|_, _| None);
        let tree = resvg::usvg::Tree::from_data(data, &options).context("parse SVG costume")?;
        let size = tree.size().to_int_size();
        ensure!(
            size.width() <= 4096 && size.height() <= 4096,
            "SVG costume exceeds 4096 pixels"
        );
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
            .context("allocate SVG costume")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        Ok((size.width(), size.height(), pixmap.data().to_vec()))
    } else {
        let mut reader =
            image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode().context("decode bitmap costume")?.to_rgba8();
        let (w, h) = image.dimensions();
        let mut rgba = image.into_raw();
        for p in rgba.chunks_exact_mut(4) {
            for c in 0..3 {
                p[c] = ((p[c] as u16 * p[3] as u16 + 127) / 255) as u8;
            }
        }
        Ok((w, h, rgba))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires a wgpu graphics adapter"]
    async fn gpu_capture_contains_backdrop_and_costume() -> Result<()> {
        let project = std::sync::Arc::new(Project::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/demo.sb3"
        ))?);
        let report = crate::runtime::spawn(
            project.clone(),
            crate::runtime::RuntimeOptions {
                max_steps: Some(0),
                ..Default::default()
            },
        )?
        .join()?;
        let mut renderer = Renderer::new(&project).await?;
        let rgba = renderer.capture(&report.sprites)?;
        let pixel = |x: usize, y: usize| &rgba[(y * 480 + x) * 4..(y * 480 + x) * 4 + 4];
        assert_eq!(pixel(300, 100), &[232, 238, 248, 255]);
        assert_eq!(pixel(40, 180), &[99, 102, 241, 255]);
        Ok(())
    }
}
