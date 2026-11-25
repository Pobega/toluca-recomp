# PortMaster Packaging

This directory holds the clean-room PortMaster release assets for *Silent Hill 2*:

- `Toluca.sh` — Handheld launch script (triggers `tools/patchscript` on first run or when `sh2pc.exe` changes, then launches `bin/toluca-runtime`).
- `toluca.gptk` — Optional `gptokeyb` controller-to-keyboard fallback mapping (`TOLUCA_NATIVE_GAMEPAD="0"`).
- `tools/patchscript` — On-device patching script that verifies `sh2pc.exe` and compiles `cache/toluca_code.bin` under a 100 MiB peak RSS budget.
- `port.json` — PortMaster v3 metadata manifest.
- `gamedata/README.txt` — User instructions for placing vanilla NA v1.0 `sh2pc.exe` and `data/`.

## Controls

By default, `Toluca.sh` runs `toluca-runtime` in native SDL2 DirectInput gamepad mode (`--nintendo-buttons --swap-shoulders`) with `gptokeyb` active for exit hotkeys, restoring the PS2-style layout on both stickless clamshells and dual-stick handhelds (Nintendo face-button labels):

- **D-Pad / Left Stick:** Move / Navigate menus
- **B (South):** Action / Confirm / Attack *(PS2 Cross)*
- **A (East):** Cancel in menus + **Flashlight** toggle in gameplay *(PS2 Circle, without popping up the PC Esc menu or locking Cancel in Hotel Room 312)*
- **Y (West):** Run *(PS2 Square)*
- **X (North):** Map *(PS2 Triangle)*
- **L1 / R1:** Search View (center/pan camera), Ready Weapon (aim) — *hold `R1 + X` to Reload*
- **L2 / R2:** Step Left / Right *(press `L2 + R2` together for 180° Quick Turn)*
- **Select / Start:** Inventory (Item Menu), Pause Menu
- **L3 / R3** *(dual-stick devices)*: Flashlight, Health Drink
- **Home + Start / Select + Start:** Exit to PortMaster / NextUI

## Supported Handheld Displays

`Toluca.sh` defaults to `--fullscreen` with `4:3` aspect fitting (using `DISPLAY_WIDTH` / `DISPLAY_HEIGHT`, defaulting to `640x480`):

- **640x480 (4:3):** Fills the display natively.
- **1280x720 (16:9):** Centered 4:3 pillarbox (`960x720`) with black sidebars.
- **720x720 (1:1):** Centered 4:3 letterbox (`720x540`) with black top/bottom bars.

## Optional User Configuration (`toluca.conf`)

Place an optional `toluca.conf` file in `ports/toluca/` to customize settings:

```bash
TOLUCA_ASPECT="4:3"         # 4:3 | 16:9 | stretch
TOLUCA_WIDTH="1280"         # override display width
TOLUCA_HEIGHT="720"         # override display height
TOLUCA_NATIVE_GAMEPAD="1"   # 1 = native SDL2 DirectInput gamepad (default), 0 = toluca.gptk keyboard fallback
TOLUCA_NINTENDO_BUTTONS="1" # 1 = Nintendo-labeled SDL2 mapping (A=East/B=South, default), 0 = Xbox/positional
TOLUCA_SWAP_SHOULDERS="1"   # 1 = L1/R1 Search+Aim, L2/R2 Strafe (default), 0 = PS2 L1/R1 Strafe, L2/R2 Search+Aim
TOLUCA_EXTRA_ARGS=""        # extra toluca-runtime CLI flags
```

## Clean-Room Notice

No copyrighted Konami executables, DLLs, videos, or game assets are included in this directory or in generated PortMaster release bundles.
