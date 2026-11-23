use std::any::Any;

use crate::state::DeviceState;
use crate::types::D3dRect;

pub trait RenderBackend {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    fn max_texture_size(&self) -> u32;
    fn supports_s3tc(&self) -> bool;

    fn on_device_create(&mut self, width: u32, height: u32, format: u32, windowed: bool);
    fn on_device_reset(&mut self, width: u32, height: u32, format: u32, windowed: bool);
    fn on_clear(&mut self, flags: u32, color: u32, z: f32, stencil: u32, rects: &[D3dRect]);
    fn on_texture_upload(
        &mut self,
        tex_handle: u32,
        level: u32,
        width: u32,
        height: u32,
        format: u32,
        data: &[u8],
    );
    fn on_texture_destroy(&mut self, tex_handle: u32);
    fn on_vb_upload(&mut self, vb_handle: u32, data: &[u8], fvf: u32, usage: u32);
    fn on_vb_destroy(&mut self, vb_handle: u32);
    fn on_ib_upload(&mut self, ib_handle: u32, data: &[u8], format: u32, usage: u32);
    fn on_ib_destroy(&mut self, ib_handle: u32);
    fn on_draw_primitive(
        &mut self,
        state: &DeviceState,
        prim_type: u32,
        start_vertex: u32,
        prim_count: u32,
    );
    fn on_draw_indexed_primitive(
        &mut self,
        state: &DeviceState,
        prim_type: u32,
        min_vertex_index: u32,
        num_vertices: u32,
        start_index: u32,
        prim_count: u32,
    );
    fn on_present(&mut self);
}

#[derive(Clone, Debug)]
pub struct NullBackend {
    pub max_tex_size: u32,
    pub s3tc_supported: bool,
    pub device_creates: u64,
    pub device_resets: u64,
    pub clears: u64,
    pub texture_uploads: u64,
    pub texture_destroys: u64,
    pub vb_uploads: u64,
    pub vb_destroys: u64,
    pub ib_uploads: u64,
    pub ib_destroys: u64,
    pub draw_calls: u64,
    pub indexed_draw_calls: u64,
    pub primitives_drawn: u64,
    pub presents: u64,
}

impl Default for NullBackend {
    fn default() -> Self {
        Self {
            max_tex_size: 4096,
            s3tc_supported: true,
            device_creates: 0,
            device_resets: 0,
            clears: 0,
            texture_uploads: 0,
            texture_destroys: 0,
            vb_uploads: 0,
            vb_destroys: 0,
            ib_uploads: 0,
            ib_destroys: 0,
            draw_calls: 0,
            indexed_draw_calls: 0,
            primitives_drawn: 0,
            presents: 0,
        }
    }
}

impl NullBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RenderBackend for NullBackend {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn max_texture_size(&self) -> u32 {
        self.max_tex_size
    }

    fn supports_s3tc(&self) -> bool {
        self.s3tc_supported
    }

    fn on_device_create(&mut self, _width: u32, _height: u32, _format: u32, _windowed: bool) {
        self.device_creates += 1;
    }

    fn on_device_reset(&mut self, _width: u32, _height: u32, _format: u32, _windowed: bool) {
        self.device_resets += 1;
    }

    fn on_clear(&mut self, _flags: u32, _color: u32, _z: f32, _stencil: u32, _rects: &[D3dRect]) {
        self.clears += 1;
    }

    fn on_texture_upload(
        &mut self,
        _tex_handle: u32,
        _level: u32,
        _width: u32,
        _height: u32,
        _format: u32,
        _data: &[u8],
    ) {
        self.texture_uploads += 1;
    }

    fn on_texture_destroy(&mut self, _tex_handle: u32) {
        self.texture_destroys += 1;
    }

    fn on_vb_upload(&mut self, _vb_handle: u32, _data: &[u8], _fvf: u32, _usage: u32) {
        self.vb_uploads += 1;
    }

    fn on_vb_destroy(&mut self, _vb_handle: u32) {
        self.vb_destroys += 1;
    }

    fn on_ib_upload(&mut self, _ib_handle: u32, _data: &[u8], _format: u32, _usage: u32) {
        self.ib_uploads += 1;
    }

    fn on_ib_destroy(&mut self, _ib_handle: u32) {
        self.ib_destroys += 1;
    }

    fn on_draw_primitive(
        &mut self,
        _state: &DeviceState,
        _prim_type: u32,
        _start_vertex: u32,
        prim_count: u32,
    ) {
        self.draw_calls += 1;
        self.primitives_drawn += u64::from(prim_count);
    }

    fn on_draw_indexed_primitive(
        &mut self,
        _state: &DeviceState,
        _prim_type: u32,
        _min_vertex_index: u32,
        _num_vertices: u32,
        _start_index: u32,
        prim_count: u32,
    ) {
        self.indexed_draw_calls += 1;
        self.primitives_drawn += u64::from(prim_count);
    }

    fn on_present(&mut self) {
        self.presents += 1;
    }
}
