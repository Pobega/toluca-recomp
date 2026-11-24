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
use crate::win32::dsound::DsoundMixer;

const MAX_PATH_LEN: usize = 512;
const BINK_STRUCT_ALLOC_BYTES: usize = 256;
const BINK_HEADER_MIN_BYTES: usize = 44;
const BINK_AUDIO_STEREO_FLAG: u16 = 0x2000;
const BINK_FLAG_NOSOUND: u32 = 0x0008_0000;

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

#[inline]
pub fn bt601_yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = i32::from(y) - 16;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
    let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
    let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
    (r, g, b)
}

fn surface_bpp(surface_type: u32) -> Option<usize> {
    match surface_type {
        BINKSURFACE32 | BINKSURFACE32R | BINKSURFACE32A | BINKSURFACE32RA => Some(4),
        BINKSURFACE24 | BINKSURFACE24R => Some(3),
        BINKSURFACE555 | BINKSURFACE565 | BINKSURFACE555R | BINKSURFACE565R => Some(2),
        _ => None,
    }
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
            let p = (u16::from(r >> 3) << 11) | (u16::from(g >> 2) << 5) | u16::from(b >> 3);
            dst[0..2].copy_from_slice(&p.to_le_bytes());
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
    let w = frame.width as usize;
    let cw = w.div_ceil(2);
    let y_base = row * w;
    let uv_base = (row / 2) * cw;

    for col in 0..copy_width {
        let y = frame.y[y_base + col];
        let u = frame.u[uv_base + (col / 2)];
        let v = frame.v[uv_base + (col / 2)];
        let a = frame
            .a
            .as_ref()
            .map_or(255, |alpha| alpha[y_base + col]);
        let (r, g, b) = bt601_yuv_to_rgb(y, u, v);
        let off = col * bpp;
        write_pixel(&mut out_row[off..off + bpp], surface_type, r, g, b, a);
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

    let mut row_buf = vec![0u8; max_cols * bpp];
    for row in 0..max_rows {
        let dst_y_coord = i64::from(target.y) + row as i64;
        if dst_y_coord < 0 || dst_y_coord >= i64::from(target.height) {
            continue;
        }
        let dst_x_coord = i64::from(target.x).max(0);
        let row_addr = i64::from(target.dest)
            + dst_y_coord * i64::from(target.pitch)
            + dst_x_coord * (bpp as i64);
        if row_addr <= 0 || row_addr > i64::from(u32::MAX) {
            continue;
        }
        convert_yuv_row(frame, row, max_cols, surface_type, &mut row_buf);
        arena.write_bytes(row_addr as GuestAddr, &row_buf)?;
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
    decoder: Option<FfmpegDecoder>,
    current_frame: Option<DecodedVideoFrame>,
}

pub struct BinkState {
    ffmpeg: Option<FfmpegApi>,
    ffmpeg_load_attempted: bool,
    logged_missing_ffmpeg: bool,
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
    open_flags: u32,
) -> (usize, Option<BinkAudioTrack>) {
    if (open_flags & BINK_FLAG_NOSOUND) != 0 || header.audio_tracks.is_empty() {
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

    let (audio_ordinal, selected_audio_track) =
        select_audio_track(&header, &ctx.win32.bink.requested_sound_tracks, flags);
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
    ctx.win32.bink.instances.insert(
        bink_va,
        BinkInstance {
            header,
            open_flags: flags,
            frame_num: 1,
            last_frame_num: 0,
            paused: false,
            sound_on: selected_audio_track.is_some(),
            selected_audio_track,
            next_frame_due: Instant::now() + frame_period,
            decoder,
            current_frame: None,
        },
    );

    return_stdcall(cpu, ctx.arena, 2, bink_va)?;
    Ok(())
}

fn bink_close(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let bink_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    if let Some(inst) = ctx.win32.bink.instances.remove(&bink_va) {
        if let (Some(dec), Some(api)) = (inst.decoder, ctx.win32.bink.ffmpeg.as_ref()) {
            dec.close(api);
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
    let ret = if let Some(inst) = ctx.win32.bink.instances.get_mut(&bink_va) {
        inst.sound_on = on_off != 0;
        i32::from(inst.sound_on) as u32
    } else {
        0
    };
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

fn bink_wait(ctx: &mut RuntimeContext<'_>, cpu: &mut GuestCpuState) -> Result<(), RuntimeError> {
    let bink_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    if ctx.win32.config.headless || ctx.win32.config.max_frames.is_some() {
        return_stdcall(cpu, ctx.arena, 1, 0)?;
        return Ok(());
    }
    let wait = match ctx.win32.bink.instances.get(&bink_va) {
        Some(inst) if inst.decoder.is_some() => {
            u32::from(inst.paused || Instant::now() < inst.next_frame_due)
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

    let (decoded_vid, decoded_aud) = if let (Some(dec), Some(api)) =
        (inst.decoder.as_mut(), ctx.win32.bink.ffmpeg.as_ref())
    {
        dec.decode_next_video_frame(api, inst.selected_audio_track, inst.sound_on)
    } else {
        (None, Vec::new())
    };

    if let Some(vid) = decoded_vid {
        inst.current_frame = Some(vid);
    } else if inst.current_frame.is_none() {
        inst.current_frame = Some(DecodedVideoFrame::black(
            inst.header.width,
            inst.header.height,
        ));
    }

    if !decoded_aud.is_empty() && inst.sound_on {
        let rate = inst
            .selected_audio_track
            .map_or(DEFAULT_AUDIO_RATE, |t| u32::from(t.sample_rate.max(1)));
        let ch = inst
            .selected_audio_track
            .map_or(DEFAULT_AUDIO_CHANNELS, |t| t.channels);
        if let Ok(mut mixer) = ctx.win32.dsound.mixer.lock() {
            push_aux_to_mixer(&mut mixer, &decoded_aud, rate, ch);
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

    return_stdcall(cpu, ctx.arena, 1, 0)?;
    Ok(())
}

fn push_aux_to_mixer(mixer: &mut DsoundMixer, samples: &[i16], rate: u32, channels: u16) {
    mixer.push_aux_pcm(samples, rate, channels);
}

fn bink_next_frame(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let bink_va = peek_arg_u32(cpu, ctx.arena, 0)?;
    if let Some(inst) = ctx.win32.bink.instances.get_mut(&bink_va) {
        if inst.frame_num >= inst.header.num_frames {
            inst.frame_num = 1;
            if let (Some(dec), Some(api)) = (inst.decoder.as_mut(), ctx.win32.bink.ffmpeg.as_ref()) {
                dec.rewind(api);
            }
        } else {
            inst.frame_num += 1;
        }
        inst.next_frame_due = Instant::now() + inst.header.frame_duration();
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
}
