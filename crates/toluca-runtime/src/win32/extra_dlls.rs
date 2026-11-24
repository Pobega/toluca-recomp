use toluca_core::GuestAddr;
use toluca_core::aot::ImportSymbol;
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

const E_NOINTERFACE: u32 = 0x8000_4002;
const E_FAIL: u32 = 0x8000_4005;
const S_OK: u32 = 0;
const DINPUT8_COM_DLL: &str = "__dinput8_com__";
const DSOUND_COM_DLL: &str = "__dsound_com__";
const DI8_VTABLE_SLOTS: usize = 8;
const DIDEV8_VTABLE_SLOTS: usize = 32;
const DS8_VTABLE_SLOTS: usize = 12;
const DSB8_VTABLE_SLOTS: usize = 22;
const DS3DLISTENER_VTABLE_SLOTS: usize = 18;
const DS3DBUFFER_VTABLE_SLOTS: usize = 21;
const DS_DEFAULT_BUFFER_BYTES: u32 = 65536;
const IID_IUNKNOWN_DATA1: u32 = 0x0000_0000;
const IID_IDSDIRECTSOUNDBUFFER_DATA1: u32 = 0x279A_FA85;
const IID_IDSDIRECTSOUNDBUFFER8_DATA1: u32 = 0x6825_A449;
const IID_IDSDIRECTSOUND3DLISTENER_DATA1: u32 = 0x279A_FA84;
const IID_IDSDIRECTSOUND3DBUFFER_DATA1: u32 = 0x279A_FA86;

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
    registry.register_name("dinput8.dll", "DirectInput8Create", direct_input_8_create);
    registry.register_ordinal("dsound.dll", 11, direct_sound_create_8);
    registry.register_name("dsound.dll", "DirectSoundCreate8", direct_sound_create_8);
    registry.register_name("dsound.dll", "DirectSoundCreate", direct_sound_create_8);

    let di_com_entries: &[(&str, ImportHandler)] = &[
        ("QueryInterface", di_query_interface),
        ("AddRef", stdcall_one_1),
        ("Release", stdcall_zero_1),
        ("DI8_CreateDevice", di8_create_device),
        ("DI8_EnumDevices", stdcall_zero_5),
        ("DIDev8_GetDeviceState", didev8_get_device_state),
        ("DIDev8_GetDeviceData", didev8_get_device_data),
        ("Std0_1", stdcall_zero_1),
        ("Std0_2", stdcall_zero_2),
        ("Std0_3", stdcall_zero_3),
        ("Std0_4", stdcall_zero_4),
        ("Std0_5", stdcall_zero_5),
    ];
    for &(name, handler) in di_com_entries {
        registry.register_name(DINPUT8_COM_DLL, name, handler);
    }

    let ds_com_entries: &[(&str, ImportHandler)] = &[
        ("QueryInterface", ds_query_interface),
        ("AddRef", stdcall_one_1),
        ("Release", stdcall_zero_1),
        ("DS_CreateSoundBuffer", ds_create_sound_buffer),
        ("DS_GetCaps", ds_get_caps),
        ("DS_DuplicateSoundBuffer", ds_duplicate_sound_buffer),
        ("DS_GetSpeakerConfig", ds_write_zero_2),
        ("DS_VerifyCertification", ds_write_zero_2),
        ("DSB_GetCaps", dsb_get_caps),
        ("DSB_GetCurrentPosition", dsb_get_current_position),
        ("DSB_GetVolume", ds_write_zero_2),
        ("DSB_GetPan", ds_write_zero_2),
        ("DSB_GetFrequency", dsb_get_frequency),
        ("DSB_GetStatus", dsb_get_status),
        ("DSB_Lock", dsb_lock),
        ("DSB_Play", dsb_play),
        ("DSB_Stop", dsb_stop),
        ("DSB_Unlock", stdcall_zero_5),
        ("Std0_1", stdcall_zero_1),
        ("Std0_2", stdcall_zero_2),
        ("Std0_3", stdcall_zero_3),
        ("Std0_4", stdcall_zero_4),
        ("Std0_5", stdcall_zero_5),
        ("Std0_8", stdcall_zero_8),
    ];
    for &(name, handler) in ds_com_entries {
        registry.register_name(DSOUND_COM_DLL, name, handler);
    }

    let bink = "binkw32.dll";
    let bink_entries: &[(&str, ImportHandler)] = &[
        ("_BinkSetSoundOnOff@8", stdcall_zero_2),
        ("_BinkOpen@8", stdcall_zero_2),
        ("_BinkSetSoundTrack@8", stdcall_zero_2),
        ("_BinkSetSoundSystem@8", stdcall_zero_2),
        ("_BinkOpenDirectSound@4", stdcall_zero_1),
        ("_BinkClose@4", stdcall_zero_1),
        ("_BinkCopyToBuffer@28", stdcall_zero_7),
        ("_BinkNextFrame@4", stdcall_zero_1),
        ("_BinkDoFrame@4", stdcall_zero_1),
        ("_BinkWait@4", stdcall_zero_1),
        ("_BinkPause@8", stdcall_zero_2),
    ];
    for &(name, handler) in bink_entries {
        registry.register_name(bink, name, handler);
    }
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

fn alloc_com_object(
    ctx: &mut RuntimeContext<'_>,
    dll_name: &str,
    slot_names: &[&str],
    extra_bytes: usize,
) -> Result<GuestAddr, RuntimeError> {
    let vtable = ctx.heap.alloc(ctx.arena, slot_names.len() * 4, true)?;
    for (idx, &name) in slot_names.iter().enumerate() {
        let thunk_va = ctx
            .register_dynamic_import(dll_name, ImportSymbol::Name(name.to_owned()))
            .unwrap_or(0);
        ctx.arena.write_u32(vtable + (idx as u32) * 4, thunk_va)?;
    }
    let obj = ctx.heap.alloc(ctx.arena, 4 + extra_bytes, true)?;
    ctx.arena.write_u32(obj, vtable)?;
    Ok(obj)
}

fn direct_input_8_create(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ppv_out = peek_arg_u32(cpu, ctx.arena, 3)?;
    if ppv_out == 0 {
        return_stdcall(cpu, ctx.arena, 5, E_FAIL)?;
        return Ok(());
    }
    const DI8_SLOTS: [&str; DI8_VTABLE_SLOTS] = [
        "QueryInterface",
        "AddRef",
        "Release",
        "DI8_CreateDevice",
        "DI8_EnumDevices",
        "Std0_2",
        "Std0_3",
        "Std0_4",
    ];
    let obj = alloc_com_object(ctx, DINPUT8_COM_DLL, &DI8_SLOTS, 0)?;
    ctx.arena.write_u32(ppv_out, obj)?;
    return_stdcall(cpu, ctx.arena, 5, S_OK)?;
    Ok(())
}

fn di_query_interface(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ppv = peek_arg_u32(cpu, ctx.arena, 2)?;
    if ppv != 0 {
        ctx.arena.write_u32(ppv, 0)?;
    }
    return_stdcall(cpu, ctx.arena, 3, E_NOINTERFACE)?;
    Ok(())
}

fn di8_create_device(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let pp_dev = peek_arg_u32(cpu, ctx.arena, 2)?;
    if pp_dev == 0 {
        return_stdcall(cpu, ctx.arena, 4, E_FAIL)?;
        return Ok(());
    }
    const DIDEV8_SLOTS: [&str; DIDEV8_VTABLE_SLOTS] = [
        "QueryInterface",
        "AddRef",
        "Release",
        "Std0_2",
        "Std0_4",
        "Std0_3",
        "Std0_3",
        "Std0_1",
        "Std0_1",
        "DIDev8_GetDeviceState",
        "DIDev8_GetDeviceData",
        "Std0_2",
        "Std0_2",
        "Std0_3",
        "Std0_4",
        "Std0_2",
        "Std0_3",
        "Std0_4",
        "Std0_5",
        "Std0_4",
        "Std0_4",
        "Std0_3",
        "Std0_1",
        "Std0_1",
        "Std0_2",
        "Std0_1",
        "Std0_1",
        "Std0_4",
        "Std0_4",
        "Std0_4",
        "Std0_2",
        "Std0_4",
    ];
    let dev_obj = alloc_com_object(ctx, DINPUT8_COM_DLL, &DIDEV8_SLOTS, 0)?;
    ctx.arena.write_u32(pp_dev, dev_obj)?;
    return_stdcall(cpu, ctx.arena, 4, S_OK)?;
    Ok(())
}

fn didev8_get_device_state(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let cb_data = peek_arg_u32(cpu, ctx.arena, 1)?;
    let lpv_data = peek_arg_u32(cpu, ctx.arena, 2)?;
    if lpv_data != 0 && cb_data > 0 && cb_data <= 4096 {
        ctx.arena.zero_range(lpv_data, cb_data as usize)?;
    }
    return_stdcall(cpu, ctx.arena, 3, S_OK)?;
    Ok(())
}

fn didev8_get_device_data(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let pdw_inout = peek_arg_u32(cpu, ctx.arena, 3)?;
    if pdw_inout != 0 {
        ctx.arena.write_u32(pdw_inout, 0)?;
    }
    return_stdcall(cpu, ctx.arena, 5, S_OK)?;
    Ok(())
}

fn alloc_ds3d_listener(ctx: &mut RuntimeContext<'_>) -> Result<GuestAddr, RuntimeError> {
    const SLOTS: [&str; DS3DLISTENER_VTABLE_SLOTS] = [
        "QueryInterface",
        "AddRef",
        "Release",
        "Std0_2",
        "Std0_2",
        "Std0_2",
        "Std0_3",
        "Std0_2",
        "Std0_2",
        "Std0_2",
        "Std0_3",
        "Std0_3",
        "Std0_3",
        "Std0_8",
        "Std0_5",
        "Std0_3",
        "Std0_5",
        "Std0_1",
    ];
    alloc_com_object(ctx, DSOUND_COM_DLL, &SLOTS, 0)
}

fn alloc_ds3d_buffer(ctx: &mut RuntimeContext<'_>) -> Result<GuestAddr, RuntimeError> {
    const SLOTS: [&str; DS3DBUFFER_VTABLE_SLOTS] = [
        "QueryInterface",
        "AddRef",
        "Release",
        "Std0_2",
        "Std0_3",
        "Std0_2",
        "Std0_2",
        "Std0_2",
        "Std0_2",
        "Std0_2",
        "Std0_2",
        "Std0_2",
        "Std0_3",
        "Std0_4",
        "Std0_5",
        "Std0_3",
        "Std0_3",
        "Std0_3",
        "Std0_3",
        "Std0_5",
        "Std0_5",
    ];
    alloc_com_object(ctx, DSOUND_COM_DLL, &SLOTS, 0)
}

fn ds_query_interface(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let riid = peek_arg_u32(cpu, ctx.arena, 1)?;
    let ppv = peek_arg_u32(cpu, ctx.arena, 2)?;
    if ppv == 0 {
        return_stdcall(cpu, ctx.arena, 3, E_FAIL)?;
        return Ok(());
    }
    let data1 = if riid != 0 {
        ctx.arena.read_u32(riid).unwrap_or(0)
    } else {
        0
    };
    let result_obj = match data1 {
        IID_IUNKNOWN_DATA1 | IID_IDSDIRECTSOUNDBUFFER_DATA1 | IID_IDSDIRECTSOUNDBUFFER8_DATA1 => {
            Some(this_ptr)
        }
        IID_IDSDIRECTSOUND3DLISTENER_DATA1 => Some(alloc_ds3d_listener(ctx)?),
        IID_IDSDIRECTSOUND3DBUFFER_DATA1 => Some(alloc_ds3d_buffer(ctx)?),
        _ => None,
    };
    match result_obj {
        Some(obj) => {
            ctx.arena.write_u32(ppv, obj)?;
            return_stdcall(cpu, ctx.arena, 3, S_OK)?;
        }
        None => {
            ctx.arena.write_u32(ppv, 0)?;
            return_stdcall(cpu, ctx.arena, 3, E_NOINTERFACE)?;
        }
    }
    Ok(())
}

fn direct_sound_create_8(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let pp_ds = peek_arg_u32(cpu, ctx.arena, 1)?;
    if pp_ds == 0 {
        return_stdcall(cpu, ctx.arena, 3, E_FAIL)?;
        return Ok(());
    }
    const DS8_SLOTS: [&str; DS8_VTABLE_SLOTS] = [
        "QueryInterface",
        "AddRef",
        "Release",
        "DS_CreateSoundBuffer",
        "DS_GetCaps",
        "DS_DuplicateSoundBuffer",
        "Std0_3",
        "Std0_1",
        "DS_GetSpeakerConfig",
        "Std0_2",
        "Std0_2",
        "DS_VerifyCertification",
    ];
    let obj = alloc_com_object(ctx, DSOUND_COM_DLL, &DS8_SLOTS, 0)?;
    ctx.arena.write_u32(pp_ds, obj)?;
    return_stdcall(cpu, ctx.arena, 3, S_OK)?;
    Ok(())
}

fn alloc_ds_buffer_object(
    ctx: &mut RuntimeContext<'_>,
    buf_bytes: u32,
) -> Result<GuestAddr, RuntimeError> {
    const DSB8_SLOTS: [&str; DSB8_VTABLE_SLOTS] = [
        "QueryInterface",
        "AddRef",
        "Release",
        "DSB_GetCaps",
        "DSB_GetCurrentPosition",
        "Std0_4",
        "DSB_GetVolume",
        "DSB_GetPan",
        "DSB_GetFrequency",
        "DSB_GetStatus",
        "Std0_3",
        "DSB_Lock",
        "DSB_Play",
        "Std0_2",
        "Std0_2",
        "Std0_2",
        "Std0_2",
        "Std0_2",
        "Std0_1",
        "DSB_Unlock",
        "Std0_1",
        "Std0_4",
    ];
    let size = buf_bytes.clamp(4096, 4 * 1024 * 1024);
    let audio_buf = ctx.heap.alloc(ctx.arena, size as usize, true)?;
    let obj = alloc_com_object(ctx, DSOUND_COM_DLL, &DSB8_SLOTS, 12)?;
    ctx.arena.write_u32(obj + 4, audio_buf)?;
    ctx.arena.write_u32(obj + 8, size)?;
    ctx.arena.write_u32(obj + 12, 0)?;
    Ok(obj)
}

fn ds_create_sound_buffer(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let desc_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let pp_buf = peek_arg_u32(cpu, ctx.arena, 2)?;
    if pp_buf == 0 {
        return_stdcall(cpu, ctx.arena, 4, E_FAIL)?;
        return Ok(());
    }
    let req_bytes = if desc_ptr != 0 {
        let raw = ctx.arena.read_u32(desc_ptr + 8).unwrap_or(0);
        if raw == 0 { DS_DEFAULT_BUFFER_BYTES } else { raw }
    } else {
        DS_DEFAULT_BUFFER_BYTES
    };
    let obj = alloc_ds_buffer_object(ctx, req_bytes)?;
    ctx.arena.write_u32(pp_buf, obj)?;
    return_stdcall(cpu, ctx.arena, 4, S_OK)?;
    Ok(())
}

fn ds_duplicate_sound_buffer(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let orig = peek_arg_u32(cpu, ctx.arena, 1)?;
    let pp_dup = peek_arg_u32(cpu, ctx.arena, 2)?;
    if pp_dup == 0 {
        return_stdcall(cpu, ctx.arena, 3, E_FAIL)?;
        return Ok(());
    }
    let size = if orig != 0 {
        ctx.arena.read_u32(orig + 8).unwrap_or(DS_DEFAULT_BUFFER_BYTES)
    } else {
        DS_DEFAULT_BUFFER_BYTES
    };
    let obj = alloc_ds_buffer_object(ctx, size)?;
    ctx.arena.write_u32(pp_dup, obj)?;
    return_stdcall(cpu, ctx.arena, 3, S_OK)?;
    Ok(())
}

fn ds_get_caps(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let caps_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if caps_ptr != 0 {
        let dw_size = ctx.arena.read_u32(caps_ptr).unwrap_or(0);
        if dw_size > 4 && dw_size <= 256 {
            ctx.arena.zero_range(caps_ptr + 4, (dw_size - 4) as usize)?;
        }
        if dw_size >= 16 {
            ctx.arena.write_u32(caps_ptr + 4, 0x10)?;
            ctx.arena.write_u32(caps_ptr + 12, 1)?;
        }
    }
    return_stdcall(cpu, ctx.arena, 2, S_OK)?;
    Ok(())
}

fn ds_write_zero_2(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let out_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out_ptr != 0 {
        ctx.arena.write_u32(out_ptr, 0)?;
    }
    return_stdcall(cpu, ctx.arena, 2, S_OK)?;
    Ok(())
}

fn dsb_get_caps(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let caps_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if caps_ptr != 0 {
        let dw_size = ctx.arena.read_u32(caps_ptr).unwrap_or(0);
        if dw_size > 4 && dw_size <= 128 {
            ctx.arena.zero_range(caps_ptr + 4, (dw_size - 4) as usize)?;
        }
        if dw_size >= 12 {
            let buf_size = ctx.arena.read_u32(this_ptr + 8).unwrap_or(DS_DEFAULT_BUFFER_BYTES);
            ctx.arena.write_u32(caps_ptr + 8, buf_size)?;
        }
    }
    return_stdcall(cpu, ctx.arena, 2, S_OK)?;
    Ok(())
}

fn dsb_get_current_position(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let p_play = peek_arg_u32(cpu, ctx.arena, 1)?;
    let p_write = peek_arg_u32(cpu, ctx.arena, 2)?;
    if p_play != 0 {
        ctx.arena.write_u32(p_play, 0)?;
    }
    if p_write != 0 {
        ctx.arena.write_u32(p_write, 0)?;
    }
    return_stdcall(cpu, ctx.arena, 3, S_OK)?;
    Ok(())
}

fn dsb_get_frequency(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let p_freq = peek_arg_u32(cpu, ctx.arena, 1)?;
    if p_freq != 0 {
        ctx.arena.write_u32(p_freq, 44_100)?;
    }
    return_stdcall(cpu, ctx.arena, 2, S_OK)?;
    Ok(())
}

fn dsb_get_status(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let p_status = peek_arg_u32(cpu, ctx.arena, 1)?;
    if p_status != 0 {
        let status = ctx.arena.read_u32(this_ptr + 12).unwrap_or(0);
        ctx.arena.write_u32(p_status, status)?;
    }
    return_stdcall(cpu, ctx.arena, 2, S_OK)?;
    Ok(())
}

fn dsb_lock(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let offset = peek_arg_u32(cpu, ctx.arena, 1)?;
    let bytes = peek_arg_u32(cpu, ctx.arena, 2)?;
    let pp1 = peek_arg_u32(cpu, ctx.arena, 3)?;
    let pb1 = peek_arg_u32(cpu, ctx.arena, 4)?;
    let pp2 = peek_arg_u32(cpu, ctx.arena, 5)?;
    let pb2 = peek_arg_u32(cpu, ctx.arena, 6)?;

    let buf_ptr = ctx.arena.read_u32(this_ptr + 4).unwrap_or(0);
    let buf_size = ctx.arena.read_u32(this_ptr + 8).unwrap_or(DS_DEFAULT_BUFFER_BYTES).max(1);
    let start = offset % buf_size;
    let total = bytes.min(buf_size);
    let len1 = total.min(buf_size - start);
    let len2 = total.saturating_sub(len1);

    if pp1 != 0 {
        ctx.arena.write_u32(pp1, buf_ptr + start)?;
    }
    if pb1 != 0 {
        ctx.arena.write_u32(pb1, len1)?;
    }
    if pp2 != 0 {
        ctx.arena.write_u32(pp2, if len2 > 0 { buf_ptr } else { 0 })?;
    }
    if pb2 != 0 {
        ctx.arena.write_u32(pb2, len2)?;
    }
    return_stdcall(cpu, ctx.arena, 8, S_OK)?;
    Ok(())
}

fn dsb_play(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let flags = peek_arg_u32(cpu, ctx.arena, 3)?;
    let status = if (flags & 1) != 0 { 5 } else { 1 };
    ctx.arena.write_u32(this_ptr + 12, status)?;
    return_stdcall(cpu, ctx.arena, 4, S_OK)?;
    Ok(())
}

fn dsb_stop(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.arena.write_u32(this_ptr + 12, 0)?;
    return_stdcall(cpu, ctx.arena, 1, S_OK)?;
    Ok(())
}

fn stdcall_one_1(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn stdcall_zero_1(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn stdcall_zero_2(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, 0)?;
    Ok(())
}

fn stdcall_zero_3(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 3, 0)?;
    Ok(())
}

fn stdcall_zero_4(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 4, 0)?;
    Ok(())
}

fn stdcall_zero_5(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 5, 0)?;
    Ok(())
}

fn stdcall_zero_7(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 7, 0)?;
    Ok(())
}

fn stdcall_zero_8(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 8, 0)?;
    Ok(())
}
