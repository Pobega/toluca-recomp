#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
PM_DIR="${REPO_ROOT}/portmaster"
BIN_DIR="${REPO_ROOT}/dist/aarch64"
OUT_ZIP="${OUT_ZIP:-${REPO_ROOT}/dist/toluca-portmaster.zip}"

for required in \
    "${PM_DIR}/Toluca.sh" \
    "${PM_DIR}/toluca.gptk" \
    "${PM_DIR}/port.json" \
    "${PM_DIR}/README.md" \
    "${PM_DIR}/gamedata/README.txt" \
    "${PM_DIR}/tools/patchscript" \
    "${PM_DIR}/licenses/LICENSE.FFmpeg.txt" \
    "${PM_DIR}/licenses/LICENSE.toluca.txt" \
    "${BIN_DIR}/toluca-recomp" \
    "${BIN_DIR}/toluca-runtime" \
    "${BIN_DIR}/libs/libavutil.so.59" \
    "${BIN_DIR}/libs/libavcodec.so.61" \
    "${BIN_DIR}/libs/libavformat.so.61"; do
    if [[ ! -f "${required}" ]]; then
        echo "Error: missing required artifact '${required}'." >&2
        echo "Run ./scripts/build-aarch64.sh first if dist/aarch64/ artifacts are missing." >&2
        exit 1
    fi
done

FORBIDDEN_REGEX='(\.exe|\.bik|\.dat|toluca_code\.bin|toluca-progs\.bin)$'

while IFS= read -r rel_path; do
    if [[ "${rel_path}" =~ ${FORBIDDEN_REGEX} ]]; then
        echo "Error: forbidden game file detected in portmaster/: ${rel_path}" >&2
        exit 1
    fi
done < <(find "${PM_DIR}" -type f -printf '%P\n')

STAGE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/toluca-portmaster-pkg.XXXXXX")"
cleanup() {
    rm -rf "${STAGE_DIR}"
}
trap cleanup EXIT

PORT_ROOT="${STAGE_DIR}/toluca"
mkdir -p \
    "${PORT_ROOT}/bin" \
    "${PORT_ROOT}/cache" \
    "${PORT_ROOT}/gamedata" \
    "${PORT_ROOT}/libs.aarch64" \
    "${PORT_ROOT}/licenses" \
    "${PORT_ROOT}/tools"

cp "${PM_DIR}/Toluca.sh" "${STAGE_DIR}/Toluca.sh"
cp "${PM_DIR}/toluca.gptk" "${PORT_ROOT}/toluca.gptk"
cp "${PM_DIR}/port.json" "${PORT_ROOT}/port.json"
cp "${PM_DIR}/README.md" "${PORT_ROOT}/README.md"
cp "${PM_DIR}/gamedata/README.txt" "${PORT_ROOT}/gamedata/README.txt"
cp "${PM_DIR}/tools/patchscript" "${PORT_ROOT}/tools/patchscript"
cp "${PM_DIR}/licenses/"* "${PORT_ROOT}/licenses/"
cp "${BIN_DIR}/toluca-recomp" "${PORT_ROOT}/bin/toluca-recomp"
cp "${BIN_DIR}/toluca-runtime" "${PORT_ROOT}/bin/toluca-runtime"
cp "${BIN_DIR}/libs/"libav*.so.* "${PORT_ROOT}/libs.aarch64/"

chmod 0755 \
    "${STAGE_DIR}/Toluca.sh" \
    "${PORT_ROOT}/tools/patchscript" \
    "${PORT_ROOT}/bin/toluca-recomp" \
    "${PORT_ROOT}/bin/toluca-runtime" \
    "${PORT_ROOT}/libs.aarch64/"libav*.so.*

mkdir -p "$(dirname "${OUT_ZIP}")"
rm -f "${OUT_ZIP}"

if command -v zip >/dev/null 2>&1; then
    (
        cd "${STAGE_DIR}"
        zip -r -9 "${OUT_ZIP}" "Toluca.sh" "toluca" >/dev/null
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
