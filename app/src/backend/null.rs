//! Headless no-op backend for platforms without a native renderer (e.g. Linux
//! dev boxes / CI). It exists so the whole workspace compiles and the scene /
//! animation logic can be exercised without a GPU surface.

use super::Renderer;
use dscore::{scene::FrameUniforms, Mesh};
use winit::window::Window;

pub struct NullRenderer {
    index_count: u32,
    frame: u64,
}

impl NullRenderer {
    pub fn new_headless(mesh: &Mesh) -> Self {
        log::warn!(
            "no native GPU backend for this platform; running headless ({} indices)",
            mesh.index_count()
        );
        Self {
            index_count: mesh.index_count(),
            frame: 0,
        }
    }
}

impl Renderer for NullRenderer {
    fn new(_window: &Window, mesh: &Mesh) -> Self {
        Self::new_headless(mesh)
    }

    fn resize(&mut self, _width: u32, _height: u32) {}

    fn render(&mut self, uniforms: &FrameUniforms) {
        // Log occasionally so a headless run shows it's alive and the uniform
        // pipeline is producing sane values.
        if self.frame % 120 == 0 {
            log::info!(
                "headless frame {} | t={:.2} audio={:.2} | {} indices",
                self.frame,
                uniforms.params[0],
                uniforms.params[1],
                self.index_count
            );
        }
        self.frame += 1;
    }
}
