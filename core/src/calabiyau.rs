//! Calabi–Yau cross-section, the classic Hanson visualization of the Fermat
//! quintic `z1^n + z2^n = 1`.
//!
//! The surface decomposes into `n x n` patches indexed by pairs of complex
//! n-th roots of unity. Each patch is a grid over `(a, b)` with
//!   z1 = e^{2πi k1/n} · cos(a + i·b)^{2/n}
//!   z2 = e^{2πi k2/n} · sin(a + i·b)^{2/n}
//! giving a point (Re z1, Im z1, Re z2, Im z2) in R^4. The 4D->3D drop blends
//! the two imaginary axes by `proj_angle` (Hanson's projection), which is the
//! knob you animate.

use crate::mesh::{Mesh, Vertex};
use glam::{Vec3, Vec4};
use num_complex::Complex32;
use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, Debug)]
pub struct CalabiYau {
    /// Degree of the quintic (5 is the canonical Calabi–Yau image; any n works).
    pub n: u32,
    /// Grid resolution per patch in each of the two parameters.
    pub res: usize,
    /// Hanson projection angle (radians) blending Im z1 / Im z2 into R^3.
    pub proj_angle: f32,
}

impl Default for CalabiYau {
    fn default() -> Self {
        Self {
            n: 5,
            res: 56,
            proj_angle: PI * 0.25,
        }
    }
}

impl CalabiYau {
    /// The raw R^4 embedding of one parameter sample on patch (k1, k2).
    pub fn point4(&self, k1: u32, k2: u32, a: f32, b: f32) -> Vec4 {
        let n = self.n as f32;
        let z = Complex32::new(a, b);
        let two_over_n = 2.0 / n;

        let phase = |k: u32| Complex32::from_polar(1.0, TAU * k as f32 / n);
        let z1 = phase(k1) * cpow(z.cos(), two_over_n);
        let z2 = phase(k2) * cpow(z.sin(), two_over_n);

        Vec4::new(z1.re, z1.im, z2.re, z2.im)
    }

    /// Drop R^4 -> R^3 using Hanson's imaginary-axis blend.
    fn project(&self, p: Vec4) -> Vec3 {
        let (s, c) = self.proj_angle.sin_cos();
        Vec3::new(p.x, p.z, p.y * c + p.w * s)
    }

    pub fn build(&self) -> Mesh {
        let res = self.res;
        let stride = res + 1;
        let mut mesh = Mesh::default();

        // a sweeps [0, π/2], b sweeps [-π/2, π/2]; outside this the quintic
        // patch parametrization repeats / diverges.
        let a_of = |i: usize| (i as f32 / res as f32) * (PI * 0.5);
        let b_of = |j: usize| (j as f32 / res as f32 - 0.5) * PI;

        for k1 in 0..self.n {
            for k2 in 0..self.n {
                let base = mesh.vertices.len() as u32;
                for i in 0..stride {
                    for j in 0..stride {
                        let p4 = self.point4(k1, k2, a_of(i), b_of(j));
                        let p3 = self.project(p4);
                        // Normal filled in by recompute_normals below.
                        mesh.vertices.push(Vertex::new4(p4, p3, Vec3::ZERO));
                    }
                }
                for i in 0..res {
                    for j in 0..res {
                        let a = base + (i * stride + j) as u32;
                        let b = base + (i * stride + j + 1) as u32;
                        let c = base + ((i + 1) * stride + j) as u32;
                        let d = base + ((i + 1) * stride + j + 1) as u32;
                        mesh.indices.extend_from_slice(&[a, c, b, b, c, d]);
                    }
                }
            }
        }

        mesh.recompute_normals();
        mesh
    }
}

/// Complex power w^e for real exponent `e`, via polar form. Returns 0 at the
/// origin (the `2/n` branch sends |w|=0 to 0, which is the surface boundary).
fn cpow(w: Complex32, e: f32) -> Complex32 {
    let (r, theta) = w.to_polar();
    if r <= f32::EPSILON {
        return Complex32::new(0.0, 0.0);
    }
    Complex32::from_polar(r.powf(e), theta * e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_is_finite_and_indexed() {
        let cy = CalabiYau::default();
        let m = cy.build();
        let patches = (cy.n * cy.n) as usize;
        let stride = cy.res + 1;
        assert_eq!(m.vertices.len(), patches * stride * stride);
        assert!(m.vertices.iter().all(|v| {
            v.position.iter().all(|c| c.is_finite()) && v.normal.iter().all(|c| c.is_finite())
        }));
        let max = m.vertices.len() as u32;
        assert!(m.indices.iter().all(|&i| i < max));
    }

    #[test]
    fn satisfies_quintic_constraint() {
        // z1^n + z2^n should equal 1 on the surface, by construction.
        let cy = CalabiYau::default();
        let n = cy.n as f32;
        let z = Complex32::new(0.6, -0.2);
        let z1 = Complex32::from_polar(1.0, 0.0) * cpow(z.cos(), 2.0 / n);
        let z2 = Complex32::from_polar(1.0, 0.0) * cpow(z.sin(), 2.0 / n);
        let sum = z1.powf(n) + z2.powf(n);
        assert!((sum.re - 1.0).abs() < 1e-3, "re={}", sum.re);
        assert!(sum.im.abs() < 1e-3, "im={}", sum.im);
    }
}
