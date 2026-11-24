use core::ffi::c_void;
use core::mem::offset_of;

use crate::cpu::{EFLAGS_DF, ExitReason, Gpr, GuestCpuState, X87_SW_C2};

pub type RecompiledFn = unsafe extern "C" fn(cpu: *mut GuestCpuState);

const COM_INTERFACE_SHIFT: u32 = 16;
const COM_METHOD_MASK: u32 = 0xFFFF;
const X87_CW_ROUND_SHIFT: u16 = 10;
const X87_CW_ROUND_MASK: u16 = 0x3;
const X87_RC_NEAREST_EVEN: u16 = 0;
const X87_RC_FLOOR: u16 = 1;
const X87_RC_CEIL: u16 = 2;

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum X87HelperOp {
    Fsin = 1,
    Fcos = 2,
    Fsincos = 3,
    Fptan = 4,
    Fpatan = 5,
    F2xm1 = 6,
    Fyl2x = 7,
    Fyl2xp1 = 8,
    Fscale = 9,
    Fprem = 10,
    Fprem1 = 11,
    Frndint = 12,
}

impl X87HelperOp {
    pub const fn from_u32(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Fsin),
            2 => Some(Self::Fcos),
            3 => Some(Self::Fsincos),
            4 => Some(Self::Fptan),
            5 => Some(Self::Fpatan),
            6 => Some(Self::F2xm1),
            7 => Some(Self::Fyl2x),
            8 => Some(Self::Fyl2xp1),
            9 => Some(Self::Fscale),
            10 => Some(Self::Fprem),
            11 => Some(Self::Fprem1),
            12 => Some(Self::Frndint),
            _ => None,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct HostBridge {
    pub user_data: *mut c_void,
    pub call_indirect: unsafe extern "C" fn(cpu: *mut GuestCpuState, target_va: u32),
    pub call_import: unsafe extern "C" fn(cpu: *mut GuestCpuState, import_index: u32),
    pub call_com:
        unsafe extern "C" fn(cpu: *mut GuestCpuState, interface_id: u32, method_index: u32),
    pub materialize_flags: unsafe extern "C" fn(cpu: *mut GuestCpuState),
    pub x87_helper: unsafe extern "C" fn(cpu: *mut GuestCpuState, op: u32),
    pub cpuid: unsafe extern "C" fn(cpu: *mut GuestCpuState),
    pub rdtsc: unsafe extern "C" fn(cpu: *mut GuestCpuState),
    pub unhandled_instruction:
        unsafe extern "C" fn(cpu: *mut GuestCpuState, guest_va: u32, reason: u32),
    pub trace_hook: unsafe extern "C" fn(cpu: *mut GuestCpuState, guest_va: u32),
    pub rep_movs: unsafe extern "C" fn(cpu: *mut GuestCpuState, elem_size: u32),
    pub rep_stos: unsafe extern "C" fn(cpu: *mut GuestCpuState, elem_size: u32),
}

impl HostBridge {
    pub const fn new_stub() -> Self {
        Self {
            user_data: core::ptr::null_mut(),
            call_indirect: stub_call_indirect,
            call_import: stub_call_import,
            call_com: stub_call_com,
            materialize_flags: stub_materialize_flags,
            x87_helper: stub_x87_helper,
            cpuid: stub_cpuid,
            rdtsc: stub_rdtsc,
            unhandled_instruction: stub_unhandled_instruction,
            trace_hook: stub_trace_hook,
            rep_movs: default_rep_movs,
            rep_stos: default_rep_stos,
        }
    }
}

impl Default for HostBridge {
    fn default() -> Self {
        Self::new_stub()
    }
}

pub mod offsets {
    use super::*;

    pub const USER_DATA: usize = offset_of!(HostBridge, user_data);
    pub const CALL_INDIRECT: usize = offset_of!(HostBridge, call_indirect);
    pub const CALL_IMPORT: usize = offset_of!(HostBridge, call_import);
    pub const CALL_COM: usize = offset_of!(HostBridge, call_com);
    pub const MATERIALIZE_FLAGS: usize = offset_of!(HostBridge, materialize_flags);
    pub const X87_HELPER: usize = offset_of!(HostBridge, x87_helper);
    pub const CPUID: usize = offset_of!(HostBridge, cpuid);
    pub const RDTSC: usize = offset_of!(HostBridge, rdtsc);
    pub const UNHANDLED_INSTRUCTION: usize = offset_of!(HostBridge, unhandled_instruction);
    pub const TRACE_HOOK: usize = offset_of!(HostBridge, trace_hook);
    pub const REP_MOVS: usize = offset_of!(HostBridge, rep_movs);
    pub const REP_STOS: usize = offset_of!(HostBridge, rep_stos);
}

unsafe fn set_exit(cpu: *mut GuestCpuState, reason: ExitReason, detail: u32) {
    if let Some(state) = unsafe { cpu.as_mut() } {
        state.exit_reason = reason as u32;
        state.exit_detail = detail;
    }
}

unsafe extern "C" fn stub_call_indirect(cpu: *mut GuestCpuState, target_va: u32) {
    unsafe { set_exit(cpu, ExitReason::HostTrap, target_va) };
}

unsafe extern "C" fn stub_call_import(cpu: *mut GuestCpuState, import_index: u32) {
    unsafe { set_exit(cpu, ExitReason::HostTrap, import_index) };
}

unsafe extern "C" fn stub_call_com(
    cpu: *mut GuestCpuState,
    interface_id: u32,
    method_index: u32,
) {
    let detail = (interface_id << COM_INTERFACE_SHIFT) | (method_index & COM_METHOD_MASK);
    unsafe { set_exit(cpu, ExitReason::HostTrap, detail) };
}

unsafe extern "C" fn stub_materialize_flags(cpu: *mut GuestCpuState) {
    if let Some(state) = unsafe { cpu.as_mut() } {
        state.materialize_flags();
    }
}

unsafe extern "C" fn stub_x87_helper(cpu: *mut GuestCpuState, op: u32) {
    unsafe { set_exit(cpu, ExitReason::UnsupportedInstruction, op) };
}

unsafe extern "C" fn stub_cpuid(cpu: *mut GuestCpuState) {
    unsafe { set_exit(cpu, ExitReason::UnsupportedInstruction, 0) };
}

unsafe extern "C" fn stub_rdtsc(cpu: *mut GuestCpuState) {
    unsafe { set_exit(cpu, ExitReason::UnsupportedInstruction, 0) };
}

unsafe extern "C" fn stub_unhandled_instruction(
    cpu: *mut GuestCpuState,
    guest_va: u32,
    _reason: u32,
) {
    unsafe { set_exit(cpu, ExitReason::UnsupportedInstruction, guest_va) };
}

unsafe extern "C" fn stub_trace_hook(_cpu: *mut GuestCpuState, _guest_va: u32) {}

/// Executes an x87 transcendental/rounding helper operation on `cpu.x87`.
///
/// # Safety
/// `cpu` must either be null or point to a valid, exclusively accessible `GuestCpuState`.
pub unsafe extern "C" fn default_x87_helper(cpu: *mut GuestCpuState, op: u32) {
    let Some(state) = (unsafe { cpu.as_mut() }) else {
        return;
    };
    let Some(kind) = X87HelperOp::from_u32(op) else {
        unsafe { set_exit(cpu, ExitReason::UnsupportedInstruction, op) };
        return;
    };

    let x87 = &mut state.x87;
    match kind {
        X87HelperOp::Fsin => {
            *x87.st_mut(0) = x87.st(0).sin();
            x87.status_word &= !X87_SW_C2;
        }
        X87HelperOp::Fcos => {
            *x87.st_mut(0) = x87.st(0).cos();
            x87.status_word &= !X87_SW_C2;
        }
        X87HelperOp::Fsincos => {
            let x = x87.st(0);
            *x87.st_mut(0) = x.sin();
            x87.push(x.cos());
            x87.status_word &= !X87_SW_C2;
        }
        X87HelperOp::Fptan => {
            let x = x87.st(0);
            *x87.st_mut(0) = x.tan();
            x87.push(1.0);
            x87.status_word &= !X87_SW_C2;
        }
        X87HelperOp::Fpatan => {
            let x = x87.pop();
            let y = x87.st(0);
            *x87.st_mut(0) = y.atan2(x);
        }
        X87HelperOp::F2xm1 => {
            *x87.st_mut(0) = x87.st(0).exp2() - 1.0;
        }
        X87HelperOp::Fyl2x => {
            let x = x87.pop();
            let y = x87.st(0);
            *x87.st_mut(0) = y * x.log2();
        }
        X87HelperOp::Fyl2xp1 => {
            let x = x87.pop();
            let y = x87.st(0);
            *x87.st_mut(0) = y * (x + 1.0).log2();
        }
        X87HelperOp::Fscale => {
            let exp = x87.st(1).trunc();
            *x87.st_mut(0) *= exp.exp2();
        }
        X87HelperOp::Fprem => {
            let d = x87.st(1);
            let q = (x87.st(0) / d).trunc();
            *x87.st_mut(0) -= q * d;
            x87.status_word &= !X87_SW_C2;
        }
        X87HelperOp::Fprem1 => {
            let d = x87.st(1);
            let q = (x87.st(0) / d).round_ties_even();
            *x87.st_mut(0) -= q * d;
            x87.status_word &= !X87_SW_C2;
        }
        X87HelperOp::Frndint => {
            let val = x87.st(0);
            let mode = (x87.control_word >> X87_CW_ROUND_SHIFT) & X87_CW_ROUND_MASK;
            let rounded = match mode {
                X87_RC_NEAREST_EVEN => val.round_ties_even(),
                X87_RC_FLOOR => val.floor(),
                X87_RC_CEIL => val.ceil(),
                _ => val.trunc(),
            };
            *x87.st_mut(0) = rounded;
        }
    }
}

unsafe fn copy_elem(src: *const u8, dst: *mut u8, elem: usize) {
    match elem {
        1 => unsafe { dst.write(src.read()) },
        2 => unsafe { dst.cast::<u16>().write_unaligned(src.cast::<u16>().read_unaligned()) },
        4 => unsafe { dst.cast::<u32>().write_unaligned(src.cast::<u32>().read_unaligned()) },
        _ => {}
    }
}

unsafe fn write_elem(dst: *mut u8, eax: u32, elem: usize) {
    match elem {
        1 => unsafe { dst.write(eax as u8) },
        2 => {
            let bytes = (eax as u16).to_le_bytes();
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, 2) };
        }
        4 => {
            let bytes = eax.to_le_bytes();
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, 4) };
        }
        _ => {}
    }
}

/// Bulk host implementation for x86 `rep movsb/movsw/movsd`.
///
/// # Safety
/// `cpu` must be null or point to a valid, exclusively accessible `GuestCpuState` whose
/// `mem_base` covers `[esi, esi + ecx*elem_size)` and `[edi, edi + ecx*elem_size)`.
pub unsafe extern "C" fn default_rep_movs(cpu: *mut GuestCpuState, elem_size: u32) {
    let Some(state) = (unsafe { cpu.as_mut() }) else {
        return;
    };
    let elem = elem_size as usize;
    let ecx = state.gpr[Gpr::Ecx as usize];
    if state.mem_base.is_null() || ecx == 0 || !matches!(elem, 1 | 2 | 4) {
        return;
    }

    let base = state.mem_base;
    let mut esi = state.gpr[Gpr::Esi as usize];
    let mut edi = state.gpr[Gpr::Edi as usize];
    let count = ecx as usize;
    let byte_len = count * elem;

    if (state.eflags & EFLAGS_DF) == 0 {
        let no_forward_rle =
            edi <= esi || (edi as usize) >= (esi as usize).saturating_add(byte_len);
        if no_forward_rle {
            unsafe {
                core::ptr::copy(base.add(esi as usize), base.add(edi as usize), byte_len);
            }
        } else {
            for _ in 0..count {
                unsafe { copy_elem(base.add(esi as usize), base.add(edi as usize), elem) };
                esi = esi.wrapping_add(elem_size);
                edi = edi.wrapping_add(elem_size);
            }
            state.gpr[Gpr::Esi as usize] = esi;
            state.gpr[Gpr::Edi as usize] = edi;
            state.gpr[Gpr::Ecx as usize] = 0;
            return;
        }
        state.gpr[Gpr::Esi as usize] = esi.wrapping_add(byte_len as u32);
        state.gpr[Gpr::Edi as usize] = edi.wrapping_add(byte_len as u32);
    } else {
        for _ in 0..count {
            unsafe { copy_elem(base.add(esi as usize), base.add(edi as usize), elem) };
            esi = esi.wrapping_sub(elem_size);
            edi = edi.wrapping_sub(elem_size);
        }
        state.gpr[Gpr::Esi as usize] = esi;
        state.gpr[Gpr::Edi as usize] = edi;
    }
    state.gpr[Gpr::Ecx as usize] = 0;
}

/// Bulk host implementation for x86 `rep stosb/stosw/stosd`.
///
/// # Safety
/// `cpu` must be null or point to a valid, exclusively accessible `GuestCpuState` whose
/// `mem_base` covers `[edi, edi + ecx*elem_size)`.
pub unsafe extern "C" fn default_rep_stos(cpu: *mut GuestCpuState, elem_size: u32) {
    let Some(state) = (unsafe { cpu.as_mut() }) else {
        return;
    };
    let elem = elem_size as usize;
    let ecx = state.gpr[Gpr::Ecx as usize];
    if state.mem_base.is_null() || ecx == 0 || !matches!(elem, 1 | 2 | 4) {
        return;
    }

    let base = state.mem_base;
    let eax = state.gpr[Gpr::Eax as usize];
    let mut edi = state.gpr[Gpr::Edi as usize];
    let count = ecx as usize;
    let byte_len = count * elem;

    if (state.eflags & EFLAGS_DF) == 0 {
        let dst = unsafe { base.add(edi as usize) };
        let fill_byte = eax as u8;
        let uniform_byte = elem == 1
            || (elem == 2 && (eax as u16) == u16::from_le_bytes([fill_byte, fill_byte]))
            || (elem == 4 && eax == u32::from_le_bytes([fill_byte, fill_byte, fill_byte, fill_byte]));
        if uniform_byte {
            unsafe { core::ptr::write_bytes(dst, fill_byte, byte_len) };
        } else {
            let slice = unsafe { core::slice::from_raw_parts_mut(dst, byte_len) };
            if elem == 2 {
                let bytes = (eax as u16).to_le_bytes();
                for chunk in slice.chunks_exact_mut(2) {
                    chunk.copy_from_slice(&bytes);
                }
            } else {
                let bytes = eax.to_le_bytes();
                for chunk in slice.chunks_exact_mut(4) {
                    chunk.copy_from_slice(&bytes);
                }
            }
        }
        state.gpr[Gpr::Edi as usize] = edi.wrapping_add(byte_len as u32);
    } else {
        for _ in 0..count {
            unsafe { write_elem(base.add(edi as usize), eax, elem) };
            edi = edi.wrapping_sub(elem_size);
        }
        state.gpr[Gpr::Edi as usize] = edi;
    }
    state.gpr[Gpr::Ecx as usize] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{EFLAGS_ZF, LazyFlagOp};

    #[test]
    fn bridge_offsets_are_distinct_and_ordered() {
        let ptr_size = core::mem::size_of::<usize>();
        let all_offsets = [
            offsets::USER_DATA,
            offsets::CALL_INDIRECT,
            offsets::CALL_IMPORT,
            offsets::CALL_COM,
            offsets::MATERIALIZE_FLAGS,
            offsets::X87_HELPER,
            offsets::CPUID,
            offsets::RDTSC,
            offsets::UNHANDLED_INSTRUCTION,
            offsets::TRACE_HOOK,
            offsets::REP_MOVS,
            offsets::REP_STOS,
        ];
        for (idx, window) in all_offsets.windows(2).enumerate() {
            assert_eq!(
                window[1] - window[0],
                ptr_size,
                "unexpected stride at field index {idx}"
            );
        }
        assert_eq!(
            all_offsets.last().copied().unwrap() + ptr_size,
            core::mem::size_of::<HostBridge>()
        );
    }

    #[test]
    fn stub_callbacks_update_cpu_state() {
        let bridge = HostBridge::new_stub();
        let mut cpu = GuestCpuState::new(core::ptr::null_mut(), &bridge);

        unsafe {
            (bridge.call_indirect)(&mut cpu, 0x0040_1234);
        }
        assert_eq!(cpu.exit_reason, ExitReason::HostTrap as u32);
        assert_eq!(cpu.exit_detail, 0x0040_1234);

        unsafe {
            (bridge.call_import)(&mut cpu, 7);
        }
        assert_eq!(cpu.exit_reason, ExitReason::HostTrap as u32);
        assert_eq!(cpu.exit_detail, 7);

        unsafe {
            (bridge.call_com)(&mut cpu, 3, 17);
        }
        assert_eq!(cpu.exit_reason, ExitReason::HostTrap as u32);
        assert_eq!(cpu.exit_detail, (3 << 16) | 17);

        unsafe {
            (bridge.x87_helper)(&mut cpu, X87HelperOp::Fsin as u32);
        }
        assert_eq!(cpu.exit_reason, ExitReason::UnsupportedInstruction as u32);
        assert_eq!(cpu.exit_detail, X87HelperOp::Fsin as u32);

        unsafe {
            (bridge.cpuid)(&mut cpu);
        }
        assert_eq!(cpu.exit_reason, ExitReason::UnsupportedInstruction as u32);

        unsafe {
            (bridge.rdtsc)(&mut cpu);
        }
        assert_eq!(cpu.exit_reason, ExitReason::UnsupportedInstruction as u32);

        unsafe {
            (bridge.unhandled_instruction)(&mut cpu, 0x0050_0000, 99);
        }
        assert_eq!(cpu.exit_reason, ExitReason::UnsupportedInstruction as u32);
        assert_eq!(cpu.exit_detail, 0x0050_0000);

        cpu.lazy_flags.op = LazyFlagOp::Sub as u32;
        cpu.lazy_flags.width = 4;
        cpu.lazy_flags.lhs = 42;
        cpu.lazy_flags.rhs = 42;
        cpu.lazy_flags.res = 0;
        unsafe {
            (bridge.materialize_flags)(&mut cpu);
            (bridge.trace_hook)(&mut cpu, 0x0040_1000);
        }
        assert_ne!(cpu.eflags & EFLAGS_ZF, 0);
        assert_eq!(cpu.lazy_flags.op, LazyFlagOp::None as u32);
    }

    #[test]
    fn default_rep_movs_handles_forward_overlap_backward_and_zero() {
        let bridge = HostBridge::new_stub();
        let mut mem = [0u8; 64];
        mem[0..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut cpu = GuestCpuState::new(mem.as_mut_ptr(), &bridge);

        // Non-overlapping forward dword copy (ecx=2, elem=4).
        cpu.set_gpr(Gpr::Esi, 0);
        cpu.set_gpr(Gpr::Edi, 16);
        cpu.set_gpr(Gpr::Ecx, 2);
        unsafe { (bridge.rep_movs)(&mut cpu, 4) };
        assert_eq!(&mem[16..24], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(cpu.gpr(Gpr::Esi), 8);
        assert_eq!(cpu.gpr(Gpr::Edi), 24);
        assert_eq!(cpu.gpr(Gpr::Ecx), 0);

        // Forward overlapping RLE copy: edi = esi + 1 repeats byte across 5 bytes.
        mem[30] = 0xAB;
        cpu.set_gpr(Gpr::Esi, 30);
        cpu.set_gpr(Gpr::Edi, 31);
        cpu.set_gpr(Gpr::Ecx, 5);
        unsafe { (bridge.rep_movs)(&mut cpu, 1) };
        assert_eq!(&mem[30..36], &[0xAB; 6]);
        assert_eq!(cpu.gpr(Gpr::Esi), 35);
        assert_eq!(cpu.gpr(Gpr::Edi), 36);
        assert_eq!(cpu.gpr(Gpr::Ecx), 0);

        // Backward copy (DF=1).
        cpu.eflags |= EFLAGS_DF;
        mem[40..44].copy_from_slice(&[9, 8, 7, 6]);
        cpu.set_gpr(Gpr::Esi, 43);
        cpu.set_gpr(Gpr::Edi, 53);
        cpu.set_gpr(Gpr::Ecx, 4);
        unsafe { (bridge.rep_movs)(&mut cpu, 1) };
        assert_eq!(&mem[50..54], &[9, 8, 7, 6]);
        assert_eq!(cpu.gpr(Gpr::Esi), 39);
        assert_eq!(cpu.gpr(Gpr::Edi), 49);
        assert_eq!(cpu.gpr(Gpr::Ecx), 0);

        // ecx = 0 is a no-op.
        cpu.eflags &= !EFLAGS_DF;
        cpu.set_gpr(Gpr::Esi, 0);
        cpu.set_gpr(Gpr::Edi, 50);
        cpu.set_gpr(Gpr::Ecx, 0);
        unsafe { (bridge.rep_movs)(&mut cpu, 4) };
        assert_eq!(&mem[50..54], &[9, 8, 7, 6]);
        assert_eq!(cpu.gpr(Gpr::Esi), 0);
        assert_eq!(cpu.gpr(Gpr::Edi), 50);
    }

    #[test]
    fn default_rep_stos_handles_fills_backward_and_zero() {
        let bridge = HostBridge::new_stub();
        let mut mem = [0xFFu8; 64];
        let mut cpu = GuestCpuState::new(mem.as_mut_ptr(), &bridge);

        // Byte fill.
        cpu.set_gpr(Gpr::Eax, 0x3C);
        cpu.set_gpr(Gpr::Edi, 2);
        cpu.set_gpr(Gpr::Ecx, 5);
        unsafe { (bridge.rep_stos)(&mut cpu, 1) };
        assert_eq!(&mem[2..7], &[0x3C; 5]);
        assert_eq!(cpu.gpr(Gpr::Edi), 7);
        assert_eq!(cpu.gpr(Gpr::Ecx), 0);

        // Non-uniform word fill.
        cpu.set_gpr(Gpr::Eax, 0x1234);
        cpu.set_gpr(Gpr::Edi, 10);
        cpu.set_gpr(Gpr::Ecx, 2);
        unsafe { (bridge.rep_stos)(&mut cpu, 2) };
        assert_eq!(&mem[10..14], &[0x34, 0x12, 0x34, 0x12]);
        assert_eq!(cpu.gpr(Gpr::Edi), 14);

        // Non-uniform dword fill.
        cpu.set_gpr(Gpr::Eax, 0xDEAD_BEEF);
        cpu.set_gpr(Gpr::Edi, 16);
        cpu.set_gpr(Gpr::Ecx, 2);
        unsafe { (bridge.rep_stos)(&mut cpu, 4) };
        assert_eq!(
            &mem[16..24],
            &[0xEF, 0xBE, 0xAD, 0xDE, 0xEF, 0xBE, 0xAD, 0xDE]
        );
        assert_eq!(cpu.gpr(Gpr::Edi), 24);

        // Zero fill fast path (dword uniform 0).
        cpu.set_gpr(Gpr::Eax, 0);
        cpu.set_gpr(Gpr::Edi, 16);
        cpu.set_gpr(Gpr::Ecx, 2);
        unsafe { (bridge.rep_stos)(&mut cpu, 4) };
        assert_eq!(&mem[16..24], &[0u8; 8]);

        // Backward dword fill (DF=1).
        cpu.eflags |= EFLAGS_DF;
        cpu.set_gpr(Gpr::Eax, 0x0102_0304);
        cpu.set_gpr(Gpr::Edi, 44);
        cpu.set_gpr(Gpr::Ecx, 2);
        unsafe { (bridge.rep_stos)(&mut cpu, 4) };
        assert_eq!(
            &mem[40..48],
            &[0x04, 0x03, 0x02, 0x01, 0x04, 0x03, 0x02, 0x01]
        );
        assert_eq!(cpu.gpr(Gpr::Edi), 36);
        assert_eq!(cpu.gpr(Gpr::Ecx), 0);

        // ecx = 0 is a no-op.
        cpu.eflags &= !EFLAGS_DF;
        cpu.set_gpr(Gpr::Eax, 0x77);
        cpu.set_gpr(Gpr::Edi, 40);
        cpu.set_gpr(Gpr::Ecx, 0);
        unsafe { (bridge.rep_stos)(&mut cpu, 1) };
        assert_eq!(mem[40], 0x04);
        assert_eq!(cpu.gpr(Gpr::Edi), 40);
    }
}
