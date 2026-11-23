use toluca_core::GuestAddr;

use crate::mem::GuestMemory;

pub type HResult = u32;

pub const D3D_OK: HResult = 0;
pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOINTERFACE: HResult = 0x8000_4002;
pub const E_POINTER: HResult = 0x8000_4003;
pub const E_FAIL: HResult = 0x8000_4005;
pub const E_INVALIDARG: HResult = 0x8007_0057;
pub const E_OUTOFMEMORY: HResult = 0x8007_000E;
pub const D3DERR_OUTOFVIDEOMEMORY: HResult = 0x8876_017C;
pub const D3DERR_NOTFOUND: HResult = 0x8876_0866;
pub const D3DERR_DEVICELOST: HResult = 0x8876_0868;
pub const D3DERR_DEVICENOTRESET: HResult = 0x8876_0869;
pub const D3DERR_NOTAVAILABLE: HResult = 0x8876_086A;
pub const D3DERR_INVALIDCALL: HResult = 0x8876_086C;

pub const D3D_SDK_VERSION: u32 = 220;
pub const D3DADAPTER_DEFAULT: u32 = 0;
pub const D3DDEVTYPE_HAL: u32 = 1;
pub const D3DDEVTYPE_REF: u32 = 2;
pub const D3DDEVTYPE_SW: u32 = 3;

pub const D3DCREATE_FPU_PRESERVE: u32 = 0x0000_0002;
pub const D3DCREATE_MULTITHREADED: u32 = 0x0000_0004;
pub const D3DCREATE_PUREDEVICE: u32 = 0x0000_0010;
pub const D3DCREATE_SOFTWARE_VERTEXPROCESSING: u32 = 0x0000_0020;
pub const D3DCREATE_HARDWARE_VERTEXPROCESSING: u32 = 0x0000_0040;
pub const D3DCREATE_MIXED_VERTEXPROCESSING: u32 = 0x0000_0080;

pub const D3DFMT_UNKNOWN: u32 = 0;
pub const D3DFMT_R8G8B8: u32 = 20;
pub const D3DFMT_A8R8G8B8: u32 = 21;
pub const D3DFMT_X8R8G8B8: u32 = 22;
pub const D3DFMT_R5G6B5: u32 = 23;
pub const D3DFMT_X1R5G5B5: u32 = 24;
pub const D3DFMT_A1R5G5B5: u32 = 25;
pub const D3DFMT_A4R4G4B4: u32 = 26;
pub const D3DFMT_R3G3B2: u32 = 27;
pub const D3DFMT_A8: u32 = 28;
pub const D3DFMT_A8R3G3B2: u32 = 29;
pub const D3DFMT_X4R4G4B4: u32 = 30;
pub const D3DFMT_A2B10G10R10: u32 = 31;
pub const D3DFMT_G16R16: u32 = 34;
pub const D3DFMT_A8P8: u32 = 40;
pub const D3DFMT_P8: u32 = 41;
pub const D3DFMT_L8: u32 = 50;
pub const D3DFMT_A8L8: u32 = 51;
pub const D3DFMT_A4L4: u32 = 52;
pub const D3DFMT_V8U8: u32 = 60;
pub const D3DFMT_L6V5U5: u32 = 61;
pub const D3DFMT_X8L8V8U8: u32 = 62;
pub const D3DFMT_Q8W8V8U8: u32 = 63;
pub const D3DFMT_V16U16: u32 = 64;
pub const D3DFMT_W11V11U10: u32 = 65;
pub const D3DFMT_D16_LOCKABLE: u32 = 70;
pub const D3DFMT_D32: u32 = 71;
pub const D3DFMT_D15S1: u32 = 73;
pub const D3DFMT_D24S8: u32 = 75;
pub const D3DFMT_D24X8: u32 = 77;
pub const D3DFMT_D24X4S4: u32 = 79;
pub const D3DFMT_D16: u32 = 80;
pub const D3DFMT_VERTEXDATA: u32 = 100;
pub const D3DFMT_INDEX16: u32 = 101;
pub const D3DFMT_INDEX32: u32 = 102;
pub const D3DFMT_DXT1: u32 = 0x3154_5844;
pub const D3DFMT_DXT2: u32 = 0x3254_5844;
pub const D3DFMT_DXT3: u32 = 0x3354_5844;
pub const D3DFMT_DXT4: u32 = 0x3454_5844;
pub const D3DFMT_DXT5: u32 = 0x3554_5844;

const DXT_BLOCK_DIM: u32 = 4;
const DXT1_BLOCK_BYTES: u32 = 8;
const DXT2_TO_5_BLOCK_BYTES: u32 = 16;

pub fn is_dxt_format(fmt: u32) -> bool {
    matches!(
        fmt,
        D3DFMT_DXT1 | D3DFMT_DXT2 | D3DFMT_DXT3 | D3DFMT_DXT4 | D3DFMT_DXT5
    )
}

pub fn dxt_block_bytes(fmt: u32) -> Option<u32> {
    match fmt {
        D3DFMT_DXT1 => Some(DXT1_BLOCK_BYTES),
        D3DFMT_DXT2 | D3DFMT_DXT3 | D3DFMT_DXT4 | D3DFMT_DXT5 => Some(DXT2_TO_5_BLOCK_BYTES),
        _ => None,
    }
}

pub fn format_bytes_per_pixel(fmt: u32) -> Option<u32> {
    match fmt {
        D3DFMT_R3G3B2 | D3DFMT_A8 | D3DFMT_P8 | D3DFMT_L8 | D3DFMT_A4L4 => Some(1),
        D3DFMT_R5G6B5
        | D3DFMT_X1R5G5B5
        | D3DFMT_A1R5G5B5
        | D3DFMT_A4R4G4B4
        | D3DFMT_A8R3G3B2
        | D3DFMT_X4R4G4B4
        | D3DFMT_A8P8
        | D3DFMT_A8L8
        | D3DFMT_V8U8
        | D3DFMT_L6V5U5
        | D3DFMT_D16_LOCKABLE
        | D3DFMT_D15S1
        | D3DFMT_D16
        | D3DFMT_INDEX16 => Some(2),
        D3DFMT_R8G8B8 => Some(3),
        D3DFMT_A8R8G8B8
        | D3DFMT_X8R8G8B8
        | D3DFMT_A2B10G10R10
        | D3DFMT_G16R16
        | D3DFMT_X8L8V8U8
        | D3DFMT_Q8W8V8U8
        | D3DFMT_V16U16
        | D3DFMT_W11V11U10
        | D3DFMT_D32
        | D3DFMT_D24S8
        | D3DFMT_D24X8
        | D3DFMT_D24X4S4
        | D3DFMT_INDEX32 => Some(4),
        _ => None,
    }
}

pub fn surface_row_pitch(fmt: u32, width: u32) -> u32 {
    if width == 0 {
        return 0;
    }
    if let Some(block_bytes) = dxt_block_bytes(fmt) {
        let blocks_wide = width.div_ceil(DXT_BLOCK_DIM).max(1);
        return blocks_wide * block_bytes;
    }
    let bpp = format_bytes_per_pixel(fmt).unwrap_or(4);
    width * bpp
}

pub fn surface_byte_size(fmt: u32, width: u32, height: u32) -> usize {
    if width == 0 || height == 0 {
        return 0;
    }
    let pitch = surface_row_pitch(fmt, width) as usize;
    if is_dxt_format(fmt) {
        let blocks_high = height.div_ceil(DXT_BLOCK_DIM).max(1) as usize;
        pitch * blocks_high
    } else {
        pitch * (height as usize)
    }
}

pub const D3DPOOL_DEFAULT: u32 = 0;
pub const D3DPOOL_MANAGED: u32 = 1;
pub const D3DPOOL_SYSTEMMEM: u32 = 2;
pub const D3DPOOL_SCRATCH: u32 = 3;

pub const D3DUSAGE_RENDERTARGET: u32 = 0x0000_0001;
pub const D3DUSAGE_DEPTHSTENCIL: u32 = 0x0000_0002;
pub const D3DUSAGE_WRITEONLY: u32 = 0x0000_0008;
pub const D3DUSAGE_SOFTWAREPROCESSING: u32 = 0x0000_0010;
pub const D3DUSAGE_DONOTCLIP: u32 = 0x0000_0020;
pub const D3DUSAGE_POINTS: u32 = 0x0000_0040;
pub const D3DUSAGE_RTPATCHES: u32 = 0x0000_0080;
pub const D3DUSAGE_NPATCHES: u32 = 0x0000_0100;
pub const D3DUSAGE_DYNAMIC: u32 = 0x0000_0200;

pub const D3DRTYPE_SURFACE: u32 = 1;
pub const D3DRTYPE_VOLUME: u32 = 2;
pub const D3DRTYPE_TEXTURE: u32 = 3;
pub const D3DRTYPE_VOLUMETEXTURE: u32 = 4;
pub const D3DRTYPE_CUBETEXTURE: u32 = 5;
pub const D3DRTYPE_VERTEXBUFFER: u32 = 6;
pub const D3DRTYPE_INDEXBUFFER: u32 = 7;

pub const D3DPT_POINTLIST: u32 = 1;
pub const D3DPT_LINELIST: u32 = 2;
pub const D3DPT_LINESTRIP: u32 = 3;
pub const D3DPT_TRIANGLELIST: u32 = 4;
pub const D3DPT_TRIANGLESTRIP: u32 = 5;
pub const D3DPT_TRIANGLEFAN: u32 = 6;

pub const D3DCLEAR_TARGET: u32 = 0x0000_0001;
pub const D3DCLEAR_ZBUFFER: u32 = 0x0000_0002;
pub const D3DCLEAR_STENCIL: u32 = 0x0000_0004;

pub const D3DLOCK_READONLY: u32 = 0x0000_0010;
pub const D3DLOCK_NOSYSLOCK: u32 = 0x0000_0800;
pub const D3DLOCK_NOOVERWRITE: u32 = 0x0000_1000;
pub const D3DLOCK_DISCARD: u32 = 0x0000_2000;

pub const D3DFVF_RESERVED0: u32 = 0x0001;
pub const D3DFVF_POSITION_MASK: u32 = 0x000E;
pub const D3DFVF_XYZ: u32 = 0x0002;
pub const D3DFVF_XYZRHW: u32 = 0x0004;
pub const D3DFVF_XYZB1: u32 = 0x0006;
pub const D3DFVF_XYZB2: u32 = 0x0008;
pub const D3DFVF_XYZB3: u32 = 0x000A;
pub const D3DFVF_XYZB4: u32 = 0x000C;
pub const D3DFVF_XYZB5: u32 = 0x000E;
pub const D3DFVF_NORMAL: u32 = 0x0010;
pub const D3DFVF_PSIZE: u32 = 0x0020;
pub const D3DFVF_DIFFUSE: u32 = 0x0040;
pub const D3DFVF_SPECULAR: u32 = 0x0080;
pub const D3DFVF_TEXCOUNT_MASK: u32 = 0x0F00;
pub const D3DFVF_TEXCOUNT_SHIFT: u32 = 8;
pub const D3DFVF_TEX0: u32 = 0x0000;
pub const D3DFVF_TEX1: u32 = 0x0100;
pub const D3DFVF_TEX2: u32 = 0x0200;
pub const D3DFVF_TEX3: u32 = 0x0300;
pub const D3DFVF_TEX4: u32 = 0x0400;
pub const D3DFVF_TEX5: u32 = 0x0500;
pub const D3DFVF_TEX6: u32 = 0x0600;
pub const D3DFVF_TEX7: u32 = 0x0700;
pub const D3DFVF_TEX8: u32 = 0x0800;

pub fn fvf_texcoord_floats(fvf: u32, tex_index: u32) -> u32 {
    let shift = tex_index * 2 + 16;
    match (fvf >> shift) & 0x3 {
        0 => 2,
        1 => 3,
        2 => 4,
        3 => 1,
        _ => 2,
    }
}

pub fn fvf_vertex_stride(fvf: u32) -> u32 {
    let mut stride = match fvf & D3DFVF_POSITION_MASK {
        D3DFVF_XYZ => 12,
        D3DFVF_XYZRHW => 16,
        D3DFVF_XYZB1 => 16,
        D3DFVF_XYZB2 => 20,
        D3DFVF_XYZB3 => 24,
        D3DFVF_XYZB4 => 28,
        D3DFVF_XYZB5 => 32,
        _ => 0,
    };

    if (fvf & D3DFVF_NORMAL) != 0 {
        stride += 12;
    }
    if (fvf & D3DFVF_PSIZE) != 0 {
        stride += 4;
    }
    if (fvf & D3DFVF_DIFFUSE) != 0 {
        stride += 4;
    }
    if (fvf & D3DFVF_SPECULAR) != 0 {
        stride += 4;
    }

    let tex_count = ((fvf & D3DFVF_TEXCOUNT_MASK) >> D3DFVF_TEXCOUNT_SHIFT).min(8);
    for idx in 0..tex_count {
        stride += fvf_texcoord_floats(fvf, idx) * 4;
    }
    stride
}

pub const D3DTS_VIEW: u32 = 2;
pub const D3DTS_PROJECTION: u32 = 3;
pub const D3DTS_TEXTURE0: u32 = 16;
pub const D3DTS_TEXTURE1: u32 = 17;
pub const D3DTS_TEXTURE2: u32 = 18;
pub const D3DTS_TEXTURE3: u32 = 19;
pub const D3DTS_TEXTURE4: u32 = 20;
pub const D3DTS_TEXTURE5: u32 = 21;
pub const D3DTS_TEXTURE6: u32 = 22;
pub const D3DTS_TEXTURE7: u32 = 23;
pub const D3DTS_WORLD: u32 = 256;
pub const D3DTS_WORLD1: u32 = 257;
pub const D3DTS_WORLD2: u32 = 258;
pub const D3DTS_WORLD3: u32 = 259;

pub const D3DRS_ZENABLE: u32 = 7;
pub const D3DRS_FILLMODE: u32 = 8;
pub const D3DRS_SHADEMODE: u32 = 9;
pub const D3DRS_LINEPATTERN: u32 = 10;
pub const D3DRS_ZWRITEENABLE: u32 = 14;
pub const D3DRS_ALPHATESTENABLE: u32 = 15;
pub const D3DRS_LASTPIXEL: u32 = 16;
pub const D3DRS_SRCBLEND: u32 = 19;
pub const D3DRS_DESTBLEND: u32 = 20;
pub const D3DRS_CULLMODE: u32 = 22;
pub const D3DRS_ZFUNC: u32 = 23;
pub const D3DRS_ALPHAREF: u32 = 24;
pub const D3DRS_ALPHAFUNC: u32 = 25;
pub const D3DRS_DITHERENABLE: u32 = 26;
pub const D3DRS_ALPHABLENDENABLE: u32 = 27;
pub const D3DRS_FOGENABLE: u32 = 28;
pub const D3DRS_SPECULARENABLE: u32 = 29;
pub const D3DRS_ZVISIBLE: u32 = 30;
pub const D3DRS_FOGCOLOR: u32 = 34;
pub const D3DRS_FOGTABLEMODE: u32 = 35;
pub const D3DRS_FOGSTART: u32 = 36;
pub const D3DRS_FOGEND: u32 = 37;
pub const D3DRS_FOGDENSITY: u32 = 38;
pub const D3DRS_EDGEANTIALIAS: u32 = 40;
pub const D3DRS_ZBIAS: u32 = 47;
pub const D3DRS_RANGEFOGENABLE: u32 = 48;
pub const D3DRS_STENCILENABLE: u32 = 52;
pub const D3DRS_STENCILFAIL: u32 = 53;
pub const D3DRS_STENCILZFAIL: u32 = 54;
pub const D3DRS_STENCILPASS: u32 = 55;
pub const D3DRS_STENCILFUNC: u32 = 56;
pub const D3DRS_STENCILREF: u32 = 57;
pub const D3DRS_STENCILMASK: u32 = 58;
pub const D3DRS_STENCILWRITEMASK: u32 = 59;
pub const D3DRS_TEXTUREFACTOR: u32 = 60;
pub const D3DRS_WRAP0: u32 = 128;
pub const D3DRS_WRAP1: u32 = 129;
pub const D3DRS_WRAP2: u32 = 130;
pub const D3DRS_WRAP3: u32 = 131;
pub const D3DRS_WRAP4: u32 = 132;
pub const D3DRS_WRAP5: u32 = 133;
pub const D3DRS_WRAP6: u32 = 134;
pub const D3DRS_WRAP7: u32 = 135;
pub const D3DRS_CLIPPING: u32 = 136;
pub const D3DRS_LIGHTING: u32 = 137;
pub const D3DRS_AMBIENT: u32 = 139;
pub const D3DRS_FOGVERTEXMODE: u32 = 140;
pub const D3DRS_COLORVERTEX: u32 = 141;
pub const D3DRS_LOCALVIEWER: u32 = 142;
pub const D3DRS_NORMALIZENORMALS: u32 = 143;
pub const D3DRS_DIFFUSEMATERIALSOURCE: u32 = 145;
pub const D3DRS_SPECULARMATERIALSOURCE: u32 = 146;
pub const D3DRS_AMBIENTMATERIALSOURCE: u32 = 147;
pub const D3DRS_EMISSIVEMATERIALSOURCE: u32 = 148;
pub const D3DRS_VERTEXBLEND: u32 = 151;
pub const D3DRS_CLIPPLANEENABLE: u32 = 152;
pub const D3DRS_SOFTWAREVERTEXPROCESSING: u32 = 153;
pub const D3DRS_POINTSIZE: u32 = 154;
pub const D3DRS_POINTSIZE_MIN: u32 = 155;
pub const D3DRS_POINTSPRITEENABLE: u32 = 156;
pub const D3DRS_POINTSCALEENABLE: u32 = 157;
pub const D3DRS_POINTSCALE_A: u32 = 158;
pub const D3DRS_POINTSCALE_B: u32 = 159;
pub const D3DRS_POINTSCALE_C: u32 = 160;
pub const D3DRS_MULTISAMPLEANTIALIAS: u32 = 161;
pub const D3DRS_MULTISAMPLEMASK: u32 = 162;
pub const D3DRS_PATCHEDGESTYLE: u32 = 163;
pub const D3DRS_PATCHSEGMENTS: u32 = 164;
pub const D3DRS_DEBUGMONITORTOKEN: u32 = 165;
pub const D3DRS_POINTSIZE_MAX: u32 = 166;
pub const D3DRS_INDEXEDVERTEXBLENDENABLE: u32 = 167;
pub const D3DRS_COLORWRITEENABLE: u32 = 168;
pub const D3DRS_TWEENFACTOR: u32 = 170;
pub const D3DRS_BLENDOP: u32 = 171;
pub const D3DRS_POSITIONORDER: u32 = 172;
pub const D3DRS_NORMALORDER: u32 = 173;

pub const D3DTSS_COLOROP: u32 = 1;
pub const D3DTSS_COLORARG1: u32 = 2;
pub const D3DTSS_COLORARG2: u32 = 3;
pub const D3DTSS_ALPHAOP: u32 = 4;
pub const D3DTSS_ALPHAARG1: u32 = 5;
pub const D3DTSS_ALPHAARG2: u32 = 6;
pub const D3DTSS_BUMPENVMAT00: u32 = 7;
pub const D3DTSS_BUMPENVMAT01: u32 = 8;
pub const D3DTSS_BUMPENVMAT10: u32 = 9;
pub const D3DTSS_BUMPENVMAT11: u32 = 10;
pub const D3DTSS_TEXCOORDINDEX: u32 = 11;
pub const D3DTSS_ADDRESSU: u32 = 13;
pub const D3DTSS_ADDRESSV: u32 = 14;
pub const D3DTSS_BORDERCOLOR: u32 = 15;
pub const D3DTSS_MAGFILTER: u32 = 16;
pub const D3DTSS_MINFILTER: u32 = 17;
pub const D3DTSS_MIPFILTER: u32 = 18;
pub const D3DTSS_MIPMAPLODBIAS: u32 = 19;
pub const D3DTSS_MAXMIPLEVEL: u32 = 20;
pub const D3DTSS_MAXANISOTROPY: u32 = 21;
pub const D3DTSS_BUMPENVLSCALE: u32 = 22;
pub const D3DTSS_BUMPENVLOFFSET: u32 = 23;
pub const D3DTSS_TEXTURETRANSFORMFLAGS: u32 = 24;
pub const D3DTSS_ADDRESSW: u32 = 25;
pub const D3DTSS_COLORARG0: u32 = 26;
pub const D3DTSS_ALPHAARG0: u32 = 27;
pub const D3DTSS_RESULTARG: u32 = 28;

pub const D3DTOP_DISABLE: u32 = 1;
pub const D3DTOP_SELECTARG1: u32 = 2;
pub const D3DTOP_SELECTARG2: u32 = 3;
pub const D3DTOP_MODULATE: u32 = 4;
pub const D3DTOP_MODULATE2X: u32 = 5;
pub const D3DTOP_MODULATE4X: u32 = 6;
pub const D3DTOP_ADD: u32 = 7;
pub const D3DTOP_ADDSIGNED: u32 = 8;
pub const D3DTOP_ADDSIGNED2X: u32 = 9;
pub const D3DTOP_SUBTRACT: u32 = 10;
pub const D3DTOP_BLENDDIFFUSEALPHA: u32 = 12;
pub const D3DTOP_BLENDTEXTUREALPHA: u32 = 13;
pub const D3DTOP_BLENDFACTORALPHA: u32 = 14;
pub const D3DTOP_BLENDCURRENTALPHA: u32 = 16;
pub const D3DTOP_DOTPRODUCT3: u32 = 24;

pub const D3DTA_SELECTMASK: u32 = 0x0F;
pub const D3DTA_DIFFUSE: u32 = 0;
pub const D3DTA_CURRENT: u32 = 1;
pub const D3DTA_TEXTURE: u32 = 2;
pub const D3DTA_TFACTOR: u32 = 3;
pub const D3DTA_SPECULAR: u32 = 4;
pub const D3DTA_TEMP: u32 = 5;
pub const D3DTA_COMPLEMENT: u32 = 0x10;
pub const D3DTA_ALPHAREPLICATE: u32 = 0x20;

pub const D3DLIGHT_POINT: u32 = 1;
pub const D3DLIGHT_SPOT: u32 = 2;
pub const D3DLIGHT_DIRECTIONAL: u32 = 3;

pub const D3DFOG_NONE: u32 = 0;
pub const D3DFOG_EXP: u32 = 1;
pub const D3DFOG_EXP2: u32 = 2;
pub const D3DFOG_LINEAR: u32 = 3;

pub const D3DTADDRESS_WRAP: u32 = 1;
pub const D3DTADDRESS_MIRROR: u32 = 2;
pub const D3DTADDRESS_CLAMP: u32 = 3;
pub const D3DTADDRESS_BORDER: u32 = 4;
pub const D3DTADDRESS_MIRRORONCE: u32 = 5;

pub const D3DTEXF_NONE: u32 = 0;
pub const D3DTEXF_POINT: u32 = 1;
pub const D3DTEXF_LINEAR: u32 = 2;
pub const D3DTEXF_ANISOTROPIC: u32 = 3;

pub const D3DFILL_POINT: u32 = 1;
pub const D3DFILL_WIREFRAME: u32 = 2;
pub const D3DFILL_SOLID: u32 = 3;

pub const D3DSHADE_FLAT: u32 = 1;
pub const D3DSHADE_GOURAUD: u32 = 2;

pub const D3DCULL_NONE: u32 = 1;
pub const D3DCULL_CW: u32 = 2;
pub const D3DCULL_CCW: u32 = 3;

pub const D3DBLEND_ZERO: u32 = 1;
pub const D3DBLEND_ONE: u32 = 2;

pub const D3DBLENDOP_ADD: u32 = 1;

pub const D3DCMP_NEVER: u32 = 1;
pub const D3DCMP_LESS: u32 = 2;
pub const D3DCMP_EQUAL: u32 = 3;
pub const D3DCMP_LESSEQUAL: u32 = 4;
pub const D3DCMP_GREATER: u32 = 5;
pub const D3DCMP_NOTEQUAL: u32 = 6;
pub const D3DCMP_GREATEREQUAL: u32 = 7;
pub const D3DCMP_ALWAYS: u32 = 8;

pub const D3DSTENCILOP_KEEP: u32 = 1;

pub const D3DMCS_MATERIAL: u32 = 0;
pub const D3DMCS_COLOR1: u32 = 1;
pub const D3DMCS_COLOR2: u32 = 2;

pub const D3DORDER_LINEAR: u32 = 1;
pub const D3DORDER_CUBIC: u32 = 3;

pub const D3DSBT_ALL: u32 = 1;
pub const D3DSBT_PIXELSTATE: u32 = 2;
pub const D3DSBT_VERTEXSTATE: u32 = 3;

pub const MAX_DEVICE_IDENTIFIER_STRING: usize = 512;

fn read_u32_slice(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}

fn read_f32_slice(bytes: &[u8], off: usize) -> f32 {
    f32::from_bits(read_u32_slice(bytes, off))
}

fn write_u32_slice(bytes: &mut [u8], off: usize, val: u32) {
    bytes[off..off + 4].copy_from_slice(&val.to_le_bytes());
}

fn write_f32_slice(bytes: &mut [u8], off: usize, val: f32) {
    write_u32_slice(bytes, off, val.to_bits());
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct D3dDisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32,
    pub format: u32,
}

impl D3dDisplayMode {
    pub const SIZE: usize = 16;

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        Ok(Self {
            width: read_u32_slice(&b, 0),
            height: read_u32_slice(&b, 4),
            refresh_rate: read_u32_slice(&b, 8),
            format: read_u32_slice(&b, 12),
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.width);
        write_u32_slice(&mut b, 4, self.height);
        write_u32_slice(&mut b, 8, self.refresh_rate);
        write_u32_slice(&mut b, 12, self.format);
        mem.write_bytes(va, &b)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct D3dPresentParameters {
    pub back_buffer_width: u32,
    pub back_buffer_height: u32,
    pub back_buffer_format: u32,
    pub back_buffer_count: u32,
    pub multi_sample_type: u32,
    pub swap_effect: u32,
    pub h_device_window: u32,
    pub windowed: u32,
    pub enable_auto_depth_stencil: u32,
    pub auto_depth_stencil_format: u32,
    pub flags: u32,
    pub full_screen_refresh_rate_in_hz: u32,
    pub full_screen_presentation_interval: u32,
}

impl D3dPresentParameters {
    pub const SIZE: usize = 52;

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        Ok(Self {
            back_buffer_width: read_u32_slice(&b, 0),
            back_buffer_height: read_u32_slice(&b, 4),
            back_buffer_format: read_u32_slice(&b, 8),
            back_buffer_count: read_u32_slice(&b, 12),
            multi_sample_type: read_u32_slice(&b, 16),
            swap_effect: read_u32_slice(&b, 20),
            h_device_window: read_u32_slice(&b, 24),
            windowed: read_u32_slice(&b, 28),
            enable_auto_depth_stencil: read_u32_slice(&b, 32),
            auto_depth_stencil_format: read_u32_slice(&b, 36),
            flags: read_u32_slice(&b, 40),
            full_screen_refresh_rate_in_hz: read_u32_slice(&b, 44),
            full_screen_presentation_interval: read_u32_slice(&b, 48),
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.back_buffer_width);
        write_u32_slice(&mut b, 4, self.back_buffer_height);
        write_u32_slice(&mut b, 8, self.back_buffer_format);
        write_u32_slice(&mut b, 12, self.back_buffer_count);
        write_u32_slice(&mut b, 16, self.multi_sample_type);
        write_u32_slice(&mut b, 20, self.swap_effect);
        write_u32_slice(&mut b, 24, self.h_device_window);
        write_u32_slice(&mut b, 28, self.windowed);
        write_u32_slice(&mut b, 32, self.enable_auto_depth_stencil);
        write_u32_slice(&mut b, 36, self.auto_depth_stencil_format);
        write_u32_slice(&mut b, 40, self.flags);
        write_u32_slice(&mut b, 44, self.full_screen_refresh_rate_in_hz);
        write_u32_slice(&mut b, 48, self.full_screen_presentation_interval);
        mem.write_bytes(va, &b)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct D3dAdapterIdentifier8 {
    pub driver: [u8; MAX_DEVICE_IDENTIFIER_STRING],
    pub description: [u8; MAX_DEVICE_IDENTIFIER_STRING],
    pub driver_version: u64,
    pub vendor_id: u32,
    pub device_id: u32,
    pub sub_sys_id: u32,
    pub revision: u32,
    pub device_identifier: [u8; 16],
    pub whql_level: u32,
}

impl Default for D3dAdapterIdentifier8 {
    fn default() -> Self {
        Self {
            driver: [0u8; MAX_DEVICE_IDENTIFIER_STRING],
            description: [0u8; MAX_DEVICE_IDENTIFIER_STRING],
            driver_version: 0x0006_000E_000A_0001,
            vendor_id: 0x10DE,
            device_id: 0x0250,
            sub_sys_id: 0,
            revision: 0xA1,
            device_identifier: [
                0xD7, 0xB7, 0x1E, 0x3E, 0x41, 0x02, 0x11, 0xCF, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38,
                0x9B, 0x71,
            ],
            whql_level: 1,
        }
    }
}

impl D3dAdapterIdentifier8 {
    pub const SIZE: usize = 1068;

    pub fn new_default() -> Self {
        let mut ident = Self::default();
        write_cstr_fixed(&mut ident.driver, "toluca_gles3.dll");
        write_cstr_fixed(&mut ident.description, "toluca-portmaster OpenGL ES 3.0 Adapter");
        ident
    }

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        let mut driver = [0u8; MAX_DEVICE_IDENTIFIER_STRING];
        let mut description = [0u8; MAX_DEVICE_IDENTIFIER_STRING];
        driver.copy_from_slice(&b[0..512]);
        description.copy_from_slice(&b[512..1024]);
        let driver_version = u64::from_le_bytes(b[1024..1032].try_into().unwrap());
        let mut device_identifier = [0u8; 16];
        device_identifier.copy_from_slice(&b[1048..1064]);
        Ok(Self {
            driver,
            description,
            driver_version,
            vendor_id: read_u32_slice(&b, 1032),
            device_id: read_u32_slice(&b, 1036),
            sub_sys_id: read_u32_slice(&b, 1040),
            revision: read_u32_slice(&b, 1044),
            device_identifier,
            whql_level: read_u32_slice(&b, 1064),
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = vec![0u8; Self::SIZE];
        b[0..512].copy_from_slice(&self.driver);
        b[512..1024].copy_from_slice(&self.description);
        b[1024..1032].copy_from_slice(&self.driver_version.to_le_bytes());
        write_u32_slice(&mut b, 1032, self.vendor_id);
        write_u32_slice(&mut b, 1036, self.device_id);
        write_u32_slice(&mut b, 1040, self.sub_sys_id);
        write_u32_slice(&mut b, 1044, self.revision);
        b[1048..1064].copy_from_slice(&self.device_identifier);
        write_u32_slice(&mut b, 1064, self.whql_level);
        mem.write_bytes(va, &b)
    }
}

fn write_cstr_fixed(dst: &mut [u8], text: &str) {
    dst.fill(0);
    let max_copy = dst.len().saturating_sub(1);
    let bytes = text.as_bytes();
    let count = bytes.len().min(max_copy);
    dst[..count].copy_from_slice(&bytes[..count]);
}

#[derive(Clone, Debug, PartialEq)]
pub struct D3dCaps8 {
    pub device_type: u32,
    pub adapter_ordinal: u32,
    pub caps: u32,
    pub caps2: u32,
    pub caps3: u32,
    pub presentation_intervals: u32,
    pub cursor_caps: u32,
    pub dev_caps: u32,
    pub primitive_misc_caps: u32,
    pub raster_caps: u32,
    pub z_cmp_caps: u32,
    pub src_blend_caps: u32,
    pub dest_blend_caps: u32,
    pub alpha_cmp_caps: u32,
    pub shade_caps: u32,
    pub texture_caps: u32,
    pub texture_filter_caps: u32,
    pub cube_texture_filter_caps: u32,
    pub volume_texture_filter_caps: u32,
    pub texture_address_caps: u32,
    pub volume_texture_address_caps: u32,
    pub line_caps: u32,
    pub max_texture_width: u32,
    pub max_texture_height: u32,
    pub max_volume_extent: u32,
    pub max_texture_repeat: u32,
    pub max_texture_aspect_ratio: u32,
    pub max_anisotropy: u32,
    pub max_vertex_w: f32,
    pub guard_band_left: f32,
    pub guard_band_top: f32,
    pub guard_band_right: f32,
    pub guard_band_bottom: f32,
    pub extents_adjust: f32,
    pub stencil_caps: u32,
    pub fvf_caps: u32,
    pub texture_op_caps: u32,
    pub max_texture_blend_stages: u32,
    pub max_simultaneous_textures: u32,
    pub vertex_processing_caps: u32,
    pub max_active_lights: u32,
    pub max_user_clip_planes: u32,
    pub max_vertex_blend_matrices: u32,
    pub max_vertex_blend_matrix_index: u32,
    pub max_point_size: f32,
    pub max_primitive_count: u32,
    pub max_vertex_index: u32,
    pub max_streams: u32,
    pub max_stream_stride: u32,
    pub vertex_shader_version: u32,
    pub max_vertex_shader_const: u32,
    pub pixel_shader_version: u32,
    pub max_pixel_shader_value: f32,
}

impl D3dCaps8 {
    pub const SIZE: usize = 212;

    pub fn gles3_defaults(device_type: u32, adapter_ordinal: u32, max_tex_size: u32) -> Self {
        let clamped_tex = max_tex_size.max(256);
        Self {
            device_type,
            adapter_ordinal,
            caps: 0x0002_0000,
            caps2: 0x2012_0000,
            caps3: 0x0000_0020,
            presentation_intervals: 0x8000_0001,
            cursor_caps: 0x0000_0001,
            dev_caps: 0x001B_EFF0,
            primitive_misc_caps: 0x0000_0CF2,
            raster_caps: 0x0032_2191,
            z_cmp_caps: 0x0000_00FF,
            src_blend_caps: 0x0000_1FFF,
            dest_blend_caps: 0x0000_1FFF,
            alpha_cmp_caps: 0x0000_00FF,
            shade_caps: 0x0008_4208,
            texture_caps: 0x0003_4C8D,
            texture_filter_caps: 0x0703_0700,
            cube_texture_filter_caps: 0x0703_0700,
            volume_texture_filter_caps: 0x0303_0300,
            texture_address_caps: 0x0000_003F,
            volume_texture_address_caps: 0x0000_003F,
            line_caps: 0x0000_001F,
            max_texture_width: clamped_tex,
            max_texture_height: clamped_tex,
            max_volume_extent: clamped_tex.min(2048),
            max_texture_repeat: 8192,
            max_texture_aspect_ratio: 0,
            max_anisotropy: 16,
            max_vertex_w: 1e10,
            guard_band_left: -32768.0,
            guard_band_top: -32768.0,
            guard_band_right: 32768.0,
            guard_band_bottom: 32768.0,
            extents_adjust: 0.0,
            stencil_caps: 0x0000_00FF,
            fvf_caps: 0x0018_0008,
            texture_op_caps: 0x03FF_FFFF,
            max_texture_blend_stages: 8,
            max_simultaneous_textures: 8,
            vertex_processing_caps: 0x0000_007B,
            max_active_lights: 8,
            max_user_clip_planes: 6,
            max_vertex_blend_matrices: 4,
            max_vertex_blend_matrix_index: 0,
            max_point_size: 64.0,
            max_primitive_count: 0x00FF_FFFF,
            max_vertex_index: 0x00FF_FFFF,
            max_streams: 16,
            max_stream_stride: 256,
            vertex_shader_version: 0xFFFE_0101,
            max_vertex_shader_const: 96,
            pixel_shader_version: 0xFFFF_0101,
            max_pixel_shader_value: 1.0,
        }
    }

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        Ok(Self {
            device_type: read_u32_slice(&b, 0),
            adapter_ordinal: read_u32_slice(&b, 4),
            caps: read_u32_slice(&b, 8),
            caps2: read_u32_slice(&b, 12),
            caps3: read_u32_slice(&b, 16),
            presentation_intervals: read_u32_slice(&b, 20),
            cursor_caps: read_u32_slice(&b, 24),
            dev_caps: read_u32_slice(&b, 28),
            primitive_misc_caps: read_u32_slice(&b, 32),
            raster_caps: read_u32_slice(&b, 36),
            z_cmp_caps: read_u32_slice(&b, 40),
            src_blend_caps: read_u32_slice(&b, 44),
            dest_blend_caps: read_u32_slice(&b, 48),
            alpha_cmp_caps: read_u32_slice(&b, 52),
            shade_caps: read_u32_slice(&b, 56),
            texture_caps: read_u32_slice(&b, 60),
            texture_filter_caps: read_u32_slice(&b, 64),
            cube_texture_filter_caps: read_u32_slice(&b, 68),
            volume_texture_filter_caps: read_u32_slice(&b, 72),
            texture_address_caps: read_u32_slice(&b, 76),
            volume_texture_address_caps: read_u32_slice(&b, 80),
            line_caps: read_u32_slice(&b, 84),
            max_texture_width: read_u32_slice(&b, 88),
            max_texture_height: read_u32_slice(&b, 92),
            max_volume_extent: read_u32_slice(&b, 96),
            max_texture_repeat: read_u32_slice(&b, 100),
            max_texture_aspect_ratio: read_u32_slice(&b, 104),
            max_anisotropy: read_u32_slice(&b, 108),
            max_vertex_w: read_f32_slice(&b, 112),
            guard_band_left: read_f32_slice(&b, 116),
            guard_band_top: read_f32_slice(&b, 120),
            guard_band_right: read_f32_slice(&b, 124),
            guard_band_bottom: read_f32_slice(&b, 128),
            extents_adjust: read_f32_slice(&b, 132),
            stencil_caps: read_u32_slice(&b, 136),
            fvf_caps: read_u32_slice(&b, 140),
            texture_op_caps: read_u32_slice(&b, 144),
            max_texture_blend_stages: read_u32_slice(&b, 148),
            max_simultaneous_textures: read_u32_slice(&b, 152),
            vertex_processing_caps: read_u32_slice(&b, 156),
            max_active_lights: read_u32_slice(&b, 160),
            max_user_clip_planes: read_u32_slice(&b, 164),
            max_vertex_blend_matrices: read_u32_slice(&b, 168),
            max_vertex_blend_matrix_index: read_u32_slice(&b, 172),
            max_point_size: read_f32_slice(&b, 176),
            max_primitive_count: read_u32_slice(&b, 180),
            max_vertex_index: read_u32_slice(&b, 184),
            max_streams: read_u32_slice(&b, 188),
            max_stream_stride: read_u32_slice(&b, 192),
            vertex_shader_version: read_u32_slice(&b, 196),
            max_vertex_shader_const: read_u32_slice(&b, 200),
            pixel_shader_version: read_u32_slice(&b, 204),
            max_pixel_shader_value: read_f32_slice(&b, 208),
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.device_type);
        write_u32_slice(&mut b, 4, self.adapter_ordinal);
        write_u32_slice(&mut b, 8, self.caps);
        write_u32_slice(&mut b, 12, self.caps2);
        write_u32_slice(&mut b, 16, self.caps3);
        write_u32_slice(&mut b, 20, self.presentation_intervals);
        write_u32_slice(&mut b, 24, self.cursor_caps);
        write_u32_slice(&mut b, 28, self.dev_caps);
        write_u32_slice(&mut b, 32, self.primitive_misc_caps);
        write_u32_slice(&mut b, 36, self.raster_caps);
        write_u32_slice(&mut b, 40, self.z_cmp_caps);
        write_u32_slice(&mut b, 44, self.src_blend_caps);
        write_u32_slice(&mut b, 48, self.dest_blend_caps);
        write_u32_slice(&mut b, 52, self.alpha_cmp_caps);
        write_u32_slice(&mut b, 56, self.shade_caps);
        write_u32_slice(&mut b, 60, self.texture_caps);
        write_u32_slice(&mut b, 64, self.texture_filter_caps);
        write_u32_slice(&mut b, 68, self.cube_texture_filter_caps);
        write_u32_slice(&mut b, 72, self.volume_texture_filter_caps);
        write_u32_slice(&mut b, 76, self.texture_address_caps);
        write_u32_slice(&mut b, 80, self.volume_texture_address_caps);
        write_u32_slice(&mut b, 84, self.line_caps);
        write_u32_slice(&mut b, 88, self.max_texture_width);
        write_u32_slice(&mut b, 92, self.max_texture_height);
        write_u32_slice(&mut b, 96, self.max_volume_extent);
        write_u32_slice(&mut b, 100, self.max_texture_repeat);
        write_u32_slice(&mut b, 104, self.max_texture_aspect_ratio);
        write_u32_slice(&mut b, 108, self.max_anisotropy);
        write_f32_slice(&mut b, 112, self.max_vertex_w);
        write_f32_slice(&mut b, 116, self.guard_band_left);
        write_f32_slice(&mut b, 120, self.guard_band_top);
        write_f32_slice(&mut b, 124, self.guard_band_right);
        write_f32_slice(&mut b, 128, self.guard_band_bottom);
        write_f32_slice(&mut b, 132, self.extents_adjust);
        write_u32_slice(&mut b, 136, self.stencil_caps);
        write_u32_slice(&mut b, 140, self.fvf_caps);
        write_u32_slice(&mut b, 144, self.texture_op_caps);
        write_u32_slice(&mut b, 148, self.max_texture_blend_stages);
        write_u32_slice(&mut b, 152, self.max_simultaneous_textures);
        write_u32_slice(&mut b, 156, self.vertex_processing_caps);
        write_u32_slice(&mut b, 160, self.max_active_lights);
        write_u32_slice(&mut b, 164, self.max_user_clip_planes);
        write_u32_slice(&mut b, 168, self.max_vertex_blend_matrices);
        write_u32_slice(&mut b, 172, self.max_vertex_blend_matrix_index);
        write_f32_slice(&mut b, 176, self.max_point_size);
        write_u32_slice(&mut b, 180, self.max_primitive_count);
        write_u32_slice(&mut b, 184, self.max_vertex_index);
        write_u32_slice(&mut b, 188, self.max_streams);
        write_u32_slice(&mut b, 192, self.max_stream_stride);
        write_u32_slice(&mut b, 196, self.vertex_shader_version);
        write_u32_slice(&mut b, 200, self.max_vertex_shader_const);
        write_u32_slice(&mut b, 204, self.pixel_shader_version);
        write_f32_slice(&mut b, 208, self.max_pixel_shader_value);
        mem.write_bytes(va, &b)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct D3dSurfaceDesc {
    pub format: u32,
    pub resource_type: u32,
    pub usage: u32,
    pub pool: u32,
    pub size: u32,
    pub multi_sample_type: u32,
    pub width: u32,
    pub height: u32,
}

impl D3dSurfaceDesc {
    pub const SIZE: usize = 32;

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        Ok(Self {
            format: read_u32_slice(&b, 0),
            resource_type: read_u32_slice(&b, 4),
            usage: read_u32_slice(&b, 8),
            pool: read_u32_slice(&b, 12),
            size: read_u32_slice(&b, 16),
            multi_sample_type: read_u32_slice(&b, 20),
            width: read_u32_slice(&b, 24),
            height: read_u32_slice(&b, 28),
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.format);
        write_u32_slice(&mut b, 4, self.resource_type);
        write_u32_slice(&mut b, 8, self.usage);
        write_u32_slice(&mut b, 12, self.pool);
        write_u32_slice(&mut b, 16, self.size);
        write_u32_slice(&mut b, 20, self.multi_sample_type);
        write_u32_slice(&mut b, 24, self.width);
        write_u32_slice(&mut b, 28, self.height);
        mem.write_bytes(va, &b)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct D3dVertexBufferDesc {
    pub format: u32,
    pub resource_type: u32,
    pub usage: u32,
    pub pool: u32,
    pub size: u32,
    pub fvf: u32,
}

impl D3dVertexBufferDesc {
    pub const SIZE: usize = 24;

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        Ok(Self {
            format: read_u32_slice(&b, 0),
            resource_type: read_u32_slice(&b, 4),
            usage: read_u32_slice(&b, 8),
            pool: read_u32_slice(&b, 12),
            size: read_u32_slice(&b, 16),
            fvf: read_u32_slice(&b, 20),
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.format);
        write_u32_slice(&mut b, 4, self.resource_type);
        write_u32_slice(&mut b, 8, self.usage);
        write_u32_slice(&mut b, 12, self.pool);
        write_u32_slice(&mut b, 16, self.size);
        write_u32_slice(&mut b, 20, self.fvf);
        mem.write_bytes(va, &b)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct D3dIndexBufferDesc {
    pub format: u32,
    pub resource_type: u32,
    pub usage: u32,
    pub pool: u32,
    pub size: u32,
}

impl D3dIndexBufferDesc {
    pub const SIZE: usize = 20;

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        Ok(Self {
            format: read_u32_slice(&b, 0),
            resource_type: read_u32_slice(&b, 4),
            usage: read_u32_slice(&b, 8),
            pool: read_u32_slice(&b, 12),
            size: read_u32_slice(&b, 16),
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.format);
        write_u32_slice(&mut b, 4, self.resource_type);
        write_u32_slice(&mut b, 8, self.usage);
        write_u32_slice(&mut b, 12, self.pool);
        write_u32_slice(&mut b, 16, self.size);
        mem.write_bytes(va, &b)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct D3dViewport8 {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub min_z: f32,
    pub max_z: f32,
}

impl D3dViewport8 {
    pub const SIZE: usize = 24;

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        Ok(Self {
            x: read_u32_slice(&b, 0),
            y: read_u32_slice(&b, 4),
            width: read_u32_slice(&b, 8),
            height: read_u32_slice(&b, 12),
            min_z: read_f32_slice(&b, 16),
            max_z: read_f32_slice(&b, 20),
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.x);
        write_u32_slice(&mut b, 4, self.y);
        write_u32_slice(&mut b, 8, self.width);
        write_u32_slice(&mut b, 12, self.height);
        write_f32_slice(&mut b, 16, self.min_z);
        write_f32_slice(&mut b, 20, self.max_z);
        mem.write_bytes(va, &b)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct D3dMaterial8 {
    pub diffuse: [f32; 4],
    pub ambient: [f32; 4],
    pub specular: [f32; 4],
    pub emissive: [f32; 4],
    pub power: f32,
}

impl D3dMaterial8 {
    pub const SIZE: usize = 68;

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        let mut floats = [0.0f32; 17];
        for (i, dst) in floats.iter_mut().enumerate() {
            *dst = read_f32_slice(&b, i * 4);
        }
        Ok(Self {
            diffuse: [floats[0], floats[1], floats[2], floats[3]],
            ambient: [floats[4], floats[5], floats[6], floats[7]],
            specular: [floats[8], floats[9], floats[10], floats[11]],
            emissive: [floats[12], floats[13], floats[14], floats[15]],
            power: floats[16],
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        let groups = [self.diffuse, self.ambient, self.specular, self.emissive];
        for (g_idx, group) in groups.iter().enumerate() {
            for (c_idx, &val) in group.iter().enumerate() {
                write_f32_slice(&mut b, (g_idx * 4 + c_idx) * 4, val);
            }
        }
        write_f32_slice(&mut b, 64, self.power);
        mem.write_bytes(va, &b)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct D3dLight8 {
    pub light_type: u32,
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub ambient: [f32; 4],
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub range: f32,
    pub falloff: f32,
    pub attenuation0: f32,
    pub attenuation1: f32,
    pub attenuation2: f32,
    pub theta: f32,
    pub phi: f32,
}

impl D3dLight8 {
    pub const SIZE: usize = 104;

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        Ok(Self {
            light_type: read_u32_slice(&b, 0),
            diffuse: [
                read_f32_slice(&b, 4),
                read_f32_slice(&b, 8),
                read_f32_slice(&b, 12),
                read_f32_slice(&b, 16),
            ],
            specular: [
                read_f32_slice(&b, 20),
                read_f32_slice(&b, 24),
                read_f32_slice(&b, 28),
                read_f32_slice(&b, 32),
            ],
            ambient: [
                read_f32_slice(&b, 36),
                read_f32_slice(&b, 40),
                read_f32_slice(&b, 44),
                read_f32_slice(&b, 48),
            ],
            position: [
                read_f32_slice(&b, 52),
                read_f32_slice(&b, 56),
                read_f32_slice(&b, 60),
            ],
            direction: [
                read_f32_slice(&b, 64),
                read_f32_slice(&b, 68),
                read_f32_slice(&b, 72),
            ],
            range: read_f32_slice(&b, 76),
            falloff: read_f32_slice(&b, 80),
            attenuation0: read_f32_slice(&b, 84),
            attenuation1: read_f32_slice(&b, 88),
            attenuation2: read_f32_slice(&b, 92),
            theta: read_f32_slice(&b, 96),
            phi: read_f32_slice(&b, 100),
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.light_type);
        for i in 0..4 {
            write_f32_slice(&mut b, 4 + i * 4, self.diffuse[i]);
            write_f32_slice(&mut b, 20 + i * 4, self.specular[i]);
            write_f32_slice(&mut b, 36 + i * 4, self.ambient[i]);
        }
        for i in 0..3 {
            write_f32_slice(&mut b, 52 + i * 4, self.position[i]);
            write_f32_slice(&mut b, 64 + i * 4, self.direction[i]);
        }
        write_f32_slice(&mut b, 76, self.range);
        write_f32_slice(&mut b, 80, self.falloff);
        write_f32_slice(&mut b, 84, self.attenuation0);
        write_f32_slice(&mut b, 88, self.attenuation1);
        write_f32_slice(&mut b, 92, self.attenuation2);
        write_f32_slice(&mut b, 96, self.theta);
        write_f32_slice(&mut b, 100, self.phi);
        mem.write_bytes(va, &b)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct D3dRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl D3dRect {
    pub const SIZE: usize = 16;

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        Ok(Self {
            left: read_u32_slice(&b, 0) as i32,
            top: read_u32_slice(&b, 4) as i32,
            right: read_u32_slice(&b, 8) as i32,
            bottom: read_u32_slice(&b, 12) as i32,
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.left as u32);
        write_u32_slice(&mut b, 4, self.top as u32);
        write_u32_slice(&mut b, 8, self.right as u32);
        write_u32_slice(&mut b, 12, self.bottom as u32);
        mem.write_bytes(va, &b)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct D3dLockedRect {
    pub pitch: i32,
    pub p_bits: u32,
}

impl D3dLockedRect {
    pub const SIZE: usize = 8;

    pub fn read_from(mem: &dyn GuestMemory, va: GuestAddr) -> Result<Self, String> {
        let b = mem.read_bytes(va, Self::SIZE)?;
        Ok(Self {
            pitch: read_u32_slice(&b, 0) as i32,
            p_bits: read_u32_slice(&b, 4),
        })
    }

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.pitch as u32);
        write_u32_slice(&mut b, 4, self.p_bits);
        mem.write_bytes(va, &b)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct D3dDeviceCreationParameters {
    pub adapter_ordinal: u32,
    pub device_type: u32,
    pub h_focus_window: u32,
    pub behavior_flags: u32,
}

impl D3dDeviceCreationParameters {
    pub const SIZE: usize = 16;

    pub fn write_to(&self, mem: &mut dyn GuestMemory, va: GuestAddr) -> Result<(), String> {
        let mut b = [0u8; Self::SIZE];
        write_u32_slice(&mut b, 0, self.adapter_ordinal);
        write_u32_slice(&mut b, 4, self.device_type);
        write_u32_slice(&mut b, 8, self.h_focus_window);
        write_u32_slice(&mut b, 12, self.behavior_flags);
        mem.write_bytes(va, &b)
    }
}

pub fn read_matrix_4x4(mem: &dyn GuestMemory, va: GuestAddr) -> Result<[f32; 16], String> {
    let b = mem.read_bytes(va, 64)?;
    let mut out = [0.0f32; 16];
    for (i, dst) in out.iter_mut().enumerate() {
        *dst = read_f32_slice(&b, i * 4);
    }
    Ok(out)
}

pub fn write_matrix_4x4(
    mem: &mut dyn GuestMemory,
    va: GuestAddr,
    mat: &[f32; 16],
) -> Result<(), String> {
    let mut b = [0u8; 64];
    for (i, &val) in mat.iter().enumerate() {
        write_f32_slice(&mut b, i * 4, val);
    }
    mem.write_bytes(va, &b)
}
