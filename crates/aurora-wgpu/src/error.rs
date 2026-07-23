//! aurora-wgpu error type.

/// Errors from creating or driving the aurora-wgpu renderer.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The GPU context (adapter or device) could not be acquired.
    #[error(transparent)]
    Gpu(#[from] aether::GpuError),

    /// A window surface could not be created from the given target.
    #[error("failed to create a window surface")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),

    /// The adapter cannot present to the surface with any default configuration.
    #[error("the surface is not supported by this adapter")]
    SurfaceUnsupported,

    /// Acquiring the next surface frame failed unrecoverably.
    #[error("failed to acquire the next surface frame")]
    SurfaceAcquire,
}
