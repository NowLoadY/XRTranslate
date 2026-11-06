use super::*;
use eframe::{egui, egui_wgpu};
use std::sync::Arc;
use xrtranslate_plugin::surface::{Drawing, GpuRenderer, Input, SharedFrame};

struct Buffers {
    name: String,
    textures: Vec<wgpu::Texture>,
    fence: ID3D12Fence,
}
struct Graphics {
    state: RenderState,
    device: Device,
    buffers: Option<Buffers>,
}
#[derive(Default)]
pub struct Page {
    graphics: Option<Graphics>,
}

impl Graphics {
    fn new(adapter_id: u64) -> Result<Self, String> {
        futures::executor::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::DX12,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapters = instance.enumerate_adapters(wgpu::Backends::DX12).await;
            let adapter = adapters
                .iter()
                .find(|adapter| unsafe {
                    adapter
                        .as_hal::<wgpu::hal::api::Dx12>()
                        .and_then(|a| a.raw_adapter().GetDesc2().ok())
                        .is_some_and(|desc| luid(desc.AdapterLuid) == adapter_id)
                })
                .cloned()
                .ok_or("The host GPU is not available to this plugin")?;
            let (device, queue) = adapter
                .request_device(&Default::default())
                .await
                .map_err(|e| e.to_string())?;
            let mut renderer = egui_wgpu::Renderer::new(&device, FORMAT, Default::default());
            crate::ui::organic_border::install(&device, FORMAT, &mut renderer);
            crate::ui::components::avatar::install(&device, FORMAT, &mut renderer);
            video::install(&device, &mut renderer);
            let state = RenderState {
                adapter,
                available_adapters: adapters,
                instance,
                device,
                queue,
                target_format: FORMAT,
                renderer: Arc::new(egui::mutex::RwLock::new(renderer)),
            };
            let device = Device::new(&state)?;
            log::info!("Plugin UI uses shared D3D12 surfaces");
            Ok(Self {
                state,
                device,
                buffers: None,
            })
        })
    }
}

impl GpuRenderer for Page {
    fn prepare(&mut self, target: &SharedTarget) -> Result<(), String> {
        validate(target)?;
        if self.graphics.is_none() {
            self.graphics = Some(Graphics::new(target.adapter)?);
        }
        let graphics = self.graphics.as_mut().unwrap();
        if luid(unsafe { graphics.device.raw.GetAdapterLuid() }) != target.adapter {
            return Err("Plugin and host GPU changed during rendering".into());
        }
        if graphics
            .buffers
            .as_ref()
            .is_none_or(|b| b.name != target.name)
        {
            let mut textures = Vec::new();
            for slot in 0..2 {
                let resource: ID3D12Resource =
                    graphics.device.open(&format!("{}-{slot}", target.name))?;
                let desc = unsafe { resource.GetDesc() };
                if desc.Width != target.size[0] as u64
                    || desc.Height != target.size[1]
                    || desc.Format != DXGI_FORMAT_R8G8B8A8_UNORM
                    || desc.SampleDesc.Count != 1
                {
                    return Err("Shared plugin texture does not match its descriptor".into());
                }
                textures.push(import(&graphics.state, resource, target.size));
            }
            graphics.buffers = Some(Buffers {
                name: target.name.clone(),
                textures,
                fence: graphics.device.open(&format!("{}-f", target.name))?,
            });
        }
        Ok(())
    }

    fn render(
        &mut self,
        input: &Input,
        mut primitives: Vec<egui::ClippedPrimitive>,
        textures: &mut egui::TexturesDelta,
        submit: bool,
    ) -> Result<(Option<SharedFrame>, Vec<Drawing>), String> {
        let graphics = self.graphics.as_mut().unwrap();
        let state = &graphics.state;
        let mut renderer = state.renderer.write();
        for (id, deltas) in textures.set.drain() {
            for delta in deltas {
                renderer.update_texture(&state.device, &state.queue, id, &delta);
            }
        }
        let external = video::extract(&mut primitives);
        let shared = if submit {
            let target = input.shared_target.as_ref().unwrap();
            let buffers = graphics.buffers.as_ref().unwrap();
            let texture = &buffers.textures[target.slot];
            unsafe { graphics.device.queue.Wait(&buffers.fence, target.released) }
                .map_err(|e| e.to_string())?;
            let mut encoder = state.device.create_command_encoder(&Default::default());
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: target.size,
                pixels_per_point: input.pixels_per_point,
            };
            let mut commands = renderer.update_buffers(
                &state.device,
                &state.queue,
                &mut encoder,
                &primitives,
                &screen,
            );
            {
                let view = texture.create_view(&Default::default());
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("plugin page"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                renderer.render(&mut pass.forget_lifetime(), &primitives, &screen);
            }
            release(&mut encoder, texture);
            commands.push(encoder.finish());
            state.queue.submit(commands);
            graphics
                .device
                .signal(&buffers.fence, target.released + 1)?;
            Some(SharedFrame {
                name: target.name.clone(),
                slot: target.slot,
                ready: target.released + 1,
            })
        } else {
            None
        };
        for id in textures.free.drain() {
            renderer.free_texture(&id);
        }
        Ok((shared, external))
    }
}
