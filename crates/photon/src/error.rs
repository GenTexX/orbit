//! photon error type: every fallible renderer operation surfaces through `RendererError`.

/// Errors produced while creating or driving the renderer.
#[derive(Debug, thiserror::Error)]
pub enum RendererError {
    /// The GPU context (adapter or device) could not be acquired.
    #[error(transparent)]
    Gpu(#[from] aether::GpuError),

    /// The GPU could not be polled to completion while reading back an image.
    #[error("failed to poll the device")]
    Poll(#[from] wgpu::PollError),

    /// A buffer could not be mapped for reading rendered pixels back to the CPU.
    #[error("failed to map the readback buffer")]
    Readback(#[from] wgpu::BufferAsyncError),

    /// A window surface could not be created from the given target.
    #[error("failed to create a window surface")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),

    /// The adapter cannot present to the surface with any default configuration.
    #[error("the surface is not supported by this adapter")]
    SurfaceUnsupported,

    /// A surface operation was requested on a headless (offscreen-only) renderer.
    #[error("this renderer has no window surface")]
    NoSurface,

    /// Acquiring the next surface frame failed unrecoverably.
    #[error("failed to acquire the next surface frame")]
    SurfaceAcquire,
}
