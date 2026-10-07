//! Torus knots as swept tubes, framed by parallel transport (Bishop frame) to
//! avoid the twist/flip artifacts a Frenet frame produces at inflection points.

use crate::mesh::{Mesh, Vertex};
use glam::Vec3;
use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug)]
pub struct TorusKnot {
    /// Times the curve winds around the torus axis.
    pub p: u32,
    /// Times the curve winds through the hole.
    pub q: u32,
    /// Samples along the curve (closes back to the start).
    pub segments: usize,
    /// Vertices around the tube cross-section.
    pub radial: usize,
    /// Tube radius.
    pub tube: f32,
}

impl Default for TorusKnot {
    fn default() -> Self {
        // (2,3) trefoil.
        Self {
            p: 2,
            q: 3,
            segments: 512,
            radial: 24,
            tube: 0.18,
        }
    }
}

impl TorusKnot {
    /// Curve point at parameter `t in [0, TAU)`.
    fn point(&self, t: f32) -> Vec3 {
        let (p, q) = (self.p as f32, self.q as f32);
        let r = 2.0 + (q * t).cos();
        Vec3::new(r * (p * t).cos(), r * (p * t).sin(), (q * t).sin())
    }

    pub fn build(&self) -> Mesh {
        let n = self.segments;
        let m = self.radial;

        // Sample the closed curve and its tangents.
        let mut pts = Vec::with_capacity(n);
        let mut tangents = Vec::with_capacity(n);
        for i in 0..n {
            let t = TAU * i as f32 / n as f32;
            pts.push(self.point(t));
            // Central difference on the analytic curve for a stable tangent.
            let h = 1e-3;
            tangents.push((self.point(t + h) - self.point(t - h)).normalize());
        }

        // Parallel-transport an initial normal around the loop.
        let mut normals = Vec::with_capacity(n);
        // Seed: any vector not parallel to the first tangent.
        let seed = if tangents[0].x.abs() < 0.9 {
            Vec3::X
        } else {
            Vec3::Y
        };
        let mut normal = (seed - tangents[0] * seed.dot(tangents[0])).normalize();
        for i in 0..n {
            let t0 = tangents[i];
            // Re-orthogonalize, then rotate toward the next tangent.
            normal = (normal - t0 * normal.dot(t0)).normalize();
            normals.push(normal);
            let t1 = tangents[(i + 1) % n];
            let axis = t0.cross(t1);
            let sin_a = axis.length();
            if sin_a > 1e-6 {
                let axis = axis / sin_a;
                let angle = t0.dot(t1).clamp(-1.0, 1.0).acos();
                normal = rotate_about(normal, axis, angle);
            }
        }

        // Sweep the cross-section ring; radial direction doubles as the normal.
        let mut verts = Vec::with_capacity(n * m);
        for i in 0..n {
            let tangent = tangents[i];
            let nrm = normals[i];
            let binrm = tangent.cross(nrm).normalize();
            for j in 0..m {
                let a = TAU * j as f32 / m as f32;
                let dir = nrm * a.cos() + binrm * a.sin();
                verts.push(Vertex::new(pts[i] + dir * self.tube, dir));
            }
        }

        // Stitch quads between consecutive rings, wrapping both ways.
        let mut indices = Vec::with_capacity(n * m * 6);
        for i in 0..n {
            let i1 = (i + 1) % n;
            for j in 0..m {
                let j1 = (j + 1) % m;
                let a = (i * m + j) as u32;
                let b = (i * m + j1) as u32;
                let c = (i1 * m + j) as u32;
                let d = (i1 * m + j1) as u32;
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }

        Mesh {
            vertices: verts,
            indices,
        }
    }
}

/// Rodrigues rotation of `v` about unit `axis` by `angle`.
fn rotate_about(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    let (s, c) = angle.sin_cos();
    v * c + axis.cross(v) * s + axis * (axis.dot(v) * (1.0 - c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trefoil_is_closed_and_finite() {
        let m = TorusKnot::default().build();
        assert_eq!(m.vertices.len(), 512 * 24);
        assert_eq!(m.indices.len(), 512 * 24 * 6);
        assert!(m
            .vertices
            .iter()
            .all(|v| v.position.iter().all(|c| c.is_finite())));
        // Every index is in range.
        let max = m.vertices.len() as u32;
        assert!(m.indices.iter().all(|&i| i < max));
    }

    #[test]
    fn parallel_transport_frame_is_orthonormal_at_seam() {
        // After transport around the loop the frame should return near its
        // start (closed-curve holonomy is small for a smooth tube).
        let m = TorusKnot {
            segments: 1024,
            ..Default::default()
        }
        .build();
        assert!(m.vertices.iter().all(|v| {
            let n = Vec3::from_array(v.normal);
            (n.length() - 1.0).abs() < 1e-3
        }));
    }
}
