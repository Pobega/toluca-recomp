use std::collections::HashMap;

use toluca_core::GuestAddr;

use crate::backend::{ActiveShaders, NullBackend, RenderBackend};
use crate::mem::{ComInterfaceId, ComObjectTable, GuestMemory};
use crate::shader_asm::{VertexDeclElement, parse_vertex_declaration};
use crate::state::{DeviceState, StateBlockData, StreamBinding};
use crate::texture::{
    PreparedTextureUpload, convert_surface_rect_to_rgba8_into, prepare_texture_upload_with_buf,
};
use crate::types::{
    D3DADAPTER_DEFAULT, D3DCLEAR_TARGET, D3DDEVTYPE_HAL, D3DDEVTYPE_REF, D3DERR_INVALIDCALL,
    D3DERR_NOTAVAILABLE, D3DFMT_A1R5G5B5, D3DFMT_A4R4G4B4, D3DFMT_A8, D3DFMT_A8L8, D3DFMT_A8R8G8B8,
    D3DFMT_D15S1, D3DFMT_D16, D3DFMT_D16_LOCKABLE, D3DFMT_D24S8, D3DFMT_D24X8, D3DFMT_D32,
    D3DFMT_DXT1, D3DFMT_DXT2, D3DFMT_DXT3, D3DFMT_DXT4, D3DFMT_DXT5, D3DFMT_INDEX16,
    D3DFMT_INDEX32, D3DFMT_L8, D3DFMT_P8, D3DFMT_R5G6B5, D3DFMT_R8G8B8, D3DFMT_UNKNOWN,
    D3DFMT_V8U8, D3DFMT_VERTEXDATA, D3DFMT_X1R5G5B5, D3DFMT_X8R8G8B8, D3DLOCK_DISCARD,
    D3DLOCK_NOOVERWRITE, D3DLOCK_READONLY,
    D3DPT_LINELIST, D3DPT_LINESTRIP, D3DPT_POINTLIST, D3DPT_TRIANGLEFAN, D3DPT_TRIANGLELIST,
    D3DPT_TRIANGLESTRIP, D3DRTYPE_INDEXBUFFER, D3DRTYPE_SURFACE, D3DRTYPE_TEXTURE,
    D3DRTYPE_VERTEXBUFFER, D3DUSAGE_DEPTHSTENCIL, D3DUSAGE_DYNAMIC, D3DUSAGE_RENDERTARGET,
    D3D_OK,
    D3dAdapterIdentifier8, D3dCaps8, D3dDeviceCreationParameters, D3dDisplayMode,
    D3dIndexBufferDesc, D3dLight8, D3dLockedRect, D3dMaterial8, D3dPresentParameters, D3dRect,
    D3dSurfaceDesc, D3dVertexBufferDesc, D3dViewport8, E_FAIL, E_INVALIDARG, E_NOINTERFACE,
    E_OUTOFMEMORY, E_POINTER, HResult, S_FALSE, dxt_block_bytes, format_bytes_per_pixel,
    read_matrix_4x4, surface_byte_size, surface_row_pitch, write_matrix_4x4,
};

pub const IDIRECT3D8_METHOD_COUNT: u32 = 16;
pub const IDIRECT3DDEVICE8_METHOD_COUNT: u32 = 97;
pub const IDIRECT3DTEXTURE8_METHOD_COUNT: u32 = 19;
pub const IDIRECT3DSURFACE8_METHOD_COUNT: u32 = 11;
pub const IDIRECT3DVERTEXBUFFER8_METHOD_COUNT: u32 = 14;
pub const IDIRECT3DINDEXBUFFER8_METHOD_COUNT: u32 = 14;

const DEFAULT_ADAPTER_MONITOR: u32 = 0x0001_0001;
const DEFAULT_TEXTURE_MEM_BYTES: u32 = 256 * 1024 * 1024;
const DEFAULT_WINDOW_WIDTH: u32 = 640;
const DEFAULT_WINDOW_HEIGHT: u32 = 480;
const VS_HANDLE_BASE: u32 = 0x1000_0001;
const PS_HANDLE_BASE: u32 = 0x2000_0001;
const SHADER_DECL_END: u32 = 0xFFFF_FFFF;
const SHADER_FUNC_END: u32 = 0x0000_FFFF;
const MAX_SHADER_TOKENS: usize = 4096;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ComCallResult {
    pub ret_val: u32,
    pub arg_count: u32,
}

#[derive(Copy, Clone)]
struct MethodMeta {
    name: &'static str,
    arg_count: u32,
}

const DIRECT3D8_METHODS: [MethodMeta; IDIRECT3D8_METHOD_COUNT as usize] = [
    MethodMeta { name: "QueryInterface", arg_count: 3 },
    MethodMeta { name: "AddRef", arg_count: 1 },
    MethodMeta { name: "Release", arg_count: 1 },
    MethodMeta { name: "RegisterSoftwareDevice", arg_count: 2 },
    MethodMeta { name: "GetAdapterCount", arg_count: 1 },
    MethodMeta { name: "GetAdapterIdentifier", arg_count: 4 },
    MethodMeta { name: "GetAdapterModeCount", arg_count: 2 },
    MethodMeta { name: "EnumAdapterModes", arg_count: 4 },
    MethodMeta { name: "GetAdapterDisplayMode", arg_count: 3 },
    MethodMeta { name: "CheckDeviceType", arg_count: 6 },
    MethodMeta { name: "CheckDeviceFormat", arg_count: 7 },
    MethodMeta { name: "CheckDeviceMultiSampleType", arg_count: 6 },
    MethodMeta { name: "CheckDepthStencilMatch", arg_count: 6 },
    MethodMeta { name: "GetDeviceCaps", arg_count: 4 },
    MethodMeta { name: "GetAdapterMonitor", arg_count: 2 },
    MethodMeta { name: "CreateDevice", arg_count: 7 },
];

const DIRECT3DDEVICE8_METHODS: [MethodMeta; IDIRECT3DDEVICE8_METHOD_COUNT as usize] = [
    MethodMeta { name: "QueryInterface", arg_count: 3 },
    MethodMeta { name: "AddRef", arg_count: 1 },
    MethodMeta { name: "Release", arg_count: 1 },
    MethodMeta { name: "TestCooperativeLevel", arg_count: 1 },
    MethodMeta { name: "GetAvailableTextureMem", arg_count: 1 },
    MethodMeta { name: "ResourceManagerDiscardBytes", arg_count: 2 },
    MethodMeta { name: "GetDirect3D", arg_count: 2 },
    MethodMeta { name: "GetDeviceCaps", arg_count: 2 },
    MethodMeta { name: "GetDisplayMode", arg_count: 2 },
    MethodMeta { name: "GetCreationParameters", arg_count: 2 },
    MethodMeta { name: "SetCursorProperties", arg_count: 4 },
    MethodMeta { name: "SetCursorPosition", arg_count: 4 },
    MethodMeta { name: "ShowCursor", arg_count: 2 },
    MethodMeta { name: "CreateAdditionalSwapChain", arg_count: 3 },
    MethodMeta { name: "Reset", arg_count: 2 },
    MethodMeta { name: "Present", arg_count: 5 },
    MethodMeta { name: "GetBackBuffer", arg_count: 4 },
    MethodMeta { name: "GetRasterStatus", arg_count: 2 },
    MethodMeta { name: "SetGammaRamp", arg_count: 3 },
    MethodMeta { name: "GetGammaRamp", arg_count: 2 },
    MethodMeta { name: "CreateTexture", arg_count: 8 },
    MethodMeta { name: "CreateVolumeTexture", arg_count: 9 },
    MethodMeta { name: "CreateCubeTexture", arg_count: 7 },
    MethodMeta { name: "CreateVertexBuffer", arg_count: 6 },
    MethodMeta { name: "CreateIndexBuffer", arg_count: 6 },
    MethodMeta { name: "CreateRenderTarget", arg_count: 7 },
    MethodMeta { name: "CreateDepthStencilSurface", arg_count: 6 },
    MethodMeta { name: "CreateImageSurface", arg_count: 5 },
    MethodMeta { name: "CopyRects", arg_count: 6 },
    MethodMeta { name: "UpdateTexture", arg_count: 3 },
    MethodMeta { name: "GetFrontBuffer", arg_count: 2 },
    MethodMeta { name: "SetRenderTarget", arg_count: 3 },
    MethodMeta { name: "GetRenderTarget", arg_count: 2 },
    MethodMeta { name: "GetDepthStencilSurface", arg_count: 2 },
    MethodMeta { name: "BeginScene", arg_count: 1 },
    MethodMeta { name: "EndScene", arg_count: 1 },
    MethodMeta { name: "Clear", arg_count: 7 },
    MethodMeta { name: "SetTransform", arg_count: 3 },
    MethodMeta { name: "GetTransform", arg_count: 3 },
    MethodMeta { name: "MultiplyTransform", arg_count: 3 },
    MethodMeta { name: "SetViewport", arg_count: 2 },
    MethodMeta { name: "GetViewport", arg_count: 2 },
    MethodMeta { name: "SetMaterial", arg_count: 2 },
    MethodMeta { name: "GetMaterial", arg_count: 2 },
    MethodMeta { name: "SetLight", arg_count: 3 },
    MethodMeta { name: "GetLight", arg_count: 3 },
    MethodMeta { name: "LightEnable", arg_count: 3 },
    MethodMeta { name: "GetLightEnable", arg_count: 3 },
    MethodMeta { name: "SetClipPlane", arg_count: 3 },
    MethodMeta { name: "GetClipPlane", arg_count: 3 },
    MethodMeta { name: "SetRenderState", arg_count: 3 },
    MethodMeta { name: "GetRenderState", arg_count: 3 },
    MethodMeta { name: "BeginStateBlock", arg_count: 1 },
    MethodMeta { name: "EndStateBlock", arg_count: 2 },
    MethodMeta { name: "ApplyStateBlock", arg_count: 2 },
    MethodMeta { name: "CaptureStateBlock", arg_count: 2 },
    MethodMeta { name: "DeleteStateBlock", arg_count: 2 },
    MethodMeta { name: "CreateStateBlock", arg_count: 3 },
    MethodMeta { name: "SetClipStatus", arg_count: 2 },
    MethodMeta { name: "GetClipStatus", arg_count: 2 },
    MethodMeta { name: "GetTexture", arg_count: 3 },
    MethodMeta { name: "SetTexture", arg_count: 3 },
    MethodMeta { name: "GetTextureStageState", arg_count: 4 },
    MethodMeta { name: "SetTextureStageState", arg_count: 4 },
    MethodMeta { name: "ValidateDevice", arg_count: 2 },
    MethodMeta { name: "GetInfo", arg_count: 4 },
    MethodMeta { name: "SetPaletteEntries", arg_count: 3 },
    MethodMeta { name: "GetPaletteEntries", arg_count: 3 },
    MethodMeta { name: "SetCurrentTexturePalette", arg_count: 2 },
    MethodMeta { name: "GetCurrentTexturePalette", arg_count: 2 },
    MethodMeta { name: "DrawPrimitive", arg_count: 4 },
    MethodMeta { name: "DrawIndexedPrimitive", arg_count: 6 },
    MethodMeta { name: "DrawPrimitiveUP", arg_count: 5 },
    MethodMeta { name: "DrawIndexedPrimitiveUP", arg_count: 9 },
    MethodMeta { name: "ProcessVertices", arg_count: 6 },
    MethodMeta { name: "CreateVertexShader", arg_count: 5 },
    MethodMeta { name: "SetVertexShader", arg_count: 2 },
    MethodMeta { name: "GetVertexShader", arg_count: 2 },
    MethodMeta { name: "DeleteVertexShader", arg_count: 2 },
    MethodMeta { name: "SetVertexShaderConstant", arg_count: 4 },
    MethodMeta { name: "GetVertexShaderConstant", arg_count: 4 },
    MethodMeta { name: "GetVertexShaderDeclaration", arg_count: 4 },
    MethodMeta { name: "GetVertexShaderFunction", arg_count: 4 },
    MethodMeta { name: "SetStreamSource", arg_count: 4 },
    MethodMeta { name: "GetStreamSource", arg_count: 4 },
    MethodMeta { name: "SetIndices", arg_count: 3 },
    MethodMeta { name: "GetIndices", arg_count: 3 },
    MethodMeta { name: "CreatePixelShader", arg_count: 3 },
    MethodMeta { name: "SetPixelShader", arg_count: 2 },
    MethodMeta { name: "GetPixelShader", arg_count: 2 },
    MethodMeta { name: "DeletePixelShader", arg_count: 2 },
    MethodMeta { name: "SetPixelShaderConstant", arg_count: 4 },
    MethodMeta { name: "GetPixelShaderConstant", arg_count: 4 },
    MethodMeta { name: "GetPixelShaderFunction", arg_count: 4 },
    MethodMeta { name: "DrawRectPatch", arg_count: 4 },
    MethodMeta { name: "DrawTriPatch", arg_count: 4 },
    MethodMeta { name: "DeletePatch", arg_count: 2 },
];

const DIRECT3DTEXTURE8_METHODS: [MethodMeta; IDIRECT3DTEXTURE8_METHOD_COUNT as usize] = [
    MethodMeta { name: "QueryInterface", arg_count: 3 },
    MethodMeta { name: "AddRef", arg_count: 1 },
    MethodMeta { name: "Release", arg_count: 1 },
    MethodMeta { name: "GetDevice", arg_count: 2 },
    MethodMeta { name: "SetPrivateData", arg_count: 5 },
    MethodMeta { name: "GetPrivateData", arg_count: 4 },
    MethodMeta { name: "FreePrivateData", arg_count: 2 },
    MethodMeta { name: "SetPriority", arg_count: 2 },
    MethodMeta { name: "GetPriority", arg_count: 1 },
    MethodMeta { name: "PreLoad", arg_count: 1 },
    MethodMeta { name: "GetType", arg_count: 1 },
    MethodMeta { name: "SetLOD", arg_count: 2 },
    MethodMeta { name: "GetLOD", arg_count: 1 },
    MethodMeta { name: "GetLevelCount", arg_count: 1 },
    MethodMeta { name: "GetLevelDesc", arg_count: 3 },
    MethodMeta { name: "GetSurfaceLevel", arg_count: 3 },
    MethodMeta { name: "LockRect", arg_count: 5 },
    MethodMeta { name: "UnlockRect", arg_count: 2 },
    MethodMeta { name: "AddDirtyRect", arg_count: 2 },
];

const DIRECT3DSURFACE8_METHODS: [MethodMeta; IDIRECT3DSURFACE8_METHOD_COUNT as usize] = [
    MethodMeta { name: "QueryInterface", arg_count: 3 },
    MethodMeta { name: "AddRef", arg_count: 1 },
    MethodMeta { name: "Release", arg_count: 1 },
    MethodMeta { name: "GetDevice", arg_count: 2 },
    MethodMeta { name: "SetPrivateData", arg_count: 5 },
    MethodMeta { name: "GetPrivateData", arg_count: 4 },
    MethodMeta { name: "FreePrivateData", arg_count: 2 },
    MethodMeta { name: "GetContainer", arg_count: 3 },
    MethodMeta { name: "GetDesc", arg_count: 2 },
    MethodMeta { name: "LockRect", arg_count: 4 },
    MethodMeta { name: "UnlockRect", arg_count: 1 },
];

const DIRECT3DVERTEXBUFFER8_METHODS: [MethodMeta; IDIRECT3DVERTEXBUFFER8_METHOD_COUNT as usize] = [
    MethodMeta { name: "QueryInterface", arg_count: 3 },
    MethodMeta { name: "AddRef", arg_count: 1 },
    MethodMeta { name: "Release", arg_count: 1 },
    MethodMeta { name: "GetDevice", arg_count: 2 },
    MethodMeta { name: "SetPrivateData", arg_count: 5 },
    MethodMeta { name: "GetPrivateData", arg_count: 4 },
    MethodMeta { name: "FreePrivateData", arg_count: 2 },
    MethodMeta { name: "SetPriority", arg_count: 2 },
    MethodMeta { name: "GetPriority", arg_count: 1 },
    MethodMeta { name: "PreLoad", arg_count: 1 },
    MethodMeta { name: "GetType", arg_count: 1 },
    MethodMeta { name: "Lock", arg_count: 5 },
    MethodMeta { name: "Unlock", arg_count: 1 },
    MethodMeta { name: "GetDesc", arg_count: 2 },
];

const DIRECT3DINDEXBUFFER8_METHODS: [MethodMeta; IDIRECT3DINDEXBUFFER8_METHOD_COUNT as usize] = [
    MethodMeta { name: "QueryInterface", arg_count: 3 },
    MethodMeta { name: "AddRef", arg_count: 1 },
    MethodMeta { name: "Release", arg_count: 1 },
    MethodMeta { name: "GetDevice", arg_count: 2 },
    MethodMeta { name: "SetPrivateData", arg_count: 5 },
    MethodMeta { name: "GetPrivateData", arg_count: 4 },
    MethodMeta { name: "FreePrivateData", arg_count: 2 },
    MethodMeta { name: "SetPriority", arg_count: 2 },
    MethodMeta { name: "GetPriority", arg_count: 1 },
    MethodMeta { name: "PreLoad", arg_count: 1 },
    MethodMeta { name: "GetType", arg_count: 1 },
    MethodMeta { name: "Lock", arg_count: 5 },
    MethodMeta { name: "Unlock", arg_count: 1 },
    MethodMeta { name: "GetDesc", arg_count: 2 },
];

fn method_meta(iface: ComInterfaceId, method_idx: u32) -> Option<MethodMeta> {
    let idx = method_idx as usize;
    match iface {
        ComInterfaceId::Direct3D8 => DIRECT3D8_METHODS.get(idx).copied(),
        ComInterfaceId::Direct3DDevice8 => DIRECT3DDEVICE8_METHODS.get(idx).copied(),
        ComInterfaceId::Direct3DTexture8 => DIRECT3DTEXTURE8_METHODS.get(idx).copied(),
        ComInterfaceId::Direct3DSurface8 => DIRECT3DSURFACE8_METHODS.get(idx).copied(),
        ComInterfaceId::Direct3DVertexBuffer8 => DIRECT3DVERTEXBUFFER8_METHODS.get(idx).copied(),
        ComInterfaceId::Direct3DIndexBuffer8 => DIRECT3DINDEXBUFFER8_METHODS.get(idx).copied(),
    }
}

pub fn method_arg_count(iface: ComInterfaceId, method_idx: u32) -> Option<u32> {
    method_meta(iface, method_idx).map(|m| m.arg_count)
}

pub fn method_name(iface: ComInterfaceId, method_idx: u32) -> Option<&'static str> {
    method_meta(iface, method_idx).map(|m| m.name)
}

pub fn interface_method_count(iface: ComInterfaceId) -> u32 {
    match iface {
        ComInterfaceId::Direct3D8 => IDIRECT3D8_METHOD_COUNT,
        ComInterfaceId::Direct3DDevice8 => IDIRECT3DDEVICE8_METHOD_COUNT,
        ComInterfaceId::Direct3DTexture8 => IDIRECT3DTEXTURE8_METHOD_COUNT,
        ComInterfaceId::Direct3DSurface8 => IDIRECT3DSURFACE8_METHOD_COUNT,
        ComInterfaceId::Direct3DVertexBuffer8 => IDIRECT3DVERTEXBUFFER8_METHOD_COUNT,
        ComInterfaceId::Direct3DIndexBuffer8 => IDIRECT3DINDEXBUFFER8_METHOD_COUNT,
    }
}

pub struct Direct3D8Obj {
    pub refcount: u32,
    pub sdk_version: u32,
    pub guest_obj: GuestAddr,
    pub display_modes: Vec<D3dDisplayMode>,
}

pub struct VertexShaderObj {
    pub declaration: Vec<u32>,
    pub parsed_decl: Vec<VertexDeclElement>,
    pub function: Vec<u32>,
    pub usage: u32,
}

pub struct PixelShaderObj {
    pub function: Vec<u32>,
}

pub struct DeviceObj {
    pub refcount: u32,
    pub guest_obj: GuestAddr,
    pub d3d8_handle: u32,
    pub adapter: u32,
    pub device_type: u32,
    pub focus_window: u32,
    pub behavior_flags: u32,
    pub present_params: D3dPresentParameters,
    pub state: DeviceState,
    pub backbuffer_surface: u32,
    pub depth_stencil_surface: Option<u32>,
    pub vertex_shaders: HashMap<u32, VertexShaderObj>,
    pub pixel_shaders: HashMap<u32, PixelShaderObj>,
    pub state_blocks: HashMap<u32, StateBlockData>,
    pub next_vs_handle: u32,
    pub next_ps_handle: u32,
    pub next_sb_handle: u32,
    pub cursor_visible: bool,
    pub clip_status: [u32; 2],
    pub gamma_ramp: Vec<u8>,
}

pub struct TextureObj {
    pub refcount: u32,
    pub guest_obj: GuestAddr,
    pub device_handle: u32,
    pub width: u32,
    pub height: u32,
    pub levels: u32,
    pub usage: u32,
    pub format: u32,
    pub pool: u32,
    pub priority: u32,
    pub lod: u32,
    pub mip_surfaces: Vec<u32>,
}

pub struct SurfaceObj {
    pub refcount: u32,
    pub guest_obj: Option<GuestAddr>,
    pub device_handle: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub usage: u32,
    pub pool: u32,
    pub multi_sample_type: u32,
    pub parent_texture: Option<(u32, u32)>,
    pub data: Vec<u8>,
    pub staging_guest_va: Option<GuestAddr>,
    pub staging_page_aligned: bool,
    pub staging_alloc_size: usize,
    pub write_watched: bool,
    pub consecutive_dirty_locks: u8,
    pub lock_flags: Option<u32>,
    pub staging_synced: bool,
    pub locked_rect: Option<(u32, u32, u32, u32)>,
    pub uploaded_once: bool,
    pub last_palette_gen: u64,
}

pub struct VertexBufferObj {
    pub refcount: u32,
    pub guest_obj: GuestAddr,
    pub device_handle: u32,
    pub length: u32,
    pub usage: u32,
    pub fvf: u32,
    pub pool: u32,
    pub priority: u32,
    pub data: Vec<u8>,
    pub staging_guest_va: Option<GuestAddr>,
    pub lock_range: Option<(u32, u32, u32)>,
}

pub struct IndexBufferObj {
    pub refcount: u32,
    pub guest_obj: GuestAddr,
    pub device_handle: u32,
    pub length: u32,
    pub usage: u32,
    pub format: u32,
    pub pool: u32,
    pub priority: u32,
    pub data: Vec<u8>,
    pub staging_guest_va: Option<GuestAddr>,
    pub lock_range: Option<(u32, u32, u32)>,
}

pub struct D3d8Context {
    pub trace_d3d8: bool,
    backend: Box<dyn RenderBackend>,
    com_objects: ComObjectTable,
    next_handle: u32,
    d3d8_objs: HashMap<u32, Direct3D8Obj>,
    devices: HashMap<u32, DeviceObj>,
    textures: HashMap<u32, TextureObj>,
    surfaces: HashMap<u32, SurfaceObj>,
    vertex_buffers: HashMap<u32, VertexBufferObj>,
    index_buffers: HashMap<u32, IndexBufferObj>,
    scratch_vtx: Vec<u8>,
    scratch_idx: Vec<u8>,
    scratch_tex: Vec<u8>,
    scratch_rgba: Vec<u8>,
}

impl Default for D3d8Context {
    fn default() -> Self {
        Self::new_null()
    }
}

const WRITE_WATCH_MIN_BYTES: usize = 64 * 1024;
const WRITE_WATCH_DIRTY_STREAK_LIMIT: u8 = 3;

impl D3d8Context {
    pub fn new(backend: Box<dyn RenderBackend>) -> Self {
        Self {
            trace_d3d8: false,
            backend,
            com_objects: ComObjectTable::new(),
            next_handle: 1,
            d3d8_objs: HashMap::new(),
            devices: HashMap::new(),
            textures: HashMap::new(),
            surfaces: HashMap::new(),
            vertex_buffers: HashMap::new(),
            index_buffers: HashMap::new(),
            scratch_vtx: Vec::new(),
            scratch_idx: Vec::new(),
            scratch_tex: Vec::new(),
            scratch_rgba: Vec::new(),
        }
    }

    pub fn new_null() -> Self {
        Self::new(Box::new(NullBackend::new()))
    }

    pub fn backend(&self) -> &dyn RenderBackend {
        self.backend.as_ref()
    }

    pub fn backend_mut(&mut self) -> &mut dyn RenderBackend {
        self.backend.as_mut()
    }

    pub fn replace_backend(&mut self, backend: Box<dyn RenderBackend>) {
        self.backend = backend;
    }

    pub fn has_devices(&self) -> bool {
        !self.devices.is_empty()
    }

    pub fn vertex_shader(&self, device_handle: u32, vs_handle: u32) -> Option<&VertexShaderObj> {
        self.devices
            .get(&device_handle)
            .and_then(|d| d.vertex_shaders.get(&vs_handle))
    }

    pub fn pixel_shader(&self, device_handle: u32, ps_handle: u32) -> Option<&PixelShaderObj> {
        self.devices
            .get(&device_handle)
            .and_then(|d| d.pixel_shaders.get(&ps_handle))
    }

    pub fn method_arg_count(iface: ComInterfaceId, method_idx: u32) -> Option<u32> {
        method_arg_count(iface, method_idx)
    }

    fn alloc_handle(&mut self) -> u32 {
        let h = self.next_handle;
        self.next_handle = self.next_handle.wrapping_add(1).max(1);
        h
    }

    pub fn create_direct3d8(
        &mut self,
        mem: &mut dyn GuestMemory,
        sdk_version: u32,
    ) -> Result<GuestAddr, String> {
        let handle = self.alloc_handle();
        let guest_obj = self.com_objects.create_object(
            mem,
            ComInterfaceId::Direct3D8,
            IDIRECT3D8_METHOD_COUNT,
            handle,
        )?;
        let obj = Direct3D8Obj {
            refcount: 1,
            sdk_version,
            guest_obj,
            display_modes: standard_display_modes(),
        };
        self.d3d8_objs.insert(handle, obj);
        if self.trace_d3d8 {
            eprintln!("[d3d8] Direct3DCreate8({sdk_version}) -> 0x{guest_obj:08x}");
        }
        Ok(guest_obj)
    }

    pub fn dispatch_com(
        &mut self,
        mem: &mut dyn GuestMemory,
        iface: ComInterfaceId,
        method_idx: u32,
        args: &[u32],
    ) -> Result<ComCallResult, String> {
        let Some(meta) = method_meta(iface, method_idx) else {
            return Err(format!(
                "unknown COM method {}::#{method_idx}",
                iface.name()
            ));
        };
        if args.len() < meta.arg_count as usize {
            return Err(format!(
                "{}::{} expected {} args, got {}",
                iface.name(),
                meta.name,
                meta.arg_count,
                args.len()
            ));
        }

        let this_va = args[0];
        let (obj_iface, handle) = self.com_objects.read_object(mem, this_va)?;
        if obj_iface != iface {
            return Err(format!(
                "COM interface mismatch: vtable is {}, object is {}",
                iface.name(),
                obj_iface.name()
            ));
        }

        let ret_val = match iface {
            ComInterfaceId::Direct3D8 => self.dispatch_direct3d8(mem, handle, method_idx, args)?,
            ComInterfaceId::Direct3DDevice8 => {
                self.dispatch_device(mem, handle, method_idx, args)?
            }
            ComInterfaceId::Direct3DTexture8 => {
                self.dispatch_texture(mem, handle, method_idx, args)?
            }
            ComInterfaceId::Direct3DSurface8 => {
                self.dispatch_surface(mem, handle, method_idx, args)?
            }
            ComInterfaceId::Direct3DVertexBuffer8 => {
                self.dispatch_vertex_buffer(mem, handle, method_idx, args)?
            }
            ComInterfaceId::Direct3DIndexBuffer8 => {
                self.dispatch_index_buffer(mem, handle, method_idx, args)?
            }
        };

        if self.trace_d3d8 {
            eprintln!(
                "[d3d8] {}::{}(this=0x{this_va:08x}, ...) -> 0x{ret_val:08x}",
                iface.name(),
                meta.name
            );
        }

        Ok(ComCallResult {
            ret_val,
            arg_count: meta.arg_count,
        })
    }

    fn query_interface_common(
        &mut self,
        mem: &mut dyn GuestMemory,
        this_va: GuestAddr,
        ppv_obj: GuestAddr,
        add_ref_fn: impl FnOnce(&mut Self) -> Option<u32>,
    ) -> Result<HResult, String> {
        if ppv_obj == 0 {
            return Ok(E_POINTER);
        }
        if add_ref_fn(self).is_some() {
            mem.write_u32(ppv_obj, this_va)?;
            Ok(D3D_OK)
        } else {
            mem.write_u32(ppv_obj, 0)?;
            Ok(E_NOINTERFACE)
        }
    }

    fn dispatch_direct3d8(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        method_idx: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        match method_idx {
            0 => {
                let this_va = args[0];
                let ppv = args[2];
                self.query_interface_common(mem, this_va, ppv, |ctx| {
                    let obj = ctx.d3d8_objs.get_mut(&handle)?;
                    obj.refcount += 1;
                    Some(obj.refcount)
                })
            }
            1 => {
                let Some(obj) = self.d3d8_objs.get_mut(&handle) else {
                    return Ok(0);
                };
                obj.refcount += 1;
                Ok(obj.refcount)
            }
            2 => {
                let Some(obj) = self.d3d8_objs.get_mut(&handle) else {
                    return Ok(0);
                };
                obj.refcount = obj.refcount.saturating_sub(1);
                let rem = obj.refcount;
                if rem == 0 {
                    if let Some(removed) = self.d3d8_objs.remove(&handle) {
                        self.com_objects.free_object(mem, removed.guest_obj)?;
                    }
                }
                Ok(rem)
            }
            3 => Ok(D3D_OK),
            4 => Ok(1),
            5 => {
                let adapter = args[1];
                let p_ident = args[3];
                if adapter != D3DADAPTER_DEFAULT || p_ident == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                D3dAdapterIdentifier8::new_default().write_to(mem, p_ident)?;
                Ok(D3D_OK)
            }
            6 => {
                let adapter = args[1];
                if adapter != D3DADAPTER_DEFAULT {
                    return Ok(0);
                }
                Ok(self
                    .d3d8_objs
                    .get(&handle)
                    .map_or(0, |o| o.display_modes.len() as u32))
            }
            7 => {
                let adapter = args[1];
                let mode_idx = args[2] as usize;
                let p_mode = args[3];
                if adapter != D3DADAPTER_DEFAULT || p_mode == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(mode) = self
                    .d3d8_objs
                    .get(&handle)
                    .and_then(|o| o.display_modes.get(mode_idx).copied())
                else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                mode.write_to(mem, p_mode)?;
                Ok(D3D_OK)
            }
            8 => {
                let adapter = args[1];
                let p_mode = args[2];
                if adapter != D3DADAPTER_DEFAULT || p_mode == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let mode = self
                    .d3d8_objs
                    .get(&handle)
                    .and_then(|o| o.display_modes.first().copied())
                    .unwrap_or(D3dDisplayMode {
                        width: DEFAULT_WINDOW_WIDTH,
                        height: DEFAULT_WINDOW_HEIGHT,
                        refresh_rate: 60,
                        format: D3DFMT_X8R8G8B8,
                    });
                mode.write_to(mem, p_mode)?;
                Ok(D3D_OK)
            }
            9 => {
                let adapter = args[1];
                let check_type = args[2];
                let display_fmt = args[3];
                let back_fmt = args[4];
                if adapter != D3DADAPTER_DEFAULT
                    || !matches!(check_type, D3DDEVTYPE_HAL | D3DDEVTYPE_REF)
                {
                    return Ok(D3DERR_NOTAVAILABLE);
                }
                if is_supported_display_format(display_fmt)
                    && is_supported_backbuffer_format(back_fmt)
                {
                    Ok(D3D_OK)
                } else {
                    Ok(D3DERR_NOTAVAILABLE)
                }
            }
            10 => {
                let adapter = args[1];
                let dev_type = args[2];
                let _adapter_fmt = args[3];
                let usage = args[4];
                let rtype = args[5];
                let check_fmt = args[6];
                if adapter != D3DADAPTER_DEFAULT
                    || !matches!(dev_type, D3DDEVTYPE_HAL | D3DDEVTYPE_REF)
                {
                    return Ok(D3DERR_NOTAVAILABLE);
                }
                if self.is_device_format_supported(usage, rtype, check_fmt) {
                    Ok(D3D_OK)
                } else {
                    Ok(D3DERR_NOTAVAILABLE)
                }
            }
            11 => {
                let adapter = args[1];
                let _dev_type = args[2];
                let _surface_fmt = args[3];
                let _windowed = args[4];
                let multisample = args[5];
                if adapter != D3DADAPTER_DEFAULT || multisample > 1 {
                    return Ok(D3DERR_NOTAVAILABLE);
                }
                Ok(D3D_OK)
            }
            12 => {
                let adapter = args[1];
                let _dev_type = args[2];
                let _adapter_fmt = args[3];
                let rt_fmt = args[4];
                let ds_fmt = args[5];
                if adapter != D3DADAPTER_DEFAULT {
                    return Ok(D3DERR_NOTAVAILABLE);
                }
                if is_supported_backbuffer_format(rt_fmt) && is_supported_depth_format(ds_fmt) {
                    Ok(D3D_OK)
                } else {
                    Ok(D3DERR_NOTAVAILABLE)
                }
            }
            13 => {
                let adapter = args[1];
                let dev_type = args[2];
                let p_caps = args[3];
                if adapter != D3DADAPTER_DEFAULT || p_caps == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                if !matches!(dev_type, D3DDEVTYPE_HAL | D3DDEVTYPE_REF) {
                    return Ok(D3DERR_NOTAVAILABLE);
                }
                let caps =
                    D3dCaps8::gles3_defaults(dev_type, adapter, self.backend.max_texture_size());
                caps.write_to(mem, p_caps)?;
                Ok(D3D_OK)
            }
            14 => {
                let adapter = args[1];
                if adapter == D3DADAPTER_DEFAULT {
                    Ok(DEFAULT_ADAPTER_MONITOR)
                } else {
                    Ok(0)
                }
            }
            15 => self.create_device(mem, handle, args),
            _ => Ok(D3DERR_INVALIDCALL),
        }
    }

    fn is_device_format_supported(&self, usage: u32, rtype: u32, check_fmt: u32) -> bool {
        if (usage & D3DUSAGE_DEPTHSTENCIL) != 0 {
            return is_supported_depth_format(check_fmt);
        }
        if (usage & D3DUSAGE_RENDERTARGET) != 0 {
            return is_supported_backbuffer_format(check_fmt);
        }
        match rtype {
            D3DRTYPE_TEXTURE | D3DRTYPE_SURFACE => is_supported_texture_format(check_fmt),
            D3DRTYPE_VERTEXBUFFER => check_fmt == D3DFMT_VERTEXDATA,
            D3DRTYPE_INDEXBUFFER => matches!(check_fmt, D3DFMT_INDEX16 | D3DFMT_INDEX32),
            _ => false,
        }
    }

    fn create_device(
        &mut self,
        mem: &mut dyn GuestMemory,
        d3d8_handle: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        let adapter = args[1];
        let device_type = args[2];
        let focus_window = args[3];
        let behavior_flags = args[4];
        let p_pp = args[5];
        let pp_dev = args[6];
        if pp_dev == 0 || p_pp == 0 || adapter != D3DADAPTER_DEFAULT {
            return Ok(D3DERR_INVALIDCALL);
        }

        let mut pp = D3dPresentParameters::read_from(mem, p_pp)?;
        if pp.back_buffer_width == 0 {
            pp.back_buffer_width = DEFAULT_WINDOW_WIDTH;
        }
        if pp.back_buffer_height == 0 {
            pp.back_buffer_height = DEFAULT_WINDOW_HEIGHT;
        }
        if pp.back_buffer_format == D3DFMT_UNKNOWN {
            pp.back_buffer_format = D3DFMT_X8R8G8B8;
        }
        if pp.back_buffer_count == 0 {
            pp.back_buffer_count = 1;
        }
        pp.write_to(mem, p_pp)?;

        let device_handle = self.alloc_handle();
        let guest_obj = match self.com_objects.create_object(
            mem,
            ComInterfaceId::Direct3DDevice8,
            IDIRECT3DDEVICE8_METHOD_COUNT,
            device_handle,
        ) {
            Ok(va) => va,
            Err(_) => return Ok(E_OUTOFMEMORY),
        };

        let bb_handle = self.insert_surface(
            device_handle,
            pp.back_buffer_width,
            pp.back_buffer_height,
            pp.back_buffer_format,
            D3DUSAGE_RENDERTARGET,
            0,
            pp.multi_sample_type,
            None,
        );

        let ds_handle = if pp.enable_auto_depth_stencil != 0 {
            let ds_fmt = if pp.auto_depth_stencil_format == D3DFMT_UNKNOWN {
                D3DFMT_D24S8
            } else {
                pp.auto_depth_stencil_format
            };
            Some(self.insert_surface(
                device_handle,
                pp.back_buffer_width,
                pp.back_buffer_height,
                ds_fmt,
                D3DUSAGE_DEPTHSTENCIL,
                0,
                pp.multi_sample_type,
                None,
            ))
        } else {
            None
        };

        let mut state = DeviceState::new(
            pp.back_buffer_width,
            pp.back_buffer_height,
            pp.enable_auto_depth_stencil != 0,
        );
        state.render_target = Some(bb_handle);
        state.depth_stencil = ds_handle;

        let dev = DeviceObj {
            refcount: 1,
            guest_obj,
            d3d8_handle,
            adapter,
            device_type,
            focus_window,
            behavior_flags,
            present_params: pp,
            state,
            backbuffer_surface: bb_handle,
            depth_stencil_surface: ds_handle,
            vertex_shaders: HashMap::new(),
            pixel_shaders: HashMap::new(),
            state_blocks: HashMap::new(),
            next_vs_handle: VS_HANDLE_BASE,
            next_ps_handle: PS_HANDLE_BASE,
            next_sb_handle: 1,
            cursor_visible: true,
            clip_status: [0, 0xFFFF_FFFF],
            gamma_ramp: vec![0u8; 1536],
        };
        self.devices.insert(device_handle, dev);
        self.backend.on_device_create(
            pp.back_buffer_width,
            pp.back_buffer_height,
            pp.back_buffer_format,
            pp.windowed != 0,
        );

        mem.write_u32(pp_dev, guest_obj)?;
        Ok(D3D_OK)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_surface(
        &mut self,
        device_handle: u32,
        width: u32,
        height: u32,
        format: u32,
        usage: u32,
        pool: u32,
        multi_sample_type: u32,
        parent_texture: Option<(u32, u32)>,
    ) -> u32 {
        let handle = self.alloc_handle();
        let byte_len = surface_byte_size(format, width, height);
        self.surfaces.insert(
            handle,
            SurfaceObj {
                refcount: 0,
                guest_obj: None,
                device_handle,
                width,
                height,
                format,
                usage,
                pool,
                multi_sample_type,
                parent_texture,
                data: vec![0u8; byte_len],
                staging_guest_va: None,
                staging_page_aligned: false,
                staging_alloc_size: 0,
                write_watched: false,
                consecutive_dirty_locks: 0,
                lock_flags: None,
                staging_synced: false,
                locked_rect: None,
                uploaded_once: false,
                last_palette_gen: 0,
            },
        );
        handle
    }

    fn obtain_surface_com_obj(
        &mut self,
        mem: &mut dyn GuestMemory,
        surf_handle: u32,
    ) -> Result<GuestAddr, String> {
        let existing = self.surfaces.get(&surf_handle).and_then(|s| s.guest_obj);
        let obj_va = match existing {
            Some(va) => va,
            None => {
                let va = self.com_objects.create_object(
                    mem,
                    ComInterfaceId::Direct3DSurface8,
                    IDIRECT3DSURFACE8_METHOD_COUNT,
                    surf_handle,
                )?;
                if let Some(surf) = self.surfaces.get_mut(&surf_handle) {
                    surf.guest_obj = Some(va);
                }
                va
            }
        };
        if let Some(surf) = self.surfaces.get_mut(&surf_handle) {
            surf.refcount += 1;
        }
        Ok(obj_va)
    }

    fn dispatch_device(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        method_idx: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        match method_idx {
            0 => {
                let this_va = args[0];
                let ppv = args[2];
                self.query_interface_common(mem, this_va, ppv, |ctx| {
                    let dev = ctx.devices.get_mut(&handle)?;
                    dev.refcount += 1;
                    Some(dev.refcount)
                })
            }
            1 => {
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(0);
                };
                dev.refcount += 1;
                Ok(dev.refcount)
            }
            2 => self.release_device(mem, handle),
            3 => Ok(D3D_OK),
            4 => Ok(DEFAULT_TEXTURE_MEM_BYTES),
            5 => Ok(D3D_OK),
            6 => {
                let pp_d3d8 = args[1];
                if pp_d3d8 == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(d3d8_handle) = self.devices.get(&handle).map(|d| d.d3d8_handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let Some(d3d8) = self.d3d8_objs.get_mut(&d3d8_handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                d3d8.refcount += 1;
                mem.write_u32(pp_d3d8, d3d8.guest_obj)?;
                Ok(D3D_OK)
            }
            7 => {
                let p_caps = args[1];
                if p_caps == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(dev) = self.devices.get(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let caps = D3dCaps8::gles3_defaults(
                    dev.device_type,
                    dev.adapter,
                    self.backend.max_texture_size(),
                );
                caps.write_to(mem, p_caps)?;
                Ok(D3D_OK)
            }
            8 => {
                let p_mode = args[1];
                if p_mode == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(dev) = self.devices.get(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let mode = D3dDisplayMode {
                    width: dev.present_params.back_buffer_width,
                    height: dev.present_params.back_buffer_height,
                    refresh_rate: 60,
                    format: dev.present_params.back_buffer_format,
                };
                mode.write_to(mem, p_mode)?;
                Ok(D3D_OK)
            }
            9 => {
                let p_params = args[1];
                if p_params == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(dev) = self.devices.get(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let params = D3dDeviceCreationParameters {
                    adapter_ordinal: dev.adapter,
                    device_type: dev.device_type,
                    h_focus_window: dev.focus_window,
                    behavior_flags: dev.behavior_flags,
                };
                params.write_to(mem, p_params)?;
                Ok(D3D_OK)
            }
            10 | 11 => Ok(D3D_OK),
            12 => {
                let show = args[1] != 0;
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(0);
                };
                let prev = dev.cursor_visible;
                dev.cursor_visible = show;
                Ok(u32::from(prev))
            }
            13 => Ok(D3DERR_NOTAVAILABLE),
            14 => self.device_reset(mem, handle, args[1]),
            15 => {
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                dev.state.frame_count += 1;
                self.backend.on_present();
                Ok(D3D_OK)
            }
            16 => {
                let back_buffer = args[1];
                let pp_bb = args[3];
                if pp_bb == 0 || back_buffer != 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(bb_handle) = self.devices.get(&handle).map(|d| d.backbuffer_surface) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let obj_va = self.obtain_surface_com_obj(mem, bb_handle)?;
                mem.write_u32(pp_bb, obj_va)?;
                Ok(D3D_OK)
            }
            17 => {
                let p_status = args[1];
                if p_status == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                mem.write_u32(p_status, 1)?;
                mem.write_u32(p_status + 4, 0)?;
                Ok(D3D_OK)
            }
            18 => {
                let p_ramp = args[2];
                if p_ramp == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let bytes = mem.read_bytes(p_ramp, 1536)?;
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.gamma_ramp = bytes;
                }
                Ok(D3D_OK)
            }
            19 => {
                let p_ramp = args[1];
                if p_ramp == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                if let Some(dev) = self.devices.get(&handle) {
                    mem.write_bytes(p_ramp, &dev.gamma_ramp)?;
                }
                Ok(D3D_OK)
            }
            20 => self.device_create_texture(mem, handle, args),
            21 | 22 => Ok(D3DERR_NOTAVAILABLE),
            23 => self.device_create_vertex_buffer(mem, handle, args),
            24 => self.device_create_index_buffer(mem, handle, args),
            25 => self.device_create_surface(
                mem,
                handle,
                args[1],
                args[2],
                args[3],
                D3DUSAGE_RENDERTARGET,
                0,
                args[4],
                args[6],
            ),
            26 => self.device_create_surface(
                mem,
                handle,
                args[1],
                args[2],
                args[3],
                D3DUSAGE_DEPTHSTENCIL,
                0,
                args[4],
                args[5],
            ),
            27 => self.device_create_surface(
                mem, handle, args[1], args[2], args[3], 0, 2, 0, args[4],
            ),
            28 => self.device_copy_rects(mem, args),
            29 => self.device_update_texture(mem, args),
            30 => Ok(D3D_OK),
            31 => self.device_set_render_target(mem, handle, args[1], args[2]),
            32 => {
                let pp_rt = args[1];
                if pp_rt == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(rt_handle) = self.devices.get(&handle).and_then(|d| d.state.render_target)
                else {
                    mem.write_u32(pp_rt, 0)?;
                    return Ok(D3DERR_INVALIDCALL);
                };
                let obj_va = self.obtain_surface_com_obj(mem, rt_handle)?;
                mem.write_u32(pp_rt, obj_va)?;
                Ok(D3D_OK)
            }
            33 => {
                let pp_ds = args[1];
                if pp_ds == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(ds_handle) = self.devices.get(&handle).and_then(|d| d.state.depth_stencil)
                else {
                    mem.write_u32(pp_ds, 0)?;
                    return Ok(D3DERR_NOTAVAILABLE);
                };
                let obj_va = self.obtain_surface_com_obj(mem, ds_handle)?;
                mem.write_u32(pp_ds, obj_va)?;
                Ok(D3D_OK)
            }
            34 => {
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                if dev.state.in_scene {
                    return Ok(D3DERR_INVALIDCALL);
                }
                dev.state.in_scene = true;
                Ok(D3D_OK)
            }
            35 => {
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                if !dev.state.in_scene {
                    return Ok(D3DERR_INVALIDCALL);
                }
                dev.state.in_scene = false;
                Ok(D3D_OK)
            }
            36 => self.device_clear(mem, args),
            37..=73 => self.dispatch_device_state_and_draw(mem, handle, method_idx, args),
            74 => Ok(D3D_OK),
            75..=96 => self.dispatch_device_shaders_and_streams(mem, handle, method_idx, args),
            _ => Ok(D3DERR_INVALIDCALL),
        }
    }

    fn dispatch_device_state_and_draw(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        method_idx: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        match method_idx {
            37 => {
                let state_id = args[1];
                let p_mat = args[2];
                if p_mat == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let mat = read_matrix_4x4(mem, p_mat)?;
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_transform(state_id, mat);
                }
                Ok(D3D_OK)
            }
            38 => {
                let state_id = args[1];
                let p_mat = args[2];
                if p_mat == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(dev) = self.devices.get(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let mat = dev.state.get_transform(state_id);
                write_matrix_4x4(mem, p_mat, &mat)?;
                Ok(D3D_OK)
            }
            39 => {
                let state_id = args[1];
                let p_mat = args[2];
                if p_mat == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let mat = read_matrix_4x4(mem, p_mat)?;
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.multiply_transform(state_id, mat);
                }
                Ok(D3D_OK)
            }
            40 => {
                let p_vp = args[1];
                if p_vp == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let vp = D3dViewport8::read_from(mem, p_vp)?;
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_viewport(vp);
                }
                Ok(D3D_OK)
            }
            41 => {
                let p_vp = args[1];
                if p_vp == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(dev) = self.devices.get(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                dev.state.viewport.write_to(mem, p_vp)?;
                Ok(D3D_OK)
            }
            42 => {
                let p_mat = args[1];
                if p_mat == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let mat = D3dMaterial8::read_from(mem, p_mat)?;
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_material(mat);
                }
                Ok(D3D_OK)
            }
            43 => {
                let p_mat = args[1];
                if p_mat == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(dev) = self.devices.get(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                dev.state.material.write_to(mem, p_mat)?;
                Ok(D3D_OK)
            }
            44 => {
                let index = args[1];
                let p_light = args[2];
                if p_light == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let light = D3dLight8::read_from(mem, p_light)?;
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_light(index, light);
                }
                Ok(D3D_OK)
            }
            45 => {
                let index = args[1];
                let p_light = args[2];
                if p_light == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(slot) = self
                    .devices
                    .get(&handle)
                    .and_then(|d| d.state.lights.get(index as usize))
                else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                slot.light.write_to(mem, p_light)?;
                Ok(D3D_OK)
            }
            46 => {
                let index = args[1];
                let enable = args[2] != 0;
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.light_enable(index, enable);
                }
                Ok(D3D_OK)
            }
            47 => {
                let index = args[1];
                let p_enable = args[2];
                if p_enable == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(slot) = self
                    .devices
                    .get(&handle)
                    .and_then(|d| d.state.lights.get(index as usize))
                else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                mem.write_u32(p_enable, u32::from(slot.enabled))?;
                Ok(D3D_OK)
            }
            48 => {
                let index = args[1];
                let p_plane = args[2];
                if p_plane == 0 || index >= 6 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let mut plane = [0.0f32; 4];
                for (i, dst) in plane.iter_mut().enumerate() {
                    *dst = mem.read_f32(p_plane + (i as u32) * 4)?;
                }
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_clip_plane(index, plane);
                }
                Ok(D3D_OK)
            }
            49 => {
                let index = args[1];
                let p_plane = args[2];
                if p_plane == 0 || index >= 6 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(plane) = self
                    .devices
                    .get(&handle)
                    .map(|d| d.state.clip_planes[index as usize])
                else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                for (i, &val) in plane.iter().enumerate() {
                    mem.write_f32(p_plane + (i as u32) * 4, val)?;
                }
                Ok(D3D_OK)
            }
            50 => {
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_render_state(args[1], args[2]);
                }
                Ok(D3D_OK)
            }
            51 => {
                let p_val = args[2];
                if p_val == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let val = self
                    .devices
                    .get(&handle)
                    .map_or(0, |d| d.state.get_render_state(args[1]));
                mem.write_u32(p_val, val)?;
                Ok(D3D_OK)
            }
            52 => {
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                if dev.state.recording_state_block.is_some() {
                    return Ok(D3DERR_INVALIDCALL);
                }
                dev.state.recording_state_block = Some(StateBlockData::default());
                Ok(D3D_OK)
            }
            53 => {
                let p_token = args[1];
                if p_token == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let Some(sb) = dev.state.recording_state_block.take() else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let token = dev.next_sb_handle;
                dev.next_sb_handle = dev.next_sb_handle.wrapping_add(1).max(1);
                dev.state_blocks.insert(token, sb);
                mem.write_u32(p_token, token)?;
                Ok(D3D_OK)
            }
            54 => {
                let token = args[1];
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let Some(sb) = dev.state_blocks.get(&token).cloned() else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                dev.state.apply_state_block(&sb);
                Ok(D3D_OK)
            }
            55 => {
                let token = args[1];
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let Some(mut sb) = dev.state_blocks.remove(&token) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                dev.state.capture_state_block(&mut sb);
                dev.state_blocks.insert(token, sb);
                Ok(D3D_OK)
            }
            56 => {
                let token = args[1];
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                if dev.state_blocks.remove(&token).is_some() {
                    Ok(D3D_OK)
                } else {
                    Ok(D3DERR_INVALIDCALL)
                }
            }
            57 => {
                let sb_type = args[1];
                let p_token = args[2];
                if p_token == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let Some(sb) = dev.state.snapshot_state_block(sb_type) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let token = dev.next_sb_handle;
                dev.next_sb_handle = dev.next_sb_handle.wrapping_add(1).max(1);
                dev.state_blocks.insert(token, sb);
                mem.write_u32(p_token, token)?;
                Ok(D3D_OK)
            }
            58 => {
                let p_status = args[1];
                if p_status == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let c0 = mem.read_u32(p_status)?;
                let c1 = mem.read_u32(p_status + 4)?;
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.clip_status = [c0, c1];
                }
                Ok(D3D_OK)
            }
            59 => {
                let p_status = args[1];
                if p_status == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                if let Some(dev) = self.devices.get(&handle) {
                    mem.write_u32(p_status, dev.clip_status[0])?;
                    mem.write_u32(p_status + 4, dev.clip_status[1])?;
                }
                Ok(D3D_OK)
            }
            60 => {
                let stage = args[1];
                let pp_tex = args[2];
                if pp_tex == 0 || stage >= 8 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let tex_handle = self
                    .devices
                    .get(&handle)
                    .and_then(|d| d.state.bound_textures[stage as usize]);
                if let Some(th) = tex_handle {
                    if let Some(tex) = self.textures.get_mut(&th) {
                        tex.refcount += 1;
                        mem.write_u32(pp_tex, tex.guest_obj)?;
                        return Ok(D3D_OK);
                    }
                }
                mem.write_u32(pp_tex, 0)?;
                Ok(D3D_OK)
            }
            61 => {
                let stage = args[1];
                let tex_va = args[2];
                if stage >= 8 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let tex_handle = if tex_va == 0 {
                    None
                } else {
                    let (iface, th) = self.com_objects.read_object(mem, tex_va)?;
                    if iface != ComInterfaceId::Direct3DTexture8 {
                        return Ok(D3DERR_INVALIDCALL);
                    }
                    Some(th)
                };
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_texture(stage, tex_handle);
                }
                Ok(D3D_OK)
            }
            62 => {
                let stage = args[1];
                let tss_type = args[2];
                let p_val = args[3];
                if p_val == 0 || stage >= 8 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let val = self
                    .devices
                    .get(&handle)
                    .map_or(0, |d| d.state.get_texture_stage_state(stage, tss_type));
                mem.write_u32(p_val, val)?;
                Ok(D3D_OK)
            }
            63 => {
                let stage = args[1];
                let tss_type = args[2];
                let val = args[3];
                if stage >= 8 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_texture_stage_state(stage, tss_type, val);
                }
                Ok(D3D_OK)
            }
            64 => {
                let p_passes = args[1];
                if p_passes != 0 {
                    mem.write_u32(p_passes, 1)?;
                }
                Ok(D3D_OK)
            }
            65 => Ok(S_FALSE),
            66 => {
                let pal_num = args[1];
                let p_entries = args[2];
                if p_entries == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let bytes = mem.read_bytes(p_entries, 1024)?;
                let mut entries = Vec::with_capacity(256);
                for chunk in bytes.chunks_exact(4) {
                    entries.push([chunk[0], chunk[1], chunk[2], chunk[3]]);
                }
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_palette_entries(pal_num, entries);
                }
                Ok(D3D_OK)
            }
            67 => {
                let pal_num = args[1];
                let p_entries = args[2];
                if p_entries == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let mut out = vec![0u8; 1024];
                if let Some(entries) = self
                    .devices
                    .get(&handle)
                    .and_then(|d| d.state.palette_entries.get(&pal_num))
                {
                    for (i, e) in entries.iter().take(256).enumerate() {
                        out[i * 4..i * 4 + 4].copy_from_slice(e);
                    }
                }
                mem.write_bytes(p_entries, &out)?;
                Ok(D3D_OK)
            }
            68 => {
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_current_palette(args[1]);
                }
                Ok(D3D_OK)
            }
            69 => {
                let p_pal = args[1];
                if p_pal == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let val = self
                    .devices
                    .get(&handle)
                    .map_or(0, |d| d.state.current_palette);
                mem.write_u32(p_pal, val)?;
                Ok(D3D_OK)
            }
            70 => {
                let prim_type = args[1];
                let start_vertex = args[2];
                let prim_count = args[3];
                if let Some(dev) = self.devices.get(&handle) {
                    let shaders = active_shaders_for_device(dev);
                    self.backend.on_draw_primitive(
                        &dev.state,
                        &shaders,
                        prim_type,
                        start_vertex,
                        prim_count,
                    );
                }
                Ok(D3D_OK)
            }
            71 => {
                let prim_type = args[1];
                let min_vertex_idx = args[2];
                let num_vertices = args[3];
                let start_index = args[4];
                let prim_count = args[5];
                if let Some(dev) = self.devices.get(&handle) {
                    let shaders = active_shaders_for_device(dev);
                    self.backend.on_draw_indexed_primitive(
                        &dev.state,
                        &shaders,
                        prim_type,
                        min_vertex_idx,
                        num_vertices,
                        start_index,
                        prim_count,
                    );
                }
                Ok(D3D_OK)
            }
            72 => {
                let prim_type = args[1];
                let prim_count = args[2];
                let p_data = args[3];
                let stride = args[4];
                if p_data == 0 || stride == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let vtx_count = vertex_count_for_primitives(prim_type, prim_count);
                let byte_len = (vtx_count as usize).saturating_mul(stride as usize);
                let mut vtx_data = std::mem::take(&mut self.scratch_vtx);
                vtx_data.resize(byte_len, 0);
                let read_res = if byte_len > 0 {
                    mem.read_into(p_data, &mut vtx_data)
                } else {
                    Ok(())
                };
                if let Err(err) = read_res {
                    self.scratch_vtx = vtx_data;
                    return Err(err);
                }
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_stream_source(0, None);
                    let shaders = active_shaders_for_device(dev);
                    self.backend.on_draw_primitive_up(
                        &dev.state,
                        &shaders,
                        prim_type,
                        prim_count,
                        &vtx_data,
                        stride,
                    );
                }
                self.scratch_vtx = vtx_data;
                Ok(D3D_OK)
            }
            73 => {
                let prim_type = args[1];
                let min_idx = args[2];
                let num_verts = args[3];
                let prim_count = args[4];
                let p_idx = args[5];
                let idx_fmt = args[6];
                let p_vtx = args[7];
                let stride = args[8];
                if p_idx == 0 || p_vtx == 0 || stride == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let idx_count = vertex_count_for_primitives(prim_type, prim_count) as usize;
                let idx_elem_size = if idx_fmt == D3DFMT_INDEX32 { 4 } else { 2 };
                let idx_byte_len = idx_count * idx_elem_size;
                let mut idx_data = std::mem::take(&mut self.scratch_idx);
                idx_data.resize(idx_byte_len, 0);
                if idx_byte_len > 0 {
                    if let Err(err) = mem.read_into(p_idx, &mut idx_data) {
                        self.scratch_idx = idx_data;
                        return Err(err);
                    }
                }
                let vtx_count = min_idx.saturating_add(num_verts) as usize;
                let vtx_byte_len = vtx_count.saturating_mul(stride as usize);
                let mut vtx_data = std::mem::take(&mut self.scratch_vtx);
                vtx_data.resize(vtx_byte_len, 0);
                if vtx_byte_len > 0 {
                    if let Err(err) = mem.read_into(p_vtx, &mut vtx_data) {
                        self.scratch_idx = idx_data;
                        self.scratch_vtx = vtx_data;
                        return Err(err);
                    }
                }
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_stream_source(0, None);
                    dev.state.set_indices(None);
                    let shaders = active_shaders_for_device(dev);
                    self.backend.on_draw_indexed_primitive_up(
                        &dev.state,
                        &shaders,
                        prim_type,
                        min_idx,
                        num_verts,
                        prim_count,
                        &idx_data,
                        idx_fmt,
                        &vtx_data,
                        stride,
                    );
                }
                self.scratch_idx = idx_data;
                self.scratch_vtx = vtx_data;
                Ok(D3D_OK)
            }
            _ => Ok(D3DERR_INVALIDCALL),
        }
    }

    fn dispatch_device_shaders_and_streams(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        method_idx: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        match method_idx {
            75 => {
                let p_decl = args[1];
                let p_func = args[2];
                let p_handle = args[3];
                let usage = args[4];
                if p_decl == 0 || p_handle == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let declaration = read_terminated_tokens(mem, p_decl, SHADER_DECL_END)?;
                let Ok(parsed_decl) = parse_vertex_declaration(&declaration) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let function = if p_func != 0 {
                    read_terminated_tokens(mem, p_func, SHADER_FUNC_END)?
                } else {
                    Vec::new()
                };
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let vs_handle = dev.next_vs_handle;
                dev.next_vs_handle = dev.next_vs_handle.wrapping_add(2);
                dev.vertex_shaders.insert(
                    vs_handle,
                    VertexShaderObj {
                        declaration,
                        parsed_decl,
                        function,
                        usage,
                    },
                );
                mem.write_u32(p_handle, vs_handle)?;
                Ok(D3D_OK)
            }
            76 => {
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_vertex_shader(args[1]);
                }
                Ok(D3D_OK)
            }
            77 => {
                let p_handle = args[1];
                if p_handle == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let val = self
                    .devices
                    .get(&handle)
                    .map_or(0, |d| d.state.current_vertex_shader);
                mem.write_u32(p_handle, val)?;
                Ok(D3D_OK)
            }
            78 => {
                let vs_handle = args[1];
                if let Some(dev) = self.devices.get_mut(&handle) {
                    if dev.vertex_shaders.remove(&vs_handle).is_some() {
                        if dev.state.current_vertex_shader == vs_handle {
                            dev.state.current_vertex_shader = 0;
                        }
                        return Ok(D3D_OK);
                    }
                }
                Ok(D3DERR_INVALIDCALL)
            }
            79 => {
                let reg = args[1];
                let p_data = args[2];
                let count = args[3];
                if p_data == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                if let Some(dev) = self.devices.get_mut(&handle) {
                    apply_shader_constants(mem, p_data, reg, count, &mut dev.state, false)?;
                }
                Ok(D3D_OK)
            }
            80 => {
                let reg = args[1];
                let p_data = args[2];
                let count = args[3];
                if p_data == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(dev) = self.devices.get(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                for i in 0..count {
                    let slot = (reg + i) as usize;
                    let vec4 = dev
                        .state
                        .vs_constants
                        .get(slot)
                        .copied()
                        .unwrap_or([0.0; 4]);
                    for (c, &val) in vec4.iter().enumerate() {
                        mem.write_f32(p_data + (i * 4 + c as u32) * 4, val)?;
                    }
                }
                Ok(D3D_OK)
            }
            81 | 82 => {
                let shader_handle = args[1];
                let p_data = args[2];
                let p_size = args[3];
                if p_size == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(vs) = self
                    .devices
                    .get(&handle)
                    .and_then(|d| d.vertex_shaders.get(&shader_handle))
                else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let tokens = if method_idx == 81 {
                    &vs.declaration
                } else {
                    &vs.function
                };
                write_token_buffer(mem, tokens, p_data, p_size)
            }
            83 => {
                let stream = args[1];
                let vb_va = args[2];
                let stride = args[3];
                if stream >= 16 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let binding = if vb_va == 0 {
                    None
                } else {
                    let (iface, vb_handle) = self.com_objects.read_object(mem, vb_va)?;
                    if iface != ComInterfaceId::Direct3DVertexBuffer8 {
                        return Ok(D3DERR_INVALIDCALL);
                    }
                    Some(StreamBinding { vb_handle, stride })
                };
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_stream_source(stream, binding);
                }
                Ok(D3D_OK)
            }
            84 => {
                let stream = args[1];
                let pp_vb = args[2];
                let p_stride = args[3];
                if stream >= 16 || pp_vb == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let binding = self
                    .devices
                    .get(&handle)
                    .and_then(|d| d.state.streams[stream as usize]);
                if let Some(b) = binding {
                    if let Some(vb) = self.vertex_buffers.get_mut(&b.vb_handle) {
                        vb.refcount += 1;
                        mem.write_u32(pp_vb, vb.guest_obj)?;
                        if p_stride != 0 {
                            mem.write_u32(p_stride, b.stride)?;
                        }
                        return Ok(D3D_OK);
                    }
                }
                mem.write_u32(pp_vb, 0)?;
                if p_stride != 0 {
                    mem.write_u32(p_stride, 0)?;
                }
                Ok(D3D_OK)
            }
            85 => {
                let ib_va = args[1];
                let base_vertex_idx = args[2] as i32;
                let binding = if ib_va == 0 {
                    None
                } else {
                    let (iface, ib_handle) = self.com_objects.read_object(mem, ib_va)?;
                    if iface != ComInterfaceId::Direct3DIndexBuffer8 {
                        return Ok(D3DERR_INVALIDCALL);
                    }
                    Some((ib_handle, base_vertex_idx))
                };
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_indices(binding);
                }
                Ok(D3D_OK)
            }
            86 => {
                let pp_ib = args[1];
                let p_base = args[2];
                if pp_ib == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let binding = self.devices.get(&handle).and_then(|d| d.state.bound_indices);
                if let Some((ib_handle, base_idx)) = binding {
                    if let Some(ib) = self.index_buffers.get_mut(&ib_handle) {
                        ib.refcount += 1;
                        mem.write_u32(pp_ib, ib.guest_obj)?;
                        if p_base != 0 {
                            mem.write_i32(p_base, base_idx)?;
                        }
                        return Ok(D3D_OK);
                    }
                }
                mem.write_u32(pp_ib, 0)?;
                if p_base != 0 {
                    mem.write_i32(p_base, 0)?;
                }
                Ok(D3D_OK)
            }
            87 => {
                let p_func = args[1];
                let p_handle = args[2];
                if p_func == 0 || p_handle == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let function = read_terminated_tokens(mem, p_func, SHADER_FUNC_END)?;
                let Some(dev) = self.devices.get_mut(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let ps_handle = dev.next_ps_handle;
                dev.next_ps_handle = dev.next_ps_handle.wrapping_add(2);
                dev.pixel_shaders
                    .insert(ps_handle, PixelShaderObj { function });
                mem.write_u32(p_handle, ps_handle)?;
                Ok(D3D_OK)
            }
            88 => {
                if let Some(dev) = self.devices.get_mut(&handle) {
                    dev.state.set_pixel_shader(args[1]);
                }
                Ok(D3D_OK)
            }
            89 => {
                let p_handle = args[1];
                if p_handle == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let val = self
                    .devices
                    .get(&handle)
                    .map_or(0, |d| d.state.current_pixel_shader);
                mem.write_u32(p_handle, val)?;
                Ok(D3D_OK)
            }
            90 => {
                let ps_handle = args[1];
                if let Some(dev) = self.devices.get_mut(&handle) {
                    if dev.pixel_shaders.remove(&ps_handle).is_some() {
                        if dev.state.current_pixel_shader == ps_handle {
                            dev.state.current_pixel_shader = 0;
                        }
                        return Ok(D3D_OK);
                    }
                }
                Ok(D3DERR_INVALIDCALL)
            }
            91 => {
                let reg = args[1];
                let p_data = args[2];
                let count = args[3];
                if p_data == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                if let Some(dev) = self.devices.get_mut(&handle) {
                    apply_shader_constants(mem, p_data, reg, count, &mut dev.state, true)?;
                }
                Ok(D3D_OK)
            }
            92 => {
                let reg = args[1];
                let p_data = args[2];
                let count = args[3];
                if p_data == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(dev) = self.devices.get(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                for i in 0..count {
                    let slot = (reg + i) as usize;
                    let vec4 = dev
                        .state
                        .ps_constants
                        .get(slot)
                        .copied()
                        .unwrap_or([0.0; 4]);
                    for (c, &val) in vec4.iter().enumerate() {
                        mem.write_f32(p_data + (i * 4 + c as u32) * 4, val)?;
                    }
                }
                Ok(D3D_OK)
            }
            93 => {
                let ps_handle = args[1];
                let p_data = args[2];
                let p_size = args[3];
                if p_size == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(ps) = self
                    .devices
                    .get(&handle)
                    .and_then(|d| d.pixel_shaders.get(&ps_handle))
                else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                write_token_buffer(mem, &ps.function, p_data, p_size)
            }
            94..=96 => Ok(D3D_OK),
            _ => Ok(D3DERR_INVALIDCALL),
        }
    }

    fn release_device(&mut self, mem: &mut dyn GuestMemory, handle: u32) -> Result<u32, String> {
        let Some(dev) = self.devices.get_mut(&handle) else {
            return Ok(0);
        };
        dev.refcount = dev.refcount.saturating_sub(1);
        let rem = dev.refcount;
        if rem == 0 {
            if let Some(removed) = self.devices.remove(&handle) {
                self.com_objects.free_object(mem, removed.guest_obj)?;
                self.destroy_surface_if_unreferenced(mem, removed.backbuffer_surface)?;
                if let Some(ds) = removed.depth_stencil_surface {
                    self.destroy_surface_if_unreferenced(mem, ds)?;
                }
            }
        }
        Ok(rem)
    }

    fn device_reset(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        p_pp: GuestAddr,
    ) -> Result<HResult, String> {
        if p_pp == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }
        let mut pp = D3dPresentParameters::read_from(mem, p_pp)?;
        if pp.back_buffer_width == 0 {
            pp.back_buffer_width = DEFAULT_WINDOW_WIDTH;
        }
        if pp.back_buffer_height == 0 {
            pp.back_buffer_height = DEFAULT_WINDOW_HEIGHT;
        }
        if pp.back_buffer_format == D3DFMT_UNKNOWN {
            pp.back_buffer_format = D3DFMT_X8R8G8B8;
        }
        pp.write_to(mem, p_pp)?;

        let Some(dev) = self.devices.get_mut(&handle) else {
            return Ok(D3DERR_INVALIDCALL);
        };
        dev.present_params = pp;
        let bb = dev.backbuffer_surface;
        let ds = dev.depth_stencil_surface;
        dev.state.reset(
            pp.back_buffer_width,
            pp.back_buffer_height,
            pp.enable_auto_depth_stencil != 0,
        );
        dev.state.render_target = Some(bb);
        dev.state.depth_stencil = ds;

        if let Some(surf) = self.surfaces.get_mut(&bb) {
            surf.width = pp.back_buffer_width;
            surf.height = pp.back_buffer_height;
            surf.format = pp.back_buffer_format;
            surf.data
                .resize(surface_byte_size(surf.format, surf.width, surf.height), 0);
        }
        if let Some(ds_h) = ds {
            if let Some(surf) = self.surfaces.get_mut(&ds_h) {
                surf.width = pp.back_buffer_width;
                surf.height = pp.back_buffer_height;
                if pp.auto_depth_stencil_format != D3DFMT_UNKNOWN {
                    surf.format = pp.auto_depth_stencil_format;
                }
                surf.data
                    .resize(surface_byte_size(surf.format, surf.width, surf.height), 0);
            }
        }
        self.backend.on_device_reset(
            pp.back_buffer_width,
            pp.back_buffer_height,
            pp.back_buffer_format,
            pp.windowed != 0,
        );
        Ok(D3D_OK)
    }

    fn device_create_texture(
        &mut self,
        mem: &mut dyn GuestMemory,
        device_handle: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        let width = args[1];
        let height = args[2];
        let levels = args[3];
        let usage = args[4];
        let format = args[5];
        let pool = args[6];
        let pp_tex = args[7];
        if pp_tex == 0 || width == 0 || height == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }

        let actual_levels = calc_mip_levels(width, height, levels);
        let tex_handle = self.alloc_handle();
        let guest_obj = match self.com_objects.create_object(
            mem,
            ComInterfaceId::Direct3DTexture8,
            IDIRECT3DTEXTURE8_METHOD_COUNT,
            tex_handle,
        ) {
            Ok(va) => va,
            Err(_) => return Ok(E_OUTOFMEMORY),
        };

        let mut mip_surfaces = Vec::with_capacity(actual_levels as usize);
        for level in 0..actual_levels {
            let mip_w = (width >> level).max(1);
            let mip_h = (height >> level).max(1);
            let surf_h = self.insert_surface(
                device_handle,
                mip_w,
                mip_h,
                format,
                usage,
                pool,
                0,
                Some((tex_handle, level)),
            );
            mip_surfaces.push(surf_h);
        }

        self.textures.insert(
            tex_handle,
            TextureObj {
                refcount: 1,
                guest_obj,
                device_handle,
                width,
                height,
                levels: actual_levels,
                usage,
                format,
                pool,
                priority: 0,
                lod: 0,
                mip_surfaces,
            },
        );
        mem.write_u32(pp_tex, guest_obj)?;
        Ok(D3D_OK)
    }

    fn device_create_vertex_buffer(
        &mut self,
        mem: &mut dyn GuestMemory,
        device_handle: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        let length = args[1];
        let usage = args[2];
        let fvf = args[3];
        let pool = args[4];
        let pp_vb = args[5];
        if pp_vb == 0 || length == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }

        let vb_handle = self.alloc_handle();
        let guest_obj = match self.com_objects.create_object(
            mem,
            ComInterfaceId::Direct3DVertexBuffer8,
            IDIRECT3DVERTEXBUFFER8_METHOD_COUNT,
            vb_handle,
        ) {
            Ok(va) => va,
            Err(_) => return Ok(E_OUTOFMEMORY),
        };

        self.vertex_buffers.insert(
            vb_handle,
            VertexBufferObj {
                refcount: 1,
                guest_obj,
                device_handle,
                length,
                usage,
                fvf,
                pool,
                priority: 0,
                data: vec![0u8; length as usize],
                staging_guest_va: None,
                lock_range: None,
            },
        );
        mem.write_u32(pp_vb, guest_obj)?;
        Ok(D3D_OK)
    }

    fn device_create_index_buffer(
        &mut self,
        mem: &mut dyn GuestMemory,
        device_handle: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        let length = args[1];
        let usage = args[2];
        let format = args[3];
        let pool = args[4];
        let pp_ib = args[5];
        if pp_ib == 0 || length == 0 || !matches!(format, D3DFMT_INDEX16 | D3DFMT_INDEX32) {
            return Ok(D3DERR_INVALIDCALL);
        }

        let ib_handle = self.alloc_handle();
        let guest_obj = match self.com_objects.create_object(
            mem,
            ComInterfaceId::Direct3DIndexBuffer8,
            IDIRECT3DINDEXBUFFER8_METHOD_COUNT,
            ib_handle,
        ) {
            Ok(va) => va,
            Err(_) => return Ok(E_OUTOFMEMORY),
        };

        self.index_buffers.insert(
            ib_handle,
            IndexBufferObj {
                refcount: 1,
                guest_obj,
                device_handle,
                length,
                usage,
                format,
                pool,
                priority: 0,
                data: vec![0u8; length as usize],
                staging_guest_va: None,
                lock_range: None,
            },
        );
        mem.write_u32(pp_ib, guest_obj)?;
        Ok(D3D_OK)
    }

    #[allow(clippy::too_many_arguments)]
    fn device_create_surface(
        &mut self,
        mem: &mut dyn GuestMemory,
        device_handle: u32,
        width: u32,
        height: u32,
        format: u32,
        usage: u32,
        pool: u32,
        multi_sample: u32,
        pp_surf: GuestAddr,
    ) -> Result<HResult, String> {
        if pp_surf == 0 || width == 0 || height == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }
        let surf_h = self.insert_surface(
            device_handle,
            width,
            height,
            format,
            usage,
            pool,
            multi_sample,
            None,
        );
        let obj_va = self.obtain_surface_com_obj(mem, surf_h)?;
        mem.write_u32(pp_surf, obj_va)?;
        Ok(D3D_OK)
    }

    fn device_copy_rects(&mut self, mem: &dyn GuestMemory, args: &[u32]) -> Result<HResult, String> {
        let src_va = args[1];
        let _p_src_rects = args[2];
        let _c_rects = args[3];
        let dst_va = args[4];
        let _p_dst_pts = args[5];
        if src_va == 0 || dst_va == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }
        let (src_iface, src_h) = self.com_objects.read_object(mem, src_va)?;
        let (dst_iface, dst_h) = self.com_objects.read_object(mem, dst_va)?;
        if src_iface != ComInterfaceId::Direct3DSurface8
            || dst_iface != ComInterfaceId::Direct3DSurface8
        {
            return Ok(D3DERR_INVALIDCALL);
        }
        if !self.surfaces.contains_key(&dst_h) {
            return Ok(D3DERR_INVALIDCALL);
        }
        let mut buf = std::mem::take(&mut self.scratch_tex);
        {
            let Some(src) = self.surfaces.get(&src_h) else {
                self.scratch_tex = buf;
                return Ok(D3DERR_INVALIDCALL);
            };
            buf.clear();
            buf.extend_from_slice(&src.data);
        }
        self.copy_into_surface_and_upload(&buf, dst_h);
        self.scratch_tex = buf;
        Ok(D3D_OK)
    }

    fn device_update_texture(
        &mut self,
        mem: &dyn GuestMemory,
        args: &[u32],
    ) -> Result<HResult, String> {
        let src_va = args[1];
        let dst_va = args[2];
        if src_va == 0 || dst_va == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }
        let (src_iface, src_h) = self.com_objects.read_object(mem, src_va)?;
        let (dst_iface, dst_h) = self.com_objects.read_object(mem, dst_va)?;
        if src_iface != ComInterfaceId::Direct3DTexture8
            || dst_iface != ComInterfaceId::Direct3DTexture8
        {
            return Ok(D3DERR_INVALIDCALL);
        }

        let Some(src_mips) = self.textures.get(&src_h).map(|t| t.mip_surfaces.clone()) else {
            return Ok(D3DERR_INVALIDCALL);
        };
        let Some(dst_mips) = self.textures.get(&dst_h).map(|t| t.mip_surfaces.clone()) else {
            return Ok(D3DERR_INVALIDCALL);
        };

        let levels = src_mips.len().min(dst_mips.len());
        let mut buf = std::mem::take(&mut self.scratch_tex);
        for level in 0..levels {
            {
                let Some(src) = self.surfaces.get(&src_mips[level]) else {
                    continue;
                };
                buf.clear();
                buf.extend_from_slice(&src.data);
            }
            self.copy_into_surface_and_upload(&buf, dst_mips[level]);
        }
        self.scratch_tex = buf;
        Ok(D3D_OK)
    }

    fn copy_into_surface_and_upload(&mut self, src_data: &[u8], dst_h: u32) {
        let Some((dev_h, fmt)) = self.surfaces.get(&dst_h).map(|s| (s.device_handle, s.format))
        else {
            return;
        };
        let cur_pal_gen = self.surface_palette_gen(dev_h);
        let Some(dst) = self.surfaces.get_mut(&dst_h) else {
            return;
        };
        let copy_len = src_data.len().min(dst.data.len());
        let palette_same = fmt != D3DFMT_P8 || cur_pal_gen == dst.last_palette_gen;
        if dst.uploaded_once
            && palette_same
            && copy_len == dst.data.len()
            && dst.data == src_data[..copy_len]
        {
            self.backend.on_texture_upload_skipped();
            return;
        }
        dst.data[..copy_len].copy_from_slice(&src_data[..copy_len]);
        dst.staging_synced = false;
        self.upload_surface_texture(dst_h);
    }

    fn surface_palette_gen(&self, device_handle: u32) -> u64 {
        self.devices
            .get(&device_handle)
            .map_or(0, |dev| dev.state.palette_gen())
    }

    fn surface_palette(&self, device_handle: u32, format: u32) -> Option<[[u8; 4]; 256]> {
        if format != D3DFMT_P8 {
            return None;
        }
        self.devices.get(&device_handle).and_then(|dev| {
            let entries = dev.state.palette_entries.get(&dev.state.current_palette)?;
            let mut arr = [[0u8; 4]; 256];
            for (i, &entry) in entries.iter().take(256).enumerate() {
                arr[i] = entry;
            }
            Some(arr)
        })
    }

    fn upload_surface_texture(&mut self, surf_handle: u32) {
        let Some((tex_h, level, dev_h, fmt, width, height)) = self.surfaces.get(&surf_handle).and_then(|s| {
            let (tex_h, level) = s.parent_texture?;
            Some((tex_h, level, s.device_handle, s.format, s.width, s.height))
        }) else {
            return;
        };
        let palette = self.surface_palette(dev_h, fmt);
        let cur_pal_gen = self.surface_palette_gen(dev_h);
        let pitch = surface_row_pitch(fmt, width);
        let scratch_buf = std::mem::take(&mut self.scratch_rgba);
        let prepared = {
            let surf = self.surfaces.get(&surf_handle).unwrap();
            prepare_texture_upload_with_buf(
                fmt,
                width,
                height,
                pitch,
                &surf.data,
                palette.as_ref(),
                self.backend.max_texture_size(),
                self.backend.supports_s3tc(),
                scratch_buf,
            )
        };
        if let Ok(prepared) = prepared {
            self.backend.on_texture_upload(tex_h, level, &prepared);
            self.scratch_rgba = match prepared {
                PreparedTextureUpload::Rgba8(img) => img.pixels,
                PreparedTextureUpload::CompressedS3tc { data, .. } => data,
            };
            if let Some(surf) = self.surfaces.get_mut(&surf_handle) {
                surf.uploaded_once = true;
                surf.last_palette_gen = cur_pal_gen;
            }
        }
    }

    fn device_set_render_target(
        &mut self,
        mem: &dyn GuestMemory,
        device_handle: u32,
        rt_va: GuestAddr,
        ds_va: GuestAddr,
    ) -> Result<HResult, String> {
        let rt_h = if rt_va == 0 {
            None
        } else {
            let (iface, h) = self.com_objects.read_object(mem, rt_va)?;
            if iface != ComInterfaceId::Direct3DSurface8 {
                return Ok(D3DERR_INVALIDCALL);
            }
            Some(h)
        };
        let ds_h = if ds_va == 0 {
            None
        } else {
            let (iface, h) = self.com_objects.read_object(mem, ds_va)?;
            if iface != ComInterfaceId::Direct3DSurface8 {
                return Ok(D3DERR_INVALIDCALL);
            }
            Some(h)
        };
        if let Some(dev) = self.devices.get_mut(&device_handle) {
            if let Some(rt) = rt_h {
                dev.state.render_target = Some(rt);
            }
            dev.state.depth_stencil = ds_h;
        }
        Ok(D3D_OK)
    }

    fn device_clear(&mut self, mem: &dyn GuestMemory, args: &[u32]) -> Result<HResult, String> {
        let count = args[1];
        let p_rects = args[2];
        let flags = args[3];
        let color = args[4];
        let z = f32::from_bits(args[5]);
        let stencil = args[6];

        let mut rects = Vec::new();
        if count > 0 && p_rects != 0 {
            let clamped = count.min(256);
            rects.reserve(clamped as usize);
            for i in 0..clamped {
                rects.push(D3dRect::read_from(mem, p_rects + i * (D3dRect::SIZE as u32))?);
            }
        } else if (flags & D3DCLEAR_TARGET) == 0 && flags == 0 {
            return Ok(D3D_OK);
        }

        self.backend.on_clear(flags, color, z, stencil, &rects);
        Ok(D3D_OK)
    }

    fn dispatch_texture(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        method_idx: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        match method_idx {
            0 => {
                let this_va = args[0];
                let ppv = args[2];
                self.query_interface_common(mem, this_va, ppv, |ctx| {
                    let t = ctx.textures.get_mut(&handle)?;
                    t.refcount += 1;
                    Some(t.refcount)
                })
            }
            1 => {
                let Some(t) = self.textures.get_mut(&handle) else {
                    return Ok(0);
                };
                t.refcount += 1;
                Ok(t.refcount)
            }
            2 => self.release_texture(mem, handle),
            3 => {
                let pp_dev = args[1];
                let Some(dev_h) = self.textures.get(&handle).map(|t| t.device_handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                self.return_device_ptr(mem, dev_h, pp_dev)
            }
            4 | 6 => Ok(D3D_OK),
            5 => Ok(E_FAIL),
            7 => {
                let prio = args[1];
                let Some(t) = self.textures.get_mut(&handle) else {
                    return Ok(0);
                };
                let prev = t.priority;
                t.priority = prio;
                Ok(prev)
            }
            8 => Ok(self.textures.get(&handle).map_or(0, |t| t.priority)),
            9 => Ok(0),
            10 => Ok(D3DRTYPE_TEXTURE),
            11 => {
                let lod = args[1];
                let Some(t) = self.textures.get_mut(&handle) else {
                    return Ok(0);
                };
                let prev = t.lod;
                t.lod = lod;
                Ok(prev)
            }
            12 => Ok(self.textures.get(&handle).map_or(0, |t| t.lod)),
            13 => Ok(self.textures.get(&handle).map_or(0, |t| t.levels)),
            14 => {
                let level = args[1];
                let p_desc = args[2];
                if p_desc == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(surf_h) = self
                    .textures
                    .get(&handle)
                    .and_then(|t| t.mip_surfaces.get(level as usize).copied())
                else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                self.write_surface_desc(mem, surf_h, p_desc)
            }
            15 => {
                let level = args[1];
                let pp_surf = args[2];
                if pp_surf == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(surf_h) = self
                    .textures
                    .get(&handle)
                    .and_then(|t| t.mip_surfaces.get(level as usize).copied())
                else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                let obj_va = self.obtain_surface_com_obj(mem, surf_h)?;
                mem.write_u32(pp_surf, obj_va)?;
                Ok(D3D_OK)
            }
            16 => {
                let level = args[1];
                let p_locked = args[2];
                let p_rect = args[3];
                let flags = args[4];
                let Some(surf_h) = self
                    .textures
                    .get(&handle)
                    .and_then(|t| t.mip_surfaces.get(level as usize).copied())
                else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                self.lock_surface(mem, surf_h, p_locked, p_rect, flags)
            }
            17 => {
                let level = args[1];
                let Some(surf_h) = self
                    .textures
                    .get(&handle)
                    .and_then(|t| t.mip_surfaces.get(level as usize).copied())
                else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                self.unlock_surface(mem, surf_h)
            }
            18 => Ok(D3D_OK),
            _ => Ok(D3DERR_INVALIDCALL),
        }
    }

    fn release_texture(&mut self, mem: &mut dyn GuestMemory, handle: u32) -> Result<u32, String> {
        let Some(tex) = self.textures.get_mut(&handle) else {
            return Ok(0);
        };
        tex.refcount = tex.refcount.saturating_sub(1);
        let rem = tex.refcount;
        if rem == 0 {
            if let Some(removed) = self.textures.remove(&handle) {
                self.backend.on_texture_destroy(handle);
                self.com_objects.free_object(mem, removed.guest_obj)?;
                for surf_h in removed.mip_surfaces {
                    if let Some(surf) = self.surfaces.get_mut(&surf_h) {
                        surf.parent_texture = None;
                    }
                    self.destroy_surface_if_unreferenced(mem, surf_h)?;
                }
            }
        }
        Ok(rem)
    }

    fn dispatch_surface(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        method_idx: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        match method_idx {
            0 => {
                let this_va = args[0];
                let ppv = args[2];
                self.query_interface_common(mem, this_va, ppv, |ctx| {
                    let s = ctx.surfaces.get_mut(&handle)?;
                    s.refcount += 1;
                    Some(s.refcount)
                })
            }
            1 => {
                let Some(s) = self.surfaces.get_mut(&handle) else {
                    return Ok(0);
                };
                s.refcount += 1;
                Ok(s.refcount)
            }
            2 => {
                let Some(s) = self.surfaces.get_mut(&handle) else {
                    return Ok(0);
                };
                s.refcount = s.refcount.saturating_sub(1);
                let rem = s.refcount;
                if rem == 0 {
                    let keep_owned = s.parent_texture.is_some();
                    if keep_owned {
                        if let Some(obj_va) = s.guest_obj.take() {
                            self.com_objects.free_object(mem, obj_va)?;
                        }
                    } else {
                        self.destroy_surface_if_unreferenced(mem, handle)?;
                    }
                }
                Ok(rem)
            }
            3 => {
                let pp_dev = args[1];
                let Some(dev_h) = self.surfaces.get(&handle).map(|s| s.device_handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                self.return_device_ptr(mem, dev_h, pp_dev)
            }
            4 | 6 => Ok(D3D_OK),
            5 => Ok(E_FAIL),
            7 => {
                let pp_container = args[2];
                if pp_container == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(parent) = self.surfaces.get(&handle).and_then(|s| s.parent_texture)
                else {
                    return Ok(E_NOINTERFACE);
                };
                let Some(tex) = self.textures.get_mut(&parent.0) else {
                    return Ok(E_NOINTERFACE);
                };
                tex.refcount += 1;
                mem.write_u32(pp_container, tex.guest_obj)?;
                Ok(D3D_OK)
            }
            8 => self.write_surface_desc(mem, handle, args[1]),
            9 => self.lock_surface(mem, handle, args[1], args[2], args[3]),
            10 => self.unlock_surface(mem, handle),
            _ => Ok(D3DERR_INVALIDCALL),
        }
    }

    fn maybe_arm_surface_write_watch(&mut self, mem: &mut dyn GuestMemory, surf_handle: u32) {
        let Some(surf) = self.surfaces.get_mut(&surf_handle) else {
            return;
        };
        let Some(va) = surf.staging_guest_va else {
            return;
        };
        if surf.staging_page_aligned
            && !surf.write_watched
            && surf.staging_synced
            && surf.consecutive_dirty_locks < WRITE_WATCH_DIRTY_STREAK_LIMIT
            && surf.lock_flags.is_none()
            && surf.uploaded_once
            && mem.arm_write_watch(va, surf.staging_alloc_size)
        {
            surf.write_watched = true;
        }
    }

    fn destroy_surface_if_unreferenced(
        &mut self,
        mem: &mut dyn GuestMemory,
        surf_handle: u32,
    ) -> Result<(), String> {
        let should_remove = self
            .surfaces
            .get(&surf_handle)
            .is_some_and(|s| s.refcount == 0 && s.parent_texture.is_none());
        if !should_remove {
            return Ok(());
        }
        if let Some(surf) = self.surfaces.remove(&surf_handle) {
            if let Some(obj_va) = surf.guest_obj {
                self.com_objects.free_object(mem, obj_va)?;
            }
            if let Some(staging_va) = surf.staging_guest_va {
                if surf.write_watched {
                    mem.remove_write_watch(staging_va, surf.staging_alloc_size);
                }
                if surf.staging_page_aligned {
                    mem.free_page_aligned(staging_va, surf.staging_alloc_size)?;
                } else {
                    mem.free(staging_va)?;
                }
            }
        }
        Ok(())
    }

    fn write_surface_desc(
        &self,
        mem: &mut dyn GuestMemory,
        surf_handle: u32,
        p_desc: GuestAddr,
    ) -> Result<HResult, String> {
        if p_desc == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }
        let Some(surf) = self.surfaces.get(&surf_handle) else {
            return Ok(D3DERR_INVALIDCALL);
        };
        let desc = D3dSurfaceDesc {
            format: surf.format,
            resource_type: D3DRTYPE_SURFACE,
            usage: surf.usage,
            pool: surf.pool,
            size: surf.data.len() as u32,
            multi_sample_type: surf.multi_sample_type,
            width: surf.width,
            height: surf.height,
        };
        desc.write_to(mem, p_desc)?;
        Ok(D3D_OK)
    }

    fn lock_surface(
        &mut self,
        mem: &mut dyn GuestMemory,
        surf_handle: u32,
        p_locked_rect: GuestAddr,
        p_rect: GuestAddr,
        flags: u32,
    ) -> Result<HResult, String> {
        if p_locked_rect == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }
        let Some(surf) = self.surfaces.get_mut(&surf_handle) else {
            return Ok(D3DERR_INVALIDCALL);
        };
        if surf.lock_flags.is_some() {
            return Ok(D3DERR_INVALIDCALL);
        }
        let (staging_va, fresh_staging) = match surf.staging_guest_va {
            Some(va) => (va, false),
            None => {
                let alloc_len = surf.data.len().max(4);
                if alloc_len >= WRITE_WATCH_MIN_BYTES {
                    let (va, alloc_size) = mem.alloc_page_aligned(alloc_len)?;
                    surf.staging_guest_va = Some(va);
                    surf.staging_page_aligned = true;
                    surf.staging_alloc_size = alloc_size;
                    (va, true)
                } else {
                    let va = mem.alloc(alloc_len, true)?;
                    surf.staging_guest_va = Some(va);
                    surf.staging_page_aligned = false;
                    surf.staging_alloc_size = alloc_len;
                    (va, true)
                }
            }
        };
        let discard_full = (flags & D3DLOCK_DISCARD) != 0 && p_rect == 0;
        let needs_host_sync =
            (fresh_staging || !surf.staging_synced) && !discard_full && !surf.data.is_empty();
        if (needs_host_sync || discard_full) && surf.write_watched {
            mem.remove_write_watch(staging_va, surf.staging_alloc_size);
            surf.write_watched = false;
        }
        if needs_host_sync {
            mem.write_bytes(staging_va, &surf.data)?;
        }
        surf.staging_synced = true;

        let pitch = surface_row_pitch(surf.format, surf.width);
        let mut bit_offset = 0u32;
        if p_rect != 0 {
            let rect = D3dRect::read_from(mem, p_rect)?;
            let left = (rect.left.max(0) as u32).min(surf.width);
            let top = (rect.top.max(0) as u32).min(surf.height);
            let right = (rect.right.max(0) as u32).min(surf.width);
            let bottom = (rect.bottom.max(0) as u32).min(surf.height);
            let w = right.saturating_sub(left);
            let h = bottom.saturating_sub(top);
            surf.locked_rect = if w > 0 && h > 0 {
                Some((left, top, w, h))
            } else {
                None
            };
            if let Some(block_bytes) = dxt_block_bytes(surf.format) {
                bit_offset = (top / 4) * pitch + (left / 4) * block_bytes;
            } else {
                let bpp = format_bytes_per_pixel(surf.format).unwrap_or(4);
                bit_offset = top * pitch + left * bpp;
            }
        } else {
            surf.locked_rect = None;
        }

        surf.lock_flags = Some(flags);
        D3dLockedRect {
            pitch: pitch as i32,
            p_bits: staging_va + bit_offset,
        }
        .write_to(mem, p_locked_rect)?;
        Ok(D3D_OK)
    }

    fn unlock_surface(
        &mut self,
        mem: &mut dyn GuestMemory,
        surf_handle: u32,
    ) -> Result<HResult, String> {
        let start = unlock_perf_enabled().then(std::time::Instant::now);
        let (hr, log_info) = self.unlock_surface_inner(mem, surf_handle)?;
        if let (Some(t0), Some(info)) = (start, log_info) {
            const SLOW_UNLOCK_SURFACE_MS: f64 = 50.0;
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            if ms >= SLOW_UNLOCK_SURFACE_MS {
                eprintln!(
                    "[toluca-perf-slow] unlock_surface {}x{} fmt=0x{:x} skip={} ms={:.1}",
                    info.w, info.h, info.fmt, info.skipped, ms
                );
            }
        }
        Ok(hr)
    }

    fn unlock_surface_inner(
        &mut self,
        mem: &mut dyn GuestMemory,
        surf_handle: u32,
    ) -> Result<(HResult, Option<UnlockLogInfo>), String> {
        let max_tex_size = self.backend.max_texture_size();
        let (
            flags,
            locked_rect,
            dev_h,
            fmt,
            w,
            h,
            staging_va,
            alloc_size,
            write_watched,
            data_len,
            uploaded_once,
            last_pal_gen,
        ) = {
            let Some(surf) = self.surfaces.get_mut(&surf_handle) else {
                return Ok((D3DERR_INVALIDCALL, None));
            };
            let Some(flags) = surf.lock_flags.take() else {
                return Ok((D3DERR_INVALIDCALL, None));
            };
            let locked_rect = surf.locked_rect.take();
            (
                flags,
                locked_rect,
                surf.device_handle,
                surf.format,
                surf.width,
                surf.height,
                surf.staging_guest_va,
                surf.staging_alloc_size,
                surf.write_watched,
                surf.data.len(),
                surf.uploaded_once,
                surf.last_palette_gen,
            )
        };
        if (flags & D3DLOCK_READONLY) != 0 {
            return Ok((D3D_OK, None));
        }

        let cur_pal_gen = self.surface_palette_gen(dev_h);
        let palette_same = fmt != D3DFMT_P8 || cur_pal_gen == last_pal_gen;
        let sub_rect = locked_rect.filter(|&(rx, ry, rw, rh)| {
            dxt_block_bytes(fmt).is_none()
                && w <= max_tex_size
                && h <= max_tex_size
                && palette_same
                && (rx > 0 || ry > 0 || rw < w || rh < h)
        });

        if let Some((rx, ry, rw, rh)) = sub_rect {
            if let Some(va) = staging_va {
                if write_watched && mem.is_write_watch_dirty(va, alloc_size) == Some(true) {
                    if let Some(surf) = self.surfaces.get_mut(&surf_handle) {
                        surf.write_watched = false;
                    }
                }
            }
            let pitch = surface_row_pitch(fmt, w);
            let parent_tex = {
                let surf = self.surfaces.get_mut(&surf_handle).unwrap();
                if let Some(va) = staging_va {
                    if !surf.data.is_empty() {
                        let row_start = (ry as usize) * (pitch as usize);
                        let row_end = ((ry + rh) as usize) * (pitch as usize);
                        let clamped_end = row_end.min(surf.data.len());
                        if row_start < clamped_end {
                            mem.read_into(
                                va + row_start as u32,
                                &mut surf.data[row_start..clamped_end],
                            )?;
                        }
                    }
                }
                surf.parent_texture
            };
            if let Some((tex_h, level)) = parent_tex {
                let palette = self.surface_palette(dev_h, fmt);
                let scratch_buf = std::mem::take(&mut self.scratch_rgba);
                let decoded = {
                    let surf = self.surfaces.get(&surf_handle).unwrap();
                    convert_surface_rect_to_rgba8_into(
                        fmt,
                        pitch,
                        &surf.data,
                        rx,
                        ry,
                        rw,
                        rh,
                        palette.as_ref(),
                        scratch_buf,
                    )
                };
                let uploaded = if let Ok(decoded) = decoded {
                    let ok = self
                        .backend
                        .on_texture_sub_upload(tex_h, level, rx, ry, &decoded);
                    self.scratch_rgba = decoded.pixels;
                    ok
                } else {
                    false
                };
                if uploaded {
                    if let Some(surf) = self.surfaces.get_mut(&surf_handle) {
                        surf.uploaded_once = true;
                        surf.last_palette_gen = cur_pal_gen;
                    }
                    self.maybe_arm_surface_write_watch(mem, surf_handle);
                    return Ok((D3D_OK, Some(UnlockLogInfo { w, h, fmt, skipped: false })));
                }
            }
            self.upload_surface_texture(surf_handle);
            self.maybe_arm_surface_write_watch(mem, surf_handle);
        } else {
            if let Some(va) = staging_va {
                if write_watched {
                    match mem.is_write_watch_dirty(va, alloc_size) {
                        Some(false) if uploaded_once && palette_same => {
                            if let Some(surf) = self.surfaces.get_mut(&surf_handle) {
                                surf.consecutive_dirty_locks = 0;
                            }
                            self.backend.on_texture_upload_skipped();
                            return Ok((D3D_OK, Some(UnlockLogInfo { w, h, fmt, skipped: true })));
                        }
                        Some(true) => {
                            let release_slot =
                                if let Some(surf) = self.surfaces.get_mut(&surf_handle) {
                                    surf.write_watched = false;
                                    surf.consecutive_dirty_locks =
                                        surf.consecutive_dirty_locks.saturating_add(1);
                                    surf.consecutive_dirty_locks >= WRITE_WATCH_DIRTY_STREAK_LIMIT
                                } else {
                                    false
                                };
                            if release_slot {
                                mem.remove_write_watch(va, alloc_size);
                            }
                        }
                        Some(false) | None => {}
                    }
                }
                if data_len > 0 {
                    if let Some(guest_bytes) = mem.bytes_slice(va, data_len)? {
                        if uploaded_once && palette_same {
                            let is_unchanged = {
                                let surf = self.surfaces.get(&surf_handle).unwrap();
                                guest_bytes == surf.data.as_slice()
                            };
                            if is_unchanged {
                                if let Some(surf) = self.surfaces.get_mut(&surf_handle) {
                                    surf.consecutive_dirty_locks = 0;
                                }
                                self.maybe_arm_surface_write_watch(mem, surf_handle);
                                self.backend.on_texture_upload_skipped();
                                return Ok((
                                    D3D_OK,
                                    Some(UnlockLogInfo { w, h, fmt, skipped: true }),
                                ));
                            }
                        }
                        let surf = self.surfaces.get_mut(&surf_handle).unwrap();
                        surf.data.copy_from_slice(guest_bytes);
                    } else {
                        let mut tex_buf = std::mem::take(&mut self.scratch_tex);
                        tex_buf.resize(data_len, 0);
                        if let Err(err) = mem.read_into(va, &mut tex_buf) {
                            self.scratch_tex = tex_buf;
                            return Err(err);
                        }
                        if uploaded_once && palette_same {
                            let is_unchanged = {
                                let surf = self.surfaces.get(&surf_handle).unwrap();
                                tex_buf == surf.data
                            };
                            if is_unchanged {
                                self.scratch_tex = tex_buf;
                                if let Some(surf) = self.surfaces.get_mut(&surf_handle) {
                                    surf.consecutive_dirty_locks = 0;
                                }
                                self.maybe_arm_surface_write_watch(mem, surf_handle);
                                self.backend.on_texture_upload_skipped();
                                return Ok((
                                    D3D_OK,
                                    Some(UnlockLogInfo { w, h, fmt, skipped: true }),
                                ));
                            }
                        }
                        let surf = self.surfaces.get_mut(&surf_handle).unwrap();
                        std::mem::swap(&mut surf.data, &mut tex_buf);
                        self.scratch_tex = tex_buf;
                    }
                }
            }
            self.upload_surface_texture(surf_handle);
            self.maybe_arm_surface_write_watch(mem, surf_handle);
        }
        Ok((D3D_OK, Some(UnlockLogInfo { w, h, fmt, skipped: false })))
    }

    fn dispatch_vertex_buffer(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        method_idx: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        match method_idx {
            0 => {
                let this_va = args[0];
                let ppv = args[2];
                self.query_interface_common(mem, this_va, ppv, |ctx| {
                    let vb = ctx.vertex_buffers.get_mut(&handle)?;
                    vb.refcount += 1;
                    Some(vb.refcount)
                })
            }
            1 => {
                let Some(vb) = self.vertex_buffers.get_mut(&handle) else {
                    return Ok(0);
                };
                vb.refcount += 1;
                Ok(vb.refcount)
            }
            2 => {
                let Some(vb) = self.vertex_buffers.get_mut(&handle) else {
                    return Ok(0);
                };
                vb.refcount = vb.refcount.saturating_sub(1);
                let rem = vb.refcount;
                if rem == 0 {
                    if let Some(removed) = self.vertex_buffers.remove(&handle) {
                        self.backend.on_vb_destroy(handle);
                        self.com_objects.free_object(mem, removed.guest_obj)?;
                        if let Some(va) = removed.staging_guest_va {
                            mem.free(va)?;
                        }
                    }
                }
                Ok(rem)
            }
            3 => {
                let pp_dev = args[1];
                let Some(dev_h) = self.vertex_buffers.get(&handle).map(|v| v.device_handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                self.return_device_ptr(mem, dev_h, pp_dev)
            }
            4 | 6 => Ok(D3D_OK),
            5 => Ok(E_FAIL),
            7 => {
                let prio = args[1];
                let Some(vb) = self.vertex_buffers.get_mut(&handle) else {
                    return Ok(0);
                };
                let prev = vb.priority;
                vb.priority = prio;
                Ok(prev)
            }
            8 => Ok(self.vertex_buffers.get(&handle).map_or(0, |v| v.priority)),
            9 => Ok(0),
            10 => Ok(D3DRTYPE_VERTEXBUFFER),
            11 => self.lock_vertex_buffer(mem, handle, args[1], args[2], args[3], args[4]),
            12 => self.unlock_vertex_buffer(mem, handle),
            13 => {
                let p_desc = args[1];
                if p_desc == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(vb) = self.vertex_buffers.get(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                D3dVertexBufferDesc {
                    format: D3DFMT_VERTEXDATA,
                    resource_type: D3DRTYPE_VERTEXBUFFER,
                    usage: vb.usage,
                    pool: vb.pool,
                    size: vb.length,
                    fvf: vb.fvf,
                }
                .write_to(mem, p_desc)?;
                Ok(D3D_OK)
            }
            _ => Ok(D3DERR_INVALIDCALL),
        }
    }

    fn lock_vertex_buffer(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        offset: u32,
        size: u32,
        ppb_data: GuestAddr,
        flags: u32,
    ) -> Result<HResult, String> {
        if ppb_data == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }
        let Some(vb) = self.vertex_buffers.get_mut(&handle) else {
            return Ok(D3DERR_INVALIDCALL);
        };
        if vb.lock_range.is_some() || offset > vb.length {
            return Ok(D3DERR_INVALIDCALL);
        }
        let lock_len = if size == 0 {
            vb.length - offset
        } else {
            size
        };
        if offset.saturating_add(lock_len) > vb.length {
            return Ok(D3DERR_INVALIDCALL);
        }

        let staging_va = sync_buffer_staging_on_lock(
            mem,
            &mut vb.staging_guest_va,
            &vb.data,
            offset,
            lock_len,
            flags,
        )?;
        vb.lock_range = Some((offset, lock_len, flags));
        mem.write_u32(ppb_data, staging_va + offset)?;
        Ok(D3D_OK)
    }

    fn unlock_vertex_buffer(
        &mut self,
        mem: &dyn GuestMemory,
        handle: u32,
    ) -> Result<HResult, String> {
        let Some(vb) = self.vertex_buffers.get_mut(&handle) else {
            return Ok(D3DERR_INVALIDCALL);
        };
        let Some((off, len, flags)) = vb.lock_range.take() else {
            return Ok(D3DERR_INVALIDCALL);
        };
        if (flags & D3DLOCK_READONLY) == 0 {
            let start = off as usize;
            let end = start + len as usize;
            if let Some(staging_va) = vb.staging_guest_va {
                mem.read_into(staging_va + off, &mut vb.data[start..end])?;
            }
            let is_dynamic = (vb.usage & D3DUSAGE_DYNAMIC) != 0;
            if is_dynamic && (flags & D3DLOCK_DISCARD) != 0 {
                if (end - start) < vb.data.len() {
                    self.backend.on_vb_discard_sub_upload(
                        handle,
                        &vb.data,
                        start,
                        &vb.data[start..end],
                        vb.fvf,
                        vb.usage,
                    );
                } else {
                    self.backend.on_vb_upload(handle, &vb.data, vb.fvf, vb.usage);
                }
            } else {
                let unsynchronized = is_dynamic && (flags & D3DLOCK_NOOVERWRITE) != 0;
                self.backend.on_vb_sub_upload(
                    handle,
                    &vb.data,
                    start,
                    &vb.data[start..end],
                    vb.fvf,
                    vb.usage,
                    unsynchronized,
                );
            }
        }
        Ok(D3D_OK)
    }

    fn dispatch_index_buffer(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        method_idx: u32,
        args: &[u32],
    ) -> Result<HResult, String> {
        match method_idx {
            0 => {
                let this_va = args[0];
                let ppv = args[2];
                self.query_interface_common(mem, this_va, ppv, |ctx| {
                    let ib = ctx.index_buffers.get_mut(&handle)?;
                    ib.refcount += 1;
                    Some(ib.refcount)
                })
            }
            1 => {
                let Some(ib) = self.index_buffers.get_mut(&handle) else {
                    return Ok(0);
                };
                ib.refcount += 1;
                Ok(ib.refcount)
            }
            2 => {
                let Some(ib) = self.index_buffers.get_mut(&handle) else {
                    return Ok(0);
                };
                ib.refcount = ib.refcount.saturating_sub(1);
                let rem = ib.refcount;
                if rem == 0 {
                    if let Some(removed) = self.index_buffers.remove(&handle) {
                        self.backend.on_ib_destroy(handle);
                        self.com_objects.free_object(mem, removed.guest_obj)?;
                        if let Some(va) = removed.staging_guest_va {
                            mem.free(va)?;
                        }
                    }
                }
                Ok(rem)
            }
            3 => {
                let pp_dev = args[1];
                let Some(dev_h) = self.index_buffers.get(&handle).map(|i| i.device_handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                self.return_device_ptr(mem, dev_h, pp_dev)
            }
            4 | 6 => Ok(D3D_OK),
            5 => Ok(E_FAIL),
            7 => {
                let prio = args[1];
                let Some(ib) = self.index_buffers.get_mut(&handle) else {
                    return Ok(0);
                };
                let prev = ib.priority;
                ib.priority = prio;
                Ok(prev)
            }
            8 => Ok(self.index_buffers.get(&handle).map_or(0, |i| i.priority)),
            9 => Ok(0),
            10 => Ok(D3DRTYPE_INDEXBUFFER),
            11 => self.lock_index_buffer(mem, handle, args[1], args[2], args[3], args[4]),
            12 => self.unlock_index_buffer(mem, handle),
            13 => {
                let p_desc = args[1];
                if p_desc == 0 {
                    return Ok(D3DERR_INVALIDCALL);
                }
                let Some(ib) = self.index_buffers.get(&handle) else {
                    return Ok(D3DERR_INVALIDCALL);
                };
                D3dIndexBufferDesc {
                    format: ib.format,
                    resource_type: D3DRTYPE_INDEXBUFFER,
                    usage: ib.usage,
                    pool: ib.pool,
                    size: ib.length,
                }
                .write_to(mem, p_desc)?;
                Ok(D3D_OK)
            }
            _ => Ok(D3DERR_INVALIDCALL),
        }
    }

    fn lock_index_buffer(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
        offset: u32,
        size: u32,
        ppb_data: GuestAddr,
        flags: u32,
    ) -> Result<HResult, String> {
        if ppb_data == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }
        let Some(ib) = self.index_buffers.get_mut(&handle) else {
            return Ok(D3DERR_INVALIDCALL);
        };
        if ib.lock_range.is_some() || offset > ib.length {
            return Ok(D3DERR_INVALIDCALL);
        }
        let lock_len = if size == 0 {
            ib.length - offset
        } else {
            size
        };
        if offset.saturating_add(lock_len) > ib.length {
            return Ok(D3DERR_INVALIDCALL);
        }

        let staging_va = sync_buffer_staging_on_lock(
            mem,
            &mut ib.staging_guest_va,
            &ib.data,
            offset,
            lock_len,
            flags,
        )?;
        ib.lock_range = Some((offset, lock_len, flags));
        mem.write_u32(ppb_data, staging_va + offset)?;
        Ok(D3D_OK)
    }

    fn unlock_index_buffer(&mut self, mem: &dyn GuestMemory, handle: u32) -> Result<HResult, String> {
        let Some(ib) = self.index_buffers.get_mut(&handle) else {
            return Ok(D3DERR_INVALIDCALL);
        };
        let Some((off, len, flags)) = ib.lock_range.take() else {
            return Ok(D3DERR_INVALIDCALL);
        };
        if (flags & D3DLOCK_READONLY) == 0 {
            let start = off as usize;
            let end = start + len as usize;
            if let Some(staging_va) = ib.staging_guest_va {
                mem.read_into(staging_va + off, &mut ib.data[start..end])?;
            }
            let is_dynamic = (ib.usage & D3DUSAGE_DYNAMIC) != 0;
            if is_dynamic && (flags & D3DLOCK_DISCARD) != 0 {
                self.backend
                    .on_ib_upload(handle, &ib.data, ib.format, ib.usage);
            } else {
                let unsynchronized = is_dynamic && (flags & D3DLOCK_NOOVERWRITE) != 0;
                self.backend.on_ib_sub_upload(
                    handle,
                    &ib.data,
                    start,
                    &ib.data[start..end],
                    ib.format,
                    ib.usage,
                    unsynchronized,
                );
            }
        }
        Ok(D3D_OK)
    }

    fn return_device_ptr(
        &mut self,
        mem: &mut dyn GuestMemory,
        dev_handle: u32,
        pp_dev: GuestAddr,
    ) -> Result<HResult, String> {
        if pp_dev == 0 {
            return Ok(D3DERR_INVALIDCALL);
        }
        let Some(dev) = self.devices.get_mut(&dev_handle) else {
            return Ok(D3DERR_INVALIDCALL);
        };
        dev.refcount += 1;
        mem.write_u32(pp_dev, dev.guest_obj)?;
        Ok(D3D_OK)
    }
}

struct UnlockLogInfo {
    w: u32,
    h: u32,
    fmt: u32,
    skipped: bool,
}

fn unlock_perf_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("TOLUCA_PERF").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

fn sync_buffer_staging_on_lock(
    mem: &mut dyn GuestMemory,
    staging_slot: &mut Option<GuestAddr>,
    data: &[u8],
    offset: u32,
    lock_len: u32,
    flags: u32,
) -> Result<GuestAddr, String> {
    let mut first_alloc = false;
    let staging_va = match *staging_slot {
        Some(va) => va,
        None => {
            first_alloc = true;
            let va = mem.alloc(data.len(), true)?;
            *staging_slot = Some(va);
            va
        }
    };
    if (flags & D3DLOCK_DISCARD) == 0 {
        if first_alloc {
            mem.write_bytes(staging_va, data)?;
        } else {
            let start = offset as usize;
            let end = start.saturating_add(lock_len as usize).min(data.len());
            if start < end {
                mem.write_bytes(staging_va + offset, &data[start..end])?;
            }
        }
    }
    Ok(staging_va)
}

fn standard_display_modes() -> Vec<D3dDisplayMode> {
    let resolutions = [(640, 480), (800, 600), (1024, 768), (1280, 720)];
    let formats = [D3DFMT_X8R8G8B8, D3DFMT_A8R8G8B8, D3DFMT_R5G6B5];
    let mut modes = Vec::with_capacity(resolutions.len() * formats.len());
    for &(width, height) in &resolutions {
        for &format in &formats {
            modes.push(D3dDisplayMode {
                width,
                height,
                refresh_rate: 60,
                format,
            });
        }
    }
    modes
}

fn is_supported_display_format(fmt: u32) -> bool {
    matches!(
        fmt,
        D3DFMT_X8R8G8B8 | D3DFMT_A8R8G8B8 | D3DFMT_R5G6B5 | D3DFMT_X1R5G5B5
    )
}

fn is_supported_backbuffer_format(fmt: u32) -> bool {
    matches!(
        fmt,
        D3DFMT_X8R8G8B8 | D3DFMT_A8R8G8B8 | D3DFMT_R5G6B5 | D3DFMT_X1R5G5B5 | D3DFMT_A1R5G5B5
    )
}

fn is_supported_depth_format(fmt: u32) -> bool {
    matches!(
        fmt,
        D3DFMT_D16_LOCKABLE | D3DFMT_D32 | D3DFMT_D15S1 | D3DFMT_D24S8 | D3DFMT_D24X8 | D3DFMT_D16
    )
}

fn is_supported_texture_format(fmt: u32) -> bool {
    matches!(
        fmt,
        D3DFMT_R8G8B8
            | D3DFMT_A8R8G8B8
            | D3DFMT_X8R8G8B8
            | D3DFMT_R5G6B5
            | D3DFMT_X1R5G5B5
            | D3DFMT_A1R5G5B5
            | D3DFMT_A4R4G4B4
            | D3DFMT_A8
            | D3DFMT_P8
            | D3DFMT_L8
            | D3DFMT_A8L8
            | D3DFMT_V8U8
            | D3DFMT_DXT1
            | D3DFMT_DXT2
            | D3DFMT_DXT3
            | D3DFMT_DXT4
            | D3DFMT_DXT5
    )
}

fn calc_mip_levels(width: u32, height: u32, requested: u32) -> u32 {
    let max_dim = width.max(height).max(1);
    let full_chain = (u32::BITS - max_dim.leading_zeros()).max(1);
    if requested == 0 {
        full_chain
    } else {
        requested.min(full_chain)
    }
}

fn read_terminated_tokens(
    mem: &dyn GuestMemory,
    va: GuestAddr,
    sentinel: u32,
) -> Result<Vec<u32>, String> {
    let mut tokens = Vec::new();
    for idx in 0..MAX_SHADER_TOKENS {
        let tok = mem.read_u32(va + (idx as u32) * 4)?;
        tokens.push(tok);
        if tok == sentinel {
            break;
        }
    }
    Ok(tokens)
}

fn write_token_buffer(
    mem: &mut dyn GuestMemory,
    tokens: &[u32],
    p_data: GuestAddr,
    p_size: GuestAddr,
) -> Result<HResult, String> {
    let needed = (tokens.len() * 4) as u32;
    let caller_cap = mem.read_u32(p_size)?;
    mem.write_u32(p_size, needed)?;
    if p_data == 0 {
        return Ok(D3D_OK);
    }
    if caller_cap < needed {
        return Ok(E_INVALIDARG);
    }
    for (i, &tok) in tokens.iter().enumerate() {
        mem.write_u32(p_data + (i as u32) * 4, tok)?;
    }
    Ok(D3D_OK)
}

const MAX_SHADER_CONST_REGS: usize = 96;

fn apply_shader_constants(
    mem: &dyn GuestMemory,
    va: GuestAddr,
    start_reg: u32,
    count: u32,
    state: &mut DeviceState,
    is_pixel: bool,
) -> Result<(), String> {
    let max_regs = if is_pixel {
        state.ps_constants.len()
    } else {
        state.vs_constants.len()
    }
    .min(MAX_SHADER_CONST_REGS);
    let effective = (count as usize).min(max_regs.saturating_sub(start_reg as usize));
    if effective == 0 {
        return Ok(());
    }
    let byte_len = effective * 16;
    let mut stack_regs = [[0.0f32; 4]; MAX_SHADER_CONST_REGS];
    let mut raw_buf = [0u8; MAX_SHADER_CONST_REGS * 16];
    let bytes: &[u8] = if let Some(slice) = mem.bytes_slice(va, byte_len)? {
        slice
    } else {
        mem.read_into(va, &mut raw_buf[..byte_len])?;
        &raw_buf[..byte_len]
    };
    for (dst_reg, chunk) in stack_regs[..effective]
        .iter_mut()
        .zip(bytes.chunks_exact(16))
    {
        for (i, dst) in dst_reg.iter_mut().enumerate() {
            let bits = u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
            *dst = f32::from_bits(bits);
        }
    }
    if is_pixel {
        state.set_ps_constants(start_reg, &stack_regs[..effective]);
    } else {
        state.set_vs_constants(start_reg, &stack_regs[..effective]);
    }
    Ok(())
}

fn active_shaders_for_device(dev: &DeviceObj) -> ActiveShaders<'_> {
    let vs = dev.vertex_shaders.get(&dev.state.current_vertex_shader);
    let ps = dev.pixel_shaders.get(&dev.state.current_pixel_shader);
    ActiveShaders {
        vs_decl_tokens: vs.map(|v| v.declaration.as_slice()),
        vs_parsed_decl: vs.map(|v| v.parsed_decl.as_slice()),
        vs_tokens: vs.and_then(|v| (!v.function.is_empty()).then_some(v.function.as_slice())),
        ps_tokens: ps.map(|p| p.function.as_slice()),
    }
}

pub fn vertex_count_for_primitives(prim_type: u32, prim_count: u32) -> u32 {
    if prim_count == 0 {
        return 0;
    }
    match prim_type {
        D3DPT_POINTLIST => prim_count,
        D3DPT_LINELIST => prim_count.saturating_mul(2),
        D3DPT_LINESTRIP => prim_count.saturating_add(1),
        D3DPT_TRIANGLELIST => prim_count.saturating_mul(3),
        D3DPT_TRIANGLESTRIP | D3DPT_TRIANGLEFAN => prim_count.saturating_add(2),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use crate::backend::RenderBackend;
    use crate::types::{D3D_SDK_VERSION, D3DPOOL_MANAGED};

    struct LocalMem {
        next_va: GuestAddr,
        allocs: HashMap<GuestAddr, Vec<u8>>,
        watches: HashMap<GuestAddr, bool>,
        arm_calls: u32,
        remove_calls: u32,
    }

    impl LocalMem {
        fn new() -> Self {
            Self {
                next_va: 0x1000_0000,
                allocs: HashMap::new(),
                watches: HashMap::new(),
                arm_calls: 0,
                remove_calls: 0,
            }
        }

        fn locate(&self, va: GuestAddr, len: usize) -> Result<(&[u8], usize, GuestAddr), String> {
            for (&base, buf) in &self.allocs {
                let end = base as usize + buf.len();
                if (va as usize) >= (base as usize) && (va as usize) + len <= end {
                    return Ok((buf.as_slice(), (va - base) as usize, base));
                }
            }
            Err(format!("unmapped read at 0x{va:08x} len={len}"))
        }

        fn locate_mut(
            &mut self,
            va: GuestAddr,
            len: usize,
        ) -> Result<(&mut [u8], usize, GuestAddr), String> {
            for (&base, buf) in &mut self.allocs {
                let end = base as usize + buf.len();
                if (va as usize) >= (base as usize) && (va as usize) + len <= end {
                    return Ok((buf.as_mut_slice(), (va - base) as usize, base));
                }
            }
            Err(format!("unmapped write at 0x{va:08x} len={len}"))
        }
    }

    impl GuestMemory for LocalMem {
        fn bytes_slice(&self, va: GuestAddr, len: usize) -> Result<Option<&[u8]>, String> {
            let (buf, off, _) = self.locate(va, len)?;
            Ok(Some(&buf[off..off + len]))
        }
        fn read_bytes(&self, va: GuestAddr, len: usize) -> Result<Vec<u8>, String> {
            let (buf, off, _) = self.locate(va, len)?;
            Ok(buf[off..off + len].to_vec())
        }
        fn write_bytes(&mut self, va: GuestAddr, data: &[u8]) -> Result<(), String> {
            let (buf, off, base) = self.locate_mut(va, data.len())?;
            buf[off..off + data.len()].copy_from_slice(data);
            if let Some(dirty) = self.watches.get_mut(&base) {
                *dirty = true;
            }
            Ok(())
        }
        fn zero_range(&mut self, va: GuestAddr, len: usize) -> Result<(), String> {
            let (buf, off, base) = self.locate_mut(va, len)?;
            buf[off..off + len].fill(0);
            if let Some(dirty) = self.watches.get_mut(&base) {
                *dirty = true;
            }
            Ok(())
        }
        fn alloc(&mut self, size: usize, _zeroed: bool) -> Result<GuestAddr, String> {
            let va = self.next_va;
            self.next_va += ((size as u32).max(16) + 0xFFF) & !0xFFF;
            self.allocs.insert(va, vec![0u8; size]);
            Ok(va)
        }
        fn free(&mut self, va: GuestAddr) -> Result<(), String> {
            self.allocs.remove(&va);
            self.watches.remove(&va);
            Ok(())
        }
        fn arm_write_watch(&mut self, va: GuestAddr, _alloc_size: usize) -> bool {
            self.arm_calls += 1;
            self.watches.insert(va, false);
            true
        }
        fn is_write_watch_dirty(&self, va: GuestAddr, _alloc_size: usize) -> Option<bool> {
            self.watches.get(&va).copied()
        }
        fn remove_write_watch(&mut self, va: GuestAddr, _alloc_size: usize) {
            self.remove_calls += 1;
            self.watches.remove(&va);
        }
    }

    #[derive(Default)]
    struct UploadCounterBackend {
        uploads: u32,
        skips: u32,
        vb_full_uploads: u32,
        vb_discard_subs: Vec<(usize, usize, usize)>,
    }

    impl RenderBackend for UploadCounterBackend {
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
            self.uploads += 1;
        }
        fn on_texture_upload_skipped(&mut self) {
            self.skips += 1;
        }
        fn on_texture_destroy(&mut self, _: u32) {}
        fn on_vb_upload(&mut self, _: u32, _: &[u8], _: u32, _: u32) {
            self.vb_full_uploads += 1;
        }
        fn on_vb_discard_sub_upload(
            &mut self,
            _: u32,
            full_data: &[u8],
            offset: usize,
            sub_data: &[u8],
            _: u32,
            _: u32,
        ) {
            self.vb_discard_subs
                .push((full_data.len(), offset, sub_data.len()));
        }
        fn on_vb_destroy(&mut self, _: u32) {}
        fn on_ib_upload(&mut self, _: u32, _: &[u8], _: u32, _: u32) {}
        fn on_ib_destroy(&mut self, _: u32) {}
        fn on_draw_primitive(&mut self, _: &DeviceState, _: &ActiveShaders<'_>, _: u32, _: u32, _: u32) {}
        fn on_draw_indexed_primitive(&mut self, _: &DeviceState, _: &ActiveShaders<'_>, _: u32, _: u32, _: u32, _: u32, _: u32) {}
        fn on_present(&mut self) {}
    }

    fn call(
        ctx: &mut D3d8Context,
        mem: &mut LocalMem,
        obj_va: GuestAddr,
        method_idx: u32,
        args: &[u32],
    ) -> HResult {
        let iface = ComInterfaceId::from_u32(mem.read_u32(obj_va + 8).unwrap()).unwrap();
        let mut full_args = Vec::with_capacity(args.len() + 1);
        full_args.push(obj_va);
        full_args.extend_from_slice(args);
        ctx.dispatch_com(mem, iface, method_idx, &full_args).unwrap().ret_val
    }

    #[test]
    fn unchanged_unlock_after_dirty_streak_skips_upload_and_rearms_write_watch() {
        let mut mem = LocalMem::new();
        let mut ctx = D3d8Context::new(Box::new(UploadCounterBackend::default()));
        let pp_va = mem.alloc(D3dPresentParameters::SIZE, true).unwrap();
        let out_ptr = mem.alloc(4, true).unwrap();
        let lr_va = mem.alloc(8, true).unwrap();
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
            call(&mut ctx, &mut mem, d3d8_va, 15, &[0, D3DDEVTYPE_HAL, 0x0002_0000, 0x40, pp_va, out_ptr]),
            D3D_OK
        );
        let dev_va = mem.read_u32(out_ptr).unwrap();
        // 128x128 A8R8G8B8 = 64 KiB (>= WRITE_WATCH_MIN_BYTES) so write-watch arms.
        assert_eq!(
            call(&mut ctx, &mut mem, dev_va, 20, &[128, 128, 1, 0, D3DFMT_A8R8G8B8, D3DPOOL_MANAGED, out_ptr]),
            D3D_OK
        );
        let tex_va = mem.read_u32(out_ptr).unwrap();

        // Initial unlock uploads and arms write-watch.
        assert_eq!(call(&mut ctx, &mut mem, tex_va, 16, &[0, lr_va, 0, 0]), D3D_OK);
        let lr = D3dLockedRect::read_from(&mem, lr_va).unwrap();
        mem.write_u32(lr.p_bits, 0xFF10_2030).unwrap();
        assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);
        let scratch_ptr = ctx.scratch_rgba.as_ptr();
        let scratch_cap = ctx.scratch_rgba.capacity();
        assert_eq!(mem.arm_calls, 1);

        // Three consecutive dirty unlocks disarm write-watch after reaching WRITE_WATCH_DIRTY_STREAK_LIMIT.
        for step in 1..=u32::from(WRITE_WATCH_DIRTY_STREAK_LIMIT) {
            assert_eq!(call(&mut ctx, &mut mem, tex_va, 16, &[0, lr_va, 0, 0]), D3D_OK);
            mem.write_u32(lr.p_bits, 0xFF10_2030 + step).unwrap();
            assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);
            assert_eq!(ctx.scratch_rgba.as_ptr(), scratch_ptr);
            assert_eq!(ctx.scratch_rgba.capacity(), scratch_cap);
        }
        assert_eq!(mem.remove_calls, 1);
        let tex_surf = ctx
            .surfaces
            .values()
            .find(|s| s.parent_texture.is_some())
            .unwrap();
        assert_eq!(tex_surf.consecutive_dirty_locks, WRITE_WATCH_DIRTY_STREAK_LIMIT);
        assert!(!tex_surf.write_watched);

        // Once pixels stabilize, unlock must still diff bytes, skip upload, reset streak, and re-arm write-watch.
        assert_eq!(call(&mut ctx, &mut mem, tex_va, 16, &[0, lr_va, 0, 0]), D3D_OK);
        assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);
        let tex_surf = ctx
            .surfaces
            .values()
            .find(|s| s.parent_texture.is_some())
            .unwrap();
        assert_eq!(tex_surf.consecutive_dirty_locks, 0);
        assert!(tex_surf.write_watched);
        assert_eq!(mem.arm_calls, 4);

        // Subsequent clean unlock skips directly via write-watch without touching bytes.
        assert_eq!(call(&mut ctx, &mut mem, tex_va, 16, &[0, lr_va, 0, 0]), D3D_OK);
        assert_eq!(call(&mut ctx, &mut mem, tex_va, 17, &[0]), D3D_OK);

        let backend = ctx.backend().as_any().downcast_ref::<UploadCounterBackend>().unwrap();
        assert_eq!(backend.uploads, 1 + u32::from(WRITE_WATCH_DIRTY_STREAK_LIMIT));
        assert_eq!(backend.skips, 2);
    }

    #[test]
    fn shader_constants_and_partial_vb_discard_upload_cleanly() {
        let mut mem = LocalMem::new();
        let mut ctx = D3d8Context::new(Box::new(UploadCounterBackend::default()));
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
            call(&mut ctx, &mut mem, d3d8_va, 15, &[0, D3DDEVTYPE_HAL, 0x0002_0000, 0x40, pp_va, out_ptr]),
            D3D_OK
        );
        let dev_va = mem.read_u32(out_ptr).unwrap();

        // SetVertexShaderConstant (79) and SetPixelShaderConstant (91)
        let vs_const_va = mem.alloc(32, true).unwrap();
        let ps_const_va = mem.alloc(16, true).unwrap();
        let vs_vals = [1.25f32, -2.5, 3.0, 4.5, 5.0, 6.0, 7.0, 8.0];
        for (i, &val) in vs_vals.iter().enumerate() {
            mem.write_f32(vs_const_va + (i as u32) * 4, val).unwrap();
        }
        let ps_vals = [0.5f32, 0.25, 0.75, 1.0];
        for (i, &val) in ps_vals.iter().enumerate() {
            mem.write_f32(ps_const_va + (i as u32) * 4, val).unwrap();
        }
        assert_eq!(call(&mut ctx, &mut mem, dev_va, 79, &[2, vs_const_va, 2]), D3D_OK);
        assert_eq!(call(&mut ctx, &mut mem, dev_va, 91, &[1, ps_const_va, 1]), D3D_OK);
        let dev = ctx.devices.values().next().unwrap();
        assert_eq!(dev.state.vs_constants[2], [1.25, -2.5, 3.0, 4.5]);
        assert_eq!(dev.state.vs_constants[3], [5.0, 6.0, 7.0, 8.0]);
        assert_eq!(dev.state.ps_constants[1], [0.5, 0.25, 0.75, 1.0]);

        // Dynamic VB partial DISCARD vs full DISCARD
        assert_eq!(
            call(&mut ctx, &mut mem, dev_va, 23, &[256, D3DUSAGE_DYNAMIC, 0x02, D3DPOOL_MANAGED, out_ptr]),
            D3D_OK
        );
        let vb_va = mem.read_u32(out_ptr).unwrap();
        assert_eq!(
            call(&mut ctx, &mut mem, vb_va, 11, &[32, 64, out_ptr, D3DLOCK_DISCARD]),
            D3D_OK
        );
        assert_eq!(call(&mut ctx, &mut mem, vb_va, 12, &[]), D3D_OK);
        assert_eq!(
            call(&mut ctx, &mut mem, vb_va, 11, &[0, 0, out_ptr, D3DLOCK_DISCARD]),
            D3D_OK
        );
        assert_eq!(call(&mut ctx, &mut mem, vb_va, 12, &[]), D3D_OK);

        let backend = ctx.backend().as_any().downcast_ref::<UploadCounterBackend>().unwrap();
        assert_eq!(backend.vb_discard_subs, vec![(256, 32, 64)]);
        assert_eq!(backend.vb_full_uploads, 1);
    }
}
