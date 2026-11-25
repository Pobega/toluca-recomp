use std::collections::HashMap;
use std::ffi::{CString, c_char, c_void};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use toluca_core::GuestAddr;
use toluca_core::cpu::GuestCpuState;

use crate::arena::GuestArena;
use crate::callconv::{peek_arg_i32, peek_arg_u32, return_stdcall};
use crate::runtime::{ImportHandler, ImportRegistry, RuntimeContext, RuntimeError};
use crate::win32::dsound::{DsoundMixer, MAX_AUX_RATE_SKEW};

const MAX_PATH_LEN: usize = 512;
const BINK_STRUCT_ALLOC_BYTES: usize = 256;
const BINK_HEADER_MIN_BYTES: usize = 44;
const BINK_AUDIO_STEREO_FLAG: u16 = 0x2000;
#[allow(dead_code)]
const BINK_FLAG_NOSKIP: u32 = 0x0008_0000;

const AV_SYNC_KP: f64 = 0.36;
const AV_SYNC_KI: f64 = 0.0324;
const AV_SYNC_INTEGRAL_LIMIT: f64 = 1.0;
const AV_SYNC_RATIO_SMOOTHING: f64 = 0.2;

const BINKSURFACE_MASK: u32 = 0x0F;
pub const BINKSURFACE24: u32 = 1;
pub const BINKSURFACE24R: u32 = 2;
pub const BINKSURFACE32: u32 = 3;
pub const BINKSURFACE32R: u32 = 4;
pub const BINKSURFACE32A: u32 = 5;
pub const BINKSURFACE32RA: u32 = 6;
pub const BINKSURFACE555R: u32 = 7;
pub const BINKSURFACE565R: u32 = 8;
pub const BINKSURFACE555: u32 = 9;
pub const BINKSURFACE565: u32 = 10;

const AVMEDIA_TYPE_VIDEO: i32 = 0;
const AVMEDIA_TYPE_AUDIO: i32 = 1;
const AV_PIX_FMT_YUV420P: i32 = 0;
const AV_PIX_FMT_YUVA420P: i32 = 33;
const AV_SAMPLE_FMT_S16: i32 = 1;
const AV_SAMPLE_FMT_S16P: i32 = 6;
const AV_SAMPLE_FMT_FLTP: i32 = 8;
const AVSEEK_FLAG_BACKWARD: i32 = 1;
const AVERROR_EAGAIN: i32 = -11;

const AVFMT_NB_STREAMS_OFF: usize = 44;
const AVFMT_STREAMS_OFF: usize = 48;
const AVCODECPAR_TYPE_OFF: usize = 0;
const AVCODECPAR_ID_OFF: usize = 4;
const AVPKT_STREAM_IDX_OFF: usize = 36;
const AVFRAME_DATA_OFF: usize = 0;
const AVFRAME_LINESIZE_OFF: usize = 64;
const AVFRAME_WIDTH_OFF: usize = 104;
const AVFRAME_HEIGHT_OFF: usize = 108;
const AVFRAME_NB_SAMPLES_OFF: usize = 112;
const AVFRAME_FORMAT_OFF: usize = 116;

const DEFAULT_FPS_NUM: u32 = 30;
const DEFAULT_AUDIO_RATE: u32 = 44_100;
const DEFAULT_AUDIO_CHANNELS: u16 = 2;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BinkAudioTrack {
    pub sample_rate: u16,
    pub channels: u16,
    pub track_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinkHeader {
    pub magic: [u8; 4],
    pub file_size: u32,
    pub num_frames: u32,
    pub largest_frame_size: u32,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub video_flags: u32,
    pub audio_tracks: Vec<BinkAudioTrack>,
}

impl BinkHeader {
    pub fn frame_duration(&self) -> Duration {
        let num = u64::from(self.fps_num.max(1));
        let den = u64::from(self.fps_den.max(1));
        let nanos = (den * 1_000_000_000) / num;
        Duration::from_nanos(nanos.max(1))
    }
}

pub fn parse_bink_header(bytes: &[u8]) -> Option<BinkHeader> {
    if bytes.len() < BINK_HEADER_MIN_BYTES {
        return None;
    }
    let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if !matches!(&magic, b"BIKf" | b"BIKg" | b"BIKh" | b"BIKi") {
        return None;
    }
    let read_u32 = |off: usize| {
        u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
    };
    let read_u16 = |off: usize| u16::from_le_bytes([bytes[off], bytes[off + 1]]);

    let file_size = read_u32(0x04);
    let num_frames = read_u32(0x08).max(1);
    let largest_frame_size = read_u32(0x0C);
    let width = read_u32(0x14).max(1);
    let height = read_u32(0x18).max(1);
    let raw_fps_num = read_u32(0x1C);
    let fps_num = if raw_fps_num == 0 { DEFAULT_FPS_NUM } else { raw_fps_num };
    let fps_den = read_u32(0x20).max(1);
    let video_flags = read_u32(0x24);
    let num_audio_tracks = read_u32(0x28) as usize;

    let mut audio_tracks = Vec::new();
    let table_needed = BINK_HEADER_MIN_BYTES + num_audio_tracks.saturating_mul(12);
    if num_audio_tracks > 0 && num_audio_tracks <= 64 && bytes.len() >= table_needed {
        let fmt_base = BINK_HEADER_MIN_BYTES + num_audio_tracks * 4;
        let id_base = fmt_base + num_audio_tracks * 4;
        for idx in 0..num_audio_tracks {
            let sample_rate = read_u16(fmt_base + idx * 4);
            let audio_flags = read_u16(fmt_base + idx * 4 + 2);
            let track_id = read_u32(id_base + idx * 4);
            let channels = if (audio_flags & BINK_AUDIO_STEREO_FLAG) != 0 {
                2
            } else {
                1
            };
            audio_tracks.push(BinkAudioTrack {
                sample_rate,
                channels,
                track_id,
            });
        }
    }

    Some(BinkHeader {
        magic,
        file_size,
        num_frames,
        largest_frame_size,
        width,
        height,
        fps_num,
        fps_den,
        video_flags,
        audio_tracks,
    })
}

#[derive(Clone, Debug)]
pub struct DecodedVideoFrame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub a: Option<Vec<u8>>,
}

impl DecodedVideoFrame {
    pub fn black(width: u32, height: u32) -> Self {
        let w = width as usize;
        let h = height as usize;
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        Self {
            width,
            height,
            y: vec![16; w * h],
            u: vec![128; cw * ch],
            v: vec![128; cw * ch],
            a: None,
        }
    }
}

type AvVersionFn = unsafe extern "C" fn() -> u32;
type AvFormatOpenInputFn = unsafe extern "C" fn(
    ps: *mut *mut c_void,
    url: *const c_char,
    fmt: *const c_void,
    options: *mut *mut c_void,
) -> i32;
type AvFormatFindStreamInfoFn =
    unsafe extern "C" fn(ic: *mut c_void, options: *mut *mut c_void) -> i32;
type AvReadFrameFn = unsafe extern "C" fn(s: *mut c_void, pkt: *mut c_void) -> i32;
type AvSeekFrameFn =
    unsafe extern "C" fn(s: *mut c_void, stream_index: i32, timestamp: i64, flags: i32) -> i32;
type AvFormatCloseInputFn = unsafe extern "C" fn(s: *mut *mut c_void);

type AvCodecFindDecoderFn = unsafe extern "C" fn(id: i32) -> *const c_void;
type AvCodecAllocContext3Fn = unsafe extern "C" fn(codec: *const c_void) -> *mut c_void;
type AvCodecParametersToContextFn =
    unsafe extern "C" fn(codec: *mut c_void, par: *const c_void) -> i32;
type AvCodecOpen2Fn = unsafe extern "C" fn(
    avctx: *mut c_void,
    codec: *const c_void,
    options: *mut *mut c_void,
) -> i32;
type AvCodecSendPacketFn = unsafe extern "C" fn(avctx: *mut c_void, avpkt: *const c_void) -> i32;
type AvCodecReceiveFrameFn = unsafe extern "C" fn(avctx: *mut c_void, frame: *mut c_void) -> i32;
type AvCodecFlushBuffersFn = unsafe extern "C" fn(avctx: *mut c_void);
type AvCodecFreeContextFn = unsafe extern "C" fn(avctx: *mut *mut c_void);
type AvPacketAllocFn = unsafe extern "C" fn() -> *mut c_void;
type AvPacketUnrefFn = unsafe extern "C" fn(pkt: *mut c_void);
type AvPacketFreeFn = unsafe extern "C" fn(pkt: *mut *mut c_void);

type AvFrameAllocFn = unsafe extern "C" fn() -> *mut c_void;
type AvFrameUnrefFn = unsafe extern "C" fn(frame: *mut c_void);
type AvFrameFreeFn = unsafe extern "C" fn(frame: *mut *mut c_void);

struct FfmpegApi {
    lib_avutil: *mut c_void,
    lib_avcodec: *mut c_void,
    lib_avformat: *mut c_void,
    avstream_codecpar_off: usize,
    avformat_open_input: AvFormatOpenInputFn,
    avformat_find_stream_info: AvFormatFindStreamInfoFn,
    av_read_frame: AvReadFrameFn,
    av_seek_frame: AvSeekFrameFn,
    avformat_close_input: AvFormatCloseInputFn,
    avcodec_find_decoder: AvCodecFindDecoderFn,
    avcodec_alloc_context3: AvCodecAllocContext3Fn,
    avcodec_parameters_to_context: AvCodecParametersToContextFn,
    avcodec_open2: AvCodecOpen2Fn,
    avcodec_send_packet: AvCodecSendPacketFn,
    avcodec_receive_frame: AvCodecReceiveFrameFn,
    avcodec_flush_buffers: AvCodecFlushBuffersFn,
    avcodec_free_context: AvCodecFreeContextFn,
    av_packet_alloc: AvPacketAllocFn,
    av_packet_unref: AvPacketUnrefFn,
    av_packet_free: AvPacketFreeFn,
    av_frame_alloc: AvFrameAllocFn,
    av_frame_unref: AvFrameUnrefFn,
    av_frame_free: AvFrameFreeFn,
}

unsafe impl Send for FfmpegApi {}

impl Drop for FfmpegApi {
    fn drop(&mut self) {
        unsafe {
            libc::dlclose(self.lib_avformat);
            libc::dlclose(self.lib_avcodec);
            libc::dlclose(self.lib_avutil);
        }
    }
}

impl FfmpegApi {
    fn try_load() -> Option<Self> {
        let lib_avutil = open_first_lib(&[
            c"libavutil.so.60",
            c"libavutil.so.59",
            c"libavutil.so.58",
            c"libavutil.so.57",
            c"libavutil.so",
        ])?;
        let Some(lib_avcodec) = open_first_lib(&[
            c"libavcodec.so.62",
            c"libavcodec.so.61",
            c"libavcodec.so.60",
            c"libavcodec.so.59",
            c"libavcodec.so",
        ]) else {
            unsafe { libc::dlclose(lib_avutil) };
            return None;
        };
        let Some(lib_avformat) = open_first_lib(&[
            c"libavformat.so.62",
            c"libavformat.so.61",
            c"libavformat.so.60",
            c"libavformat.so.59",
            c"libavformat.so",
        ]) else {
            unsafe {
                libc::dlclose(lib_avcodec);
                libc::dlclose(lib_avutil);
            }
            return None;
        };

        let close_all = || unsafe {
            libc::dlclose(lib_avformat);
            libc::dlclose(lib_avcodec);
            libc::dlclose(lib_avutil);
        };

        let Some(avformat_version) = load_sym::<AvVersionFn>(lib_avformat, c"avformat_version") else {
            close_all();
            return None;
        };
        let avformat_major = unsafe { avformat_version() } >> 16;
        let avstream_codecpar_off = if avformat_major >= 61 { 16 } else { 8 };

        let (
            Some(avformat_open_input),
            Some(avformat_find_stream_info),
            Some(av_read_frame),
            Some(av_seek_frame),
            Some(avformat_close_input),
            Some(avcodec_find_decoder),
            Some(avcodec_alloc_context3),
            Some(avcodec_parameters_to_context),
            Some(avcodec_open2),
            Some(avcodec_send_packet),
            Some(avcodec_receive_frame),
            Some(avcodec_flush_buffers),
            Some(avcodec_free_context),
            Some(av_packet_alloc),
            Some(av_packet_unref),
            Some(av_packet_free),
            Some(av_frame_alloc),
            Some(av_frame_unref),
            Some(av_frame_free),
        ) = (
            load_sym::<AvFormatOpenInputFn>(lib_avformat, c"avformat_open_input"),
            load_sym::<AvFormatFindStreamInfoFn>(lib_avformat, c"avformat_find_stream_info"),
            load_sym::<AvReadFrameFn>(lib_avformat, c"av_read_frame"),
            load_sym::<AvSeekFrameFn>(lib_avformat, c"av_seek_frame"),
            load_sym::<AvFormatCloseInputFn>(lib_avformat, c"avformat_close_input"),
            load_sym::<AvCodecFindDecoderFn>(lib_avcodec, c"avcodec_find_decoder"),
            load_sym::<AvCodecAllocContext3Fn>(lib_avcodec, c"avcodec_alloc_context3"),
            load_sym::<AvCodecParametersToContextFn>(lib_avcodec, c"avcodec_parameters_to_context"),
            load_sym::<AvCodecOpen2Fn>(lib_avcodec, c"avcodec_open2"),
            load_sym::<AvCodecSendPacketFn>(lib_avcodec, c"avcodec_send_packet"),
            load_sym::<AvCodecReceiveFrameFn>(lib_avcodec, c"avcodec_receive_frame"),
            load_sym::<AvCodecFlushBuffersFn>(lib_avcodec, c"avcodec_flush_buffers"),
            load_sym::<AvCodecFreeContextFn>(lib_avcodec, c"avcodec_free_context"),
            load_sym::<AvPacketAllocFn>(lib_avcodec, c"av_packet_alloc"),
            load_sym::<AvPacketUnrefFn>(lib_avcodec, c"av_packet_unref"),
            load_sym::<AvPacketFreeFn>(lib_avcodec, c"av_packet_free"),
            load_sym::<AvFrameAllocFn>(lib_avutil, c"av_frame_alloc"),
            load_sym::<AvFrameUnrefFn>(lib_avutil, c"av_frame_unref"),
            load_sym::<AvFrameFreeFn>(lib_avutil, c"av_frame_free"),
        )
        else {
            close_all();
            return None;
        };

        Some(Self {
            lib_avutil,
            lib_avcodec,
            lib_avformat,
            avstream_codecpar_off,
            avformat_open_input,
            avformat_find_stream_info,
            av_read_frame,
            av_seek_frame,
            avformat_close_input,
            avcodec_find_decoder,
            avcodec_alloc_context3,
            avcodec_parameters_to_context,
            avcodec_open2,
            avcodec_send_packet,
            avcodec_receive_frame,
            avcodec_flush_buffers,
            avcodec_free_context,
            av_packet_alloc,
            av_packet_unref,
            av_packet_free,
            av_frame_alloc,
            av_frame_unref,
            av_frame_free,
        })
    }
}

struct FfmpegDecoder {
    fmt_ctx: *mut c_void,
    video_stream_idx: i32,
    video_ctx: *mut c_void,
    audio_stream_idx: Option<i32>,
    audio_ctx: *mut c_void,
    pkt: *mut c_void,
    frame: *mut c_void,
}

unsafe impl Send for FfmpegDecoder {}

impl FfmpegDecoder {
    fn open(api: &FfmpegApi, path: &Path, audio_track_ordinal: usize) -> Option<Self> {
        let path_str = path.to_str()?;
        let c_path = CString::new(path_str).ok()?;
        let mut fmt_ctx: *mut c_void = std::ptr::null_mut();
        if unsafe {
            (api.avformat_open_input)(
                &mut fmt_ctx,
                c_path.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        } < 0
            || fmt_ctx.is_null()
        {
            return None;
        }
        if unsafe { (api.avformat_find_stream_info)(fmt_ctx, std::ptr::null_mut()) } < 0 {
            unsafe { (api.avformat_close_input)(&mut fmt_ctx) };
            return None;
        }

        let nb_streams = unsafe { *(fmt_ctx.cast::<u8>().add(AVFMT_NB_STREAMS_OFF).cast::<u32>()) };
        let streams = unsafe {
            *(fmt_ctx
                .cast::<u8>()
                .add(AVFMT_STREAMS_OFF)
                .cast::<*const *const c_void>())
        };
        if streams.is_null() {
            unsafe { (api.avformat_close_input)(&mut fmt_ctx) };
            return None;
        }

        let mut video_stream: Option<(i32, *const c_void)> = None;
        let mut audio_streams: Vec<(i32, *const c_void)> = Vec::new();
        for idx in 0..nb_streams as usize {
            let st = unsafe { *streams.add(idx) };
            if st.is_null() {
                continue;
            }
            let codecpar =
                unsafe { *(st.cast::<u8>().add(api.avstream_codecpar_off).cast::<*const c_void>()) };
            if codecpar.is_null() {
                continue;
            }
            let codec_type = unsafe { *(codecpar.cast::<u8>().add(AVCODECPAR_TYPE_OFF).cast::<i32>()) };
            if codec_type == AVMEDIA_TYPE_VIDEO && video_stream.is_none() {
                video_stream = Some((idx as i32, codecpar));
            } else if codec_type == AVMEDIA_TYPE_AUDIO {
                audio_streams.push((idx as i32, codecpar));
            }
        }

        let Some((video_stream_idx, video_par)) = video_stream else {
            unsafe { (api.avformat_close_input)(&mut fmt_ctx) };
            return None;
        };
        let Some(video_ctx) = open_codec_ctx(api, video_par) else {
            unsafe { (api.avformat_close_input)(&mut fmt_ctx) };
            return None;
        };

        let chosen_audio = audio_streams
            .get(audio_track_ordinal)
            .or_else(|| audio_streams.first())
            .copied();
        let (audio_stream_idx, audio_ctx) = match chosen_audio {
            Some((idx, par)) => match open_codec_ctx(api, par) {
                Some(ctx) => (Some(idx), ctx),
                None => (None, std::ptr::null_mut()),
            },
            None => (None, std::ptr::null_mut()),
        };

        let pkt = unsafe { (api.av_packet_alloc)() };
        let frame = unsafe { (api.av_frame_alloc)() };
        if pkt.is_null() || frame.is_null() {
            if !pkt.is_null() {
                let mut p = pkt;
                unsafe { (api.av_packet_free)(&mut p) };
            }
            if !frame.is_null() {
                let mut f = frame;
                unsafe { (api.av_frame_free)(&mut f) };
            }
            if !audio_ctx.is_null() {
                let mut ac = audio_ctx;
                unsafe { (api.avcodec_free_context)(&mut ac) };
            }
            let mut vc = video_ctx;
            unsafe {
                (api.avcodec_free_context)(&mut vc);
                (api.avformat_close_input)(&mut fmt_ctx);
            }
            return None;
        }

        Some(Self {
            fmt_ctx,
            video_stream_idx,
            video_ctx,
            audio_stream_idx,
            audio_ctx,
            pkt,
            frame,
        })
    }

    fn close(mut self, api: &FfmpegApi) {
        unsafe {
            (api.av_frame_free)(&mut self.frame);
            (api.av_packet_free)(&mut self.pkt);
            if !self.audio_ctx.is_null() {
                (api.avcodec_free_context)(&mut self.audio_ctx);
            }
            if !self.video_ctx.is_null() {
                (api.avcodec_free_context)(&mut self.video_ctx);
            }
            if !self.fmt_ctx.is_null() {
                (api.avformat_close_input)(&mut self.fmt_ctx);
            }
        }
    }

    fn rewind(&mut self, api: &FfmpegApi) {
        unsafe {
            (api.av_seek_frame)(self.fmt_ctx, -1, 0, AVSEEK_FLAG_BACKWARD);
            (api.avcodec_flush_buffers)(self.video_ctx);
            if !self.audio_ctx.is_null() {
                (api.avcodec_flush_buffers)(self.audio_ctx);
            }
        }
    }

    fn decode_next_video_frame(
        &mut self,
        api: &FfmpegApi,
        audio_track: Option<BinkAudioTrack>,
        sound_enabled: bool,
    ) -> (Option<DecodedVideoFrame>, Vec<i16>) {
        let mut decoded_audio = Vec::new();
        let drain_audio = |api: &FfmpegApi,
                           audio_ctx: *mut c_void,
                           frame: *mut c_void,
                           out: &mut Vec<i16>| {
            if audio_ctx.is_null() || !sound_enabled {
                return;
            }
            let channels = audio_track.map_or(DEFAULT_AUDIO_CHANNELS, |t| t.channels);
            loop {
                let rc = unsafe { (api.avcodec_receive_frame)(audio_ctx, frame) };
                if rc < 0 {
                    break;
                }
                extract_audio_samples(frame, channels, out);
                unsafe { (api.av_frame_unref)(frame) };
            }
        };

        // Check for a buffered video frame before reading more packets.
        if unsafe { (api.avcodec_receive_frame)(self.video_ctx, self.frame) } == 0 {
            let vid = extract_video_frame(self.frame);
            unsafe { (api.av_frame_unref)(self.frame) };
            if !self.audio_ctx.is_null() {
                drain_audio(api, self.audio_ctx, self.frame, &mut decoded_audio);
            }
            return (vid, decoded_audio);
        }

        loop {
            let read_rc = unsafe { (api.av_read_frame)(self.fmt_ctx, self.pkt) };
            if read_rc < 0 {
                // Flush buffered decoder frames on EOF.
                unsafe { (api.avcodec_send_packet)(self.video_ctx, std::ptr::null()) };
                if unsafe { (api.avcodec_receive_frame)(self.video_ctx, self.frame) } == 0 {
                    let vid = extract_video_frame(self.frame);
                    unsafe { (api.av_frame_unref)(self.frame) };
                    if !self.audio_ctx.is_null() {
                        drain_audio(api, self.audio_ctx, self.frame, &mut decoded_audio);
                    }
                    return (vid, decoded_audio);
                }
                return (None, decoded_audio);
            }

            let stream_idx =
                unsafe { *(self.pkt.cast::<u8>().add(AVPKT_STREAM_IDX_OFF).cast::<i32>()) };
            if stream_idx == self.video_stream_idx {
                let send_rc = unsafe { (api.avcodec_send_packet)(self.video_ctx, self.pkt) };
                unsafe { (api.av_packet_unref)(self.pkt) };
                if (send_rc >= 0 || send_rc == AVERROR_EAGAIN)
                    && unsafe { (api.avcodec_receive_frame)(self.video_ctx, self.frame) } == 0
                {
                    let vid = extract_video_frame(self.frame);
                    unsafe { (api.av_frame_unref)(self.frame) };
                    if !self.audio_ctx.is_null() {
                        drain_audio(api, self.audio_ctx, self.frame, &mut decoded_audio);
                    }
                    return (vid, decoded_audio);
                }
            } else if Some(stream_idx) == self.audio_stream_idx && !self.audio_ctx.is_null() {
                if sound_enabled {
                    let send_rc = unsafe { (api.avcodec_send_packet)(self.audio_ctx, self.pkt) };
                    unsafe { (api.av_packet_unref)(self.pkt) };
                    if send_rc >= 0 || send_rc == AVERROR_EAGAIN {
                        drain_audio(api, self.audio_ctx, self.frame, &mut decoded_audio);
                    }
                } else {
                    unsafe { (api.av_packet_unref)(self.pkt) };
                }
            } else {
                unsafe { (api.av_packet_unref)(self.pkt) };
            }
        }
    }
}

fn open_codec_ctx(api: &FfmpegApi, codecpar: *const c_void) -> Option<*mut c_void> {
    let codec_id = unsafe { *(codecpar.cast::<u8>().add(AVCODECPAR_ID_OFF).cast::<i32>()) };
    let decoder = unsafe { (api.avcodec_find_decoder)(codec_id) };
    if decoder.is_null() {
        return None;
    }
    let ctx = unsafe { (api.avcodec_alloc_context3)(decoder) };
    if ctx.is_null() {
        return None;
    }
    if unsafe { (api.avcodec_parameters_to_context)(ctx, codecpar) } < 0
        || unsafe { (api.avcodec_open2)(ctx, decoder, std::ptr::null_mut()) } < 0
    {
        let mut c = ctx;
        unsafe { (api.avcodec_free_context)(&mut c) };
        return None;
    }
    Some(ctx)
}

fn extract_video_frame(frame: *const c_void) -> Option<DecodedVideoFrame> {
    let base = frame.cast::<u8>();
    let width = unsafe { *(base.add(AVFRAME_WIDTH_OFF).cast::<i32>()) };
    let height = unsafe { *(base.add(AVFRAME_HEIGHT_OFF).cast::<i32>()) };
    let format = unsafe { *(base.add(AVFRAME_FORMAT_OFF).cast::<i32>()) };
    if width <= 0 || height <= 0 || (format != AV_PIX_FMT_YUV420P && format != AV_PIX_FMT_YUVA420P)
    {
        return None;
    }

    let w = width as usize;
    let h = height as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let data = unsafe { *(base.add(AVFRAME_DATA_OFF).cast::<[*const u8; 8]>()) };
    let linesize = unsafe { *(base.add(AVFRAME_LINESIZE_OFF).cast::<[i32; 8]>()) };

    let y = copy_plane(data[0], linesize[0], w, h)?;
    let u = copy_plane(data[1], linesize[1], cw, ch)?;
    let v = copy_plane(data[2], linesize[2], cw, ch)?;
    let a = if format == AV_PIX_FMT_YUVA420P && !data[3].is_null() {
        copy_plane(data[3], linesize[3], w, h)
    } else {
        None
    };

    Some(DecodedVideoFrame {
        width: width as u32,
        height: height as u32,
        y,
        u,
        v,
        a,
    })
}

fn copy_plane(ptr: *const u8, stride: i32, width: usize, height: usize) -> Option<Vec<u8>> {
    if ptr.is_null() || stride <= 0 {
        return None;
    }
    let stride = stride as usize;
    if stride < width {
        return None;
    }
    let mut out = vec![0u8; width * height];
    for row in 0..height {
        let src = unsafe { std::slice::from_raw_parts(ptr.add(row * stride), width) };
        out[row * width..(row + 1) * width].copy_from_slice(src);
    }
    Some(out)
}

fn extract_audio_samples(frame: *const c_void, channels: u16, out: &mut Vec<i16>) {
    let base = frame.cast::<u8>();
    let nb_samples = unsafe { *(base.add(AVFRAME_NB_SAMPLES_OFF).cast::<i32>()) };
    let format = unsafe { *(base.add(AVFRAME_FORMAT_OFF).cast::<i32>()) };
    if nb_samples <= 0 {
        return;
    }
    let n = nb_samples as usize;
    let ch = usize::from(channels.clamp(1, 2));
    let data = unsafe { *(base.add(AVFRAME_DATA_OFF).cast::<[*const u8; 8]>()) };

    match format {
        AV_SAMPLE_FMT_FLTP => {
            if data[0].is_null() {
                return;
            }
            let ch0 = unsafe { std::slice::from_raw_parts(data[0].cast::<f32>(), n) };
            let ch1 = if ch >= 2 && !data[1].is_null() {
                Some(unsafe { std::slice::from_raw_parts(data[1].cast::<f32>(), n) })
            } else {
                None
            };
            if let Some(c1) = ch1 {
                planar_f32_to_interleaved_i16(&[ch0, c1], out);
            } else if ch >= 2 {
                planar_f32_to_interleaved_i16(&[ch0, ch0], out);
            } else {
                planar_f32_to_interleaved_i16(&[ch0], out);
            }
        }
        AV_SAMPLE_FMT_S16P => {
            if data[0].is_null() {
                return;
            }
            let ch0 = unsafe { std::slice::from_raw_parts(data[0].cast::<i16>(), n) };
            if ch >= 2 && !data[1].is_null() {
                let ch1 = unsafe { std::slice::from_raw_parts(data[1].cast::<i16>(), n) };
                for i in 0..n {
                    out.push(ch0[i]);
                    out.push(ch1[i]);
                }
            } else if ch >= 2 {
                for &s in ch0 {
                    out.push(s);
                    out.push(s);
                }
            } else {
                out.extend_from_slice(ch0);
            }
        }
        AV_SAMPLE_FMT_S16 => {
            if data[0].is_null() {
                return;
            }
            let slice = unsafe { std::slice::from_raw_parts(data[0].cast::<i16>(), n * ch) };
            out.extend_from_slice(slice);
        }
        _ => {}
    }
}

pub fn planar_f32_to_interleaved_i16(channels: &[&[f32]], out: &mut Vec<i16>) {
    if channels.is_empty() {
        return;
    }
    let n = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    out.reserve(n * channels.len());
    for i in 0..n {
        for ch in channels {
            let sample = (ch[i].clamp(-1.0, 1.0) * 32_767.0).round() as i16;
            out.push(sample);
        }
    }
}

const BT601_Y_OFFSET: i32 = 16;
const BT601_UV_OFFSET: i32 = 128;
const BT601_Y_COEFF: i32 = 298;
const BT601_RV_COEFF: i32 = 409;
const BT601_GU_COEFF: i32 = -100;
const BT601_GV_COEFF: i32 = -208;
const BT601_BU_COEFF: i32 = 516;
const BT601_ROUND: i32 = 128;
const BT601_SHIFT: i32 = 8;

#[inline]
fn bt601_uv_terms(u: u8, v: u8) -> (i32, i32, i32) {
    let d = i32::from(u) - BT601_UV_OFFSET;
    let e = i32::from(v) - BT601_UV_OFFSET;
    let r_uv = BT601_RV_COEFF * e + BT601_ROUND;
    let g_uv = BT601_GU_COEFF * d + BT601_GV_COEFF * e + BT601_ROUND;
    let b_uv = BT601_BU_COEFF * d + BT601_ROUND;
    (r_uv, g_uv, b_uv)
}

#[inline]
fn bt601_luma_apply(y: u8, r_uv: i32, g_uv: i32, b_uv: i32) -> (u8, u8, u8) {
    let y_term = BT601_Y_COEFF * (i32::from(y) - BT601_Y_OFFSET);
    let r = ((y_term + r_uv) >> BT601_SHIFT).clamp(0, 255) as u8;
    let g = ((y_term + g_uv) >> BT601_SHIFT).clamp(0, 255) as u8;
    let b = ((y_term + b_uv) >> BT601_SHIFT).clamp(0, 255) as u8;
    (r, g, b)
}

#[inline]
pub fn bt601_yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let (r_uv, g_uv, b_uv) = bt601_uv_terms(u, v);
    bt601_luma_apply(y, r_uv, g_uv, b_uv)
}

fn surface_bpp(surface_type: u32) -> Option<usize> {
    match surface_type {
        BINKSURFACE32 | BINKSURFACE32R | BINKSURFACE32A | BINKSURFACE32RA => Some(4),
        BINKSURFACE24 | BINKSURFACE24R => Some(3),
        BINKSURFACE555 | BINKSURFACE565 | BINKSURFACE555R | BINKSURFACE565R => Some(2),
        _ => None,
    }
}

#[inline]
fn pack_rgb565(r: u8, g: u8, b: u8) -> [u8; 2] {
    let p = (u16::from(r >> 3) << 11) | (u16::from(g >> 2) << 5) | u16::from(b >> 3);
    p.to_le_bytes()
}

fn write_pixel(dst: &mut [u8], surface_type: u32, r: u8, g: u8, b: u8, a: u8) {
    match surface_type {
        BINKSURFACE32 => {
            dst[0] = b;
            dst[1] = g;
            dst[2] = r;
            dst[3] = 255;
        }
        BINKSURFACE32A => {
            dst[0] = b;
            dst[1] = g;
            dst[2] = r;
            dst[3] = a;
        }
        BINKSURFACE32R => {
            dst[0] = r;
            dst[1] = g;
            dst[2] = b;
            dst[3] = 255;
        }
        BINKSURFACE32RA => {
            dst[0] = r;
            dst[1] = g;
            dst[2] = b;
            dst[3] = a;
        }
        BINKSURFACE24 => {
            dst[0] = b;
            dst[1] = g;
            dst[2] = r;
        }
        BINKSURFACE24R => {
            dst[0] = r;
            dst[1] = g;
            dst[2] = b;
        }
        BINKSURFACE565 => {
            dst[0..2].copy_from_slice(&pack_rgb565(r, g, b));
        }
        BINKSURFACE555 => {
            let p = (u16::from(r >> 3) << 10) | (u16::from(g >> 3) << 5) | u16::from(b >> 3);
            dst[0..2].copy_from_slice(&p.to_le_bytes());
        }
        BINKSURFACE565R => {
            let p = (u16::from(b >> 3) << 11) | (u16::from(g >> 2) << 5) | u16::from(r >> 3);
            dst[0..2].copy_from_slice(&p.to_le_bytes());
        }
        BINKSURFACE555R => {
            let p = (u16::from(b >> 3) << 10) | (u16::from(g >> 3) << 5) | u16::from(r >> 3);
            dst[0..2].copy_from_slice(&p.to_le_bytes());
        }
        _ => {}
    }
}

#[inline]
fn write_pixel_32<const R_FIRST: bool>(dst: &mut [u8], r: u8, g: u8, b: u8, a: u8) {
    if R_FIRST {
        dst[0] = r;
        dst[1] = g;
        dst[2] = b;
        dst[3] = a;
    } else {
        dst[0] = b;
        dst[1] = g;
        dst[2] = r;
        dst[3] = a;
    }
}

fn convert_yuv_row_32<const R_FIRST: bool>(
    y_row: &[u8],
    u_row: &[u8],
    v_row: &[u8],
    a_row: Option<&[u8]>,
    out_row: &mut [u8],
) {
    let uv_iter = u_row.iter().zip(v_row.iter());
    if let Some(alpha) = a_row {
        for (((yp, ap), (&u, &v)), dst) in y_row
            .chunks_exact(2)
            .zip(alpha.chunks_exact(2))
            .zip(uv_iter)
            .zip(out_row.chunks_exact_mut(8))
        {
            let (r_uv, g_uv, b_uv) = bt601_uv_terms(u, v);
            let (r0, g0, b0) = bt601_luma_apply(yp[0], r_uv, g_uv, b_uv);
            let (r1, g1, b1) = bt601_luma_apply(yp[1], r_uv, g_uv, b_uv);
            write_pixel_32::<R_FIRST>(&mut dst[0..4], r0, g0, b0, ap[0]);
            write_pixel_32::<R_FIRST>(&mut dst[4..8], r1, g1, b1, ap[1]);
        }
    } else {
        for ((yp, (&u, &v)), dst) in y_row
            .chunks_exact(2)
            .zip(uv_iter)
            .zip(out_row.chunks_exact_mut(8))
        {
            let (r_uv, g_uv, b_uv) = bt601_uv_terms(u, v);
            let (r0, g0, b0) = bt601_luma_apply(yp[0], r_uv, g_uv, b_uv);
            let (r1, g1, b1) = bt601_luma_apply(yp[1], r_uv, g_uv, b_uv);
            write_pixel_32::<R_FIRST>(&mut dst[0..4], r0, g0, b0, 255);
            write_pixel_32::<R_FIRST>(&mut dst[4..8], r1, g1, b1, 255);
        }
    }
    if !y_row.len().is_multiple_of(2) {
        let last = y_row.len() - 1;
        let (r, g, b) = bt601_yuv_to_rgb(y_row[last], u_row[last / 2], v_row[last / 2]);
        let a = a_row.map_or(255, |alpha| alpha[last]);
        write_pixel_32::<R_FIRST>(&mut out_row[last * 4..last * 4 + 4], r, g, b, a);
    }
}

fn convert_yuv_row_565(y_row: &[u8], u_row: &[u8], v_row: &[u8], out_row: &mut [u8]) {
    for ((yp, (&u, &v)), dst) in y_row
        .chunks_exact(2)
        .zip(u_row.iter().zip(v_row.iter()))
        .zip(out_row.chunks_exact_mut(4))
    {
        let (r_uv, g_uv, b_uv) = bt601_uv_terms(u, v);
        let (r0, g0, b0) = bt601_luma_apply(yp[0], r_uv, g_uv, b_uv);
        let (r1, g1, b1) = bt601_luma_apply(yp[1], r_uv, g_uv, b_uv);
        dst[0..2].copy_from_slice(&pack_rgb565(r0, g0, b0));
        dst[2..4].copy_from_slice(&pack_rgb565(r1, g1, b1));
    }
    if !y_row.len().is_multiple_of(2) {
        let last = y_row.len() - 1;
        let (r, g, b) = bt601_yuv_to_rgb(y_row[last], u_row[last / 2], v_row[last / 2]);
        out_row[last * 2..last * 2 + 2].copy_from_slice(&pack_rgb565(r, g, b));
    }
}

fn convert_yuv_row_generic(
    y_row: &[u8],
    u_row: &[u8],
    v_row: &[u8],
    surface_type: u32,
    bpp: usize,
    out_row: &mut [u8],
) {
    for ((yp, (&u, &v)), dst) in y_row
        .chunks_exact(2)
        .zip(u_row.iter().zip(v_row.iter()))
        .zip(out_row.chunks_exact_mut(bpp * 2))
    {
        let (r_uv, g_uv, b_uv) = bt601_uv_terms(u, v);
        let (r0, g0, b0) = bt601_luma_apply(yp[0], r_uv, g_uv, b_uv);
        let (r1, g1, b1) = bt601_luma_apply(yp[1], r_uv, g_uv, b_uv);
        write_pixel(&mut dst[..bpp], surface_type, r0, g0, b0, 255);
        write_pixel(&mut dst[bpp..bpp * 2], surface_type, r1, g1, b1, 255);
    }
    if !y_row.len().is_multiple_of(2) {
        let last = y_row.len() - 1;
        let (r, g, b) = bt601_yuv_to_rgb(y_row[last], u_row[last / 2], v_row[last / 2]);
        let off = last * bpp;
        write_pixel(&mut out_row[off..off + bpp], surface_type, r, g, b, 255);
    }
}

pub fn convert_yuv_row(
    frame: &DecodedVideoFrame,
    row: usize,
    copy_width: usize,
    surface_type: u32,
    out_row: &mut [u8],
) {
    let Some(bpp) = surface_bpp(surface_type) else {
        return;
    };
    if copy_width == 0 {
        return;
    }
    let w = frame.width as usize;
    let cw = w.div_ceil(2);
    let y_base = row * w;
    let uv_base = (row / 2) * cw;
    let uv_len = copy_width.div_ceil(2);

    let y_row = &frame.y[y_base..y_base + copy_width];
    let u_row = &frame.u[uv_base..uv_base + uv_len];
    let v_row = &frame.v[uv_base..uv_base + uv_len];
    let a_row = frame
        .a
        .as_deref()
        .map(|alpha| &alpha[y_base..y_base + copy_width]);
    let out_row = &mut out_row[..copy_width * bpp];

    match surface_type {
        BINKSURFACE32 => convert_yuv_row_32::<false>(y_row, u_row, v_row, None, out_row),
        BINKSURFACE32A => convert_yuv_row_32::<false>(y_row, u_row, v_row, a_row, out_row),
        BINKSURFACE32R => convert_yuv_row_32::<true>(y_row, u_row, v_row, None, out_row),
        BINKSURFACE32RA => convert_yuv_row_32::<true>(y_row, u_row, v_row, a_row, out_row),
        BINKSURFACE565 => convert_yuv_row_565(y_row, u_row, v_row, out_row),
        _ => convert_yuv_row_generic(y_row, u_row, v_row, surface_type, bpp, out_row),
    }
}

#[derive(Copy, Clone, Debug)]
pub struct GuestSurfaceTarget {
    pub dest: GuestAddr,
    pub pitch: i32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub flags: u32,
}

pub fn copy_frame_to_guest_surface(
    arena: &mut GuestArena,
    frame: &DecodedVideoFrame,
    target: GuestSurfaceTarget,
) -> Result<u32, RuntimeError> {
    let surface_type = target.flags & BINKSURFACE_MASK;
    let Some(bpp) = surface_bpp(surface_type) else {
        return Ok(1);
    };
    if target.dest == 0 || target.pitch == 0 || target.height == 0 {
        return Ok(1);
    }

    let max_rows =
        (frame.height as usize).min((target.height as i64 - i64::from(target.y)).max(0) as usize);
    let row_pitch_avail = target.pitch.unsigned_abs() as usize / bpp;
    let max_cols =
        (frame.width as usize).min(row_pitch_avail.saturating_sub(target.x.max(0) as usize));
    if max_rows == 0 || max_cols == 0 {
        return Ok(0);
    }

    let row_bytes = max_cols * bpp;
    if target.pitch > 0 && target.y >= 0 && target.x >= 0 {
        let pitch = target.pitch as usize;
        let first_row = i64::from(target.dest)
            + i64::from(target.y) * i64::from(target.pitch)
            + i64::from(target.x) * (bpp as i64);
        let total_span = (max_rows - 1) * pitch + row_bytes;
        if first_row > 0
            && u32::try_from(first_row).is_ok()
            && (first_row as u64)
                .checked_add(total_span as u64)
                .is_some_and(|end| end <= u64::from(u32::MAX))
        {
            let dst_span = arena.write_slice_mut(first_row as GuestAddr, total_span)?;
            for row in 0..max_rows {
                let row_off = row * pitch;
                convert_yuv_row(
                    frame,
                    row,
                    max_cols,
                    surface_type,
                    &mut dst_span[row_off..row_off + row_bytes],
                );
            }
            return Ok(0);
        }
    }

    let dst_x_coord = i64::from(target.x).max(0);
    for row in 0..max_rows {
        let dst_y_coord = i64::from(target.y) + row as i64;
        if dst_y_coord < 0 || dst_y_coord >= i64::from(target.height) {
            continue;
        }
        let row_addr = i64::from(target.dest)
            + dst_y_coord * i64::from(target.pitch)
            + dst_x_coord * (bpp as i64);
        if row_addr <= 0 || row_addr > i64::from(u32::MAX) {
            continue;
        }
        let dst_row = arena.write_slice_mut(row_addr as GuestAddr, row_bytes)?;
        convert_yuv_row(frame, row, max_cols, surface_type, dst_row);
    }
    Ok(0)
}

struct BinkInstance {
    header: BinkHeader,
    open_flags: u32,
    frame_num: u32,
    last_frame_num: u32,
    paused: bool,
    sound_on: bool,
    selected_audio_track: Option<BinkAudioTrack>,
    next_frame_due: Instant,
    start_audio_frames: u64,
    av_err_integral: f64,
    av_rate_ratio: f64,
    open_instant: Instant,
    first_frame_instant: Option<Instant>,
    wait_true: u32,
    wait_false: u32,
    dec_ns: u64,
    yuv_ns: u64,
    decoder: Option<FfmpegDecoder>,
    current_frame: Option<DecodedVideoFrame>,
}

pub struct BinkState {
    ffmpeg: Option<FfmpegApi>,
    ffmpeg_load_attempted: bool,
    logged_missing_ffmpeg: bool,
    sound_tracks_disabled: bool,
    requested_sound_tracks: Vec<u32>,
    instances: HashMap<GuestAddr, BinkInstance>,
}

impl Default for BinkState {
    fn default() -> Self {
        Self::new()
    }
}

impl BinkState {
    pub fn new() -> Self {
        Self {
            ffmpeg: None,
            ffmpeg_load_attempted: false,
            logged_missing_ffmpeg: false,
            sound_tracks_disabled: false,
            requested_sound_tracks: Vec::new(),
            instances: HashMap::new(),
        }
    }

    fn ensure_ffmpeg(&mut self) -> Option<&FfmpegApi> {
        if !self.ffmpeg_load_attempted {
            self.ffmpeg_load_attempted = true;
            self.ffmpeg = FfmpegApi::try_load();
        }
        self.ffmpeg.as_ref()
    }
}

impl Drop for BinkState {
    fn drop(&mut self) {
        if let Some(api) = self.ffmpeg.as_ref() {
            for (_, inst) in self.instances.drain() {
                if let Some(dec) = inst.decoder {
                    dec.close(api);
                }
            }
        }
    }
}

pub fn register_imports(registry: &mut ImportRegistry) {
    let bink = "binkw32.dll";
    let entries: &[(&str, ImportHandler)] = &[
        ("_BinkSetSoundOnOff@8", bink_set_sound_on_off),
        ("_BinkOpen@8", bink_open),
        ("_BinkSetSoundTrack@8", bink_set_sound_track),
        ("_BinkSetSoundSystem@8", bink_set_sound_system),
        ("_BinkOpenDirectSound@4", bink_open_direct_sound),
        ("_BinkClose@4", bink_close),
        ("_BinkCopyToBuffer@28", bink_copy_to_buffer),
        ("_BinkNextFrame@4", bink_next_frame),
        ("_BinkDoFrame@4", bink_do_frame),
        ("_BinkWait@4", bink_wait),
        ("_BinkPause@8", bink_pause),
    ];
    for &(name, handler) in entries {
        registry.register_name(bink, name, handler);
    }
}

fn write_guest_bink_struct(
    arena: &mut GuestArena,
    va: GuestAddr,
    header: &BinkHeader,
    frame_num: u32,
    last_frame_num: u32,
    open_flags: u32,
) -> Result<(), RuntimeError> {
    arena.write_u32(va, header.width)?;
    arena.write_u32(va + 0x04, header.height)?;
    arena.write_u32(va + 0x08, header.num_frames)?;
    arena.write_u32(va + 0x0C, frame_num)?;
    arena.write_u32(va + 0x10, last_frame_num)?;
    arena.write_u32(va + 0x14, header.fps_num)?;
    arena.write_u32(va + 0x18, header.fps_den)?;
    arena.write_u32(va + 0x1C, 0)?;
    arena.write_u32(va + 0x20, open_flags)?;
    arena.write_u32(va + 0x24, u32::from_le_bytes(header.magic))?;
    arena.write_u32(va + 0x28, header.file_size)?;
    arena.write_u32(va + 0x2C, header.largest_frame_size)?;
    arena.write_u32(va + 0x30, 0)?;
    Ok(())
}

fn read_bink_header_from_file(path: &Path) -> Option<BinkHeader> {
    let mut file = File::open(path).ok()?;
    let mut buf = [0u8; 4096];
    let n = file.read(&mut buf).ok()?;
    parse_bink_header(&buf[..n])
}

fn select_audio_track(
    header: &BinkHeader,
    requested_tracks: &[u32],
    sound_tracks_disabled: bool,
) -> (usize, Option<BinkAudioTrack>) {
    if sound_tracks_disabled || header.audio_tracks.is_empty() {
        return (0, None);
    }
    if !requested_tracks.is_empty() {
        for &req_id in requested_tracks {
            if let Some((idx, &track)) = header
                .audio_tracks
                .iter()
                .enumerate()
                .find(|(_, t)| t.track_id == req_id)
            {
                return (idx, Some(track));
            }
        }
    }
    (0, Some(header.audio_tracks[0]))
}

const BINK_PERF_LOG_FRAME_STRIDE: u32 = 30;

fn bink_perf_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("TOLUCA_PERF").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

fn log_bink_perf(inst: &BinkInstance, mixer_mutex: &std::sync::Mutex<DsoundMixer>) {
    let open_ms = inst.open_instant.elapsed().as_millis();
    let play_ms = inst
        .first_frame_instant
        .map_or(0, |t| t.elapsed().as_millis());
    let vid_ms = inst.header.frame_duration().as_nanos()
        * u128::from(inst.frame_num.saturating_sub(1))
        / 1_000_000;
    let (aud_ms, aud_q_ms) = if let Ok(mixer) = mixer_mutex.lock() {
        let rate = u64::from(mixer.output_sample_rate().max(1));
        let played = mixer
            .aux_played_frames()
            .saturating_sub(inst.start_audio_frames)
            * 1000
            / rate;
        let queued = (mixer.aux_queued_frames() as u64) * 1000 / rate;
        (played, queued)
    } else {
        (0, 0)
    };
    let dec_ms = (inst.dec_ns as f64) / 1_000_000.0;
    let yuv_ms = (inst.yuv_ns as f64) / 1_000_000.0;
    eprintln!(
        "[toluca-bink-perf] frame={}/{} open_ms={open_ms} play_ms={play_ms} vid_ms={vid_ms} aud_ms={aud_ms} aud_q_ms={aud_q_ms} wait_t={} wait_f={} dec_ms={dec_ms:.1} yuv_ms={yuv_ms:.1} rate={:.4}",
        inst.frame_num,
        inst.header.num_frames,
        inst.wait_true,
        inst.wait_false,
        inst.av_rate_ratio,
    );
}

fn bink_open(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let name_ptr = peek_arg_u32(cpu, ctx.arena, 0)?;
    let flags = peek_arg_u32(cpu, ctx.arena, 1)?;
    if name_ptr == 0 {
        return_stdcall(cpu, ctx.arena, 2, 0)?;
        return Ok(());
    }

    let win_path = ctx.arena.read_cstr(name_ptr, MAX_PATH_LEN)?;
    let host_path = ctx.win32.resolve_path(&win_path);
    let Some(header) = read_bink_header_from_file(&host_path) else {
        return_stdcall(cpu, ctx.arena, 2, 0)?;
        return Ok(());
    };

    let (audio_ordinal, selected_audio_track) = select_audio_track(
        &header,
        &ctx.win32.bink.requested_sound_tracks,
        ctx.win32.bink.sound_tracks_disabled,
    );
    if bink_perf_enabled() {
        let track_rate = selected_audio_track.map_or(0, |t| t.sample_rate);
        let track_ch = selected_audio_track.map_or(0, |t| t.channels);
        eprintln!(
            "[toluca-bink] open file={} {}x{} fps={}/{} track_rate={} ch={}",
            win_path,
            header.width,
            header.height,
            header.fps_num,
            header.fps_den,
            track_rate,
            track_ch
        );
    }
    let decoder = if let Some(api) = ctx.win32.bink.ensure_ffmpeg() {
        FfmpegDecoder::open(api, &host_path, audio_ordinal)
    } else {
        if !ctx.win32.config.headless && !ctx.win32.bink.logged_missing_ffmpeg {
            ctx.win32.bink.logged_missing_ffmpeg = true;
            eprintln!("toluca: FFmpeg (libavcodec/libavformat/libavutil) not found; skipping Bink FMV playback");
        }
        None
    };

    let bink_va = ctx.heap.alloc(ctx.arena, BINK_STRUCT_ALLOC_BYTES, true)?;
    write_guest_bink_struct(ctx.arena, bink_va, &header, 1, 0, flags)?;

    let frame_period = header.frame_duration();
    let sound_on = selected_audio_track.is_some();
    let start_audio_frames = if sound_on {
        if let Ok(mut mixer) = ctx.win32.dsound.mixer.lock() {
            mixer.clear_aux_pcm();
            mixer.aux_played_frames()
        } else {
            0
        }
    } else {
        0
    };
    let now = Instant::now();
    ctx.win32.bink.instances.insert(
        bink_va,
        BinkInstance {
            header,
            open_flags: flags,
            frame_num: 1,
            last_frame_num: 0,
            paused: false,
            sound_on,
            selected_audio_track,
            next_frame_due: now + frame_period,
            start_audio_frames,
            av_err_integral: 0.0,
            av_rate_ratio: 1.0,
            open_instant: now,
            first_frame_instant: None,
            wait_true: 0,
            wait_false: 0,
            dec_ns: 0,
            yuv_ns: 0,
            decoder,
            current_frame: None,
        },
    );

    return_stdcall(cpu, ctx.arena, 2, bink_va)?;
    Ok(())
}

fn bink_close(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let bink_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    if let Some(mut inst) = ctx.win32.bink.instances.remove(&bink_va) {
        if bink_perf_enabled() {
            log_bink_perf(&inst, &ctx.win32.dsound.mixer);
        }
        inst.av_err_integral = 0.0;
        inst.av_rate_ratio = 1.0;
        if let (Some(dec), Some(api)) = (inst.decoder, ctx.win32.bink.ffmpeg.as_ref()) {
            dec.close(api);
        }
        if let Ok(mut mixer) = ctx.win32.dsound.mixer.lock() {
            mixer.clear_aux_pcm();
            mixer.set_aux_rate_ratio(1.0);
        }
        let _ = ctx.heap.free(ctx.arena, bink_va);
    }
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn bink_set_sound_track(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let total_tracks = peek_arg_u32(cpu, ctx.arena, 0)?;
    let track_ids_ptr = peek_arg_u32(cpu, ctx.arena, 1)?;
    ctx.win32.bink.requested_sound_tracks.clear();
    ctx.win32.bink.sound_tracks_disabled = total_tracks == 0;
    if total_tracks > 0 && track_ids_ptr != 0 {
        let count = total_tracks.min(32);
        for i in 0..count {
            let tid = ctx.arena.read_u32(track_ids_ptr + i * 4)?;
            ctx.win32.bink.requested_sound_tracks.push(tid);
        }
    }
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn bink_set_sound_system(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    return_stdcall(cpu, ctx.arena, 2, 1)?;
    Ok(())
}

fn bink_open_direct_sound(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    ctx.win32
        .dsound
        .ensure_audio_opened(ctx.win32.window_mgr.sdl.as_mut());
    return_stdcall(cpu, ctx.arena, 1, 1)?;
    Ok(())
}

fn bink_set_sound_on_off(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let bink_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    let on_off = peek_arg_i32(cpu, ctx.arena, 1)?;
    let (ret, turned_on, changed) = if let Some(inst) = ctx.win32.bink.instances.get_mut(&bink_va) {
        let was_on = inst.sound_on;
        inst.sound_on = on_off != 0;
        let changed = was_on != inst.sound_on;
        if changed {
            inst.av_err_integral = 0.0;
            inst.av_rate_ratio = 1.0;
        }
        (
            i32::from(inst.sound_on) as u32,
            !was_on && inst.sound_on,
            changed,
        )
    } else {
        (0, false, false)
    };
    if turned_on || changed {
        if let Ok(mut mixer) = ctx.win32.dsound.mixer.lock() {
            if turned_on {
                mixer.clear_aux_pcm();
                if let Some(inst) = ctx.win32.bink.instances.get_mut(&bink_va) {
                    inst.start_audio_frames = mixer.aux_played_frames();
                }
            }
            mixer.set_aux_rate_ratio(1.0);
        }
    }
    return_stdcall(cpu, ctx.arena, 2, ret)?;
    Ok(())
}

fn bink_pause(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let bink_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    let pause = peek_arg_i32(cpu, ctx.arena, 1)?;
    let ret = if let Some(inst) = ctx.win32.bink.instances.get_mut(&bink_va) {
        inst.paused = pause != 0;
        i32::from(inst.paused) as u32
    } else {
        0
    };
    return_stdcall(cpu, ctx.arena, 2, ret)?;
    Ok(())
}

fn should_wait_for_frame(
    inst: &BinkInstance,
    mixer_mutex: &std::sync::Mutex<DsoundMixer>,
) -> bool {
    if inst.paused {
        return true;
    }
    if inst.sound_on {
        if let Ok(mixer) = mixer_mutex.lock() {
            let played = mixer.aux_played_frames();
            if mixer.hardware_clocked
                && played > inst.start_audio_frames
                && mixer.aux_queued_frames() > 0
            {
                let elapsed_frames = played - inst.start_audio_frames;
                let rate = u128::from(mixer.output_sample_rate().max(1));
                let elapsed_ns = u128::from(elapsed_frames) * 1_000_000_000 / rate;
                let target_ns = inst.header.frame_duration().as_nanos()
                    * u128::from(inst.frame_num.saturating_sub(1));
                return elapsed_ns < target_ns;
            }
        }
    }
    Instant::now() < inst.next_frame_due
}

fn bink_wait(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let bink_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    if ctx.win32.config.headless || ctx.win32.config.max_frames.is_some() {
        return_stdcall(cpu, ctx.arena, 1, 0)?;
        return Ok(());
    }
    let wait = match ctx.win32.bink.instances.get_mut(&bink_va) {
        Some(inst) if inst.decoder.is_some() => {
            let should_wait = should_wait_for_frame(inst, &ctx.win32.dsound.mixer);
            if bink_perf_enabled() {
                if should_wait {
                    inst.wait_true = inst.wait_true.saturating_add(1);
                } else {
                    inst.wait_false = inst.wait_false.saturating_add(1);
                }
            }
            u32::from(should_wait)
        }
        _ => 0,
    };
    return_stdcall(cpu, ctx.arena, 1, wait)?;
    Ok(())
}

fn bink_do_frame(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let bink_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    let Some(inst) = ctx.win32.bink.instances.get_mut(&bink_va) else {
        return_stdcall(cpu, ctx.arena, 1, 0)?;
        return Ok(());
    };

    let perf = bink_perf_enabled();
    if perf {
        inst.first_frame_instant.get_or_insert_with(Instant::now);
    }
    let dec_t0 = perf.then(Instant::now);
    let (decoded_vid, decoded_aud) = if let (Some(dec), Some(api)) =
        (inst.decoder.as_mut(), ctx.win32.bink.ffmpeg.as_ref())
    {
        dec.decode_next_video_frame(api, inst.selected_audio_track, inst.sound_on)
    } else {
        (None, Vec::new())
    };
    if let Some(start) = dec_t0 {
        inst.dec_ns += start.elapsed().as_nanos() as u64;
    }

    if let Some(vid) = decoded_vid {
        inst.current_frame = Some(vid);
    } else if inst.current_frame.is_none() {
        inst.current_frame = Some(DecodedVideoFrame::black(
            inst.header.width,
            inst.header.height,
        ));
    }

    if inst.sound_on {
        let track = inst
            .selected_audio_track
            .or_else(|| inst.header.audio_tracks.first().copied());
        let rate = track.map_or(DEFAULT_AUDIO_RATE, |t| u32::from(t.sample_rate.max(1)));
        let ch = track.map_or(DEFAULT_AUDIO_CHANNELS, |t| t.channels);
        if let Ok(mut mixer) = ctx.win32.dsound.mixer.lock() {
            if !decoded_aud.is_empty() {
                push_aux_to_mixer(&mut mixer, &decoded_aud, rate, ch);
            }
            update_aux_rate_control(inst, &mut mixer);
        }
    }

    inst.last_frame_num = inst.frame_num;
    write_guest_bink_struct(
        ctx.arena,
        bink_va,
        &inst.header,
        inst.frame_num,
        inst.last_frame_num,
        inst.open_flags,
    )?;

    if perf
        && (inst.frame_num == 1
            || inst.frame_num == 2
            || inst.frame_num.is_multiple_of(BINK_PERF_LOG_FRAME_STRIDE))
    {
        log_bink_perf(inst, &ctx.win32.dsound.mixer);
        if inst.frame_num.is_multiple_of(BINK_PERF_LOG_FRAME_STRIDE) {
            inst.wait_true = 0;
            inst.wait_false = 0;
            inst.dec_ns = 0;
            inst.yuv_ns = 0;
        }
    }

    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn push_aux_to_mixer(mixer: &mut DsoundMixer, samples: &[i16], rate: u32, channels: u16) {
    mixer.push_aux_pcm(samples, rate, channels);
}

fn update_aux_rate_control(inst: &mut BinkInstance, mixer: &mut DsoundMixer) {
    if !inst.sound_on || !mixer.hardware_clocked || mixer.aux_queued_frames() == 0 || inst.frame_num <= 1 {
        return;
    }
    let played_frames = mixer.aux_played_frames().saturating_sub(inst.start_audio_frames);
    if played_frames == 0 {
        return;
    }
    let rate = f64::from(mixer.output_sample_rate().max(1));
    let aud_sec = (played_frames as f64) / rate;
    let dt = inst.header.frame_duration().as_secs_f64();
    let vid_sec = dt * f64::from(inst.frame_num.saturating_sub(1));
    let err_sec = aud_sec - vid_sec;
    inst.av_err_integral =
        (inst.av_err_integral + err_sec * dt).clamp(-AV_SYNC_INTEGRAL_LIMIT, AV_SYNC_INTEGRAL_LIMIT);
    let target_ratio = (1.0 - AV_SYNC_KP * err_sec - AV_SYNC_KI * inst.av_err_integral)
        .clamp(1.0 - MAX_AUX_RATE_SKEW, 1.0 + MAX_AUX_RATE_SKEW);
    inst.av_rate_ratio += (target_ratio - inst.av_rate_ratio) * AV_SYNC_RATIO_SMOOTHING;
    mixer.set_aux_rate_ratio(inst.av_rate_ratio);
}

fn bink_next_frame(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let bink_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    if let Some(inst) = ctx.win32.bink.instances.get_mut(&bink_va) {
        if inst.frame_num >= inst.header.num_frames {
            inst.frame_num = 1;
            inst.av_err_integral = 0.0;
            inst.av_rate_ratio = 1.0;
            if let (Some(dec), Some(api)) = (inst.decoder.as_mut(), ctx.win32.bink.ffmpeg.as_ref()) {
                dec.rewind(api);
            }
            if let Ok(mut mixer) = ctx.win32.dsound.mixer.lock() {
                if inst.sound_on {
                    inst.start_audio_frames = mixer.aux_played_frames();
                }
                mixer.set_aux_rate_ratio(1.0);
            }
        } else {
            inst.frame_num += 1;
        }
        let frame_period = inst.header.frame_duration();
        let now = Instant::now();
        let min_due = now.checked_sub(frame_period * 2).unwrap_or(now);
        inst.next_frame_due = (inst.next_frame_due + frame_period).max(min_due);
        write_guest_bink_struct(
            ctx.arena,
            bink_va,
            &inst.header,
            inst.frame_num,
            inst.last_frame_num,
            inst.open_flags,
        )?;
    }
    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn bink_copy_to_buffer(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let bink_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    let dest = peek_arg_u32(cpu, ctx.arena, 1)?;
    let dest_pitch = peek_arg_i32(cpu, ctx.arena, 2)?;
    let dest_height = peek_arg_u32(cpu, ctx.arena, 3)?;
    let dest_x = peek_arg_i32(cpu, ctx.arena, 4)?;
    let dest_y = peek_arg_i32(cpu, ctx.arena, 5)?;
    let flags = peek_arg_u32(cpu, ctx.arena, 6)?;

    let Some(frame) = ctx
        .win32
        .bink
        .instances
        .get(&bink_va)
        .and_then(|inst| inst.current_frame.clone())
    else {
        return_stdcall(cpu, ctx.arena, 7, 1)?;
        return Ok(());
    };

    let yuv_t0 = bink_perf_enabled().then(Instant::now);
    let ret = copy_frame_to_guest_surface(
        ctx.arena,
        &frame,
        GuestSurfaceTarget {
            dest,
            pitch: dest_pitch,
            height: dest_height,
            x: dest_x,
            y: dest_y,
            flags,
        },
    )?;
    if let Some(start) = yuv_t0 {
        if let Some(inst) = ctx.win32.bink.instances.get_mut(&bink_va) {
            inst.yuv_ns += start.elapsed().as_nanos() as u64;
        }
    }
    return_stdcall(cpu, ctx.arena, 7, ret)?;
    Ok(())
}

fn open_first_lib(candidates: &[&std::ffi::CStr]) -> Option<*mut c_void> {
    for &name in candidates {
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use toluca_core::memory::DEFAULT_IMAGE_BASE;

    use crate::heap::GuestHeap;

    #[test]
    fn parses_synthetic_bink1_header_with_audio_tracks() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"BIKi");
        raw.extend_from_slice(&123_456u32.to_le_bytes());
        raw.extend_from_slice(&140u32.to_le_bytes());
        raw.extend_from_slice(&32_000u32.to_le_bytes());
        raw.extend_from_slice(&140u32.to_le_bytes());
        raw.extend_from_slice(&640u32.to_le_bytes());
        raw.extend_from_slice(&480u32.to_le_bytes());
        raw.extend_from_slice(&42_000u32.to_le_bytes());
        raw.extend_from_slice(&1411u32.to_le_bytes());
        raw.extend_from_slice(&0u32.to_le_bytes());
        raw.extend_from_slice(&2u32.to_le_bytes());
        raw.extend_from_slice(&4096u32.to_le_bytes());
        raw.extend_from_slice(&2048u32.to_le_bytes());
        raw.extend_from_slice(&48_000u16.to_le_bytes());
        raw.extend_from_slice(&0x7000u16.to_le_bytes());
        raw.extend_from_slice(&22_050u16.to_le_bytes());
        raw.extend_from_slice(&0x1000u16.to_le_bytes());
        raw.extend_from_slice(&10u32.to_le_bytes());
        raw.extend_from_slice(&20u32.to_le_bytes());

        let hdr = parse_bink_header(&raw).unwrap();
        assert_eq!(hdr.width, 640);
        assert_eq!(hdr.height, 480);
        assert_eq!(hdr.num_frames, 140);
        assert_eq!(hdr.fps_num, 42_000);
        assert_eq!(hdr.fps_den, 1411);
        assert_eq!(hdr.audio_tracks.len(), 2);
        assert_eq!(hdr.audio_tracks[0].sample_rate, 48_000);
        assert_eq!(hdr.audio_tracks[0].channels, 2);
        assert_eq!(hdr.audio_tracks[0].track_id, 10);
        assert_eq!(hdr.audio_tracks[1].sample_rate, 22_050);
        assert_eq!(hdr.audio_tracks[1].channels, 1);
        assert_eq!(hdr.audio_tracks[1].track_id, 20);
    }

    #[test]
    fn bt601_yuv_conversion_and_guest_surface_formats() {
        assert_eq!(bt601_yuv_to_rgb(16, 128, 128), (0, 0, 0));
        assert_eq!(bt601_yuv_to_rgb(235, 128, 128), (255, 255, 255));
        let (rr, rg, rb) = bt601_yuv_to_rgb(81, 90, 240);
        assert!(rr > 240 && rg < 15 && rb < 15);
        let (gr, gg, gb) = bt601_yuv_to_rgb(145, 54, 34);
        assert!(gr < 15 && gg > 240 && gb < 15);
        let (br, bg, bb) = bt601_yuv_to_rgb(41, 240, 110);
        assert!(br < 15 && bg < 15 && bb > 240);

        let frame = DecodedVideoFrame {
            width: 2,
            height: 2,
            y: vec![235, 16, 81, 41],
            u: vec![128],
            v: vec![128],
            a: None,
        };
        let mut arena = GuestArena::new().unwrap();
        arena.init_thread_environment(DEFAULT_IMAGE_BASE).unwrap();
        let mut heap = GuestHeap::new();
        let surf = heap.alloc(&mut arena, 64, true).unwrap();

        assert_eq!(
            copy_frame_to_guest_surface(
                &mut arena,
                &frame,
                GuestSurfaceTarget {
                    dest: surf,
                    pitch: 16,
                    height: 4,
                    x: 1,
                    y: 1,
                    flags: BINKSURFACE32,
                },
            )
            .unwrap(),
            0
        );
        let p00 = arena.read_bytes(surf + 16 + 4, 4).unwrap();
        assert_eq!(p00, &[255, 255, 255, 255]);
        let p01 = arena.read_bytes(surf + 16 + 8, 4).unwrap();
        assert_eq!(p01, &[0, 0, 0, 255]);

        assert_eq!(
            copy_frame_to_guest_surface(
                &mut arena,
                &frame,
                GuestSurfaceTarget {
                    dest: surf,
                    pitch: 8,
                    height: 2,
                    x: 0,
                    y: 0,
                    flags: BINKSURFACE32R,
                },
            )
            .unwrap(),
            0
        );
        assert_eq!(arena.read_bytes(surf, 4).unwrap(), &[255, 255, 255, 255]);

        assert_eq!(
            copy_frame_to_guest_surface(
                &mut arena,
                &frame,
                GuestSurfaceTarget {
                    dest: surf,
                    pitch: 4,
                    height: 2,
                    x: 0,
                    y: 0,
                    flags: BINKSURFACE565,
                },
            )
            .unwrap(),
            0
        );
        assert_eq!(arena.read_u16(surf).unwrap(), 0xFFFF);
        assert_eq!(arena.read_u16(surf + 2).unwrap(), 0x0000);

        assert_eq!(
            copy_frame_to_guest_surface(
                &mut arena,
                &frame,
                GuestSurfaceTarget {
                    dest: surf,
                    pitch: 4,
                    height: 2,
                    x: 0,
                    y: 0,
                    flags: BINKSURFACE555,
                },
            )
            .unwrap(),
            0
        );
        assert_eq!(arena.read_u16(surf).unwrap(), 0x7FFF);
        assert_eq!(arena.read_u16(surf + 2).unwrap(), 0x0000);
    }

    #[test]
    fn planar_f32_to_interleaved_i16_clamps_and_interleaves() {
        let left = [0.0f32, 1.5, -1.5];
        let right = [-0.5f32, 0.5, 0.0];
        let mut out = Vec::new();
        planar_f32_to_interleaved_i16(&[&left, &right], &mut out);
        assert_eq!(out, vec![0, -16_384, 32_767, 16_384, -32_767, 0]);
    }

    #[test]
    fn real_bik_decode_smoke_test() {
        let bik_path = PathBuf::from("../../gamedata/Silent Hill 2/data/movie/flash.bik");
        if !bik_path.is_file() {
            return;
        }
        let Some(api) = FfmpegApi::try_load() else {
            return;
        };
        let header = read_bink_header_from_file(&bik_path).unwrap();
        assert_eq!(header.width, 640);
        assert_eq!(header.height, 480);

        let mut dec = FfmpegDecoder::open(&api, &bik_path, 0).unwrap();
        let mut frames = Vec::new();
        let mut audio_samples = 0usize;
        for _ in 0..5 {
            let (vid, aud) = dec.decode_next_video_frame(
                &api,
                header.audio_tracks.first().copied(),
                true,
            );
            audio_samples += aud.len();
            if let Some(frame) = vid {
                frames.push(frame);
            }
        }
        dec.close(&api);

        assert!(frames.len() >= 3);
        assert!(audio_samples > 0);
        let first = &frames[0];
        assert_eq!(first.width, 640);
        assert_eq!(first.height, 480);
        assert_eq!(first.y.len(), 640 * 480);
        assert_eq!(first.u.len(), 320 * 240);
        assert_eq!(first.v.len(), 320 * 240);

        let mut row_bgra = vec![0u8; 640 * 4];
        convert_yuv_row(first, 0, 640, BINKSURFACE32, &mut row_bgra);
        assert_eq!(row_bgra.len(), 640 * 4);
        assert!(row_bgra.chunks_exact(4).all(|px| px[3] == 255));
    }

    #[test]
    fn select_audio_track_keeps_sound_with_binknoskip_and_disables_on_zero_tracks() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"BIKi");
        raw.extend_from_slice(&123_456u32.to_le_bytes());
        raw.extend_from_slice(&10u32.to_le_bytes());
        raw.extend_from_slice(&4096u32.to_le_bytes());
        raw.extend_from_slice(&10u32.to_le_bytes());
        raw.extend_from_slice(&640u32.to_le_bytes());
        raw.extend_from_slice(&480u32.to_le_bytes());
        raw.extend_from_slice(&30u32.to_le_bytes());
        raw.extend_from_slice(&1u32.to_le_bytes());
        raw.extend_from_slice(&(BINK_FLAG_NOSKIP | 0x4000).to_le_bytes());
        raw.extend_from_slice(&1u32.to_le_bytes());
        raw.extend_from_slice(&4096u32.to_le_bytes());
        raw.extend_from_slice(&48_000u16.to_le_bytes());
        raw.extend_from_slice(&0x7000u16.to_le_bytes());
        raw.extend_from_slice(&0u32.to_le_bytes());

        let hdr = parse_bink_header(&raw).unwrap();
        let (ord, track) = select_audio_track(&hdr, &[0], false);
        assert_eq!(ord, 0);
        let track = track.expect("0x0008_4000 (BINKSNDTRACK | BINKNOSKIP) must keep audio");
        assert_eq!(track.sample_rate, 48_000);
        assert_eq!(track.channels, 2);

        let (_, disabled) = select_audio_track(&hdr, &[], true);
        assert!(disabled.is_none());
    }

    #[test]
    fn should_wait_for_frame_tracks_audio_clock_without_holdback_and_falls_back_when_drained() {
        let header = BinkHeader {
            magic: *b"BIKi",
            file_size: 1024,
            num_frames: 60,
            largest_frame_size: 256,
            width: 640,
            height: 480,
            fps_num: 30,
            fps_den: 1,
            video_flags: 0,
            audio_tracks: vec![BinkAudioTrack {
                track_id: 0,
                sample_rate: 48_000,
                channels: 2,
            }],
        };

        let mixer_mutex = std::sync::Mutex::new(DsoundMixer::new());
        {
            let mut mixer = mixer_mutex.lock().unwrap();
            mixer.hardware_clocked = true;
            mixer.output_sample_rate = 48_000;
            mixer.aux_played_frames = 5_799; // 4,799 frames past start (< 4,800 needed for frame 4 @ 30fps)
            mixer.push_aux_pcm(&[1, 2], 48_000, 2);
        }

        let mut inst = BinkInstance {
            header,
            open_flags: 0,
            frame_num: 4, // (4 - 1) * (1/30 s) = 100 ms = 4,800 frames @ 48 kHz
            last_frame_num: 3,
            paused: false,
            sound_on: true,
            selected_audio_track: None,
            next_frame_due: Instant::now() + Duration::from_secs(5),
            start_audio_frames: 1_000,
            av_err_integral: 0.0,
            av_rate_ratio: 1.0,
            open_instant: Instant::now(),
            first_frame_instant: None,
            wait_true: 0,
            wait_false: 0,
            dec_ns: 0,
            yuv_ns: 0,
            decoder: None,
            current_frame: None,
        };

        // Steady state: 4,799 frames played -> waits; 4,800 frames played -> advances immediately (no 85ms holdback).
        assert!(should_wait_for_frame(&inst, &mixer_mutex));
        mixer_mutex.lock().unwrap().aux_played_frames = 5_800;
        assert!(!should_wait_for_frame(&inst, &mixer_mutex));

        // End-of-FMV: aux_pcm drained, audio clock behind frame_num -> falls back to next_frame_due without hanging.
        {
            let mut mixer = mixer_mutex.lock().unwrap();
            mixer.clear_aux_pcm();
            mixer.aux_played_frames = 1_100;
            assert_eq!(mixer.aux_queued_frames(), 0);
        }
        inst.frame_num = 10;
        inst.next_frame_due = Instant::now() - Duration::from_millis(1);
        assert!(!should_wait_for_frame(&inst, &mixer_mutex));
        inst.next_frame_due = Instant::now() + Duration::from_millis(50);
        assert!(should_wait_for_frame(&inst, &mixer_mutex));
    }

    #[test]
    fn update_aux_rate_control_tracks_fast_hardware_clock_without_drift() {
        let header = BinkHeader {
            magic: *b"BIKi",
            file_size: 1024,
            num_frames: 700,
            largest_frame_size: 256,
            width: 640,
            height: 480,
            fps_num: 30,
            fps_den: 1,
            video_flags: 0,
            audio_tracks: vec![BinkAudioTrack {
                track_id: 0,
                sample_rate: 48_000,
                channels: 2,
            }],
        };

        let mut mixer = DsoundMixer::new();
        mixer.hardware_clocked = true;
        mixer.output_sample_rate = 48_000;
        let pcm_chunk = vec![100i16; 1600 * 2];
        for _ in 0..5 {
            mixer.push_aux_pcm(&pcm_chunk, 48_000, 2);
        }

        let mut inst = BinkInstance {
            header,
            open_flags: 0,
            frame_num: 1,
            last_frame_num: 0,
            paused: false,
            sound_on: true,
            selected_audio_track: None,
            next_frame_due: Instant::now(),
            start_audio_frames: 0,
            av_err_integral: 0.0,
            av_rate_ratio: 1.0,
            open_instant: Instant::now(),
            first_frame_instant: None,
            wait_true: 0,
            wait_false: 0,
            dec_ns: 0,
            yuv_ns: 0,
            decoder: None,
            current_frame: None,
        };

        let mut out_accum = 0.0f64;
        let mut out_buf = Vec::new();
        let mut final_err_ms = 0.0f64;
        for _ in 0..600 {
            inst.frame_num += 1;
            mixer.push_aux_pcm(&pcm_chunk, 48_000, 2);
            out_accum += 1600.0 * 1.0145;
            let out_frames = out_accum.floor() as usize;
            out_accum -= out_frames as f64;
            out_buf.resize(out_frames * 2, 0);
            mixer.mix_into(&mut out_buf);
            update_aux_rate_control(&mut inst, &mut mixer);

            let aud_ms = (mixer.aux_played_frames() as f64) * 1000.0 / 48_000.0;
            let vid_ms = f64::from(inst.frame_num - 1) * 1000.0 / 30.0;
            final_err_ms = (aud_ms - vid_ms).abs();
            assert!(
                final_err_ms < 50.0,
                "A/V drift exceeded 50ms at frame {}: {final_err_ms:.2}ms",
                inst.frame_num
            );
        }

        assert!(
            final_err_ms < 15.0,
            "expected final A/V error < 15ms at frame 600, got {final_err_ms:.2}ms"
        );
        assert!(
            (0.980..=0.990).contains(&inst.av_rate_ratio),
            "expected converged av_rate_ratio near 0.985, got {:.5}",
            inst.av_rate_ratio
        );
    }

    #[test]
    fn convert_yuv_row_matches_reference_all_formats_even_odd_and_alpha() {
        let formats = [
            BINKSURFACE24,
            BINKSURFACE24R,
            BINKSURFACE32,
            BINKSURFACE32R,
            BINKSURFACE32A,
            BINKSURFACE32RA,
            BINKSURFACE555R,
            BINKSURFACE565R,
            BINKSURFACE555,
            BINKSURFACE565,
        ];
        for width in [6u32, 5u32] {
            let height = 2u32;
            let w = width as usize;
            let cw = w.div_ceil(2);
            let y: Vec<u8> = (0..(w * height as usize))
                .map(|i| ((i * 29 + 16) & 0xFF) as u8)
                .collect();
            let u: Vec<u8> = (0..cw).map(|i| ((i * 53 + 64) & 0xFF) as u8).collect();
            let v: Vec<u8> = (0..cw).map(|i| ((i * 71 + 192) & 0xFF) as u8).collect();
            let alpha: Vec<u8> = (0..(w * height as usize))
                .map(|i| ((i * 37 + 11) & 0xFF) as u8)
                .collect();

            for with_alpha in [false, true] {
                let frame = DecodedVideoFrame {
                    width,
                    height,
                    y: y.clone(),
                    u: u.clone(),
                    v: v.clone(),
                    a: with_alpha.then(|| alpha.clone()),
                };
                for &surface_type in &formats {
                    let bpp = surface_bpp(surface_type).unwrap();
                    for row in 0..height as usize {
                        let mut actual = vec![0xAAu8; w * bpp];
                        convert_yuv_row(&frame, row, w, surface_type, &mut actual);

                        let mut expected = vec![0xAAu8; w * bpp];
                        let y_base = row * w;
                        let uv_base = (row / 2) * cw;
                        for col in 0..w {
                            let (r, g, b) = bt601_yuv_to_rgb(
                                frame.y[y_base + col],
                                frame.u[uv_base + (col / 2)],
                                frame.v[uv_base + (col / 2)],
                            );
                            let a = frame.a.as_ref().map_or(255, |ap| ap[y_base + col]);
                            write_pixel(
                                &mut expected[col * bpp..(col + 1) * bpp],
                                surface_type,
                                r,
                                g,
                                b,
                                a,
                            );
                        }
                        assert_eq!(
                            actual, expected,
                            "mismatch width={width} with_alpha={with_alpha} surface_type={surface_type} row={row}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn copy_frame_to_guest_surface_handles_positive_negative_pitch_and_clipped_y() {
        let frame = DecodedVideoFrame {
            width: 3,
            height: 3,
            y: vec![
                235, 16, 81, // row 0
                145, 41, 200, // row 1
                100, 150, 220, // row 2
            ],
            u: vec![128, 90, 200, 110],
            v: vec![128, 240, 60, 170],
            a: None,
        };
        let mut arena = GuestArena::new().unwrap();
        arena.init_thread_environment(DEFAULT_IMAGE_BASE).unwrap();
        let mut heap = GuestHeap::new();
        let pitch = 24usize;
        let surf = heap.alloc(&mut arena, pitch * 5, true).unwrap();

        let mut expected_rows = [Vec::new(), Vec::new(), Vec::new()];
        for (r, slot) in expected_rows.iter_mut().enumerate() {
            *slot = vec![0u8; 3 * 4];
            convert_yuv_row(&frame, r, 3, BINKSURFACE32, slot);
        }

        // Positive pitch with non-zero x/y offset (fast single-span path).
        arena.write_bytes(surf, &vec![0x55u8; pitch * 5]).unwrap();
        assert_eq!(
            copy_frame_to_guest_surface(
                &mut arena,
                &frame,
                GuestSurfaceTarget {
                    dest: surf,
                    pitch: pitch as i32,
                    height: 5,
                    x: 1,
                    y: 1,
                    flags: BINKSURFACE32,
                },
            )
            .unwrap(),
            0
        );
        for (r, exp) in expected_rows.iter().enumerate() {
            let row_addr = surf + ((r + 1) * pitch + 4) as GuestAddr;
            assert_eq!(arena.read_bytes(row_addr, 12).unwrap(), *exp);
        }

        // Negative pitch (bottom-up fallback path).
        arena.write_bytes(surf, &vec![0x55u8; pitch * 5]).unwrap();
        let bottom_row = surf + (3 * pitch) as GuestAddr;
        assert_eq!(
            copy_frame_to_guest_surface(
                &mut arena,
                &frame,
                GuestSurfaceTarget {
                    dest: bottom_row,
                    pitch: -(pitch as i32),
                    height: 4,
                    x: 0,
                    y: 0,
                    flags: BINKSURFACE32,
                },
            )
            .unwrap(),
            0
        );
        for (r, exp) in expected_rows.iter().enumerate() {
            let row_addr = bottom_row - (r * pitch) as GuestAddr;
            assert_eq!(arena.read_bytes(row_addr, 12).unwrap(), *exp);
        }

        // Clipped negative y (top row clipped, bottom guest row left untouched).
        arena.write_bytes(surf, &vec![0xAAu8; pitch * 3]).unwrap();
        assert_eq!(
            copy_frame_to_guest_surface(
                &mut arena,
                &frame,
                GuestSurfaceTarget {
                    dest: surf,
                    pitch: pitch as i32,
                    height: 3,
                    x: 0,
                    y: -1,
                    flags: BINKSURFACE32,
                },
            )
            .unwrap(),
            0
        );
        assert_eq!(arena.read_bytes(surf, 12).unwrap(), expected_rows[1]);
        assert_eq!(
            arena.read_bytes(surf + pitch as GuestAddr, 12).unwrap(),
            expected_rows[2]
        );
        assert_eq!(
            arena.read_bytes(surf + (2 * pitch) as GuestAddr, 12).unwrap(),
            vec![0xAAu8; 12]
        );
    }
}
