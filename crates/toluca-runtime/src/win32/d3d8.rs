use toluca_core::GuestAddr;
use toluca_core::cpu::{ExitReason, GuestCpuState};
use toluca_core::memory::GUEST_PAGE_SIZE;
use toluca_d3d8::{ComInterfaceId, D3d8Context, Gles3Backend, GuestMemory};

use crate::arena::GuestArena;
use crate::callconv::{peek_arg_u32, return_stdcall};
use crate::heap::{
    GuestHeap, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualMemoryManager,
};
use crate::runtime::{ImportRegistry, RuntimeContext, RuntimeError, TrapInfo};
use crate::win32::{ERROR_NOT_ENOUGH_MEMORY, ERROR_SUCCESS, write_watch};

pub struct ArenaGuestMemory<'a> {
    pub arena: &'a mut GuestArena,
    pub heap: &'a mut GuestHeap,
    pub vmem: &'a mut VirtualMemoryManager,
}

impl GuestMemory for ArenaGuestMemory<'_> {
    fn bytes_slice(&self, va: GuestAddr, len: usize) -> Result<Option<&[u8]>, String> {
        self.arena
            .read_bytes(va, len)
            .map(Some)
            .map_err(|err| err.to_string())
    }

    fn read_bytes(&self, va: GuestAddr, len: usize) -> Result<Vec<u8>, String> {
        self.arena
            .read_bytes(va, len)
            .map(|slice| slice.to_vec())
            .map_err(|err| err.to_string())
    }

    fn read_into(&self, va: GuestAddr, buf: &mut [u8]) -> Result<(), String> {
        let src = self
            .arena
            .read_bytes(va, buf.len())
            .map_err(|err| err.to_string())?;
        buf.copy_from_slice(src);
        Ok(())
    }

    fn read_u32(&self, va: GuestAddr) -> Result<u32, String> {
        self.arena.read_u32(va).map_err(|err| err.to_string())
    }

    fn write_bytes(&mut self, va: GuestAddr, data: &[u8]) -> Result<(), String> {
        self.arena
            .write_bytes(va, data)
            .map_err(|err| err.to_string())
    }

    fn write_u32(&mut self, va: GuestAddr, val: u32) -> Result<(), String> {
        self.arena
            .write_u32(va, val)
            .map_err(|err| err.to_string())
    }

    fn zero_range(&mut self, va: GuestAddr, len: usize) -> Result<(), String> {
        self.arena
            .zero_range(va, len)
            .map_err(|err| err.to_string())
    }

    fn alloc(&mut self, size: usize, zeroed: bool) -> Result<GuestAddr, String> {
        self.heap
            .alloc(self.arena, size, zeroed)
            .map_err(|err| err.to_string())
    }

    fn free(&mut self, va: GuestAddr) -> Result<(), String> {
        self.heap
            .free(self.arena, va)
            .map_err(|err| err.to_string())
    }

    fn alloc_page_aligned(&mut self, size: usize) -> Result<(GuestAddr, usize), String> {
        let page_align = self.arena.host_page_size().max(GUEST_PAGE_SIZE as usize);
        let aligned = size.max(page_align).next_multiple_of(page_align);
        let va = self
            .vmem
            .virtual_alloc(
                self.arena,
                None,
                aligned,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
            .map_err(|err| err.to_string())?;
        Ok((va, aligned))
    }

    fn free_page_aligned(&mut self, va: GuestAddr, alloc_size: usize) -> Result<(), String> {
        self.remove_write_watch(va, alloc_size);
        self.vmem
            .virtual_free(self.arena, va, 0, MEM_RELEASE)
            .map_err(|err| err.to_string())
    }

    fn arm_write_watch(&mut self, va: GuestAddr, alloc_size: usize) -> bool {
        let Ok(slice) = self.arena.read_bytes(va, alloc_size) else {
            return false;
        };
        let ptr = slice.as_ptr().cast_mut();
        write_watch::arm(ptr, alloc_size)
    }

    fn is_write_watch_dirty(&self, va: GuestAddr, alloc_size: usize) -> Option<bool> {
        if self.arena.read_bytes(va, alloc_size).is_err() {
            return None;
        }
        let ptr = unsafe { self.arena.base_ptr().add(va as usize) };
        write_watch::is_dirty(ptr, alloc_size)
    }

    fn remove_write_watch(&mut self, va: GuestAddr, alloc_size: usize) {
        if alloc_size == 0 || self.arena.read_bytes(va, alloc_size).is_err() {
            return;
        }
        let ptr = unsafe { self.arena.base_ptr().add(va as usize) };
        write_watch::remove(ptr, alloc_size);
    }
}

pub fn register_imports(registry: &mut ImportRegistry) {
    registry.register_name("d3d8.dll", "Direct3DCreate8", direct3d_create_8);
}

fn maybe_attach_gles3(ctx: &mut RuntimeContext<'_>, focus_hwnd: u32) {
    if ctx.win32.config.headless || ctx.win32.d3d8.backend().as_any().is::<Gles3Backend>() {
        return;
    }

    let sdl_win = ctx
        .win32
        .window_mgr
        .windows
        .get(&focus_hwnd)
        .and_then(|w| w.sdl_window)
        .or_else(|| {
            ctx.win32
                .window_mgr
                .sdl
                .as_ref()
                .and_then(|sdl| sdl.primary_window())
        });
    let Some(win_ptr) = sdl_win else {
        return;
    };

    let Some(sdl) = ctx.win32.window_mgr.sdl.as_mut() else {
        return;
    };
    let Some((get_proc, present_target)) = (unsafe { sdl.create_gles3_context(win_ptr) }) else {
        return;
    };

    let cache_path = ctx.win32.config.shader_cache_path.clone();
    let backend_res = {
        let _host_cores = crate::win32::HostCoresGuard::new();
        Gles3Backend::new(Some(get_proc), present_target, cache_path)
    };
    match backend_res {
        Ok(mut backend) => {
            backend.set_aspect_mode(ctx.win32.config.aspect_mode);
            ctx.win32.d3d8.replace_backend(Box::new(backend));
        }
        Err(err) => {
            eprintln!("[d3d8] GLES 3.0 backend init failed, falling back to NullBackend: {err}");
        }
    }
}

fn direct3d_create_8(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let sdk_version = peek_arg_u32(cpu, ctx.arena, 0)?;
    ctx.win32.d3d8_create_calls.push(sdk_version);
    maybe_attach_gles3(ctx, 0);

    let mut mem = ArenaGuestMemory {
        arena: ctx.arena,
        heap: ctx.heap,
        vmem: ctx.vmem,
    };
    let obj_va = match ctx.win32.d3d8.create_direct3d8(&mut mem, sdk_version) {
        Ok(va) => {
            *ctx.last_error = ERROR_SUCCESS;
            va
        }
        Err(_) => {
            *ctx.last_error = ERROR_NOT_ENOUGH_MEMORY;
            0
        }
    };

    return_stdcall(cpu, ctx.arena, 1, obj_va)?;
    Ok(())
}

pub fn dispatch_com_call(
    ctx: &mut RuntimeContext<'_>,
    state: &mut GuestCpuState,
    iface: ComInterfaceId,
    method_idx: u32,
) {
    let Some(arg_count) = D3d8Context::method_arg_count(iface, method_idx) else {
        set_unimplemented_com_trap(ctx, state, iface as u32, method_idx);
        return;
    };

    let mut args = Vec::with_capacity(arg_count as usize);
    for idx in 0..arg_count {
        match peek_arg_u32(state, ctx.arena, idx) {
            Ok(val) => args.push(val),
            Err(err) => {
                fail_com_call(ctx, state, iface, method_idx, RuntimeError::Arena(err));
                return;
            }
        }
    }

    if iface == ComInterfaceId::Direct3D8 && method_idx == 15 {
        let focus_hwnd = args.get(3).copied().unwrap_or(0);
        maybe_attach_gles3(ctx, focus_hwnd);
    }

    let result = {
        let mut mem = ArenaGuestMemory {
            arena: ctx.arena,
            heap: ctx.heap,
            vmem: ctx.vmem,
        };
        ctx.win32.d3d8.dispatch_com(&mut mem, iface, method_idx, &args)
    };

    match result {
        Ok(call_res) => {
            if let Err(err) = return_stdcall(state, ctx.arena, call_res.arg_count, call_res.ret_val)
            {
                fail_com_call(ctx, state, iface, method_idx, RuntimeError::Arena(err));
                return;
            }
            if iface == ComInterfaceId::Direct3DDevice8 && method_idx == 15 {
                ctx.win32.d3d8_present_calls += 1;
                crate::win32::dinput8::sync_scripted_keys_for_frame(ctx);
                if ctx
                    .win32
                    .config
                    .max_frames
                    .is_some_and(|limit| ctx.win32.d3d8_present_calls >= limit)
                {
                    ctx.win32.exit_code = Some(0);
                    state.exit_reason = ExitReason::Halt as u32;
                }
            }
        }
        Err(msg) => {
            fail_com_call(
                ctx,
                state,
                iface,
                method_idx,
                RuntimeError::ImportHandlerFailed(msg),
            );
        }
    }
}

fn set_unimplemented_com_trap(
    ctx: &mut RuntimeContext<'_>,
    state: &mut GuestCpuState,
    interface_id: u32,
    method_index: u32,
) {
    state.exit_reason = ExitReason::HostTrap as u32;
    state.exit_detail = (interface_id << 16) | (method_index & 0xFFFF);
    *ctx.trap = Some(TrapInfo::UnimplementedCom {
        interface_id,
        method_index,
    });
}

fn fail_com_call(
    ctx: &mut RuntimeContext<'_>,
    state: &mut GuestCpuState,
    iface: ComInterfaceId,
    method_idx: u32,
    err: RuntimeError,
) {
    state.exit_reason = ExitReason::HostTrap as u32;
    state.exit_detail = ((iface as u32) << 16) | (method_idx & 0xFFFF);
    if ctx.pending_error.is_none() {
        *ctx.pending_error = Some(err);
    }
}
