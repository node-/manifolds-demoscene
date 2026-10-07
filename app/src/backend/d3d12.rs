//! Raw Direct3D 12 backend (Windows). No abstraction layer — direct `windows`
//! crate calls. Single-frame synchronization (flush per frame) keeps the
//! scaffold simple; frame pipelining is a later optimization.
//!
//! Targets the `windows` crate 0.58 API surface. Verified by CI on a Windows
//! runner; run locally with `cargo run -p demoscene-app`.

use super::Renderer;
use dscore::{scene::FrameUniforms, Mesh, Vertex};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::mem::{size_of, ManuallyDrop};
use windows::core::{s, Interface};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_DEBUG};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};

const FRAME_COUNT: usize = 2;
const RT_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R8G8B8A8_UNORM;
const MSAA_SAMPLES: u32 = 4;

pub struct D3D12Renderer {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    swapchain: IDXGISwapChain3,
    rtv_heap: ID3D12DescriptorHeap,
    rtv_size: usize,
    dsv_heap: ID3D12DescriptorHeap,
    render_targets: Vec<ID3D12Resource>,
    depth: ID3D12Resource,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    pso_ms: ID3D12PipelineState,
    vsync: bool,
    msaa: bool,
    ms_target: Option<ID3D12Resource>,
    vbv: D3D12_VERTEX_BUFFER_VIEW,
    ibv: D3D12_INDEX_BUFFER_VIEW,
    _vb: ID3D12Resource,
    _ib: ID3D12Resource,
    index_count: u32,
    _cbuffer: ID3D12Resource,
    cb_ptr: *mut u8,
    cb_gpu: u64,
    fence: ID3D12Fence,
    fence_value: u64,
    fence_event: HANDLE,
    width: u32,
    height: u32,
}

impl Renderer for D3D12Renderer {
    fn new(window: &winit::window::Window, mesh: &Mesh) -> Self {
        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));
        let hwnd = match window.window_handle().unwrap().as_raw() {
            RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
            _ => panic!("expected a Win32 window handle"),
        };

        unsafe {
            // Debug layer (no-op in release / if the SDK layer is absent).
            if cfg!(debug_assertions) {
                let mut debug: Option<ID3D12Debug> = None;
                if D3D12GetDebugInterface(&mut debug).is_ok() {
                    if let Some(d) = debug {
                        d.EnableDebugLayer();
                    }
                }
            }

            let factory: IDXGIFactory4 =
                CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).expect("create dxgi factory");

            let mut device: Option<ID3D12Device> = None;
            D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut device).expect("create device");
            let device = device.unwrap();

            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })
                .expect("command queue");

            let sc_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: RT_FORMAT,
                BufferCount: FRAME_COUNT as u32,
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                ..Default::default()
            };
            let swapchain: IDXGISwapChain3 = factory
                .CreateSwapChainForHwnd(&queue, hwnd, &sc_desc, None, None)
                .expect("swapchain")
                .cast()
                .unwrap();

            let rtv_heap: ID3D12DescriptorHeap = device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                    NumDescriptors: FRAME_COUNT as u32 + 1, // + MSAA colour target
                    ..Default::default()
                })
                .expect("rtv heap");
            let rtv_size =
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize;

            let dsv_heap: ID3D12DescriptorHeap = device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                    NumDescriptors: 1,
                    ..Default::default()
                })
                .expect("dsv heap");

            let render_targets = create_render_targets(&device, &swapchain, &rtv_heap, rtv_size);
            let depth = create_depth(&device, &dsv_heap, width, height, 1);

            let allocator: ID3D12CommandAllocator = device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .expect("allocator");
            let list: ID3D12GraphicsCommandList = device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                .expect("command list");
            list.Close().unwrap();

            let root = create_root_signature(&device);
            let pso = create_pipeline(&device, &root, 1);
            let pso_ms = create_pipeline(&device, &root, MSAA_SAMPLES);

            // Static geometry in upload heaps (fine at scaffold scale).
            let vb = upload_buffer(&device, mesh.vertex_bytes());
            let ib = upload_buffer(&device, mesh.index_bytes());
            let vbv = D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: vb.GetGPUVirtualAddress(),
                SizeInBytes: mesh.vertex_bytes().len() as u32,
                StrideInBytes: Vertex::STRIDE as u32,
            };
            let ibv = D3D12_INDEX_BUFFER_VIEW {
                BufferLocation: ib.GetGPUVirtualAddress(),
                SizeInBytes: mesh.index_bytes().len() as u32,
                Format: DXGI_FORMAT_R32_UINT,
            };

            // Persistently-mapped constant buffer (256-byte aligned).
            let cb_size = ((size_of::<FrameUniforms>() + 255) & !255) as usize;
            let cbuffer = upload_buffer_sized(&device, cb_size);
            let mut cb_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            cbuffer.Map(0, None, Some(&mut cb_ptr)).unwrap();
            let cb_gpu = cbuffer.GetGPUVirtualAddress();

            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).expect("fence");
            let fence_event = CreateEventW(None, false, false, None).expect("event");

            let mut me = Self {
                device,
                queue,
                swapchain,
                rtv_heap,
                rtv_size,
                dsv_heap,
                render_targets,
                depth,
                allocator,
                list,
                root,
                pso,
                pso_ms,
                vsync: false,
                msaa: false,
                ms_target: None,
                vbv,
                ibv,
                _vb: vb,
                _ib: ib,
                index_count: mesh.index_count(),
                _cbuffer: cbuffer,
                cb_ptr: cb_ptr as *mut u8,
                cb_gpu,
                fence,
                fence_value: 0,
                fence_event,
                width,
                height,
            };
            me.wait_for_gpu();
            me
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if (width, height) == (self.width, self.height) {
            return;
        }
        unsafe {
            self.wait_for_gpu();
            // A recorded command list keeps references to the back buffers it
            // touched; DXGI refuses to resize until they are all released.
            self.allocator.Reset().expect("reset allocator");
            self.list.Reset(&self.allocator, None).expect("reset list");
            self.list.Close().expect("close list");
            self.render_targets.clear();
            self.swapchain
                .ResizeBuffers(
                    FRAME_COUNT as u32,
                    width,
                    height,
                    RT_FORMAT,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .expect("resize buffers");
            self.render_targets =
                create_render_targets(&self.device, &self.swapchain, &self.rtv_heap, self.rtv_size);
            self.width = width;
            self.height = height;
            self.rebuild_msaa_targets();
        }
    }

    fn set_options(&mut self, vsync: bool, msaa: bool) {
        self.vsync = vsync;
        if msaa != self.msaa {
            self.wait_for_gpu();
            self.msaa = msaa;
            unsafe { self.rebuild_msaa_targets() };
        }
    }

    fn render(&mut self, uniforms: &FrameUniforms) {
        unsafe {
            // Update the constant buffer in place.
            std::ptr::copy_nonoverlapping(
                uniforms.bytes().as_ptr(),
                self.cb_ptr,
                size_of::<FrameUniforms>(),
            );

            let frame = self.swapchain.GetCurrentBackBufferIndex() as usize;
            self.allocator.Reset().unwrap();
            self.list.Reset(&self.allocator, &self.pso).unwrap();

            let rt = &self.render_targets[frame];
            let ms = if self.msaa {
                self.ms_target.as_ref()
            } else {
                None
            };
            self.list.ResourceBarrier(&[transition(
                rt,
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            )]);

            let rtv_slot = if ms.is_some() { FRAME_COUNT } else { frame };
            let rtv = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: self.rtv_heap.GetCPUDescriptorHandleForHeapStart().ptr
                    + rtv_slot * self.rtv_size,
            };
            let dsv = self.dsv_heap.GetCPUDescriptorHandleForHeapStart();
            self.list
                .OMSetRenderTargets(1, Some(&rtv), false, Some(&dsv));

            self.list.ClearRenderTargetView(rtv, &uniforms.bg, None);
            self.list
                .ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 1.0, 0, &[]);

            let viewport = D3D12_VIEWPORT {
                Width: self.width as f32,
                Height: self.height as f32,
                MaxDepth: 1.0,
                ..Default::default()
            };
            let scissor = windows::Win32::Foundation::RECT {
                right: self.width as i32,
                bottom: self.height as i32,
                ..Default::default()
            };
            self.list.RSSetViewports(&[viewport]);
            self.list.RSSetScissorRects(&[scissor]);

            self.list.SetGraphicsRootSignature(&self.root);
            self.list.SetGraphicsRootConstantBufferView(0, self.cb_gpu);
            self.list
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.list.IASetVertexBuffers(0, Some(&[self.vbv]));
            self.list.IASetIndexBuffer(Some(&self.ibv));
            if ms.is_some() {
                self.list.SetPipelineState(&self.pso_ms);
            }
            self.list.DrawIndexedInstanced(self.index_count, 1, 0, 0, 0);

            if let Some(ms) = ms {
                self.list.ResourceBarrier(&[
                    transition(
                        ms,
                        D3D12_RESOURCE_STATE_RENDER_TARGET,
                        D3D12_RESOURCE_STATE_RESOLVE_SOURCE,
                    ),
                    transition(
                        rt,
                        D3D12_RESOURCE_STATE_RENDER_TARGET,
                        D3D12_RESOURCE_STATE_RESOLVE_DEST,
                    ),
                ]);
                self.list.ResolveSubresource(rt, 0, ms, 0, RT_FORMAT);
                self.list.ResourceBarrier(&[
                    transition(
                        ms,
                        D3D12_RESOURCE_STATE_RESOLVE_SOURCE,
                        D3D12_RESOURCE_STATE_RENDER_TARGET,
                    ),
                    transition(
                        rt,
                        D3D12_RESOURCE_STATE_RESOLVE_DEST,
                        D3D12_RESOURCE_STATE_PRESENT,
                    ),
                ]);
            } else {
                self.list.ResourceBarrier(&[transition(
                    rt,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                    D3D12_RESOURCE_STATE_PRESENT,
                )]);
            }
            self.list.Close().unwrap();

            // windows-rs models the COM-pointer array as `&[Option<_>]`.
            let cmd: ID3D12CommandList = self.list.cast().unwrap();
            self.queue.ExecuteCommandLists(&[Some(cmd)]);
            self.swapchain
                .Present(self.vsync as u32, DXGI_PRESENT(0))
                .ok()
                .unwrap();
            self.wait_for_gpu();
        }
    }
}

impl D3D12Renderer {
    /// (Re)create the depth buffer and, when MSAA is on, the multisampled
    /// colour target at the current size. GPU must be idle.
    unsafe fn rebuild_msaa_targets(&mut self) {
        let samples = if self.msaa { MSAA_SAMPLES } else { 1 };
        self.depth = create_depth(
            &self.device,
            &self.dsv_heap,
            self.width,
            self.height,
            samples,
        );
        self.ms_target = if self.msaa {
            let ms = create_msaa_target(&self.device, self.width, self.height);
            let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: self.rtv_heap.GetCPUDescriptorHandleForHeapStart().ptr
                    + FRAME_COUNT * self.rtv_size,
            };
            self.device.CreateRenderTargetView(&ms, None, handle);
            Some(ms)
        } else {
            None
        };
    }

    /// Flush: signal the fence and block until the GPU reaches it.
    fn wait_for_gpu(&mut self) {
        unsafe {
            self.fence_value += 1;
            let v = self.fence_value;
            self.queue.Signal(&self.fence, v).unwrap();
            if self.fence.GetCompletedValue() < v {
                self.fence
                    .SetEventOnCompletion(v, self.fence_event)
                    .unwrap();
                WaitForSingleObject(self.fence_event, INFINITE);
            }
        }
    }
}

impl Drop for D3D12Renderer {
    fn drop(&mut self) {
        self.wait_for_gpu();
        unsafe {
            let _ = CloseHandle(self.fence_event);
        }
    }
}

// --- helpers ---------------------------------------------------------------

unsafe fn create_render_targets(
    device: &ID3D12Device,
    swapchain: &IDXGISwapChain3,
    heap: &ID3D12DescriptorHeap,
    rtv_size: usize,
) -> Vec<ID3D12Resource> {
    let start = heap.GetCPUDescriptorHandleForHeapStart();
    (0..FRAME_COUNT)
        .map(|i| {
            let rt: ID3D12Resource = swapchain.GetBuffer(i as u32).unwrap();
            let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: start.ptr + i * rtv_size,
            };
            device.CreateRenderTargetView(&rt, None, handle);
            rt
        })
        .collect()
}

unsafe fn create_depth(
    device: &ID3D12Device,
    dsv_heap: &ID3D12DescriptorHeap,
    width: u32,
    height: u32,
    samples: u32,
) -> ID3D12Resource {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: width as u64,
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_D32_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: samples,
            Quality: 0,
        },
        Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
        ..Default::default()
    };
    let clear = D3D12_CLEAR_VALUE {
        Format: DXGI_FORMAT_D32_FLOAT,
        Anonymous: D3D12_CLEAR_VALUE_0 {
            DepthStencil: D3D12_DEPTH_STENCIL_VALUE {
                Depth: 1.0,
                Stencil: 0,
            },
        },
    };
    let heap_props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let mut res: Option<ID3D12Resource> = None;
    device
        .CreateCommittedResource(
            &heap_props,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_DEPTH_WRITE,
            Some(&clear),
            &mut res,
        )
        .expect("depth buffer");
    let res = res.unwrap();
    device.CreateDepthStencilView(&res, None, dsv_heap.GetCPUDescriptorHandleForHeapStart());
    res
}

unsafe fn create_msaa_target(device: &ID3D12Device, width: u32, height: u32) -> ID3D12Resource {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: width as u64,
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: RT_FORMAT,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: MSAA_SAMPLES,
            Quality: 0,
        },
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        ..Default::default()
    };
    let clear = D3D12_CLEAR_VALUE {
        Format: RT_FORMAT,
        Anonymous: D3D12_CLEAR_VALUE_0 {
            Color: [0.94, 0.935, 0.92, 1.0],
        },
    };
    let heap_props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let mut res: Option<ID3D12Resource> = None;
    device
        .CreateCommittedResource(
            &heap_props,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            Some(&clear),
            &mut res,
        )
        .expect("msaa colour target");
    res.unwrap()
}

unsafe fn upload_buffer(device: &ID3D12Device, data: &[u8]) -> ID3D12Resource {
    let res = upload_buffer_sized(device, data.len());
    let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    res.Map(0, None, Some(&mut ptr)).unwrap();
    std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
    res.Unmap(0, None);
    res
}

unsafe fn upload_buffer_sized(device: &ID3D12Device, size: usize) -> ID3D12Resource {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size as u64,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let heap_props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        ..Default::default()
    };
    let mut res: Option<ID3D12Resource> = None;
    device
        .CreateCommittedResource(
            &heap_props,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut res,
        )
        .expect("upload buffer");
    res.unwrap()
}

unsafe fn create_root_signature(device: &ID3D12Device) -> ID3D12RootSignature {
    // Single inline root CBV at b0.
    let param = D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: 0,
                RegisterSpace: 0,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    };
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: 1,
        pParameters: &param,
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
        ..Default::default()
    };
    let mut blob: Option<ID3DBlob> = None;
    let mut err: Option<ID3DBlob> = None;
    D3D12SerializeRootSignature(
        &desc,
        D3D_ROOT_SIGNATURE_VERSION_1,
        &mut blob,
        Some(&mut err),
    )
    .expect("serialize root signature");
    let blob = blob.unwrap();
    let bytes =
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize());
    device
        .CreateRootSignature(0, bytes)
        .expect("root signature")
}

unsafe fn create_pipeline(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    samples: u32,
) -> ID3D12PipelineState {
    let src = include_str!("../../shaders/mesh.hlsl");
    let vs = compile(src, s!("VSMain"), s!("vs_5_0"));
    let ps = compile(src, s!("PSMain"), s!("ps_5_0"));

    let input = [
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("POSITION"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32_FLOAT,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            AlignedByteOffset: 0,
            ..Default::default()
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("POSITION"),
            SemanticIndex: 1,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            AlignedByteOffset: 12,
            ..Default::default()
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("NORMAL"),
            Format: DXGI_FORMAT_R32G32B32_FLOAT,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            AlignedByteOffset: 28,
            ..Default::default()
        },
    ];

    let blend_rt = D3D12_RENDER_TARGET_BLEND_DESC {
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
        ..Default::default()
    };
    let mut blend = D3D12_BLEND_DESC::default();
    blend.RenderTarget[0] = blend_rt;

    let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(Some(root.clone())),
        VS: bytecode(&vs),
        PS: bytecode(&ps),
        BlendState: blend,
        SampleMask: u32::MAX,
        RasterizerState: D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            DepthClipEnable: true.into(),
            ..Default::default()
        },
        DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
            DepthEnable: true.into(),
            DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
            DepthFunc: D3D12_COMPARISON_FUNC_LESS,
            ..Default::default()
        },
        InputLayout: D3D12_INPUT_LAYOUT_DESC {
            pInputElementDescs: input.as_ptr(),
            NumElements: input.len() as u32,
        },
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: 1,
        DSVFormat: DXGI_FORMAT_D32_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: samples,
            Quality: 0,
        },
        ..Default::default()
    };
    desc.RTVFormats[0] = RT_FORMAT;

    device
        .CreateGraphicsPipelineState(&desc)
        .expect("pipeline state")
}

unsafe fn compile(
    src: &str,
    entry: windows::core::PCSTR,
    target: windows::core::PCSTR,
) -> ID3DBlob {
    let flags = if cfg!(debug_assertions) {
        D3DCOMPILE_DEBUG
    } else {
        0
    };
    let mut code: Option<ID3DBlob> = None;
    let mut err: Option<ID3DBlob> = None;
    let result = D3DCompile(
        src.as_ptr() as *const _,
        src.len(),
        None,
        None,
        None,
        entry,
        target,
        flags,
        0,
        &mut code,
        Some(&mut err),
    );
    if result.is_err() {
        if let Some(e) = err {
            let msg =
                std::slice::from_raw_parts(e.GetBufferPointer() as *const u8, e.GetBufferSize());
            panic!("shader compile failed: {}", String::from_utf8_lossy(msg));
        }
        result.expect("shader compile");
    }
    code.unwrap()
}

fn bytecode(blob: &ID3DBlob) -> D3D12_SHADER_BYTECODE {
    unsafe {
        D3D12_SHADER_BYTECODE {
            pShaderBytecode: blob.GetBufferPointer(),
            BytecodeLength: blob.GetBufferSize(),
        }
    }
}

fn transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // Borrow without AddRef: the barrier is consumed before `resource` can drop.
                pResource: unsafe { std::mem::transmute_copy(resource) },
                StateBefore: before,
                StateAfter: after,
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
            }),
        },
    }
}
