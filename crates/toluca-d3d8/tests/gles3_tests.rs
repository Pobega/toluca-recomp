use std::collections::HashMap;
use std::ffi::c_void;

use toluca_core::GuestAddr;
use toluca_d3d8::device::D3d8Context;
use toluca_d3d8::gles3::{GlGetProcAddrFn, Gles3Backend, NoopPresentTarget};
use toluca_d3d8::mem::{GuestMemory, decode_com_thunk};
use toluca_d3d8::shader_asm::{
    D3DVSD_END, D3DVSDT_FLOAT4, SHADER_END_TOKEN,
};
use toluca_d3d8::types::{
    D3D_OK, D3D_SDK_VERSION, D3DCLEAR_TARGET, D3DCLEAR_ZBUFFER, D3DCULL_NONE, D3DDEVTYPE_HAL,
    D3DFMT_D24S8, D3DFMT_DXT1, D3DFMT_X8R8G8B8, D3DFVF_DIFFUSE, D3DFVF_NORMAL, D3DFVF_TEX1,
    D3DFVF_XYZ, D3DFVF_XYZRHW, D3DLIGHT_DIRECTIONAL, D3DPOOL_MANAGED, D3DPT_TRIANGLELIST,
    D3DRS_CULLMODE, D3DRS_LIGHTING, D3DRS_ZENABLE, D3DTS_PROJECTION, D3DTS_VIEW, D3DTS_WORLD,
    D3DUSAGE_WRITEONLY, D3dLight8, D3dLockedRect, D3dMaterial8, D3dPresentParameters,
    write_matrix_4x4,
};

const EGL_NONE: i32 = 0x3038;
const EGL_RED_SIZE: i32 = 0x3024;
const EGL_GREEN_SIZE: i32 = 0x3023;
const EGL_BLUE_SIZE: i32 = 0x3022;
const EGL_ALPHA_SIZE: i32 = 0x3021;
const EGL_DEPTH_SIZE: i32 = 0x3025;
const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_PBUFFER_BIT: i32 = 0x0001;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_OPENGL_ES3_BIT: i32 = 0x0040;
const EGL_WIDTH: i32 = 0x3057;
const EGL_HEIGHT: i32 = 0x3056;
const EGL_OPENGL_ES_API: u32 = 0x30A0;
const EGL_CONTEXT_CLIENT_VERSION: i32 = 0x3098;
const EGL_PLATFORM_SURFACELESS_MESA: u32 = 0x31DD;

type EglDisplay = *mut c_void;
type EglConfig = *mut c_void;
type EglContext = *mut c_void;
type EglSurface = *mut c_void;

struct EglOffscreenContext {
    lib_egl: *mut c_void,
    display: EglDisplay,
    surface: EglSurface,
    context: EglContext,
    get_proc_addr: GlGetProcAddrFn,
    egl_make_current: unsafe extern "C" fn(EglDisplay, EglSurface, EglSurface, EglContext) -> u32,
    egl_destroy_context: unsafe extern "C" fn(EglDisplay, EglContext) -> u32,
    egl_destroy_surface: unsafe extern "C" fn(EglDisplay, EglSurface) -> u32,
    egl_terminate: unsafe extern "C" fn(EglDisplay) -> u32,
}

impl EglOffscreenContext {
    fn try_create() -> Option<Self> {
        let lib_egl = unsafe { libc::dlopen(c"libEGL.so.1".as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if lib_egl.is_null() {
            return None;
        }

        macro_rules! sym {
            ($name:expr) => {{
                let p = unsafe { libc::dlsym(lib_egl, $name.as_ptr()) };
                if p.is_null() {
                    unsafe { libc::dlclose(lib_egl) };
                    return None;
                }
                unsafe { std::mem::transmute_copy(&p) }
            }};
        }

        let egl_get_display: unsafe extern "C" fn(*mut c_void) -> EglDisplay = sym!(c"eglGetDisplay");
        let egl_initialize: unsafe extern "C" fn(EglDisplay, *mut i32, *mut i32) -> u32 = sym!(c"eglInitialize");
        let egl_bind_api: unsafe extern "C" fn(u32) -> u32 = sym!(c"eglBindAPI");
        let egl_choose_config: unsafe extern "C" fn(EglDisplay, *const i32, *mut EglConfig, i32, *mut i32) -> u32 = sym!(c"eglChooseConfig");
        let egl_create_pbuffer_surface: unsafe extern "C" fn(EglDisplay, EglConfig, *const i32) -> EglSurface = sym!(c"eglCreatePbufferSurface");
        let egl_create_context: unsafe extern "C" fn(EglDisplay, EglConfig, EglContext, *const i32) -> EglContext = sym!(c"eglCreateContext");
        let egl_make_current: unsafe extern "C" fn(EglDisplay, EglSurface, EglSurface, EglContext) -> u32 = sym!(c"eglMakeCurrent");
        let egl_destroy_context: unsafe extern "C" fn(EglDisplay, EglContext) -> u32 = sym!(c"eglDestroyContext");
        let egl_destroy_surface: unsafe extern "C" fn(EglDisplay, EglSurface) -> u32 = sym!(c"eglDestroySurface");
        let egl_terminate: unsafe extern "C" fn(EglDisplay) -> u32 = sym!(c"eglTerminate");
        let get_proc_addr: GlGetProcAddrFn = sym!(c"eglGetProcAddress");

        let mut display = unsafe { egl_get_display(std::ptr::null_mut()) };
        let mut major = 0i32;
        let mut minor = 0i32;
        if display.is_null() || unsafe { egl_initialize(display, &mut major, &mut minor) } == 0 {
            let ext_ptr = unsafe { get_proc_addr(c"eglGetPlatformDisplayEXT".as_ptr()) };
            if !ext_ptr.is_null() {
                let egl_get_platform_display: unsafe extern "C" fn(u32, *mut c_void, *const i32) -> EglDisplay =
                    unsafe { std::mem::transmute_copy(&ext_ptr) };
                display = unsafe { egl_get_platform_display(EGL_PLATFORM_SURFACELESS_MESA, std::ptr::null_mut(), std::ptr::null()) };
            }
            if display.is_null() || unsafe { egl_initialize(display, &mut major, &mut minor) } == 0 {
                unsafe { libc::dlclose(lib_egl) };
                return None;
            }
        }

        if unsafe { egl_bind_api(EGL_OPENGL_ES_API) } == 0 {
            unsafe {
                egl_terminate(display);
                libc::dlclose(lib_egl);
            }
            return None;
        }

        let cfg_attribs = [
            EGL_SURFACE_TYPE,
            EGL_PBUFFER_BIT,
            EGL_RENDERABLE_TYPE,
            EGL_OPENGL_ES3_BIT,
            EGL_RED_SIZE,
            8,
            EGL_GREEN_SIZE,
            8,
            EGL_BLUE_SIZE,
            8,
            EGL_ALPHA_SIZE,
            8,
            EGL_DEPTH_SIZE,
            24,
            EGL_NONE,
        ];
        let mut config: EglConfig = std::ptr::null_mut();
        let mut num_configs = 0i32;
        if unsafe { egl_choose_config(display, cfg_attribs.as_ptr(), &mut config, 1, &mut num_configs) } == 0
            || num_configs == 0
        {
            unsafe {
                egl_terminate(display);
                libc::dlclose(lib_egl);
            }
            return None;
        }

        let pbuf_attribs = [EGL_WIDTH, 1, EGL_HEIGHT, 1, EGL_NONE];
        let surface = unsafe { egl_create_pbuffer_surface(display, config, pbuf_attribs.as_ptr()) };
        let ctx_attribs = [EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE];
        let context = unsafe { egl_create_context(display, config, std::ptr::null_mut(), ctx_attribs.as_ptr()) };
        if surface.is_null() || context.is_null() || unsafe { egl_make_current(display, surface, surface, context) } == 0 {
            unsafe {
                if !context.is_null() {
                    egl_destroy_context(display, context);
                }
                if !surface.is_null() {
                    egl_destroy_surface(display, surface);
                }
                egl_terminate(display);
                libc::dlclose(lib_egl);
            }
            return None;
        }

        Some(Self {
            lib_egl,
            display,
            surface,
            context,
            get_proc_addr,
            egl_make_current,
            egl_destroy_context,
            egl_destroy_surface,
            egl_terminate,
        })
    }
}

impl Drop for EglOffscreenContext {
    fn drop(&mut self) {
        unsafe {
            (self.egl_make_current)(
                self.display,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            (self.egl_destroy_context)(self.display, self.context);
            (self.egl_destroy_surface)(self.display, self.surface);
            (self.egl_terminate)(self.display);
            libc::dlclose(self.lib_egl);
        }
    }
}

struct MockMemory {
    next_va: GuestAddr,
    allocs: HashMap<GuestAddr, Vec<u8>>,
}

impl MockMemory {
    fn new() -> Self {
        Self {
            next_va: 0x1000_0000,
            allocs: HashMap::new(),
        }
    }

    fn locate(&self, va: GuestAddr, len: usize) -> Result<(&[u8], usize), String> {
        for (&base, buf) in &self.allocs {
            let end = base as usize + buf.len();
            if (va as usize) >= (base as usize) && (va as usize) + len <= end {
                return Ok((buf.as_slice(), (va - base) as usize));
            }
        }
        Err(format!("unmapped mock read at 0x{va:08x} len={len}"))
    }

    fn locate_mut(&mut self, va: GuestAddr, len: usize) -> Result<(&mut [u8], usize), String> {
        for (&base, buf) in &mut self.allocs {
            let end = base as usize + buf.len();
            if (va as usize) >= (base as usize) && (va as usize) + len <= end {
                return Ok((buf.as_mut_slice(), (va - base) as usize));
            }
        }
        Err(format!("unmapped mock write at 0x{va:08x} len={len}"))
    }
}

impl GuestMemory for MockMemory {
    fn read_bytes(&self, va: GuestAddr, len: usize) -> Result<Vec<u8>, String> {
        let (buf, off) = self.locate(va, len)?;
        Ok(buf[off..off + len].to_vec())
    }

    fn write_bytes(&mut self, va: GuestAddr, data: &[u8]) -> Result<(), String> {
        let (buf, off) = self.locate_mut(va, data.len())?;
        buf[off..off + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn zero_range(&mut self, va: GuestAddr, len: usize) -> Result<(), String> {
        let (buf, off) = self.locate_mut(va, len)?;
        buf[off..off + len].fill(0);
        Ok(())
    }

    fn alloc(&mut self, size: usize, _zeroed: bool) -> Result<GuestAddr, String> {
        let aligned = size.max(16).next_multiple_of(16);
        let va = self.next_va;
        self.next_va += aligned as u32 + 64;
        self.allocs.insert(va, vec![0u8; aligned]);
        Ok(va)
    }

    fn free(&mut self, va: GuestAddr) -> Result<(), String> {
        self.allocs.remove(&va);
        Ok(())
    }
}

fn call(
    ctx: &mut D3d8Context,
    mem: &mut MockMemory,
    obj_va: GuestAddr,
    method_idx: u32,
    extra_args: &[u32],
) -> u32 {
    let vtable = mem.read_u32(obj_va).unwrap();
    let thunk = mem.read_u32(vtable + method_idx * 4).unwrap();
    let (iface, decoded_method) = decode_com_thunk(thunk).unwrap();
    assert_eq!(decoded_method, method_idx);
    let mut args = Vec::with_capacity(1 + extra_args.len());
    args.push(obj_va);
    args.extend_from_slice(extra_args);
    ctx.dispatch_com(mem, iface, method_idx, &args).unwrap().ret_val
}

fn write_xyzrhw_vertex(buf: &mut Vec<u8>, pos: [f32; 4], color: u32, uv: [f32; 2]) {
    for f in pos {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    buf.extend_from_slice(&color.to_le_bytes());
    for f in uv {
        buf.extend_from_slice(&f.to_le_bytes());
    }
}

fn write_xyz_norm_diff_vertex(buf: &mut Vec<u8>, pos: [f32; 3], norm: [f32; 3], color: u32) {
    for f in pos {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    for f in norm {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    buf.extend_from_slice(&color.to_le_bytes());
}

fn pixel_at(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * width + x) * 4) as usize;
    [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
}

#[test]
fn real_gles3_renders_ui_quad_lit_3d_and_programmable_shaders() {
    let Some(egl) = EglOffscreenContext::try_create() else {
        eprintln!("[gles3_tests] skipping: no EGL ES 3.0 display/driver available");
        return;
    };

    let width = 64u32;
    let height = 64u32;
    let backend = Gles3Backend::new(
        Some(egl.get_proc_addr),
        Box::new(NoopPresentTarget { width, height }),
        None,
    )
    .expect("Gles3Backend should initialize on real EGL ES 3.0 context");

    let mut ctx = D3d8Context::new(Box::new(backend));
    let mut mem = MockMemory::new();
    let scratch = mem.alloc(4096, true).unwrap();
    let out_ptr = scratch;
    let pp_va = scratch + 64;
    let locked_rect_va = scratch + 128;
    let mat_va = scratch + 256;
    let light_va = scratch + 384;
    let vdecl_va = scratch + 512;
    let vs_tok_va = scratch + 640;
    let ps_tok_va = scratch + 768;
    let const_va = scratch + 896;

    let d3d8_va = ctx.create_direct3d8(&mut mem, D3D_SDK_VERSION).unwrap();
    let pp = D3dPresentParameters {
        back_buffer_width: width,
        back_buffer_height: height,
        back_buffer_format: D3DFMT_X8R8G8B8,
        back_buffer_count: 1,
        multi_sample_type: 0,
        swap_effect: 1,
        h_device_window: 0x0002_0000,
        windowed: 1,
        enable_auto_depth_stencil: 1,
        auto_depth_stencil_format: D3DFMT_D24S8,
        flags: 0,
        full_screen_refresh_rate_in_hz: 0,
        full_screen_presentation_interval: 1,
    };
    pp.write_to(&mut mem, pp_va).unwrap();
    assert_eq!(
        call(&mut ctx, &mut mem, d3d8_va, 15, &[0, D3DDEVTYPE_HAL, 0x0002_0000, 0x40, pp_va, out_ptr]),
        D3D_OK
    );
    let dev_va = mem.read_u32(out_ptr).unwrap();

    // Test 1: Clear + DXT1 white texture + XYZRHW colored quad verifying D3DCOLOR .bgra swizzle
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 20, &[4, 4, 1, 0, D3DFMT_DXT1, D3DPOOL_MANAGED, out_ptr]),
        D3D_OK
    );
    let tex_va = mem.read_u32(out_ptr).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 16, &[0, locked_rect_va, 0, 0]), D3D_OK);
    let lr = D3dLockedRect::read_from(&mem, locked_rect_va).unwrap();
    let dxt1_white: [u8; 8] = [0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0];
    mem.write_bytes(lr.p_bits, &dxt1_white).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);

    let mut ui_verts = Vec::new();
    let red_argb = 0xFFFF_0000u32;
    write_xyzrhw_vertex(&mut ui_verts, [8.0, 8.0, 0.5, 1.0], red_argb, [0.0, 0.0]);
    write_xyzrhw_vertex(&mut ui_verts, [32.0, 8.0, 0.5, 1.0], red_argb, [1.0, 0.0]);
    write_xyzrhw_vertex(&mut ui_verts, [8.0, 32.0, 0.5, 1.0], red_argb, [0.0, 1.0]);
    write_xyzrhw_vertex(&mut ui_verts, [32.0, 8.0, 0.5, 1.0], red_argb, [1.0, 0.0]);
    write_xyzrhw_vertex(&mut ui_verts, [32.0, 32.0, 0.5, 1.0], red_argb, [1.0, 1.0]);
    write_xyzrhw_vertex(&mut ui_verts, [8.0, 32.0, 0.5, 1.0], red_argb, [0.0, 1.0]);

    let fvf_ui = D3DFVF_XYZRHW | D3DFVF_DIFFUSE | D3DFVF_TEX1;
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 23, &[ui_verts.len() as u32, D3DUSAGE_WRITEONLY, fvf_ui, D3DPOOL_MANAGED, out_ptr]),
        D3D_OK
    );
    let ui_vb = mem.read_u32(out_ptr).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, ui_vb, 11, &[0, 0, out_ptr, 0]), D3D_OK);
    let ui_vb_ptr = mem.read_u32(out_ptr).unwrap();
    mem.write_bytes(ui_vb_ptr, &ui_verts).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, ui_vb, 12, &[]), D3D_OK);

    assert_eq!(call(&mut ctx, &mut mem, dev_va, 34, &[]), D3D_OK);
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 36, &[0, 0, D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER, 0xFF20_4080, 1.0f32.to_bits(), 0]),
        D3D_OK
    );
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_ZENABLE, 0]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_CULLMODE, D3DCULL_NONE]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 61, &[0, tex_va]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 83, &[0, ui_vb, 28]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 76, &[fvf_ui]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 70, &[D3DPT_TRIANGLELIST, 0, 2]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 35, &[]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 15, &[0, 0, 0, 0]), D3D_OK);

    let (rb_w, rb_h, pixels_ui) = {
        let gles = ctx.backend().as_any().downcast_ref::<Gles3Backend>().unwrap();
        assert_eq!(gles.last_gl_error(), 0);
        gles.readback_rgba8().unwrap()
    };
    assert_eq!((rb_w, rb_h), (64, 64));
    assert_eq!(pixel_at(&pixels_ui, 64, 2, 2), [0x20, 0x40, 0x80, 0xFF]);
    assert_eq!(pixel_at(&pixels_ui, 64, 20, 20), [0xFF, 0x00, 0x00, 0xFF]);

    // Test 2: Untransformed 3D triangle with lighting, depth buffer, and World/View/Proj matrices
    let identity = [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ];
    write_matrix_4x4(&mut mem, mat_va, &identity).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 37, &[D3DTS_WORLD, mat_va]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 37, &[D3DTS_VIEW, mat_va]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 37, &[D3DTS_PROJECTION, mat_va]), D3D_OK);

    let d3d_mat = D3dMaterial8 {
        diffuse: [0.0, 1.0, 0.0, 1.0],
        ambient: [0.0, 0.0, 0.0, 1.0],
        specular: [0.0; 4],
        emissive: [0.0; 4],
        power: 1.0,
    };
    d3d_mat.write_to(&mut mem, mat_va).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 42, &[mat_va]), D3D_OK);

    let dir_light = D3dLight8 {
        light_type: D3DLIGHT_DIRECTIONAL,
        diffuse: [1.0, 1.0, 1.0, 1.0],
        direction: [0.0, 0.0, 1.0],
        ..Default::default()
    };
    dir_light.write_to(&mut mem, light_va).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 44, &[0, light_va]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 46, &[0, 1]), D3D_OK);

    let mut tri3d = Vec::new();
    write_xyz_norm_diff_vertex(&mut tri3d, [-0.5, -0.5, 0.5], [0.0, 0.0, -1.0], 0xFFFF_FFFF);
    write_xyz_norm_diff_vertex(&mut tri3d, [0.5, -0.5, 0.5], [0.0, 0.0, -1.0], 0xFFFF_FFFF);
    write_xyz_norm_diff_vertex(&mut tri3d, [0.0, 0.5, 0.5], [0.0, 0.0, -1.0], 0xFFFF_FFFF);

    let fvf_3d = D3DFVF_XYZ | D3DFVF_NORMAL | D3DFVF_DIFFUSE;
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 23, &[tri3d.len() as u32, D3DUSAGE_WRITEONLY, fvf_3d, D3DPOOL_MANAGED, out_ptr]),
        D3D_OK
    );
    let vb_3d = mem.read_u32(out_ptr).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, vb_3d, 11, &[0, 0, out_ptr, 0]), D3D_OK);
    let vb_3d_ptr = mem.read_u32(out_ptr).unwrap();
    mem.write_bytes(vb_3d_ptr, &tri3d).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, vb_3d, 12, &[]), D3D_OK);

    assert_eq!(call(&mut ctx, &mut mem, dev_va, 34, &[]), D3D_OK);
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 36, &[0, 0, D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER, 0xFF00_0000, 1.0f32.to_bits(), 0]),
        D3D_OK
    );
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 61, &[0, 0]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_ZENABLE, 1]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_LIGHTING, 1]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 83, &[0, vb_3d, 28]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 76, &[fvf_3d]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 70, &[D3DPT_TRIANGLELIST, 0, 1]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 35, &[]), D3D_OK);

    let (_, _, pixels_3d) = {
        let gles = ctx.backend().as_any().downcast_ref::<Gles3Backend>().unwrap();
        assert_eq!(gles.last_gl_error(), 0);
        gles.readback_rgba8().unwrap()
    };
    let center = pixel_at(&pixels_3d, 64, 32, 32);
    assert!(center[1] > 200 && center[0] == 0 && center[2] == 0, "expected lit green triangle at center, got {center:?}");

    // Test 3: Programmable vs_1_1 + ps_1_1 shader pair using c0 constants
    let decl_tokens = [
        (1 << 29),
        (2 << 29) | ((u32::from(D3DVSDT_FLOAT4)) << 16),
        D3DVSD_END,
    ];
    for (i, &t) in decl_tokens.iter().enumerate() {
        mem.write_u32(vdecl_va + (i as u32) * 4, t).unwrap();
    }
    // vs_1_1: mov oPos, v0
    let vs_tokens = [
        0xFFFE_0101u32,
        1,
        0x8000_0000 | (4 << 28) | (0xF << 16),
        0x8000_0000 | (1 << 28) | (0xE4 << 16),
        SHADER_END_TOKEN,
    ];
    for (i, &t) in vs_tokens.iter().enumerate() {
        mem.write_u32(vs_tok_va + (i as u32) * 4, t).unwrap();
    }
    // ps_1_1: mov r0, c0
    let ps_tokens = [
        0xFFFF_0101u32,
        1,
        0x8000_0000 | (0xF << 16),
        0x8000_0000 | (2 << 28) | (0xE4 << 16),
        SHADER_END_TOKEN,
    ];
    for (i, &t) in ps_tokens.iter().enumerate() {
        mem.write_u32(ps_tok_va + (i as u32) * 4, t).unwrap();
    }

    assert_eq!(call(&mut ctx, &mut mem, dev_va, 75, &[vdecl_va, vs_tok_va, out_ptr, 0]), D3D_OK);
    let vs_h = mem.read_u32(out_ptr).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 87, &[ps_tok_va, out_ptr]), D3D_OK);
    let ps_h = mem.read_u32(out_ptr).unwrap();

    let ps_const = [0.0f32, 0.0f32, 1.0f32, 1.0f32];
    for (i, &f) in ps_const.iter().enumerate() {
        mem.write_f32(const_va + (i as u32) * 4, f).unwrap();
    }
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 91, &[0, const_va, 1]), D3D_OK);

    let mut quad_v = Vec::new();
    let quad_coords = [
        [-1.0f32, -1.0, 0.5, 1.0],
        [1.0, -1.0, 0.5, 1.0],
        [-1.0, 1.0, 0.5, 1.0],
        [1.0, -1.0, 0.5, 1.0],
        [1.0, 1.0, 0.5, 1.0],
        [-1.0, 1.0, 0.5, 1.0],
    ];
    for pos in quad_coords {
        for f in pos {
            quad_v.extend_from_slice(&f.to_le_bytes());
        }
    }
    let quad_va = mem.alloc(quad_v.len(), true).unwrap();
    mem.write_bytes(quad_va, &quad_v).unwrap();

    assert_eq!(call(&mut ctx, &mut mem, dev_va, 34, &[]), D3D_OK);
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 36, &[0, 0, D3DCLEAR_TARGET, 0xFF00_0000, 1.0f32.to_bits(), 0]),
        D3D_OK
    );
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_ZENABLE, 0]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 76, &[vs_h]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 88, &[ps_h]), D3D_OK);
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 72, &[D3DPT_TRIANGLELIST, 2, quad_va, 16]),
        D3D_OK
    );
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 35, &[]), D3D_OK);

    let (_, _, pixels_ps) = {
        let gles = ctx.backend().as_any().downcast_ref::<Gles3Backend>().unwrap();
        assert_eq!(gles.last_gl_error(), 0);
        gles.readback_rgba8().unwrap()
    };
    assert_eq!(pixel_at(&pixels_ps, 64, 32, 32), [0x00, 0x00, 0xFF, 0xFF]);

    // Test 4: DrawPrimitiveUP fullscreen XYZRHW quad with DXT1 orange texture + D3DTOP_MODULATE2X
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 20, &[4, 4, 1, 0, D3DFMT_DXT1, D3DPOOL_MANAGED, out_ptr]),
        D3D_OK
    );
    let orange_tex_va = mem.read_u32(out_ptr).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, orange_tex_va, 16, &[0, locked_rect_va, 0, 0]), D3D_OK);
    let orange_lr = D3dLockedRect::read_from(&mem, locked_rect_va).unwrap();
    // RGB565 (31, 34, 2) = 0xFC42 (orange)
    let dxt1_orange: [u8; 8] = [0x42, 0xFC, 0x42, 0xFC, 0, 0, 0, 0];
    mem.write_bytes(orange_lr.p_bits, &dxt1_orange).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, orange_tex_va, 17, &[0]), D3D_OK);

    let mut dpup_verts = Vec::new();
    let half_grey = 0x8080_8080u32;
    let w_f = width as f32;
    let h_f = height as f32;
    write_xyzrhw_vertex(&mut dpup_verts, [-0.5, -0.5, 0.01, 1.0], half_grey, [0.0, 0.0]);
    write_xyzrhw_vertex(&mut dpup_verts, [w_f - 0.5, -0.5, 0.01, 1.0], half_grey, [1.0, 0.0]);
    write_xyzrhw_vertex(&mut dpup_verts, [w_f - 0.5, h_f - 0.5, 0.01, 1.0], half_grey, [1.0, 1.0]);
    write_xyzrhw_vertex(&mut dpup_verts, [-0.5, -0.5, 0.01, 1.0], half_grey, [0.0, 0.0]);
    write_xyzrhw_vertex(&mut dpup_verts, [w_f - 0.5, h_f - 0.5, 0.01, 1.0], half_grey, [1.0, 1.0]);
    write_xyzrhw_vertex(&mut dpup_verts, [-0.5, h_f - 0.5, 0.01, 1.0], half_grey, [0.0, 1.0]);
    let dpup_va = mem.alloc(dpup_verts.len(), true).unwrap();
    mem.write_bytes(dpup_va, &dpup_verts).unwrap();

    assert_eq!(call(&mut ctx, &mut mem, dev_va, 34, &[]), D3D_OK);
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 36, &[0, 0, D3DCLEAR_TARGET, 0xFF00_0000, 1.0f32.to_bits(), 0]),
        D3D_OK
    );
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_ZENABLE, 0]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_CULLMODE, D3DCULL_NONE]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 61, &[0, orange_tex_va]), D3D_OK);
    // Stage 0 COLOROP=D3DTOP_MODULATE2X (5), COLORARG1=TEXTURE (2), COLORARG2=DIFFUSE (0)
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 63, &[0, 1, 5]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 63, &[0, 2, 2]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 63, &[0, 3, 0]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 76, &[fvf_ui]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 88, &[0]), D3D_OK);
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 72, &[D3DPT_TRIANGLELIST, 2, dpup_va, 28]),
        D3D_OK
    );
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 35, &[]), D3D_OK);

    let (_, _, pixels_dpup) = {
        let gles = ctx.backend().as_any().downcast_ref::<Gles3Backend>().unwrap();
        assert_eq!(gles.last_gl_error(), 0);
        gles.readback_rgba8().unwrap()
    };
    for sample in [pixel_at(&pixels_dpup, 64, 48, 16), pixel_at(&pixels_dpup, 64, 16, 48)] {
        assert!(
            sample[0] > 230 && sample[1] > 120 && sample[1] < 160 && sample[2] < 30,
            "expected orange in both triangles with MODULATE2X, got {sample:?}"
        );
    }

    eprintln!("[gles3_tests] verified UI quad, lit 3D triangle, vs_1_1+ps_1_1, and DPUP MODULATE2X on real GLES 3.0");
}
