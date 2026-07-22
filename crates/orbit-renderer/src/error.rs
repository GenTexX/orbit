//! orbit-renderer error type: every fallible renderer operation surfaces through `RendererError`.

/// Errors produced while creating or driving the renderer.
#[derive(Debug, thiserror::Error)]
pub enum RendererError {
    /// No graphics adapter could satisfy the requested options.
    #[error("failed to acquire a graphics adapter")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),

    /// An adapter was found, but a logical device could not be created from it.
    #[error("failed to create a graphics device")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
}
