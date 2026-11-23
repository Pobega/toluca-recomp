# PortMaster Packaging

This directory will hold the PortMaster release assets:

- `Toluca.sh` — Handheld launch script (triggers `patchscript` on first run, then launches `toluca-runtime`).
- `tools/patchscript` — On-device patching script that invokes `toluca-recomp` against the user's vanilla `sh2pc.exe` to generate `toluca_code.bin`.
- `port.json` — PortMaster metadata manifest.
