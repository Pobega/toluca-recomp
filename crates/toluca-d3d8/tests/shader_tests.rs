use toluca_d3d8::shader_asm::{
    D3DVSD_END, D3DVSDT_D3DCOLOR, D3DVSDT_FLOAT2, D3DVSDT_FLOAT3, SHADER_END_TOKEN,
    VertexDeclElement, parse_vertex_declaration, translate_ps_1_x, translate_vs_1_x,
};
use toluca_d3d8::shader_cache::{ShaderRecipe, ShaderRecipeCache};
use toluca_d3d8::shader_ffp::{FfpShaderKey, MAX_FFP_STAGES, TextureStageKey, generate_ffp_shaders};
use toluca_d3d8::state::{DeviceState, LightSlot};
use toluca_d3d8::types::{
    D3DCMP_GREATER, D3DFOG_LINEAR, D3DFVF_DIFFUSE, D3DFVF_NORMAL, D3DFVF_TEX1, D3DFVF_TEX2,
    D3DFVF_XYZ, D3DFVF_XYZRHW, D3DLIGHT_SPOT, D3DRS_ALPHAFUNC, D3DRS_ALPHATESTENABLE,
    D3DRS_FOGENABLE, D3DRS_FOGVERTEXMODE, D3DRS_LIGHTING, D3DRS_SPECULARENABLE, D3DTA_ALPHAREPLICATE,
    D3DTA_COMPLEMENT, D3DTA_CURRENT, D3DTA_DIFFUSE, D3DTA_TEXTURE, D3DTOP_ADD,
    D3DTOP_BLENDTEXTUREALPHA, D3DTOP_DOTPRODUCT3, D3DTOP_MODULATE, D3DTOP_SELECTARG1,
    D3DTSS_ALPHAARG1, D3DTSS_ALPHAOP, D3DTSS_COLORARG1, D3DTSS_COLORARG2, D3DTSS_COLOROP,
    D3dLight8,
};

fn vsd_stream(stream: u32) -> u32 {
    (1 << 29) | (stream & 0xF)
}

fn vsd_reg(reg: u32, data_type: u8) -> u32 {
    (2 << 29) | ((u32::from(data_type) & 0xF) << 16) | (reg & 0xF)
}

fn vsd_skip(dwords: u32) -> u32 {
    (2 << 29) | (1 << 28) | ((dwords & 0xF) << 16)
}

fn enc_dst(reg_type: u32, reg_num: u32, write_mask: u32, sat: bool, shift: u32) -> u32 {
    let mut tok = 0x8000_0000
        | ((reg_type & 0x7) << 28)
        | ((reg_type & 0x18) << 8)
        | (write_mask << 16)
        | ((shift & 0xF) << 24)
        | (reg_num & 0x7FF);
    if sat {
        tok |= 0x0010_0000;
    }
    tok
}

fn enc_src(reg_type: u32, reg_num: u32, swizzle: u32, src_mod: u32, relative: bool) -> u32 {
    let mut tok = 0x8000_0000
        | ((reg_type & 0x7) << 28)
        | ((reg_type & 0x18) << 8)
        | ((src_mod & 0xF) << 24)
        | ((swizzle & 0xFF) << 16)
        | (reg_num & 0x7FF);
    if relative {
        tok |= 0x0000_2000;
    }
    tok
}

#[test]
fn ffp_transformed_ui_quad_shader() {
    let mut state = DeviceState::new(640, 480, true);
    state.set_vertex_shader(D3DFVF_XYZRHW | D3DFVF_DIFFUSE | D3DFVF_TEX1);
    state.set_texture(0, Some(1));
    state.set_render_state(D3DRS_ALPHATESTENABLE, 1);
    state.set_render_state(D3DRS_ALPHAFUNC, D3DCMP_GREATER);

    let key = FfpShaderKey::from_state(&state);
    assert!(key.is_transformed);
    assert!(key.has_diffuse);
    assert!(!key.lighting_enabled);
    assert_eq!(key.alpha_test_func, D3DCMP_GREATER as u8);

    let pair = generate_ffp_shaders(&key);
    assert!(pair.vertex_glsl.contains("#version 300 es"));
    assert!(pair.vertex_glsl.contains("a_diffuse.bgra"));
    assert!(pair.vertex_glsl.contains("u_viewport"));
    assert!(pair.vertex_glsl.contains("u_y_flip"));
    assert!(pair.vertex_glsl.contains("a_pos.z * 2.0 - 1.0"));
    assert!(pair.fragment_glsl.contains("#version 300 es"));
    assert!(pair.fragment_glsl.contains("discard;"));
}

#[test]
fn ffp_lit_3d_spotlight_fog_and_multistage() {
    let mut state = DeviceState::new(800, 600, true);
    state.set_vertex_shader(D3DFVF_XYZ | D3DFVF_NORMAL | D3DFVF_DIFFUSE | D3DFVF_TEX2);
    state.set_render_state(D3DRS_LIGHTING, 1);
    state.set_render_state(D3DRS_SPECULARENABLE, 1);
    state.set_render_state(D3DRS_FOGENABLE, 1);
    state.set_render_state(D3DRS_FOGVERTEXMODE, D3DFOG_LINEAR);
    state.lights.push(LightSlot {
        light: D3dLight8 {
            light_type: D3DLIGHT_SPOT,
            ..Default::default()
        },
        enabled: true,
    });

    state.set_texture(0, Some(10));
    state.set_texture(1, Some(11));
    state.set_texture_stage_state(0, D3DTSS_COLOROP, D3DTOP_MODULATE);
    state.set_texture_stage_state(
        0,
        D3DTSS_COLORARG1,
        D3DTA_TEXTURE | D3DTA_COMPLEMENT,
    );
    state.set_texture_stage_state(0, D3DTSS_COLORARG2, D3DTA_DIFFUSE | D3DTA_ALPHAREPLICATE);
    state.set_texture_stage_state(0, D3DTSS_ALPHAOP, D3DTOP_SELECTARG1);
    state.set_texture_stage_state(0, D3DTSS_ALPHAARG1, D3DTA_TEXTURE);

    state.set_texture_stage_state(1, D3DTSS_COLOROP, D3DTOP_BLENDTEXTUREALPHA);
    state.set_texture_stage_state(1, D3DTSS_COLORARG1, D3DTA_TEXTURE);
    state.set_texture_stage_state(1, D3DTSS_COLORARG2, D3DTA_CURRENT);

    state.set_texture_stage_state(2, D3DTSS_COLOROP, D3DTOP_ADD);
    state.set_texture_stage_state(3, D3DTSS_COLOROP, D3DTOP_DOTPRODUCT3);

    let key = FfpShaderKey::from_state(&state);
    assert!(!key.is_transformed);
    assert!(key.lighting_enabled);
    assert_eq!(key.active_light_mask, 1);
    assert_eq!(key.fog_mode, D3DFOG_LINEAR as u8);

    let pair = generate_ffp_shaders(&key);
    assert!(pair.vertex_glsl.contains("gl_Position.z = gl_Position.z * 2.0 - gl_Position.w;"));
    assert!(pair.vertex_glsl.contains("gl_Position.y *= u_y_flip;"));
    assert!(pair.vertex_glsl.contains("u_light_params[0]"));
    assert!(pair.vertex_glsl.contains("u_fog_params"));
    assert!(pair.fragment_glsl.contains("u_fog_color"));
    assert!(pair.fragment_glsl.contains("(1.0 - tex0.rgb)"));
    assert!(pair.fragment_glsl.contains("v_diffuse.aaa"));
    assert!(pair.fragment_glsl.contains("mix(current.rgb, tex1.rgb, tex1.a)"));
    assert!(pair.fragment_glsl.contains("dot("));
}

#[test]
fn vertex_declaration_parsing_tracks_skips_and_d3dcolor() {
    let decl_tokens = [
        vsd_stream(0),
        vsd_reg(0, D3DVSDT_FLOAT3),
        vsd_skip(2),
        vsd_reg(5, D3DVSDT_D3DCOLOR),
        vsd_reg(7, D3DVSDT_FLOAT2),
        vsd_stream(1),
        vsd_reg(1, D3DVSDT_FLOAT3),
        D3DVSD_END,
    ];
    let parsed = parse_vertex_declaration(&decl_tokens).unwrap();
    assert_eq!(
        parsed,
        vec![
            VertexDeclElement {
                stream: 0,
                register: 0,
                data_type: D3DVSDT_FLOAT3,
                byte_offset: 0,
            },
            VertexDeclElement {
                stream: 0,
                register: 5,
                data_type: D3DVSDT_D3DCOLOR,
                byte_offset: 20,
            },
            VertexDeclElement {
                stream: 0,
                register: 7,
                data_type: D3DVSDT_FLOAT2,
                byte_offset: 24,
            },
            VertexDeclElement {
                stream: 1,
                register: 1,
                data_type: D3DVSDT_FLOAT3,
                byte_offset: 0,
            },
        ]
    );
}

#[test]
fn translate_vs_1_1_bytecode_with_matrix_and_relative_addr() {
    let decl = vec![
        VertexDeclElement {
            stream: 0,
            register: 0,
            data_type: D3DVSDT_FLOAT3,
            byte_offset: 0,
        },
        VertexDeclElement {
            stream: 0,
            register: 3,
            data_type: D3DVSDT_FLOAT3,
            byte_offset: 12,
        },
        VertexDeclElement {
            stream: 0,
            register: 5,
            data_type: D3DVSDT_D3DCOLOR,
            byte_offset: 24,
        },
        VertexDeclElement {
            stream: 0,
            register: 7,
            data_type: D3DVSDT_FLOAT2,
            byte_offset: 28,
        },
    ];

    let tokens = [
        0xFFFE_0101,
        20,
        enc_dst(4, 0, 0xF, false, 0),
        enc_src(1, 0, 0xE4, 0, false),
        enc_src(2, 0, 0xE4, 0, false),
        1,
        enc_dst(3, 0, 0x1, false, 0),
        enc_src(1, 0, 0xFF, 0, false),
        8,
        enc_dst(0, 0, 0x7, false, 0),
        enc_src(1, 3, 0xE4, 0, false),
        enc_src(2, 4, 0xE4, 0, true),
        1,
        enc_dst(5, 0, 0xF, false, 0),
        enc_src(1, 5, 0xE4, 0, false),
        1,
        enc_dst(6, 0, 0x3, false, 0),
        enc_src(1, 7, 0xE4, 0, false),
        SHADER_END_TOKEN,
    ];

    let glsl = translate_vs_1_x(&tokens, &decl).unwrap();
    assert!(glsl.contains("#version 300 es"));
    assert!(glsl.contains("vec4 v5 = a_v5.bgra;"));
    assert!(glsl.contains("dot(v0, u_vc[0])"));
    assert!(glsl.contains("a0.x = int(floor"));
    assert!(glsl.contains("u_vc[clamp(4 + a0.x, 0, 95)]"));
    assert!(glsl.contains("oD0 = v5;"));
    assert!(glsl.contains("oT0.xy = (v7).xy;"));
    assert!(glsl.contains("oPos.z = oPos.z * 2.0 - oPos.w;"));
    assert!(glsl.contains("oPos.y *= u_y_flip;"));
}

#[test]
fn translate_ps_1_1_bytecode_with_def_tex_mul_lrp_cnd() {
    let tokens = [
        0xFFFF_0101,
        81,
        enc_dst(2, 0, 0xF, false, 0),
        0.5f32.to_bits(),
        0.25f32.to_bits(),
        0.75f32.to_bits(),
        1.0f32.to_bits(),
        66,
        enc_dst(3, 0, 0xF, false, 0),
        5,
        enc_dst(0, 0, 0xF, true, 1),
        enc_src(3, 0, 0xE4, 0, false),
        enc_src(1, 0, 0xE4, 0, false),
        18,
        enc_dst(0, 1, 0xF, false, 0),
        enc_src(2, 0, 0xFF, 0, false),
        enc_src(0, 0, 0xE4, 0, false),
        enc_src(3, 0, 0xE4, 0, false),
        80,
        enc_dst(0, 0, 0xF, false, 0),
        enc_src(0, 0, 0xFF, 0, false),
        enc_src(0, 1, 0xE4, 0, false),
        enc_src(3, 0, 0xE4, 0, false),
        SHADER_END_TOKEN,
    ];

    let glsl = translate_ps_1_x(&tokens, D3DCMP_GREATER as u8, D3DFOG_LINEAR as u8).unwrap();
    assert!(glsl.contains("#version 300 es"));
    assert!(glsl.contains("c0 = vec4(0.5, 0.25, 0.75, 1.0);"));
    assert!(glsl.contains("t0 = texture(u_tex0, v_oT0.xy);"));
    assert!(glsl.contains("clamp((((t0 * v0)) * 2.0), 0.0, 1.0)"));
    assert!(glsl.contains("mix(t0, r0, c0.wwww)"));
    assert!(glsl.contains("greaterThan(r0.wwww, vec4(0.5))"));
    assert!(glsl.contains("u_fog_color"));
    assert!(glsl.contains("discard;"));
}

#[test]
fn shader_recipe_cache_roundtrip_dedup_and_corruption() {
    let mut state = DeviceState::new(640, 480, true);
    state.set_vertex_shader(D3DFVF_XYZ | D3DFVF_DIFFUSE | D3DFVF_TEX1);
    let ffp_key = FfpShaderKey::from_state(&state);

    let mut cache = ShaderRecipeCache::new();
    assert!(cache.insert(ShaderRecipe::Ffp(ffp_key)));
    assert!(!cache.insert(ShaderRecipe::Ffp(ffp_key)));
    assert!(cache.insert(ShaderRecipe::Programmable {
        decl_tokens: vec![vsd_stream(0), vsd_reg(0, D3DVSDT_FLOAT3), D3DVSD_END],
        vs_tokens: vec![0xFFFE_0101, SHADER_END_TOKEN],
        ps_tokens: vec![0xFFFF_0101, SHADER_END_TOKEN],
        alpha_test_func: D3DCMP_GREATER as u8,
        fog_mode: D3DFOG_LINEAR as u8,
        ffp_stages: [TextureStageKey::default(); MAX_FFP_STAGES],
        specular_enabled: false,
    }));
    let mut custom_stages = [TextureStageKey::default(); MAX_FFP_STAGES];
    custom_stages[0] = TextureStageKey {
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
    assert!(cache.insert(ShaderRecipe::Programmable {
        decl_tokens: vec![vsd_stream(0), vsd_reg(0, D3DVSDT_FLOAT3), D3DVSD_END],
        vs_tokens: vec![0xFFFE_0101, SHADER_END_TOKEN],
        ps_tokens: Vec::new(),
        alpha_test_func: D3DCMP_GREATER as u8,
        fog_mode: D3DFOG_LINEAR as u8,
        ffp_stages: custom_stages,
        specular_enabled: true,
    }));
    assert_eq!(cache.len(), 3);

    let bytes = cache.serialize();
    let decoded = ShaderRecipeCache::deserialize(&bytes).unwrap();
    assert_eq!(decoded.len(), 3);
    assert_eq!(decoded.recipes(), cache.recipes());

    let mut bad_magic = bytes.clone();
    bad_magic[0] = b'X';
    assert!(ShaderRecipeCache::deserialize(&bad_magic).is_err());

    let mut bad_version = bytes.clone();
    bad_version[4] = 99;
    assert!(ShaderRecipeCache::deserialize(&bad_version).is_err());

    let mut bad_payload = bytes.clone();
    bad_payload[15] ^= 0xFF;
    assert!(ShaderRecipeCache::deserialize(&bad_payload).is_err());
}

#[test]
fn translate_vs_1_1_mad_ofog_x_writes_scalar_fog() {
    let decl_tokens = [vsd_stream(0), vsd_reg(0, D3DVSDT_FLOAT3), D3DVSD_END];
    let decl = parse_vertex_declaration(&decl_tokens).unwrap();

    const OP_MOV: u32 = 1;
    const OP_MAD: u32 = 4;
    const REG_TEMP: u32 = 0;
    const REG_INPUT: u32 = 1;
    const REG_CONST: u32 = 2;
    const REG_RASTOUT: u32 = 4;
    const RAST_POS: u32 = 0;
    const RAST_FOG: u32 = 1;
    const SWZ_XYZW: u32 = 0xE4;
    const SWZ_XXXX: u32 = 0x00;
    const SWZ_YYYY: u32 = 0x55;

    let tokens = [
        0xFFFE_0101,
        OP_MOV,
        enc_dst(REG_RASTOUT, RAST_POS, 0xF, false, 0),
        enc_src(REG_INPUT, 0, SWZ_XYZW, 0, false),
        OP_MOV,
        enc_dst(REG_TEMP, 0, 0x1, false, 0),
        enc_src(REG_INPUT, 0, SWZ_XYZW, 0, false),
        OP_MAD,
        enc_dst(REG_RASTOUT, RAST_FOG, 0x1, false, 0),
        enc_src(REG_CONST, 23, SWZ_YYYY, 0, false),
        enc_src(REG_TEMP, 0, SWZ_XXXX, 0, false),
        enc_src(REG_CONST, 23, SWZ_XXXX, 0, false),
        SHADER_END_TOKEN,
    ];

    let glsl = translate_vs_1_x(&tokens, &decl).unwrap();
    assert!(glsl.contains("out float v_oFog;"));
    assert!(glsl.contains("vec4 oFog = vec4(1.0);"));
    assert!(glsl.contains("oFog.x = (("));
    assert!(glsl.contains("v_oFog = oFog.x;"));
}
