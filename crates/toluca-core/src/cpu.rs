use core::mem::offset_of;

use crate::bridge::HostBridge;
use crate::memory::{GUEST_MAIN_STACK_TOP, GUEST_TEB_ADDR};

pub const GPR_COUNT: usize = 8;
pub const X87_STACK_DEPTH: usize = 8;
pub const MMX_REG_COUNT: usize = 8;
pub const XMM_REG_COUNT: usize = 8;

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Gpr {
    Eax = 0,
    Ecx = 1,
    Edx = 2,
    Ebx = 3,
    Esp = 4,
    Ebp = 5,
    Esi = 6,
    Edi = 7,
}

pub const EFLAGS_CF: u32 = 1 << 0;
pub const EFLAGS_FIXED_ONE: u32 = 1 << 1;
pub const EFLAGS_PF: u32 = 1 << 2;
pub const EFLAGS_AF: u32 = 1 << 4;
pub const EFLAGS_ZF: u32 = 1 << 6;
pub const EFLAGS_SF: u32 = 1 << 7;
pub const EFLAGS_DF: u32 = 1 << 10;
pub const EFLAGS_OF: u32 = 1 << 11;
pub const EFLAGS_ARITH_MASK: u32 =
    EFLAGS_CF | EFLAGS_PF | EFLAGS_AF | EFLAGS_ZF | EFLAGS_SF | EFLAGS_OF;

const AF_BIT_MASK: u32 = 0x10;
const SHIFT_COUNT_MASK: u32 = 0x1F;
const BITS_PER_BYTE: u32 = 8;

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LazyFlagOp {
    None = 0,
    Add = 1,
    Sub = 2,
    Logic = 3,
    Inc = 4,
    Dec = 5,
    Shl = 6,
    Shr = 7,
    Sar = 8,
}

impl LazyFlagOp {
    pub const fn from_u32(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::None),
            1 => Some(Self::Add),
            2 => Some(Self::Sub),
            3 => Some(Self::Logic),
            4 => Some(Self::Inc),
            5 => Some(Self::Dec),
            6 => Some(Self::Shl),
            7 => Some(Self::Shr),
            8 => Some(Self::Sar),
            _ => None,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct LazyFlags {
    pub op: u32,
    pub width: u32,
    pub lhs: u32,
    pub rhs: u32,
    pub res: u32,
}

pub const X87_DEFAULT_CONTROL_WORD: u16 = 0x027F;
pub const X87_DEFAULT_TAG_WORD: u16 = 0xFFFF;
pub const X87_SW_C0: u16 = 1 << 8;
pub const X87_SW_C1: u16 = 1 << 9;
pub const X87_SW_C2: u16 = 1 << 10;
pub const X87_SW_C3: u16 = 1 << 14;
pub const X87_SW_TOP_SHIFT: u16 = 11;
pub const X87_SW_TOP_MASK: u16 = 0x3800;
const X87_TOP_MASK: u32 = (X87_STACK_DEPTH as u32) - 1;
const X87_SW_CC_MASK: u16 = X87_SW_C0 | X87_SW_C2 | X87_SW_C3;

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct X87State {
    pub regs: [f64; X87_STACK_DEPTH],
    pub top: u32,
    pub control_word: u16,
    pub status_word: u16,
    pub tag_word: u16,
    pub _pad: u16,
}

impl X87State {
    pub const fn new() -> Self {
        Self {
            regs: [0.0; X87_STACK_DEPTH],
            top: 0,
            control_word: X87_DEFAULT_CONTROL_WORD,
            status_word: 0,
            tag_word: X87_DEFAULT_TAG_WORD,
            _pad: 0,
        }
    }

    pub fn st(&self, idx: usize) -> f64 {
        let phys = ((self.top as usize).wrapping_add(idx)) & (X87_STACK_DEPTH - 1);
        self.regs[phys]
    }

    pub fn st_mut(&mut self, idx: usize) -> &mut f64 {
        let phys = ((self.top as usize).wrapping_add(idx)) & (X87_STACK_DEPTH - 1);
        &mut self.regs[phys]
    }

    pub fn push(&mut self, val: f64) {
        self.top = self.top.wrapping_sub(1) & X87_TOP_MASK;
        self.regs[self.top as usize] = val;
    }

    pub fn pop(&mut self) -> f64 {
        let val = self.regs[(self.top & X87_TOP_MASK) as usize];
        self.top = self.top.wrapping_add(1) & X87_TOP_MASK;
        val
    }

    pub fn set_condition_codes(&mut self, c0: bool, c2: bool, c3: bool) {
        self.status_word &= !X87_SW_CC_MASK;
        if c0 {
            self.status_word |= X87_SW_C0;
        }
        if c2 {
            self.status_word |= X87_SW_C2;
        }
        if c3 {
            self.status_word |= X87_SW_C3;
        }
    }

    pub const fn effective_status_word(&self) -> u16 {
        let top_bits = ((self.top as u16) & (X87_TOP_MASK as u16)) << X87_SW_TOP_SHIFT;
        (self.status_word & !X87_SW_TOP_MASK) | top_bits
    }
}

impl Default for X87State {
    fn default() -> Self {
        Self::new()
    }
}

const XMM_LANE_BYTES: usize = 4;
const XMM_LANE_COUNT: usize = 4;

#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct XmmReg(pub [u8; 16]);

impl XmmReg {
    pub fn from_f32x4(lanes: [f32; XMM_LANE_COUNT]) -> Self {
        let mut bytes = [0u8; 16];
        for (idx, lane) in lanes.iter().enumerate() {
            let start = idx * XMM_LANE_BYTES;
            bytes[start..start + XMM_LANE_BYTES].copy_from_slice(&lane.to_le_bytes());
        }
        Self(bytes)
    }

    pub fn to_f32x4(&self) -> [f32; XMM_LANE_COUNT] {
        let mut out = [0.0f32; XMM_LANE_COUNT];
        for (idx, slot) in out.iter_mut().enumerate() {
            let start = idx * XMM_LANE_BYTES;
            let mut chunk = [0u8; XMM_LANE_BYTES];
            chunk.copy_from_slice(&self.0[start..start + XMM_LANE_BYTES]);
            *slot = f32::from_le_bytes(chunk);
        }
        out
    }

    pub fn from_u32x4(lanes: [u32; XMM_LANE_COUNT]) -> Self {
        let mut bytes = [0u8; 16];
        for (idx, lane) in lanes.iter().enumerate() {
            let start = idx * XMM_LANE_BYTES;
            bytes[start..start + XMM_LANE_BYTES].copy_from_slice(&lane.to_le_bytes());
        }
        Self(bytes)
    }

    pub fn to_u32x4(&self) -> [u32; XMM_LANE_COUNT] {
        let mut out = [0u32; XMM_LANE_COUNT];
        for (idx, slot) in out.iter_mut().enumerate() {
            let start = idx * XMM_LANE_BYTES;
            let mut chunk = [0u8; XMM_LANE_BYTES];
            chunk.copy_from_slice(&self.0[start..start + XMM_LANE_BYTES]);
            *slot = u32::from_le_bytes(chunk);
        }
        out
    }
}

pub const MXCSR_DEFAULT: u32 = 0x1F80;

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExitReason {
    Running = 0,
    Returned = 1,
    Halt = 2,
    UnmappedTarget = 3,
    UnsupportedInstruction = 4,
    HostTrap = 5,
    GuestException = 6,
    Longjmp = 7,
}

#[repr(C, align(16))]
pub struct GuestCpuState {
    pub mem_base: *mut u8,
    pub bridge: *const HostBridge,
    pub gpr: [u32; GPR_COUNT],
    pub eip: u32,
    pub fs_base: u32,
    pub gs_base: u32,
    pub eflags: u32,
    pub lazy_flags: LazyFlags,
    pub exit_reason: u32,
    pub exit_detail: u32,
    pub mxcsr: u32,
    pub _pad_fpu: u32,
    pub x87: X87State,
    pub mm: [u64; MMX_REG_COUNT],
    pub xmm: [XmmReg; XMM_REG_COUNT],
}

impl GuestCpuState {
    pub fn new(mem_base: *mut u8, bridge: *const HostBridge) -> Self {
        let mut gpr = [0u32; GPR_COUNT];
        gpr[Gpr::Esp as usize] = GUEST_MAIN_STACK_TOP;
        Self {
            mem_base,
            bridge,
            gpr,
            eip: 0,
            fs_base: GUEST_TEB_ADDR,
            gs_base: 0,
            eflags: EFLAGS_FIXED_ONE,
            lazy_flags: LazyFlags::default(),
            exit_reason: ExitReason::Running as u32,
            exit_detail: 0,
            mxcsr: MXCSR_DEFAULT,
            _pad_fpu: 0,
            x87: X87State::new(),
            mm: [0u64; MMX_REG_COUNT],
            xmm: [XmmReg::default(); XMM_REG_COUNT],
        }
    }

    pub const fn gpr(&self, reg: Gpr) -> u32 {
        self.gpr[reg as usize]
    }

    pub fn set_gpr(&mut self, reg: Gpr, val: u32) {
        self.gpr[reg as usize] = val;
    }

    pub const fn gpr_offset(reg: Gpr) -> usize {
        offsets::GPR + (reg as usize) * core::mem::size_of::<u32>()
    }

    pub fn materialize_flags(&mut self) {
        let Some(op) = LazyFlagOp::from_u32(self.lazy_flags.op) else {
            self.lazy_flags.op = LazyFlagOp::None as u32;
            return;
        };
        if op == LazyFlagOp::None {
            return;
        }

        let Some(width_spec) = WidthSpec::from_byte_width(self.lazy_flags.width) else {
            self.lazy_flags.op = LazyFlagOp::None as u32;
            return;
        };

        let lhs = self.lazy_flags.lhs & width_spec.mask;
        let rhs = self.lazy_flags.rhs & width_spec.mask;
        let res = self.lazy_flags.res & width_spec.mask;

        if matches!(op, LazyFlagOp::Shl | LazyFlagOp::Shr | LazyFlagOp::Sar)
            && (self.lazy_flags.rhs & SHIFT_COUNT_MASK) == 0
        {
            self.lazy_flags.op = LazyFlagOp::None as u32;
            return;
        }

        let prev_cf = (self.eflags & EFLAGS_CF) != 0;
        let (cf, af, of) = match op {
            LazyFlagOp::Add => compute_add_flags(lhs, rhs, res, width_spec),
            LazyFlagOp::Inc => compute_add_flags_with_cf(lhs, 1, res, width_spec, prev_cf),
            LazyFlagOp::Sub => compute_sub_flags(lhs, rhs, res, width_spec),
            LazyFlagOp::Dec => compute_sub_flags_with_cf(lhs, 1, res, width_spec, prev_cf),
            LazyFlagOp::Logic => (false, false, false),
            LazyFlagOp::Shl => compute_shl_flags(lhs, self.lazy_flags.rhs, res, width_spec),
            LazyFlagOp::Shr => compute_shr_flags(lhs, self.lazy_flags.rhs, width_spec),
            LazyFlagOp::Sar => compute_sar_flags(lhs, self.lazy_flags.rhs, width_spec),
            LazyFlagOp::None => unreachable!(),
        };

        let mut flags = (self.eflags & !EFLAGS_ARITH_MASK) | EFLAGS_FIXED_ONE;
        if cf {
            flags |= EFLAGS_CF;
        }
        if is_even_parity(res as u8) {
            flags |= EFLAGS_PF;
        }
        if af {
            flags |= EFLAGS_AF;
        }
        if res == 0 {
            flags |= EFLAGS_ZF;
        }
        if (res & width_spec.sign_bit) != 0 {
            flags |= EFLAGS_SF;
        }
        if of {
            flags |= EFLAGS_OF;
        }

        self.eflags = flags;
        self.lazy_flags.op = LazyFlagOp::None as u32;
    }
}

#[derive(Copy, Clone)]
struct WidthSpec {
    bit_width: u32,
    mask: u32,
    sign_bit: u32,
}

impl WidthSpec {
    const fn from_byte_width(width: u32) -> Option<Self> {
        match width {
            1 => Some(Self {
                bit_width: BITS_PER_BYTE,
                mask: 0x0000_00FF,
                sign_bit: 0x0000_0080,
            }),
            2 => Some(Self {
                bit_width: BITS_PER_BYTE * 2,
                mask: 0x0000_FFFF,
                sign_bit: 0x0000_8000,
            }),
            4 => Some(Self {
                bit_width: BITS_PER_BYTE * 4,
                mask: 0xFFFF_FFFF,
                sign_bit: 0x8000_0000,
            }),
            _ => None,
        }
    }
}

const fn is_even_parity(low_byte: u8) -> bool {
    low_byte.count_ones().is_multiple_of(2)
}

const fn compute_add_flags(lhs: u32, rhs: u32, res: u32, spec: WidthSpec) -> (bool, bool, bool) {
    let cf = (lhs as u64) + (rhs as u64) > (spec.mask as u64);
    compute_add_flags_with_cf(lhs, rhs, res, spec, cf)
}

const fn compute_add_flags_with_cf(
    lhs: u32,
    rhs: u32,
    res: u32,
    spec: WidthSpec,
    cf: bool,
) -> (bool, bool, bool) {
    let af = ((lhs ^ rhs ^ res) & AF_BIT_MASK) != 0;
    let of = ((!(lhs ^ rhs) & (lhs ^ res)) & spec.sign_bit) != 0;
    (cf, af, of)
}

const fn compute_sub_flags(lhs: u32, rhs: u32, res: u32, spec: WidthSpec) -> (bool, bool, bool) {
    let cf = lhs < rhs;
    compute_sub_flags_with_cf(lhs, rhs, res, spec, cf)
}

const fn compute_sub_flags_with_cf(
    lhs: u32,
    rhs: u32,
    res: u32,
    spec: WidthSpec,
    cf: bool,
) -> (bool, bool, bool) {
    let af = ((lhs ^ rhs ^ res) & AF_BIT_MASK) != 0;
    let of = (((lhs ^ rhs) & (lhs ^ res)) & spec.sign_bit) != 0;
    (cf, af, of)
}

const fn compute_shl_flags(lhs: u32, raw_count: u32, res: u32, spec: WidthSpec) -> (bool, bool, bool) {
    let count = raw_count & SHIFT_COUNT_MASK;
    let cf = if count <= spec.bit_width {
        ((lhs >> (spec.bit_width - count)) & 1) != 0
    } else {
        false
    };
    let of = if count == 1 {
        ((res & spec.sign_bit) != 0) ^ cf
    } else {
        false
    };
    (cf, false, of)
}

const fn compute_shr_flags(lhs: u32, raw_count: u32, spec: WidthSpec) -> (bool, bool, bool) {
    let count = raw_count & SHIFT_COUNT_MASK;
    let cf = if count <= spec.bit_width {
        ((lhs >> (count - 1)) & 1) != 0
    } else {
        false
    };
    let of = if count == 1 {
        (lhs & spec.sign_bit) != 0
    } else {
        false
    };
    (cf, false, of)
}

const fn compute_sar_flags(lhs: u32, raw_count: u32, spec: WidthSpec) -> (bool, bool, bool) {
    let count = raw_count & SHIFT_COUNT_MASK;
    let cf = if count <= spec.bit_width {
        ((lhs >> (count - 1)) & 1) != 0
    } else {
        (lhs & spec.sign_bit) != 0
    };
    (cf, false, false)
}

pub mod offsets {
    use super::*;

    pub const MEM_BASE: usize = offset_of!(GuestCpuState, mem_base);
    pub const BRIDGE: usize = offset_of!(GuestCpuState, bridge);
    pub const GPR: usize = offset_of!(GuestCpuState, gpr);
    pub const EIP: usize = offset_of!(GuestCpuState, eip);
    pub const FS_BASE: usize = offset_of!(GuestCpuState, fs_base);
    pub const GS_BASE: usize = offset_of!(GuestCpuState, gs_base);
    pub const EFLAGS: usize = offset_of!(GuestCpuState, eflags);
    pub const LAZY_FLAGS: usize = offset_of!(GuestCpuState, lazy_flags);
    pub const EXIT_REASON: usize = offset_of!(GuestCpuState, exit_reason);
    pub const EXIT_DETAIL: usize = offset_of!(GuestCpuState, exit_detail);
    pub const MXCSR: usize = offset_of!(GuestCpuState, mxcsr);
    pub const PAD_FPU: usize = offset_of!(GuestCpuState, _pad_fpu);
    pub const X87: usize = offset_of!(GuestCpuState, x87);
    pub const MM: usize = offset_of!(GuestCpuState, mm);
    pub const XMM: usize = offset_of!(GuestCpuState, xmm);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_cpu() -> GuestCpuState {
        GuestCpuState::new(core::ptr::null_mut(), core::ptr::null())
    }

    #[test]
    fn cpu_state_alignment_and_offsets() {
        assert!(core::mem::align_of::<GuestCpuState>() >= 16);
        assert!(core::mem::align_of::<XmmReg>() >= 16);
        assert_eq!(offsets::MEM_BASE, 0);
        assert_eq!(GuestCpuState::gpr_offset(Gpr::Eax), offsets::GPR);
        assert_eq!(GuestCpuState::gpr_offset(Gpr::Edi), offsets::GPR + 7 * 4);
        assert_eq!(offsets::XMM % 16, 0);

        let cpu = new_cpu();
        assert_eq!(cpu.eflags, EFLAGS_FIXED_ONE);
        assert_eq!(cpu.mxcsr, MXCSR_DEFAULT);
        assert_eq!(cpu.fs_base, GUEST_TEB_ADDR);
        assert_eq!(cpu.gpr(Gpr::Esp), GUEST_MAIN_STACK_TOP);
    }

    #[test]
    fn x87_stack_push_pop_wraparound_and_status_word() {
        let mut x87 = X87State::new();
        assert_eq!(x87.control_word, X87_DEFAULT_CONTROL_WORD);
        assert_eq!(x87.tag_word, X87_DEFAULT_TAG_WORD);
        assert_eq!(x87.effective_status_word() & X87_SW_TOP_MASK, 0);

        x87.push(1.0);
        assert_eq!(x87.top, 7);
        assert_eq!((x87.effective_status_word() & X87_SW_TOP_MASK) >> X87_SW_TOP_SHIFT, 7);
        assert_eq!(x87.st(0), 1.0);

        x87.push(2.0);
        assert_eq!(x87.top, 6);
        assert_eq!(x87.st(0), 2.0);
        assert_eq!(x87.st(1), 1.0);

        *x87.st_mut(1) = 3.5;
        assert_eq!(x87.pop(), 2.0);
        assert_eq!(x87.top, 7);
        assert_eq!(x87.pop(), 3.5);
        assert_eq!(x87.top, 0);

        x87.set_condition_codes(true, false, true);
        assert_ne!(x87.status_word & X87_SW_C0, 0);
        assert_eq!(x87.status_word & X87_SW_C2, 0);
        assert_ne!(x87.status_word & X87_SW_C3, 0);
        let eff = x87.effective_status_word();
        assert_ne!(eff & X87_SW_C0, 0);
        assert_ne!(eff & X87_SW_C3, 0);
    }

    #[test]
    fn xmm_lane_round_trips() {
        let f_lanes = [1.5f32, -2.25, 0.0, 42.125];
        let xmm_f = XmmReg::from_f32x4(f_lanes);
        assert_eq!(xmm_f.to_f32x4(), f_lanes);

        let u_lanes = [0x1122_3344, 0x5566_7788, 0xDEAD_BEEF, 0xCAFE_BABE];
        let xmm_u = XmmReg::from_u32x4(u_lanes);
        assert_eq!(xmm_u.to_u32x4(), u_lanes);
    }

    #[test]
    fn materialize_add_sub_across_widths() {
        let mut cpu = new_cpu();

        // 8-bit: 0x7F + 0x01 = 0x80 -> OF=1, SF=1, AF=1, PF=0, ZF=0, CF=0
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Add as u32,
            width: 1,
            lhs: 0x7F,
            rhs: 0x01,
            res: 0x80,
        };
        cpu.materialize_flags();
        assert_eq!(
            cpu.eflags & EFLAGS_ARITH_MASK,
            EFLAGS_OF | EFLAGS_SF | EFLAGS_AF
        );
        assert_eq!(cpu.lazy_flags.op, LazyFlagOp::None as u32);

        // 8-bit: 0xFF + 0x01 = 0x00 -> CF=1, ZF=1, PF=1, AF=1
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Add as u32,
            width: 1,
            lhs: 0xFF,
            rhs: 0x01,
            res: 0x00,
        };
        cpu.materialize_flags();
        assert_eq!(
            cpu.eflags & EFLAGS_ARITH_MASK,
            EFLAGS_CF | EFLAGS_ZF | EFLAGS_PF | EFLAGS_AF
        );

        // 16-bit: 0x0000 - 0x0001 = 0xFFFF -> CF=1, AF=1, SF=1, PF=1
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Sub as u32,
            width: 2,
            lhs: 0x0000,
            rhs: 0x0001,
            res: 0xFFFF,
        };
        cpu.materialize_flags();
        assert_eq!(
            cpu.eflags & EFLAGS_ARITH_MASK,
            EFLAGS_CF | EFLAGS_AF | EFLAGS_SF | EFLAGS_PF
        );

        // 32-bit signed overflow: 0x8000_0000 - 1 = 0x7FFF_FFFF -> OF=1, AF=1, PF=1
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Sub as u32,
            width: 4,
            lhs: 0x8000_0000,
            rhs: 1,
            res: 0x7FFF_FFFF,
        };
        cpu.materialize_flags();
        assert_eq!(
            cpu.eflags & EFLAGS_ARITH_MASK,
            EFLAGS_OF | EFLAGS_AF | EFLAGS_PF
        );
    }

    #[test]
    fn materialize_inc_dec_preserves_cf() {
        let mut cpu = new_cpu();

        // Start with CF=1, verify INC wrapping 0xFF -> 0x00 keeps CF=1
        cpu.eflags |= EFLAGS_CF;
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Inc as u32,
            width: 1,
            lhs: 0xFF,
            rhs: 0,
            res: 0x00,
        };
        cpu.materialize_flags();
        assert_eq!(
            cpu.eflags & EFLAGS_ARITH_MASK,
            EFLAGS_CF | EFLAGS_ZF | EFLAGS_PF | EFLAGS_AF
        );

        // Clear CF, verify DEC 0x8000 -> 0x7FFF keeps CF=0 and sets OF=1, AF=1, PF=1
        cpu.eflags &= !EFLAGS_CF;
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Dec as u32,
            width: 2,
            lhs: 0x8000,
            rhs: 0,
            res: 0x7FFF,
        };
        cpu.materialize_flags();
        assert_eq!(
            cpu.eflags & EFLAGS_ARITH_MASK,
            EFLAGS_OF | EFLAGS_AF | EFLAGS_PF
        );
    }

    #[test]
    fn materialize_logic_and_shifts() {
        let mut cpu = new_cpu();

        // Logic sets CF=0, OF=0, AF=0; test 0x00 -> ZF=1, PF=1
        cpu.eflags |= EFLAGS_CF | EFLAGS_OF | EFLAGS_AF;
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Logic as u32,
            width: 4,
            lhs: 0x0F0F_0000,
            rhs: 0x0000_FFFF,
            res: 0,
        };
        cpu.materialize_flags();
        assert_eq!(cpu.eflags & EFLAGS_ARITH_MASK, EFLAGS_ZF | EFLAGS_PF);

        // 8-bit SHL 0x81 << 1 = 0x02 -> CF=1, OF=(0 ^ 1)=1, PF=0
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Shl as u32,
            width: 1,
            lhs: 0x81,
            rhs: 1,
            res: 0x02,
        };
        cpu.materialize_flags();
        assert_eq!(cpu.eflags & EFLAGS_ARITH_MASK, EFLAGS_CF | EFLAGS_OF);

        // 8-bit SHR 0x81 >> 1 = 0x40 -> CF=1, OF=1 (MSB of lhs), PF=0
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Shr as u32,
            width: 1,
            lhs: 0x81,
            rhs: 1,
            res: 0x40,
        };
        cpu.materialize_flags();
        assert_eq!(cpu.eflags & EFLAGS_ARITH_MASK, EFLAGS_CF | EFLAGS_OF);

        // 8-bit SAR 0x81 >> 1 = 0xC0 -> CF=1, OF=0, SF=1, PF=1
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Sar as u32,
            width: 1,
            lhs: 0x81,
            rhs: 1,
            res: 0xC0,
        };
        cpu.materialize_flags();
        assert_eq!(
            cpu.eflags & EFLAGS_ARITH_MASK,
            EFLAGS_CF | EFLAGS_SF | EFLAGS_PF
        );

        // Shift count 0 leaves flags untouched
        let before = cpu.eflags;
        cpu.lazy_flags = LazyFlags {
            op: LazyFlagOp::Shl as u32,
            width: 4,
            lhs: 0,
            rhs: 0,
            res: 0,
        };
        cpu.materialize_flags();
        assert_eq!(cpu.eflags, before);
    }
}
