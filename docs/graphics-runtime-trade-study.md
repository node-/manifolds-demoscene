# Graphics Runtime Trade Study — `demoscene` visuals

> Status: draft / decision pending
> Date: 2026-06-19
> Author: node- (with Claude)
>
> **Scope note:** this is a standalone project that does not depend on any
> particular audio engine. Language/ecosystem fit is therefore *not* a forcing
> function; the decision is driven by **performance ceiling, portability, and
> iteration speed**.

## 1. Purpose

Pick a portable GPU graphics stack for live audio-reactive visuals. The visuals are demoscene-style, audio-reactive renderings of
**low-dimensional (3D/2D) projections of higher-dimensional geometric objects**:

- Calabi–Yau manifold cross-sections (the classic Hanson quintic visualization)
- Knots: trefoil, torus knots `(p,q)`, and higher-order / composite knots
- Generally: objects living in `R^n` (n = 4..6) rotated and projected down to
  `R^3 → R^2`, with parameters driven by audio.

## 2. Hard requirements & priorities

| Priority | Requirement | Notes |
|---|---|---|
| Fixed constraint | **Portable** — one codebase | Portability is held constant; we trade other axes against it. |
| Must | **Native macOS + Windows builds** | Metal on macOS, D3D12/D3D11 on Windows preferred over translation layers. |
| Must | **Fast, close to the GPU** | Explicit pipelines/command buffers; minimal CPU overhead; demoscene framerates. |
| Strong want | **Compute shaders** | Generate parametric meshes / do `R^n` projection on-GPU; drive from audio. |
| Want | **Small footprint, fast iteration** | Demoscene workflow favors quick shader edit/reload, small binaries. |
| Nice | **Web (WASM/WebGPU) target** | Shareable browser demos for free if the stack supports it. |
| Not needed (yet) | Hardware ray tracing, mesh shaders | CY/knot workloads are parametric meshes + raster; RT is optional polish. |

## 3. The actual rendering workload

Understanding the workload narrows the stack choice — none of this needs a
AAA-engine feature set.

**Calabi–Yau cross-section (Hanson visualization).** The standard image is the
real 2D cross-section of the Fermat quintic projective variety
`z1^n + z2^n = 1` (n = 5). It decomposes into `n × n` parametric patches indexed
by pairs of complex `n`-th roots of unity; each patch is a smooth grid mapped
into `R^4`, then projected to `R^3` by a (animated) rotation in the imaginary
plane. **=> Parametric mesh generation**, `~n²` patches × grid resolution,
trivially a compute shader or CPU-side vertex buffer build. Self-intersecting,
so wants either transparency / order-independent blending or good depth+normals.

**Knots.** Torus knot `(p,q)` is a parametric space curve; sweep a circular
tube along it using a parallel-transport (Bishop) frame to avoid Frenet
twist/flip. Higher-order knots = larger `(p,q)` or other knot families / knot
tables. **=> Curve sample + tube extrusion mesh**, cheap, also a natural compute
job.

**Higher-dim projection.** Keep vertices in `R^n`; apply an animated `n×n`
rotation (audio-driven), then `R^n → R^3 → R^2`. A handful of matrix multiplies
per vertex — fits in a vertex or compute shader.

**Look.** Demoscene polish: matcap or lightweight PBR, bloom/glow, depth fog,
maybe OIT for the CY self-intersections, post FX. All standard fragment / full-
screen-pass work.

**Implication:** the workload is *parametric-mesh + raster + post*, with
optional compute for generation. Every candidate below can do it. The decision
is really about **language/ecosystem fit, iteration speed, and how close to the
metal you want to sit**, not raw capability.

## 4. Candidate stacks

### A. `wgpu` (Rust) — WebGPU implementation
- **Backends:** Vulkan, **Metal (native macOS)**, **D3D12 (native Windows)**, GL fallback, WebGPU (browser).
- **API level:** modern explicit — pipelines, bind groups, command encoders. Close to the metal with far less boilerplate than raw Vulkan.
- **Shaders:** WGSL (native), can also ingest SPIR-V/GLSL via `naga`.
- **Compute:** yes, first-class.
- **Pros:** Single codebase → all targets incl. **web**. Memory-safe. **Same language as common Rust audio engines** — in-process audio-reactive coupling (share buffers/state directly, no IPC). Huge ecosystem (`winit`, `egui`, `bevy`'s renderer, `cpal`/`fundsp` for audio). Excellent for procedural/compute-heavy work.
- **Cons:** WebGPU is a deliberately conservative abstraction — no bleeding-edge RT/mesh-shader extensions (don't need them). Rust compile times. Larger binaries than `sokol`. Some validation overhead (disable in release).

### B. `sokol` (C) — `sokol_gfx.h` + `sokol_app.h`
- **Backends:** **Metal (native macOS)**, **D3D11 (native Windows)**, GL/GLES, WebGPU.
- **API level:** thin, modern-ish wrapper; very lean.
- **Shaders:** author in GLSL, cross-compile to all backends with `sokol-shdc`.
- **Compute:** supported (added relatively recently, 2024-era — less battle-tested than its render path).
- **Pros:** Tiny single-header libs, **smallest footprint**, fastest iteration, the demoscene-native choice. Trivial to embed; callable from Rust via FFI if you want C rendering under a Rust app.
- **Cons:** Windows backend is **D3D11, not D3D12** (still native, just less "modern explicit"). Thinner feature set — you write more scene/math yourself (fine, even desirable, for demoscene). C ergonomics. Audio integration with Rust engines means an FFI boundary.

### C. `bgfx` (C++)
- **Backends:** D3D11/**D3D12**, **Metal**, Vulkan, GL/GLES, (WebGPU-ish via emscripten).
- **API level:** "stateless" view-based abstraction — one layer further from the GPU than wgpu/sokol.
- **Shaders:** bgfx's own flavor, cross-compiled with `shaderc`.
- **Pros:** Mature, battle-tested in shipping games/tools, widest backend matrix, good docs.
- **Cons:** C++ build integration; heavier; idiosyncratic shader toolchain; extra abstraction layer. No clean web story for our purposes. Language mismatch with the Rust audio side.

### D. Raw native — Metal (+ D3D12 or Vulkan) directly
- **API level:** maximal — full feature access (RT, mesh shaders), tightest control.
- **Pros:** Absolute peak performance and capability.
- **Cons:** **Portability is the casualty** — you maintain ≥2 backends, or accept MoltenVK (Vulkan→Metal *translation*) on macOS. Enormous boilerplate, slowest iteration. Overkill for parametric-mesh + raster. Only justified if we later want hardware ray tracing of implicit CY surfaces.

### E. (Orthogonal) Minimalist fullscreen-raymarch path
Not a separate stack but a *technique* worth flagging: knots and some implicit
surfaces can be **raymarched as SDFs in a single fragment shader** over one
fullscreen triangle — the true 4k/64k-intro approach, reducing the runtime to
almost nothing. Caveat: the **CY quintic cross-section is not a clean SDF**
(it's naturally parametric), so a pure-raymarch path handles knots/tori well but
forces awkward tricks for CY. Likely outcome: **hybrid** — parametric meshes for
CY, raymarch or thin tubes for knots, depending on look. This technique runs on
top of *any* stack above; it argues for one with painless shader hot-reload
(wgpu or sokol).

## 5. Comparison

| Axis | wgpu (Rust) | sokol (C) | bgfx (C++) | Raw native |
|---|---|---|---|---|
| macOS native | Metal ✅ | Metal ✅ | Metal ✅ | Metal ✅ |
| Windows native | D3D12 ✅ | D3D11 ✅ | D3D12 ✅ | D3D12 ✅ |
| Single codebase | ✅ | ✅ | ✅ | ❌ (2+ backends) |
| Close to GPU | High | Medium-High | Medium | Highest |
| Compute shaders | ✅ mature | ✅ newer | ✅ mature | ✅ |
| Web target | ✅ WebGPU | ✅ WebGPU | ~ (emscripten) | ❌ |
| Iteration speed | Medium (Rust builds) | **Fast** | Medium | Slow |
| Binary size | Medium | **Tiny** | Medium-Large | Varies |
| Maturity | High | Medium-High | **Highest** | n/a |

## 5a. Performance ceiling under the portability constraint

Estimates of *headroom when pushed hard* (heavy OIT on self-intersecting CY,
high-step raymarched knots, massive instancing/tessellation), **not** the
typical case — for the baseline workload every option saturates a display.
Figures are engineering judgment, not benchmarks.

- **A. wgpu — ~85–95% of native.** Thin layer over Metal/D3D12/Vulkan; modern
  explicit API (indirect draw, compute, multithreaded encoding) on both OSes.
  - ⚠️ **Single-queue model** — no async compute; can't overlap heavy mesh
    generation with graphics.
  - ⚠️ **LCD spec** — no bindless / mesh shaders / core subgroups; tight binding
    limits. Native-only extensions exist but **using them breaks the web target**.
  - ⚠️ Validation overhead (disable in release); occasional WGSL/`naga` codegen quirks.
- **B. sokol — Metal ~90% native; Windows capped at D3D11-era throughput.**
  Near-native within its backend; the backend is the cap.
  - ⚠️ **Windows = D3D11, not D3D12 (biggest cap)** — high per-draw CPU cost, no
    GPU-driven rendering, poor scaling past ~thousands of draws.
  - ⚠️ **Young compute path** — less battle-tested than the render path.
  - ⚠️ **DIY perf** — hand-rolled batching/instancing; easy to leave perf on the table.
- **C. bgfx — ~80–90% native; modern backends, abstraction tax.** D3D12/Vulkan/
  Metal give a modern ceiling (beats sokol on Windows).
  - ⚠️ **Stateless-abstraction overhead** — per-submit key gen + state sort +
    command translation caps the high-draw-count top end.
  - ⚠️ **Opaque backend quality**; bindless/mesh shaders not exposed.
- **D. Raw dual-native (Metal + D3D12) — 100%, the true ceiling.** Async compute,
  argument buffers / descriptor heaps, mesh shaders, indirect command buffers,
  explicit barriers, multithreaded encoding.
  - ⚠️ **2× backend maintenance (the whole cost)** — two renderers, divergence
    bugs, slowest iteration. "Portable" = ships native on both, *not* one codebase.
  - ⚠️ **MoltenVK shortcut undoes it** — one Vulkan codebase reintroduces a
    Vulkan→Metal **translation layer** (~10–30% loss + lost Metal features).

**Ranking (most perf while portable):** D (highest ceiling, 2× effort) → A
(best perf-per-effort, single codebase) → C (abstraction tax) → B (lowest
ceiling via D3D11 on Windows, but leanest/fastest to iterate).

## 6. Recommendation

**Primary: `wgpu` (Rust).** Best performance-per-engineering-effort under the
portability constraint: ~85–95% of native, a modern explicit API on *both* OSes
(native Metal + native D3D12), first-class compute, all from one codebase — plus
a free WebGPU browser build. The ~5–15% gap is async-compute / bindless / mesh-
shader features this workload almost certainly won't need. Costs (Rust compile
times, binary size) are irrelevant here.

**If absolute performance is the goal: raw dual-native (Metal + D3D12).** The
only way to reach 100% of the hardware ceiling while shipping native on both
OSes — at the price of two hand-written backends and the slowest iteration loop.
Justified only if a specific feature (async compute overlap, mesh shaders,
bindless) becomes a measured bottleneck in the PoC.

**Lean alternative: `sokol` (C).** Choose if minimal footprint and the fastest
shader-edit/reload loop matter most. Most demoscene-authentic. Accept the
**D3D11 Windows ceiling** and a younger compute path.

**Not recommended now:** `bgfx` — mature and modern-backend, but its stateless-
abstraction tax caps the top end while offering nothing wgpu doesn't for this
workload.

## 7. Suggested next step (proof-of-concept)

Validate the recommendation cheaply before committing:

1. `wgpu` + `winit` window, native macOS + Windows.
2. Render an animated **`(2,3)` trefoil tube** (parallel-transport frame) and a
   **single-patch CY quintic** mesh, both generated in a compute shader, with
   an `R^4 → R^3 → R^2` projection matrix updated per frame.
3. Wire one audio scalar (e.g. an FFT band magnitude from `cpal`) to the
   projection rotation speed — proves the audio→render reactive path.
4. Confirm build + run on both OSes; **measure frame time under a stress case**
   (high tessellation + OIT) to see whether the wgpu ceiling is ever a concern.

Success here de-risks the whole stack choice. If Rust build friction proves
painful in practice, fall back to `sokol` with the same PoC.

## 8. Open questions

- **Audio source of truth:** does the audio engine expose a Rust library crate the
  visuals can link directly, or will visuals run as a separate process consuming
  a stream (OSC / shared mem / socket)? Answers whether wgpu's in-process edge
  actually applies.
- **Output context:** live VJ / realtime, or offline high-quality renders to
  video? Affects whether we optimize for latency or for fidelity (MSAA/OIT/RT).
- **Knot ambition:** torus knots `(p,q)` only, or arbitrary knots from a knot
  table (needs a knot data source / Conway notation parser)?
- **CY fidelity:** schematic Hanson look, or physically-motivated metric
  visualization (much harder)?
