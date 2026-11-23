pub mod backend;
pub mod device;
pub mod mem;
pub mod shader_asm;
pub mod shader_cache;
pub mod shader_ffp;
pub mod state;
pub mod texture;
pub mod types;

pub use backend::{NullBackend, RenderBackend};
pub use device::{ComCallResult, D3d8Context, interface_method_count, method_arg_count, method_name};
pub use mem::{
    COM_METHOD_STRIDE_BITS, COM_THUNK_BASE, COM_THUNK_STRIDE, ComInterfaceId, ComObjectTable,
    GuestMemory, decode_com_thunk, encode_com_thunk,
};
pub use shader_asm::{VertexDeclElement, parse_vertex_declaration, translate_ps_1_x, translate_vs_1_x};
pub use shader_cache::{ShaderRecipe, ShaderRecipeCache};
pub use shader_ffp::{FfpShaderKey, GeneratedShaderPair, TextureStageKey, generate_ffp_shaders};
pub use state::{DeviceState, LightSlot, StateBlockData, StreamBinding};
pub use texture::{DecodedRgba8, PreparedTextureUpload, prepare_texture_upload};
