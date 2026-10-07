//! Runtime control plane: shared parameter state, binary delta packets, UDP
//! receiver, and a tiny HTTP front-end.

use dscore::{
    scene::{FrameUniforms, NBANDS},
    SceneControl,
};
use std::{
    fmt::Write as _,
    io::{Read, Write},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

pub const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:34200";
pub const DEFAULT_UDP_ADDR: &str = "127.0.0.1:34201";
const MAGIC: &[u8; 4] = b"DSC1";
const VERSION: u8 = 1;
const KIND_PARAM_DELTAS: u8 = 1;
const HEADER_LEN: usize = 16;
const DELTA_LEN: usize = 6;
const MAX_DELTAS: usize = 256;

const AUDIO_MASTER_GAIN: usize = 0;
const AUDIO_FLOOR_DB: usize = 1;
const AUDIO_CEILING_DB: usize = 2;
const AUDIO_NOISE_GATE: usize = 3;
const SCENE_SPIN_BASE: usize = 4;
const SCENE_SPIN_AUDIO: usize = 5;
const CY_PROJECTION_DEPTH: usize = 6;
const CY_W_SCALE: usize = 7;
const CY_DIMENSION_PULSE: usize = 8;
const CAMERA_DISTANCE: usize = 9;
const CAMERA_FOV_DEG: usize = 10;
const VISUAL_EXPOSURE: usize = 11;
const VISUAL_COLOR_MIX: usize = 12;
const COLOR_A_R: usize = 13;
const COLOR_A_G: usize = 14;
const COLOR_A_B: usize = 15;
const COLOR_B_R: usize = 16;
const COLOR_B_G: usize = 17;
const COLOR_B_B: usize = 18;
const BAND_BASE: usize = 19;
const BAND_STRIDE: usize = 4;
/// Bands 0..4 sit at ids 19..35; bands 4 and 5 were added later at the end.
const BAND_EXTRA_BASE: usize = 91;

fn band_base(i: usize) -> usize {
    if i < 4 {
        BAND_BASE + i * BAND_STRIDE
    } else {
        BAND_EXTRA_BASE + (i - 4) * BAND_STRIDE
    }
}
const AXIS_BASE: usize = 35;
const AXIS_STRIDE: usize = 3;
const PLANE_BASE: usize = 47;
const PLANE_STRIDE: usize = 3;
const LIGHT_BASE: usize = 65;
const MATERIAL_BASE: usize = 73;
const BG_BASE: usize = 76;
const RENDER_VSYNC: usize = 79;
const RENDER_MSAA: usize = 80;
const AUDIO_TILT: usize = 81;
const AUDIO_AGC: usize = 82;
const AUDIO_AGC_RANGE: usize = 83;
const AUDIO_AGC_RELEASE: usize = 84;
const SWING_BASE: usize = 85;

#[derive(Clone, Copy)]
pub struct ParamSpec {
    pub id: u16,
    pub key: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub unit: &'static str,
    pub control: &'static str,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub default: f32,
}

macro_rules! param {
    ($id:expr, $key:expr, $label:expr, $group:expr, $unit:expr, $control:expr, $min:expr, $max:expr, $step:expr, $default:expr) => {
        ParamSpec {
            id: $id,
            key: $key,
            label: $label,
            group: $group,
            unit: $unit,
            control: $control,
            min: $min,
            max: $max,
            step: $step,
            default: $default,
        }
    };
}

pub const PARAMS: &[ParamSpec] = &[
    param!(
        0,
        "audio.master_gain",
        "Master gain",
        "Audio",
        "x",
        "knob",
        0.0,
        4.0,
        0.01,
        1.35
    ),
    param!(
        1,
        "audio.floor_db",
        "Floor",
        "Audio",
        "dB",
        "setpoint",
        -96.0,
        -12.0,
        0.5,
        -62.0
    ),
    param!(
        2,
        "audio.ceiling_db",
        "Ceiling",
        "Audio",
        "dB",
        "setpoint",
        -60.0,
        12.0,
        0.5,
        -18.0
    ),
    param!(
        3,
        "audio.noise_gate",
        "Noise gate",
        "Audio",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.015
    ),
    param!(
        4,
        "scene.spin_base",
        "Base spin",
        "Scene",
        "rad/s",
        "knob",
        0.0,
        2.0,
        0.001,
        0.18
    ),
    param!(
        5,
        "scene.spin_audio",
        "Audio spin",
        "Scene",
        "rad/s",
        "knob",
        0.0,
        5.0,
        0.001,
        1.4
    ),
    param!(
        6,
        "cy.projection_depth",
        "Projection depth",
        "Calabi-Yau",
        "",
        "slider",
        1.5,
        12.0,
        0.01,
        4.0
    ),
    param!(
        7,
        "cy.w_scale",
        "W scale",
        "Calabi-Yau",
        "x",
        "knob",
        0.0,
        4.0,
        0.01,
        1.0
    ),
    param!(
        8,
        "cy.dimension_pulse",
        "Dimension pulse",
        "Calabi-Yau",
        "x",
        "knob",
        0.0,
        1.5,
        0.01,
        0.22
    ),
    param!(
        9,
        "camera.distance",
        "Distance",
        "Camera",
        "",
        "slider",
        2.0,
        16.0,
        0.01,
        6.5
    ),
    param!(
        10,
        "camera.fov_deg",
        "FOV",
        "Camera",
        "deg",
        "slider",
        25.0,
        110.0,
        0.1,
        60.0
    ),
    param!(
        11,
        "visual.exposure",
        "Exposure",
        "Visual",
        "x",
        "knob",
        0.0,
        3.0,
        0.01,
        1.0
    ),
    param!(
        12,
        "visual.color_mix",
        "Color band mix",
        "Visual",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.65
    ),
    param!(
        13,
        "color.a.r",
        "Color A red",
        "Color A",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.08
    ),
    param!(
        14,
        "color.a.g",
        "Color A green",
        "Color A",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.55
    ),
    param!(
        15,
        "color.a.b",
        "Color A blue",
        "Color A",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.95
    ),
    param!(
        16,
        "color.b.r",
        "Color B red",
        "Color B",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        1.0
    ),
    param!(
        17,
        "color.b.g",
        "Color B green",
        "Color B",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.28
    ),
    param!(
        18,
        "color.b.b",
        "Color B blue",
        "Color B",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.72
    ),
    param!(
        19,
        "band.0.low_hz",
        "Band 1 low",
        "Band 1",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        25.0
    ),
    param!(
        20,
        "band.0.high_hz",
        "Band 1 high",
        "Band 1",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        90.0
    ),
    param!(
        21,
        "band.0.gain",
        "Band 1 gain",
        "Band 1",
        "x",
        "knob",
        0.0,
        6.0,
        0.01,
        1.05
    ),
    param!(
        22,
        "band.0.smooth_ms",
        "Band 1 smooth",
        "Band 1",
        "ms",
        "slider",
        0.0,
        500.0,
        1.0,
        60.0
    ),
    param!(
        23,
        "band.1.low_hz",
        "Band 2 low",
        "Band 2",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        90.0
    ),
    param!(
        24,
        "band.1.high_hz",
        "Band 2 high",
        "Band 2",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        250.0
    ),
    param!(
        25,
        "band.1.gain",
        "Band 2 gain",
        "Band 2",
        "x",
        "knob",
        0.0,
        6.0,
        0.01,
        0.94
    ),
    param!(
        26,
        "band.1.smooth_ms",
        "Band 2 smooth",
        "Band 2",
        "ms",
        "slider",
        0.0,
        500.0,
        1.0,
        50.0
    ),
    param!(
        27,
        "band.2.low_hz",
        "Band 3 low",
        "Band 3",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        250.0
    ),
    param!(
        28,
        "band.2.high_hz",
        "Band 3 high",
        "Band 3",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        700.0
    ),
    param!(
        29,
        "band.2.gain",
        "Band 3 gain",
        "Band 3",
        "x",
        "knob",
        0.0,
        6.0,
        0.01,
        0.8
    ),
    param!(
        30,
        "band.2.smooth_ms",
        "Band 3 smooth",
        "Band 3",
        "ms",
        "slider",
        0.0,
        500.0,
        1.0,
        45.0
    ),
    param!(
        31,
        "band.3.low_hz",
        "Band 4 low",
        "Band 4",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        700.0
    ),
    param!(
        32,
        "band.3.high_hz",
        "Band 4 high",
        "Band 4",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        2000.0
    ),
    param!(
        33,
        "band.3.gain",
        "Band 4 gain",
        "Band 4",
        "x",
        "knob",
        0.0,
        6.0,
        0.01,
        0.66
    ),
    param!(
        34,
        "band.3.smooth_ms",
        "Band 4 smooth",
        "Band 4",
        "ms",
        "slider",
        0.0,
        500.0,
        1.0,
        40.0
    ),
    param!(
        35,
        "axis.x.band",
        "X band",
        "Axis X",
        "",
        "setpoint",
        0.0,
        5.0,
        1.0,
        0.0
    ),
    param!(
        36,
        "axis.x.gain",
        "X gain",
        "Axis X",
        "x",
        "knob",
        -4.0,
        4.0,
        0.001,
        0.5
    ),
    param!(
        37,
        "axis.x.bias",
        "X bias",
        "Axis X",
        "",
        "slider",
        -1.0,
        1.0,
        0.001,
        0.0
    ),
    param!(
        38,
        "axis.y.band",
        "Y band",
        "Axis Y",
        "",
        "setpoint",
        0.0,
        5.0,
        1.0,
        1.0
    ),
    param!(
        39,
        "axis.y.gain",
        "Y gain",
        "Axis Y",
        "x",
        "knob",
        -4.0,
        4.0,
        0.001,
        0.5
    ),
    param!(
        40,
        "axis.y.bias",
        "Y bias",
        "Axis Y",
        "",
        "slider",
        -1.0,
        1.0,
        0.001,
        0.0
    ),
    param!(
        41,
        "axis.z.band",
        "Z band",
        "Axis Z",
        "",
        "setpoint",
        0.0,
        5.0,
        1.0,
        2.0
    ),
    param!(
        42,
        "axis.z.gain",
        "Z gain",
        "Axis Z",
        "x",
        "knob",
        -4.0,
        4.0,
        0.001,
        0.5
    ),
    param!(
        43,
        "axis.z.bias",
        "Z bias",
        "Axis Z",
        "",
        "slider",
        -1.0,
        1.0,
        0.001,
        0.0
    ),
    param!(
        44,
        "axis.w.band",
        "W band",
        "Axis W",
        "",
        "setpoint",
        0.0,
        5.0,
        1.0,
        3.0
    ),
    param!(
        45,
        "axis.w.gain",
        "W gain",
        "Axis W",
        "x",
        "knob",
        -4.0,
        4.0,
        0.001,
        0.5
    ),
    param!(
        46,
        "axis.w.bias",
        "W bias",
        "Axis W",
        "",
        "slider",
        -1.0,
        1.0,
        0.001,
        0.0
    ),
    param!(
        47,
        "plane.xy.band",
        "XY band",
        "Plane XY",
        "",
        "setpoint",
        0.0,
        5.0,
        1.0,
        0.0
    ),
    param!(
        48,
        "plane.xy.speed",
        "XY speed",
        "Plane XY",
        "rad/s",
        "knob",
        -4.0,
        4.0,
        0.001,
        0.0
    ),
    param!(
        49,
        "plane.xy.gain",
        "XY audio spin (rate, accumulates)",
        "Plane XY",
        "rad/s",
        "knob",
        -8.0,
        8.0,
        0.001,
        0.0
    ),
    param!(
        50,
        "plane.xz.band",
        "XZ band",
        "Plane XZ",
        "",
        "setpoint",
        0.0,
        5.0,
        1.0,
        1.0
    ),
    param!(
        51,
        "plane.xz.speed",
        "XZ speed",
        "Plane XZ",
        "rad/s",
        "knob",
        -4.0,
        4.0,
        0.001,
        0.0
    ),
    param!(
        52,
        "plane.xz.gain",
        "XZ audio spin (rate, accumulates)",
        "Plane XZ",
        "rad/s",
        "knob",
        -8.0,
        8.0,
        0.001,
        0.0
    ),
    param!(
        53,
        "plane.xw.band",
        "XW band",
        "Plane XW",
        "",
        "setpoint",
        0.0,
        5.0,
        1.0,
        2.0
    ),
    param!(
        54,
        "plane.xw.speed",
        "XW speed",
        "Plane XW",
        "rad/s",
        "knob",
        -4.0,
        4.0,
        0.001,
        0.0
    ),
    param!(
        55,
        "plane.xw.gain",
        "XW audio spin (rate, accumulates)",
        "Plane XW",
        "rad/s",
        "knob",
        -8.0,
        8.0,
        0.001,
        0.0
    ),
    param!(
        56,
        "plane.yz.band",
        "YZ band",
        "Plane YZ",
        "",
        "setpoint",
        0.0,
        5.0,
        1.0,
        3.0
    ),
    param!(
        57,
        "plane.yz.speed",
        "YZ speed",
        "Plane YZ",
        "rad/s",
        "knob",
        -4.0,
        4.0,
        0.001,
        0.0
    ),
    param!(
        58,
        "plane.yz.gain",
        "YZ audio spin (rate, accumulates)",
        "Plane YZ",
        "rad/s",
        "knob",
        -8.0,
        8.0,
        0.001,
        0.0
    ),
    param!(
        59,
        "plane.yw.band",
        "YW band",
        "Plane YW",
        "",
        "setpoint",
        0.0,
        5.0,
        1.0,
        4.0
    ),
    param!(
        60,
        "plane.yw.speed",
        "YW speed",
        "Plane YW",
        "rad/s",
        "knob",
        -4.0,
        4.0,
        0.001,
        0.0
    ),
    param!(
        61,
        "plane.yw.gain",
        "YW audio spin (rate, accumulates)",
        "Plane YW",
        "rad/s",
        "knob",
        -8.0,
        8.0,
        0.001,
        0.0
    ),
    param!(
        62,
        "plane.zw.band",
        "ZW band",
        "Plane ZW",
        "",
        "setpoint",
        0.0,
        5.0,
        1.0,
        5.0
    ),
    param!(
        63,
        "plane.zw.speed",
        "ZW speed",
        "Plane ZW",
        "rad/s",
        "knob",
        -4.0,
        4.0,
        0.001,
        0.0
    ),
    param!(
        64,
        "plane.zw.gain",
        "ZW audio spin (rate, accumulates)",
        "Plane ZW",
        "rad/s",
        "knob",
        -8.0,
        8.0,
        0.001,
        0.0
    ),
    param!(
        65,
        "light.azimuth",
        "Key azimuth",
        "Lighting",
        "deg",
        "knob",
        -180.0,
        180.0,
        1.0,
        -40.0
    ),
    param!(
        66,
        "light.elevation",
        "Key elevation",
        "Lighting",
        "deg",
        "knob",
        -20.0,
        90.0,
        1.0,
        45.0
    ),
    param!(
        67,
        "light.key",
        "Key level",
        "Lighting",
        "x",
        "slider",
        0.0,
        3.0,
        0.001,
        0.95
    ),
    param!(
        68,
        "light.fill",
        "Fill level",
        "Lighting",
        "x",
        "slider",
        0.0,
        2.0,
        0.001,
        0.22
    ),
    param!(
        69,
        "light.ambient",
        "Ambient",
        "Lighting",
        "x",
        "slider",
        0.0,
        2.0,
        0.001,
        0.42
    ),
    param!(
        70,
        "light.rim",
        "Rim",
        "Lighting",
        "x",
        "slider",
        0.0,
        1.0,
        0.001,
        0.09
    ),
    param!(
        71,
        "light.spec",
        "Specular",
        "Lighting",
        "x",
        "slider",
        0.0,
        1.0,
        0.001,
        0.07
    ),
    param!(
        72,
        "light.ao",
        "Curvature shade",
        "Lighting",
        "x",
        "slider",
        0.0,
        1.0,
        0.001,
        0.35
    ),
    param!(
        73,
        "material.r",
        "Clay R",
        "Material",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.6
    ),
    param!(
        74,
        "material.g",
        "Clay G",
        "Material",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.6
    ),
    param!(
        75,
        "material.b",
        "Clay B",
        "Material",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.59
    ),
    param!(
        76,
        "bg.r",
        "Bg R",
        "Background",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.94
    ),
    param!(
        77,
        "bg.g",
        "Bg G",
        "Background",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.935
    ),
    param!(
        78,
        "bg.b",
        "Bg B",
        "Background",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.92
    ),
    param!(
        79,
        "render.vsync",
        "VSync",
        "Render",
        "",
        "toggle",
        0.0,
        1.0,
        1.0,
        0.0
    ),
    param!(
        80,
        "render.msaa",
        "Anti-aliasing (4x MSAA)",
        "Render",
        "",
        "toggle",
        0.0,
        1.0,
        1.0,
        0.0
    ),
    param!(
        81,
        "audio.tilt_db_oct",
        "Spectral tilt",
        "Audio",
        "dB/oct",
        "knob",
        0.0,
        9.0,
        0.05,
        3.8
    ),
    param!(
        82,
        "audio.agc",
        "Auto-gain amount",
        "Audio",
        "",
        "slider",
        0.0,
        1.0,
        0.001,
        0.75
    ),
    param!(
        83,
        "audio.agc_range_db",
        "Auto-gain range",
        "Audio",
        "dB",
        "knob",
        6.0,
        60.0,
        0.5,
        32.0
    ),
    param!(
        84,
        "audio.agc_release_s",
        "Auto-gain release",
        "Audio",
        "s",
        "knob",
        0.1,
        30.0,
        0.1,
        3.0
    ),
    param!(
        85,
        "plane.xy.swing",
        "XY audio (follows level)",
        "Plane XY",
        "rad",
        "knob",
        -6.0,
        6.0,
        0.001,
        0.0
    ),
    param!(
        86,
        "plane.xz.swing",
        "XZ audio (follows level)",
        "Plane XZ",
        "rad",
        "knob",
        -6.0,
        6.0,
        0.001,
        0.0
    ),
    param!(
        87,
        "plane.xw.swing",
        "XW audio (follows level)",
        "Plane XW",
        "rad",
        "knob",
        -6.0,
        6.0,
        0.001,
        0.0
    ),
    param!(
        88,
        "plane.yz.swing",
        "YZ audio (follows level)",
        "Plane YZ",
        "rad",
        "knob",
        -6.0,
        6.0,
        0.001,
        0.0
    ),
    param!(
        89,
        "plane.yw.swing",
        "YW audio (follows level)",
        "Plane YW",
        "rad",
        "knob",
        -6.0,
        6.0,
        0.001,
        0.0
    ),
    param!(
        90,
        "plane.zw.swing",
        "ZW audio (follows level)",
        "Plane ZW",
        "rad",
        "knob",
        -6.0,
        6.0,
        0.001,
        0.0
    ),
    param!(
        91,
        "band.4.low_hz",
        "Band 5 low",
        "Band 5",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        2000.0
    ),
    param!(
        92,
        "band.4.high_hz",
        "Band 5 high",
        "Band 5",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        5500.0
    ),
    param!(
        93,
        "band.4.gain",
        "Band 5 gain",
        "Band 5",
        "x",
        "knob",
        0.0,
        6.0,
        0.001,
        0.86
    ),
    param!(
        94,
        "band.4.smooth_ms",
        "Band 5 smooth",
        "Band 5",
        "ms",
        "slider",
        0.0,
        500.0,
        1.0,
        35.0
    ),
    param!(
        95,
        "band.5.low_hz",
        "Band 6 low",
        "Band 6",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        5500.0
    ),
    param!(
        96,
        "band.5.high_hz",
        "Band 6 high",
        "Band 6",
        "Hz",
        "setpoint",
        10.0,
        20000.0,
        1.0,
        14000.0
    ),
    param!(
        97,
        "band.5.gain",
        "Band 6 gain",
        "Band 6",
        "x",
        "knob",
        0.0,
        6.0,
        0.001,
        0.9
    ),
    param!(
        98,
        "band.5.smooth_ms",
        "Band 6 smooth",
        "Band 6",
        "ms",
        "slider",
        0.0,
        500.0,
        1.0,
        30.0
    ),
];

#[derive(Clone, Copy, Debug)]
pub struct AudioBandConfig {
    pub low_hz: f32,
    pub high_hz: f32,
    pub gain: f32,
    pub smooth_ms: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct AudioControl {
    pub master_gain: f32,
    pub floor_db: f32,
    pub ceiling_db: f32,
    pub noise_gate: f32,
    /// dB/octave lift around 1 kHz to flatten the usual spectral tilt.
    pub tilt_db_per_oct: f32,
    /// 0 = fixed floor/ceiling window, 1 = fully per-band auto-gain.
    pub agc_amount: f32,
    pub agc_range_db: f32,
    pub agc_release_s: f32,
    pub bands: [AudioBandConfig; NBANDS],
}

#[derive(Clone, Debug)]
pub struct ControlSnapshot {
    values: Vec<f32>,
}

impl ControlSnapshot {
    fn value(&self, id: usize) -> f32 {
        self.values
            .get(id)
            .copied()
            .unwrap_or_else(|| PARAMS[id].default)
    }

    pub fn audio_control(&self) -> AudioControl {
        let mut bands = [AudioBandConfig {
            low_hz: 20.0,
            high_hz: 200.0,
            gain: 1.0,
            smooth_ms: 80.0,
        }; NBANDS];
        for (i, band) in bands.iter_mut().enumerate() {
            let base = band_base(i);
            let a = self.value(base);
            let b = self.value(base + 1);
            band.low_hz = a.min(b).max(1.0);
            band.high_hz = a.max(b).max(band.low_hz + 1.0);
            band.gain = self.value(base + 2);
            band.smooth_ms = self.value(base + 3);
        }
        AudioControl {
            master_gain: self.value(AUDIO_MASTER_GAIN),
            floor_db: self.value(AUDIO_FLOOR_DB),
            ceiling_db: self.value(AUDIO_CEILING_DB),
            noise_gate: self.value(AUDIO_NOISE_GATE),
            tilt_db_per_oct: self.value(AUDIO_TILT),
            agc_amount: self.value(AUDIO_AGC),
            agc_range_db: self.value(AUDIO_AGC_RANGE),
            agc_release_s: self.value(AUDIO_AGC_RELEASE),
            bands,
        }
    }

    /// Presentation options: (vsync, msaa). Both default off.
    pub fn render_options(&self) -> (bool, bool) {
        (
            self.value(RENDER_VSYNC) >= 0.5,
            self.value(RENDER_MSAA) >= 0.5,
        )
    }

    pub fn scene_control(&self) -> SceneControl {
        let mut control = SceneControl {
            master_gain: 1.0,
            spin_base: self.value(SCENE_SPIN_BASE),
            spin_audio: self.value(SCENE_SPIN_AUDIO),
            projection_depth: self.value(CY_PROJECTION_DEPTH),
            w_scale: self.value(CY_W_SCALE),
            dimension_pulse: self.value(CY_DIMENSION_PULSE),
            camera_distance: self.value(CAMERA_DISTANCE),
            fov_deg: self.value(CAMERA_FOV_DEG),
            exposure: self.value(VISUAL_EXPOSURE),
            color_mix: self.value(VISUAL_COLOR_MIX),
            color_a: [
                self.value(COLOR_A_R),
                self.value(COLOR_A_G),
                self.value(COLOR_A_B),
            ],
            color_b: [
                self.value(COLOR_B_R),
                self.value(COLOR_B_G),
                self.value(COLOR_B_B),
            ],
            ..SceneControl::default()
        };

        for i in 0..4 {
            let base = AXIS_BASE + i * AXIS_STRIDE;
            control.axis_band[i] =
                self.value(base).round().clamp(0.0, (NBANDS - 1) as f32) as usize;
            control.axis_gain[i] = self.value(base + 1);
            control.axis_bias[i] = self.value(base + 2);
        }

        for i in 0..6 {
            let base = PLANE_BASE + i * PLANE_STRIDE;
            control.plane_band[i] =
                self.value(base).round().clamp(0.0, (NBANDS - 1) as f32) as usize;
            control.plane_speed[i] = self.value(base + 1);
            control.plane_gain[i] = self.value(base + 2);
        }

        for i in 0..6 {
            control.plane_swing[i] = self.value(SWING_BASE + i);
        }

        control.light_azimuth_deg = self.value(LIGHT_BASE);
        control.light_elevation_deg = self.value(LIGHT_BASE + 1);
        control.light_key = self.value(LIGHT_BASE + 2);
        control.light_fill = self.value(LIGHT_BASE + 3);
        control.light_ambient = self.value(LIGHT_BASE + 4);
        control.light_rim = self.value(LIGHT_BASE + 5);
        control.light_spec = self.value(LIGHT_BASE + 6);
        control.light_ao = self.value(LIGHT_BASE + 7);
        for i in 0..3 {
            control.material[i] = self.value(MATERIAL_BASE + i);
            control.bg[i] = self.value(BG_BASE + i);
        }

        control
    }
}

#[derive(Clone, Copy, Default)]
struct Telemetry {
    audio_bands: [f32; NBANDS],
    dimension_drive: [f32; 4],
    master_level: f32,
}

struct State {
    values: Vec<f32>,
    seq: u32,
    telemetry: Telemetry,
}

#[derive(Clone)]
pub struct SharedControls {
    inner: Arc<Mutex<State>>,
}

impl SharedControls {
    pub fn new() -> Self {
        for (i, spec) in PARAMS.iter().enumerate() {
            debug_assert_eq!(i, spec.id as usize);
        }
        Self {
            inner: Arc::new(Mutex::new(State {
                values: PARAMS.iter().map(|p| p.default).collect(),
                seq: 0,
                telemetry: Telemetry::default(),
            })),
        }
    }

    pub fn snapshot(&self) -> ControlSnapshot {
        let state = self.inner.lock().unwrap();
        ControlSnapshot {
            values: state.values.clone(),
        }
    }

    pub fn set_telemetry(&self, uniforms: &FrameUniforms) {
        let mut state = self.inner.lock().unwrap();
        state.telemetry = Telemetry {
            audio_bands: [
                uniforms.audio_bands[0],
                uniforms.audio_bands[1],
                uniforms.audio_bands[2],
                uniforms.audio_bands[3],
                uniforms.audio_bands_hi[0],
                uniforms.audio_bands_hi[1],
            ],
            dimension_drive: uniforms.dimension_drive,
            master_level: uniforms.params[1],
        };
    }

    /// Write every parameter, keyed by name (stable across id renumbering).
    fn save_profile(&self, slot: u32) -> Result<usize, String> {
        let values = self.inner.lock().unwrap().values.clone();
        let mut out = String::new();
        let _ = write!(out, "{{\n \"name\": \"User {slot}\",\n \"params\": {{\n");
        for (i, p) in PARAMS.iter().enumerate() {
            let sep = if i + 1 < PARAMS.len() { "," } else { "" };
            let _ = writeln!(out, "  \"{}\": {}{}", json_escape(p.key), values[i], sep);
        }
        out.push_str(" }\n}\n");
        let path = profile_path(slot)?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&path, out).map_err(|e| e.to_string())?;
        log::info!("saved profile {slot} -> {}", path.display());
        Ok(PARAMS.len())
    }

    /// Apply a saved profile. Unknown keys and out-of-range values are skipped
    /// (so profiles survive schema changes); the count of applied params is returned.
    fn load_profile(&self, slot: u32) -> Result<usize, String> {
        let path = profile_path(slot)?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("no profile {slot} ({}): {e}", path.display()))?;
        let pairs = parse_profile(&text);
        let mut state = self.inner.lock().unwrap();
        let mut applied = 0;
        for (key, value) in pairs {
            if let Some(spec) = PARAMS.iter().find(|p| p.key == key) {
                if set_value(&mut state.values, spec.id, value).is_ok() {
                    applied += 1;
                }
            }
        }
        log::info!("loaded profile {slot}: {applied} params");
        Ok(applied)
    }

    fn apply_packet(&self, bytes: &[u8]) -> Result<u32, String> {
        let decoded = decode_packet(bytes)?;
        let mut state = self.inner.lock().unwrap();
        for (id, value) in decoded.deltas {
            set_value(&mut state.values, id, value)?;
        }
        state.seq = decoded.seq;
        Ok(state.seq)
    }

    fn schema_json(&self, http_addr: &str, udp_addr: &str) -> String {
        let mut out = String::new();
        out.push_str("{\"protocol\":{");
        let _ = write!(
            out,
            "\"magic\":\"DSC1\",\"version\":{},\"kind_param_deltas\":{},\"header_len\":{},\"delta_len\":{},\"delta_ms\":5,\"http_addr\":\"{}\",\"udp_addr\":\"{}\",\"http_control_path\":\"/control\"",
            VERSION,
            KIND_PARAM_DELTAS,
            HEADER_LEN,
            DELTA_LEN,
            json_escape(http_addr),
            json_escape(udp_addr)
        );
        out.push_str("},\"params\":[");
        for (i, p) in PARAMS.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"id\":{},\"key\":\"{}\",\"label\":\"{}\",\"group\":\"{}\",\"unit\":\"{}\",\"control\":\"{}\",\"min\":{},\"max\":{},\"step\":{},\"default\":{}}}",
                p.id,
                json_escape(p.key),
                json_escape(p.label),
                json_escape(p.group),
                json_escape(p.unit),
                json_escape(p.control),
                p.min,
                p.max,
                p.step,
                p.default
            );
        }
        out.push_str("]}");
        out
    }

    fn state_json(&self) -> String {
        let state = self.inner.lock().unwrap();
        let mut out = String::new();
        let _ = write!(out, "{{\"seq\":{},\"values\":[", state.seq);
        for (i, value) in state.values.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "{value}");
        }
        out.push_str("],\"telemetry\":{\"audio_bands\":[");
        for (i, value) in state.telemetry.audio_bands.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "{value}");
        }
        out.push_str("],\"dimension_drive\":[");
        for (i, value) in state.telemetry.dimension_drive.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "{value}");
        }
        let _ = write!(
            out,
            "],\"master_level\":{}}}}}",
            state.telemetry.master_level
        );
        out
    }
}

pub fn start_servers(shared: SharedControls) {
    let http_addr =
        std::env::var("DEMOSCENE_HTTP_ADDR").unwrap_or_else(|_| DEFAULT_HTTP_ADDR.to_string());
    let udp_addr =
        std::env::var("DEMOSCENE_UDP_ADDR").unwrap_or_else(|_| DEFAULT_UDP_ADDR.to_string());

    start_udp(shared.clone(), udp_addr.clone());
    start_http(shared, http_addr, udp_addr);
}

fn start_udp(shared: SharedControls, udp_addr: String) {
    thread::spawn(move || {
        let socket = match UdpSocket::bind(&udp_addr) {
            Ok(socket) => socket,
            Err(err) => {
                log::error!("control UDP bind failed on {udp_addr}: {err}");
                return;
            }
        };
        log::info!("control UDP listening on udp://{udp_addr}");
        let mut buf = [0u8; 2048];
        loop {
            match socket.recv_from(&mut buf) {
                Ok((len, _peer)) => {
                    if let Err(err) = shared.apply_packet(&buf[..len]) {
                        log::warn!("discarded control UDP packet: {err}");
                    }
                }
                Err(err) => log::warn!("control UDP receive failed: {err}"),
            }
        }
    });
}

fn start_http(shared: SharedControls, http_addr: String, udp_addr: String) {
    thread::spawn(move || {
        let listener = match TcpListener::bind(&http_addr) {
            Ok(listener) => listener,
            Err(err) => {
                log::error!("control HTTP bind failed on {http_addr}: {err}");
                return;
            }
        };
        log::info!("control HTTP listening on http://{http_addr}");
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let shared = shared.clone();
                    let http_addr = http_addr.clone();
                    let udp_addr = udp_addr.clone();
                    thread::spawn(move || {
                        if let Err(err) = handle_http(stream, shared, &http_addr, &udp_addr) {
                            log::warn!("control HTTP request failed: {err}");
                        }
                    });
                }
                Err(err) => log::warn!("control HTTP accept failed: {err}"),
            }
        }
    });
}

fn profile_path(slot: u32) -> Result<std::path::PathBuf, String> {
    if !(1..=99).contains(&slot) {
        return Err(format!("profile slot {slot} out of range 1..=99"));
    }
    let dir = std::env::var("DEMOSCENE_PROFILE_DIR").unwrap_or_else(|_| "profiles".to_string());
    Ok(std::path::Path::new(&dir).join(format!("user-{slot}.json")))
}

/// Minimal reader for the flat `"key": number` pairs in a profile file.
fn parse_profile(text: &str) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let Some(len) = text[start..].find('"') else {
                break;
            };
            let key = &text[start..start + len];
            i = start + len + 1;
            let rest = text[i..].trim_start();
            if let Some(after) = rest.strip_prefix(':') {
                let after = after.trim_start();
                let end = after
                    .find(|c: char| !(c.is_ascii_digit() || "+-.eE".contains(c)))
                    .unwrap_or(after.len());
                if let Ok(v) = after[..end].parse::<f32>() {
                    out.push((key.to_string(), v));
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

fn profile_slot(path: &str, prefix: &str) -> Option<u32> {
    path.strip_prefix(prefix)?.parse().ok()
}

fn handle_http(
    mut stream: TcpStream,
    shared: SharedControls,
    http_addr: &str,
    udp_addr: &str,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|err| err.to_string())?;
    let request = read_http_request(&mut stream)?;
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => write_response(
            &mut stream,
            200,
            "text/html; charset=utf-8",
            INDEX_HTML.as_bytes(),
        ),
        ("GET", "/schema.json") => write_response(
            &mut stream,
            200,
            "application/json",
            shared.schema_json(http_addr, udp_addr).as_bytes(),
        ),
        ("GET", "/state.json") => write_response(
            &mut stream,
            200,
            "application/json",
            shared.state_json().as_bytes(),
        ),
        ("POST", "/control") => match shared.apply_packet(&request.body) {
            Ok(seq) => write_response(
                &mut stream,
                200,
                "application/json",
                format!("{{\"ok\":true,\"seq\":{seq}}}").as_bytes(),
            ),
            Err(err) => write_response(
                &mut stream,
                400,
                "application/json",
                format!("{{\"ok\":false,\"error\":\"{}\"}}", json_escape(&err)).as_bytes(),
            ),
        },
        ("GET", "/profiles.json") => {
            let slots: Vec<String> = (1..=8)
                .filter(|s| profile_path(*s).map(|p| p.exists()).unwrap_or(false))
                .map(|s| s.to_string())
                .collect();
            write_response(
                &mut stream,
                200,
                "application/json",
                format!("{{\"slots\":[{}]}}", slots.join(",")).as_bytes(),
            )
        }
        ("POST", path)
            if path.starts_with("/profile/save/") || path.starts_with("/profile/load/") =>
        {
            let saving = path.starts_with("/profile/save/");
            let prefix = if saving {
                "/profile/save/"
            } else {
                "/profile/load/"
            };
            let result = match profile_slot(path, prefix) {
                None => Err("bad profile slot".to_string()),
                Some(slot) if saving => shared.save_profile(slot),
                Some(slot) => shared.load_profile(slot),
            };
            match result {
                Ok(n) => write_response(
                    &mut stream,
                    200,
                    "application/json",
                    format!("{{\"ok\":true,\"params\":{n}}}").as_bytes(),
                ),
                Err(err) => write_response(
                    &mut stream,
                    400,
                    "application/json",
                    format!("{{\"ok\":false,\"error\":\"{}\"}}", json_escape(&err)).as_bytes(),
                ),
            }
        }
        ("OPTIONS", _) => write_response(&mut stream, 204, "text/plain", &[]),
        _ => write_response(&mut stream, 404, "text/plain", b"not found"),
    }
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).map_err(|err| err.to_string())?;
        if n == 0 {
            return Err("client closed before headers".to_string());
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 128 * 1024 {
            return Err("headers too large".to_string());
        }
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
    };

    let header = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = header.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "missing request line".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "missing method".to_string())?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| "missing path".to_string())?
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string();

    let mut content_len = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_len = value
                    .trim()
                    .parse()
                    .map_err(|_| "bad content-length".to_string())?;
            }
        }
    }

    let body_start = header_end + 4;
    let mut body = buf[body_start..].to_vec();
    while body.len() < content_len {
        let n = stream.read(&mut tmp).map_err(|err| err.to_string())?;
        if n == 0 {
            return Err("client closed before body".to_string());
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_len);
    Ok(HttpRequest { method, path, body })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(headers.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|err| err.to_string())
}

struct DecodedPacket {
    seq: u32,
    deltas: Vec<(u16, f32)>,
}

fn decode_packet(bytes: &[u8]) -> Result<DecodedPacket, String> {
    if bytes.len() < HEADER_LEN {
        return Err("packet shorter than header".to_string());
    }
    if &bytes[0..4] != MAGIC {
        return Err("bad magic".to_string());
    }
    if bytes[4] != VERSION {
        return Err(format!("unsupported version {}", bytes[4]));
    }
    if bytes[5] != KIND_PARAM_DELTAS {
        return Err(format!("unsupported packet kind {}", bytes[5]));
    }
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    if count > MAX_DELTAS {
        return Err("too many deltas".to_string());
    }
    let seq = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let checksum = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let expected_len = HEADER_LEN + count * DELTA_LEN;
    if bytes.len() != expected_len {
        return Err(format!(
            "bad packet length {}, expected {}",
            bytes.len(),
            expected_len
        ));
    }
    let payload = &bytes[HEADER_LEN..];
    let actual = fnv1a32(payload);
    if checksum != actual {
        return Err("payload checksum mismatch".to_string());
    }

    let mut deltas = Vec::with_capacity(count);
    for chunk in payload.chunks_exact(DELTA_LEN) {
        let id = u16::from_le_bytes([chunk[0], chunk[1]]);
        let value = f32::from_le_bytes([chunk[2], chunk[3], chunk[4], chunk[5]]);
        validate_value(id, value)?;
        deltas.push((id, value));
    }

    Ok(DecodedPacket { seq, deltas })
}

fn set_value(values: &mut [f32], id: u16, value: f32) -> Result<(), String> {
    validate_value(id, value)?;
    values[id as usize] = value;
    Ok(())
}

fn validate_value(id: u16, value: f32) -> Result<(), String> {
    let Some(spec) = PARAMS.get(id as usize) else {
        return Err(format!("unknown param id {id}"));
    };
    if !value.is_finite() {
        return Err(format!("param {} is not finite", spec.key));
    }
    if value < spec.min || value > spec.max {
        return Err(format!(
            "param {}={} out of range [{}, {}]",
            spec.key, value, spec.min, spec.max
        ));
    }
    Ok(())
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>demoscene control</title>
<style>
:root {
  color-scheme: dark;
  font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  background: #111317;
  color: #e6ebf2;
}
* { box-sizing: border-box; }
body { margin: 0; min-height: 100vh; background: #111317; }
main { max-width: 1480px; margin: 0 auto; padding: 18px; }
.topbar {
  display: grid;
  grid-template-columns: 1fr auto;
  gap: 16px;
  align-items: center;
  margin-bottom: 16px;
}
h1 { font-size: 22px; line-height: 1.1; margin: 0; font-weight: 680; letter-spacing: 0; }
.meta { display: flex; gap: 8px; flex-wrap: wrap; justify-content: end; }
.pill { padding: 6px 9px; border: 1px solid #303844; border-radius: 6px; color: #aeb8c6; background: #171b21; font-size: 12px; }
.presets { display: grid; gap: 8px; margin-bottom: 12px; }
.preset-row { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.preset-title { width: 110px; color: #8794a6; font-size: 12px; text-transform: uppercase; letter-spacing: .04em; }
.presets button { background: #171b21; color: #cfd8e3; border: 1px solid #303844; border-radius: 6px; padding: 6px 10px; font-size: 12px; cursor: pointer; }
.presets button:disabled, #profiles button:disabled { opacity: .35; cursor: default; }
#profiles button { background: #171b21; color: #cfd8e3; border: 1px solid #303844; border-radius: 6px; padding: 6px 10px; font-size: 12px; cursor: pointer; }
.presets button:hover { background: #232a32; border-color: #4db6ac; }
.meters { display: grid; grid-template-columns: repeat(7, minmax(80px, 1fr)); gap: 8px; margin-bottom: 16px; }
.meter { background: #171b21; border: 1px solid #2b333d; border-radius: 6px; padding: 8px; min-width: 0; }
.meter label { display: flex; justify-content: space-between; color: #aeb8c6; font-size: 12px; margin-bottom: 6px; }
.bar { height: 8px; background: #232a32; border-radius: 4px; overflow: hidden; }
.bar span { display: block; height: 100%; width: 0; background: #4db6ac; }
.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(310px, 1fr)); gap: 12px; align-items: start; }
section { border-top: 1px solid #2b333d; padding-top: 12px; min-width: 0; }
section h2 { margin: 0 0 8px; font-size: 14px; color: #d6dde8; font-weight: 650; letter-spacing: 0; }
.control {
  display: grid;
  grid-template-columns: minmax(105px, 1fr) minmax(120px, 1.6fr) 82px;
  gap: 8px;
  align-items: center;
  min-height: 34px;
  padding: 5px 0;
}
.control label { color: #aeb8c6; font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
input[type="range"] { width: 100%; accent-color: #4db6ac; }
input[type="number"] {
  width: 82px;
  padding: 5px 6px;
  background: #171b21;
  border: 1px solid #303844;
  border-radius: 5px;
  color: #e6ebf2;
  font: inherit;
  font-size: 12px;
}
.knob input[type="range"] { accent-color: #f0b35a; }
.setpoint input[type="range"] { accent-color: #7ca7ff; }
@media (max-width: 720px) {
  main { padding: 12px; }
  .topbar { grid-template-columns: 1fr; }
  .meta { justify-content: start; }
  .meters { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .control { grid-template-columns: 1fr; gap: 4px; padding: 8px 0; }
  input[type="number"] { width: 100%; }
}
</style>
</head>
<body>
<main>
  <div class="topbar">
    <h1>demoscene control</h1>
    <div class="meta">
      <span class="pill" id="seq">seq 0</span>
      <span class="pill" id="net">loading</span>
    </div>
  </div>
  <div class="presets" id="presets"></div>
  <div class="preset-row" id="profiles" style="margin-bottom:12px"></div>
  <div class="meters" id="meters"></div>
  <div class="grid" id="controls"></div>
</main>
<script>
const MAGIC = [0x44, 0x53, 0x43, 0x31];
const VERSION = 1;
const KIND_PARAM_DELTAS = 1;
const pending = new Map();
let schema;
let values = [];
let seq = 1;
let focusedId = null;

function fnv1a(bytes) {
  let hash = 0x811c9dc5;
  for (const b of bytes) {
    hash ^= b;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash >>> 0;
}

function queue(id, value) {
  pending.set(id, value);
}

async function flush() {
  if (!pending.size) return;
  const entries = Array.from(pending.entries());
  pending.clear();
  const payloadLen = entries.length * 6;
  const buffer = new ArrayBuffer(16 + payloadLen);
  const bytes = new Uint8Array(buffer);
  const view = new DataView(buffer);
  bytes.set(MAGIC, 0);
  view.setUint8(4, VERSION);
  view.setUint8(5, KIND_PARAM_DELTAS);
  view.setUint16(6, entries.length, true);
  view.setUint32(8, seq++, true);
  let off = 16;
  for (const [id, value] of entries) {
    view.setUint16(off, id, true);
    view.setFloat32(off + 2, value, true);
    off += 6;
  }
  view.setUint32(12, fnv1a(bytes.slice(16)), true);
  try {
    const res = await fetch("/control", {
      method: "POST",
      headers: { "Content-Type": "application/x-demoscene-control-v1" },
      body: buffer
    });
    document.getElementById("net").textContent = res.ok ? "http control ok" : "http control error";
  } catch (_err) {
    document.getElementById("net").textContent = "http offline";
  }
}

function addMeter(name, index) {
  const el = document.createElement("div");
  el.className = "meter";
  el.innerHTML = `<label><span>${name}</span><span id="meter-value-${index}">0.00</span></label><div class="bar"><span id="meter-bar-${index}"></span></div>`;
  document.getElementById("meters").appendChild(el);
}


const LOOK_PRESETS = {
  "Clay studio":   {"light.azimuth":-40,"light.elevation":45,"light.key":0.95,"light.fill":0.22,"light.ambient":0.42,"light.rim":0.09,"light.spec":0.07,"light.ao":0.35,"material.r":0.60,"material.g":0.60,"material.b":0.59,"bg.r":0.94,"bg.g":0.935,"bg.b":0.92,"visual.exposure":1.0,"visual.color_mix":0.0},
  "Porcelain":     {"light.azimuth":-25,"light.elevation":60,"light.key":0.85,"light.fill":0.45,"light.ambient":0.65,"light.rim":0.05,"light.spec":0.35,"light.ao":0.25,"material.r":0.93,"material.g":0.92,"material.b":0.90,"bg.r":0.97,"bg.g":0.97,"bg.b":0.97,"visual.exposure":1.0,"visual.color_mix":0.0},
  "Graphite":      {"light.azimuth":-55,"light.elevation":35,"light.key":1.3,"light.fill":0.12,"light.ambient":0.22,"light.rim":0.35,"light.spec":0.30,"light.ao":0.50,"material.r":0.25,"material.g":0.26,"material.b":0.28,"bg.r":0.10,"bg.g":0.10,"bg.b":0.11,"visual.exposure":1.1,"visual.color_mix":0.0},
  "Midnight neon": {"light.azimuth":60,"light.elevation":20,"light.key":1.1,"light.fill":0.5,"light.ambient":0.12,"light.rim":0.8,"light.spec":0.5,"light.ao":0.45,"material.r":0.20,"material.g":0.22,"material.b":0.35,"bg.r":0.02,"bg.g":0.02,"bg.b":0.05,"visual.exposure":1.3,"visual.color_mix":1.0,"color.a.r":0.05,"color.a.g":0.75,"color.a.b":1.0,"color.b.r":1.0,"color.b.g":0.15,"color.b.b":0.75},
  "Warm gallery":  {"light.azimuth":-70,"light.elevation":30,"light.key":1.15,"light.fill":0.15,"light.ambient":0.30,"light.rim":0.15,"light.spec":0.10,"light.ao":0.40,"material.r":0.80,"material.g":0.66,"material.b":0.52,"bg.r":0.22,"bg.g":0.17,"bg.b":0.14,"visual.exposure":1.1,"visual.color_mix":0.0},
  "Bronze":        {"light.azimuth":-35,"light.elevation":50,"light.key":1.2,"light.fill":0.2,"light.ambient":0.28,"light.rim":0.25,"light.spec":0.55,"light.ao":0.40,"material.r":0.62,"material.g":0.40,"material.b":0.20,"bg.r":0.08,"bg.g":0.07,"bg.b":0.06,"visual.exposure":1.1,"visual.color_mix":0.0},
  "Ice":           {"light.azimuth":30,"light.elevation":55,"light.key":0.9,"light.fill":0.5,"light.ambient":0.55,"light.rim":0.45,"light.spec":0.40,"light.ao":0.30,"material.r":0.62,"material.g":0.78,"material.b":0.90,"bg.r":0.86,"bg.g":0.92,"bg.b":0.97,"visual.exposure":1.0,"visual.color_mix":0.0},
  "Rim noir":      {"light.azimuth":150,"light.elevation":25,"light.key":0.5,"light.fill":0.0,"light.ambient":0.05,"light.rim":1.0,"light.spec":0.30,"light.ao":0.60,"material.r":0.35,"material.g":0.35,"material.b":0.36,"bg.r":0.0,"bg.g":0.0,"bg.b":0.0,"visual.exposure":1.4,"visual.color_mix":0.0},
  "Audio tint":    {"light.azimuth":-40,"light.elevation":45,"light.key":0.95,"light.fill":0.3,"light.ambient":0.40,"light.rim":0.2,"light.spec":0.12,"light.ao":0.35,"material.r":0.60,"material.g":0.60,"material.b":0.59,"bg.r":0.06,"bg.g":0.06,"bg.b":0.08,"visual.exposure":1.1,"visual.color_mix":1.0,"color.a.r":0.08,"color.a.g":0.55,"color.a.b":0.95,"color.b.r":1.0,"color.b.g":0.28,"color.b.b":0.72}
};

const AUDIO_PRESETS = {
  "Balanced": {"audio.tilt_db_oct":3.8,"audio.agc":0.75,"audio.agc_range_db":32,"audio.agc_release_s":3.0,"band.0.gain":1.05,"band.0.smooth_ms":60,"band.1.gain":0.94,"band.1.smooth_ms":50,"band.2.gain":0.8,"band.2.smooth_ms":45,"band.3.gain":0.66,"band.3.smooth_ms":40,"band.4.gain":0.86,"band.4.smooth_ms":35,"band.5.gain":0.9,"band.5.smooth_ms":30},
  "Kick-forward": {"audio.tilt_db_oct":2.6,"audio.agc":0.5,"audio.agc_range_db":36,"audio.agc_release_s":3.0,"band.0.gain":1.0,"band.0.smooth_ms":70,"band.1.gain":1.0,"band.1.smooth_ms":55,"band.2.gain":0.9,"band.2.smooth_ms":50,"band.3.gain":0.9,"band.3.smooth_ms":45,"band.4.gain":0.95,"band.4.smooth_ms":40,"band.5.gain":1.2,"band.5.smooth_ms":40},
  "Bright": {"audio.tilt_db_oct":5.2,"audio.agc":0.8,"audio.agc_range_db":30,"audio.agc_release_s":3.0,"band.0.gain":1.0,"band.0.smooth_ms":55,"band.1.gain":1.0,"band.1.smooth_ms":45,"band.2.gain":0.95,"band.2.smooth_ms":40,"band.3.gain":0.9,"band.3.smooth_ms":35,"band.4.gain":0.95,"band.4.smooth_ms":30,"band.5.gain":0.95,"band.5.smooth_ms":28},
  "Tight": {"audio.tilt_db_oct":3.8,"audio.agc":0.75,"audio.agc_range_db":32,"audio.agc_release_s":1.5,"band.0.gain":1.05,"band.0.smooth_ms":22,"band.1.gain":0.94,"band.1.smooth_ms":18,"band.2.gain":0.8,"band.2.smooth_ms":15,"band.3.gain":0.66,"band.3.smooth_ms":14,"band.4.gain":0.86,"band.4.smooth_ms":12,"band.5.gain":0.9,"band.5.smooth_ms":12},
  "Fluid": {"audio.tilt_db_oct":3.8,"audio.agc":0.75,"audio.agc_range_db":32,"audio.agc_release_s":6.0,"band.0.gain":1.05,"band.0.smooth_ms":160,"band.1.gain":0.94,"band.1.smooth_ms":140,"band.2.gain":0.8,"band.2.smooth_ms":120,"band.3.gain":0.66,"band.3.smooth_ms":110,"band.4.gain":0.86,"band.4.smooth_ms":100,"band.5.gain":0.9,"band.5.smooth_ms":90}
};
const MOTION_PRESETS = {
  "Hyper pump": {"plane.xy.band":3,"plane.xy.swing":0.31,"plane.xy.speed":0.03,"plane.xy.gain":0,"plane.xz.band":4,"plane.xz.swing":0.96,"plane.xz.speed":0.03,"plane.xz.gain":0,"plane.xw.band":0,"plane.xw.swing":2.61,"plane.xw.speed":0.03,"plane.xw.gain":0,"plane.yz.band":5,"plane.yz.swing":0.52,"plane.yz.speed":0.03,"plane.yz.gain":0,"plane.yw.band":1,"plane.yw.swing":2.47,"plane.yw.speed":0.03,"plane.yw.gain":0,"plane.zw.band":2,"plane.zw.swing":0.65,"plane.zw.speed":0.03,"plane.zw.gain":0,"axis.x.band":0,"axis.x.gain":0.25,"axis.x.bias":0.0,"axis.y.band":1,"axis.y.gain":0.25,"axis.y.bias":0.0,"axis.z.band":2,"axis.z.gain":0.25,"axis.z.bias":0.0,"axis.w.band":3,"axis.w.gain":0.25,"axis.w.bias":0.0,"cy.projection_depth":5.3,"cy.w_scale":2.05,"cy.dimension_pulse":0.2,"camera.distance":8.0},
  "Ladder warp": {"plane.xy.band":0,"plane.xy.swing":2.87,"plane.xy.speed":0.04,"plane.xy.gain":0,"plane.xz.band":1,"plane.xz.swing":0.41,"plane.xz.speed":0.04,"plane.xz.gain":0,"plane.xw.band":2,"plane.xw.swing":3.2,"plane.xw.speed":0.04,"plane.xw.gain":0,"plane.yz.band":3,"plane.yz.swing":3.44,"plane.yz.speed":0.04,"plane.yz.gain":0,"plane.yw.band":4,"plane.yw.swing":3.31,"plane.yw.speed":0.04,"plane.yw.gain":0,"plane.zw.band":5,"plane.zw.swing":1.31,"plane.zw.speed":0.04,"plane.zw.gain":0,"axis.x.band":0,"axis.x.gain":0.2,"axis.x.bias":0.0,"axis.y.band":1,"axis.y.gain":0.2,"axis.y.bias":0.0,"axis.z.band":2,"axis.z.gain":0.2,"axis.z.bias":0.0,"axis.w.band":3,"axis.w.gain":0.2,"axis.w.bias":0.0,"cy.projection_depth":7.2,"cy.w_scale":2.18,"cy.dimension_pulse":0.16,"camera.distance":8.5},
  "Spectrum web": {"plane.xy.band":0,"plane.xy.swing":0.12,"plane.xy.speed":0.04,"plane.xy.gain":0,"plane.xz.band":1,"plane.xz.swing":0.74,"plane.xz.speed":0.04,"plane.xz.gain":0,"plane.xw.band":5,"plane.xw.swing":2.41,"plane.xw.speed":0.04,"plane.xw.gain":0,"plane.yz.band":2,"plane.yz.swing":0.78,"plane.yz.speed":0.04,"plane.yz.gain":0,"plane.yw.band":4,"plane.yw.swing":1.95,"plane.yw.speed":0.04,"plane.yw.gain":0,"plane.zw.band":3,"plane.zw.swing":1.32,"plane.zw.speed":0.04,"plane.zw.gain":0,"axis.x.band":5,"axis.x.gain":0.35,"axis.x.bias":0.0,"axis.y.band":4,"axis.y.gain":0.35,"axis.y.bias":0.0,"axis.z.band":3,"axis.z.gain":0.35,"axis.z.bias":0.0,"axis.w.band":2,"axis.w.gain":0.35,"axis.w.bias":0.0,"cy.projection_depth":7.46,"cy.w_scale":2.03,"cy.dimension_pulse":0.41,"camera.distance":8.5},
  "Tesseract roll": {"plane.xy.band":3,"plane.xy.swing":0.2,"plane.xy.speed":0.05,"plane.xy.gain":0.8,"plane.xz.band":4,"plane.xz.swing":0.2,"plane.xz.speed":0.04,"plane.xz.gain":0.6,"plane.xw.band":0,"plane.xw.swing":1.0,"plane.xw.speed":0.12,"plane.xw.gain":3.0,"plane.yz.band":5,"plane.yz.swing":0.2,"plane.yz.speed":0.03,"plane.yz.gain":0.6,"plane.yw.band":1,"plane.yw.swing":1.0,"plane.yw.speed":0.09,"plane.yw.gain":2.4,"plane.zw.band":2,"plane.zw.swing":1.0,"plane.zw.speed":0.15,"plane.zw.gain":3.2,"axis.x.band":0,"axis.x.gain":0.2,"axis.x.bias":0.0,"axis.y.band":1,"axis.y.gain":0.2,"axis.y.bias":0.0,"axis.z.band":2,"axis.z.gain":0.2,"axis.z.bias":0.0,"axis.w.band":3,"axis.w.gain":0.2,"axis.w.bias":0.0,"cy.projection_depth":6.0,"cy.w_scale":2.0,"cy.dimension_pulse":0.2,"camera.distance":8.0},
  "Kick fold": {"plane.xy.band":3,"plane.xy.swing":0.2,"plane.xy.speed":0.02,"plane.xy.gain":0,"plane.xz.band":4,"plane.xz.swing":0.2,"plane.xz.speed":0.02,"plane.xz.gain":0,"plane.xw.band":0,"plane.xw.swing":3.3,"plane.xw.speed":0.02,"plane.xw.gain":0,"plane.yz.band":5,"plane.yz.swing":0.2,"plane.yz.speed":0.02,"plane.yz.gain":0,"plane.yw.band":1,"plane.yw.swing":1.2,"plane.yw.speed":0.02,"plane.yw.gain":0,"plane.zw.band":2,"plane.zw.swing":0.8,"plane.zw.speed":0.02,"plane.zw.gain":0,"axis.x.band":0,"axis.x.gain":0.15,"axis.x.bias":0.0,"axis.y.band":1,"axis.y.gain":0.15,"axis.y.bias":0.0,"axis.z.band":2,"axis.z.gain":0.15,"axis.z.bias":0.0,"axis.w.band":3,"axis.w.gain":0.15,"axis.w.bias":0.0,"cy.projection_depth":4.5,"cy.w_scale":1.8,"cy.dimension_pulse":0.15,"camera.distance":7.5},
  "Shimmer web": {"plane.xy.band":0,"plane.xy.swing":0.5,"plane.xy.speed":0.05,"plane.xy.gain":0,"plane.xz.band":1,"plane.xz.swing":0.5,"plane.xz.speed":0.05,"plane.xz.gain":0,"plane.xw.band":5,"plane.xw.swing":3.1,"plane.xw.speed":0.05,"plane.xw.gain":0,"plane.yz.band":2,"plane.yz.swing":0.4,"plane.yz.speed":0.05,"plane.yz.gain":0,"plane.yw.band":4,"plane.yw.swing":2.8,"plane.yw.speed":0.05,"plane.yw.gain":0,"plane.zw.band":3,"plane.zw.swing":2.4,"plane.zw.speed":0.05,"plane.zw.gain":0,"axis.x.band":5,"axis.x.gain":0.25,"axis.x.bias":0.0,"axis.y.band":4,"axis.y.gain":0.25,"axis.y.bias":0.0,"axis.z.band":3,"axis.z.gain":0.25,"axis.z.bias":0.0,"axis.w.band":2,"axis.w.gain":0.25,"axis.w.bias":0.0,"cy.projection_depth":6.8,"cy.w_scale":2.1,"cy.dimension_pulse":0.25,"camera.distance":8.0},
  "Deep breath": {"plane.xy.band":3,"plane.xy.swing":0.3,"plane.xy.speed":0.03,"plane.xy.gain":0,"plane.xz.band":4,"plane.xz.swing":0.3,"plane.xz.speed":0.03,"plane.xz.gain":0,"plane.xw.band":0,"plane.xw.swing":1.8,"plane.xw.speed":0.08,"plane.xw.gain":0,"plane.yz.band":5,"plane.yz.swing":0.3,"plane.yz.speed":0.02,"plane.yz.gain":0,"plane.yw.band":1,"plane.yw.swing":1.6,"plane.yw.speed":0.06,"plane.yw.gain":0,"plane.zw.band":2,"plane.zw.swing":1.8,"plane.zw.speed":0.1,"plane.zw.gain":0,"axis.x.band":0,"axis.x.gain":0.7,"axis.x.bias":0.0,"axis.y.band":1,"axis.y.gain":0.7,"axis.y.bias":0.0,"axis.z.band":2,"axis.z.gain":0.7,"axis.z.bias":0.0,"axis.w.band":3,"axis.w.gain":0.7,"axis.w.bias":0.0,"cy.projection_depth":5.5,"cy.w_scale":2.0,"cy.dimension_pulse":0.5,"camera.distance":8.5},
  "Still": {"plane.xy.band":0,"plane.xy.swing":0,"plane.xy.speed":0,"plane.xy.gain":0,"plane.xz.band":1,"plane.xz.swing":0,"plane.xz.speed":0,"plane.xz.gain":0,"plane.xw.band":2,"plane.xw.swing":0,"plane.xw.speed":0,"plane.xw.gain":0,"plane.yz.band":3,"plane.yz.swing":0,"plane.yz.speed":0,"plane.yz.gain":0,"plane.yw.band":4,"plane.yw.swing":0,"plane.yw.speed":0,"plane.yw.gain":0,"plane.zw.band":5,"plane.zw.swing":0,"plane.zw.speed":0,"plane.zw.gain":0,"axis.x.band":0,"axis.x.gain":0.0,"axis.x.bias":0.0,"axis.y.band":1,"axis.y.gain":0.0,"axis.y.bias":0.0,"axis.z.band":2,"axis.z.gain":0.0,"axis.z.bias":0.0,"axis.w.band":3,"axis.w.gain":0.0,"axis.w.bias":0.0,"cy.projection_depth":4.0,"cy.w_scale":1.0,"cy.dimension_pulse":0.0,"camera.distance":6.5}
};

const PRESET_GROUPS = [
  { title: "Look", items: LOOK_PRESETS },
  { title: "Audio response", items: AUDIO_PRESETS },
  { title: "4D motion", items: MOTION_PRESETS }
];

function applyPreset(set) {
  for (const p of schema.params) {
    if (!(p.key in set)) continue;
    const v = Math.min(p.max, Math.max(p.min, set[p.key]));
    values[p.id] = v;
    queue(p.id, v);
  }
  renderControls();
}

async function profileCall(action, slot) {
  const res = await fetch(`/profile/${action}/${slot}`, { method: "POST" });
  const body = await res.json().catch(() => ({}));
  document.getElementById("net").textContent = res.ok
    ? `profile ${slot} ${action === "save" ? "saved" : "loaded"}`
    : `profile ${slot}: ${body.error || "error"}`;
  if (res.ok && action === "load") {
    values = (await fetch("/state.json", { cache: "no-store" }).then(r => r.json())).values;
    renderControls();
  }
  return res.ok;
}

async function renderProfiles() {
  const row = document.getElementById("profiles");
  row.textContent = "";
  const title = document.createElement("span");
  title.className = "preset-title";
  title.textContent = "Profiles";
  row.appendChild(title);
  let have = [];
  try { have = (await fetch("/profiles.json").then(r => r.json())).slots.map(Number); } catch (_e) {}
  for (let slot = 1; slot <= 4; slot++) {
    const load = document.createElement("button");
    load.textContent = `User ${slot}`;
    load.disabled = !have.includes(slot);
    load.title = "load";
    load.addEventListener("click", () => profileCall("load", slot));
    const save = document.createElement("button");
    save.textContent = "save";
    save.title = `overwrite user profile ${slot} with the current settings`;
    save.addEventListener("click", async () => { if (await profileCall("save", slot)) renderProfiles(); });
    row.append(load, save);
  }
}

function renderPresets() {
  const bar = document.getElementById("presets");
  bar.textContent = "";
  for (const group of PRESET_GROUPS) {
    const row = document.createElement("div");
    row.className = "preset-row";
    const title = document.createElement("span");
    title.className = "preset-title";
    title.textContent = group.title;
    row.appendChild(title);
    for (const [name, set] of Object.entries(group.items)) {
      const b = document.createElement("button");
      b.textContent = name;
      b.addEventListener("click", () => applyPreset(set));
      row.appendChild(b);
    }
    bar.appendChild(row);
  }
}

function renderControls() {
  const groups = new Map();
  for (const p of schema.params) {
    if (!groups.has(p.group)) groups.set(p.group, []);
    groups.get(p.group).push(p);
  }
  const root = document.getElementById("controls");
  root.textContent = "";
  for (const [group, params] of groups) {
    const section = document.createElement("section");
    const h = document.createElement("h2");
    h.textContent = group;
    section.appendChild(h);
    for (const p of params) {
      const row = document.createElement("div");
      row.className = `control ${p.control}`;
      const label = document.createElement("label");
      label.textContent = p.label + (p.unit ? ` (${p.unit})` : "");
      label.htmlFor = `p-${p.id}`;
      const range = document.createElement("input");
      range.type = "range";
      range.id = `p-${p.id}`;
      range.min = p.min;
      range.max = p.max;
      range.step = p.step;
      range.value = values[p.id] ?? p.default;
      const number = document.createElement("input");
      number.type = "number";
      number.min = p.min;
      number.max = p.max;
      number.step = p.step;
      number.value = Number(range.value).toFixed(p.step < 0.01 ? 3 : p.step < 0.1 ? 2 : 1);
      const apply = value => {
        const clamped = Math.min(p.max, Math.max(p.min, Number(value)));
        range.value = clamped;
        number.value = Number(clamped).toFixed(p.step < 0.01 ? 3 : p.step < 0.1 ? 2 : 1);
        values[p.id] = clamped;
        queue(p.id, clamped);
      };
      range.addEventListener("input", () => apply(range.value));
      if (p.control === "toggle") {
        const box = document.createElement("input");
        box.type = "checkbox";
        box.id = `p-${p.id}`;
        box.checked = (values[p.id] ?? p.default) >= 0.5;
        box.addEventListener("change", () => { values[p.id] = box.checked ? 1 : 0; queue(p.id, values[p.id]); });
        row.append(label, box);
        section.appendChild(row);
        continue;
      }
      number.addEventListener("focus", () => focusedId = p.id);
      number.addEventListener("blur", () => focusedId = null);
      number.addEventListener("change", () => apply(number.value));
      row.append(label, range, number);
      section.appendChild(row);
    }
    root.appendChild(section);
  }
}

async function pollState() {
  try {
    const state = await fetch("/state.json", { cache: "no-store" }).then(r => r.json());
    values = state.values;
    document.getElementById("seq").textContent = `seq ${state.seq}`;
    const bands = state.telemetry.audio_bands || [0, 0, 0, 0, 0, 0];
    const drives = state.telemetry.dimension_drive || [0, 0, 0, 0];
    const meterValues = [...bands, state.telemetry.master_level || 0];
    meterValues.forEach((v, i) => {
      const text = document.getElementById(`meter-value-${i}`);
      const bar = document.getElementById(`meter-bar-${i}`);
      if (text) text.textContent = Number(v).toFixed(2);
      if (bar) bar.style.width = `${Math.max(0, Math.min(100, v * 100))}%`;
    });
    for (let i = 0; i < 4; i++) {
      const text = document.getElementById(`drive-${i}`);
      if (text) text.textContent = Number(drives[i] || 0).toFixed(2);
    }
  } catch (_err) {
    document.getElementById("net").textContent = "state offline";
  }
}

async function init() {
  schema = await fetch("/schema.json").then(r => r.json());
  values = (await fetch("/state.json").then(r => r.json())).values;
  ["sub", "bass", "low-mid", "mid", "presence", "air", "master"].forEach(addMeter);
  renderPresets();
  renderProfiles();
  renderControls();
  setInterval(flush, 5);
  setInterval(pollState, 250);
  pollState();
  document.getElementById("net").textContent = `udp ${schema.protocol.udp_addr}`;
}

init().catch(err => {
  document.getElementById("net").textContent = err.message || "load failed";
});
</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_parse_reads_flat_pairs() {
        let text =
            "{\n \"name\": \"User 1\",\n \"params\": {\n  \"a.b\": 0.25,\n  \"c\": -3e-1\n }\n}";
        let pairs = parse_profile(text);
        assert_eq!(
            pairs,
            vec![("a.b".to_string(), 0.25), ("c".to_string(), -0.3)]
        );
    }

    #[test]
    fn param_ids_match_positions() {
        for (i, spec) in PARAMS.iter().enumerate() {
            assert_eq!(i, spec.id as usize);
        }
    }

    #[test]
    fn decodes_valid_packet() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.push(VERSION);
        bytes.push(KIND_PARAM_DELTAS);
        bytes.extend_from_slice(&(1u16).to_le_bytes());
        bytes.extend_from_slice(&(42u32).to_le_bytes());
        bytes.extend_from_slice(&(0u32).to_le_bytes());
        bytes.extend_from_slice(&(AUDIO_MASTER_GAIN as u16).to_le_bytes());
        bytes.extend_from_slice(&(1.5f32).to_le_bytes());
        let checksum = fnv1a32(&bytes[HEADER_LEN..]);
        bytes[12..16].copy_from_slice(&checksum.to_le_bytes());

        let decoded = decode_packet(&bytes).unwrap();
        assert_eq!(decoded.seq, 42);
        assert_eq!(decoded.deltas, vec![(AUDIO_MASTER_GAIN as u16, 1.5)]);
    }
}
