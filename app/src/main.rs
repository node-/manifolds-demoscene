//! demoscene — dual-native (D3D12 / Metal) renderer for higher-dimensional
//! manifold projections. See `docs/graphics-runtime-trade-study.md`.

mod audio;
mod backend;
mod control;

use backend::{ActiveRenderer, Renderer};
use control::SharedControls;
use dscore::{Object, Scene};
#[cfg(not(any(windows, target_os = "macos")))]
use std::time::Duration;
use std::time::Instant;
#[cfg(any(windows, target_os = "macos"))]
use winit::{
    dpi::LogicalSize,
    event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Fullscreen, Window, WindowBuilder},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Pick the object from argv: `demoscene cy` | `trefoil` | `knot P Q`.
    let object = parse_object(std::env::args().skip(1).collect());
    log::info!("rendering {object:?}");

    let controls = SharedControls::new();
    control::start_servers(controls.clone());

    run(object, controls)
}

#[cfg(any(windows, target_os = "macos"))]
fn run(object: Object, controls: SharedControls) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    let window = WindowBuilder::new()
        .with_title("demoscene")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .build(&event_loop)?;

    let mut scene = Scene::new(object);
    let mesh = scene.build_mesh();
    log::info!(
        "mesh: {} verts, {} indices",
        mesh.vertices.len(),
        mesh.index_count()
    );

    let mut renderer = ActiveRenderer::new(&window, &mesh);
    let mut audio = audio::AudioSource::new();
    let start = Instant::now();
    let mut last = start;
    let mut dragging = false;
    let mut last_cursor: Option<(f64, f64)> = None;
    let mut last_click: Option<(Instant, (f64, f64))> = None;

    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run(move |event, elwt| match event {
        Event::WindowEvent { event, .. } => match event {
            WindowEvent::CloseRequested => elwt.exit(),
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.logical_key {
                    Key::Named(NamedKey::F11) => toggle_fullscreen(&window),
                    // Esc only leaves fullscreen; it never quits a live set.
                    Key::Named(NamedKey::Escape) if window.fullscreen().is_some() => {
                        window.set_fullscreen(None)
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                dragging = state == ElementState::Pressed;
                if dragging {
                    // winit has no double-click event: two presses within
                    // 350 ms and 6 px count as one.
                    let pos = last_cursor.unwrap_or((0.0, 0.0));
                    let now = Instant::now();
                    let is_double = last_click.is_some_and(|(at, p)| {
                        now.duration_since(at).as_millis() < 350
                            && (p.0 - pos.0).abs() < 6.0
                            && (p.1 - pos.1).abs() < 6.0
                    });
                    if is_double {
                        toggle_fullscreen(&window);
                        last_click = None;
                    } else {
                        last_click = Some((now, pos));
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if dragging {
                    if let Some((lx, ly)) = last_cursor {
                        scene.orbit(
                            -(position.x - lx) as f32 * 0.005,
                            (position.y - ly) as f32 * 0.005,
                        );
                    }
                }
                last_cursor = Some((position.x, position.y));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                };
                scene.zoom_by(0.9f32.powf(lines));
            }
            WindowEvent::Resized(size) => {
                renderer.resize(size.width.max(1), size.height.max(1));
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - last).as_secs_f32();
                last = now;
                let t = (now - start).as_secs_f32();

                let size = window.inner_size();
                let aspect = size.width as f32 / size.height.max(1) as f32;
                let snapshot = controls.snapshot();
                let audio_control = snapshot.audio_control();
                let scene_control = snapshot.scene_control();
                let bands = audio.bands(&audio_control, t);

                let (vsync, msaa) = snapshot.render_options();
                renderer.set_options(vsync, msaa);
                let uniforms = scene.update(dt, bands, t, aspect, &scene_control);
                controls.set_telemetry(&uniforms);
                renderer.render(&uniforms);
            }
            _ => {}
        },
        Event::AboutToWait => window.request_redraw(),
        _ => {}
    })?;

    Ok(())
}

#[cfg(any(windows, target_os = "macos"))]
fn toggle_fullscreen(window: &Window) {
    let next = match window.fullscreen() {
        Some(_) => None,
        None => Some(Fullscreen::Borderless(None)),
    };
    window.set_fullscreen(next);
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run(object: Object, controls: SharedControls) -> Result<(), Box<dyn std::error::Error>> {
    let mut scene = Scene::new(object);
    let mesh = scene.build_mesh();
    log::info!(
        "mesh: {} verts, {} indices",
        mesh.vertices.len(),
        mesh.index_count()
    );

    let mut renderer = ActiveRenderer::new_headless(&mesh);
    let mut audio = audio::AudioSource::new();
    let start = Instant::now();
    let mut last = start;
    let mut frame = 0u64;
    let frame_interval = Duration::from_millis(16);
    let frame_limit = std::env::var("DEMOSCENE_HEADLESS_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());

    loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32();
        last = now;
        let t = (now - start).as_secs_f32();

        let snapshot = controls.snapshot();
        let audio_control = snapshot.audio_control();
        let scene_control = snapshot.scene_control();
        let bands = audio.bands(&audio_control, t);
        let uniforms = scene.update(dt, bands, t, 1280.0 / 800.0, &scene_control);
        controls.set_telemetry(&uniforms);
        renderer.render(&uniforms);

        frame += 1;
        if let Some(limit) = frame_limit {
            if frame >= limit {
                log::info!("headless run complete after {frame} frames");
                break;
            }
        }

        std::thread::sleep(frame_interval);
    }

    Ok(())
}

fn parse_object(args: Vec<String>) -> Object {
    match args.first().map(String::as_str) {
        Some("cy") | Some("calabiyau") => Object::CalabiYau,
        Some("knot") => {
            let p = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(3);
            let q = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4);
            Object::TorusKnot { p, q }
        }
        _ => Object::Trefoil,
    }
}
