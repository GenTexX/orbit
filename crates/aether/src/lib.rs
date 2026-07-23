//! aether - the shared wgpu bootstrap: one instance, adapter, device, and queue,
//! plus the validation policy, borrowed by photon, aurora-wgpu, and the editor
//! so they all draw on a single device (ADR 0018). wgpu stays confined to the
//! two rendering backends and this bootstrap; the engine and model logic never
//! see a wgpu type.

mod error;

pub use error::GpuError;

/// The wgpu handles shared by every renderer on the same device: cheap to
/// clone (each field is an `Arc`-backed handle internally), so `photon` and
/// `aurora-wgpu` each hold their own clone of the same underlying device.
#[derive(Debug, Clone)]
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// A default instance with no display handle. `_from_env` honors WGPU_*
    /// variables (e.g. WGPU_BACKEND to force a backend). A caller that also
    /// needs a window surface passes it to `create_surface` separately, which
    /// carries its own display handle.
    ///
    /// Vulkan validation layers are left off by default: they are a debugging
    /// aid, not always-on noise, and wgpu 30 has a benign but very chatty
    /// validation bug on Linux swapchain acquire (the acquire fence is only
    /// reset on Windows). Opt in with `WGPU_VALIDATION=1` when chasing a GPU
    /// problem; that path also surfaces every real validation message.
    pub fn default_instance() -> wgpu::Instance {
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
    /// but that never leaks to callers: constructors built on this stay
    /// synchronous by design (see the Milestone 1 plan).
    pub fn headless() -> Result<Self, GpuError> {
        pollster::block_on(Self::request(Self::default_instance(), None))
    }

    /// Acquire an adapter and device from `instance`. `compatible_surface`
    /// constrains adapter selection to one that can present to the given
    /// surface; `None` selects any available adapter (headless, or an
    /// offscreen renderer sharing a device that already has one).
    pub async fn request(
        instance: wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, GpuError> {
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
                label: Some("aether device"),
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
        assert!(!info.name.is_empty(), "adapter reported an empty name");
        println!("aether using adapter: {} ({:?})", info.name, info.backend);
    }
}
