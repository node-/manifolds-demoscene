# demoscene

Dual-native (Direct3D 12 + Metal) renderer for low-dimensional projections of
higher-dimensional geometry — Calabi–Yau cross-sections and torus knots —
built as an audio-reactive visual layer for live music.

Stack rationale and the rejected alternatives live in
[`docs/graphics-runtime-trade-study.md`](docs/graphics-runtime-trade-study.md).
The realtime control protocol is documented in
[`docs/control-protocol.md`](docs/control-protocol.md).
Short version: raw dual-native was chosen for the 100%-of-hardware performance
ceiling; Rust (`windows` + `metal` crates) gives that with one workspace and no
abstraction layer between us and the native APIs.

## Layout

```
core/                 platform-agnostic math — no GPU deps, tested everywhere
  src/mesh.rs         Vertex/Mesh, normal recompute, raw-byte upload views
  src/projection.rs   SO(4) rotations + R^4 -> R^3 projection
  src/knot.rs         torus knots as parallel-transport-framed tubes
  src/calabiyau.rs    Hanson quintic z1^n + z2^n = 1 cross-section
  src/scene.rs        object selection + per-frame FrameUniforms
app/                  windowing + the two raw backends
  src/main.rs         winit loop, arg parsing, timing
  src/audio.rs        Windows WASAPI loopback bands; synthetic fallback elsewhere
  src/backend/d3d12.rs   raw Direct3D 12 (Windows)   <- runnable here
  src/backend/metal.rs   raw Metal (macOS)
  src/backend/null.rs    headless no-op (Linux/CI)
  shaders/mesh.hlsl   D3D12 shaders
  shaders/mesh.metal  Metal shaders (mirror of the HLSL)
```

The split is deliberate: everything mathematically interesting is in `core`,
compiled and tested on every platform, so the **Metal backend stays correct
without a Mac to run it on**. The backends only upload bytes and issue draws.

## Build & run

```sh
# Windows (D3D12):
cargo run -p demoscene-app --release            # trefoil
cargo run -p demoscene-app --release -- cy       # Calabi-Yau quintic
cargo run -p demoscene-app --release -- knot 3 4 # (3,4) torus knot

# macOS (Metal): same commands.
# Linux/WSL: builds and runs headless (logs frames); no GPU output.
DEMOSCENE_HEADLESS_FRAMES=10 cargo run -p demoscene-app --release

# From WSL, launch the Windows-native D3D12 build instead:
scripts/run-windows-native.sh --check    # verify Windows cargo.exe is visible
scripts/run-windows-native.sh            # trefoil
scripts/run-windows-native.sh cy         # Calabi-Yau quintic
scripts/run-windows-native.sh knot 3 4   # (3,4) torus knot
```

Left-drag orbits, wheel zooms. Double-click or F11 toggles borderless fullscreen;
Esc leaves fullscreen. Close the window to quit.

The app also starts a local control panel and binary control receiver:

```text
http://127.0.0.1:34200  # browser front-end
udp://127.0.0.1:34201   # binary 5 ms delta control packets
```

The Windows build captures the native OS output mix with WASAPI loopback, splits
it into four configurable FFT bands, and maps those bands onto the Calabi-Yau R4
dimensions. If output loopback is unavailable it falls back to the default input
device, and Linux/WSL keeps the synthetic band source for headless testing.

The WSL harness delegates to Windows PowerShell and `cargo.exe`, so it uses the
Windows Rust toolchain and D3D12 backend rather than building a Linux binary.
It writes Windows artifacts under `target/windows-native` to avoid sharing the
same Cargo target directory with WSL builds. It defaults to the MSVC Rust target
(`x86_64-pc-windows-msvc`), which avoids the MinGW `dlltool.exe` dependency used
by the GNU target. If you see a `dlltool.exe` error, you are building with the
GNU target instead of MSVC.

Windows-native prerequisites:

```powershell
rustup toolchain install stable-x86_64-pc-windows-msvc
rustup +stable-x86_64-pc-windows-msvc target add x86_64-pc-windows-msvc
```

Also install Visual Studio Build Tools with the C++ build tools and Windows SDK,
so the MSVC linker (`link.exe`) is available. The PowerShell script can also be
run directly from Windows:

```powershell
scripts\run-windows-native.ps1 cy
```

## Verification status

| Component | Status |
|---|---|
| `core` (math) | ✅ compiled + unit-tested locally and in CI (all platforms) |
| D3D12 backend | ⚙️ written against `windows` 0.58; compiled in CI on `windows-latest`. **Not yet run on real hardware** — first local run may need minor fixups. |
| Metal backend | ⚙️ written against `metal` 0.29; compiled in CI on `macos-latest`. Authored without a Mac — the `CAMetalLayer` attach + drawable path are the likeliest to need a tweak on first run. |

CI builds the D3D12 and Metal backends on their native runners on every push,
which is what keeps the platform you *can't* run locally from silently breaking.

- `.github/workflows/ci.yml` runs the Linux `core` job (fmt, tests, clippy) and
  builds the app on Windows and macOS hosted runners.

## Status / next steps

This is early-stage: native windowed rendering with camera-space clay lighting,
six-band audio analysis routed one-to-one onto the six R^4 rotation planes,
a live control panel and presets, and a desktop recorder. Roadmap, roughly in order:

1. **Run the D3D12 path on hardware**, fix any first-run issues, capture a frame
   time under a stress case (high tessellation) to confirm the ceiling argument
   in the trade study.
2. **Audio polish** — add explicit device selection, per-band meters in the
   renderer, and optional ASIO/JACK routes.
3. **Animate the projection itself** — move the `R^4` rotation / Hanson
   `proj_angle` into the vertex (or a compute) shader so the higher-dimensional
   structure animates without rebuilding the mesh on the CPU.
4. **Demoscene polish** — bloom/glow post pass, order-independent transparency
   for the self-intersecting Calabi–Yau surface, MSAA.
5. **Frame pipelining** — the backends currently flush per frame; add 2–3 frames
   in flight once correctness is nailed down.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
