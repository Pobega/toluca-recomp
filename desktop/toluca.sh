#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RECOMP_BIN="${SCRIPT_DIR}/bin/toluca-recomp"
RUNTIME_BIN="${SCRIPT_DIR}/bin/toluca-runtime"
CACHE_DIR="${SCRIPT_DIR}/cache"
CACHE_BIN="${CACHE_DIR}/toluca_code.bin"
SHA_STAMP="${CACHE_DIR}/sh2pc.sha1"

if [[ ! -x "${RECOMP_BIN}" ]]; then
    echo "Error: toluca-recomp binary not found or not executable at '${RECOMP_BIN}'." >&2
    exit 1
fi
if [[ ! -x "${RUNTIME_BIN}" ]]; then
    echo "Error: toluca-runtime binary not found or not executable at '${RUNTIME_BIN}'." >&2
    exit 1
fi

EXE_PATH=""
if [[ $# -gt 0 && "$1" != -* ]]; then
    arg_path="$1"
    shift
    if [[ -d "${arg_path}" ]]; then
        for candidate in \
            "${arg_path}/sh2pc.exe" \
            "${arg_path}/Silent Hill 2/sh2pc.exe"; do
            if [[ -f "${candidate}" ]]; then
                EXE_PATH="${candidate}"
                break
            fi
        done
        if [[ -z "${EXE_PATH}" ]]; then
            echo "Error: sh2pc.exe not found in directory '${arg_path}'." >&2
            exit 1
        fi
    elif [[ -f "${arg_path}" ]]; then
        EXE_PATH="${arg_path}"
    else
        echo "Error: '${arg_path}' is not a file or directory." >&2
        exit 1
    fi
else
    for candidate in \
        "${SCRIPT_DIR}/gamedata/sh2pc.exe" \
        "${SCRIPT_DIR}/gamedata/Silent Hill 2/sh2pc.exe"; do
        if [[ -f "${candidate}" ]]; then
            EXE_PATH="${candidate}"
            break
        fi
    done
    if [[ -z "${EXE_PATH}" ]]; then
        echo "Error: sh2pc.exe not found in '${SCRIPT_DIR}/gamedata/'." >&2
        echo "Copy vanilla North American v1.0 sh2pc.exe and data/ into '${SCRIPT_DIR}/gamedata/'," >&2
        echo "or pass the path as the first argument: $0 /path/to/sh2pc.exe [runtime options...]" >&2
        exit 1
    fi
fi

GAME_DATA_DIR="$(cd "$(dirname "${EXE_PATH}")" && pwd)"
EXE_PATH="${GAME_DATA_DIR}/$(basename "${EXE_PATH}")"

if [[ ! -d "${GAME_DATA_DIR}/data" && ! -d "${GAME_DATA_DIR}/DATA" && ! -d "${GAME_DATA_DIR}/Data" ]]; then
    echo "Error: 'data/' directory not found next to '${EXE_PATH}'." >&2
    exit 1
fi

if command -v sha1sum >/dev/null 2>&1; then
    EXE_SHA1="$(sha1sum "${EXE_PATH}" | awk '{ print $1 }')"
    RECOMP_SHA1="$(sha1sum "${RECOMP_BIN}" | awk '{ print $1 }')"
else
    EXE_SHA1="$("${RECOMP_BIN}" verify "${EXE_PATH}" | awk '/^SHA-1:/ { print $2; exit }')"
    RECOMP_SHA1="$(cksum "${RECOMP_BIN}" 2>/dev/null | awk '{ print $1 "-" $2 }' || echo "unknown")"
fi
CURRENT_SHA1="${EXE_SHA1}:${RECOMP_SHA1}"

SAVED_SHA1=""
if [[ -f "${SHA_STAMP}" ]]; then
    SAVED_SHA1="$(cat "${SHA_STAMP}" 2>/dev/null || true)"
fi

if [[ ! -f "${CACHE_BIN}" || -z "${EXE_SHA1}" || "${CURRENT_SHA1}" != "${SAVED_SHA1}" ]]; then
    echo "Verifying PE32 executable: ${EXE_PATH}"
    "${RECOMP_BIN}" verify "${EXE_PATH}"

    mkdir -p "${CACHE_DIR}"
    TMP_CACHE="$(mktemp "${CACHE_DIR}/.toluca_code.bin.XXXXXX")"
    trap 'rm -f "${TMP_CACHE:-}"' EXIT

    echo "Compiling ${EXE_PATH} -> ${CACHE_BIN}..."
    "${RECOMP_BIN}" compile "${EXE_PATH}" -o "${TMP_CACHE}"
    mv -f "${TMP_CACHE}" "${CACHE_BIN}"
    printf '%s\n' "${CURRENT_SHA1}" > "${SHA_STAMP}"
fi

if [[ -d "${SCRIPT_DIR}/lib" ]]; then
    export LD_LIBRARY_PATH="${SCRIPT_DIR}/lib${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
fi

if [[ "${TOLUCA_KEEP_SAFE_MODE:-0}" != "1" ]]; then
    for cfg in "${GAME_DATA_DIR}"/*; do
        [[ -f "${cfg}" && -w "${cfg}" ]] || continue
        case "${cfg##*/}" in
            [Ss][Ee][Tt][Tt][Ii][Nn][Gg][Ss].[Ii][Nn][Ii])
                if grep -q '^SET DX_CONFIG_SAFE_MODE 1' "${cfg}" 2>/dev/null; then
                    sed -i 's/^SET DX_CONFIG_SAFE_MODE 1\([[:space:]]*\)$/SET DX_CONFIG_SAFE_MODE 0\1/' "${cfg}"
                fi
                ;;
        esac
    done
fi

exec "${RUNTIME_BIN}" \
    --game-dir "${GAME_DATA_DIR}" \
    --shader-cache "${SCRIPT_DIR}/cache/toluca-progs.bin" \
    "$@" \
    "${EXE_PATH}" \
    "${CACHE_BIN}"
