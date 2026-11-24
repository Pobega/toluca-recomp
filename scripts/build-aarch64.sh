#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
IMAGE_NAME="toluca-portmaster-aarch64-builder"
TARGET_TRIPLE="aarch64-unknown-linux-gnu"
MAX_GLIBC_MINOR=31

RUN_TESTS=0
for arg in "$@"; do
    case "${arg}" in
        --test)
            RUN_TESTS=1
            ;;
        -h|--help)
            echo "Usage: $0 [--test]"
            exit 0
            ;;
        *)
            echo "Unknown option: ${arg}" >&2
            exit 2
            ;;
    esac
done

if command -v podman >/dev/null 2>&1; then
    ENGINE="podman"
    ENGINE_RUN_ARGS=(--rm --userns=keep-id -v "${REPO_ROOT}:/work:Z")
elif command -v docker >/dev/null 2>&1; then
    ENGINE="docker"
    ENGINE_RUN_ARGS=(--rm -u "$(id -u):$(id -g)" -v "${REPO_ROOT}:/work")
else
    echo "Error: neither podman nor docker is installed" >&2
    exit 1
fi

echo "Building container image ${IMAGE_NAME} with ${ENGINE}..."
"${ENGINE}" build -f "${REPO_ROOT}/Containerfile.aarch64" -t "${IMAGE_NAME}" "${REPO_ROOT}"

mkdir -p "${REPO_ROOT}/dist/aarch64/libs" "${REPO_ROOT}/target/aarch64-container" "${REPO_ROOT}/portmaster/licenses"

CONTAINER_SCRIPT="$(cat <<'EOF'
set -euo pipefail

TARGET_TRIPLE="aarch64-unknown-linux-gnu"
export CARGO_TARGET_DIR="/work/target/aarch64-container"
export CARGO_HOME="/work/target/aarch64-container/.cargo"
mkdir -p "${CARGO_HOME}" /work/dist/aarch64/libs /work/portmaster/licenses

verify_aarch64_elf() {
    local dst="$1"
    local allowed_needed_regex="$2"

    echo "=== Verifying ${dst} ==="
    local file_desc
    file_desc="$(file "${dst}")"
    echo "file: ${file_desc}"
    if ! readelf -h "${dst}" | grep -qE 'Machine:[[:space:]]+AArch64'; then
        echo "Error: ${dst} is not an AArch64 ELF binary" >&2
        exit 1
    fi

    local needed_libs
    needed_libs="$(readelf -d "${dst}" | awk -F'[][]' '/\(NEEDED\)/ { print $2 }')"
    echo "DT_NEEDED:"
    echo "${needed_libs}" | sed 's/^/  /'
    while IFS= read -r lib; do
        [[ -z "${lib}" ]] && continue
        if [[ ! "${lib}" =~ ${allowed_needed_regex} ]]; then
            echo "Error: forbidden DT_NEEDED dependency '${lib}' in ${dst}" >&2
            exit 1
        fi
    done <<< "${needed_libs}"

    local glibc_versions
    glibc_versions="$(readelf --version-info "${dst}" | grep -oE 'GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?' | sort -u || true)"
    echo "GLIBC requirements: $(echo "${glibc_versions}" | tr '\n' ' ')"
    while IFS= read -r ver; do
        [[ -z "${ver}" ]] && continue
        local ver_num="${ver#GLIBC_}"
        local major="${ver_num%%.*}"
        local rest="${ver_num#*.}"
        local minor="${rest%%.*}"
        if (( major > 2 || (major == 2 && minor > MAX_GLIBC_MINOR_PLACEHOLDER) )); then
            echo "Error: ${dst} requires ${ver}, exceeding baseline GLIBC_2.MAX_GLIBC_MINOR_PLACEHOLDER" >&2
            exit 1
        fi
    done <<< "${glibc_versions}"
}

cargo build --release --target "${TARGET_TRIPLE}" -p toluca-recomp -p toluca-runtime

BIN_ALLOWED_REGEX='^(libc\.so\.6|libm\.so\.6|libdl\.so\.2|libpthread\.so\.0|libgcc_s\.so\.1|ld-linux-aarch64\.so\.1)$'
for bin in toluca-recomp toluca-runtime; do
    src="${CARGO_TARGET_DIR}/${TARGET_TRIPLE}/release/${bin}"
    dst="/work/dist/aarch64/${bin}"
    cp "${src}" "${dst}"
    aarch64-linux-gnu-strip "${dst}"
    verify_aarch64_elf "${dst}" "${BIN_ALLOWED_REGEX}"
done

LIB_ALLOWED_REGEX='^(libc\.so\.6|libm\.so\.6|libdl\.so\.2|libpthread\.so\.0|libgcc_s\.so\.1|ld-linux-aarch64\.so\.1|libavutil\.so\.59|libavcodec\.so\.61)$'
for soname in libavutil.so.59 libavcodec.so.61 libavformat.so.61; do
    src="/opt/toluca-ffmpeg-aarch64/lib/${soname}"
    dst="/work/dist/aarch64/libs/${soname}"
    cp -L "${src}" "${dst}"
    aarch64-linux-gnu-strip --strip-unneeded "${dst}"
    verify_aarch64_elf "${dst}" "${LIB_ALLOWED_REGEX}"
done

cp /opt/toluca-ffmpeg-aarch64/LICENSE.FFmpeg.txt /work/portmaster/licenses/LICENSE.FFmpeg.txt
if [[ ! -f /work/portmaster/licenses/LICENSE.toluca.txt && -f /work/LICENSE ]]; then
    cp /work/LICENSE /work/portmaster/licenses/LICENSE.toluca.txt
fi
EOF
)"

CONTAINER_SCRIPT="${CONTAINER_SCRIPT//MAX_GLIBC_MINOR_PLACEHOLDER/${MAX_GLIBC_MINOR}}"

"${ENGINE}" run "${ENGINE_RUN_ARGS[@]}" "${IMAGE_NAME}" bash -c "${CONTAINER_SCRIPT}"

if [[ "${RUN_TESTS}" -eq 1 ]]; then
    echo "Running workspace tests under qemu-aarch64-static..."
    "${ENGINE}" run "${ENGINE_RUN_ARGS[@]}" \
        -e CARGO_TARGET_DIR=/work/target/aarch64-container \
        -e CARGO_HOME=/work/target/aarch64-container/.cargo \
        -e "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER=qemu-aarch64-static -L /usr/aarch64-linux-gnu -E LD_LIBRARY_PATH=/work/dist/aarch64/libs" \
        "${IMAGE_NAME}" \
        bash -c "cargo test --workspace --target '${TARGET_TRIPLE}' --quiet && cargo test -p toluca-runtime --target '${TARGET_TRIPLE}' --lib binkw32"
fi

echo "Done. Stripped AArch64 binaries and FFmpeg libs are in dist/aarch64/"
ls -lh "${REPO_ROOT}/dist/aarch64/toluca-recomp" "${REPO_ROOT}/dist/aarch64/toluca-runtime" "${REPO_ROOT}/dist/aarch64/libs"/libav*.so.*
