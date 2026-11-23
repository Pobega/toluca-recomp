pub mod d3d8_stub;
pub mod extra_dlls;
pub mod kernel32;
pub mod msvcrt;
pub mod path;
pub mod sdl2;
pub mod user32;

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::fs::File;
use std::path::PathBuf;

use toluca_core::GuestAddr;

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
const FIRST_CFILE_PTR: u32 = 0x7200_0000;
const CFILE_STRIDE: u32 = 32;

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
    pub trace_imports: bool,
    pub command_line: String,
    pub exe_name: String,
}

impl Default for Win32Config {
    fn default() -> Self {
        Self {
            game_root: PathBuf::from("."),
            headless: false,
            trace_imports: false,
            command_line: "sh2pc.exe".to_owned(),
            exe_name: "sh2pc.exe".to_owned(),
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
    },
    Mutex,
    Thread {
        exit_code: u32,
    },
    Module {
        name: String,
    },
    Heap {
        id: u32,
    },
}

pub struct CFileEntry {
    pub file: File,
    pub eof: bool,
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
    pub fn new(headless: bool) -> Self {
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
            sdl: sdl2::Sdl2WindowBackend::try_init(headless),
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
    next_cfile_ptr: u32,
    pub mapped_views: HashMap<GuestAddr, usize>,
    pub window_mgr: WindowManager,
    pub d3d8_create_calls: Vec<u32>,
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
}

impl Win32State {
    pub fn new(config: Win32Config) -> Self {
        let current_dir = config.game_root.clone();
        let headless = config.headless;
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
            next_cfile_ptr: FIRST_CFILE_PTR,
            mapped_views: HashMap::new(),
            window_mgr: WindowManager::new(headless),
            d3d8_create_calls: Vec::new(),
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

    pub fn alloc_cfile(&mut self, entry: CFileEntry) -> u32 {
        let ptr = self.next_cfile_ptr;
        self.next_cfile_ptr = self.next_cfile_ptr.wrapping_add(CFILE_STRIDE);
        self.c_files.insert(ptr, entry);
        ptr
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
    d3d8_stub::register_imports(registry);
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
