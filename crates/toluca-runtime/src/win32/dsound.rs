use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use toluca_core::GuestAddr;
use toluca_core::aot::ImportSymbol;
use toluca_core::cpu::GuestCpuState;

use crate::arena::GuestArena;
use crate::callconv::{peek_arg_i32, peek_arg_u32, return_stdcall};
use crate::runtime::{ImportHandler, ImportRegistry, RuntimeContext, RuntimeError};
use crate::win32::sdl2::Sdl2AudioDevice;

const DSOUND_PERF_LOG_INTERVAL: Duration = Duration::from_secs(10);

fn dsound_perf_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("TOLUCA_PERF").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

pub const DEFAULT_OUTPUT_SAMPLE_RATE: u32 = 48_000;
const OUTPUT_CHANNELS: usize = 2;
const MAX_AUX_PCM_SAMPLES: usize = (DEFAULT_OUTPUT_SAMPLE_RATE as usize) * OUTPUT_CHANNELS * 4;
const WRITE_CURSOR_LEAD_FRAMES: usize = 1024;
const MIN_BUFFER_BYTES: u32 = 4096;
const MAX_BUFFER_BYTES: u32 = 4 * 1024 * 1024;
const DEFAULT_BUFFER_BYTES: u32 = 65_536;

const DSOUND_COM_DLL: &str = "__dsound_com__";
const DS8_VTABLE_SLOTS: usize = 12;
const DSB8_VTABLE_SLOTS: usize = 22;
const DS3DLISTENER_VTABLE_SLOTS: usize = 18;
const DS3DBUFFER_VTABLE_SLOTS: usize = 21;

pub const DS_OK: u32 = 0;
pub const DSERR_NOINTERFACE: u32 = 0x8000_4002;
pub const DSERR_GENERIC: u32 = 0x8000_4005;
pub const DSERR_INVALIDPARAM: u32 = 0x8007_0057;
pub const DSERR_BADFORMAT: u32 = 0x8878_0064;

const DSBPLAY_LOOPING: u32 = 0x0000_0001;
const DSBSTATUS_PLAYING: u32 = 0x0000_0001;
const DSBSTATUS_LOOPING: u32 = 0x0000_0004;

const DSBCAPS_PRIMARYBUFFER: u32 = 0x0000_0001;
const DSBCAPS_CTRL3D: u32 = 0x0000_0010;
const DSBCAPS_CTRLFREQUENCY: u32 = 0x0000_0020;
const DSBCAPS_CTRLPAN: u32 = 0x0000_0040;
const DSBCAPS_CTRLVOLUME: u32 = 0x0000_0080;

const DSBLOCK_FROMWRITECURSOR: u32 = 0x0000_0001;
const DSBLOCK_ENTIREBUFFER: u32 = 0x0000_0002;

pub const DS3DMODE_NORMAL: u32 = 0;
pub const DS3DMODE_HEADRELATIVE: u32 = 1;
pub const DS3DMODE_DISABLE: u32 = 2;
const DS3D_DEFERRED: u32 = 1;

const DSBVOLUME_MIN: i32 = -10_000;
const DSBVOLUME_MAX: i32 = 0;
const DSBPAN_LEFT: i32 = -10_000;
const DSBPAN_RIGHT: i32 = 10_000;

const WAVE_FORMAT_PCM: u16 = 1;
const WAVEFORMATEX_SIZE: u32 = 18;
const DS3DLISTENER_SIZE: u32 = 64;
const DS3DBUFFER_SIZE: u32 = 64;

const IID_IUNKNOWN_DATA1: u32 = 0x0000_0000;
const IID_IDIRECTSOUND_DATA1: u32 = 0x279A_FA83;
const IID_IDIRECTSOUND8_DATA1: u32 = 0xC50A_7E93;
const IID_IDIRECTSOUNDBUFFER_DATA1: u32 = 0x279A_FA85;
const IID_IDIRECTSOUNDBUFFER8_DATA1: u32 = 0x6825_A449;
const IID_IDIRECTSOUND3DLISTENER_DATA1: u32 = 0x279A_FA84;
const IID_IDIRECTSOUND3DBUFFER_DATA1: u32 = 0x279A_FA86;

#[derive(Clone, Debug)]
pub struct Ds3dBufferParams {
    pub pos: [f32; 3],
    pub velocity: [f32; 3],
    pub min_distance: f32,
    pub max_distance: f32,
    pub mode: u32,
    pub is_3d: bool,
    pub inside_cone_angle: u32,
    pub outside_cone_angle: u32,
    pub cone_orientation: [f32; 3],
    pub cone_outside_volume: i32,
}

impl Default for Ds3dBufferParams {
    fn default() -> Self {
        Self {
            pos: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            min_distance: 1.0,
            max_distance: 1_000_000_000.0,
            mode: DS3DMODE_NORMAL,
            is_3d: false,
            inside_cone_angle: 360,
            outside_cone_angle: 360,
            cone_orientation: [0.0, 0.0, 1.0],
            cone_outside_volume: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Ds3dListenerParams {
    pub pos: [f32; 3],
    pub velocity: [f32; 3],
    pub front: [f32; 3],
    pub top: [f32; 3],
    pub distance_factor: f32,
    pub rolloff_factor: f32,
    pub doppler_factor: f32,
}

impl Default for Ds3dListenerParams {
    fn default() -> Self {
        Self {
            pos: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            front: [0.0, 0.0, 1.0],
            top: [0.0, 1.0, 0.0],
            distance_factor: 1.0,
            rolloff_factor: 1.0,
            doppler_factor: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DsoundBufferData {
    pub data: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub playing: bool,
    pub looping: bool,
    pub is_primary: bool,
    pub flags: u32,
    pub play_pos_frames: f64,
    pub volume_db: i32,
    pub pan_db: i32,
    pub ds3d: Ds3dBufferParams,
}

impl DsoundBufferData {
    pub fn frame_bytes(&self) -> usize {
        (self.channels.max(1) as usize) * ((self.bits_per_sample.max(8) as usize) / 8)
    }

    pub fn total_frames(&self) -> usize {
        let fb = self.frame_bytes().max(1);
        self.data.len() / fb
    }

    pub fn play_byte_offset(&self) -> u32 {
        let total = self.total_frames();
        if total == 0 {
            return 0;
        }
        let frame = (self.play_pos_frames.floor() as usize) % total;
        (frame * self.frame_bytes()) as u32
    }

    pub fn write_byte_offset(&self) -> u32 {
        let total = self.total_frames();
        if total == 0 {
            return 0;
        }
        let lead = WRITE_CURSOR_LEAD_FRAMES.min(total / 4);
        let frame = ((self.play_pos_frames.floor() as usize) + lead) % total;
        (frame * self.frame_bytes()) as u32
    }
}

const SOFT_LIMIT_KNEE: f32 = 0.85;
const DEFAULT_OUTPUT_BUFFER_FRAMES: u32 = 2048;
pub const MAX_AUX_RATE_SKEW: f64 = 0.025;
const AUX_RATE_UNITY_EPSILON: f64 = 1e-6;
// Net audio-minus-video pipeline delay beyond the just-popped SDL2 mixbuf period:
// ALSA holds ~2 periods ahead of mixbuf (~85 ms at 2048/48 kHz) minus ~30 ms LCD
// scanout latency, putting mean audible audio ~2.0 periods (0.5 + 1.5) behind
// aux_played_frames.
const NET_AUDIO_LATENCY_EXTRA_PERIODS: f64 = 1.5;

pub struct DsoundMixer {
    pub buffers: HashMap<usize, DsoundBufferData>,
    next_buf_id: usize,
    pub listener: Ds3dListenerParams,
    pub deferred_listener: Ds3dListenerParams,
    pub aux_pcm: VecDeque<i16>,
    aux_resample_frac: f64,
    aux_play_frac: f64,
    aux_rate_ratio: f64,
    pub aux_played_frames: u64,
    pub last_aux_popped_frames: u64,
    pub output_sample_rate: u32,
    output_buffer_frames: u32,
    mix_accum: Vec<f32>,
    pub hardware_clocked: bool,
    pub last_advance: Instant,
    pub last_audio_callback: Instant,
    last_perf_log: Instant,
}

impl Default for DsoundMixer {
    fn default() -> Self {
        Self::new()
    }
}

impl DsoundMixer {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            buffers: HashMap::new(),
            next_buf_id: 1,
            listener: Ds3dListenerParams::default(),
            deferred_listener: Ds3dListenerParams::default(),
            aux_pcm: VecDeque::new(),
            aux_resample_frac: 0.0,
            aux_play_frac: 0.0,
            aux_rate_ratio: 1.0,
            aux_played_frames: 0,
            last_aux_popped_frames: 0,
            output_sample_rate: DEFAULT_OUTPUT_SAMPLE_RATE,
            output_buffer_frames: DEFAULT_OUTPUT_BUFFER_FRAMES,
            mix_accum: Vec::new(),
            hardware_clocked: false,
            last_advance: now,
            last_audio_callback: now,
            last_perf_log: now,
        }
    }

    pub fn output_buffer_frames(&self) -> u32 {
        self.output_buffer_frames
    }

    pub fn clear_aux_pcm(&mut self) {
        self.aux_pcm.clear();
        self.aux_resample_frac = 0.0;
        self.aux_play_frac = 0.0;
        self.aux_rate_ratio = 1.0;
        self.last_aux_popped_frames = 0;
    }

    pub fn set_aux_rate_ratio(&mut self, ratio: f64) {
        self.aux_rate_ratio = if ratio.is_finite() {
            ratio.clamp(1.0 - MAX_AUX_RATE_SKEW, 1.0 + MAX_AUX_RATE_SKEW)
        } else {
            1.0
        };
    }

    pub fn aux_rate_ratio(&self) -> f64 {
        self.aux_rate_ratio
    }

    pub fn aux_played_frames(&self) -> u64 {
        self.aux_played_frames
    }

    pub fn estimated_audible_aux_sec(&self, start_audio_frames: u64, now: Instant) -> Option<f64> {
        if !self.hardware_clocked
            || self.aux_played_frames <= start_audio_frames
            || self.aux_queued_frames() == 0
        {
            return None;
        }
        let rate = f64::from(self.output_sample_rate.max(1));
        let buf_frames = f64::from(self.output_buffer_frames.max(1));
        let period_sec = buf_frames / rate;
        let elapsed_sec = now
            .saturating_duration_since(self.last_audio_callback)
            .as_secs_f64()
            .min(period_sec);
        let played_since_start = (self.aux_played_frames - start_audio_frames) as f64;
        let base_frames = played_since_start
            - (self.last_aux_popped_frames as f64)
            - NET_AUDIO_LATENCY_EXTRA_PERIODS * buf_frames;
        let interp_frames = elapsed_sec * rate * self.aux_rate_ratio;
        Some(((base_frames + interp_frames).max(0.0)) / rate)
    }

    pub fn aux_queued_frames(&self) -> usize {
        self.aux_pcm.len() / OUTPUT_CHANNELS
    }

    pub fn output_sample_rate(&self) -> u32 {
        self.output_sample_rate
    }

    pub fn alloc_buffer(&mut self, buf: DsoundBufferData) -> usize {
        let id = self.next_buf_id;
        self.next_buf_id = self.next_buf_id.wrapping_add(1).max(1);
        self.buffers.insert(id, buf);
        id
    }

    pub fn tick_headless_cursors(&mut self) {
        if self.hardware_clocked {
            return;
        }
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_advance).as_secs_f64();
        self.last_advance = now;
        if elapsed <= 0.0 {
            return;
        }
        for buf in self.buffers.values_mut() {
            if !buf.playing || buf.is_primary {
                continue;
            }
            let total = buf.total_frames();
            if total == 0 {
                continue;
            }
            buf.play_pos_frames += elapsed * f64::from(buf.sample_rate.max(1));
            let total_f = total as f64;
            if buf.play_pos_frames >= total_f {
                if buf.looping {
                    buf.play_pos_frames %= total_f;
                } else {
                    buf.playing = false;
                    buf.play_pos_frames = 0.0;
                }
            }
        }
    }

    pub fn push_aux_pcm(&mut self, samples: &[i16], src_rate: u32, src_channels: u16) {
        if samples.is_empty() || src_rate == 0 || src_channels == 0 {
            return;
        }
        let ch = usize::from(src_channels);
        let src_frames = samples.len() / ch;
        if src_frames == 0 {
            return;
        }

        let out_rate = self.output_sample_rate.max(1);
        if src_rate == out_rate {
            self.aux_resample_frac = 0.0;
            let new_samples = src_frames * OUTPUT_CHANNELS;
            if new_samples >= MAX_AUX_PCM_SAMPLES {
                self.aux_pcm.clear();
            } else {
                while self.aux_pcm.len() + new_samples > MAX_AUX_PCM_SAMPLES {
                    self.aux_pcm.pop_front();
                    self.aux_pcm.pop_front();
                }
            }
            if ch == 2 {
                let take = new_samples.min(MAX_AUX_PCM_SAMPLES);
                let skip = (src_frames * 2).saturating_sub(take);
                self.aux_pcm.extend(samples[skip..src_frames * 2].iter().copied());
            } else if ch == 1 {
                let max_frames = MAX_AUX_PCM_SAMPLES / OUTPUT_CHANNELS;
                let skip = src_frames.saturating_sub(max_frames);
                for &s in &samples[skip..src_frames] {
                    self.aux_pcm.push_back(s);
                    self.aux_pcm.push_back(s);
                }
            } else {
                let max_frames = MAX_AUX_PCM_SAMPLES / OUTPUT_CHANNELS;
                let skip = src_frames.saturating_sub(max_frames);
                for f in skip..src_frames {
                    let base = f * ch;
                    self.aux_pcm.push_back(samples[base]);
                    self.aux_pcm.push_back(samples[base + 1]);
                }
            }
            return;
        }

        let step = f64::from(src_rate) / f64::from(out_rate);
        let mut pos = self.aux_resample_frac;
        while (pos as usize) < src_frames {
            let f0 = pos as usize;
            let f1 = (f0 + 1).min(src_frames - 1);
            let frac = (pos - f0 as f64) as f32;

            let (l0, r0) = read_i16_stereo_frame(samples, f0, ch);
            let (l1, r1) = read_i16_stereo_frame(samples, f1, ch);
            let l = (l0 + (l1 - l0) * frac).round().clamp(-32_768.0, 32_767.0) as i16;
            let r = (r0 + (r1 - r0) * frac).round().clamp(-32_768.0, 32_767.0) as i16;

            while self.aux_pcm.len() + 2 > MAX_AUX_PCM_SAMPLES {
                self.aux_pcm.pop_front();
                self.aux_pcm.pop_front();
            }
            self.aux_pcm.push_back(l);
            self.aux_pcm.push_back(r);
            pos += step;
        }
        self.aux_resample_frac = (pos - src_frames as f64).max(0.0);
    }

    pub fn mix_into(&mut self, out: &mut [i16]) {
        let now = Instant::now();
        self.last_advance = now;
        self.last_audio_callback = now;
        let out_frames = out.len() / OUTPUT_CHANNELS;
        if out_frames == 0 {
            return;
        }
        let accum_len = out_frames * OUTPUT_CHANNELS;
        self.mix_accum.resize(accum_len, 0.0);
        self.mix_accum[..accum_len].fill(0.0);

        let out_rate_u32 = self.output_sample_rate.max(1);
        let out_rate = f64::from(out_rate_u32);
        let listener = self.listener.clone();
        for buf in self.buffers.values_mut() {
            if !buf.playing || buf.is_primary {
                continue;
            }
            let total_frames = buf.total_frames();
            if total_frames == 0 {
                buf.playing = false;
                continue;
            }

            let (gain_l, gain_r) = compute_channel_gains(buf, &listener);
            if buf.sample_rate == out_rate_u32 && buf.bits_per_sample == 16 {
                mix_pcm_16_1to1(buf, total_frames, out_frames, gain_l, gain_r, &mut self.mix_accum);
                continue;
            }

            let step = f64::from(buf.sample_rate.max(1)) / out_rate;
            let channels = buf.channels;
            let bits = buf.bits_per_sample;
            let total_f = total_frames as f64;

            for frame_out in 0..out_frames {
                if !buf.playing {
                    break;
                }
                if buf.play_pos_frames >= total_f {
                    if buf.looping {
                        buf.play_pos_frames %= total_f;
                    } else {
                        buf.playing = false;
                        buf.play_pos_frames = 0.0;
                        break;
                    }
                }

                let f0 = (buf.play_pos_frames as usize).min(total_frames - 1);
                let frac = (buf.play_pos_frames - f0 as f64) as f32;
                let f1 = if f0 + 1 < total_frames {
                    f0 + 1
                } else if buf.looping {
                    0
                } else {
                    f0
                };

                let (l0, r0) = read_pcm_frame(&buf.data, f0, channels, bits);
                let (l1, r1) = read_pcm_frame(&buf.data, f1, channels, bits);
                let l = l0 + (l1 - l0) * frac;
                let r = r0 + (r1 - r0) * frac;

                self.mix_accum[frame_out * 2] += l * gain_l;
                self.mix_accum[frame_out * 2 + 1] += r * gain_r;

                buf.play_pos_frames += step;
                if buf.play_pos_frames >= total_f {
                    if buf.looping {
                        buf.play_pos_frames %= total_f;
                    } else {
                        buf.playing = false;
                        buf.play_pos_frames = 0.0;
                    }
                }
            }
        }

        self.mix_aux_pcm(out_frames);

        for (dst, &sample) in out.iter_mut().zip(self.mix_accum[..accum_len].iter()) {
            let limited = soft_limit_sample(sample);
            let scaled = (limited * 32_768.0)
                .round()
                .clamp(f32::from(i16::MIN), f32::from(i16::MAX));
            *dst = scaled as i16;
        }

        if dsound_perf_enabled() && now.duration_since(self.last_perf_log) >= DSOUND_PERF_LOG_INTERVAL
        {
            self.last_perf_log = now;
            let playing = self
                .buffers
                .values()
                .filter(|b| b.playing && !b.is_primary)
                .count();
            eprintln!(
                "[toluca-dsound] playing={} total={} aux_frames={}",
                playing,
                self.buffers.len(),
                self.aux_played_frames
            );
        }
    }

    fn mix_aux_pcm(&mut self, out_frames: usize) {
        let accum_len = out_frames * OUTPUT_CHANNELS;
        if (self.aux_rate_ratio - 1.0).abs() < AUX_RATE_UNITY_EPSILON && self.aux_play_frac == 0.0
        {
            let mut popped_aux = 0usize;
            for slot in self.mix_accum[..accum_len].iter_mut() {
                let Some(aux) = self.aux_pcm.pop_front() else {
                    break;
                };
                *slot += f32::from(aux) / 32_768.0;
                popped_aux += 1;
            }
            let popped_frames = (popped_aux / OUTPUT_CHANNELS) as u64;
            self.aux_played_frames += popped_frames;
            self.last_aux_popped_frames = popped_frames;
            return;
        }

        let mut total_popped_frames = 0u64;
        for frame_out in 0..out_frames {
            if self.aux_pcm.len() < OUTPUT_CHANNELS {
                break;
            }
            let l0 = f32::from(self.aux_pcm[0]);
            let r0 = f32::from(self.aux_pcm[1]);
            let (l1, r1) = if self.aux_pcm.len() >= 2 * OUTPUT_CHANNELS {
                (f32::from(self.aux_pcm[2]), f32::from(self.aux_pcm[3]))
            } else {
                (l0, r0)
            };
            let t = self.aux_play_frac as f32;
            let l = l0 + (l1 - l0) * t;
            let r = r0 + (r1 - r0) * t;
            let out_idx = frame_out * OUTPUT_CHANNELS;
            self.mix_accum[out_idx] += l / 32_768.0;
            self.mix_accum[out_idx + 1] += r / 32_768.0;

            self.aux_play_frac += self.aux_rate_ratio;
            let whole = self.aux_play_frac.floor() as usize;
            self.aux_play_frac -= whole as f64;
            let frames_to_pop = whole.min(self.aux_pcm.len() / OUTPUT_CHANNELS);
            for _ in 0..(frames_to_pop * OUTPUT_CHANNELS) {
                self.aux_pcm.pop_front();
            }
            let popped_frames = frames_to_pop as u64;
            self.aux_played_frames += popped_frames;
            total_popped_frames += popped_frames;
        }
        self.last_aux_popped_frames = total_popped_frames;
        if self.aux_pcm.len() < OUTPUT_CHANNELS {
            self.aux_play_frac = 0.0;
        }
    }
}

fn mix_pcm_16_1to1(
    buf: &mut DsoundBufferData,
    total_frames: usize,
    out_frames: usize,
    gain_l: f32,
    gain_r: f32,
    accum: &mut [f32],
) {
    let mut pos = if (buf.play_pos_frames as usize) >= total_frames {
        if buf.looping {
            (buf.play_pos_frames as usize) % total_frames
        } else {
            buf.playing = false;
            buf.play_pos_frames = 0.0;
            return;
        }
    } else {
        buf.play_pos_frames as usize
    };

    let ch = usize::from(buf.channels.max(1));
    let scale_l = gain_l / 32_768.0;
    let scale_r = gain_r / 32_768.0;
    let mut out_idx = 0usize;
    let mut remaining = out_frames;

    while remaining > 0 {
        let chunk = remaining.min(total_frames - pos);
        if ch >= 2 {
            let mut src_off = pos * ch * 2;
            let step = ch * 2;
            for i in 0..chunk {
                let l = f32::from(i16::from_le_bytes([buf.data[src_off], buf.data[src_off + 1]]));
                let r =
                    f32::from(i16::from_le_bytes([buf.data[src_off + 2], buf.data[src_off + 3]]));
                let dst = (out_idx + i) * 2;
                accum[dst] += l * scale_l;
                accum[dst + 1] += r * scale_r;
                src_off += step;
            }
        } else {
            let mut src_off = pos * 2;
            for i in 0..chunk {
                let s = f32::from(i16::from_le_bytes([buf.data[src_off], buf.data[src_off + 1]]));
                let dst = (out_idx + i) * 2;
                accum[dst] += s * scale_l;
                accum[dst + 1] += s * scale_r;
                src_off += 2;
            }
        }
        pos += chunk;
        out_idx += chunk;
        remaining -= chunk;
        if pos >= total_frames {
            if buf.looping {
                pos = 0;
            } else {
                buf.playing = false;
                buf.play_pos_frames = 0.0;
                return;
            }
        }
    }
    buf.play_pos_frames = pos as f64;
}

fn soft_limit_sample(sample: f32) -> f32 {
    let mag = sample.abs();
    if mag <= SOFT_LIMIT_KNEE {
        sample
    } else {
        let headroom = 1.0 - SOFT_LIMIT_KNEE;
        let compressed = SOFT_LIMIT_KNEE + headroom * ((mag - SOFT_LIMIT_KNEE) / headroom).tanh();
        sample.signum() * compressed
    }
}

fn read_i16_stereo_frame(samples: &[i16], frame: usize, channels: usize) -> (f32, f32) {
    let base = frame * channels;
    let l = f32::from(samples[base]);
    let r = if channels >= 2 {
        f32::from(samples[base + 1])
    } else {
        l
    };
    (l, r)
}

fn read_pcm_frame(data: &[u8], frame: usize, channels: u16, bits: u16) -> (f32, f32) {
    let ch = usize::from(channels.max(1));
    if bits <= 8 {
        let idx = frame * ch;
        let l = (f32::from(data[idx]) - 128.0) / 128.0;
        let r = if ch >= 2 {
            (f32::from(data[idx + 1]) - 128.0) / 128.0
        } else {
            l
        };
        (l, r)
    } else {
        let idx = frame * ch * 2;
        let l_raw = i16::from_le_bytes([data[idx], data[idx + 1]]);
        let l = f32::from(l_raw) / 32_768.0;
        let r = if ch >= 2 {
            let r_raw = i16::from_le_bytes([data[idx + 2], data[idx + 3]]);
            f32::from(r_raw) / 32_768.0
        } else {
            l
        };
        (l, r)
    }
}

pub fn db_to_linear(db: i32) -> f32 {
    if db <= DSBVOLUME_MIN {
        0.0
    } else if db >= DSBVOLUME_MAX {
        1.0
    } else {
        10f32.powf(db as f32 / 2000.0)
    }
}

fn compute_channel_gains(buf: &DsoundBufferData, listener: &Ds3dListenerParams) -> (f32, f32) {
    let vol_gain = db_to_linear(buf.volume_db);
    let mut left = vol_gain;
    let mut right = vol_gain;

    if buf.pan_db < 0 {
        right *= db_to_linear(buf.pan_db.max(DSBPAN_LEFT));
    } else if buf.pan_db > 0 {
        left *= db_to_linear((-buf.pan_db).max(DSBPAN_LEFT));
    }

    if buf.ds3d.is_3d && buf.ds3d.mode != DS3DMODE_DISABLE {
        let rel = if buf.ds3d.mode == DS3DMODE_HEADRELATIVE {
            buf.ds3d.pos
        } else {
            [
                buf.ds3d.pos[0] - listener.pos[0],
                buf.ds3d.pos[1] - listener.pos[1],
                buf.ds3d.pos[2] - listener.pos[2],
            ]
        };
        let raw_dist = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
        let dist = raw_dist * listener.distance_factor.max(0.0001);
        let min_d = buf.ds3d.min_distance.max(0.001);
        let max_d = buf.ds3d.max_distance.max(min_d);
        let clamped_d = dist.clamp(min_d, max_d);
        let rolloff = listener.rolloff_factor.max(0.0);
        let atten = min_d / (min_d + rolloff * (clamped_d - min_d));

        let mut pan_x = 0.0f32;
        if raw_dist > 1e-6 {
            let right_axis = normalize3(cross3(listener.top, listener.front));
            let dir = [rel[0] / raw_dist, rel[1] / raw_dist, rel[2] / raw_dist];
            pan_x = dot3(dir, right_axis).clamp(-1.0, 1.0);
        }

        let pan_l = 1.0 - pan_x.max(0.0);
        let pan_r = 1.0 + pan_x.min(0.0);
        left *= atten * pan_l;
        right *= atten * pan_r;
    }

    (left, right)
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = dot3(v, v).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [1.0, 0.0, 0.0]
    }
}

#[derive(Clone, Debug)]
struct DsoundBufferGuestMeta {
    mixer_id: usize,
    guest_buf_va: GuestAddr,
    guest_buf_bytes: u32,
    refcount: u32,
}

pub struct DsoundState {
    pub mixer: Arc<Mutex<DsoundMixer>>,
    buffers: HashMap<GuestAddr, DsoundBufferGuestMeta>,
    ds3d_buffers: HashMap<GuestAddr, (usize, u32)>,
    audio_device: Option<Sdl2AudioDevice>,
    audio_open_attempted: bool,
}

impl Default for DsoundState {
    fn default() -> Self {
        Self::new()
    }
}

impl DsoundState {
    pub fn new() -> Self {
        Self {
            mixer: Arc::new(Mutex::new(DsoundMixer::new())),
            buffers: HashMap::new(),
            ds3d_buffers: HashMap::new(),
            audio_device: None,
            audio_open_attempted: false,
        }
    }

    pub fn ensure_audio_opened(&mut self, sdl: Option<&mut crate::win32::sdl2::Sdl2WindowBackend>) {
        if self.audio_open_attempted {
            return;
        }
        self.audio_open_attempted = true;
        let Some(sdl) = sdl else {
            return;
        };
        if let Some(dev) = sdl.open_audio_device(sdl_audio_callback, Arc::clone(&self.mixer)) {
            if let Ok(mut guard) = self.mixer.lock() {
                guard.hardware_clocked = true;
                guard.output_sample_rate = dev.obtained_freq();
                guard.output_buffer_frames = dev.obtained_samples();
            }
            self.audio_device = Some(dev);
        }
    }
}

unsafe extern "C" fn sdl_audio_callback(userdata: *mut c_void, stream: *mut u8, len: i32) {
    if userdata.is_null() || stream.is_null() || len <= 0 {
        return;
    }
    let mixer = unsafe { &*userdata.cast::<Arc<Mutex<DsoundMixer>>>() };
    let total_samples = (len as usize) / std::mem::size_of::<i16>();
    let mut chunk = [0i16; 4096];
    let mut offset = 0usize;

    let Ok(mut guard) = mixer.lock() else {
        unsafe {
            std::ptr::write_bytes(stream, 0, total_samples * 2);
        }
        return;
    };

    let start_aux_played = guard.aux_played_frames;
    while offset < total_samples {
        let count = (total_samples - offset).min(chunk.len());
        chunk[..count].fill(0);
        guard.mix_into(&mut chunk[..count]);
        unsafe {
            std::ptr::copy_nonoverlapping(
                chunk.as_ptr().cast::<u8>(),
                stream.add(offset * 2),
                count * 2,
            );
        }
        offset += count;
    }
    guard.last_aux_popped_frames = guard.aux_played_frames.saturating_sub(start_aux_played);
    guard.last_audio_callback = Instant::now();
}

pub fn register_imports(registry: &mut ImportRegistry) {
    registry.register_ordinal("dsound.dll", 11, direct_sound_create_8);
    registry.register_name("dsound.dll", "DirectSoundCreate8", direct_sound_create_8);
    registry.register_name("dsound.dll", "DirectSoundCreate", direct_sound_create_8);

    let ds_com_entries: &[(&str, ImportHandler)] = &[
        ("QueryInterface", ds_query_interface),
        ("AddRef", stdcall_one_1),
        ("Release", stdcall_zero_1),
        ("DSB_AddRef", dsb_add_ref),
        ("DSB_Release", dsb_release),
        ("DS3DB_AddRef", ds3db_add_ref),
        ("DS3DB_Release", ds3db_release),
        ("DS_CreateSoundBuffer", ds_create_sound_buffer),
        ("DS_GetCaps", ds_get_caps),
        ("DS_DuplicateSoundBuffer", ds_duplicate_sound_buffer),
        ("DS_GetSpeakerConfig", ds_write_zero_2),
        ("DS_VerifyCertification", ds_write_zero_2),
        ("DSB_GetCaps", dsb_get_caps),
        ("DSB_GetCurrentPosition", dsb_get_current_position),
        ("DSB_GetFormat", dsb_get_format),
        ("DSB_GetVolume", dsb_get_volume),
        ("DSB_GetPan", dsb_get_pan),
        ("DSB_GetFrequency", dsb_get_frequency),
        ("DSB_GetStatus", dsb_get_status),
        ("DSB_Lock", dsb_lock),
        ("DSB_Play", dsb_play),
        ("DSB_SetCurrentPosition", dsb_set_current_position),
        ("DSB_SetFormat", dsb_set_format),
        ("DSB_SetVolume", dsb_set_volume),
        ("DSB_SetPan", dsb_set_pan),
        ("DSB_SetFrequency", dsb_set_frequency),
        ("DSB_Stop", dsb_stop),
        ("DSB_Unlock", dsb_unlock),
        ("DS3DL_GetAllParameters", ds3dl_get_all_parameters),
        ("DS3DL_GetDistanceFactor", ds3dl_get_distance_factor),
        ("DS3DL_GetDopplerFactor", ds3dl_get_doppler_factor),
        ("DS3DL_GetOrientation", ds3dl_get_orientation),
        ("DS3DL_GetPosition", ds3dl_get_position),
        ("DS3DL_GetRolloffFactor", ds3dl_get_rolloff_factor),
        ("DS3DL_GetVelocity", ds3dl_get_velocity),
        ("DS3DL_SetAllParameters", ds3dl_set_all_parameters),
        ("DS3DL_SetDistanceFactor", ds3dl_set_distance_factor),
        ("DS3DL_SetDopplerFactor", ds3dl_set_doppler_factor),
        ("DS3DL_SetOrientation", ds3dl_set_orientation),
        ("DS3DL_SetPosition", ds3dl_set_position),
        ("DS3DL_SetRolloffFactor", ds3dl_set_rolloff_factor),
        ("DS3DL_SetVelocity", ds3dl_set_velocity),
        ("DS3DL_CommitDeferredSettings", ds3dl_commit_deferred_settings),
        ("DS3DB_GetAllParameters", ds3db_get_all_parameters),
        ("DS3DB_GetConeAngles", ds3db_get_cone_angles),
        ("DS3DB_GetConeOrientation", ds3db_get_cone_orientation),
        ("DS3DB_GetConeOutsideVolume", ds3db_get_cone_outside_volume),
        ("DS3DB_GetMaxDistance", ds3db_get_max_distance),
        ("DS3DB_GetMinDistance", ds3db_get_min_distance),
        ("DS3DB_GetMode", ds3db_get_mode),
        ("DS3DB_GetPosition", ds3db_get_position),
        ("DS3DB_GetVelocity", ds3db_get_velocity),
        ("DS3DB_SetAllParameters", ds3db_set_all_parameters),
        ("DS3DB_SetConeAngles", ds3db_set_cone_angles),
        ("DS3DB_SetConeOrientation", ds3db_set_cone_orientation),
        ("DS3DB_SetConeOutsideVolume", ds3db_set_cone_outside_volume),
        ("DS3DB_SetMaxDistance", ds3db_set_max_distance),
        ("DS3DB_SetMinDistance", ds3db_set_min_distance),
        ("DS3DB_SetMode", ds3db_set_mode),
        ("DS3DB_SetPosition", ds3db_set_position),
        ("DS3DB_SetVelocity", ds3db_set_velocity),
        ("Std0_1", stdcall_zero_1),
        ("Std0_2", stdcall_zero_2),
        ("Std0_3", stdcall_zero_3),
        ("Std0_4", stdcall_zero_4),
    ];
    for &(name, handler) in ds_com_entries {
        registry.register_name(DSOUND_COM_DLL, name, handler);
    }
}

fn alloc_com_object(
    ctx: &mut RuntimeContext<'_>,
    slot_names: &[&str],
) -> Result<GuestAddr, RuntimeError> {
    let vtable = ctx.heap.alloc(ctx.arena, slot_names.len() * 4, true)?;
    for (idx, &name) in slot_names.iter().enumerate() {
        let thunk_va = ctx
            .register_dynamic_import(DSOUND_COM_DLL, ImportSymbol::Name(name.to_owned()))
            .unwrap_or(0);
        ctx.arena.write_u32(vtable + (idx as u32) * 4, thunk_va)?;
    }
    let obj = ctx.heap.alloc(ctx.arena, 4, true)?;
    ctx.arena.write_u32(obj, vtable)?;
    Ok(obj)
}

fn direct_sound_create_8(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let pp_ds = peek_arg_u32(cpu, ctx.arena, 1)?;
    if pp_ds == 0 {
        return_stdcall(cpu, ctx.arena, 3, DSERR_INVALIDPARAM)?;
        return Ok(());
    }
    ctx.win32
        .dsound
        .ensure_audio_opened(ctx.win32.window_mgr.sdl.as_mut());

    const DS8_SLOTS: [&str; DS8_VTABLE_SLOTS] = [
        "QueryInterface",
        "AddRef",
        "Release",
        "DS_CreateSoundBuffer",
        "DS_GetCaps",
        "DS_DuplicateSoundBuffer",
        "Std0_3",
        "Std0_1",
        "DS_GetSpeakerConfig",
        "Std0_2",
        "Std0_2",
        "DS_VerifyCertification",
    ];
    let obj = alloc_com_object(ctx, &DS8_SLOTS)?;
    ctx.arena.write_u32(pp_ds, obj)?;
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn alloc_ds3d_listener(ctx: &mut RuntimeContext<'_>) -> Result<GuestAddr, RuntimeError> {
    const SLOTS: [&str; DS3DLISTENER_VTABLE_SLOTS] = [
        "QueryInterface",
        "AddRef",
        "Release",
        "DS3DL_GetAllParameters",
        "DS3DL_GetDistanceFactor",
        "DS3DL_GetDopplerFactor",
        "DS3DL_GetOrientation",
        "DS3DL_GetPosition",
        "DS3DL_GetRolloffFactor",
        "DS3DL_GetVelocity",
        "DS3DL_SetAllParameters",
        "DS3DL_SetDistanceFactor",
        "DS3DL_SetDopplerFactor",
        "DS3DL_SetOrientation",
        "DS3DL_SetPosition",
        "DS3DL_SetRolloffFactor",
        "DS3DL_SetVelocity",
        "DS3DL_CommitDeferredSettings",
    ];
    alloc_com_object(ctx, &SLOTS)
}

fn alloc_ds3d_buffer(
    ctx: &mut RuntimeContext<'_>,
    mixer_id: usize,
) -> Result<GuestAddr, RuntimeError> {
    const SLOTS: [&str; DS3DBUFFER_VTABLE_SLOTS] = [
        "QueryInterface",
        "DS3DB_AddRef",
        "DS3DB_Release",
        "DS3DB_GetAllParameters",
        "DS3DB_GetConeAngles",
        "DS3DB_GetConeOrientation",
        "DS3DB_GetConeOutsideVolume",
        "DS3DB_GetMaxDistance",
        "DS3DB_GetMinDistance",
        "DS3DB_GetMode",
        "DS3DB_GetPosition",
        "DS3DB_GetVelocity",
        "DS3DB_SetAllParameters",
        "DS3DB_SetConeAngles",
        "DS3DB_SetConeOrientation",
        "DS3DB_SetConeOutsideVolume",
        "DS3DB_SetMaxDistance",
        "DS3DB_SetMinDistance",
        "DS3DB_SetMode",
        "DS3DB_SetPosition",
        "DS3DB_SetVelocity",
    ];
    let obj = alloc_com_object(ctx, &SLOTS)?;
    ctx.win32.dsound.ds3d_buffers.insert(obj, (mixer_id, 1));
    Ok(obj)
}

fn ds_query_interface(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let riid = peek_arg_u32(cpu, ctx.arena, 1)?;
    let ppv = peek_arg_u32(cpu, ctx.arena, 2)?;
    if ppv == 0 {
        return_stdcall(cpu, ctx.arena, 3, DSERR_INVALIDPARAM)?;
        return Ok(());
    }
    let data1 = if riid != 0 {
        ctx.arena.read_u32(riid).unwrap_or(0)
    } else {
        0
    };
    let result_obj = match data1 {
        IID_IUNKNOWN_DATA1
        | IID_IDIRECTSOUND_DATA1
        | IID_IDIRECTSOUND8_DATA1
        | IID_IDIRECTSOUNDBUFFER_DATA1
        | IID_IDIRECTSOUNDBUFFER8_DATA1 => {
            if let Some(meta) = ctx.win32.dsound.buffers.get_mut(&this_ptr) {
                meta.refcount = meta.refcount.saturating_add(1);
            } else if let Some((_, refcnt)) = ctx.win32.dsound.ds3d_buffers.get_mut(&this_ptr) {
                *refcnt = refcnt.saturating_add(1);
            }
            Some(this_ptr)
        }
        IID_IDIRECTSOUND3DLISTENER_DATA1 => Some(alloc_ds3d_listener(ctx)?),
        IID_IDIRECTSOUND3DBUFFER_DATA1 => {
            if let Some((_, refcnt)) = ctx.win32.dsound.ds3d_buffers.get_mut(&this_ptr) {
                *refcnt = refcnt.saturating_add(1);
                Some(this_ptr)
            } else {
                let mixer_id = ctx
                    .win32
                    .dsound
                    .buffers
                    .get(&this_ptr)
                    .map_or(0, |m| m.mixer_id);
                Some(alloc_ds3d_buffer(ctx, mixer_id)?)
            }
        }
        _ => None,
    };
    match result_obj {
        Some(obj) => {
            ctx.arena.write_u32(ppv, obj)?;
            return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
        }
        None => {
            ctx.arena.write_u32(ppv, 0)?;
            return_stdcall(cpu, ctx.arena, 3, DSERR_NOINTERFACE)?;
        }
    }
    Ok(())
}

fn alloc_ds_buffer_object(
    ctx: &mut RuntimeContext<'_>,
    mixer_buf: DsoundBufferData,
) -> Result<GuestAddr, RuntimeError> {
    const DSB8_SLOTS: [&str; DSB8_VTABLE_SLOTS] = [
        "QueryInterface",
        "DSB_AddRef",
        "DSB_Release",
        "DSB_GetCaps",
        "DSB_GetCurrentPosition",
        "DSB_GetFormat",
        "DSB_GetVolume",
        "DSB_GetPan",
        "DSB_GetFrequency",
        "DSB_GetStatus",
        "Std0_3",
        "DSB_Lock",
        "DSB_Play",
        "DSB_SetCurrentPosition",
        "DSB_SetFormat",
        "DSB_SetVolume",
        "DSB_SetPan",
        "DSB_SetFrequency",
        "DSB_Stop",
        "DSB_Unlock",
        "Std0_1",
        "Std0_4",
    ];
    let guest_buf_bytes = mixer_buf.data.len() as u32;
    let guest_buf_va = if mixer_buf.is_primary || guest_buf_bytes == 0 {
        0
    } else {
        ctx.heap.alloc(ctx.arena, guest_buf_bytes as usize, true)?
    };
    let mixer_id = ctx
        .win32
        .dsound
        .mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .alloc_buffer(mixer_buf);
    let obj = alloc_com_object(ctx, &DSB8_SLOTS)?;
    ctx.win32.dsound.buffers.insert(
        obj,
        DsoundBufferGuestMeta {
            mixer_id,
            guest_buf_va,
            guest_buf_bytes,
            refcount: 1,
        },
    );
    Ok(obj)
}

fn read_wave_format_ex(arena: &GuestArena, wfx_va: GuestAddr) -> Result<(u32, u16, u16), u32> {
    let tag = arena.read_u16(wfx_va).map_err(|_| DSERR_INVALIDPARAM)?;
    let channels = arena.read_u16(wfx_va + 2).map_err(|_| DSERR_INVALIDPARAM)?;
    let sample_rate = arena.read_u32(wfx_va + 4).map_err(|_| DSERR_INVALIDPARAM)?;
    let bits = arena.read_u16(wfx_va + 14).map_err(|_| DSERR_INVALIDPARAM)?;
    if tag != WAVE_FORMAT_PCM || !(1..=2).contains(&channels) || sample_rate == 0 || (bits != 8 && bits != 16) {
        return Err(DSERR_BADFORMAT);
    }
    Ok((sample_rate, channels, bits))
}

fn ds_create_sound_buffer(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let desc_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let pp_buf = peek_arg_u32(cpu, ctx.arena, 2)?;
    if desc_ptr == 0 || pp_buf == 0 {
        return_stdcall(cpu, ctx.arena, 4, DSERR_INVALIDPARAM)?;
        return Ok(());
    }

    let dw_flags = ctx.arena.read_u32(desc_ptr + 4)?;
    let raw_bytes = ctx.arena.read_u32(desc_ptr + 8)?;
    let lpwfx = ctx.arena.read_u32(desc_ptr + 16)?;
    let is_primary = (dw_flags & DSBCAPS_PRIMARYBUFFER) != 0;

    let (sample_rate, channels, bits_per_sample) = if lpwfx != 0 {
        match read_wave_format_ex(ctx.arena, lpwfx) {
            Ok(fmt) => fmt,
            Err(hr) => {
                ctx.arena.write_u32(pp_buf, 0)?;
                return_stdcall(cpu, ctx.arena, 4, hr)?;
                return Ok(());
            }
        }
    } else {
        (DEFAULT_OUTPUT_SAMPLE_RATE, 2, 16)
    };

    let size = if is_primary {
        0
    } else if raw_bytes == 0 {
        DEFAULT_BUFFER_BYTES
    } else {
        raw_bytes.clamp(1, MAX_BUFFER_BYTES)
    };
    let alloc_len = if is_primary { 0 } else { size.clamp(MIN_BUFFER_BYTES, MAX_BUFFER_BYTES) };
    let data = vec![if bits_per_sample <= 8 { 0x80 } else { 0 }; alloc_len as usize];

    let mixer_buf = DsoundBufferData {
        data,
        sample_rate,
        channels,
        bits_per_sample,
        playing: false,
        looping: false,
        is_primary,
        flags: dw_flags,
        play_pos_frames: 0.0,
        volume_db: DSBVOLUME_MAX,
        pan_db: 0,
        ds3d: Ds3dBufferParams {
            is_3d: (dw_flags & DSBCAPS_CTRL3D) != 0,
            ..Ds3dBufferParams::default()
        },
    };

    let obj = alloc_ds_buffer_object(ctx, mixer_buf)?;
    ctx.arena.write_u32(pp_buf, obj)?;
    return_stdcall(cpu, ctx.arena, 4, DS_OK)?;
    Ok(())
}

fn ds_duplicate_sound_buffer(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let orig = peek_arg_u32(cpu, ctx.arena, 1)?;
    let pp_dup = peek_arg_u32(cpu, ctx.arena, 2)?;
    if orig == 0 || pp_dup == 0 {
        return_stdcall(cpu, ctx.arena, 3, DSERR_INVALIDPARAM)?;
        return Ok(());
    }
    let Some(meta) = ctx.win32.dsound.buffers.get(&orig).cloned() else {
        return_stdcall(cpu, ctx.arena, 3, DSERR_GENERIC)?;
        return Ok(());
    };
    let mut cloned_buf = {
        let guard = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(src) = guard.buffers.get(&meta.mixer_id) else {
            return_stdcall(cpu, ctx.arena, 3, DSERR_GENERIC)?;
            return Ok(());
        };
        src.clone()
    };
    cloned_buf.playing = false;
    cloned_buf.looping = false;
    cloned_buf.play_pos_frames = 0.0;

    let dup_obj = alloc_ds_buffer_object(ctx, cloned_buf)?;
    let dup_guest_va = ctx
        .win32
        .dsound
        .buffers
        .get(&dup_obj)
        .map_or(0, |m| m.guest_buf_va);
    if dup_guest_va != 0 && meta.guest_buf_va != 0 && meta.guest_buf_bytes > 0 {
        let bytes = ctx
            .arena
            .read_bytes(meta.guest_buf_va, meta.guest_buf_bytes as usize)?
            .to_vec();
        ctx.arena.write_bytes(dup_guest_va, &bytes)?;
    }
    ctx.arena.write_u32(pp_dup, dup_obj)?;
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds_get_caps(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let caps_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if caps_ptr == 0 {
        return_stdcall(cpu, ctx.arena, 2, DSERR_INVALIDPARAM)?;
        return Ok(());
    }
    let dw_size = ctx.arena.read_u32(caps_ptr)?;
    if dw_size > 4 && dw_size <= 256 {
        ctx.arena.zero_range(caps_ptr + 4, (dw_size - 4) as usize)?;
    }
    if dw_size >= 32 {
        ctx.arena.write_u32(caps_ptr + 4, 0x0000_0CF0)?;
        ctx.arena.write_u32(caps_ptr + 8, 100)?;
        ctx.arena.write_u32(caps_ptr + 12, 100_000)?;
        ctx.arena.write_u32(caps_ptr + 16, 1)?;
        ctx.arena.write_u32(caps_ptr + 20, 32)?;
        ctx.arena.write_u32(caps_ptr + 24, 32)?;
        ctx.arena.write_u32(caps_ptr + 28, 32)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds_write_zero_2(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let out_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out_ptr != 0 {
        ctx.arena.write_u32(out_ptr, 0)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn dsb_get_caps(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let caps_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if caps_ptr == 0 {
        return_stdcall(cpu, ctx.arena, 2, DSERR_INVALIDPARAM)?;
        return Ok(());
    }
    let dw_size = ctx.arena.read_u32(caps_ptr)?;
    if dw_size > 4 && dw_size <= 128 {
        ctx.arena.zero_range(caps_ptr + 4, (dw_size - 4) as usize)?;
    }
    if let Some(meta) = ctx.win32.dsound.buffers.get(&this_ptr) {
        let guard = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let flags = guard
            .buffers
            .get(&meta.mixer_id)
            .map_or(DSBCAPS_CTRLVOLUME | DSBCAPS_CTRLPAN | DSBCAPS_CTRLFREQUENCY, |b| b.flags);
        if dw_size >= 12 {
            ctx.arena.write_u32(caps_ptr + 4, flags)?;
            ctx.arena.write_u32(caps_ptr + 8, meta.guest_buf_bytes)?;
        }
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn dsb_get_current_position(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let p_play = peek_arg_u32(cpu, ctx.arena, 1)?;
    let p_write = peek_arg_u32(cpu, ctx.arena, 2)?;
    let (play_ofs, write_ofs) = if let Some(meta) = ctx.win32.dsound.buffers.get(&this_ptr) {
        let mut guard = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.tick_headless_cursors();
        guard
            .buffers
            .get(&meta.mixer_id)
            .map_or((0, 0), |b| (b.play_byte_offset(), b.write_byte_offset()))
    } else {
        (0, 0)
    };
    if p_play != 0 {
        ctx.arena.write_u32(p_play, play_ofs)?;
    }
    if p_write != 0 {
        ctx.arena.write_u32(p_write, write_ofs)?;
    }
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn dsb_get_format(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pwfx = peek_arg_u32(cpu, ctx.arena, 1)?;
    let allocated = peek_arg_u32(cpu, ctx.arena, 2)?;
    let p_written = peek_arg_u32(cpu, ctx.arena, 3)?;
    if p_written != 0 {
        ctx.arena.write_u32(p_written, WAVEFORMATEX_SIZE)?;
    }
    if pwfx != 0 && allocated >= 16 {
        let (rate, ch, bits) = ctx
            .win32
            .dsound
            .buffers
            .get(&this_ptr)
            .and_then(|m| {
                ctx.win32
                    .dsound
                    .mixer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .buffers
                    .get(&m.mixer_id)
                    .map(|b| (b.sample_rate, b.channels, b.bits_per_sample))
            })
            .unwrap_or((DEFAULT_OUTPUT_SAMPLE_RATE, 2, 16));
        let block_align = ch * (bits / 8);
        let avg_bytes = rate * u32::from(block_align);
        ctx.arena.write_u16(pwfx, WAVE_FORMAT_PCM)?;
        ctx.arena.write_u16(pwfx + 2, ch)?;
        ctx.arena.write_u32(pwfx + 4, rate)?;
        ctx.arena.write_u32(pwfx + 8, avg_bytes)?;
        ctx.arena.write_u16(pwfx + 12, block_align)?;
        ctx.arena.write_u16(pwfx + 14, bits)?;
        if allocated >= WAVEFORMATEX_SIZE {
            ctx.arena.write_u16(pwfx + 16, 0)?;
        }
    }
    return_stdcall(cpu, ctx.arena, 4, DS_OK)?;
    Ok(())
}

fn dsb_set_format(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pcfx = peek_arg_u32(cpu, ctx.arena, 1)?;
    if pcfx == 0 {
        return_stdcall(cpu, ctx.arena, 2, DSERR_INVALIDPARAM)?;
        return Ok(());
    }
    let (rate, ch, bits) = match read_wave_format_ex(ctx.arena, pcfx) {
        Ok(fmt) => fmt,
        Err(hr) => {
            return_stdcall(cpu, ctx.arena, 2, hr)?;
            return Ok(());
        }
    };
    if let Some(meta) = ctx.win32.dsound.buffers.get(&this_ptr) {
        let mut guard = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(buf) = guard.buffers.get_mut(&meta.mixer_id) {
            buf.sample_rate = rate;
            buf.channels = ch;
            buf.bits_per_sample = bits;
        }
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn dsb_get_volume(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let p_vol = peek_arg_u32(cpu, ctx.arena, 1)?;
    if p_vol != 0 {
        let vol = with_buf(ctx, this_ptr, |b| b.volume_db).unwrap_or(0);
        ctx.arena.write_u32(p_vol, vol as u32)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn dsb_set_volume(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let vol = peek_arg_i32(cpu, ctx.arena, 1)?.clamp(DSBVOLUME_MIN, DSBVOLUME_MAX);
    with_buf_mut(ctx, this_ptr, |b| b.volume_db = vol);
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn dsb_get_pan(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let p_pan = peek_arg_u32(cpu, ctx.arena, 1)?;
    if p_pan != 0 {
        let pan = with_buf(ctx, this_ptr, |b| b.pan_db).unwrap_or(0);
        ctx.arena.write_u32(p_pan, pan as u32)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn dsb_set_pan(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pan = peek_arg_i32(cpu, ctx.arena, 1)?.clamp(DSBPAN_LEFT, DSBPAN_RIGHT);
    with_buf_mut(ctx, this_ptr, |b| b.pan_db = pan);
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn dsb_get_frequency(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let p_freq = peek_arg_u32(cpu, ctx.arena, 1)?;
    if p_freq != 0 {
        let freq = with_buf(ctx, this_ptr, |b| b.sample_rate).unwrap_or(DEFAULT_OUTPUT_SAMPLE_RATE);
        ctx.arena.write_u32(p_freq, freq)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn dsb_set_frequency(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let freq = peek_arg_u32(cpu, ctx.arena, 1)?;
    if freq != 0 {
        with_buf_mut(ctx, this_ptr, |b| b.sample_rate = freq.clamp(100, 200_000));
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn dsb_get_status(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let p_status = peek_arg_u32(cpu, ctx.arena, 1)?;
    if p_status != 0 {
        let status = if let Some(meta) = ctx.win32.dsound.buffers.get(&this_ptr) {
            let mut guard = ctx
                .win32
                .dsound
                .mixer
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            guard.tick_headless_cursors();
            guard.buffers.get(&meta.mixer_id).map_or(0, |b| {
                let mut s = 0u32;
                if b.playing {
                    s |= DSBSTATUS_PLAYING;
                    if b.looping {
                        s |= DSBSTATUS_LOOPING;
                    }
                }
                s
            })
        } else {
            0
        };
        ctx.arena.write_u32(p_status, status)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn dsb_lock(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let offset = peek_arg_u32(cpu, ctx.arena, 1)?;
    let bytes = peek_arg_u32(cpu, ctx.arena, 2)?;
    let pp1 = peek_arg_u32(cpu, ctx.arena, 3)?;
    let pb1 = peek_arg_u32(cpu, ctx.arena, 4)?;
    let pp2 = peek_arg_u32(cpu, ctx.arena, 5)?;
    let pb2 = peek_arg_u32(cpu, ctx.arena, 6)?;
    let flags = peek_arg_u32(cpu, ctx.arena, 7)?;

    let Some(meta) = ctx.win32.dsound.buffers.get(&this_ptr).cloned() else {
        return_stdcall(cpu, ctx.arena, 8, DSERR_GENERIC)?;
        return Ok(());
    };
    if meta.guest_buf_va == 0 || meta.guest_buf_bytes == 0 {
        return_stdcall(cpu, ctx.arena, 8, DSERR_INVALIDPARAM)?;
        return Ok(());
    }

    let buf_size = meta.guest_buf_bytes;
    let start = if (flags & DSBLOCK_ENTIREBUFFER) != 0 {
        0
    } else if (flags & DSBLOCK_FROMWRITECURSOR) != 0 {
        with_buf(ctx, this_ptr, |b| b.write_byte_offset()).unwrap_or(0) % buf_size
    } else {
        offset % buf_size
    };
    let total = if (flags & DSBLOCK_ENTIREBUFFER) != 0 {
        buf_size
    } else {
        bytes.min(buf_size)
    };
    let len1 = total.min(buf_size - start);
    let len2 = total.saturating_sub(len1);

    if pp1 != 0 {
        ctx.arena.write_u32(pp1, meta.guest_buf_va + start)?;
    }
    if pb1 != 0 {
        ctx.arena.write_u32(pb1, len1)?;
    }
    if pp2 != 0 {
        ctx.arena
            .write_u32(pp2, if len2 > 0 { meta.guest_buf_va } else { 0 })?;
    }
    if pb2 != 0 {
        ctx.arena.write_u32(pb2, len2)?;
    }
    return_stdcall(cpu, ctx.arena, 8, DS_OK)?;
    Ok(())
}

fn dsb_unlock(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pv1 = peek_arg_u32(cpu, ctx.arena, 1)?;
    let len1 = peek_arg_u32(cpu, ctx.arena, 2)?;
    let pv2 = peek_arg_u32(cpu, ctx.arena, 3)?;
    let len2 = peek_arg_u32(cpu, ctx.arena, 4)?;

    let Some(meta) = ctx.win32.dsound.buffers.get(&this_ptr).cloned() else {
        return_stdcall(cpu, ctx.arena, 5, DS_OK)?;
        return Ok(());
    };
    if meta.guest_buf_va == 0 || meta.guest_buf_bytes == 0 {
        return_stdcall(cpu, ctx.arena, 5, DS_OK)?;
        return Ok(());
    }

    let copy_region = |arena: &GuestArena,
                       mixer_buf: &mut DsoundBufferData,
                       ptr: GuestAddr,
                       len: u32|
     -> Result<(), RuntimeError> {
        if ptr == 0 || len == 0 || ptr < meta.guest_buf_va {
            return Ok(());
        }
        let off = (ptr - meta.guest_buf_va) as usize;
        if off >= mixer_buf.data.len() {
            return Ok(());
        }
        let copy_len = (len as usize).min(mixer_buf.data.len() - off);
        let bytes = arena.read_bytes(ptr, copy_len)?;
        mixer_buf.data[off..off + copy_len].copy_from_slice(bytes);
        Ok(())
    };

    let mut guard = ctx
        .win32
        .dsound
        .mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(buf) = guard.buffers.get_mut(&meta.mixer_id) {
        copy_region(ctx.arena, buf, pv1, len1)?;
        copy_region(ctx.arena, buf, pv2, len2)?;
    }
    return_stdcall(cpu, ctx.arena, 5, DS_OK)?;
    Ok(())
}

fn dsb_play(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let flags = peek_arg_u32(cpu, ctx.arena, 3)?;
    let looping = (flags & DSBPLAY_LOOPING) != 0;
    if let Some(meta) = ctx.win32.dsound.buffers.get(&this_ptr) {
        let mut guard = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.tick_headless_cursors();
        if let Some(buf) = guard.buffers.get_mut(&meta.mixer_id) {
            buf.playing = true;
            buf.looping = looping;
        }
    }
    return_stdcall(cpu, ctx.arena, 4, DS_OK)?;
    Ok(())
}

fn dsb_stop(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    with_buf_mut(ctx, this_ptr, |b| {
        b.playing = false;
        b.looping = false;
    });
    return_stdcall(cpu, ctx.arena, 1, DS_OK)?;
    Ok(())
}

fn dsb_set_current_position(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let new_pos_bytes = peek_arg_u32(cpu, ctx.arena, 1)?;
    with_buf_mut(ctx, this_ptr, |b| {
        let total = b.total_frames();
        if total > 0 {
            let fb = b.frame_bytes().max(1);
            let frame = ((new_pos_bytes as usize) / fb) % total;
            b.play_pos_frames = frame as f64;
        } else {
            b.play_pos_frames = 0.0;
        }
    });
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3dl_get_all_parameters(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if ptr == 0 {
        return_stdcall(cpu, ctx.arena, 2, DSERR_INVALIDPARAM)?;
        return Ok(());
    }
    let l = ctx
        .win32
        .dsound
        .mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .listener
        .clone();
    ctx.arena.write_u32(ptr, DS3DLISTENER_SIZE)?;
    write_vec3(ctx.arena, ptr + 0x04, l.pos)?;
    write_vec3(ctx.arena, ptr + 0x10, l.velocity)?;
    write_vec3(ctx.arena, ptr + 0x1C, l.front)?;
    write_vec3(ctx.arena, ptr + 0x28, l.top)?;
    ctx.arena.write_u32(ptr + 0x34, l.distance_factor.to_bits())?;
    ctx.arena.write_u32(ptr + 0x38, l.rolloff_factor.to_bits())?;
    ctx.arena.write_u32(ptr + 0x3C, l.doppler_factor.to_bits())?;
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3dl_get_distance_factor(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let val = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .listener
            .distance_factor;
        ctx.arena.write_u32(out, val.to_bits())?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3dl_get_doppler_factor(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let val = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .listener
            .doppler_factor;
        ctx.arena.write_u32(out, val.to_bits())?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3dl_get_orientation(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let p_front = peek_arg_u32(cpu, ctx.arena, 1)?;
    let p_top = peek_arg_u32(cpu, ctx.arena, 2)?;
    let l = ctx
        .win32
        .dsound
        .mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .listener
        .clone();
    if p_front != 0 {
        write_vec3(ctx.arena, p_front, l.front)?;
    }
    if p_top != 0 {
        write_vec3(ctx.arena, p_top, l.top)?;
    }
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3dl_get_position(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let pos = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .listener
            .pos;
        write_vec3(ctx.arena, out, pos)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3dl_get_rolloff_factor(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let val = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .listener
            .rolloff_factor;
        ctx.arena.write_u32(out, val.to_bits())?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3dl_get_velocity(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let vel = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .listener
            .velocity;
        write_vec3(ctx.arena, out, vel)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3dl_set_all_parameters(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    let apply = peek_arg_u32(cpu, ctx.arena, 2)?;
    if ptr == 0 {
        return_stdcall(cpu, ctx.arena, 3, DSERR_INVALIDPARAM)?;
        return Ok(());
    }
    let next = Ds3dListenerParams {
        pos: read_vec3(ctx.arena, ptr + 0x04)?,
        velocity: read_vec3(ctx.arena, ptr + 0x10)?,
        front: read_vec3(ctx.arena, ptr + 0x1C)?,
        top: read_vec3(ctx.arena, ptr + 0x28)?,
        distance_factor: f32::from_bits(ctx.arena.read_u32(ptr + 0x34)?),
        rolloff_factor: f32::from_bits(ctx.arena.read_u32(ptr + 0x38)?),
        doppler_factor: f32::from_bits(ctx.arena.read_u32(ptr + 0x3C)?),
    };
    let mut guard = ctx
        .win32
        .dsound
        .mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.deferred_listener = next.clone();
    if apply != DS3D_DEFERRED {
        guard.listener = next;
    }
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3dl_set_distance_factor(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let val = f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?);
    let apply = peek_arg_u32(cpu, ctx.arena, 2)?;
    update_listener(ctx, apply, |l| l.distance_factor = val);
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3dl_set_doppler_factor(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let val = f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?);
    let apply = peek_arg_u32(cpu, ctx.arena, 2)?;
    update_listener(ctx, apply, |l| l.doppler_factor = val);
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3dl_set_orientation(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let front = [
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 2)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 3)?),
    ];
    let top = [
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 4)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 5)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 6)?),
    ];
    let apply = peek_arg_u32(cpu, ctx.arena, 7)?;
    update_listener(ctx, apply, |l| {
        l.front = front;
        l.top = top;
    });
    return_stdcall(cpu, ctx.arena, 8, DS_OK)?;
    Ok(())
}

fn ds3dl_set_position(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let pos = [
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 2)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 3)?),
    ];
    let apply = peek_arg_u32(cpu, ctx.arena, 4)?;
    update_listener(ctx, apply, |l| l.pos = pos);
    return_stdcall(cpu, ctx.arena, 5, DS_OK)?;
    Ok(())
}

fn ds3dl_set_rolloff_factor(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let val = f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?);
    let apply = peek_arg_u32(cpu, ctx.arena, 2)?;
    update_listener(ctx, apply, |l| l.rolloff_factor = val);
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3dl_set_velocity(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let vel = [
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 2)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 3)?),
    ];
    let apply = peek_arg_u32(cpu, ctx.arena, 4)?;
    update_listener(ctx, apply, |l| l.velocity = vel);
    return_stdcall(cpu, ctx.arena, 5, DS_OK)?;
    Ok(())
}

fn ds3dl_commit_deferred_settings(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let mut guard = ctx
        .win32
        .dsound
        .mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.listener = guard.deferred_listener.clone();
    return_stdcall(cpu, ctx.arena, 1, DS_OK)?;
    Ok(())
}

fn ds3db_get_all_parameters(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if ptr == 0 {
        return_stdcall(cpu, ctx.arena, 2, DSERR_INVALIDPARAM)?;
        return Ok(());
    }
    let p = with_3dbuf(ctx, this_ptr, |b| b.ds3d.clone()).unwrap_or_default();
    ctx.arena.write_u32(ptr, DS3DBUFFER_SIZE)?;
    write_vec3(ctx.arena, ptr + 0x04, p.pos)?;
    write_vec3(ctx.arena, ptr + 0x10, p.velocity)?;
    ctx.arena.write_u32(ptr + 0x1C, p.inside_cone_angle)?;
    ctx.arena.write_u32(ptr + 0x20, p.outside_cone_angle)?;
    write_vec3(ctx.arena, ptr + 0x24, p.cone_orientation)?;
    ctx.arena.write_u32(ptr + 0x30, p.cone_outside_volume as u32)?;
    ctx.arena.write_u32(ptr + 0x34, p.min_distance.to_bits())?;
    ctx.arena.write_u32(ptr + 0x38, p.max_distance.to_bits())?;
    ctx.arena.write_u32(ptr + 0x3C, p.mode)?;
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3db_get_cone_angles(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let p_in = peek_arg_u32(cpu, ctx.arena, 1)?;
    let p_out = peek_arg_u32(cpu, ctx.arena, 2)?;
    let (a_in, a_out) = with_3dbuf(ctx, this_ptr, |b| {
        (b.ds3d.inside_cone_angle, b.ds3d.outside_cone_angle)
    })
    .unwrap_or((360, 360));
    if p_in != 0 {
        ctx.arena.write_u32(p_in, a_in)?;
    }
    if p_out != 0 {
        ctx.arena.write_u32(p_out, a_out)?;
    }
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3db_get_cone_orientation(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let v = with_3dbuf(ctx, this_ptr, |b| b.ds3d.cone_orientation).unwrap_or([0.0, 0.0, 1.0]);
        write_vec3(ctx.arena, out, v)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3db_get_cone_outside_volume(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let v = with_3dbuf(ctx, this_ptr, |b| b.ds3d.cone_outside_volume).unwrap_or(0);
        ctx.arena.write_u32(out, v as u32)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3db_get_max_distance(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let v = with_3dbuf(ctx, this_ptr, |b| b.ds3d.max_distance).unwrap_or(1_000_000_000.0);
        ctx.arena.write_u32(out, v.to_bits())?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3db_get_min_distance(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let v = with_3dbuf(ctx, this_ptr, |b| b.ds3d.min_distance).unwrap_or(1.0);
        ctx.arena.write_u32(out, v.to_bits())?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3db_get_mode(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let v = with_3dbuf(ctx, this_ptr, |b| b.ds3d.mode).unwrap_or(DS3DMODE_NORMAL);
        ctx.arena.write_u32(out, v)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3db_get_position(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let v = with_3dbuf(ctx, this_ptr, |b| b.ds3d.pos).unwrap_or([0.0; 3]);
        write_vec3(ctx.arena, out, v)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3db_get_velocity(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let out = peek_arg_u32(cpu, ctx.arena, 1)?;
    if out != 0 {
        let v = with_3dbuf(ctx, this_ptr, |b| b.ds3d.velocity).unwrap_or([0.0; 3]);
        write_vec3(ctx.arena, out, v)?;
    }
    return_stdcall(cpu, ctx.arena, 2, DS_OK)?;
    Ok(())
}

fn ds3db_set_all_parameters(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    if ptr == 0 {
        return_stdcall(cpu, ctx.arena, 3, DSERR_INVALIDPARAM)?;
        return Ok(());
    }
    let pos = read_vec3(ctx.arena, ptr + 0x04)?;
    let velocity = read_vec3(ctx.arena, ptr + 0x10)?;
    let inside_cone_angle = ctx.arena.read_u32(ptr + 0x1C)?;
    let outside_cone_angle = ctx.arena.read_u32(ptr + 0x20)?;
    let cone_orientation = read_vec3(ctx.arena, ptr + 0x24)?;
    let cone_outside_volume = ctx.arena.read_u32(ptr + 0x30)? as i32;
    let min_distance = f32::from_bits(ctx.arena.read_u32(ptr + 0x34)?);
    let max_distance = f32::from_bits(ctx.arena.read_u32(ptr + 0x38)?);
    let mode = ctx.arena.read_u32(ptr + 0x3C)?;

    with_3dbuf_mut(ctx, this_ptr, |b| {
        b.ds3d = Ds3dBufferParams {
            pos,
            velocity,
            min_distance,
            max_distance,
            mode,
            is_3d: true,
            inside_cone_angle,
            outside_cone_angle,
            cone_orientation,
            cone_outside_volume,
        };
    });
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3db_set_cone_angles(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let a_in = peek_arg_u32(cpu, ctx.arena, 1)?;
    let a_out = peek_arg_u32(cpu, ctx.arena, 2)?;
    with_3dbuf_mut(ctx, this_ptr, |b| {
        b.ds3d.inside_cone_angle = a_in;
        b.ds3d.outside_cone_angle = a_out;
    });
    return_stdcall(cpu, ctx.arena, 4, DS_OK)?;
    Ok(())
}

fn ds3db_set_cone_orientation(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let orient = [
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 2)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 3)?),
    ];
    with_3dbuf_mut(ctx, this_ptr, |b| b.ds3d.cone_orientation = orient);
    return_stdcall(cpu, ctx.arena, 5, DS_OK)?;
    Ok(())
}

fn ds3db_set_cone_outside_volume(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let vol = peek_arg_i32(cpu, ctx.arena, 1)?;
    with_3dbuf_mut(ctx, this_ptr, |b| b.ds3d.cone_outside_volume = vol);
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3db_set_max_distance(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let dist = f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?);
    with_3dbuf_mut(ctx, this_ptr, |b| b.ds3d.max_distance = dist);
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3db_set_min_distance(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let dist = f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?);
    with_3dbuf_mut(ctx, this_ptr, |b| b.ds3d.min_distance = dist);
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3db_set_mode(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let mode = peek_arg_u32(cpu, ctx.arena, 1)?;
    with_3dbuf_mut(ctx, this_ptr, |b| b.ds3d.mode = mode);
    return_stdcall(cpu, ctx.arena, 3, DS_OK)?;
    Ok(())
}

fn ds3db_set_position(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pos = [
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 2)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 3)?),
    ];
    with_3dbuf_mut(ctx, this_ptr, |b| {
        b.ds3d.pos = pos;
        b.ds3d.is_3d = true;
    });
    return_stdcall(cpu, ctx.arena, 5, DS_OK)?;
    Ok(())
}

fn ds3db_set_velocity(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let vel = [
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 1)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 2)?),
        f32::from_bits(peek_arg_u32(cpu, ctx.arena, 3)?),
    ];
    with_3dbuf_mut(ctx, this_ptr, |b| b.ds3d.velocity = vel);
    return_stdcall(cpu, ctx.arena, 5, DS_OK)?;
    Ok(())
}

fn with_buf<R>(
    ctx: &RuntimeContext<'_>,
    dsb_va: GuestAddr,
    f: impl FnOnce(&DsoundBufferData) -> R,
) -> Option<R> {
    let meta = ctx.win32.dsound.buffers.get(&dsb_va)?;
    let guard = ctx
        .win32
        .dsound
        .mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.buffers.get(&meta.mixer_id).map(f)
}

fn with_buf_mut(
    ctx: &mut RuntimeContext<'_>,
    dsb_va: GuestAddr,
    f: impl FnOnce(&mut DsoundBufferData),
) {
    if let Some(meta) = ctx.win32.dsound.buffers.get(&dsb_va) {
        let mut guard = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(buf) = guard.buffers.get_mut(&meta.mixer_id) {
            f(buf);
        }
    }
}

fn with_3dbuf<R>(
    ctx: &RuntimeContext<'_>,
    ds3d_va: GuestAddr,
    f: impl FnOnce(&DsoundBufferData) -> R,
) -> Option<R> {
    let &(mixer_id, _) = ctx.win32.dsound.ds3d_buffers.get(&ds3d_va)?;
    let guard = ctx
        .win32
        .dsound
        .mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.buffers.get(&mixer_id).map(f)
}

fn with_3dbuf_mut(
    ctx: &mut RuntimeContext<'_>,
    ds3d_va: GuestAddr,
    f: impl FnOnce(&mut DsoundBufferData),
) {
    if let Some(&(mixer_id, _)) = ctx.win32.dsound.ds3d_buffers.get(&ds3d_va) {
        let mut guard = ctx
            .win32
            .dsound
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(buf) = guard.buffers.get_mut(&mixer_id) {
            f(buf);
        }
    }
}

fn dsb_add_ref(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let refcnt = if let Some(meta) = ctx.win32.dsound.buffers.get_mut(&this_ptr) {
        meta.refcount = meta.refcount.saturating_add(1);
        meta.refcount
    } else {
        1
    };
    return_stdcall(cpu, ctx.arena, 1, refcnt)?;
    Ok(())
}

fn dsb_release(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let (rem, removed_meta) = match ctx.win32.dsound.buffers.get_mut(&this_ptr) {
        Some(meta) => {
            meta.refcount = meta.refcount.saturating_sub(1);
            let rem = meta.refcount;
            if rem == 0 {
                (0, ctx.win32.dsound.buffers.remove(&this_ptr))
            } else {
                (rem, None)
            }
        }
        None => (0, None),
    };
    if let Some(meta) = removed_meta {
        if let Ok(mut guard) = ctx.win32.dsound.mixer.lock() {
            guard.buffers.remove(&meta.mixer_id);
        }
        if meta.guest_buf_va != 0 {
            let _ = ctx.heap.free(ctx.arena, meta.guest_buf_va);
        }
    }
    return_stdcall(cpu, ctx.arena, 1, rem)?;
    Ok(())
}

fn ds3db_add_ref(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let refcnt = if let Some((_, rc)) = ctx.win32.dsound.ds3d_buffers.get_mut(&this_ptr) {
        *rc = rc.saturating_add(1);
        *rc
    } else {
        1
    };
    return_stdcall(cpu, ctx.arena, 1, refcnt)?;
    Ok(())
}

fn ds3db_release(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let this_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let rem = if let Some((_, rc)) = ctx.win32.dsound.ds3d_buffers.get_mut(&this_ptr) {
        *rc = rc.saturating_sub(1);
        let rem = *rc;
        if rem == 0 {
            ctx.win32.dsound.ds3d_buffers.remove(&this_ptr);
        }
        rem
    } else {
        0
    };
    return_stdcall(cpu, ctx.arena, 1, rem)?;
    Ok(())
}

fn update_listener(
    ctx: &mut RuntimeContext<'_>,
    apply: u32,
    mut f: impl FnMut(&mut Ds3dListenerParams),
) {
    let mut guard = ctx
        .win32
        .dsound
        .mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    f(&mut guard.deferred_listener);
    if apply != DS3D_DEFERRED {
        f(&mut guard.listener);
    }
}

fn read_vec3(arena: &GuestArena, va: GuestAddr) -> Result<[f32; 3], RuntimeError> {
    Ok([
        f32::from_bits(arena.read_u32(va)?),
        f32::from_bits(arena.read_u32(va + 4)?),
        f32::from_bits(arena.read_u32(va + 8)?),
    ])
}

fn write_vec3(arena: &mut GuestArena, va: GuestAddr, v: [f32; 3]) -> Result<(), RuntimeError> {
    arena.write_u32(va, v[0].to_bits())?;
    arena.write_u32(va + 4, v[1].to_bits())?;
    arena.write_u32(va + 8, v[2].to_bits())?;
    Ok(())
}

fn stdcall_one_1(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn stdcall_zero_1(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn stdcall_zero_2(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, 0)?;
    Ok(())
}

fn stdcall_zero_3(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 3, 0)?;
    Ok(())
}

fn stdcall_zero_4(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 4, 0)?;
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
        fn new() -> Self {
            let mut arena = GuestArena::new().unwrap();
            arena.init_thread_environment(DEFAULT_IMAGE_BASE).unwrap();
            let heap = GuestHeap::new();
            let vmem = VirtualMemoryManager::new();
            let mut registry = ImportRegistry::new();
            register_imports(&mut registry);
            let win32 = Win32State::new(Win32Config {
                headless: true,
                ..Win32Config::default()
            });
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

        fn create_ds8(&mut self) -> GuestAddr {
            let ppds = self.heap.alloc(&mut self.arena, 4, true).unwrap();
            for arg in [0u32, ppds, 0].into_iter() {
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
            direct_sound_create_8(&mut ctx, &mut self.cpu).unwrap();
            assert_eq!(self.cpu.gpr(Gpr::Eax), DS_OK);
            self.arena.read_u32(ppds).unwrap()
        }

        fn create_buffer(&mut self, ds8: GuestAddr, bytes: u32, rate: u32, ch: u16, bits: u16, flags: u32) -> GuestAddr {
            let wfx = self.heap.alloc(&mut self.arena, 20, true).unwrap();
            let block_align = ch * (bits / 8);
            self.arena.write_u16(wfx, WAVE_FORMAT_PCM).unwrap();
            self.arena.write_u16(wfx + 2, ch).unwrap();
            self.arena.write_u32(wfx + 4, rate).unwrap();
            self.arena.write_u32(wfx + 8, rate * u32::from(block_align)).unwrap();
            self.arena.write_u16(wfx + 12, block_align).unwrap();
            self.arena.write_u16(wfx + 14, bits).unwrap();

            let desc = self.heap.alloc(&mut self.arena, 36, true).unwrap();
            self.arena.write_u32(desc, 36).unwrap();
            self.arena.write_u32(desc + 4, flags).unwrap();
            self.arena.write_u32(desc + 8, bytes).unwrap();
            self.arena.write_u32(desc + 16, wfx).unwrap();

            let ppbuf = self.heap.alloc(&mut self.arena, 4, true).unwrap();
            assert_eq!(self.call_com(ds8, 3, &[desc, ppbuf, 0]), DS_OK);
            self.arena.read_u32(ppbuf).unwrap()
        }
    }

    #[test]
    fn mixer_resamples_8bit_and_16bit_pcm_and_clamps_output() {
        let mut mixer = DsoundMixer::new();
        mixer.alloc_buffer(DsoundBufferData {
            data: vec![255, 255, 255, 255],
            sample_rate: DEFAULT_OUTPUT_SAMPLE_RATE / 2,
            channels: 1,
            bits_per_sample: 8,
            playing: true,
            looping: false,
            is_primary: false,
            flags: 0,
            play_pos_frames: 0.0,
            volume_db: 0,
            pan_db: 0,
            ds3d: Ds3dBufferParams::default(),
        });
        let mut s16_bytes = Vec::new();
        for _ in 0..8 {
            s16_bytes.extend_from_slice(&20_000i16.to_le_bytes());
            s16_bytes.extend_from_slice(&(-10_000i16).to_le_bytes());
        }
        mixer.alloc_buffer(DsoundBufferData {
            data: s16_bytes,
            sample_rate: DEFAULT_OUTPUT_SAMPLE_RATE,
            channels: 2,
            bits_per_sample: 16,
            playing: true,
            looping: false,
            is_primary: false,
            flags: 0,
            play_pos_frames: 0.0,
            volume_db: 0,
            pan_db: 0,
            ds3d: Ds3dBufferParams::default(),
        });

        let mut out = [0i16; 16];
        mixer.mix_into(&mut out);
        assert_eq!(out[0], i16::MAX);
        assert!(out[1] > 20_000);
        // 8-bit half-rate 4-frame buffer and 16-bit full-rate 8-frame buffer both finish after 8 output frames.
        assert_eq!(mixer.buffers.values().filter(|b| b.playing).count(), 0);
    }

    #[test]
    fn mixer_applies_db_volume_and_stereo_panning() {
        let mut mixer = DsoundMixer::new();
        let mut s16_bytes = Vec::new();
        for _ in 0..4 {
            s16_bytes.extend_from_slice(&16_384i16.to_le_bytes());
        }
        let id = mixer.alloc_buffer(DsoundBufferData {
            data: s16_bytes,
            sample_rate: DEFAULT_OUTPUT_SAMPLE_RATE,
            channels: 1,
            bits_per_sample: 16,
            playing: true,
            looping: false,
            is_primary: false,
            flags: 0,
            play_pos_frames: 0.0,
            volume_db: -602, // ~0.5x amplitude
            pan_db: -10_000, // full left
            ds3d: Ds3dBufferParams::default(),
        });

        let mut out = [0i16; 4];
        mixer.mix_into(&mut out);
        assert!(out[0] > 7_900 && out[0] < 8_400);
        assert_eq!(out[1], 0);

        mixer.buffers.get_mut(&id).unwrap().pan_db = 10_000;
        mixer.buffers.get_mut(&id).unwrap().playing = true;
        mixer.buffers.get_mut(&id).unwrap().play_pos_frames = 0.0;
        mixer.mix_into(&mut out);
        assert_eq!(out[0], 0);
        assert!(out[1] > 7_900 && out[1] < 8_400);
    }

    #[test]
    fn mixer_applies_3d_distance_attenuation_and_azimuth_panning() {
        let mut mixer = DsoundMixer::new();
        let mut s16_bytes = Vec::new();
        for _ in 0..4 {
            s16_bytes.extend_from_slice(&20_000i16.to_le_bytes());
        }
        mixer.alloc_buffer(DsoundBufferData {
            data: s16_bytes,
            sample_rate: DEFAULT_OUTPUT_SAMPLE_RATE,
            channels: 1,
            bits_per_sample: 16,
            playing: true,
            looping: false,
            is_primary: false,
            flags: DSBCAPS_CTRL3D,
            play_pos_frames: 0.0,
            volume_db: 0,
            pan_db: 0,
            ds3d: Ds3dBufferParams {
                pos: [2.0, 0.0, 0.0], // right of listener, distance 2.0 (0.5x attenuation)
                min_distance: 1.0,
                max_distance: 100.0,
                mode: DS3DMODE_NORMAL,
                is_3d: true,
                ..Ds3dBufferParams::default()
            },
        });

        let mut out = [0i16; 4];
        mixer.mix_into(&mut out);
        assert_eq!(out[0], 0);
        assert!((out[1] - 10_000).abs() <= 4);
    }

    #[test]
    fn circular_lock_unlock_copies_wrapped_pcm_into_mixer() {
        let mut h = TestHarness::new();
        let ds8 = h.create_ds8();
        let buf = h.create_buffer(ds8, 4096, DEFAULT_OUTPUT_SAMPLE_RATE, 1, 16, DSBCAPS_CTRLVOLUME);

        let pp1 = h.heap.alloc(&mut h.arena, 4, true).unwrap();
        let pb1 = h.heap.alloc(&mut h.arena, 4, true).unwrap();
        let pp2 = h.heap.alloc(&mut h.arena, 4, true).unwrap();
        let pb2 = h.heap.alloc(&mut h.arena, 4, true).unwrap();

        assert_eq!(h.call_com(buf, 11, &[4092, 8, pp1, pb1, pp2, pb2, 0]), DS_OK);
        let p1 = h.arena.read_u32(pp1).unwrap();
        let l1 = h.arena.read_u32(pb1).unwrap();
        let p2 = h.arena.read_u32(pp2).unwrap();
        let l2 = h.arena.read_u32(pb2).unwrap();
        assert_eq!(l1, 4);
        assert_eq!(l2, 4);

        h.arena.write_bytes(p1, &[1, 2, 3, 4]).unwrap();
        h.arena.write_bytes(p2, &[5, 6, 7, 8]).unwrap();
        assert_eq!(h.call_com(buf, 19, &[p1, l1, p2, l2]), DS_OK);

        let mixer_id = h.win32.dsound.buffers[&buf].mixer_id;
        let guard = h.win32.dsound.mixer.lock().unwrap();
        let data = &guard.buffers[&mixer_id].data;
        assert_eq!(&data[4092..4096], &[1, 2, 3, 4]);
        assert_eq!(&data[0..4], &[5, 6, 7, 8]);
    }

    #[test]
    fn play_get_current_position_looping_and_one_shot_auto_stop() {
        let mut h = TestHarness::new();
        h.win32.dsound.mixer.lock().unwrap().hardware_clocked = true;
        let ds8 = h.create_ds8();
        let buf_loop = h.create_buffer(ds8, 4096, DEFAULT_OUTPUT_SAMPLE_RATE, 1, 16, 0);
        let buf_once = h.create_buffer(ds8, 4096, DEFAULT_OUTPUT_SAMPLE_RATE, 1, 16, 0);

        assert_eq!(h.call_com(buf_loop, 12, &[0, 0, DSBPLAY_LOOPING]), DS_OK);
        assert_eq!(h.call_com(buf_once, 12, &[0, 0, 0]), DS_OK);

        let p_status = h.heap.alloc(&mut h.arena, 4, true).unwrap();
        assert_eq!(h.call_com(buf_loop, 9, &[p_status]), DS_OK);
        assert_eq!(
            h.arena.read_u32(p_status).unwrap(),
            DSBSTATUS_PLAYING | DSBSTATUS_LOOPING
        );

        let mut mix_out = [0i16; 2048];
        h.win32.dsound.mixer.lock().unwrap().mix_into(&mut mix_out);

        let p_play = h.heap.alloc(&mut h.arena, 4, true).unwrap();
        let p_write = h.heap.alloc(&mut h.arena, 4, true).unwrap();
        assert_eq!(h.call_com(buf_loop, 4, &[p_play, p_write]), DS_OK);
        assert_eq!(h.arena.read_u32(p_play).unwrap(), 2048);

        h.win32.dsound.mixer.lock().unwrap().mix_into(&mut mix_out);
        assert_eq!(h.call_com(buf_loop, 9, &[p_status]), DS_OK);
        assert_eq!(
            h.arena.read_u32(p_status).unwrap(),
            DSBSTATUS_PLAYING | DSBSTATUS_LOOPING
        );
        assert_eq!(h.call_com(buf_once, 9, &[p_status]), DS_OK);
        assert_eq!(h.arena.read_u32(p_status).unwrap(), 0);

        // Verify headless wall-clock fallback advances cursor when no SDL audio device is attached.
        {
            let mut guard = h.win32.dsound.mixer.lock().unwrap();
            guard.hardware_clocked = false;
            guard.last_advance = Instant::now() - std::time::Duration::from_millis(10);
        }
        assert_eq!(h.call_com(buf_loop, 4, &[p_play, p_write]), DS_OK);
        assert!(h.arena.read_u32(p_play).unwrap() > 0);
    }

    #[test]
    fn push_aux_pcm_resamples_and_mixes_into_output() {
        let mut mixer = DsoundMixer::new();
        mixer.push_aux_pcm(&[10_000, 10_000, 10_000], DEFAULT_OUTPUT_SAMPLE_RATE / 2, 1);
        let mut out = [0i16; 12];
        mixer.mix_into(&mut out);
        assert_eq!(out[0], 10_000);
        assert_eq!(out[1], 10_000);
        assert_eq!(out[4], 10_000);
        assert_eq!(out[5], 10_000);
        assert_eq!(mixer.aux_pcm.len(), 0);
    }

    #[test]
    fn push_aux_pcm_fast_path_fractional_continuity_and_played_frames() {
        let mut mixer = DsoundMixer::new();
        // 1:1 48 kHz stereo copy path preserves exact samples and tracks aux_played_frames.
        mixer.push_aux_pcm(&[1_234, -2_345, 3_456, -4_567], 48_000, 2);
        assert_eq!(mixer.aux_pcm.len(), 4);
        let mut out = [0i16; 4];
        mixer.mix_into(&mut out);
        assert_eq!(out, [1_234, -2_345, 3_456, -4_567]);
        assert_eq!(mixer.aux_played_frames(), 2);

        // Fractional phase continuity: 1000 frames @ 44100 Hz pushed in 10x100-frame chunks vs 1x1000-frame push.
        let src: Vec<i16> = (0..1000)
            .flat_map(|i| [((i * 13) % 20_000) as i16, ((i * 7) % 20_000) as i16])
            .collect();
        let mut single = DsoundMixer::new();
        single.push_aux_pcm(&src, 44_100, 2);

        let mut chunked = DsoundMixer::new();
        for chunk in src.chunks(200) {
            chunked.push_aux_pcm(chunk, 44_100, 2);
        }
        assert_eq!(single.aux_pcm.len(), chunked.aux_pcm.len());
        chunked.clear_aux_pcm();
        assert_eq!(chunked.aux_pcm.len(), 0);
    }

    #[test]
    fn soft_limiter_preserves_normal_signals_and_rounds_hot_peaks() {
        assert_eq!(soft_limit_sample(0.5), 0.5);
        assert_eq!(soft_limit_sample(-0.85), -0.85);
        let peak = soft_limit_sample(1.1);
        assert!(peak > 0.85 && peak < 0.999);
        let neg_peak = soft_limit_sample(-1.1);
        assert!(neg_peak < -0.85 && neg_peak > -0.999);
    }

    #[test]
    fn com_refcount_keeps_looping_buffer_alive_until_release_zero() {
        let mut h = TestHarness::new();
        h.win32.dsound.mixer.lock().unwrap().hardware_clocked = true;
        let ds8 = h.create_ds8();
        let buf = h.create_buffer(ds8, 4096, DEFAULT_OUTPUT_SAMPLE_RATE, 2, 16, DSBCAPS_CTRL3D);
        let mixer_id = h.win32.dsound.buffers[&buf].mixer_id;

        // Fill buffer with non-zero 48 kHz 16-bit PCM so mix_into produces audible samples.
        {
            let mut guard = h.win32.dsound.mixer.lock().unwrap();
            let data = &mut guard.buffers.get_mut(&mixer_id).unwrap().data;
            for sample in data.chunks_exact_mut(2) {
                sample.copy_from_slice(&8_000i16.to_le_bytes());
            }
        }
        assert_eq!(h.call_com(buf, 12, &[0, 0, DSBPLAY_LOOPING]), DS_OK);

        // AddRef -> 2, QueryInterface(IDirectSoundBuffer8) -> 3.
        assert_eq!(h.call_com(buf, 1, &[]), 2);
        let iid_va = h.heap.alloc(&mut h.arena, 16, true).unwrap();
        h.arena.write_u32(iid_va, IID_IDIRECTSOUNDBUFFER8_DATA1).unwrap();
        let ppv = h.heap.alloc(&mut h.arena, 4, true).unwrap();
        assert_eq!(h.call_com(buf, 0, &[iid_va, ppv]), DS_OK);
        assert_eq!(h.arena.read_u32(ppv).unwrap(), buf);

        // Also acquire IDirectSound3DBuffer interface and verify its independent refcount cleanup.
        h.arena.write_u32(iid_va, IID_IDIRECTSOUND3DBUFFER_DATA1).unwrap();
        assert_eq!(h.call_com(buf, 0, &[iid_va, ppv]), DS_OK);
        let ds3d = h.arena.read_u32(ppv).unwrap();
        assert_eq!(h.call_com(ds3d, 1, &[]), 2);
        assert_eq!(h.call_com(ds3d, 2, &[]), 1);
        assert_eq!(h.call_com(ds3d, 2, &[]), 0);
        assert!(!h.win32.dsound.ds3d_buffers.contains_key(&ds3d));

        // Release twice: refcount drops 3 -> 2 -> 1; looping buffer stays alive and mixes audio.
        assert_eq!(h.call_com(buf, 2, &[]), 2);
        assert_eq!(h.call_com(buf, 2, &[]), 1);
        let mut out = [0i16; 32];
        h.win32.dsound.mixer.lock().unwrap().mix_into(&mut out);
        assert!(out.iter().any(|&s| s != 0));
        assert!(h.win32.dsound.mixer.lock().unwrap().buffers.contains_key(&mixer_id));

        // Final Release drops refcount to 0, stopping playback and removing buffer.
        assert_eq!(h.call_com(buf, 2, &[]), 0);
        assert!(!h.win32.dsound.buffers.contains_key(&buf));
        assert!(!h.win32.dsound.mixer.lock().unwrap().buffers.contains_key(&mixer_id));

        out.fill(0);
        h.win32.dsound.mixer.lock().unwrap().mix_into(&mut out);
        assert!(out.iter().all(|&s| s == 0));
    }

    #[test]
    fn aux_rate_ratio_adjusts_consumption_smoothly_and_clamps() {
        let mut mixer = DsoundMixer::new();
        mixer.set_aux_rate_ratio(0.5);
        assert!((mixer.aux_rate_ratio() - (1.0 - MAX_AUX_RATE_SKEW)).abs() < 1e-12);
        mixer.set_aux_rate_ratio(2.0);
        assert!((mixer.aux_rate_ratio() - (1.0 + MAX_AUX_RATE_SKEW)).abs() < 1e-12);
        mixer.set_aux_rate_ratio(f64::NAN);
        assert_eq!(mixer.aux_rate_ratio(), 1.0);

        let sine_frames = 2400usize;
        let sine: Vec<i16> = (0..sine_frames)
            .flat_map(|i| {
                let phase = 2.0 * std::f64::consts::PI * 440.0 * (i as f64) / 48_000.0;
                let s = (phase.sin() * 16_000.0).round() as i16;
                [s, -s]
            })
            .collect();

        mixer.push_aux_pcm(&sine, 48_000, 2);
        mixer.set_aux_rate_ratio(0.98);
        let mut rendered_slow = Vec::new();
        let mut buf = [0i16; 250 * OUTPUT_CHANNELS];
        for _ in 0..4 {
            mixer.mix_into(&mut buf);
            rendered_slow.extend_from_slice(&buf);
        }
        assert!(
            (979..=981).contains(&mixer.aux_played_frames()),
            "expected ~980 frames consumed at 0.98x, got {}",
            mixer.aux_played_frames()
        );
        let max_step_l = rendered_slow
            .chunks_exact(OUTPUT_CHANNELS)
            .map(|c| c[0])
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (i32::from(w[1]) - i32::from(w[0])).abs())
            .max()
            .unwrap();
        assert!(max_step_l < 1100, "unexpected discontinuity: {max_step_l}");

        mixer.clear_aux_pcm();
        assert_eq!(mixer.aux_rate_ratio(), 1.0);
        mixer.push_aux_pcm(&sine, 48_000, 2);
        mixer.set_aux_rate_ratio(1.02);
        let start_played = mixer.aux_played_frames();
        for _ in 0..4 {
            mixer.mix_into(&mut buf);
        }
        let fast_played = mixer.aux_played_frames() - start_played;
        assert!(
            (1019..=1021).contains(&fast_played),
            "expected ~1020 frames consumed at 1.02x, got {fast_played}"
        );
    }

    #[test]
    fn estimated_audible_aux_sec_is_c0_continuous_across_callbacks() {
        let mut mixer = DsoundMixer::new();
        mixer.hardware_clocked = true;
        mixer.output_sample_rate = 48_000;
        let period_frames = mixer.output_buffer_frames() as usize; // 2048 frames
        let period_dur = Duration::from_secs_f64(f64::from(mixer.output_buffer_frames()) / 48_000.0);

        let pcm = vec![100i16; period_frames * 6 * OUTPUT_CHANNELS];
        mixer.push_aux_pcm(&pcm, 48_000, 2);

        let mut out = vec![0i16; period_frames * OUTPUT_CHANNELS];
        // Prime 3 periods so audible is positive beyond the 2.5-period initial pipeline delay.
        for _ in 0..3 {
            mixer.mix_into(&mut out);
        }
        let t_end_k = mixer.last_audio_callback + period_dur;
        let est_before = mixer.estimated_audible_aux_sec(0, t_end_k).unwrap();

        // Next callback at the exact end of period K
        mixer.mix_into(&mut out);
        let t_start_k1 = mixer.last_audio_callback;
        let est_after = mixer.estimated_audible_aux_sec(0, t_start_k1).unwrap();

        assert!(
            (est_before - est_after).abs() < 1e-6,
            "expected C0 continuity across 2048-frame callback: before={est_before:.6}s after={est_after:.6}s"
        );
    }

    #[test]
    fn sdl_audio_callback_tracks_full_period_and_ignores_headless_cursor_polls() {
        let mixer = Arc::new(Mutex::new(DsoundMixer::new()));
        {
            let mut guard = mixer.lock().unwrap();
            guard.hardware_clocked = true;
            guard.output_sample_rate = 48_000;
            // Push enough audio for multiple 2048-frame callbacks plus multi-chunk callbacks.
            let pcm = vec![120i16; 2048 * 8 * OUTPUT_CHANNELS];
            guard.push_aux_pcm(&pcm, 48_000, 2);
        }

        // Simulate a 4096-frame (16384-byte) callback that spans multiple internal chunks.
        let mut raw_stream = vec![0u8; 4096 * OUTPUT_CHANNELS * std::mem::size_of::<i16>()];
        let userdata = std::ptr::addr_of!(mixer).cast::<c_void>().cast_mut();
        unsafe {
            sdl_audio_callback(userdata, raw_stream.as_mut_ptr(), raw_stream.len() as i32);
        }

        let mut guard = mixer.lock().unwrap();
        assert_eq!(guard.last_aux_popped_frames, 4096);
        let cb_ts = guard.last_audio_callback;
        guard.tick_headless_cursors();
        assert_eq!(
            guard.last_audio_callback, cb_ts,
            "tick_headless_cursors must not clobber last_audio_callback when hardware_clocked"
        );
    }
}
