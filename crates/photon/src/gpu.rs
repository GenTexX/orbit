//! photon GPU context: owns the wgpu instance, adapter, device, and queue shared by every render path.

use crate::error::RendererError;

/// The wgpu objects shared by both the windowed and headless render paths.
///
/// Fields beyond `adapter`/`device`/`queue` may sit unused between Milestone 1
/// steps (e.g. `instance` is retained so surfaces stay valid).
#[allow(dead_code)]
pub(crate) struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// A default instance with no display handle. `_from_env` honors WGPU_*
    /// variables (e.g. WGPU_BACKEND to force a backend). The windowed path
    /// passes a window to `create_surface` separately, which carries its own
    /// display handle.
    ///
    /// Vulkan validation layers are left off by default: they are a debugging
    /// aid, not always-on noise, and wgpu 30 has a benign but very chatty
    /// validation bug on Linux swapchain acquire (the acquire fence is only
    /// reset on Windows). Opt in with `WGPU_VALIDATION=1` when chasing a GPU
    /// problem; that path also surfaces every real validation message.
    pub(crate) fn default_instance() -> wgpu::Instance {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        if std::env::var_os("WGPU_VALIDATION").is_none() {
            desc.flags.remove(wgpu::InstanceFlags::VALIDATION);
        }
        wgpu::Instance::new(desc)
    }

    /// Create a headless GPU context with no surface, suitable for offscreen
    /// rendering and tests.
    ///
    /// Blocks until the device is acquired. wgpu's setup is async internally,
    /// but that never leaks to callers: the renderer's public constructors are
    /// synchronous by design (see the Milestone 1 plan).
    pub(crate) fn headless() -> Result<Self, RendererError> {
        pollster::block_on(Self::request(Self::default_instance(), None))
    }

    /// Acquire an adapter and device from `instance`. `compatible_surface`
    /// constrains adapter selection to one that can present to the given
    /// surface; `None` selects any available adapter (headless).
    pub(crate) async fn request(
        instance: wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, RendererError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface,
                ..Default::default()
            })
            .await?;

        let info = adapter.get_info();
        tracing::info!(adapter = %info.name, backend = ?info.backend, "acquired GPU adapter");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("photon device"),
                ..Default::default()
            })
            .await?;
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a GPU adapter; run locally with --ignored, or in CI once lavapipe is wired up"]
    fn headless_context_initializes() {
        let gpu = Gpu::headless().expect("headless GPU context should initialize");
        let info = gpu.adapter.get_info();
        // A successfully acquired adapter always reports a device name and backend.
        assert!(!info.name.is_empty(), "adapter reported an empty name");
        println!("photon using adapter: {} ({:?})", info.name, info.backend);
    }
}
