use toluca_core::GuestAddr;
use toluca_core::cpu::{Gpr, GuestCpuState};

use crate::callconv::{peek_arg_i32, peek_arg_u32, return_cdecl, return_stdcall};
use crate::runtime::{ImportHandler, ImportRegistry, RuntimeContext, RuntimeError};
use crate::win32::msvcrt::format_printf;
use crate::win32::sdl2::SdlTranslatedEvent;
use crate::win32::{Win32Msg, WindowClassInfo, WindowRecord};

const MAX_CLASS_LEN: usize = 256;
const MAX_TITLE_LEN: usize = 512;

const DEFAULT_SCREEN_WIDTH: i32 = 640;
const DEFAULT_SCREEN_HEIGHT: i32 = 480;
const CW_USEDEFAULT: i32 = i32::MIN;

const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_ACTIVATE: u32 = 0x0006;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_CLOSE: u32 = 0x0010;
const WM_QUIT: u32 = 0x0012;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;

const PM_REMOVE: u32 = 0x0001;
const SWP_NOSIZE: u32 = 0x0001;
const GWL_WNDPROC: i32 = -4;
const GWL_USERDATA: i32 = -21;

const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const SM_CYCAPTION: i32 = 4;
const SM_CXBORDER: i32 = 5;
const SM_CYBORDER: i32 = 6;
const SM_CXFRAME: i32 = 32;
const SM_CYFRAME: i32 = 33;

const SYNTHETIC_CURSOR_HANDLE: u32 = 0x0001_0001;
const SYNTHETIC_ICON_HANDLE: u32 = 0x0001_0002;
const SYNTHETIC_GDI_STOCK_BASE: u32 = 0x0002_0000;
const IDOK: u32 = 1;
const KEY_PRESSED_MASK: u32 = 0x0000_8000;

const GDI_HORZRES: i32 = 8;
const GDI_VERTRES: i32 = 10;
const GDI_BITSPIXEL: i32 = 12;
const GDI_PLANES: i32 = 14;
const GDI_LOGPIXELSX: i32 = 88;
const GDI_LOGPIXELSY: i32 = 90;

pub fn register_imports(registry: &mut ImportRegistry) {
    let u32_dll = "user32.dll";
    let entries: &[(&str, ImportHandler)] = &[
        ("RegisterClassA", register_class_a),
        ("RegisterClassExA", register_class_ex_a),
        ("UnregisterClassA", unregister_class_a),
        ("CreateWindowExA", create_window_ex_a),
        ("DestroyWindow", destroy_window),
        ("ShowWindow", show_window),
        ("UpdateWindow", update_window),
        ("SetFocus", set_focus),
        ("SetForegroundWindow", set_foreground_window),
        ("MoveWindow", move_window),
        ("SetWindowPos", set_window_pos),
        ("IsIconic", is_iconic),
        ("GetForegroundWindow", get_foreground_window),
        ("GetActiveWindow", get_active_window),
        ("GetClientRect", get_client_rect),
        ("GetWindowRect", get_window_rect),
        ("AdjustWindowRect", adjust_window_rect),
        ("AdjustWindowRectEx", adjust_window_rect_ex),
        ("GetSystemMetrics", get_system_metrics),
        ("PeekMessageA", peek_message_a),
        ("GetMessageA", get_message_a),
        ("TranslateMessage", translate_message),
        ("DispatchMessageA", dispatch_message_a),
        ("DefWindowProcA", def_window_proc_a),
        ("SendMessageA", send_message_a),
        ("PostMessageA", post_message_a),
        ("PostQuitMessage", post_quit_message),
        ("SetWindowLongA", set_window_long_a),
        ("GetWindowLongA", get_window_long_a),
        ("SetCursor", set_cursor),
        ("ShowCursor", show_cursor),
        ("LoadCursorA", load_cursor_a),
        ("LoadIconA", load_icon_a),
        ("GetKeyState", get_key_state),
        ("GetAsyncKeyState", get_async_key_state),
        ("MessageBoxA", message_box_a),
        ("wsprintfA", wsprintf_a),
        ("BeginPaint", begin_paint),
        ("EndPaint", end_paint),
    ];

    for &(name, handler) in entries {
        registry.register_name(u32_dll, name, handler);
    }

    registry.register_name("gdi32.dll", "GetStockObject", gdi_get_stock_object);
    registry.register_name("gdi32.dll", "GetDeviceCaps", gdi_get_device_caps);
}

fn register_class_common(
    ctx: &mut RuntimeContext<'_>,
    style: u32,
    wndproc: GuestAddr,
    hinstance: u32,
    class_name_ptr: GuestAddr,
) -> Result<u32, RuntimeError> {
    if class_name_ptr == 0 {
        return Ok(0);
    }
    let class_name = ctx.arena.read_cstr(class_name_ptr, MAX_CLASS_LEN)?;
    let key = class_name.to_ascii_lowercase();

    let atom = if let Some(existing) = ctx.win32.window_mgr.registered_classes.get_mut(&key) {
        existing.style = style;
        existing.wndproc = wndproc;
        existing.hinstance = hinstance;
        existing.atom
    } else {
        let atom = ctx.win32.window_mgr.next_atom;
        ctx.win32.window_mgr.next_atom = ctx.win32.window_mgr.next_atom.wrapping_add(1).max(1);
        ctx.win32.window_mgr.registered_classes.insert(
            key.clone(),
            WindowClassInfo {
                style,
                wndproc,
                hinstance,
                atom,
            },
        );
        ctx.win32.window_mgr.class_atoms.insert(atom, key);
        atom
    };

    Ok(atom as u32)
}

fn register_class_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let style = ctx.arena.read_u32(ptr)?;
    let wndproc = ctx.arena.read_u32(ptr + 4)?;
    let hinstance = ctx.arena.read_u32(ptr + 16)?;
    let class_name_ptr = ctx.arena.read_u32(ptr + 36)?;
    let atom = register_class_common(ctx, style, wndproc, hinstance, class_name_ptr)?;
    return_stdcall(cpu, ctx.arena, 1, atom)?;
    Ok(())
}

fn register_class_ex_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let style = ctx.arena.read_u32(ptr + 4)?;
    let wndproc = ctx.arena.read_u32(ptr + 8)?;
    let hinstance = ctx.arena.read_u32(ptr + 20)?;
    let class_name_ptr = ctx.arena.read_u32(ptr + 40)?;
    let atom = register_class_common(ctx, style, wndproc, hinstance, class_name_ptr)?;
    return_stdcall(cpu, ctx.arena, 1, atom)?;
    Ok(())
}

fn unregister_class_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let class_arg = peek_arg_u32(cpu, ctx.arena, 0)?;
    let removed = if let Some(key) = resolve_class_key(ctx, class_arg)? {
        if let Some(info) = ctx.win32.window_mgr.registered_classes.remove(&key) {
            ctx.win32.window_mgr.class_atoms.remove(&info.atom);
            true
        } else {
            false
        }
    } else {
        false
    };
    return_stdcall(cpu, ctx.arena, 2, u32::from(removed))?;
    Ok(())
}

fn create_window_ex_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let class_arg = peek_arg_u32(cpu, ctx.arena, 1)?;
    let title_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let raw_width = peek_arg_i32(cpu, ctx.arena, 6)?;
    let raw_height = peek_arg_i32(cpu, ctx.arena, 7)?;

    let width = if raw_width == CW_USEDEFAULT || raw_width <= 0 {
        DEFAULT_SCREEN_WIDTH
    } else {
        raw_width
    };
    let height = if raw_height == CW_USEDEFAULT || raw_height <= 0 {
        DEFAULT_SCREEN_HEIGHT
    } else {
        raw_height
    };

    let class_key = resolve_class_key(ctx, class_arg)?.unwrap_or_default();
    let wndproc = ctx
        .win32
        .window_mgr
        .registered_classes
        .get(&class_key)
        .map_or(0, |c| c.wndproc);

    let title = if title_ptr != 0 {
        ctx.arena.read_cstr(title_ptr, MAX_TITLE_LEN)?
    } else {
        "toluca-runtime".to_owned()
    };

    let hwnd = ctx.win32.window_mgr.alloc_hwnd();
    let sdl_window = ctx
        .win32
        .window_mgr
        .sdl
        .as_mut()
        .and_then(|sdl| sdl.create_window(&title, width, height));

    ctx.win32.window_mgr.windows.insert(
        hwnd,
        WindowRecord {
            hwnd,
            class_name: class_key,
            wndproc,
            width,
            height,
            visible: true,
            user_data: 0,
            sdl_window,
        },
    );
    if ctx.win32.window_mgr.active_hwnd.is_none() {
        ctx.win32.window_mgr.active_hwnd = Some(hwnd);
        ctx.win32.window_mgr.focused_hwnd = Some(hwnd);
    }

    if wndproc != 0 {
        if ctx.win32.config.trace_imports {
            eprintln!(
                "[user32] CreateWindowExA -> invoking WndProc(0x{wndproc:08x}, hwnd=0x{hwnd:08x}, WM_CREATE) esp=0x{:08x}",
                cpu.gpr(toluca_core::cpu::Gpr::Esp)
            );
        }
        let create_res = ctx.invoke_guest_callback(cpu, wndproc, &[hwnd, WM_CREATE, 0, 0])?;
        if ctx.win32.config.trace_imports {
            eprintln!("[user32] WndProc(WM_CREATE) returned 0x{create_res:08x}");
        }
        if create_res == u32::MAX {
            remove_window_record(ctx, hwnd);
            return_stdcall(cpu, ctx.arena, 12, 0)?;
            return Ok(());
        }
    }

    if ctx.win32.config.trace_imports {
        eprintln!("[user32] CreateWindowExA returning hwnd=0x{hwnd:08x}");
    }
    return_stdcall(cpu, ctx.arena, 12, hwnd)?;
    Ok(())
}

fn destroy_window(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let wndproc = ctx
        .win32
        .window_mgr
        .windows
        .get(&hwnd)
        .map_or(0, |w| w.wndproc);

    if wndproc != 0 {
        let _ = ctx.invoke_guest_callback(cpu, wndproc, &[hwnd, WM_DESTROY, 0, 0])?;
    }

    let existed = remove_window_record(ctx, hwnd);
    return_stdcall(cpu, ctx.arena, 1, u32::from(existed))?;
    Ok(())
}

fn remove_window_record(ctx: &mut RuntimeContext<'_>, hwnd: u32) -> bool {
    let Some(record) = ctx.win32.window_mgr.windows.remove(&hwnd) else {
        return false;
    };
    if let (Some(sdl), Some(ptr)) = (ctx.win32.window_mgr.sdl.as_mut(), record.sdl_window) {
        unsafe {
            sdl.destroy_window(ptr);
        }
    }
    if ctx.win32.window_mgr.active_hwnd == Some(hwnd) {
        ctx.win32.window_mgr.active_hwnd = None;
    }
    if ctx.win32.window_mgr.focused_hwnd == Some(hwnd) {
        ctx.win32.window_mgr.focused_hwnd = None;
    }
    true
}

fn show_window(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let cmd = peek_arg_u32(cpu, ctx.arena, 1)?;
    let show = cmd != 0;

    let mut was_visible = false;
    let mut sdl_ptr = None;
    if let Some(rec) = ctx.win32.window_mgr.windows.get_mut(&hwnd) {
        was_visible = rec.visible;
        rec.visible = show;
        sdl_ptr = rec.sdl_window;
    }
    if let (Some(sdl), Some(ptr)) = (ctx.win32.window_mgr.sdl.as_mut(), sdl_ptr) {
        unsafe {
            sdl.show_window(ptr, show);
        }
    }

    return_stdcall(cpu, ctx.arena, 2, u32::from(was_visible))?;
    Ok(())
}

fn update_window(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn set_focus(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let prev = ctx.win32.window_mgr.focused_hwnd.unwrap_or(0);
    if ctx.win32.window_mgr.windows.contains_key(&hwnd) {
        ctx.win32.window_mgr.focused_hwnd = Some(hwnd);
    }
    return_stdcall(cpu, ctx.arena, 1, prev)?;
    Ok(())
}

fn set_foreground_window(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    if ctx.win32.window_mgr.windows.contains_key(&hwnd) {
        ctx.win32.window_mgr.active_hwnd = Some(hwnd);
        return_stdcall(cpu, ctx.arena, 1, 1)?;
    } else {
        return_stdcall(cpu, ctx.arena, 1, 0)?;
    }
    Ok(())
}

fn move_window(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let width = peek_arg_i32(cpu, ctx.arena, 3)?;
    let height = peek_arg_i32(cpu, ctx.arena, 4)?;
    if let Some(rec) = ctx.win32.window_mgr.windows.get_mut(&hwnd) {
        if width > 0 {
            rec.width = width;
        }
        if height > 0 {
            rec.height = height;
        }
        return_stdcall(cpu, ctx.arena, 6, 1)?;
    } else {
        return_stdcall(cpu, ctx.arena, 6, 0)?;
    }
    Ok(())
}

fn set_window_pos(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let cx = peek_arg_i32(cpu, ctx.arena, 4)?;
    let cy = peek_arg_i32(cpu, ctx.arena, 5)?;
    let flags = peek_arg_u32(cpu, ctx.arena, 6)?;
    if let Some(rec) = ctx.win32.window_mgr.windows.get_mut(&hwnd) {
        if (flags & SWP_NOSIZE) == 0 {
            if cx > 0 {
                rec.width = cx;
            }
            if cy > 0 {
                rec.height = cy;
            }
        }
        return_stdcall(cpu, ctx.arena, 7, 1)?;
    } else {
        return_stdcall(cpu, ctx.arena, 7, 0)?;
    }
    Ok(())
}

fn is_iconic(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn get_foreground_window(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let hwnd = ctx.win32.window_mgr.active_hwnd.unwrap_or(0);
    return_stdcall(cpu, ctx.arena, 0, hwnd)?;
    Ok(())
}

fn get_active_window(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let hwnd = ctx.win32.window_mgr.active_hwnd.unwrap_or(0);
    return_stdcall(cpu, ctx.arena, 0, hwnd)?;
    Ok(())
}

fn write_window_rect(
    ctx: &mut RuntimeContext<'_>,
    hwnd: u32,
    rect_ptr: GuestAddr,
) -> Result<u32, RuntimeError> {
    let Some(rec) = ctx.win32.window_mgr.windows.get(&hwnd) else {
        return Ok(0);
    };
    ctx.arena.write_u32(rect_ptr, 0)?;
    ctx.arena.write_u32(rect_ptr + 4, 0)?;
    ctx.arena.write_u32(rect_ptr + 8, rec.width as u32)?;
    ctx.arena.write_u32(rect_ptr + 12, rec.height as u32)?;
    Ok(1)
}

fn get_client_rect(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let rect_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let ok = write_window_rect(ctx, hwnd, rect_ptr)?;
    return_stdcall(cpu, ctx.arena, 2, ok)?;
    Ok(())
}

fn get_window_rect(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let rect_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let ok = write_window_rect(ctx, hwnd, rect_ptr)?;
    return_stdcall(cpu, ctx.arena, 2, ok)?;
    Ok(())
}

fn adjust_window_rect(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 3, 1)?;
    Ok(())
}

fn adjust_window_rect_ex(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 4, 1)?;
    Ok(())
}

fn get_system_metrics(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let index = peek_arg_i32(cpu, ctx.arena, 0)?;
    let active_size = ctx
        .win32
        .window_mgr
        .active_hwnd
        .and_then(|h| ctx.win32.window_mgr.windows.get(&h))
        .map(|w| (w.width, w.height));

    let val = match index {
        SM_CXSCREEN => active_size.map_or(DEFAULT_SCREEN_WIDTH, |(w, _)| w),
        SM_CYSCREEN => active_size.map_or(DEFAULT_SCREEN_HEIGHT, |(_, h)| h),
        SM_CYCAPTION | SM_CXBORDER | SM_CYBORDER | SM_CXFRAME | SM_CYFRAME => 0,
        _ => 0,
    };
    return_stdcall(cpu, ctx.arena, 1, val as u32)?;
    Ok(())
}

fn peek_message_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let msg_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let hwnd_filter = peek_arg_u32(cpu, ctx.arena, 1)?;
    let msg_min = peek_arg_u32(cpu, ctx.arena, 2)?;
    let msg_max = peek_arg_u32(cpu, ctx.arena, 3)?;
    let remove_flags = peek_arg_u32(cpu, ctx.arena, 4)?;

    poll_sdl_into_queue(ctx);

    let match_idx = ctx
        .win32
        .window_mgr
        .msg_queue
        .iter()
        .position(|m| msg_matches_filter(m, hwnd_filter, msg_min, msg_max));

    if let Some(idx) = match_idx {
        let msg = if (remove_flags & PM_REMOVE) != 0 {
            ctx.win32.window_mgr.msg_queue.remove(idx).unwrap()
        } else {
            ctx.win32.window_mgr.msg_queue[idx]
        };
        write_guest_msg(ctx.arena, msg_ptr, &msg)?;
        return_stdcall(cpu, ctx.arena, 5, 1)?;
    } else {
        return_stdcall(cpu, ctx.arena, 5, 0)?;
    }
    Ok(())
}

fn get_message_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let msg_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let hwnd_filter = peek_arg_u32(cpu, ctx.arena, 1)?;
    let msg_min = peek_arg_u32(cpu, ctx.arena, 2)?;
    let msg_max = peek_arg_u32(cpu, ctx.arena, 3)?;

    let msg = loop {
        poll_sdl_into_queue(ctx);
        if let Some(idx) = ctx
            .win32
            .window_mgr
            .msg_queue
            .iter()
            .position(|m| msg_matches_filter(m, hwnd_filter, msg_min, msg_max))
        {
            break ctx.win32.window_mgr.msg_queue.remove(idx).unwrap();
        }
        if ctx.win32.config.headless
            || ctx.win32.window_mgr.quit_requested
            || ctx.win32.window_mgr.sdl.is_none()
        {
            break Win32Msg {
                hwnd: ctx.win32.window_mgr.active_hwnd.unwrap_or(0),
                message: WM_QUIT,
                wparam: ctx.win32.window_mgr.quit_code,
                lparam: 0,
                time: ctx.host_start_instant.elapsed().as_millis() as u32,
                pt_x: 0,
                pt_y: 0,
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    };

    write_guest_msg(ctx.arena, msg_ptr, &msg)?;
    let ret = u32::from(msg.message != WM_QUIT);
    return_stdcall(cpu, ctx.arena, 4, ret)?;
    Ok(())
}

fn translate_message(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn dispatch_message_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let msg_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let hwnd = ctx.arena.read_u32(msg_ptr)?;
    let message = ctx.arena.read_u32(msg_ptr + 4)?;
    let wparam = ctx.arena.read_u32(msg_ptr + 8)?;
    let lparam = ctx.arena.read_u32(msg_ptr + 12)?;

    let wndproc = ctx
        .win32
        .window_mgr
        .windows
        .get(&hwnd)
        .map_or(0, |w| w.wndproc);

    let res = if wndproc != 0 {
        ctx.invoke_guest_callback(cpu, wndproc, &[hwnd, message, wparam, lparam])?
    } else {
        0
    };
    return_stdcall(cpu, ctx.arena, 1, res)?;
    Ok(())
}

fn def_window_proc_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let msg = peek_arg_u32(cpu, ctx.arena, 1)?;
    if msg == WM_CLOSE {
        let time = ctx.host_start_instant.elapsed().as_millis() as u32;
        ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
            hwnd,
            message: WM_DESTROY,
            wparam: 0,
            lparam: 0,
            time,
            pt_x: 0,
            pt_y: 0,
        });
    }
    return_stdcall(cpu, ctx.arena, 4, 0)?;
    Ok(())
}

fn send_message_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let msg = peek_arg_u32(cpu, ctx.arena, 1)?;
    let wparam = peek_arg_u32(cpu, ctx.arena, 2)?;
    let lparam = peek_arg_u32(cpu, ctx.arena, 3)?;

    let wndproc = ctx
        .win32
        .window_mgr
        .windows
        .get(&hwnd)
        .map_or(0, |w| w.wndproc);

    let res = if wndproc != 0 {
        ctx.invoke_guest_callback(cpu, wndproc, &[hwnd, msg, wparam, lparam])?
    } else {
        0
    };
    return_stdcall(cpu, ctx.arena, 4, res)?;
    Ok(())
}

fn post_message_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let message = peek_arg_u32(cpu, ctx.arena, 1)?;
    let wparam = peek_arg_u32(cpu, ctx.arena, 2)?;
    let lparam = peek_arg_u32(cpu, ctx.arena, 3)?;

    let target_hwnd = if hwnd == 0 {
        ctx.win32.window_mgr.active_hwnd.unwrap_or(0)
    } else {
        hwnd
    };
    let time = ctx.host_start_instant.elapsed().as_millis() as u32;
    ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
        hwnd: target_hwnd,
        message,
        wparam,
        lparam,
        time,
        pt_x: 0,
        pt_y: 0,
    });
    return_stdcall(cpu, ctx.arena, 4, 1)?;
    Ok(())
}

fn post_quit_message(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let exit_code = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.win32.window_mgr.quit_requested = true;
    ctx.win32.window_mgr.quit_code = exit_code;
    let time = ctx.host_start_instant.elapsed().as_millis() as u32;
    ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
        hwnd: ctx.win32.window_mgr.active_hwnd.unwrap_or(0),
        message: WM_QUIT,
        wparam: exit_code,
        lparam: 0,
        time,
        pt_x: 0,
        pt_y: 0,
    });
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn set_window_long_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let index = peek_arg_i32(cpu, ctx.arena, 1)?;
    let new_val = peek_arg_u32(cpu, ctx.arena, 2)?;

    let prev = if let Some(rec) = ctx.win32.window_mgr.windows.get_mut(&hwnd) {
        match index {
            GWL_WNDPROC => {
                let old = rec.wndproc;
                rec.wndproc = new_val;
                old
            }
            GWL_USERDATA => {
                let old = rec.user_data;
                rec.user_data = new_val;
                old
            }
            _ => 0,
        }
    } else {
        0
    };
    return_stdcall(cpu, ctx.arena, 3, prev)?;
    Ok(())
}

fn get_window_long_a(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let hwnd = peek_arg_u32(cpu, ctx.arena, 0)?;
    let index = peek_arg_i32(cpu, ctx.arena, 1)?;

    let val = ctx
        .win32
        .window_mgr
        .windows
        .get(&hwnd)
        .map_or(0, |rec| match index {
            GWL_WNDPROC => rec.wndproc,
            GWL_USERDATA => rec.user_data,
            _ => 0,
        });
    return_stdcall(cpu, ctx.arena, 2, val)?;
    Ok(())
}

fn set_cursor(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let new_cursor = peek_arg_u32(cpu, ctx.arena, 0)?;
    let prev = ctx.win32.window_mgr.current_cursor;
    ctx.win32.window_mgr.current_cursor = new_cursor;
    return_stdcall(cpu, ctx.arena, 1, prev)?;
    Ok(())
}

fn show_cursor(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let show = peek_arg_i32(cpu, ctx.arena, 0)? != 0;
    if show {
        ctx.win32.window_mgr.cursor_count += 1;
    } else {
        ctx.win32.window_mgr.cursor_count -= 1;
    }
    let count = ctx.win32.window_mgr.cursor_count;
    if let Some(sdl) = ctx.win32.window_mgr.sdl.as_mut() {
        sdl.show_cursor(count >= 0);
    }
    return_stdcall(cpu, ctx.arena, 1, count as u32)?;
    Ok(())
}

fn load_cursor_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, SYNTHETIC_CURSOR_HANDLE)?;
    Ok(())
}

fn load_icon_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, SYNTHETIC_ICON_HANDLE)?;
    Ok(())
}

fn get_key_state(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let vk = (peek_arg_u32(cpu, ctx.arena, 0)? & 0xFF) as usize;
    let state = if ctx.win32.window_mgr.key_states[vk] {
        KEY_PRESSED_MASK
    } else {
        0
    };
    return_stdcall(cpu, ctx.arena, 1, state)?;
    Ok(())
}

fn get_async_key_state(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    poll_sdl_into_queue(ctx);
    get_key_state(ctx, cpu)
}

fn message_box_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let text_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let caption_ptr = peek_arg_u32(cpu, ctx.arena, 2)?;
    let text = if text_ptr != 0 {
        ctx.arena
            .read_cstr(text_ptr, MAX_TITLE_LEN)
            .unwrap_or_default()
    } else {
        String::new()
    };
    let caption = if caption_ptr != 0 {
        ctx.arena
            .read_cstr(caption_ptr, MAX_TITLE_LEN)
            .unwrap_or_default()
    } else {
        "MessageBoxA".to_owned()
    };
    if !ctx.win32.config.headless {
        eprintln!("[MessageBoxA] {caption}: {text}");
    }
    return_stdcall(cpu, ctx.arena, 4, IDOK)?;
    Ok(())
}

fn gdi_get_stock_object(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let obj_id = peek_arg_u32(cpu, ctx.arena, 0)?;
    return_stdcall(cpu, ctx.arena, 1, SYNTHETIC_GDI_STOCK_BASE + obj_id)?;
    Ok(())
}

fn wsprintf_a(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let buf_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let fmt_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let fmt_str = ctx.arena.read_cstr(fmt_ptr, MAX_TITLE_LEN)?;
    let formatted = format_printf(ctx.arena, &fmt_str, cpu.gpr(Gpr::Esp) + 12)?;
    ctx.arena.write_cstr(buf_ptr, &formatted)?;
    return_cdecl(cpu, ctx.arena, formatted.len() as u32)?;
    Ok(())
}

fn begin_paint(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let ps_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if ps_ptr != 0 {
        ctx.arena.zero_range(ps_ptr, 64)?;
    }
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn end_paint(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn gdi_get_device_caps(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let index = peek_arg_i32(cpu, ctx.arena, 1)?;
    let val = match index {
        GDI_HORZRES => DEFAULT_SCREEN_WIDTH,
        GDI_VERTRES => DEFAULT_SCREEN_HEIGHT,
        GDI_BITSPIXEL => 32,
        GDI_PLANES => 1,
        GDI_LOGPIXELSX | GDI_LOGPIXELSY => 96,
        _ => 0,
    };
    return_stdcall(cpu, ctx.arena, 2, val as u32)?;
    Ok(())
}

fn resolve_class_key(
    ctx: &RuntimeContext<'_>,
    class_arg: u32,
) -> Result<Option<String>, RuntimeError> {
    if class_arg == 0 {
        return Ok(None);
    }
    if (class_arg >> 16) == 0 {
        let atom = class_arg as u16;
        return Ok(ctx.win32.window_mgr.class_atoms.get(&atom).cloned());
    }
    let name = ctx.arena.read_cstr(class_arg, MAX_CLASS_LEN)?;
    Ok(Some(name.to_ascii_lowercase()))
}

fn poll_sdl_into_queue(ctx: &mut RuntimeContext<'_>) {
    let Some(events) = ctx.win32.window_mgr.sdl.as_mut().map(|s| s.poll_events()) else {
        return;
    };

    let active_hwnd = ctx.win32.window_mgr.active_hwnd.unwrap_or(0);
    let time = ctx.host_start_instant.elapsed().as_millis() as u32;

    for ev in events {
        match ev {
            SdlTranslatedEvent::Quit | SdlTranslatedEvent::WindowClose => {
                ctx.win32.window_mgr.quit_requested = true;
                ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
                    hwnd: active_hwnd,
                    message: WM_QUIT,
                    wparam: 0,
                    lparam: 0,
                    time,
                    pt_x: 0,
                    pt_y: 0,
                });
            }
            SdlTranslatedEvent::KeyDown(vk) => {
                ctx.win32.window_mgr.key_states[vk as usize] = true;
                ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
                    hwnd: active_hwnd,
                    message: WM_KEYDOWN,
                    wparam: vk as u32,
                    lparam: 1,
                    time,
                    pt_x: 0,
                    pt_y: 0,
                });
            }
            SdlTranslatedEvent::KeyUp(vk) => {
                ctx.win32.window_mgr.key_states[vk as usize] = false;
                ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
                    hwnd: active_hwnd,
                    message: WM_KEYUP,
                    wparam: vk as u32,
                    lparam: 0xC000_0001,
                    time,
                    pt_x: 0,
                    pt_y: 0,
                });
            }
            SdlTranslatedEvent::FocusGained => {
                ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
                    hwnd: active_hwnd,
                    message: WM_ACTIVATE,
                    wparam: 1,
                    lparam: 0,
                    time,
                    pt_x: 0,
                    pt_y: 0,
                });
                ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
                    hwnd: active_hwnd,
                    message: WM_SETFOCUS,
                    wparam: 0,
                    lparam: 0,
                    time,
                    pt_x: 0,
                    pt_y: 0,
                });
            }
            SdlTranslatedEvent::FocusLost => {
                ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
                    hwnd: active_hwnd,
                    message: WM_ACTIVATE,
                    wparam: 0,
                    lparam: 0,
                    time,
                    pt_x: 0,
                    pt_y: 0,
                });
                ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
                    hwnd: active_hwnd,
                    message: WM_KILLFOCUS,
                    wparam: 0,
                    lparam: 0,
                    time,
                    pt_x: 0,
                    pt_y: 0,
                });
            }
        }
    }
}

fn msg_matches_filter(msg: &Win32Msg, hwnd_filter: u32, msg_min: u32, msg_max: u32) -> bool {
    let hwnd_ok = hwnd_filter == 0 || msg.hwnd == hwnd_filter || msg.hwnd == 0;
    let range_ok = (msg_min == 0 && msg_max == 0) || (msg.message >= msg_min && msg.message <= msg_max);
    hwnd_ok && range_ok
}

fn write_guest_msg(
    arena: &mut crate::arena::GuestArena,
    ptr: GuestAddr,
    msg: &Win32Msg,
) -> Result<(), RuntimeError> {
    arena.write_u32(ptr, msg.hwnd)?;
    arena.write_u32(ptr + 4, msg.message)?;
    arena.write_u32(ptr + 8, msg.wparam)?;
    arena.write_u32(ptr + 12, msg.lparam)?;
    arena.write_u32(ptr + 16, msg.time)?;
    arena.write_u32(ptr + 20, msg.pt_x as u32)?;
    arena.write_u32(ptr + 24, msg.pt_y as u32)?;
    Ok(())
}
