use std::collections::HashMap;

use crate::types::{
    D3DBLEND_ONE, D3DBLEND_ZERO, D3DBLENDOP_ADD, D3DCMP_ALWAYS, D3DCMP_LESSEQUAL, D3DCULL_CCW,
    D3DFILL_SOLID, D3DMCS_COLOR1, D3DMCS_COLOR2, D3DMCS_MATERIAL, D3DORDER_CUBIC, D3DORDER_LINEAR,
    D3DRS_ALPHABLENDENABLE, D3DRS_ALPHAFUNC, D3DRS_ALPHAREF, D3DRS_ALPHATESTENABLE, D3DRS_AMBIENT,
    D3DRS_AMBIENTMATERIALSOURCE, D3DRS_BLENDOP, D3DRS_CLIPPING, D3DRS_CLIPPLANEENABLE,
    D3DRS_COLORVERTEX, D3DRS_COLORWRITEENABLE, D3DRS_CULLMODE, D3DRS_DESTBLEND,
    D3DRS_DIFFUSEMATERIALSOURCE, D3DRS_DITHERENABLE, D3DRS_EDGEANTIALIAS,
    D3DRS_EMISSIVEMATERIALSOURCE, D3DRS_FILLMODE, D3DRS_FOGCOLOR, D3DRS_FOGDENSITY,
    D3DRS_FOGENABLE, D3DRS_FOGEND, D3DRS_FOGSTART, D3DRS_FOGTABLEMODE, D3DRS_FOGVERTEXMODE,
    D3DRS_INDEXEDVERTEXBLENDENABLE, D3DRS_LASTPIXEL, D3DRS_LIGHTING, D3DRS_LINEPATTERN,
    D3DRS_LOCALVIEWER, D3DRS_MULTISAMPLEANTIALIAS, D3DRS_MULTISAMPLEMASK,
    D3DRS_NORMALIZENORMALS, D3DRS_NORMALORDER, D3DRS_PATCHEDGESTYLE, D3DRS_PATCHSEGMENTS,
    D3DRS_POINTSCALE_A, D3DRS_POINTSCALE_B, D3DRS_POINTSCALE_C, D3DRS_POINTSCALEENABLE,
    D3DRS_POINTSIZE, D3DRS_POINTSIZE_MAX, D3DRS_POINTSIZE_MIN, D3DRS_POINTSPRITEENABLE,
    D3DRS_POSITIONORDER, D3DRS_RANGEFOGENABLE, D3DRS_SHADEMODE, D3DRS_SOFTWAREVERTEXPROCESSING,
    D3DRS_SPECULARENABLE, D3DRS_SPECULARMATERIALSOURCE, D3DRS_SRCBLEND, D3DRS_STENCILENABLE,
    D3DRS_STENCILFAIL, D3DRS_STENCILFUNC, D3DRS_STENCILMASK, D3DRS_STENCILPASS, D3DRS_STENCILREF,
    D3DRS_STENCILWRITEMASK, D3DRS_STENCILZFAIL, D3DRS_TEXTUREFACTOR, D3DRS_TWEENFACTOR,
    D3DRS_VERTEXBLEND, D3DRS_ZBIAS, D3DRS_ZENABLE, D3DRS_ZFUNC, D3DRS_ZVISIBLE,
    D3DRS_ZWRITEENABLE, D3DSBT_ALL, D3DSBT_PIXELSTATE, D3DSBT_VERTEXSTATE, D3DSHADE_GOURAUD,
    D3DSTENCILOP_KEEP, D3DTA_CURRENT, D3DTA_TEXTURE, D3DTADDRESS_WRAP, D3DTEXF_NONE, D3DTEXF_POINT,
    D3DTOP_DISABLE, D3DTOP_MODULATE, D3DTOP_SELECTARG1, D3DTS_PROJECTION, D3DTS_TEXTURE0,
    D3DTS_TEXTURE7, D3DTS_VIEW, D3DTS_WORLD, D3DTS_WORLD3, D3DTSS_ADDRESSU, D3DTSS_ADDRESSV,
    D3DTSS_ADDRESSW, D3DTSS_ALPHAARG0, D3DTSS_ALPHAARG1, D3DTSS_ALPHAARG2, D3DTSS_ALPHAOP,
    D3DTSS_COLORARG0, D3DTSS_COLORARG1, D3DTSS_COLORARG2, D3DTSS_COLOROP, D3DTSS_MAGFILTER,
    D3DTSS_MAXANISOTROPY, D3DTSS_MAXMIPLEVEL, D3DTSS_MINFILTER, D3DTSS_MIPFILTER,
    D3DTSS_RESULTARG, D3DTSS_TEXCOORDINDEX, D3DTSS_TEXTURETRANSFORMFLAGS, D3dLight8,
    D3dMaterial8, D3dViewport8,
};

pub const NUM_RENDER_STATES: usize = 256;
pub const NUM_TEXTURE_STAGES: usize = 8;
pub const NUM_TSS_SLOTS: usize = 32;
pub const NUM_STREAM_SOURCES: usize = 16;
pub const NUM_VS_CONSTANTS: usize = 96;
pub const NUM_PS_CONSTANTS: usize = 8;
pub const NUM_CLIP_PLANES: usize = 6;

pub const IDENTITY_MATRIX_4X4: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 1.0, 0.0,
    0.0, 0.0, 0.0, 1.0,
];

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct LightSlot {
    pub light: D3dLight8,
    pub enabled: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct StreamBinding {
    pub vb_handle: u32,
    pub stride: u32,
}

#[derive(Clone, Debug, Default)]
pub struct StateBlockData {
    pub render_states: HashMap<u32, u32>,
    pub tss: HashMap<(u32, u32), u32>,
    pub transforms: HashMap<u32, [f32; 16]>,
    pub viewport: Option<D3dViewport8>,
    pub material: Option<D3dMaterial8>,
    pub lights: HashMap<u32, LightSlot>,
    pub clip_planes: HashMap<u32, [f32; 4]>,
    pub textures: HashMap<u32, Option<u32>>,
    pub streams: HashMap<u32, Option<StreamBinding>>,
    pub indices: Option<Option<(u32, i32)>>,
    pub vertex_shader: Option<u32>,
    pub pixel_shader: Option<u32>,
    pub vs_constants: HashMap<u32, [f32; 4]>,
    pub ps_constants: HashMap<u32, [f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct DeviceState {
    pub render_states: [u32; NUM_RENDER_STATES],
    pub texture_stage_states: [[u32; NUM_TSS_SLOTS]; NUM_TEXTURE_STAGES],
    pub transforms: HashMap<u32, [f32; 16]>,
    pub viewport: D3dViewport8,
    pub material: D3dMaterial8,
    pub lights: Vec<LightSlot>,
    pub clip_planes: [[f32; 4]; NUM_CLIP_PLANES],
    pub bound_textures: [Option<u32>; NUM_TEXTURE_STAGES],
    pub streams: [Option<StreamBinding>; NUM_STREAM_SOURCES],
    pub bound_indices: Option<(u32, i32)>,
    pub current_vertex_shader: u32,
    pub current_pixel_shader: u32,
    pub vs_constants: [[f32; 4]; NUM_VS_CONSTANTS],
    pub ps_constants: [[f32; 4]; NUM_PS_CONSTANTS],
    pub palette_entries: HashMap<u32, Vec<[u8; 4]>>,
    pub current_palette: u32,
    pub render_target: Option<u32>,
    pub depth_stencil: Option<u32>,
    pub in_scene: bool,
    pub frame_count: u64,
    pub recording_state_block: Option<StateBlockData>,
    transforms_gen: u64,
    render_state_gen: u64,
    lights_gen: u64,
    material_gen: u64,
    vs_const_gen: u64,
    ps_const_gen: u64,
}

impl DeviceState {
    pub fn new(width: u32, height: u32, z_enable: bool) -> Self {
        let mut state = Self {
            render_states: default_render_states(z_enable),
            texture_stage_states: default_texture_stage_states(),
            transforms: HashMap::new(),
            viewport: D3dViewport8 {
                x: 0,
                y: 0,
                width,
                height,
                min_z: 0.0,
                max_z: 1.0,
            },
            material: D3dMaterial8::default(),
            lights: Vec::new(),
            clip_planes: [[0.0; 4]; NUM_CLIP_PLANES],
            bound_textures: [None; NUM_TEXTURE_STAGES],
            streams: [None; NUM_STREAM_SOURCES],
            bound_indices: None,
            current_vertex_shader: 0,
            current_pixel_shader: 0,
            vs_constants: [[0.0; 4]; NUM_VS_CONSTANTS],
            ps_constants: [[0.0; 4]; NUM_PS_CONSTANTS],
            palette_entries: HashMap::new(),
            current_palette: 0,
            render_target: None,
            depth_stencil: None,
            in_scene: false,
            frame_count: 0,
            recording_state_block: None,
            transforms_gen: 1,
            render_state_gen: 1,
            lights_gen: 1,
            material_gen: 1,
            vs_const_gen: 1,
            ps_const_gen: 1,
        };
        state.transforms.insert(D3DTS_VIEW, IDENTITY_MATRIX_4X4);
        state.transforms.insert(D3DTS_PROJECTION, IDENTITY_MATRIX_4X4);
        state.transforms.insert(D3DTS_WORLD, IDENTITY_MATRIX_4X4);
        state
    }

    pub fn reset(&mut self, width: u32, height: u32, z_enable: bool) {
        let frame_count = self.frame_count;
        let transforms_gen = self.transforms_gen + 1;
        let render_state_gen = self.render_state_gen + 1;
        let lights_gen = self.lights_gen + 1;
        let material_gen = self.material_gen + 1;
        let vs_const_gen = self.vs_const_gen + 1;
        let ps_const_gen = self.ps_const_gen + 1;
        *self = Self::new(width, height, z_enable);
        self.frame_count = frame_count;
        self.transforms_gen = transforms_gen;
        self.render_state_gen = render_state_gen;
        self.lights_gen = lights_gen;
        self.material_gen = material_gen;
        self.vs_const_gen = vs_const_gen;
        self.ps_const_gen = ps_const_gen;
    }

    pub fn transforms_gen(&self) -> u64 {
        self.transforms_gen
    }

    pub fn render_state_gen(&self) -> u64 {
        self.render_state_gen
    }

    pub fn lights_gen(&self) -> u64 {
        self.lights_gen
    }

    pub fn material_gen(&self) -> u64 {
        self.material_gen
    }

    pub fn vs_const_gen(&self) -> u64 {
        self.vs_const_gen
    }

    pub fn ps_const_gen(&self) -> u64 {
        self.ps_const_gen
    }

    pub fn get_transform(&self, state: u32) -> [f32; 16] {
        self.transforms
            .get(&state)
            .copied()
            .unwrap_or(IDENTITY_MATRIX_4X4)
    }

    pub fn set_transform(&mut self, state: u32, mat: [f32; 16]) {
        if let Some(sb) = &mut self.recording_state_block {
            sb.transforms.insert(state, mat);
        } else {
            self.transforms.insert(state, mat);
            self.transforms_gen += 1;
        }
    }

    pub fn multiply_transform(&mut self, state: u32, mat: [f32; 16]) {
        let cur = self.get_transform(state);
        let result = mul_matrix_4x4(&mat, &cur);
        self.set_transform(state, result);
    }

    pub fn set_render_state(&mut self, state: u32, value: u32) {
        if let Some(sb) = &mut self.recording_state_block {
            sb.render_states.insert(state, value);
        } else if (state as usize) < NUM_RENDER_STATES {
            let slot = &mut self.render_states[state as usize];
            if *slot != value {
                *slot = value;
                self.render_state_gen += 1;
            }
        }
    }

    pub fn get_render_state(&self, state: u32) -> u32 {
        self.render_states
            .get(state as usize)
            .copied()
            .unwrap_or(0)
    }

    pub fn set_texture_stage_state(&mut self, stage: u32, tss_type: u32, value: u32) {
        if (stage as usize) >= NUM_TEXTURE_STAGES || (tss_type as usize) >= NUM_TSS_SLOTS {
            return;
        }
        if let Some(sb) = &mut self.recording_state_block {
            sb.tss.insert((stage, tss_type), value);
        } else {
            self.texture_stage_states[stage as usize][tss_type as usize] = value;
        }
    }

    pub fn get_texture_stage_state(&self, stage: u32, tss_type: u32) -> u32 {
        self.texture_stage_states
            .get(stage as usize)
            .and_then(|row| row.get(tss_type as usize))
            .copied()
            .unwrap_or(0)
    }

    pub fn set_viewport(&mut self, viewport: D3dViewport8) {
        if let Some(sb) = &mut self.recording_state_block {
            sb.viewport = Some(viewport);
        } else {
            self.viewport = viewport;
        }
    }

    pub fn set_material(&mut self, material: D3dMaterial8) {
        if let Some(sb) = &mut self.recording_state_block {
            sb.material = Some(material);
        } else {
            self.material = material;
            self.material_gen += 1;
        }
    }

    pub fn set_light(&mut self, index: u32, light: D3dLight8) {
        let idx = index as usize;
        if let Some(sb) = &mut self.recording_state_block {
            let enabled = sb
                .lights
                .get(&index)
                .map(|slot| slot.enabled)
                .unwrap_or_else(|| self.lights.get(idx).is_some_and(|s| s.enabled));
            sb.lights.insert(index, LightSlot { light, enabled });
        } else {
            if idx >= self.lights.len() {
                self.lights.resize(idx + 1, LightSlot::default());
            }
            self.lights[idx].light = light;
            self.lights_gen += 1;
        }
    }

    pub fn light_enable(&mut self, index: u32, enable: bool) {
        let idx = index as usize;
        if let Some(sb) = &mut self.recording_state_block {
            let light = sb
                .lights
                .get(&index)
                .map(|slot| slot.light)
                .unwrap_or_else(|| self.lights.get(idx).map(|s| s.light).unwrap_or_default());
            sb.lights.insert(
                index,
                LightSlot {
                    light,
                    enabled: enable,
                },
            );
        } else {
            if idx >= self.lights.len() {
                self.lights.resize(idx + 1, LightSlot::default());
            }
            self.lights[idx].enabled = enable;
            self.lights_gen += 1;
        }
    }

    pub fn set_clip_plane(&mut self, index: u32, plane: [f32; 4]) {
        if (index as usize) >= NUM_CLIP_PLANES {
            return;
        }
        if let Some(sb) = &mut self.recording_state_block {
            sb.clip_planes.insert(index, plane);
        } else {
            self.clip_planes[index as usize] = plane;
        }
    }

    pub fn set_texture(&mut self, stage: u32, tex_handle: Option<u32>) {
        if (stage as usize) >= NUM_TEXTURE_STAGES {
            return;
        }
        if let Some(sb) = &mut self.recording_state_block {
            sb.textures.insert(stage, tex_handle);
        } else {
            self.bound_textures[stage as usize] = tex_handle;
        }
    }

    pub fn set_stream_source(&mut self, stream: u32, binding: Option<StreamBinding>) {
        if (stream as usize) >= NUM_STREAM_SOURCES {
            return;
        }
        if let Some(sb) = &mut self.recording_state_block {
            sb.streams.insert(stream, binding);
        } else {
            self.streams[stream as usize] = binding;
        }
    }

    pub fn set_indices(&mut self, binding: Option<(u32, i32)>) {
        if let Some(sb) = &mut self.recording_state_block {
            sb.indices = Some(binding);
        } else {
            self.bound_indices = binding;
        }
    }

    pub fn set_vertex_shader(&mut self, handle: u32) {
        if let Some(sb) = &mut self.recording_state_block {
            sb.vertex_shader = Some(handle);
        } else {
            self.current_vertex_shader = handle;
        }
    }

    pub fn set_pixel_shader(&mut self, handle: u32) {
        if let Some(sb) = &mut self.recording_state_block {
            sb.pixel_shader = Some(handle);
        } else {
            self.current_pixel_shader = handle;
        }
    }

    pub fn set_vs_constants(&mut self, start_reg: u32, regs: &[[f32; 4]]) {
        let mut mutated = false;
        for (i, &val) in regs.iter().enumerate() {
            let reg = start_reg + i as u32;
            if (reg as usize) >= NUM_VS_CONSTANTS {
                break;
            }
            if let Some(sb) = &mut self.recording_state_block {
                sb.vs_constants.insert(reg, val);
            } else {
                self.vs_constants[reg as usize] = val;
                mutated = true;
            }
        }
        if mutated {
            self.vs_const_gen += 1;
        }
    }

    pub fn set_ps_constants(&mut self, start_reg: u32, regs: &[[f32; 4]]) {
        let mut mutated = false;
        for (i, &val) in regs.iter().enumerate() {
            let reg = start_reg + i as u32;
            if (reg as usize) >= NUM_PS_CONSTANTS {
                break;
            }
            if let Some(sb) = &mut self.recording_state_block {
                sb.ps_constants.insert(reg, val);
            } else {
                self.ps_constants[reg as usize] = val;
                mutated = true;
            }
        }
        if mutated {
            self.ps_const_gen += 1;
        }
    }

    pub fn snapshot_state_block(&self, block_type: u32) -> Option<StateBlockData> {
        let mut sb = StateBlockData::default();
        let include_pixel = block_type == D3DSBT_ALL || block_type == D3DSBT_PIXELSTATE;
        let include_vertex = block_type == D3DSBT_ALL || block_type == D3DSBT_VERTEXSTATE;
        if !include_pixel && !include_vertex {
            return None;
        }

        for rs in 0..NUM_RENDER_STATES {
            let rs_u32 = rs as u32;
            let keep = block_type == D3DSBT_ALL
                || (include_pixel && is_pixel_render_state(rs_u32))
                || (include_vertex && is_vertex_render_state(rs_u32));
            if keep {
                sb.render_states.insert(rs_u32, self.render_states[rs]);
            }
        }

        for stage in 0..NUM_TEXTURE_STAGES {
            for tss in 1..NUM_TSS_SLOTS {
                let tss_u32 = tss as u32;
                let keep = block_type == D3DSBT_ALL
                    || (include_pixel && !is_vertex_tss(tss_u32))
                    || (include_vertex && is_vertex_tss(tss_u32));
                if keep {
                    sb.tss
                        .insert((stage as u32, tss_u32), self.texture_stage_states[stage][tss]);
                }
            }
        }

        if include_pixel {
            sb.pixel_shader = Some(self.current_pixel_shader);
            for (reg, &val) in self.ps_constants.iter().enumerate() {
                sb.ps_constants.insert(reg as u32, val);
            }
        }

        if include_vertex {
            sb.vertex_shader = Some(self.current_vertex_shader);
            for (reg, &val) in self.vs_constants.iter().enumerate() {
                sb.vs_constants.insert(reg as u32, val);
            }
            for (idx, slot) in self.lights.iter().enumerate() {
                sb.lights.insert(idx as u32, *slot);
            }
        }

        if block_type == D3DSBT_ALL {
            sb.transforms = self.transforms.clone();
            sb.viewport = Some(self.viewport);
            sb.material = Some(self.material);
            for (idx, &plane) in self.clip_planes.iter().enumerate() {
                sb.clip_planes.insert(idx as u32, plane);
            }
            for (stage, &tex) in self.bound_textures.iter().enumerate() {
                sb.textures.insert(stage as u32, tex);
            }
            for (stream, &binding) in self.streams.iter().enumerate() {
                sb.streams.insert(stream as u32, binding);
            }
            sb.indices = Some(self.bound_indices);
        } else if include_vertex {
            for ts in [D3DTS_VIEW, D3DTS_PROJECTION]
                .into_iter()
                .chain(D3DTS_TEXTURE0..=D3DTS_TEXTURE7)
                .chain(D3DTS_WORLD..=D3DTS_WORLD3)
            {
                if let Some(&mat) = self.transforms.get(&ts) {
                    sb.transforms.insert(ts, mat);
                }
            }
        }

        Some(sb)
    }

    pub fn apply_state_block(&mut self, sb: &StateBlockData) {
        let mut rs_mutated = false;
        for (&rs, &val) in &sb.render_states {
            if (rs as usize) < NUM_RENDER_STATES {
                let slot = &mut self.render_states[rs as usize];
                if *slot != val {
                    *slot = val;
                    rs_mutated = true;
                }
            }
        }
        if rs_mutated {
            self.render_state_gen += 1;
        }
        for (&(stage, tss), &val) in &sb.tss {
            if (stage as usize) < NUM_TEXTURE_STAGES && (tss as usize) < NUM_TSS_SLOTS {
                self.texture_stage_states[stage as usize][tss as usize] = val;
            }
        }
        if !sb.transforms.is_empty() {
            for (&ts, &mat) in &sb.transforms {
                self.transforms.insert(ts, mat);
            }
            self.transforms_gen += 1;
        }
        if let Some(vp) = sb.viewport {
            self.viewport = vp;
        }
        if let Some(mat) = sb.material {
            self.material = mat;
            self.material_gen += 1;
        }
        if !sb.lights.is_empty() {
            for (&idx, &slot) in &sb.lights {
                let idx_usize = idx as usize;
                if idx_usize >= self.lights.len() {
                    self.lights.resize(idx_usize + 1, LightSlot::default());
                }
                self.lights[idx_usize] = slot;
            }
            self.lights_gen += 1;
        }
        for (&idx, &plane) in &sb.clip_planes {
            if (idx as usize) < NUM_CLIP_PLANES {
                self.clip_planes[idx as usize] = plane;
            }
        }
        for (&stage, &tex) in &sb.textures {
            if (stage as usize) < NUM_TEXTURE_STAGES {
                self.bound_textures[stage as usize] = tex;
            }
        }
        for (&stream, &binding) in &sb.streams {
            if (stream as usize) < NUM_STREAM_SOURCES {
                self.streams[stream as usize] = binding;
            }
        }
        if let Some(indices) = sb.indices {
            self.bound_indices = indices;
        }
        if let Some(vs) = sb.vertex_shader {
            self.current_vertex_shader = vs;
        }
        if let Some(ps) = sb.pixel_shader {
            self.current_pixel_shader = ps;
        }
        let mut vs_mutated = false;
        for (&reg, &val) in &sb.vs_constants {
            if (reg as usize) < NUM_VS_CONSTANTS {
                self.vs_constants[reg as usize] = val;
                vs_mutated = true;
            }
        }
        if vs_mutated {
            self.vs_const_gen += 1;
        }
        let mut ps_mutated = false;
        for (&reg, &val) in &sb.ps_constants {
            if (reg as usize) < NUM_PS_CONSTANTS {
                self.ps_constants[reg as usize] = val;
                ps_mutated = true;
            }
        }
        if ps_mutated {
            self.ps_const_gen += 1;
        }
    }

    pub fn capture_state_block(&self, sb: &mut StateBlockData) {
        for (&rs, val) in &mut sb.render_states {
            *val = self.get_render_state(rs);
        }
        for (&(stage, tss), val) in &mut sb.tss {
            *val = self.get_texture_stage_state(stage, tss);
        }
        for (&ts, mat) in &mut sb.transforms {
            *mat = self.get_transform(ts);
        }
        if sb.viewport.is_some() {
            sb.viewport = Some(self.viewport);
        }
        if sb.material.is_some() {
            sb.material = Some(self.material);
        }
        for (&idx, slot) in &mut sb.lights {
            if let Some(&cur) = self.lights.get(idx as usize) {
                *slot = cur;
            }
        }
        for (&idx, plane) in &mut sb.clip_planes {
            if let Some(&cur) = self.clip_planes.get(idx as usize) {
                *plane = cur;
            }
        }
        for (&stage, tex) in &mut sb.textures {
            if let Some(&cur) = self.bound_textures.get(stage as usize) {
                *tex = cur;
            }
        }
        for (&stream, binding) in &mut sb.streams {
            if let Some(&cur) = self.streams.get(stream as usize) {
                *binding = cur;
            }
        }
        if sb.indices.is_some() {
            sb.indices = Some(self.bound_indices);
        }
        if sb.vertex_shader.is_some() {
            sb.vertex_shader = Some(self.current_vertex_shader);
        }
        if sb.pixel_shader.is_some() {
            sb.pixel_shader = Some(self.current_pixel_shader);
        }
        for (&reg, val) in &mut sb.vs_constants {
            if let Some(&cur) = self.vs_constants.get(reg as usize) {
                *val = cur;
            }
        }
        for (&reg, val) in &mut sb.ps_constants {
            if let Some(&cur) = self.ps_constants.get(reg as usize) {
                *val = cur;
            }
        }
    }
}

fn default_render_states(z_enable: bool) -> [u32; NUM_RENDER_STATES] {
    let mut rs = [0u32; NUM_RENDER_STATES];
    rs[D3DRS_ZENABLE as usize] = u32::from(z_enable);
    rs[D3DRS_FILLMODE as usize] = D3DFILL_SOLID;
    rs[D3DRS_SHADEMODE as usize] = D3DSHADE_GOURAUD;
    rs[D3DRS_LINEPATTERN as usize] = 0;
    rs[D3DRS_ZWRITEENABLE as usize] = 1;
    rs[D3DRS_ALPHATESTENABLE as usize] = 0;
    rs[D3DRS_LASTPIXEL as usize] = 1;
    rs[D3DRS_SRCBLEND as usize] = D3DBLEND_ONE;
    rs[D3DRS_DESTBLEND as usize] = D3DBLEND_ZERO;
    rs[D3DRS_CULLMODE as usize] = D3DCULL_CCW;
    rs[D3DRS_ZFUNC as usize] = D3DCMP_LESSEQUAL;
    rs[D3DRS_ALPHAREF as usize] = 0;
    rs[D3DRS_ALPHAFUNC as usize] = D3DCMP_ALWAYS;
    rs[D3DRS_DITHERENABLE as usize] = 0;
    rs[D3DRS_ALPHABLENDENABLE as usize] = 0;
    rs[D3DRS_FOGENABLE as usize] = 0;
    rs[D3DRS_SPECULARENABLE as usize] = 0;
    rs[D3DRS_ZVISIBLE as usize] = 0;
    rs[D3DRS_FOGCOLOR as usize] = 0;
    rs[D3DRS_FOGTABLEMODE as usize] = 0;
    rs[D3DRS_FOGSTART as usize] = 0.0f32.to_bits();
    rs[D3DRS_FOGEND as usize] = 1.0f32.to_bits();
    rs[D3DRS_FOGDENSITY as usize] = 1.0f32.to_bits();
    rs[D3DRS_EDGEANTIALIAS as usize] = 0;
    rs[D3DRS_ZBIAS as usize] = 0;
    rs[D3DRS_RANGEFOGENABLE as usize] = 0;
    rs[D3DRS_STENCILENABLE as usize] = 0;
    rs[D3DRS_STENCILFAIL as usize] = D3DSTENCILOP_KEEP;
    rs[D3DRS_STENCILZFAIL as usize] = D3DSTENCILOP_KEEP;
    rs[D3DRS_STENCILPASS as usize] = D3DSTENCILOP_KEEP;
    rs[D3DRS_STENCILFUNC as usize] = D3DCMP_ALWAYS;
    rs[D3DRS_STENCILREF as usize] = 0;
    rs[D3DRS_STENCILMASK as usize] = 0xFFFF_FFFF;
    rs[D3DRS_STENCILWRITEMASK as usize] = 0xFFFF_FFFF;
    rs[D3DRS_TEXTUREFACTOR as usize] = 0xFFFF_FFFF;
    rs[D3DRS_CLIPPING as usize] = 1;
    rs[D3DRS_LIGHTING as usize] = 1;
    rs[D3DRS_AMBIENT as usize] = 0;
    rs[D3DRS_FOGVERTEXMODE as usize] = 0;
    rs[D3DRS_COLORVERTEX as usize] = 1;
    rs[D3DRS_LOCALVIEWER as usize] = 1;
    rs[D3DRS_NORMALIZENORMALS as usize] = 0;
    rs[D3DRS_DIFFUSEMATERIALSOURCE as usize] = D3DMCS_COLOR1;
    rs[D3DRS_SPECULARMATERIALSOURCE as usize] = D3DMCS_COLOR2;
    rs[D3DRS_AMBIENTMATERIALSOURCE as usize] = D3DMCS_MATERIAL;
    rs[D3DRS_EMISSIVEMATERIALSOURCE as usize] = D3DMCS_MATERIAL;
    rs[D3DRS_VERTEXBLEND as usize] = 0;
    rs[D3DRS_CLIPPLANEENABLE as usize] = 0;
    rs[D3DRS_SOFTWAREVERTEXPROCESSING as usize] = 0;
    rs[D3DRS_POINTSIZE as usize] = 1.0f32.to_bits();
    rs[D3DRS_POINTSIZE_MIN as usize] = 1.0f32.to_bits();
    rs[D3DRS_POINTSPRITEENABLE as usize] = 0;
    rs[D3DRS_POINTSCALEENABLE as usize] = 0;
    rs[D3DRS_POINTSCALE_A as usize] = 1.0f32.to_bits();
    rs[D3DRS_POINTSCALE_B as usize] = 0.0f32.to_bits();
    rs[D3DRS_POINTSCALE_C as usize] = 0.0f32.to_bits();
    rs[D3DRS_MULTISAMPLEANTIALIAS as usize] = 1;
    rs[D3DRS_MULTISAMPLEMASK as usize] = 0xFFFF_FFFF;
    rs[D3DRS_PATCHEDGESTYLE as usize] = 0;
    rs[D3DRS_PATCHSEGMENTS as usize] = 1.0f32.to_bits();
    rs[D3DRS_POINTSIZE_MAX as usize] = 64.0f32.to_bits();
    rs[D3DRS_INDEXEDVERTEXBLENDENABLE as usize] = 0;
    rs[D3DRS_COLORWRITEENABLE as usize] = 0xF;
    rs[D3DRS_TWEENFACTOR as usize] = 0.0f32.to_bits();
    rs[D3DRS_BLENDOP as usize] = D3DBLENDOP_ADD;
    rs[D3DRS_POSITIONORDER as usize] = D3DORDER_CUBIC;
    rs[D3DRS_NORMALORDER as usize] = D3DORDER_LINEAR;
    rs
}

fn default_texture_stage_states() -> [[u32; NUM_TSS_SLOTS]; NUM_TEXTURE_STAGES] {
    let mut stages = [[0u32; NUM_TSS_SLOTS]; NUM_TEXTURE_STAGES];
    for (stage_idx, row) in stages.iter_mut().enumerate() {
        row[D3DTSS_COLOROP as usize] = if stage_idx == 0 {
            D3DTOP_MODULATE
        } else {
            D3DTOP_DISABLE
        };
        row[D3DTSS_COLORARG1 as usize] = D3DTA_TEXTURE;
        row[D3DTSS_COLORARG2 as usize] = D3DTA_CURRENT;
        row[D3DTSS_ALPHAOP as usize] = if stage_idx == 0 {
            D3DTOP_SELECTARG1
        } else {
            D3DTOP_DISABLE
        };
        row[D3DTSS_ALPHAARG1 as usize] = D3DTA_TEXTURE;
        row[D3DTSS_ALPHAARG2 as usize] = D3DTA_CURRENT;
        row[D3DTSS_TEXCOORDINDEX as usize] = stage_idx as u32;
        row[D3DTSS_ADDRESSU as usize] = D3DTADDRESS_WRAP;
        row[D3DTSS_ADDRESSV as usize] = D3DTADDRESS_WRAP;
        row[D3DTSS_ADDRESSW as usize] = D3DTADDRESS_WRAP;
        row[D3DTSS_MAGFILTER as usize] = D3DTEXF_POINT;
        row[D3DTSS_MINFILTER as usize] = D3DTEXF_POINT;
        row[D3DTSS_MIPFILTER as usize] = D3DTEXF_NONE;
        row[D3DTSS_MAXMIPLEVEL as usize] = 0;
        row[D3DTSS_MAXANISOTROPY as usize] = 1;
        row[D3DTSS_COLORARG0 as usize] = D3DTA_CURRENT;
        row[D3DTSS_ALPHAARG0 as usize] = D3DTA_CURRENT;
        row[D3DTSS_RESULTARG as usize] = D3DTA_CURRENT;
    }
    stages
}

fn is_vertex_tss(tss: u32) -> bool {
    matches!(tss, D3DTSS_TEXCOORDINDEX | D3DTSS_TEXTURETRANSFORMFLAGS)
}

fn is_vertex_render_state(rs: u32) -> bool {
    matches!(
        rs,
        D3DRS_CULLMODE
            | D3DRS_FOGENABLE
            | D3DRS_FOGCOLOR
            | D3DRS_FOGTABLEMODE
            | D3DRS_FOGSTART
            | D3DRS_FOGEND
            | D3DRS_FOGDENSITY
            | D3DRS_RANGEFOGENABLE
            | D3DRS_CLIPPING
            | D3DRS_LIGHTING
            | D3DRS_AMBIENT
            | D3DRS_FOGVERTEXMODE
            | D3DRS_COLORVERTEX
            | D3DRS_LOCALVIEWER
            | D3DRS_NORMALIZENORMALS
            | D3DRS_DIFFUSEMATERIALSOURCE
            | D3DRS_SPECULARMATERIALSOURCE
            | D3DRS_AMBIENTMATERIALSOURCE
            | D3DRS_EMISSIVEMATERIALSOURCE
            | D3DRS_VERTEXBLEND
            | D3DRS_CLIPPLANEENABLE
            | D3DRS_POINTSIZE
            | D3DRS_POINTSIZE_MIN
            | D3DRS_POINTSPRITEENABLE
            | D3DRS_POINTSCALEENABLE
            | D3DRS_POINTSCALE_A
            | D3DRS_POINTSCALE_B
            | D3DRS_POINTSCALE_C
            | D3DRS_MULTISAMPLEANTIALIAS
            | D3DRS_MULTISAMPLEMASK
            | D3DRS_PATCHEDGESTYLE
            | D3DRS_PATCHSEGMENTS
            | D3DRS_POINTSIZE_MAX
            | D3DRS_INDEXEDVERTEXBLENDENABLE
            | D3DRS_TWEENFACTOR
            | D3DRS_POSITIONORDER
            | D3DRS_NORMALORDER
    )
}

fn is_pixel_render_state(rs: u32) -> bool {
    !is_vertex_render_state(rs)
}

fn mul_matrix_4x4(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for row in 0..4 {
        for col in 0..4 {
            out[row * 4 + col] = a[row * 4] * b[col]
                + a[row * 4 + 1] * b[4 + col]
                + a[row * 4 + 2] * b[8 + col]
                + a[row * 4 + 3] * b[12 + col];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_generation_counters_track_mutations_and_reset() {
        let mut s = DeviceState::new(640, 480, true);
        assert_eq!(s.transforms_gen(), 1);
        assert_eq!(s.render_state_gen(), 1);
        assert_eq!(s.lights_gen(), 1);
        assert_eq!(s.material_gen(), 1);
        assert_eq!(s.vs_const_gen(), 1);
        assert_eq!(s.ps_const_gen(), 1);

        s.set_transform(D3DTS_WORLD, IDENTITY_MATRIX_4X4);
        assert_eq!(s.transforms_gen(), 2);
        s.multiply_transform(D3DTS_WORLD, IDENTITY_MATRIX_4X4);
        assert_eq!(s.transforms_gen(), 3);

        let cur_zen = s.get_render_state(D3DRS_ZENABLE);
        s.set_render_state(D3DRS_ZENABLE, cur_zen);
        assert_eq!(s.render_state_gen(), 1);
        s.set_render_state(D3DRS_ZENABLE, cur_zen ^ 1);
        assert_eq!(s.render_state_gen(), 2);

        s.set_light(0, D3dLight8::default());
        assert_eq!(s.lights_gen(), 2);
        s.light_enable(0, true);
        assert_eq!(s.lights_gen(), 3);

        s.set_material(D3dMaterial8::default());
        assert_eq!(s.material_gen(), 2);

        s.set_vs_constants(0, &[[1.0, 2.0, 3.0, 4.0]]);
        assert_eq!(s.vs_const_gen(), 2);
        s.set_ps_constants(0, &[[0.5, 0.5, 0.5, 1.0]]);
        assert_eq!(s.ps_const_gen(), 2);

        let sb = s.snapshot_state_block(D3DSBT_ALL).unwrap();
        s.apply_state_block(&sb);
        assert_eq!(s.transforms_gen(), 4);
        assert_eq!(s.lights_gen(), 4);
        assert_eq!(s.material_gen(), 3);
        assert_eq!(s.vs_const_gen(), 3);
        assert_eq!(s.ps_const_gen(), 3);

        s.reset(800, 600, true);
        assert!(s.transforms_gen() > 4);
        assert!(s.render_state_gen() > 2);
        assert!(s.lights_gen() > 4);
        assert!(s.material_gen() > 3);
        assert!(s.vs_const_gen() > 3);
        assert!(s.ps_const_gen() > 3);
    }
}
