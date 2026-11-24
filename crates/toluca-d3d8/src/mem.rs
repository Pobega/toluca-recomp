use toluca_core::GuestAddr;

pub const COM_THUNK_BASE: GuestAddr = 0x7080_0000;
pub const COM_THUNK_STRIDE: u32 = 4;
pub const COM_METHOD_STRIDE_BITS: u32 = 8;
pub const COM_METHODS_PER_INTERFACE: u32 = 1 << COM_METHOD_STRIDE_BITS;
pub const COM_METHOD_MASK: u32 = COM_METHODS_PER_INTERFACE - 1;
pub const COM_OBJECT_SIZE: usize = 16;

const COM_OBJ_OFF_VTABLE: u32 = 0;
const COM_OBJ_OFF_HANDLE: u32 = 4;
const COM_OBJ_OFF_IFACE: u32 = 8;
const COM_OBJ_OFF_RESERVED: u32 = 12;

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ComInterfaceId {
    Direct3D8 = 1,
    Direct3DDevice8 = 2,
    Direct3DTexture8 = 3,
    Direct3DSurface8 = 4,
    Direct3DVertexBuffer8 = 5,
    Direct3DIndexBuffer8 = 6,
}

impl ComInterfaceId {
    pub const COUNT: usize = 6;

    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Direct3D8),
            2 => Some(Self::Direct3DDevice8),
            3 => Some(Self::Direct3DTexture8),
            4 => Some(Self::Direct3DSurface8),
            5 => Some(Self::Direct3DVertexBuffer8),
            6 => Some(Self::Direct3DIndexBuffer8),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Direct3D8 => "IDirect3D8",
            Self::Direct3DDevice8 => "IDirect3DDevice8",
            Self::Direct3DTexture8 => "IDirect3DTexture8",
            Self::Direct3DSurface8 => "IDirect3DSurface8",
            Self::Direct3DVertexBuffer8 => "IDirect3DVertexBuffer8",
            Self::Direct3DIndexBuffer8 => "IDirect3DIndexBuffer8",
        }
    }
}

pub fn encode_com_thunk(iface: ComInterfaceId, method_idx: u32) -> GuestAddr {
    debug_assert!(method_idx < COM_METHODS_PER_INTERFACE);
    let iface_slot = (iface as u32) - 1;
    let slot = (iface_slot << COM_METHOD_STRIDE_BITS) | (method_idx & COM_METHOD_MASK);
    COM_THUNK_BASE + slot * COM_THUNK_STRIDE
}

pub fn decode_com_thunk(va: GuestAddr) -> Option<(ComInterfaceId, u32)> {
    if va < COM_THUNK_BASE {
        return None;
    }
    let offset = va - COM_THUNK_BASE;
    if !offset.is_multiple_of(COM_THUNK_STRIDE) {
        return None;
    }
    let slot = offset / COM_THUNK_STRIDE;
    let iface_raw = (slot >> COM_METHOD_STRIDE_BITS) + 1;
    let method_idx = slot & COM_METHOD_MASK;
    ComInterfaceId::from_u32(iface_raw).map(|iface| (iface, method_idx))
}

pub trait GuestMemory {
    fn bytes_slice(&self, _va: GuestAddr, _len: usize) -> Result<Option<&[u8]>, String> {
        Ok(None)
    }
    fn read_bytes(&self, va: GuestAddr, len: usize) -> Result<Vec<u8>, String>;
    fn read_into(&self, va: GuestAddr, buf: &mut [u8]) -> Result<(), String> {
        let data = self.read_bytes(va, buf.len())?;
        buf.copy_from_slice(&data);
        Ok(())
    }
    fn read_u32(&self, va: GuestAddr) -> Result<u32, String> {
        let mut buf = [0u8; 4];
        self.read_into(va, &mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }
    fn read_i32(&self, va: GuestAddr) -> Result<i32, String> {
        Ok(self.read_u32(va)? as i32)
    }
    fn read_f32(&self, va: GuestAddr) -> Result<f32, String> {
        Ok(f32::from_bits(self.read_u32(va)?))
    }
    fn write_bytes(&mut self, va: GuestAddr, data: &[u8]) -> Result<(), String>;
    fn write_u32(&mut self, va: GuestAddr, val: u32) -> Result<(), String> {
        self.write_bytes(va, &val.to_le_bytes())
    }
    fn write_i32(&mut self, va: GuestAddr, val: i32) -> Result<(), String> {
        self.write_u32(va, val as u32)
    }
    fn write_f32(&mut self, va: GuestAddr, val: f32) -> Result<(), String> {
        self.write_u32(va, val.to_bits())
    }
    fn zero_range(&mut self, va: GuestAddr, len: usize) -> Result<(), String>;
    fn alloc(&mut self, size: usize, zeroed: bool) -> Result<GuestAddr, String>;
    fn free(&mut self, va: GuestAddr) -> Result<(), String>;
    fn alloc_page_aligned(&mut self, size: usize) -> Result<(GuestAddr, usize), String> {
        let va = self.alloc(size, true)?;
        Ok((va, size))
    }
    fn free_page_aligned(&mut self, va: GuestAddr, _alloc_size: usize) -> Result<(), String> {
        self.free(va)
    }
    fn arm_write_watch(&mut self, _va: GuestAddr, _alloc_size: usize) -> bool {
        false
    }
    fn is_write_watch_dirty(&self, _va: GuestAddr, _alloc_size: usize) -> Option<bool> {
        None
    }
    fn remove_write_watch(&mut self, _va: GuestAddr, _alloc_size: usize) {}
}

#[derive(Default)]
pub struct ComObjectTable {
    vtables: [Option<GuestAddr>; ComInterfaceId::COUNT],
}

impl ComObjectTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ensure_vtable(
        &mut self,
        mem: &mut dyn GuestMemory,
        iface: ComInterfaceId,
        method_count: u32,
    ) -> Result<GuestAddr, String> {
        let slot = (iface as usize) - 1;
        if let Some(existing) = self.vtables[slot] {
            return Ok(existing);
        }

        let vtable_bytes = (method_count as usize) * 4;
        let vtable_va = mem.alloc(vtable_bytes, true)?;
        for method_idx in 0..method_count {
            let thunk_va = encode_com_thunk(iface, method_idx);
            mem.write_u32(vtable_va + method_idx * 4, thunk_va)?;
        }
        self.vtables[slot] = Some(vtable_va);
        Ok(vtable_va)
    }

    pub fn create_object(
        &mut self,
        mem: &mut dyn GuestMemory,
        iface: ComInterfaceId,
        method_count: u32,
        handle: u32,
    ) -> Result<GuestAddr, String> {
        let vtable_va = self.ensure_vtable(mem, iface, method_count)?;
        let obj_va = mem.alloc(COM_OBJECT_SIZE, true)?;
        mem.write_u32(obj_va + COM_OBJ_OFF_VTABLE, vtable_va)?;
        mem.write_u32(obj_va + COM_OBJ_OFF_HANDLE, handle)?;
        mem.write_u32(obj_va + COM_OBJ_OFF_IFACE, iface as u32)?;
        mem.write_u32(obj_va + COM_OBJ_OFF_RESERVED, 0)?;
        Ok(obj_va)
    }

    pub fn read_object(
        &self,
        mem: &dyn GuestMemory,
        obj_va: GuestAddr,
    ) -> Result<(ComInterfaceId, u32), String> {
        if obj_va == 0 {
            return Err("null COM object pointer".to_owned());
        }
        let handle = mem.read_u32(obj_va + COM_OBJ_OFF_HANDLE)?;
        let iface_raw = mem.read_u32(obj_va + COM_OBJ_OFF_IFACE)?;
        let iface = ComInterfaceId::from_u32(iface_raw)
            .ok_or_else(|| format!("invalid COM interface id {iface_raw} at 0x{obj_va:08x}"))?;
        Ok((iface, handle))
    }

    pub fn free_object(&self, mem: &mut dyn GuestMemory, obj_va: GuestAddr) -> Result<(), String> {
        if obj_va != 0 {
            mem.free(obj_va)?;
        }
        Ok(())
    }
}
