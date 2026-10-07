//! Higher-dimensional rotation + projection down to R^3.
//!
//! Objects are authored in R^4 (Calabi–Yau cross-sections live there naturally;
//! knots are lifted trivially). We apply an animated SO(4) rotation, then drop
//! to R^3. The R^3 -> R^2 step is the ordinary camera/perspective in the shader.

use glam::{Mat4, Vec3, Vec4};

/// A rotation in one of the six coordinate planes of R^4.
#[derive(Clone, Copy, Debug)]
pub enum Plane4 {
    Xy,
    Xz,
    Xw,
    Yz,
    Yw,
    Zw,
}

/// Build a 4x4 rotation by `angle` (radians) in the given coordinate plane.
pub fn rotation4(plane: Plane4, angle: f32) -> Mat4 {
    let (c, s) = (angle.cos(), angle.sin());
    let mut m = Mat4::IDENTITY;
    // glam Mat4 is column-major: m.col_mut(col)[row].
    let set = |m: &mut Mat4, row: usize, col: usize, v: f32| {
        m.col_mut(col)[row] = v;
    };
    let (a, b) = match plane {
        Plane4::Xy => (0, 1),
        Plane4::Xz => (0, 2),
        Plane4::Xw => (0, 3),
        Plane4::Yz => (1, 2),
        Plane4::Yw => (1, 3),
        Plane4::Zw => (2, 3),
    };
    set(&mut m, a, a, c);
    set(&mut m, b, b, c);
    set(&mut m, a, b, -s);
    set(&mut m, b, a, s);
    m
}

/// Project a point in R^4 to R^3 by perspective "w-divide" toward a 4D eye at
/// distance `d` along +w. Larger `d` -> closer to a parallel (orthographic)
/// drop of the w axis.
pub fn project_4_to_3(p: Vec4, d: f32) -> Vec3 {
    let denom = (d - p.w).max(1e-4);
    let s = d / denom;
    Vec3::new(p.x * s, p.y * s, p.z * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_preserves_length() {
        let v = Vec4::new(1.0, 2.0, -0.5, 0.3);
        let r = rotation4(Plane4::Xw, 0.7) * v;
        assert!((r.length() - v.length()).abs() < 1e-5);
    }

    #[test]
    fn rotation_zero_is_identity() {
        assert_eq!(rotation4(Plane4::Yz, 0.0), Mat4::IDENTITY);
    }
}
