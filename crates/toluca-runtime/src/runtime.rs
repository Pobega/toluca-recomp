use std::collections::HashMap;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use toluca_core::GuestAddr;
use toluca_core::aot::ImportSymbol;
use toluca_core::bridge::{HostBridge, default_rep_movs, default_rep_stos, default_x87_helper};
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
pub(crate) struct ResolvedImportSlot {
    pub(crate) dll_name: String,
    pub(crate) symbol: ImportSymbol,
    pub(crate) handler: Option<ImportHandler>,
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
    pub active_host_tid: u64,
    pub host_start_instant: Instant,
    pub pe_image_base: GuestAddr,
    pub(crate) code: &'a AotCodeImage,
    pub(crate) imports: &'a mut Vec<ResolvedImportSlot>,
    pub(crate) import_slot_map: &'a mut HashMap<ImportKey, usize>,
    pub(crate) registry: &'a ImportRegistry,
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
        let saved_exit_reason = cpu.exit_reason;
        let saved_exit_detail = cpu.exit_detail;

        push_args_and_sentinel(cpu, self.arena, args)?;
        cpu.eip = target_va;
        cpu.exit_reason = ExitReason::Running as u32;
        cpu.exit_detail = 0;

        unsafe {
            cb_fn(cpu);
        }

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

struct RuntimeInner {
    arena: GuestArena,
    heap: GuestHeap,
    vmem: VirtualMemoryManager,
    win32: Win32State,
    imports: Vec<ResolvedImportSlot>,
    import_slot_map: HashMap<ImportKey, usize>,
    last_error: u32,
}

struct ScheduledTimer {
    timer_id: u32,
    delay: Duration,
    next_fire: Instant,
    callback_va: GuestAddr,
    dw_user: u32,
    periodic: bool,
    cancelled: Arc<AtomicBool>,
}

struct TimerQueueState {
    started: bool,
    timers: Vec<ScheduledTimer>,
}

struct SharedRuntime {
    pe_image_base: GuestAddr,
    code: AotCodeImage,
    registry: ImportRegistry,
    host_start_instant: Instant,
    arena_base: usize,
    inner: Mutex<RuntimeInner>,
    sync_cv: Condvar,
    timer_queue: Mutex<TimerQueueState>,
    timer_cv: Condvar,
    shutdown: AtomicBool,
    worker_done_flags: Mutex<Vec<Arc<AtomicBool>>>,
    guest_thread_tids: Mutex<Vec<(libc::pid_t, String)>>,
}

unsafe impl Send for SharedRuntime {}
unsafe impl Sync for SharedRuntime {}

pub struct LockedRef<'a, T> {
    guard: MutexGuard<'a, RuntimeInner>,
    proj: fn(&RuntimeInner) -> &T,
}

impl<T> Deref for LockedRef<'_, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        (self.proj)(&self.guard)
    }
}

pub struct LockedMut<'a, T> {
    guard: MutexGuard<'a, RuntimeInner>,
    proj_ref: fn(&RuntimeInner) -> &T,
    proj_mut: fn(&mut RuntimeInner) -> &mut T,
}

impl<T> Deref for LockedMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        (self.proj_ref)(&self.guard)
    }
}

impl<T> DerefMut for LockedMut<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        (self.proj_mut)(&mut self.guard)
    }
}

pub struct Runtime {
    pe: PeImage,
    shared: Arc<SharedRuntime>,
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

        let arena_base = arena.base_ptr() as usize;
        let inner = RuntimeInner {
            arena,
            heap,
            vmem: VirtualMemoryManager::new(),
            win32,
            imports,
            import_slot_map,
            last_error: 0,
        };
        let shared = Arc::new(SharedRuntime {
            pe_image_base: pe.image_base,
            code,
            registry,
            host_start_instant: Instant::now(),
            arena_base,
            inner: Mutex::new(inner),
            sync_cv: Condvar::new(),
            timer_queue: Mutex::new(TimerQueueState {
                started: false,
                timers: Vec::new(),
            }),
            timer_cv: Condvar::new(),
            shutdown: AtomicBool::new(false),
            worker_done_flags: Mutex::new(Vec::new()),
            guest_thread_tids: Mutex::new(Vec::new()),
        });

        Ok(Self { pe, shared })
    }

    pub fn register_dynamic_import(
        &mut self,
        dll_name: &str,
        symbol: ImportSymbol,
    ) -> Option<GuestAddr> {
        let mut guard = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
        let inner = &mut *guard;
        register_dynamic_slot(
            &mut inner.imports,
            &mut inner.import_slot_map,
            &self.shared.registry,
            dll_name,
            symbol,
        )
    }

    pub fn run_entry(&mut self) -> Result<RunOutcome, RuntimeError> {
        self.call_va(self.shared.code.entry_point_va(), &[])
    }

    pub fn call_va(&mut self, va: GuestAddr, args: &[u32]) -> Result<RunOutcome, RuntimeError> {
        let entry_fn = self
            .shared
            .code
            .resolve_va(va)
            .ok_or(RuntimeError::UnmappedEntry(va))?;

        self.shared.shutdown.store(false, Ordering::Release);
        let mut cpu = GuestCpuState::new(self.shared.arena_base as *mut u8, std::ptr::null());
        cpu.eip = va;
        cpu.fs_base = GUEST_TEB_ADDR;
        {
            let mut guard = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            push_args_and_sentinel(&mut cpu, &mut guard.arena, args)?;
        }

        let mut tctx = ThreadBridgeContext {
            shared: Arc::clone(&self.shared),
            tid: 1,
            held_inner: None,
            trap: None,
            pending_error: None,
            suspend_slot: None,
            perf: rt_perf_enabled().then(MainThreadPerf::new),
        };
        let bridge = build_host_bridge((&raw mut tctx).cast());
        cpu.bridge = &bridge;

        let _sig_guard = SigsegvDiagnosticGuard::install(
            self.shared.arena_base as *mut u8,
            self.shared.code.code_range(),
            (&raw mut cpu) as usize,
        );
        crate::win32::pin_current_thread_to_guest_core();
        unsafe {
            entry_fn(&mut cpu);
        }
        drop(_sig_guard);

        self.shutdown_workers();

        if let Some(err) = tctx.pending_error {
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
            trap: tctx.trap,
        })
    }

    fn shutdown_workers(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
        crate::win32::wake_all_suspended_threads();
        self.shared.sync_cv.notify_all();
        self.shared.timer_cv.notify_all();
        let flags = {
            let mut list = self
                .shared
                .worker_done_flags
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *list)
        };
        for _ in 0..10 {
            if flags.iter().all(|f| f.load(Ordering::Acquire)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn pe(&self) -> &PeImage {
        &self.pe
    }

    pub fn code(&self) -> &AotCodeImage {
        &self.shared.code
    }

    pub fn arena(&self) -> LockedRef<'_, GuestArena> {
        LockedRef {
            guard: self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()),
            proj: |inner| &inner.arena,
        }
    }

    pub fn arena_mut(&mut self) -> LockedMut<'_, GuestArena> {
        LockedMut {
            guard: self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()),
            proj_ref: |inner| &inner.arena,
            proj_mut: |inner| &mut inner.arena,
        }
    }

    pub fn heap(&self) -> LockedRef<'_, GuestHeap> {
        LockedRef {
            guard: self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()),
            proj: |inner| &inner.heap,
        }
    }

    pub fn vmem(&self) -> LockedRef<'_, VirtualMemoryManager> {
        LockedRef {
            guard: self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()),
            proj: |inner| &inner.vmem,
        }
    }

    pub fn win32(&self) -> LockedRef<'_, Win32State> {
        LockedRef {
            guard: self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()),
            proj: |inner| &inner.win32,
        }
    }

    pub fn win32_mut(&mut self) -> LockedMut<'_, Win32State> {
        LockedMut {
            guard: self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()),
            proj_ref: |inner| &inner.win32,
            proj_mut: |inner| &mut inner.win32,
        }
    }

    pub fn last_error(&self) -> u32 {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_error
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
        7 => ExitReason::Longjmp,
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
        rep_movs: default_rep_movs,
        rep_stos: default_rep_stos,
    }
}

const RT_PERF_LOG_INTERVAL: Duration = Duration::from_secs(1);
const RT_PERF_CHECK_STRIDE: u32 = 256;
#[cfg(target_os = "linux")]
const BG_THREAD_REPORT_MIN_MS: u64 = 10;

fn rt_perf_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("TOLUCA_PERF").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

fn current_os_tid() -> libc::pid_t {
    #[cfg(target_os = "linux")]
    {
        unsafe { libc::syscall(libc::SYS_gettid) as libc::pid_t }
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

fn thread_cpu_time_ns() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        if rc == 0 {
            return (ts.tv_sec as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(ts.tv_nsec as u64);
        }
        0
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

#[cfg(target_os = "linux")]
fn linux_clk_tck() -> u64 {
    static CLK_TCK: OnceLock<u64> = OnceLock::new();
    *CLK_TCK.get_or_init(|| {
        let val = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if val > 0 { val as u64 } else { 100 }
    })
}

#[cfg(target_os = "linux")]
fn read_task_stat_ticks_and_nice(os_tid: libc::pid_t) -> Option<(u64, i64)> {
    let path = format!("/proc/self/task/{os_tid}/stat");
    let contents = std::fs::read_to_string(path).ok()?;
    let rparen = contents.rfind(')')?;
    let rest = contents.get(rparen + 1..)?;
    let mut fields = rest.split_whitespace();
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    let nice: i64 = fields.nth(3)?.parse().ok()?;
    Some((utime.saturating_add(stime), nice))
}

fn format_bg_thread_summary(
    main_os_tid: libc::pid_t,
    guest_thread_tids: &Mutex<Vec<(libc::pid_t, String)>>,
    prev_bg_ticks: &mut HashMap<libc::pid_t, u64>,
) -> (i64, String) {
    #[cfg(target_os = "linux")]
    {
        let main_nice = read_task_stat_ticks_and_nice(main_os_tid).map_or(0, |(_, n)| n);
        let tracked = guest_thread_tids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        prev_bg_ticks.retain(|tid, _| tracked.iter().any(|(t, _)| t == tid));
        let clk_tck = linux_clk_tck().max(1);
        let mut bg_entries: Vec<(u64, String, i64)> = Vec::new();
        for (tid, label) in tracked {
            if let Some((ticks, nice)) = read_task_stat_ticks_and_nice(tid) {
                let prev = prev_bg_ticks.insert(tid, ticks).unwrap_or(0);
                let delta_ms = ticks.saturating_sub(prev).saturating_mul(1000) / clk_tck;
                if delta_ms >= BG_THREAD_REPORT_MIN_MS {
                    bg_entries.push((delta_ms, label, nice));
                }
            }
        }
        bg_entries.sort_by(|a, b| b.0.cmp(&a.0));
        let bg_str = bg_entries
            .into_iter()
            .map(|(ms, label, nice)| format!("{label}:{ms}ms/n{nice}"))
            .collect::<Vec<_>>()
            .join(" ");
        (main_nice, bg_str)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (main_os_tid, guest_thread_tids, prev_bg_ticks);
        (0, String::new())
    }
}

struct GuestTidPerfGuard {
    shared: Arc<SharedRuntime>,
    os_tid: libc::pid_t,
}

impl GuestTidPerfGuard {
    fn enter(shared: &Arc<SharedRuntime>, os_tid: libc::pid_t, label: String) -> Option<Self> {
        if !rt_perf_enabled() || os_tid == 0 {
            return None;
        }
        shared
            .guest_thread_tids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((os_tid, label));
        Some(Self {
            shared: Arc::clone(shared),
            os_tid,
        })
    }
}

impl Drop for GuestTidPerfGuard {
    fn drop(&mut self) {
        self.shared
            .guest_thread_tids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(tid, _)| *tid != self.os_tid);
    }
}

fn format_import_slot_name(imports: &[ResolvedImportSlot], slot_idx: u32) -> String {
    match imports.get(slot_idx as usize) {
        Some(slot) => match &slot.symbol {
            ImportSymbol::Name(name) => name.clone(),
            ImportSymbol::Ordinal(ord) => format!("{}#{}", slot.dll_name, ord),
        },
        None => format!("slot#{slot_idx}"),
    }
}

#[derive(Clone, Copy)]
enum PerfCallKind {
    Import(u32),
    Com { iface: u32, method: u32 },
}

struct MainThreadPerf {
    main_os_tid: libc::pid_t,
    wall_start: Instant,
    cpu_start_ns: u64,
    lock_wait_ns: u64,
    imp_ns: u64,
    imp_calls: u64,
    com_ns: u64,
    com_calls: u64,
    ind_calls: u64,
    ind_lookup_ns: u64,
    wait_ns: u64,
    imp_by_slot: HashMap<u32, (u64, u64)>,
    com_by_method: HashMap<(u32, u32), (u64, u64)>,
    prev_bg_ticks: HashMap<libc::pid_t, u64>,
    check_counter: u32,
}

impl MainThreadPerf {
    fn new() -> Self {
        Self {
            main_os_tid: current_os_tid(),
            wall_start: Instant::now(),
            cpu_start_ns: thread_cpu_time_ns(),
            lock_wait_ns: 0,
            imp_ns: 0,
            imp_calls: 0,
            com_ns: 0,
            com_calls: 0,
            ind_calls: 0,
            ind_lookup_ns: 0,
            wait_ns: 0,
            imp_by_slot: HashMap::new(),
            com_by_method: HashMap::new(),
            prev_bg_ticks: HashMap::new(),
            check_counter: 0,
        }
    }

    fn record_import(&mut self, slot: u32, elapsed_ns: u64) {
        self.imp_calls += 1;
        self.imp_ns += elapsed_ns;
        let entry = self.imp_by_slot.entry(slot).or_insert((0, 0));
        entry.0 += elapsed_ns;
        entry.1 += 1;
    }

    fn record_com(&mut self, iface: u32, method: u32, elapsed_ns: u64) {
        self.com_calls += 1;
        self.com_ns += elapsed_ns;
        let entry = self.com_by_method.entry((iface, method)).or_insert((0, 0));
        entry.0 += elapsed_ns;
        entry.1 += 1;
    }

    fn needs_flush(&self) -> bool {
        self.wall_start.elapsed() >= RT_PERF_LOG_INTERVAL
    }

    fn maybe_flush(
        &mut self,
        guest_thread_tids: &Mutex<Vec<(libc::pid_t, String)>>,
        imports: &[ResolvedImportSlot],
        force_check: bool,
    ) {
        if !force_check {
            self.check_counter = self.check_counter.wrapping_add(1);
            if !self.check_counter.is_multiple_of(RT_PERF_CHECK_STRIDE) {
                return;
            }
        }
        let now = Instant::now();
        let wall_dur = now.duration_since(self.wall_start);
        if wall_dur < RT_PERF_LOG_INTERVAL {
            return;
        }
        let cpu_now_ns = thread_cpu_time_ns();
        let cpu_ns = cpu_now_ns.saturating_sub(self.cpu_start_ns);
        let wall_ms = wall_dur.as_millis() as u64;
        let cpu_ms = cpu_ns / 1_000_000;
        let wait_ms = self.wait_ns / 1_000_000;
        let lock_ms = self.lock_wait_ns / 1_000_000;
        let imp_ms = self.imp_ns / 1_000_000;
        let com_ms = self.com_ns / 1_000_000;
        let host_ns = self.imp_ns.saturating_add(self.com_ns);
        let host_ms = host_ns / 1_000_000;
        let preempt_ms = wall_ms.saturating_sub(wait_ms).saturating_sub(cpu_ms);
        let guest_ms = cpu_ms.saturating_sub(host_ms);
        let ind_ms = (self.ind_lookup_ns as f64) / 1_000_000.0;

        let (top_com_iface, top_com_method, top_com_ms, top_com_calls) = self
            .com_by_method
            .iter()
            .max_by_key(|(_, (ns, _))| *ns)
            .map_or(("none", 0, 0.0, 0), |(&(iface_id, method), &(ns, calls))| {
                let iface_name = toluca_d3d8::ComInterfaceId::from_u32(iface_id)
                    .map_or("unknown", |id| id.name());
                (iface_name, method, (ns as f64) / 1_000_000.0, calls)
            });

        let (top_imp_name, top_imp_ms, top_imp_calls) = self
            .imp_by_slot
            .iter()
            .max_by_key(|(_, (ns, _))| *ns)
            .map_or_else(
                || ("none".to_string(), 0.0, 0),
                |(&slot_idx, &(ns, calls))| {
                    let name = format_import_slot_name(imports, slot_idx);
                    (name, (ns as f64) / 1_000_000.0, calls)
                },
            );

        let (main_nice, bg_str) = format_bg_thread_summary(
            self.main_os_tid,
            guest_thread_tids,
            &mut self.prev_bg_ticks,
        );

        eprintln!(
            "[toluca-rt] wall={wall_ms}ms cpu={cpu_ms}ms preempt={preempt_ms}ms wait={wait_ms}ms host={host_ms}ms(lock={lock_ms}ms imp={imp_ms}ms/{} com={com_ms}ms/{}) guest={guest_ms}ms ind={}({ind_ms:.1}ms) top_com={top_com_iface}:{top_com_method}({top_com_ms:.1}ms/{top_com_calls}) top_imp={top_imp_name}({top_imp_ms:.1}ms/{top_imp_calls}) mn={main_nice} bg=[{bg_str}]",
            self.imp_calls,
            self.com_calls,
            self.ind_calls,
        );

        self.wall_start = now;
        self.cpu_start_ns = cpu_now_ns;
        self.lock_wait_ns = 0;
        self.imp_ns = 0;
        self.imp_calls = 0;
        self.com_ns = 0;
        self.com_calls = 0;
        self.ind_calls = 0;
        self.ind_lookup_ns = 0;
        self.wait_ns = 0;
        self.imp_by_slot.clear();
        self.com_by_method.clear();
    }
}

struct ThreadBridgeContext {
    shared: Arc<SharedRuntime>,
    tid: u64,
    held_inner: Option<*mut RuntimeInner>,
    trap: Option<TrapInfo>,
    pending_error: Option<RuntimeError>,
    suspend_slot: Option<&'static crate::win32::ThreadSuspendState>,
    perf: Option<MainThreadPerf>,
}

unsafe fn thread_ctx_from_cpu<'a>(
    cpu: *mut GuestCpuState,
) -> Option<(&'a mut GuestCpuState, &'a mut ThreadBridgeContext)> {
    let state = unsafe { cpu.as_mut() }?;
    let bridge = unsafe { state.bridge.as_ref() }?;
    let tctx = unsafe { bridge.user_data.cast::<ThreadBridgeContext>().as_mut() }?;
    Some((state, tctx))
}

fn with_locked_ctx<R>(
    tctx: &mut ThreadBridgeContext,
    state: &mut GuestCpuState,
    perf_kind: PerfCallKind,
    f: impl FnOnce(&mut RuntimeContext<'_>, &mut GuestCpuState) -> R,
) -> R {
    if let Some(inner_ptr) = tctx.held_inner {
        let inner = unsafe { &mut *inner_ptr };
        let mut ctx = RuntimeContext {
            arena: &mut inner.arena,
            heap: &mut inner.heap,
            vmem: &mut inner.vmem,
            win32: &mut inner.win32,
            last_error: &mut inner.last_error,
            active_host_tid: tctx.tid,
            host_start_instant: tctx.shared.host_start_instant,
            pe_image_base: tctx.shared.pe_image_base,
            code: &tctx.shared.code,
            imports: &mut inner.imports,
            import_slot_map: &mut inner.import_slot_map,
            registry: &tctx.shared.registry,
            trap: &mut tctx.trap,
            pending_error: &mut tctx.pending_error,
        };
        return f(&mut ctx, state);
    }

    if let Some(slot) = tctx.suspend_slot {
        slot.in_host_code.store(true, Ordering::SeqCst);
    }

    let perf_enabled = tctx.perf.is_some();
    let (
        result,
        pending_wait,
        pending_threads,
        pending_timers,
        killed_timers,
        wake_slots,
        notify_sync,
        lock_ns,
        call_ns,
    ) = {
        let lock_t0 = perf_enabled.then(Instant::now);
        let mut guard = tctx.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
        let lock_ns = lock_t0.map_or(0, |t| t.elapsed().as_nanos() as u64);
        tctx.held_inner = Some(&raw mut *guard);
        let inner = &mut *guard;
        let mut ctx = RuntimeContext {
            arena: &mut inner.arena,
            heap: &mut inner.heap,
            vmem: &mut inner.vmem,
            win32: &mut inner.win32,
            last_error: &mut inner.last_error,
            active_host_tid: tctx.tid,
            host_start_instant: tctx.shared.host_start_instant,
            pe_image_base: tctx.shared.pe_image_base,
            code: &tctx.shared.code,
            imports: &mut inner.imports,
            import_slot_map: &mut inner.import_slot_map,
            registry: &tctx.shared.registry,
            trap: &mut tctx.trap,
            pending_error: &mut tctx.pending_error,
        };
        let call_t0 = perf_enabled.then(Instant::now);
        let res = f(&mut ctx, state);
        let call_ns = call_t0.map_or(0, |t| t.elapsed().as_nanos() as u64);
        let wait = inner.win32.pending_wait.take();
        let threads = std::mem::take(&mut inner.win32.pending_threads);
        let timers = std::mem::take(&mut inner.win32.pending_timers);
        let killed = std::mem::take(&mut inner.win32.pending_killed_timers);
        let wake_slots = std::mem::take(&mut inner.win32.pending_wake_slots);
        let notify = std::mem::replace(&mut inner.win32.notify_sync, false);
        tctx.held_inner = None;
        if let Some(perf) = tctx.perf.as_mut() {
            perf.lock_wait_ns += lock_ns;
            match perf_kind {
                PerfCallKind::Import(slot) => perf.record_import(slot, call_ns),
                PerfCallKind::Com { iface, method } => perf.record_com(iface, method, call_ns),
            }
            if wait.is_none() {
                perf.maybe_flush(&tctx.shared.guest_thread_tids, &inner.imports, false);
            }
        }
        (
            res,
            wait,
            threads,
            timers,
            killed,
            wake_slots,
            notify,
            lock_ns,
            call_ns,
        )
    };
    let _ = (lock_ns, call_ns);

    if notify_sync {
        tctx.shared.sync_cv.notify_all();
    }
    for slot_idx in wake_slots {
        crate::win32::wake_suspended_thread(Some(slot_idx));
    }
    for spawn in pending_threads {
        spawn_guest_thread(Arc::clone(&tctx.shared), spawn);
    }
    if !pending_timers.is_empty() || !killed_timers.is_empty() {
        schedule_timers(&tctx.shared, pending_timers, killed_timers);
    }
    if let Some(wait) = pending_wait {
        let wait_t0 = tctx.perf.is_some().then(Instant::now);
        perform_host_wait(&tctx.shared, tctx.tid, state, wait);
        if let (Some(perf), Some(start)) = (tctx.perf.as_mut(), wait_t0) {
            perf.wait_ns += start.elapsed().as_nanos() as u64;
            if perf.needs_flush() {
                let guard = tctx.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
                perf.maybe_flush(&tctx.shared.guest_thread_tids, &guard.imports, true);
            }
        }
    }
    if let Some(slot) = tctx.suspend_slot {
        slot.in_host_code.store(false, Ordering::SeqCst);
        crate::win32::park_while_suspended(slot, &tctx.shared.shutdown);
    }
    result
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
static CODE_BASE_ADDR: AtomicUsize = AtomicUsize::new(0);
static CODE_END_ADDR: AtomicUsize = AtomicUsize::new(0);
static CPU_STATE_ADDR: AtomicUsize = AtomicUsize::new(0);
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
    fn install(arena_base: *mut u8, code_range: (usize, usize), cpu_addr: usize) -> Self {
        let base = arena_base as usize;
        let end = base.saturating_add(GUEST_ARENA_SIZE as usize);
        ARENA_BASE_ADDR.store(base, Ordering::Relaxed);
        ARENA_END_ADDR.store(end, Ordering::Relaxed);
        CODE_BASE_ADDR.store(code_range.0, Ordering::Relaxed);
        CODE_END_ADDR.store(code_range.1, Ordering::Relaxed);
        CPU_STATE_ADDR.store(cpu_addr, Ordering::Relaxed);

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
        CODE_BASE_ADDR.store(0, Ordering::Relaxed);
        CODE_END_ADDR.store(0, Ordering::Relaxed);
        CPU_STATE_ADDR.store(0, Ordering::Relaxed);
    }
}

extern "C" fn sigsegv_diagnostic_handler(
    sig: libc::c_int,
    info: *mut libc::siginfo_t,
    ucontext: *mut core::ffi::c_void,
) {
    let fault_addr = if info.is_null() {
        0usize
    } else {
        unsafe { (*info).si_addr() as usize }
    };
    let base = ARENA_BASE_ADDR.load(Ordering::Relaxed);
    let end = ARENA_END_ADDR.load(Ordering::Relaxed);
    let code_base = CODE_BASE_ADDR.load(Ordering::Relaxed);
    let code_end = CODE_END_ADDR.load(Ordering::Relaxed);

    let mut out = [0u8; 320];
    let mut pos = 0usize;
    pos = push_bytes(&mut out, pos, b"[SIGSEGV] host=0x");
    pos = push_hex_usize(&mut out, pos, fault_addr);
    if base != 0 && fault_addr >= base && fault_addr < end {
        pos = push_bytes(&mut out, pos, b" guest=0x");
        pos = push_hex_usize(&mut out, pos, fault_addr - base);
    } else {
        pos = push_bytes(&mut out, pos, b" (outside guest arena)");
    }

    #[cfg(target_arch = "x86_64")]
    if !ucontext.is_null() {
        let uc = ucontext.cast::<libc::ucontext_t>();
        let rip = unsafe { (*uc).uc_mcontext.gregs[libc::REG_RIP as usize] as usize };
        pos = push_bytes(&mut out, pos, b" rip=0x");
        pos = push_hex_usize(&mut out, pos, rip);
        if code_base != 0 && rip >= code_base && rip < code_end {
            pos = push_bytes(&mut out, pos, b" code_off=0x");
            pos = push_hex_usize(&mut out, pos, rip - code_base);
        }
    }

    #[cfg(target_arch = "aarch64")]
    if !ucontext.is_null() {
        let uc = ucontext.cast::<libc::ucontext_t>();
        let pc = unsafe { (*uc).uc_mcontext.pc as usize };
        pos = push_bytes(&mut out, pos, b" pc=0x");
        pos = push_hex_usize(&mut out, pos, pc);
        if code_base != 0 && pc >= code_base && pc < code_end {
            pos = push_bytes(&mut out, pos, b" code_off=0x");
            pos = push_hex_usize(&mut out, pos, pc - code_base);
        }
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = (ucontext, code_base, code_end);

    let cpu_addr = CPU_STATE_ADDR.load(Ordering::Relaxed);
    if cpu_addr != 0 {
        let eip = unsafe { (*(cpu_addr as *const GuestCpuState)).eip };
        pos = push_bytes(&mut out, pos, b" eip=0x");
        pos = push_hex_usize(&mut out, pos, eip as usize);
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
    let Some((state, tctx)) = (unsafe { thread_ctx_from_cpu(cpu) }) else {
        return;
    };
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }
    if tctx.shared.shutdown.load(Ordering::Acquire) && tctx.tid != 1 {
        state.exit_reason = ExitReason::Halt as u32;
        return;
    }
    with_locked_ctx(tctx, state, PerfCallKind::Import(import_index), |ctx, state| {
        dispatch_import_slot(ctx, state, import_index);
    });
}

unsafe extern "C" fn bridge_call_indirect(cpu: *mut GuestCpuState, target_va: u32) {
    let Some((state, tctx)) = (unsafe { thread_ctx_from_cpu(cpu) }) else {
        return;
    };
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }
    if tctx.shared.shutdown.load(Ordering::Acquire) && tctx.tid != 1 {
        state.exit_reason = ExitReason::Halt as u32;
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
        with_locked_ctx(tctx, state, PerfCallKind::Import(slot_idx), |ctx, state| {
            dispatch_import_slot(ctx, state, slot_idx);
        });
        return;
    }

    if let Some((iface, method_idx)) = toluca_d3d8::decode_com_thunk(target_va) {
        with_locked_ctx(
            tctx,
            state,
            PerfCallKind::Com {
                iface: iface as u32,
                method: method_idx,
            },
            |ctx, state| {
                crate::win32::d3d8::dispatch_com_call(ctx, state, iface, method_idx);
            },
        );
        return;
    }

    if (COM_THUNK_BASE..GUEST_THUNK_END).contains(&target_va) {
        state.exit_reason = ExitReason::HostTrap as u32;
        state.exit_detail = target_va;
        tctx.trap = Some(TrapInfo::UnimplementedCom {
            interface_id: 0,
            method_index: 0,
        });
        return;
    }

    let lookup_t0 = tctx.perf.is_some().then(Instant::now);
    let resolved = tctx.shared.code.resolve_va(target_va);
    if let (Some(perf), Some(start)) = (tctx.perf.as_mut(), lookup_t0) {
        perf.ind_calls += 1;
        perf.ind_lookup_ns += start.elapsed().as_nanos() as u64;
    }
    if let Some(target_fn) = resolved {
        unsafe {
            target_fn(cpu);
        }
        return;
    }

    state.exit_reason = ExitReason::UnmappedTarget as u32;
    state.exit_detail = target_va;
    tctx.trap = Some(TrapInfo::UnresolvedIndirectCall(target_va));
}

unsafe extern "C" fn bridge_call_com(
    cpu: *mut GuestCpuState,
    interface_id: u32,
    method_index: u32,
) {
    let Some((state, tctx)) = (unsafe { thread_ctx_from_cpu(cpu) }) else {
        return;
    };
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }
    if let Some(iface) = toluca_d3d8::ComInterfaceId::from_u32(interface_id) {
        with_locked_ctx(
            tctx,
            state,
            PerfCallKind::Com {
                iface: interface_id,
                method: method_index,
            },
            |ctx, state| {
                crate::win32::d3d8::dispatch_com_call(ctx, state, iface, method_index);
            },
        );
        return;
    }
    state.exit_reason = ExitReason::HostTrap as u32;
    state.exit_detail = (interface_id << COM_INTERFACE_SHIFT) | (method_index & COM_METHOD_MASK);
    tctx.trap = Some(TrapInfo::UnimplementedCom {
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
    let Some((state, _tctx)) = (unsafe { thread_ctx_from_cpu(cpu) }) else {
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
    let Some((state, tctx)) = (unsafe { thread_ctx_from_cpu(cpu) }) else {
        return;
    };
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }

    let ticks = tctx.shared.host_start_instant.elapsed().as_nanos() as u64;
    state.set_gpr(Gpr::Eax, ticks as u32);
    state.set_gpr(Gpr::Edx, (ticks >> 32) as u32);
}

unsafe extern "C" fn bridge_unhandled_instruction(
    cpu: *mut GuestCpuState,
    guest_va: u32,
    reason: u32,
) {
    let Some((state, tctx)) = (unsafe { thread_ctx_from_cpu(cpu) }) else {
        return;
    };
    if state.exit_reason != ExitReason::Running as u32 {
        return;
    }
    state.exit_reason = ExitReason::UnsupportedInstruction as u32;
    state.exit_detail = guest_va;
    tctx.trap = Some(TrapInfo::UnhandledInstruction { guest_va, reason });
}

unsafe extern "C" fn bridge_trace_hook(_cpu: *mut GuestCpuState, _guest_va: u32) {}

fn perform_host_wait(
    shared: &Arc<SharedRuntime>,
    tid: u64,
    cpu: &mut GuestCpuState,
    wait: crate::win32::PendingHostWait,
) {
    const INFINITE_MS: u32 = 0xFFFF_FFFF;
    const WAIT_OBJECT_0: u32 = 0;

    match wait {
        crate::win32::PendingHostWait::SleepMs(ms) => {
            if ms == 0 {
                std::thread::sleep(Duration::from_micros(250));
                return;
            }
            let deadline = Instant::now() + Duration::from_millis(ms as u64);
            let mut guard = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if shared.shutdown.load(Ordering::Acquire) {
                    if tid != 1 {
                        cpu.exit_reason = ExitReason::Halt as u32;
                    }
                    break;
                }
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let rem = (deadline - now).min(Duration::from_millis(20));
                let (next_guard, _) = shared
                    .sync_cv
                    .wait_timeout(guard, rem)
                    .unwrap_or_else(|e| e.into_inner());
                guard = next_guard;
            }
        }
        crate::win32::PendingHostWait::WaitObject {
            handle,
            timeout_ms,
            initial_pulse,
        } => {
            let deadline =
                (timeout_ms != INFINITE_MS).then(|| Instant::now() + Duration::from_millis(timeout_ms as u64));
            let mut guard = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if shared.shutdown.load(Ordering::Acquire) {
                    if tid != 1 {
                        cpu.exit_reason = ExitReason::Halt as u32;
                    }
                    break;
                }
                match guard.win32.get_handle_mut(handle) {
                    Some(crate::win32::Win32Handle::Event {
                        manual_reset,
                        signaled,
                        pulse_seq,
                    }) => {
                        let pulsed = initial_pulse.is_some_and(|p| *pulse_seq != p);
                        if *signaled || pulsed {
                            if !*manual_reset {
                                *signaled = false;
                            }
                            cpu.set_gpr(Gpr::Eax, WAIT_OBJECT_0);
                            break;
                        }
                    }
                    Some(crate::win32::Win32Handle::Mutex {
                        owner_tid,
                        recursion,
                    }) => {
                        if *owner_tid == 0 || *owner_tid == tid {
                            *owner_tid = tid;
                            *recursion = recursion.saturating_add(1);
                            cpu.set_gpr(Gpr::Eax, WAIT_OBJECT_0);
                            break;
                        }
                    }
                    Some(crate::win32::Win32Handle::Thread { finished, .. }) => {
                        if *finished {
                            cpu.set_gpr(Gpr::Eax, WAIT_OBJECT_0);
                            break;
                        }
                    }
                    _ => break,
                }
                let wait_dur = match deadline {
                    Some(dl) => {
                        let now = Instant::now();
                        if now >= dl {
                            break;
                        }
                        (dl - now).min(Duration::from_millis(20))
                    }
                    None => Duration::from_millis(20),
                };
                let (next_guard, _) = shared
                    .sync_cv
                    .wait_timeout(guard, wait_dur)
                    .unwrap_or_else(|e| e.into_inner());
                guard = next_guard;
            }
        }
        crate::win32::PendingHostWait::EnterCriticalSection { cs_addr } => {
            let mut guard = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if shared.shutdown.load(Ordering::Acquire) {
                    if tid != 1 {
                        cpu.exit_reason = ExitReason::Halt as u32;
                    }
                    break;
                }
                let entry = guard.win32.crit_sections.entry(cs_addr).or_insert((0, 0));
                if entry.0 == 0 || entry.0 == tid {
                    entry.0 = tid;
                    entry.1 = entry.1.saturating_add(1);
                    break;
                }
                let (next_guard, _) = shared
                    .sync_cv
                    .wait_timeout(guard, Duration::from_millis(10))
                    .unwrap_or_else(|e| e.into_inner());
                guard = next_guard;
            }
        }
        crate::win32::PendingHostWait::SuspendSelf { handle } => {
            let mut guard = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if shared.shutdown.load(Ordering::Acquire) {
                    if tid != 1 {
                        cpu.exit_reason = ExitReason::Halt as u32;
                    }
                    break;
                }
                let suspended = matches!(
                    guard.win32.get_handle(handle),
                    Some(crate::win32::Win32Handle::Thread {
                        suspended: true,
                        ..
                    })
                );
                if !suspended {
                    break;
                }
                let (next_guard, _) = shared
                    .sync_cv
                    .wait_timeout(guard, Duration::from_millis(20))
                    .unwrap_or_else(|e| e.into_inner());
                guard = next_guard;
            }
        }
    }
}

fn spawn_guest_thread(shared: Arc<SharedRuntime>, spawn: crate::win32::PendingThreadSpawn) {
    let done = Arc::new(AtomicBool::new(false));
    {
        let mut flags = shared
            .worker_done_flags
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        flags.push(Arc::clone(&done));
    }
    let thread_name = format!("guest_{:x}", spawn.start_va);
    let _ = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let os_tid = current_os_tid();
            let _tid_perf_guard =
                GuestTidPerfGuard::enter(&shared, os_tid, format!("{:x}", spawn.start_va));

            let slot_idx = crate::win32::register_thread_suspend_slot(os_tid);
            let suspend_slot = slot_idx.and_then(crate::win32::thread_suspend_slot);
            crate::win32::pin_current_thread_to_guest_core();

            let thread_priority = {
                let mut guard = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(crate::win32::Win32Handle::Thread {
                    host_tid,
                    suspend_slot: handle_slot,
                    ..
                }) = guard.win32.get_handle_mut(spawn.handle)
                {
                    *host_tid = Some(os_tid);
                    *handle_slot = slot_idx;
                }
                loop {
                    if shared.shutdown.load(Ordering::Acquire) {
                        crate::win32::unregister_thread_suspend_slot(slot_idx);
                        done.store(true, Ordering::Release);
                        return;
                    }
                    let (suspended, prio) = match guard.win32.get_handle(spawn.handle) {
                        Some(crate::win32::Win32Handle::Thread {
                            suspended,
                            priority,
                            ..
                        }) => (*suspended, *priority),
                        _ => (false, 0),
                    };
                    if !suspended {
                        break prio;
                    }
                    let (next_guard, _) = shared
                        .sync_cv
                        .wait_timeout(guard, Duration::from_millis(10))
                        .unwrap_or_else(|e| e.into_inner());
                    guard = next_guard;
                }
            };
            crate::win32::apply_host_thread_priority(os_tid, thread_priority);

            if let Some(entry_fn) = shared.code.resolve_va(spawn.start_va) {
                let mut cpu = GuestCpuState::new(shared.arena_base as *mut u8, std::ptr::null());
                cpu.eip = spawn.start_va;
                cpu.fs_base = spawn.teb_va;
                cpu.set_gpr(Gpr::Esp, spawn.stack_top);
                let pushed = {
                    let mut guard = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
                    push_args_and_sentinel(&mut cpu, &mut guard.arena, &[spawn.param]).is_ok()
                };
                if pushed {
                    let mut tctx = ThreadBridgeContext {
                        shared: Arc::clone(&shared),
                        tid: spawn.handle as u64,
                        held_inner: None,
                        trap: None,
                        pending_error: None,
                        suspend_slot,
                        perf: None,
                    };
                    let bridge = build_host_bridge((&raw mut tctx).cast());
                    cpu.bridge = &bridge;
                    if let Some(slot) = suspend_slot {
                        slot.in_host_code.store(false, Ordering::SeqCst);
                        crate::win32::park_while_suspended(slot, &shared.shutdown);
                    }
                    unsafe {
                        entry_fn(&mut cpu);
                    }
                    if let Some(slot) = suspend_slot {
                        slot.in_host_code.store(true, Ordering::SeqCst);
                    }
                }
                let exit_val = cpu.gpr(Gpr::Eax);
                let mut guard = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(crate::win32::Win32Handle::Thread {
                    exit_code,
                    finished,
                    ..
                }) = guard.win32.get_handle_mut(spawn.handle)
                {
                    *exit_code = exit_val;
                    *finished = true;
                }
                shared.sync_cv.notify_all();
            }
            crate::win32::unregister_thread_suspend_slot(slot_idx);
            done.store(true, Ordering::Release);
        });
}

fn schedule_timers(
    shared: &Arc<SharedRuntime>,
    pending: Vec<crate::win32::PendingTimerSpawn>,
    killed: Vec<u32>,
) {
    let mut spawn_worker = false;
    {
        let mut tq = shared
            .timer_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !killed.is_empty() {
            tq.timers
                .retain(|t| !killed.contains(&t.timer_id) && !t.cancelled.load(Ordering::Acquire));
        }
        let now = Instant::now();
        for t in pending {
            if t.cancelled.load(Ordering::Acquire) {
                continue;
            }
            let delay = Duration::from_millis(t.delay_ms.max(1) as u64);
            tq.timers.push(ScheduledTimer {
                timer_id: t.timer_id,
                delay,
                next_fire: now + delay,
                callback_va: t.callback_va,
                dw_user: t.dw_user,
                periodic: t.periodic,
                cancelled: t.cancelled,
            });
        }
        if !tq.timers.is_empty() && !tq.started {
            tq.started = true;
            spawn_worker = true;
        }
    }

    if spawn_worker {
        let done = Arc::new(AtomicBool::new(false));
        {
            let mut flags = shared
                .worker_done_flags
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            flags.push(Arc::clone(&done));
        }
        let worker_shared = Arc::clone(shared);
        let _ = std::thread::Builder::new()
            .name("guest_timer".to_string())
            .spawn(move || timer_worker_loop(worker_shared, done));
    }
    shared.timer_cv.notify_all();
}

fn timer_worker_loop(shared: Arc<SharedRuntime>, done: Arc<AtomicBool>) {
    const TIMER_STACK_BYTES: usize = 64 * 1024;
    const TIMER_TEB_BYTES: usize = 0x1000;

    let _tid_perf_guard =
        GuestTidPerfGuard::enter(&shared, current_os_tid(), "timer".to_string());
    crate::win32::pin_current_thread_to_guest_core();

    let (stack_top, teb_va) = {
        let mut guard = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
        let inner = &mut *guard;
        let Ok(stack_base) = inner.vmem.virtual_alloc(
            &mut inner.arena,
            None,
            TIMER_STACK_BYTES,
            crate::heap::MEM_RESERVE | crate::heap::MEM_COMMIT,
            crate::heap::PAGE_READWRITE,
        ) else {
            done.store(true, Ordering::Release);
            return;
        };
        let Ok(teb) = inner.heap.alloc(&mut inner.arena, TIMER_TEB_BYTES, true) else {
            done.store(true, Ordering::Release);
            return;
        };
        (stack_base.wrapping_add(TIMER_STACK_BYTES as u32), teb)
    };

    loop {
        let due = {
            let mut tq = shared
                .timer_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            loop {
                if shared.shutdown.load(Ordering::Acquire) {
                    done.store(true, Ordering::Release);
                    return;
                }
                tq.timers
                    .retain(|t| !t.cancelled.load(Ordering::Acquire));
                let earliest_idx = tq
                    .timers
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, t)| t.next_fire)
                    .map(|(i, _)| i);
                let Some(idx) = earliest_idx else {
                    tq = shared
                        .timer_cv
                        .wait(tq)
                        .unwrap_or_else(|e| e.into_inner());
                    continue;
                };
                let now = Instant::now();
                let next_fire = tq.timers[idx].next_fire;
                if now >= next_fire {
                    break tq.timers.swap_remove(idx);
                }
                let rem = next_fire.saturating_duration_since(now);
                let (next_tq, _) = shared
                    .timer_cv
                    .wait_timeout(tq, rem)
                    .unwrap_or_else(|e| e.into_inner());
                tq = next_tq;
            }
        };

        if shared.shutdown.load(Ordering::Acquire) || due.cancelled.load(Ordering::Acquire) {
            continue;
        }

        if let Some(cb_fn) = shared.code.resolve_va(due.callback_va) {
            let tid = 0x10_000u64 + due.timer_id as u64;
            let mut cpu = GuestCpuState::new(shared.arena_base as *mut u8, std::ptr::null());
            cpu.eip = due.callback_va;
            cpu.fs_base = teb_va;
            cpu.set_gpr(Gpr::Esp, stack_top);
            let pushed = {
                let mut guard = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
                push_args_and_sentinel(
                    &mut cpu,
                    &mut guard.arena,
                    &[due.timer_id, 0, due.dw_user, 0, 0],
                )
                .is_ok()
            };
            if pushed {
                let mut tctx = ThreadBridgeContext {
                    shared: Arc::clone(&shared),
                    tid,
                    held_inner: None,
                    trap: None,
                    pending_error: None,
                    suspend_slot: None,
                    perf: None,
                };
                let bridge = build_host_bridge((&raw mut tctx).cast());
                cpu.bridge = &bridge;
                unsafe {
                    cb_fn(&mut cpu);
                }
            }
        }

        if due.periodic
            && !due.cancelled.load(Ordering::Acquire)
            && !shared.shutdown.load(Ordering::Acquire)
        {
            let mut next = due;
            next.next_fire = (next.next_fire + next.delay).max(Instant::now());
            let mut tq = shared
                .timer_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            tq.timers.push(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn timer_worker_reuses_single_thread_and_stack_across_repeated_oneshots() {
        let mut arena = GuestArena::new().unwrap();
        let arena_base = arena.base_ptr() as usize;
        arena
            .init_thread_environment(toluca_core::memory::DEFAULT_IMAGE_BASE)
            .unwrap();
        let mut heap = GuestHeap::new();
        let vmem = VirtualMemoryManager::new();
        let win32 = Win32State::new(Win32Config {
            headless: true,
            ..Win32Config::default()
        });
        let base_alloc = heap.alloc(&mut arena, 64, true).unwrap();
        let _ = heap.free(&mut arena, base_alloc);

        let shared = Arc::new(SharedRuntime {
            pe_image_base: toluca_core::memory::DEFAULT_IMAGE_BASE,
            code: AotCodeImage::empty_for_test(),
            registry: ImportRegistry::new(),
            host_start_instant: Instant::now(),
            arena_base,
            inner: Mutex::new(RuntimeInner {
                arena,
                heap,
                vmem,
                win32,
                imports: Vec::new(),
                import_slot_map: HashMap::new(),
                last_error: 0,
            }),
            sync_cv: Condvar::new(),
            timer_queue: Mutex::new(TimerQueueState {
                started: false,
                timers: Vec::new(),
            }),
            timer_cv: Condvar::new(),
            shutdown: AtomicBool::new(false),
            worker_done_flags: Mutex::new(Vec::new()),
            guest_thread_tids: Mutex::new(Vec::new()),
        });

        let mut cancelled_ids = Vec::new();
        for i in 1..=30u32 {
            let cancelled = Arc::new(AtomicBool::new(false));
            if i % 3 == 0 {
                cancelled.store(true, Ordering::Release);
                cancelled_ids.push(i);
            }
            schedule_timers(
                &shared,
                vec![crate::win32::PendingTimerSpawn {
                    timer_id: i,
                    delay_ms: 1,
                    callback_va: 0x0040_1000,
                    dw_user: i,
                    periodic: false,
                    cancelled,
                }],
                if i % 3 == 0 { vec![i] } else { Vec::new() },
            );
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let empty = shared.timer_queue.lock().unwrap().timers.is_empty();
            if empty || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(shared.timer_queue.lock().unwrap().timers.is_empty());
        assert_eq!(shared.worker_done_flags.lock().unwrap().len(), 1);

        // Shut down worker cleanly and verify its single done flag flips true.
        shared.shutdown.store(true, Ordering::Release);
        shared.timer_cv.notify_all();
        let flags = std::mem::take(&mut *shared.worker_done_flags.lock().unwrap());
        let exit_deadline = Instant::now() + Duration::from_secs(1);
        while !flags[0].load(Ordering::Acquire) && Instant::now() < exit_deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(flags[0].load(Ordering::Acquire));

        // Verify another timer batch while keeping the single 64 KiB stack allocation does not grow threads.
        assert_eq!(cancelled_ids.len(), 10);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_cross_thread_suspend_and_resume_pauses_spinning_thread() {
        let counter = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let ready = Arc::new((Mutex::new(None::<(libc::pid_t, Option<usize>)>), Condvar::new()));

        let c2 = Arc::clone(&counter);
        let s2 = Arc::clone(&stop);
        let r2 = Arc::clone(&ready);
        let handle = std::thread::spawn(move || {
            let os_tid = unsafe { libc::syscall(libc::SYS_gettid) as libc::pid_t };
            let slot_idx = crate::win32::register_thread_suspend_slot(os_tid);
            let slot = slot_idx.and_then(crate::win32::thread_suspend_slot).unwrap();
            slot.in_host_code.store(false, Ordering::SeqCst);
            {
                let mut g = r2.0.lock().unwrap();
                *g = Some((os_tid, slot_idx));
                r2.1.notify_all();
            }
            while !s2.load(Ordering::Relaxed) {
                c2.fetch_add(1, Ordering::Relaxed);
                std::hint::spin_loop();
            }
            crate::win32::unregister_thread_suspend_slot(slot_idx);
        });

        let (os_tid, slot_idx) = {
            let mut g = ready.0.lock().unwrap();
            while g.is_none() {
                g = ready.1.wait(g).unwrap();
            }
            g.unwrap()
        };

        while counter.load(Ordering::Relaxed) < 100 {
            std::thread::yield_now();
        }
        crate::win32::signal_suspend_thread(os_tid, slot_idx);
        std::thread::sleep(Duration::from_millis(15));
        let paused_at = counter.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(25));
        assert_eq!(counter.load(Ordering::Relaxed), paused_at);

        crate::win32::wake_suspended_thread(slot_idx);
        let wake_deadline = Instant::now() + Duration::from_secs(1);
        while counter.load(Ordering::Relaxed) <= paused_at && Instant::now() < wake_deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(counter.load(Ordering::Relaxed) > paused_at);

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_read_task_stat_parses_ticks_and_nice() {
        let tid = current_os_tid();
        assert_ne!(tid, 0);
        let (_ticks, nice) = read_task_stat_ticks_and_nice(tid).expect("/proc stat must parse");
        assert!((-20..=19).contains(&nice));
    }
}
