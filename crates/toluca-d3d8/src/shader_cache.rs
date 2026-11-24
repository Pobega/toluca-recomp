use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::shader_ffp::{FfpShaderKey, MAX_FFP_LIGHTS, MAX_FFP_STAGES, TextureStageKey};

pub const SHADER_CACHE_MAGIC: &[u8; 4] = b"TLCP";
pub const SHADER_CACHE_VERSION: u32 = 2;

const TAG_FFP: u8 = 0;
const TAG_PROGRAMMABLE: u8 = 1;
const HEADER_SIZE: usize = 12;
const CHECKSUM_SIZE: usize = 8;
const MAX_TOKEN_ARRAY_LEN: usize = 65536;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ShaderRecipe {
    Ffp(FfpShaderKey),
    Programmable {
        decl_tokens: Vec<u32>,
        vs_tokens: Vec<u32>,
        ps_tokens: Vec<u32>,
        alpha_test_func: u8,
        fog_mode: u8,
        ffp_stages: [TextureStageKey; MAX_FFP_STAGES],
        specular_enabled: bool,
    },
}

#[derive(Clone, Debug, Default)]
pub struct ShaderRecipeCache {
    recipes: Vec<ShaderRecipe>,
    seen: HashSet<ShaderRecipe>,
}

impl ShaderRecipeCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, recipe: ShaderRecipe) -> bool {
        if self.seen.insert(recipe.clone()) {
            self.recipes.push(recipe);
            true
        } else {
            false
        }
    }

    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }

    pub fn recipes(&self) -> &[ShaderRecipe] {
        &self.recipes
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_SIZE + self.recipes.len() * 64 + CHECKSUM_SIZE);
        buf.extend_from_slice(SHADER_CACHE_MAGIC);
        buf.extend_from_slice(&SHADER_CACHE_VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.recipes.len() as u32).to_le_bytes());

        for recipe in &self.recipes {
            match recipe {
                ShaderRecipe::Ffp(key) => {
                    buf.push(TAG_FFP);
                    encode_ffp_key(&mut buf, key);
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
                    buf.push(TAG_PROGRAMMABLE);
                    encode_u32_vec(&mut buf, decl_tokens);
                    encode_u32_vec(&mut buf, vs_tokens);
                    encode_u32_vec(&mut buf, ps_tokens);
                    buf.push(*alpha_test_func);
                    buf.push(*fog_mode);
                    encode_stages(&mut buf, ffp_stages);
                    buf.push(u8::from(*specular_enabled));
                }
            }
        }

        let checksum = fnv1a_64(&buf);
        buf.extend_from_slice(&checksum.to_le_bytes());
        buf
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < HEADER_SIZE + CHECKSUM_SIZE {
            return Err("shader cache too small".to_owned());
        }
        if &bytes[0..4] != SHADER_CACHE_MAGIC {
            return Err("invalid shader cache magic".to_owned());
        }
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if version != SHADER_CACHE_VERSION {
            return Err(format!("unsupported shader cache version {version}"));
        }

        let payload_end = bytes.len() - CHECKSUM_SIZE;
        let expected_sum = u64::from_le_bytes(bytes[payload_end..].try_into().unwrap());
        let actual_sum = fnv1a_64(&bytes[..payload_end]);
        if actual_sum != expected_sum {
            return Err(format!(
                "shader cache checksum mismatch: expected 0x{expected_sum:016x}, got 0x{actual_sum:016x}"
            ));
        }

        let entry_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let mut cursor = ByteCursor::new(&bytes[HEADER_SIZE..payload_end]);
        let mut cache = Self::new();

        for _ in 0..entry_count {
            let tag = cursor.read_u8()?;
            let recipe = match tag {
                TAG_FFP => ShaderRecipe::Ffp(decode_ffp_key(&mut cursor)?),
                TAG_PROGRAMMABLE => {
                    let decl_tokens = decode_u32_vec(&mut cursor)?;
                    let vs_tokens = decode_u32_vec(&mut cursor)?;
                    let ps_tokens = decode_u32_vec(&mut cursor)?;
                    let alpha_test_func = cursor.read_u8()?;
                    let fog_mode = cursor.read_u8()?;
                    let ffp_stages = decode_stages(&mut cursor)?;
                    let specular_enabled = cursor.read_bool()?;
                    ShaderRecipe::Programmable {
                        decl_tokens,
                        vs_tokens,
                        ps_tokens,
                        alpha_test_func,
                        fog_mode,
                        ffp_stages,
                        specular_enabled,
                    }
                }
                other => return Err(format!("unknown shader recipe tag {other}")),
            };
            cache.insert(recipe);
        }

        if !cursor.is_empty() {
            return Err("trailing bytes in shader cache payload".to_owned());
        }

        Ok(cache)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        let bytes = self.serialize();
        fs::write(path, bytes).map_err(|err| format!("failed to write {}: {err}", path.display()))
    }

    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let bytes =
            fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        Self::deserialize(&bytes)
    }
}

fn encode_stages(buf: &mut Vec<u8>, stages: &[TextureStageKey; MAX_FFP_STAGES]) {
    for stage in stages {
        buf.push(stage.color_op);
        buf.push(stage.color_arg1);
        buf.push(stage.color_arg2);
        buf.push(stage.alpha_op);
        buf.push(stage.alpha_arg1);
        buf.push(stage.alpha_arg2);
        buf.push(stage.texcoord_index);
        buf.push(u8::from(stage.tex_bound));
        buf.push(stage.tex_transform_dim);
    }
}

fn decode_stages(cur: &mut ByteCursor<'_>) -> Result<[TextureStageKey; MAX_FFP_STAGES], String> {
    let mut stages = [TextureStageKey::default(); MAX_FFP_STAGES];
    for stage in &mut stages {
        *stage = TextureStageKey {
            color_op: cur.read_u8()?,
            color_arg1: cur.read_u8()?,
            color_arg2: cur.read_u8()?,
            alpha_op: cur.read_u8()?,
            alpha_arg1: cur.read_u8()?,
            alpha_arg2: cur.read_u8()?,
            texcoord_index: cur.read_u8()?,
            tex_bound: cur.read_bool()?,
            tex_transform_dim: cur.read_u8()?,
        };
    }
    Ok(stages)
}

fn fnv1a_64(data: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xCBF2_9CE4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01B3;
    let mut hash = OFFSET_BASIS;
    for &b in data {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn encode_u32_vec(buf: &mut Vec<u8>, items: &[u32]) {
    buf.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for &item in items {
        buf.extend_from_slice(&item.to_le_bytes());
    }
}

fn encode_ffp_key(buf: &mut Vec<u8>, key: &FfpShaderKey) {
    buf.push(u8::from(key.is_transformed));
    buf.push(u8::from(key.has_normal));
    buf.push(u8::from(key.has_diffuse));
    buf.push(u8::from(key.has_specular));
    buf.push(key.texcoord_count);
    buf.push(u8::from(key.lighting_enabled));
    buf.push(u8::from(key.specular_enabled));
    buf.push(key.active_light_mask);
    buf.extend_from_slice(&key.light_types);
    buf.push(key.fog_mode);
    buf.push(key.alpha_test_func);
    for stage in &key.stages {
        buf.push(stage.color_op);
        buf.push(stage.color_arg1);
        buf.push(stage.color_arg2);
        buf.push(stage.alpha_op);
        buf.push(stage.alpha_arg1);
        buf.push(stage.alpha_arg2);
        buf.push(stage.texcoord_index);
        buf.push(u8::from(stage.tex_bound));
        buf.push(stage.tex_transform_dim);
    }
}

struct ByteCursor<'a> {
    slice: &'a [u8],
    pos: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(slice: &'a [u8]) -> Self {
        Self { slice, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos == self.slice.len()
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        let b = *self
            .slice
            .get(self.pos)
            .ok_or("unexpected EOF in shader cache")?;
        self.pos += 1;
        Ok(b)
    }

    fn read_bool(&mut self) -> Result<bool, String> {
        Ok(self.read_u8()? != 0)
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        if self.pos + 4 > self.slice.len() {
            return Err("unexpected EOF reading u32 in shader cache".to_owned());
        }
        let val = u32::from_le_bytes(self.slice[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(val)
    }
}

fn decode_u32_vec(cur: &mut ByteCursor<'_>) -> Result<Vec<u32>, String> {
    let len = cur.read_u32()? as usize;
    if len > MAX_TOKEN_ARRAY_LEN {
        return Err(format!("shader token array length {len} exceeds limit"));
    }
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(cur.read_u32()?);
    }
    Ok(out)
}

fn decode_ffp_key(cur: &mut ByteCursor<'_>) -> Result<FfpShaderKey, String> {
    let is_transformed = cur.read_bool()?;
    let has_normal = cur.read_bool()?;
    let has_diffuse = cur.read_bool()?;
    let has_specular = cur.read_bool()?;
    let texcoord_count = cur.read_u8()?;
    let lighting_enabled = cur.read_bool()?;
    let specular_enabled = cur.read_bool()?;
    let active_light_mask = cur.read_u8()?;
    let mut light_types = [0u8; MAX_FFP_LIGHTS];
    for slot in &mut light_types {
        *slot = cur.read_u8()?;
    }
    let fog_mode = cur.read_u8()?;
    let alpha_test_func = cur.read_u8()?;
    let mut stages = [TextureStageKey::default(); MAX_FFP_STAGES];
    for stage in &mut stages {
        *stage = TextureStageKey {
            color_op: cur.read_u8()?,
            color_arg1: cur.read_u8()?,
            color_arg2: cur.read_u8()?,
            alpha_op: cur.read_u8()?,
            alpha_arg1: cur.read_u8()?,
            alpha_arg2: cur.read_u8()?,
            texcoord_index: cur.read_u8()?,
            tex_bound: cur.read_bool()?,
            tex_transform_dim: cur.read_u8()?,
        };
    }
    Ok(FfpShaderKey {
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
    })
}
