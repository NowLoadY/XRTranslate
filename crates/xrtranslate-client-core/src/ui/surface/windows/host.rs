use super::*;
use eframe::egui;
use xrtranslate_plugin::surface::{Compositor, SharedFrame};

struct Buffers {
    name: String,
    size: [u32; 2],
    textures: Vec<(wgpu::Texture, egui::TextureId)>,
    fence: ID3D12Fence,
    _handles: Vec<Handle>,
    serial: u64,
    displayed: Option<usize>,
    pending: Option<SharedFrame>,
    in_flight: bool,
}

pub struct Host {
    state: RenderState,
    device: Device,
    buffers: Option<Buffers>,
}
impl Host {
    pub fn new(state: &RenderState) -> Result<Self, String> {
        Ok(Self {
            state: state.clone(),
            device: Device::new(state)?,
            buffers: None,
        })
    }
    fn allocate(&self, size: [u32; 2]) -> Result<Buffers, String> {
        let name = format!("Local\\XRTranslate-{}", uuid::Uuid::new_v4());
        let fence = unsafe { self.device.raw.CreateFence(0, D3D12_FENCE_FLAG_SHARED) }
            .map_err(|e| e.to_string())?;
        let mut handles = vec![self.device.share(&fence, &format!("{name}-f"))?];
        let mut textures = Vec::new();
        for slot in 0..2 {
            let mut resource: Option<ID3D12Resource> = None;
            unsafe {
                self.device.raw.CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: D3D12_HEAP_TYPE_DEFAULT,
                        CreationNodeMask: 1,
                        VisibleNodeMask: 1,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_SHARED,
                    &D3D12_RESOURCE_DESC {
                        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                        Width: size[0] as u64,
                        Height: size[1],
                        DepthOrArraySize: 1,
                        MipLevels: 1,
                        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                        ..Default::default()
                    },
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut resource,
                )
            }
            .map_err(|e| e.to_string())?;
            let resource = resource.unwrap();
            handles.push(self.device.share(&resource, &format!("{name}-{slot}"))?);
            let texture = import(&self.state, resource, size);
            textures.push(texture);
        }
        // Register only after all allocations succeed, so partial setup is released.
        let textures = textures
            .into_iter()
            .map(|texture| {
                let id = self.state.renderer.write().register_native_texture(
                    &self.state.device,
                    &texture.create_view(&Default::default()),
                    wgpu::FilterMode::Linear,
                );
                (texture, id)
            })
            .collect();
        Ok(Buffers {
            name,
            size,
            textures,
            fence,
            _handles: handles,
            serial: 0,
            displayed: None,
            pending: None,
            in_flight: false,
        })
    }
    fn clear(&mut self) {
        if let Some(buffers) = self.buffers.take() {
            for (_, id) in buffers.textures {
                self.state.renderer.write().free_texture(&id);
            }
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.clear();
    }
}

impl Compositor for Host {
    fn target(&mut self, size: [u32; 2]) -> Result<SharedTarget, String> {
        if size.iter().any(|n| *n == 0 || *n > 8192) {
            return Err("Plugin surface size is outside GPU limits".into());
        }
        if self.buffers.as_ref().is_none_or(|b| b.size != size) {
            let buffers = self.allocate(size)?;
            self.clear();
            self.buffers = Some(buffers);
        }
        let buffers = self.buffers.as_mut().unwrap();
        let slot = buffers.displayed.map_or(0, |slot| 1 - slot);
        let mut encoder = self
            .state
            .device
            .create_command_encoder(&Default::default());
        release(&mut encoder, &buffers.textures[slot].0);
        self.state.queue.submit([encoder.finish()]);
        buffers.serial += 2;
        self.device.signal(&buffers.fence, buffers.serial)?;
        buffers.in_flight = true;
        Ok(SharedTarget {
            name: buffers.name.clone(),
            size,
            slot,
            released: buffers.serial,
            adapter: luid(unsafe { self.device.raw.GetAdapterLuid() }),
        })
    }
    fn accept(&mut self, frame: SharedFrame) {
        if let Some(buffers) = &mut self.buffers
            && frame.name == buffers.name
            && frame.slot < 2
            && frame.ready == buffers.serial + 1
            && Some(frame.slot) != buffers.displayed
        {
            buffers.pending = Some(frame);
        }
    }
    fn poll(&mut self, ctx: &egui::Context) -> bool {
        let Some(buffers) = &mut self.buffers else {
            return true;
        };
        if let Some(frame) = &buffers.pending {
            let completed = unsafe { buffers.fence.GetCompletedValue() };
            if completed != u64::MAX && completed >= frame.ready {
                buffers.displayed = Some(frame.slot);
                buffers.pending = None;
                buffers.in_flight = false;
            } else {
                // A crashed plugin must never block the host's command queue.
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
                return false;
            }
        }
        // The pipe reply and frame queue are read independently. Receiving the
        // reply alone must not release the target before its frame is accepted.
        if buffers.in_flight {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return false;
        }
        true
    }
    fn paint(&self, ui: &egui::Ui) {
        if let Some(buffers) = &self.buffers
            && let Some(slot) = buffers.displayed
        {
            let size = egui::vec2(buffers.size[0] as f32, buffers.size[1] as f32)
                / ui.ctx().pixels_per_point();
            ui.painter().image(
                buffers.textures[slot].1,
                egui::Rect::from_min_size(egui::Pos2::ZERO, size),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }
}
