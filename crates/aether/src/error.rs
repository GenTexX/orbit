//! aether error type: everything that can go wrong acquiring a GPU context.

/// An error acquiring an adapter or device.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// No graphics adapter could satisfy the requested options.
    #[error("failed to acquire a graphics adapter")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),

    /// An adapter was found, but a logical device could not be created from it.
    #[error("failed to create a graphics device")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
}
