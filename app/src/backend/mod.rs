//! Backend selection. Each platform gets a raw, native renderer; there is no
//! cross-platform abstraction layer between us and the API — only this trait,
//! which is the seam the `main` loop talks to.
//!
//! - Windows -> `d3d12` (raw Direct3D 12 via the `windows` crate)
//! - macOS   -> `metal` (raw Metal via the `metal` crate)
//! - other   -> `null`  (headless; lets `core` + app logic build & test anywhere)

use dscore::{scene::FrameUniforms, Mesh};
use winit::window::Window;

/// The contract every backend implements. Deliberately tiny: upload a static
/// mesh once at construction, then draw it each frame with fresh uniforms.
pub trait Renderer {
    /// Create swapchain/device and upload `mesh`. `window` must outlive `self`.
    #[allow(dead_code)]
    fn new(window: &Window, mesh: &Mesh) -> Self
    where
        Self: Sized;

    #[allow(dead_code)]
    fn resize(&mut self, width: u32, height: u32);

    /// Presentation options, applied between frames. Default: ignored.
    #[allow(unused_variables)]
    fn set_options(&mut self, vsync: bool, msaa: bool) {}

    fn render(&mut self, uniforms: &FrameUniforms);
}

#[cfg(windows)]
mod d3d12;
#[cfg(windows)]
pub use d3d12::D3D12Renderer as ActiveRenderer;

#[cfg(target_os = "macos")]
mod metal;
#[cfg(target_os = "macos")]
pub use metal::MetalRenderer as ActiveRenderer;

#[cfg(not(any(windows, target_os = "macos")))]
mod null;
#[cfg(not(any(windows, target_os = "macos")))]
pub use null::NullRenderer as ActiveRenderer;
