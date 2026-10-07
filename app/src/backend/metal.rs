//! Raw Metal backend (macOS). No abstraction layer — direct `metal` crate
//! calls, with the `CAMetalLayer` attached to the winit `NSView` via `objc`.
//!
//! NOTE: this is maintained without a Mac to run it on. It is compile-checked
//! by CI on a macOS runner (see `.github/workflows/ci.yml`);
//! the layer-attach and drawable handling are the spots most likely to need a
//! fixup the first time it runs on real hardware.

use super::Renderer;
use core_graphics_types::geometry::CGSize;
use dscore::{scene::FrameUniforms, Mesh, Vertex};
use metal::{
    Buffer, CommandQueue, CompileOptions, DepthStencilDescriptor, DepthStencilState, Device,
    MTLClearColor, MTLCompareFunction, MTLIndexType, MTLLoadAction, MTLPixelFormat,
    MTLPrimitiveType, MTLResourceOptions, MTLStorageMode, MTLStoreAction, MTLTextureType,
    MTLTextureUsage, MTLVertexFormat, MetalLayer, RenderPassDescriptor, RenderPipelineDescriptor,
    RenderPipelineState, Texture, TextureDescriptor, VertexDescriptor,
};
use objc::{msg_send, runtime::Object, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const PIXEL_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;
const DEPTH_FORMAT: MTLPixelFormat = MTLPixelFormat::Depth32Float;
const MSAA_SAMPLES: u64 = 4;

pub struct MetalRenderer {
    device: Device,
    queue: CommandQueue,
    layer: MetalLayer,
    pipeline: RenderPipelineState,
    pipeline_ms: RenderPipelineState,
    msaa: bool,
    ms_color: Option<Texture>,
    depth_state: DepthStencilState,
    depth_tex: Texture,
    vb: Buffer,
    ib: Buffer,
    ub: Buffer,
    index_count: u64,
    width: u32,
    height: u32,
}

impl Renderer for MetalRenderer {
    fn new(window: &winit::window::Window, mesh: &Mesh) -> Self {
        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));

        let device = Device::system_default().expect("no Metal device");
        let queue = device.new_command_queue();

        let layer = MetalLayer::new();
        layer.set_device(&device);
        layer.set_pixel_format(PIXEL_FORMAT);
        layer.set_presents_with_transaction(false);
        layer.set_drawable_size(CGSize::new(width as f64, height as f64));

        // Attach the layer to the NSView.
        match window.window_handle().unwrap().as_raw() {
            RawWindowHandle::AppKit(h) => unsafe {
                let view = h.ns_view.as_ptr() as *mut Object;
                let _: () = msg_send![view, setWantsLayer: true];
                let _: () = msg_send![view, setLayer: layer.as_ptr()];
            },
            _ => panic!("expected an AppKit window handle"),
        }

        let src = include_str!("../../shaders/mesh.metal");
        let lib = device
            .new_library_with_source(src, &CompileOptions::new())
            .expect("compile metal library");
        let vs = lib.get_function("vs_main", None).unwrap();
        let fs = lib.get_function("fs_main", None).unwrap();

        let vertex_desc = VertexDescriptor::new();
        let attr0 = vertex_desc.attributes().object_at(0).unwrap();
        attr0.set_format(MTLVertexFormat::Float3);
        attr0.set_offset(0);
        attr0.set_buffer_index(0);
        let attr1 = vertex_desc.attributes().object_at(1).unwrap();
        attr1.set_format(MTLVertexFormat::Float4);
        attr1.set_offset(12);
        attr1.set_buffer_index(0);
        let attr2 = vertex_desc.attributes().object_at(2).unwrap();
        attr2.set_format(MTLVertexFormat::Float3);
        attr2.set_offset(28);
        attr2.set_buffer_index(0);
        let layout0 = vertex_desc.layouts().object_at(0).unwrap();
        layout0.set_stride(Vertex::STRIDE as u64);

        let pdesc = RenderPipelineDescriptor::new();
        pdesc.set_vertex_function(Some(&vs));
        pdesc.set_fragment_function(Some(&fs));
        pdesc.set_vertex_descriptor(Some(vertex_desc));
        pdesc
            .color_attachments()
            .object_at(0)
            .unwrap()
            .set_pixel_format(PIXEL_FORMAT);
        pdesc.set_depth_attachment_pixel_format(DEPTH_FORMAT);
        let pipeline = device
            .new_render_pipeline_state(&pdesc)
            .expect("pipeline state");
        pdesc.set_sample_count(MSAA_SAMPLES);
        let pipeline_ms = device
            .new_render_pipeline_state(&pdesc)
            .expect("msaa pipeline state");
        layer.set_display_sync_enabled(false);

        let dsdesc = DepthStencilDescriptor::new();
        dsdesc.set_depth_compare_function(MTLCompareFunction::Less);
        dsdesc.set_depth_write_enabled(true);
        let depth_state = device.new_depth_stencil_state(&dsdesc);

        let vb = device.new_buffer_with_data(
            mesh.vertex_bytes().as_ptr() as *const _,
            mesh.vertex_bytes().len() as u64,
            MTLResourceOptions::StorageModeShared,
        );
        let ib = device.new_buffer_with_data(
            mesh.index_bytes().as_ptr() as *const _,
            mesh.index_bytes().len() as u64,
            MTLResourceOptions::StorageModeShared,
        );
        let ub = device.new_buffer(
            std::mem::size_of::<FrameUniforms>() as u64,
            MTLResourceOptions::StorageModeShared,
        );

        let depth_tex = make_depth(&device, width, height, 1);

        Self {
            device,
            queue,
            layer,
            pipeline,
            pipeline_ms,
            msaa: false,
            ms_color: None,
            depth_state,
            depth_tex,
            vb,
            ib,
            ub,
            index_count: mesh.index_count() as u64,
            width,
            height,
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if (width, height) == (self.width, self.height) {
            return;
        }
        self.layer
            .set_drawable_size(CGSize::new(width as f64, height as f64));
        self.width = width;
        self.height = height;
        self.rebuild_targets();
    }

    fn set_options(&mut self, vsync: bool, msaa: bool) {
        self.layer.set_display_sync_enabled(vsync);
        if msaa != self.msaa {
            self.msaa = msaa;
            self.rebuild_targets();
        }
    }

    fn render(&mut self, uniforms: &FrameUniforms) {
        // Update uniforms.
        unsafe {
            std::ptr::copy_nonoverlapping(
                uniforms.bytes().as_ptr(),
                self.ub.contents() as *mut u8,
                std::mem::size_of::<FrameUniforms>(),
            );
        }

        let drawable = match self.layer.next_drawable() {
            Some(d) => d,
            None => return,
        };

        let rpd = RenderPassDescriptor::new();
        let color = rpd.color_attachments().object_at(0).unwrap();
        if let Some(ms) = self.ms_color.as_ref().filter(|_| self.msaa) {
            color.set_texture(Some(ms));
            color.set_resolve_texture(Some(drawable.texture()));
            color.set_store_action(MTLStoreAction::MultisampleResolve);
        } else {
            color.set_texture(Some(drawable.texture()));
            color.set_store_action(MTLStoreAction::Store);
        }
        color.set_load_action(MTLLoadAction::Clear);
        color.set_clear_color(MTLClearColor::new(
            uniforms.bg[0] as f64,
            uniforms.bg[1] as f64,
            uniforms.bg[2] as f64,
            1.0,
        ));
        let depth = rpd.depth_attachment().unwrap();
        depth.set_texture(Some(&self.depth_tex));
        depth.set_load_action(MTLLoadAction::Clear);
        depth.set_clear_depth(1.0);
        depth.set_store_action(MTLStoreAction::DontCare);

        let cmd = self.queue.new_command_buffer();
        let enc = cmd.new_render_command_encoder(rpd);
        enc.set_render_pipeline_state(if self.msaa {
            &self.pipeline_ms
        } else {
            &self.pipeline
        });
        enc.set_depth_stencil_state(&self.depth_state);
        enc.set_vertex_buffer(0, Some(&self.vb), 0);
        enc.set_vertex_buffer(1, Some(&self.ub), 0);
        enc.set_fragment_buffer(1, Some(&self.ub), 0);
        enc.draw_indexed_primitives(
            MTLPrimitiveType::Triangle,
            self.index_count,
            MTLIndexType::UInt32,
            &self.ib,
            0,
        );
        enc.end_encoding();
        cmd.present_drawable(drawable);
        cmd.commit();
    }
}

impl MetalRenderer {
    fn rebuild_targets(&mut self) {
        let samples = if self.msaa { MSAA_SAMPLES } else { 1 };
        self.depth_tex = make_depth(&self.device, self.width, self.height, samples);
        self.ms_color = if self.msaa {
            let td = TextureDescriptor::new();
            td.set_texture_type(MTLTextureType::D2Multisample);
            td.set_pixel_format(PIXEL_FORMAT);
            td.set_width(self.width as u64);
            td.set_height(self.height as u64);
            td.set_sample_count(MSAA_SAMPLES);
            td.set_storage_mode(MTLStorageMode::Private);
            td.set_usage(MTLTextureUsage::RenderTarget);
            Some(self.device.new_texture(&td))
        } else {
            None
        };
    }
}

fn make_depth(device: &Device, width: u32, height: u32, samples: u64) -> Texture {
    let td = TextureDescriptor::new();
    td.set_texture_type(if samples > 1 {
        MTLTextureType::D2Multisample
    } else {
        MTLTextureType::D2
    });
    td.set_sample_count(samples);
    td.set_pixel_format(DEPTH_FORMAT);
    td.set_width(width as u64);
    td.set_height(height as u64);
    td.set_storage_mode(MTLStorageMode::Private);
    td.set_usage(MTLTextureUsage::RenderTarget);
    device.new_texture(&td)
}
