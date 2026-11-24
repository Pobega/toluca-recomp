use std::cmp::Ordering;
use std::fs::OpenOptions;
use std::io::SeekFrom;

use toluca_core::GuestAddr;
use toluca_core::cpu::{ExitReason, Gpr, GuestCpuState};

use crate::arena::{ArenaError, GuestArena};
use crate::callconv::{peek_arg_i32, peek_arg_u32, return_cdecl, return_cdecl_u64, return_stdcall};
use crate::runtime::{ImportHandler, ImportRegistry, RuntimeContext, RuntimeError};
use crate::win32::{
    CFileEntry, MSVCRT_IOBUF_FLAG_OFF, MSVCRT_IOBUF_SIZE, MSVCRT_IOEOF, MSVCRT_IOERR,
    MSVCRT_IOREAD, MSVCRT_IORW, MSVCRT_IOWRT,
};

const MAX_STR_SCAN: usize = 65536;
const SEEK_SET: u32 = 0;
const SEEK_CUR: u32 = 1;
const SEEK_END: u32 = 2;
const RAND_MAX_MASK: u32 = 0x7FFF;

pub fn register_imports(registry: &mut ImportRegistry) {
    const CRT_DLLS: &[&str] = &["msvcrt.dll", "msvcr70.dll"];
    let entries: &[(&str, ImportHandler)] = &[
        ("malloc", c_malloc),
        ("calloc", c_calloc),
        ("realloc", c_realloc),
        ("free", c_free),
        ("memcpy", c_memcpy),
        ("memmove", c_memmove),
        ("memset", c_memset),
        ("memcmp", c_memcmp),
        ("??2@YAPAXI@Z", c_malloc),
        ("??3@YAXPAX@Z", c_free),
        ("strlen", c_strlen),
        ("strcpy", c_strcpy),
        ("strncpy", c_strncpy),
        ("strcmp", c_strcmp),
        ("strncmp", c_strncmp),
        ("_stricmp", c_stricmp),
        ("_strnicmp", c_strnicmp),
        ("strcat", c_strcat),
        ("strncat", c_strncat),
        ("strchr", c_strchr),
        ("strrchr", c_strrchr),
        ("strstr", c_strstr),
        ("atoi", c_atoi),
        ("atol", c_atoi),
        ("strtol", c_strtol),
        ("strtoul", c_strtoul),
        ("sprintf", c_sprintf),
        ("_snprintf", c_snprintf),
        ("vsprintf", c_vsprintf),
        ("_vsnprintf", c_vsnprintf),
        ("sscanf", c_sscanf),
        ("fopen", c_fopen),
        ("fread", c_fread),
        ("fwrite", c_fwrite),
        ("fseek", c_fseek),
        ("ftell", c_ftell),
        ("feof", c_feof),
        ("ferror", c_ferror),
        ("clearerr", c_clearerr),
        ("rewind", c_rewind),
        ("fgetc", c_fgetc),
        ("getc", c_fgetc),
        ("fflush", c_fflush),
        ("fclose", c_fclose),
        ("fgets", c_fgets),
        ("sin", c_sin),
        ("cos", c_cos),
        ("tan", c_tan),
        ("asin", c_asin),
        ("acos", c_acos),
        ("atan", c_atan),
        ("atan2", c_atan2),
        ("sqrt", c_sqrt),
        ("pow", c_pow),
        ("exp", c_exp),
        ("log", c_log),
        ("log10", c_log10),
        ("floor", c_floor),
        ("ceil", c_ceil),
        ("fabs", c_fabs),
        ("fmod", c_fmod),
        ("_ftol", c_ftol),
        ("_setjmp3", c_setjmp),
        ("setjmp", c_setjmp),
        ("longjmp", c_longjmp),
        ("_initterm", c_initterm),
        ("__set_app_type", c_stub_void_1),
        ("_controlfp", c_controlfp),
        ("atexit", c_atexit),
        ("exit", c_exit),
        ("_exit", c_exit),
        ("srand", c_srand),
        ("rand", c_rand),
        ("clock", c_clock),
        ("time", c_time),
        ("qsort", c_qsort),
        ("__p__fmode", c_p_fmode),
        ("__p__commode", c_p_commode),
        ("__getmainargs", c_getmainargs),
        ("__setusermatherr", c_stub_void_1),
        ("_onexit", c_onexit),
        ("__dllonexit", c_dllonexit),
        ("_cexit", c_stub_void_0),
        ("_c_exit", c_stub_void_0),
        ("_amsg_exit", c_exit),
        ("_XcptFilter", c_stub_void_0),
        ("__security_error_handler", c_exit),
        ("_except_handler3", c_stub_one),
        ("__CxxFrameHandler", c_stub_one),
        ("?terminate@@YAXXZ", c_terminate),
        ("_CxxThrowException", c_cxx_throw_exception),
        ("??_U@YAPAXI@Z", c_malloc),
        ("??_V@YAXPAX@Z", c_free),
        ("_aligned_malloc", c_aligned_malloc),
        ("_aligned_free", c_aligned_free),
        ("wcslen", c_wcslen),
        ("isspace", c_isspace),
        ("isdigit", c_isdigit),
        ("iswascii", c_iswascii),
        ("_finite", c_finite),
        ("puts", c_puts),
        ("fputs", c_fputs),
        ("fprintf", c_fprintf),
        ("printf", c_printf),
        ("strtok", c_strtok),
        ("_beginthread", c_beginthread),
        ("??0exception@@QAE@XZ", c_exception_ctor_default),
        ("??0exception@@QAE@ABV0@@Z", c_exception_ctor_copy),
        ("??1exception@@UAE@XZ", c_exception_dtor),
        ("??1type_info@@UAE@XZ", c_exception_dtor),
        ("_CIasin", c_ci_asin),
        ("_CIacos", c_ci_acos),
        ("_CIpow", c_ci_pow),
        ("_CIfmod", c_ci_fmod),
    ];

    for &dll in CRT_DLLS {
        for &(name, handler) in entries {
            registry.register_name(dll, name, handler);
        }
    }
}

fn c_malloc(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let size = peek_arg_u32(cpu, ctx.arena, 0)? as usize;
    let ptr = ctx.heap.alloc(ctx.arena, size, false).unwrap_or(0);
    return_cdecl(cpu, ctx.arena, ptr)?;
    Ok(())
}

fn c_calloc(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let count = peek_arg_u32(cpu, ctx.arena, 0)? as usize;
    let elem_size = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let ptr = count
        .checked_mul(elem_size)
        .and_then(|total| ctx.heap.alloc(ctx.arena, total, true).ok())
        .unwrap_or(0);
    return_cdecl(cpu, ctx.arena, ptr)?;
    Ok(())
}

fn c_realloc(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let size = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let new_ptr = ctx.heap.realloc(ctx.arena, ptr, size, false).unwrap_or(0);
    return_cdecl(cpu, ctx.arena, new_ptr)?;
    Ok(())
}

fn c_free(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    if ptr != 0 {
        let _ = ctx.heap.free(ctx.arena, ptr);
    }
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_memcpy(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let dst = peek_arg_u32(cpu, ctx.arena, 0)?;
    let src = peek_arg_u32(cpu, ctx.arena, 1)?;
    let len = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    ctx.arena.copy_bytes(dst, src, len)?;
    return_cdecl(cpu, ctx.arena, dst)?;
    Ok(())
}

fn c_memmove(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    c_memcpy(ctx, cpu)
}

fn c_memset(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let dst = peek_arg_u32(cpu, ctx.arena, 0)?;
    let val = peek_arg_u32(cpu, ctx.arena, 1)? as u8;
    let len = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    if val == 0 {
        ctx.arena.zero_range(dst, len)?;
    } else {
        let buf = vec![val; len];
        ctx.arena.write_bytes(dst, &buf)?;
    }
    return_cdecl(cpu, ctx.arena, dst)?;
    Ok(())
}

fn c_memcmp(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let lhs = peek_arg_u32(cpu, ctx.arena, 0)?;
    let rhs = peek_arg_u32(cpu, ctx.arena, 1)?;
    let len = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    let a = ctx.arena.read_bytes(lhs, len)?;
    let b = ctx.arena.read_bytes(rhs, len)?;
    let res = match a.cmp(b) {
        Ordering::Less => -1i32 as u32,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    };
    return_cdecl(cpu, ctx.arena, res)?;
    Ok(())
}

fn read_cstr_bytes(arena: &GuestArena, addr: GuestAddr) -> Result<Vec<u8>, ArenaError> {
    let s = arena.read_cstr(addr, MAX_STR_SCAN)?;
    Ok(s.into_bytes())
}

fn c_strlen(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let s_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let bytes = read_cstr_bytes(ctx.arena, s_ptr)?;
    return_cdecl(cpu, ctx.arena, bytes.len() as u32)?;
    Ok(())
}

fn c_strcpy(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let dst = peek_arg_u32(cpu, ctx.arena, 0)?;
    let src = peek_arg_u32(cpu, ctx.arena, 1)?;
    let mut bytes = read_cstr_bytes(ctx.arena, src)?;
    bytes.push(0);
    ctx.arena.write_bytes(dst, &bytes)?;
    return_cdecl(cpu, ctx.arena, dst)?;
    Ok(())
}

fn c_strncpy(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let dst = peek_arg_u32(cpu, ctx.arena, 0)?;
    let src = peek_arg_u32(cpu, ctx.arena, 1)?;
    let count = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    let src_bytes = read_cstr_bytes(ctx.arena, src)?;
    let mut out = vec![0u8; count];
    let copy_len = src_bytes.len().min(count);
    out[..copy_len].copy_from_slice(&src_bytes[..copy_len]);
    ctx.arena.write_bytes(dst, &out)?;
    return_cdecl(cpu, ctx.arena, dst)?;
    Ok(())
}

fn compare_cstr_bytes(a: &[u8], b: &[u8], case_insensitive: bool) -> u32 {
    let len = a.len().min(b.len());
    for i in 0..len {
        let ca = if case_insensitive {
            a[i].to_ascii_lowercase()
        } else {
            a[i]
        };
        let cb = if case_insensitive {
            b[i].to_ascii_lowercase()
        } else {
            b[i]
        };
        if ca != cb {
            return (ca as i32 - cb as i32) as u32;
        }
    }
    (a.len() as i32 - b.len() as i32).signum() as u32
}

fn c_strcmp(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let a = read_cstr_bytes(ctx.arena, peek_arg_u32(cpu, ctx.arena, 0)?)?;
    let b = read_cstr_bytes(ctx.arena, peek_arg_u32(cpu, ctx.arena, 1)?)?;
    return_cdecl(cpu, ctx.arena, compare_cstr_bytes(&a, &b, false))?;
    Ok(())
}

fn c_strncmp(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let mut a = read_cstr_bytes(ctx.arena, peek_arg_u32(cpu, ctx.arena, 0)?)?;
    let mut b = read_cstr_bytes(ctx.arena, peek_arg_u32(cpu, ctx.arena, 1)?)?;
    let n = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    a.truncate(n);
    b.truncate(n);
    return_cdecl(cpu, ctx.arena, compare_cstr_bytes(&a, &b, false))?;
    Ok(())
}

fn c_stricmp(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let a = read_cstr_bytes(ctx.arena, peek_arg_u32(cpu, ctx.arena, 0)?)?;
    let b = read_cstr_bytes(ctx.arena, peek_arg_u32(cpu, ctx.arena, 1)?)?;
    return_cdecl(cpu, ctx.arena, compare_cstr_bytes(&a, &b, true))?;
    Ok(())
}

fn c_strnicmp(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let mut a = read_cstr_bytes(ctx.arena, peek_arg_u32(cpu, ctx.arena, 0)?)?;
    let mut b = read_cstr_bytes(ctx.arena, peek_arg_u32(cpu, ctx.arena, 1)?)?;
    let n = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    a.truncate(n);
    b.truncate(n);
    return_cdecl(cpu, ctx.arena, compare_cstr_bytes(&a, &b, true))?;
    Ok(())
}

fn c_strcat(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let dst = peek_arg_u32(cpu, ctx.arena, 0)?;
    let src = peek_arg_u32(cpu, ctx.arena, 1)?;
    let dst_len = read_cstr_bytes(ctx.arena, dst)?.len() as u32;
    let mut src_bytes = read_cstr_bytes(ctx.arena, src)?;
    src_bytes.push(0);
    ctx.arena.write_bytes(dst + dst_len, &src_bytes)?;
    return_cdecl(cpu, ctx.arena, dst)?;
    Ok(())
}

fn c_strncat(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let dst = peek_arg_u32(cpu, ctx.arena, 0)?;
    let src = peek_arg_u32(cpu, ctx.arena, 1)?;
    let max_n = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    let dst_len = read_cstr_bytes(ctx.arena, dst)?.len() as u32;
    let mut src_bytes = read_cstr_bytes(ctx.arena, src)?;
    src_bytes.truncate(max_n);
    src_bytes.push(0);
    ctx.arena.write_bytes(dst + dst_len, &src_bytes)?;
    return_cdecl(cpu, ctx.arena, dst)?;
    Ok(())
}

fn c_strchr(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let ch = (peek_arg_u32(cpu, ctx.arena, 1)? & 0xFF) as u8;
    let bytes = read_cstr_bytes(ctx.arena, ptr)?;
    let found = if ch == 0 {
        Some(ptr + bytes.len() as u32)
    } else {
        bytes.iter().position(|&b| b == ch).map(|i| ptr + i as u32)
    };
    return_cdecl(cpu, ctx.arena, found.unwrap_or(0))?;
    Ok(())
}

fn c_strrchr(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let ch = (peek_arg_u32(cpu, ctx.arena, 1)? & 0xFF) as u8;
    let bytes = read_cstr_bytes(ctx.arena, ptr)?;
    let found = if ch == 0 {
        Some(ptr + bytes.len() as u32)
    } else {
        bytes.iter().rposition(|&b| b == ch).map(|i| ptr + i as u32)
    };
    return_cdecl(cpu, ctx.arena, found.unwrap_or(0))?;
    Ok(())
}

fn c_strstr(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let hay_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let needle_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let hay = read_cstr_bytes(ctx.arena, hay_ptr)?;
    let needle = read_cstr_bytes(ctx.arena, needle_ptr)?;
    let found = if needle.is_empty() {
        Some(hay_ptr)
    } else {
        hay.windows(needle.len())
            .position(|w| w == needle.as_slice())
            .map(|i| hay_ptr + i as u32)
    };
    return_cdecl(cpu, ctx.arena, found.unwrap_or(0))?;
    Ok(())
}

fn c_atoi(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let text = ctx.arena.read_cstr(ptr, MAX_STR_SCAN)?;
    let (val, _) = parse_c_int(&text, 10);
    return_cdecl(cpu, ctx.arena, val as u32)?;
    Ok(())
}

fn c_strtol(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let end_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let base = peek_arg_i32(cpu, ctx.arena, 2)?;
    let text = ctx.arena.read_cstr(ptr, MAX_STR_SCAN)?;
    let (val, consumed) = parse_c_int(&text, base);
    if end_ptr != 0 {
        ctx.arena.write_u32(end_ptr, ptr + consumed as u32)?;
    }
    return_cdecl(cpu, ctx.arena, val as u32)?;
    Ok(())
}

fn c_strtoul(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let end_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let base = peek_arg_i32(cpu, ctx.arena, 2)?;
    let text = ctx.arena.read_cstr(ptr, MAX_STR_SCAN)?;
    let (val, consumed) = parse_c_int(&text, base);
    if end_ptr != 0 {
        ctx.arena.write_u32(end_ptr, ptr + consumed as u32)?;
    }
    return_cdecl(cpu, ctx.arena, val as u32)?;
    Ok(())
}

fn parse_c_int(text: &str, requested_base: i32) -> (i64, usize) {
    let bytes = text.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }

    let mut neg = false;
    if idx < bytes.len() {
        if bytes[idx] == b'-' {
            neg = true;
            idx += 1;
        } else if bytes[idx] == b'+' {
            idx += 1;
        }
    }

    let mut base = requested_base;
    if base == 0 {
        if idx + 1 < bytes.len() && bytes[idx] == b'0' && matches!(bytes[idx + 1], b'x' | b'X') {
            base = 16;
            idx += 2;
        } else if idx < bytes.len() && bytes[idx] == b'0' {
            base = 8;
        } else {
            base = 10;
        }
    } else if base == 16
        && idx + 1 < bytes.len()
        && bytes[idx] == b'0'
        && matches!(bytes[idx + 1], b'x' | b'X')
    {
        idx += 2;
    }

    if !(2..=36).contains(&base) {
        return (0, 0);
    }

    let start_digits = idx;
    let mut acc: u64 = 0;
    while idx < bytes.len() {
        let Some(digit) = (bytes[idx] as char).to_digit(base as u32) else {
            break;
        };
        acc = acc.wrapping_mul(base as u64).wrapping_add(digit as u64);
        idx += 1;
    }

    if idx == start_digits {
        return (0, 0);
    }

    let signed_val = if neg {
        (acc as i64).wrapping_neg()
    } else {
        acc as i64
    };
    (signed_val, idx)
}

fn c_sprintf(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let fmt_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let fmt_str = ctx.arena.read_cstr(fmt_ptr, MAX_STR_SCAN)?;
    let arg_base = cpu.gpr(Gpr::Esp) + 12;

    let formatted = format_printf(ctx.arena, &fmt_str, arg_base)?;
    ctx.arena.write_cstr(buf_ptr, &formatted)?;
    return_cdecl(cpu, ctx.arena, formatted.len() as u32)?;
    Ok(())
}

fn c_snprintf(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let count = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let fmt_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let fmt_str = ctx.arena.read_cstr(fmt_ptr, MAX_STR_SCAN)?;
    let arg_base = cpu.gpr(Gpr::Esp) + 16;

    let formatted = format_printf(ctx.arena, &fmt_str, arg_base)?;
    let ret = write_bounded_printf(ctx.arena, buf_ptr, count, &formatted)?;
    return_cdecl(cpu, ctx.arena, ret)?;
    Ok(())
}

fn c_vsprintf(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let fmt_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let va_list = peek_arg_u32(cpu, ctx.arena, 2)?;
    let fmt_str = ctx.arena.read_cstr(fmt_ptr, MAX_STR_SCAN)?;

    let formatted = format_printf(ctx.arena, &fmt_str, va_list)?;
    ctx.arena.write_cstr(buf_ptr, &formatted)?;
    return_cdecl(cpu, ctx.arena, formatted.len() as u32)?;
    Ok(())
}

fn c_vsnprintf(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let count = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let fmt_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let va_list = peek_arg_u32(cpu, ctx.arena, 3)?;
    let fmt_str = ctx.arena.read_cstr(fmt_ptr, MAX_STR_SCAN)?;

    let formatted = format_printf(ctx.arena, &fmt_str, va_list)?;
    let ret = write_bounded_printf(ctx.arena, buf_ptr, count, &formatted)?;
    return_cdecl(cpu, ctx.arena, ret)?;
    Ok(())
}

fn write_bounded_printf(
    arena: &mut GuestArena,
    buf_ptr: GuestAddr,
    count: usize,
    formatted: &str,
) -> Result<u32, ArenaError> {
    let bytes = formatted.as_bytes();
    if count == 0 {
        return Ok(-1i32 as u32);
    }
    if bytes.len() < count {
        arena.write_cstr(buf_ptr, formatted)?;
        Ok(bytes.len() as u32)
    } else if bytes.len() == count {
        arena.write_bytes(buf_ptr, bytes)?;
        Ok(count as u32)
    } else {
        arena.write_bytes(buf_ptr, &bytes[..count])?;
        Ok(-1i32 as u32)
    }
}

struct PrintfCursor<'a> {
    arena: &'a GuestArena,
    addr: GuestAddr,
}

impl<'a> PrintfCursor<'a> {
    fn next_u32(&mut self) -> Result<u32, ArenaError> {
        let val = self.arena.read_u32(self.addr)?;
        self.addr += 4;
        Ok(val)
    }

    fn next_f64(&mut self) -> Result<f64, ArenaError> {
        let bits = self.arena.read_u64(self.addr)?;
        self.addr += 8;
        Ok(f64::from_bits(bits))
    }
}

pub(crate) fn format_printf(
    arena: &GuestArena,
    fmt: &str,
    arg_base: GuestAddr,
) -> Result<String, ArenaError> {
    let mut cursor = PrintfCursor {
        arena,
        addr: arg_base,
    };
    let bytes = fmt.as_bytes();
    let mut out = String::new();
    let mut idx = 0usize;

    while idx < bytes.len() {
        if bytes[idx] != b'%' {
            out.push(bytes[idx] as char);
            idx += 1;
            continue;
        }
        idx += 1;
        if idx < bytes.len() && bytes[idx] == b'%' {
            out.push('%');
            idx += 1;
            continue;
        }

        let mut left_align = false;
        let mut zero_pad = false;
        let mut plus_sign = false;
        let mut space_sign = false;
        while idx < bytes.len() {
            match bytes[idx] {
                b'-' => left_align = true,
                b'0' => zero_pad = true,
                b'+' => plus_sign = true,
                b' ' => space_sign = true,
                b'#' => {}
                _ => break,
            }
            idx += 1;
        }

        let width = if idx < bytes.len() && bytes[idx] == b'*' {
            idx += 1;
            let w = cursor.next_u32()? as i32;
            if w < 0 {
                left_align = true;
                (-w) as usize
            } else {
                w as usize
            }
        } else {
            parse_decimal_digits(bytes, &mut idx)
        };

        let precision = if idx < bytes.len() && bytes[idx] == b'.' {
            idx += 1;
            if idx < bytes.len() && bytes[idx] == b'*' {
                idx += 1;
                let p = cursor.next_u32()? as i32;
                (p >= 0).then_some(p as usize)
            } else {
                Some(parse_decimal_digits(bytes, &mut idx))
            }
        } else {
            None
        };

        while idx < bytes.len() && matches!(bytes[idx], b'l' | b'h' | b'z') {
            idx += 1;
        }

        if idx >= bytes.len() {
            break;
        }

        let spec = bytes[idx];
        idx += 1;
        if left_align {
            zero_pad = false;
        }

        let piece = match spec {
            b'd' | b'i' => {
                let v = cursor.next_u32()? as i32;
                let sign = if v < 0 {
                    "-"
                } else if plus_sign {
                    "+"
                } else if space_sign {
                    " "
                } else {
                    ""
                };
                let mag = (v as i64).unsigned_abs().to_string();
                pad_signed_number(sign, &mag, width, precision, zero_pad, left_align)
            }
            b'u' => {
                let v = cursor.next_u32()?;
                pad_signed_number("", &v.to_string(), width, precision, zero_pad, left_align)
            }
            b'x' => {
                let v = cursor.next_u32()?;
                pad_signed_number("", &format!("{v:x}"), width, precision, zero_pad, left_align)
            }
            b'X' => {
                let v = cursor.next_u32()?;
                pad_signed_number("", &format!("{v:X}"), width, precision, zero_pad, left_align)
            }
            b'p' => {
                let v = cursor.next_u32()?;
                pad_signed_number("", &format!("{v:08X}"), width, None, zero_pad, left_align)
            }
            b'c' => {
                let ch = (cursor.next_u32()? & 0xFF) as u8 as char;
                pad_text(&ch.to_string(), width, left_align)
            }
            b's' => {
                let ptr = cursor.next_u32()?;
                let s = if ptr == 0 {
                    "(null)".to_owned()
                } else {
                    arena.read_cstr(ptr, MAX_STR_SCAN)?
                };
                let sliced = match precision {
                    Some(max_len) => s.chars().take(max_len).collect::<String>(),
                    None => s,
                };
                pad_text(&sliced, width, left_align)
            }
            b'f' | b'g' | b'G' => {
                let v = cursor.next_f64()?;
                let prec = precision.unwrap_or(6);
                let formatted = format!("{:.*}", prec, v);
                pad_text(&formatted, width, left_align)
            }
            other => format!("%{}", other as char),
        };
        out.push_str(&piece);
    }

    Ok(out)
}

fn parse_decimal_digits(bytes: &[u8], idx: &mut usize) -> usize {
    let mut val = 0usize;
    while *idx < bytes.len() && bytes[*idx].is_ascii_digit() {
        val = val * 10 + (bytes[*idx] - b'0') as usize;
        *idx += 1;
    }
    val
}

fn pad_signed_number(
    sign: &str,
    digits: &str,
    width: usize,
    precision: Option<usize>,
    zero_pad: bool,
    left_align: bool,
) -> String {
    let min_digits = precision.unwrap_or(1);
    let digit_pad = min_digits.saturating_sub(digits.len());
    let mut body = String::with_capacity(sign.len() + digit_pad + digits.len());
    body.push_str(sign);
    for _ in 0..digit_pad {
        body.push('0');
    }
    body.push_str(digits);

    if body.len() >= width {
        return body;
    }

    let pad_len = width - body.len();
    if left_align {
        let mut res = body;
        for _ in 0..pad_len {
            res.push(' ');
        }
        res
    } else if zero_pad && precision.is_none() {
        let mut res = String::with_capacity(width);
        res.push_str(sign);
        for _ in 0..pad_len {
            res.push('0');
        }
        res.push_str(&body[sign.len()..]);
        res
    } else {
        let mut res = String::with_capacity(width);
        for _ in 0..pad_len {
            res.push(' ');
        }
        res.push_str(&body);
        res
    }
}

fn pad_text(text: &str, width: usize, left_align: bool) -> String {
    if text.len() >= width {
        return text.to_owned();
    }
    let pad = " ".repeat(width - text.len());
    if left_align {
        format!("{text}{pad}")
    } else {
        format!("{pad}{text}")
    }
}

fn c_sscanf(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let src_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let fmt_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let input = ctx.arena.read_cstr(src_ptr, MAX_STR_SCAN)?;
    let fmt = ctx.arena.read_cstr(fmt_ptr, MAX_STR_SCAN)?;

    let assigned = run_sscanf(ctx.arena, &input, &fmt, cpu.gpr(Gpr::Esp) + 12)?;
    return_cdecl(cpu, ctx.arena, assigned)?;
    Ok(())
}

fn run_sscanf(
    arena: &mut GuestArena,
    input: &str,
    fmt: &str,
    mut arg_addr: GuestAddr,
) -> Result<u32, ArenaError> {
    let in_bytes = input.as_bytes();
    let fmt_bytes = fmt.as_bytes();
    let mut ii = 0usize;
    let mut fi = 0usize;
    let mut count = 0u32;

    while fi < fmt_bytes.len() {
        if fmt_bytes[fi].is_ascii_whitespace() {
            while fi < fmt_bytes.len() && fmt_bytes[fi].is_ascii_whitespace() {
                fi += 1;
            }
            while ii < in_bytes.len() && in_bytes[ii].is_ascii_whitespace() {
                ii += 1;
            }
            continue;
        }
        if fmt_bytes[fi] != b'%' {
            if ii >= in_bytes.len() || in_bytes[ii] != fmt_bytes[fi] {
                break;
            }
            ii += 1;
            fi += 1;
            continue;
        }

        fi += 1;
        if fi < fmt_bytes.len() && fmt_bytes[fi] == b'%' {
            if ii >= in_bytes.len() || in_bytes[ii] != b'%' {
                break;
            }
            ii += 1;
            fi += 1;
            continue;
        }

        while fi < fmt_bytes.len() && matches!(fmt_bytes[fi], b'l' | b'h') {
            fi += 1;
        }
        if fi >= fmt_bytes.len() {
            break;
        }

        let spec = fmt_bytes[fi];
        fi += 1;
        let out_ptr = arena.read_u32(arg_addr)?;
        arg_addr += 4;

        if spec != b'c' {
            while ii < in_bytes.len() && in_bytes[ii].is_ascii_whitespace() {
                ii += 1;
            }
        }
        if ii >= in_bytes.len() {
            break;
        }

        match spec {
            b'd' | b'i' | b'u' | b'x' | b'X' => {
                let base = match spec {
                    b'x' | b'X' => 16,
                    b'i' => 0,
                    _ => 10,
                };
                let (val, used) = parse_c_int(&input[ii..], base);
                if used == 0 {
                    break;
                }
                arena.write_u32(out_ptr, val as u32)?;
                ii += used;
                count += 1;
            }
            b's' => {
                let start = ii;
                while ii < in_bytes.len() && !in_bytes[ii].is_ascii_whitespace() {
                    ii += 1;
                }
                if ii == start {
                    break;
                }
                arena.write_cstr(out_ptr, &input[start..ii])?;
                count += 1;
            }
            b'c' => {
                arena.write_u8(out_ptr, in_bytes[ii])?;
                ii += 1;
                count += 1;
            }
            b'f' => {
                let start = ii;
                while ii < in_bytes.len()
                    && (in_bytes[ii].is_ascii_digit()
                        || matches!(in_bytes[ii], b'.' | b'+' | b'-' | b'e' | b'E'))
                {
                    ii += 1;
                }
                let Ok(val) = input[start..ii].parse::<f32>() else {
                    break;
                };
                arena.write_u32(out_ptr, val.to_bits())?;
                count += 1;
            }
            _ => break,
        }
    }

    Ok(count)
}

fn sync_cfile_flags(
    arena: &mut GuestArena,
    stream: GuestAddr,
    base_flag: u32,
    eof: bool,
    err: bool,
) -> Result<(), RuntimeError> {
    let flags = base_flag
        | if eof { MSVCRT_IOEOF } else { 0 }
        | if err { MSVCRT_IOERR } else { 0 };
    arena.write_u32(stream + MSVCRT_IOBUF_FLAG_OFF, flags)?;
    Ok(())
}

fn c_fopen(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let path_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let mode_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let win_path = ctx.arena.read_cstr(path_ptr, MAX_STR_SCAN)?;
    let mode = ctx.arena.read_cstr(mode_ptr, 16)?;
    let host_path = ctx.win32.resolve_path(&win_path);

    let plus = mode.contains('+');
    let mut opts = OpenOptions::new();
    let base_flag = match mode.bytes().next() {
        Some(b'r') => {
            opts.read(true).write(plus);
            if plus { MSVCRT_IORW } else { MSVCRT_IOREAD }
        }
        Some(b'w') => {
            opts.read(plus).write(true).create(true).truncate(true);
            if plus { MSVCRT_IORW } else { MSVCRT_IOWRT }
        }
        Some(b'a') => {
            opts.read(plus).append(true).create(true);
            if plus { MSVCRT_IORW } else { MSVCRT_IOWRT }
        }
        _ => {
            return_cdecl(cpu, ctx.arena, 0)?;
            return Ok(());
        }
    };

    let fp = match opts.open(&host_path) {
        Ok(file) => {
            let iobuf_va = ctx.heap.alloc(ctx.arena, MSVCRT_IOBUF_SIZE, true)?;
            ctx.arena.write_u32(iobuf_va + MSVCRT_IOBUF_FLAG_OFF, base_flag)?;
            ctx.win32
                .insert_cfile(iobuf_va, CFileEntry::new(file, base_flag));
            iobuf_va
        }
        Err(_) => 0,
    };
    return_cdecl(cpu, ctx.arena, fp)?;
    Ok(())
}

fn c_fread(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let item_size = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let count = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    let stream = peek_arg_u32(cpu, ctx.arena, 3)?;

    let total_bytes = item_size.saturating_mul(count);
    if item_size == 0 || total_bytes == 0 {
        return_cdecl(cpu, ctx.arena, 0)?;
        return Ok(());
    }

    let (filled, base_flag, eof, err) = {
        let Some(entry) = ctx.win32.get_cfile_mut(stream) else {
            return_cdecl(cpu, ctx.arena, 0)?;
            return Ok(());
        };
        let dst = ctx.arena.write_slice_mut(buf_ptr, total_bytes)?;
        let filled = entry.read_up_to(dst);
        (filled, entry.base_flag, entry.eof, entry.err)
    };

    sync_cfile_flags(ctx.arena, stream, base_flag, eof, err)?;
    return_cdecl(cpu, ctx.arena, (filled / item_size) as u32)?;
    Ok(())
}

fn c_fwrite(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let item_size = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let count = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    let stream = peek_arg_u32(cpu, ctx.arena, 3)?;

    let total_bytes = item_size.saturating_mul(count);
    if item_size == 0 || total_bytes == 0 {
        return_cdecl(cpu, ctx.arena, 0)?;
        return Ok(());
    }

    let (written, base_flag, eof, err) = {
        let data = ctx.arena.read_bytes(buf_ptr, total_bytes)?;
        let Some(entry) = ctx.win32.get_cfile_mut(stream) else {
            return_cdecl(cpu, ctx.arena, 0)?;
            return Ok(());
        };
        let written = if entry.write_bytes(data) {
            count as u32
        } else {
            0
        };
        (written, entry.base_flag, entry.eof, entry.err)
    };

    sync_cfile_flags(ctx.arena, stream, base_flag, eof, err)?;
    return_cdecl(cpu, ctx.arena, written)?;
    Ok(())
}

fn c_fseek(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let stream = peek_arg_u32(cpu, ctx.arena, 0)?;
    let offset = peek_arg_i32(cpu, ctx.arena, 1)? as i64;
    let origin = peek_arg_u32(cpu, ctx.arena, 2)?;

    let seek_from = match origin {
        SEEK_SET => SeekFrom::Start(offset as u64),
        SEEK_CUR => SeekFrom::Current(offset),
        SEEK_END => SeekFrom::End(offset),
        _ => {
            return_cdecl(cpu, ctx.arena, -1i32 as u32)?;
            return Ok(());
        }
    };

    let (res, flags_to_sync) = match ctx.win32.get_cfile_mut(stream) {
        Some(entry) => {
            let ok = entry.seek(seek_from);
            (
                if ok { 0 } else { -1i32 as u32 },
                Some((entry.base_flag, entry.eof, entry.err)),
            )
        }
        None => (-1i32 as u32, None),
    };
    if let Some((base_flag, eof, err)) = flags_to_sync {
        sync_cfile_flags(ctx.arena, stream, base_flag, eof, err)?;
    }
    return_cdecl(cpu, ctx.arena, res)?;
    Ok(())
}

fn c_ftell(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let stream = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pos = match ctx.win32.get_cfile_mut(stream) {
        Some(entry) => entry.stream_position().map_or(-1i32 as u32, |p| p as u32),
        None => -1i32 as u32,
    };
    return_cdecl(cpu, ctx.arena, pos)?;
    Ok(())
}

fn c_feof(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let stream = peek_arg_u32(cpu, ctx.arena, 0)?;
    let eof = ctx
        .win32
        .get_cfile_mut(stream)
        .is_some_and(|entry| entry.eof);
    return_cdecl(cpu, ctx.arena, u32::from(eof))?;
    Ok(())
}

fn c_ferror(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let stream = peek_arg_u32(cpu, ctx.arena, 0)?;
    let err = ctx
        .win32
        .get_cfile_mut(stream)
        .is_some_and(|entry| entry.err);
    return_cdecl(cpu, ctx.arena, u32::from(err))?;
    Ok(())
}

fn c_clearerr(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let stream = peek_arg_u32(cpu, ctx.arena, 0)?;
    let flags = ctx.win32.get_cfile_mut(stream).map(|entry| {
        entry.eof = false;
        entry.err = false;
        entry.base_flag
    });
    if let Some(base_flag) = flags {
        sync_cfile_flags(ctx.arena, stream, base_flag, false, false)?;
    }
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_rewind(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let stream = peek_arg_u32(cpu, ctx.arena, 0)?;
    let flags = ctx.win32.get_cfile_mut(stream).map(|entry| {
        entry.err = false;
        let _ = entry.seek(SeekFrom::Start(0));
        entry.base_flag
    });
    if let Some(base_flag) = flags {
        sync_cfile_flags(ctx.arena, stream, base_flag, false, false)?;
    }
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_fgetc(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let stream = peek_arg_u32(cpu, ctx.arena, 0)?;
    let (ret, flags) = match ctx.win32.get_cfile_mut(stream) {
        Some(entry) => {
            let r = entry.read_byte().map_or(-1i32 as u32, u32::from);
            (r, Some((entry.base_flag, entry.eof, entry.err)))
        }
        None => (-1i32 as u32, None),
    };
    if let Some((base_flag, eof, err)) = flags {
        sync_cfile_flags(ctx.arena, stream, base_flag, eof, err)?;
    }
    return_cdecl(cpu, ctx.arena, ret)?;
    Ok(())
}

fn c_fflush(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let stream = peek_arg_u32(cpu, ctx.arena, 0)?;
    if stream != 0 {
        if let Some(entry) = ctx.win32.get_cfile_mut(stream) {
            entry.flush();
        }
    }
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_fclose(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let stream = peek_arg_u32(cpu, ctx.arena, 0)?;
    let res = if ctx.win32.close_cfile(stream) {
        let _ = ctx.heap.free(ctx.arena, stream);
        0
    } else {
        -1i32 as u32
    };
    return_cdecl(cpu, ctx.arena, res)?;
    Ok(())
}

fn c_fgets(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let max_count = peek_arg_i32(cpu, ctx.arena, 1)?;
    let stream = peek_arg_u32(cpu, ctx.arena, 2)?;

    if max_count <= 0 {
        return_cdecl(cpu, ctx.arena, 0)?;
        return Ok(());
    }

    let (mut out, base_flag, eof, err) = {
        let Some(entry) = ctx.win32.get_cfile_mut(stream) else {
            return_cdecl(cpu, ctx.arena, 0)?;
            return Ok(());
        };

        let mut out = Vec::new();
        while out.len() + 1 < max_count as usize {
            let Some(b) = entry.read_byte() else {
                break;
            };
            out.push(b);
            if b == b'\n' {
                break;
            }
        }
        (out, entry.base_flag, entry.eof, entry.err)
    };

    sync_cfile_flags(ctx.arena, stream, base_flag, eof, err)?;
    if out.is_empty() {
        return_cdecl(cpu, ctx.arena, 0)?;
        return Ok(());
    }

    out.push(0);
    ctx.arena.write_bytes(buf_ptr, &out)?;
    return_cdecl(cpu, ctx.arena, buf_ptr)?;
    Ok(())
}

fn peek_f64_arg(cpu: &GuestCpuState, arena: &GuestArena, byte_offset: u32) -> Result<f64, ArenaError> {
    let addr = cpu.gpr(Gpr::Esp) + 4 + byte_offset;
    Ok(f64::from_bits(arena.read_u64(addr)?))
}

fn return_f64_x87(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
    val: f64,
) -> Result<(), RuntimeError> {
    cpu.x87.push(val);
    let saved_eax = cpu.gpr(Gpr::Eax);
    return_cdecl(cpu, ctx.arena, saved_eax)?;
    Ok(())
}

fn c_sin(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.sin())
}

fn c_cos(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.cos())
}

fn c_tan(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.tan())
}

fn c_asin(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.asin())
}

fn c_acos(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.acos())
}

fn c_atan(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.atan())
}

fn c_atan2(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let y = peek_f64_arg(cpu, ctx.arena, 0)?;
    let x = peek_f64_arg(cpu, ctx.arena, 8)?;
    return_f64_x87(ctx, cpu, y.atan2(x))
}

fn c_sqrt(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.sqrt())
}

fn c_pow(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let base = peek_f64_arg(cpu, ctx.arena, 0)?;
    let exp = peek_f64_arg(cpu, ctx.arena, 8)?;
    return_f64_x87(ctx, cpu, base.powf(exp))
}

fn c_exp(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.exp())
}

fn c_log(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.ln())
}

fn c_log10(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.log10())
}

fn c_floor(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.floor())
}

fn c_ceil(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.ceil())
}

fn c_fabs(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_f64_x87(ctx, cpu, x.abs())
}

fn c_fmod(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = peek_f64_arg(cpu, ctx.arena, 0)?;
    let y = peek_f64_arg(cpu, ctx.arena, 8)?;
    return_f64_x87(ctx, cpu, x % y)
}

fn c_ftol(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let val = cpu.x87.pop().trunc();
    let res: i64 = if val.is_nan() || val >= (i64::MAX as f64) || val <= (i64::MIN as f64) {
        i64::MIN
    } else {
        val as i64
    };
    return_cdecl_u64(cpu, ctx.arena, res as u64)?;
    Ok(())
}

fn c_setjmp(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let esp = cpu.gpr(Gpr::Esp);
    let ret_va = ctx.arena.read_u32(esp)?;

    ctx.arena.write_u32(buf_ptr, cpu.gpr(Gpr::Ebp))?;
    ctx.arena.write_u32(buf_ptr + 4, cpu.gpr(Gpr::Ebx))?;
    ctx.arena.write_u32(buf_ptr + 8, cpu.gpr(Gpr::Edi))?;
    ctx.arena.write_u32(buf_ptr + 12, cpu.gpr(Gpr::Esi))?;
    ctx.arena.write_u32(buf_ptr + 16, esp + 4)?;
    ctx.arena.write_u32(buf_ptr + 20, ret_va)?;
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_longjmp(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let raw_val = peek_arg_i32(cpu, ctx.arena, 1)?;
    let ret_val = if raw_val == 0 { 1 } else { raw_val as u32 };

    let ebp = ctx.arena.read_u32(buf_ptr)?;
    let ebx = ctx.arena.read_u32(buf_ptr + 4)?;
    let edi = ctx.arena.read_u32(buf_ptr + 8)?;
    let esi = ctx.arena.read_u32(buf_ptr + 12)?;
    let esp = ctx.arena.read_u32(buf_ptr + 16)?;
    let ret_va = ctx.arena.read_u32(buf_ptr + 20)?;

    cpu.set_gpr(Gpr::Ebp, ebp);
    cpu.set_gpr(Gpr::Ebx, ebx);
    cpu.set_gpr(Gpr::Edi, edi);
    cpu.set_gpr(Gpr::Esi, esi);
    cpu.set_gpr(Gpr::Esp, esp);
    cpu.set_gpr(Gpr::Eax, ret_val);
    cpu.eip = ret_va;
    cpu.exit_reason = ExitReason::Longjmp as u32;
    Ok(())
}

fn c_initterm(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let pf_begin = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pf_end = peek_arg_u32(cpu, ctx.arena, 1)?;

    let mut cur = pf_begin;
    while cur < pf_end {
        let fn_va = ctx.arena.read_u32(cur)?;
        if fn_va != 0 {
            ctx.invoke_guest_callback(cpu, fn_va, &[])?;
        }
        cur = cur.saturating_add(4);
    }

    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_stub_void_1(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_controlfp(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_cdecl(cpu, ctx.arena, cpu.x87.control_word as u32)?;
    Ok(())
}

fn c_atexit(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let fn_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.win32.atexit_callbacks.push(fn_va);
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_exit(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let code = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.win32.exit_code = Some(code);
    cpu.exit_reason = ExitReason::Halt as u32;
    cpu.exit_detail = code;
    return_cdecl(cpu, ctx.arena, code)?;
    Ok(())
}

fn c_srand(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let seed = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.win32.rng_state = seed;
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_rand(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    ctx.win32.rng_state = ctx.win32.rng_state.wrapping_mul(214013).wrapping_add(2531011);
    let val = (ctx.win32.rng_state >> 16) & RAND_MAX_MASK;
    return_cdecl(cpu, ctx.arena, val)?;
    Ok(())
}

fn c_clock(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let millis = ctx.host_start_instant.elapsed().as_millis() as u32;
    return_cdecl(cpu, ctx.arena, millis)?;
    Ok(())
}

fn c_time(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let out_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32);
    if out_ptr != 0 {
        ctx.arena.write_u32(out_ptr, secs)?;
    }
    return_cdecl(cpu, ctx.arena, secs)?;
    Ok(())
}

fn c_qsort(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let base = peek_arg_u32(cpu, ctx.arena, 0)?;
    let num = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let width = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    let cmp_va = peek_arg_u32(cpu, ctx.arena, 3)?;

    if num <= 1 || width == 0 {
        return_cdecl(cpu, ctx.arena, 0)?;
        return Ok(());
    }

    let tmp_a = ctx.heap.alloc(ctx.arena, width, false).unwrap_or(0);
    let tmp_b = ctx.heap.alloc(ctx.arena, width, false).unwrap_or(0);
    if tmp_a == 0 || tmp_b == 0 {
        if tmp_a != 0 {
            let _ = ctx.heap.free(ctx.arena, tmp_a);
        }
        if tmp_b != 0 {
            let _ = ctx.heap.free(ctx.arena, tmp_b);
        }
        return_cdecl(cpu, ctx.arena, 0)?;
        return Ok(());
    }

    for i in 1..num {
        let mut j = i;
        while j > 0 {
            let elem_prev = base + ((j - 1) * width) as u32;
            let elem_cur = base + (j * width) as u32;
            let ord = ctx.invoke_guest_callback(cpu, cmp_va, &[elem_prev, elem_cur])? as i32;
            if ord <= 0 {
                break;
            }
            ctx.arena.copy_bytes(tmp_a, elem_prev, width)?;
            ctx.arena.copy_bytes(tmp_b, elem_cur, width)?;
            ctx.arena.copy_bytes(elem_prev, tmp_b, width)?;
            ctx.arena.copy_bytes(elem_cur, tmp_a, width)?;
            j -= 1;
        }
    }

    let _ = ctx.heap.free(ctx.arena, tmp_a);
    let _ = ctx.heap.free(ctx.arena, tmp_b);
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_p_fmode(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = match ctx.win32.fmode_guest {
        Some(existing) => existing,
        None => {
            let allocated = ctx.heap.alloc(ctx.arena, 4, true).unwrap_or(0);
            ctx.win32.fmode_guest = (allocated != 0).then_some(allocated);
            allocated
        }
    };
    return_cdecl(cpu, ctx.arena, ptr)?;
    Ok(())
}

fn c_p_commode(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = match ctx.win32.commode_guest {
        Some(existing) => existing,
        None => {
            let allocated = ctx.heap.alloc(ctx.arena, 4, true).unwrap_or(0);
            ctx.win32.commode_guest = (allocated != 0).then_some(allocated);
            allocated
        }
    };
    return_cdecl(cpu, ctx.arena, ptr)?;
    Ok(())
}

fn c_getmainargs(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let argc_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let argv_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let env_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;

    let exe_name = ctx.win32.config.exe_name.clone();
    let arg0_va = ctx.heap.alloc(ctx.arena, exe_name.len() + 1, true).unwrap_or(0);
    if arg0_va != 0 {
        ctx.arena.write_cstr(arg0_va, &exe_name)?;
    }

    let argv_arr = ctx.heap.alloc(ctx.arena, 8, true).unwrap_or(0);
    if argv_arr != 0 {
        ctx.arena.write_u32(argv_arr, arg0_va)?;
        ctx.arena.write_u32(argv_arr + 4, 0)?;
    }

    let env_arr = ctx.heap.alloc(ctx.arena, 4, true).unwrap_or(0);
    if argc_ptr != 0 {
        ctx.arena.write_u32(argc_ptr, 1)?;
    }
    if argv_ptr != 0 {
        ctx.arena.write_u32(argv_ptr, argv_arr)?;
    }
    if env_ptr != 0 {
        ctx.arena.write_u32(env_ptr, env_arr)?;
    }

    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_onexit(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let fn_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    if fn_va != 0 {
        ctx.win32.atexit_callbacks.push(fn_va);
    }
    return_cdecl(cpu, ctx.arena, fn_va)?;
    Ok(())
}

fn c_dllonexit(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let fn_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    if fn_va != 0 {
        ctx.win32.atexit_callbacks.push(fn_va);
    }
    return_cdecl(cpu, ctx.arena, fn_va)?;
    Ok(())
}

fn c_stub_void_0(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_stub_one(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_cdecl(cpu, ctx.arena, 1)?;
    Ok(())
}

fn c_terminate(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    ctx.win32.exit_code = Some(1);
    cpu.exit_reason = ExitReason::Halt as u32;
    cpu.exit_detail = 1;
    return_cdecl(cpu, ctx.arena, 1)?;
    Ok(())
}

fn c_cxx_throw_exception(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let obj = peek_arg_u32(cpu, ctx.arena, 0)?;
    cpu.exit_reason = ExitReason::GuestException as u32;
    cpu.exit_detail = obj;
    return_stdcall(cpu, ctx.arena, 2, 0)?;
    Ok(())
}

fn c_aligned_malloc(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let size = peek_arg_u32(cpu, ctx.arena, 0)? as usize;
    let align = (peek_arg_u32(cpu, ctx.arena, 1)? as usize).max(4);
    let total = size.saturating_add(align).saturating_add(4);
    let raw_ptr = ctx.heap.alloc(ctx.arena, total, false).unwrap_or(0);
    if raw_ptr == 0 {
        return_cdecl(cpu, ctx.arena, 0)?;
        return Ok(());
    }
    let min_aligned = raw_ptr + 4;
    let mask = (align as u32) - 1;
    let aligned_ptr = (min_aligned + mask) & !mask;
    ctx.arena.write_u32(aligned_ptr - 4, raw_ptr)?;
    return_cdecl(cpu, ctx.arena, aligned_ptr)?;
    Ok(())
}

fn c_aligned_free(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let mem_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    if mem_ptr != 0 {
        if let Ok(raw_ptr) = ctx.arena.read_u32(mem_ptr - 4) {
            let _ = ctx.heap.free(ctx.arena, raw_ptr);
        }
    }
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_wcslen(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let mut len = 0u32;
    while (len as usize) < MAX_STR_SCAN {
        if ctx.arena.read_u16(ptr + len * 2)? == 0 {
            break;
        }
        len += 1;
    }
    return_cdecl(cpu, ctx.arena, len)?;
    Ok(())
}

fn c_isspace(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ch = peek_arg_u32(cpu, ctx.arena, 0)? as u8;
    return_cdecl(cpu, ctx.arena, u32::from(ch.is_ascii_whitespace()))?;
    Ok(())
}

fn c_isdigit(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ch = peek_arg_u32(cpu, ctx.arena, 0)? as u8;
    return_cdecl(cpu, ctx.arena, u32::from(ch.is_ascii_digit()))?;
    Ok(())
}

fn c_iswascii(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ch = peek_arg_u32(cpu, ctx.arena, 0)?;
    return_cdecl(cpu, ctx.arena, u32::from(ch < 0x80))?;
    Ok(())
}

fn c_finite(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let val = peek_f64_arg(cpu, ctx.arena, 0)?;
    return_cdecl(cpu, ctx.arena, u32::from(val.is_finite()))?;
    Ok(())
}

fn c_puts(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    if ptr != 0 {
        let text = ctx.arena.read_cstr(ptr, MAX_STR_SCAN)?;
        if !ctx.win32.config.headless {
            println!("{text}");
        }
    }
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_fputs(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let stream = peek_arg_u32(cpu, ctx.arena, 1)?;
    if ptr != 0 {
        let bytes = read_cstr_bytes(ctx.arena, ptr)?;
        if let Some(entry) = ctx.win32.get_cfile_mut(stream) {
            entry.write_bytes(&bytes);
        }
    }
    return_cdecl(cpu, ctx.arena, 0)?;
    Ok(())
}

fn c_fprintf(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let stream = peek_arg_u32(cpu, ctx.arena, 0)?;
    let fmt_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let fmt_str = ctx.arena.read_cstr(fmt_ptr, MAX_STR_SCAN)?;
    let formatted = format_printf(ctx.arena, &fmt_str, cpu.gpr(Gpr::Esp) + 12)?;
    let len = formatted.len() as u32;
    if let Some(entry) = ctx.win32.get_cfile_mut(stream) {
        entry.write_bytes(formatted.as_bytes());
    }
    return_cdecl(cpu, ctx.arena, len)?;
    Ok(())
}

fn c_printf(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let fmt_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let fmt_str = ctx.arena.read_cstr(fmt_ptr, MAX_STR_SCAN)?;
    let formatted = format_printf(ctx.arena, &fmt_str, cpu.gpr(Gpr::Esp) + 8)?;
    if !ctx.win32.config.headless {
        print!("{formatted}");
    }
    return_cdecl(cpu, ctx.arena, formatted.len() as u32)?;
    Ok(())
}

fn c_strtok(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let str_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let delim_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let delims = read_cstr_bytes(ctx.arena, delim_ptr)?;

    let mut cur = if str_ptr != 0 {
        str_ptr
    } else {
        let Some(saved) = ctx.win32.strtok_cursor else {
            return_cdecl(cpu, ctx.arena, 0)?;
            return Ok(());
        };
        saved
    };

    loop {
        let b = ctx.arena.read_u8(cur)?;
        if b == 0 {
            ctx.win32.strtok_cursor = None;
            return_cdecl(cpu, ctx.arena, 0)?;
            return Ok(());
        }
        if !delims.contains(&b) {
            break;
        }
        cur += 1;
    }

    let token_start = cur;
    loop {
        let b = ctx.arena.read_u8(cur)?;
        if b == 0 {
            ctx.win32.strtok_cursor = None;
            break;
        }
        if delims.contains(&b) {
            ctx.arena.write_u8(cur, 0)?;
            ctx.win32.strtok_cursor = Some(cur + 1);
            break;
        }
        cur += 1;
    }

    return_cdecl(cpu, ctx.arena, token_start)?;
    Ok(())
}

fn c_beginthread(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let start_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    let param = peek_arg_u32(cpu, ctx.arena, 2)?;
    let handle = crate::win32::kernel32::alloc_and_queue_guest_thread(ctx, start_va, param, false)?;
    return_cdecl(cpu, ctx.arena, handle.unwrap_or(u32::MAX))?;
    Ok(())
}

fn c_exception_ctor_default(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_va = cpu.gpr(Gpr::Ecx);
    return_stdcall(cpu, ctx.arena, 0, this_va)?;
    Ok(())
}

fn c_exception_ctor_copy(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_va = cpu.gpr(Gpr::Ecx);
    return_stdcall(cpu, ctx.arena, 1, this_va)?;
    Ok(())
}

fn c_exception_dtor(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_va = cpu.gpr(Gpr::Ecx);
    return_stdcall(cpu, ctx.arena, 0, this_va)?;
    Ok(())
}

fn c_ci_asin(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = cpu.x87.st(0);
    *cpu.x87.st_mut(0) = x.asin();
    let saved_eax = cpu.gpr(Gpr::Eax);
    return_cdecl(cpu, ctx.arena, saved_eax)?;
    Ok(())
}

fn c_ci_acos(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let x = cpu.x87.st(0);
    *cpu.x87.st_mut(0) = x.acos();
    let saved_eax = cpu.gpr(Gpr::Eax);
    return_cdecl(cpu, ctx.arena, saved_eax)?;
    Ok(())
}

fn c_ci_pow(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let exp = cpu.x87.pop();
    let base = cpu.x87.st(0);
    *cpu.x87.st_mut(0) = base.powf(exp);
    let saved_eax = cpu.gpr(Gpr::Eax);
    return_cdecl(cpu, ctx.arena, saved_eax)?;
    Ok(())
}

fn c_ci_fmod(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let y = cpu.x87.pop();
    let x = cpu.x87.st(0);
    *cpu.x87.st_mut(0) = x % y;
    let saved_eax = cpu.gpr(Gpr::Eax);
    return_cdecl(cpu, ctx.arena, saved_eax)?;
    Ok(())
}
