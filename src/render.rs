use crate::{
    pen::{PenCommand, PenLine},
    project::Project,
    runtime::{SharedScene, SpriteState},
};
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
    shape: [f32; 4],
    color: [f32; 4],
    skin_size: [f32; 2],
    effect_mask: u32,
}
struct Costume {
    bind: wgpu::BindGroup,
    nearest_bind: wgpu::BindGroup,
    svg: bool,
    width: f64,
    height: f64,
    center: [f64; 2],
    resolution: f64,
    alpha: Vec<u8>,
}
impl Costume {
    fn nearest(&self, state: &SpriteState) -> bool {
        !self.svg
            || (!state.effects.changes_shape()
                && (state.is_stage
                    || state.rotation_style != "all around"
                    || state.direction % 90.0 == 0.0)
                && (state.size - 100.0).abs() < 0.001)
    }
    fn opaque(&self, uv: [f64; 2], state: &SpriteState) -> bool {
        if !uv.into_iter().all(f64::is_finite) {
            return false;
        }
        let read = |x: f64, y: f64| {
            let x = x.clamp(0.0, self.width - 1.0) as usize;
            let y = y.clamp(0.0, self.height - 1.0) as usize;
            self.alpha[y * self.width as usize + x] > 0
        };
        if self.nearest(state) {
            return read((uv[0] * self.width).floor(), (uv[1] * self.height).floor());
        }
        let (x, y) = (uv[0] * self.width - 0.5, uv[1] * self.height - 0.5);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        read(x.floor(), y.floor()) && (1.0 - fx) * (1.0 - fy) > 0.0
            || read(x.floor() + 1.0, y.floor()) && fx * (1.0 - fy) > 0.0
            || read(x.floor(), y.floor() + 1.0) && (1.0 - fx) * fy > 0.0
            || read(x.floor() + 1.0, y.floor() + 1.0) && fx * fy > 0.0
    }
}
pub(crate) struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    target: wgpu::Texture,
    readback: wgpu::Buffer,
    vertices: wgpu::Buffer,
    costumes: Vec<Vec<Costume>>,
    pen_texture: wgpu::Texture,
    pen_bind: wgpu::BindGroup,
    pen_generation: u64,
    pen_pipeline: wgpu::RenderPipeline,
    pen_overlay_layout: wgpu::BindGroupLayout,
    pen_sampler: wgpu::Sampler,
    pen_quad: wgpu::Buffer,
    monitors: crate::monitors::display::Overlay,
}
fn draw_order(states: &[SpriteState]) -> Vec<usize> {
    let mut order: Vec<_> = states
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.active.then_some(i))
        .collect();
    order.sort_by_key(|&i| (!states[i].is_stage, states[i].layer_order));
    order
}
impl Renderer {
    pub async fn new(project: &Project) -> Result<Self> {
        // Avoid loading optional Vulkan ICDs when opening a Windows stage.
        // A broken installed ICD can crash inside driver enumeration before
        // wgpu has an opportunity to return an adapter error.
        let descriptor = wgpu::InstanceDescriptor {
            backends: if cfg!(target_os = "windows") {
                wgpu::Backends::DX12
            } else {
                wgpu::Backends::all()
            },
            ..Default::default()
        }
        .with_env();
        let instance = wgpu::Instance::new(&descriptor);
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
                    attributes: &wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2,2=>Float32x4,3=>Float32x4,4=>Float32x2,5=>Uint32],
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
        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let pen_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Pen capsule shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pen.wgsl").into()),
        });
        let pen_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Instanced pen strokes"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &pen_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<PenLine>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32],
                }],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &pen_shader,
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
        let quad: Vec<_> = [[0., 0.], [1., 0.], [0., 1.], [0., 1.], [1., 0.], [1., 1.]]
            .into_iter()
            .map(|[u, v]| Vertex {
                position: [u * 2. - 1., 1. - v * 2.],
                uv: [u, v],
                shape: [0., 1., 0., 0.],
                color: [1., 0., 1., 0.],
                skin_size: [480., 360.],
                effect_mask: 0,
            })
            .collect();
        let pen_quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Pen stamp quad"),
            contents: bytemuck::cast_slice(&quad),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let pen_texture = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("Persistent pen layer"),
                size: wgpu::Extent3d {
                    width: WIDTH,
                    height: HEIGHT,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &vec![0; (WIDTH * HEIGHT * 4) as usize],
        );
        let pen_view = pen_texture.create_view(&Default::default());
        let pen_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Pen layer"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&pen_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let mut costumes = vec![];
        let mut pixel_budget = 0u64;
        for target in &project.targets {
            let mut list = vec![];
            for costume in &target.costumes {
                let bytes = project.assets.get(&costume.md5ext).with_context(|| {
                    format!(
                        "costume {} has no asset; rendering requires an .sb3 archive",
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
                let nearest_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&nearest_sampler),
                        },
                    ],
                });
                list.push(Costume {
                    bind,
                    nearest_bind,
                    svg: costume.md5ext.to_ascii_lowercase().ends_with(".svg"),
                    width: width as f64,
                    height: height as f64,
                    center: [costume.rotation_center_x, costume.rotation_center_y],
                    resolution: costume.bitmap_resolution,
                    alpha: rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect(),
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
            size: ((project.targets.len() + crate::runtime::MAX_CLONES + 1)
                * 6
                * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            target,
            readback,
            vertices,
            costumes,
            pen_texture,
            pen_bind,
            pen_generation: u64::MAX,
            pen_pipeline,
            pen_overlay_layout: layout,
            pen_sampler: nearest_sampler,
            pen_quad,
            monitors: Default::default(),
        })
    }
    /// Pick the frontmost visible, nontransparent costume pixel. Transform the
    /// point by the inverse of the same rotation/size/flip used for drawing.
    pub fn hit_test(&self, states: &[SpriteState], x: f64, y: f64) -> Option<usize> {
        if !x.is_finite() || !y.is_finite() || x.abs() > 240.0 || y.abs() > 180.0 {
            return None;
        }
        for id in draw_order(states).into_iter().rev() {
            let state = &states[id];
            if state.is_stage {
                return Some(state.id);
            }
            if !state.visible || state.effects.ghost >= 100.0 {
                continue;
            }
            let Some(costume) = self.costumes[state.target].get(state.costume) else {
                continue;
            };
            let scale = state.size / 100.0 / costume.resolution;
            if !scale.is_finite() || scale <= 0.0 {
                continue;
            }
            let angle = if state.rotation_style == "all around" {
                (90.0 - state.direction).to_radians()
            } else {
                0.0
            };
            let (sin, cos) = angle.sin_cos();
            let (dx, dy) = (x - state.x, y - state.y);
            let flip = if state.rotation_style == "left-right" && state.direction < 0.0 {
                -1.0
            } else {
                1.0
            };
            let px = (dx * cos + dy * sin) / scale / flip + costume.center[0];
            let py = costume.center[1] - (-dx * sin + dy * cos) / scale;
            if px < 0.0 || py < 0.0 || px >= costume.width || py >= costume.height {
                continue;
            }
            let uv = state.effects.point(
                [px / costume.width, py / costume.height],
                [
                    costume.width / costume.resolution,
                    costume.height / costume.resolution,
                ],
            );
            if costume.opaque(uv, state) {
                return Some(state.id);
            }
        }
        None
    }
    pub fn capture_scene(
        &mut self,
        scene: &SharedScene,
        states: &[SpriteState],
    ) -> Result<Vec<u8>> {
        let (update, commands) = {
            let mut layer = scene.pen.lock().unwrap_or_else(|e| e.into_inner());
            let update = (!layer.deferred && layer.generation != self.pen_generation)
                .then(|| (layer.generation, layer.pixels.clone()));
            (update, layer.take_commands())
        };
        if let Some((generation, mut pixels)) = update {
            pixels.resize((WIDTH * HEIGHT * 4) as usize, 0);
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.pen_texture,
                    mip_level: 0,
                    origin: Default::default(),
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(WIDTH * 4),
                    rows_per_image: Some(HEIGHT),
                },
                wgpu::Extent3d {
                    width: WIDTH,
                    height: HEIGHT,
                    depth_or_array_layers: 1,
                },
            );
            self.pen_generation = generation;
        }
        self.draw_pen(commands);
        let mut rgba = self.capture(states)?;
        self.monitors.composite(scene, &mut rgba);
        crate::monitors::display::question_overlay(scene, &mut rgba);
        Ok(rgba)
    }
    fn draw_pen(&self, commands: Vec<PenCommand>) {
        if commands.is_empty() {
            return;
        }
        let lines: Vec<_> = commands
            .iter()
            .filter_map(|c| match c {
                PenCommand::Line(line) => Some(*line),
                _ => None,
            })
            .collect();
        let instances = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Pen stroke instances"),
                contents: bytemuck::cast_slice(&lines),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let view = self.pen_texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut cursor = 0;
        let mut line_index = 0;
        while cursor < commands.len() {
            let clear = matches!(commands[cursor], PenCommand::Clear);
            if clear {
                cursor += 1;
            }
            let first = line_index;
            while cursor < commands.len() && matches!(commands[cursor], PenCommand::Line(_)) {
                cursor += 1;
                line_index += 1;
            }
            if clear || first != line_index {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Pen strokes"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: if clear {
                                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                            } else {
                                wgpu::LoadOp::Load
                            },
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                if first != line_index {
                    pass.set_pipeline(&self.pen_pipeline);
                    pass.set_vertex_buffer(0, instances.slice(..));
                    pass.draw(0..6, first..line_index);
                }
            }
            if let Some(PenCommand::Image(pixels)) = commands.get(cursor) {
                let texture = self.device.create_texture_with_data(
                    &self.queue,
                    &wgpu::TextureDescriptor {
                        label: Some("Pen stamp overlay"),
                        size: wgpu::Extent3d {
                            width: WIDTH,
                            height: HEIGHT,
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
                    pixels,
                );
                let stamp_view = texture.create_view(&Default::default());
                let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Pen stamp overlay"),
                    layout: &self.pen_overlay_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&stamp_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.pen_sampler),
                        },
                    ],
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Pen stamp"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, self.pen_quad.slice(..));
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..6, 0..1);
                cursor += 1;
            }
        }
        self.queue.submit(Some(encoder.finish()));
    }
    fn capture(&mut self, states: &[SpriteState]) -> Result<Vec<u8>> {
        let mut vertices = Vec::with_capacity((states.len() + 1) * 6);
        for [u, v] in [[0., 0.], [1., 0.], [0., 1.], [0., 1.], [1., 0.], [1., 1.]] {
            vertices.push(Vertex {
                position: [u * 2.0 - 1.0, 1.0 - v * 2.0],
                uv: [u, v],
                shape: [0.0, 1.0, 0.0, 0.0],
                color: [1.0, 0.0, 1.0, 0.0],
                skin_size: [480.0, 360.0],
                effect_mask: 0,
            });
        }
        let mut draws = vec![];
        for id in draw_order(states) {
            let state = &states[id];
            if !state.is_stage && !state.visible {
                continue;
            }
            let Some(costume) = self.costumes[state.target].get(state.costume) else {
                continue;
            };
            let start = vertices.len() as u32;
            let (shape, color) = state.effects.uniforms();
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
                    shape,
                    color,
                    skin_size: [
                        (costume.width / costume.resolution) as f32,
                        (costume.height / costume.resolution) as f32,
                    ],
                    effect_mask: state.effects.mask(),
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
            let mut pen_drawn = false;
            for (id, costume, start) in draws {
                if !states[id].is_stage && !pen_drawn {
                    pass.set_bind_group(0, &self.pen_bind, &[]);
                    pass.draw(0..6, 0..1);
                    pen_drawn = true;
                }
                let c = &self.costumes[states[id].target][costume];
                let state = &states[id];
                pass.set_bind_group(
                    0,
                    if c.nearest(state) {
                        &c.nearest_bind
                    } else {
                        &c.bind
                    },
                    &[],
                );
                pass.draw(start..start + 6, 0..1);
            }
            if !pen_drawn {
                pass.set_bind_group(0, &self.pen_bind, &[]);
                pass.draw(0..6, 0..1);
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
pub(crate) fn decode(name: &str, data: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    if name.to_ascii_lowercase().ends_with(".svg") {
        let mut options = resvg::usvg::Options::default();
        // Keep costume text available on headless hosts too. Never depend on
        // installed fonts or resolve external resources from project SVGs.
        static FONTS: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> =
            std::sync::OnceLock::new();
        options.fontdb = FONTS
            .get_or_init(|| {
                let mut db = resvg::usvg::fontdb::Database::new();
                db.load_font_data(include_bytes!("../assets/fonts/Arimo-Regular.ttf").to_vec());
                db.load_font_data(include_bytes!("../assets/fonts/Arimo-Bold.ttf").to_vec());
                db.load_font_data(include_bytes!("../assets/fonts/Knewave.ttf").to_vec());
                db.set_sans_serif_family("Arimo");
                db.set_serif_family("Arimo");
                std::sync::Arc::new(db)
            })
            .clone();
        options.font_family = "Arimo".into();
        let select = resvg::usvg::FontResolver::default_font_selector();
        options.font_resolver.select_font = Box::new(move |font, db| {
            if font
                .families()
                .iter()
                .any(|f| f.to_string().trim_matches(['\'', '"']) == "Marker")
            {
                return db.query(&resvg::usvg::fontdb::Query {
                    families: &[resvg::usvg::fontdb::Family::Name("Knewave")],
                    ..Default::default()
                });
            }
            select(font, db)
        });
        options.image_href_resolver.resolve_string = Box::new(|_, _| None);
        let tree = match resvg::usvg::Tree::from_data(data, &options) {
            Ok(tree) => tree,
            // Scratch projects commonly retain empty, zero-sized SVG costumes.
            Err(resvg::usvg::Error::InvalidSize) => return Ok((1, 1, vec![0; 4])),
            Err(error) => return Err(error).context("parse SVG costume"),
        };
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
        for p in rgba.as_chunks_mut::<4>().0 {
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
    #[test]
    #[ignore = "requires MSBJIT_MNTOS_PROJECT and a wgpu adapter"]
    fn mntos_list_terminal_boots_and_renders() -> Result<()> {
        use crate::{
            input::InputEvent,
            runtime::{self, RuntimeOptions},
        };
        use std::{
            sync::Arc,
            time::{Duration, Instant},
        };
        let project = Arc::new(Project::load(std::env::var("MSBJIT_MNTOS_PROJECT")?)?);
        let mut renderer = pollster::block_on(Renderer::new(&project))?;
        let run = runtime::spawn_interactive(
            project.clone(),
            RuntimeOptions {
                turbo: true,
                time_limit: Some(Duration::from_secs(120)),
                ..Default::default()
            },
        )?;
        let scene = run.scene.clone();
        scene.enable_gpu_pen();
        // Observe the project's keyboard-input mode without changing its data.
        for m in scene
            .monitors
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|m| m.label == "getting_input")
        {
            m.visible = true;
        }
        scene.input.send(InputEvent::Flag);
        let deadline = Instant::now() + Duration::from_secs(150);
        let mut last_question = None;
        let mut enabled_output = false;
        let mut typed_command = false;
        loop {
            // The host must consume GPU work during boot too: font stamps can
            // otherwise fill the bounded pen queue before the list is shown.
            renderer.capture_scene(&scene, &scene.snapshot(&project))?;
            let question = scene.question.lock().unwrap().clone();
            if let Some(question) = question {
                if last_question != Some(question.id) {
                    eprintln!("MNTOS prompt: {}", question.text);
                    last_question = Some(question.id);
                }
                let response = if question.text.contains("new input type") {
                    Some("0")
                } else if question.text.starts_with("mint-os:") && !enabled_output {
                    enabled_output = true;
                    Some("set visout 1")
                } else {
                    None
                };
                if let Some(text) = response {
                    scene.input.send(InputEvent::Answer {
                        id: question.id,
                        text: text.into(),
                    });
                }
            }
            if !typed_command
                && scene.question.lock().unwrap().is_none()
                && scene
                    .monitors
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|m| m.label == "getting_input" && m.number == 1.)
            {
                typed_command = true;
                for key in [
                    "s", "e", "t", "space", "v", "i", "s", "o", "u", "t", "space", "1", "enter",
                ] {
                    scene.input.send(InputEvent::Key {
                        key: key.into(),
                        down: true,
                    });
                    std::thread::sleep(Duration::from_millis(80));
                    scene.input.send(InputEvent::Key {
                        key: key.into(),
                        down: false,
                    });
                    std::thread::sleep(Duration::from_millis(80));
                }
            }
            let ready = scene.monitors.lock().unwrap().iter().any(|m| {
                m.label == "output" && m.visible && m.list.as_ref().is_some_and(|l| l.len > 0)
            });
            if ready {
                break;
            }
            if run.is_finished() {
                let report = run.join()?;
                eprintln!(
                    "output list: {:?}",
                    report
                        .lists
                        .iter()
                        .find(|l| l.name == "output")
                        .map(|l| &l.items)
                );
                anyhow::bail!("MNTOS stopped before its list terminal appeared");
            }
            ensure!(Instant::now() < deadline, "MNTOS terminal did not appear");
            std::thread::sleep(Duration::from_millis(10));
        }
        for m in scene
            .monitors
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|m| m.label == "getting_input")
        {
            m.visible = false;
        }
        let image = renderer.capture_scene(&scene, &scene.snapshot(&project))?;
        ensure!(
            image
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[..3] == [255, 102, 26]),
            "terminal rows were not drawn"
        );
        if let Ok(path) = std::env::var("MSBJIT_MNTOS_CAPTURE") {
            image::save_buffer(path, &image, WIDTH, HEIGHT, image::ColorType::Rgba8)?;
        }
        run.stop();
        let report = run.join()?;
        eprintln!("MNTOS terminal initialized: {} native steps", report.steps);
        Ok(())
    }
    #[test]
    #[ignore = "requires a wgpu adapter"]
    fn gpu_pen_matches_cpu_capsules_and_stamp_order() -> Result<()> {
        use crate::{
            pen::{PenCanvas, PenState},
            runtime::{self, RuntimeOptions},
            value::ScratchValue,
        };
        let project = std::sync::Arc::new(serde_json::from_value::<Project>(
            serde_json::json!({"targets":[{"name":"Stage","isStage":true}]}),
        )?);
        let running = runtime::spawn(project.clone(), RuntimeOptions::default())?;
        let scene = running.scene.clone();
        let report = running.join()?;
        let mut renderer = pollster::block_on(Renderer::new(&project))?;
        let mut cpu = PenCanvas::default();
        let mut gpu = PenCanvas::default();
        gpu.enable_gpu();
        let mut pen = PenState::default();
        for (size, color, transparency, from, to) in [
            (12., "#ff0000", 0., (-220., -140.), (190., 125.)),
            (6., "#00ff00", 50., (-180., 140.), (160., -120.)),
            (37., "#0000ff", 25., (0., 0.), (0., 0.)),
            (1., "#000000", 0., (-230., 70.), (220., 70.)),
            (3., "#ff00ff", 0., (-10000., -20.), (10000., -20.)),
            (1200., "#ffaa00", 90., (-250., -190.), (-250., -190.)),
        ] {
            pen.size = size;
            pen.set_color(&ScratchValue::String(color.into()));
            pen.parameter("transparency", transparency, false);
            cpu.line(&pen, from, to);
            gpu.line(&pen, from, to);
        }
        cpu.publish_to(&mut scene.pen.lock().unwrap());
        let reference = renderer.capture_scene(&scene, &report.sprites)?;
        gpu.publish_to(&mut scene.pen.lock().unwrap());
        let actual = renderer.capture_scene(&scene, &report.sprites)?;
        let max_error = reference
            .iter()
            .zip(&actual)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(max_error <= 3, "CPU/GPU capsule error: {max_error}");
        let mut stamp = vec![0; (WIDTH * HEIGHT * 4) as usize];
        let center = (180 * WIDTH as usize + 240) * 4;
        stamp[center..center + 4].copy_from_slice(&[128, 0, 0, 128]);
        gpu.clear();
        gpu.stamp_image(stamp);
        gpu.publish_to(&mut scene.pen.lock().unwrap());
        let stamped = renderer.capture_scene(&scene, &report.sprites)?;
        assert_eq!(&stamped[center..center + 4], &[255, 127, 127, 255]);
        assert_eq!(&stamped[..4], &[255; 4]);
        gpu.clear();
        gpu.publish_to(&mut scene.pen.lock().unwrap());
        assert!(
            renderer
                .capture_scene(&scene, &report.sprites)?
                .iter()
                .all(|v| *v == 255)
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires MSBJIT_PEN_BENCH_PROJECT and a wgpu adapter; use release mode"]
    fn sub1k_complete_frames() -> Result<()> {
        use crate::{
            runtime::{self, RuntimeOptions},
            value::ScratchValue,
        };
        use std::{
            sync::Arc,
            time::{Duration, Instant},
        };
        let path = std::env::var("MSBJIT_PEN_BENCH_PROJECT")?;
        let project = Arc::new(Project::load(path)?);
        let expected: u32 = std::env::var("MSBJIT_PEN_BENCH_FRAMES")
            .unwrap_or("30".into())
            .parse()?;
        let mut renderer = pollster::block_on(Renderer::new(&project))?;
        let mut samples = Vec::new();
        let mut reference = None;
        for pass in 0..6 {
            let gpu = if pass % 4 < 2 {
                pass % 2 == 1
            } else {
                pass % 2 == 0
            };
            let running = runtime::spawn(
                project.clone(),
                RuntimeOptions {
                    turbo: true,
                    time_limit: Some(Duration::from_secs(120)),
                    ..Default::default()
                },
            )?;
            let scene = running.scene.clone();
            if gpu {
                scene.enable_gpu_pen();
            }
            let start = Instant::now();
            while !running.is_finished() {
                renderer.capture_scene(&scene, &scene.snapshot(&project))?;
                std::thread::sleep(Duration::from_millis(8));
                ensure!(
                    start.elapsed() < Duration::from_secs(180),
                    "benchmark timed out"
                );
            }
            let report = running.join()?;
            ensure!(!report.interrupted, "benchmark did not finish");
            let frames = report
                .variables
                .iter()
                .find(|v| v.name == "System: Frames Elapsed")
                .context("frame counter missing")?;
            ensure!(
                frames.value == ScratchValue::Number(expected as f64 + 1.),
                "incorrect frame count: {:?}",
                frames.value
            );
            renderer.capture_scene(&scene, &report.sprites)?;
            // FPS/timer monitor text varies with execution speed; compare all
            // stage drawing without that separate UI overlay.
            let image = renderer.capture(&report.sprites)?;
            if let Some(reference) = &reference {
                let total: u64 = image
                    .iter()
                    .zip(reference)
                    .map(|(a, b): (&u8, &u8)| a.abs_diff(*b) as u64)
                    .sum();
                ensure!(
                    total < image.len() as u64,
                    "final rendered frame differs excessively: {total}"
                );
            } else {
                reference = Some(image.clone());
            }
            if let Ok(path) = std::env::var("MSBJIT_PEN_BENCH_CAPTURE") {
                image::save_buffer(path, &image, WIDTH, HEIGHT, image::ColorType::Rgba8)?;
            }
            let sample = serde_json::json!({"gpu":gpu,"elapsed":report.elapsed_seconds,"frames":expected,
                "fps": expected as f64 / report.elapsed_seconds, "steps":report.steps});
            eprintln!("{sample}");
            samples.push(sample);
        }
        if let Ok(path) = std::env::var("MSBJIT_PEN_BENCH_OUTPUT") {
            std::fs::write(path, serde_json::to_vec_pretty(&samples)?)?;
        }
        Ok(())
    }
    #[test]
    fn svg_text_uses_bundled_fonts_and_scratch_marker_alias() -> Result<()> {
        let svg = |family| {
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60"><text x="5" y="45" font-family="{family}" font-size="40">B</text></svg>"#
            )
        };
        let (_, _, marker) = decode("text.svg", svg("Marker").as_bytes())?;
        let (_, _, knewave) = decode("text.svg", svg("Knewave").as_bytes())?;
        let (_, _, fallback) = decode("text.svg", svg("unavailable-font").as_bytes())?;
        assert_eq!(marker, knewave);
        assert_ne!(marker, fallback);
        for pixels in [&marker, &fallback] {
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[3] > 0)
                    .count()
                    > 100
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires a wgpu graphics adapter"]
    fn all_graphic_effects_match_scratch_webgl_reference() -> Result<()> {
        let mut p: Project = serde_json::from_value(serde_json::json!({"targets":[
            {"name":"Stage","isStage":true},
            {"name":"Sprite","costumes":[{"name":"pattern","md5ext":"skin.png","rotationCenterX":32,"rotationCenterY":32}]}
        ]}))?;
        p.assets.insert(
            "skin.png".into(),
            include_bytes!("../tests/fixtures/effects/skin.png").to_vec(),
        );
        let p = std::sync::Arc::new(p);
        let report = crate::runtime::spawn(p.clone(), Default::default())?.join()?;
        let mut states = report.sprites;
        let mut renderer = pollster::block_on(Renderer::new(&p))?;
        assert_eq!(renderer.hit_test(&states, -7.5, 24.5), Some(1));
        states[1].effects.mosaic = 50.0;
        assert_eq!(renderer.hit_test(&states, -7.5, 24.5), Some(0));
        states[1].effects = crate::effects::GraphicEffects {
            ghost: 100.0,
            ..Default::default()
        };
        assert_eq!(renderer.hit_test(&states, -7.5, 24.5), Some(0));
        states[1].effects.ghost = 99.0;
        assert_eq!(renderer.hit_test(&states, -7.5, 24.5), Some(1));
        let reference: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/effects/reference.json"))?;
        for case in reference["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            states[1].effects =
                crate::effects::GraphicEffects::from_values(std::array::from_fn(|i| {
                    case["values"][i].as_f64().unwrap()
                }));
            let resolution = case["resolution"].as_f64().unwrap();
            renderer.costumes[1][0].resolution = resolution;
            renderer.costumes[1][0].svg = case["svg"].as_bool().unwrap_or(false);
            states[1].size = 100.0 * resolution;
            let rgba = renderer.capture(&states)?;
            let expected = image::open(format!(
                "{}/tests/fixtures/effects/{name}.png",
                env!("CARGO_MANIFEST_DIR")
            ))?
            .to_rgba8();
            let mut max = 0;
            let mut bad = 0;
            let mut total = 0u64;
            let mut crop = Vec::new();
            for y in 0..64 {
                for x in 0..64 {
                    let pixel = &rgba[((y + 148) * 480 + x + 208) * 4..][..4];
                    crop.extend_from_slice(pixel);
                    for (a, b) in pixel.iter().zip(expected.get_pixel(x as u32, y as u32).0) {
                        let diff = a.abs_diff(b);
                        max = max.max(diff);
                        bad += usize::from(diff > 2);
                        total += diff as u64;
                    }
                }
            }
            eprintln!("{name}: max {max}, channels >2 {bad}, total {total}");
            if let Ok(dir) = std::env::var("MSBJIT_EFFECT_CAPTURE") {
                image::save_buffer(
                    format!("{dir}/{name}.png"),
                    &crop,
                    64,
                    64,
                    image::ColorType::Rgba8,
                )?;
            }
            ensure!(
                max <= 2,
                "{name}: effect diverged from Scratch: max={max}, bad={bad}, total={total}"
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires a wgpu graphics adapter"]
    fn variable_monitors_composite_above_the_gpu_stage() -> Result<()> {
        let mut project = Project::load(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/demo.sb3"))?;
        for t in &mut project.targets {
            t.blocks.clear();
        }
        let stage = project.targets.iter_mut().find(|t| t.is_stage).unwrap();
        stage.variables.insert(
            "monitor".into(),
            ("score".into(), crate::value::ScratchValue::Number(42.0)),
        );
        project.monitors = serde_json::from_value(serde_json::json!([
            {"id":"monitor","opcode":"data_variable","mode":"large","x":10,"y":10,"visible":true}
        ]))?;
        let project = std::sync::Arc::new(project);
        let run = crate::runtime::spawn(project.clone(), Default::default())?;
        let scene = run.scene.clone();
        let report = run.join()?;
        let mut renderer = pollster::block_on(Renderer::new(&project))?;
        let baseline = renderer.capture(&report.sprites)?;
        let frame = renderer.capture_scene(&scene, &report.sprites)?;
        assert_eq!(&frame[(15 * 480 + 15) * 4..][..4], &[255, 140, 26, 255]);
        for m in scene.monitors.lock().unwrap().iter_mut() {
            m.visible = false;
        }
        let hidden = renderer.capture_scene(&scene, &report.sprites)?;
        assert_eq!(hidden, baseline);
        Ok(())
    }
    #[test]
    fn zero_sized_svg_costumes_are_transparent_but_malformed_svgs_fail() -> Result<()> {
        let (w, h, rgba) = decode(
            "empty.svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="0" height="0" viewBox="0 0 0 0"/>"#,
        )?;
        assert_eq!((w, h), (1, 1));
        assert_eq!(rgba, vec![0; 4]);
        assert!(decode("invalid.svg", b"<svg").is_err());
        Ok(())
    }
    #[test]
    #[ignore = "requires --release, MSBJIT_LINUX_PROJECT, and a wgpu adapter"]
    fn riscv_linux_project_starts_and_draws() -> Result<()> {
        let Ok(path) = std::env::var("MSBJIT_LINUX_PROJECT") else {
            return Ok(());
        };
        ensure!(
            !cfg!(debug_assertions),
            "run this emulator regression with cargo test --release"
        );
        let project = std::sync::Arc::new(Project::load(path)?);
        let running = crate::runtime::spawn(
            project.clone(),
            crate::runtime::RuntimeOptions {
                turbo: true,
                time_limit: Some(std::time::Duration::from_secs(8)),
                ..Default::default()
            },
        )?;
        let scene = running.scene.clone();
        let report = running.join()?;
        eprintln!("Linux: {} steps, {} ticks", report.steps, report.ticks);
        for variable in &report.variables {
            if [
                "state.pc",
                "state.cyclel",
                "__pc",
                "Global.stageWidth",
                "Global.stageHeight",
                "@runFrame.state",
            ]
            .contains(&variable.name.as_str())
            {
                eprintln!("{} = {}", variable.name, variable.value.text());
            }
        }
        let output = report
            .lists
            .iter()
            .find(|l| l.name == "Terminal.outputBuffer")
            .context("missing terminal output")?;
        eprintln!(
            "Terminal queue: {:?}",
            &output.items[..output.items.len().min(100)]
        );
        let mut renderer = pollster::block_on(Renderer::new(&project))?;
        let rgba = renderer.capture_scene(&scene, &report.sprites)?;
        if let Ok(path) = std::env::var("MSBJIT_LINUX_CAPTURE") {
            image::save_buffer(path, &rgba, WIDTH, HEIGHT, image::ColorType::Rgba8)?;
        }
        assert!(report.steps > 10_000);
        let cycles = report
            .variables
            .iter()
            .find(|v| v.name == "state.cyclel")
            .context("missing CPU cycle counter")?;
        assert!(
            cycles.value.to_number() > 1.0,
            "CPU did not progress: {}",
            cycles.value.text()
        );
        assert!(
            scene
                .pen
                .lock()
                .unwrap()
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0)
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires MSBJIT_RAYTRACE_PROJECT and a wgpu adapter"]
    fn raytrace_intro_rendering_sliders_and_keyboard() -> Result<()> {
        use crate::{
            input::InputEvent,
            runtime::{self, RuntimeOptions},
        };
        use std::{
            sync::Arc,
            thread,
            time::{Duration, Instant},
        };
        let Ok(path) = std::env::var("MSBJIT_RAYTRACE_PROJECT") else {
            eprintln!("Set MSBJIT_RAYTRACE_PROJECT to raytrace.sb3");
            return Ok(());
        };
        let project = Arc::new(Project::load(path)?);
        let mut renderer = pollster::block_on(Renderer::new(&project))?;
        let running = runtime::spawn_interactive(
            project.clone(),
            RuntimeOptions {
                turbo: true,
                warp_timeout: Duration::from_millis(20),
                time_limit: Some(Duration::from_secs(35)),
                ..Default::default()
            },
        )?;
        let scene = running.scene.clone();
        scene.input.send(InputEvent::Flag);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let states = scene.snapshot(&project);
            if states.iter().filter(|s| s.is_clone && s.active).count() == 3
                && states[3].visible
                && states[3].size >= 99.0
            {
                break;
            }
            if running.is_finished() {
                running.join()?;
                anyhow::bail!("ray tracer stopped during introduction");
            }
            ensure!(
                Instant::now() < deadline,
                "introduction clones did not appear"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let intro = scene.snapshot(&project);
        assert_eq!(intro.iter().filter(|s| s.is_clone && s.active).count(), 3);
        let rgba = renderer.capture_scene(&scene, &intro)?;
        if let Ok(path) = std::env::var("MSBJIT_RAYTRACE_INTRO_CAPTURE") {
            image::save_buffer(path, &rgba, WIDTH, HEIGHT, image::ColorType::Rgba8)?;
        }
        let monitor = |name: &str| {
            scene
                .monitors
                .lock()
                .unwrap()
                .iter()
                .find(|m| m.label == name || m.label == format!("pen: {name}"))
                .cloned()
        };
        // The saved file already has visible controls and stale monitor values.
        // Wait for the actual start broadcast and first pen output instead.
        while scene
            .snapshot(&project)
            .iter()
            .any(|s| s.is_clone && s.active)
            || !scene
                .pen
                .lock()
                .unwrap()
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0)
        {
            if running.is_finished() {
                running.join()?;
                anyhow::bail!("ray tracer stopped during introduction");
            }
            ensure!(Instant::now() < deadline, "introduction timed out");
            thread::sleep(Duration::from_millis(10));
        }
        let slot = monitor("#resolution")
            .context("missing resolution control")?
            .slot;
        scene
            .input
            .send(InputEvent::MonitorSlider { slot, value: 8.0 });
        // Observe two hidden project variables through existing monitor bindings.
        // Their visibility is restored before the final player capture.
        for m in scene.monitors.lock().unwrap().iter_mut() {
            if m.label == "cam xdir" || m.label == "pen: time" {
                m.visible = true;
                m.number = 0.0;
            }
        }
        while !monitor("time").is_some_and(|m| m.number > 0.0) {
            if running.is_finished() {
                running.join()?;
                anyhow::bail!("ray tracer stopped before its first frame");
            }
            ensure!(
                Instant::now() < deadline,
                "first ray-traced frame timed out"
            );
            thread::sleep(Duration::from_millis(20));
        }
        let before = renderer.capture_scene(&scene, &scene.snapshot(&project))?;
        let pen_before = scene.pen.lock().unwrap().pixels.clone();
        let camera = monitor("cam xdir").context("missing camera state")?.number;
        scene.input.send(InputEvent::Key {
            key: "right arrow".into(),
            down: true,
        });
        while !monitor("cam xdir").is_some_and(|m| m.number != camera) {
            if running.is_finished() {
                running.join()?;
                anyhow::bail!("ray tracer stopped while moving the camera");
            }
            ensure!(
                Instant::now() < deadline,
                "camera did not accept arrow input"
            );
            thread::sleep(Duration::from_millis(20));
        }
        scene.input.send(InputEvent::Release);
        scene.input.send(InputEvent::Key {
            key: "space".into(),
            down: true,
        });
        thread::sleep(Duration::from_secs(1));
        scene.input.send(InputEvent::Release);
        for m in scene.monitors.lock().unwrap().iter_mut() {
            if m.label == "cam xdir" || m.label == "pen: time" {
                m.visible = false;
            }
        }
        let after = renderer.capture_scene(&scene, &scene.snapshot(&project))?;
        assert_ne!(
            before, after,
            "camera input did not change the rendered scene"
        );
        assert_ne!(
            pen_before,
            scene.pen.lock().unwrap().pixels,
            "camera input did not change pen output"
        );
        if let Ok(path) = std::env::var("MSBJIT_RAYTRACE_CAPTURE") {
            image::save_buffer(path, &after, WIDTH, HEIGHT, image::ColorType::Rgba8)?;
        }
        let pen_pixels = scene
            .pen
            .lock()
            .unwrap()
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 0)
            .count();
        assert!(
            pen_pixels > 10_000,
            "ray tracer produced only {pen_pixels} pen pixels"
        );
        running.stop();
        let report = running.join()?;
        assert_eq!(
            report
                .variables
                .iter()
                .find(|v| v.name == "#resolution")
                .unwrap()
                .value
                .to_number(),
            8.0
        );
        assert_eq!(
            report
                .variables
                .iter()
                .find(|v| v.name == "Intro")
                .unwrap()
                .value
                .to_number(),
            4.0
        );
        assert!(
            report
                .sprites
                .iter()
                .filter(|s| s.is_clone)
                .all(|s| !s.active)
        );
        eprintln!(
            "Ray tracer: {} native steps, {} pen pixels, camera {} -> {}",
            report.steps,
            pen_pixels,
            camera,
            report
                .variables
                .iter()
                .find(|v| v.name == "cam xdir")
                .unwrap()
                .value
                .text()
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires MSBJIT_HARDWARE_PROJECT and a wgpu adapter"]
    fn sbtext_hardware_emulator_boots_draws_and_accepts_keyboard() -> Result<()> {
        use crate::{
            input::InputEvent,
            runtime::{self, RuntimeOptions},
        };
        use std::{
            sync::{Arc, atomic::Ordering},
            thread,
            time::{Duration, Instant},
        };
        let Ok(path) = std::env::var("MSBJIT_HARDWARE_PROJECT") else {
            eprintln!(
                "Skipping optional emulator fixture: set MSBJIT_HARDWARE_PROJECT to hardware_emu.sb3"
            );
            return Ok(());
        };
        let project = Arc::new(Project::load(path)?);
        let running = runtime::spawn_interactive(
            project.clone(),
            RuntimeOptions {
                time_limit: Some(Duration::from_secs(20)),
                ..Default::default()
            },
        )?;
        running.scene.input.send(InputEvent::Flag);
        let deadline = Instant::now() + Duration::from_secs(40);
        while running.scene.steps.load(Ordering::Relaxed) < 650_000 {
            if running.is_finished() {
                running.join()?;
                anyhow::bail!("emulator stopped before boot completed");
            }
            ensure!(Instant::now() < deadline, "emulator boot timed out");
            thread::sleep(Duration::from_millis(10));
        }
        running.scene.input.send(InputEvent::Key {
            key: "up arrow".into(),
            down: true,
        });
        thread::sleep(Duration::from_millis(150));
        running.scene.input.send(InputEvent::Release);
        let scene = running.scene.clone();
        running.stop();
        let report = running.join()?;
        let variable = |name: &str| {
            &report
                .variables
                .iter()
                .find(|v| v.name == name)
                .unwrap()
                .value
        };
        assert_eq!(
            variable("CPU_RUNNING"),
            &crate::value::ScratchValue::Number(1.0)
        );
        assert_eq!(
            variable("STEP_MODE"),
            &crate::value::ScratchValue::Number(1.0)
        );
        for name in ["RAM_BYTES", "HDD_BYTES"] {
            assert_eq!(
                report
                    .lists
                    .iter()
                    .find(|l| l.name == name)
                    .unwrap()
                    .items
                    .len(),
                65_536
            );
        }
        let text = &report
            .lists
            .iter()
            .find(|l| l.name == "IO_TEXT_BUFFER")
            .unwrap()
            .items;
        assert!(!text.is_empty(), "CPU did not produce terminal output");
        let layer = scene.pen.lock().unwrap();
        assert!(
            layer.pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
            "terminal did not draw any pen pixels"
        );
        drop(layer);
        let mut renderer = pollster::block_on(Renderer::new(&project))?;
        let rgba = renderer.capture_scene(&scene, &report.sprites)?;
        assert!(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[..3] != [255, 255, 255])
        );
        if let Ok(path) = std::env::var("MSBJIT_HARDWARE_CAPTURE") {
            image::save_buffer(path, &rgba, WIDTH, HEIGHT, image::ColorType::Rgba8)?;
        }
        eprintln!(
            "Emulator: {} native steps; terminal bytes {:?}; keyboard STEP_MODE=1",
            report.steps, text
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires MSBJIT_DOIRS_PROJECT and a wgpu adapter"]
    fn doirs_project_renders_and_starts_without_runtime_errors() -> Result<()> {
        use crate::{
            input::InputEvent,
            runtime::{self, RuntimeOptions},
        };
        use std::{
            sync::Arc,
            thread,
            time::{Duration, Instant},
        };
        let Ok(path) = std::env::var("MSBJIT_DOIRS_PROJECT") else {
            return Ok(());
        };
        let p = Arc::new(Project::load(path)?);
        let mut renderer = pollster::block_on(Renderer::new(&p))?;
        let player = p
            .targets
            .iter()
            .position(|t| t.name == "Player")
            .context("missing player")?;
        let restored = std::env::var_os("MSBJIT_DOIRS_RESTORED").is_some();
        let menu = p
            .targets
            .iter()
            .position(|t| t.name == "main menu")
            .context("missing menu")?;
        let running = runtime::spawn_interactive(
            p.clone(),
            RuntimeOptions {
                time_limit: Some(Duration::from_secs(20)),
                ..Default::default()
            },
        )?;
        let scene = running.scene.clone();
        scene.input.send(InputEvent::Flag);
        let deadline = Instant::now() + Duration::from_secs(15);
        while scene
            .snapshot(&p)
            .iter()
            .filter(|s| s.is_clone && s.target == menu && s.active)
            .count()
            < 6
        {
            if running.is_finished() {
                running.join()?;
                anyhow::bail!("game stopped before menu initialized");
            }
            ensure!(Instant::now() < deadline, "menu initialization timed out");
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(200));
        let menu_image = renderer.capture_scene(&scene, &scene.snapshot(&p))?;
        if let Ok(path) = std::env::var("MSBJIT_DOIRS_MENU_CAPTURE") {
            image::save_buffer(path, &menu_image, WIDTH, HEIGHT, image::ColorType::Rgba8)?;
        }
        scene.input.send(InputEvent::Key {
            key: "space".into(),
            down: true,
        });
        while scene.snapshot(&p)[menu].visible {
            if running.is_finished() {
                running.join()?;
                anyhow::bail!("game stopped while starting");
            }
            ensure!(Instant::now() < deadline, "space did not start the game");
            thread::sleep(Duration::from_millis(10));
        }
        scene.input.send(InputEvent::Release);
        thread::sleep(Duration::from_millis(if restored { 3000 } else { 1500 }));
        let before = scene.snapshot(&p)[player].clone();
        for key in ["right arrow", "up arrow", "left arrow", "down arrow"] {
            scene.input.send(InputEvent::Key {
                key: key.into(),
                down: true,
            });
            thread::sleep(Duration::from_millis(400));
            scene.input.send(InputEvent::Release);
            let state = scene.snapshot(&p)[player].clone();
            if (state.x, state.y) != (before.x, before.y) {
                break;
            }
        }
        let after = scene.snapshot(&p)[player].clone();
        let image = renderer.capture_scene(&scene, &scene.snapshot(&p))?;
        if let Ok(path) = std::env::var("MSBJIT_DOIRS_CAPTURE") {
            image::save_buffer(path, &image, WIDTH, HEIGHT, image::ColorType::Rgba8)?;
        }
        assert_ne!(menu_image, image);
        running.stop();
        let report = running.join()?;
        eprintln!(
            "Input state: {:?}",
            report
                .variables
                .iter()
                .filter(
                    |v| ["lpx", "lpy", "controls", "InCloset", "speed"].contains(&v.name.as_str())
                )
                .collect::<Vec<_>>()
        );
        // This SBText export replaced mouse/motion reporters with constant
        // zero. Its saved proximity/movement logic traps the player in a closet
        // after Space starts the game. Synthetic tests cover movement
        // and alpha collision semantics independently of that export damage.
        assert!(report.steps > 1000);
        if restored {
            assert_ne!(
                (before.x, before.y),
                (after.x, after.y),
                "restored input reporters must allow player movement"
            );
        }
        eprintln!(
            "Doirs: {} steps, player ({},{}) -> ({},{})",
            report.steps, before.x, before.y, after.x, after.y
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires a wgpu graphics adapter"]
    fn native_pen_blocks_render_between_backdrop_and_sprites() -> Result<()> {
        let mut project = Project::load(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/demo.sb3"))?;
        for target in &mut project.targets {
            target.blocks.clear();
        }
        let sprite = project.targets.iter_mut().find(|t| !t.is_stage).unwrap();
        sprite.blocks = serde_json::from_value(serde_json::json!({
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"home"},
            "home":{"opcode":"motion_gotoxy","inputs":{"X":[1,[4,"0"]],"Y":[1,[4,"0"]]},"next":"color"},
            "color":{"opcode":"pen_setPenColorToColor","inputs":{"COLOR":[1,[9,"#ff0000"]]},"next":"size"},
            "size":{"opcode":"pen_setPenSizeTo","inputs":{"SIZE":[1,[4,"12"]]},"next":"down"},
            "down":{"opcode":"pen_penDown","next":"line"},
            "line":{"opcode":"motion_gotoxy","inputs":{"X":[1,[4,"100"]],"Y":[1,[4,"0"]]},"next":"up"},
            "up":{"opcode":"pen_penUp"}
        }))?;
        let project = std::sync::Arc::new(project);
        let running = crate::runtime::spawn(project.clone(), Default::default())?;
        let scene = running.scene.clone();
        let report = running.join()?;
        let mut renderer = pollster::block_on(Renderer::new(&project))?;
        let baseline = renderer.capture(&report.sprites)?;
        let rgba = renderer.capture_scene(&scene, &report.sprites)?;
        let pixel = |x: usize, y: usize| &rgba[(y * 480 + x) * 4..][..4];
        assert_eq!(pixel(290, 180), &[255, 0, 0, 255]);
        assert_eq!(pixel(340, 180), &[99, 102, 241, 255]);
        assert_eq!(pixel(450, 100), &[232, 238, 248, 255]);
        scene.pen.lock().unwrap().clear();
        let rgba = renderer.capture_scene(&scene, &report.sprites)?;
        assert_eq!(
            &rgba[(180 * 480 + 290) * 4..][..4],
            &baseline[(180 * 480 + 290) * 4..][..4]
        );
        Ok(())
    }
    #[test]
    #[ignore = "requires a wgpu graphics adapter"]
    fn gpu_capture_contains_backdrop_and_costume() -> Result<()> {
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
        let mut renderer = pollster::block_on(Renderer::new(&project))?;
        let rgba = renderer.capture(&report.sprites)?;
        let pixel = |x: usize, y: usize| &rgba[(y * 480 + x) * 4..(y * 480 + x) * 4 + 4];
        assert_eq!(pixel(300, 100), &[232, 238, 248, 255]);
        assert_eq!(pixel(40, 180), &[99, 102, 241, 255]);
        Ok(())
    }

    #[test]
    #[ignore = "requires a wgpu graphics adapter"]
    fn click_picking_follows_costume_alpha_and_transforms() -> Result<()> {
        let project = std::sync::Arc::new(Project::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/interactive.sb3"
        ))?);
        let report = crate::runtime::spawn(
            project.clone(),
            crate::runtime::RuntimeOptions {
                max_steps: Some(0),
                ..Default::default()
            },
        )?
        .join()?;
        let renderer = pollster::block_on(Renderer::new(&project))?;
        let mut states = report.sprites;
        assert_eq!(renderer.hit_test(&states, 0.0, 0.0), Some(1));
        assert_eq!(renderer.hit_test(&states, 31.0, 31.0), Some(0));
        assert_eq!(renderer.hit_test(&states, 250.0, 0.0), None);
        states[1].x = 80.0;
        states[1].y = -50.0;
        states[1].direction = 0.0;
        states[1].size = 200.0;
        assert_eq!(renderer.hit_test(&states, 80.0, -50.0), Some(1));
        assert_eq!(renderer.hit_test(&states, 130.0, -50.0), Some(1));
        assert_eq!(renderer.hit_test(&states, 0.0, 0.0), Some(0));
        states[1].rotation_style = "left-right".into();
        states[1].direction = -90.0;
        assert_eq!(renderer.hit_test(&states, 80.0, -50.0), Some(1));
        states[1].visible = false;
        assert_eq!(renderer.hit_test(&states, 80.0, -50.0), Some(0));
        Ok(())
    }
}
