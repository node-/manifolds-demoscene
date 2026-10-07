//! Platform-agnostic mesh data. Backends upload `Vertex`/index slices verbatim.

use glam::Vec3;

/// Interleaved vertex. `#[repr(C)]` so the raw byte layout is what D3D12 / Metal
/// vertex layouts expect: position (12 B), R4 position (16 B), then normal
/// (12 B), stride 40 B.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub position4: [f32; 4],
    pub normal: [f32; 3],
}

impl Vertex {
    pub const STRIDE: usize = std::mem::size_of::<Self>();

    pub fn new(position: Vec3, normal: Vec3) -> Self {
        Self {
            position: position.to_array(),
            position4: position.extend(0.0).to_array(),
            normal: normal.normalize_or_zero().to_array(),
        }
    }

    pub fn new4(position4: glam::Vec4, position: Vec3, normal: Vec3) -> Self {
        Self {
            position: position.to_array(),
            position4: position4.to_array(),
            normal: normal.normalize_or_zero().to_array(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn index_count(&self) -> u32 {
        self.indices.len() as u32
    }

    /// Raw vertex bytes for a GPU upload.
    pub fn vertex_bytes(&self) -> &[u8] {
        // SAFETY: `Vertex` is `#[repr(C)]` and POD (only f32 arrays).
        unsafe {
            std::slice::from_raw_parts(
                self.vertices.as_ptr() as *const u8,
                std::mem::size_of_val(self.vertices.as_slice()),
            )
        }
    }

    pub fn index_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self.indices.as_ptr() as *const u8,
                std::mem::size_of_val(self.indices.as_slice()),
            )
        }
    }

    /// Recompute smooth normals by area-weighted face-normal accumulation.
    /// Used by surfaces (Calabi–Yau) where per-vertex normals aren't analytic.
    pub fn recompute_normals(&mut self) {
        let mut accum = vec![Vec3::ZERO; self.vertices.len()];
        for tri in self.indices.as_chunks::<3>().0 {
            let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let pa = Vec3::from_array(self.vertices[a].position);
            let pb = Vec3::from_array(self.vertices[b].position);
            let pc = Vec3::from_array(self.vertices[c].position);
            // Cross product magnitude is proportional to triangle area => weighting.
            let face = (pb - pa).cross(pc - pa);
            accum[a] += face;
            accum[b] += face;
            accum[c] += face;
        }
        for (v, n) in self.vertices.iter_mut().zip(accum) {
            v.normal = n.normalize_or_zero().to_array();
        }
    }
}
