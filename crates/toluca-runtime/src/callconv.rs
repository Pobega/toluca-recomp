use toluca_core::cpu::{Gpr, GuestCpuState};

use crate::arena::{ArenaError, GuestArena};

const STACK_WORD_BYTES: u32 = 4;

pub fn peek_arg_u32(
    cpu: &GuestCpuState,
    arena: &GuestArena,
    index: u32,
) -> Result<u32, ArenaError> {
    let esp = cpu.gpr(Gpr::Esp);
    let arg_byte_offset = index
        .checked_add(1)
        .and_then(|slots| slots.checked_mul(STACK_WORD_BYTES))
        .ok_or(ArenaError::AddressOverflow {
            addr: esp,
            len: index as usize,
        })?;
    let addr = esp
        .checked_add(arg_byte_offset)
        .ok_or(ArenaError::AddressOverflow {
            addr: esp,
            len: arg_byte_offset as usize,
        })?;
    arena.read_u32(addr)
}

pub fn peek_arg_i32(
    cpu: &GuestCpuState,
    arena: &GuestArena,
    index: u32,
) -> Result<i32, ArenaError> {
    Ok(peek_arg_u32(cpu, arena, index)? as i32)
}

pub fn return_stdcall(
    cpu: &mut GuestCpuState,
    arena: &GuestArena,
    arg_count: u32,
    ret_eax: u32,
) -> Result<(), ArenaError> {
    let esp = cpu.gpr(Gpr::Esp);
    let ret_va = arena.read_u32(esp)?;
    let arg_bytes = arg_count
        .checked_mul(STACK_WORD_BYTES)
        .ok_or(ArenaError::AddressOverflow {
            addr: esp,
            len: arg_count as usize,
        })?;
    let total_pop = STACK_WORD_BYTES
        .checked_add(arg_bytes)
        .ok_or(ArenaError::AddressOverflow {
            addr: esp,
            len: arg_bytes as usize,
        })?;
    let new_esp = esp
        .checked_add(total_pop)
        .ok_or(ArenaError::AddressOverflow {
            addr: esp,
            len: total_pop as usize,
        })?;

    cpu.eip = ret_va;
    cpu.set_gpr(Gpr::Esp, new_esp);
    cpu.set_gpr(Gpr::Eax, ret_eax);
    Ok(())
}

pub fn return_stdcall_u64(
    cpu: &mut GuestCpuState,
    arena: &GuestArena,
    arg_count: u32,
    ret_val: u64,
) -> Result<(), ArenaError> {
    return_stdcall(cpu, arena, arg_count, ret_val as u32)?;
    cpu.set_gpr(Gpr::Edx, (ret_val >> 32) as u32);
    Ok(())
}

pub fn return_cdecl(
    cpu: &mut GuestCpuState,
    arena: &GuestArena,
    ret_eax: u32,
) -> Result<(), ArenaError> {
    return_stdcall(cpu, arena, 0, ret_eax)
}

pub fn return_cdecl_u64(
    cpu: &mut GuestCpuState,
    arena: &GuestArena,
    ret_val: u64,
) -> Result<(), ArenaError> {
    return_stdcall_u64(cpu, arena, 0, ret_val)
}

pub fn push_u32(
    cpu: &mut GuestCpuState,
    arena: &mut GuestArena,
    val: u32,
) -> Result<(), ArenaError> {
    let new_esp = cpu
        .gpr(Gpr::Esp)
        .checked_sub(STACK_WORD_BYTES)
        .ok_or(ArenaError::NullGuardViolation(0))?;
    arena.write_u32(new_esp, val)?;
    cpu.set_gpr(Gpr::Esp, new_esp);
    Ok(())
}

pub fn pop_u32(cpu: &mut GuestCpuState, arena: &GuestArena) -> Result<u32, ArenaError> {
    let esp = cpu.gpr(Gpr::Esp);
    let val = arena.read_u32(esp)?;
    let new_esp = esp
        .checked_add(STACK_WORD_BYTES)
        .ok_or(ArenaError::AddressOverflow {
            addr: esp,
            len: STACK_WORD_BYTES as usize,
        })?;
    cpu.set_gpr(Gpr::Esp, new_esp);
    Ok(val)
}

#[cfg(test)]
mod tests {
    use super::*;
    use toluca_core::bridge::HostBridge;
    use toluca_core::memory::{DEFAULT_IMAGE_BASE, GUEST_MAIN_STACK_TOP};

    #[test]
    fn stack_push_pop_peek_and_returns() {
        let mut arena = GuestArena::new().unwrap();
        arena.init_thread_environment(DEFAULT_IMAGE_BASE).unwrap();
        let bridge = HostBridge::new_stub();
        let mut cpu = GuestCpuState::new(arena.base_ptr(), &bridge);

        push_u32(&mut cpu, &mut arena, 0x2222_2222).unwrap();
        push_u32(&mut cpu, &mut arena, 0xFFFF_FFFC).unwrap();
        push_u32(&mut cpu, &mut arena, 0x0040_1234).unwrap();

        assert_eq!(peek_arg_u32(&cpu, &arena, 0).unwrap(), 0xFFFF_FFFC);
        assert_eq!(peek_arg_i32(&cpu, &arena, 0).unwrap(), -4);
        assert_eq!(peek_arg_u32(&cpu, &arena, 1).unwrap(), 0x2222_2222);

        return_stdcall(&mut cpu, &arena, 2, 0xABCD_EF01).unwrap();
        assert_eq!(cpu.eip, 0x0040_1234);
        assert_eq!(cpu.gpr(Gpr::Esp), GUEST_MAIN_STACK_TOP);
        assert_eq!(cpu.gpr(Gpr::Eax), 0xABCD_EF01);

        push_u32(&mut cpu, &mut arena, 0x1111_1111).unwrap();
        push_u32(&mut cpu, &mut arena, 0x0040_5678).unwrap();
        return_cdecl_u64(&mut cpu, &arena, 0x1234_5678_9ABC_DEF0).unwrap();
        assert_eq!(cpu.eip, 0x0040_5678);
        assert_eq!(cpu.gpr(Gpr::Eax), 0x9ABC_DEF0);
        assert_eq!(cpu.gpr(Gpr::Edx), 0x1234_5678);
        assert_eq!(pop_u32(&mut cpu, &arena).unwrap(), 0x1111_1111);
        assert_eq!(cpu.gpr(Gpr::Esp), GUEST_MAIN_STACK_TOP);
    }
}
