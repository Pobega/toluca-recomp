pub mod binkw32;
pub mod d3d8;
pub mod dinput8;
pub mod dsound;
pub mod extra_dlls;
pub mod kernel32;
pub mod msvcrt;
pub mod path;
pub mod sdl2;
pub mod user32;

pub use dinput8::ScriptedKeyEvent;

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use toluca_core::GuestAddr;
use toluca_d3d8::D3d8Context;

use crate::runtime::ImportRegistry;

pub const INVALID_HANDLE_VALUE: u32 = 0xFFFF_FFFF;
pub const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;
pub const INVALID_SET_FILE_POINTER: u32 = 0xFFFF_FFFF;

pub const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6;
pub const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
pub const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4;
pub const STD_IN_HOST_HANDLE: u32 = 0x0000_0001;
pub const STD_OUT_HOST_HANDLE: u32 = 0x0000_0002;
pub const STD_ERR_HOST_HANDLE: u32 = 0x0000_0003;

pub const PROCESS_HEAP_HANDLE: u32 = 0x0001_0000;
const FIRST_DYNAMIC_HANDLE: u32 = 0x0000_0100;
const FIRST_MODULE_HANDLE: u32 = 0x7600_0000;
const MODULE_HANDLE_STRIDE: u32 = 0x0001_0000;

pub const MSVCRT_IOBUF_SIZE: usize = 32;
pub const MSVCRT_IOBUF_FLAG_OFF: u32 = 0x0C;
pub const MSVCRT_IOREAD: u32 = 0x0001;
pub const MSVCRT_IOWRT: u32 = 0x0002;
pub const MSVCRT_IOEOF: u32 = 0x0010;
pub const MSVCRT_IOERR: u32 = 0x0020;
pub const MSVCRT_IORW: u32 = 0x0080;

pub const TLS_INITIAL_SLOTS: usize = 64;
pub const TLS_MAX_SLOTS: usize = 1088;
pub const TLS_OUT_OF_INDEXES: u32 = 0xFFFF_FFFF;

pub const ERROR_SUCCESS: u32 = 0;
pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_PATH_NOT_FOUND: u32 = 3;
pub const ERROR_ACCESS_DENIED: u32 = 5;
pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
pub const ERROR_NO_MORE_FILES: u32 = 18;
pub const ERROR_FILE_EXISTS: u32 = 80;
pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
pub const ERROR_MOD_NOT_FOUND: u32 = 126;
pub const ERROR_PROC_NOT_FOUND: u32 = 127;
pub const ERROR_ALREADY_EXISTS: u32 = 183;

#[derive(Clone, Debug)]
pub struct Win32Config {
    pub game_root: PathBuf,
    pub headless: bool,
    pub no_gamepad: bool,
    pub trace_imports: bool,
    pub trace_d3d8: bool,
    pub shader_cache_path: Option<PathBuf>,
    pub max_frames: Option<u64>,
    pub command_line: String,
    pub exe_name: String,
    pub script_keys: Vec<ScriptedKeyEvent>,
    pub fullscreen: bool,
    pub display_width: Option<u32>,
    pub display_height: Option<u32>,
    pub aspect_mode: toluca_d3d8::AspectMode,
}

impl Default for Win32Config {
    fn default() -> Self {
        Self {
            game_root: PathBuf::from("."),
            headless: false,
            no_gamepad: false,
            trace_imports: false,
            trace_d3d8: false,
            shader_cache_path: None,
            max_frames: None,
            command_line: "sh2pc.exe".to_owned(),
            exe_name: "sh2pc.exe".to_owned(),
            script_keys: Vec::new(),
            fullscreen: false,
            display_width: None,
            display_height: None,
            aspect_mode: toluca_d3d8::AspectMode::Ratio4x3,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FindEntry {
    pub file_name: String,
    pub is_dir: bool,
    pub file_size: u64,
}

#[derive(Debug)]
pub struct FindState {
    pub entries: Vec<FindEntry>,
    pub next_index: usize,
}

pub enum Win32Handle {
    File {
        file: File,
        path: PathBuf,
    },
    FileMapping {
        bytes: Vec<u8>,
    },
    FindFile(FindState),
    Event {
        manual_reset: bool,
        signaled: bool,
        pulse_seq: u64,
    },
    Mutex {
        owner_tid: u64,
        recursion: u32,
    },
    Thread {
        exit_code: u32,
        finished: bool,
        suspended: bool,
    },
    Module {
        name: String,
    },
    Heap {
        id: u32,
    },
}

#[derive(Clone, Debug)]
pub enum PendingHostWait {
    SleepMs(u32),
    WaitObject { handle: u32, timeout_ms: u32 },
    EnterCriticalSection { cs_addr: GuestAddr },
    SuspendSelf { handle: u32 },
}

#[derive(Clone, Debug)]
pub struct PendingThreadSpawn {
    pub handle: u32,
    pub start_va: GuestAddr,
    pub param: u32,
    pub stack_top: GuestAddr,
    pub teb_va: GuestAddr,
}

#[derive(Clone, Debug)]
pub struct PendingTimerSpawn {
    pub timer_id: u32,
    pub delay_ms: u32,
    pub callback_va: GuestAddr,
    pub dw_user: u32,
    pub periodic: bool,
    pub cancelled: Arc<AtomicBool>,
}

const CFILE_READ_BUF_CAP: usize = 8192;

pub struct CFileEntry {
    pub file: File,
    pub eof: bool,
    pub err: bool,
    pub base_flag: u32,
    read_buf: Vec<u8>,
    read_pos: usize,
    read_len: usize,
}

impl CFileEntry {
    pub fn new(file: File, base_flag: u32) -> Self {
        Self {
            file,
            eof: false,
            err: false,
            base_flag,
            read_buf: Vec::new(),
            read_pos: 0,
            read_len: 0,
        }
    }

    fn unconsumed_read_bytes(&self) -> usize {
        self.read_len - self.read_pos
    }

    fn discard_read_buf(&mut self) {
        self.read_pos = 0;
        self.read_len = 0;
    }

    fn sync_read_cursor(&mut self) -> std::io::Result<()> {
        let ahead = self.unconsumed_read_bytes();
        if ahead > 0 {
            use std::io::Seek;
            self.file.seek(std::io::SeekFrom::Current(-(ahead as i64)))?;
            self.discard_read_buf();
        }
        Ok(())
    }

    pub fn read_byte(&mut self) -> Option<u8> {
        use std::io::Read;
        if self.read_pos < self.read_len {
            let b = self.read_buf[self.read_pos];
            self.read_pos += 1;
            return Some(b);
        }
        if self.read_buf.is_empty() {
            self.read_buf.resize(CFILE_READ_BUF_CAP, 0);
        }
        match self.file.read(&mut self.read_buf) {
            Ok(0) => {
                self.eof = true;
                None
            }
            Ok(n) => {
                self.read_pos = 1;
                self.read_len = n;
                Some(self.read_buf[0])
            }
            Err(_) => {
                self.err = true;
                None
            }
        }
    }

    pub fn read_up_to(&mut self, dst: &mut [u8]) -> usize {
        use std::io::Read;
        let mut filled = 0usize;
        let avail = self.unconsumed_read_bytes();
        if avail > 0 {
            let take = avail.min(dst.len());
            dst[..take].copy_from_slice(&self.read_buf[self.read_pos..self.read_pos + take]);
            self.read_pos += take;
            filled += take;
        }
        while filled < dst.len() {
            let rem = dst.len() - filled;
            if rem >= CFILE_READ_BUF_CAP {
                match self.file.read(&mut dst[filled..]) {
                    Ok(0) => {
                        self.eof = true;
                        break;
                    }
                    Ok(n) => filled += n,
                    Err(_) => {
                        self.err = true;
                        break;
                    }
                }
            } else {
                if self.read_buf.is_empty() {
                    self.read_buf.resize(CFILE_READ_BUF_CAP, 0);
                }
                match self.file.read(&mut self.read_buf) {
                    Ok(0) => {
                        self.eof = true;
                        break;
                    }
                    Ok(n) => {
                        self.read_pos = 0;
                        let take = n.min(rem);
                        dst[filled..filled + take].copy_from_slice(&self.read_buf[..take]);
                        self.read_pos = take;
                        self.read_len = n;
                        filled += take;
                    }
                    Err(_) => {
                        self.err = true;
                        break;
                    }
                }
            }
        }
        filled
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) -> bool {
        use std::io::Write;
        if self.sync_read_cursor().is_err() || self.file.write_all(bytes).is_err() {
            self.err = true;
            false
        } else {
            true
        }
    }

    pub fn seek(&mut self, from: std::io::SeekFrom) -> bool {
        use std::io::Seek;
        self.eof = false;
        let adjusted = match from {
            std::io::SeekFrom::Current(delta) => {
                std::io::SeekFrom::Current(delta - self.unconsumed_read_bytes() as i64)
            }
            other => other,
        };
        self.discard_read_buf();
        if self.file.seek(adjusted).is_ok() {
            true
        } else {
            self.err = true;
            false
        }
    }

    pub fn stream_position(&mut self) -> Option<u64> {
        use std::io::Seek;
        let ahead = self.unconsumed_read_bytes() as u64;
        self.file.stream_position().ok().map(|p| p.saturating_sub(ahead))
    }

    pub fn flush(&mut self) {
        use std::io::Write;
        let _ = self.sync_read_cursor();
        let _ = self.file.flush();
    }
}

#[derive(Clone, Debug)]
pub struct WindowClassInfo {
    pub style: u32,
    pub wndproc: GuestAddr,
    pub hinstance: u32,
    pub atom: u16,
}

#[derive(Clone, Debug)]
pub struct WindowRecord {
    pub hwnd: u32,
    pub class_name: String,
    pub wndproc: GuestAddr,
    pub width: i32,
    pub height: i32,
    pub visible: bool,
    pub user_data: u32,
    pub sdl_window: Option<*mut c_void>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Win32Msg {
    pub hwnd: u32,
    pub message: u32,
    pub wparam: u32,
    pub lparam: u32,
    pub time: u32,
    pub pt_x: i32,
    pub pt_y: i32,
}

const FIRST_HWND: u32 = 0x0002_0000;
const HWND_STRIDE: u32 = 4;

pub struct WindowManager {
    pub registered_classes: HashMap<String, WindowClassInfo>,
    pub class_atoms: HashMap<u16, String>,
    pub next_atom: u16,
    pub windows: HashMap<u32, WindowRecord>,
    pub next_hwnd: u32,
    pub active_hwnd: Option<u32>,
    pub focused_hwnd: Option<u32>,
    pub msg_queue: VecDeque<Win32Msg>,
    pub quit_requested: bool,
    pub quit_code: u32,
    pub cursor_count: i32,
    pub current_cursor: u32,
    pub key_states: [bool; 256],
    pub sdl: Option<sdl2::Sdl2WindowBackend>,
}

impl WindowManager {
    pub fn new(headless: bool, no_gamepad: bool) -> Self {
        Self {
            registered_classes: HashMap::new(),
            class_atoms: HashMap::new(),
            next_atom: 1,
            windows: HashMap::new(),
            next_hwnd: FIRST_HWND,
            active_hwnd: None,
            focused_hwnd: None,
            msg_queue: VecDeque::new(),
            quit_requested: false,
            quit_code: 0,
            cursor_count: 0,
            current_cursor: 0,
            key_states: [false; 256],
            sdl: sdl2::Sdl2WindowBackend::try_init(headless, no_gamepad),
        }
    }

    pub fn alloc_hwnd(&mut self) -> u32 {
        let hwnd = self.next_hwnd;
        self.next_hwnd = self.next_hwnd.wrapping_add(HWND_STRIDE);
        hwnd
    }
}

pub struct Win32State {
    pub config: Win32Config,
    pub current_dir: PathBuf,
    handles: HashMap<u32, Win32Handle>,
    next_handle: u32,
    module_handles: HashMap<String, u32>,
    next_module_handle: u32,
    tls_allocated: Vec<bool>,
    tls_values: Vec<u32>,
    c_files: HashMap<u32, CFileEntry>,
    pub mapped_views: HashMap<GuestAddr, usize>,
    pub d3d8: D3d8Context,
    pub dinput8: dinput8::Dinput8State,
    // Declared before window_mgr so Sdl2AudioDevice closes before Sdl2WindowBackend drops.
    pub dsound: dsound::DsoundState,
    pub bink: binkw32::BinkState,
    pub window_mgr: WindowManager,
    pub d3d8_create_calls: Vec<u32>,
    pub d3d8_present_calls: u64,
    pub exit_code: Option<u32>,
    pub command_line_guest: Option<GuestAddr>,
    pub module_filename_guest: Option<GuestAddr>,
    pub env_strings_guest: Option<GuestAddr>,
    pub fmode_guest: Option<GuestAddr>,
    pub commode_guest: Option<GuestAddr>,
    pub strtok_cursor: Option<GuestAddr>,
    pub unhandled_exception_filter: GuestAddr,
    pub rng_state: u32,
    pub atexit_callbacks: Vec<GuestAddr>,
    pub crit_sections: HashMap<GuestAddr, (u64, u32)>,
    pub timers: HashMap<u32, Arc<AtomicBool>>,
    pub next_timer_id: u32,
    pub pending_wait: Option<PendingHostWait>,
    pub pending_threads: Vec<PendingThreadSpawn>,
    pub pending_timers: Vec<PendingTimerSpawn>,
    pub notify_sync: bool,
}

unsafe impl Send for Win32State {}

impl Win32State {
    pub fn new(config: Win32Config) -> Self {
        let current_dir = config.game_root.clone();
        let headless = config.headless;
        let no_gamepad = config.no_gamepad;
        let dinput8 = dinput8::Dinput8State::new(&config.script_keys);
        let dsound = dsound::DsoundState::new();
        let bink = binkw32::BinkState::new();
        let mut d3d8 = D3d8Context::new_null();
        d3d8.trace_d3d8 = config.trace_d3d8;
        Self {
            config,
            current_dir,
            handles: HashMap::new(),
            next_handle: FIRST_DYNAMIC_HANDLE,
            module_handles: HashMap::new(),
            next_module_handle: FIRST_MODULE_HANDLE,
            tls_allocated: vec![false; TLS_INITIAL_SLOTS],
            tls_values: vec![0; TLS_INITIAL_SLOTS],
            c_files: HashMap::new(),
            mapped_views: HashMap::new(),
            d3d8,
            dinput8,
            dsound,
            bink,
            window_mgr: WindowManager::new(headless, no_gamepad),
            d3d8_create_calls: Vec::new(),
            d3d8_present_calls: 0,
            exit_code: None,
            command_line_guest: None,
            module_filename_guest: None,
            env_strings_guest: None,
            fmode_guest: None,
            commode_guest: None,
            strtok_cursor: None,
            unhandled_exception_filter: 0,
            rng_state: 1,
            atexit_callbacks: Vec::new(),
            crit_sections: HashMap::new(),
            timers: HashMap::new(),
            next_timer_id: 1,
            pending_wait: None,
            pending_threads: Vec::new(),
            pending_timers: Vec::new(),
            notify_sync: false,
        }
    }

    pub fn resolve_path(&self, win_path: &str) -> PathBuf {
        let base = if is_rooted_win_path(win_path) {
            &self.config.game_root
        } else {
            &self.current_dir
        };
        path::resolve_win32_path(base, win_path)
    }

    pub fn alloc_handle(&mut self, handle: Win32Handle) -> u32 {
        let id = self.next_handle;
        self.next_handle = self.next_handle.wrapping_add(1);
        if self.next_handle == PROCESS_HEAP_HANDLE {
            self.next_handle = self.next_handle.wrapping_add(1);
        }
        self.handles.insert(id, handle);
        id
    }

    pub fn get_handle(&self, handle: u32) -> Option<&Win32Handle> {
        self.handles.get(&handle)
    }

    pub fn get_handle_mut(&mut self, handle: u32) -> Option<&mut Win32Handle> {
        self.handles.get_mut(&handle)
    }

    pub fn close_handle(&mut self, handle: u32) -> bool {
        if let Some(Win32Handle::Module { name }) = self.handles.get(&handle) {
            let key = name.to_ascii_lowercase();
            self.module_handles.remove(&key);
        }
        self.handles.remove(&handle).is_some()
    }

    pub fn get_or_alloc_module(&mut self, dll_name: &str) -> u32 {
        let key = normalize_module_name(dll_name);
        if let Some(&handle) = self.module_handles.get(&key) {
            return handle;
        }
        let handle = self.next_module_handle;
        self.next_module_handle = self.next_module_handle.wrapping_add(MODULE_HANDLE_STRIDE);
        self.module_handles.insert(key.clone(), handle);
        self.handles.insert(handle, Win32Handle::Module { name: key });
        handle
    }

    pub fn module_name_for_handle(
        &self,
        handle: u32,
        pe_image_base: GuestAddr,
    ) -> Option<String> {
        if handle == 0 || handle == pe_image_base {
            return Some(self.config.exe_name.clone());
        }
        match self.handles.get(&handle) {
            Some(Win32Handle::Module { name }) => Some(name.clone()),
            _ => None,
        }
    }

    pub fn tls_alloc(&mut self) -> u32 {
        if let Some(idx) = self.tls_allocated.iter().position(|&used| !used) {
            self.tls_allocated[idx] = true;
            self.tls_values[idx] = 0;
            return idx as u32;
        }
        if self.tls_allocated.len() >= TLS_MAX_SLOTS {
            return TLS_OUT_OF_INDEXES;
        }
        let idx = self.tls_allocated.len();
        self.tls_allocated.push(true);
        self.tls_values.push(0);
        idx as u32
    }

    pub fn tls_get(&self, index: u32) -> Option<u32> {
        let idx = index as usize;
        if *self.tls_allocated.get(idx)? {
            Some(self.tls_values[idx])
        } else {
            None
        }
    }

    pub fn tls_set(&mut self, index: u32, value: u32) -> bool {
        let idx = index as usize;
        if matches!(self.tls_allocated.get(idx), Some(true)) {
            self.tls_values[idx] = value;
            true
        } else {
            false
        }
    }

    pub fn tls_free(&mut self, index: u32) -> bool {
        let idx = index as usize;
        if matches!(self.tls_allocated.get(idx), Some(true)) {
            self.tls_allocated[idx] = false;
            self.tls_values[idx] = 0;
            true
        } else {
            false
        }
    }

    pub fn insert_cfile(&mut self, ptr: u32, entry: CFileEntry) {
        self.c_files.insert(ptr, entry);
    }

    pub fn get_cfile_mut(&mut self, ptr: u32) -> Option<&mut CFileEntry> {
        self.c_files.get_mut(&ptr)
    }

    pub fn close_cfile(&mut self, ptr: u32) -> bool {
        self.c_files.remove(&ptr).is_some()
    }
}

pub fn register_kernel32_and_msvcrt(registry: &mut ImportRegistry) {
    kernel32::register_imports(registry);
    msvcrt::register_imports(registry);
}

pub fn register_all_win32(registry: &mut ImportRegistry) {
    register_kernel32_and_msvcrt(registry);
    user32::register_imports(registry);
    d3d8::register_imports(registry);
    dinput8::register_imports(registry);
    dsound::register_imports(registry);
    binkw32::register_imports(registry);
    extra_dlls::register_imports(registry);
}

fn is_rooted_win_path(raw: &str) -> bool {
    if raw.starts_with(r"\\?\") || raw.starts_with('\\') || raw.starts_with('/') {
        return true;
    }
    let bytes = raw.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

pub(crate) fn normalize_module_name(raw: &str) -> String {
    let trimmed = raw
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(raw)
        .to_ascii_lowercase();
    if trimmed.contains('.') {
        trimmed
    } else {
        format!("{trimmed}.dll")
    }
}

pub(crate) fn map_io_error(err: &std::io::Error) -> u32 {
    match err.kind() {
        std::io::ErrorKind::NotFound => ERROR_FILE_NOT_FOUND,
        std::io::ErrorKind::PermissionDenied => ERROR_ACCESS_DENIED,
        std::io::ErrorKind::AlreadyExists => ERROR_FILE_EXISTS,
        std::io::ErrorKind::InvalidInput => ERROR_INVALID_PARAMETER,
        _ => ERROR_ACCESS_DENIED,
    }
}
