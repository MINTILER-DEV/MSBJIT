use crate::{
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
}
struct Costume {
    bind: wgpu::BindGroup,
    width: f64,
    height: f64,
    center: [f64; 2],
    resolution: f64,
    alpha: Vec<u8>,
}
pub(crate) struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    target: wgpu::Texture,
    readback: wgpu::Buffer,
    vertices: wgpu::Buffer,
    costumes: Vec<Vec<Costume>>,
    order: Vec<usize>,
    pen_texture: wgpu::Texture,
    pen_bind: wgpu::BindGroup,
    pen_generation: u64,
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
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
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
                list.push(Costume {
                    bind,
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
            size: ((project.targets.len() + 1) * 6 * std::mem::size_of::<Vertex>()) as u64,
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
            pen_texture,
            pen_bind,
            pen_generation: u64::MAX,
        })
    }
    /// Pick the frontmost visible, nontransparent costume pixel. Transform the
    /// point by the inverse of the same rotation/size/flip used for drawing.
    pub fn hit_test(&self, states: &[SpriteState], x: f64, y: f64) -> Option<usize> {
        if !x.is_finite() || !y.is_finite() || x.abs() > 240.0 || y.abs() > 180.0 {
            return None;
        }
        for &id in self.order.iter().rev() {
            let state = &states[id];
            if state.is_stage {
                return Some(id);
            }
            if !state.visible {
                continue;
            }
            let Some(costume) = self.costumes[id].get(state.costume) else {
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
            if px >= 0.0
                && py >= 0.0
                && px < costume.width
                && py < costume.height
                && costume.alpha[py as usize * costume.width as usize + px as usize] > 0
            {
                return Some(id);
            }
        }
        None
    }
    pub fn capture_scene(
        &mut self,
        scene: &SharedScene,
        states: &[SpriteState],
    ) -> Result<Vec<u8>> {
        let update = {
            let layer = scene.pen.lock().unwrap_or_else(|e| e.into_inner());
            (layer.generation != self.pen_generation)
                .then(|| (layer.generation, layer.pixels.clone()))
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
        self.capture(states)
    }
    fn capture(&mut self, states: &[SpriteState]) -> Result<Vec<u8>> {
        let mut vertices = Vec::with_capacity((states.len() + 1) * 6);
        for [u, v] in [[0., 0.], [1., 0.], [0., 1.], [0., 1.], [1., 0.], [1., 1.]] {
            vertices.push(Vertex {
                position: [u * 2.0 - 1.0, 1.0 - v * 2.0],
                uv: [u, v],
            });
        }
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
            let mut pen_drawn = false;
            for (id, costume, start) in draws {
                if !states[id].is_stage && !pen_drawn {
                    pass.set_bind_group(0, &self.pen_bind, &[]);
                    pass.draw(0..6, 0..1);
                    pen_drawn = true;
                }
                pass.set_bind_group(0, &self.costumes[id][costume].bind, &[]);
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
