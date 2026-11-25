# Toluca — Linux x86-64 Desktop

Clean-room static recompiler and native OpenGL ES 3.0 runtime for the vanilla North American v1.0 PC version of *Silent Hill 2*.

## Requirements

- Linux x86-64 with OpenGL ES 3.0 / EGL drivers (`libEGL.so.1`, `libGLESv2.so.2`) and SDL2 (`libSDL2-2.0.so.0`)
- Minimal Bink-only LGPL FFmpeg 7.1.1 shared libraries (`libavformat`, `libavcodec`, `libavutil`) are bundled in `lib/` for FMV playback (`licenses/LICENSE.FFmpeg.txt`).

## Quick Start

1. Copy your legal, unprotected vanilla North American v1.0 `sh2pc.exe` and `data/` folder into `gamedata/` (or `gamedata/Silent Hill 2/`), or keep them anywhere on disk.
2. Launch from a terminal:

```bash
# Auto-discovers gamedata/sh2pc.exe (or gamedata/Silent Hill 2/sh2pc.exe)
./toluca.sh

# Or point directly at an existing installation and pass runtime flags
./toluca.sh "/path/to/Silent Hill 2/sh2pc.exe" --fullscreen --aspect 4:3
```

On first launch, `toluca.sh` verifies `sh2pc.exe`, compiles `cache/toluca_code.bin`, and starts `bin/toluca-runtime`. Subsequent launches skip recompilation unless `sh2pc.exe` or `bin/toluca-recomp` changes.

## Controls

### Keyboard

- **Arrow Keys:** Move / Navigate menus
- **Space:** Action / Confirm / Attack
- **Escape:** Cancel / Pause menu
- **Left Shift:** Run
- **M / I:** Map / Inventory
- **A / D:** Search View / Ready Weapon
- **S / F:** Step Left / Right
- **L / H:** Flashlight / Health Drink

### Native PS2-Style Gamepad (SDL2)

- **D-Pad / Left Stick:** Move / Navigate menus
- **South (Cross):** Action / Confirm / Attack
- **East (Circle):** Cancel in menus + Flashlight toggle in gameplay
- **West (Square):** Run
- **North (Triangle):** Map
- **L1 / R1:** Step Left / Right *(`L1 + R1` for 180° Quick Turn)*
- **L2 / R2:** Search View (center/pan camera), Ready Weapon (aim) — *hold `R2 + North` to Reload*
- **Select / Start:** Inventory / Pause *(pass `--swap-shoulders` to move Search/Aim to `L1/R1`)*

### Useful Runtime Flags

Pass flags after `./toluca.sh` (or after the game path):

- `--fullscreen` — Launch in fullscreen mode
- `--width <px> --height <px>` — Set window or fullscreen resolution
- `--aspect 4:3|16:9|stretch` — Aspect fitting mode (default `4:3`)
- `--nintendo-buttons` — Swap A/B and X/Y labels for Nintendo-style controllers
- `--swap-shoulders` — Swap `L1/R1` (Search/Aim) and `L2/R2` (Strafe)
- `--no-gamepad` — Disable SDL2 DirectInput gamepad mapping (keyboard only)

## Clean-Room Notice

This bundle is MIT-licensed (with LGPLv2.1 Bink-only FFmpeg libraries in `lib/`) and contains zero copyrighted Konami binaries or game assets.
