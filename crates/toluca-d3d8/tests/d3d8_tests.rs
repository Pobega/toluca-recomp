use std::collections::HashMap;

use toluca_core::GuestAddr;
use toluca_d3d8::backend::NullBackend;
use toluca_d3d8::device::{
    D3d8Context, IDIRECT3D8_METHOD_COUNT, IDIRECT3DDEVICE8_METHOD_COUNT,
    IDIRECT3DINDEXBUFFER8_METHOD_COUNT, IDIRECT3DSURFACE8_METHOD_COUNT,
    IDIRECT3DTEXTURE8_METHOD_COUNT, IDIRECT3DVERTEXBUFFER8_METHOD_COUNT, interface_method_count,
};
use toluca_d3d8::mem::{
    COM_THUNK_BASE, ComInterfaceId, GuestMemory, decode_com_thunk, encode_com_thunk,
};
use toluca_d3d8::types::{
    D3D_OK, D3D_SDK_VERSION, D3DADAPTER_DEFAULT, D3DCLEAR_TARGET, D3DCLEAR_ZBUFFER, D3DDEVTYPE_HAL,
    D3DFMT_A8R8G8B8, D3DFMT_D24S8, D3DFMT_DXT1, D3DFMT_DXT3, D3DFMT_DXT5, D3DFMT_INDEX16,
    D3DFMT_R5G6B5, D3DFMT_X8R8G8B8, D3DFVF_DIFFUSE, D3DFVF_NORMAL, D3DFVF_TEX2, D3DFVF_XYZ,
    D3DLOCK_DISCARD, D3DPOOL_MANAGED, D3DPT_TRIANGLELIST, D3DRS_ALPHABLENDENABLE, D3DRS_CULLMODE,
    D3DRS_ZENABLE, D3DSBT_ALL, D3DTS_VIEW, D3DUSAGE_DYNAMIC, D3DUSAGE_WRITEONLY, D3dAdapterIdentifier8, D3dCaps8, D3dDisplayMode,
    D3dIndexBufferDesc, D3dLockedRect, D3dPresentParameters, D3dSurfaceDesc, D3dVertexBufferDesc,
    fvf_vertex_stride, surface_byte_size, surface_row_pitch, write_matrix_4x4,
};

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

    fn live_allocations(&self) -> usize {
        self.allocs.len()
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
        self.allocs
            .remove(&va)
            .map(|_| ())
            .ok_or_else(|| format!("free of unknown mock ptr 0x{va:08x}"))
    }
}

#[test]
fn struct_sizes_and_roundtrips() {
    assert_eq!(D3dDisplayMode::SIZE, 16);
    assert_eq!(D3dPresentParameters::SIZE, 52);
    assert_eq!(D3dAdapterIdentifier8::SIZE, 1068);
    assert_eq!(D3dCaps8::SIZE, 212);
    assert_eq!(D3dSurfaceDesc::SIZE, 32);
    assert_eq!(D3dVertexBufferDesc::SIZE, 24);
    assert_eq!(D3dIndexBufferDesc::SIZE, 20);
    assert_eq!(D3dLockedRect::SIZE, 8);

    let mut mem = MockMemory::new();
    let scratch = mem.alloc(2048, true).unwrap();

    let caps = D3dCaps8::gles3_defaults(D3DDEVTYPE_HAL, D3DADAPTER_DEFAULT, 4096);
    caps.write_to(&mut mem, scratch).unwrap();
    let read_caps = D3dCaps8::read_from(&mem, scratch).unwrap();
    assert_eq!(read_caps, caps);
    assert_eq!(read_caps.vertex_shader_version, 0xFFFE_0101);
    assert_eq!(read_caps.pixel_shader_version, 0xFFFF_0101);

    let ident = D3dAdapterIdentifier8::new_default();
    ident.write_to(&mut mem, scratch).unwrap();
    let read_ident = D3dAdapterIdentifier8::read_from(&mem, scratch).unwrap();
    assert_eq!(read_ident, ident);

    let pp = D3dPresentParameters {
        back_buffer_width: 800,
        back_buffer_height: 600,
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
    pp.write_to(&mut mem, scratch).unwrap();
    assert_eq!(D3dPresentParameters::read_from(&mem, scratch).unwrap(), pp);
}

#[test]
fn format_pitch_and_fvf_helpers() {
    assert_eq!(surface_row_pitch(D3DFMT_A8R8G8B8, 64), 256);
    assert_eq!(surface_byte_size(D3DFMT_A8R8G8B8, 64, 32), 8192);
    assert_eq!(surface_row_pitch(D3DFMT_R5G6B5, 64), 128);

    assert_eq!(surface_row_pitch(D3DFMT_DXT1, 8), 16);
    assert_eq!(surface_byte_size(D3DFMT_DXT1, 8, 8), 32);
    assert_eq!(surface_row_pitch(D3DFMT_DXT3, 8), 32);
    assert_eq!(surface_byte_size(D3DFMT_DXT5, 16, 16), 256);

    let fvf = D3DFVF_XYZ | D3DFVF_NORMAL | D3DFVF_DIFFUSE | D3DFVF_TEX2;
    assert_eq!(fvf_vertex_stride(fvf), 12 + 12 + 4 + 16);
}

#[test]
fn com_thunk_roundtrip_all_interfaces() {
    assert_eq!(encode_com_thunk(ComInterfaceId::Direct3D8, 0), COM_THUNK_BASE);
    assert_eq!(
        encode_com_thunk(ComInterfaceId::Direct3D8, 15),
        COM_THUNK_BASE + 15 * 4
    );

    let ifaces = [
        ComInterfaceId::Direct3D8,
        ComInterfaceId::Direct3DDevice8,
        ComInterfaceId::Direct3DTexture8,
        ComInterfaceId::Direct3DSurface8,
        ComInterfaceId::Direct3DVertexBuffer8,
        ComInterfaceId::Direct3DIndexBuffer8,
    ];
    for iface in ifaces {
        let count = interface_method_count(iface);
        for method in 0..count {
            let va = encode_com_thunk(iface, method);
            assert_eq!(decode_com_thunk(va), Some((iface, method)));
        }
    }
    assert_eq!(decode_com_thunk(COM_THUNK_BASE - 4), None);
    assert_eq!(decode_com_thunk(COM_THUNK_BASE + 2), None);
}

fn call(
    ctx: &mut D3d8Context,
    mem: &mut impl GuestMemory,
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
    let res = ctx.dispatch_com(mem, iface, method_idx, &args).unwrap();
    assert_eq!(res.arg_count as usize, args.len());
    res.ret_val
}

#[test]
fn end_to_end_d3d8_render_sequence() {
    let mut mem = MockMemory::new();
    let mut ctx = D3d8Context::new_null();

    let scratch = mem.alloc(2048, true).unwrap();
    let out_ptr = scratch;
    let pp_va = scratch + 64;
    let caps_va = scratch + 128;
    let ident_va = scratch + 512;
    let mode_va = scratch + 1600;
    let mat_va = scratch + 1664;
    let locked_rect_va = scratch + 1760;

    let d3d8_va = ctx.create_direct3d8(&mut mem, D3D_SDK_VERSION).unwrap();
    assert_ne!(d3d8_va, 0);

    assert_eq!(call(&mut ctx, &mut mem, d3d8_va, 4, &[]), 1);
    assert_eq!(
        call(&mut ctx, &mut mem, d3d8_va, 5, &[0, 0, ident_va]),
        D3D_OK
    );
    let mode_count = call(&mut ctx, &mut mem, d3d8_va, 6, &[0]);
    assert_eq!(mode_count, 12);
    assert_eq!(
        call(&mut ctx, &mut mem, d3d8_va, 7, &[0, 0, mode_va]),
        D3D_OK
    );
    assert_eq!(
        D3dDisplayMode::read_from(&mem, mode_va).unwrap().width,
        640
    );
    assert_eq!(
        call(&mut ctx, &mut mem, d3d8_va, 13, &[0, D3DDEVTYPE_HAL, caps_va]),
        D3D_OK
    );
    assert_eq!(
        D3dCaps8::read_from(&mem, caps_va).unwrap().max_streams,
        16
    );

    let pp = D3dPresentParameters {
        back_buffer_width: 640,
        back_buffer_height: 480,
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
        call(
            &mut ctx,
            &mut mem,
            d3d8_va,
            15,
            &[0, D3DDEVTYPE_HAL, 0x0002_0000, 0x40, pp_va, out_ptr]
        ),
        D3D_OK
    );
    let dev_va = mem.read_u32(out_ptr).unwrap();
    assert_ne!(dev_va, 0);

    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            dev_va,
            20,
            &[16, 16, 1, 0, D3DFMT_A8R8G8B8, D3DPOOL_MANAGED, out_ptr]
        ),
        D3D_OK
    );
    let tex_va = mem.read_u32(out_ptr).unwrap();
    assert_eq!(
        call(&mut ctx, &mut mem, tex_va, 16, &[0, locked_rect_va, 0, 0]),
        D3D_OK
    );
    let lr = D3dLockedRect::read_from(&mem, locked_rect_va).unwrap();
    assert_eq!(lr.pitch, 64);
    mem.write_u32(lr.p_bits, 0xDEAD_BEEF).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);

    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            dev_va,
            23,
            &[
                128,
                D3DUSAGE_WRITEONLY,
                D3DFVF_XYZ | D3DFVF_DIFFUSE,
                D3DPOOL_MANAGED,
                out_ptr
            ]
        ),
        D3D_OK
    );
    let vb_va = mem.read_u32(out_ptr).unwrap();
    assert_eq!(
        call(&mut ctx, &mut mem, vb_va, 11, &[0, 0, out_ptr, 0]),
        D3D_OK
    );
    let vb_ptr = mem.read_u32(out_ptr).unwrap();
    mem.write_f32(vb_ptr, 1.5).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, vb_va, 12, &[]), D3D_OK);

    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            dev_va,
            24,
            &[24, D3DUSAGE_WRITEONLY, D3DFMT_INDEX16, D3DPOOL_MANAGED, out_ptr]
        ),
        D3D_OK
    );
    let ib_va = mem.read_u32(out_ptr).unwrap();
    assert_eq!(
        call(&mut ctx, &mut mem, ib_va, 11, &[0, 0, out_ptr, 0]),
        D3D_OK
    );
    let ib_ptr = mem.read_u32(out_ptr).unwrap();
    mem.write_u32(ib_ptr, 0x0001_0000).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, ib_va, 12, &[]), D3D_OK);

    assert_eq!(call(&mut ctx, &mut mem, dev_va, 34, &[]), D3D_OK);
    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            dev_va,
            36,
            &[0, 0, D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER, 0xFF10_2030, 1.0f32.to_bits(), 0]
        ),
        D3D_OK
    );
    write_matrix_4x4(&mut mem, mat_va, &[2.0; 16]).unwrap();
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 37, &[D3DTS_VIEW, mat_va]),
        D3D_OK
    );
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_ZENABLE, 1]),
        D3D_OK
    );
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_ALPHABLENDENABLE, 1]),
        D3D_OK
    );
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_CULLMODE, 2]),
        D3D_OK
    );
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 61, &[0, tex_va]), D3D_OK);
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 83, &[0, vb_va, 16]),
        D3D_OK
    );
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 85, &[ib_va, 0]), D3D_OK);
    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            dev_va,
            76,
            &[D3DFVF_XYZ | D3DFVF_DIFFUSE]
        ),
        D3D_OK
    );
    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            dev_va,
            71,
            &[D3DPT_TRIANGLELIST, 0, 4, 0, 2]
        ),
        D3D_OK
    );
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 35, &[]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 15, &[0, 0, 0, 0]), D3D_OK);

    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 57, &[D3DSBT_ALL, out_ptr]),
        D3D_OK
    );
    let sb_token = mem.read_u32(out_ptr).unwrap();
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 50, &[D3DRS_CULLMODE, 3]),
        D3D_OK
    );
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 54, &[sb_token]), D3D_OK);
    assert_eq!(
        call(&mut ctx, &mut mem, dev_va, 51, &[D3DRS_CULLMODE, out_ptr]),
        D3D_OK
    );
    assert_eq!(mem.read_u32(out_ptr).unwrap(), 2);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 56, &[sb_token]), D3D_OK);

    assert_eq!(call(&mut ctx, &mut mem, ib_va, 2, &[]), 0);
    assert_eq!(call(&mut ctx, &mut mem, vb_va, 2, &[]), 0);
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 2, &[]), 0);
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 2, &[]), 0);
    assert_eq!(call(&mut ctx, &mut mem, d3d8_va, 2, &[]), 0);

    let backend = ctx.backend().as_any().downcast_ref::<NullBackend>().unwrap();
    assert_eq!(backend.device_creates, 1);
    assert_eq!(backend.texture_uploads, 1);
    assert_eq!(backend.texture_destroys, 1);
    assert_eq!(backend.vb_uploads, 1);
    assert_eq!(backend.vb_destroys, 1);
    assert_eq!(backend.ib_uploads, 1);
    assert_eq!(backend.ib_destroys, 1);
    assert_eq!(backend.clears, 1);
    assert_eq!(backend.indexed_draw_calls, 1);
    assert_eq!(backend.primitives_drawn, 2);
    assert_eq!(backend.presents, 1);

    let _ = IDIRECT3D8_METHOD_COUNT;
    let _ = IDIRECT3DDEVICE8_METHOD_COUNT;
    let _ = IDIRECT3DTEXTURE8_METHOD_COUNT;
    let _ = IDIRECT3DSURFACE8_METHOD_COUNT;
    let _ = IDIRECT3DVERTEXBUFFER8_METHOD_COUNT;
    let _ = IDIRECT3DINDEXBUFFER8_METHOD_COUNT;
    assert!(mem.live_allocations() <= 7);
}

struct CountingMemory {
    inner: MockMemory,
    write_calls: Vec<(GuestAddr, usize)>,
}

impl GuestMemory for CountingMemory {
    fn read_bytes(&self, va: GuestAddr, len: usize) -> Result<Vec<u8>, String> {
        self.inner.read_bytes(va, len)
    }

    fn write_bytes(&mut self, va: GuestAddr, data: &[u8]) -> Result<(), String> {
        self.write_calls.push((va, data.len()));
        self.inner.write_bytes(va, data)
    }

    fn zero_range(&mut self, va: GuestAddr, len: usize) -> Result<(), String> {
        self.inner.zero_range(va, len)
    }

    fn alloc(&mut self, size: usize, zeroed: bool) -> Result<GuestAddr, String> {
        self.inner.alloc(size, zeroed)
    }

    fn free(&mut self, va: GuestAddr) -> Result<(), String> {
        self.inner.free(va)
    }
}

#[test]
fn vb_and_ib_partial_lock_and_discard_sync_only_requested_range() {
    let mut mem = CountingMemory {
        inner: MockMemory::new(),
        write_calls: Vec::new(),
    };
    let mut ctx = D3d8Context::new_null();

    let pp_va = mem.alloc(D3dPresentParameters::SIZE, true).unwrap();
    let out_ptr = mem.alloc(4, true).unwrap();
    let d3d8_va = ctx.create_direct3d8(&mut mem, D3D_SDK_VERSION).unwrap();
    let pp = D3dPresentParameters {
        back_buffer_width: 640,
        back_buffer_height: 480,
        back_buffer_format: D3DFMT_X8R8G8B8,
        back_buffer_count: 1,
        swap_effect: 1,
        windowed: 1,
        enable_auto_depth_stencil: 1,
        auto_depth_stencil_format: D3DFMT_D24S8,
        ..Default::default()
    };
    pp.write_to(&mut mem, pp_va).unwrap();
    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            d3d8_va,
            15,
            &[0, D3DDEVTYPE_HAL, 0x0002_0000, 0x40, pp_va, out_ptr]
        ),
        D3D_OK
    );
    let dev_va = mem.read_u32(out_ptr).unwrap();

    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            dev_va,
            23,
            &[
                1024,
                D3DUSAGE_DYNAMIC | D3DUSAGE_WRITEONLY,
                D3DFVF_XYZ,
                D3DPOOL_MANAGED,
                out_ptr,
            ]
        ),
        D3D_OK
    );
    let vb_va = mem.read_u32(out_ptr).unwrap();

    // First lock with D3DLOCK_DISCARD skips copying 1024 zero bytes into staging.
    mem.write_calls.clear();
    assert_eq!(
        call(&mut ctx, &mut mem, vb_va, 11, &[0, 16, out_ptr, D3DLOCK_DISCARD]),
        D3D_OK
    );
    assert_eq!(mem.write_calls, vec![(out_ptr, 4)]);
    let ptr0 = mem.read_u32(out_ptr).unwrap();
    mem.write_u32(ptr0, 0x1122_3344).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, vb_va, 12, &[]), D3D_OK);

    // Subsequent partial lock syncs only [offset..offset+lock_len] (32 bytes), not 1024 bytes.
    mem.write_calls.clear();
    assert_eq!(
        call(&mut ctx, &mut mem, vb_va, 11, &[64, 32, out_ptr, 0]),
        D3D_OK
    );
    assert_eq!(mem.write_calls, vec![(ptr0 + 64, 32), (out_ptr, 4)]);
    let ptr64 = mem.read_u32(out_ptr).unwrap();
    assert_eq!(ptr64, ptr0 + 64);
    mem.write_u32(ptr64, 0x5566_7788).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, vb_va, 12, &[]), D3D_OK);

    // First slice remains intact in staging and backend upload buffer.
    assert_eq!(mem.read_u32(ptr0).unwrap(), 0x1122_3344);
    assert_eq!(
        ctx.backend()
            .as_any()
            .downcast_ref::<NullBackend>()
            .unwrap()
            .vb_uploads,
        2
    );
}

#[test]
fn lock_rect_unlock_sub_upload_uses_sub_rect_hook_and_staging_cache() {
    use std::any::Any;
    use toluca_d3d8::backend::{ActiveShaders, RenderBackend};
    use toluca_d3d8::state::DeviceState;
    use toluca_d3d8::texture::{DecodedRgba8, PreparedTextureUpload};
    use toluca_d3d8::types::D3dRect;

    struct SubUploadRecord {
        level: u32,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        pixels: Vec<u8>,
    }

    #[derive(Default)]
    struct SubUploadBackend {
        full_uploads: u32,
        sub_uploads: Vec<SubUploadRecord>,
    }

    impl RenderBackend for SubUploadBackend {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
        fn max_texture_size(&self) -> u32 {
            2048
        }
        fn supports_s3tc(&self) -> bool {
            false
        }
        fn on_device_create(&mut self, _: u32, _: u32, _: u32, _: bool) {}
        fn on_device_reset(&mut self, _: u32, _: u32, _: u32, _: bool) {}
        fn on_clear(&mut self, _: u32, _: u32, _: f32, _: u32, _: &[D3dRect]) {}
        fn on_texture_upload(&mut self, _: u32, _: u32, _: &PreparedTextureUpload) {
            self.full_uploads += 1;
        }
        fn on_texture_sub_upload(
            &mut self,
            _tex_handle: u32,
            level: u32,
            x: u32,
            y: u32,
            upload: &DecodedRgba8,
        ) -> bool {
            self.sub_uploads.push(SubUploadRecord {
                level,
                x,
                y,
                w: upload.width,
                h: upload.height,
                pixels: upload.pixels.clone(),
            });
            true
        }
        fn on_texture_destroy(&mut self, _: u32) {}
        fn on_vb_upload(&mut self, _: u32, _: &[u8], _: u32, _: u32) {}
        fn on_vb_destroy(&mut self, _: u32) {}
        fn on_ib_upload(&mut self, _: u32, _: &[u8], _: u32, _: u32) {}
        fn on_ib_destroy(&mut self, _: u32) {}
        fn on_draw_primitive(&mut self, _: &DeviceState, _: &ActiveShaders<'_>, _: u32, _: u32, _: u32) {}
        fn on_draw_indexed_primitive(&mut self, _: &DeviceState, _: &ActiveShaders<'_>, _: u32, _: u32, _: u32, _: u32, _: u32) {}
        fn on_present(&mut self) {}
    }

    let mut mem = MockMemory::new();
    let mut ctx = D3d8Context::new(Box::new(SubUploadBackend::default()));

    let pp_va = mem.alloc(D3dPresentParameters::SIZE, true).unwrap();
    let out_ptr = mem.alloc(4, true).unwrap();
    let d3d8_va = ctx.create_direct3d8(&mut mem, D3D_SDK_VERSION).unwrap();
    let pp = D3dPresentParameters {
        back_buffer_width: 640,
        back_buffer_height: 480,
        back_buffer_format: D3DFMT_X8R8G8B8,
        back_buffer_count: 1,
        swap_effect: 1,
        windowed: 1,
        ..Default::default()
    };
    pp.write_to(&mut mem, pp_va).unwrap();
    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            d3d8_va,
            15,
            &[0, D3DDEVTYPE_HAL, 0x0002_0000, 0x40, pp_va, out_ptr]
        ),
        D3D_OK
    );
    let dev_va = mem.read_u32(out_ptr).unwrap();

    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            dev_va,
            20,
            &[8, 8, 1, 0, D3DFMT_A8R8G8B8, D3DPOOL_MANAGED, out_ptr]
        ),
        D3D_OK
    );
    let tex_va = mem.read_u32(out_ptr).unwrap();
    let lr_va = mem.alloc(8, true).unwrap();
    let rect_va = mem.alloc(16, true).unwrap();

    // First full lock populates initial texture via full upload.
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 16, &[0, lr_va, 0, 0]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);

    // Partial LockRect (2, 3, 4, 5) -> 2x2 sub-rect.
    mem.write_u32(rect_va, 2).unwrap();
    mem.write_u32(rect_va + 4, 3).unwrap();
    mem.write_u32(rect_va + 8, 4).unwrap();
    mem.write_u32(rect_va + 12, 5).unwrap();
    assert_eq!(
        call(&mut ctx, &mut mem, tex_va, 16, &[0, lr_va, rect_va, 0]),
        D3D_OK
    );
    let lr = D3dLockedRect::read_from(&mem, lr_va).unwrap();
    assert_eq!(lr.pitch, 32);
    // Write ARGB 0xFF112233 -> RGBA [0x11, 0x22, 0x33, 0xFF] to (2, 3) and (3, 4).
    mem.write_u32(lr.p_bits, 0xFF11_2233).unwrap();
    mem.write_u32(lr.p_bits + 32 + 4, 0xFF44_5566).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);

    let backend = ctx
        .backend()
        .as_any()
        .downcast_ref::<SubUploadBackend>()
        .unwrap();
    assert_eq!(backend.full_uploads, 1);
    assert_eq!(backend.sub_uploads.len(), 1);
    let rec = &backend.sub_uploads[0];
    assert_eq!((rec.level, rec.x, rec.y, rec.w, rec.h), (0, 2, 3, 2, 2));
    assert_eq!(&rec.pixels[0..4], &[0x11, 0x22, 0x33, 0xFF]);
    assert_eq!(&rec.pixels[12..16], &[0x44, 0x55, 0x66, 0xFF]);
}

#[test]
fn unchanged_full_surface_unlock_skips_upload_until_bytes_or_palette_change() {
    use std::any::Any;
    use toluca_d3d8::backend::{ActiveShaders, RenderBackend};
    use toluca_d3d8::state::DeviceState;
    use toluca_d3d8::texture::PreparedTextureUpload;
    use toluca_d3d8::types::{D3DFMT_P8, D3dRect};

    #[derive(Default)]
    struct SkipTrackBackend {
        full_uploads: u32,
        skipped_uploads: u32,
    }

    impl RenderBackend for SkipTrackBackend {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
        fn max_texture_size(&self) -> u32 {
            2048
        }
        fn supports_s3tc(&self) -> bool {
            false
        }
        fn on_device_create(&mut self, _: u32, _: u32, _: u32, _: bool) {}
        fn on_device_reset(&mut self, _: u32, _: u32, _: u32, _: bool) {}
        fn on_clear(&mut self, _: u32, _: u32, _: f32, _: u32, _: &[D3dRect]) {}
        fn on_texture_upload(&mut self, _: u32, _: u32, _: &PreparedTextureUpload) {
            self.full_uploads += 1;
        }
        fn on_texture_upload_skipped(&mut self) {
            self.skipped_uploads += 1;
        }
        fn on_texture_destroy(&mut self, _: u32) {}
        fn on_vb_upload(&mut self, _: u32, _: &[u8], _: u32, _: u32) {}
        fn on_vb_destroy(&mut self, _: u32) {}
        fn on_ib_upload(&mut self, _: u32, _: &[u8], _: u32, _: u32) {}
        fn on_ib_destroy(&mut self, _: u32) {}
        fn on_draw_primitive(&mut self, _: &DeviceState, _: &ActiveShaders<'_>, _: u32, _: u32, _: u32) {}
        fn on_draw_indexed_primitive(&mut self, _: &DeviceState, _: &ActiveShaders<'_>, _: u32, _: u32, _: u32, _: u32, _: u32) {}
        fn on_present(&mut self) {}
    }

    let mut mem = MockMemory::new();
    let mut ctx = D3d8Context::new(Box::new(SkipTrackBackend::default()));

    let pp_va = mem.alloc(D3dPresentParameters::SIZE, true).unwrap();
    let out_ptr = mem.alloc(4, true).unwrap();
    let d3d8_va = ctx.create_direct3d8(&mut mem, D3D_SDK_VERSION).unwrap();
    let pp = D3dPresentParameters {
        back_buffer_width: 640,
        back_buffer_height: 480,
        back_buffer_format: D3DFMT_X8R8G8B8,
        back_buffer_count: 1,
        swap_effect: 1,
        windowed: 1,
        ..Default::default()
    };
    pp.write_to(&mut mem, pp_va).unwrap();
    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            d3d8_va,
            15,
            &[0, D3DDEVTYPE_HAL, 0x0002_0000, 0x40, pp_va, out_ptr]
        ),
        D3D_OK
    );
    let dev_va = mem.read_u32(out_ptr).unwrap();
    let lr_va = mem.alloc(8, true).unwrap();

    // P8 texture so palette change invalidation is also covered.
    assert_eq!(
        call(
            &mut ctx,
            &mut mem,
            dev_va,
            20,
            &[4, 4, 1, 0, D3DFMT_P8, D3DPOOL_MANAGED, out_ptr]
        ),
        D3D_OK
    );
    let tex_va = mem.read_u32(out_ptr).unwrap();

    // Initial full lock + unlock uploads once.
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 16, &[0, lr_va, 0, 0]), D3D_OK);
    let lr = D3dLockedRect::read_from(&mem, lr_va).unwrap();
    mem.write_u32(lr.p_bits, 0x0102_0304).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);

    // Second full lock + unlock without modifying bytes skips upload.
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 16, &[0, lr_va, 0, 0]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);

    // Modifying a byte triggers a real re-upload.
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 16, &[0, lr_va, 0, 0]), D3D_OK);
    mem.write_u32(lr.p_bits, 0x0506_0708).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);

    // Changing palette entries invalidates P8 cache and forces re-upload even with unchanged texels.
    let pal_va = mem.alloc(1024, true).unwrap();
    mem.write_u32(pal_va, 0xFFFF_0000).unwrap();
    assert_eq!(call(&mut ctx, &mut mem, dev_va, 66, &[0, pal_va]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 16, &[0, lr_va, 0, 0]), D3D_OK);
    assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);

    let backend = ctx
        .backend()
        .as_any()
        .downcast_ref::<SkipTrackBackend>()
        .unwrap();
    assert_eq!(backend.full_uploads, 3);
    assert_eq!(backend.skipped_uploads, 1);
}
