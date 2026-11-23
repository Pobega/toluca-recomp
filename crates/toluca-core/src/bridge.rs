use core::ffi::c_void;
use core::mem::offset_of;

use crate::cpu::{ExitReason, GuestCpuState};

pub type RecompiledFn = unsafe extern "C" fn(cpu: *mut GuestCpuState);

const COM_INTERFACE_SHIFT: u32 = 16;
const COM_METHOD_MASK: u32 = 0xFFFF;

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
}
