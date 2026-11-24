# PortMaster Packaging

This directory holds the clean-room PortMaster release assets for *Silent Hill 2*:

- `Toluca.sh` — Handheld launch script (triggers `tools/patchscript` on first run or when `sh2pc.exe` changes, then launches `bin/toluca-runtime`).
- `tools/patchscript` — On-device patching script that verifies `sh2pc.exe` and compiles `cache/toluca_code.bin` under a 100 MiB peak RSS budget.
- `port.json` — PortMaster v3 metadata manifest.
- `gamedata/README.txt` — User instructions for placing vanilla NA v1.0 `sh2pc.exe` and `data/`.

## Supported Handheld Displays

`Toluca.sh` defaults to `--fullscreen` with `4:3` aspect fitting (using `DISPLAY_WIDTH` / `DISPLAY_HEIGHT`, defaulting to `640x480`):

- **640x480 (4:3):** Fills the display natively.
- **1280x720 (16:9):** Centered 4:3 pillarbox (`960x720`) with black sidebars.
- **720x720 (1:1):** Centered 4:3 letterbox (`720x540`) with black top/bottom bars.

## Optional User Configuration (`toluca.conf`)

Place an optional `toluca.conf` file in `ports/toluca/` to customize settings:

```bash
TOLUCA_ASPECT="4:3"   # 4:3 | 16:9 | stretch
TOLUCA_WIDTH="1280"   # override display width
TOLUCA_HEIGHT="720"   # override display height
TOLUCA_EXTRA_ARGS=""  # extra toluca-runtime CLI flags
```

## Clean-Room Notice

No copyrighted Konami executables, DLLs, videos, or game assets are included in this directory or in generated PortMaster release bundles.
