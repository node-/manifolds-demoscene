//! `demoscene-core` — platform-agnostic geometry and scene math.
//!
//! Nothing in this crate touches a GPU API; it produces `Mesh` data and
//! `FrameUniforms` that the D3D12 / Metal backends in `app` upload and draw.
//! That split is what lets the Metal backend stay correct without a Mac:
//! everything mathematically interesting is here, compiled and tested on every
//! platform.

pub mod calabiyau;
pub mod knot;
pub mod mesh;
pub mod projection;
pub mod scene;

pub use mesh::{Mesh, Vertex};
pub use scene::{FrameUniforms, Object, Scene, SceneControl};
