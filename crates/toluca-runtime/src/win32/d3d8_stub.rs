use toluca_core::cpu::GuestCpuState;

use crate::callconv::{peek_arg_u32, return_stdcall};
use crate::runtime::{COM_THUNK_BASE, ImportRegistry, RuntimeContext, RuntimeError, THUNK_STRIDE};
use crate::win32::{ERROR_NOT_ENOUGH_MEMORY, ERROR_SUCCESS};

const IDIRECT3D8_METHOD_COUNT: usize = 16;
const IDIRECT3D8_VTABLE_BYTES: usize = IDIRECT3D8_METHOD_COUNT * 4;
const IDIRECT3D8_OBJECT_BYTES: usize = 16;

pub fn register_imports(registry: &mut ImportRegistry) {
    registry.register_name("d3d8.dll", "Direct3DCreate8", direct3d_create_8);
}

fn direct3d_create_8(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let sdk_version = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.win32.d3d8_create_calls.push(sdk_version);

    let Ok(vtable_va) = ctx.heap.alloc(ctx.arena, IDIRECT3D8_VTABLE_BYTES, true) else {
        *ctx.last_error = ERROR_NOT_ENOUGH_MEMORY;
        return_stdcall(cpu, ctx.arena, 1, 0)?;
        return Ok(());
    };

    for method_idx in 0..IDIRECT3D8_METHOD_COUNT {
        let thunk_va = COM_THUNK_BASE + (method_idx as u32) * THUNK_STRIDE;
        ctx.arena.write_u32(vtable_va + (method_idx as u32) * 4, thunk_va)?;
    }

    let Ok(obj_va) = ctx.heap.alloc(ctx.arena, IDIRECT3D8_OBJECT_BYTES, true) else {
        let _ = ctx.heap.free(ctx.arena, vtable_va);
        *ctx.last_error = ERROR_NOT_ENOUGH_MEMORY;
        return_stdcall(cpu, ctx.arena, 1, 0)?;
        return Ok(());
    };

    ctx.arena.write_u32(obj_va, vtable_va)?;
    ctx.arena.write_u32(obj_va + 4, 1)?;
    ctx.arena.write_u32(obj_va + 8, sdk_version)?;
    *ctx.last_error = ERROR_SUCCESS;

    return_stdcall(cpu, ctx.arena, 1, obj_va)?;
    Ok(())
}
