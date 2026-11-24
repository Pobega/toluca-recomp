use std::ffi::{CStr, CString, c_char, c_void};
use std::path::Path;

use toluca_d3d8::{GlGetProcAddrFn, PresentTarget};

const SDL_INIT_AUDIO: u32 = 0x0000_0010;
const SDL_INIT_VIDEO: u32 = 0x0000_0020;
const SDL_INIT_GAMECONTROLLER: u32 = 0x0000_2000;
pub const SDL_AUDIO_S16LSB: u16 = 0x8010;
pub const SDL_AUDIO_DEFAULT_FREQ: i32 = 48_000;
const SDL_AUDIO_ALLOW_FREQUENCY_CHANGE: i32 = 1;
pub const SDL_AUDIO_DEFAULT_CHANNELS: u8 = 2;
pub const SDL_AUDIO_DEFAULT_SAMPLES: u16 = 2048;
const SDL_WINDOWPOS_CENTERED: i32 = 0x2FFF_0000;
const SDL_WINDOW_FULLSCREEN_DESKTOP: u32 = 0x0000_1001;
const SDL_WINDOW_OPENGL: u32 = 0x0000_0002;
const SDL_WINDOW_SHOWN: u32 = 0x0000_0004;

const SDL_GL_DOUBLEBUFFER: i32 = 5;
const SDL_GL_DEPTH_SIZE: i32 = 6;
const SDL_GL_STENCIL_SIZE: i32 = 7;
const SDL_GL_CONTEXT_MAJOR_VERSION: i32 = 17;
const SDL_GL_CONTEXT_MINOR_VERSION: i32 = 18;
const SDL_GL_CONTEXT_PROFILE_MASK: i32 = 21;
const SDL_GL_CONTEXT_PROFILE_ES: i32 = 4;

const SDL_EVENT_SIZE: usize = 56;
const SDL_QUIT: u32 = 0x100;
const SDL_WINDOWEVENT: u32 = 0x200;
const SDL_KEYDOWN: u32 = 0x300;
const SDL_KEYUP: u32 = 0x301;
const SDL_MOUSEWHEEL: u32 = 0x403;
const SDL_CONTROLLERDEVICEADDED: u32 = 0x653;
const SDL_CONTROLLERDEVICEREMOVED: u32 = 0x654;

const SDL_MOUSEWHEEL_FLIPPED: u32 = 1;

const SDL_WINDOWEVENT_FOCUS_GAINED: u8 = 12;
const SDL_WINDOWEVENT_FOCUS_LOST: u8 = 13;
const SDL_WINDOWEVENT_CLOSE: u8 = 14;

const SDL_EVENT_OFF_CDEVICE_WHICH: usize = 8;
const SDL_EVENT_OFF_WINDOW_EVENT: usize = 12;
const SDL_EVENT_OFF_SCANCODE: usize = 16;
const SDL_EVENT_OFF_WHEEL_Y: usize = 20;
const SDL_EVENT_OFF_WHEEL_DIRECTION: usize = 24;

pub const SDL_SCANCODE_COUNT: usize = 512;
pub const SDL_GAMEPAD_AXIS_COUNT: usize = 6;
pub const SDL_GAMEPAD_BUTTON_COUNT: usize = 16;
const SDL_TRIGGER_DIGITAL_THRESHOLD: i16 = 8192;
const DPAD_AXIS_DEADZONE: i32 = 8192;
const DI_POV_CENTERED: u32 = 0xFFFF_FFFF;

const SDL_CONTROLLER_BUTTON_A: i32 = 0;
const SDL_CONTROLLER_BUTTON_B: i32 = 1;
const SDL_CONTROLLER_BUTTON_X: i32 = 2;
const SDL_CONTROLLER_BUTTON_Y: i32 = 3;
const SDL_CONTROLLER_BUTTON_BACK: i32 = 4;
const SDL_CONTROLLER_BUTTON_START: i32 = 6;
const SDL_CONTROLLER_BUTTON_LEFTSTICK: i32 = 7;
const SDL_CONTROLLER_BUTTON_RIGHTSTICK: i32 = 8;
const SDL_CONTROLLER_BUTTON_LEFTSHOULDER: i32 = 9;
const SDL_CONTROLLER_BUTTON_RIGHTSHOULDER: i32 = 10;
const SDL_CONTROLLER_BUTTON_DPAD_UP: i32 = 11;
const SDL_CONTROLLER_BUTTON_DPAD_DOWN: i32 = 12;
const SDL_CONTROLLER_BUTTON_DPAD_LEFT: i32 = 13;
const SDL_CONTROLLER_BUTTON_DPAD_RIGHT: i32 = 14;

const SDL_CONTROLLER_AXIS_LEFTX: i32 = 0;
const SDL_CONTROLLER_AXIS_LEFTY: i32 = 1;
const SDL_CONTROLLER_AXIS_RIGHTX: i32 = 2;
const SDL_CONTROLLER_AXIS_RIGHTY: i32 = 3;
const SDL_CONTROLLER_AXIS_TRIGGERLEFT: i32 = 4;
const SDL_CONTROLLER_AXIS_TRIGGERRIGHT: i32 = 5;

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

    fn read_i32(&self, offset: usize) -> i32 {
        i32::from_ne_bytes([
            self.raw[offset],
            self.raw[offset + 1],
            self.raw[offset + 2],
            self.raw[offset + 3],
        ])
    }

    fn read_u32(&self, offset: usize) -> u32 {
        u32::from_ne_bytes([
            self.raw[offset],
            self.raw[offset + 1],
            self.raw[offset + 2],
            self.raw[offset + 3],
        ])
    }

    fn event_type(&self) -> u32 {
        self.read_u32(0)
    }

    fn window_event_id(&self) -> u8 {
        self.raw[SDL_EVENT_OFF_WINDOW_EVENT]
    }

    fn scancode(&self) -> i32 {
        self.read_i32(SDL_EVENT_OFF_SCANCODE)
    }

    fn cdevice_which(&self) -> i32 {
        self.read_i32(SDL_EVENT_OFF_CDEVICE_WHICH)
    }

    fn wheel_y(&self) -> i32 {
        let y = self.read_i32(SDL_EVENT_OFF_WHEEL_Y);
        if self.read_u32(SDL_EVENT_OFF_WHEEL_DIRECTION) == SDL_MOUSEWHEEL_FLIPPED {
            y.saturating_neg()
        } else {
            y
        }
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GamepadSnapshot {
    pub axes: [i16; SDL_GAMEPAD_AXIS_COUNT],
    pub dpad_hat: u32,
    pub buttons: [bool; SDL_GAMEPAD_BUTTON_COUNT],
}

impl Default for GamepadSnapshot {
    fn default() -> Self {
        Self {
            axes: [0; SDL_GAMEPAD_AXIS_COUNT],
            dpad_hat: DI_POV_CENTERED,
            buttons: [false; SDL_GAMEPAD_BUTTON_COUNT],
        }
    }
}

#[derive(Clone, Debug)]
pub struct SdlInputSnapshot {
    pub key_scancodes: [bool; SDL_SCANCODE_COUNT],
    pub mouse_dx: i32,
    pub mouse_dy: i32,
    pub mouse_dz: i32,
    pub mouse_buttons: u32,
    pub gamepad: Option<GamepadSnapshot>,
}

impl Default for SdlInputSnapshot {
    fn default() -> Self {
        Self {
            key_scancodes: [false; SDL_SCANCODE_COUNT],
            mouse_dx: 0,
            mouse_dy: 0,
            mouse_dz: 0,
            mouse_buttons: 0,
            gamepad: None,
        }
    }
}

type SdlInitFn = unsafe extern "C" fn(flags: u32) -> i32;
type SdlInitSubSystemFn = unsafe extern "C" fn(flags: u32) -> i32;
type SdlQuitSubSystemFn = unsafe extern "C" fn(flags: u32);
type SdlCreateWindowFn =
    unsafe extern "C" fn(title: *const c_char, x: i32, y: i32, w: i32, h: i32, flags: u32) -> *mut c_void;
type SdlDestroyWindowFn = unsafe extern "C" fn(window: *mut c_void);
type SdlShowWindowFn = unsafe extern "C" fn(window: *mut c_void);
type SdlHideWindowFn = unsafe extern "C" fn(window: *mut c_void);
type SdlShowCursorFn = unsafe extern "C" fn(toggle: i32) -> i32;
type SdlPollEventFn = unsafe extern "C" fn(event: *mut SdlEventBuf) -> i32;
type SdlPumpEventsFn = unsafe extern "C" fn();
type SdlGetKeyboardStateFn = unsafe extern "C" fn(numkeys: *mut i32) -> *const u8;
type SdlGetMouseStateFn = unsafe extern "C" fn(x: *mut i32, y: *mut i32) -> u32;
type SdlNumJoysticksFn = unsafe extern "C" fn() -> i32;
type SdlIsGameControllerFn = unsafe extern "C" fn(joystick_index: i32) -> i32;
type SdlGameControllerOpenFn = unsafe extern "C" fn(joystick_index: i32) -> *mut c_void;
type SdlGameControllerCloseFn = unsafe extern "C" fn(gamecontroller: *mut c_void);
type SdlGameControllerUpdateFn = unsafe extern "C" fn();
type SdlGameControllerGetButtonFn =
    unsafe extern "C" fn(gamecontroller: *mut c_void, button: i32) -> u8;
type SdlGameControllerGetAxisFn =
    unsafe extern "C" fn(gamecontroller: *mut c_void, axis: i32) -> i16;
type SdlGameControllerRumbleFn = unsafe extern "C" fn(
    gamecontroller: *mut c_void,
    low_frequency_rumble: u16,
    high_frequency_rumble: u16,
    duration_ms: u32,
) -> i32;
type SdlGameControllerNameFn = unsafe extern "C" fn(gamecontroller: *mut c_void) -> *const c_char;
type SdlGameControllerFromInstanceIdFn = unsafe extern "C" fn(joyid: i32) -> *mut c_void;
type SdlGameControllerAddMappingsFromFileFn = unsafe extern "C" fn(file: *const c_char) -> i32;
type SdlGameControllerAddMappingsFromRwFn =
    unsafe extern "C" fn(rw: *mut c_void, freerw: i32) -> i32;
type SdlRwFromFileFn =
    unsafe extern "C" fn(file: *const c_char, mode: *const c_char) -> *mut c_void;

type SdlGlSetAttributeFn = unsafe extern "C" fn(attr: i32, value: i32) -> i32;
pub type SdlAudioCallback =
    unsafe extern "C" fn(userdata: *mut c_void, stream: *mut u8, len: i32);

#[repr(C)]
#[derive(Copy, Clone)]
pub struct SdlAudioSpec {
    pub freq: i32,
    pub format: u16,
    pub channels: u8,
    pub silence: u8,
    pub samples: u16,
    pub padding: u16,
    pub size: u32,
    pub callback: Option<SdlAudioCallback>,
    pub userdata: *mut c_void,
}

type SdlOpenAudioDeviceFn = unsafe extern "C" fn(
    device: *const c_char,
    iscapture: i32,
    desired: *const SdlAudioSpec,
    obtained: *mut SdlAudioSpec,
    allowed_changes: i32,
) -> u32;
type SdlPauseAudioDeviceFn = unsafe extern "C" fn(dev: u32, pause_on: i32);
type SdlCloseAudioDeviceFn = unsafe extern "C" fn(dev: u32);

pub struct Sdl2AudioDevice {
    device_id: u32,
    obtained_freq: u32,
    obtained_samples: u32,
    close_fn: SdlCloseAudioDeviceFn,
    userdata: *mut c_void,
    drop_userdata: unsafe fn(*mut c_void),
}

unsafe impl Send for Sdl2AudioDevice {}

impl Sdl2AudioDevice {
    pub fn device_id(&self) -> u32 {
        self.device_id
    }

    pub fn obtained_freq(&self) -> u32 {
        self.obtained_freq
    }

    pub fn obtained_samples(&self) -> u32 {
        self.obtained_samples
    }
}

impl Drop for Sdl2AudioDevice {
    fn drop(&mut self) {
        // Close SDL audio device first so the callback cannot touch freed userdata.
        unsafe {
            (self.close_fn)(self.device_id);
            (self.drop_userdata)(self.userdata);
        }
    }
}

type SdlGlCreateContextFn = unsafe extern "C" fn(window: *mut c_void) -> *mut c_void;
type SdlGlDeleteContextFn = unsafe extern "C" fn(context: *mut c_void);
type SdlGlMakeCurrentFn = unsafe extern "C" fn(window: *mut c_void, context: *mut c_void) -> i32;
type SdlGlSwapWindowFn = unsafe extern "C" fn(window: *mut c_void);
type SdlGlGetDrawableSizeFn = unsafe extern "C" fn(window: *mut c_void, w: *mut i32, h: *mut i32);
type SdlGetWindowSizeFn = unsafe extern "C" fn(window: *mut c_void, w: *mut i32, h: *mut i32);
type SdlGlSetSwapIntervalFn = unsafe extern "C" fn(interval: i32) -> i32;

struct SdlPresentTarget {
    window: *mut c_void,
    swap_window_fn: SdlGlSwapWindowFn,
    drawable_size_fn: Option<SdlGlGetDrawableSizeFn>,
    window_size_fn: Option<SdlGetWindowSizeFn>,
    fallback_size: (u32, u32),
}

unsafe impl Send for SdlPresentTarget {}

impl PresentTarget for SdlPresentTarget {
    fn drawable_size(&self) -> (u32, u32) {
        let mut w = 0i32;
        let mut h = 0i32;
        if let Some(f) = self.drawable_size_fn {
            unsafe {
                f(self.window, &mut w, &mut h);
            }
            if w > 0 && h > 0 {
                return (w as u32, h as u32);
            }
        }
        if let Some(f) = self.window_size_fn {
            unsafe {
                f(self.window, &mut w, &mut h);
            }
            if w > 0 && h > 0 {
                return (w as u32, h as u32);
            }
        }
        self.fallback_size
    }

    fn swap_buffers(&mut self) {
        unsafe {
            (self.swap_window_fn)(self.window);
        }
    }
}

pub struct Sdl2WindowBackend {
    lib_handle: *mut c_void,
    quit_subsystem: SdlQuitSubSystemFn,
    init_subsystem_fn: Option<SdlInitSubSystemFn>,
    create_window_fn: SdlCreateWindowFn,
    destroy_window_fn: SdlDestroyWindowFn,
    show_window_fn: SdlShowWindowFn,
    hide_window_fn: SdlHideWindowFn,
    show_cursor_fn: SdlShowCursorFn,
    poll_event_fn: SdlPollEventFn,
    pump_events_fn: Option<SdlPumpEventsFn>,
    get_keyboard_state_fn: Option<SdlGetKeyboardStateFn>,
    get_rel_mouse_state_fn: Option<SdlGetMouseStateFn>,
    get_mouse_state_fn: Option<SdlGetMouseStateFn>,
    num_joysticks_fn: Option<SdlNumJoysticksFn>,
    is_game_controller_fn: Option<SdlIsGameControllerFn>,
    gc_open_fn: Option<SdlGameControllerOpenFn>,
    gc_close_fn: Option<SdlGameControllerCloseFn>,
    gc_update_fn: Option<SdlGameControllerUpdateFn>,
    gc_get_button_fn: Option<SdlGameControllerGetButtonFn>,
    gc_get_axis_fn: Option<SdlGameControllerGetAxisFn>,
    gc_rumble_fn: Option<SdlGameControllerRumbleFn>,
    gc_name_fn: Option<SdlGameControllerNameFn>,
    gc_from_instance_id_fn: Option<SdlGameControllerFromInstanceIdFn>,
    gc_add_mappings_from_file_fn: Option<SdlGameControllerAddMappingsFromFileFn>,
    gc_add_mappings_from_rw_fn: Option<SdlGameControllerAddMappingsFromRwFn>,
    rw_from_file_fn: Option<SdlRwFromFileFn>,
    open_audio_device_fn: Option<SdlOpenAudioDeviceFn>,
    pause_audio_device_fn: Option<SdlPauseAudioDeviceFn>,
    close_audio_device_fn: Option<SdlCloseAudioDeviceFn>,
    gl_set_attr_fn: Option<SdlGlSetAttributeFn>,
    gl_create_context_fn: Option<SdlGlCreateContextFn>,
    gl_delete_context_fn: Option<SdlGlDeleteContextFn>,
    gl_make_current_fn: Option<SdlGlMakeCurrentFn>,
    gl_swap_window_fn: Option<SdlGlSwapWindowFn>,
    gl_get_proc_addr_fn: Option<GlGetProcAddrFn>,
    gl_drawable_size_fn: Option<SdlGlGetDrawableSizeFn>,
    get_window_size_fn: Option<SdlGetWindowSizeFn>,
    gl_set_swap_interval_fn: Option<SdlGlSetSwapIntervalFn>,
    gl_contexts: Vec<*mut c_void>,
    windows: Vec<*mut c_void>,
    window_sizes: std::collections::HashMap<usize, (u32, u32)>,
    audio_inited: bool,
    no_gamepad: bool,
    gamecontroller_inited: bool,
    active_controller: Option<*mut c_void>,
    key_scancodes: [bool; SDL_SCANCODE_COUNT],
    mouse_wheel_delta: i32,
}

impl Sdl2WindowBackend {
    pub fn try_init(headless: bool, no_gamepad: bool) -> Option<Self> {
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

        let init_subsystem_fn = load_sym::<SdlInitSubSystemFn>(lib_handle, c"SDL_InitSubSystem");
        let mut gamecontroller_inited = false;
        if no_gamepad {
            if unsafe { init_fn(SDL_INIT_VIDEO) } != 0 {
                unsafe { libc::dlclose(lib_handle) };
                return None;
            }
        } else {
            gamecontroller_inited = unsafe { init_fn(SDL_INIT_VIDEO | SDL_INIT_GAMECONTROLLER) } == 0;
            if !gamecontroller_inited {
                if unsafe { init_fn(SDL_INIT_VIDEO) } != 0 {
                    unsafe { libc::dlclose(lib_handle) };
                    return None;
                }
                if let Some(init_sub) = init_subsystem_fn {
                    gamecontroller_inited = unsafe { init_sub(SDL_INIT_GAMECONTROLLER) } == 0;
                }
            }
        }

        let mut backend = Self {
            lib_handle,
            quit_subsystem,
            init_subsystem_fn,
            create_window_fn,
            destroy_window_fn,
            show_window_fn,
            hide_window_fn,
            show_cursor_fn,
            poll_event_fn,
            pump_events_fn: load_sym::<SdlPumpEventsFn>(lib_handle, c"SDL_PumpEvents"),
            get_keyboard_state_fn: load_sym::<SdlGetKeyboardStateFn>(lib_handle, c"SDL_GetKeyboardState"),
            get_rel_mouse_state_fn: load_sym::<SdlGetMouseStateFn>(
                lib_handle,
                c"SDL_GetRelativeMouseState",
            ),
            get_mouse_state_fn: load_sym::<SdlGetMouseStateFn>(lib_handle, c"SDL_GetMouseState"),
            num_joysticks_fn: load_sym::<SdlNumJoysticksFn>(lib_handle, c"SDL_NumJoysticks"),
            is_game_controller_fn: load_sym::<SdlIsGameControllerFn>(
                lib_handle,
                c"SDL_IsGameController",
            ),
            gc_open_fn: load_sym::<SdlGameControllerOpenFn>(lib_handle, c"SDL_GameControllerOpen"),
            gc_close_fn: load_sym::<SdlGameControllerCloseFn>(lib_handle, c"SDL_GameControllerClose"),
            gc_update_fn: load_sym::<SdlGameControllerUpdateFn>(
                lib_handle,
                c"SDL_GameControllerUpdate",
            ),
            gc_get_button_fn: load_sym::<SdlGameControllerGetButtonFn>(
                lib_handle,
                c"SDL_GameControllerGetButton",
            ),
            gc_get_axis_fn: load_sym::<SdlGameControllerGetAxisFn>(
                lib_handle,
                c"SDL_GameControllerGetAxis",
            ),
            gc_rumble_fn: load_sym::<SdlGameControllerRumbleFn>(
                lib_handle,
                c"SDL_GameControllerRumble",
            ),
            gc_name_fn: load_sym::<SdlGameControllerNameFn>(lib_handle, c"SDL_GameControllerName"),
            gc_from_instance_id_fn: load_sym::<SdlGameControllerFromInstanceIdFn>(
                lib_handle,
                c"SDL_GameControllerFromInstanceID",
            ),
            gc_add_mappings_from_file_fn: load_sym::<SdlGameControllerAddMappingsFromFileFn>(
                lib_handle,
                c"SDL_GameControllerAddMappingsFromFile",
            ),
            gc_add_mappings_from_rw_fn: load_sym::<SdlGameControllerAddMappingsFromRwFn>(
                lib_handle,
                c"SDL_GameControllerAddMappingsFromRW",
            ),
            rw_from_file_fn: load_sym::<SdlRwFromFileFn>(lib_handle, c"SDL_RWFromFile"),
            open_audio_device_fn: load_sym::<SdlOpenAudioDeviceFn>(
                lib_handle,
                c"SDL_OpenAudioDevice",
            ),
            pause_audio_device_fn: load_sym::<SdlPauseAudioDeviceFn>(
                lib_handle,
                c"SDL_PauseAudioDevice",
            ),
            close_audio_device_fn: load_sym::<SdlCloseAudioDeviceFn>(
                lib_handle,
                c"SDL_CloseAudioDevice",
            ),
            gl_set_attr_fn: load_sym::<SdlGlSetAttributeFn>(lib_handle, c"SDL_GL_SetAttribute"),
            gl_create_context_fn: load_sym::<SdlGlCreateContextFn>(
                lib_handle,
                c"SDL_GL_CreateContext",
            ),
            gl_delete_context_fn: load_sym::<SdlGlDeleteContextFn>(
                lib_handle,
                c"SDL_GL_DeleteContext",
            ),
            gl_make_current_fn: load_sym::<SdlGlMakeCurrentFn>(lib_handle, c"SDL_GL_MakeCurrent"),
            gl_swap_window_fn: load_sym::<SdlGlSwapWindowFn>(lib_handle, c"SDL_GL_SwapWindow"),
            gl_get_proc_addr_fn: load_sym::<GlGetProcAddrFn>(lib_handle, c"SDL_GL_GetProcAddress"),
            gl_drawable_size_fn: load_sym::<SdlGlGetDrawableSizeFn>(
                lib_handle,
                c"SDL_GL_GetDrawableSize",
            ),
            get_window_size_fn: load_sym::<SdlGetWindowSizeFn>(lib_handle, c"SDL_GetWindowSize"),
            gl_set_swap_interval_fn: load_sym::<SdlGlSetSwapIntervalFn>(
                lib_handle,
                c"SDL_GL_SetSwapInterval",
            ),
            gl_contexts: Vec::new(),
            windows: Vec::new(),
            window_sizes: std::collections::HashMap::new(),
            audio_inited: false,
            no_gamepad,
            gamecontroller_inited,
            active_controller: None,
            key_scancodes: [false; SDL_SCANCODE_COUNT],
            mouse_wheel_delta: 0,
        };

        if !backend.no_gamepad && backend.gamecontroller_inited {
            backend.load_controller_mapping_files();
            backend.open_first_controller();
        }

        Some(backend)
    }

    fn load_controller_mapping_files(&self) {
        if let Some(path) = std::env::var_os("SDL_GAMECONTROLLERCONFIG_FILE") {
            let path = Path::new(&path);
            if path.is_file() {
                self.add_mappings_from_file(path);
            }
        }
        let cwd_db = Path::new("gamecontrollerdb.txt");
        if cwd_db.is_file() {
            self.add_mappings_from_file(cwd_db);
        }
    }

    fn add_mappings_from_file(&self, path: &Path) {
        let Some(path_str) = path.to_str() else {
            return;
        };
        let Ok(c_path) = CString::new(path_str) else {
            return;
        };
        if let Some(add_file) = self.gc_add_mappings_from_file_fn {
            unsafe {
                add_file(c_path.as_ptr());
            }
            return;
        }
        // SDL2 defines SDL_GameControllerAddMappingsFromFile as a macro over RWFromFile.
        if let (Some(add_rw), Some(rw_from_file)) =
            (self.gc_add_mappings_from_rw_fn, self.rw_from_file_fn)
        {
            let rw = unsafe { rw_from_file(c_path.as_ptr(), c"rb".as_ptr()) };
            if !rw.is_null() {
                unsafe {
                    add_rw(rw, 1);
                }
            }
        }
    }

    fn open_first_controller(&mut self) {
        if self.no_gamepad || self.active_controller.is_some() {
            return;
        }
        let Some(num_joysticks) = self.num_joysticks_fn else {
            return;
        };
        let count = unsafe { num_joysticks() };
        for index in 0..count {
            if self.try_open_controller_at(index) {
                break;
            }
        }
    }

    fn try_open_controller_at(&mut self, device_index: i32) -> bool {
        if self.no_gamepad || self.active_controller.is_some() {
            return false;
        }
        let Some(is_gc) = self.is_game_controller_fn else {
            return false;
        };
        let Some(open_gc) = self.gc_open_fn else {
            return false;
        };
        if unsafe { is_gc(device_index) } == 0 {
            return false;
        }
        let ctrl = unsafe { open_gc(device_index) };
        if ctrl.is_null() {
            return false;
        }
        self.active_controller = Some(ctrl);
        true
    }

    fn handle_controller_removed(&mut self, instance_id: i32) {
        let Some(active) = self.active_controller else {
            return;
        };
        let matches_active = match self.gc_from_instance_id_fn {
            Some(from_id) => unsafe { from_id(instance_id) == active },
            None => true,
        };
        if !matches_active {
            return;
        }
        if let Some(close_gc) = self.gc_close_fn {
            unsafe {
                close_gc(active);
            }
        }
        self.active_controller = None;
        self.open_first_controller();
    }

    pub fn has_controller(&self) -> bool {
        self.active_controller.is_some()
    }

    pub fn controller_name(&self) -> Option<String> {
        let ctrl = self.active_controller?;
        let name_fn = self.gc_name_fn?;
        let ptr = unsafe { name_fn(ctrl) };
        if ptr.is_null() {
            return None;
        }
        let cstr = unsafe { CStr::from_ptr(ptr) };
        Some(cstr.to_string_lossy().into_owned())
    }

    pub fn init_controller_subsystem(&mut self) -> bool {
        if self.no_gamepad {
            return false;
        }
        if self.gamecontroller_inited {
            return true;
        }
        if let Some(init_sub) = self.init_subsystem_fn {
            self.gamecontroller_inited = unsafe { init_sub(SDL_INIT_GAMECONTROLLER) } == 0;
        }
        self.gamecontroller_inited
    }

    pub fn primary_window(&self) -> Option<*mut c_void> {
        self.windows.first().copied()
    }

    fn configure_gles3_attributes(&self) {
        if let Some(set_attr) = self.gl_set_attr_fn {
            unsafe {
                set_attr(SDL_GL_CONTEXT_PROFILE_MASK, SDL_GL_CONTEXT_PROFILE_ES);
                set_attr(SDL_GL_CONTEXT_MAJOR_VERSION, 3);
                set_attr(SDL_GL_CONTEXT_MINOR_VERSION, 0);
                set_attr(SDL_GL_DOUBLEBUFFER, 1);
                set_attr(SDL_GL_DEPTH_SIZE, 24);
                set_attr(SDL_GL_STENCIL_SIZE, 8);
            }
        }
    }

    /// Creates and activates an OpenGL ES 3.0 context for `window`.
    ///
    /// # Safety
    /// `window` must be a valid non-null SDL2 window pointer returned by `create_window`.
    pub unsafe fn create_gles3_context(
        &mut self,
        window: *mut c_void,
    ) -> Option<(GlGetProcAddrFn, Box<dyn PresentTarget>)> {
        if window.is_null() {
            return None;
        }
        let create_ctx = self.gl_create_context_fn?;
        let make_current = self.gl_make_current_fn?;
        let swap_window = self.gl_swap_window_fn?;
        let get_proc = self.gl_get_proc_addr_fn?;

        crate::win32::pin_current_thread_to_host_cores();
        let ctx = unsafe { create_ctx(window) };
        crate::win32::pin_current_thread_to_guest_core();
        if ctx.is_null() {
            return None;
        }
        if unsafe { make_current(window, ctx) } != 0 {
            if let Some(del_ctx) = self.gl_delete_context_fn {
                unsafe { del_ctx(ctx) };
            }
            return None;
        }
        if let Some(set_swap) = self.gl_set_swap_interval_fn {
            unsafe {
                set_swap(1);
            }
        }
        self.gl_contexts.push(ctx);
        let fallback_size = self
            .window_sizes
            .get(&(window as usize))
            .copied()
            .unwrap_or((0, 0));
        let target: Box<dyn PresentTarget> = Box::new(SdlPresentTarget {
            window,
            swap_window_fn: swap_window,
            drawable_size_fn: self.gl_drawable_size_fn,
            window_size_fn: self.get_window_size_fn,
            fallback_size,
        });
        Some((get_proc, target))
    }

    pub fn create_window(&mut self, title: &str, width: i32, height: i32) -> Option<*mut c_void> {
        self.create_window_with_options(title, width, height, false)
    }

    pub fn create_window_with_options(
        &mut self,
        title: &str,
        width: i32,
        height: i32,
        fullscreen: bool,
    ) -> Option<*mut c_void> {
        self.configure_gles3_attributes();
        let c_title = CString::new(title).unwrap_or_else(|_| CString::new("toluca-runtime").unwrap());
        let w = width.max(1);
        let h = height.max(1);
        let mut flags = SDL_WINDOW_SHOWN | SDL_WINDOW_OPENGL;
        if fullscreen {
            flags |= SDL_WINDOW_FULLSCREEN_DESKTOP;
        }
        let ptr = unsafe {
            (self.create_window_fn)(
                c_title.as_ptr(),
                SDL_WINDOWPOS_CENTERED,
                SDL_WINDOWPOS_CENTERED,
                w,
                h,
                flags,
            )
        };
        if ptr.is_null() {
            None
        } else {
            self.windows.push(ptr);
            self.window_sizes.insert(ptr as usize, (w as u32, h as u32));
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
        self.window_sizes.remove(&(ptr as usize));
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
                    let sc = buf.scancode();
                    if (0..SDL_SCANCODE_COUNT as i32).contains(&sc) {
                        self.key_scancodes[sc as usize] = true;
                    }
                    if let Some(vk) = scancode_to_vk(sc) {
                        events.push(SdlTranslatedEvent::KeyDown(vk));
                    }
                }
                SDL_KEYUP => {
                    let sc = buf.scancode();
                    if (0..SDL_SCANCODE_COUNT as i32).contains(&sc) {
                        self.key_scancodes[sc as usize] = false;
                    }
                    if let Some(vk) = scancode_to_vk(sc) {
                        events.push(SdlTranslatedEvent::KeyUp(vk));
                    }
                }
                SDL_MOUSEWHEEL => {
                    self.mouse_wheel_delta = self.mouse_wheel_delta.saturating_add(buf.wheel_y());
                }
                SDL_CONTROLLERDEVICEADDED => {
                    self.try_open_controller_at(buf.cdevice_which());
                }
                SDL_CONTROLLERDEVICEREMOVED => {
                    self.handle_controller_removed(buf.cdevice_which());
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

    pub fn poll_input_snapshot(&mut self) -> SdlInputSnapshot {
        if let Some(pump) = self.pump_events_fn {
            unsafe {
                pump();
            }
        }

        if let Some(get_kbd) = self.get_keyboard_state_fn {
            let mut numkeys = 0i32;
            let ptr = unsafe { get_kbd(&mut numkeys) };
            if !ptr.is_null() && numkeys > 0 {
                let count = (numkeys as usize).min(SDL_SCANCODE_COUNT);
                let slice = unsafe { std::slice::from_raw_parts(ptr, count) };
                for (dst, &src) in self.key_scancodes[..count].iter_mut().zip(slice.iter()) {
                    *dst = src != 0;
                }
            }
        }

        let mut mouse_dx = 0i32;
        let mut mouse_dy = 0i32;
        let mouse_buttons = if let Some(get_rel) = self.get_rel_mouse_state_fn {
            unsafe { get_rel(&mut mouse_dx, &mut mouse_dy) }
        } else if let Some(get_abs) = self.get_mouse_state_fn {
            let mut x = 0i32;
            let mut y = 0i32;
            unsafe { get_abs(&mut x, &mut y) }
        } else {
            0
        };
        let mouse_dz = std::mem::take(&mut self.mouse_wheel_delta);

        let gamepad = self.read_gamepad_snapshot();

        SdlInputSnapshot {
            key_scancodes: self.key_scancodes,
            mouse_dx,
            mouse_dy,
            mouse_dz,
            mouse_buttons,
            gamepad,
        }
    }

    fn read_gamepad_snapshot(&mut self) -> Option<GamepadSnapshot> {
        let ctrl = self.active_controller?;
        if let Some(update) = self.gc_update_fn {
            unsafe {
                update();
            }
        }

        let mut axes = [0i16; SDL_GAMEPAD_AXIS_COUNT];
        if let Some(get_axis) = self.gc_get_axis_fn {
            let sdl_axes = [
                SDL_CONTROLLER_AXIS_LEFTX,
                SDL_CONTROLLER_AXIS_LEFTY,
                SDL_CONTROLLER_AXIS_RIGHTX,
                SDL_CONTROLLER_AXIS_RIGHTY,
                SDL_CONTROLLER_AXIS_TRIGGERLEFT,
                SDL_CONTROLLER_AXIS_TRIGGERRIGHT,
            ];
            for (dst, sdl_axis) in axes.iter_mut().zip(sdl_axes) {
                *dst = unsafe { get_axis(ctrl, sdl_axis) };
            }
        }

        let mut buttons = [false; SDL_GAMEPAD_BUTTON_COUNT];
        let mut dpad_up = false;
        let mut dpad_down = false;
        let mut dpad_left = false;
        let mut dpad_right = false;

        if let Some(get_btn) = self.gc_get_button_fn {
            let pressed = |btn: i32| unsafe { get_btn(ctrl, btn) != 0 };
            buttons[0] = pressed(SDL_CONTROLLER_BUTTON_A);
            buttons[1] = pressed(SDL_CONTROLLER_BUTTON_B);
            buttons[2] = pressed(SDL_CONTROLLER_BUTTON_X);
            buttons[3] = pressed(SDL_CONTROLLER_BUTTON_Y);
            buttons[4] = pressed(SDL_CONTROLLER_BUTTON_LEFTSHOULDER);
            buttons[5] = pressed(SDL_CONTROLLER_BUTTON_RIGHTSHOULDER);
            buttons[6] = pressed(SDL_CONTROLLER_BUTTON_BACK);
            buttons[7] = pressed(SDL_CONTROLLER_BUTTON_START);
            buttons[10] = pressed(SDL_CONTROLLER_BUTTON_LEFTSTICK);
            buttons[11] = pressed(SDL_CONTROLLER_BUTTON_RIGHTSTICK);
            dpad_up = pressed(SDL_CONTROLLER_BUTTON_DPAD_UP);
            dpad_down = pressed(SDL_CONTROLLER_BUTTON_DPAD_DOWN);
            dpad_left = pressed(SDL_CONTROLLER_BUTTON_DPAD_LEFT);
            dpad_right = pressed(SDL_CONTROLLER_BUTTON_DPAD_RIGHT);
        }

        buttons[8] = axes[4] > SDL_TRIGGER_DIGITAL_THRESHOLD;
        buttons[9] = axes[5] > SDL_TRIGGER_DIGITAL_THRESHOLD;
        apply_dpad_to_left_stick(&mut axes, dpad_up, dpad_down, dpad_left, dpad_right);

        Some(GamepadSnapshot {
            axes,
            dpad_hat: dpad_to_pov(dpad_up, dpad_down, dpad_left, dpad_right),
            buttons,
        })
    }

    pub fn set_controller_rumble(
        &mut self,
        low_freq: u16,
        high_freq: u16,
        duration_ms: u32,
    ) -> bool {
        let Some(ctrl) = self.active_controller else {
            return false;
        };
        let Some(rumble_fn) = self.gc_rumble_fn else {
            return false;
        };
        unsafe { rumble_fn(ctrl, low_freq, high_freq, duration_ms) == 0 }
    }

    pub fn open_audio_device<T>(
        &mut self,
        callback: SdlAudioCallback,
        userdata: T,
    ) -> Option<Sdl2AudioDevice> {
        let open_fn = self.open_audio_device_fn?;
        let pause_fn = self.pause_audio_device_fn?;
        let close_fn = self.close_audio_device_fn?;

        if !self.audio_inited {
            let init_sub = self.init_subsystem_fn?;
            if unsafe { init_sub(SDL_INIT_AUDIO) } != 0 {
                return None;
            }
            self.audio_inited = true;
        }

        let boxed = Box::into_raw(Box::new(userdata)).cast::<c_void>();
        let desired = SdlAudioSpec {
            freq: SDL_AUDIO_DEFAULT_FREQ,
            format: SDL_AUDIO_S16LSB,
            channels: SDL_AUDIO_DEFAULT_CHANNELS,
            silence: 0,
            samples: SDL_AUDIO_DEFAULT_SAMPLES,
            padding: 0,
            size: 0,
            callback: Some(callback),
            userdata: boxed,
        };
        let mut obtained = unsafe { std::mem::zeroed::<SdlAudioSpec>() };
        crate::win32::pin_current_thread_to_host_cores();
        let dev_id = unsafe {
            open_fn(
                std::ptr::null(),
                0,
                &desired,
                &mut obtained,
                SDL_AUDIO_ALLOW_FREQUENCY_CHANGE,
            )
        };
        crate::win32::pin_current_thread_to_guest_core();
        if dev_id == 0 {
            unsafe {
                drop(Box::from_raw(boxed.cast::<T>()));
            }
            return None;
        }
        let obtained_freq = if obtained.freq > 0 {
            obtained.freq as u32
        } else {
            SDL_AUDIO_DEFAULT_FREQ as u32
        };
        let obtained_samples = if obtained.samples > 0 {
            u32::from(obtained.samples)
        } else {
            u32::from(SDL_AUDIO_DEFAULT_SAMPLES)
        };
        let obtained_ch = if obtained.channels > 0 {
            obtained.channels
        } else {
            SDL_AUDIO_DEFAULT_CHANNELS
        };
        if std::env::var_os("TOLUCA_PERF").is_some_and(|v| !v.is_empty() && v != "0") {
            eprintln!(
                "[toluca-audio] opened freq={} samples={} ch={}",
                obtained_freq, obtained_samples, obtained_ch
            );
        }

        unsafe {
            pause_fn(dev_id, 0);
        }

        unsafe fn drop_boxed<U>(ptr: *mut c_void) {
            if !ptr.is_null() {
                unsafe {
                    drop(Box::from_raw(ptr.cast::<U>()));
                }
            }
        }

        Some(Sdl2AudioDevice {
            device_id: dev_id,
            obtained_freq,
            obtained_samples,
            close_fn,
            userdata: boxed,
            drop_userdata: drop_boxed::<T>,
        })
    }
}

pub fn dpad_to_pov(up: bool, down: bool, left: bool, right: bool) -> u32 {
    let vert = i8::from(up) - i8::from(down);
    let horiz = i8::from(right) - i8::from(left);
    match (vert, horiz) {
        (1, 0) => 0,
        (1, 1) => 4500,
        (0, 1) => 9000,
        (-1, 1) => 13500,
        (-1, 0) => 18000,
        (-1, -1) => 22500,
        (0, -1) => 27000,
        (1, -1) => 31500,
        _ => DI_POV_CENTERED,
    }
}

pub fn apply_dpad_to_left_stick(
    axes: &mut [i16; SDL_GAMEPAD_AXIS_COUNT],
    dpad_up: bool,
    dpad_down: bool,
    dpad_left: bool,
    dpad_right: bool,
) {
    if i32::from(axes[0]).abs() < DPAD_AXIS_DEADZONE {
        if dpad_left && !dpad_right {
            axes[0] = i16::MIN;
        } else if dpad_right && !dpad_left {
            axes[0] = i16::MAX;
        }
    }
    if i32::from(axes[1]).abs() < DPAD_AXIS_DEADZONE {
        if dpad_up && !dpad_down {
            axes[1] = i16::MIN;
        } else if dpad_down && !dpad_up {
            axes[1] = i16::MAX;
        }
    }
}

impl Drop for Sdl2WindowBackend {
    fn drop(&mut self) {
        if let Some(ctrl) = self.active_controller.take() {
            if let Some(close_gc) = self.gc_close_fn {
                unsafe {
                    close_gc(ctrl);
                }
            }
        }
        if let Some(del_ctx) = self.gl_delete_context_fn {
            while let Some(ctx) = self.gl_contexts.pop() {
                unsafe {
                    del_ctx(ctx);
                }
            }
        }
        while let Some(win) = self.windows.pop() {
            unsafe {
                (self.destroy_window_fn)(win);
            }
        }
        unsafe {
            if self.audio_inited {
                (self.quit_subsystem)(SDL_INIT_AUDIO);
            }
            if self.gamecontroller_inited {
                (self.quit_subsystem)(SDL_INIT_GAMECONTROLLER);
            }
            (self.quit_subsystem)(SDL_INIT_VIDEO);
            libc::dlclose(self.lib_handle);
        }
    }
}

fn open_sdl2_library() -> Option<*mut c_void> {
    for candidate in [c"libSDL2-2.0.so.0", c"libSDL2.so"] {
        // Use RTLD_GLOBAL so LD_PRELOAD shims (such as handheld input remappers
        // that hook SDL_* and forward via RTLD_NEXT) can resolve real SDL symbols.
        let handle =
            unsafe { libc::dlopen(candidate.as_ptr(), libc::RTLD_LAZY | libc::RTLD_GLOBAL) };
        if !handle.is_null() {
            return Some(handle);
        }
    }
    None
}

fn load_sym<F: Copy>(lib_handle: *mut c_void, name: &CStr) -> Option<F> {
    // Search RTLD_DEFAULT first so LD_PRELOAD interposers (such as NextUI/PortMaster
    // input shims) intercept SDL_Init/SDL_PollEvent/SDL_GetKeyboardState.
    let mut ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    if ptr.is_null() {
        ptr = unsafe { libc::dlsym(lib_handle, name.as_ptr()) };
    }
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
        assert!(Sdl2WindowBackend::try_init(true, false).is_none());
        assert!(Sdl2WindowBackend::try_init(true, true).is_none());
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

    #[test]
    fn dpad_to_pov_computes_all_eight_directions_and_centered() {
        assert_eq!(dpad_to_pov(false, false, false, false), DI_POV_CENTERED);
        assert_eq!(dpad_to_pov(true, true, false, false), DI_POV_CENTERED);
        assert_eq!(dpad_to_pov(true, false, false, false), 0);
        assert_eq!(dpad_to_pov(true, false, false, true), 4500);
        assert_eq!(dpad_to_pov(false, false, false, true), 9000);
        assert_eq!(dpad_to_pov(false, true, false, true), 13500);
        assert_eq!(dpad_to_pov(false, true, false, false), 18000);
        assert_eq!(dpad_to_pov(false, true, true, false), 22500);
        assert_eq!(dpad_to_pov(false, false, true, false), 27000);
        assert_eq!(dpad_to_pov(true, false, true, false), 31500);
    }

    #[test]
    fn dpad_mirrors_onto_left_stick_only_within_deadzone() {
        let mut axes = [100, -200, 0, 0, 0, 0];
        apply_dpad_to_left_stick(&mut axes, true, false, true, false);
        assert_eq!(axes[0], -32768);
        assert_eq!(axes[1], -32768);

        let mut active_stick = [15000, -15000, 0, 0, 0, 0];
        apply_dpad_to_left_stick(&mut active_stick, false, true, true, false);
        assert_eq!(active_stick[0], 15000);
        assert_eq!(active_stick[1], -15000);
    }
}
