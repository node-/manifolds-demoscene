//! Scene state: builds the active object's mesh once and produces per-frame
//! uniforms. Audio reactivity enters here as four band magnitudes, which are
//! each routed directly to an R4 axis scale or rotation-plane rate.

use crate::calabiyau::CalabiYau;
use crate::knot::TorusKnot;
use crate::mesh::Mesh;
use glam::{Mat4, Vec3};
use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug)]
pub enum Object {
    Trefoil,
    TorusKnot { p: u32, q: u32 },
    CalabiYau,
}

/// Number of analysis bands. One per R^4 rotation plane, so the band->plane
/// map can be the identity.
pub const NBANDS: usize = 6;

/// Uniform block shared with the GPU. `#[repr(C)]`, 16-byte aligned fields, so
/// the layout matches the HLSL `cbuffer` / MSL `constant` struct exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FrameUniforms {
    pub view_proj: [f32; 16],     // column-major mat4
    pub light_dir: [f32; 4],      // xyz + pad
    pub params: [f32; 4],         // time, master_level, object_kind, _
    pub audio_bands: [f32; 4],    // bands 0..4
    pub audio_bands_hi: [f32; 4], // bands 4,5,_,_
    pub dimension_drive: [f32; 4],
    pub plane_angles0: [f32; 4], // xy, xz, xw, yz
    pub plane_angles1: [f32; 4], // yw, zw, projection_depth, w_scale
    pub color_a: [f32; 4],
    pub color_b: [f32; 4],
    pub visual: [f32; 4],      // exposure, color_mix, dimension_pulse, _
    pub model_view: [f32; 16], // column-major; camera-space lighting
    pub light_fill: [f32; 4],  // camera-space fill dir xyz + intensity
    pub light_misc: [f32; 4],  // ambient, rim, spec, ao
    pub material: [f32; 4],    // base clay rgb
    pub bg: [f32; 4],          // clear colour rgb
}

impl FrameUniforms {
    pub fn bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                (self as *const Self) as *const u8,
                std::mem::size_of::<Self>(),
            )
        }
    }
}

#[derive(Clone, Debug)]
pub struct SceneControl {
    pub master_gain: f32,
    pub spin_base: f32,
    pub spin_audio: f32,
    pub projection_depth: f32,
    pub w_scale: f32,
    pub dimension_pulse: f32,
    pub camera_distance: f32,
    pub fov_deg: f32,
    pub exposure: f32,
    pub color_mix: f32,
    pub color_a: [f32; 3],
    pub color_b: [f32; 3],
    /// Axis breathing: scale_i = bias + gain * band[axis_band], one band per R^4 axis.
    pub axis_band: [usize; 4],
    pub axis_gain: [f32; 4],
    pub axis_bias: [f32; 4],
    /// Plane rotation rate (rad/s) = speed + gain * band[plane_band]; one band
    /// per R^4 rotation plane (xy, xz, xw, yz, yw, zw).
    pub plane_band: [usize; 6],
    pub plane_speed: [f32; 6],
    pub plane_gain: [f32; 6],
    /// Instant angle offset (rad) = swing * band. Unlike `plane_gain` this is
    /// positional, so a hit throws the plane open and it relaxes back.
    pub plane_swing: [f32; 6],
    /// Camera-space lighting rig.
    pub light_azimuth_deg: f32,
    pub light_elevation_deg: f32,
    pub light_key: f32,
    pub light_fill: f32,
    pub light_ambient: f32,
    pub light_rim: f32,
    pub light_spec: f32,
    pub light_ao: f32,
    pub material: [f32; 3],
    pub bg: [f32; 3],
}

impl Default for SceneControl {
    fn default() -> Self {
        Self {
            master_gain: 1.0,
            spin_base: 0.18,
            spin_audio: 1.4,
            projection_depth: 4.0,
            w_scale: 1.0,
            dimension_pulse: 0.22,
            camera_distance: 6.5,
            fov_deg: 60.0,
            exposure: 1.0,
            color_mix: 0.65,
            color_a: [0.08, 0.55, 0.95],
            color_b: [1.0, 0.28, 0.72],
            axis_band: [0, 1, 2, 3],
            axis_gain: [0.5; 4],
            axis_bias: [0.0; 4],
            plane_band: [0, 1, 2, 3, 4, 5],
            plane_speed: [0.0; 6],
            plane_gain: [0.0; 6],
            plane_swing: [0.0; 6],
            light_azimuth_deg: -40.0,
            light_elevation_deg: 45.0,
            light_key: 0.95,
            light_fill: 0.22,
            light_ambient: 0.42,
            light_rim: 0.09,
            light_spec: 0.07,
            light_ao: 0.35,
            material: [0.60, 0.60, 0.59],
            bg: [0.94, 0.935, 0.92],
        }
    }
}

pub struct Scene {
    pub object: Object,
    rot: f32,
    /// Per-plane accumulated angle. Integrating `speed * dt` (instead of
    /// `speed * time`) lets speeds change live without the angle jumping.
    plane_phase: [f32; 6],
    /// Mouse-driven orbit camera (radians) and zoom multiplier.
    yaw: f32,
    pitch: f32,
    zoom: f32,
}

impl Scene {
    pub fn new(object: Object) -> Self {
        Self {
            object,
            rot: 0.0,
            plane_phase: [0.0; 6],
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
        }
    }

    /// Orbit the camera by a pointer delta in radians.
    pub fn orbit(&mut self, d_yaw: f32, d_pitch: f32) {
        self.yaw = (self.yaw + d_yaw).rem_euclid(TAU);
        self.pitch = (self.pitch + d_pitch).clamp(-1.5, 1.5);
    }

    /// Multiplicative zoom; `factor` < 1 moves closer.
    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(0.25, 3.0);
    }

    pub fn build_mesh(&self) -> Mesh {
        match self.object {
            Object::Trefoil => TorusKnot::default().build(),
            Object::TorusKnot { p, q } => TorusKnot {
                p,
                q,
                ..Default::default()
            }
            .build(),
            Object::CalabiYau => CalabiYau::default().build(),
        }
    }

    /// Advance animation by `dt` seconds. `audio_bands` are normalized
    /// magnitudes in ~[0, 1] ordered low -> high.
    pub fn update(
        &mut self,
        dt: f32,
        audio_bands: [f32; NBANDS],
        time: f32,
        aspect: f32,
        control: &SceneControl,
    ) -> FrameUniforms {
        let bands = audio_bands.map(|v| (v * control.master_gain).clamp(0.0, 1.0));
        let master_level = (bands.iter().sum::<f32>() / NBANDS as f32).clamp(0.0, 1.0);
        self.rot += dt * (control.spin_base + control.spin_audio * master_level).max(0.0);

        // Band -> degree of freedom, directly. No mixing matrix.
        let mut dimension_drive = [0.0; 4];
        for i in 0..4 {
            dimension_drive[i] = control.axis_bias[i]
                + control.axis_gain[i] * bands[control.axis_band[i].min(NBANDS - 1)];
        }

        // Accumulate angle from the instantaneous rate so audio modulates
        // speed smoothly instead of snapping the angle.
        let mut plane_angles = [0.0; 6];
        for i in 0..6 {
            let rate = control.plane_speed[i]
                + control.plane_gain[i] * bands[control.plane_band[i].min(NBANDS - 1)];
            self.plane_phase[i] = (self.plane_phase[i] + dt * rate).rem_euclid(TAU);
            plane_angles[i] = self.plane_phase[i]
                + control.plane_swing[i] * bands[control.plane_band[i].min(NBANDS - 1)];
        }

        let model = Mat4::from_rotation_y(self.rot * 0.15) * Mat4::from_rotation_x(self.rot * 0.07);
        let dist = (control.camera_distance * self.zoom).clamp(1.0, 30.0);
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let eye = Vec3::new(sy * cp, sp, cy * cp) * dist;
        let view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        // glam `perspective_rh` uses [0,1] clip depth — the D3D12 / Metal convention.
        let proj = Mat4::perspective_rh(
            control.fov_deg.clamp(25.0, 110.0).to_radians(),
            aspect.max(0.1),
            0.1,
            100.0,
        );
        let model_view = view * model;
        let view_proj = proj * model_view;

        let dir = |az_deg: f32, el_deg: f32| {
            let (az, el) = (az_deg.to_radians(), el_deg.to_radians());
            Vec3::new(az.sin() * el.cos(), el.sin(), az.cos() * el.cos())
        };
        let key_dir = dir(control.light_azimuth_deg, control.light_elevation_deg);
        let fill_dir = dir(-control.light_azimuth_deg, 0.0);

        FrameUniforms {
            view_proj: view_proj.to_cols_array(),
            light_dir: key_dir.extend(control.light_key.clamp(0.0, 3.0)).to_array(),
            params: [
                time,
                master_level,
                match self.object {
                    Object::CalabiYau => 1.0,
                    _ => 0.0,
                },
                0.0,
            ],
            audio_bands: [bands[0], bands[1], bands[2], bands[3]],
            audio_bands_hi: [bands[4], bands[5], 0.0, 0.0],
            dimension_drive,
            plane_angles0: [
                plane_angles[0],
                plane_angles[1],
                plane_angles[2],
                plane_angles[3],
            ],
            plane_angles1: [
                plane_angles[4],
                plane_angles[5],
                control.projection_depth.clamp(1.5, 12.0),
                control.w_scale.clamp(0.0, 4.0),
            ],
            color_a: [
                control.color_a[0],
                control.color_a[1],
                control.color_a[2],
                1.0,
            ],
            color_b: [
                control.color_b[0],
                control.color_b[1],
                control.color_b[2],
                1.0,
            ],
            visual: [
                control.exposure.clamp(0.0, 4.0),
                control.color_mix.clamp(0.0, 1.0),
                control.dimension_pulse.clamp(0.0, 2.0),
                0.0,
            ],
            model_view: model_view.to_cols_array(),
            light_fill: fill_dir
                .extend(control.light_fill.clamp(0.0, 2.0))
                .to_array(),
            light_misc: [
                control.light_ambient.clamp(0.0, 2.0),
                control.light_rim.clamp(0.0, 1.0),
                control.light_spec.clamp(0.0, 1.0),
                control.light_ao.clamp(0.0, 1.0),
            ],
            material: [
                control.material[0],
                control.material[1],
                control.material[2],
                1.0,
            ],
            bg: [control.bg[0], control.bg[1], control.bg[2], 1.0],
        }
    }
}
