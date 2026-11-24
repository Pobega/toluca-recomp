use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use toluca_core::GuestAddr;
use toluca_core::aot::ImportSymbol;
use toluca_core::cpu::{ExitReason, GuestCpuState};

use crate::callconv::{peek_arg_i32, peek_arg_u32, return_stdcall};
use crate::heap::{
    MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
};
use crate::runtime::{ImportRegistry, RuntimeContext, RuntimeError};
use crate::win32::path::wildcard_match_ci;
use crate::win32::{
    ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE,
    ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, ERROR_NOT_ENOUGH_MEMORY, ERROR_PATH_NOT_FOUND,
    ERROR_PROC_NOT_FOUND, ERROR_SUCCESS, FindEntry, FindState, INVALID_FILE_ATTRIBUTES,
    INVALID_HANDLE_VALUE, INVALID_SET_FILE_POINTER, PROCESS_HEAP_HANDLE, PendingHostWait,
    PendingThreadSpawn, PendingTimerSpawn, STD_ERR_HOST_HANDLE, STD_ERROR_HANDLE,
    STD_IN_HOST_HANDLE, STD_INPUT_HANDLE, STD_OUT_HOST_HANDLE, STD_OUTPUT_HANDLE,
    TLS_OUT_OF_INDEXES, Win32Handle, map_io_error,
};

const MAX_PATH_LEN: usize = 512;
const MAX_DEBUG_STR_LEN: usize = 4096;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_NEW: u32 = 1;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const TRUNCATE_EXISTING: u32 = 5;

const FILE_BEGIN: u32 = 0;
const FILE_CURRENT: u32 = 1;
const FILE_END: u32 = 2;

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
const WIN32_FIND_DATA_SIZE: usize = 320;
const FIND_DATA_OFF_ATTRS: u32 = 0;
const FIND_DATA_OFF_SIZE_HIGH: u32 = 28;
const FIND_DATA_OFF_SIZE_LOW: u32 = 32;
const FIND_DATA_OFF_NAME: u32 = 44;

const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 0x0000_0102;
const CREATE_SUSPENDED: u32 = 0x0000_0004;
const STILL_ACTIVE: u32 = 259;
const GUEST_THREAD_STACK_SIZE: usize = 256 * 1024;
const GUEST_THREAD_TEB_SIZE: usize = 0x1000;
const TIME_PERIODIC: u32 = 0x0001;
const FIND_DATA_NAME_MAX: usize = 259;

const HEAP_ZERO_MEMORY: u32 = 0x0000_0008;
const GMEM_ZEROINIT: u32 = 0x0040;
const LMEM_ZEROINIT: u32 = 0x0040;

const CURRENT_THREAD_ID: u32 = 1;
const CURRENT_PROCESS_ID: u32 = 1000;
const CURRENT_PROCESS_HANDLE: u32 = 0xFFFF_FFFF;
const CURRENT_THREAD_HANDLE: u32 = 0xFFFF_FFFE;

const QPC_FREQUENCY_HZ: u64 = 1_000_000_000;
const WIN_XP_VERSION: u32 = 0x0A28_0105;
const VER_PLATFORM_WIN32_NT: u32 = 2;
const STARTUPINFOA_SIZE: usize = 68;
const OSVERSIONINFOA_SIZE: usize = 148;
const SYSTEM_INFO_SIZE: usize = 36;
const CODEPAGE_WIN1252: u32 = 1252;

const FILE_TYPE_UNKNOWN: u32 = 0;
const FILE_TYPE_DISK: u32 = 1;
const FILE_TYPE_CHAR: u32 = 2;

const PF_FLOATING_POINT_PRECISION_ERRATA: u32 = 0;
const PF_FLOATING_POINT_EMULATED: u32 = 1;
const PF_COMPARE_EXCHANGE_DOUBLE: u32 = 2;
const PF_MMX_INSTRUCTIONS_AVAILABLE: u32 = 3;
const PF_XMMI_INSTRUCTIONS_AVAILABLE: u32 = 6;
const PF_RDTSC_INSTRUCTION_AVAILABLE: u32 = 8;
const PF_XMMI64_INSTRUCTIONS_AVAILABLE: u32 = 10;

pub fn register_imports(registry: &mut ImportRegistry) {
    let k32 = "kernel32.dll";
    let entries: &[(&str, crate::runtime::ImportHandler)] = &[
        ("CreateFileA", create_file_a),
        ("ReadFile", read_file),
        ("WriteFile", write_file),
        ("SetFilePointer", set_file_pointer),
        ("GetFileSize", get_file_size),
        ("CloseHandle", close_handle),
        ("DeleteFileA", delete_file_a),
        ("CreateDirectoryA", create_directory_a),
        ("GetFileAttributesA", get_file_attributes_a),
        ("FindFirstFileA", find_first_file_a),
        ("FindNextFileA", find_next_file_a),
        ("FindClose", find_close),
        ("GetCurrentDirectoryA", get_current_directory_a),
        ("SetCurrentDirectoryA", set_current_directory_a),
        ("GetModuleFileNameA", get_module_file_name_a),
        ("GetFullPathNameA", get_full_path_name_a),
        ("VirtualAlloc", virtual_alloc),
        ("VirtualFree", virtual_free),
        ("VirtualProtect", virtual_protect),
        ("GetProcessHeap", get_process_heap),
        ("HeapCreate", heap_create),
        ("HeapDestroy", heap_destroy),
        ("HeapAlloc", heap_alloc),
        ("HeapReAlloc", heap_realloc),
        ("HeapFree", heap_free),
        ("HeapSize", heap_size),
        ("GlobalAlloc", global_alloc),
        ("GlobalFree", global_free),
        ("GlobalLock", global_lock),
        ("GlobalUnlock", global_unlock),
        ("LocalAlloc", local_alloc),
        ("LocalFree", local_free),
        ("InitializeCriticalSection", initialize_critical_section),
        ("EnterCriticalSection", enter_critical_section),
        ("LeaveCriticalSection", leave_critical_section),
        ("DeleteCriticalSection", delete_critical_section),
        ("CreateEventA", create_event_a),
        ("SetEvent", set_event),
        ("ResetEvent", reset_event),
        ("WaitForSingleObject", wait_for_single_object),
        ("TlsAlloc", tls_alloc),
        ("TlsGetValue", tls_get_value),
        ("TlsSetValue", tls_set_value),
        ("TlsFree", tls_free),
        ("GetCurrentThreadId", get_current_thread_id),
        ("GetCurrentProcessId", get_current_process_id),
        ("GetCurrentProcess", get_current_process),
        ("GetCurrentThread", get_current_thread),
        ("InterlockedIncrement", interlocked_increment),
        ("InterlockedDecrement", interlocked_decrement),
        ("InterlockedExchange", interlocked_exchange),
        ("QueryPerformanceFrequency", query_performance_frequency),
        ("QueryPerformanceCounter", query_performance_counter),
        ("GetTickCount", get_tick_count),
        ("Sleep", sleep_ms),
        ("GetLocalTime", get_local_time),
        ("GetSystemTime", get_system_time),
        ("GetModuleHandleA", get_module_handle_a),
        ("LoadLibraryA", load_library_a),
        ("FreeLibrary", free_library),
        ("GetProcAddress", get_proc_address),
        ("GetStartupInfoA", get_startup_info_a),
        ("GetCommandLineA", get_command_line_a),
        ("GetVersion", get_version),
        ("GetVersionExA", get_version_ex_a),
        ("GetSystemInfo", get_system_info),
        ("GetACP", get_acp),
        ("GetStdHandle", get_std_handle),
        ("GetFileType", get_file_type),
        ("GetEnvironmentStrings", get_environment_strings),
        ("GetEnvironmentStringsA", get_environment_strings),
        ("FreeEnvironmentStringsA", free_environment_strings_a),
        ("MultiByteToWideChar", multi_byte_to_wide_char),
        ("WideCharToMultiByte", wide_char_to_multi_byte),
        ("GetLastError", get_last_error),
        ("SetLastError", set_last_error),
        ("SetUnhandledExceptionFilter", set_unhandled_exception_filter),
        ("IsProcessorFeaturePresent", is_processor_feature_present),
        ("OutputDebugStringA", output_debug_string_a),
        ("ExitProcess", exit_process),
        ("lstrlenA", lstrlen_a),
        ("lstrcpyA", lstrcpy_a),
        ("lstrcatA", lstrcat_a),
        ("lstrcmpiA", lstrcmpi_a),
        ("CreateFileW", create_file_w),
        ("GlobalMemoryStatus", global_memory_status),
        ("CreateMutexA", create_mutex_a),
        ("ReleaseMutex", release_mutex),
        ("GetSystemTimeAsFileTime", get_system_time_as_file_time),
        ("FileTimeToLocalFileTime", file_time_to_local_file_time),
        ("FileTimeToSystemTime", file_time_to_system_time),
        ("SystemTimeToFileTime", system_time_to_file_time),
        ("GetThreadLocale", get_thread_locale),
        ("GetLocaleInfoA", get_locale_info_a),
        ("GetDriveTypeA", get_drive_type_a),
        ("GetDiskFreeSpaceExA", get_disk_free_space_ex_a),
        ("CreateFileMappingA", create_file_mapping_a),
        ("MapViewOfFile", map_view_of_file),
        ("UnmapViewOfFile", unmap_view_of_file),
        ("FindResourceA", find_resource_null),
        ("FindResourceW", find_resource_null),
        ("LoadResource", load_resource),
        ("LockResource", lock_resource),
        ("SizeofResource", sizeof_resource),
        ("SetFileAttributesA", set_file_attributes_a),
        ("CopyFileA", copy_file_a),
        ("RemoveDirectoryA", remove_directory_a),
        ("GetOverlappedResult", get_overlapped_result),
        ("CreateThread", create_thread),
        ("ExitThread", exit_thread),
        ("GetExitCodeThread", get_exit_code_thread),
        ("SetThreadPriority", set_thread_priority),
        ("GetThreadPriority", get_thread_priority),
        ("SetThreadPriorityBoost", set_thread_priority_boost),
        ("ResumeThread", resume_thread),
        ("SuspendThread", suspend_thread),
        ("PulseEvent", pulse_event),
    ];

    for &(name, handler) in entries {
        registry.register_name(k32, name, handler);
    }

    registry.register_name("gdi32.dll", "DeleteObject", gdi_delete_object);

    registry.register_name("winmm.dll", "timeGetTime", time_get_time);
    registry.register_name("winmm.dll", "timeBeginPeriod", time_begin_period);
    registry.register_name("winmm.dll", "timeEndPeriod", time_end_period);
    registry.register_name("winmm.dll", "timeSetEvent", time_set_event);
    registry.register_name("winmm.dll", "timeKillEvent", time_kill_event);
}

fn create_file_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let name_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let desired_access = peek_arg_u32(cpu, ctx.arena, 1)?;
    let disposition = peek_arg_u32(cpu, ctx.arena, 4)?;

    let win_path = ctx.arena.read_cstr(name_ptr, MAX_PATH_LEN)?;
    let host_path = ctx.win32.resolve_path(&win_path);
    let existed = host_path.exists();

    let want_read = (desired_access & GENERIC_READ) != 0 || desired_access == 0;
    let want_write = (desired_access & GENERIC_WRITE) != 0;

    let mut opts = OpenOptions::new();
    match disposition {
        CREATE_NEW => {
            opts.read(want_read).write(true).create_new(true);
        }
        CREATE_ALWAYS => {
            opts.read(want_read).write(true).create(true).truncate(true);
        }
        OPEN_EXISTING => {
            opts.read(want_read || !want_write).write(want_write);
        }
        OPEN_ALWAYS => {
            opts.read(want_read || !want_write)
                .write(want_write)
                .create(true);
        }
        TRUNCATE_EXISTING => {
            opts.read(want_read).write(true).truncate(true);
        }
        _ => {
            *ctx.last_error = ERROR_INVALID_PARAMETER;
            return_stdcall(cpu, ctx.arena, 7, INVALID_HANDLE_VALUE)?;
            return Ok(());
        }
    }

    match opts.open(&host_path) {
        Ok(file) => {
            *ctx.last_error = if existed && (disposition == CREATE_ALWAYS || disposition == OPEN_ALWAYS) {
                ERROR_ALREADY_EXISTS
            } else {
                ERROR_SUCCESS
            };
            let handle = ctx.win32.alloc_handle(Win32Handle::File {
                file,
                path: host_path,
            });
            return_stdcall(cpu, ctx.arena, 7, handle)?;
        }
        Err(err) => {
            *ctx.last_error = map_io_error(&err);
            return_stdcall(cpu, ctx.arena, 7, INVALID_HANDLE_VALUE)?;
        }
    }
    Ok(())
}

fn read_file(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let to_read = peek_arg_u32(cpu, ctx.arena, 2)?;
    let bytes_read_ptr = peek_arg_u32(cpu, ctx.arena, 3)?;
    let overlapped_ptr = peek_arg_u32(cpu, ctx.arena, 4)?;

    let overlapped_offset = if overlapped_ptr != 0 {
        let lo = u64::from(ctx.arena.read_u32(overlapped_ptr + 8)?);
        let hi = u64::from(ctx.arena.read_u32(overlapped_ptr + 12)?);
        Some((hi << 32) | lo)
    } else {
        None
    };

    let mut host_buf = vec![0u8; to_read as usize];
    let result = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::File { file, .. }) => {
            if let Some(off) = overlapped_offset {
                if let Err(err) = file.seek(SeekFrom::Start(off)) {
                    *ctx.last_error = map_io_error(&err);
                    return_stdcall(cpu, ctx.arena, 5, 0)?;
                    return Ok(());
                }
            }
            file.read(&mut host_buf)
        }
        _ => {
            *ctx.last_error = ERROR_INVALID_HANDLE;
            return_stdcall(cpu, ctx.arena, 5, 0)?;
            return Ok(());
        }
    };

    match result {
        Ok(n) => {
            ctx.arena.write_bytes(buf_ptr, &host_buf[..n])?;
            if bytes_read_ptr != 0 {
                ctx.arena.write_u32(bytes_read_ptr, n as u32)?;
            }
            if overlapped_ptr != 0 {
                ctx.arena.write_u32(overlapped_ptr, 0)?;
                ctx.arena.write_u32(overlapped_ptr + 4, n as u32)?;
                let event_handle = ctx.arena.read_u32(overlapped_ptr + 16)?;
                if event_handle != 0 {
                    if let Some(Win32Handle::Event { signaled, .. }) =
                        ctx.win32.get_handle_mut(event_handle)
                    {
                        *signaled = true;
                        ctx.win32.notify_sync = true;
                    }
                }
            }
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 5, 1)?;
        }
        Err(err) => {
            *ctx.last_error = map_io_error(&err);
            return_stdcall(cpu, ctx.arena, 5, 0)?;
        }
    }
    Ok(())
}

fn write_file(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let to_write = peek_arg_u32(cpu, ctx.arena, 2)?;
    let bytes_written_ptr = peek_arg_u32(cpu, ctx.arena, 3)?;

    let data = ctx.arena.read_bytes(buf_ptr, to_write as usize)?.to_vec();

    if handle == STD_OUT_HOST_HANDLE || handle == STD_ERR_HOST_HANDLE {
        let written = if ctx.win32.config.headless {
            Ok(data.len())
        } else if handle == STD_OUT_HOST_HANDLE {
            std::io::stdout().write(&data)
        } else {
            std::io::stderr().write(&data)
        };
        match written {
            Ok(n) => {
                if bytes_written_ptr != 0 {
                    ctx.arena.write_u32(bytes_written_ptr, n as u32)?;
                }
                *ctx.last_error = ERROR_SUCCESS;
                return_stdcall(cpu, ctx.arena, 5, 1)?;
            }
            Err(err) => {
                *ctx.last_error = map_io_error(&err);
                return_stdcall(cpu, ctx.arena, 5, 0)?;
            }
        }
        return Ok(());
    }

    let result = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::File { file, .. }) => file.write(&data),
        _ => {
            *ctx.last_error = ERROR_INVALID_HANDLE;
            return_stdcall(cpu, ctx.arena, 5, 0)?;
            return Ok(());
        }
    };

    match result {
        Ok(n) => {
            if bytes_written_ptr != 0 {
                ctx.arena.write_u32(bytes_written_ptr, n as u32)?;
            }
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 5, 1)?;
        }
        Err(err) => {
            *ctx.last_error = map_io_error(&err);
            return_stdcall(cpu, ctx.arena, 5, 0)?;
        }
    }
    Ok(())
}

fn set_file_pointer(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let dist_low = peek_arg_i32(cpu, ctx.arena, 1)?;
    let dist_high_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let move_method = peek_arg_u32(cpu, ctx.arena, 3)?;

    let offset = if dist_high_ptr != 0 {
        let high = ctx.arena.read_i32(dist_high_ptr)?;
        ((high as i64) << 32) | ((dist_low as u32) as i64)
    } else {
        dist_low as i64
    };

    let seek_from = match move_method {
        FILE_BEGIN => SeekFrom::Start(offset as u64),
        FILE_CURRENT => SeekFrom::Current(offset),
        FILE_END => SeekFrom::End(offset),
        _ => {
            *ctx.last_error = ERROR_INVALID_PARAMETER;
            return_stdcall(cpu, ctx.arena, 4, INVALID_SET_FILE_POINTER)?;
            return Ok(());
        }
    };

    let result = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::File { file, .. }) => file.seek(seek_from),
        _ => {
            *ctx.last_error = ERROR_INVALID_HANDLE;
            return_stdcall(cpu, ctx.arena, 4, INVALID_SET_FILE_POINTER)?;
            return Ok(());
        }
    };

    match result {
        Ok(pos) => {
            if dist_high_ptr != 0 {
                ctx.arena.write_u32(dist_high_ptr, (pos >> 32) as u32)?;
            }
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 4, pos as u32)?;
        }
        Err(err) => {
            *ctx.last_error = map_io_error(&err);
            return_stdcall(cpu, ctx.arena, 4, INVALID_SET_FILE_POINTER)?;
        }
    }
    Ok(())
}

fn get_file_size(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let size_high_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;

    let meta = match ctx.win32.get_handle(handle) {
        Some(Win32Handle::File { file, .. }) => file.metadata(),
        _ => {
            *ctx.last_error = ERROR_INVALID_HANDLE;
            return_stdcall(cpu, ctx.arena, 2, INVALID_HANDLE_VALUE)?;
            return Ok(());
        }
    };

    match meta {
        Ok(m) => {
            let len = m.len();
            if size_high_ptr != 0 {
                ctx.arena.write_u32(size_high_ptr, (len >> 32) as u32)?;
            }
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 2, len as u32)?;
        }
        Err(err) => {
            *ctx.last_error = map_io_error(&err);
            return_stdcall(cpu, ctx.arena, 2, INVALID_HANDLE_VALUE)?;
        }
    }
    Ok(())
}

fn close_handle(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    if ctx.win32.close_handle(handle) {
        *ctx.last_error = ERROR_SUCCESS;
        return_stdcall(cpu, ctx.arena, 1, 1)?;
    } else {
        *ctx.last_error = ERROR_INVALID_HANDLE;
        return_stdcall(cpu, ctx.arena, 1, 0)?;
    }
    Ok(())
}

fn delete_file_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let path_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let win_path = ctx.arena.read_cstr(path_ptr, MAX_PATH_LEN)?;
    let host_path = ctx.win32.resolve_path(&win_path);
    match std::fs::remove_file(&host_path) {
        Ok(()) => {
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 1, 1)?;
        }
        Err(err) => {
            *ctx.last_error = map_io_error(&err);
            return_stdcall(cpu, ctx.arena, 1, 0)?;
        }
    }
    Ok(())
}

fn create_directory_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let path_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let win_path = ctx.arena.read_cstr(path_ptr, MAX_PATH_LEN)?;
    let host_path = ctx.win32.resolve_path(&win_path);
    match std::fs::create_dir(&host_path) {
        Ok(()) => {
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 2, 1)?;
        }
        Err(err) => {
            *ctx.last_error = if err.kind() == std::io::ErrorKind::AlreadyExists {
                ERROR_ALREADY_EXISTS
            } else {
                map_io_error(&err)
            };
            return_stdcall(cpu, ctx.arena, 2, 0)?;
        }
    }
    Ok(())
}

fn get_file_attributes_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let path_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let win_path = ctx.arena.read_cstr(path_ptr, MAX_PATH_LEN)?;
    let host_path = ctx.win32.resolve_path(&win_path);
    match std::fs::metadata(&host_path) {
        Ok(meta) => {
            let attrs = if meta.is_dir() {
                FILE_ATTRIBUTE_DIRECTORY
            } else {
                FILE_ATTRIBUTE_NORMAL
            };
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 1, attrs)?;
        }
        Err(err) => {
            *ctx.last_error = map_io_error(&err);
            return_stdcall(cpu, ctx.arena, 1, INVALID_FILE_ATTRIBUTES)?;
        }
    }
    Ok(())
}

fn find_first_file_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let pattern_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let data_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let win_pattern = ctx.arena.read_cstr(pattern_ptr, MAX_PATH_LEN)?;

    let (dir_part, file_pattern) = match win_pattern.rfind(['\\', '/']) {
        Some(idx) => (&win_pattern[..idx], &win_pattern[idx + 1..]),
        None => ("", win_pattern.as_str()),
    };

    let host_dir = if dir_part.is_empty() {
        ctx.win32.current_dir.clone()
    } else {
        ctx.win32.resolve_path(dir_part)
    };

    let Ok(read_dir) = std::fs::read_dir(&host_dir) else {
        *ctx.last_error = ERROR_PATH_NOT_FOUND;
        return_stdcall(cpu, ctx.arena, 2, INVALID_HANDLE_VALUE)?;
        return Ok(());
    };

    let mut entries = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if wildcard_match_ci(file_pattern, &name) {
            let meta = entry.metadata().ok();
            entries.push(FindEntry {
                file_name: name,
                is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                file_size: meta.as_ref().map_or(0, |m| m.len()),
            });
        }
    }

    entries.sort_by(|a, b| a.file_name.to_ascii_lowercase().cmp(&b.file_name.to_ascii_lowercase()));

    if entries.is_empty() {
        *ctx.last_error = ERROR_FILE_NOT_FOUND;
        return_stdcall(cpu, ctx.arena, 2, INVALID_HANDLE_VALUE)?;
        return Ok(());
    }

    write_find_data(ctx.arena, data_ptr, &entries[0])?;
    let handle = ctx.win32.alloc_handle(Win32Handle::FindFile(FindState {
        entries,
        next_index: 1,
    }));
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 2, handle)?;
    Ok(())
}

fn find_next_file_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let data_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;

    let entry = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::FindFile(state)) => {
            if state.next_index < state.entries.len() {
                let item = state.entries[state.next_index].clone();
                state.next_index += 1;
                Some(item)
            } else {
                None
            }
        }
        _ => {
            *ctx.last_error = ERROR_INVALID_HANDLE;
            return_stdcall(cpu, ctx.arena, 2, 0)?;
            return Ok(());
        }
    };

    if let Some(item) = entry {
        write_find_data(ctx.arena, data_ptr, &item)?;
        *ctx.last_error = ERROR_SUCCESS;
        return_stdcall(cpu, ctx.arena, 2, 1)?;
    } else {
        *ctx.last_error = ERROR_NO_MORE_FILES;
        return_stdcall(cpu, ctx.arena, 2, 0)?;
    }
    Ok(())
}

fn find_close(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let ok = if matches!(ctx.win32.get_handle(handle), Some(Win32Handle::FindFile(_))) {
        ctx.win32.close_handle(handle)
    } else {
        false
    };
    if ok {
        *ctx.last_error = ERROR_SUCCESS;
        return_stdcall(cpu, ctx.arena, 1, 1)?;
    } else {
        *ctx.last_error = ERROR_INVALID_HANDLE;
        return_stdcall(cpu, ctx.arena, 1, 0)?;
    }
    Ok(())
}

fn write_find_data(
    arena: &mut crate::arena::GuestArena,
    addr: GuestAddr,
    entry: &FindEntry,
) -> Result<(), RuntimeError> {
    arena.zero_range(addr, WIN32_FIND_DATA_SIZE)?;
    let attrs = if entry.is_dir {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };
    arena.write_u32(addr + FIND_DATA_OFF_ATTRS, attrs)?;
    arena.write_u32(addr + FIND_DATA_OFF_SIZE_HIGH, (entry.file_size >> 32) as u32)?;
    arena.write_u32(addr + FIND_DATA_OFF_SIZE_LOW, entry.file_size as u32)?;
    let name_bytes = entry.file_name.as_bytes();
    let copy_len = name_bytes.len().min(FIND_DATA_NAME_MAX);
    arena.write_bytes(addr + FIND_DATA_OFF_NAME, &name_bytes[..copy_len])?;
    arena.write_u8(addr + FIND_DATA_OFF_NAME + copy_len as u32, 0)?;
    Ok(())
}

fn get_current_directory_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let buf_len = peek_arg_u32(cpu, ctx.arena, 0)? as usize;
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let win_dir = format_windows_path(&ctx.win32.config.game_root, &ctx.win32.current_dir);

    let needed_with_nul = win_dir.len() + 1;
    if buf_ptr == 0 || buf_len < needed_with_nul {
        return_stdcall(cpu, ctx.arena, 2, needed_with_nul as u32)?;
        return Ok(());
    }
    ctx.arena.write_cstr(buf_ptr, &win_dir)?;
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 2, win_dir.len() as u32)?;
    Ok(())
}

fn set_current_directory_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let path_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let win_path = ctx.arena.read_cstr(path_ptr, MAX_PATH_LEN)?;
    let resolved = ctx.win32.resolve_path(&win_path);
    if resolved.is_dir() {
        ctx.win32.current_dir = resolved;
        *ctx.last_error = ERROR_SUCCESS;
        return_stdcall(cpu, ctx.arena, 1, 1)?;
    } else {
        *ctx.last_error = ERROR_PATH_NOT_FOUND;
        return_stdcall(cpu, ctx.arena, 1, 0)?;
    }
    Ok(())
}

fn get_module_file_name_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let h_module = peek_arg_u32(cpu, ctx.arena, 0)?;
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let size = peek_arg_u32(cpu, ctx.arena, 2)? as usize;

    let Some(name) = ctx.win32.module_name_for_handle(h_module, ctx.pe_image_base) else {
        *ctx.last_error = ERROR_INVALID_HANDLE;
        return_stdcall(cpu, ctx.arena, 3, 0)?;
        return Ok(());
    };

    let full = format!("C:\\{name}");
    if size == 0 || buf_ptr == 0 {
        *ctx.last_error = ERROR_INSUFFICIENT_BUFFER;
        return_stdcall(cpu, ctx.arena, 3, 0)?;
        return Ok(());
    }

    let bytes = full.as_bytes();
    let copy_len = bytes.len().min(size - 1);
    ctx.arena.write_bytes(buf_ptr, &bytes[..copy_len])?;
    ctx.arena.write_u8(buf_ptr + copy_len as u32, 0)?;
    *ctx.last_error = if copy_len < bytes.len() {
        ERROR_INSUFFICIENT_BUFFER
    } else {
        ERROR_SUCCESS
    };
    return_stdcall(cpu, ctx.arena, 3, copy_len as u32)?;
    Ok(())
}

fn get_full_path_name_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let name_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let buf_len = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let file_part_ptr = peek_arg_u32(cpu, ctx.arena, 3)?;

    let win_path = ctx.arena.read_cstr(name_ptr, MAX_PATH_LEN)?;
    let resolved = ctx.win32.resolve_path(&win_path);
    let full = format_windows_path(&ctx.win32.config.game_root, &resolved);

    let needed = full.len() + 1;
    if buf_ptr == 0 || buf_len < needed {
        return_stdcall(cpu, ctx.arena, 4, needed as u32)?;
        return Ok(());
    }

    ctx.arena.write_cstr(buf_ptr, &full)?;
    if file_part_ptr != 0 {
        let part_addr = match full.rfind('\\') {
            Some(pos) if pos + 1 < full.len() => buf_ptr + (pos as u32) + 1,
            _ => 0,
        };
        ctx.arena.write_u32(file_part_ptr, part_addr)?;
    }
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 4, full.len() as u32)?;
    Ok(())
}

fn format_windows_path(game_root: &Path, host_path: &Path) -> String {
    let rel = host_path.strip_prefix(game_root).unwrap_or(host_path);
    let mut parts = Vec::new();
    for comp in rel.components() {
        if let std::path::Component::Normal(os) = comp {
            parts.push(os.to_string_lossy().into_owned());
        }
    }
    if parts.is_empty() {
        "C:\\".to_owned()
    } else {
        format!("C:\\{}", parts.join("\\"))
    }
}

fn virtual_alloc(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let addr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let size = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let alloc_type = peek_arg_u32(cpu, ctx.arena, 2)?;
    let protect = peek_arg_u32(cpu, ctx.arena, 3)?;

    let req = (addr != 0).then_some(addr);
    match ctx
        .vmem
        .virtual_alloc(ctx.arena, req, size, alloc_type, protect)
    {
        Ok(base) => {
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 4, base)?;
        }
        Err(_) => {
            *ctx.last_error = ERROR_NOT_ENOUGH_MEMORY;
            return_stdcall(cpu, ctx.arena, 4, 0)?;
        }
    }
    Ok(())
}

fn virtual_free(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let addr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let size = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let free_type = peek_arg_u32(cpu, ctx.arena, 2)?;

    if free_type != MEM_DECOMMIT && free_type != MEM_RELEASE {
        *ctx.last_error = ERROR_INVALID_PARAMETER;
        return_stdcall(cpu, ctx.arena, 3, 0)?;
        return Ok(());
    }

    match ctx.vmem.virtual_free(ctx.arena, addr, size, free_type) {
        Ok(()) => {
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 3, 1)?;
        }
        Err(_) => {
            *ctx.last_error = ERROR_INVALID_PARAMETER;
            return_stdcall(cpu, ctx.arena, 3, 0)?;
        }
    }
    Ok(())
}

fn virtual_protect(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let addr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let size = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let new_protect = peek_arg_u32(cpu, ctx.arena, 2)?;
    let old_protect_ptr = peek_arg_u32(cpu, ctx.arena, 3)?;

    let writable = match new_protect {
        PAGE_READONLY | PAGE_EXECUTE_READ | PAGE_NOACCESS => false,
        PAGE_READWRITE | PAGE_EXECUTE_READWRITE => true,
        _ => {
            *ctx.last_error = ERROR_INVALID_PARAMETER;
            return_stdcall(cpu, ctx.arena, 4, 0)?;
            return Ok(());
        }
    };

    if ctx.arena.is_range_committed(addr, size.max(1)) {
        let _ = ctx.arena.commit_range(addr, size.max(1), writable);
    }
    if old_protect_ptr != 0 {
        ctx.arena.write_u32(old_protect_ptr, PAGE_READWRITE)?;
    }
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 4, 1)?;
    Ok(())
}

fn get_process_heap(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 0, PROCESS_HEAP_HANDLE)?;
    Ok(())
}

fn heap_create(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = ctx.win32.alloc_handle(Win32Handle::Heap { id: 1 });
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 3, handle)?;
    Ok(())
}

fn heap_destroy(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let h_heap = peek_arg_u32(cpu, ctx.arena, 0)?;
    if h_heap != PROCESS_HEAP_HANDLE {
        ctx.win32.close_handle(h_heap);
    }
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn heap_alloc(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let flags = peek_arg_u32(cpu, ctx.arena, 1)?;
    let bytes = peek_arg_u32(cpu, ctx.arena, 2)? as usize;
    let zeroed = (flags & HEAP_ZERO_MEMORY) != 0;

    match ctx.heap.alloc(ctx.arena, bytes, zeroed) {
        Ok(ptr) => return_stdcall(cpu, ctx.arena, 3, ptr)?,
        Err(_) => {
            *ctx.last_error = ERROR_NOT_ENOUGH_MEMORY;
            return_stdcall(cpu, ctx.arena, 3, 0)?;
        }
    }
    Ok(())
}

fn heap_realloc(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let flags = peek_arg_u32(cpu, ctx.arena, 1)?;
    let mem_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let bytes = peek_arg_u32(cpu, ctx.arena, 3)? as usize;
    let zeroed = (flags & HEAP_ZERO_MEMORY) != 0;

    match ctx.heap.realloc(ctx.arena, mem_ptr, bytes, zeroed) {
        Ok(ptr) => return_stdcall(cpu, ctx.arena, 4, ptr)?,
        Err(_) => {
            *ctx.last_error = ERROR_NOT_ENOUGH_MEMORY;
            return_stdcall(cpu, ctx.arena, 4, 0)?;
        }
    }
    Ok(())
}

fn heap_free(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let mem_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    if mem_ptr == 0 {
        return_stdcall(cpu, ctx.arena, 3, 1)?;
        return Ok(());
    }
    let ok = u32::from(ctx.heap.free(ctx.arena, mem_ptr).is_ok());
    return_stdcall(cpu, ctx.arena, 3, ok)?;
    Ok(())
}

fn heap_size(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let mem_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let size = ctx
        .heap
        .block_size(mem_ptr)
        .map_or(u32::MAX, |s| s as u32);
    return_stdcall(cpu, ctx.arena, 3, size)?;
    Ok(())
}

fn global_alloc(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let flags = peek_arg_u32(cpu, ctx.arena, 0)?;
    let bytes = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let zeroed = (flags & GMEM_ZEROINIT) != 0;
    let ptr = ctx.heap.alloc(ctx.arena, bytes, zeroed).unwrap_or(0);
    return_stdcall(cpu, ctx.arena, 2, ptr)?;
    Ok(())
}

fn global_free(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let h_mem = peek_arg_u32(cpu, ctx.arena, 0)?;
    if h_mem != 0 {
        let _ = ctx.heap.free(ctx.arena, h_mem);
    }
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn global_lock(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let h_mem = peek_arg_u32(cpu, ctx.arena, 0)?;
    return_stdcall(cpu, ctx.arena, 1, h_mem)?;
    Ok(())
}

fn global_unlock(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn local_alloc(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let flags = peek_arg_u32(cpu, ctx.arena, 0)?;
    let bytes = peek_arg_u32(cpu, ctx.arena, 1)? as usize;
    let zeroed = (flags & LMEM_ZEROINIT) != 0;
    let ptr = ctx.heap.alloc(ctx.arena, bytes, zeroed).unwrap_or(0);
    return_stdcall(cpu, ctx.arena, 2, ptr)?;
    Ok(())
}

fn local_free(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let h_mem = peek_arg_u32(cpu, ctx.arena, 0)?;
    if h_mem != 0 {
        let _ = ctx.heap.free(ctx.arena, h_mem);
    }
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn initialize_critical_section(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn enter_critical_section(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let cs_addr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let tid = ctx.active_host_tid;
    let entry = ctx.win32.crit_sections.entry(cs_addr).or_insert((0, 0));
    if entry.0 == 0 || entry.0 == tid {
        entry.0 = tid;
        entry.1 = entry.1.saturating_add(1);
    } else {
        ctx.win32.pending_wait = Some(PendingHostWait::EnterCriticalSection { cs_addr });
    }
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn leave_critical_section(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let cs_addr = peek_arg_u32(cpu, ctx.arena, 0)?;
    if let Some(entry) = ctx.win32.crit_sections.get_mut(&cs_addr) {
        if entry.0 == ctx.active_host_tid && entry.1 > 0 {
            entry.1 -= 1;
            if entry.1 == 0 {
                entry.0 = 0;
                ctx.win32.notify_sync = true;
            }
        }
    }
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn delete_critical_section(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let cs_addr = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.win32.crit_sections.remove(&cs_addr);
    ctx.win32.notify_sync = true;
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn create_event_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let manual_reset = peek_arg_u32(cpu, ctx.arena, 1)? != 0;
    let initial_state = peek_arg_u32(cpu, ctx.arena, 2)? != 0;
    let handle = ctx.win32.alloc_handle(Win32Handle::Event {
        manual_reset,
        signaled: initial_state,
        pulse_seq: 0,
    });
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 4, handle)?;
    Ok(())
}

fn set_event(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let ok = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::Event { signaled, .. }) => {
            *signaled = true;
            ctx.win32.notify_sync = true;
            true
        }
        _ => false,
    };
    return_stdcall(cpu, ctx.arena, 1, u32::from(ok))?;
    Ok(())
}

fn reset_event(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let ok = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::Event { signaled, .. }) => {
            *signaled = false;
            true
        }
        _ => false,
    };
    return_stdcall(cpu, ctx.arena, 1, u32::from(ok))?;
    Ok(())
}

fn wait_for_single_object(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let timeout_ms = peek_arg_u32(cpu, ctx.arena, 1)?;
    let tid = ctx.active_host_tid;
    let result = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::Event {
            manual_reset,
            signaled,
            ..
        }) => {
            if *signaled {
                if !*manual_reset {
                    *signaled = false;
                }
                WAIT_OBJECT_0
            } else if timeout_ms == 0 {
                WAIT_TIMEOUT
            } else {
                ctx.win32.pending_wait = Some(PendingHostWait::WaitObject { handle, timeout_ms });
                WAIT_TIMEOUT
            }
        }
        Some(Win32Handle::Mutex {
            owner_tid,
            recursion,
        }) => {
            if *owner_tid == 0 || *owner_tid == tid {
                *owner_tid = tid;
                *recursion = recursion.saturating_add(1);
                WAIT_OBJECT_0
            } else if timeout_ms == 0 {
                WAIT_TIMEOUT
            } else {
                ctx.win32.pending_wait = Some(PendingHostWait::WaitObject { handle, timeout_ms });
                WAIT_TIMEOUT
            }
        }
        Some(Win32Handle::Thread { finished, .. }) => {
            if *finished {
                WAIT_OBJECT_0
            } else if timeout_ms == 0 {
                WAIT_TIMEOUT
            } else {
                ctx.win32.pending_wait = Some(PendingHostWait::WaitObject { handle, timeout_ms });
                WAIT_TIMEOUT
            }
        }
        _ => WAIT_OBJECT_0,
    };
    return_stdcall(cpu, ctx.arena, 2, result)?;
    Ok(())
}

fn tls_alloc(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let slot = ctx.win32.tls_alloc();
    if slot == TLS_OUT_OF_INDEXES {
        *ctx.last_error = ERROR_NOT_ENOUGH_MEMORY;
    } else {
        *ctx.last_error = ERROR_SUCCESS;
    }
    return_stdcall(cpu, ctx.arena, 0, slot)?;
    Ok(())
}

fn tls_get_value(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let index = peek_arg_u32(cpu, ctx.arena, 0)?;
    match ctx.win32.tls_get(index) {
        Some(val) => {
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 1, val)?;
        }
        None => {
            *ctx.last_error = ERROR_INVALID_PARAMETER;
            return_stdcall(cpu, ctx.arena, 1, 0)?;
        }
    }
    Ok(())
}

fn tls_set_value(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let index = peek_arg_u32(cpu, ctx.arena, 0)?;
    let value = peek_arg_u32(cpu, ctx.arena, 1)?;
    let ok = ctx.win32.tls_set(index, value);
    *ctx.last_error = if ok {
        ERROR_SUCCESS
    } else {
        ERROR_INVALID_PARAMETER
    };
    return_stdcall(cpu, ctx.arena, 2, u32::from(ok))?;
    Ok(())
}

fn tls_free(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let index = peek_arg_u32(cpu, ctx.arena, 0)?;
    let ok = ctx.win32.tls_free(index);
    *ctx.last_error = if ok {
        ERROR_SUCCESS
    } else {
        ERROR_INVALID_PARAMETER
    };
    return_stdcall(cpu, ctx.arena, 1, u32::from(ok))?;
    Ok(())
}

fn get_current_thread_id(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let tid = u32::try_from(ctx.active_host_tid).unwrap_or(CURRENT_THREAD_ID);
    return_stdcall(cpu, ctx.arena, 0, tid)?;
    Ok(())
}

fn get_current_process_id(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 0, CURRENT_PROCESS_ID)?;
    Ok(())
}

fn get_current_process(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 0, CURRENT_PROCESS_HANDLE)?;
    Ok(())
}

fn get_current_thread(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 0, CURRENT_THREAD_HANDLE)?;
    Ok(())
}

fn interlocked_increment(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let next = ctx.arena.read_i32(ptr)?.wrapping_add(1);
    ctx.arena.write_u32(ptr, next as u32)?;
    return_stdcall(cpu, ctx.arena, 1, next as u32)?;
    Ok(())
}

fn interlocked_decrement(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let next = ctx.arena.read_i32(ptr)?.wrapping_sub(1);
    ctx.arena.write_u32(ptr, next as u32)?;
    return_stdcall(cpu, ctx.arena, 1, next as u32)?;
    Ok(())
}

fn interlocked_exchange(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let new_val = peek_arg_u32(cpu, ctx.arena, 1)?;
    let prev = ctx.arena.read_u32(ptr)?;
    ctx.arena.write_u32(ptr, new_val)?;
    return_stdcall(cpu, ctx.arena, 2, prev)?;
    Ok(())
}

fn query_performance_frequency(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.arena.write_u64(ptr, QPC_FREQUENCY_HZ)?;
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn query_performance_counter(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let nanos = ctx.host_start_instant.elapsed().as_nanos() as u64;
    ctx.arena.write_u64(ptr, nanos)?;
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn get_tick_count(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let millis = ctx.host_start_instant.elapsed().as_millis() as u32;
    return_stdcall(cpu, ctx.arena, 0, millis)?;
    Ok(())
}

fn sleep_ms(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ms = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.win32.pending_wait = Some(PendingHostWait::SleepMs(ms));
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn get_local_time(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    write_system_time(ctx.arena, ptr, true)?;
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn get_system_time(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    write_system_time(ctx.arena, ptr, false)?;
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn write_system_time(
    arena: &mut crate::arena::GuestArena,
    ptr: GuestAddr,
    local: bool,
) -> Result<(), RuntimeError> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
        if local {
            libc::localtime_r(&ts.tv_sec, &mut tm);
        } else {
            libc::gmtime_r(&ts.tv_sec, &mut tm);
        }
    }

    let millis = (ts.tv_nsec / 1_000_000) as u16;
    arena.write_u16(ptr, (tm.tm_year + 1900) as u16)?;
    arena.write_u16(ptr + 2, (tm.tm_mon + 1) as u16)?;
    arena.write_u16(ptr + 4, tm.tm_wday as u16)?;
    arena.write_u16(ptr + 6, tm.tm_mday as u16)?;
    arena.write_u16(ptr + 8, tm.tm_hour as u16)?;
    arena.write_u16(ptr + 10, tm.tm_min as u16)?;
    arena.write_u16(ptr + 12, tm.tm_sec.min(59) as u16)?;
    arena.write_u16(ptr + 14, millis)?;
    Ok(())
}

fn time_get_time(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let millis = ctx.host_start_instant.elapsed().as_millis() as u32;
    return_stdcall(cpu, ctx.arena, 0, millis)?;
    Ok(())
}

fn time_begin_period(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn time_end_period(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn get_module_handle_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let name_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    if name_ptr == 0 {
        *ctx.last_error = ERROR_SUCCESS;
        return_stdcall(cpu, ctx.arena, 1, ctx.pe_image_base)?;
        return Ok(());
    }

    let name = ctx.arena.read_cstr(name_ptr, MAX_PATH_LEN)?;
    if name.eq_ignore_ascii_case(&ctx.win32.config.exe_name) {
        *ctx.last_error = ERROR_SUCCESS;
        return_stdcall(cpu, ctx.arena, 1, ctx.pe_image_base)?;
        return Ok(());
    }

    let handle = ctx.win32.get_or_alloc_module(&name);
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 1, handle)?;
    Ok(())
}

fn load_library_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let name_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let name = ctx.arena.read_cstr(name_ptr, MAX_PATH_LEN)?;
    let handle = ctx.win32.get_or_alloc_module(&name);
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 1, handle)?;
    Ok(())
}

fn free_library(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn get_proc_address(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let h_module = peek_arg_u32(cpu, ctx.arena, 0)?;
    let proc_arg = peek_arg_u32(cpu, ctx.arena, 1)?;

    let Some(dll_name) = ctx.win32.module_name_for_handle(h_module, ctx.pe_image_base) else {
        *ctx.last_error = ERROR_INVALID_HANDLE;
        return_stdcall(cpu, ctx.arena, 2, 0)?;
        return Ok(());
    };

    let symbol = if (proc_arg >> 16) == 0 {
        ImportSymbol::Ordinal(proc_arg as u16)
    } else {
        let name = ctx.arena.read_cstr(proc_arg, MAX_PATH_LEN)?;
        ImportSymbol::Name(name)
    };

    if let Some(thunk_va) = ctx.register_dynamic_import(&dll_name, symbol) {
        *ctx.last_error = ERROR_SUCCESS;
        return_stdcall(cpu, ctx.arena, 2, thunk_va)?;
    } else {
        *ctx.last_error = ERROR_PROC_NOT_FOUND;
        return_stdcall(cpu, ctx.arena, 2, 0)?;
    }
    Ok(())
}

fn get_startup_info_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let info_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.arena.zero_range(info_ptr, STARTUPINFOA_SIZE)?;
    ctx.arena.write_u32(info_ptr, STARTUPINFOA_SIZE as u32)?;
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn get_command_line_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = match ctx.win32.command_line_guest {
        Some(existing) => existing,
        None => {
            let cmd = ctx.win32.config.command_line.clone();
            let allocated = ctx
                .heap
                .alloc(ctx.arena, cmd.len() + 1, true)
                .unwrap_or(0);
            if allocated != 0 {
                ctx.arena.write_cstr(allocated, &cmd)?;
                ctx.win32.command_line_guest = Some(allocated);
            }
            allocated
        }
    };
    return_stdcall(cpu, ctx.arena, 0, ptr)?;
    Ok(())
}

fn get_version(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 0, WIN_XP_VERSION)?;
    Ok(())
}

fn get_version_ex_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let cb_size = ctx.arena.read_u32(ptr)?;
    let clear_len = (cb_size as usize).clamp(20, OSVERSIONINFOA_SIZE);
    ctx.arena.zero_range(ptr, clear_len)?;
    ctx.arena.write_u32(ptr, cb_size)?;
    ctx.arena.write_u32(ptr + 4, 5)?;
    ctx.arena.write_u32(ptr + 8, 1)?;
    ctx.arena.write_u32(ptr + 12, 2600)?;
    ctx.arena.write_u32(ptr + 16, VER_PLATFORM_WIN32_NT)?;
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn get_system_info(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.arena.zero_range(ptr, SYSTEM_INFO_SIZE)?;
    ctx.arena.write_u16(ptr, 0)?;
    ctx.arena.write_u32(ptr + 4, 4096)?;
    ctx.arena.write_u32(ptr + 8, 0x0001_0000)?;
    ctx.arena.write_u32(ptr + 12, 0x7FFF_FFFF)?;
    ctx.arena.write_u32(ptr + 16, 1)?;
    ctx.arena.write_u32(ptr + 20, 1)?;
    ctx.arena.write_u32(ptr + 24, 586)?;
    ctx.arena.write_u32(ptr + 28, 0x0001_0000)?;
    ctx.arena.write_u16(ptr + 32, 6)?;
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn get_acp(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 0, CODEPAGE_WIN1252)?;
    Ok(())
}

fn get_std_handle(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let std_id = peek_arg_u32(cpu, ctx.arena, 0)?;
    let handle = match std_id {
        STD_INPUT_HANDLE => STD_IN_HOST_HANDLE,
        STD_OUTPUT_HANDLE => STD_OUT_HOST_HANDLE,
        STD_ERROR_HANDLE => STD_ERR_HOST_HANDLE,
        _ => {
            *ctx.last_error = ERROR_INVALID_PARAMETER;
            INVALID_HANDLE_VALUE
        }
    };
    return_stdcall(cpu, ctx.arena, 1, handle)?;
    Ok(())
}

fn get_file_type(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let file_type = if handle == STD_IN_HOST_HANDLE
        || handle == STD_OUT_HOST_HANDLE
        || handle == STD_ERR_HOST_HANDLE
    {
        FILE_TYPE_CHAR
    } else if matches!(ctx.win32.get_handle(handle), Some(Win32Handle::File { .. })) {
        FILE_TYPE_DISK
    } else {
        FILE_TYPE_UNKNOWN
    };
    return_stdcall(cpu, ctx.arena, 1, file_type)?;
    Ok(())
}

fn get_environment_strings(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = match ctx.win32.env_strings_guest {
        Some(existing) => existing,
        None => {
            let env = b"PATH=C:\\\0\0";
            let allocated = ctx.heap.alloc(ctx.arena, env.len(), true).unwrap_or(0);
            if allocated != 0 {
                ctx.arena.write_bytes(allocated, env)?;
                ctx.win32.env_strings_guest = Some(allocated);
            }
            allocated
        }
    };
    return_stdcall(cpu, ctx.arena, 0, ptr)?;
    Ok(())
}

fn free_environment_strings_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn multi_byte_to_wide_char(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let mb_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let mb_len = peek_arg_i32(cpu, ctx.arena, 3)?;
    let wc_ptr = peek_arg_u32(cpu, ctx.arena, 4)?;
    let wc_cap = peek_arg_i32(cpu, ctx.arena, 5)?;

    if mb_ptr == 0 || mb_len == 0 {
        *ctx.last_error = ERROR_INVALID_PARAMETER;
        return_stdcall(cpu, ctx.arena, 6, 0)?;
        return Ok(());
    }

    let raw_bytes = if mb_len < 0 {
        let mut s = ctx.arena.read_cstr(mb_ptr, MAX_DEBUG_STR_LEN)?.into_bytes();
        s.push(0);
        s
    } else {
        ctx.arena.read_bytes(mb_ptr, mb_len as usize)?.to_vec()
    };

    let text = String::from_utf8_lossy(&raw_bytes);
    let utf16: Vec<u16> = text.encode_utf16().collect();

    if wc_cap == 0 {
        return_stdcall(cpu, ctx.arena, 6, utf16.len() as u32)?;
        return Ok(());
    }

    let cap = wc_cap.max(0) as usize;
    if cap < utf16.len() {
        *ctx.last_error = ERROR_INSUFFICIENT_BUFFER;
        return_stdcall(cpu, ctx.arena, 6, 0)?;
        return Ok(());
    }

    for (idx, &unit) in utf16.iter().enumerate() {
        ctx.arena.write_u16(wc_ptr + (idx as u32) * 2, unit)?;
    }
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 6, utf16.len() as u32)?;
    Ok(())
}

fn wide_char_to_multi_byte(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let wc_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let wc_len = peek_arg_i32(cpu, ctx.arena, 3)?;
    let mb_ptr = peek_arg_u32(cpu, ctx.arena, 4)?;
    let mb_cap = peek_arg_i32(cpu, ctx.arena, 5)?;

    if wc_ptr == 0 || wc_len == 0 {
        *ctx.last_error = ERROR_INVALID_PARAMETER;
        return_stdcall(cpu, ctx.arena, 8, 0)?;
        return Ok(());
    }

    let mut units = Vec::new();
    if wc_len < 0 {
        let mut off = 0u32;
        while units.len() < MAX_DEBUG_STR_LEN {
            let unit = ctx.arena.read_u16(wc_ptr + off)?;
            units.push(unit);
            if unit == 0 {
                break;
            }
            off += 2;
        }
    } else {
        for idx in 0..(wc_len as u32) {
            units.push(ctx.arena.read_u16(wc_ptr + idx * 2)?);
        }
    }

    let decoded = String::from_utf16_lossy(&units);
    let out_bytes = decoded.as_bytes();

    if mb_cap == 0 {
        return_stdcall(cpu, ctx.arena, 8, out_bytes.len() as u32)?;
        return Ok(());
    }

    let cap = mb_cap.max(0) as usize;
    if cap < out_bytes.len() {
        *ctx.last_error = ERROR_INSUFFICIENT_BUFFER;
        return_stdcall(cpu, ctx.arena, 8, 0)?;
        return Ok(());
    }

    ctx.arena.write_bytes(mb_ptr, out_bytes)?;
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 8, out_bytes.len() as u32)?;
    Ok(())
}

fn get_last_error(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let err = *ctx.last_error;
    return_stdcall(cpu, ctx.arena, 0, err)?;
    Ok(())
}

fn set_last_error(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let err = peek_arg_u32(cpu, ctx.arena, 0)?;
    *ctx.last_error = err;
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn set_unhandled_exception_filter(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let new_filter = peek_arg_u32(cpu, ctx.arena, 0)?;
    let prev = ctx.win32.unhandled_exception_filter;
    ctx.win32.unhandled_exception_filter = new_filter;
    return_stdcall(cpu, ctx.arena, 1, prev)?;
    Ok(())
}

fn is_processor_feature_present(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let feature = peek_arg_u32(cpu, ctx.arena, 0)?;
    let supported = matches!(
        feature,
        PF_FLOATING_POINT_PRECISION_ERRATA
            | PF_COMPARE_EXCHANGE_DOUBLE
            | PF_MMX_INSTRUCTIONS_AVAILABLE
            | PF_XMMI_INSTRUCTIONS_AVAILABLE
            | PF_RDTSC_INSTRUCTION_AVAILABLE
            | PF_XMMI64_INSTRUCTIONS_AVAILABLE
    ) && feature != PF_FLOATING_POINT_EMULATED;
    return_stdcall(cpu, ctx.arena, 1, u32::from(supported))?;
    Ok(())
}

fn output_debug_string_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let str_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    if str_ptr != 0 {
        let _ = ctx.arena.read_cstr(str_ptr, MAX_DEBUG_STR_LEN);
    }
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn exit_process(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let code = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.win32.exit_code = Some(code);
    cpu.exit_reason = ExitReason::Halt as u32;
    cpu.exit_detail = code;
    return_stdcall(cpu, ctx.arena, 1, code)?;
    Ok(())
}

fn lstrlen_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let len = if ptr == 0 {
        0
    } else {
        ctx.arena.read_cstr(ptr, MAX_DEBUG_STR_LEN)?.len() as u32
    };
    return_stdcall(cpu, ctx.arena, 1, len)?;
    Ok(())
}

fn lstrcpy_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let dst = peek_arg_u32(cpu, ctx.arena, 0)?;
    let src = peek_arg_u32(cpu, ctx.arena, 1)?;
    let text = ctx.arena.read_cstr(src, MAX_DEBUG_STR_LEN)?;
    ctx.arena.write_cstr(dst, &text)?;
    return_stdcall(cpu, ctx.arena, 2, dst)?;
    Ok(())
}

fn lstrcat_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let dst = peek_arg_u32(cpu, ctx.arena, 0)?;
    let src = peek_arg_u32(cpu, ctx.arena, 1)?;
    let dst_len = ctx.arena.read_cstr(dst, MAX_DEBUG_STR_LEN)?.len() as u32;
    let src_text = ctx.arena.read_cstr(src, MAX_DEBUG_STR_LEN)?;
    ctx.arena.write_cstr(dst + dst_len, &src_text)?;
    return_stdcall(cpu, ctx.arena, 2, dst)?;
    Ok(())
}

fn lstrcmpi_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let a_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let b_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let a = ctx.arena.read_cstr(a_ptr, MAX_DEBUG_STR_LEN)?;
    let b = ctx.arena.read_cstr(b_ptr, MAX_DEBUG_STR_LEN)?;
    let ord = a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase());
    let val = match ord {
        std::cmp::Ordering::Less => -1i32 as u32,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    return_stdcall(cpu, ctx.arena, 2, val)?;
    Ok(())
}

fn create_file_w(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let name_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let desired_access = peek_arg_u32(cpu, ctx.arena, 1)?;
    let disposition = peek_arg_u32(cpu, ctx.arena, 4)?;

    let mut units = Vec::new();
    let mut off = 0u32;
    while units.len() < MAX_PATH_LEN {
        let u = ctx.arena.read_u16(name_ptr + off)?;
        if u == 0 {
            break;
        }
        units.push(u);
        off += 2;
    }
    let win_path = String::from_utf16_lossy(&units);
    let host_path = ctx.win32.resolve_path(&win_path);
    let existed = host_path.exists();

    let want_read = (desired_access & GENERIC_READ) != 0 || desired_access == 0;
    let want_write = (desired_access & GENERIC_WRITE) != 0;

    let mut opts = OpenOptions::new();
    match disposition {
        CREATE_NEW => {
            opts.read(want_read).write(true).create_new(true);
        }
        CREATE_ALWAYS => {
            opts.read(want_read).write(true).create(true).truncate(true);
        }
        OPEN_EXISTING => {
            opts.read(want_read || !want_write).write(want_write);
        }
        OPEN_ALWAYS => {
            opts.read(want_read || !want_write)
                .write(want_write)
                .create(true);
        }
        TRUNCATE_EXISTING => {
            opts.read(want_read).write(true).truncate(true);
        }
        _ => {
            *ctx.last_error = ERROR_INVALID_PARAMETER;
            return_stdcall(cpu, ctx.arena, 7, INVALID_HANDLE_VALUE)?;
            return Ok(());
        }
    }

    match opts.open(&host_path) {
        Ok(file) => {
            *ctx.last_error = if existed && (disposition == CREATE_ALWAYS || disposition == OPEN_ALWAYS) {
                ERROR_ALREADY_EXISTS
            } else {
                ERROR_SUCCESS
            };
            let handle = ctx.win32.alloc_handle(Win32Handle::File {
                file,
                path: host_path,
            });
            return_stdcall(cpu, ctx.arena, 7, handle)?;
        }
        Err(err) => {
            *ctx.last_error = map_io_error(&err);
            return_stdcall(cpu, ctx.arena, 7, INVALID_HANDLE_VALUE)?;
        }
    }
    Ok(())
}

fn global_memory_status(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    const PHYS_512_MIB: u32 = 512 * 1024 * 1024;
    const AVAIL_384_MIB: u32 = 384 * 1024 * 1024;
    const PAGE_1_GIB: u32 = 1024 * 1024 * 1024;
    const VIRT_2_GIB: u32 = 0x7FFF_0000;
    ctx.arena.write_u32(ptr, 32)?;
    ctx.arena.write_u32(ptr + 4, 25)?;
    ctx.arena.write_u32(ptr + 8, PHYS_512_MIB)?;
    ctx.arena.write_u32(ptr + 12, AVAIL_384_MIB)?;
    ctx.arena.write_u32(ptr + 16, PAGE_1_GIB)?;
    ctx.arena.write_u32(ptr + 20, PAGE_1_GIB)?;
    ctx.arena.write_u32(ptr + 24, VIRT_2_GIB)?;
    ctx.arena.write_u32(ptr + 28, VIRT_2_GIB)?;
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn create_mutex_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let initial_owner = peek_arg_u32(cpu, ctx.arena, 1)? != 0;
    let handle = ctx.win32.alloc_handle(Win32Handle::Mutex {
        owner_tid: if initial_owner {
            ctx.active_host_tid
        } else {
            0
        },
        recursion: u32::from(initial_owner),
    });
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 3, handle)?;
    Ok(())
}

fn release_mutex(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let tid = ctx.active_host_tid;
    let ok = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::Mutex {
            owner_tid,
            recursion,
        }) if *owner_tid == tid && *recursion > 0 => {
            *recursion -= 1;
            if *recursion == 0 {
                *owner_tid = 0;
                ctx.win32.notify_sync = true;
            }
            true
        }
        _ => false,
    };
    return_stdcall(cpu, ctx.arena, 1, u32::from(ok))?;
    Ok(())
}

const UNIX_EPOCH_TO_FILETIME_100NS: u64 = 116_444_736_000_000_000;

fn get_system_time_as_file_time(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let ft = UNIX_EPOCH_TO_FILETIME_100NS + (nanos / 100);
    ctx.arena.write_u32(ptr, ft as u32)?;
    ctx.arena.write_u32(ptr + 4, (ft >> 32) as u32)?;
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn file_time_to_local_file_time(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let in_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let out_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let ft = ctx.arena.read_u64(in_ptr)?;
    ctx.arena.write_u64(out_ptr, ft)?;
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn file_time_to_system_time(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let in_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let out_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let ft = ctx.arena.read_u64(in_ptr)?;
    let unix_100ns = ft.saturating_sub(UNIX_EPOCH_TO_FILETIME_100NS);
    let secs = (unix_100ns / 10_000_000) as libc::time_t;
    let millis = ((unix_100ns % 10_000_000) / 10_000) as u16;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::gmtime_r(&secs, &mut tm);
    }
    ctx.arena.write_u16(out_ptr, (tm.tm_year + 1900) as u16)?;
    ctx.arena.write_u16(out_ptr + 2, (tm.tm_mon + 1) as u16)?;
    ctx.arena.write_u16(out_ptr + 4, tm.tm_wday as u16)?;
    ctx.arena.write_u16(out_ptr + 6, tm.tm_mday as u16)?;
    ctx.arena.write_u16(out_ptr + 8, tm.tm_hour as u16)?;
    ctx.arena.write_u16(out_ptr + 10, tm.tm_min as u16)?;
    ctx.arena.write_u16(out_ptr + 12, tm.tm_sec.min(59) as u16)?;
    ctx.arena.write_u16(out_ptr + 14, millis)?;
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn system_time_to_file_time(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let out_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    ctx.arena.write_u64(out_ptr, UNIX_EPOCH_TO_FILETIME_100NS)?;
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn get_thread_locale(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 0, 0x0409)?;
    Ok(())
}

fn get_locale_info_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let buf_len = peek_arg_i32(cpu, ctx.arena, 3)?;
    let val = "1";
    if buf_ptr == 0 || buf_len == 0 {
        return_stdcall(cpu, ctx.arena, 4, (val.len() + 1) as u32)?;
        return Ok(());
    }
    if (buf_len as usize) < val.len() + 1 {
        *ctx.last_error = ERROR_INSUFFICIENT_BUFFER;
        return_stdcall(cpu, ctx.arena, 4, 0)?;
        return Ok(());
    }
    ctx.arena.write_cstr(buf_ptr, val)?;
    return_stdcall(cpu, ctx.arena, 4, (val.len() + 1) as u32)?;
    Ok(())
}

fn get_drive_type_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    const DRIVE_NO_ROOT_DIR: u32 = 1;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_CDROM: u32 = 5;

    let root_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let drive_type = if root_ptr == 0 {
        DRIVE_FIXED
    } else {
        let root = ctx.arena.read_cstr(root_ptr, MAX_PATH_LEN)?;
        match root.as_bytes().first().map(u8::to_ascii_lowercase) {
            Some(b'c') => DRIVE_FIXED,
            Some(b'd') => DRIVE_CDROM,
            _ => DRIVE_NO_ROOT_DIR,
        }
    };
    return_stdcall(cpu, ctx.arena, 1, drive_type)?;
    Ok(())
}

fn get_disk_free_space_ex_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let avail_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let total_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let free_ptr = peek_arg_u32(cpu, ctx.arena, 3)?;
    const TWO_GIB: u64 = 2u64 * 1024 * 1024 * 1024;
    if avail_ptr != 0 {
        ctx.arena.write_u64(avail_ptr, TWO_GIB)?;
    }
    if total_ptr != 0 {
        ctx.arena.write_u64(total_ptr, TWO_GIB)?;
    }
    if free_ptr != 0 {
        ctx.arena.write_u64(free_ptr, TWO_GIB)?;
    }
    return_stdcall(cpu, ctx.arena, 4, 1)?;
    Ok(())
}

fn create_file_mapping_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let h_file = peek_arg_u32(cpu, ctx.arena, 0)?;
    let max_low = peek_arg_u32(cpu, ctx.arena, 4)? as usize;

    let bytes = if h_file == INVALID_HANDLE_VALUE {
        vec![0u8; max_low.max(4096)]
    } else if let Some(Win32Handle::File { file, .. }) = ctx.win32.get_handle_mut(h_file) {
        let saved = file.stream_position().unwrap_or(0);
        let _ = file.seek(SeekFrom::Start(0));
        let mut buf = Vec::new();
        let _ = file.read_to_end(&mut buf);
        let _ = file.seek(SeekFrom::Start(saved));
        buf
    } else {
        *ctx.last_error = ERROR_INVALID_HANDLE;
        return_stdcall(cpu, ctx.arena, 6, 0)?;
        return Ok(());
    };

    let handle = ctx.win32.alloc_handle(Win32Handle::FileMapping { bytes });
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 6, handle)?;
    Ok(())
}

fn map_view_of_file(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let h_map = peek_arg_u32(cpu, ctx.arena, 0)?;
    let off_low = peek_arg_u32(cpu, ctx.arena, 3)? as usize;
    let num_bytes = peek_arg_u32(cpu, ctx.arena, 4)? as usize;

    let slice = match ctx.win32.get_handle(h_map) {
        Some(Win32Handle::FileMapping { bytes }) => {
            let start = off_low.min(bytes.len());
            let end = if num_bytes == 0 {
                bytes.len()
            } else {
                start.saturating_add(num_bytes).min(bytes.len())
            };
            bytes[start..end].to_vec()
        }
        _ => {
            *ctx.last_error = ERROR_INVALID_HANDLE;
            return_stdcall(cpu, ctx.arena, 5, 0)?;
            return Ok(());
        }
    };

    let alloc_len = slice.len().max(1);
    let Ok(base) = ctx
        .vmem
        .virtual_alloc(ctx.arena, None, alloc_len, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE)
    else {
        *ctx.last_error = ERROR_NOT_ENOUGH_MEMORY;
        return_stdcall(cpu, ctx.arena, 5, 0)?;
        return Ok(());
    };
    if !slice.is_empty() {
        ctx.arena.write_bytes(base, &slice)?;
    }
    ctx.win32.mapped_views.insert(base, alloc_len);
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 5, base)?;
    Ok(())
}

fn unmap_view_of_file(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let base = peek_arg_u32(cpu, ctx.arena, 0)?;
    if ctx.win32.mapped_views.remove(&base).is_some() {
        let _ = ctx.vmem.virtual_free(ctx.arena, base, 0, MEM_RELEASE);
    }
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn find_resource_null(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    *ctx.last_error = ERROR_FILE_NOT_FOUND;
    return_stdcall(cpu, ctx.arena, 3, 0)?;
    Ok(())
}

fn load_resource(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, 0)?;
    Ok(())
}

fn lock_resource(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn sizeof_resource(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, 0)?;
    Ok(())
}

fn set_file_attributes_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn copy_file_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let src_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let dst_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let fail_if_exists = peek_arg_u32(cpu, ctx.arena, 2)? != 0;
    let src = ctx.win32.resolve_path(&ctx.arena.read_cstr(src_ptr, MAX_PATH_LEN)?);
    let dst = ctx.win32.resolve_path(&ctx.arena.read_cstr(dst_ptr, MAX_PATH_LEN)?);

    if fail_if_exists && dst.exists() {
        *ctx.last_error = ERROR_ALREADY_EXISTS;
        return_stdcall(cpu, ctx.arena, 3, 0)?;
        return Ok(());
    }
    match std::fs::copy(&src, &dst) {
        Ok(_) => {
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 3, 1)?;
        }
        Err(err) => {
            *ctx.last_error = map_io_error(&err);
            return_stdcall(cpu, ctx.arena, 3, 0)?;
        }
    }
    Ok(())
}

fn remove_directory_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let path_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let path = ctx.win32.resolve_path(&ctx.arena.read_cstr(path_ptr, MAX_PATH_LEN)?);
    match std::fs::remove_dir(&path) {
        Ok(()) => {
            *ctx.last_error = ERROR_SUCCESS;
            return_stdcall(cpu, ctx.arena, 1, 1)?;
        }
        Err(err) => {
            *ctx.last_error = map_io_error(&err);
            return_stdcall(cpu, ctx.arena, 1, 0)?;
        }
    }
    Ok(())
}

fn get_overlapped_result(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let overlapped_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let bytes_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    if bytes_ptr != 0 {
        let transferred = if overlapped_ptr != 0 {
            ctx.arena.read_u32(overlapped_ptr + 4)?
        } else {
            0
        };
        ctx.arena.write_u32(bytes_ptr, transferred)?;
    }
    *ctx.last_error = ERROR_SUCCESS;
    return_stdcall(cpu, ctx.arena, 4, 1)?;
    Ok(())
}

pub(crate) fn alloc_and_queue_guest_thread(
    ctx: &mut RuntimeContext<'_>,
    start_va: u32,
    param: u32,
    suspended: bool,
) -> Result<Option<u32>, RuntimeError> {
    let Ok(stack_base) = ctx.vmem.virtual_alloc(
        ctx.arena,
        None,
        GUEST_THREAD_STACK_SIZE,
        MEM_RESERVE | MEM_COMMIT,
        PAGE_READWRITE,
    ) else {
        *ctx.last_error = ERROR_NOT_ENOUGH_MEMORY;
        return Ok(None);
    };
    let stack_top = stack_base.wrapping_add(GUEST_THREAD_STACK_SIZE as u32);
    let teb_va = ctx.heap.alloc(ctx.arena, GUEST_THREAD_TEB_SIZE, true)?;
    let handle = ctx.win32.alloc_handle(Win32Handle::Thread {
        exit_code: STILL_ACTIVE,
        finished: false,
        suspended,
    });
    ctx.arena.write_u32(teb_va, 0xFFFF_FFFF)?;
    ctx.arena.write_u32(teb_va + 4, stack_top)?;
    ctx.arena.write_u32(teb_va + 8, stack_base)?;
    ctx.arena.write_u32(teb_va + 0x18, teb_va)?;
    ctx.arena.write_u32(teb_va + 0x24, handle)?;

    ctx.win32.pending_threads.push(PendingThreadSpawn {
        handle,
        start_va,
        param,
        stack_top,
        teb_va,
    });
    *ctx.last_error = ERROR_SUCCESS;
    Ok(Some(handle))
}

fn create_thread(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let start_va = peek_arg_u32(cpu, ctx.arena, 2)?;
    let param = peek_arg_u32(cpu, ctx.arena, 3)?;
    let flags = peek_arg_u32(cpu, ctx.arena, 4)?;
    let id_ptr = peek_arg_u32(cpu, ctx.arena, 5)?;

    let suspended = (flags & CREATE_SUSPENDED) != 0;
    let Some(handle) = alloc_and_queue_guest_thread(ctx, start_va, param, suspended)? else {
        return_stdcall(cpu, ctx.arena, 6, 0)?;
        return Ok(());
    };
    if id_ptr != 0 {
        ctx.arena.write_u32(id_ptr, handle)?;
    }
    return_stdcall(cpu, ctx.arena, 6, handle)?;
    Ok(())
}

fn exit_thread(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let code = peek_arg_u32(cpu, ctx.arena, 0)?;
    cpu.exit_reason = ExitReason::Halt as u32;
    cpu.exit_detail = code;
    return_stdcall(cpu, ctx.arena, 1, code)?;
    Ok(())
}

fn get_exit_code_thread(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let code_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let code = match ctx.win32.get_handle(handle) {
        Some(Win32Handle::Thread {
            exit_code,
            finished,
            ..
        }) => {
            if *finished {
                *exit_code
            } else {
                STILL_ACTIVE
            }
        }
        _ => 0,
    };
    if code_ptr != 0 {
        ctx.arena.write_u32(code_ptr, code)?;
    }
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn set_thread_priority(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn get_thread_priority(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn set_thread_priority_boost(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn resume_thread(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let prev = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::Thread { suspended, .. }) => {
            let was = u32::from(*suspended);
            *suspended = false;
            ctx.win32.notify_sync = true;
            was
        }
        _ => 0,
    };
    return_stdcall(cpu, ctx.arena, 1, prev)?;
    Ok(())
}

fn suspend_thread(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let prev = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::Thread { suspended, .. }) => {
            let was = u32::from(*suspended);
            *suspended = true;
            if handle as u64 == ctx.active_host_tid {
                ctx.win32.pending_wait = Some(PendingHostWait::SuspendSelf { handle });
            }
            was
        }
        _ => 0,
    };
    return_stdcall(cpu, ctx.arena, 1, prev)?;
    Ok(())
}

fn pulse_event(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let handle = peek_arg_u32(cpu, ctx.arena, 0)?;
    let ok = match ctx.win32.get_handle_mut(handle) {
        Some(Win32Handle::Event {
            signaled,
            pulse_seq,
            ..
        }) => {
            *signaled = false;
            *pulse_seq = pulse_seq.wrapping_add(1);
            ctx.win32.notify_sync = true;
            true
        }
        _ => false,
    };
    return_stdcall(cpu, ctx.arena, 1, u32::from(ok))?;
    Ok(())
}

fn gdi_delete_object(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn time_set_event(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let delay_ms = peek_arg_u32(cpu, ctx.arena, 0)?;
    let callback_va = peek_arg_u32(cpu, ctx.arena, 2)?;
    let dw_user = peek_arg_u32(cpu, ctx.arena, 3)?;
    let fu_event = peek_arg_u32(cpu, ctx.arena, 4)?;
    if callback_va == 0 {
        return_stdcall(cpu, ctx.arena, 5, 0)?;
        return Ok(());
    }
    let timer_id = ctx.win32.next_timer_id;
    ctx.win32.next_timer_id = ctx.win32.next_timer_id.wrapping_add(1).max(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    ctx.win32.timers.insert(timer_id, Arc::clone(&cancelled));
    ctx.win32.pending_timers.push(PendingTimerSpawn {
        timer_id,
        delay_ms: delay_ms.max(1),
        callback_va,
        dw_user,
        periodic: (fu_event & TIME_PERIODIC) != 0,
        cancelled,
    });
    return_stdcall(cpu, ctx.arena, 5, timer_id)?;
    Ok(())
}

fn time_kill_event(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let timer_id = peek_arg_u32(cpu, ctx.arena, 0)?;
    if let Some(cancelled) = ctx.win32.timers.remove(&timer_id) {
        cancelled.store(true, Ordering::Release);
        ctx.win32.notify_sync = true;
    }
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}
