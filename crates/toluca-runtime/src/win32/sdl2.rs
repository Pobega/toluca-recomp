use std::ffi::{CString, c_char, c_void};

const SDL_INIT_VIDEO: u32 = 0x0000_0020;
const SDL_WINDOWPOS_CENTERED: i32 = 0x2FFF_0000;
const SDL_WINDOW_OPENGL: u32 = 0x0000_0002;
const SDL_WINDOW_SHOWN: u32 = 0x0000_0004;

const SDL_EVENT_SIZE: usize = 56;
const SDL_QUIT: u32 = 0x100;
const SDL_WINDOWEVENT: u32 = 0x200;
const SDL_KEYDOWN: u32 = 0x300;
const SDL_KEYUP: u32 = 0x301;

const SDL_WINDOWEVENT_FOCUS_GAINED: u8 = 12;
const SDL_WINDOWEVENT_FOCUS_LOST: u8 = 13;
const SDL_WINDOWEVENT_CLOSE: u8 = 14;

const SDL_EVENT_OFF_WINDOW_EVENT: usize = 12;
const SDL_EVENT_OFF_SCANCODE: usize = 16;

const SDL_SCANCODE_A: i32 = 4;
const SDL_SCANCODE_Z: i32 = 29;
const SDL_SCANCODE_1: i32 = 30;
const SDL_SCANCODE_9: i32 = 38;
const SDL_SCANCODE_0: i32 = 39;
const SDL_SCANCODE_RETURN: i32 = 40;
const SDL_SCANCODE_ESCAPE: i32 = 41;
const SDL_SCANCODE_BACKSPACE: i32 = 42;
const SDL_SCANCODE_TAB: i32 = 43;
const SDL_SCANCODE_SPACE: i32 = 44;
const SDL_SCANCODE_F1: i32 = 58;
const SDL_SCANCODE_F12: i32 = 69;
const SDL_SCANCODE_RIGHT: i32 = 79;
const SDL_SCANCODE_LEFT: i32 = 80;
const SDL_SCANCODE_DOWN: i32 = 81;
const SDL_SCANCODE_UP: i32 = 82;
const SDL_SCANCODE_LCTRL: i32 = 224;
const SDL_SCANCODE_LSHIFT: i32 = 225;
const SDL_SCANCODE_LALT: i32 = 226;
const SDL_SCANCODE_RCTRL: i32 = 228;
const SDL_SCANCODE_RSHIFT: i32 = 229;
const SDL_SCANCODE_RALT: i32 = 230;

const VK_BACK: u8 = 0x08;
const VK_TAB: u8 = 0x09;
const VK_RETURN: u8 = 0x0D;
const VK_SHIFT: u8 = 0x10;
const VK_CONTROL: u8 = 0x11;
const VK_MENU: u8 = 0x12;
const VK_ESCAPE: u8 = 0x1B;
const VK_SPACE: u8 = 0x20;
const VK_LEFT: u8 = 0x25;
const VK_UP: u8 = 0x26;
const VK_RIGHT: u8 = 0x27;
const VK_DOWN: u8 = 0x28;
const VK_F1: u8 = 0x70;

#[repr(C, align(8))]
struct SdlEventBuf {
    raw: [u8; SDL_EVENT_SIZE],
}

impl SdlEventBuf {
    const fn zeroed() -> Self {
        Self {
            raw: [0u8; SDL_EVENT_SIZE],
        }
    }

    fn event_type(&self) -> u32 {
        u32::from_ne_bytes([self.raw[0], self.raw[1], self.raw[2], self.raw[3]])
    }

    fn window_event_id(&self) -> u8 {
        self.raw[SDL_EVENT_OFF_WINDOW_EVENT]
    }

    fn scancode(&self) -> i32 {
        let o = SDL_EVENT_OFF_SCANCODE;
        i32::from_ne_bytes([self.raw[o], self.raw[o + 1], self.raw[o + 2], self.raw[o + 3]])
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SdlTranslatedEvent {
    Quit,
    KeyDown(u8),
    KeyUp(u8),
    FocusGained,
    FocusLost,
    WindowClose,
}

type SdlInitFn = unsafe extern "C" fn(flags: u32) -> i32;
type SdlQuitSubSystemFn = unsafe extern "C" fn(flags: u32);
type SdlCreateWindowFn =
    unsafe extern "C" fn(title: *const c_char, x: i32, y: i32, w: i32, h: i32, flags: u32) -> *mut c_void;
type SdlDestroyWindowFn = unsafe extern "C" fn(window: *mut c_void);
type SdlShowWindowFn = unsafe extern "C" fn(window: *mut c_void);
type SdlHideWindowFn = unsafe extern "C" fn(window: *mut c_void);
type SdlShowCursorFn = unsafe extern "C" fn(toggle: i32) -> i32;
type SdlPollEventFn = unsafe extern "C" fn(event: *mut SdlEventBuf) -> i32;

pub struct Sdl2WindowBackend {
    lib_handle: *mut c_void,
    quit_subsystem: SdlQuitSubSystemFn,
    create_window_fn: SdlCreateWindowFn,
    destroy_window_fn: SdlDestroyWindowFn,
    show_window_fn: SdlShowWindowFn,
    hide_window_fn: SdlHideWindowFn,
    show_cursor_fn: SdlShowCursorFn,
    poll_event_fn: SdlPollEventFn,
    windows: Vec<*mut c_void>,
}

impl Sdl2WindowBackend {
    pub fn try_init(headless: bool) -> Option<Self> {
        if headless {
            return None;
        }

        let lib_handle = open_sdl2_library()?;
        let Some(init_fn) = load_sym::<SdlInitFn>(lib_handle, c"SDL_Init") else {
            unsafe { libc::dlclose(lib_handle) };
            return None;
        };
        let Some(quit_subsystem) = load_sym::<SdlQuitSubSystemFn>(lib_handle, c"SDL_QuitSubSystem") else {
            unsafe { libc::dlclose(lib_handle) };
            return None;
        };
        let Some(create_window_fn) = load_sym::<SdlCreateWindowFn>(lib_handle, c"SDL_CreateWindow") else {
            unsafe { libc::dlclose(lib_handle) };
            return None;
        };
        let Some(destroy_window_fn) = load_sym::<SdlDestroyWindowFn>(lib_handle, c"SDL_DestroyWindow") else {
            unsafe { libc::dlclose(lib_handle) };
            return None;
        };
        let Some(show_window_fn) = load_sym::<SdlShowWindowFn>(lib_handle, c"SDL_ShowWindow") else {
            unsafe { libc::dlclose(lib_handle) };
            return None;
        };
        let Some(hide_window_fn) = load_sym::<SdlHideWindowFn>(lib_handle, c"SDL_HideWindow") else {
            unsafe { libc::dlclose(lib_handle) };
            return None;
        };
        let Some(show_cursor_fn) = load_sym::<SdlShowCursorFn>(lib_handle, c"SDL_ShowCursor") else {
            unsafe { libc::dlclose(lib_handle) };
            return None;
        };
        let Some(poll_event_fn) = load_sym::<SdlPollEventFn>(lib_handle, c"SDL_PollEvent") else {
            unsafe { libc::dlclose(lib_handle) };
            return None;
        };

        if unsafe { init_fn(SDL_INIT_VIDEO) } != 0 {
            unsafe { libc::dlclose(lib_handle) };
            return None;
        }

        Some(Self {
            lib_handle,
            quit_subsystem,
            create_window_fn,
            destroy_window_fn,
            show_window_fn,
            hide_window_fn,
            show_cursor_fn,
            poll_event_fn,
            windows: Vec::new(),
        })
    }

    pub fn create_window(&mut self, title: &str, width: i32, height: i32) -> Option<*mut c_void> {
        let c_title = CString::new(title).unwrap_or_else(|_| CString::new("toluca-runtime").unwrap());
        let ptr = unsafe {
            (self.create_window_fn)(
                c_title.as_ptr(),
                SDL_WINDOWPOS_CENTERED,
                SDL_WINDOWPOS_CENTERED,
                width.max(1),
                height.max(1),
                SDL_WINDOW_SHOWN | SDL_WINDOW_OPENGL,
            )
        };
        if ptr.is_null() {
            None
        } else {
            self.windows.push(ptr);
            Some(ptr)
        }
    }

    /// Destroys a previously created SDL2 window handle.
    ///
    /// # Safety
    /// `ptr` must be null or a valid window pointer previously returned by `create_window`.
    pub unsafe fn destroy_window(&mut self, ptr: *mut c_void) {
        if ptr.is_null() {
            return;
        }
        if let Some(pos) = self.windows.iter().position(|&w| w == ptr) {
            self.windows.swap_remove(pos);
        }
        unsafe {
            (self.destroy_window_fn)(ptr);
        }
    }

    /// Shows or hides an SDL2 window handle.
    ///
    /// # Safety
    /// `ptr` must be null or a valid window pointer previously returned by `create_window`.
    pub unsafe fn show_window(&mut self, ptr: *mut c_void, show: bool) {
        if ptr.is_null() {
            return;
        }
        unsafe {
            if show {
                (self.show_window_fn)(ptr);
            } else {
                (self.hide_window_fn)(ptr);
            }
        }
    }

    pub fn show_cursor(&mut self, show: bool) {
        unsafe {
            (self.show_cursor_fn)(i32::from(show));
        }
    }

    pub fn poll_events(&mut self) -> Vec<SdlTranslatedEvent> {
        let mut events = Vec::new();
        let mut buf = SdlEventBuf::zeroed();
        while unsafe { (self.poll_event_fn)(&mut buf) } != 0 {
            match buf.event_type() {
                SDL_QUIT => events.push(SdlTranslatedEvent::Quit),
                SDL_KEYDOWN => {
                    if let Some(vk) = scancode_to_vk(buf.scancode()) {
                        events.push(SdlTranslatedEvent::KeyDown(vk));
                    }
                }
                SDL_KEYUP => {
                    if let Some(vk) = scancode_to_vk(buf.scancode()) {
                        events.push(SdlTranslatedEvent::KeyUp(vk));
                    }
                }
                SDL_WINDOWEVENT => match buf.window_event_id() {
                    SDL_WINDOWEVENT_FOCUS_GAINED => events.push(SdlTranslatedEvent::FocusGained),
                    SDL_WINDOWEVENT_FOCUS_LOST => events.push(SdlTranslatedEvent::FocusLost),
                    SDL_WINDOWEVENT_CLOSE => events.push(SdlTranslatedEvent::WindowClose),
                    _ => {}
                },
                _ => {}
            }
        }
        events
    }
}

impl Drop for Sdl2WindowBackend {
    fn drop(&mut self) {
        while let Some(win) = self.windows.pop() {
            unsafe {
                (self.destroy_window_fn)(win);
            }
        }
        unsafe {
            (self.quit_subsystem)(SDL_INIT_VIDEO);
            libc::dlclose(self.lib_handle);
        }
    }
}

fn open_sdl2_library() -> Option<*mut c_void> {
    for candidate in [c"libSDL2-2.0.so.0", c"libSDL2.so"] {
        let handle = unsafe { libc::dlopen(candidate.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if !handle.is_null() {
            return Some(handle);
        }
    }
    None
}

fn load_sym<F: Copy>(lib_handle: *mut c_void, name: &std::ffi::CStr) -> Option<F> {
    let ptr = unsafe { libc::dlsym(lib_handle, name.as_ptr()) };
    if ptr.is_null() {
        return None;
    }
    debug_assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*mut c_void>());
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&ptr) })
}

pub fn scancode_to_vk(scancode: i32) -> Option<u8> {
    if (SDL_SCANCODE_A..=SDL_SCANCODE_Z).contains(&scancode) {
        return Some(b'A' + (scancode - SDL_SCANCODE_A) as u8);
    }
    if (SDL_SCANCODE_1..=SDL_SCANCODE_9).contains(&scancode) {
        return Some(b'1' + (scancode - SDL_SCANCODE_1) as u8);
    }
    if (SDL_SCANCODE_F1..=SDL_SCANCODE_F12).contains(&scancode) {
        return Some(VK_F1 + (scancode - SDL_SCANCODE_F1) as u8);
    }
    match scancode {
        SDL_SCANCODE_0 => Some(b'0'),
        SDL_SCANCODE_RETURN => Some(VK_RETURN),
        SDL_SCANCODE_ESCAPE => Some(VK_ESCAPE),
        SDL_SCANCODE_BACKSPACE => Some(VK_BACK),
        SDL_SCANCODE_TAB => Some(VK_TAB),
        SDL_SCANCODE_SPACE => Some(VK_SPACE),
        SDL_SCANCODE_RIGHT => Some(VK_RIGHT),
        SDL_SCANCODE_LEFT => Some(VK_LEFT),
        SDL_SCANCODE_DOWN => Some(VK_DOWN),
        SDL_SCANCODE_UP => Some(VK_UP),
        SDL_SCANCODE_LCTRL | SDL_SCANCODE_RCTRL => Some(VK_CONTROL),
        SDL_SCANCODE_LSHIFT | SDL_SCANCODE_RSHIFT => Some(VK_SHIFT),
        SDL_SCANCODE_LALT | SDL_SCANCODE_RALT => Some(VK_MENU),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_returns_none_and_scancode_mapping_matches_win32_vk() {
        assert!(Sdl2WindowBackend::try_init(true).is_none());
        assert_eq!(scancode_to_vk(SDL_SCANCODE_A), Some(b'A'));
        assert_eq!(scancode_to_vk(SDL_SCANCODE_Z), Some(b'Z'));
        assert_eq!(scancode_to_vk(SDL_SCANCODE_0), Some(b'0'));
        assert_eq!(scancode_to_vk(SDL_SCANCODE_1), Some(b'1'));
        assert_eq!(scancode_to_vk(SDL_SCANCODE_RETURN), Some(VK_RETURN));
        assert_eq!(scancode_to_vk(SDL_SCANCODE_ESCAPE), Some(VK_ESCAPE));
        assert_eq!(scancode_to_vk(SDL_SCANCODE_SPACE), Some(VK_SPACE));
        assert_eq!(scancode_to_vk(SDL_SCANCODE_UP), Some(VK_UP));
        assert_eq!(scancode_to_vk(SDL_SCANCODE_LSHIFT), Some(VK_SHIFT));
        assert_eq!(scancode_to_vk(SDL_SCANCODE_RCTRL), Some(VK_CONTROL));
    }
}
