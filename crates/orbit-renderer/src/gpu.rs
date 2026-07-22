//! orbit-renderer GPU context: owns the wgpu instance, adapter, device, and queue shared by every render path.

use crate::error::RendererError;

/// The wgpu objects shared by both the windowed and headless render paths.
///
/// Created without a surface for headless rendering (tests, offscreen targets),
/// or with a surface-compatible adapter for on-screen presentation. Fields
/// beyond `adapter` are consumed by later Milestone 1 steps (surface
/// configuration, buffer and texture creation), so they may sit unused for now.
#[allow(dead_code)]
pub(crate) struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// Create a headless GPU context with no surface, suitable for offscreen
    /// rendering and tests.
    ///
    /// Blocks until the device is acquired. wgpu's setup is async internally,
    /// but that never leaks to callers: the renderer's public constructors are
    /// synchronous by design (see the Milestone 1 plan).
    pub(crate) fn headless() -> Result<Self, RendererError> {
        pollster::block_on(Self::request(None))
    }

    /// Shared async setup for both render paths. `compatible_surface` constrains
    /// adapter selection to one that can present to the given surface; `None`
    /// selects any available adapter.
    async fn request(
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, RendererError> {
        // No display handle for the headless path; `_from_env` still honors
        // WGPU_BACKEND and friends for forcing a backend (e.g. Vulkan on CI).
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface,
                ..Default::default()
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("orbit-renderer device"),
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
        println!(
            "orbit-renderer using adapter: {} ({:?})",
            info.name, info.backend
        );
    }
}
