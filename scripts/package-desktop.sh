#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DESKTOP_DIR="${REPO_ROOT}/desktop"
BIN_DIR="${BIN_DIR:-${REPO_ROOT}/target/release}"
FFMPEG_LIB_DIR="${FFMPEG_LIB_DIR:-${REPO_ROOT}/dist/x86_64/libs}"
OUT_ZIP="${OUT_ZIP:-${REPO_ROOT}/dist/toluca-linux-x86_64.zip}"
MAX_GLIBC_MINOR="${MAX_GLIBC_MINOR:-}"

REQUIRE_FFMPEG=0
for arg in "$@"; do
    case "${arg}" in
        --require-ffmpeg)
            REQUIRE_FFMPEG=1
            ;;
        -h|--help)
            echo "Usage: $0 [--require-ffmpeg]"
            exit 0
            ;;
        *)
            echo "Unknown option: ${arg}" >&2
            exit 2
            ;;
    esac
done

for required in \
    "${DESKTOP_DIR}/toluca.sh" \
    "${DESKTOP_DIR}/README.md" \
    "${DESKTOP_DIR}/gamedata/README.txt" \
    "${REPO_ROOT}/LICENSE" \
    "${BIN_DIR}/toluca-recomp" \
    "${BIN_DIR}/toluca-runtime"; do
    if [[ ! -f "${required}" ]]; then
        echo "Error: missing required artifact '${required}'." >&2
        exit 1
    fi
done

FORBIDDEN_REGEX='(\.exe|\.bik|\.dat|toluca_code\.bin|toluca-progs\.bin)$'
while IFS= read -r rel_path; do
    if [[ "${rel_path}" =~ ${FORBIDDEN_REGEX} ]]; then
        echo "Error: forbidden game file detected in desktop/: ${rel_path}" >&2
        exit 1
    fi
done < <(find "${DESKTOP_DIR}" -type f -printf '%P\n')

verify_x86_64_elf() {
    local dst="$1"
    local allowed_needed_regex="$2"

    echo "=== Verifying ${dst} ==="
    local file_desc
    file_desc="$(file "${dst}")"
    echo "file: ${file_desc}"
    if ! readelf -h "${dst}" | grep -qE 'Machine:[[:space:]]+Advanced Micro Devices X86-64'; then
        echo "Error: ${dst} is not an x86-64 ELF binary" >&2
        exit 1
    fi

    local needed_libs
    needed_libs="$(readelf -d "${dst}" | awk -F'[][]' '/\(NEEDED\)/ { print $2 }')"
    echo "DT_NEEDED:"
    while IFS= read -r lib; do
        [[ -z "${lib}" ]] && continue
        echo "  ${lib}"
        if [[ ! "${lib}" =~ ${allowed_needed_regex} ]]; then
            echo "Error: forbidden DT_NEEDED dependency '${lib}' in ${dst}" >&2
            exit 1
        fi
    done <<< "${needed_libs}"

    local glibc_versions
    glibc_versions="$(readelf --version-info "${dst}" | grep -oE 'GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?' | sort -u || true)"
    echo "GLIBC requirements: $(echo "${glibc_versions}" | tr '\n' ' ')"
    if [[ -n "${MAX_GLIBC_MINOR}" ]]; then
        while IFS= read -r ver; do
            [[ -z "${ver}" ]] && continue
            local ver_num="${ver#GLIBC_}"
            local major="${ver_num%%.*}"
            local rest="${ver_num#*.}"
            local minor="${rest%%.*}"
            if (( major > 2 || (major == 2 && minor > MAX_GLIBC_MINOR) )); then
                echo "Error: ${dst} requires ${ver}, exceeding baseline GLIBC_2.${MAX_GLIBC_MINOR}" >&2
                exit 1
            fi
        done <<< "${glibc_versions}"
    fi
}

STAGE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/toluca-desktop-pkg.XXXXXX")"
cleanup() {
    rm -rf "${STAGE_DIR}"
}
trap cleanup EXIT

BUNDLE_ROOT="${STAGE_DIR}/toluca"
mkdir -p \
    "${BUNDLE_ROOT}/bin" \
    "${BUNDLE_ROOT}/cache" \
    "${BUNDLE_ROOT}/gamedata" \
    "${BUNDLE_ROOT}/licenses"

cp "${DESKTOP_DIR}/toluca.sh" "${BUNDLE_ROOT}/toluca.sh"
cp "${DESKTOP_DIR}/README.md" "${BUNDLE_ROOT}/README.md"
cp "${DESKTOP_DIR}/gamedata/README.txt" "${BUNDLE_ROOT}/gamedata/README.txt"
cp "${REPO_ROOT}/LICENSE" "${BUNDLE_ROOT}/licenses/LICENSE.toluca.txt"

BIN_ALLOWED_REGEX='^(libc\.so\.6|libm\.so\.6|libdl\.so\.2|libpthread\.so\.0|libgcc_s\.so\.1|ld-linux-x86-64\.so\.2)$'
for bin in toluca-recomp toluca-runtime; do
    cp "${BIN_DIR}/${bin}" "${BUNDLE_ROOT}/bin/${bin}"
    strip "${BUNDLE_ROOT}/bin/${bin}"
    verify_x86_64_elf "${BUNDLE_ROOT}/bin/${bin}" "${BIN_ALLOWED_REGEX}"
done

FFMPEG_SONAMES=(libavutil.so.59 libavcodec.so.61 libavformat.so.61)
has_all_ffmpeg=1
for soname in "${FFMPEG_SONAMES[@]}"; do
    if [[ ! -f "${FFMPEG_LIB_DIR}/${soname}" ]]; then
        has_all_ffmpeg=0
        break
    fi
done

if [[ "${REQUIRE_FFMPEG}" -eq 1 && "${has_all_ffmpeg}" -eq 0 ]]; then
    echo "Error: --require-ffmpeg was specified, but FFmpeg libs are missing from '${FFMPEG_LIB_DIR}'." >&2
    echo "Run ./scripts/build-desktop-ffmpeg.sh first." >&2
    exit 1
fi

if [[ "${has_all_ffmpeg}" -eq 1 ]]; then
    mkdir -p "${BUNDLE_ROOT}/lib"
    LIB_ALLOWED_REGEX='^(libc\.so\.6|libm\.so\.6|libdl\.so\.2|libpthread\.so\.0|libgcc_s\.so\.1|ld-linux-x86-64\.so\.2|libavutil\.so\.59|libavcodec\.so\.61)$'
    for soname in "${FFMPEG_SONAMES[@]}"; do
        dst="${BUNDLE_ROOT}/lib/${soname}"
        cp -L "${FFMPEG_LIB_DIR}/${soname}" "${dst}"
        strip --strip-unneeded "${dst}"
        verify_x86_64_elf "${dst}" "${LIB_ALLOWED_REGEX}"
    done

    ffmpeg_license=""
    for candidate in \
        "${REPO_ROOT}/dist/x86_64/LICENSE.FFmpeg.txt" \
        "$(cd "${FFMPEG_LIB_DIR}/.." 2>/dev/null && pwd)/LICENSE.FFmpeg.txt" \
        "${REPO_ROOT}/portmaster/licenses/LICENSE.FFmpeg.txt"; do
        if [[ -f "${candidate}" ]]; then
            ffmpeg_license="${candidate}"
            break
        fi
    done
    if [[ -z "${ffmpeg_license}" ]]; then
        echo "Error: LICENSE.FFmpeg.txt not found." >&2
        exit 1
    fi
    cp "${ffmpeg_license}" "${BUNDLE_ROOT}/licenses/LICENSE.FFmpeg.txt"
    chmod 0755 "${BUNDLE_ROOT}/lib/"libav*.so.*
fi

chmod 0755 \
    "${BUNDLE_ROOT}/toluca.sh" \
    "${BUNDLE_ROOT}/bin/toluca-recomp" \
    "${BUNDLE_ROOT}/bin/toluca-runtime"

mkdir -p "$(dirname "${OUT_ZIP}")"
rm -f "${OUT_ZIP}"

if command -v zip >/dev/null 2>&1; then
    (
        cd "${STAGE_DIR}"
        zip -r -9 "${OUT_ZIP}" "toluca" >/dev/null
    )
else
    python3 - "${STAGE_DIR}" "${OUT_ZIP}" <<'PY'
import os
import stat
import sys
import zipfile

stage_dir, out_zip = sys.argv[1], sys.argv[2]

with zipfile.ZipFile(out_zip, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
    for root, dirs, files in os.walk(stage_dir):
        dirs.sort()
        files.sort()
        rel_root = os.path.relpath(root, stage_dir)
        if rel_root != "." and not dirs and not files:
            info = zipfile.ZipInfo(rel_root + "/")
            info.external_attr = (stat.S_IFDIR | 0o755) << 16
            zf.writestr(info, "")
        for name in files:
            full_path = os.path.join(root, name)
            rel_path = os.path.relpath(full_path, stage_dir)
            st = os.stat(full_path)
            info = zipfile.ZipInfo(rel_path)
            info.external_attr = (stat.S_IFREG | stat.S_IMODE(st.st_mode)) << 16
            with open(full_path, "rb") as fh:
                zf.writestr(info, fh.read())
PY
fi

python3 - "${OUT_ZIP}" <<'PY'
import re
import sys
import zipfile

out_zip = sys.argv[1]
forbidden = re.compile(r"(\.exe|\.bik|\.dat|toluca_code\.bin|toluca-progs\.bin)$", re.IGNORECASE)

with zipfile.ZipFile(out_zip, "r") as zf:
    names = sorted(zf.namelist())
    for name in names:
        if forbidden.search(name):
            raise SystemExit(f"Error: forbidden entry inside bundle: {name}")
    print(f"Bundle verified: {out_zip} ({len(names)} entries)")
    for info in sorted(zf.infolist(), key=lambda i: i.filename):
        mode = (info.external_attr >> 16) & 0o777
        print(f"  {mode:04o}  {info.file_size:10d}  {info.filename}")
PY
