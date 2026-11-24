use std::fmt::Write;

use crate::shader_ffp::emit_alpha_test;
use crate::types::D3DFOG_NONE;

pub const D3DVSDT_FLOAT1: u8 = 0;
pub const D3DVSDT_FLOAT2: u8 = 1;
pub const D3DVSDT_FLOAT3: u8 = 2;
pub const D3DVSDT_FLOAT4: u8 = 3;
pub const D3DVSDT_D3DCOLOR: u8 = 4;
pub const D3DVSDT_UBYTE4: u8 = 5;
pub const D3DVSDT_SHORT2: u8 = 6;
pub const D3DVSDT_SHORT4: u8 = 7;

pub const D3DVSD_END: u32 = 0xFFFF_FFFF;
pub const SHADER_END_TOKEN: u32 = 0x0000_FFFF;

const D3DVSD_TOKEN_NOP: u32 = 0;
const D3DVSD_TOKEN_STREAM: u32 = 1;
const D3DVSD_TOKEN_STREAMDATA: u32 = 2;
const D3DVSD_TOKEN_TESSELLATOR: u32 = 3;
const D3DVSD_TOKEN_CONSTMEM: u32 = 4;
const D3DVSD_TOKEN_EXT: u32 = 5;
const D3DVSD_SKIP_FLAG: u32 = 0x1000_0000;

const D3DSIO_NOP: u32 = 0;
const D3DSIO_MOV: u32 = 1;
const D3DSIO_ADD: u32 = 2;
const D3DSIO_SUB: u32 = 3;
const D3DSIO_MAD: u32 = 4;
const D3DSIO_MUL: u32 = 5;
const D3DSIO_RCP: u32 = 6;
const D3DSIO_RSQ: u32 = 7;
const D3DSIO_DP3: u32 = 8;
const D3DSIO_DP4: u32 = 9;
const D3DSIO_MIN: u32 = 10;
const D3DSIO_MAX: u32 = 11;
const D3DSIO_SLT: u32 = 12;
const D3DSIO_SGE: u32 = 13;
const D3DSIO_EXP: u32 = 14;
const D3DSIO_LOG: u32 = 15;
const D3DSIO_LIT: u32 = 16;
const D3DSIO_DST: u32 = 17;
const D3DSIO_LRP: u32 = 18;
const D3DSIO_FRC: u32 = 19;
const D3DSIO_M4X4: u32 = 20;
const D3DSIO_M4X3: u32 = 21;
const D3DSIO_M3X4: u32 = 22;
const D3DSIO_M3X3: u32 = 23;
const D3DSIO_M3X2: u32 = 24;
const D3DSIO_TEXCOORD: u32 = 64;
const D3DSIO_TEXKILL: u32 = 65;
const D3DSIO_TEX: u32 = 66;
const D3DSIO_TEXBEM: u32 = 67;
const D3DSIO_TEXM3X2PAD: u32 = 71;
const D3DSIO_TEXM3X2TEX: u32 = 72;
const D3DSIO_TEXM3X3PAD: u32 = 73;
const D3DSIO_TEXM3X3TEX: u32 = 74;
const D3DSIO_TEXM3X3VSPEC: u32 = 77;
const D3DSIO_EXPP: u32 = 78;
const D3DSIO_LOGP: u32 = 79;
const D3DSIO_CND: u32 = 80;
const D3DSIO_DEF: u32 = 81;
const D3DSIO_CMP: u32 = 88;
const D3DSIO_PHASE: u32 = 0xFFFD;
const D3DSIO_COMMENT: u32 = 0xFFFE;

const REG_TEMP: u32 = 0;
const REG_INPUT: u32 = 1;
const REG_CONST: u32 = 2;
const REG_ADDR_OR_TEX: u32 = 3;
const REG_RASTOUT: u32 = 4;
const REG_ATTROUT: u32 = 5;
const REG_TEXCRDOUT: u32 = 6;

const DST_SAT_MASK: u32 = 0x0010_0000;
const SRC_REL_ADDR_MASK: u32 = 0x0000_2000;
const VS_VERSION_PREFIX: u32 = 0xFFFE_0000;
const PS_VERSION_PREFIX: u32 = 0xFFFF_0000;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct VertexDeclElement {
    pub stream: u8,
    pub register: u8,
    pub data_type: u8,
    pub byte_offset: u16,
}

pub fn vsdt_byte_size(data_type: u8) -> u16 {
    match data_type {
        D3DVSDT_FLOAT1 | D3DVSDT_D3DCOLOR | D3DVSDT_UBYTE4 | D3DVSDT_SHORT2 => 4,
        D3DVSDT_FLOAT2 | D3DVSDT_SHORT4 => 8,
        D3DVSDT_FLOAT3 => 12,
        D3DVSDT_FLOAT4 => 16,
        _ => 4,
    }
}

pub fn parse_vertex_declaration(tokens: &[u32]) -> Result<Vec<VertexDeclElement>, String> {
    let mut elements = Vec::new();
    let mut stream = 0u8;
    let mut byte_offset = 0u16;
    let mut idx = 0usize;

    while idx < tokens.len() {
        let tok = tokens[idx];
        idx += 1;
        if tok == D3DVSD_END {
            return Ok(elements);
        }

        let token_type = (tok >> 29) & 0x7;
        match token_type {
            D3DVSD_TOKEN_NOP | D3DVSD_TOKEN_TESSELLATOR => {}
            D3DVSD_TOKEN_STREAM => {
                stream = (tok & 0xF) as u8;
                byte_offset = 0;
            }
            D3DVSD_TOKEN_STREAMDATA => {
                if (tok & D3DVSD_SKIP_FLAG) != 0 {
                    let skip_dwords = ((tok >> 16) & 0xF) as u16;
                    byte_offset = byte_offset.saturating_add(skip_dwords * 4);
                } else {
                    let register = (tok & 0xF) as u8;
                    let data_type = ((tok >> 16) & 0xF) as u8;
                    elements.push(VertexDeclElement {
                        stream,
                        register,
                        data_type,
                        byte_offset,
                    });
                    byte_offset = byte_offset.saturating_add(vsdt_byte_size(data_type));
                }
            }
            D3DVSD_TOKEN_CONSTMEM => {
                let vec_count = ((tok >> 25) & 0xF) as usize;
                idx = idx.saturating_add(vec_count * 4);
            }
            D3DVSD_TOKEN_EXT => {
                let ext_count = ((tok >> 24) & 0x1F) as usize;
                idx = idx.saturating_add(ext_count);
            }
            other => {
                return Err(format!("unknown vertex decl token type {other} (0x{tok:08x})"));
            }
        }
    }

    Ok(elements)
}

#[derive(Copy, Clone)]
struct DecodedParam {
    reg_type: u32,
    reg_num: u32,
    write_mask: u32,
    sat: bool,
    shift: u32,
    swizzle: u32,
    src_mod: u32,
    relative: bool,
}

fn decode_param(tok: u32) -> DecodedParam {
    let reg_type = ((tok >> 28) & 0x7) | ((tok >> 8) & 0x18);
    DecodedParam {
        reg_type,
        reg_num: tok & 0x7FF,
        write_mask: (tok >> 16) & 0xF,
        sat: (tok & DST_SAT_MASK) != 0,
        shift: (tok >> 24) & 0xF,
        swizzle: (tok >> 16) & 0xFF,
        src_mod: (tok >> 24) & 0xF,
        relative: (tok & SRC_REL_ADDR_MASK) != 0,
    }
}

fn swizzle_component(idx: u32) -> char {
    match idx & 0x3 {
        0 => 'x',
        1 => 'y',
        2 => 'z',
        _ => 'w',
    }
}

fn format_swizzle(swizzle: u32) -> String {
    if swizzle == 0xE4 {
        return String::new();
    }
    let mut s = String::with_capacity(5);
    s.push('.');
    for shift in [0, 2, 4, 6] {
        s.push(swizzle_component(swizzle >> shift));
    }
    s
}

fn format_writemask(mask: u32) -> String {
    if mask == 0xF || mask == 0 {
        return String::new();
    }
    let mut s = String::with_capacity(5);
    s.push('.');
    if (mask & 0x1) != 0 {
        s.push('x');
    }
    if (mask & 0x2) != 0 {
        s.push('y');
    }
    if (mask & 0x4) != 0 {
        s.push('z');
    }
    if (mask & 0x8) != 0 {
        s.push('w');
    }
    s
}

fn apply_src_modifier(expr: &str, src_mod: u32) -> String {
    match src_mod {
        0 => expr.to_owned(),
        1 => format!("(-({expr}))"),
        2 => format!("(({expr}) - 0.5)"),
        3 => format!("(-(({expr}) - 0.5))"),
        4 => format!("(2.0 * ({expr}) - 1.0)"),
        5 => format!("(-(2.0 * ({expr}) - 1.0))"),
        6 => format!("(1.0 - ({expr}))"),
        7 => format!("(2.0 * ({expr}))"),
        8 => format!("(-2.0 * ({expr}))"),
        _ => expr.to_owned(),
    }
}

fn apply_dst_modifiers(expr: &str, p: DecodedParam, is_ps: bool) -> String {
    let mut out = expr.to_owned();
    if is_ps {
        out = match p.shift {
            1 => format!("(({out}) * 2.0)"),
            2 => format!("(({out}) * 4.0)"),
            3 => format!("(({out}) * 8.0)"),
            15 => format!("(({out}) * 0.5)"),
            14 => format!("(({out}) * 0.25)"),
            13 => format!("(({out}) * 0.125)"),
            _ => out,
        };
    }
    if p.sat {
        out = format!("clamp({out}, 0.0, 1.0)");
    }
    out
}

fn emit_masked_assign(out: &mut String, dst_name: &str, p: DecodedParam, expr: &str, is_ps: bool) {
    let modified = apply_dst_modifiers(expr, p, is_ps);
    let mask = format_writemask(p.write_mask);
    if mask.is_empty() {
        writeln!(out, "    {dst_name} = {modified};").unwrap();
    } else {
        writeln!(out, "    {dst_name}{mask} = ({modified}){mask};").unwrap();
    }
}

fn vs_dst_name(p: DecodedParam) -> Result<String, String> {
    match p.reg_type {
        REG_TEMP => Ok(format!("r{}", p.reg_num.min(11))),
        REG_ADDR_OR_TEX => Ok("a0".to_owned()),
        REG_RASTOUT => match p.reg_num {
            0 => Ok("oPos".to_owned()),
            1 => Ok("oFog".to_owned()),
            2 => Ok("oPts".to_owned()),
            other => Err(format!("invalid vs rastout register {other}")),
        },
        REG_ATTROUT => Ok(format!("oD{}", p.reg_num.min(1))),
        REG_TEXCRDOUT => Ok(format!("oT{}", p.reg_num.min(7))),
        other => Err(format!("invalid vs dest register type {other}")),
    }
}

fn vs_src_expr(tok: u32) -> Result<String, String> {
    let p = decode_param(tok);
    let base = match p.reg_type {
        REG_TEMP => format!("r{}", p.reg_num.min(11)),
        REG_INPUT => format!("v{}", p.reg_num.min(15)),
        REG_CONST => {
            if p.relative {
                format!("u_vc[clamp({} + a0.x, 0, 95)]", p.reg_num)
            } else {
                format!("u_vc[{}]", p.reg_num.min(95))
            }
        }
        other => return Err(format!("invalid vs source register type {other}")),
    };
    let swz = format_swizzle(p.swizzle);
    Ok(apply_src_modifier(&format!("{base}{swz}"), p.src_mod))
}

fn vs_const_indexed(tok: u32, row_offset: u32) -> String {
    let p = decode_param(tok);
    let idx_expr = if p.relative {
        format!("clamp({} + a0.x, 0, 95)", p.reg_num + row_offset)
    } else {
        format!("{}", (p.reg_num + row_offset).min(95))
    };
    let swz = format_swizzle(p.swizzle);
    apply_src_modifier(&format!("u_vc[{idx_expr}]{swz}"), p.src_mod)
}

pub fn translate_vs_1_x(tokens: &[u32], decl: &[VertexDeclElement]) -> Result<String, String> {
    let Some(&version) = tokens.first() else {
        return Err("empty vertex shader token stream".to_owned());
    };
    let minor = version & 0xFFFF;
    if (version & 0xFFFF_0000) != VS_VERSION_PREFIX || !(0x0100..=0x0101).contains(&minor) {
        return Err(format!("unsupported vertex shader version 0x{version:08x}"));
    }

    let mut glsl = String::with_capacity(4096);
    writeln!(glsl, "#version 300 es").unwrap();
    writeln!(glsl, "precision highp float;").unwrap();

    let mut declared_regs = [false; 16];
    let mut color_regs = [false; 16];
    for elem in decl {
        let reg = elem.register as usize;
        if reg < 16 {
            if !declared_regs[reg] {
                writeln!(glsl, "layout(location = {reg}) in vec4 a_v{reg};").unwrap();
                declared_regs[reg] = true;
            }
            if elem.data_type == D3DVSDT_D3DCOLOR {
                color_regs[reg] = true;
            }
        }
    }
    if decl.is_empty() {
        for (reg, slot) in declared_regs.iter_mut().enumerate().take(8) {
            writeln!(glsl, "layout(location = {reg}) in vec4 a_v{reg};").unwrap();
            *slot = true;
        }
    }

    writeln!(glsl, "uniform vec4 u_vc[96];").unwrap();
    writeln!(glsl, "uniform float u_y_flip;").unwrap();
    writeln!(glsl, "out vec4 v_oD0;").unwrap();
    writeln!(glsl, "out vec4 v_oD1;").unwrap();
    for t in 0..4 {
        writeln!(glsl, "out vec4 v_oT{t};").unwrap();
    }
    writeln!(glsl, "out float v_oFog;").unwrap();

    writeln!(glsl, "void main() {{").unwrap();
    for reg in 0..16 {
        if declared_regs[reg] {
            if color_regs[reg] {
                writeln!(glsl, "    vec4 v{reg} = a_v{reg}.bgra;").unwrap();
            } else {
                writeln!(glsl, "    vec4 v{reg} = a_v{reg};").unwrap();
            }
        } else {
            writeln!(glsl, "    vec4 v{reg} = vec4(0.0, 0.0, 0.0, 1.0);").unwrap();
        }
    }
    for r in 0..12 {
        writeln!(glsl, "    vec4 r{r} = vec4(0.0);").unwrap();
    }
    writeln!(glsl, "    ivec4 a0 = ivec4(0);").unwrap();
    writeln!(glsl, "    vec4 oPos = vec4(0.0, 0.0, 0.0, 1.0);").unwrap();
    writeln!(glsl, "    vec4 oD0 = vec4(1.0);").unwrap();
    writeln!(glsl, "    vec4 oD1 = vec4(0.0, 0.0, 0.0, 1.0);").unwrap();
    for t in 0..8 {
        writeln!(glsl, "    vec4 oT{t} = vec4(0.0, 0.0, 0.0, 1.0);").unwrap();
    }
    writeln!(glsl, "    vec4 oFog = vec4(1.0);").unwrap();
    writeln!(glsl, "    vec4 oPts = vec4(1.0);").unwrap();

    let mut idx = 1usize;
    while idx < tokens.len() {
        let tok = tokens[idx];
        idx += 1;
        if tok == SHADER_END_TOKEN {
            break;
        }
        let opcode = tok & 0xFFFF;
        if opcode == D3DSIO_COMMENT {
            let len = ((tok >> 16) & 0x7FFF) as usize;
            idx = idx.saturating_add(len);
            continue;
        }
        if opcode == D3DSIO_NOP {
            continue;
        }

        let dst_tok = *tokens.get(idx).ok_or("truncated vs dst token")?;
        idx += 1;
        let dst_p = decode_param(dst_tok);
        let dst_name = vs_dst_name(dst_p)?;

        let emit_vs_dst = |out: &mut String, expr: &str| {
            if dst_p.reg_type == REG_ADDR_OR_TEX {
                writeln!(out, "    a0.x = int(floor(({expr}).x + 0.5));").unwrap();
            } else {
                emit_masked_assign(out, &dst_name, dst_p, expr, false);
            }
        };

        match opcode {
            D3DSIO_MOV => {
                let s0 = vs_src_expr(*tokens.get(idx).ok_or("truncated vs mov")?)?;
                idx += 1;
                emit_vs_dst(&mut glsl, &s0);
            }
            D3DSIO_RCP => {
                let s0 = vs_src_expr(*tokens.get(idx).ok_or("truncated vs rcp")?)?;
                idx += 1;
                let expr = format!("vec4((({s0}).x != 0.0) ? (1.0 / ({s0}).x) : 1e30)");
                emit_vs_dst(&mut glsl, &expr);
            }
            D3DSIO_RSQ => {
                let s0 = vs_src_expr(*tokens.get(idx).ok_or("truncated vs rsq")?)?;
                idx += 1;
                let expr = format!("vec4(inversesqrt(max(abs(({s0}).x), 1e-12)))");
                emit_vs_dst(&mut glsl, &expr);
            }
            D3DSIO_EXP | D3DSIO_EXPP => {
                let s0 = vs_src_expr(*tokens.get(idx).ok_or("truncated vs exp")?)?;
                idx += 1;
                let expr = format!(
                    "vec4(exp2(floor(({s0}).x)), fract(({s0}).x), exp2(({s0}).x), 1.0)"
                );
                emit_vs_dst(&mut glsl, &expr);
            }
            D3DSIO_LOG | D3DSIO_LOGP => {
                let s0 = vs_src_expr(*tokens.get(idx).ok_or("truncated vs log")?)?;
                idx += 1;
                let expr = format!(
                    "vec4(floor(log2(max(abs(({s0}).x), 1e-12))), max(abs(({s0}).x), 1e-12) / exp2(floor(log2(max(abs(({s0}).x), 1e-12)))), log2(max(abs(({s0}).x), 1e-12)), 1.0)"
                );
                emit_vs_dst(&mut glsl, &expr);
            }
            D3DSIO_LIT => {
                let s0 = vs_src_expr(*tokens.get(idx).ok_or("truncated vs lit")?)?;
                idx += 1;
                let expr = format!(
                    "vec4(1.0, max(({s0}).x, 0.0), ((({s0}).x > 0.0 && ({s0}).y > 0.0) ? pow(({s0}).y, clamp(({s0}).w, -128.0, 128.0)) : 0.0), 1.0)"
                );
                emit_vs_dst(&mut glsl, &expr);
            }
            D3DSIO_FRC => {
                let s0 = vs_src_expr(*tokens.get(idx).ok_or("truncated vs frc")?)?;
                idx += 1;
                emit_vs_dst(&mut glsl, &format!("fract({s0})"));
            }
            D3DSIO_ADD | D3DSIO_SUB | D3DSIO_MUL | D3DSIO_DP3 | D3DSIO_DP4 | D3DSIO_MIN
            | D3DSIO_MAX | D3DSIO_SLT | D3DSIO_SGE | D3DSIO_DST => {
                let s0 = vs_src_expr(*tokens.get(idx).ok_or("truncated vs binop s0")?)?;
                let s1 = vs_src_expr(*tokens.get(idx + 1).ok_or("truncated vs binop s1")?)?;
                idx += 2;
                let expr = match opcode {
                    D3DSIO_ADD => format!("({s0} + {s1})"),
                    D3DSIO_SUB => format!("({s0} - {s1})"),
                    D3DSIO_MUL => format!("({s0} * {s1})"),
                    D3DSIO_DP3 => format!("vec4(dot(({s0}).xyz, ({s1}).xyz))"),
                    D3DSIO_DP4 => format!("vec4(dot({s0}, {s1}))"),
                    D3DSIO_MIN => format!("min({s0}, {s1})"),
                    D3DSIO_MAX => format!("max({s0}, {s1})"),
                    D3DSIO_SLT => format!("vec4(lessThan({s0}, {s1}))"),
                    D3DSIO_SGE => format!("vec4(greaterThanEqual({s0}, {s1}))"),
                    D3DSIO_DST => format!("vec4(1.0, ({s0}).y * ({s1}).y, ({s0}).z, ({s1}).w)"),
                    _ => unreachable!(),
                };
                emit_vs_dst(&mut glsl, &expr);
            }
            D3DSIO_MAD => {
                let s0 = vs_src_expr(*tokens.get(idx).ok_or("truncated vs mad s0")?)?;
                let s1 = vs_src_expr(*tokens.get(idx + 1).ok_or("truncated vs mad s1")?)?;
                let s2 = vs_src_expr(*tokens.get(idx + 2).ok_or("truncated vs mad s2")?)?;
                idx += 3;
                emit_vs_dst(&mut glsl, &format!("({s0} * {s1} + {s2})"));
            }
            D3DSIO_M4X4 | D3DSIO_M4X3 | D3DSIO_M3X4 | D3DSIO_M3X3 | D3DSIO_M3X2 => {
                let s0 = vs_src_expr(*tokens.get(idx).ok_or("truncated vs mat s0")?)?;
                let c_tok = *tokens.get(idx + 1).ok_or("truncated vs mat s1")?;
                idx += 2;
                let (rows, is_dp4) = match opcode {
                    D3DSIO_M4X4 => (4u32, true),
                    D3DSIO_M4X3 => (3u32, true),
                    D3DSIO_M3X4 => (4u32, false),
                    D3DSIO_M3X3 => (3u32, false),
                    D3DSIO_M3X2 => (2u32, false),
                    _ => unreachable!(),
                };
                let mut parts = Vec::with_capacity(4);
                for row in 0..rows {
                    let row_c = vs_const_indexed(c_tok, row);
                    if is_dp4 {
                        parts.push(format!("dot({s0}, {row_c})"));
                    } else {
                        parts.push(format!("dot(({s0}).xyz, ({row_c}).xyz)"));
                    }
                }
                while parts.len() < 4 {
                    parts.push("1.0".to_owned());
                }
                let expr = format!("vec4({}, {}, {}, {})", parts[0], parts[1], parts[2], parts[3]);
                emit_vs_dst(&mut glsl, &expr);
            }
            other => return Err(format!("unsupported vs_1_1 opcode {other}")),
        }
    }

    writeln!(glsl, "    oPos.z = oPos.z * 2.0 - oPos.w;").unwrap();
    writeln!(glsl, "    oPos.y *= u_y_flip;").unwrap();
    writeln!(glsl, "    gl_Position = oPos;").unwrap();
    writeln!(glsl, "    v_oD0 = oD0;").unwrap();
    writeln!(glsl, "    v_oD1 = oD1;").unwrap();
    for t in 0..4 {
        writeln!(glsl, "    v_oT{t} = oT{t};").unwrap();
    }
    writeln!(glsl, "    v_oFog = oFog.x;").unwrap();
    writeln!(glsl, "}}").unwrap();
    Ok(glsl)
}

fn ps_reg_name(p: DecodedParam) -> Result<String, String> {
    match p.reg_type {
        REG_TEMP => Ok(format!("r{}", p.reg_num.min(5))),
        REG_INPUT => Ok(format!("v{}", p.reg_num.min(1))),
        REG_CONST => Ok(format!("c{}", p.reg_num.min(7))),
        REG_ADDR_OR_TEX => Ok(format!("t{}", p.reg_num.min(5))),
        other => Err(format!("invalid ps register type {other}")),
    }
}

fn ps_src_expr(tok: u32) -> Result<String, String> {
    let p = decode_param(tok);
    let base = ps_reg_name(p)?;
    let swz = format_swizzle(p.swizzle);
    Ok(apply_src_modifier(&format!("{base}{swz}"), p.src_mod))
}

pub fn translate_ps_1_x(
    tokens: &[u32],
    alpha_test_func: u8,
    fog_mode: u8,
) -> Result<String, String> {
    let Some(&version) = tokens.first() else {
        return Err("empty pixel shader token stream".to_owned());
    };
    let minor = version & 0xFFFF;
    if (version & 0xFFFF_0000) != PS_VERSION_PREFIX || !(0x0100..=0x0104).contains(&minor) {
        return Err(format!("unsupported pixel shader version 0x{version:08x}"));
    }
    let is_ps_1_4 = minor >= 0x0104;

    let mut glsl = String::with_capacity(4096);
    writeln!(glsl, "#version 300 es").unwrap();
    writeln!(glsl, "precision highp float;").unwrap();
    writeln!(glsl, "in vec4 v_oD0;").unwrap();
    writeln!(glsl, "in vec4 v_oD1;").unwrap();
    for t in 0..4 {
        writeln!(glsl, "in vec4 v_oT{t};").unwrap();
    }
    writeln!(glsl, "in float v_oFog;").unwrap();
    for t in 0..4 {
        writeln!(glsl, "uniform sampler2D u_tex{t};").unwrap();
    }
    writeln!(glsl, "uniform vec4 u_pc[8];").unwrap();
    writeln!(glsl, "uniform vec4 u_fog_color;").unwrap();
    writeln!(glsl, "uniform float u_alpha_ref;").unwrap();
    writeln!(glsl, "out vec4 frag_color;").unwrap();

    writeln!(glsl, "void main() {{").unwrap();
    for r in 0..6 {
        writeln!(glsl, "    vec4 r{r} = vec4(0.0);").unwrap();
    }
    for t in 0..6 {
        writeln!(glsl, "    vec4 t{t} = vec4(0.0);").unwrap();
    }
    for c in 0..8 {
        writeln!(glsl, "    vec4 c{c} = u_pc[{c}];").unwrap();
    }
    writeln!(glsl, "    vec4 v0 = v_oD0;").unwrap();
    writeln!(glsl, "    vec4 v1 = v_oD1;").unwrap();

    let mut texm_rows: Vec<String> = Vec::new();
    let mut idx = 1usize;
    while idx < tokens.len() {
        let tok = tokens[idx];
        idx += 1;
        if tok == SHADER_END_TOKEN {
            break;
        }
        let opcode = tok & 0xFFFF;
        if opcode == D3DSIO_COMMENT {
            let len = ((tok >> 16) & 0x7FFF) as usize;
            idx = idx.saturating_add(len);
            continue;
        }
        if opcode == D3DSIO_NOP || opcode == D3DSIO_PHASE {
            continue;
        }

        let dst_tok = *tokens.get(idx).ok_or("truncated ps dst token")?;
        idx += 1;
        let dst_p = decode_param(dst_tok);
        let dst_name = ps_reg_name(dst_p)?;

        match opcode {
            D3DSIO_DEF => {
                if idx + 4 > tokens.len() {
                    return Err("truncated ps def constants".to_owned());
                }
                let f0 = f32::from_bits(tokens[idx]);
                let f1 = f32::from_bits(tokens[idx + 1]);
                let f2 = f32::from_bits(tokens[idx + 2]);
                let f3 = f32::from_bits(tokens[idx + 3]);
                idx += 4;
                writeln!(glsl, "    {dst_name} = vec4({f0:?}, {f1:?}, {f2:?}, {f3:?});").unwrap();
            }
            D3DSIO_TEX => {
                let stage = dst_p.reg_num.min(3);
                if is_ps_1_4 {
                    let s0 = ps_src_expr(*tokens.get(idx).ok_or("truncated ps texld src")?)?;
                    idx += 1;
                    let expr = format!("texture(u_tex{stage}, ({s0}).xy)");
                    emit_masked_assign(&mut glsl, &dst_name, dst_p, &expr, true);
                } else {
                    writeln!(
                        glsl,
                        "    {dst_name} = texture(u_tex{stage}, v_oT{stage}.xy);"
                    )
                    .unwrap();
                }
            }
            D3DSIO_TEXCOORD => {
                let stage = dst_p.reg_num.min(3);
                writeln!(glsl, "    {dst_name} = clamp(v_oT{stage}, 0.0, 1.0);").unwrap();
            }
            D3DSIO_TEXKILL => {
                writeln!(
                    glsl,
                    "    if (any(lessThan({dst_name}.xyz, vec3(0.0)))) {{ discard; }}"
                )
                .unwrap();
            }
            D3DSIO_TEXBEM => {
                let s0 = ps_src_expr(*tokens.get(idx).ok_or("truncated ps texbem src")?)?;
                idx += 1;
                let stage = dst_p.reg_num.min(3);
                writeln!(
                    glsl,
                    "    {dst_name} = texture(u_tex{stage}, v_oT{stage}.xy + ({s0}).xy);"
                )
                .unwrap();
            }
            D3DSIO_TEXM3X2PAD | D3DSIO_TEXM3X3PAD => {
                let s0 = ps_src_expr(*tokens.get(idx).ok_or("truncated ps texm pad src")?)?;
                idx += 1;
                let stage = dst_p.reg_num.min(3);
                texm_rows.push(format!("dot(v_oT{stage}.xyz, ({s0}).xyz)"));
            }
            D3DSIO_TEXM3X2TEX => {
                let s0 = ps_src_expr(*tokens.get(idx).ok_or("truncated ps texm3x2 src")?)?;
                idx += 1;
                let stage = dst_p.reg_num.min(3);
                let u = texm_rows.first().cloned().unwrap_or_else(|| "0.0".to_owned());
                let v = format!("dot(v_oT{stage}.xyz, ({s0}).xyz)");
                texm_rows.clear();
                writeln!(
                    glsl,
                    "    {dst_name} = texture(u_tex{stage}, vec2({u}, {v}));"
                )
                .unwrap();
            }
            D3DSIO_TEXM3X3TEX | D3DSIO_TEXM3X3VSPEC => {
                let s0 = ps_src_expr(*tokens.get(idx).ok_or("truncated ps texm3x3 src")?)?;
                idx += 1;
                let stage = dst_p.reg_num.min(3);
                let u = texm_rows.first().cloned().unwrap_or_else(|| "0.0".to_owned());
                let v = texm_rows.get(1).cloned().unwrap_or_else(|| "0.0".to_owned());
                let _w = format!("dot(v_oT{stage}.xyz, ({s0}).xyz)");
                texm_rows.clear();
                writeln!(
                    glsl,
                    "    {dst_name} = texture(u_tex{stage}, vec2({u}, {v}));"
                )
                .unwrap();
            }
            D3DSIO_MOV => {
                let s0 = ps_src_expr(*tokens.get(idx).ok_or("truncated ps mov")?)?;
                idx += 1;
                emit_masked_assign(&mut glsl, &dst_name, dst_p, &s0, true);
            }
            D3DSIO_ADD | D3DSIO_SUB | D3DSIO_MUL | D3DSIO_DP3 | D3DSIO_DP4 => {
                let s0 = ps_src_expr(*tokens.get(idx).ok_or("truncated ps binop s0")?)?;
                let s1 = ps_src_expr(*tokens.get(idx + 1).ok_or("truncated ps binop s1")?)?;
                idx += 2;
                let expr = match opcode {
                    D3DSIO_ADD => format!("({s0} + {s1})"),
                    D3DSIO_SUB => format!("({s0} - {s1})"),
                    D3DSIO_MUL => format!("({s0} * {s1})"),
                    D3DSIO_DP3 => format!("vec4(dot(({s0}).xyz, ({s1}).xyz))"),
                    D3DSIO_DP4 => format!("vec4(dot({s0}, {s1}))"),
                    _ => unreachable!(),
                };
                emit_masked_assign(&mut glsl, &dst_name, dst_p, &expr, true);
            }
            D3DSIO_MAD | D3DSIO_LRP | D3DSIO_CND | D3DSIO_CMP => {
                let s0 = ps_src_expr(*tokens.get(idx).ok_or("truncated ps triop s0")?)?;
                let s1 = ps_src_expr(*tokens.get(idx + 1).ok_or("truncated ps triop s1")?)?;
                let s2 = ps_src_expr(*tokens.get(idx + 2).ok_or("truncated ps triop s2")?)?;
                idx += 3;
                let expr = match opcode {
                    D3DSIO_MAD => format!("({s0} * {s1} + {s2})"),
                    D3DSIO_LRP => format!("mix({s2}, {s1}, {s0})"),
                    D3DSIO_CND => format!("mix({s2}, {s1}, greaterThan({s0}, vec4(0.5)))"),
                    D3DSIO_CMP => format!("mix({s2}, {s1}, greaterThanEqual({s0}, vec4(0.0)))"),
                    _ => unreachable!(),
                };
                emit_masked_assign(&mut glsl, &dst_name, dst_p, &expr, true);
            }
            other => return Err(format!("unsupported ps_1_x opcode {other}")),
        }
    }

    writeln!(glsl, "    vec4 current = r0;").unwrap();
    if u32::from(fog_mode) != D3DFOG_NONE {
        writeln!(
            glsl,
            "    current.rgb = mix(u_fog_color.rgb, current.rgb, clamp(v_oFog, 0.0, 1.0));"
        )
        .unwrap();
    }
    emit_alpha_test(&mut glsl, alpha_test_func);
    writeln!(glsl, "    frag_color = current;").unwrap();
    writeln!(glsl, "}}").unwrap();
    Ok(glsl)
}
