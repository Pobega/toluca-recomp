Toluca (PortMaster) — Game Data Instructions
====================================================

1. Required Game Files (Not Included)
-------------------------------------
Copy the following from your legal, unprotected vanilla North American v1.0 PC
installation of Silent Hill 2 into this `gamedata/` directory (either directly
here or inside `gamedata/Silent Hill 2/`):

  - `sh2pc.exe`
  - `data/` (full directory with all subdirectories and assets)

Note: SafeDisc-encrypted retail executables are rejected automatically during
first-run verification.

2. First Launch (On-Device Recompilation)
-----------------------------------------
On first launch, PortMaster runs `tools/patchscript` to verify `sh2pc.exe` and
compile it directly on your handheld into `cache/toluca_code.bin` (~1-3 minutes).
Repeat launches skip compilation unless `sh2pc.exe` changes.

3. Supported Screens & Optional `toluca.conf`
------------------------------------------
By default, the runtime runs fullscreen using PortMaster's `DISPLAY_WIDTH` /
`DISPLAY_HEIGHT` (falling back to 640x480) and preserves crisp 4:3 framing:
  - 640x480 (4:3 handhelds): fills the screen natively
  - 1280x720 (16:9 handhelds): centered 4:3 pillarboxed (960x720)
  - 720x720 (1:1 handhelds): centered 4:3 letterboxed (720x540)

You may optionally create `ports/toluca/toluca.conf` to override display
settings:
  TOLUCA_ASPECT="4:3"      # 4:3 | 16:9 | stretch
  TOLUCA_WIDTH="1280"      # optional width override
  TOLUCA_HEIGHT="720"      # optional height override
  TOLUCA_EXTRA_ARGS=""     # optional extra toluca-runtime CLI flags

4. Clean-Room Notice
--------------------
toluca-recomp is an independent, clean-room MIT-licensed reimplementation.
No copyrighted Konami binaries or game assets are included or distributed.
