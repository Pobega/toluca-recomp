use std::fmt::Write;

use crate::state::DeviceState;
use crate::types::{
    D3DCMP_ALWAYS, D3DCMP_EQUAL, D3DCMP_GREATER, D3DCMP_GREATEREQUAL, D3DCMP_LESS,
    D3DCMP_LESSEQUAL, D3DCMP_NEVER, D3DCMP_NOTEQUAL, D3DFOG_EXP, D3DFOG_EXP2, D3DFOG_LINEAR,
    D3DFOG_NONE, D3DFVF_DIFFUSE, D3DFVF_NORMAL, D3DFVF_POSITION_MASK, D3DFVF_SPECULAR,
    D3DFVF_TEXCOUNT_MASK, D3DFVF_TEXCOUNT_SHIFT, D3DFVF_XYZRHW, D3DLIGHT_DIRECTIONAL,
    D3DLIGHT_POINT, D3DLIGHT_SPOT, D3DRS_ALPHAFUNC, D3DRS_ALPHATESTENABLE, D3DRS_FOGENABLE,
    D3DRS_FOGTABLEMODE, D3DRS_FOGVERTEXMODE, D3DRS_LIGHTING, D3DRS_SPECULARENABLE,
    D3DTA_ALPHAREPLICATE, D3DTA_COMPLEMENT, D3DTA_CURRENT, D3DTA_DIFFUSE, D3DTA_SELECTMASK,
    D3DTA_SPECULAR, D3DTA_TFACTOR, D3DTA_TEXTURE, D3DTOP_ADD, D3DTOP_ADDSIGNED,
    D3DTOP_ADDSIGNED2X, D3DTOP_BLENDCURRENTALPHA, D3DTOP_BLENDDIFFUSEALPHA,
    D3DTOP_BLENDFACTORALPHA, D3DTOP_BLENDTEXTUREALPHA, D3DTOP_DISABLE, D3DTOP_DOTPRODUCT3,
    D3DTOP_MODULATE, D3DTOP_MODULATE2X, D3DTOP_MODULATE4X, D3DTOP_SELECTARG1, D3DTOP_SELECTARG2,
    D3DTOP_SUBTRACT, D3DTSS_ALPHAARG1, D3DTSS_ALPHAARG2, D3DTSS_ALPHAOP, D3DTSS_COLORARG1,
    D3DTSS_COLORARG2, D3DTSS_COLOROP, D3DTSS_TEXCOORDINDEX, D3DTSS_TEXTURETRANSFORMFLAGS,
};

pub const ATTR_POS: u32 = 0;
pub const ATTR_NORMAL: u32 = 1;
pub const ATTR_DIFFUSE: u32 = 2;
pub const ATTR_SPECULAR: u32 = 3;
pub const ATTR_TEX0: u32 = 4;
pub const ATTR_TEX1: u32 = 5;
pub const ATTR_TEX2: u32 = 6;
pub const ATTR_TEX3: u32 = 7;

pub const MAX_FFP_STAGES: usize = 4;
pub const MAX_FFP_LIGHTS: usize = 8;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextureStageKey {
    pub color_op: u8,
    pub color_arg1: u8,
    pub color_arg2: u8,
    pub alpha_op: u8,
    pub alpha_arg1: u8,
    pub alpha_arg2: u8,
    pub texcoord_index: u8,
    pub tex_bound: bool,
    pub tex_transform_dim: u8,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfpShaderKey {
    pub is_transformed: bool,
    pub has_normal: bool,
    pub has_diffuse: bool,
    pub has_specular: bool,
    pub texcoord_count: u8,
    pub lighting_enabled: bool,
    pub specular_enabled: bool,
    pub active_light_mask: u8,
    pub light_types: [u8; MAX_FFP_LIGHTS],
    pub fog_mode: u8,
    pub alpha_test_func: u8,
    pub stages: [TextureStageKey; MAX_FFP_STAGES],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedShaderPair {
    pub vertex_glsl: String,
    pub fragment_glsl: String,
}

impl FfpShaderKey {
    pub fn from_state(state: &DeviceState) -> Self {
        let fvf = state.current_vertex_shader;
        let is_transformed = (fvf & D3DFVF_POSITION_MASK) == D3DFVF_XYZRHW;
        let has_normal = (fvf & D3DFVF_NORMAL) != 0;
        let has_diffuse = (fvf & D3DFVF_DIFFUSE) != 0;
        let has_specular = (fvf & D3DFVF_SPECULAR) != 0;
        let texcoord_count =
            (((fvf & D3DFVF_TEXCOUNT_MASK) >> D3DFVF_TEXCOUNT_SHIFT) as usize).min(MAX_FFP_STAGES)
                as u8;

        let lighting_enabled = !is_transformed && state.get_render_state(D3DRS_LIGHTING) != 0;
        let specular_enabled = state.get_render_state(D3DRS_SPECULARENABLE) != 0;

        let mut active_light_mask = 0u8;
        let mut light_types = [0u8; MAX_FFP_LIGHTS];
        if lighting_enabled {
            for (idx, slot) in state.lights.iter().take(MAX_FFP_LIGHTS).enumerate() {
                if slot.enabled {
                    active_light_mask |= 1u8 << idx;
                    light_types[idx] = slot.light.light_type as u8;
                }
            }
        }

        let fog_mode = if state.get_render_state(D3DRS_FOGENABLE) != 0 {
            let table_mode = state.get_render_state(D3DRS_FOGTABLEMODE);
            let vertex_mode = state.get_render_state(D3DRS_FOGVERTEXMODE);
            let chosen = if table_mode != D3DFOG_NONE {
                table_mode
            } else if vertex_mode != D3DFOG_NONE {
                vertex_mode
            } else {
                D3DFOG_LINEAR
            };
            chosen.min(D3DFOG_LINEAR) as u8
        } else {
            D3DFOG_NONE as u8
        };

        let alpha_test_func = if state.get_render_state(D3DRS_ALPHATESTENABLE) != 0 {
            state
                .get_render_state(D3DRS_ALPHAFUNC)
                .clamp(D3DCMP_NEVER, D3DCMP_ALWAYS) as u8
        } else {
            0
        };

        let mut stages = [TextureStageKey::default(); MAX_FFP_STAGES];
        for (stage_idx, stage) in stages.iter_mut().enumerate() {
            let s = stage_idx as u32;
            let raw_tci = state.get_texture_stage_state(s, D3DTSS_TEXCOORDINDEX);
            let ttf = state.get_texture_stage_state(s, D3DTSS_TEXTURETRANSFORMFLAGS) & 0xFF;
            *stage = TextureStageKey {
                color_op: state.get_texture_stage_state(s, D3DTSS_COLOROP) as u8,
                color_arg1: state.get_texture_stage_state(s, D3DTSS_COLORARG1) as u8,
                color_arg2: state.get_texture_stage_state(s, D3DTSS_COLORARG2) as u8,
                alpha_op: state.get_texture_stage_state(s, D3DTSS_ALPHAOP) as u8,
                alpha_arg1: state.get_texture_stage_state(s, D3DTSS_ALPHAARG1) as u8,
                alpha_arg2: state.get_texture_stage_state(s, D3DTSS_ALPHAARG2) as u8,
                texcoord_index: (raw_tci & 0xF).min(3) as u8,
                tex_bound: state.bound_textures[stage_idx].is_some(),
                tex_transform_dim: ttf.min(4) as u8,
            };
        }

        Self {
            is_transformed,
            has_normal,
            has_diffuse,
            has_specular,
            texcoord_count,
            lighting_enabled,
            specular_enabled,
            active_light_mask,
            light_types,
            fog_mode,
            alpha_test_func,
            stages,
        }
    }
}

pub fn generate_ffp_shaders(key: &FfpShaderKey) -> GeneratedShaderPair {
    GeneratedShaderPair {
        vertex_glsl: generate_ffp_vertex(key),
        fragment_glsl: generate_ffp_fragment(key),
    }
}

fn generate_ffp_vertex(key: &FfpShaderKey) -> String {
    let mut vs = String::with_capacity(2048);
    writeln!(vs, "#version 300 es").unwrap();
    writeln!(vs, "precision highp float;").unwrap();
    writeln!(vs, "layout(location = {ATTR_POS}) in vec4 a_pos;").unwrap();
    if key.has_normal {
        writeln!(vs, "layout(location = {ATTR_NORMAL}) in vec3 a_normal;").unwrap();
    }
    if key.has_diffuse {
        writeln!(vs, "layout(location = {ATTR_DIFFUSE}) in vec4 a_diffuse;").unwrap();
    }
    if key.has_specular {
        writeln!(vs, "layout(location = {ATTR_SPECULAR}) in vec4 a_specular;").unwrap();
    }
    for t in 0..key.texcoord_count {
        let loc = ATTR_TEX0 + u32::from(t);
        writeln!(vs, "layout(location = {loc}) in vec4 a_tex{t};").unwrap();
    }

    writeln!(vs, "uniform mat4 u_world;").unwrap();
    writeln!(vs, "uniform mat4 u_view;").unwrap();
    writeln!(vs, "uniform mat4 u_inv_view;").unwrap();
    writeln!(vs, "uniform mat4 u_proj;").unwrap();
    writeln!(vs, "uniform mat4 u_tex_mats[{MAX_FFP_STAGES}];").unwrap();
    writeln!(vs, "uniform vec4 u_viewport;").unwrap();
    writeln!(vs, "uniform float u_y_flip;").unwrap();
    writeln!(vs, "uniform vec4 u_ambient;").unwrap();
    writeln!(vs, "uniform vec4 u_mat_diffuse;").unwrap();
    writeln!(vs, "uniform vec4 u_mat_ambient;").unwrap();
    writeln!(vs, "uniform vec4 u_mat_specular;").unwrap();
    writeln!(vs, "uniform vec4 u_mat_emissive;").unwrap();
    writeln!(vs, "uniform float u_mat_power;").unwrap();
    writeln!(vs, "uniform vec4 u_fog_params;").unwrap();

    if key.lighting_enabled && key.active_light_mask != 0 {
        writeln!(vs, "uniform vec4 u_light_pos[{MAX_FFP_LIGHTS}];").unwrap();
        writeln!(vs, "uniform vec4 u_light_dir[{MAX_FFP_LIGHTS}];").unwrap();
        writeln!(vs, "uniform vec4 u_light_diffuse[{MAX_FFP_LIGHTS}];").unwrap();
        writeln!(vs, "uniform vec4 u_light_ambient[{MAX_FFP_LIGHTS}];").unwrap();
        writeln!(vs, "uniform vec4 u_light_specular[{MAX_FFP_LIGHTS}];").unwrap();
        writeln!(vs, "uniform vec4 u_light_params[{MAX_FFP_LIGHTS}];").unwrap();
        writeln!(vs, "uniform vec4 u_light_atten[{MAX_FFP_LIGHTS}];").unwrap();
    }

    writeln!(vs, "out vec4 v_diffuse;").unwrap();
    writeln!(vs, "out vec4 v_specular;").unwrap();
    for t in 0..MAX_FFP_STAGES {
        writeln!(vs, "out vec4 v_tex{t};").unwrap();
    }
    writeln!(vs, "out float v_fog_factor;").unwrap();

    writeln!(vs, "void main() {{").unwrap();
    if key.has_diffuse {
        writeln!(vs, "    vec4 base_diffuse = a_diffuse.bgra;").unwrap();
    } else {
        writeln!(vs, "    vec4 base_diffuse = vec4(1.0);").unwrap();
    }
    if key.has_specular {
        writeln!(vs, "    vec4 base_specular = a_specular.bgra;").unwrap();
    } else {
        writeln!(vs, "    vec4 base_specular = vec4(0.0, 0.0, 0.0, 1.0);").unwrap();
    }

    if key.is_transformed {
        emit_transformed_position(&mut vs);
        writeln!(vs, "    v_diffuse = base_diffuse;").unwrap();
        writeln!(vs, "    v_specular = base_specular;").unwrap();
        writeln!(vs, "    v_fog_factor = 1.0;").unwrap();
    } else {
        emit_untransformed_vertex(&mut vs, key);
    }

    for (stage_idx, stage) in key.stages.iter().enumerate() {
        let coord_idx = usize::from(stage.texcoord_index);
        if coord_idx < usize::from(key.texcoord_count) {
            if stage.tex_transform_dim > 0 {
                writeln!(
                    vs,
                    "    v_tex{stage_idx} = u_tex_mats[{stage_idx}] * a_tex{coord_idx};"
                )
                .unwrap();
            } else {
                writeln!(vs, "    v_tex{stage_idx} = a_tex{coord_idx};").unwrap();
            }
        } else {
            writeln!(vs, "    v_tex{stage_idx} = vec4(0.0, 0.0, 0.0, 1.0);").unwrap();
        }
    }

    writeln!(vs, "}}").unwrap();
    vs
}

fn emit_transformed_position(vs: &mut String) {
    writeln!(
        vs,
        "    float nx = ((a_pos.x - u_viewport.x) / max(u_viewport.z, 1.0)) * 2.0 - 1.0;"
    )
    .unwrap();
    writeln!(
        vs,
        "    float ny = 1.0 - ((a_pos.y - u_viewport.y) / max(u_viewport.w, 1.0)) * 2.0;"
    )
    .unwrap();
    writeln!(vs, "    float nz = a_pos.z * 2.0 - 1.0;").unwrap();
    writeln!(
        vs,
        "    float inv_w = (a_pos.w != 0.0) ? (1.0 / a_pos.w) : 1.0;"
    )
    .unwrap();
    writeln!(
        vs,
        "    gl_Position = vec4(nx * inv_w, ny * inv_w * u_y_flip, nz * inv_w, inv_w);"
    )
    .unwrap();
}

fn emit_untransformed_vertex(vs: &mut String, key: &FfpShaderKey) {
    writeln!(vs, "    vec4 world_pos = u_world * vec4(a_pos.xyz, 1.0);").unwrap();
    writeln!(vs, "    vec4 view_pos = u_view * world_pos;").unwrap();
    writeln!(vs, "    gl_Position = u_proj * view_pos;").unwrap();

    if key.lighting_enabled {
        if key.has_normal {
            writeln!(
                vs,
                "    vec3 n_raw = mat3(u_world) * a_normal;\n    float n_len = length(n_raw);\n    vec3 world_n = (n_len > 1e-6) ? (n_raw / n_len) : vec3(0.0, 1.0, 0.0);"
            )
            .unwrap();
        } else {
            writeln!(vs, "    vec3 world_n = vec3(0.0, 1.0, 0.0);").unwrap();
        }
        writeln!(
            vs,
            "    vec3 lit_rgb = u_mat_emissive.rgb + u_ambient.rgb * u_mat_ambient.rgb;"
        )
        .unwrap();
        writeln!(vs, "    vec3 spec_rgb = vec3(0.0);").unwrap();
        writeln!(
            vs,
            "    vec3 eye_pos = (u_inv_view * vec4(0.0, 0.0, 0.0, 1.0)).xyz;\n    vec3 to_eye = normalize(eye_pos - world_pos.xyz);"
        )
        .unwrap();

        for idx in 0..MAX_FFP_LIGHTS {
            if (key.active_light_mask & (1u8 << idx)) == 0 {
                continue;
            }
            let ltype = u32::from(key.light_types[idx]);
            writeln!(vs, "    {{").unwrap();
            match ltype {
                D3DLIGHT_DIRECTIONAL => {
                    writeln!(vs, "        vec3 l_dir = normalize(-u_light_dir[{idx}].xyz);").unwrap();
                    writeln!(vs, "        float atten = 1.0;").unwrap();
                }
                D3DLIGHT_POINT | D3DLIGHT_SPOT => {
                    writeln!(vs, "        vec3 to_light = u_light_pos[{idx}].xyz - world_pos.xyz;").unwrap();
                    writeln!(vs, "        float dist = length(to_light);").unwrap();
                    writeln!(vs, "        vec3 l_dir = (dist > 1e-6) ? (to_light / dist) : vec3(0.0, 1.0, 0.0);").unwrap();
                    writeln!(vs, "        float atten = 0.0;").unwrap();
                    writeln!(vs, "        if (dist <= u_light_params[{idx}].x) {{").unwrap();
                    writeln!(
                        vs,
                        "            float denom = u_light_atten[{idx}].x + u_light_atten[{idx}].y * dist + u_light_atten[{idx}].z * dist * dist;"
                    )
                    .unwrap();
                    writeln!(vs, "            atten = (denom > 1e-6) ? (1.0 / denom) : 1.0;").unwrap();
                    if ltype == D3DLIGHT_SPOT {
                        writeln!(
                            vs,
                            "            float rho = dot(-l_dir, normalize(u_light_dir[{idx}].xyz));"
                        )
                        .unwrap();
                        writeln!(
                            vs,
                            "            float cos_theta = u_light_params[{idx}].z;\n            float cos_phi = u_light_params[{idx}].w;\n            float spot = (rho <= cos_phi) ? 0.0 : ((rho >= cos_theta) ? 1.0 : pow((rho - cos_phi) / max(cos_theta - cos_phi, 1e-5), u_light_params[{idx}].y));"
                        )
                        .unwrap();
                        writeln!(vs, "            atten *= spot;").unwrap();
                    }
                    writeln!(vs, "        }}").unwrap();
                }
                _ => {
                    writeln!(vs, "        vec3 l_dir = vec3(0.0, 1.0, 0.0);").unwrap();
                    writeln!(vs, "        float atten = 0.0;").unwrap();
                }
            }
            writeln!(vs, "        float ndotl = max(dot(world_n, l_dir), 0.0);").unwrap();
            writeln!(
                vs,
                "        lit_rgb += atten * (u_light_ambient[{idx}].rgb * u_mat_ambient.rgb + ndotl * u_light_diffuse[{idx}].rgb * u_mat_diffuse.rgb);"
            )
            .unwrap();
            if key.specular_enabled {
                writeln!(
                    vs,
                    "        if (ndotl > 0.0) {{\n            vec3 h = normalize(l_dir + to_eye);\n            float spec_pow = pow(max(dot(world_n, h), 0.0), max(u_mat_power, 1.0));\n            spec_rgb += atten * spec_pow * u_light_specular[{idx}].rgb * u_mat_specular.rgb;\n        }}"
                )
                .unwrap();
            }
            writeln!(vs, "    }}").unwrap();
        }
        writeln!(
            vs,
            "    v_diffuse = vec4(clamp(lit_rgb, 0.0, 1.0), clamp(u_mat_diffuse.a * base_diffuse.a, 0.0, 1.0));"
        )
        .unwrap();
        writeln!(
            vs,
            "    v_specular = vec4(clamp(spec_rgb, 0.0, 1.0), base_specular.a);"
        )
        .unwrap();
    } else {
        writeln!(vs, "    v_diffuse = base_diffuse;").unwrap();
        writeln!(vs, "    v_specular = base_specular;").unwrap();
    }

    match u32::from(key.fog_mode) {
        D3DFOG_LINEAR => {
            writeln!(
                vs,
                "    float fog_d = abs(view_pos.z);\n    v_fog_factor = clamp((u_fog_params.y - fog_d) / max(u_fog_params.y - u_fog_params.x, 1e-5), 0.0, 1.0);"
            )
            .unwrap();
        }
        D3DFOG_EXP => {
            writeln!(
                vs,
                "    float fog_d = abs(view_pos.z);\n    v_fog_factor = clamp(exp(-u_fog_params.z * fog_d), 0.0, 1.0);"
            )
            .unwrap();
        }
        D3DFOG_EXP2 => {
            writeln!(
                vs,
                "    float fog_fd = u_fog_params.z * abs(view_pos.z);\n    v_fog_factor = clamp(exp(-(fog_fd * fog_fd)), 0.0, 1.0);"
            )
            .unwrap();
        }
        _ => {
            writeln!(vs, "    v_fog_factor = 1.0;").unwrap();
        }
    }

    writeln!(vs, "    gl_Position.z = gl_Position.z * 2.0 - gl_Position.w;").unwrap();
    writeln!(vs, "    gl_Position.y *= u_y_flip;").unwrap();
}

fn generate_ffp_fragment(key: &FfpShaderKey) -> String {
    let mut fs = String::with_capacity(2048);
    writeln!(fs, "#version 300 es").unwrap();
    writeln!(fs, "precision highp float;").unwrap();
    writeln!(fs, "in vec4 v_diffuse;").unwrap();
    writeln!(fs, "in vec4 v_specular;").unwrap();
    for t in 0..MAX_FFP_STAGES {
        writeln!(fs, "in vec4 v_tex{t};").unwrap();
    }
    writeln!(fs, "in float v_fog_factor;").unwrap();

    for t in 0..MAX_FFP_STAGES {
        writeln!(fs, "uniform sampler2D u_tex{t};").unwrap();
    }
    writeln!(fs, "uniform vec4 u_tfactor;").unwrap();
    writeln!(fs, "uniform vec4 u_fog_color;").unwrap();
    writeln!(fs, "uniform float u_alpha_ref;").unwrap();
    writeln!(fs, "out vec4 frag_color;").unwrap();

    writeln!(fs, "void main() {{").unwrap();
    writeln!(fs, "    vec4 current = v_diffuse;").unwrap();

    for (idx, stage) in key.stages.iter().enumerate() {
        if u32::from(stage.color_op) == D3DTOP_DISABLE {
            break;
        }
        if stage.tex_bound {
            writeln!(
                fs,
                "    vec4 tex{idx} = texture(u_tex{idx}, v_tex{idx}.xy);"
            )
            .unwrap();
        } else {
            writeln!(fs, "    vec4 tex{idx} = vec4(1.0);").unwrap();
        }

        let c_arg1 = format_stage_arg(stage.color_arg1, idx, false);
        let c_arg2 = format_stage_arg(stage.color_arg2, idx, false);
        let c_expr = format_stage_op(stage.color_op, &c_arg1, &c_arg2, idx, false);
        writeln!(fs, "    vec3 stage_rgb{idx} = {c_expr};").unwrap();

        let a_expr = if u32::from(stage.alpha_op) == D3DTOP_DISABLE {
            "current.a".to_owned()
        } else {
            let a_arg1 = format_stage_arg(stage.alpha_arg1, idx, true);
            let a_arg2 = format_stage_arg(stage.alpha_arg2, idx, true);
            format_stage_op(stage.alpha_op, &a_arg1, &a_arg2, idx, true)
        };
        writeln!(fs, "    float stage_a{idx} = {a_expr};").unwrap();
        writeln!(
            fs,
            "    current = vec4(clamp(stage_rgb{idx}, 0.0, 1.0), clamp(stage_a{idx}, 0.0, 1.0));"
        )
        .unwrap();
    }

    if key.specular_enabled {
        writeln!(fs, "    current.rgb = clamp(current.rgb + v_specular.rgb, 0.0, 1.0);").unwrap();
    }
    if u32::from(key.fog_mode) != D3DFOG_NONE {
        writeln!(
            fs,
            "    current.rgb = mix(u_fog_color.rgb, current.rgb, clamp(v_fog_factor, 0.0, 1.0));"
        )
        .unwrap();
    }

    emit_alpha_test(&mut fs, key.alpha_test_func);

    writeln!(fs, "    frag_color = current;").unwrap();
    writeln!(fs, "}}").unwrap();
    fs
}

fn format_stage_arg(raw: u8, stage_idx: usize, alpha_only: bool) -> String {
    let raw_u32 = u32::from(raw);
    let base = match raw_u32 & D3DTA_SELECTMASK {
        D3DTA_DIFFUSE => "v_diffuse".to_owned(),
        D3DTA_CURRENT => "current".to_owned(),
        D3DTA_TEXTURE => format!("tex{stage_idx}"),
        D3DTA_TFACTOR => "u_tfactor".to_owned(),
        D3DTA_SPECULAR => "v_specular".to_owned(),
        _ => "current".to_owned(),
    };

    let swizzle = if alpha_only {
        ".a"
    } else if (raw_u32 & D3DTA_ALPHAREPLICATE) != 0 {
        ".aaa"
    } else {
        ".rgb"
    };

    let expr = format!("{base}{swizzle}");
    if (raw_u32 & D3DTA_COMPLEMENT) != 0 {
        format!("(1.0 - {expr})")
    } else {
        expr
    }
}

fn format_stage_op(
    op: u8,
    arg1: &str,
    arg2: &str,
    stage_idx: usize,
    alpha_only: bool,
) -> String {
    let zero_half = if alpha_only { "0.5" } else { "vec3(0.5)" };
    match u32::from(op) {
        D3DTOP_SELECTARG1 => arg1.to_owned(),
        D3DTOP_SELECTARG2 => arg2.to_owned(),
        D3DTOP_MODULATE => format!("({arg1} * {arg2})"),
        D3DTOP_MODULATE2X => format!("({arg1} * {arg2} * 2.0)"),
        D3DTOP_MODULATE4X => format!("({arg1} * {arg2} * 4.0)"),
        D3DTOP_ADD => format!("({arg1} + {arg2})"),
        D3DTOP_ADDSIGNED => format!("({arg1} + {arg2} - {zero_half})"),
        D3DTOP_ADDSIGNED2X => format!("(({arg1} + {arg2} - {zero_half}) * 2.0)"),
        D3DTOP_SUBTRACT => format!("({arg1} - {arg2})"),
        D3DTOP_BLENDDIFFUSEALPHA => format!("mix({arg2}, {arg1}, v_diffuse.a)"),
        D3DTOP_BLENDTEXTUREALPHA => format!("mix({arg2}, {arg1}, tex{stage_idx}.a)"),
        D3DTOP_BLENDFACTORALPHA => format!("mix({arg2}, {arg1}, u_tfactor.a)"),
        D3DTOP_BLENDCURRENTALPHA => format!("mix({arg2}, {arg1}, current.a)"),
        D3DTOP_DOTPRODUCT3 => {
            if alpha_only {
                format!("clamp(dot({arg1} - 0.5, {arg2} - 0.5) * 4.0, 0.0, 1.0)")
            } else {
                format!("vec3(clamp(dot({arg1} - vec3(0.5), {arg2} - vec3(0.5)) * 4.0, 0.0, 1.0))")
            }
        }
        _ => arg1.to_owned(),
    }
}

pub fn emit_alpha_test(fs: &mut String, alpha_test_func: u8) {
    let cond = match u32::from(alpha_test_func) {
        0 | D3DCMP_ALWAYS => return,
        D3DCMP_NEVER => "true",
        D3DCMP_LESS => "!(current.a < u_alpha_ref)",
        D3DCMP_EQUAL => "!(abs(current.a - u_alpha_ref) < (0.5 / 255.0))",
        D3DCMP_LESSEQUAL => "!(current.a <= u_alpha_ref)",
        D3DCMP_GREATER => "!(current.a > u_alpha_ref)",
        D3DCMP_NOTEQUAL => "!(abs(current.a - u_alpha_ref) >= (0.5 / 255.0))",
        D3DCMP_GREATEREQUAL => "!(current.a >= u_alpha_ref)",
        _ => return,
    };
    writeln!(fs, "    if ({cond}) {{ discard; }}").unwrap();
}
