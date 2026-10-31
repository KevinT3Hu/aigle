//! Vulkan WSI HDR presentation into the embedded Wayland canvas.
//! The raw handle boundary is limited to borrowing live Wayland objects. wgpu's surface
//! owns an Arc to both objects, so the handles outlive every Vulkan use of the surface.
use anyhow::{Context, bail};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle,
};
use std::{ptr::NonNull, sync::Arc};
use wayland_client::{Connection, Proxy, protocol::wl_surface::WlSurface};

struct Target {
    conn: Connection,
    surface: WlSurface,
}
impl HasDisplayHandle for Target {
    #[allow(unsafe_code)]
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let ptr = NonNull::new(self.conn.backend().display_ptr().cast())
            .ok_or(HandleError::Unavailable)?;
        // SAFETY: self.conn owns the live libwayland display for the returned borrow.
        Ok(unsafe {
            DisplayHandle::borrow_raw(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(ptr)))
        })
    }
}
impl HasWindowHandle for Target {
    #[allow(unsafe_code)]
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let ptr =
            NonNull::new(self.surface.id().as_ptr().cast()).ok_or(HandleError::Unavailable)?;
        // SAFETY: self.surface owns the live wl_surface for the returned borrow.
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::Wayland(WaylandWindowHandle::new(ptr)))
        })
    }
}

#[derive(Clone)]
pub struct CanvasScene {
    /// Edited overview/detail, then original overview/detail.
    pub frames: [Option<Arc<aigle_engine_api::Frame>>; 4],
    pub region: aigle_core::geometry::Rect,
    pub size: (u32, u32),
    pub split: Option<f32>,
}
impl CanvasScene {
    pub fn has_images(&self) -> bool {
        self.frames.iter().any(Option::is_some)
    }
}

struct ImageSlot {
    texture: wgpu::Texture,
    frame: Option<Arc<aigle_engine_api::Frame>>,
    size: (u32, u32),
}

pub struct Presenter {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    _sampler: wgpu::Sampler,
    images: [ImageSlot; 4],
    uniforms: wgpu::Buffer,
    bindings: wgpu::BindGroup,
    configured: bool,
}
impl Presenter {
    pub fn new(conn: Connection, surface: WlSurface) -> anyhow::Result<Self> {
        futures::executor::block_on(Self::new_async(conn, surface))
    }
    async fn new_async(conn: Connection, surface: WlSurface) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance.create_surface(Arc::new(Target { conn, surface }))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: false,
                ..Default::default()
            })
            .await?;
        let caps = surface.get_capabilities(&adapter);
        let format = [
            wgpu::TextureFormat::Rgb10a2Unorm,
            wgpu::TextureFormat::Rgba16Float,
        ]
        .into_iter()
        .find(|format| {
            caps.format_capabilities.iter().any(|c| {
                c.format == *format && c.color_spaces.contains(wgpu::SurfaceColorSpaces::BT2100_PQ)
            })
        })
        .context("Vulkan WSI does not expose a 10-bit/float BT.2100 PQ surface")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("aigle HDR display"),
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("HDR presentation"),
            source: wgpu::ShaderSource::Wgsl(include_str!("hdr_surface.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("HDR PQ presentation"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Bt2100Pq,
            width: 1280,
            height: 800,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode: if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
                wgpu::CompositeAlphaMode::Opaque
            } else {
                caps.alpha_modes[0]
            },
            view_formats: vec![],
        };
        tracing::info!(adapter=%adapter.get_info().name,?format,"native HDR swapchain");
        let images = std::array::from_fn(|_| ImageSlot {
            texture: create_image(&device, (1, 1)),
            frame: None,
            size: (1, 1),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native canvas mappings"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bindings = create_bindings(&device, &pipeline, &images, &sampler, &uniforms);
        for image in &images {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &image.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &[0; 8],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(8),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            _sampler: sampler,
            images,
            uniforms,
            bindings,
            configured: false,
        })
    }
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Image resampling, overview coverage and split composition all happen in
    /// linear Rec.2020 before PQ encoding. A pan only updates the mappings until
    /// newly rendered detail arrives; existing textures remain resident.
    pub fn present(&mut self, scene: &CanvasScene) -> anyhow::Result<bool> {
        let size = scene.size;
        let limit = self.device.limits().max_texture_dimension_2d;
        anyhow::ensure!(
            size.0 > 0 && size.1 > 0 && size.0 <= limit && size.1 <= limit,
            "canvas exceeds GPU texture limits"
        );
        if !self.configured || (self.config.width, self.config.height) != size {
            self.config.width = size.0;
            self.config.height = size.1;
            self.surface.configure(&self.device, &self.config);
            self.configured = true;
        }
        let mut mappings = [[-2.0f32, -2.0, 0.0, 0.0]; 5];
        let mut rebound = false;
        for (i, frame) in scene.frames.iter().enumerate() {
            let Some(frame) = frame else {
                continue;
            };
            let p = &frame.pixels;
            anyhow::ensure!(
                p.format == aigle_engine_api::PixelFormat::Rgba16F
                    && p.width > 0
                    && p.height > 0
                    && p.width <= limit
                    && p.height <= limit
                    && p.stride as usize >= p.width as usize * 8
                    && p.data.len() >= p.stride as usize * p.height as usize,
                "invalid HDR canvas frame"
            );
            let r = frame.viewport.region;
            anyhow::ensure!(r.w > 0.0 && r.h > 0.0, "invalid canvas coverage");
            mappings[i] = [
                ((scene.region.x - r.x) / r.w) as f32,
                ((scene.region.y - r.y) / r.h) as f32,
                (scene.region.w / r.w) as f32,
                (scene.region.h / r.h) as f32,
            ];
            let slot = &mut self.images[i];
            if slot.size != (p.width, p.height) {
                slot.texture = create_image(&self.device, (p.width, p.height));
                slot.size = (p.width, p.height);
                slot.frame = None;
                rebound = true;
            }
            if slot
                .frame
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, frame))
            {
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &slot.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &p.data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(p.stride),
                        rows_per_image: Some(p.height),
                    },
                    wgpu::Extent3d {
                        width: p.width,
                        height: p.height,
                        depth_or_array_layers: 1,
                    },
                );
                slot.frame = Some(frame.clone());
            }
        }
        mappings[4] = [scene.split.unwrap_or(-1.0), 0.0, 0.0, 0.0];
        let bytes: Vec<u8> = mappings
            .into_iter()
            .flatten()
            .flat_map(f32::to_ne_bytes)
            .collect();
        self.queue.write_buffer(&self.uniforms, 0, &bytes);
        if rebound {
            self.bindings = create_bindings(
                &self.device,
                &self.pipeline,
                &self.images,
                &self._sampler,
                &self.uniforms,
            );
        }
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.configured = false;
                return Ok(false);
            }
            _ => bail!("native HDR surface lost"),
        };
        let view = output.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native HDR canvas"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bindings, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(output);
        Ok(true)
    }
}

fn create_image(device: &wgpu::Device, size: (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("native HDR image"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}
fn create_bindings(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    images: &[ImageSlot; 4],
    sampler: &wgpu::Sampler,
    uniforms: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let views: Vec<_> = images
        .iter()
        .map(|i| i.texture.create_view(&Default::default()))
        .collect();
    let mut entries: Vec<_> = views
        .iter()
        .enumerate()
        .map(|(i, v)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: wgpu::BindingResource::TextureView(v),
        })
        .collect();
    entries.push(wgpu::BindGroupEntry {
        binding: 4,
        resource: wgpu::BindingResource::Sampler(sampler),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 5,
        resource: uniforms.as_entire_binding(),
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("native canvas composition"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    })
}
