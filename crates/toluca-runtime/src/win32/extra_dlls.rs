use toluca_core::GuestAddr;
use toluca_core::cpu::{Gpr, GuestCpuState};

use crate::arena::GuestArena;
use crate::callconv::{peek_arg_u32, return_cdecl, return_stdcall};
use crate::runtime::{ImportHandler, ImportRegistry, RuntimeContext, RuntimeError};
use crate::win32::ERROR_FILE_NOT_FOUND;

const MSVC7_STRING_SSO_CAP: usize = 15;
const MSVC7_STRING_BUF_OFF: u32 = 4;
const MSVC7_STRING_SIZE_OFF: u32 = 20;
const MSVC7_STRING_RES_OFF: u32 = 24;
const MAX_STRING_BYTES: usize = 65536;

const S_OK: u32 = 0;

pub fn register_imports(registry: &mut ImportRegistry) {
    let msvcp70 = "msvcp70.dll";
    let cpp_entries: &[(&str, ImportHandler)] = &[
        (
            "??0?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QAE@PBD@Z",
            msvc7_string_ctor_cstr,
        ),
        (
            "??0?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QAE@ABV01@@Z",
            msvc7_string_ctor_copy,
        ),
        (
            "??1?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@QAE@XZ",
            msvc7_string_dtor,
        ),
        (
            "??Mstd@@YA_NABV?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@0@0@Z",
            msvc7_string_less,
        ),
    ];
    for &(name, handler) in cpp_entries {
        registry.register_name(msvcp70, name, handler);
    }

    registry.register_name("advapi32.dll", "RegOpenKeyA", reg_open_key_a);
    registry.register_name("advapi32.dll", "RegQueryValueExA", reg_query_value_ex_a);
    registry.register_name("advapi32.dll", "RegCloseKey", reg_close_key);

    registry.register_name("ole32.dll", "CoInitialize", co_initialize);
}

fn init_msvc7_string(
    ctx: &mut RuntimeContext<'_>,
    this_va: GuestAddr,
    bytes: &[u8],
) -> Result<(), RuntimeError> {
    ctx.arena.write_u32(this_va, 0)?;
    if bytes.len() <= MSVC7_STRING_SSO_CAP {
        ctx.arena.zero_range(this_va + MSVC7_STRING_BUF_OFF, 16)?;
        if !bytes.is_empty() {
            ctx.arena.write_bytes(this_va + MSVC7_STRING_BUF_OFF, bytes)?;
        }
        ctx.arena.write_u32(this_va + MSVC7_STRING_SIZE_OFF, bytes.len() as u32)?;
        ctx.arena.write_u32(this_va + MSVC7_STRING_RES_OFF, MSVC7_STRING_SSO_CAP as u32)?;
    } else {
        let cap = (bytes.len() + 1).next_power_of_two().max(16);
        let heap_ptr = ctx.heap.alloc(ctx.arena, cap, true).unwrap_or(0);
        if heap_ptr != 0 {
            ctx.arena.write_bytes(heap_ptr, bytes)?;
            ctx.arena.write_u8(heap_ptr + bytes.len() as u32, 0)?;
            ctx.arena.write_u32(this_va + MSVC7_STRING_BUF_OFF, heap_ptr)?;
            ctx.arena.write_u32(this_va + MSVC7_STRING_SIZE_OFF, bytes.len() as u32)?;
            ctx.arena.write_u32(this_va + MSVC7_STRING_RES_OFF, (cap - 1) as u32)?;
        } else {
            ctx.arena.write_u32(this_va + MSVC7_STRING_SIZE_OFF, 0)?;
            ctx.arena.write_u32(this_va + MSVC7_STRING_RES_OFF, MSVC7_STRING_SSO_CAP as u32)?;
        }
    }
    Ok(())
}

fn read_msvc7_string(arena: &GuestArena, this_va: GuestAddr) -> Result<Vec<u8>, RuntimeError> {
    let size = arena.read_u32(this_va + MSVC7_STRING_SIZE_OFF)? as usize;
    let res = arena.read_u32(this_va + MSVC7_STRING_RES_OFF)? as usize;
    let clamped_size = size.min(MAX_STRING_BYTES);
    let data_va = if res > MSVC7_STRING_SSO_CAP {
        arena.read_u32(this_va + MSVC7_STRING_BUF_OFF)?
    } else {
        this_va + MSVC7_STRING_BUF_OFF
    };
    if clamped_size == 0 || data_va == 0 {
        return Ok(Vec::new());
    }
    Ok(arena.read_bytes(data_va, clamped_size)?.to_vec())
}

fn msvc7_string_ctor_cstr(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_va = cpu.gpr(Gpr::Ecx);
    let cstr_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let bytes = if cstr_ptr != 0 {
        ctx.arena.read_cstr(cstr_ptr, MAX_STRING_BYTES)?.into_bytes()
    } else {
        Vec::new()
    };
    init_msvc7_string(ctx, this_va, &bytes)?;
    return_stdcall(cpu, ctx.arena, 1, this_va)?;
    Ok(())
}

fn msvc7_string_ctor_copy(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_va = cpu.gpr(Gpr::Ecx);
    let other_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    let bytes = if other_va != 0 {
        read_msvc7_string(ctx.arena, other_va)?
    } else {
        Vec::new()
    };
    init_msvc7_string(ctx, this_va, &bytes)?;
    return_stdcall(cpu, ctx.arena, 1, this_va)?;
    Ok(())
}

fn msvc7_string_dtor(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_va = cpu.gpr(Gpr::Ecx);
    let res = ctx.arena.read_u32(this_va + MSVC7_STRING_RES_OFF)? as usize;
    if res > MSVC7_STRING_SSO_CAP {
        let heap_ptr = ctx.arena.read_u32(this_va + MSVC7_STRING_BUF_OFF)?;
        if heap_ptr != 0 {
            let _ = ctx.heap.free(ctx.arena, heap_ptr);
        }
    }
    ctx.arena.write_u32(this_va + MSVC7_STRING_SIZE_OFF, 0)?;
    ctx.arena.write_u32(this_va + MSVC7_STRING_RES_OFF, MSVC7_STRING_SSO_CAP as u32)?;
    return_stdcall(cpu, ctx.arena, 0, this_va)?;
    Ok(())
}

fn msvc7_string_less(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let lhs_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    let rhs_va = peek_arg_u32(cpu, ctx.arena, 1)?;
    let lhs = read_msvc7_string(ctx.arena, lhs_va)?;
    let rhs = read_msvc7_string(ctx.arena, rhs_va)?;
    return_cdecl(cpu, ctx.arena, u32::from(lhs < rhs))?;
    Ok(())
}

fn reg_open_key_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let phk_result = peek_arg_u32(cpu, ctx.arena, 2)?;
    if phk_result != 0 {
        ctx.arena.write_u32(phk_result, 0)?;
    }
    return_stdcall(cpu, ctx.arena, 3, ERROR_FILE_NOT_FOUND)?;
    Ok(())
}

fn reg_query_value_ex_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 6, ERROR_FILE_NOT_FOUND)?;
    Ok(())
}

fn reg_close_key(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn co_initialize(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, S_OK)?;
    Ok(())
}
