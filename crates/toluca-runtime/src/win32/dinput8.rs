use std::collections::{HashMap, VecDeque};

use toluca_core::GuestAddr;
use toluca_core::aot::ImportSymbol;
use toluca_core::cpu::GuestCpuState;

use crate::arena::GuestArena;
use crate::callconv::{peek_arg_u32, return_stdcall};
use crate::runtime::{ImportHandler, ImportRegistry, RuntimeContext, RuntimeError};
use crate::win32::Win32Msg;
use crate::win32::sdl2::{
    GamepadSnapshot, SDL_GAMEPAD_AXIS_COUNT, SDL_GAMEPAD_BUTTON_COUNT, SdlInputSnapshot,
};

const DINPUT8_COM_DLL: &str = "__dinput8_com__";

const DI8_VTABLE_SLOTS: usize = 11;
const DIDEV8_VTABLE_SLOTS: usize = 32;
const DIEFF_VTABLE_SLOTS: usize = 16;

pub const DI_OK: u32 = 0;
pub const DI_PROPNOEFFECT: u32 = 1;
pub const DI_NOTATTACHED: u32 = 1;
pub const DIENUM_STOP: u32 = 0;
pub const DIENUM_CONTINUE: u32 = 1;

pub const DIERR_NOTACQUIRED: u32 = 0x8007_001E;
pub const DIERR_INVALIDPARAM: u32 = 0x8007_0057;
pub const DIERR_OBJECTNOTFOUND: u32 = 0x8007_0002;
pub const DIERR_UNSUPPORTED: u32 = 0x8000_4001;
pub const DIERR_NOINTERFACE: u32 = 0x8000_4002;
pub const DIERR_GENERIC: u32 = 0x8000_4005;

pub const DI8DEVCLASS_ALL: u32 = 0;
pub const DI8DEVCLASS_DEVICE: u32 = 1;
pub const DI8DEVCLASS_POINTER: u32 = 2;
pub const DI8DEVCLASS_KEYBOARD: u32 = 3;
pub const DI8DEVCLASS_GAMECTRL: u32 = 4;

pub const DI8DEVTYPE_DEVICE: u32 = 0x11;
pub const DI8DEVTYPE_MOUSE: u32 = 0x12;
pub const DI8DEVTYPE_KEYBOARD: u32 = 0x13;
pub const DI8DEVTYPE_JOYSTICK: u32 = 0x14;
pub const DI8DEVTYPE_GAMEPAD: u32 = 0x15;
pub const DI8DEVTYPE_DRIVING: u32 = 0x16;
pub const DI8DEVTYPE_FLIGHT: u32 = 0x17;
pub const DI8DEVTYPE_1STPERSON: u32 = 0x18;

const DI8DEVTYPEMOUSE_TRADITIONAL: u32 = 2;
const DI8DEVTYPEKEYBOARD_PCENH: u32 = 4;
const DI8DEVTYPEGAMEPAD_STANDARD: u32 = 2;

const DIDC_ATTACHED: u32 = 0x0000_0001;
const DIDC_EMULATED: u32 = 0x0000_0004;
const DIDC_FORCEFEEDBACK: u32 = 0x0000_0100;

const DIDFT_RELAXIS: u32 = 0x0000_0001;
const DIDFT_ABSAXIS: u32 = 0x0000_0002;
const DIDFT_PSHBUTTON: u32 = 0x0000_0004;
const DIDFT_POV: u32 = 0x0000_0010;
const DIDFT_FFACTUATOR: u32 = 0x0100_0000;

const DIDOI_FFACTUATOR: u32 = 0x0000_0001;
const DIDOI_ASPECTPOSITION: u32 = 0x0000_0100;

const DIPH_DEVICE: u32 = 0;
const DIPH_BYOFFSET: u32 = 1;
const DIPH_BYID: u32 = 2;

pub const DIPROP_BUFFERSIZE: u32 = 1;
pub const DIPROP_AXISMODE: u32 = 2;
pub const DIPROP_GRANULARITY: u32 = 3;
pub const DIPROP_RANGE: u32 = 4;
pub const DIPROP_DEADZONE: u32 = 5;
pub const DIPROP_SATURATION: u32 = 6;
pub const DIPROP_FFGAIN: u32 = 9;
pub const DIPROP_AUTOCENTER: u32 = 11;

const DIPROPAXISMODE_ABS: u32 = 0;
const DIGDD_PEEK: u32 = 0x0000_0001;

const DISFFC_RESET: u32 = 0x0000_0001;
const DISFFC_STOPALL: u32 = 0x0000_0002;
const DIGFFS_EMPTY: u32 = 0x0000_0001;
const DIGFFS_STOPPED: u32 = 0x0000_0002;
const DIEGES_PLAYING: u32 = 0x0000_0001;
const DIEP_START: u32 = 0x2000_0000;
const DI_FFNOMINALMAX: u32 = 10_000;

const DIDEVICEINSTANCEA_SIZE_DX5: u32 = 580;
const DIDEVICEINSTANCEA_SIZE_DX3: u32 = 560;
const DIDEVCAPS_SIZE_DX5: u32 = 44;
const DIDEVCAPS_SIZE_DX3: u32 = 20;
const DIDEVICEOBJECTINSTANCEA_SIZE_DX5: u32 = 316;
const DIDEVICEOBJECTINSTANCEA_SIZE_DX3: u32 = 288;
const DIEFFECTINFOA_SIZE: u32 = 292;
const DIEFFECT_SIZE_DX5: u32 = 52;
const DIEFFECT_SIZE_DX6: u32 = 56;
const DIPROPHEADER_SIZE: u32 = 16;
const DIPROPDWORD_SIZE: u32 = 20;
const DIPROPRANGE_SIZE: u32 = 24;

pub const DI_POV_CENTERED: u32 = 0xFFFF_FFFF;
pub const DIMOUSESTATE_SIZE: usize = 16;
pub const DIMOUSESTATE2_SIZE: usize = 20;
pub const DIJOYSTATE_SIZE: usize = 80;
pub const DIJOYSTATE2_SIZE: usize = 272;
const WHEEL_DELTA: i32 = 120;
const DEFAULT_AXIS_MIN: i32 = -1000;
const DEFAULT_AXIS_MAX: i32 = 1000;
const MAX_DEVICE_BUFFER_SIZE: u32 = 4096;
const MAX_RUMBLE_MS: u32 = 60_000;

const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;

pub const DIK_ESCAPE: u8 = 0x01;
pub const DIK_1: u8 = 0x02;
pub const DIK_2: u8 = 0x03;
pub const DIK_3: u8 = 0x04;
pub const DIK_4: u8 = 0x05;
pub const DIK_5: u8 = 0x06;
pub const DIK_6: u8 = 0x07;
pub const DIK_7: u8 = 0x08;
pub const DIK_8: u8 = 0x09;
pub const DIK_9: u8 = 0x0A;
pub const DIK_0: u8 = 0x0B;
pub const DIK_MINUS: u8 = 0x0C;
pub const DIK_EQUALS: u8 = 0x0D;
pub const DIK_BACK: u8 = 0x0E;
pub const DIK_TAB: u8 = 0x0F;
pub const DIK_Q: u8 = 0x10;
pub const DIK_W: u8 = 0x11;
pub const DIK_E: u8 = 0x12;
pub const DIK_R: u8 = 0x13;
pub const DIK_T: u8 = 0x14;
pub const DIK_Y: u8 = 0x15;
pub const DIK_U: u8 = 0x16;
pub const DIK_I: u8 = 0x17;
pub const DIK_O: u8 = 0x18;
pub const DIK_P: u8 = 0x19;
pub const DIK_LBRACKET: u8 = 0x1A;
pub const DIK_RBRACKET: u8 = 0x1B;
pub const DIK_RETURN: u8 = 0x1C;
pub const DIK_LCONTROL: u8 = 0x1D;
pub const DIK_A: u8 = 0x1E;
pub const DIK_S: u8 = 0x1F;
pub const DIK_D: u8 = 0x20;
pub const DIK_F: u8 = 0x21;
pub const DIK_G: u8 = 0x22;
pub const DIK_H: u8 = 0x23;
pub const DIK_J: u8 = 0x24;
pub const DIK_K: u8 = 0x25;
pub const DIK_L: u8 = 0x26;
pub const DIK_SEMICOLON: u8 = 0x27;
pub const DIK_APOSTROPHE: u8 = 0x28;
pub const DIK_GRAVE: u8 = 0x29;
pub const DIK_LSHIFT: u8 = 0x2A;
pub const DIK_BACKSLASH: u8 = 0x2B;
pub const DIK_Z: u8 = 0x2C;
pub const DIK_X: u8 = 0x2D;
pub const DIK_C: u8 = 0x2E;
pub const DIK_V: u8 = 0x2F;
pub const DIK_B: u8 = 0x30;
pub const DIK_N: u8 = 0x31;
pub const DIK_M: u8 = 0x32;
pub const DIK_COMMA: u8 = 0x33;
pub const DIK_PERIOD: u8 = 0x34;
pub const DIK_SLASH: u8 = 0x35;
pub const DIK_RSHIFT: u8 = 0x36;
pub const DIK_MULTIPLY: u8 = 0x37;
pub const DIK_LMENU: u8 = 0x38;
pub const DIK_SPACE: u8 = 0x39;
pub const DIK_CAPITAL: u8 = 0x3A;
pub const DIK_F1: u8 = 0x3B;
pub const DIK_F2: u8 = 0x3C;
pub const DIK_F3: u8 = 0x3D;
pub const DIK_F4: u8 = 0x3E;
pub const DIK_F5: u8 = 0x3F;
pub const DIK_F6: u8 = 0x40;
pub const DIK_F7: u8 = 0x41;
pub const DIK_F8: u8 = 0x42;
pub const DIK_F9: u8 = 0x43;
pub const DIK_F10: u8 = 0x44;
pub const DIK_NUMLOCK: u8 = 0x45;
pub const DIK_SCROLL: u8 = 0x46;
pub const DIK_NUMPAD7: u8 = 0x47;
pub const DIK_NUMPAD8: u8 = 0x48;
pub const DIK_NUMPAD9: u8 = 0x49;
pub const DIK_SUBTRACT: u8 = 0x4A;
pub const DIK_NUMPAD4: u8 = 0x4B;
pub const DIK_NUMPAD5: u8 = 0x4C;
pub const DIK_NUMPAD6: u8 = 0x4D;
pub const DIK_ADD: u8 = 0x4E;
pub const DIK_NUMPAD1: u8 = 0x4F;
pub const DIK_NUMPAD2: u8 = 0x50;
pub const DIK_NUMPAD3: u8 = 0x51;
pub const DIK_NUMPAD0: u8 = 0x52;
pub const DIK_DECIMAL: u8 = 0x53;
pub const DIK_OEM_102: u8 = 0x56;
pub const DIK_F11: u8 = 0x57;
pub const DIK_F12: u8 = 0x58;
pub const DIK_F13: u8 = 0x64;
pub const DIK_F14: u8 = 0x65;
pub const DIK_F15: u8 = 0x66;
pub const DIK_NUMPADENTER: u8 = 0x9C;
pub const DIK_RCONTROL: u8 = 0x9D;
pub const DIK_DIVIDE: u8 = 0xB5;
pub const DIK_SYSRQ: u8 = 0xB7;
pub const DIK_RMENU: u8 = 0xB8;
pub const DIK_PAUSE: u8 = 0xC5;
pub const DIK_HOME: u8 = 0xC7;
pub const DIK_UP: u8 = 0xC8;
pub const DIK_PRIOR: u8 = 0xC9;
pub const DIK_LEFT: u8 = 0xCB;
pub const DIK_RIGHT: u8 = 0xCD;
pub const DIK_END: u8 = 0xCF;
pub const DIK_DOWN: u8 = 0xD0;
pub const DIK_NEXT: u8 = 0xD1;
pub const DIK_INSERT: u8 = 0xD2;
pub const DIK_DELETE: u8 = 0xD3;
pub const DIK_LWIN: u8 = 0xDB;
pub const DIK_RWIN: u8 = 0xDC;
pub const DIK_APPS: u8 = 0xDD;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WinGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl WinGuid {
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }

    pub fn to_bytes(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.data1.to_le_bytes());
        out[4..6].copy_from_slice(&self.data2.to_le_bytes());
        out[6..8].copy_from_slice(&self.data3.to_le_bytes());
        out[8..16].copy_from_slice(&self.data4);
        out
    }

    pub fn read_from(arena: &GuestArena, va: GuestAddr) -> Result<Self, RuntimeError> {
        let b = arena.read_bytes(va, 16)?;
        Ok(Self {
            data1: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            data2: u16::from_le_bytes([b[4], b[5]]),
            data3: u16::from_le_bytes([b[6], b[7]]),
            data4: [b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]],
        })
    }

    pub fn write_to(self, arena: &mut GuestArena, va: GuestAddr) -> Result<(), RuntimeError> {
        arena.write_bytes(va, &self.to_bytes())?;
        Ok(())
    }
}

pub const GUID_NULL: WinGuid = WinGuid::new(0, 0, 0, [0; 8]);
pub const IID_IUNKNOWN: WinGuid = WinGuid::new(0x0000_0000, 0x0000, 0x0000, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_IDIRECTINPUT8A: WinGuid = WinGuid::new(
    0xBF79_8030,
    0x483A,
    0x4DA2,
    [0xAA, 0x99, 0x5D, 0x64, 0xED, 0x36, 0x97, 0x00],
);
pub const IID_IDIRECTINPUTDEVICE8A: WinGuid = WinGuid::new(
    0x54D4_1080,
    0xDC15,
    0x4833,
    [0xA4, 0x1B, 0x74, 0x8F, 0x73, 0xA3, 0x81, 0x79],
);
pub const IID_IDIRECTINPUTEFFECT: WinGuid = WinGuid::new(
    0xE7E1_F7C0,
    0x88D7,
    0x11D0,
    [0x9A, 0xD0, 0x00, 0xA0, 0xC9, 0xA0, 0x6E, 0x35],
);

pub const GUID_SYSMOUSE: WinGuid = WinGuid::new(
    0x6F1D_2B60,
    0xD5A0,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);
pub const GUID_SYSKEYBOARD: WinGuid = WinGuid::new(
    0x6F1D_2B61,
    0xD5A0,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);
pub const GUID_TOLUCA_GAMEPAD: WinGuid = WinGuid::new(
    0x5348_3200,
    0x4741,
    0x4D45,
    [0x50, 0x41, 0x44, 0x30, 0x30, 0x30, 0x30, 0x31],
);

const GUID_XAXIS: WinGuid = WinGuid::new(0xA36D_02E0, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
const GUID_YAXIS: WinGuid = WinGuid::new(0xA36D_02E1, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
const GUID_ZAXIS: WinGuid = WinGuid::new(0xA36D_02E2, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
const GUID_RZAXIS: WinGuid = WinGuid::new(0xA36D_02E3, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
const GUID_RXAXIS: WinGuid = WinGuid::new(0xA36D_02F4, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
const GUID_RYAXIS: WinGuid = WinGuid::new(0xA36D_02F5, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
const GUID_POV: WinGuid = WinGuid::new(0xA36D_02F2, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
const GUID_BUTTON: WinGuid = WinGuid::new(0xA36D_02F0, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
const GUID_KEY: WinGuid = WinGuid::new(0x5572_8220, 0xD33C, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
const GUID_CONSTANTFORCE: WinGuid = WinGuid::new(
    0x1354_1C20,
    0x8E33,
    0x11D0,
    [0x9A, 0xD0, 0x00, 0xA0, 0xC9, 0xA0, 0x6E, 0x35],
);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ScriptedKeyEvent {
    pub start_frame: u64,
    pub duration_frames: u64,
    pub dik_code: u8,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DinputDeviceKind {
    Keyboard,
    Mouse,
    Gamepad,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BufferedObjectData {
    pub dw_ofs: u32,
    pub dw_data: u32,
    pub dw_time_stamp: u32,
    pub dw_sequence: u32,
}

#[derive(Clone, Debug)]
pub struct DinputInterfaceObject {
    pub ref_count: u32,
}

#[derive(Clone, Debug)]
pub struct DinputDeviceState {
    pub kind: DinputDeviceKind,
    pub ref_count: u32,
    pub acquired: bool,
    pub data_format_size: u32,
    pub default_axis_range: (i32, i32),
    pub axis_ranges: [Option<(i32, i32)>; SDL_GAMEPAD_AXIS_COUNT],
    pub axis_mode: u32,
    pub deadzone: u32,
    pub saturation: u32,
    pub ff_gain: u32,
    pub buffer_size: u32,
    pub event_queue: VecDeque<BufferedObjectData>,
    pub prev_keys: [bool; 256],
    pub prev_mouse_buttons: [bool; 8],
    pub pending_mouse_delta: (i32, i32, i32),
    pub prev_axes: [i32; SDL_GAMEPAD_AXIS_COUNT],
    pub prev_pov: u32,
    pub prev_gamepad_buttons: [bool; SDL_GAMEPAD_BUTTON_COUNT],
}

impl DinputDeviceState {
    pub fn new(kind: DinputDeviceKind) -> Self {
        let default_format = match kind {
            DinputDeviceKind::Keyboard => 256,
            DinputDeviceKind::Mouse => DIMOUSESTATE_SIZE as u32,
            DinputDeviceKind::Gamepad => DIJOYSTATE_SIZE as u32,
        };
        Self {
            kind,
            ref_count: 1,
            acquired: false,
            data_format_size: default_format,
            default_axis_range: (DEFAULT_AXIS_MIN, DEFAULT_AXIS_MAX),
            axis_ranges: [None; SDL_GAMEPAD_AXIS_COUNT],
            axis_mode: DIPROPAXISMODE_ABS,
            deadzone: 0,
            saturation: DI_FFNOMINALMAX,
            ff_gain: DI_FFNOMINALMAX,
            buffer_size: 0,
            event_queue: VecDeque::new(),
            prev_keys: [false; 256],
            prev_mouse_buttons: [false; 8],
            pending_mouse_delta: (0, 0, 0),
            prev_axes: [0; SDL_GAMEPAD_AXIS_COUNT],
            prev_pov: DI_POV_CENTERED,
            prev_gamepad_buttons: [false; SDL_GAMEPAD_BUTTON_COUNT],
        }
    }

    fn axis_range_for(&self, axis_idx: usize) -> (i32, i32) {
        self.axis_ranges
            .get(axis_idx)
            .copied()
            .flatten()
            .unwrap_or(self.default_axis_range)
    }

    fn push_buffered(&mut self, item: BufferedObjectData) {
        if self.buffer_size == 0 {
            return;
        }
        let cap = self.buffer_size as usize;
        while self.event_queue.len() >= cap {
            self.event_queue.pop_front();
        }
        self.event_queue.push_back(item);
    }
}

#[derive(Clone, Debug)]
pub struct DinputEffectState {
    pub ref_count: u32,
    pub device_va: GuestAddr,
    pub guid: WinGuid,
    pub duration_us: u32,
    pub gain: u32,
    pub magnitude: u32,
    pub started: bool,
}

impl DinputEffectState {
    fn new(device_va: GuestAddr, guid: WinGuid) -> Self {
        Self {
            ref_count: 1,
            device_va,
            guid,
            duration_us: 200_000,
            gain: DI_FFNOMINALMAX,
            magnitude: DI_FFNOMINALMAX,
            started: false,
        }
    }
}

pub struct Dinput8State {
    pub interfaces: HashMap<GuestAddr, DinputInterfaceObject>,
    pub devices: HashMap<GuestAddr, DinputDeviceState>,
    pub effects: HashMap<GuestAddr, DinputEffectState>,
    pub script_keys: Vec<ScriptedKeyEvent>,
    pub active_script_diks: [bool; 256],
    pub last_script_frame: Option<u64>,
    pub next_sequence: u32,
    pub override_snapshot: Option<SdlInputSnapshot>,
    pub override_gamepad_name: Option<String>,
    pub last_rumble_command: Option<(u16, u16, u32)>,
}

impl Dinput8State {
    pub fn new(script_keys: &[ScriptedKeyEvent]) -> Self {
        Self {
            interfaces: HashMap::new(),
            devices: HashMap::new(),
            effects: HashMap::new(),
            script_keys: script_keys.to_vec(),
            active_script_diks: [false; 256],
            last_script_frame: None,
            next_sequence: 1,
            override_snapshot: None,
            override_gamepad_name: None,
            last_rumble_command: None,
        }
    }
}

pub fn register_imports(registry: &mut ImportRegistry) {
    registry.register_name("dinput8.dll", "DirectInput8Create", direct_input_8_create);

    let entries: &[(&str, ImportHandler)] = &[
        ("DI8_QueryInterface", di8_query_interface),
        ("DI8_AddRef", di8_add_ref),
        ("DI8_Release", di8_release),
        ("DI8_CreateDevice", di8_create_device),
        ("DI8_EnumDevices", di8_enum_devices),
        ("DI8_GetDeviceStatus", di8_get_device_status),
        ("DI8_FindDevice", di8_find_device),
        ("DIDev8_QueryInterface", didev8_query_interface),
        ("DIDev8_AddRef", didev8_add_ref),
        ("DIDev8_Release", didev8_release),
        ("DIDev8_GetCapabilities", didev8_get_capabilities),
        ("DIDev8_EnumObjects", didev8_enum_objects),
        ("DIDev8_GetProperty", didev8_get_property),
        ("DIDev8_SetProperty", didev8_set_property),
        ("DIDev8_Acquire", didev8_acquire),
        ("DIDev8_Unacquire", didev8_unacquire),
        ("DIDev8_GetDeviceState", didev8_get_device_state),
        ("DIDev8_GetDeviceData", didev8_get_device_data),
        ("DIDev8_SetDataFormat", didev8_set_data_format),
        ("DIDev8_GetObjectInfo", didev8_get_object_info),
        ("DIDev8_GetDeviceInfo", didev8_get_device_info),
        ("DIDev8_CreateEffect", didev8_create_effect),
        ("DIDev8_EnumEffects", didev8_enum_effects),
        ("DIDev8_GetEffectInfo", didev8_get_effect_info),
        ("DIDev8_GetForceFeedbackState", didev8_get_force_feedback_state),
        ("DIDev8_SendForceFeedbackCommand", didev8_send_force_feedback_command),
        ("DIDev8_Poll", didev8_poll),
        ("DIEff_QueryInterface", dieff_query_interface),
        ("DIEff_AddRef", dieff_add_ref),
        ("DIEff_Release", dieff_release),
        ("DIEff_GetEffectGuid", dieff_get_effect_guid),
        ("DIEff_GetParameters", dieff_get_parameters),
        ("DIEff_SetParameters", dieff_set_parameters),
        ("DIEff_Start", dieff_start),
        ("DIEff_Stop", dieff_stop),
        ("DIEff_GetEffectStatus", dieff_get_effect_status),
        ("Std0_1", stdcall_zero_1),
        ("Std0_2", stdcall_zero_2),
        ("Std0_3", stdcall_zero_3),
        ("Std0_4", stdcall_zero_4),
        ("Std0_5", stdcall_zero_5),
        ("Std0_6", stdcall_zero_6),
    ];
    for &(name, handler) in entries {
        registry.register_name(DINPUT8_COM_DLL, name, handler);
    }
}

fn alloc_com_object(
    ctx: &mut RuntimeContext<'_>,
    slot_names: &[&str],
) -> Result<GuestAddr, RuntimeError> {
    let vtable = ctx.heap.alloc(ctx.arena, slot_names.len() * 4, true)?;
    for (idx, &name) in slot_names.iter().enumerate() {
        let thunk_va = ctx
            .register_dynamic_import(DINPUT8_COM_DLL, ImportSymbol::Name(name.to_owned()))
            .unwrap_or(0);
        ctx.arena.write_u32(vtable + (idx as u32) * 4, thunk_va)?;
    }
    let obj = ctx.heap.alloc(ctx.arena, 4, true)?;
    ctx.arena.write_u32(obj, vtable)?;
    Ok(obj)
}

fn direct_input_8_create(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ppv_out = peek_arg_u32(cpu, ctx.arena, 3)?;
    if ppv_out == 0 {
        return_stdcall(cpu, ctx.arena, 5, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    const DI8_SLOTS: [&str; DI8_VTABLE_SLOTS] = [
        "DI8_QueryInterface",
        "DI8_AddRef",
        "DI8_Release",
        "DI8_CreateDevice",
        "DI8_EnumDevices",
        "DI8_GetDeviceStatus",
        "Std0_3",
        "Std0_3",
        "DI8_FindDevice",
        "Std0_6",
        "Std0_5",
    ];
    let obj = alloc_com_object(ctx, &DI8_SLOTS)?;
    ctx.win32
        .dinput8
        .interfaces
        .insert(obj, DinputInterfaceObject { ref_count: 1 });
    ctx.arena.write_u32(ppv_out, obj)?;
    return_stdcall(cpu, ctx.arena, 5, DI_OK)?;
    Ok(())
}

fn di8_query_interface(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let riid = peek_arg_u32(cpu, ctx.arena, 1)?;
    let ppv = peek_arg_u32(cpu, ctx.arena, 2)?;
    if ppv == 0 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let matches = if riid != 0 {
        let g = WinGuid::read_from(ctx.arena, riid)?;
        g == IID_IUNKNOWN || g == IID_IDIRECTINPUT8A
    } else {
        false
    };
    if matches {
        if let Some(obj) = ctx.win32.dinput8.interfaces.get_mut(&this_ptr) {
            obj.ref_count = obj.ref_count.saturating_add(1);
        }
        ctx.arena.write_u32(ppv, this_ptr)?;
        return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
    } else {
        ctx.arena.write_u32(ppv, 0)?;
        return_stdcall(cpu, ctx.arena, 3, DIERR_NOINTERFACE)?;
    }
    Ok(())
}

fn di8_add_ref(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let count = ctx
        .win32
        .dinput8
        .interfaces
        .get_mut(&this_ptr)
        .map_or(1, |obj| {
            obj.ref_count = obj.ref_count.saturating_add(1);
            obj.ref_count
        });
    return_stdcall(cpu, ctx.arena, 1, count)?;
    Ok(())
}

fn di8_release(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let remaining = if let Some(obj) = ctx.win32.dinput8.interfaces.get_mut(&this_ptr) {
        obj.ref_count = obj.ref_count.saturating_sub(1);
        obj.ref_count
    } else {
        0
    };
    if remaining == 0 {
        ctx.win32.dinput8.interfaces.remove(&this_ptr);
    }
    return_stdcall(cpu, ctx.arena, 1, remaining)?;
    Ok(())
}

fn di8_create_device(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let rguid = peek_arg_u32(cpu, ctx.arena, 1)?;
    let pp_dev = peek_arg_u32(cpu, ctx.arena, 2)?;
    if rguid == 0 || pp_dev == 0 {
        return_stdcall(cpu, ctx.arena, 4, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let guid = WinGuid::read_from(ctx.arena, rguid)?;
    let kind = if guid == GUID_SYSKEYBOARD {
        DinputDeviceKind::Keyboard
    } else if guid == GUID_SYSMOUSE {
        DinputDeviceKind::Mouse
    } else {
        DinputDeviceKind::Gamepad
    };

    const DIDEV8_SLOTS: [&str; DIDEV8_VTABLE_SLOTS] = [
        "DIDev8_QueryInterface",
        "DIDev8_AddRef",
        "DIDev8_Release",
        "DIDev8_GetCapabilities",
        "DIDev8_EnumObjects",
        "DIDev8_GetProperty",
        "DIDev8_SetProperty",
        "DIDev8_Acquire",
        "DIDev8_Unacquire",
        "DIDev8_GetDeviceState",
        "DIDev8_GetDeviceData",
        "DIDev8_SetDataFormat",
        "Std0_2",
        "Std0_3",
        "DIDev8_GetObjectInfo",
        "DIDev8_GetDeviceInfo",
        "Std0_3",
        "Std0_4",
        "DIDev8_CreateEffect",
        "DIDev8_EnumEffects",
        "DIDev8_GetEffectInfo",
        "DIDev8_GetForceFeedbackState",
        "DIDev8_SendForceFeedbackCommand",
        "Std0_4",
        "Std0_2",
        "DIDev8_Poll",
        "Std0_5",
        "Std0_5",
        "Std0_5",
        "Std0_4",
        "Std0_4",
        "Std0_2",
    ];
    let dev_obj = alloc_com_object(ctx, &DIDEV8_SLOTS)?;
    ctx.win32
        .dinput8
        .devices
        .insert(dev_obj, DinputDeviceState::new(kind));
    ctx.arena.write_u32(pp_dev, dev_obj)?;
    return_stdcall(cpu, ctx.arena, 4, DI_OK)?;
    Ok(())
}

fn active_gamepad_name(ctx: &RuntimeContext<'_>) -> Option<String> {
    if let Some(name) = &ctx.win32.dinput8.override_gamepad_name {
        return Some(name.clone());
    }
    if let Some(snap) = &ctx.win32.dinput8.override_snapshot {
        return snap.gamepad.as_ref().map(|_| "SH2 Gamepad".to_owned());
    }
    let sdl = ctx.win32.window_mgr.sdl.as_ref()?;
    if sdl.has_controller() {
        Some(sdl.controller_name().unwrap_or_else(|| "SH2 Gamepad".to_owned()))
    } else {
        None
    }
}

fn dev_type_matches(filter_dev_type: u32, kind: DinputDeviceKind) -> bool {
    let class_or_type = filter_dev_type & 0xFF;
    match class_or_type {
        DI8DEVCLASS_ALL | DI8DEVCLASS_DEVICE | DI8DEVTYPE_DEVICE => true,
        DI8DEVCLASS_POINTER | DI8DEVTYPE_MOUSE => kind == DinputDeviceKind::Mouse,
        DI8DEVCLASS_KEYBOARD | DI8DEVTYPE_KEYBOARD => kind == DinputDeviceKind::Keyboard,
        DI8DEVCLASS_GAMECTRL
        | DI8DEVTYPE_JOYSTICK
        | DI8DEVTYPE_GAMEPAD
        | DI8DEVTYPE_DRIVING
        | DI8DEVTYPE_FLIGHT
        | DI8DEVTYPE_1STPERSON => kind == DinputDeviceKind::Gamepad,
        _ => false,
    }
}

fn di8_enum_devices(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let dw_dev_type = peek_arg_u32(cpu, ctx.arena, 1)?;
    let lp_callback = peek_arg_u32(cpu, ctx.arena, 2)?;
    let pv_ref = peek_arg_u32(cpu, ctx.arena, 3)?;
    if lp_callback == 0 {
        return_stdcall(cpu, ctx.arena, 5, DIERR_INVALIDPARAM)?;
        return Ok(());
    }

    let mut candidates = Vec::new();
    if dev_type_matches(dw_dev_type, DinputDeviceKind::Keyboard) {
        candidates.push((DinputDeviceKind::Keyboard, "Keyboard".to_owned()));
    }
    if dev_type_matches(dw_dev_type, DinputDeviceKind::Mouse) {
        candidates.push((DinputDeviceKind::Mouse, "Mouse".to_owned()));
    }
    if dev_type_matches(dw_dev_type, DinputDeviceKind::Gamepad) {
        if let Some(name) = active_gamepad_name(ctx) {
            candidates.push((DinputDeviceKind::Gamepad, name));
        }
    }

    if candidates.is_empty() {
        return_stdcall(cpu, ctx.arena, 5, DI_OK)?;
        return Ok(());
    }

    let inst_va = ctx
        .heap
        .alloc(ctx.arena, DIDEVICEINSTANCEA_SIZE_DX5 as usize, true)?;
    for (kind, name) in candidates {
        write_device_instance(ctx.arena, inst_va, DIDEVICEINSTANCEA_SIZE_DX5, kind, &name)?;
        let res = ctx.invoke_guest_callback(cpu, lp_callback, &[inst_va, pv_ref])?;
        if res == DIENUM_STOP {
            break;
        }
    }
    let _ = ctx.heap.free(ctx.arena, inst_va);
    return_stdcall(cpu, ctx.arena, 5, DI_OK)?;
    Ok(())
}

fn di8_get_device_status(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let rguid = peek_arg_u32(cpu, ctx.arena, 1)?;
    if rguid == 0 {
        return_stdcall(cpu, ctx.arena, 2, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let guid = WinGuid::read_from(ctx.arena, rguid)?;
    let attached = if guid == GUID_SYSKEYBOARD || guid == GUID_SYSMOUSE {
        true
    } else if guid == GUID_TOLUCA_GAMEPAD {
        active_gamepad_name(ctx).is_some()
    } else {
        false
    };
    let hr = if attached { DI_OK } else { DI_NOTATTACHED };
    return_stdcall(cpu, ctx.arena, 2, hr)?;
    Ok(())
}

fn di8_find_device(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let pguid_instance = peek_arg_u32(cpu, ctx.arena, 3)?;
    if pguid_instance == 0 {
        return_stdcall(cpu, ctx.arena, 4, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    if active_gamepad_name(ctx).is_some() {
        GUID_TOLUCA_GAMEPAD.write_to(ctx.arena, pguid_instance)?;
        return_stdcall(cpu, ctx.arena, 4, DI_OK)?;
    } else {
        return_stdcall(cpu, ctx.arena, 4, DIERR_OBJECTNOTFOUND)?;
    }
    Ok(())
}

fn didev8_query_interface(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let riid = peek_arg_u32(cpu, ctx.arena, 1)?;
    let ppv = peek_arg_u32(cpu, ctx.arena, 2)?;
    if ppv == 0 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let matches = if riid != 0 {
        let g = WinGuid::read_from(ctx.arena, riid)?;
        g == IID_IUNKNOWN || g == IID_IDIRECTINPUTDEVICE8A
    } else {
        false
    };
    if matches {
        if let Some(dev) = ctx.win32.dinput8.devices.get_mut(&this_ptr) {
            dev.ref_count = dev.ref_count.saturating_add(1);
        }
        ctx.arena.write_u32(ppv, this_ptr)?;
        return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
    } else {
        ctx.arena.write_u32(ppv, 0)?;
        return_stdcall(cpu, ctx.arena, 3, DIERR_NOINTERFACE)?;
    }
    Ok(())
}

fn didev8_add_ref(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let count = ctx
        .win32
        .dinput8
        .devices
        .get_mut(&this_ptr)
        .map_or(1, |dev| {
            dev.ref_count = dev.ref_count.saturating_add(1);
            dev.ref_count
        });
    return_stdcall(cpu, ctx.arena, 1, count)?;
    Ok(())
}

fn didev8_release(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let remaining = if let Some(dev) = ctx.win32.dinput8.devices.get_mut(&this_ptr) {
        dev.ref_count = dev.ref_count.saturating_sub(1);
        dev.ref_count
    } else {
        0
    };
    if remaining == 0 {
        ctx.win32.dinput8.devices.remove(&this_ptr);
    }
    return_stdcall(cpu, ctx.arena, 1, remaining)?;
    Ok(())
}

fn didev8_get_capabilities(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let caps_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if caps_ptr == 0 {
        return_stdcall(cpu, ctx.arena, 2, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let Some(kind) = ctx.win32.dinput8.devices.get(&this_ptr).map(|d| d.kind) else {
        return_stdcall(cpu, ctx.arena, 2, DIERR_GENERIC)?;
        return Ok(());
    };
    let dw_size = ctx.arena.read_u32(caps_ptr)?;
    if dw_size != DIDEVCAPS_SIZE_DX5 && dw_size != DIDEVCAPS_SIZE_DX3 {
        return_stdcall(cpu, ctx.arena, 2, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    ctx.arena.zero_range(caps_ptr + 4, (dw_size - 4) as usize)?;

    let (flags, dev_type, axes, buttons, povs) = match kind {
        DinputDeviceKind::Keyboard => (
            DIDC_ATTACHED | DIDC_EMULATED,
            DI8DEVTYPE_KEYBOARD | (DI8DEVTYPEKEYBOARD_PCENH << 8),
            0,
            256,
            0,
        ),
        DinputDeviceKind::Mouse => (
            DIDC_ATTACHED | DIDC_EMULATED,
            DI8DEVTYPE_MOUSE | (DI8DEVTYPEMOUSE_TRADITIONAL << 8),
            3,
            8,
            0,
        ),
        DinputDeviceKind::Gamepad => (
            DIDC_ATTACHED | DIDC_FORCEFEEDBACK,
            DI8DEVTYPE_GAMEPAD | (DI8DEVTYPEGAMEPAD_STANDARD << 8),
            SDL_GAMEPAD_AXIS_COUNT as u32,
            SDL_GAMEPAD_BUTTON_COUNT as u32,
            1,
        ),
    };

    ctx.arena.write_u32(caps_ptr + 4, flags)?;
    ctx.arena.write_u32(caps_ptr + 8, dev_type)?;
    ctx.arena.write_u32(caps_ptr + 12, axes)?;
    ctx.arena.write_u32(caps_ptr + 16, buttons)?;
    if dw_size >= DIDEVCAPS_SIZE_DX5 {
        ctx.arena.write_u32(caps_ptr + 20, povs)?;
        if kind == DinputDeviceKind::Gamepad {
            ctx.arena.write_u32(caps_ptr + 24, 1000)?;
            ctx.arena.write_u32(caps_ptr + 28, 1000)?;
            ctx.arena.write_u32(caps_ptr + 32, 1)?;
            ctx.arena.write_u32(caps_ptr + 36, 1)?;
            ctx.arena.write_u32(caps_ptr + 40, 1)?;
        }
    }
    return_stdcall(cpu, ctx.arena, 2, DI_OK)?;
    Ok(())
}

#[derive(Clone)]
struct DeviceObjectDesc {
    guid_type: WinGuid,
    dw_ofs: u32,
    dw_type: u32,
    dw_flags: u32,
    name: String,
}

fn describe_device_objects(kind: DinputDeviceKind) -> Vec<DeviceObjectDesc> {
    match kind {
        DinputDeviceKind::Gamepad => {
            let mut list = vec![
                DeviceObjectDesc {
                    guid_type: GUID_XAXIS,
                    dw_ofs: 0,
                    dw_type: DIDFT_ABSAXIS | DIDFT_FFACTUATOR,
                    dw_flags: DIDOI_FFACTUATOR | DIDOI_ASPECTPOSITION,
                    name: "X Axis".to_owned(),
                },
                DeviceObjectDesc {
                    guid_type: GUID_YAXIS,
                    dw_ofs: 4,
                    dw_type: DIDFT_ABSAXIS | DIDFT_FFACTUATOR | (1 << 8),
                    dw_flags: DIDOI_FFACTUATOR | DIDOI_ASPECTPOSITION,
                    name: "Y Axis".to_owned(),
                },
                DeviceObjectDesc {
                    guid_type: GUID_ZAXIS,
                    dw_ofs: 8,
                    dw_type: DIDFT_ABSAXIS | (2 << 8),
                    dw_flags: DIDOI_ASPECTPOSITION,
                    name: "Z Axis".to_owned(),
                },
                DeviceObjectDesc {
                    guid_type: GUID_RXAXIS,
                    dw_ofs: 12,
                    dw_type: DIDFT_ABSAXIS | (3 << 8),
                    dw_flags: DIDOI_ASPECTPOSITION,
                    name: "X Rotation".to_owned(),
                },
                DeviceObjectDesc {
                    guid_type: GUID_RYAXIS,
                    dw_ofs: 16,
                    dw_type: DIDFT_ABSAXIS | (4 << 8),
                    dw_flags: DIDOI_ASPECTPOSITION,
                    name: "Y Rotation".to_owned(),
                },
                DeviceObjectDesc {
                    guid_type: GUID_RZAXIS,
                    dw_ofs: 20,
                    dw_type: DIDFT_ABSAXIS | (5 << 8),
                    dw_flags: DIDOI_ASPECTPOSITION,
                    name: "Z Rotation".to_owned(),
                },
                DeviceObjectDesc {
                    guid_type: GUID_POV,
                    dw_ofs: 32,
                    dw_type: DIDFT_POV,
                    dw_flags: 0,
                    name: "Hat Switch".to_owned(),
                },
            ];
            for idx in 0..SDL_GAMEPAD_BUTTON_COUNT as u32 {
                list.push(DeviceObjectDesc {
                    guid_type: GUID_BUTTON,
                    dw_ofs: 48 + idx,
                    dw_type: DIDFT_PSHBUTTON | (idx << 8),
                    dw_flags: 0,
                    name: format!("Button {idx}"),
                });
            }
            list
        }
        DinputDeviceKind::Mouse => {
            let mut list = vec![
                DeviceObjectDesc {
                    guid_type: GUID_XAXIS,
                    dw_ofs: 0,
                    dw_type: DIDFT_RELAXIS,
                    dw_flags: 0,
                    name: "X-axis".to_owned(),
                },
                DeviceObjectDesc {
                    guid_type: GUID_YAXIS,
                    dw_ofs: 4,
                    dw_type: DIDFT_RELAXIS | (1 << 8),
                    dw_flags: 0,
                    name: "Y-axis".to_owned(),
                },
                DeviceObjectDesc {
                    guid_type: GUID_ZAXIS,
                    dw_ofs: 8,
                    dw_type: DIDFT_RELAXIS | (2 << 8),
                    dw_flags: 0,
                    name: "Wheel".to_owned(),
                },
            ];
            for idx in 0..8u32 {
                list.push(DeviceObjectDesc {
                    guid_type: GUID_BUTTON,
                    dw_ofs: 12 + idx,
                    dw_type: DIDFT_PSHBUTTON | (idx << 8),
                    dw_flags: 0,
                    name: format!("Button {idx}"),
                });
            }
            list
        }
        DinputDeviceKind::Keyboard => (1u32..=0xDD)
            .map(|dik| DeviceObjectDesc {
                guid_type: GUID_KEY,
                dw_ofs: dik,
                dw_type: DIDFT_PSHBUTTON | (dik << 8),
                dw_flags: 0,
                name: format!("Key 0x{dik:02X}"),
            })
            .collect(),
    }
}

fn object_matches_flags(dw_type: u32, dw_flags: u32) -> bool {
    if dw_flags == 0 {
        return true;
    }
    let type_mask = dw_flags & 0x0000_00FF;
    let type_ok = type_mask == 0 || (dw_type & type_mask) != 0;
    let ff_ok = (dw_flags & DIDFT_FFACTUATOR) == 0 || (dw_type & DIDFT_FFACTUATOR) != 0;
    type_ok && ff_ok
}

fn write_device_object_instance(
    arena: &mut GuestArena,
    va: GuestAddr,
    dw_size: u32,
    desc: &DeviceObjectDesc,
) -> Result<(), RuntimeError> {
    arena.zero_range(va + 4, (dw_size - 4) as usize)?;
    arena.write_u32(va, dw_size)?;
    desc.guid_type.write_to(arena, va + 4)?;
    arena.write_u32(va + 0x14, desc.dw_ofs)?;
    arena.write_u32(va + 0x18, desc.dw_type)?;
    arena.write_u32(va + 0x1C, desc.dw_flags)?;
    write_fixed_cstr(arena, va + 0x20, 260, &desc.name)?;
    if dw_size >= DIDEVICEOBJECTINSTANCEA_SIZE_DX5 && (desc.dw_type & DIDFT_FFACTUATOR) != 0 {
        arena.write_u32(va + 0x124, DI_FFNOMINALMAX)?;
        arena.write_u32(va + 0x128, 1000)?;
    }
    Ok(())
}

fn didev8_enum_objects(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let lp_callback = peek_arg_u32(cpu, ctx.arena, 1)?;
    let pv_ref = peek_arg_u32(cpu, ctx.arena, 2)?;
    let dw_flags = peek_arg_u32(cpu, ctx.arena, 3)?;
    if lp_callback == 0 {
        return_stdcall(cpu, ctx.arena, 4, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let Some(kind) = ctx.win32.dinput8.devices.get(&this_ptr).map(|d| d.kind) else {
        return_stdcall(cpu, ctx.arena, 4, DIERR_GENERIC)?;
        return Ok(());
    };
    let objects: Vec<DeviceObjectDesc> = describe_device_objects(kind)
        .into_iter()
        .filter(|obj| object_matches_flags(obj.dw_type, dw_flags))
        .collect();
    if objects.is_empty() {
        return_stdcall(cpu, ctx.arena, 4, DI_OK)?;
        return Ok(());
    }

    let obj_va = ctx
        .heap
        .alloc(ctx.arena, DIDEVICEOBJECTINSTANCEA_SIZE_DX5 as usize, true)?;
    for desc in &objects {
        write_device_object_instance(ctx.arena, obj_va, DIDEVICEOBJECTINSTANCEA_SIZE_DX5, desc)?;
        let res = ctx.invoke_guest_callback(cpu, lp_callback, &[obj_va, pv_ref])?;
        if res == DIENUM_STOP {
            break;
        }
    }
    let _ = ctx.heap.free(ctx.arena, obj_va);
    return_stdcall(cpu, ctx.arena, 4, DI_OK)?;
    Ok(())
}

fn didev8_get_object_info(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pdidoi = peek_arg_u32(cpu, ctx.arena, 1)?;
    let dw_obj = peek_arg_u32(cpu, ctx.arena, 2)?;
    let dw_how = peek_arg_u32(cpu, ctx.arena, 3)?;
    if pdidoi == 0 {
        return_stdcall(cpu, ctx.arena, 4, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let Some(kind) = ctx.win32.dinput8.devices.get(&this_ptr).map(|d| d.kind) else {
        return_stdcall(cpu, ctx.arena, 4, DIERR_GENERIC)?;
        return Ok(());
    };
    let dw_size = ctx.arena.read_u32(pdidoi)?;
    if dw_size != DIDEVICEOBJECTINSTANCEA_SIZE_DX5 && dw_size != DIDEVICEOBJECTINSTANCEA_SIZE_DX3 {
        return_stdcall(cpu, ctx.arena, 4, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let found = describe_device_objects(kind).into_iter().find(|desc| match dw_how {
        DIPH_BYOFFSET => desc.dw_ofs == dw_obj,
        DIPH_BYID => desc.dw_type == dw_obj || ((desc.dw_type >> 8) & 0xFFFF) == ((dw_obj >> 8) & 0xFFFF),
        _ => false,
    });
    let Some(desc) = found else {
        return_stdcall(cpu, ctx.arena, 4, DIERR_OBJECTNOTFOUND)?;
        return Ok(());
    };
    write_device_object_instance(ctx.arena, pdidoi, dw_size, &desc)?;
    return_stdcall(cpu, ctx.arena, 4, DI_OK)?;
    Ok(())
}

fn axis_index_from_prop_target(dw_how: u32, dw_obj: u32) -> Option<usize> {
    match dw_how {
        DIPH_BYOFFSET => match dw_obj {
            0 => Some(0),
            4 => Some(1),
            8 => Some(2),
            12 => Some(3),
            16 => Some(4),
            20 => Some(5),
            _ => None,
        },
        DIPH_BYID => {
            let inst = ((dw_obj >> 8) & 0xFFFF) as usize;
            (inst < SDL_GAMEPAD_AXIS_COUNT).then_some(inst)
        }
        _ => None,
    }
}

fn didev8_get_property(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let rguid_prop = peek_arg_u32(cpu, ctx.arena, 1)?;
    let pdiph = peek_arg_u32(cpu, ctx.arena, 2)?;
    if pdiph == 0 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let Some(dev) = ctx.win32.dinput8.devices.get(&this_ptr) else {
        return_stdcall(cpu, ctx.arena, 3, DIERR_GENERIC)?;
        return Ok(());
    };
    let dw_size = ctx.arena.read_u32(pdiph)?;
    let dw_header_size = ctx.arena.read_u32(pdiph + 4)?;
    let dw_obj = ctx.arena.read_u32(pdiph + 8)?;
    let dw_how = ctx.arena.read_u32(pdiph + 12)?;
    if dw_header_size != DIPROPHEADER_SIZE {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }

    match rguid_prop {
        DIPROP_BUFFERSIZE | DIPROP_AXISMODE | DIPROP_GRANULARITY | DIPROP_DEADZONE
        | DIPROP_SATURATION | DIPROP_FFGAIN | DIPROP_AUTOCENTER => {
            if dw_size < DIPROPDWORD_SIZE {
                return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
                return Ok(());
            }
            let val = match rguid_prop {
                DIPROP_BUFFERSIZE => dev.buffer_size,
                DIPROP_AXISMODE => dev.axis_mode,
                DIPROP_GRANULARITY | DIPROP_AUTOCENTER => 1,
                DIPROP_DEADZONE => dev.deadzone,
                DIPROP_SATURATION => dev.saturation,
                DIPROP_FFGAIN => dev.ff_gain,
                _ => 0,
            };
            ctx.arena.write_u32(pdiph + 16, val)?;
            return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
        }
        DIPROP_RANGE => {
            if dw_size < DIPROPRANGE_SIZE {
                return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
                return Ok(());
            }
            let (l_min, l_max) = if dw_how == DIPH_DEVICE {
                dev.default_axis_range
            } else if let Some(idx) = axis_index_from_prop_target(dw_how, dw_obj) {
                dev.axis_range_for(idx)
            } else {
                dev.default_axis_range
            };
            ctx.arena.write_u32(pdiph + 16, l_min as u32)?;
            ctx.arena.write_u32(pdiph + 20, l_max as u32)?;
            return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
        }
        _ => {
            return_stdcall(cpu, ctx.arena, 3, DIERR_UNSUPPORTED)?;
        }
    }
    Ok(())
}

fn didev8_set_property(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let rguid_prop = peek_arg_u32(cpu, ctx.arena, 1)?;
    let pdiph = peek_arg_u32(cpu, ctx.arena, 2)?;
    if pdiph == 0 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let dw_size = ctx.arena.read_u32(pdiph)?;
    let dw_header_size = ctx.arena.read_u32(pdiph + 4)?;
    let dw_obj = ctx.arena.read_u32(pdiph + 8)?;
    let dw_how = ctx.arena.read_u32(pdiph + 12)?;
    if dw_header_size != DIPROPHEADER_SIZE {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let Some(dev) = ctx.win32.dinput8.devices.get_mut(&this_ptr) else {
        return_stdcall(cpu, ctx.arena, 3, DIERR_GENERIC)?;
        return Ok(());
    };

    match rguid_prop {
        DIPROP_BUFFERSIZE | DIPROP_AXISMODE | DIPROP_DEADZONE | DIPROP_SATURATION
        | DIPROP_FFGAIN | DIPROP_AUTOCENTER => {
            if dw_size < DIPROPDWORD_SIZE {
                return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
                return Ok(());
            }
            let data = ctx.arena.read_u32(pdiph + 16)?;
            match rguid_prop {
                DIPROP_BUFFERSIZE => dev.buffer_size = data.min(MAX_DEVICE_BUFFER_SIZE),
                DIPROP_AXISMODE => dev.axis_mode = data,
                DIPROP_DEADZONE => dev.deadzone = data.min(DI_FFNOMINALMAX),
                DIPROP_SATURATION => dev.saturation = data.min(DI_FFNOMINALMAX),
                DIPROP_FFGAIN => dev.ff_gain = data.min(DI_FFNOMINALMAX),
                DIPROP_AUTOCENTER => {
                    return_stdcall(cpu, ctx.arena, 3, DI_PROPNOEFFECT)?;
                    return Ok(());
                }
                _ => {}
            }
            return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
        }
        DIPROP_RANGE => {
            if dw_size < DIPROPRANGE_SIZE {
                return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
                return Ok(());
            }
            let l_min = ctx.arena.read_u32(pdiph + 16)? as i32;
            let l_max = ctx.arena.read_u32(pdiph + 20)? as i32;
            if l_min > l_max {
                return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
                return Ok(());
            }
            if dw_how == DIPH_DEVICE {
                dev.default_axis_range = (l_min, l_max);
                dev.axis_ranges = [None; SDL_GAMEPAD_AXIS_COUNT];
            } else if let Some(idx) = axis_index_from_prop_target(dw_how, dw_obj) {
                dev.axis_ranges[idx] = Some((l_min, l_max));
            } else {
                dev.default_axis_range = (l_min, l_max);
            }
            return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
        }
        _ => {
            return_stdcall(cpu, ctx.arena, 3, DI_PROPNOEFFECT)?;
        }
    }
    Ok(())
}

fn didev8_acquire(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    if let Some(dev) = ctx.win32.dinput8.devices.get_mut(&this_ptr) {
        dev.acquired = true;
        return_stdcall(cpu, ctx.arena, 1, DI_OK)?;
    } else {
        return_stdcall(cpu, ctx.arena, 1, DIERR_GENERIC)?;
    }
    Ok(())
}

fn didev8_unacquire(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    if let Some(dev) = ctx.win32.dinput8.devices.get_mut(&this_ptr) {
        dev.acquired = false;
        return_stdcall(cpu, ctx.arena, 1, DI_OK)?;
    } else {
        return_stdcall(cpu, ctx.arena, 1, DIERR_GENERIC)?;
    }
    Ok(())
}

fn didev8_set_data_format(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let lpdf = peek_arg_u32(cpu, ctx.arena, 1)?;
    if lpdf == 0 {
        return_stdcall(cpu, ctx.arena, 2, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let dw_data_size = ctx.arena.read_u32(lpdf + 12)?;
    if let Some(dev) = ctx.win32.dinput8.devices.get_mut(&this_ptr) {
        if dw_data_size > 0 {
            dev.data_format_size = dw_data_size;
        }
        return_stdcall(cpu, ctx.arena, 2, DI_OK)?;
    } else {
        return_stdcall(cpu, ctx.arena, 2, DIERR_GENERIC)?;
    }
    Ok(())
}

fn didev8_get_device_info(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pdidi = peek_arg_u32(cpu, ctx.arena, 1)?;
    if pdidi == 0 {
        return_stdcall(cpu, ctx.arena, 2, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let dw_size = ctx.arena.read_u32(pdidi)?;
    if dw_size != DIDEVICEINSTANCEA_SIZE_DX5 && dw_size != DIDEVICEINSTANCEA_SIZE_DX3 {
        return_stdcall(cpu, ctx.arena, 2, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let Some(kind) = ctx.win32.dinput8.devices.get(&this_ptr).map(|d| d.kind) else {
        return_stdcall(cpu, ctx.arena, 2, DIERR_GENERIC)?;
        return Ok(());
    };
    let name = match kind {
        DinputDeviceKind::Keyboard => "Keyboard".to_owned(),
        DinputDeviceKind::Mouse => "Mouse".to_owned(),
        DinputDeviceKind::Gamepad => active_gamepad_name(ctx).unwrap_or_else(|| "SH2 Gamepad".to_owned()),
    };
    write_device_instance(ctx.arena, pdidi, dw_size, kind, &name)?;
    return_stdcall(cpu, ctx.arena, 2, DI_OK)?;
    Ok(())
}

fn sample_input_snapshot(ctx: &mut RuntimeContext<'_>) -> SdlInputSnapshot {
    if let Some(snap) = ctx.win32.dinput8.override_snapshot.clone() {
        return snap;
    }
    if let Some(sdl) = ctx.win32.window_mgr.sdl.as_mut() {
        return sdl.poll_input_snapshot();
    }
    SdlInputSnapshot::default()
}

pub fn active_scripted_diks_at_frame(
    script_keys: &[ScriptedKeyEvent],
    frame: u64,
) -> [bool; 256] {
    let mut keys = [false; 256];
    for ev in script_keys {
        if frame >= ev.start_frame && frame < ev.start_frame.saturating_add(ev.duration_frames) {
            keys[ev.dik_code as usize] = true;
        }
    }
    keys
}

pub fn sync_scripted_keys_for_frame(ctx: &mut RuntimeContext<'_>) {
    if ctx.win32.dinput8.script_keys.is_empty() {
        return;
    }
    let frame = ctx.win32.d3d8_present_calls;
    if ctx.win32.dinput8.last_script_frame == Some(frame) {
        return;
    }
    let next = active_scripted_diks_at_frame(&ctx.win32.dinput8.script_keys, frame);
    let active_hwnd = ctx.win32.window_mgr.active_hwnd.unwrap_or(0);
    let time = ctx.host_start_instant.elapsed().as_millis() as u32;

    for (dik_idx, (&was_down, &is_down)) in ctx
        .win32
        .dinput8
        .active_script_diks
        .iter()
        .zip(next.iter())
        .enumerate()
    {
        if was_down == is_down {
            continue;
        }
        if let Some(vk) = dik_to_vk(dik_idx as u8) {
            ctx.win32.window_mgr.key_states[vk as usize] = is_down;
            ctx.win32.window_mgr.msg_queue.push_back(Win32Msg {
                hwnd: active_hwnd,
                message: if is_down { WM_KEYDOWN } else { WM_KEYUP },
                wparam: vk as u32,
                lparam: if is_down { 1 } else { 0xC000_0001 },
                time,
                pt_x: 0,
                pt_y: 0,
            });
        }
    }
    ctx.win32.dinput8.active_script_diks = next;
    ctx.win32.dinput8.last_script_frame = Some(frame);
}

fn compose_keyboard_state(snapshot: &SdlInputSnapshot, script_keys: &[ScriptedKeyEvent], frame: u64) -> [bool; 256] {
    let mut keys = [false; 256];
    for (sc, &down) in snapshot.key_scancodes.iter().enumerate() {
        if down {
            if let Some(dik) = sdl_scancode_to_dik(sc as i32) {
                keys[dik as usize] = true;
            }
        }
    }
    let scripted = active_scripted_diks_at_frame(script_keys, frame);
    for (dst, &src) in keys.iter_mut().zip(scripted.iter()) {
        *dst |= src;
    }
    keys
}

struct RefreshedDeviceState {
    kind: DinputDeviceKind,
    kbd: [bool; 256],
    mouse_delta: (i32, i32, i32),
    mouse_btns: [bool; 8],
    gamepad: Option<GamepadSnapshot>,
}

fn refresh_device_state(
    ctx: &mut RuntimeContext<'_>,
    dev_va: GuestAddr,
) -> Result<RefreshedDeviceState, u32> {
    let Some(kind) = ctx.win32.dinput8.devices.get(&dev_va).map(|d| d.kind) else {
        return Err(DIERR_GENERIC);
    };
    let acquired = ctx
        .win32
        .dinput8
        .devices
        .get(&dev_va)
        .is_some_and(|d| d.acquired);
    if !acquired {
        return Err(DIERR_NOTACQUIRED);
    }

    let snapshot = sample_input_snapshot(ctx);
    let frame = ctx.win32.d3d8_present_calls;
    let time_ms = ctx.host_start_instant.elapsed().as_millis() as u32;
    let kbd = compose_keyboard_state(&snapshot, &ctx.win32.dinput8.script_keys, frame);
    let mouse_btns = sdl_mouse_mask_to_di_buttons(snapshot.mouse_buttons);
    let mouse_dz = snapshot.mouse_dz.saturating_mul(WHEEL_DELTA);

    let mut seq = ctx.win32.dinput8.next_sequence;
    let mut alloc_seq = || {
        let s = seq;
        seq = seq.wrapping_add(1).max(1);
        s
    };

    if let Some(dev) = ctx.win32.dinput8.devices.get_mut(&dev_va) {
        match kind {
            DinputDeviceKind::Keyboard => {
                for (dik, &down) in kbd.iter().enumerate().skip(1) {
                    if down != dev.prev_keys[dik] {
                        let data = if down { 0x80 } else { 0x00 };
                        dev.push_buffered(BufferedObjectData {
                            dw_ofs: dik as u32,
                            dw_data: data,
                            dw_time_stamp: time_ms,
                            dw_sequence: alloc_seq(),
                        });
                    }
                }
                dev.prev_keys = kbd;
            }
            DinputDeviceKind::Mouse => {
                dev.pending_mouse_delta.0 = dev.pending_mouse_delta.0.saturating_add(snapshot.mouse_dx);
                dev.pending_mouse_delta.1 = dev.pending_mouse_delta.1.saturating_add(snapshot.mouse_dy);
                dev.pending_mouse_delta.2 = dev.pending_mouse_delta.2.saturating_add(mouse_dz);
                if snapshot.mouse_dx != 0 {
                    dev.push_buffered(BufferedObjectData {
                        dw_ofs: 0,
                        dw_data: snapshot.mouse_dx as u32,
                        dw_time_stamp: time_ms,
                        dw_sequence: alloc_seq(),
                    });
                }
                if snapshot.mouse_dy != 0 {
                    dev.push_buffered(BufferedObjectData {
                        dw_ofs: 4,
                        dw_data: snapshot.mouse_dy as u32,
                        dw_time_stamp: time_ms,
                        dw_sequence: alloc_seq(),
                    });
                }
                if mouse_dz != 0 {
                    dev.push_buffered(BufferedObjectData {
                        dw_ofs: 8,
                        dw_data: mouse_dz as u32,
                        dw_time_stamp: time_ms,
                        dw_sequence: alloc_seq(),
                    });
                }
                for (idx, &down) in mouse_btns.iter().enumerate() {
                    if down != dev.prev_mouse_buttons[idx] {
                        dev.push_buffered(BufferedObjectData {
                            dw_ofs: 12 + idx as u32,
                            dw_data: if down { 0x80 } else { 0x00 },
                            dw_time_stamp: time_ms,
                            dw_sequence: alloc_seq(),
                        });
                        dev.prev_mouse_buttons[idx] = down;
                    }
                }
            }
            DinputDeviceKind::Gamepad => {
                let gp = snapshot.gamepad.unwrap_or_default();
                let raw_axes = gamepad_axes_in_di_order(&gp);
                for (idx, &raw) in raw_axes.iter().enumerate() {
                    let scaled = scale_axis(raw, dev.axis_range_for(idx), dev.deadzone);
                    if scaled != dev.prev_axes[idx] {
                        dev.push_buffered(BufferedObjectData {
                            dw_ofs: (idx as u32) * 4,
                            dw_data: scaled as u32,
                            dw_time_stamp: time_ms,
                            dw_sequence: alloc_seq(),
                        });
                        dev.prev_axes[idx] = scaled;
                    }
                }
                if gp.dpad_hat != dev.prev_pov {
                    dev.push_buffered(BufferedObjectData {
                        dw_ofs: 32,
                        dw_data: gp.dpad_hat,
                        dw_time_stamp: time_ms,
                        dw_sequence: alloc_seq(),
                    });
                    dev.prev_pov = gp.dpad_hat;
                }
                for (idx, &down) in gp.buttons.iter().enumerate() {
                    if down != dev.prev_gamepad_buttons[idx] {
                        dev.push_buffered(BufferedObjectData {
                            dw_ofs: 48 + idx as u32,
                            dw_data: if down { 0x80 } else { 0x00 },
                            dw_time_stamp: time_ms,
                            dw_sequence: alloc_seq(),
                        });
                        dev.prev_gamepad_buttons[idx] = down;
                    }
                }
            }
        }
    }

    ctx.win32.dinput8.next_sequence = seq;
    let mouse_delta = ctx
        .win32
        .dinput8
        .devices
        .get(&dev_va)
        .map_or((0, 0, 0), |d| d.pending_mouse_delta);
    Ok(RefreshedDeviceState {
        kind,
        kbd,
        mouse_delta,
        mouse_btns,
        gamepad: snapshot.gamepad,
    })
}

fn didev8_poll(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let hr = match refresh_device_state(ctx, this_ptr) {
        Ok(_) => DI_OK,
        Err(err_hr) => err_hr,
    };
    return_stdcall(cpu, ctx.arena, 1, hr)?;
    Ok(())
}

fn didev8_get_device_state(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let cb_data = peek_arg_u32(cpu, ctx.arena, 1)?;
    let lpv_data = peek_arg_u32(cpu, ctx.arena, 2)?;
    if lpv_data == 0 || cb_data == 0 || cb_data > 4096 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }

    let state = match refresh_device_state(ctx, this_ptr) {
        Ok(state) => state,
        Err(err_hr) => {
            return_stdcall(cpu, ctx.arena, 3, err_hr)?;
            return Ok(());
        }
    };

    let len = cb_data as usize;
    match state.kind {
        DinputDeviceKind::Keyboard => {
            let packed = pack_keyboard_state(&state.kbd);
            let write_len = len.min(packed.len());
            ctx.arena.zero_range(lpv_data, len)?;
            ctx.arena.write_bytes(lpv_data, &packed[..write_len])?;
        }
        DinputDeviceKind::Mouse => {
            let packed = pack_mouse_state(
                state.mouse_delta.0,
                state.mouse_delta.1,
                state.mouse_delta.2,
                &state.mouse_btns,
                len,
            );
            ctx.arena.write_bytes(lpv_data, &packed)?;
            if let Some(dev) = ctx.win32.dinput8.devices.get_mut(&this_ptr) {
                dev.pending_mouse_delta = (0, 0, 0);
            }
        }
        DinputDeviceKind::Gamepad => {
            let Some(dev) = ctx.win32.dinput8.devices.get(&this_ptr) else {
                return_stdcall(cpu, ctx.arena, 3, DIERR_GENERIC)?;
                return Ok(());
            };
            let gp = state.gamepad.unwrap_or_default();
            let packed = pack_gamepad_state(
                &gp,
                dev.default_axis_range,
                &dev.axis_ranges,
                dev.deadzone,
                len,
            );
            ctx.arena.write_bytes(lpv_data, &packed)?;
        }
    }

    return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
    Ok(())
}

fn didev8_get_device_data(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let cb_object_data = peek_arg_u32(cpu, ctx.arena, 1)?;
    let rgdod = peek_arg_u32(cpu, ctx.arena, 2)?;
    let pdw_inout = peek_arg_u32(cpu, ctx.arena, 3)?;
    let dw_flags = peek_arg_u32(cpu, ctx.arena, 4)?;
    if pdw_inout == 0 || (cb_object_data != 16 && cb_object_data != 20) {
        return_stdcall(cpu, ctx.arena, 5, DIERR_INVALIDPARAM)?;
        return Ok(());
    }

    if let Err(err_hr) = refresh_device_state(ctx, this_ptr) {
        ctx.arena.write_u32(pdw_inout, 0)?;
        return_stdcall(cpu, ctx.arena, 5, err_hr)?;
        return Ok(());
    }

    let requested = ctx.arena.read_u32(pdw_inout)?;
    let peek = (dw_flags & DIGDD_PEEK) != 0;
    let Some(dev) = ctx.win32.dinput8.devices.get_mut(&this_ptr) else {
        ctx.arena.write_u32(pdw_inout, 0)?;
        return_stdcall(cpu, ctx.arena, 5, DIERR_GENERIC)?;
        return Ok(());
    };

    if rgdod == 0 {
        if requested == 0xFFFF_FFFF && !peek {
            dev.event_queue.clear();
            ctx.arena.write_u32(pdw_inout, 0)?;
        } else {
            ctx.arena.write_u32(pdw_inout, dev.event_queue.len() as u32)?;
        }
        return_stdcall(cpu, ctx.arena, 5, DI_OK)?;
        return Ok(());
    }

    let count = (requested as usize).min(dev.event_queue.len());
    let mut items = Vec::with_capacity(count);
    if peek {
        items.extend(dev.event_queue.iter().take(count).copied());
    } else {
        for _ in 0..count {
            if let Some(ev) = dev.event_queue.pop_front() {
                items.push(ev);
            }
        }
    }

    for (idx, ev) in items.iter().enumerate() {
        let dst = rgdod + (idx as u32) * cb_object_data;
        ctx.arena.write_u32(dst, ev.dw_ofs)?;
        ctx.arena.write_u32(dst + 4, ev.dw_data)?;
        ctx.arena.write_u32(dst + 8, ev.dw_time_stamp)?;
        ctx.arena.write_u32(dst + 12, ev.dw_sequence)?;
        if cb_object_data >= 20 {
            ctx.arena.write_u32(dst + 16, 0)?;
        }
    }
    ctx.arena.write_u32(pdw_inout, items.len() as u32)?;
    return_stdcall(cpu, ctx.arena, 5, DI_OK)?;
    Ok(())
}

fn didev8_create_effect(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let rguid = peek_arg_u32(cpu, ctx.arena, 1)?;
    let lpeff = peek_arg_u32(cpu, ctx.arena, 2)?;
    let ppdeff = peek_arg_u32(cpu, ctx.arena, 3)?;
    if ppdeff == 0 {
        return_stdcall(cpu, ctx.arena, 5, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let guid = if rguid != 0 {
        WinGuid::read_from(ctx.arena, rguid)?
    } else {
        GUID_CONSTANTFORCE
    };

    const DIEFF_SLOTS: [&str; DIEFF_VTABLE_SLOTS] = [
        "DIEff_QueryInterface",
        "DIEff_AddRef",
        "DIEff_Release",
        "Std0_4",
        "DIEff_GetEffectGuid",
        "DIEff_GetParameters",
        "DIEff_SetParameters",
        "DIEff_Start",
        "DIEff_Stop",
        "DIEff_GetEffectStatus",
        "Std0_1",
        "Std0_1",
        "Std0_2",
        "Std0_1",
        "Std0_1",
        "Std0_1",
    ];
    let eff_va = alloc_com_object(ctx, &DIEFF_SLOTS)?;
    let mut eff = DinputEffectState::new(this_ptr, guid);
    if lpeff != 0 {
        if let Err(hr) = parse_dieffect_params(ctx.arena, lpeff, &mut eff) {
            ctx.arena.write_u32(ppdeff, 0)?;
            return_stdcall(cpu, ctx.arena, 5, hr)?;
            return Ok(());
        }
    }
    ctx.win32.dinput8.effects.insert(eff_va, eff);
    ctx.arena.write_u32(ppdeff, eff_va)?;
    return_stdcall(cpu, ctx.arena, 5, DI_OK)?;
    Ok(())
}

fn didev8_enum_effects(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let lp_callback = peek_arg_u32(cpu, ctx.arena, 1)?;
    let pv_ref = peek_arg_u32(cpu, ctx.arena, 2)?;
    if lp_callback == 0 {
        return_stdcall(cpu, ctx.arena, 4, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let info_va = ctx.heap.alloc(ctx.arena, DIEFFECTINFOA_SIZE as usize, true)?;
    ctx.arena.write_u32(info_va, DIEFFECTINFOA_SIZE)?;
    GUID_CONSTANTFORCE.write_to(ctx.arena, info_va + 4)?;
    ctx.arena.write_u32(info_va + 20, 1)?; // DIEFT_CONSTANTFORCE
    ctx.arena.write_u32(info_va + 24, 0)?;
    ctx.arena.write_u32(info_va + 28, 0)?;
    write_fixed_cstr(ctx.arena, info_va + 32, 260, "Constant Force")?;
    let _ = ctx.invoke_guest_callback(cpu, lp_callback, &[info_va, pv_ref])?;
    let _ = ctx.heap.free(ctx.arena, info_va);
    return_stdcall(cpu, ctx.arena, 4, DI_OK)?;
    Ok(())
}

fn didev8_get_effect_info(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let pdei = peek_arg_u32(cpu, ctx.arena, 1)?;
    let rguid = peek_arg_u32(cpu, ctx.arena, 2)?;
    if pdei == 0 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let dw_size = ctx.arena.read_u32(pdei)?;
    if dw_size < 32 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    ctx.arena.zero_range(pdei + 4, (dw_size - 4) as usize)?;
    let guid = if rguid != 0 {
        WinGuid::read_from(ctx.arena, rguid)?
    } else {
        GUID_CONSTANTFORCE
    };
    guid.write_to(ctx.arena, pdei + 4)?;
    ctx.arena.write_u32(pdei + 20, 1)?;
    if dw_size >= DIEFFECTINFOA_SIZE {
        write_fixed_cstr(ctx.arena, pdei + 32, 260, "Constant Force")?;
    }
    return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
    Ok(())
}

fn didev8_get_force_feedback_state(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pdw_out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if pdw_out == 0 {
        return_stdcall(cpu, ctx.arena, 2, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let any_playing = ctx
        .win32
        .dinput8
        .effects
        .values()
        .any(|e| e.device_va == this_ptr && e.started);
    let flags = if any_playing {
        DIGFFS_EMPTY
    } else {
        DIGFFS_EMPTY | DIGFFS_STOPPED
    };
    ctx.arena.write_u32(pdw_out, flags)?;
    return_stdcall(cpu, ctx.arena, 2, DI_OK)?;
    Ok(())
}

fn didev8_send_force_feedback_command(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let dw_flags = peek_arg_u32(cpu, ctx.arena, 1)?;
    if (dw_flags & (DISFFC_RESET | DISFFC_STOPALL)) != 0 {
        for eff in ctx.win32.dinput8.effects.values_mut() {
            if eff.device_va == this_ptr {
                eff.started = false;
            }
        }
        dispatch_rumble(ctx, 0, 0, 0);
    }
    return_stdcall(cpu, ctx.arena, 2, DI_OK)?;
    Ok(())
}

fn dieff_query_interface(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let riid = peek_arg_u32(cpu, ctx.arena, 1)?;
    let ppv = peek_arg_u32(cpu, ctx.arena, 2)?;
    if ppv == 0 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let matches = if riid != 0 {
        let g = WinGuid::read_from(ctx.arena, riid)?;
        g == IID_IUNKNOWN || g == IID_IDIRECTINPUTEFFECT
    } else {
        false
    };
    if matches {
        if let Some(eff) = ctx.win32.dinput8.effects.get_mut(&this_ptr) {
            eff.ref_count = eff.ref_count.saturating_add(1);
        }
        ctx.arena.write_u32(ppv, this_ptr)?;
        return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
    } else {
        ctx.arena.write_u32(ppv, 0)?;
        return_stdcall(cpu, ctx.arena, 3, DIERR_NOINTERFACE)?;
    }
    Ok(())
}

fn dieff_add_ref(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let count = ctx
        .win32
        .dinput8
        .effects
        .get_mut(&this_ptr)
        .map_or(1, |eff| {
            eff.ref_count = eff.ref_count.saturating_add(1);
            eff.ref_count
        });
    return_stdcall(cpu, ctx.arena, 1, count)?;
    Ok(())
}

fn dieff_release(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let (remaining, was_started) = if let Some(eff) = ctx.win32.dinput8.effects.get_mut(&this_ptr) {
        eff.ref_count = eff.ref_count.saturating_sub(1);
        (eff.ref_count, eff.started)
    } else {
        (0, false)
    };
    if remaining == 0 {
        ctx.win32.dinput8.effects.remove(&this_ptr);
        if was_started {
            dispatch_rumble(ctx, 0, 0, 0);
        }
    }
    return_stdcall(cpu, ctx.arena, 1, remaining)?;
    Ok(())
}

fn dieff_get_effect_guid(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pguid = peek_arg_u32(cpu, ctx.arena, 1)?;
    if pguid == 0 {
        return_stdcall(cpu, ctx.arena, 2, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let guid = ctx
        .win32
        .dinput8
        .effects
        .get(&this_ptr)
        .map_or(GUID_CONSTANTFORCE, |e| e.guid);
    guid.write_to(ctx.arena, pguid)?;
    return_stdcall(cpu, ctx.arena, 2, DI_OK)?;
    Ok(())
}

fn dieff_get_parameters(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let peff = peek_arg_u32(cpu, ctx.arena, 1)?;
    if peff == 0 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let dw_size = ctx.arena.read_u32(peff)?;
    if dw_size < DIEFFECT_SIZE_DX5 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let Some(eff) = ctx.win32.dinput8.effects.get(&this_ptr) else {
        return_stdcall(cpu, ctx.arena, 3, DIERR_GENERIC)?;
        return Ok(());
    };
    ctx.arena.write_u32(peff + 8, eff.duration_us)?;
    ctx.arena.write_u32(peff + 16, eff.gain)?;
    if dw_size >= DIEFFECT_SIZE_DX6 {
        ctx.arena.write_u32(peff + 52, 0)?;
    }
    return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
    Ok(())
}

fn parse_dieffect_params(
    arena: &GuestArena,
    peff: GuestAddr,
    eff: &mut DinputEffectState,
) -> Result<(), u32> {
    let Ok(dw_size) = arena.read_u32(peff) else {
        return Err(DIERR_INVALIDPARAM);
    };
    if dw_size < DIEFFECT_SIZE_DX5 {
        return Err(DIERR_INVALIDPARAM);
    }
    let duration_us = arena.read_u32(peff + 8).map_err(|_| DIERR_INVALIDPARAM)?;
    let gain = arena.read_u32(peff + 16).map_err(|_| DIERR_INVALIDPARAM)?;
    let cb_specific = arena.read_u32(peff + 44).map_err(|_| DIERR_INVALIDPARAM)?;
    let lpv_specific = arena.read_u32(peff + 48).map_err(|_| DIERR_INVALIDPARAM)?;

    eff.duration_us = duration_us;
    eff.gain = gain.min(DI_FFNOMINALMAX);
    if lpv_specific != 0 && cb_specific >= 4 {
        let mag = match cb_specific {
            4 => (arena.read_u32(lpv_specific).unwrap_or(0) as i32)
                .unsigned_abs()
                .min(DI_FFNOMINALMAX),
            8 => {
                let s = (arena.read_u32(lpv_specific).unwrap_or(0) as i32).unsigned_abs();
                let e = (arena.read_u32(lpv_specific + 4).unwrap_or(0) as i32).unsigned_abs();
                s.max(e).min(DI_FFNOMINALMAX)
            }
            _ => arena
                .read_u32(lpv_specific)
                .unwrap_or(DI_FFNOMINALMAX)
                .min(DI_FFNOMINALMAX),
        };
        eff.magnitude = mag;
    }
    Ok(())
}

fn compute_rumble_for_effect(
    eff: &DinputEffectState,
    dev_ff_gain: u32,
    iterations: u32,
) -> (u16, u16, u32) {
    if iterations == 0 {
        return (0, 0, 0);
    }
    let level_10k = (u64::from(eff.magnitude.min(DI_FFNOMINALMAX))
        * u64::from(eff.gain.min(DI_FFNOMINALMAX))
        * u64::from(dev_ff_gain.min(DI_FFNOMINALMAX)))
        / 100_000_000;
    let strength = ((level_10k * u64::from(u16::MAX)) / u64::from(DI_FFNOMINALMAX)) as u16;
    let duration_ms = if eff.duration_us == 0xFFFF_FFFF || iterations == 0xFFFF_FFFF {
        MAX_RUMBLE_MS
    } else {
        (eff.duration_us / 1000)
            .max(1)
            .saturating_mul(iterations)
            .min(MAX_RUMBLE_MS)
    };
    (strength, strength, duration_ms)
}

fn dispatch_rumble(ctx: &mut RuntimeContext<'_>, low: u16, high: u16, duration_ms: u32) {
    ctx.win32.dinput8.last_rumble_command = Some((low, high, duration_ms));
    if let Some(sdl) = ctx.win32.window_mgr.sdl.as_mut() {
        sdl.set_controller_rumble(low, high, duration_ms);
    }
}

fn dieff_set_parameters(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let peff = peek_arg_u32(cpu, ctx.arena, 1)?;
    let dw_flags = peek_arg_u32(cpu, ctx.arena, 2)?;
    if peff == 0 {
        return_stdcall(cpu, ctx.arena, 3, DIERR_INVALIDPARAM)?;
        return Ok(());
    }

    let Some(mut eff) = ctx.win32.dinput8.effects.get(&this_ptr).cloned() else {
        return_stdcall(cpu, ctx.arena, 3, DIERR_GENERIC)?;
        return Ok(());
    };
    if let Err(hr) = parse_dieffect_params(ctx.arena, peff, &mut eff) {
        return_stdcall(cpu, ctx.arena, 3, hr)?;
        return Ok(());
    }
    if (dw_flags & DIEP_START) != 0 {
        eff.started = true;
    }
    let dev_gain = ctx
        .win32
        .dinput8
        .devices
        .get(&eff.device_va)
        .map_or(DI_FFNOMINALMAX, |d| d.ff_gain);
    let should_trigger = eff.started;
    let rumble = compute_rumble_for_effect(&eff, dev_gain, 1);
    ctx.win32.dinput8.effects.insert(this_ptr, eff);
    if should_trigger {
        dispatch_rumble(ctx, rumble.0, rumble.1, rumble.2);
    }
    return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
    Ok(())
}

fn dieff_start(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let dw_iterations = peek_arg_u32(cpu, ctx.arena, 1)?;
    let Some(eff) = ctx.win32.dinput8.effects.get_mut(&this_ptr) else {
        return_stdcall(cpu, ctx.arena, 3, DIERR_GENERIC)?;
        return Ok(());
    };
    eff.started = dw_iterations != 0;
    let eff_snapshot = eff.clone();
    let dev_gain = ctx
        .win32
        .dinput8
        .devices
        .get(&eff_snapshot.device_va)
        .map_or(DI_FFNOMINALMAX, |d| d.ff_gain);
    let (low, high, ms) = compute_rumble_for_effect(&eff_snapshot, dev_gain, dw_iterations);
    dispatch_rumble(ctx, low, high, ms);
    return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
    Ok(())
}

fn dieff_stop(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    if let Some(eff) = ctx.win32.dinput8.effects.get_mut(&this_ptr) {
        eff.started = false;
    }
    dispatch_rumble(ctx, 0, 0, 0);
    return_stdcall(cpu, ctx.arena, 1, DI_OK)?;
    Ok(())
}

fn dieff_get_effect_status(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pdw_flags = peek_arg_u32(cpu, ctx.arena, 1)?;
    if pdw_flags == 0 {
        return_stdcall(cpu, ctx.arena, 2, DIERR_INVALIDPARAM)?;
        return Ok(());
    }
    let playing = ctx
        .win32
        .dinput8
        .effects
        .get(&this_ptr)
        .is_some_and(|e| e.started);
    ctx.arena
        .write_u32(pdw_flags, if playing { DIEGES_PLAYING } else { 0 })?;
    return_stdcall(cpu, ctx.arena, 2, DI_OK)?;
    Ok(())
}

fn write_device_instance(
    arena: &mut GuestArena,
    va: GuestAddr,
    dw_size: u32,
    kind: DinputDeviceKind,
    name: &str,
) -> Result<(), RuntimeError> {
    arena.zero_range(va + 4, (dw_size - 4) as usize)?;
    arena.write_u32(va, dw_size)?;
    let (guid_inst, guid_prod, dev_type, usage_page, usage) = match kind {
        DinputDeviceKind::Keyboard => (
            GUID_SYSKEYBOARD,
            GUID_SYSKEYBOARD,
            DI8DEVTYPE_KEYBOARD | (DI8DEVTYPEKEYBOARD_PCENH << 8),
            0x01u16,
            0x06u16,
        ),
        DinputDeviceKind::Mouse => (
            GUID_SYSMOUSE,
            GUID_SYSMOUSE,
            DI8DEVTYPE_MOUSE | (DI8DEVTYPEMOUSE_TRADITIONAL << 8),
            0x01u16,
            0x02u16,
        ),
        DinputDeviceKind::Gamepad => (
            GUID_TOLUCA_GAMEPAD,
            GUID_TOLUCA_GAMEPAD,
            DI8DEVTYPE_GAMEPAD | (DI8DEVTYPEGAMEPAD_STANDARD << 8),
            0x01u16,
            0x05u16,
        ),
    };
    guid_inst.write_to(arena, va + 0x004)?;
    guid_prod.write_to(arena, va + 0x014)?;
    arena.write_u32(va + 0x024, dev_type)?;
    write_fixed_cstr(arena, va + 0x028, 260, name)?;
    write_fixed_cstr(arena, va + 0x12C, 260, name)?;
    if dw_size >= DIDEVICEINSTANCEA_SIZE_DX5 {
        GUID_NULL.write_to(arena, va + 0x230)?;
        arena.write_u16(va + 0x240, usage_page)?;
        arena.write_u16(va + 0x242, usage)?;
    }
    Ok(())
}

fn write_fixed_cstr(
    arena: &mut GuestArena,
    va: GuestAddr,
    cap: usize,
    text: &str,
) -> Result<(), RuntimeError> {
    arena.zero_range(va, cap)?;
    if cap == 0 {
        return Ok(());
    }
    let bytes = text.as_bytes();
    let copy_len = bytes.len().min(cap - 1);
    if copy_len > 0 {
        arena.write_bytes(va, &bytes[..copy_len])?;
    }
    arena.write_u8(va + copy_len as u32, 0)?;
    Ok(())
}

pub fn pack_keyboard_state(keys: &[bool; 256]) -> [u8; 256] {
    let mut out = [0u8; 256];
    for (dst, &pressed) in out.iter_mut().zip(keys.iter()) {
        if pressed {
            *dst = 0x80;
        }
    }
    out
}

pub fn sdl_mouse_mask_to_di_buttons(mask: u32) -> [bool; 8] {
    // SDL button bits: 1=Left, 2=Middle, 4=Right, 8=X1, 16=X2.
    // DirectInput DIMOUSESTATE: 0=Left, 1=Right, 2=Middle, 3=X1, 4=X2.
    [
        (mask & 1) != 0,
        (mask & 4) != 0,
        (mask & 2) != 0,
        (mask & 8) != 0,
        (mask & 16) != 0,
        false,
        false,
        false,
    ]
}

pub fn pack_mouse_state(dx: i32, dy: i32, dz: i32, buttons: &[bool; 8], out_len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; out_len];
    if out_len >= 4 {
        buf[0..4].copy_from_slice(&dx.to_le_bytes());
    }
    if out_len >= 8 {
        buf[4..8].copy_from_slice(&dy.to_le_bytes());
    }
    if out_len >= 12 {
        buf[8..12].copy_from_slice(&dz.to_le_bytes());
    }
    let btn_count = out_len.saturating_sub(12).min(buttons.len());
    for i in 0..btn_count {
        if buttons[i] {
            buf[12 + i] = 0x80;
        }
    }
    buf
}

fn gamepad_axes_in_di_order(gp: &GamepadSnapshot) -> [i16; SDL_GAMEPAD_AXIS_COUNT] {
    // DirectInput order: lX, lY, lZ (RightX), lRx (TriggerL), lRy (TriggerR), lRz (RightY).
    [
        gp.axes[0],
        gp.axes[1],
        gp.axes[2],
        gp.axes[4],
        gp.axes[5],
        gp.axes[3],
    ]
}

pub fn scale_axis(raw: i16, range: (i32, i32), deadzone: u32) -> i32 {
    let (l_min, l_max) = range;
    let adjusted = apply_deadzone(raw, deadzone);
    let mid = (i64::from(l_min) + i64::from(l_max)) / 2;
    if adjusted == 0 {
        return mid as i32;
    }
    if adjusted < 0 {
        (mid + (i64::from(adjusted) * (mid - i64::from(l_min))) / 32_768) as i32
    } else {
        (mid + (i64::from(adjusted) * (i64::from(l_max) - mid)) / 32_767) as i32
    }
}

fn apply_deadzone(raw: i16, deadzone: u32) -> i32 {
    if deadzone == 0 {
        return i32::from(raw);
    }
    let dz = deadzone.min(DI_FFNOMINALMAX);
    if dz >= DI_FFNOMINALMAX {
        return 0;
    }
    let dz_thresh = (i64::from(dz) * 32_768) / i64::from(DI_FFNOMINALMAX);
    let abs_raw = i64::from(raw).abs();
    if abs_raw <= dz_thresh {
        return 0;
    }
    let max_mag = if raw < 0 { 32_768i64 } else { 32_767i64 };
    let span = (32_768i64 - dz_thresh).max(1);
    let rescaled = ((abs_raw - dz_thresh) * max_mag) / span;
    let clamped = rescaled.min(max_mag) as i32;
    if raw < 0 { -clamped } else { clamped }
}

pub fn pack_gamepad_state(
    gp: &GamepadSnapshot,
    default_range: (i32, i32),
    axis_ranges: &[Option<(i32, i32)>; SDL_GAMEPAD_AXIS_COUNT],
    deadzone: u32,
    out_len: usize,
) -> Vec<u8> {
    let mut buf = vec![0u8; out_len];
    let raw_axes = gamepad_axes_in_di_order(gp);
    let mut scaled = [0i32; SDL_GAMEPAD_AXIS_COUNT];
    for (idx, &raw) in raw_axes.iter().enumerate() {
        let range = axis_ranges[idx].unwrap_or(default_range);
        scaled[idx] = scale_axis(raw, range, deadzone);
    }

    for (idx, &val) in scaled.iter().enumerate() {
        let off = idx * 4;
        if off + 4 <= out_len {
            buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
        }
    }
    // Sliders mirror triggers (lRx / lRy) for games reading rglSlider[0..2].
    if 0x18 + 4 <= out_len {
        buf[0x18..0x1C].copy_from_slice(&scaled[3].to_le_bytes());
    }
    if 0x1C + 4 <= out_len {
        buf[0x1C..0x20].copy_from_slice(&scaled[4].to_le_bytes());
    }
    for pov_idx in 0..4usize {
        let off = 0x20 + pov_idx * 4;
        if off + 4 <= out_len {
            let val = if pov_idx == 0 {
                gp.dpad_hat
            } else {
                DI_POV_CENTERED
            };
            buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
        }
    }
    let max_buttons = if out_len >= DIJOYSTATE2_SIZE { 128 } else { 32 };
    for (idx, &pressed) in gp.buttons.iter().enumerate() {
        if idx >= max_buttons {
            break;
        }
        let off = 0x30 + idx;
        if off < out_len && pressed {
            buf[off] = 0x80;
        }
    }
    buf
}

pub fn sdl_scancode_to_dik(scancode: i32) -> Option<u8> {
    const LETTER_DIK: [u8; 26] = [
        DIK_A, DIK_B, DIK_C, DIK_D, DIK_E, DIK_F, DIK_G, DIK_H, DIK_I, DIK_J, DIK_K, DIK_L, DIK_M,
        DIK_N, DIK_O, DIK_P, DIK_Q, DIK_R, DIK_S, DIK_T, DIK_U, DIK_V, DIK_W, DIK_X, DIK_Y, DIK_Z,
    ];
    if (4..=29).contains(&scancode) {
        return Some(LETTER_DIK[(scancode - 4) as usize]);
    }
    if (30..=38).contains(&scancode) {
        return Some(DIK_1 + (scancode - 30) as u8);
    }
    if (58..=67).contains(&scancode) {
        return Some(DIK_F1 + (scancode - 58) as u8);
    }
    match scancode {
        39 => Some(DIK_0),
        40 => Some(DIK_RETURN),
        41 => Some(DIK_ESCAPE),
        42 => Some(DIK_BACK),
        43 => Some(DIK_TAB),
        44 => Some(DIK_SPACE),
        45 => Some(DIK_MINUS),
        46 => Some(DIK_EQUALS),
        47 => Some(DIK_LBRACKET),
        48 => Some(DIK_RBRACKET),
        49 => Some(DIK_BACKSLASH),
        51 => Some(DIK_SEMICOLON),
        52 => Some(DIK_APOSTROPHE),
        53 => Some(DIK_GRAVE),
        54 => Some(DIK_COMMA),
        55 => Some(DIK_PERIOD),
        56 => Some(DIK_SLASH),
        57 => Some(DIK_CAPITAL),
        68 => Some(DIK_F11),
        69 => Some(DIK_F12),
        70 => Some(DIK_SYSRQ),
        71 => Some(DIK_SCROLL),
        72 => Some(DIK_PAUSE),
        73 => Some(DIK_INSERT),
        74 => Some(DIK_HOME),
        75 => Some(DIK_PRIOR),
        76 => Some(DIK_DELETE),
        77 => Some(DIK_END),
        78 => Some(DIK_NEXT),
        79 => Some(DIK_RIGHT),
        80 => Some(DIK_LEFT),
        81 => Some(DIK_DOWN),
        82 => Some(DIK_UP),
        83 => Some(DIK_NUMLOCK),
        84 => Some(DIK_DIVIDE),
        85 => Some(DIK_MULTIPLY),
        86 => Some(DIK_SUBTRACT),
        87 => Some(DIK_ADD),
        88 => Some(DIK_NUMPADENTER),
        89 => Some(DIK_NUMPAD1),
        90 => Some(DIK_NUMPAD2),
        91 => Some(DIK_NUMPAD3),
        92 => Some(DIK_NUMPAD4),
        93 => Some(DIK_NUMPAD5),
        94 => Some(DIK_NUMPAD6),
        95 => Some(DIK_NUMPAD7),
        96 => Some(DIK_NUMPAD8),
        97 => Some(DIK_NUMPAD9),
        98 => Some(DIK_NUMPAD0),
        99 => Some(DIK_DECIMAL),
        100 => Some(DIK_OEM_102),
        101 => Some(DIK_APPS),
        104 => Some(DIK_F13),
        105 => Some(DIK_F14),
        106 => Some(DIK_F15),
        224 => Some(DIK_LCONTROL),
        225 => Some(DIK_LSHIFT),
        226 => Some(DIK_LMENU),
        227 => Some(DIK_LWIN),
        228 => Some(DIK_RCONTROL),
        229 => Some(DIK_RSHIFT),
        230 => Some(DIK_RMENU),
        231 => Some(DIK_RWIN),
        _ => None,
    }
}

pub fn dik_to_vk(dik: u8) -> Option<u8> {
    match dik {
        DIK_ESCAPE => Some(0x1B),
        DIK_1..=DIK_9 => Some(b'1' + (dik - DIK_1)),
        DIK_0 => Some(b'0'),
        DIK_BACK => Some(0x08),
        DIK_TAB => Some(0x09),
        DIK_RETURN | DIK_NUMPADENTER => Some(0x0D),
        DIK_SPACE => Some(0x20),
        DIK_LSHIFT | DIK_RSHIFT => Some(0x10),
        DIK_LCONTROL | DIK_RCONTROL => Some(0x11),
        DIK_LMENU | DIK_RMENU => Some(0x12),
        DIK_UP => Some(0x26),
        DIK_DOWN => Some(0x28),
        DIK_LEFT => Some(0x25),
        DIK_RIGHT => Some(0x27),
        DIK_F1..=DIK_F10 => Some(0x70 + (dik - DIK_F1)),
        DIK_F11 => Some(0x7A),
        DIK_F12 => Some(0x7B),
        DIK_A => Some(b'A'),
        DIK_B => Some(b'B'),
        DIK_C => Some(b'C'),
        DIK_D => Some(b'D'),
        DIK_E => Some(b'E'),
        DIK_F => Some(b'F'),
        DIK_G => Some(b'G'),
        DIK_H => Some(b'H'),
        DIK_I => Some(b'I'),
        DIK_J => Some(b'J'),
        DIK_K => Some(b'K'),
        DIK_L => Some(b'L'),
        DIK_M => Some(b'M'),
        DIK_N => Some(b'N'),
        DIK_O => Some(b'O'),
        DIK_P => Some(b'P'),
        DIK_Q => Some(b'Q'),
        DIK_R => Some(b'R'),
        DIK_S => Some(b'S'),
        DIK_T => Some(b'T'),
        DIK_U => Some(b'U'),
        DIK_V => Some(b'V'),
        DIK_W => Some(b'W'),
        DIK_X => Some(b'X'),
        DIK_Y => Some(b'Y'),
        DIK_Z => Some(b'Z'),
        _ => None,
    }
}

pub fn parse_dik_name(raw: &str) -> Option<u8> {
    let lower = raw.trim().to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        return u8::from_str_radix(hex, 16).ok();
    }
    if lower.len() == 1 {
        let ch = lower.as_bytes()[0];
        if ch.is_ascii_lowercase() {
            return sdl_scancode_to_dik(4 + i32::from(ch - b'a'));
        }
        if ch == b'0' {
            return Some(DIK_0);
        }
        if (b'1'..=b'9').contains(&ch) {
            return Some(DIK_1 + (ch - b'1'));
        }
    }
    match lower.as_str() {
        "return" | "enter" => Some(DIK_RETURN),
        "escape" | "esc" => Some(DIK_ESCAPE),
        "space" => Some(DIK_SPACE),
        "tab" => Some(DIK_TAB),
        "backspace" | "back" => Some(DIK_BACK),
        "up" => Some(DIK_UP),
        "down" => Some(DIK_DOWN),
        "left" => Some(DIK_LEFT),
        "right" => Some(DIK_RIGHT),
        "shift" | "lshift" => Some(DIK_LSHIFT),
        "rshift" => Some(DIK_RSHIFT),
        "ctrl" | "lctrl" | "lcontrol" => Some(DIK_LCONTROL),
        "rctrl" | "rcontrol" => Some(DIK_RCONTROL),
        "alt" | "lalt" | "lmenu" => Some(DIK_LMENU),
        "ralt" | "rmenu" => Some(DIK_RMENU),
        "insert" => Some(DIK_INSERT),
        "delete" | "del" => Some(DIK_DELETE),
        "home" => Some(DIK_HOME),
        "end" => Some(DIK_END),
        "pageup" | "pgup" | "prior" => Some(DIK_PRIOR),
        "pagedown" | "pgdn" | "next" => Some(DIK_NEXT),
        "f1" => Some(DIK_F1),
        "f2" => Some(DIK_F2),
        "f3" => Some(DIK_F3),
        "f4" => Some(DIK_F4),
        "f5" => Some(DIK_F5),
        "f6" => Some(DIK_F6),
        "f7" => Some(DIK_F7),
        "f8" => Some(DIK_F8),
        "f9" => Some(DIK_F9),
        "f10" => Some(DIK_F10),
        "f11" => Some(DIK_F11),
        "f12" => Some(DIK_F12),
        _ => lower.parse::<u8>().ok(),
    }
}

pub fn parse_script_keys(spec: &str) -> Result<Vec<ScriptedKeyEvent>, String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut events = Vec::new();
    for entry in trimmed.split(',') {
        let parts: Vec<&str> = entry.trim().split(':').collect();
        if parts.len() != 3 {
            return Err(format!(
                "Invalid --script-keys entry '{entry}'; expected <start_frame>:<key>:<duration_frames>"
            ));
        }
        let start_frame = parts[0]
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("Invalid start_frame '{}' in '{entry}'", parts[0]))?;
        let dik_code = parse_dik_name(parts[1])
            .ok_or_else(|| format!("Unknown key '{}' in '{entry}'", parts[1]))?;
        let duration_frames = parts[2]
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("Invalid duration_frames '{}' in '{entry}'", parts[2]))?;
        events.push(ScriptedKeyEvent {
            start_frame,
            duration_frames,
            dik_code,
        });
    }
    Ok(events)
}

fn stdcall_zero_1(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, DI_OK)?;
    Ok(())
}

fn stdcall_zero_2(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, DI_OK)?;
    Ok(())
}

fn stdcall_zero_3(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 3, DI_OK)?;
    Ok(())
}

fn stdcall_zero_4(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 4, DI_OK)?;
    Ok(())
}

fn stdcall_zero_5(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 5, DI_OK)?;
    Ok(())
}

fn stdcall_zero_6(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 6, DI_OK)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Instant;

    use toluca_core::bridge::HostBridge;
    use toluca_core::cpu::Gpr;
    use toluca_core::memory::DEFAULT_IMAGE_BASE;

    use crate::callconv::push_u32;
    use crate::heap::{GuestHeap, VirtualMemoryManager};
    use crate::loader::AotCodeImage;
    use crate::runtime::{ImportRegistry, ResolvedImportSlot};
    use crate::win32::{Win32Config, Win32State};

    struct TestHarness {
        arena: GuestArena,
        heap: GuestHeap,
        vmem: VirtualMemoryManager,
        win32: Win32State,
        registry: ImportRegistry,
        imports: Vec<ResolvedImportSlot>,
        import_slot_map: HashMap<crate::runtime::ImportKey, usize>,
        code: AotCodeImage,
        cpu: GuestCpuState,
        last_error: u32,
        start: Instant,
        trap: Option<crate::runtime::TrapInfo>,
        pending_error: Option<RuntimeError>,
    }

    impl TestHarness {
        fn new(script_keys: Vec<ScriptedKeyEvent>) -> Self {
            let mut arena = GuestArena::new().unwrap();
            arena.init_thread_environment(DEFAULT_IMAGE_BASE).unwrap();
            let heap = GuestHeap::new();
            let vmem = VirtualMemoryManager::new();
            let mut registry = ImportRegistry::new();
            register_imports(&mut registry);
            let config = Win32Config {
                headless: true,
                script_keys,
                ..Win32Config::default()
            };
            let win32 = Win32State::new(config);
            let bridge = HostBridge::new_stub();
            let cpu = GuestCpuState::new(arena.base_ptr(), &bridge);
            Self {
                arena,
                heap,
                vmem,
                win32,
                registry,
                imports: Vec::new(),
                import_slot_map: HashMap::new(),
                code: AotCodeImage::empty_for_test(),
                cpu,
                last_error: 0,
                start: Instant::now(),
                trap: None,
                pending_error: None,
            }
        }

        fn call_com(&mut self, obj_va: GuestAddr, method_idx: u32, args: &[u32]) -> u32 {
            let vtable = self.arena.read_u32(obj_va).unwrap();
            let thunk_va = self.arena.read_u32(vtable + method_idx * 4).unwrap();
            let slot_idx = ((thunk_va - crate::runtime::IMPORT_THUNK_BASE) / 4) as usize;
            let handler = self.imports[slot_idx].handler.unwrap();

            for &arg in args.iter().rev() {
                push_u32(&mut self.cpu, &mut self.arena, arg).unwrap();
            }
            push_u32(&mut self.cpu, &mut self.arena, obj_va).unwrap();
            push_u32(&mut self.cpu, &mut self.arena, 0x0040_9999).unwrap();

            let mut ctx = RuntimeContext {
                pe_image_base: DEFAULT_IMAGE_BASE,
                arena: &mut self.arena,
                heap: &mut self.heap,
                vmem: &mut self.vmem,
                win32: &mut self.win32,
                imports: &mut self.imports,
                import_slot_map: &mut self.import_slot_map,
                registry: &self.registry,
                code: &self.code,
                last_error: &mut self.last_error,
                active_host_tid: 1,
                host_start_instant: self.start,
                trap: &mut self.trap,
                pending_error: &mut self.pending_error,
            };
            handler(&mut ctx, &mut self.cpu).unwrap();
            self.cpu.gpr(Gpr::Eax)
        }

        fn create_di8(&mut self) -> GuestAddr {
            let ppv = self.heap.alloc(&mut self.arena, 4, true).unwrap();
            for arg in [0u32, ppv, 0, 0x0800, 0].into_iter() {
                push_u32(&mut self.cpu, &mut self.arena, arg).unwrap();
            }
            push_u32(&mut self.cpu, &mut self.arena, 0x0040_9999).unwrap();

            let mut ctx = RuntimeContext {
                pe_image_base: DEFAULT_IMAGE_BASE,
                arena: &mut self.arena,
                heap: &mut self.heap,
                vmem: &mut self.vmem,
                win32: &mut self.win32,
                imports: &mut self.imports,
                import_slot_map: &mut self.import_slot_map,
                registry: &self.registry,
                code: &self.code,
                last_error: &mut self.last_error,
                active_host_tid: 1,
                host_start_instant: self.start,
                trap: &mut self.trap,
                pending_error: &mut self.pending_error,
            };
            direct_input_8_create(&mut ctx, &mut self.cpu).unwrap();
            assert_eq!(self.cpu.gpr(Gpr::Eax), DI_OK);
            self.arena.read_u32(ppv).unwrap()
        }

        fn create_device(&mut self, di8: GuestAddr, guid: WinGuid) -> GuestAddr {
            let guid_va = self.heap.alloc(&mut self.arena, 16, true).unwrap();
            guid.write_to(&mut self.arena, guid_va).unwrap();
            let ppdev = self.heap.alloc(&mut self.arena, 4, true).unwrap();
            let hr = self.call_com(di8, 3, &[guid_va, ppdev, 0]);
            assert_eq!(hr, DI_OK);
            self.arena.read_u32(ppdev).unwrap()
        }
    }

    #[test]
    fn scancode_mapping_and_keyboard_device_state() {
        assert_eq!(sdl_scancode_to_dik(4), Some(DIK_A));
        assert_eq!(sdl_scancode_to_dik(29), Some(DIK_Z));
        assert_eq!(sdl_scancode_to_dik(30), Some(DIK_1));
        assert_eq!(sdl_scancode_to_dik(39), Some(DIK_0));
        assert_eq!(sdl_scancode_to_dik(40), Some(DIK_RETURN));
        assert_eq!(sdl_scancode_to_dik(41), Some(DIK_ESCAPE));
        assert_eq!(sdl_scancode_to_dik(44), Some(DIK_SPACE));
        assert_eq!(sdl_scancode_to_dik(58), Some(DIK_F1));
        assert_eq!(sdl_scancode_to_dik(69), Some(DIK_F12));
        assert_eq!(sdl_scancode_to_dik(79), Some(DIK_RIGHT));
        assert_eq!(sdl_scancode_to_dik(80), Some(DIK_LEFT));
        assert_eq!(sdl_scancode_to_dik(81), Some(DIK_DOWN));
        assert_eq!(sdl_scancode_to_dik(82), Some(DIK_UP));
        assert_eq!(sdl_scancode_to_dik(88), Some(DIK_NUMPADENTER));
        assert_eq!(sdl_scancode_to_dik(225), Some(DIK_LSHIFT));
        assert_eq!(sdl_scancode_to_dik(228), Some(DIK_RCONTROL));
        assert_eq!(sdl_scancode_to_dik(230), Some(DIK_RMENU));

        let mut h = TestHarness::new(Vec::new());
        let di8 = h.create_di8();
        let kbd = h.create_device(di8, GUID_SYSKEYBOARD);
        let state_va = h.heap.alloc(&mut h.arena, 256, true).unwrap();

        assert_eq!(h.call_com(kbd, 9, &[256, state_va]), DIERR_NOTACQUIRED);
        assert_eq!(h.call_com(kbd, 7, &[]), DI_OK);

        let mut snap = SdlInputSnapshot::default();
        snap.key_scancodes[40] = true; // Return
        snap.key_scancodes[82] = true; // Up
        snap.key_scancodes[225] = true; // LShift
        h.win32.dinput8.override_snapshot = Some(snap);

        assert_eq!(h.call_com(kbd, 9, &[256, state_va]), DI_OK);
        let bytes = h.arena.read_bytes(state_va, 256).unwrap();
        assert_eq!(bytes[DIK_RETURN as usize], 0x80);
        assert_eq!(bytes[DIK_UP as usize], 0x80);
        assert_eq!(bytes[DIK_LSHIFT as usize], 0x80);
        assert_eq!(bytes[DIK_ESCAPE as usize], 0x00);
    }

    #[test]
    fn dijoystate_and_dijoystate2_packing_scaling_deadzone_pov_and_buttons() {
        let mut gp = GamepadSnapshot {
            axes: [-32768, 32767, 16384, -16384, 0, 32767],
            dpad_hat: 13500,
            buttons: [false; SDL_GAMEPAD_BUTTON_COUNT],
        };
        gp.buttons[0] = true;
        gp.buttons[7] = true;
        gp.buttons[9] = true;
        gp.buttons[11] = true;

        let ranges = [None; SDL_GAMEPAD_AXIS_COUNT];
        let joy1 = pack_gamepad_state(&gp, (-1000, 1000), &ranges, 0, DIJOYSTATE_SIZE);
        assert_eq!(joy1.len(), DIJOYSTATE_SIZE);
        let read_i32 = |buf: &[u8], off: usize| {
            i32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
        };
        let read_u32 = |buf: &[u8], off: usize| {
            u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
        };

        assert_eq!(read_i32(&joy1, 0), -1000);
        assert_eq!(read_i32(&joy1, 4), 1000);
        assert!(read_i32(&joy1, 8) > 490 && read_i32(&joy1, 8) < 510);
        assert_eq!(read_i32(&joy1, 12), 0);
        assert_eq!(read_i32(&joy1, 16), 1000);
        assert!(read_i32(&joy1, 20) < -490 && read_i32(&joy1, 20) > -510);
        assert_eq!(read_u32(&joy1, 32), 13500);
        assert_eq!(read_u32(&joy1, 36), DI_POV_CENTERED);
        assert_eq!(joy1[0x30], 0x80);
        assert_eq!(joy1[0x31], 0x00);
        assert_eq!(joy1[0x37], 0x80);
        assert_eq!(joy1[0x39], 0x80);
        assert_eq!(joy1[0x3B], 0x80);

        let gp_small = GamepadSnapshot {
            axes: [2500, -2500, 0, 0, 0, 0],
            dpad_hat: DI_POV_CENTERED,
            buttons: [false; SDL_GAMEPAD_BUTTON_COUNT],
        };
        let joy2 = pack_gamepad_state(&gp_small, (0, 65535), &ranges, 2000, DIJOYSTATE2_SIZE);
        assert_eq!(joy2.len(), DIJOYSTATE2_SIZE);
        assert_eq!(read_i32(&joy2, 0), 32767);
        assert_eq!(read_i32(&joy2, 4), 32767);
    }

    #[test]
    fn set_and_get_property_diprop_range() {
        let mut h = TestHarness::new(Vec::new());
        let di8 = h.create_di8();
        let pad = h.create_device(di8, GUID_TOLUCA_GAMEPAD);

        let prop_va = h.heap.alloc(&mut h.arena, 24, true).unwrap();
        h.arena.write_u32(prop_va, DIPROPRANGE_SIZE).unwrap();
        h.arena.write_u32(prop_va + 4, DIPROPHEADER_SIZE).unwrap();
        h.arena.write_u32(prop_va + 8, 0).unwrap();
        h.arena.write_u32(prop_va + 12, DIPH_DEVICE).unwrap();
        h.arena.write_u32(prop_va + 16, (-5000i32) as u32).unwrap();
        h.arena.write_u32(prop_va + 20, 5000u32).unwrap();

        assert_eq!(h.call_com(pad, 6, &[DIPROP_RANGE, prop_va]), DI_OK);

        h.arena.write_u32(prop_va + 16, 0).unwrap();
        h.arena.write_u32(prop_va + 20, 0).unwrap();
        assert_eq!(h.call_com(pad, 5, &[DIPROP_RANGE, prop_va]), DI_OK);
        assert_eq!(h.arena.read_u32(prop_va + 16).unwrap() as i32, -5000);
        assert_eq!(h.arena.read_u32(prop_va + 20).unwrap() as i32, 5000);

        assert_eq!(h.call_com(pad, 7, &[]), DI_OK);
        let snap = SdlInputSnapshot {
            gamepad: Some(GamepadSnapshot {
                axes: [-32768, 32767, 0, 0, 0, 0],
                ..GamepadSnapshot::default()
            }),
            ..SdlInputSnapshot::default()
        };
        h.win32.dinput8.override_snapshot = Some(snap);
        let joy_va = h.heap.alloc(&mut h.arena, DIJOYSTATE_SIZE, true).unwrap();
        assert_eq!(
            h.call_com(pad, 9, &[DIJOYSTATE_SIZE as u32, joy_va]),
            DI_OK
        );
        assert_eq!(h.arena.read_u32(joy_va).unwrap() as i32, -5000);
        assert_eq!(h.arena.read_u32(joy_va + 4).unwrap() as i32, 5000);
    }

    #[test]
    fn buffered_get_device_data_records_press_and_release_transitions() {
        let mut h = TestHarness::new(Vec::new());
        let di8 = h.create_di8();
        let kbd = h.create_device(di8, GUID_SYSKEYBOARD);

        let prop_va = h.heap.alloc(&mut h.arena, DIPROPDWORD_SIZE as usize, true).unwrap();
        h.arena.write_u32(prop_va, DIPROPDWORD_SIZE).unwrap();
        h.arena.write_u32(prop_va + 4, DIPROPHEADER_SIZE).unwrap();
        h.arena.write_u32(prop_va + 8, 0).unwrap();
        h.arena.write_u32(prop_va + 12, DIPH_DEVICE).unwrap();
        h.arena.write_u32(prop_va + 16, 16).unwrap();
        assert_eq!(h.call_com(kbd, 6, &[DIPROP_BUFFERSIZE, prop_va]), DI_OK);
        assert_eq!(h.call_com(kbd, 7, &[]), DI_OK);

        let mut snap_press = SdlInputSnapshot::default();
        snap_press.key_scancodes[40] = true; // Return
        h.win32.dinput8.override_snapshot = Some(snap_press);
        assert_eq!(h.call_com(kbd, 25, &[]), DI_OK);

        let snap_release = SdlInputSnapshot::default();
        h.win32.dinput8.override_snapshot = Some(snap_release);

        let dod_va = h.heap.alloc(&mut h.arena, 20 * 4, true).unwrap();
        let count_va = h.heap.alloc(&mut h.arena, 4, true).unwrap();
        h.arena.write_u32(count_va, 4).unwrap();
        assert_eq!(h.call_com(kbd, 10, &[20, dod_va, count_va, 0]), DI_OK);
        assert_eq!(h.arena.read_u32(count_va).unwrap(), 2);

        assert_eq!(h.arena.read_u32(dod_va).unwrap(), u32::from(DIK_RETURN));
        assert_eq!(h.arena.read_u32(dod_va + 4).unwrap(), 0x80);
        assert_eq!(h.arena.read_u32(dod_va + 20).unwrap(), u32::from(DIK_RETURN));
        assert_eq!(h.arena.read_u32(dod_va + 24).unwrap(), 0x00);
    }

    #[test]
    fn direct_input_effect_set_parameters_start_and_stop_dispatch_rumble() {
        let mut h = TestHarness::new(Vec::new());
        let di8 = h.create_di8();
        let pad = h.create_device(di8, GUID_TOLUCA_GAMEPAD);

        let guid_va = h.heap.alloc(&mut h.arena, 16, true).unwrap();
        GUID_CONSTANTFORCE.write_to(&mut h.arena, guid_va).unwrap();
        let ppeff = h.heap.alloc(&mut h.arena, 4, true).unwrap();
        assert_eq!(h.call_com(pad, 18, &[guid_va, 0, ppeff, 0]), DI_OK);
        let eff = h.arena.read_u32(ppeff).unwrap();
        assert_ne!(eff, 0);

        let cf_va = h.heap.alloc(&mut h.arena, 4, true).unwrap();
        h.arena.write_u32(cf_va, 5000).unwrap();
        let dieff_va = h.heap.alloc(&mut h.arena, DIEFFECT_SIZE_DX6 as usize, true).unwrap();
        h.arena.write_u32(dieff_va, DIEFFECT_SIZE_DX6).unwrap();
        h.arena.write_u32(dieff_va + 8, 250_000).unwrap();
        h.arena.write_u32(dieff_va + 16, 10_000).unwrap();
        h.arena.write_u32(dieff_va + 44, 4).unwrap();
        h.arena.write_u32(dieff_va + 48, cf_va).unwrap();

        assert_eq!(h.call_com(eff, 6, &[dieff_va, 0]), DI_OK);
        assert_eq!(h.call_com(eff, 7, &[1, 0]), DI_OK);
        assert_eq!(h.win32.dinput8.last_rumble_command, Some((32767, 32767, 250)));

        assert_eq!(h.call_com(eff, 8, &[]), DI_OK);
        assert_eq!(h.win32.dinput8.last_rumble_command, Some((0, 0, 0)));
    }

    #[test]
    fn scripted_keys_parse_and_fire_only_within_target_frame_window() {
        let parsed = parse_script_keys("480:return:6,530:space:2").unwrap();
        assert_eq!(
            parsed,
            vec![
                ScriptedKeyEvent {
                    start_frame: 480,
                    duration_frames: 6,
                    dik_code: DIK_RETURN,
                },
                ScriptedKeyEvent {
                    start_frame: 530,
                    duration_frames: 2,
                    dik_code: DIK_SPACE,
                },
            ]
        );

        let mut h = TestHarness::new(vec![ScriptedKeyEvent {
            start_frame: 10,
            duration_frames: 3,
            dik_code: DIK_RETURN,
        }]);
        let di8 = h.create_di8();
        let kbd = h.create_device(di8, GUID_SYSKEYBOARD);
        assert_eq!(h.call_com(kbd, 7, &[]), DI_OK);
        let state_va = h.heap.alloc(&mut h.arena, 256, true).unwrap();

        h.win32.d3d8_present_calls = 9;
        assert_eq!(h.call_com(kbd, 9, &[256, state_va]), DI_OK);
        assert_eq!(h.arena.read_bytes(state_va, 256).unwrap()[DIK_RETURN as usize], 0x00);

        h.win32.d3d8_present_calls = 10;
        assert_eq!(h.call_com(kbd, 9, &[256, state_va]), DI_OK);
        assert_eq!(h.arena.read_bytes(state_va, 256).unwrap()[DIK_RETURN as usize], 0x80);

        h.win32.d3d8_present_calls = 12;
        assert_eq!(h.call_com(kbd, 9, &[256, state_va]), DI_OK);
        assert_eq!(h.arena.read_bytes(state_va, 256).unwrap()[DIK_RETURN as usize], 0x80);

        h.win32.d3d8_present_calls = 13;
        assert_eq!(h.call_com(kbd, 9, &[256, state_va]), DI_OK);
        assert_eq!(h.arena.read_bytes(state_va, 256).unwrap()[DIK_RETURN as usize], 0x00);
    }
}
