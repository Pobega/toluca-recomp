use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::ffi::{CStr, CString, c_char, c_void};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::backend::{ActiveShaders, RenderBackend};
use crate::device::vertex_count_for_primitives;
use crate::shader_asm::{
    D3DVSDT_D3DCOLOR, D3DVSDT_FLOAT1, D3DVSDT_FLOAT2, D3DVSDT_FLOAT3, D3DVSDT_FLOAT4,
    D3DVSDT_SHORT2, D3DVSDT_SHORT4, D3DVSDT_UBYTE4, VertexDeclElement, translate_ps_1_x,
    translate_vs_1_x,
};
use crate::shader_cache::{CachedProgramBinary, ShaderRecipe, ShaderRecipeCache};
use crate::shader_ffp::{
    ATTR_DIFFUSE, ATTR_NORMAL, ATTR_POS, ATTR_SPECULAR, ATTR_TEX0, FfpShaderKey, MAX_FFP_LIGHTS,
    MAX_FFP_STAGES, TextureStageKey, generate_ffp_shaders,
};
use crate::state::DeviceState;
use crate::texture::PreparedTextureUpload;
use crate::types::{
    D3DCLEAR_STENCIL, D3DCLEAR_TARGET, D3DCLEAR_ZBUFFER, D3DCMP_ALWAYS, D3DCMP_EQUAL,
    D3DCMP_GREATER, D3DCMP_GREATEREQUAL, D3DCMP_LESS, D3DCMP_LESSEQUAL, D3DCMP_NEVER,
    D3DCMP_NOTEQUAL, D3DCULL_CCW, D3DCULL_CW, D3DCULL_NONE, D3DFMT_DXT1, D3DFMT_DXT3, D3DFMT_DXT5,
    D3DFMT_INDEX32, D3DFVF_DIFFUSE, D3DFVF_NORMAL, D3DFVF_POSITION_MASK, D3DFVF_PSIZE,
    D3DFVF_SPECULAR, D3DFVF_TEXCOUNT_MASK, D3DFVF_TEXCOUNT_SHIFT, D3DFVF_XYZ, D3DFVF_XYZB1,
    D3DFVF_XYZB5, D3DFVF_XYZRHW, D3DPT_LINELIST, D3DPT_LINESTRIP, D3DPT_POINTLIST,
    D3DPT_TRIANGLEFAN, D3DPT_TRIANGLELIST, D3DPT_TRIANGLESTRIP, D3DRS_ALPHABLENDENABLE,
    D3DRS_ALPHAREF, D3DRS_AMBIENT, D3DRS_BLENDOP, D3DRS_COLORWRITEENABLE, D3DRS_CULLMODE,
    D3DRS_DESTBLEND,
    D3DRS_FOGCOLOR, D3DRS_FOGDENSITY, D3DRS_FOGEND, D3DRS_FOGSTART, D3DRS_SRCBLEND,
    D3DRS_STENCILENABLE, D3DRS_STENCILFAIL, D3DRS_STENCILFUNC, D3DRS_STENCILMASK,
    D3DRS_STENCILPASS, D3DRS_STENCILREF, D3DRS_STENCILWRITEMASK, D3DRS_STENCILZFAIL,
    D3DRS_TEXTUREFACTOR, D3DRS_ZENABLE, D3DRS_ZFUNC, D3DRS_ZWRITEENABLE, D3DTADDRESS_BORDER,
    D3DTADDRESS_CLAMP, D3DTADDRESS_MIRROR, D3DTADDRESS_MIRRORONCE, D3DTADDRESS_WRAP,
    D3DTEXF_LINEAR, D3DTEXF_NONE, D3DTEXF_POINT, D3DTS_PROJECTION, D3DTS_TEXTURE0, D3DTS_VIEW,
    D3DTS_WORLD, D3DTSS_ADDRESSU, D3DTSS_ADDRESSV, D3DTSS_MAGFILTER, D3DTSS_MINFILTER,
    D3DTSS_MIPFILTER, D3DUSAGE_DYNAMIC, D3dRect, fvf_texcoord_floats,
};

pub type GlGetProcAddrFn = unsafe extern "C" fn(name: *const c_char) -> *mut c_void;

pub trait PresentTarget: Send {
    fn drawable_size(&self) -> (u32, u32);
    fn swap_buffers(&mut self);
}

#[derive(Copy, Clone, Debug)]
pub struct NoopPresentTarget {
    pub width: u32,
    pub height: u32,
}

impl PresentTarget for NoopPresentTarget {
    fn drawable_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn swap_buffers(&mut self) {}
}

const ASPECT_EPSILON: f64 = 1e-4;
const ASPECT_RATIO_4_3: f64 = 4.0 / 3.0;
const ASPECT_RATIO_16_9: f64 = 16.0 / 9.0;
const CACHE_FLUSH_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AspectMode {
    #[default]
    Ratio4x3,
    Ratio16x9,
    Stretch,
}

impl AspectMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "4:3" | "4x3" => Some(Self::Ratio4x3),
            "16:9" | "16x9" => Some(Self::Ratio16x9),
            "stretch" | "fill" => Some(Self::Stretch),
            _ => None,
        }
    }
}

type GLenum = u32;
type GLuint = u32;
type GLint = i32;
type GLsizei = i32;
type GLboolean = u8;
type GLfloat = f32;
type GLbitfield = u32;
type GLintptr = isize;
type GLsizeiptr = isize;

const GL_FALSE: GLboolean = 0;
const GL_TRUE: GLboolean = 1;

const GL_DEPTH_BUFFER_BIT: GLbitfield = 0x0000_0100;
const GL_STENCIL_BUFFER_BIT: GLbitfield = 0x0000_0400;
const GL_COLOR_BUFFER_BIT: GLbitfield = 0x0000_4000;

const GL_POINTS: GLenum = 0x0000;
const GL_LINES: GLenum = 0x0001;
const GL_LINE_STRIP: GLenum = 0x0003;
const GL_TRIANGLES: GLenum = 0x0004;
const GL_TRIANGLE_STRIP: GLenum = 0x0005;
const GL_TRIANGLE_FAN: GLenum = 0x0006;

const GL_ZERO: GLenum = 0;
const GL_ONE: GLenum = 1;
const GL_SRC_COLOR: GLenum = 0x0300;
const GL_ONE_MINUS_SRC_COLOR: GLenum = 0x0301;
const GL_SRC_ALPHA: GLenum = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;
const GL_DST_ALPHA: GLenum = 0x0304;
const GL_ONE_MINUS_DST_ALPHA: GLenum = 0x0305;
const GL_DST_COLOR: GLenum = 0x0306;
const GL_ONE_MINUS_DST_COLOR: GLenum = 0x0307;
const GL_SRC_ALPHA_SATURATE: GLenum = 0x0308;

const GL_FUNC_ADD: GLenum = 0x8006;
const GL_MIN: GLenum = 0x8007;
const GL_MAX: GLenum = 0x8008;
const GL_FUNC_SUBTRACT: GLenum = 0x800A;
const GL_FUNC_REVERSE_SUBTRACT: GLenum = 0x800B;

const GL_NEVER: GLenum = 0x0200;
const GL_LESS: GLenum = 0x0201;
const GL_EQUAL: GLenum = 0x0202;
const GL_LEQUAL: GLenum = 0x0203;
const GL_GREATER: GLenum = 0x0204;
const GL_NOTEQUAL: GLenum = 0x0205;
const GL_GEQUAL: GLenum = 0x0206;
const GL_ALWAYS: GLenum = 0x0207;

const GL_FRONT: GLenum = 0x0404;
const GL_BACK: GLenum = 0x0405;
const GL_CCW: GLenum = 0x0901;

const GL_CULL_FACE: GLenum = 0x0B44;
const GL_DEPTH_TEST: GLenum = 0x0B71;
const GL_STENCIL_TEST: GLenum = 0x0B90;
const GL_BLEND: GLenum = 0x0BE2;
const GL_SCISSOR_TEST: GLenum = 0x0C11;

const GL_PACK_ALIGNMENT: GLenum = 0x0D05;
const GL_UNPACK_ALIGNMENT: GLenum = 0x0CF5;
const GL_MAX_TEXTURE_SIZE: GLenum = 0x0D33;

const GL_TEXTURE_2D: GLenum = 0x0DE1;
const GL_SHORT: GLenum = 0x1402;
const GL_UNSIGNED_BYTE: GLenum = 0x1401;
const GL_UNSIGNED_SHORT: GLenum = 0x1403;
const GL_UNSIGNED_INT: GLenum = 0x1405;
const GL_FLOAT: GLenum = 0x1406;
const GL_RGBA: GLenum = 0x1908;
const GL_RGBA8: GLenum = 0x8058;

const GL_KEEP: GLenum = 0x1E00;
const GL_REPLACE: GLenum = 0x1E01;
const GL_INCR: GLenum = 0x1E02;
const GL_DECR: GLenum = 0x1E03;
const GL_INVERT: GLenum = 0x150A;
const GL_INCR_WRAP: GLenum = 0x8507;
const GL_DECR_WRAP: GLenum = 0x8508;

const GL_NEAREST: GLenum = 0x2600;
const GL_LINEAR: GLenum = 0x2601;
const GL_NEAREST_MIPMAP_NEAREST: GLenum = 0x2700;
const GL_LINEAR_MIPMAP_NEAREST: GLenum = 0x2701;
const GL_NEAREST_MIPMAP_LINEAR: GLenum = 0x2702;
const GL_LINEAR_MIPMAP_LINEAR: GLenum = 0x2703;
const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
const GL_REPEAT: GLenum = 0x2901;
const GL_CLAMP_TO_EDGE: GLenum = 0x812F;
const GL_MIRRORED_REPEAT: GLenum = 0x8370;
const GL_TEXTURE_BASE_LEVEL: GLenum = 0x813C;
const GL_TEXTURE_MAX_LEVEL: GLenum = 0x813D;

const GL_TEXTURE0: GLenum = 0x84C0;
const GL_RENDERER: GLenum = 0x1F01;
const GL_VERSION: GLenum = 0x1F02;
const GL_NUM_EXTENSIONS: GLenum = 0x821D;
const GL_EXTENSIONS: GLenum = 0x1F03;

const GL_ARRAY_BUFFER: GLenum = 0x8892;
const GL_ELEMENT_ARRAY_BUFFER: GLenum = 0x8893;
const GL_STATIC_DRAW: GLenum = 0x88E4;
const GL_DYNAMIC_DRAW: GLenum = 0x88E8;

const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
const GL_VERTEX_SHADER: GLenum = 0x8B31;
const GL_COMPILE_STATUS: GLenum = 0x8B81;
const GL_LINK_STATUS: GLenum = 0x8B82;
const GL_INFO_LOG_LENGTH: GLenum = 0x8B84;
const GL_PROGRAM_BINARY_RETRIEVABLE_HINT: GLenum = 0x8257;
const GL_PROGRAM_BINARY_LENGTH: GLenum = 0x8741;

const GL_FRAMEBUFFER: GLenum = 0x8D40;
const GL_RENDERBUFFER: GLenum = 0x8D41;
const GL_DEPTH24_STENCIL8: GLenum = 0x88F0;
const GL_COLOR_ATTACHMENT0: GLenum = 0x8CE0;
const GL_DEPTH_STENCIL_ATTACHMENT: GLenum = 0x821A;
const GL_FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;

const GL_COMPRESSED_RGBA_S3TC_DXT1_EXT: GLenum = 0x83F1;
const GL_COMPRESSED_RGBA_S3TC_DXT3_EXT: GLenum = 0x83F2;
const GL_COMPRESSED_RGBA_S3TC_DXT5_EXT: GLenum = 0x83F3;

const PRESENT_VS_GLSL: &str = "#version 300 es
precision highp float;
out vec2 v_uv;
void main() {
    vec2 pos = vec2(float((gl_VertexID & 1) << 1) - 1.0, float(gl_VertexID & 2) - 1.0);
    v_uv = pos * 0.5 + 0.5;
    v_uv.y = 1.0 - v_uv.y;
    gl_Position = vec4(pos, 0.0, 1.0);
}
";

const PRESENT_FS_GLSL: &str = "#version 300 es
precision mediump float;
in vec2 v_uv;
uniform sampler2D u_tex0;
out vec4 frag_color;
void main() {
    frag_color = texture(u_tex0, v_uv);
}
";

struct Gles3Functions {
    lib_gles: *mut c_void,
    gl_get_error: unsafe extern "C" fn() -> GLenum,
    gl_get_integerv: unsafe extern "C" fn(GLenum, *mut GLint),
    gl_get_string: unsafe extern "C" fn(GLenum) -> *const u8,
    gl_get_stringi: Option<unsafe extern "C" fn(GLenum, GLuint) -> *const u8>,
    gl_enable: unsafe extern "C" fn(GLenum),
    gl_disable: unsafe extern "C" fn(GLenum),
    gl_viewport: unsafe extern "C" fn(GLint, GLint, GLsizei, GLsizei),
    gl_scissor: unsafe extern "C" fn(GLint, GLint, GLsizei, GLsizei),
    gl_clear: unsafe extern "C" fn(GLbitfield),
    gl_clear_color: unsafe extern "C" fn(GLfloat, GLfloat, GLfloat, GLfloat),
    gl_clear_depthf: unsafe extern "C" fn(GLfloat),
    gl_clear_stencil: unsafe extern "C" fn(GLint),
    gl_depth_func: unsafe extern "C" fn(GLenum),
    gl_depth_mask: unsafe extern "C" fn(GLboolean),
    gl_cull_face: unsafe extern "C" fn(GLenum),
    gl_front_face: unsafe extern "C" fn(GLenum),
    gl_blend_func_separate: unsafe extern "C" fn(GLenum, GLenum, GLenum, GLenum),
    gl_blend_equation_separate: unsafe extern "C" fn(GLenum, GLenum),
    gl_color_mask: unsafe extern "C" fn(GLboolean, GLboolean, GLboolean, GLboolean),
    gl_stencil_func: unsafe extern "C" fn(GLenum, GLint, GLuint),
    gl_stencil_op: unsafe extern "C" fn(GLenum, GLenum, GLenum),
    gl_stencil_mask: unsafe extern "C" fn(GLuint),
    gl_pixel_storei: unsafe extern "C" fn(GLenum, GLint),
    gl_create_shader: unsafe extern "C" fn(GLenum) -> GLuint,
    gl_shader_source: unsafe extern "C" fn(GLuint, GLsizei, *const *const c_char, *const GLint),
    gl_compile_shader: unsafe extern "C" fn(GLuint),
    gl_get_shaderiv: unsafe extern "C" fn(GLuint, GLenum, *mut GLint),
    gl_get_shader_info_log: unsafe extern "C" fn(GLuint, GLsizei, *mut GLsizei, *mut c_char),
    gl_delete_shader: unsafe extern "C" fn(GLuint),
    gl_create_program: unsafe extern "C" fn() -> GLuint,
    gl_attach_shader: unsafe extern "C" fn(GLuint, GLuint),
    gl_link_program: unsafe extern "C" fn(GLuint),
    gl_get_programiv: unsafe extern "C" fn(GLuint, GLenum, *mut GLint),
    gl_get_program_info_log: unsafe extern "C" fn(GLuint, GLsizei, *mut GLsizei, *mut c_char),
    gl_delete_program: unsafe extern "C" fn(GLuint),
    gl_use_program: unsafe extern "C" fn(GLuint),
    gl_program_parameteri: Option<unsafe extern "C" fn(GLuint, GLenum, GLint)>,
    gl_get_program_binary:
        Option<unsafe extern "C" fn(GLuint, GLsizei, *mut GLsizei, *mut GLenum, *mut c_void)>,
    gl_program_binary: Option<unsafe extern "C" fn(GLuint, GLenum, *const c_void, GLsizei)>,
    gl_get_uniform_location: unsafe extern "C" fn(GLuint, *const c_char) -> GLint,
    gl_uniform1i: unsafe extern "C" fn(GLint, GLint),
    gl_uniform1f: unsafe extern "C" fn(GLint, GLfloat),
    gl_uniform4f: unsafe extern "C" fn(GLint, GLfloat, GLfloat, GLfloat, GLfloat),
    gl_uniform4fv: unsafe extern "C" fn(GLint, GLsizei, *const GLfloat),
    gl_uniform_matrix4fv: unsafe extern "C" fn(GLint, GLsizei, GLboolean, *const GLfloat),
    gl_gen_buffers: unsafe extern "C" fn(GLsizei, *mut GLuint),
    gl_delete_buffers: unsafe extern "C" fn(GLsizei, *const GLuint),
    gl_bind_buffer: unsafe extern "C" fn(GLenum, GLuint),
    gl_buffer_data: unsafe extern "C" fn(GLenum, GLsizeiptr, *const c_void, GLenum),
    gl_buffer_sub_data: unsafe extern "C" fn(GLenum, GLintptr, GLsizeiptr, *const c_void),
    gl_gen_vertex_arrays: unsafe extern "C" fn(GLsizei, *mut GLuint),
    gl_delete_vertex_arrays: unsafe extern "C" fn(GLsizei, *const GLuint),
    gl_bind_vertex_array: unsafe extern "C" fn(GLuint),
    gl_enable_vertex_attrib_array: unsafe extern "C" fn(GLuint),
    gl_disable_vertex_attrib_array: unsafe extern "C" fn(GLuint),
    gl_vertex_attrib_pointer:
        unsafe extern "C" fn(GLuint, GLint, GLenum, GLboolean, GLsizei, *const c_void),
    gl_draw_arrays: unsafe extern "C" fn(GLenum, GLint, GLsizei),
    gl_draw_elements: unsafe extern "C" fn(GLenum, GLsizei, GLenum, *const c_void),
    gl_gen_textures: unsafe extern "C" fn(GLsizei, *mut GLuint),
    gl_delete_textures: unsafe extern "C" fn(GLsizei, *const GLuint),
    gl_active_texture: unsafe extern "C" fn(GLenum),
    gl_bind_texture: unsafe extern "C" fn(GLenum, GLuint),
    gl_tex_image2d: unsafe extern "C" fn(
        GLenum,
        GLint,
        GLint,
        GLsizei,
        GLsizei,
        GLint,
        GLenum,
        GLenum,
        *const c_void,
    ),
    gl_tex_sub_image2d: unsafe extern "C" fn(
        GLenum,
        GLint,
        GLint,
        GLint,
        GLsizei,
        GLsizei,
        GLenum,
        GLenum,
        *const c_void,
    ),
    gl_compressed_tex_image2d:
        unsafe extern "C" fn(GLenum, GLint, GLenum, GLsizei, GLsizei, GLint, GLsizei, *const c_void),
    gl_compressed_tex_sub_image2d: unsafe extern "C" fn(
        GLenum,
        GLint,
        GLint,
        GLint,
        GLsizei,
        GLsizei,
        GLenum,
        GLsizei,
        *const c_void,
    ),
    gl_tex_parameteri: unsafe extern "C" fn(GLenum, GLenum, GLint),
    gl_gen_framebuffers: unsafe extern "C" fn(GLsizei, *mut GLuint),
    gl_delete_framebuffers: unsafe extern "C" fn(GLsizei, *const GLuint),
    gl_bind_framebuffer: unsafe extern "C" fn(GLenum, GLuint),
    gl_framebuffer_texture2d: unsafe extern "C" fn(GLenum, GLenum, GLenum, GLuint, GLint),
    gl_gen_renderbuffers: unsafe extern "C" fn(GLsizei, *mut GLuint),
    gl_delete_renderbuffers: unsafe extern "C" fn(GLsizei, *const GLuint),
    gl_bind_renderbuffer: unsafe extern "C" fn(GLenum, GLuint),
    gl_renderbuffer_storage: unsafe extern "C" fn(GLenum, GLenum, GLsizei, GLsizei),
    gl_framebuffer_renderbuffer: unsafe extern "C" fn(GLenum, GLenum, GLenum, GLuint),
    gl_check_framebuffer_status: unsafe extern "C" fn(GLenum) -> GLenum,
    gl_read_pixels:
        unsafe extern "C" fn(GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, *mut c_void),
}

impl Gles3Functions {
    fn load(get_proc: Option<GlGetProcAddrFn>) -> Result<Self, String> {
        let lib_gles = open_glesv2_library();
        if get_proc.is_none() && lib_gles.is_null() {
            return Err("failed to open libGLESv2.so.2 and no get_proc_address supplied".to_owned());
        }

        let load_req = |name: &CStr| -> Result<*mut c_void, String> {
            resolve_gl_proc(get_proc, lib_gles, name)
                .ok_or_else(|| format!("missing GLES 3.0 symbol {}", name.to_string_lossy()))
        };

        macro_rules! sym {
            ($name:expr) => {{
                let ptr = load_req($name)?;
                unsafe { std::mem::transmute_copy(&ptr) }
            }};
        }
        macro_rules! opt_sym {
            ($name:expr) => {{
                resolve_gl_proc(get_proc, lib_gles, $name)
                    .map(|ptr| unsafe { std::mem::transmute_copy(&ptr) })
            }};
        }

        Ok(Self {
            lib_gles,
            gl_get_error: sym!(c"glGetError"),
            gl_get_integerv: sym!(c"glGetIntegerv"),
            gl_get_string: sym!(c"glGetString"),
            gl_get_stringi: opt_sym!(c"glGetStringi"),
            gl_enable: sym!(c"glEnable"),
            gl_disable: sym!(c"glDisable"),
            gl_viewport: sym!(c"glViewport"),
            gl_scissor: sym!(c"glScissor"),
            gl_clear: sym!(c"glClear"),
            gl_clear_color: sym!(c"glClearColor"),
            gl_clear_depthf: sym!(c"glClearDepthf"),
            gl_clear_stencil: sym!(c"glClearStencil"),
            gl_depth_func: sym!(c"glDepthFunc"),
            gl_depth_mask: sym!(c"glDepthMask"),
            gl_cull_face: sym!(c"glCullFace"),
            gl_front_face: sym!(c"glFrontFace"),
            gl_blend_func_separate: sym!(c"glBlendFuncSeparate"),
            gl_blend_equation_separate: sym!(c"glBlendEquationSeparate"),
            gl_color_mask: sym!(c"glColorMask"),
            gl_stencil_func: sym!(c"glStencilFunc"),
            gl_stencil_op: sym!(c"glStencilOp"),
            gl_stencil_mask: sym!(c"glStencilMask"),
            gl_pixel_storei: sym!(c"glPixelStorei"),
            gl_create_shader: sym!(c"glCreateShader"),
            gl_shader_source: sym!(c"glShaderSource"),
            gl_compile_shader: sym!(c"glCompileShader"),
            gl_get_shaderiv: sym!(c"glGetShaderiv"),
            gl_get_shader_info_log: sym!(c"glGetShaderInfoLog"),
            gl_delete_shader: sym!(c"glDeleteShader"),
            gl_create_program: sym!(c"glCreateProgram"),
            gl_attach_shader: sym!(c"glAttachShader"),
            gl_link_program: sym!(c"glLinkProgram"),
            gl_get_programiv: sym!(c"glGetProgramiv"),
            gl_get_program_info_log: sym!(c"glGetProgramInfoLog"),
            gl_delete_program: sym!(c"glDeleteProgram"),
            gl_use_program: sym!(c"glUseProgram"),
            gl_program_parameteri: opt_sym!(c"glProgramParameteri"),
            gl_get_program_binary: opt_sym!(c"glGetProgramBinary"),
            gl_program_binary: opt_sym!(c"glProgramBinary"),
            gl_get_uniform_location: sym!(c"glGetUniformLocation"),
            gl_uniform1i: sym!(c"glUniform1i"),
            gl_uniform1f: sym!(c"glUniform1f"),
            gl_uniform4f: sym!(c"glUniform4f"),
            gl_uniform4fv: sym!(c"glUniform4fv"),
            gl_uniform_matrix4fv: sym!(c"glUniformMatrix4fv"),
            gl_gen_buffers: sym!(c"glGenBuffers"),
            gl_delete_buffers: sym!(c"glDeleteBuffers"),
            gl_bind_buffer: sym!(c"glBindBuffer"),
            gl_buffer_data: sym!(c"glBufferData"),
            gl_buffer_sub_data: sym!(c"glBufferSubData"),
            gl_gen_vertex_arrays: sym!(c"glGenVertexArrays"),
            gl_delete_vertex_arrays: sym!(c"glDeleteVertexArrays"),
            gl_bind_vertex_array: sym!(c"glBindVertexArray"),
            gl_enable_vertex_attrib_array: sym!(c"glEnableVertexAttribArray"),
            gl_disable_vertex_attrib_array: sym!(c"glDisableVertexAttribArray"),
            gl_vertex_attrib_pointer: sym!(c"glVertexAttribPointer"),
            gl_draw_arrays: sym!(c"glDrawArrays"),
            gl_draw_elements: sym!(c"glDrawElements"),
            gl_gen_textures: sym!(c"glGenTextures"),
            gl_delete_textures: sym!(c"glDeleteTextures"),
            gl_active_texture: sym!(c"glActiveTexture"),
            gl_bind_texture: sym!(c"glBindTexture"),
            gl_tex_image2d: sym!(c"glTexImage2D"),
            gl_tex_sub_image2d: sym!(c"glTexSubImage2D"),
            gl_compressed_tex_image2d: sym!(c"glCompressedTexImage2D"),
            gl_compressed_tex_sub_image2d: sym!(c"glCompressedTexSubImage2D"),
            gl_tex_parameteri: sym!(c"glTexParameteri"),
            gl_gen_framebuffers: sym!(c"glGenFramebuffers"),
            gl_delete_framebuffers: sym!(c"glDeleteFramebuffers"),
            gl_bind_framebuffer: sym!(c"glBindFramebuffer"),
            gl_framebuffer_texture2d: sym!(c"glFramebufferTexture2D"),
            gl_gen_renderbuffers: sym!(c"glGenRenderbuffers"),
            gl_delete_renderbuffers: sym!(c"glDeleteRenderbuffers"),
            gl_bind_renderbuffer: sym!(c"glBindRenderbuffer"),
            gl_renderbuffer_storage: sym!(c"glRenderbufferStorage"),
            gl_framebuffer_renderbuffer: sym!(c"glFramebufferRenderbuffer"),
            gl_check_framebuffer_status: sym!(c"glCheckFramebufferStatus"),
            gl_read_pixels: sym!(c"glReadPixels"),
        })
    }
}

impl Drop for Gles3Functions {
    fn drop(&mut self) {
        if !self.lib_gles.is_null() {
            unsafe { libc::dlclose(self.lib_gles) };
        }
    }
}

unsafe impl Send for Gles3Functions {}

fn open_glesv2_library() -> *mut c_void {
    for candidate in [c"libGLESv2.so.2", c"libGLESv2.so"] {
        let handle = unsafe { libc::dlopen(candidate.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if !handle.is_null() {
            return handle;
        }
    }
    std::ptr::null_mut()
}

fn resolve_gl_proc(
    get_proc: Option<GlGetProcAddrFn>,
    lib_gles: *mut c_void,
    name: &CStr,
) -> Option<*mut c_void> {
    if let Some(loader) = get_proc {
        let ptr = unsafe { loader(name.as_ptr()) };
        if !ptr.is_null() {
            return Some(ptr);
        }
    }
    if !lib_gles.is_null() {
        let ptr = unsafe { libc::dlsym(lib_gles, name.as_ptr()) };
        if !ptr.is_null() {
            return Some(ptr);
        }
    }
    None
}

struct GlTexture {
    id: GLuint,
    max_level: u32,
    levels: HashMap<u32, (u32, u32, GLenum)>,
    wrap_s: GLenum,
    wrap_t: GLenum,
    mag_filter: GLenum,
    min_filter: GLenum,
}

struct GlBuffer {
    id: GLuint,
    data: Vec<u8>,
    fvf: u32,
    format: u32,
    gl_usage: GLenum,
}

struct GlStateCache {
    current_program: Option<GLuint>,
    active_tex_unit: Option<u32>,
    bound_textures: [Option<GLuint>; MAX_FFP_STAGES],
}

impl Default for GlStateCache {
    fn default() -> Self {
        Self {
            current_program: None,
            active_tex_unit: None,
            bound_textures: [None; MAX_FFP_STAGES],
        }
    }
}

impl GlStateCache {
    fn invalidate_all(&mut self) {
        *self = Self::default();
    }

    fn use_program(&mut self, gl: &Gles3Functions, program: GLuint) {
        if self.current_program != Some(program) {
            unsafe {
                (gl.gl_use_program)(program);
            }
            self.current_program = Some(program);
        }
    }

    fn set_active_texture(&mut self, gl: &Gles3Functions, stage: u32) {
        if self.active_tex_unit != Some(stage) {
            unsafe {
                (gl.gl_active_texture)(GL_TEXTURE0 + stage);
            }
            self.active_tex_unit = Some(stage);
        }
    }

    fn bind_texture(&mut self, gl: &Gles3Functions, stage: u32, tex_id: GLuint) {
        let idx = stage as usize;
        if self.bound_textures[idx] == Some(tex_id) {
            return;
        }
        self.set_active_texture(gl, stage);
        unsafe {
            (gl.gl_bind_texture)(GL_TEXTURE_2D, tex_id);
        }
        self.bound_textures[idx] = Some(tex_id);
    }

    fn bind_texture_active(&mut self, gl: &Gles3Functions, stage: u32, tex_id: GLuint) {
        self.set_active_texture(gl, stage);
        let idx = stage as usize;
        if self.bound_textures[idx] != Some(tex_id) {
            unsafe {
                (gl.gl_bind_texture)(GL_TEXTURE_2D, tex_id);
            }
            self.bound_textures[idx] = Some(tex_id);
        }
    }

    fn invalidate_texture(&mut self, tex_id: GLuint) {
        for slot in &mut self.bound_textures {
            if *slot == Some(tex_id) {
                *slot = None;
            }
        }
    }
}

struct CompiledProgram {
    program: GLuint,
    u_world: GLint,
    u_view: GLint,
    u_inv_view: GLint,
    u_proj: GLint,
    u_tex_mats: [GLint; MAX_FFP_STAGES],
    u_viewport: GLint,
    u_y_flip: GLint,
    u_ambient: GLint,
    u_mat_diffuse: GLint,
    u_mat_ambient: GLint,
    u_mat_specular: GLint,
    u_mat_emissive: GLint,
    u_mat_power: GLint,
    u_fog_params: GLint,
    u_fog_color: GLint,
    u_alpha_ref: GLint,
    u_tfactor: GLint,
    u_tex: [GLint; MAX_FFP_STAGES],
    u_light_pos: [GLint; MAX_FFP_LIGHTS],
    u_light_dir: [GLint; MAX_FFP_LIGHTS],
    u_light_diffuse: [GLint; MAX_FFP_LIGHTS],
    u_light_ambient: [GLint; MAX_FFP_LIGHTS],
    u_light_specular: [GLint; MAX_FFP_LIGHTS],
    u_light_params: [GLint; MAX_FFP_LIGHTS],
    u_light_atten: [GLint; MAX_FFP_LIGHTS],
    u_vc: GLint,
    u_pc: GLint,
}

struct ProgramEntry {
    recipe: ShaderRecipe,
    compiled: CompiledProgram,
}

pub struct Gles3Backend {
    gl: Gles3Functions,
    present_target: Box<dyn PresentTarget>,
    driver_id: String,
    max_tex_size: u32,
    s3tc_supported: bool,
    fbo: GLuint,
    fbo_color_tex: GLuint,
    fbo_depth_rbo: GLuint,
    fbo_width: u32,
    fbo_height: u32,
    present_program: GLuint,
    present_tex_loc: GLint,
    present_vao: GLuint,
    draw_vao: GLuint,
    up_vbo: GLuint,
    up_ibo: GLuint,
    textures: HashMap<u32, GlTexture>,
    vbos: HashMap<u32, GlBuffer>,
    ibos: HashMap<u32, GlBuffer>,
    programs: Vec<ProgramEntry>,
    program_buckets: HashMap<u64, Vec<usize>>,
    state_cache: GlStateCache,
    last_view: Option<[f32; 16]>,
    last_inv_view: [f32; 16],
    recipe_cache: ShaderRecipeCache,
    recipe_cache_path: Option<PathBuf>,
    cache_dirty: bool,
    last_cache_change: Instant,
    last_cache_save: Instant,
    aspect_mode: AspectMode,
    pub draw_calls: u64,
    pub present_calls: u64,
}

impl Gles3Backend {
    pub fn new(
        get_proc: Option<GlGetProcAddrFn>,
        present_target: Box<dyn PresentTarget>,
        recipe_cache_path: Option<PathBuf>,
    ) -> Result<Self, String> {
        let gl = Gles3Functions::load(get_proc)?;
        let driver_id = query_driver_id(&gl);
        let max_tex_size = query_max_texture_size(&gl);
        let s3tc_supported = query_s3tc_support(&gl);

        let present_program = compile_and_link_program(&gl, PRESENT_VS_GLSL, PRESENT_FS_GLSL, false)?;
        let present_tex_loc = get_uniform_loc(&gl, present_program, "u_tex0");

        let mut present_vao = 0;
        let mut draw_vao = 0;
        let mut up_vbo = 0;
        let mut up_ibo = 0;
        unsafe {
            (gl.gl_gen_vertex_arrays)(1, &mut present_vao);
            (gl.gl_gen_vertex_arrays)(1, &mut draw_vao);
            (gl.gl_gen_buffers)(1, &mut up_vbo);
            (gl.gl_gen_buffers)(1, &mut up_ibo);
        }

        let now = Instant::now();
        let mut recipe_cache = ShaderRecipeCache::new();
        recipe_cache.set_driver_id(driver_id.clone());

        let mut backend = Self {
            gl,
            present_target,
            driver_id,
            max_tex_size,
            s3tc_supported,
            fbo: 0,
            fbo_color_tex: 0,
            fbo_depth_rbo: 0,
            fbo_width: 0,
            fbo_height: 0,
            present_program,
            present_tex_loc,
            present_vao,
            draw_vao,
            up_vbo,
            up_ibo,
            textures: HashMap::new(),
            vbos: HashMap::new(),
            ibos: HashMap::new(),
            programs: Vec::new(),
            program_buckets: HashMap::new(),
            state_cache: GlStateCache::default(),
            last_view: None,
            last_inv_view: IDENTITY_4X4,
            recipe_cache,
            recipe_cache_path: recipe_cache_path.clone(),
            cache_dirty: false,
            last_cache_change: now,
            last_cache_save: now,
            aspect_mode: AspectMode::default(),
            draw_calls: 0,
            present_calls: 0,
        };

        if let Some(path) = recipe_cache_path.as_deref() {
            if path.exists() {
                let _ = backend.load_recipe_cache(path);
            }
        }

        Ok(backend)
    }

    pub fn set_aspect_mode(&mut self, mode: AspectMode) {
        self.aspect_mode = mode;
    }

    pub fn aspect_mode(&self) -> AspectMode {
        self.aspect_mode
    }

    pub fn load_recipe_cache(&mut self, path: &Path) -> Result<usize, String> {
        let mut loaded = ShaderRecipeCache::load_from_file(path)?;
        let driver_matched = loaded.validate_driver_id(&self.driver_id);
        self.recipe_cache = loaded;
        if !driver_matched {
            self.cache_dirty = true;
            self.last_cache_change = Instant::now();
        }
        let mut compiled = 0usize;
        let total = self.recipe_cache.len();
        for idx in 0..total {
            let recipe = self.recipe_cache.recipes()[idx].clone();
            let cached_bin = self.recipe_cache.binary_at(idx).cloned();
            if self
                .compile_and_register_program(recipe, cached_bin.as_ref())
                .is_ok()
            {
                compiled += 1;
            }
        }
        Ok(compiled)
    }

    pub fn save_recipe_cache(&self, path: &Path) -> Result<(), String> {
        self.recipe_cache.save_to_file(path)
    }

    fn flush_recipe_cache_if_needed(&mut self) {
        if !self.cache_dirty {
            return;
        }
        let Some(path) = self.recipe_cache_path.as_deref() else {
            return;
        };
        let now = Instant::now();
        if now.duration_since(self.last_cache_change) >= CACHE_FLUSH_INTERVAL
            && now.duration_since(self.last_cache_save) >= CACHE_FLUSH_INTERVAL
            && self.recipe_cache.save_to_file(path).is_ok()
        {
            self.cache_dirty = false;
            self.last_cache_save = now;
        }
    }

    pub fn readback_rgba8(&self) -> Option<(u32, u32, Vec<u8>)> {
        if self.fbo == 0 || self.fbo_width == 0 || self.fbo_height == 0 {
            return None;
        }
        let len = (self.fbo_width as usize) * (self.fbo_height as usize) * 4;
        let mut pixels = vec![0u8; len];
        unsafe {
            (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
            (self.gl.gl_pixel_storei)(GL_PACK_ALIGNMENT, 1);
            (self.gl.gl_read_pixels)(
                0,
                0,
                self.fbo_width as GLsizei,
                self.fbo_height as GLsizei,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                pixels.as_mut_ptr().cast(),
            );
        }
        Some((self.fbo_width, self.fbo_height, pixels))
    }

    pub fn last_gl_error(&self) -> GLenum {
        unsafe { (self.gl.gl_get_error)() }
    }

    fn ensure_fbo(&mut self, width: u32, height: u32) {
        let w = width.max(1);
        let h = height.max(1);
        if self.fbo != 0 && self.fbo_width == w && self.fbo_height == h {
            unsafe {
                (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
            }
            return;
        }

        self.destroy_fbo();

        let mut fbo = 0;
        let mut color_tex = 0;
        let mut depth_rbo = 0;
        unsafe {
            (self.gl.gl_gen_textures)(1, &mut color_tex);
        }
        self.state_cache.bind_texture_active(&self.gl, 0, color_tex);
        unsafe {
            (self.gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR as GLint);
            (self.gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR as GLint);
            (self.gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE as GLint);
            (self.gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE as GLint);
            (self.gl.gl_tex_image2d)(
                GL_TEXTURE_2D,
                0,
                GL_RGBA8 as GLint,
                w as GLsizei,
                h as GLsizei,
                0,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                std::ptr::null(),
            );
        }
        self.state_cache.bind_texture(&self.gl, 0, 0);
        unsafe {
            (self.gl.gl_gen_renderbuffers)(1, &mut depth_rbo);
            (self.gl.gl_bind_renderbuffer)(GL_RENDERBUFFER, depth_rbo);
            (self.gl.gl_renderbuffer_storage)(
                GL_RENDERBUFFER,
                GL_DEPTH24_STENCIL8,
                w as GLsizei,
                h as GLsizei,
            );
            (self.gl.gl_bind_renderbuffer)(GL_RENDERBUFFER, 0);

            (self.gl.gl_gen_framebuffers)(1, &mut fbo);
            (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, fbo);
            (self.gl.gl_framebuffer_texture2d)(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                color_tex,
                0,
            );
            (self.gl.gl_framebuffer_renderbuffer)(
                GL_FRAMEBUFFER,
                GL_DEPTH_STENCIL_ATTACHMENT,
                GL_RENDERBUFFER,
                depth_rbo,
            );

            let status = (self.gl.gl_check_framebuffer_status)(GL_FRAMEBUFFER);
            if status != GL_FRAMEBUFFER_COMPLETE {
                eprintln!("[gles3] offscreen FBO incomplete: 0x{status:04x}");
            }
            (self.gl.gl_viewport)(0, 0, w as GLsizei, h as GLsizei);
        }

        self.fbo = fbo;
        self.fbo_color_tex = color_tex;
        self.fbo_depth_rbo = depth_rbo;
        self.fbo_width = w;
        self.fbo_height = h;
    }

    fn destroy_fbo(&mut self) {
        unsafe {
            if self.fbo != 0 {
                (self.gl.gl_delete_framebuffers)(1, &self.fbo);
                self.fbo = 0;
            }
            if self.fbo_color_tex != 0 {
                self.state_cache.invalidate_texture(self.fbo_color_tex);
                (self.gl.gl_delete_textures)(1, &self.fbo_color_tex);
                self.fbo_color_tex = 0;
            }
            if self.fbo_depth_rbo != 0 {
                (self.gl.gl_delete_renderbuffers)(1, &self.fbo_depth_rbo);
                self.fbo_depth_rbo = 0;
            }
        }
    }

    fn find_program_ffp(&self, hash: u64, key: &FfpShaderKey) -> Option<usize> {
        let bucket = self.program_buckets.get(&hash)?;
        bucket.iter().copied().find(|&idx| {
            matches!(&self.programs[idx].recipe, ShaderRecipe::Ffp(k) if k == key)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn find_program_prog(
        &self,
        hash: u64,
        decl_tokens: &[u32],
        vs_tokens: &[u32],
        ps_tokens: &[u32],
        alpha_test_func: u8,
        fog_mode: u8,
        ffp_stages: &[TextureStageKey; MAX_FFP_STAGES],
        specular_enabled: bool,
    ) -> Option<usize> {
        let bucket = self.program_buckets.get(&hash)?;
        bucket.iter().copied().find(|&idx| {
            matches!(
                &self.programs[idx].recipe,
                ShaderRecipe::Programmable {
                    decl_tokens: d,
                    vs_tokens: v,
                    ps_tokens: p,
                    alpha_test_func: a,
                    fog_mode: f,
                    ffp_stages: s,
                    specular_enabled: sp,
                } if d.as_slice() == decl_tokens
                    && v.as_slice() == vs_tokens
                    && p.as_slice() == ps_tokens
                    && *a == alpha_test_func
                    && *f == fog_mode
                    && s == ffp_stages
                    && *sp == specular_enabled
            )
        })
    }

    fn compile_and_register_program(
        &mut self,
        recipe: ShaderRecipe,
        cached_bin: Option<&CachedProgramBinary>,
    ) -> Result<usize, String> {
        let hash = hash_recipe(&recipe);
        if let Some(bucket) = self.program_buckets.get(&hash) {
            if let Some(&existing) = bucket.iter().find(|&&i| self.programs[i].recipe == recipe) {
                return Ok(existing);
            }
        }

        let mut program_from_bin = None;
        if let Some(bin) = cached_bin {
            program_from_bin = load_program_from_binary(&self.gl, bin);
        }

        let (program, newly_compiled) = if let Some(prog) = program_from_bin {
            (prog, false)
        } else {
            let (vs_glsl, fs_glsl) = build_glsl_for_recipe(&recipe)?;
            let prog = compile_and_link_program(&self.gl, &vs_glsl, &fs_glsl, true)?;
            (prog, true)
        };

        let compiled = CompiledProgram::from_program(&self.gl, program);
        self.state_cache.use_program(&self.gl, program);
        unsafe {
            for (stage, &loc) in compiled.u_tex.iter().enumerate() {
                if loc >= 0 {
                    (self.gl.gl_uniform1i)(loc, stage as GLint);
                }
            }
        }

        let is_new_recipe = self.recipe_cache.insert(recipe.clone());
        if newly_compiled {
            if let Some(bin) = capture_program_binary(&self.gl, program) {
                self.recipe_cache.set_binary(&recipe, bin);
            }
            self.cache_dirty = true;
            self.last_cache_change = Instant::now();
        } else if is_new_recipe {
            self.cache_dirty = true;
            self.last_cache_change = Instant::now();
        }

        let idx = self.programs.len();
        self.programs.push(ProgramEntry { recipe, compiled });
        self.program_buckets.entry(hash).or_default().push(idx);
        Ok(idx)
    }

    fn bind_pipeline_and_uniforms(
        &mut self,
        state: &DeviceState,
        shaders: &ActiveShaders<'_>,
    ) -> Option<usize> {
        let ffp_key = FfpShaderKey::from_state(state);
        let prog_idx = if shaders.vs_tokens.is_some() || shaders.ps_tokens.is_some() {
            let decl_tokens = shaders.vs_decl_tokens.unwrap_or(&[]);
            let vs_tokens = shaders.vs_tokens.unwrap_or(&[]);
            let ps_tokens = shaders.ps_tokens.unwrap_or(&[]);
            let (ffp_stages, specular_enabled) = if shaders.ps_tokens.is_none() {
                (ffp_key.stages, ffp_key.specular_enabled)
            } else {
                ([TextureStageKey::default(); MAX_FFP_STAGES], false)
            };
            let hash = hash_prog_parts(
                decl_tokens,
                vs_tokens,
                ps_tokens,
                ffp_key.alpha_test_func,
                ffp_key.fog_mode,
                &ffp_stages,
                specular_enabled,
            );
            if let Some(idx) = self.find_program_prog(
                hash,
                decl_tokens,
                vs_tokens,
                ps_tokens,
                ffp_key.alpha_test_func,
                ffp_key.fog_mode,
                &ffp_stages,
                specular_enabled,
            ) {
                idx
            } else {
                let recipe = ShaderRecipe::Programmable {
                    decl_tokens: decl_tokens.to_vec(),
                    vs_tokens: vs_tokens.to_vec(),
                    ps_tokens: ps_tokens.to_vec(),
                    alpha_test_func: ffp_key.alpha_test_func,
                    fog_mode: ffp_key.fog_mode,
                    ffp_stages,
                    specular_enabled,
                };
                match self.compile_and_register_program(recipe, None) {
                    Ok(idx) => idx,
                    Err(err) => {
                        eprintln!("[gles3] shader compilation failed: {err}");
                        return None;
                    }
                }
            }
        } else {
            let hash = hash_ffp_key(&ffp_key);
            if let Some(idx) = self.find_program_ffp(hash, &ffp_key) {
                idx
            } else {
                let recipe = ShaderRecipe::Ffp(ffp_key);
                match self.compile_and_register_program(recipe, None) {
                    Ok(idx) => idx,
                    Err(err) => {
                        eprintln!("[gles3] shader compilation failed: {err}");
                        return None;
                    }
                }
            }
        };

        unsafe {
            (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
            apply_render_states(&self.gl, state, self.fbo_height);
        }
        bind_textures(&self.gl, &mut self.textures, &mut self.state_cache, state);
        let prog_id = self.programs[prog_idx].compiled.program;
        self.state_cache.use_program(&self.gl, prog_id);

        let view = state.get_transform(D3DTS_VIEW);
        let inv_view = if self.programs[prog_idx].compiled.u_inv_view >= 0 {
            if self.last_view.as_ref() != Some(&view) {
                self.last_inv_view = invert_4x4(&view);
                self.last_view = Some(view);
            }
            Some(&self.last_inv_view)
        } else {
            None
        };
        unsafe {
            upload_uniforms(
                &self.gl,
                &self.programs[prog_idx].compiled,
                state,
                &view,
                inv_view,
            );
        }
        Some(prog_idx)
    }

    fn setup_fvf_attributes(&self, fvf: u32, stride: u32, base_byte_offset: usize) {
        let mut offset = base_byte_offset;
        let pos_mask = fvf & D3DFVF_POSITION_MASK;
        let pos_floats = match pos_mask {
            D3DFVF_XYZ => 3,
            D3DFVF_XYZRHW => 4,
            b if (D3DFVF_XYZB1..=D3DFVF_XYZB5).contains(&b) => 3 + ((b - D3DFVF_XYZB1) / 2 + 1) as i32,
            _ => 0,
        };
        if pos_floats > 0 {
            enable_attrib(
                &self.gl,
                ATTR_POS,
                pos_floats.min(4),
                GL_FLOAT,
                GL_FALSE,
                stride,
                offset,
            );
            offset += (pos_floats as usize) * 4;
        }
        if (fvf & D3DFVF_NORMAL) != 0 {
            enable_attrib(&self.gl, ATTR_NORMAL, 3, GL_FLOAT, GL_FALSE, stride, offset);
            offset += 12;
        }
        if (fvf & D3DFVF_PSIZE) != 0 {
            offset += 4;
        }
        if (fvf & D3DFVF_DIFFUSE) != 0 {
            enable_attrib(
                &self.gl,
                ATTR_DIFFUSE,
                4,
                GL_UNSIGNED_BYTE,
                GL_TRUE,
                stride,
                offset,
            );
            offset += 4;
        }
        if (fvf & D3DFVF_SPECULAR) != 0 {
            enable_attrib(
                &self.gl,
                ATTR_SPECULAR,
                4,
                GL_UNSIGNED_BYTE,
                GL_TRUE,
                stride,
                offset,
            );
            offset += 4;
        }
        let tex_count = ((fvf & D3DFVF_TEXCOUNT_MASK) >> D3DFVF_TEXCOUNT_SHIFT).min(4);
        for t in 0..tex_count {
            let comps = fvf_texcoord_floats(fvf, t) as i32;
            enable_attrib(
                &self.gl,
                ATTR_TEX0 + t,
                comps,
                GL_FLOAT,
                GL_FALSE,
                stride,
                offset,
            );
            offset += (comps as usize) * 4;
        }
    }

    fn setup_decl_attributes(
        &self,
        decl: &[VertexDeclElement],
        state: &DeviceState,
        up_override: Option<(GLuint, u32)>,
        base_vertex_offset: usize,
    ) {
        for elem in decl {
            let (gl_id, stride) = if elem.stream == 0 {
                if let Some((up_vbo, up_stride)) = up_override {
                    (up_vbo, up_stride)
                } else if let Some(b) = state.streams[0] {
                    let Some(vb) = self.vbos.get(&b.vb_handle) else {
                        continue;
                    };
                    (vb.id, b.stride)
                } else {
                    continue;
                }
            } else if let Some(b) = state.streams.get(elem.stream as usize).and_then(|s| *s) {
                let Some(vb) = self.vbos.get(&b.vb_handle) else {
                    continue;
                };
                (vb.id, b.stride)
            } else {
                continue;
            };

            let (comps, gl_type, normalized) = match elem.data_type {
                D3DVSDT_FLOAT1 => (1, GL_FLOAT, GL_FALSE),
                D3DVSDT_FLOAT2 => (2, GL_FLOAT, GL_FALSE),
                D3DVSDT_FLOAT3 => (3, GL_FLOAT, GL_FALSE),
                D3DVSDT_FLOAT4 => (4, GL_FLOAT, GL_FALSE),
                D3DVSDT_D3DCOLOR => (4, GL_UNSIGNED_BYTE, GL_TRUE),
                D3DVSDT_UBYTE4 => (4, GL_UNSIGNED_BYTE, GL_FALSE),
                D3DVSDT_SHORT2 => (2, GL_SHORT, GL_FALSE),
                D3DVSDT_SHORT4 => (4, GL_SHORT, GL_FALSE),
                _ => continue,
            };

            unsafe {
                (self.gl.gl_bind_buffer)(GL_ARRAY_BUFFER, gl_id);
            }
            let offset =
                usize::from(elem.byte_offset) + base_vertex_offset.saturating_mul(stride as usize);
            enable_attrib(
                &self.gl,
                u32::from(elem.register),
                comps,
                gl_type,
                normalized,
                stride,
                offset,
            );
        }
    }

    fn bind_vertex_attributes(
        &self,
        state: &DeviceState,
        shaders: &ActiveShaders<'_>,
        up_override: Option<(GLuint, u32)>,
        base_vertex_offset: usize,
    ) -> bool {
        unsafe {
            (self.gl.gl_bind_vertex_array)(self.draw_vao);
            for attr in 0..16 {
                (self.gl.gl_disable_vertex_attrib_array)(attr);
            }
        }

        if let Some(decl) = shaders.vs_parsed_decl {
            self.setup_decl_attributes(decl, state, up_override, base_vertex_offset);
            return true;
        }

        let fvf = state.current_vertex_shader;
        if let Some((up_vbo, stride)) = up_override {
            unsafe {
                (self.gl.gl_bind_buffer)(GL_ARRAY_BUFFER, up_vbo);
            }
            let offset = base_vertex_offset.saturating_mul(stride as usize);
            self.setup_fvf_attributes(fvf, stride, offset);
            return true;
        }

        let Some(stream0) = state.streams[0] else {
            return false;
        };
        let Some(vb) = self.vbos.get(&stream0.vb_handle) else {
            return false;
        };
        let effective_fvf = if fvf != 0 { fvf } else { vb.fvf };
        let stride = stream0.stride.max(1);
        unsafe {
            (self.gl.gl_bind_buffer)(GL_ARRAY_BUFFER, vb.id);
        }
        let offset = base_vertex_offset.saturating_mul(stride as usize);
        self.setup_fvf_attributes(effective_fvf, stride, offset);
        true
    }
}

impl Drop for Gles3Backend {
    fn drop(&mut self) {
        if self.cache_dirty {
            if let Some(path) = self.recipe_cache_path.as_deref() {
                let _ = self.save_recipe_cache(path);
            }
        }
        self.destroy_fbo();
        unsafe {
            if self.present_program != 0 {
                (self.gl.gl_delete_program)(self.present_program);
            }
            if self.present_vao != 0 {
                (self.gl.gl_delete_vertex_arrays)(1, &self.present_vao);
            }
            if self.draw_vao != 0 {
                (self.gl.gl_delete_vertex_arrays)(1, &self.draw_vao);
            }
            if self.up_vbo != 0 {
                (self.gl.gl_delete_buffers)(1, &self.up_vbo);
            }
            if self.up_ibo != 0 {
                (self.gl.gl_delete_buffers)(1, &self.up_ibo);
            }
            for tex in self.textures.values() {
                (self.gl.gl_delete_textures)(1, &tex.id);
            }
            for buf in self.vbos.values().chain(self.ibos.values()) {
                (self.gl.gl_delete_buffers)(1, &buf.id);
            }
            for entry in &self.programs {
                (self.gl.gl_delete_program)(entry.compiled.program);
            }
        }
    }
}

impl RenderBackend for Gles3Backend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn max_texture_size(&self) -> u32 {
        self.max_tex_size
    }

    fn supports_s3tc(&self) -> bool {
        self.s3tc_supported
    }

    fn on_device_create(&mut self, width: u32, height: u32, _format: u32, _windowed: bool) {
        self.ensure_fbo(width, height);
    }

    fn on_device_reset(&mut self, width: u32, height: u32, _format: u32, _windowed: bool) {
        self.ensure_fbo(width, height);
    }

    fn on_clear(&mut self, flags: u32, color: u32, z: f32, stencil: u32, rects: &[D3dRect]) {
        if self.fbo == 0 {
            return;
        }
        let mut mask = 0u32;
        if (flags & D3DCLEAR_TARGET) != 0 {
            mask |= GL_COLOR_BUFFER_BIT;
        }
        if (flags & D3DCLEAR_ZBUFFER) != 0 {
            mask |= GL_DEPTH_BUFFER_BIT;
        }
        if (flags & D3DCLEAR_STENCIL) != 0 {
            mask |= GL_STENCIL_BUFFER_BIT;
        }
        if mask == 0 {
            return;
        }

        let [r, g, b, a] = d3dcolor_to_rgba_f32(color);
        unsafe {
            (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
            (self.gl.gl_color_mask)(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
            (self.gl.gl_depth_mask)(GL_TRUE);
            (self.gl.gl_stencil_mask)(0xFF);
            (self.gl.gl_clear_color)(r, g, b, a);
            (self.gl.gl_clear_depthf)(z);
            (self.gl.gl_clear_stencil)(stencil as GLint);

            if rects.is_empty() {
                (self.gl.gl_disable)(GL_SCISSOR_TEST);
                (self.gl.gl_clear)(mask);
            } else {
                (self.gl.gl_enable)(GL_SCISSOR_TEST);
                for rect in rects {
                    let x = rect.left.max(0);
                    let w = (rect.right - rect.left).max(0);
                    let h = (rect.bottom - rect.top).max(0);
                    let y = rect.top.max(0);
                    (self.gl.gl_scissor)(x, y, w, h);
                    (self.gl.gl_clear)(mask);
                }
                (self.gl.gl_disable)(GL_SCISSOR_TEST);
            }
        }
    }

    fn on_texture_upload(&mut self, tex_handle: u32, level: u32, upload: &PreparedTextureUpload) {
        let mut created = false;
        let tex_id = self
            .textures
            .entry(tex_handle)
            .or_insert_with(|| {
                created = true;
                let mut id = 0;
                unsafe {
                    (self.gl.gl_gen_textures)(1, &mut id);
                }
                GlTexture {
                    id,
                    max_level: 0,
                    levels: HashMap::new(),
                    wrap_s: GL_REPEAT,
                    wrap_t: GL_REPEAT,
                    mag_filter: GL_NEAREST,
                    min_filter: GL_NEAREST,
                }
            })
            .id;

        self.state_cache.bind_texture_active(&self.gl, 0, tex_id);
        let entry = self.textures.get_mut(&tex_handle).unwrap();

        if created {
            unsafe {
                (self.gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_BASE_LEVEL, 0);
                (self.gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAX_LEVEL, 0);
                (self.gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_REPEAT as GLint);
                (self.gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_REPEAT as GLint);
                (self.gl.gl_tex_parameteri)(
                    GL_TEXTURE_2D,
                    GL_TEXTURE_MIN_FILTER,
                    GL_NEAREST as GLint,
                );
                (self.gl.gl_tex_parameteri)(
                    GL_TEXTURE_2D,
                    GL_TEXTURE_MAG_FILTER,
                    GL_NEAREST as GLint,
                );
            }
        }
        if level > entry.max_level {
            entry.max_level = level;
            unsafe {
                (self.gl.gl_tex_parameteri)(
                    GL_TEXTURE_2D,
                    GL_TEXTURE_MAX_LEVEL,
                    entry.max_level as GLint,
                );
            }
        }

        unsafe {
            (self.gl.gl_pixel_storei)(GL_UNPACK_ALIGNMENT, 1);

            match upload {
                PreparedTextureUpload::Rgba8(img) => {
                    let can_sub = entry.levels.get(&level)
                        == Some(&(img.width, img.height, GL_RGBA8));
                    if can_sub {
                        (self.gl.gl_tex_sub_image2d)(
                            GL_TEXTURE_2D,
                            level as GLint,
                            0,
                            0,
                            img.width as GLsizei,
                            img.height as GLsizei,
                            GL_RGBA,
                            GL_UNSIGNED_BYTE,
                            img.pixels.as_ptr().cast(),
                        );
                    } else {
                        (self.gl.gl_tex_image2d)(
                            GL_TEXTURE_2D,
                            level as GLint,
                            GL_RGBA8 as GLint,
                            img.width as GLsizei,
                            img.height as GLsizei,
                            0,
                            GL_RGBA,
                            GL_UNSIGNED_BYTE,
                            img.pixels.as_ptr().cast(),
                        );
                        entry
                            .levels
                            .insert(level, (img.width, img.height, GL_RGBA8));
                    }
                }
                PreparedTextureUpload::CompressedS3tc {
                    format,
                    width,
                    height,
                    data,
                } => {
                    let internal_fmt = match *format {
                        D3DFMT_DXT1 => GL_COMPRESSED_RGBA_S3TC_DXT1_EXT,
                        D3DFMT_DXT3 => GL_COMPRESSED_RGBA_S3TC_DXT3_EXT,
                        D3DFMT_DXT5 => GL_COMPRESSED_RGBA_S3TC_DXT5_EXT,
                        _ => GL_COMPRESSED_RGBA_S3TC_DXT5_EXT,
                    };
                    let can_sub =
                        entry.levels.get(&level) == Some(&(*width, *height, internal_fmt));
                    if can_sub {
                        (self.gl.gl_compressed_tex_sub_image2d)(
                            GL_TEXTURE_2D,
                            level as GLint,
                            0,
                            0,
                            *width as GLsizei,
                            *height as GLsizei,
                            internal_fmt,
                            data.len() as GLsizei,
                            data.as_ptr().cast(),
                        );
                    } else {
                        (self.gl.gl_compressed_tex_image2d)(
                            GL_TEXTURE_2D,
                            level as GLint,
                            internal_fmt,
                            *width as GLsizei,
                            *height as GLsizei,
                            0,
                            data.len() as GLsizei,
                            data.as_ptr().cast(),
                        );
                        entry.levels.insert(level, (*width, *height, internal_fmt));
                    }
                }
            }
        }
        self.state_cache.bind_texture(&self.gl, 0, 0);
    }

    fn on_texture_destroy(&mut self, tex_handle: u32) {
        if let Some(tex) = self.textures.remove(&tex_handle) {
            self.state_cache.invalidate_texture(tex.id);
            unsafe {
                (self.gl.gl_delete_textures)(1, &tex.id);
            }
        }
    }

    fn on_vb_upload(&mut self, vb_handle: u32, data: &[u8], fvf: u32, usage: u32) {
        let entry = self.vbos.entry(vb_handle).or_insert_with(|| {
            let mut id = 0;
            unsafe {
                (self.gl.gl_gen_buffers)(1, &mut id);
            }
            GlBuffer {
                id,
                data: Vec::new(),
                fvf,
                format: 0,
                gl_usage: 0,
            }
        });
        entry.fvf = fvf;
        upload_gl_buffer(&self.gl, entry, GL_ARRAY_BUFFER, data, usage);
    }

    fn on_vb_sub_upload(
        &mut self,
        vb_handle: u32,
        full_data: &[u8],
        offset: usize,
        sub_data: &[u8],
        fvf: u32,
        usage: u32,
    ) {
        if let Some(entry) = self.vbos.get_mut(&vb_handle) {
            entry.fvf = fvf;
            if upload_gl_buffer_sub(
                &self.gl,
                entry,
                GL_ARRAY_BUFFER,
                full_data.len(),
                offset,
                sub_data,
                usage,
            ) {
                return;
            }
        }
        self.on_vb_upload(vb_handle, full_data, fvf, usage);
    }

    fn on_vb_destroy(&mut self, vb_handle: u32) {
        if let Some(buf) = self.vbos.remove(&vb_handle) {
            unsafe {
                (self.gl.gl_delete_buffers)(1, &buf.id);
            }
        }
    }

    fn on_ib_upload(&mut self, ib_handle: u32, data: &[u8], format: u32, usage: u32) {
        let entry = self.ibos.entry(ib_handle).or_insert_with(|| {
            let mut id = 0;
            unsafe {
                (self.gl.gl_gen_buffers)(1, &mut id);
            }
            GlBuffer {
                id,
                data: Vec::new(),
                fvf: 0,
                format,
                gl_usage: 0,
            }
        });
        entry.format = format;
        upload_gl_buffer(&self.gl, entry, GL_ELEMENT_ARRAY_BUFFER, data, usage);
    }

    fn on_ib_sub_upload(
        &mut self,
        ib_handle: u32,
        full_data: &[u8],
        offset: usize,
        sub_data: &[u8],
        format: u32,
        usage: u32,
    ) {
        if let Some(entry) = self.ibos.get_mut(&ib_handle) {
            entry.format = format;
            if upload_gl_buffer_sub(
                &self.gl,
                entry,
                GL_ELEMENT_ARRAY_BUFFER,
                full_data.len(),
                offset,
                sub_data,
                usage,
            ) {
                return;
            }
        }
        self.on_ib_upload(ib_handle, full_data, format, usage);
    }

    fn on_ib_destroy(&mut self, ib_handle: u32) {
        if let Some(buf) = self.ibos.remove(&ib_handle) {
            unsafe {
                (self.gl.gl_delete_buffers)(1, &buf.id);
            }
        }
    }

    fn on_draw_primitive(
        &mut self,
        state: &DeviceState,
        shaders: &ActiveShaders<'_>,
        prim_type: u32,
        start_vertex: u32,
        prim_count: u32,
    ) {
        let Some(gl_mode) = map_primitive_type(prim_type) else {
            return;
        };
        let vtx_count = vertex_count_for_primitives(prim_type, prim_count);
        if vtx_count == 0 || self.fbo == 0 {
            return;
        }
        if self.bind_pipeline_and_uniforms(state, shaders).is_none() {
            return;
        }
        if !self.bind_vertex_attributes(state, shaders, None, 0) {
            return;
        }
        unsafe {
            (self.gl.gl_draw_arrays)(gl_mode, start_vertex as GLint, vtx_count as GLsizei);
        }
        self.draw_calls += 1;
    }

    fn on_draw_indexed_primitive(
        &mut self,
        state: &DeviceState,
        shaders: &ActiveShaders<'_>,
        prim_type: u32,
        _min_vertex_index: u32,
        _num_vertices: u32,
        start_index: u32,
        prim_count: u32,
    ) {
        let Some(gl_mode) = map_primitive_type(prim_type) else {
            return;
        };
        let idx_count = vertex_count_for_primitives(prim_type, prim_count);
        if idx_count == 0 || self.fbo == 0 {
            return;
        }
        let Some((ib_handle, base_vertex_idx)) = state.bound_indices else {
            return;
        };
        let Some((ib_id, ib_format, ib_len)) = self
            .ibos
            .get(&ib_handle)
            .map(|b| (b.id, b.format, b.data.len()))
        else {
            return;
        };
        if self.bind_pipeline_and_uniforms(state, shaders).is_none() {
            return;
        }

        let base_off = base_vertex_idx.max(0) as usize;
        if !self.bind_vertex_attributes(state, shaders, None, base_off) {
            return;
        }

        let (gl_idx_type, idx_elem_bytes) = if ib_format == D3DFMT_INDEX32 {
            (GL_UNSIGNED_INT, 4usize)
        } else {
            (GL_UNSIGNED_SHORT, 2usize)
        };
        let byte_offset = (start_index as usize) * idx_elem_bytes;
        if byte_offset + (idx_count as usize) * idx_elem_bytes > ib_len {
            return;
        }

        unsafe {
            (self.gl.gl_bind_buffer)(GL_ELEMENT_ARRAY_BUFFER, ib_id);
            (self.gl.gl_draw_elements)(
                gl_mode,
                idx_count as GLsizei,
                gl_idx_type,
                byte_offset as *const c_void,
            );
        }
        self.draw_calls += 1;
    }

    fn on_draw_primitive_up(
        &mut self,
        state: &DeviceState,
        shaders: &ActiveShaders<'_>,
        prim_type: u32,
        prim_count: u32,
        vertex_data: &[u8],
        stride: u32,
    ) {
        let Some(gl_mode) = map_primitive_type(prim_type) else {
            return;
        };
        let vtx_count = vertex_count_for_primitives(prim_type, prim_count);
        if vtx_count == 0 || vertex_data.is_empty() || self.fbo == 0 {
            return;
        }
        if self.bind_pipeline_and_uniforms(state, shaders).is_none() {
            return;
        }
        unsafe {
            (self.gl.gl_bind_buffer)(GL_ARRAY_BUFFER, self.up_vbo);
            (self.gl.gl_buffer_data)(
                GL_ARRAY_BUFFER,
                vertex_data.len() as GLsizeiptr,
                vertex_data.as_ptr().cast(),
                GL_DYNAMIC_DRAW,
            );
        }
        if !self.bind_vertex_attributes(state, shaders, Some((self.up_vbo, stride)), 0) {
            return;
        }
        unsafe {
            (self.gl.gl_draw_arrays)(gl_mode, 0, vtx_count as GLsizei);
        }
        self.draw_calls += 1;
    }

    fn on_draw_indexed_primitive_up(
        &mut self,
        state: &DeviceState,
        shaders: &ActiveShaders<'_>,
        prim_type: u32,
        _min_vertex_index: u32,
        _num_vertices: u32,
        prim_count: u32,
        index_data: &[u8],
        index_format: u32,
        vertex_data: &[u8],
        stride: u32,
    ) {
        let Some(gl_mode) = map_primitive_type(prim_type) else {
            return;
        };
        let idx_count = vertex_count_for_primitives(prim_type, prim_count);
        if idx_count == 0 || index_data.is_empty() || vertex_data.is_empty() || self.fbo == 0 {
            return;
        }
        if self.bind_pipeline_and_uniforms(state, shaders).is_none() {
            return;
        }
        unsafe {
            (self.gl.gl_bind_buffer)(GL_ARRAY_BUFFER, self.up_vbo);
            (self.gl.gl_buffer_data)(
                GL_ARRAY_BUFFER,
                vertex_data.len() as GLsizeiptr,
                vertex_data.as_ptr().cast(),
                GL_DYNAMIC_DRAW,
            );
        }
        if !self.bind_vertex_attributes(state, shaders, Some((self.up_vbo, stride)), 0) {
            return;
        }
        let gl_idx_type = if index_format == D3DFMT_INDEX32 {
            GL_UNSIGNED_INT
        } else {
            GL_UNSIGNED_SHORT
        };
        unsafe {
            (self.gl.gl_bind_buffer)(GL_ELEMENT_ARRAY_BUFFER, self.up_ibo);
            (self.gl.gl_buffer_data)(
                GL_ELEMENT_ARRAY_BUFFER,
                index_data.len() as GLsizeiptr,
                index_data.as_ptr().cast(),
                GL_DYNAMIC_DRAW,
            );
            (self.gl.gl_draw_elements)(gl_mode, idx_count as GLsizei, gl_idx_type, std::ptr::null());
        }
        self.draw_calls += 1;
    }

    fn on_present(&mut self) {
        self.present_calls += 1;
        if self.fbo == 0 || self.fbo_color_tex == 0 {
            return;
        }

        let (win_w, win_h) = self.present_target.drawable_size();
        let (target_w, target_h) = if win_w > 0 && win_h > 0 {
            (win_w, win_h)
        } else {
            (self.fbo_width.max(1), self.fbo_height.max(1))
        };

        let (vp_x, vp_y, vp_w, vp_h) = compute_aspect_viewport(
            self.fbo_width,
            self.fbo_height,
            target_w,
            target_h,
            self.aspect_mode,
        );

        unsafe {
            (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, 0);
            (self.gl.gl_disable)(GL_DEPTH_TEST);
            (self.gl.gl_disable)(GL_CULL_FACE);
            (self.gl.gl_disable)(GL_BLEND);
            (self.gl.gl_disable)(GL_STENCIL_TEST);
            (self.gl.gl_disable)(GL_SCISSOR_TEST);
            (self.gl.gl_color_mask)(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
            (self.gl.gl_viewport)(0, 0, target_w as GLsizei, target_h as GLsizei);
            (self.gl.gl_clear_color)(0.0, 0.0, 0.0, 1.0);
            (self.gl.gl_clear)(GL_COLOR_BUFFER_BIT);

            (self.gl.gl_viewport)(vp_x, vp_y, vp_w, vp_h);
        }
        self.state_cache.use_program(&self.gl, self.present_program);
        self.state_cache.bind_texture(&self.gl, 0, self.fbo_color_tex);
        unsafe {
            if self.present_tex_loc >= 0 {
                (self.gl.gl_uniform1i)(self.present_tex_loc, 0);
            }
            (self.gl.gl_bind_vertex_array)(self.present_vao);
            (self.gl.gl_draw_arrays)(GL_TRIANGLE_STRIP, 0, 4);
        }

        self.present_target.swap_buffers();
        self.state_cache.invalidate_all();

        unsafe {
            (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
            (self.gl.gl_viewport)(0, 0, self.fbo_width as GLsizei, self.fbo_height as GLsizei);
        }

        self.flush_recipe_cache_if_needed();
    }
}

pub fn compute_aspect_viewport(
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    mode: AspectMode,
) -> (GLint, GLint, GLsizei, GLsizei) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return (0, 0, dst_w as GLsizei, dst_h as GLsizei);
    }
    let target_aspect = match mode {
        AspectMode::Stretch => return (0, 0, dst_w as GLsizei, dst_h as GLsizei),
        AspectMode::Ratio4x3 => ASPECT_RATIO_4_3,
        AspectMode::Ratio16x9 => ASPECT_RATIO_16_9,
    };
    let dst_aspect = f64::from(dst_w) / f64::from(dst_h);
    if (target_aspect - dst_aspect).abs() < ASPECT_EPSILON {
        (0, 0, dst_w as GLsizei, dst_h as GLsizei)
    } else if dst_aspect > target_aspect {
        let vp_h = dst_h as GLsizei;
        let vp_w = ((f64::from(dst_h) * target_aspect).round() as GLsizei).max(1);
        let vp_x = ((dst_w as GLsizei) - vp_w) / 2;
        (vp_x, 0, vp_w, vp_h)
    } else {
        let vp_w = dst_w as GLsizei;
        let vp_h = ((f64::from(dst_w) / target_aspect).round() as GLsizei).max(1);
        let vp_y = ((dst_h as GLsizei) - vp_h) / 2;
        (0, vp_y, vp_w, vp_h)
    }
}

fn upload_gl_buffer(
    gl: &Gles3Functions,
    entry: &mut GlBuffer,
    target: GLenum,
    data: &[u8],
    usage: u32,
) {
    let gl_usage = if (usage & D3DUSAGE_DYNAMIC) != 0 {
        GL_DYNAMIC_DRAW
    } else {
        GL_STATIC_DRAW
    };
    let can_sub_data =
        !data.is_empty() && entry.data.len() == data.len() && entry.gl_usage == gl_usage;
    entry.data.clear();
    entry.data.extend_from_slice(data);
    entry.gl_usage = gl_usage;
    unsafe {
        (gl.gl_bind_buffer)(target, entry.id);
        if can_sub_data {
            (gl.gl_buffer_sub_data)(target, 0, data.len() as GLsizeiptr, data.as_ptr().cast());
        } else {
            (gl.gl_buffer_data)(target, data.len() as GLsizeiptr, data.as_ptr().cast(), gl_usage);
        }
        (gl.gl_bind_buffer)(target, 0);
    }
}

fn upload_gl_buffer_sub(
    gl: &Gles3Functions,
    entry: &mut GlBuffer,
    target: GLenum,
    full_len: usize,
    offset: usize,
    sub_data: &[u8],
    usage: u32,
) -> bool {
    let gl_usage = if (usage & D3DUSAGE_DYNAMIC) != 0 {
        GL_DYNAMIC_DRAW
    } else {
        GL_STATIC_DRAW
    };
    if sub_data.is_empty()
        || entry.data.len() != full_len
        || entry.gl_usage != gl_usage
        || offset.saturating_add(sub_data.len()) > entry.data.len()
    {
        return false;
    }
    entry.data[offset..offset + sub_data.len()].copy_from_slice(sub_data);
    unsafe {
        (gl.gl_bind_buffer)(target, entry.id);
        (gl.gl_buffer_sub_data)(
            target,
            offset as GLintptr,
            sub_data.len() as GLsizeiptr,
            sub_data.as_ptr().cast(),
        );
        (gl.gl_bind_buffer)(target, 0);
    }
    true
}

fn build_glsl_for_recipe(recipe: &ShaderRecipe) -> Result<(String, String), String> {
    match recipe {
        ShaderRecipe::Ffp(key) => {
            let pair = generate_ffp_shaders(key);
            Ok((pair.vertex_glsl, pair.fragment_glsl))
        }
        ShaderRecipe::Programmable {
            decl_tokens,
            vs_tokens,
            ps_tokens,
            alpha_test_func,
            fog_mode,
            ffp_stages,
            specular_enabled,
        } => {
            let decl = if decl_tokens.is_empty() {
                Vec::new()
            } else {
                crate::shader_asm::parse_vertex_declaration(decl_tokens)?
            };
            let mut vs_glsl = if !vs_tokens.is_empty() {
                translate_vs_1_x(vs_tokens, &decl)?
            } else {
                let ffp = FfpShaderKey {
                    has_diffuse: decl.iter().any(|e| e.register == 5),
                    texcoord_count: decl.iter().filter(|e| e.register >= 7).count().min(4) as u8,
                    ..Default::default()
                };
                generate_ffp_shaders(&ffp).vertex_glsl
            };

            let fs_glsl = if !ps_tokens.is_empty() {
                if vs_tokens.is_empty() {
                    vs_glsl = adapt_ffp_vs_to_ps_inputs(&vs_glsl);
                }
                translate_ps_1_x(ps_tokens, *alpha_test_func, *fog_mode)?
            } else {
                let ffp = FfpShaderKey {
                    alpha_test_func: *alpha_test_func,
                    fog_mode: *fog_mode,
                    specular_enabled: *specular_enabled,
                    stages: *ffp_stages,
                    ..Default::default()
                };
                if !vs_tokens.is_empty() {
                    vs_glsl = adapt_asm_vs_to_ffp_inputs(&vs_glsl);
                }
                generate_ffp_shaders(&ffp).fragment_glsl
            };
            Ok((vs_glsl, fs_glsl))
        }
    }
}

fn adapt_asm_vs_to_ffp_inputs(vs_glsl: &str) -> String {
    vs_glsl
        .replace("v_oD0", "v_diffuse")
        .replace("v_oD1", "v_specular")
        .replace("v_oT0", "v_tex0")
        .replace("v_oT1", "v_tex1")
        .replace("v_oT2", "v_tex2")
        .replace("v_oT3", "v_tex3")
        .replace("v_oFog", "v_fog_factor")
}

fn adapt_ffp_vs_to_ps_inputs(vs_glsl: &str) -> String {
    vs_glsl
        .replace("v_diffuse", "v_oD0")
        .replace("v_specular", "v_oD1")
        .replace("v_tex0", "v_oT0")
        .replace("v_tex1", "v_oT1")
        .replace("v_tex2", "v_oT2")
        .replace("v_tex3", "v_oT3")
        .replace("v_fog_factor", "v_oFog")
}

fn query_driver_id(gl: &Gles3Functions) -> String {
    let read_str = |name: GLenum| -> String {
        let ptr = unsafe { (gl.gl_get_string)(name) };
        if ptr.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(ptr.cast()) }
                .to_string_lossy()
                .into_owned()
        }
    };
    format!("{}|{}", read_str(GL_RENDERER), read_str(GL_VERSION))
}

fn query_max_texture_size(gl: &Gles3Functions) -> u32 {
    let mut val: GLint = 2048;
    unsafe {
        (gl.gl_get_integerv)(GL_MAX_TEXTURE_SIZE, &mut val);
    }
    (val.max(256)) as u32
}

fn query_s3tc_support(gl: &Gles3Functions) -> bool {
    if let Some(get_stringi) = gl.gl_get_stringi {
        let mut count: GLint = 0;
        unsafe {
            (gl.gl_get_integerv)(GL_NUM_EXTENSIONS, &mut count);
        }
        for i in 0..count.max(0) as GLuint {
            let ptr = unsafe { get_stringi(GL_EXTENSIONS, i) };
            if !ptr.is_null() {
                let ext = unsafe { CStr::from_ptr(ptr.cast()) }.to_string_lossy();
                if ext == "GL_EXT_texture_compression_s3tc"
                    || ext == "GL_ANGLE_texture_compression_dxt5"
                {
                    return true;
                }
            }
        }
    }
    let ptr = unsafe { (gl.gl_get_string)(GL_EXTENSIONS) };
    if !ptr.is_null() {
        let all = unsafe { CStr::from_ptr(ptr.cast()) }.to_string_lossy();
        return all.contains("GL_EXT_texture_compression_s3tc")
            || all.contains("GL_ANGLE_texture_compression_dxt5");
    }
    false
}

fn compile_shader(gl: &Gles3Functions, stage: GLenum, source: &str) -> Result<GLuint, String> {
    let shader = unsafe { (gl.gl_create_shader)(stage) };
    if shader == 0 {
        return Err("glCreateShader returned 0".to_owned());
    }
    let c_src = CString::new(source).map_err(|_| "shader source contains NUL".to_owned())?;
    let ptr = c_src.as_ptr();
    let len = source.len() as GLint;
    let mut status: GLint = 0;
    unsafe {
        (gl.gl_shader_source)(shader, 1, &ptr, &len);
        (gl.gl_compile_shader)(shader);
        (gl.gl_get_shaderiv)(shader, GL_COMPILE_STATUS, &mut status);
    }
    if status == 0 {
        let mut log_len: GLint = 0;
        unsafe { (gl.gl_get_shaderiv)(shader, GL_INFO_LOG_LENGTH, &mut log_len) };
        let mut buf = vec![0u8; log_len.max(1) as usize];
        let mut written: GLsizei = 0;
        unsafe {
            (gl.gl_get_shader_info_log)(
                shader,
                buf.len() as GLsizei,
                &mut written,
                buf.as_mut_ptr().cast(),
            );
            (gl.gl_delete_shader)(shader);
        }
        let msg = String::from_utf8_lossy(&buf[..written.max(0) as usize]).into_owned();
        return Err(format!("shader compile failed: {msg}\n---SOURCE---\n{source}"));
    }
    Ok(shader)
}

fn compile_and_link_program(
    gl: &Gles3Functions,
    vs_src: &str,
    fs_src: &str,
    binary_retrievable: bool,
) -> Result<GLuint, String> {
    let vs = compile_shader(gl, GL_VERTEX_SHADER, vs_src)?;
    let fs = match compile_shader(gl, GL_FRAGMENT_SHADER, fs_src) {
        Ok(s) => s,
        Err(err) => {
            unsafe { (gl.gl_delete_shader)(vs) };
            return Err(err);
        }
    };

    let program = unsafe { (gl.gl_create_program)() };
    if program == 0 {
        unsafe {
            (gl.gl_delete_shader)(vs);
            (gl.gl_delete_shader)(fs);
        }
        return Err("glCreateProgram returned 0".to_owned());
    }

    if binary_retrievable {
        if let Some(param_i) = gl.gl_program_parameteri {
            unsafe {
                param_i(program, GL_PROGRAM_BINARY_RETRIEVABLE_HINT, GL_TRUE as GLint);
            }
        }
    }

    let mut status: GLint = 0;
    unsafe {
        (gl.gl_attach_shader)(program, vs);
        (gl.gl_attach_shader)(program, fs);
        (gl.gl_link_program)(program);
        (gl.gl_get_programiv)(program, GL_LINK_STATUS, &mut status);
        (gl.gl_delete_shader)(vs);
        (gl.gl_delete_shader)(fs);
    }

    if status == 0 {
        let mut log_len: GLint = 0;
        unsafe { (gl.gl_get_programiv)(program, GL_INFO_LOG_LENGTH, &mut log_len) };
        let mut buf = vec![0u8; log_len.max(1) as usize];
        let mut written: GLsizei = 0;
        unsafe {
            (gl.gl_get_program_info_log)(
                program,
                buf.len() as GLsizei,
                &mut written,
                buf.as_mut_ptr().cast(),
            );
            (gl.gl_delete_program)(program);
        }
        let msg = String::from_utf8_lossy(&buf[..written.max(0) as usize]).into_owned();
        return Err(format!("program link failed: {msg}"));
    }
    Ok(program)
}

fn load_program_from_binary(gl: &Gles3Functions, bin: &CachedProgramBinary) -> Option<GLuint> {
    let prog_bin_fn = gl.gl_program_binary?;
    if bin.bytes.is_empty() {
        return None;
    }
    let program = unsafe { (gl.gl_create_program)() };
    if program == 0 {
        return None;
    }
    let mut status: GLint = 0;
    unsafe {
        prog_bin_fn(
            program,
            bin.format,
            bin.bytes.as_ptr().cast(),
            bin.bytes.len() as GLsizei,
        );
        (gl.gl_get_programiv)(program, GL_LINK_STATUS, &mut status);
    }
    if status == 0 {
        unsafe {
            (gl.gl_delete_program)(program);
        }
        None
    } else {
        Some(program)
    }
}

fn capture_program_binary(gl: &Gles3Functions, program: GLuint) -> Option<CachedProgramBinary> {
    let get_bin_fn = gl.gl_get_program_binary?;
    let mut bin_len: GLint = 0;
    unsafe {
        (gl.gl_get_programiv)(program, GL_PROGRAM_BINARY_LENGTH, &mut bin_len);
    }
    if bin_len <= 0 {
        return None;
    }
    let mut bytes = vec![0u8; bin_len as usize];
    let mut actual_len: GLsizei = 0;
    let mut format: GLenum = 0;
    unsafe {
        get_bin_fn(
            program,
            bin_len as GLsizei,
            &mut actual_len,
            &mut format,
            bytes.as_mut_ptr().cast(),
        );
    }
    if actual_len <= 0 {
        return None;
    }
    bytes.truncate((actual_len as usize).min(bytes.len()));
    Some(CachedProgramBinary { format, bytes })
}

fn get_uniform_loc(gl: &Gles3Functions, program: GLuint, name: &str) -> GLint {
    let Ok(c_name) = CString::new(name) else {
        return -1;
    };
    unsafe { (gl.gl_get_uniform_location)(program, c_name.as_ptr()) }
}

impl CompiledProgram {
    fn from_program(gl: &Gles3Functions, program: GLuint) -> Self {
        let mut u_tex_mats = [-1; MAX_FFP_STAGES];
        let mut u_tex = [-1; MAX_FFP_STAGES];
        for i in 0..MAX_FFP_STAGES {
            u_tex_mats[i] = get_uniform_loc(gl, program, &format!("u_tex_mats[{i}]"));
            u_tex[i] = get_uniform_loc(gl, program, &format!("u_tex{i}"));
        }
        let mut u_light_pos = [-1; MAX_FFP_LIGHTS];
        let mut u_light_dir = [-1; MAX_FFP_LIGHTS];
        let mut u_light_diffuse = [-1; MAX_FFP_LIGHTS];
        let mut u_light_ambient = [-1; MAX_FFP_LIGHTS];
        let mut u_light_specular = [-1; MAX_FFP_LIGHTS];
        let mut u_light_params = [-1; MAX_FFP_LIGHTS];
        let mut u_light_atten = [-1; MAX_FFP_LIGHTS];
        for i in 0..MAX_FFP_LIGHTS {
            u_light_pos[i] = get_uniform_loc(gl, program, &format!("u_light_pos[{i}]"));
            u_light_dir[i] = get_uniform_loc(gl, program, &format!("u_light_dir[{i}]"));
            u_light_diffuse[i] = get_uniform_loc(gl, program, &format!("u_light_diffuse[{i}]"));
            u_light_ambient[i] = get_uniform_loc(gl, program, &format!("u_light_ambient[{i}]"));
            u_light_specular[i] = get_uniform_loc(gl, program, &format!("u_light_specular[{i}]"));
            u_light_params[i] = get_uniform_loc(gl, program, &format!("u_light_params[{i}]"));
            u_light_atten[i] = get_uniform_loc(gl, program, &format!("u_light_atten[{i}]"));
        }

        Self {
            program,
            u_world: get_uniform_loc(gl, program, "u_world"),
            u_view: get_uniform_loc(gl, program, "u_view"),
            u_inv_view: get_uniform_loc(gl, program, "u_inv_view"),
            u_proj: get_uniform_loc(gl, program, "u_proj"),
            u_tex_mats,
            u_viewport: get_uniform_loc(gl, program, "u_viewport"),
            u_y_flip: get_uniform_loc(gl, program, "u_y_flip"),
            u_ambient: get_uniform_loc(gl, program, "u_ambient"),
            u_mat_diffuse: get_uniform_loc(gl, program, "u_mat_diffuse"),
            u_mat_ambient: get_uniform_loc(gl, program, "u_mat_ambient"),
            u_mat_specular: get_uniform_loc(gl, program, "u_mat_specular"),
            u_mat_emissive: get_uniform_loc(gl, program, "u_mat_emissive"),
            u_mat_power: get_uniform_loc(gl, program, "u_mat_power"),
            u_fog_params: get_uniform_loc(gl, program, "u_fog_params"),
            u_fog_color: get_uniform_loc(gl, program, "u_fog_color"),
            u_alpha_ref: get_uniform_loc(gl, program, "u_alpha_ref"),
            u_tfactor: get_uniform_loc(gl, program, "u_tfactor"),
            u_tex,
            u_light_pos,
            u_light_dir,
            u_light_diffuse,
            u_light_ambient,
            u_light_specular,
            u_light_params,
            u_light_atten,
            u_vc: get_uniform_loc(gl, program, "u_vc"),
            u_pc: get_uniform_loc(gl, program, "u_pc"),
        }
    }
}

unsafe fn apply_render_states(gl: &Gles3Functions, state: &DeviceState, fbo_height: u32) {
    let vp = &state.viewport;
    let vp_w = vp.width.max(1) as GLsizei;
    let vp_h = vp.height.max(1) as GLsizei;
    let _ = fbo_height;
    unsafe {
        (gl.gl_viewport)(vp.x as GLint, vp.y as GLint, vp_w, vp_h);
    }

    let z_enable = state.get_render_state(D3DRS_ZENABLE) != 0;
    let z_write = state.get_render_state(D3DRS_ZWRITEENABLE) != 0;
    let z_func = map_cmp_func(state.get_render_state(D3DRS_ZFUNC));
    unsafe {
        if z_enable {
            (gl.gl_enable)(GL_DEPTH_TEST);
        } else {
            (gl.gl_disable)(GL_DEPTH_TEST);
        }
        (gl.gl_depth_mask)(u8::from(z_write));
        (gl.gl_depth_func)(z_func);
    }

    let blend_enable = state.get_render_state(D3DRS_ALPHABLENDENABLE) != 0;
    let src_blend = map_blend_factor(state.get_render_state(D3DRS_SRCBLEND));
    let dst_blend = map_blend_factor(state.get_render_state(D3DRS_DESTBLEND));
    let blend_op = map_blend_op(state.get_render_state(D3DRS_BLENDOP));
    unsafe {
        if blend_enable {
            (gl.gl_enable)(GL_BLEND);
            (gl.gl_blend_func_separate)(src_blend, dst_blend, src_blend, dst_blend);
            (gl.gl_blend_equation_separate)(blend_op, blend_op);
        } else {
            (gl.gl_disable)(GL_BLEND);
        }
    }

    let cull_mode = state.get_render_state(D3DRS_CULLMODE);
    unsafe {
        (gl.gl_front_face)(GL_CCW);
        match cull_mode {
            D3DCULL_NONE => (gl.gl_disable)(GL_CULL_FACE),
            D3DCULL_CW => {
                (gl.gl_enable)(GL_CULL_FACE);
                (gl.gl_cull_face)(GL_FRONT);
            }
            D3DCULL_CCW => {
                (gl.gl_enable)(GL_CULL_FACE);
                (gl.gl_cull_face)(GL_BACK);
            }
            _ => {
                (gl.gl_enable)(GL_CULL_FACE);
                (gl.gl_cull_face)(GL_BACK);
            }
        }
    }

    let cw = state.get_render_state(D3DRS_COLORWRITEENABLE);
    unsafe {
        (gl.gl_color_mask)(
            u8::from((cw & 0x1) != 0),
            u8::from((cw & 0x2) != 0),
            u8::from((cw & 0x4) != 0),
            u8::from((cw & 0x8) != 0),
        );
    }

    let stencil_enable = state.get_render_state(D3DRS_STENCILENABLE) != 0;
    unsafe {
        if stencil_enable {
            (gl.gl_enable)(GL_STENCIL_TEST);
            (gl.gl_stencil_func)(
                map_cmp_func(state.get_render_state(D3DRS_STENCILFUNC)),
                state.get_render_state(D3DRS_STENCILREF) as GLint,
                state.get_render_state(D3DRS_STENCILMASK),
            );
            (gl.gl_stencil_op)(
                map_stencil_op(state.get_render_state(D3DRS_STENCILFAIL)),
                map_stencil_op(state.get_render_state(D3DRS_STENCILZFAIL)),
                map_stencil_op(state.get_render_state(D3DRS_STENCILPASS)),
            );
            (gl.gl_stencil_mask)(state.get_render_state(D3DRS_STENCILWRITEMASK));
        } else {
            (gl.gl_disable)(GL_STENCIL_TEST);
        }
    }
}

fn bind_textures(
    gl: &Gles3Functions,
    textures: &mut HashMap<u32, GlTexture>,
    state_cache: &mut GlStateCache,
    state: &DeviceState,
) {
    for stage in 0..MAX_FFP_STAGES {
        let s = stage as u32;
        let Some(tex_handle) = state.bound_textures[stage] else {
            state_cache.bind_texture(gl, s, 0);
            continue;
        };
        let Some(gl_tex) = textures.get_mut(&tex_handle) else {
            state_cache.bind_texture(gl, s, 0);
            continue;
        };
        state_cache.bind_texture(gl, s, gl_tex.id);

        let addr_u = map_tex_address(state.get_texture_stage_state(s, D3DTSS_ADDRESSU));
        let addr_v = map_tex_address(state.get_texture_stage_state(s, D3DTSS_ADDRESSV));
        let mag = map_mag_filter(state.get_texture_stage_state(s, D3DTSS_MAGFILTER));
        let min = map_min_filter(
            state.get_texture_stage_state(s, D3DTSS_MINFILTER),
            state.get_texture_stage_state(s, D3DTSS_MIPFILTER),
            gl_tex.max_level > 0,
        );
        if gl_tex.wrap_s != addr_u
            || gl_tex.wrap_t != addr_v
            || gl_tex.mag_filter != mag
            || gl_tex.min_filter != min
        {
            state_cache.set_active_texture(gl, s);
            unsafe {
                if gl_tex.wrap_s != addr_u {
                    (gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, addr_u as GLint);
                    gl_tex.wrap_s = addr_u;
                }
                if gl_tex.wrap_t != addr_v {
                    (gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, addr_v as GLint);
                    gl_tex.wrap_t = addr_v;
                }
                if gl_tex.mag_filter != mag {
                    (gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, mag as GLint);
                    gl_tex.mag_filter = mag;
                }
                if gl_tex.min_filter != min {
                    (gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, min as GLint);
                    gl_tex.min_filter = min;
                }
            }
        }
    }
}

unsafe fn upload_uniforms(
    gl: &Gles3Functions,
    prog: &CompiledProgram,
    state: &DeviceState,
    view: &[f32; 16],
    inv_view: Option<&[f32; 16]>,
) {
    let world = state.get_transform(D3DTS_WORLD);
    let proj = state.get_transform(D3DTS_PROJECTION);
    unsafe {
        if prog.u_world >= 0 {
            (gl.gl_uniform_matrix4fv)(prog.u_world, 1, GL_FALSE, world.as_ptr());
        }
        if prog.u_view >= 0 {
            (gl.gl_uniform_matrix4fv)(prog.u_view, 1, GL_FALSE, view.as_ptr());
        }
        if let Some(inv) = inv_view {
            if prog.u_inv_view >= 0 {
                (gl.gl_uniform_matrix4fv)(prog.u_inv_view, 1, GL_FALSE, inv.as_ptr());
            }
        }
        if prog.u_proj >= 0 {
            (gl.gl_uniform_matrix4fv)(prog.u_proj, 1, GL_FALSE, proj.as_ptr());
        }
        for (i, &loc) in prog.u_tex_mats.iter().enumerate() {
            if loc >= 0 {
                let mat = state.get_transform(D3DTS_TEXTURE0 + i as u32);
                (gl.gl_uniform_matrix4fv)(loc, 1, GL_FALSE, mat.as_ptr());
            }
        }
        if prog.u_viewport >= 0 {
            (gl.gl_uniform4f)(
                prog.u_viewport,
                state.viewport.x as f32,
                state.viewport.y as f32,
                state.viewport.width.max(1) as f32,
                state.viewport.height.max(1) as f32,
            );
        }
        if prog.u_y_flip >= 0 {
            (gl.gl_uniform1f)(prog.u_y_flip, -1.0);
        }
        if prog.u_ambient >= 0 {
            let [r, g, b, a] = d3dcolor_to_rgba_f32(state.get_render_state(D3DRS_AMBIENT));
            (gl.gl_uniform4f)(prog.u_ambient, r, g, b, a);
        }
        set_vec4(gl, prog.u_mat_diffuse, state.material.diffuse);
        set_vec4(gl, prog.u_mat_ambient, state.material.ambient);
        set_vec4(gl, prog.u_mat_specular, state.material.specular);
        set_vec4(gl, prog.u_mat_emissive, state.material.emissive);
        if prog.u_mat_power >= 0 {
            (gl.gl_uniform1f)(prog.u_mat_power, state.material.power);
        }
        if prog.u_fog_params >= 0 {
            let start = f32::from_bits(state.get_render_state(D3DRS_FOGSTART));
            let end = f32::from_bits(state.get_render_state(D3DRS_FOGEND));
            let density = f32::from_bits(state.get_render_state(D3DRS_FOGDENSITY));
            (gl.gl_uniform4f)(prog.u_fog_params, start, end, density, 0.0);
        }
        if prog.u_fog_color >= 0 {
            let [r, g, b, a] = d3dcolor_to_rgba_f32(state.get_render_state(D3DRS_FOGCOLOR));
            (gl.gl_uniform4f)(prog.u_fog_color, r, g, b, a);
        }
        if prog.u_alpha_ref >= 0 {
            let aref = (state.get_render_state(D3DRS_ALPHAREF) & 0xFF) as f32 / 255.0;
            (gl.gl_uniform1f)(prog.u_alpha_ref, aref);
        }
        if prog.u_tfactor >= 0 {
            let [r, g, b, a] = d3dcolor_to_rgba_f32(state.get_render_state(D3DRS_TEXTUREFACTOR));
            (gl.gl_uniform4f)(prog.u_tfactor, r, g, b, a);
        }

        for (i, slot) in state.lights.iter().take(MAX_FFP_LIGHTS).enumerate() {
            if !slot.enabled {
                continue;
            }
            let l = &slot.light;
            set_vec4(
                gl,
                prog.u_light_pos[i],
                [l.position[0], l.position[1], l.position[2], 1.0],
            );
            set_vec4(
                gl,
                prog.u_light_dir[i],
                [l.direction[0], l.direction[1], l.direction[2], 0.0],
            );
            set_vec4(gl, prog.u_light_diffuse[i], l.diffuse);
            set_vec4(gl, prog.u_light_ambient[i], l.ambient);
            set_vec4(gl, prog.u_light_specular[i], l.specular);
            set_vec4(
                gl,
                prog.u_light_params[i],
                [
                    l.range,
                    l.falloff,
                    (l.theta * 0.5).cos(),
                    (l.phi * 0.5).cos(),
                ],
            );
            set_vec4(
                gl,
                prog.u_light_atten[i],
                [l.attenuation0, l.attenuation1, l.attenuation2, 0.0],
            );
        }

        if prog.u_vc >= 0 {
            (gl.gl_uniform4fv)(
                prog.u_vc,
                state.vs_constants.len() as GLsizei,
                state.vs_constants.as_ptr().cast(),
            );
        }
        if prog.u_pc >= 0 {
            (gl.gl_uniform4fv)(
                prog.u_pc,
                state.ps_constants.len() as GLsizei,
                state.ps_constants.as_ptr().cast(),
            );
        }
    }
}

unsafe fn set_vec4(gl: &Gles3Functions, loc: GLint, v: [f32; 4]) {
    if loc >= 0 {
        unsafe {
            (gl.gl_uniform4f)(loc, v[0], v[1], v[2], v[3]);
        }
    }
}

fn enable_attrib(
    gl: &Gles3Functions,
    location: GLuint,
    components: GLint,
    gl_type: GLenum,
    normalized: GLboolean,
    stride: u32,
    offset: usize,
) {
    unsafe {
        (gl.gl_enable_vertex_attrib_array)(location);
        (gl.gl_vertex_attrib_pointer)(
            location,
            components,
            gl_type,
            normalized,
            stride as GLsizei,
            offset as *const c_void,
        );
    }
}

fn d3dcolor_to_rgba_f32(argb: u32) -> [f32; 4] {
    let a = ((argb >> 24) & 0xFF) as f32 / 255.0;
    let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
    let b = (argb & 0xFF) as f32 / 255.0;
    [r, g, b, a]
}

fn map_primitive_type(prim_type: u32) -> Option<GLenum> {
    match prim_type {
        D3DPT_POINTLIST => Some(GL_POINTS),
        D3DPT_LINELIST => Some(GL_LINES),
        D3DPT_LINESTRIP => Some(GL_LINE_STRIP),
        D3DPT_TRIANGLELIST => Some(GL_TRIANGLES),
        D3DPT_TRIANGLESTRIP => Some(GL_TRIANGLE_STRIP),
        D3DPT_TRIANGLEFAN => Some(GL_TRIANGLE_FAN),
        _ => None,
    }
}

fn map_cmp_func(func: u32) -> GLenum {
    match func {
        D3DCMP_NEVER => GL_NEVER,
        D3DCMP_LESS => GL_LESS,
        D3DCMP_EQUAL => GL_EQUAL,
        D3DCMP_LESSEQUAL => GL_LEQUAL,
        D3DCMP_GREATER => GL_GREATER,
        D3DCMP_NOTEQUAL => GL_NOTEQUAL,
        D3DCMP_GREATEREQUAL => GL_GEQUAL,
        D3DCMP_ALWAYS => GL_ALWAYS,
        _ => GL_LEQUAL,
    }
}

fn map_blend_factor(factor: u32) -> GLenum {
    match factor {
        1 => GL_ZERO,
        2 => GL_ONE,
        3 => GL_SRC_COLOR,
        4 => GL_ONE_MINUS_SRC_COLOR,
        5 => GL_SRC_ALPHA,
        6 => GL_ONE_MINUS_SRC_ALPHA,
        7 => GL_DST_ALPHA,
        8 => GL_ONE_MINUS_DST_ALPHA,
        9 => GL_DST_COLOR,
        10 => GL_ONE_MINUS_DST_COLOR,
        11 => GL_SRC_ALPHA_SATURATE,
        _ => GL_ONE,
    }
}

fn map_blend_op(op: u32) -> GLenum {
    match op {
        2 => GL_FUNC_SUBTRACT,
        3 => GL_FUNC_REVERSE_SUBTRACT,
        4 => GL_MIN,
        5 => GL_MAX,
        _ => GL_FUNC_ADD,
    }
}

fn map_stencil_op(op: u32) -> GLenum {
    match op {
        2 => GL_ZERO,
        3 => GL_REPLACE,
        4 => GL_INCR,
        5 => GL_DECR,
        6 => GL_INVERT,
        7 => GL_INCR_WRAP,
        8 => GL_DECR_WRAP,
        _ => GL_KEEP,
    }
}

fn map_tex_address(mode: u32) -> GLenum {
    match mode {
        D3DTADDRESS_WRAP => GL_REPEAT,
        D3DTADDRESS_MIRROR | D3DTADDRESS_MIRRORONCE => GL_MIRRORED_REPEAT,
        D3DTADDRESS_CLAMP | D3DTADDRESS_BORDER => GL_CLAMP_TO_EDGE,
        _ => GL_REPEAT,
    }
}

fn map_mag_filter(filter: u32) -> GLenum {
    match filter {
        D3DTEXF_LINEAR | 3 => GL_LINEAR,
        _ => GL_NEAREST,
    }
}

fn map_min_filter(min_filter: u32, mip_filter: u32, has_mips: bool) -> GLenum {
    let linear_min = matches!(min_filter, D3DTEXF_LINEAR | 3);
    if !has_mips || mip_filter == D3DTEXF_NONE {
        return if linear_min { GL_LINEAR } else { GL_NEAREST };
    }
    match (linear_min, mip_filter) {
        (false, D3DTEXF_POINT) => GL_NEAREST_MIPMAP_NEAREST,
        (true, D3DTEXF_POINT) => GL_LINEAR_MIPMAP_NEAREST,
        (false, _) => GL_NEAREST_MIPMAP_LINEAR,
        (true, _) => GL_LINEAR_MIPMAP_LINEAR,
    }
}

const IDENTITY_4X4: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 1.0, 0.0,
    0.0, 0.0, 0.0, 1.0,
];

fn hash_ffp_key(key: &FfpShaderKey) -> u64 {
    let mut hasher = DefaultHasher::new();
    0u8.hash(&mut hasher);
    key.hash(&mut hasher);
    hasher.finish()
}

fn hash_prog_parts(
    decl_tokens: &[u32],
    vs_tokens: &[u32],
    ps_tokens: &[u32],
    alpha_test_func: u8,
    fog_mode: u8,
    ffp_stages: &[TextureStageKey; MAX_FFP_STAGES],
    specular_enabled: bool,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    1u8.hash(&mut hasher);
    decl_tokens.hash(&mut hasher);
    vs_tokens.hash(&mut hasher);
    ps_tokens.hash(&mut hasher);
    alpha_test_func.hash(&mut hasher);
    fog_mode.hash(&mut hasher);
    ffp_stages.hash(&mut hasher);
    specular_enabled.hash(&mut hasher);
    hasher.finish()
}

fn hash_recipe(recipe: &ShaderRecipe) -> u64 {
    match recipe {
        ShaderRecipe::Ffp(key) => hash_ffp_key(key),
        ShaderRecipe::Programmable {
            decl_tokens,
            vs_tokens,
            ps_tokens,
            alpha_test_func,
            fog_mode,
            ffp_stages,
            specular_enabled,
        } => hash_prog_parts(
            decl_tokens,
            vs_tokens,
            ps_tokens,
            *alpha_test_func,
            *fog_mode,
            ffp_stages,
            *specular_enabled,
        ),
    }
}

fn invert_4x4(m: &[f32; 16]) -> [f32; 16] {
    let mut inv = [0.0f32; 16];

    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
        + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
        - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
        + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
        - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];

    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
        - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
        + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
        - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
        + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];

    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
        + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
        - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
        + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
        - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];

    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
        - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
        + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
        - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
        + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];

    let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if det.abs() <= 1e-12 {
        return IDENTITY_4X4;
    }
    let inv_det = 1.0 / det;
    for slot in &mut inv {
        *slot *= inv_det;
    }
    inv
}

#[cfg(test)]
mod tests {
    use super::{build_glsl_for_recipe, invert_4x4};
    use crate::shader_cache::ShaderRecipe;
    use crate::shader_ffp::{MAX_FFP_STAGES, TextureStageKey};

    #[test]
    fn programmable_vs_with_empty_ps_preserves_ffp_stages() {
        let mut ffp_stages = [TextureStageKey::default(); MAX_FFP_STAGES];
        ffp_stages[0] = TextureStageKey {
            color_op: 4,
            color_arg1: 0,
            color_arg2: 2,
            alpha_op: 2,
            alpha_arg1: 2,
            alpha_arg2: 1,
            texcoord_index: 0,
            tex_bound: true,
            tex_transform_dim: 0,
        };
        ffp_stages[1].color_op = 1;

        let recipe = ShaderRecipe::Programmable {
            decl_tokens: vec![0x2000_0000, 0x4002_0000, 0x4001_0007, 0xFFFF_FFFF],
            vs_tokens: vec![
                0xFFFE_0101,
                1,
                0xC00F_0000,
                0x90E4_0000,
                1,
                0xD00F_0000,
                0x90E4_0000,
                1,
                0xE003_0000,
                0x90E4_0007,
                0xFFFF,
            ],
            ps_tokens: Vec::new(),
            alpha_test_func: 5,
            fog_mode: 0,
            ffp_stages,
            specular_enabled: false,
        };

        let (vs_glsl, fs_glsl) = build_glsl_for_recipe(&recipe).unwrap();
        assert!(vs_glsl.contains("out vec4 v_diffuse;"));
        assert!(vs_glsl.contains("out vec4 v_tex0;"));
        assert!(fs_glsl.contains("texture(u_tex0, v_tex0.xy)"));
        assert!(fs_glsl.contains("!(current.a > u_alpha_ref)"));
        assert!(fs_glsl.contains("v_diffuse"));
    }

    #[test]
    fn compute_aspect_viewport_covers_handheld_displays() {
        use super::{AspectMode, compute_aspect_viewport};

        assert_eq!(
            compute_aspect_viewport(640, 480, 640, 480, AspectMode::Ratio4x3),
            (0, 0, 640, 480)
        );
        assert_eq!(
            compute_aspect_viewport(640, 480, 1280, 720, AspectMode::Ratio4x3),
            (160, 0, 960, 720)
        );
        assert_eq!(
            compute_aspect_viewport(640, 480, 1280, 720, AspectMode::Stretch),
            (0, 0, 1280, 720)
        );
        assert_eq!(
            compute_aspect_viewport(640, 480, 720, 720, AspectMode::Ratio4x3),
            (0, 90, 720, 540)
        );
        assert_eq!(
            compute_aspect_viewport(640, 480, 1280, 720, AspectMode::Ratio16x9),
            (0, 0, 1280, 720)
        );
        assert_eq!(AspectMode::parse("4:3"), Some(AspectMode::Ratio4x3));
        assert_eq!(AspectMode::parse("16:9"), Some(AspectMode::Ratio16x9));
        assert_eq!(AspectMode::parse("stretch"), Some(AspectMode::Stretch));
        assert_eq!(AspectMode::parse("invalid"), None);
    }

    #[test]
    fn invert_4x4_produces_inverse_for_rigid_transform() {
        let m = [
            0.0, 1.0, 0.0, 0.0,
            -1.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            3.0, -4.0, 5.0, 1.0,
        ];
        let inv = invert_4x4(&m);
        for row in 0..4 {
            for col in 0..4 {
                let mut sum = 0.0f32;
                for k in 0..4 {
                    sum += m[row * 4 + k] * inv[k * 4 + col];
                }
                let expected = if row == col { 1.0 } else { 0.0 };
                assert!((sum - expected).abs() < 1e-5);
            }
        }
    }
}
