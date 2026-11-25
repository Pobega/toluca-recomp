#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
OUT_DIR="${REPO_ROOT}/dist/x86_64"
LIB_OUT_DIR="${OUT_DIR}/libs"
LICENSE_OUT="${OUT_DIR}/LICENSE.FFmpeg.txt"

FFMPEG_VERSION="7.1.1"
FFMPEG_SHA256="733984395e0dbbe5c046abda2dc49a5544e7e0e1e2366bba849222ae9e3a03b1"
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"
REQUIRED_SONAMES=(libavutil.so.59 libavcodec.so.61 libavformat.so.61)

FORCE=0
for arg in "$@"; do
    case "${arg}" in
        --force)
            FORCE=1
            ;;
        -h|--help)
            echo "Usage: $0 [--force]"
            exit 0
            ;;
        *)
            echo "Unknown option: ${arg}" >&2
            exit 2
            ;;
    esac
done

all_present=1
if [[ ! -f "${LICENSE_OUT}" ]]; then
    all_present=0
else
    for soname in "${REQUIRED_SONAMES[@]}"; do
        if [[ ! -f "${LIB_OUT_DIR}/${soname}" ]]; then
            all_present=0
            break
        fi
    done
fi

if [[ "${FORCE}" -eq 0 && "${all_present}" -eq 1 ]]; then
    echo "Desktop FFmpeg libraries already present in ${LIB_OUT_DIR} (pass --force to rebuild)."
    exit 0
fi

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/toluca-desktop-ffmpeg.XXXXXX")"
cleanup() {
    rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

TARBALL="${WORK_DIR}/ffmpeg-${FFMPEG_VERSION}.tar.xz"
PREFIX="${WORK_DIR}/install"

echo "Downloading FFmpeg ${FFMPEG_VERSION}..."
if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fsSL -o "${TARBALL}" "${FFMPEG_URL}"
elif command -v wget >/dev/null 2>&1; then
    wget -q -O "${TARBALL}" "${FFMPEG_URL}"
else
    echo "Error: neither curl nor wget is installed" >&2
    exit 1
fi

echo "${FFMPEG_SHA256}  ${TARBALL}" | sha256sum -c -
tar -xf "${TARBALL}" -C "${WORK_DIR}"

SRC_DIR="${WORK_DIR}/ffmpeg-${FFMPEG_VERSION}"
(
    cd "${SRC_DIR}"
    ./configure \
        --prefix="${PREFIX}" \
        --enable-shared \
        --disable-static \
        --disable-programs \
        --disable-doc \
        --disable-everything \
        --disable-avdevice \
        --disable-swscale \
        --disable-swresample \
        --disable-avfilter \
        --disable-network \
        --disable-autodetect \
        --disable-x86asm \
        --enable-demuxer=bink \
        --enable-decoder=bink,binkaudio_dct,binkaudio_rdft \
        --enable-protocol=file \
        --disable-debug \
        --enable-small
    make -j"$(nproc 2>/dev/null || echo 4)"
    make install
)

cat <<'EOF' > "${WORK_DIR}/check_offsets.c"
#include <stddef.h>
#include <stdio.h>
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavcodec/packet.h>
#include <libavutil/frame.h>

#define CHECK(label, expr, expected) do { \
    size_t actual = (expr); \
    printf("%s=%zu (expected %zu)\n", (label), actual, (size_t)(expected)); \
    if (actual != (size_t)(expected)) { \
        fprintf(stderr, "MISMATCH: %s=%zu != %zu\n", (label), actual, (size_t)(expected)); \
        fail = 1; \
    } \
} while (0)

int main(void) {
    int fail = 0;
    CHECK("AVFMT_NB_STREAMS_OFF", offsetof(AVFormatContext, nb_streams), 44);
    CHECK("AVFMT_STREAMS_OFF", offsetof(AVFormatContext, streams), 48);
    CHECK("AVSTREAM_CODECPAR_OFF", offsetof(AVStream, codecpar), 16);
    CHECK("AVCODECPAR_TYPE_OFF", offsetof(AVCodecParameters, codec_type), 0);
    CHECK("AVCODECPAR_ID_OFF", offsetof(AVCodecParameters, codec_id), 4);
    CHECK("AVPKT_STREAM_IDX_OFF", offsetof(AVPacket, stream_index), 36);
    CHECK("AVFRAME_DATA_OFF", offsetof(AVFrame, data), 0);
    CHECK("AVFRAME_LINESIZE_OFF", offsetof(AVFrame, linesize), 64);
    CHECK("AVFRAME_WIDTH_OFF", offsetof(AVFrame, width), 104);
    CHECK("AVFRAME_HEIGHT_OFF", offsetof(AVFrame, height), 108);
    CHECK("AVFRAME_NB_SAMPLES_OFF", offsetof(AVFrame, nb_samples), 112);
    CHECK("AVFRAME_FORMAT_OFF", offsetof(AVFrame, format), 116);
    return fail;
}
EOF

CC_BIN="${CC:-gcc}"
"${CC_BIN}" -I"${PREFIX}/include" "${WORK_DIR}/check_offsets.c" -o "${WORK_DIR}/check_offsets"
"${WORK_DIR}/check_offsets"

mkdir -p "${LIB_OUT_DIR}"
for soname in "${REQUIRED_SONAMES[@]}"; do
    src="${PREFIX}/lib/${soname}"
    dst="${LIB_OUT_DIR}/${soname}"
    cp -L "${src}" "${dst}"
    strip --strip-unneeded "${dst}"
done
cp "${SRC_DIR}/COPYING.LGPLv2.1" "${LICENSE_OUT}"

echo "Done. Stripped desktop FFmpeg libraries are in ${LIB_OUT_DIR}"
ls -lh "${LIB_OUT_DIR}"/libav*.so.* "${LICENSE_OUT}"
