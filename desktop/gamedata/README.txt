Toluca (Linux x86-64 Desktop) — Game Data Instructions
======================================================

1. Required Game Files (Not Included)
-------------------------------------
Copy the following from your legal, unprotected vanilla North American v1.0 PC
installation of Silent Hill 2 into this `gamedata/` directory (either directly
here or inside `gamedata/Silent Hill 2/`):

  - `sh2pc.exe`
  - `data/` (full directory with all subdirectories and assets)

Alternatively, leave the files elsewhere and pass the path to `./toluca.sh`:
  ./toluca.sh "/path/to/Silent Hill 2/sh2pc.exe"

Note: SafeDisc-encrypted retail executables are rejected automatically during
first-run verification.

2. First Launch (Local Recompilation)
-------------------------------------
On first launch, `./toluca.sh` verifies `sh2pc.exe` and compiles it into
`cache/toluca_code.bin` (~15-45 seconds on desktop). Repeat launches skip
recompilation unless `sh2pc.exe` or `bin/toluca-recomp` changes.

3. Clean-Room Notice
--------------------
toluca-recomp is an independent, clean-room MIT-licensed reimplementation.
No copyrighted Konami binaries or game assets are included or distributed.
