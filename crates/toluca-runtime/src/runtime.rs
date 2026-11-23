use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::time::Instant;

use toluca_core::GuestAddr;
use toluca_core::aot::ImportSymbol;
use toluca_core::bridge::{HostBridge, default_x87_helper};
use toluca_core::cpu::{ExitReason, Gpr, GuestCpuState};
use toluca_core::memory::{GUEST_ARENA_SIZE, GUEST_TEB_ADDR, GUEST_THUNK_BASE, GUEST_THUNK_END};
use toluca_core::pe::{PeError, PeImage};

use crate::arena::{ArenaError, GuestArena};
use crate::callconv;
use crate::heap::{GuestHeap, HeapError, VirtualMemoryManager};
use crate::loader::{AotCodeImage, LoaderError};
use crate::win32::{Win32Config, Win32State};

pub const IMPORT_THUNK_BASE: GuestAddr = GUEST_THUNK_BASE;
pub const COM_THUNK_BASE: GuestAddr = 0x7080_0000;
pub const IMPORT_THUNK_END: GuestAddr = COM_THUNK_BASE;
pub const THUNK_STRIDE: u32 = 4;
pub const MAX_IMPORT_THUNK_SLOTS: usize = ((IMPORT_THUNK_END - IMPORT_THUNK_BASE) / THUNK_STRIDE) as usize;
pub const GUEST_RETURN_SENTINEL: GuestAddr = 0xFFFF_F000;

const COM_INTERFACE_SHIFT: u32 = 16;
const COM_METHOD_MASK: u32 = 0xFFFF;

const CPUID_MAX_BASIC_LEAF: u32 = 0x0000_0001;
const CPUID_VENDOR_EBX: u32 = 0x756E_6547;
const CPUID_VENDOR_EDX: u32 = 0x4965_6E69;
const CPUID_VENDOR_ECX: u32 = 0x6C65_746E;
const CPUID_PENTIUM_III_SIG: u32 = 0x0000_0680;
const CPUID_FEATURE_FPU: u32 = 1 << 0;
const CPUID_FEATURE_TSC: u32 = 1 << 4;
const CPUID_FEATURE_CMOV: u32 = 1 << 15;
const CPUID_FEATURE_MMX: u32 = 1 << 23;
const CPUID_FEATURE_FXSR: u32 = 1 << 24;
const CPUID_FEATURE_SSE: u32 = 1 << 25;
const CPUID_FEATURE_SSE2: u32 = 1 << 26;
const CPUID_PENTIUM_III_EDX: u32 = CPUID_FEATURE_FPU
    | CPUID_FEATURE_TSC
    | CPUID_FEATURE_CMOV
    | CPUID_FEATURE_MMX
    | CPUID_FEATURE_FXSR
    | CPUID_FEATURE_SSE
    | CPUID_FEATURE_SSE2;

#[derive(Debug)]
pub enum RuntimeError {
    Io(std::io::Error),
    Pe(PeError),
    Loader(LoaderError),
    Arena(ArenaError),
    Heap(HeapError),
    TooManyImports(usize),
    UnmappedEntry(GuestAddr),
    UnmappedCallback(GuestAddr),
    CallbackTrapped { target_va: GuestAddr, reason: ExitReason, detail: u32 },
    ImportHandlerFailed(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Pe(err) => write!(f, "PE32 parse error: {err}"),
            Self::Loader(err) => write!(f, "AOT loader error: {err}"),
            Self::Arena(err) => write!(f, "arena error: {err}"),
            Self::Heap(err) => write!(f, "heap error: {err}"),
            Self::TooManyImports(count) => write!(f, "import count {count} exceeds thunk table capacity"),
            Self::UnmappedEntry(va) => write!(f, "no recompiled function found at entry VA 0x{va:08x}"),
            Self::UnmappedCallback(va) => write!(f, "no recompiled function found at callback VA 0x{va:08x}"),
            Self::CallbackTrapped {
                target_va,
                reason,
                detail,
            } => write!(
                f,
                "guest callback 0x{target_va:08x} exited unexpectedly ({reason:?}, detail 0x{detail:08x})"
            ),
            Self::ImportHandlerFailed(msg) => write!(f, "import handler failed: {msg}"),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Pe(err) => Some(err),
            Self::Loader(err) => Some(err),
            Self::Arena(err) => Some(err),
            Self::Heap(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<PeError> for RuntimeError {
    fn from(err: PeError) -> Self {
        Self::Pe(err)
    }
}

impl From<LoaderError> for RuntimeError {
    fn from(err: LoaderError) -> Self {
        Self::Loader(err)
    }
}

impl From<ArenaError> for RuntimeError {
    fn from(err: ArenaError) -> Self {
        Self::Arena(err)
    }
}

impl From<HeapError> for RuntimeError {
    fn from(err: HeapError) -> Self {
        Self::Heap(err)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImportSymbolKey {
    Name(String),
    Ordinal(u16),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImportKey {
    pub dll_name: String,
    pub symbol: ImportSymbolKey,
}

impl ImportKey {
    pub fn from_name(dll_name: &str, symbol_name: &str) -> Self {
        Self {
            dll_name: dll_name.to_ascii_lowercase(),
            symbol: ImportSymbolKey::Name(symbol_name.to_owned()),
        }
    }

    pub fn from_ordinal(dll_name: &str, ordinal: u16) -> Self {
        Self {
            dll_name: dll_name.to_ascii_lowercase(),
            symbol: ImportSymbolKey::Ordinal(ordinal),
        }
    }

    pub fn from_import(dll_name: &str, symbol: &ImportSymbol) -> Self {
        match symbol {
            ImportSymbol::Name(name) => Self::from_name(dll_name, name),
            ImportSymbol::Ordinal(ordinal) => Self::from_ordinal(dll_name, *ordinal),
        }
    }
}

pub type ImportHandler = fn(&mut RuntimeContext<'_>, &mut GuestCpuState) -> Result<(), RuntimeError>;

#[derive(Clone, Default)]
pub struct ImportRegistry {
    handlers: HashMap<ImportKey, ImportHandler>,
}

impl ImportRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    pub fn register_name(&mut self, dll_name: &str, symbol_name: &str, handler: ImportHandler) {
        self.handlers
            .insert(ImportKey::from_name(dll_name, symbol_name), handler);
    }

    pub fn register_ordinal(&mut self, dll_name: &str, ordinal: u16, handler: ImportHandler) {
        self.handlers
            .insert(ImportKey::from_ordinal(dll_name, ordinal), handler);
    }

    pub fn get(&self, dll_name: &str, symbol: &ImportSymbol) -> Option<ImportHandler> {
        let key = ImportKey::from_import(dll_name, symbol);
        self.handlers.get(&key).copied()
    }
}

#[derive(Clone)]
struct ResolvedImportSlot {
    dll_name: String,
    symbol: ImportSymbol,
    handler: Option<ImportHandler>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrapInfo {
    UnresolvedImport {
        index: u32,
        dll_name: String,
        symbol: ImportSymbol,
    },
    UnresolvedIndirectCall(GuestAddr),
    UnimplementedCom {
        interface_id: u32,
        method_index: u32,
    },
    UnhandledInstruction {
        guest_va: GuestAddr,
        reason: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutcome {
    pub exit_reason: ExitReason,
    pub exit_detail: u32,
    pub eax: u32,
    pub eip: u32,
    pub esp: u32,
    pub trap: Option<TrapInfo>,
}

pub struct RuntimeContext<'a> {
    pub arena: &'a mut GuestArena,
    pub heap: &'a mut GuestHeap,
    pub vmem: &'a mut VirtualMemoryManager,
    pub win32: &'a mut Win32State,
    pub last_error: &'a mut u32,
    pub host_start_instant: Instant,
    pub pe_image_base: GuestAddr,
    code: &'a AotCodeImage,
    imports: &'a mut Vec<ResolvedImportSlot>,
    import_slot_map: &'a mut HashMap<ImportKey, usize>,
    registry: &'a ImportRegistry,
    pub(crate) trap: &'a mut Option<TrapInfo>,
    pub(crate) pending_error: &'a mut Option<RuntimeError>,
}

impl<'a> RuntimeContext<'a> {
    pub fn register_dynamic_import(
        &mut self,
        dll_name: &str,
        symbol: ImportSymbol,
    ) -> Option<GuestAddr> {
        register_dynamic_slot(
            self.imports,
            self.import_slot_map,
            self.registry,
            dll_name,
            symbol,
        )
    }

    pub fn invoke_guest_callback(
        &mut self,
        cpu: &mut GuestCpuState,
        target_va: GuestAddr,
        args: &[u32],
    ) -> Result<u32, RuntimeError> {
        let cb_fn = self
            .code
            .resolve_va(target_va)
            .ok_or(RuntimeError::UnmappedCallback(target_va))?;

        let saved_eip = cpu.eip;
        let saved_esp = cpu.gpr(Gpr::Esp);
        let saved_bridge = cpu.bridge;
        let saved_exit_reason = cpu.exit_reason;
        let saved_exit_detail = cpu.exit_detail;

        push_args_and_sentinel(cpu, self.arena, args)?;
        cpu.eip = target_va;
        cpu.exit_reason = ExitReason::Running as u32;
        cpu.exit_detail = 0;

        let local_bridge = build_host_bridge((self as *mut RuntimeContext<'a>).cast());
        cpu.bridge = &local_bridge;
        unsafe {
            cb_fn(cpu);
        }
        cpu.bridge = saved_bridge;

        if let Some(err) = self.pending_error.take() {
            return Err(err);
        }

        let exit_reason = exit_reason_from_u32(cpu.exit_reason);
        if exit_reason != ExitReason::Running
            && exit_reason != ExitReason::Returned
            && exit_reason != ExitReason::Halt
        {
            return Err(RuntimeError::CallbackTrapped {
                target_va,
                reason: exit_reason,
                detail: cpu.exit_detail,
            });
        }

        let ret_eax = cpu.gpr(Gpr::Eax);
        cpu.eip = saved_eip;
        cpu.set_gpr(Gpr::Esp, saved_esp);
        cpu.exit_reason = saved_exit_reason;
        cpu.exit_detail = saved_exit_detail;
        Ok(ret_eax)
    }
}

pub struct Runtime {
    pe: PeImage,
    code: AotCodeImage,
    arena: GuestArena,
    heap: GuestHeap,
    vmem: VirtualMemoryManager,
    win32: Win32State,
    imports: Vec<ResolvedImportSlot>,
    import_slot_map: HashMap<ImportKey, usize>,
    registry: ImportRegistry,
    last_error: u32,
    host_start_instant: Instant,
}

impl Runtime {
    pub fn load(
        pe_path: &Path,
        aot_path: &Path,
        registry: ImportRegistry,
    ) -> Result<Self, RuntimeError> {
        Self::load_with_config(pe_path, aot_path, registry, Win32Config::default())
    }

    pub fn load_with_config(
        pe_path: &Path,
        aot_path: &Path,
        registry: ImportRegistry,
        config: Win32Config,
    ) -> Result<Self, RuntimeError> {
        let pe_bytes = std::fs::read(pe_path)?;
        Self::load_from_bytes_with_config(pe_bytes, aot_path, registry, config)
    }

    pub fn load_from_bytes(
        pe_bytes: Vec<u8>,
        aot_path: &Path,
        registry: ImportRegistry,
    ) -> Result<Self, RuntimeError> {
        Self::load_from_bytes_with_config(pe_bytes, aot_path, registry, Win32Config::default())
    }

    pub fn load_from_bytes_with_config(
        pe_bytes: Vec<u8>,
        aot_path: &Path,
        registry: ImportRegistry,
        config: Win32Config,
    ) -> Result<Self, RuntimeError> {
        let pe = PeImage::parse(pe_bytes)?;
        let code = AotCodeImage::load(aot_path)?;
        code.verify_source_pe(&pe)?;

        let mut arena = GuestArena::new()?;
        arena.map_pe_image(&pe, &pe.raw)?;
        arena.init_thread_environment(pe.image_base)?;

        let mut heap = GuestHeap::new();
        let mut win32 = Win32State::new(config);

        let manifest_imports = &code.manifest().imports;
        if manifest_imports.len() > MAX_IMPORT_THUNK_SLOTS {
            return Err(RuntimeError::TooManyImports(manifest_imports.len()));
        }

        let mut imports = Vec::with_capacity(manifest_imports.len());
        let mut import_slot_map = HashMap::with_capacity(manifest_imports.len());

        for (slot_idx, imp) in manifest_imports.iter().enumerate() {
            if let Some(data_va) =
                resolve_data_import(&imp.dll_name, &imp.symbol, &mut arena, &mut heap, &mut win32)?
            {
                arena.write_iat_u32(imp.iat_va, data_va)?;
            } else {
                let thunk_va = import_slot_to_va(slot_idx);
                arena.write_iat_u32(imp.iat_va, thunk_va)?;
            }

            let handler = registry.get(&imp.dll_name, &imp.symbol);
            let key = ImportKey::from_import(&imp.dll_name, &imp.symbol);
            import_slot_map.entry(key).or_insert(slot_idx);

            imports.push(ResolvedImportSlot {
                dll_name: imp.dll_name.clone(),
                symbol: imp.symbol.clone(),
                handler,
            });
        }

        Ok(Self {
            pe,
            code,
            arena,
            heap,
            vmem: VirtualMemoryManager::new(),
            win32,
            imports,
            import_slot_map,
            registry,
            last_error: 0,
            host_start_instant: Instant::now(),
        })
    }

    pub fn register_dynamic_import(
        &mut self,
        dll_name: &str,
        symbol: ImportSymbol,
    ) -> Option<GuestAddr> {
        register_dynamic_slot(
            &mut self.imports,
            &mut self.import_slot_map,
            &self.registry,
            dll_name,
            symbol,
        )
    }

    pub fn run_entry(&mut self) -> Result<RunOutcome, RuntimeError> {
        self.call_va(self.code.entry_point_va(), &[])
    }

    pub fn call_va(&mut self, va: GuestAddr, args: &[u32]) -> Result<RunOutcome, RuntimeError> {
        let entry_fn = self
            .code
            .resolve_va(va)
            .ok_or(RuntimeError::UnmappedEntry(va))?;

        let mut cpu = GuestCpuState::new(self.arena.base_ptr(), std::ptr::null());
        cpu.eip = va;
        cpu.fs_base = GUEST_TEB_ADDR;

        push_args_and_sentinel(&mut cpu, &mut self.arena, args)?;

        let mut trap: Option<TrapInfo> = None;
        let mut pending_error: Option<RuntimeError> = None;

        let mut ctx = RuntimeContext {
            arena: &mut self.arena,
            heap: &mut self.heap,
            vmem: &mut self.vmem,
            win32: &mut self.win32,
            last_error: &mut self.last_error,
            host_start_instant: self.host_start_instant,
            pe_image_base: self.pe.image_base,
            code: &self.code,
            imports: &mut self.imports,
            import_slot_map: &mut self.import_slot_map,
            registry: &self.registry,
            trap: &mut trap,
            pending_error: &mut pending_error,
        };

        let bridge = build_host_bridge((&mut ctx as *mut RuntimeContext<'_>).cast());
        cpu.bridge = &bridge;

        let _sig_guard = SigsegvDiagnosticGuard::install(ctx.arena.base_ptr());
        unsafe {
            entry_fn(&mut cpu);
        }
        drop(_sig_guard);

        if let Some(err) = pending_error {
            return Err(err);
        }

        let mut exit_reason = exit_reason_from_u32(cpu.exit_reason);
        if exit_reason == ExitReason::Running && cpu.eip == GUEST_RETURN_SENTINEL {
            exit_reason = ExitReason::Returned;
        }

        Ok(RunOutcome {
            exit_reason,
            exit_detail: cpu.exit_detail,
            eax: cpu.gpr(Gpr::Eax),
            eip: cpu.eip,
            esp: cpu.gpr(Gpr::Esp),
            trap,
        })
    }

    pub fn pe(&self) -> &PeImage {
        &self.pe
    }

    pub fn code(&self) -> &AotCodeImage {
        &self.code
    }

    pub fn arena(&self) -> &GuestArena {
        &self.arena
    }

    pub fn arena_mut(&mut self) -> &mut GuestArena {
        &mut self.arena
    }

    pub fn heap(&self) -> &GuestHeap {
        &self.heap
    }

    pub fn vmem(&self) -> &VirtualMemoryManager {
        &self.vmem
    }

    pub fn win32(&self) -> &Win32State {
        &self.win32
    }

    pub fn win32_mut(&mut self) -> &mut Win32State {
        &mut self.win32
    }

    pub fn last_error(&self) -> u32 {
        self.last_error
    }
}

fn import_slot_to_va(slot_idx: usize) -> GuestAddr {
    IMPORT_THUNK_BASE + (slot_idx as u32) * THUNK_STRIDE
}

fn register_dynamic_slot(
    imports: &mut Vec<ResolvedImportSlot>,
    import_slot_map: &mut HashMap<ImportKey, usize>,
    registry: &ImportRegistry,
    dll_name: &str,
    symbol: ImportSymbol,
) -> Option<GuestAddr> {
    let key = ImportKey::from_import(dll_name, &symbol);
    if let Some(&existing_slot) = import_slot_map.get(&key) {
        return Some(import_slot_to_va(existing_slot));
    }

    let handler = registry.get(dll_name, &symbol)?;
    let slot_idx = imports.len();
    if slot_idx >= MAX_IMPORT_THUNK_SLOTS {
        return None;
    }

    imports.push(ResolvedImportSlot {
        dll_name: dll_name.to_owned(),
        symbol,
        handler: Some(handler),
    });
    import_slot_map.insert(key, slot_idx);
    Some(import_slot_to_va(slot_idx))
}

fn push_args_and_sentinel(
    cpu: &mut GuestCpuState,
    arena: &mut GuestArena,
    args: &[u32],
) -> Result<(), ArenaError> {
    for &arg in args.iter().rev() {
        callconv::push_u32(cpu, arena, arg)?;
    }
    callconv::push_u32(cpu, arena, GUEST_RETURN_SENTINEL)?;
    Ok(())
}

fn exit_reason_from_u32(raw: u32) -> ExitReason {
    match raw {
        0 => ExitReason::Running,
        1 => ExitReason::Returned,
        2 => ExitReason::Halt,
        3 => ExitReason::UnmappedTarget,
        4 => ExitReason::UnsupportedInstruction,
        5 => ExitReason::HostTrap,
        6 => ExitReason::GuestException,
        _ => ExitReason::HostTrap,
    }
}

fn build_host_bridge(user_data: *mut core::ffi::c_void) -> HostBridge {
    HostBridge {
        user_data,
        call_indirect: bridge_call_indirect,
        call_import: bridge_call_import,
        call_com: bridge_call_com,
        materialize_flags: bridge_materialize_flags,
        x87_helper: default_x87_helper,
        cpuid: bridge_cpuid,
        rdtsc: bridge_rdtsc,
        unhandled_instruction: bridge_unhandled_instruction,
        trace_hook: bridge_trace_hook,
    }
}

unsafe fn context_from_cpu<'a>(
    cpu: *mut GuestCpuState,
) -> Option<(&'a mut GuestCpuState, &'a mut RuntimeContext<'a>)> {
    let state = unsafe { cpu.as_mut() }?;
    let bridge = unsafe { state.bridge.as_ref() }?;
    let ctx = unsafe { bridge.user_data.cast::<RuntimeContext<'a>>().as_mut() }?;
    Some((state, ctx))
}

fn dispatch_import_slot(ctx: &mut RuntimeContext<'_>, state: &mut GuestCpuState, import_index: u32) {
    let Some(slot) = ctx.imports.get(import_index as usize).cloned() else {
        state.exit_reason = ExitReason::HostTrap as u32;
        state.exit_detail = import_index;
        *ctx.trap = Some(TrapInfo::UnresolvedImport {
            index: import_index,
            dll_name: String::new(),
            symbol: ImportSymbol::Ordinal(0),
        });
        return;
    };

    record_last_import(&slot.dll_name, &slot.symbol);
    if ctx.win32.config.trace_imports {
        let a0 = callconv::peek_arg_u32(state, ctx.arena, 0).unwrap_or(0);
        let a1 = callconv::peek_arg_u32(state, ctx.arena, 1).unwrap_or(0);
        let a2 = callconv::peek_arg_u32(state, ctx.arena, 2).unwrap_or(0);
        let a3 = callconv::peek_arg_u32(state, ctx.arena, 3).unwrap_or(0);
        eprintln!(
            "[import] {}!{:?}(0x{a0:08x}, 0x{a1:08x}, 0x{a2:08x}, 0x{a3:08x})",
            slot.dll_name, slot.symbol
        );
    }

    let Some(handler) = slot.handler else {
        state.exit_reason = ExitReason::HostTrap as u32;
        state.exit_detail = import_index;
        *ctx.trap = Some(TrapInfo::UnresolvedImport {
            index: import_index,
            dll_name: slot.dll_name,
            symbol: slot.symbol,
        });
        return;
    };

    if let Err(err) = handler(ctx, state) {
        state.exit_reason = ExitReason::HostTrap as u32;
        state.exit_detail = import_index;
        if ctx.pending_error.is_none() {
            *ctx.pending_error = Some(err);
        }
    }
}

fn resolve_data_import(
    dll_name: &str,
    symbol: &ImportSymbol,
    arena: &mut GuestArena,
    heap: &mut GuestHeap,
    win32: &mut Win32State,
) -> Result<Option<GuestAddr>, RuntimeError> {
    if !dll_name.eq_ignore_ascii_case("msvcr70.dll") && !dll_name.eq_ignore_ascii_case("msvcrt.dll")
    {
        return Ok(None);
    }
    let ImportSymbol::Name(name) = symbol else {
        return Ok(None);
    };
    match name.as_str() {
        "_acmdln" => {
            let str_va = match win32.command_line_guest {
                Some(existing) => existing,
                None => {
                    let cmd = win32.config.command_line.clone();
                    let allocated = heap.alloc(arena, cmd.len() + 1, true)?;
                    arena.write_cstr(allocated, &cmd)?;
                    win32.command_line_guest = Some(allocated);
                    allocated
                }
            };
            let ptr_slot = heap.alloc(arena, 4, true)?;
            arena.write_u32(ptr_slot, str_va)?;
            Ok(Some(ptr_slot))
        }
        "_adjust_fdiv" => {
            let slot = heap.alloc(arena, 4, true)?;
            Ok(Some(slot))
        }
        _ => Ok(None),
    }
}

const LAST_IMPORT_CAP: usize = 96;
static ARENA_BASE_ADDR: AtomicUsize = AtomicUsize::new(0);
static ARENA_END_ADDR: AtomicUsize = AtomicUsize::new(0);
static LAST_IMPORT_LEN: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_U8: AtomicU8 = AtomicU8::new(0);
static LAST_IMPORT_BUF: [AtomicU8; LAST_IMPORT_CAP] = [ZERO_U8; LAST_IMPORT_CAP];

fn record_last_import(dll_name: &str, symbol: &ImportSymbol) {
    let label = match symbol {
        ImportSymbol::Name(name) => format!("{dll_name}!{name}"),
        ImportSymbol::Ordinal(ord) => format!("{dll_name}!#{ord}"),
    };
    let bytes = label.as_bytes();
    let len = bytes.len().min(LAST_IMPORT_CAP);
    for (idx, &b) in bytes[..len].iter().enumerate() {
        LAST_IMPORT_BUF[idx].store(b, Ordering::Relaxed);
    }
    LAST_IMPORT_LEN.store(len, Ordering::Relaxed);
}

struct SigsegvDiagnosticGuard {
    prev_action: libc::sigaction,
    installed: bool,
}

impl SigsegvDiagnosticGuard {
    fn install(arena_base: *mut u8) -> Self {
        let base = arena_base as usize;
        let end = base.saturating_add(GUEST_ARENA_SIZE as usize);
        ARENA_BASE_ADDR.store(base, Ordering::Relaxed);
        ARENA_END_ADDR.store(end, Ordering::Relaxed);

        let mut new_action: libc::sigaction = unsafe { std::mem::zeroed() };
        new_action.sa_sigaction = sigsegv_diagnostic_handler as usize;
        new_action.sa_flags = libc::SA_SIGINFO | libc::SA_RESETHAND;
        let mut prev_action: libc::sigaction = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::sigaction(libc::SIGSEGV, &new_action, &mut prev_action) };
        Self {
            prev_action,
            installed: rc == 0,
        }
    }
}

impl Drop for SigsegvDiagnosticGuard {
    fn drop(&mut self) {
        if self.installed {
            unsafe {
                libc::sigaction(libc::SIGSEGV, &self.prev_action, std::ptr::null_mut());
            }
        }
        ARENA_BASE_ADDR.store(0, Ordering::Relaxed);
        ARENA_END_ADDR.store(0, Ordering::Relaxed);
    }
}

extern "C" fn sigsegv_diagnostic_handler(
    sig: libc::c_int,
    info: *mut libc::siginfo_t,
    _ucontext: *mut core::ffi::c_void,
) {
    let fault_addr = if info.is_null() {
        0usize
    } else {
        unsafe { (*info).si_addr() as usize }
    };
    let base = ARENA_BASE_ADDR.load(Ordering::Relaxed);
    let end = ARENA_END_ADDR.load(Ordering::Relaxed);

    let mut out = [0u8; 256];
    let mut pos = 0usize;
    pos = push_bytes(&mut out, pos, b"[SIGSEGV] host=0x");
    pos = push_hex_usize(&mut out, pos, fault_addr);
    if base != 0 && fault_addr >= base && fault_addr < end {
        pos = push_bytes(&mut out, pos, b" guest=0x");
        pos = push_hex_usize(&mut out, pos, fault_addr - base);
    } else {
        pos = push_bytes(&mut out, pos, b" (outside guest arena)");
    }
    pos = push_bytes(&mut out, pos, b" last_import=");
    let imp_len = LAST_IMPORT_LEN.load(Ordering::Relaxed).min(LAST_IMPORT_CAP);
    for slot in &LAST_IMPORT_BUF[..imp_len] {
        if pos < out.len() {
            out[pos] = slot.load(Ordering::Relaxed);
            pos += 1;
        }
    }
    pos = push_bytes(&mut out, pos, b"\n");
    unsafe {
        libc::write(libc::STDERR_FILENO, out.as_ptr().cast(), pos);
        libc::raise(sig);
    }
}

fn push_bytes(buf: &mut [u8], mut pos: usize, src: &[u8]) -> usize {
    for &b in src {
        if pos < buf.len() {
            buf[pos] = b;
            pos += 1;
        }
    }
    pos
}

fn push_hex_usize(buf: &mut [u8], mut pos: usize, val: usize) -> usize {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digits = core::mem::size_of::<usize>() * 2;
    for shift in (0..digits).rev() {
        if pos < buf.len() {
            buf[pos] = HEX[(val >> (shift * 4)) & 0xF];
            pos += 1;
        }
    }
    pos
}

unsafe extern "C" fn bridge_call_import(cpu: *mut GuestCpuState, import_index: u32) {
    let Some((state, ctx)) = (unsafe { context_from_cpu(cpu) }) else {
        return;
    };
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }
    dispatch_import_slot(ctx, state, import_index);
}

unsafe extern "C" fn bridge_call_indirect(cpu: *mut GuestCpuState, target_va: u32) {
    let Some((state, ctx)) = (unsafe { context_from_cpu(cpu) }) else {
        return;
    };
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }

    if target_va == GUEST_RETURN_SENTINEL {
        state.exit_reason = ExitReason::Halt as u32;
        state.exit_detail = target_va;
        return;
    }

    if (IMPORT_THUNK_BASE..IMPORT_THUNK_END).contains(&target_va)
        && (target_va - IMPORT_THUNK_BASE).is_multiple_of(THUNK_STRIDE)
    {
        let slot_idx = (target_va - IMPORT_THUNK_BASE) / THUNK_STRIDE;
        dispatch_import_slot(ctx, state, slot_idx);
        return;
    }

    if let Some((iface, method_idx)) = toluca_d3d8::decode_com_thunk(target_va) {
        crate::win32::d3d8::dispatch_com_call(ctx, state, iface, method_idx);
        return;
    }

    if (COM_THUNK_BASE..GUEST_THUNK_END).contains(&target_va) {
        state.exit_reason = ExitReason::HostTrap as u32;
        state.exit_detail = target_va;
        *ctx.trap = Some(TrapInfo::UnimplementedCom {
            interface_id: 0,
            method_index: 0,
        });
        return;
    }

    if let Some(target_fn) = ctx.code.resolve_va(target_va) {
        unsafe {
            target_fn(cpu);
        }
        return;
    }

    state.exit_reason = ExitReason::UnmappedTarget as u32;
    state.exit_detail = target_va;
    *ctx.trap = Some(TrapInfo::UnresolvedIndirectCall(target_va));
}

unsafe extern "C" fn bridge_call_com(
    cpu: *mut GuestCpuState,
    interface_id: u32,
    method_index: u32,
) {
    let Some((state, ctx)) = (unsafe { context_from_cpu(cpu) }) else {
        return;
    };
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }
    if let Some(iface) = toluca_d3d8::ComInterfaceId::from_u32(interface_id) {
        crate::win32::d3d8::dispatch_com_call(ctx, state, iface, method_index);
        return;
    }
    state.exit_reason = ExitReason::HostTrap as u32;
    state.exit_detail = (interface_id << COM_INTERFACE_SHIFT) | (method_index & COM_METHOD_MASK);
    *ctx.trap = Some(TrapInfo::UnimplementedCom {
        interface_id,
        method_index,
    });
}

unsafe extern "C" fn bridge_materialize_flags(cpu: *mut GuestCpuState) {
    if let Some(state) = unsafe { cpu.as_mut() } {
        state.materialize_flags();
    }
}

unsafe extern "C" fn bridge_cpuid(cpu: *mut GuestCpuState) {
    let Some((state, _ctx)) = (unsafe { context_from_cpu(cpu) }) else {
        return;
    };
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }

    match state.gpr(Gpr::Eax) {
        0x0000_0000 => {
            state.set_gpr(Gpr::Eax, CPUID_MAX_BASIC_LEAF);
            state.set_gpr(Gpr::Ebx, CPUID_VENDOR_EBX);
            state.set_gpr(Gpr::Edx, CPUID_VENDOR_EDX);
            state.set_gpr(Gpr::Ecx, CPUID_VENDOR_ECX);
        }
        _ => {
            state.set_gpr(Gpr::Eax, CPUID_PENTIUM_III_SIG);
            state.set_gpr(Gpr::Ebx, 0);
            state.set_gpr(Gpr::Ecx, 0);
            state.set_gpr(Gpr::Edx, CPUID_PENTIUM_III_EDX);
        }
    }
}

unsafe extern "C" fn bridge_rdtsc(cpu: *mut GuestCpuState) {
    let Some((state, ctx)) = (unsafe { context_from_cpu(cpu) }) else {
        return;
    };
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }

    let ticks = ctx.host_start_instant.elapsed().as_nanos() as u64;
    state.set_gpr(Gpr::Eax, ticks as u32);
    state.set_gpr(Gpr::Edx, (ticks >> 32) as u32);
}

unsafe extern "C" fn bridge_unhandled_instruction(
    cpu: *mut GuestCpuState,
    guest_va: u32,
    reason: u32,
) {
    let Some((state, ctx)) = (unsafe { context_from_cpu(cpu) }) else {
        return;
    };
    if ctx.win32.config.trace_imports {
        eprintln!("[unhandled_insn] va=0x{guest_va:08x} reason={reason}");
    }
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }
    state.exit_reason = ExitReason::UnsupportedInstruction as u32;
    state.exit_detail = guest_va;
    *ctx.trap = Some(TrapInfo::UnhandledInstruction { guest_va, reason });
}

unsafe extern "C" fn bridge_trace_hook(_cpu: *mut GuestCpuState, _guest_va: u32) {}
