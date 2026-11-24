# toluca-recomp

A clean-room, open-source static recompiler and native Linux/OpenGL ES 3.0 runtime for running the PC version of *Silent Hill 2* on [PortMaster](https://portmaster.games/) ARM64 Linux handhelds (1 GB RAM class devices such as Anbernic RG35XX/H700, RK3566, and RK3326) as well as Linux x86-64 desktops.

## Design Goals

- **100% Clean-Room & Open Source (MIT):** Contains zero copyrighted Konami game code, binary blobs, or assets. Built strictly from public specifications (PE32/COFF, Intel x86-32 SDM, Win32/DirectX 8 SDK docs, OpenGL ES 3.0 spec).
- **100% On-Device Binary Generation (`patchscript`):** End users drop their legal, unprotected vanilla v1.0 `sh2pc.exe` and `data/` directory onto their handheld. On first launch, `toluca-recomp` translates `sh2pc.exe` directly into a native AOT code cache (`toluca_code.bin`) on-device using Rust + Cranelift — no PC build step, no GCC, and no Python required on the handheld.
- **1 GB RAM Friendly:** Targets vanilla assets first with streaming per-function compilation (<100 MB peak RAM during patching) and a compact guest memory allocator.
- **Native OpenGL ES 3.0 Renderer:** Translates Direct3D 8 fixed-function and shader pipelines straight to GLSL ES 3.00 with software S3TC/DXT fallback for Mali/PowerVR drivers.

## Workspace Layout

- `crates/toluca-core` — Shared guest CPU state, AOT cache (`toluca_code.bin`) binary format, memory map constants, and host-bridge ABI contract.
- `crates/toluca-recomp` — Clean-room PE32 + `iced-x86` → Cranelift streaming static recompiler CLI (runs inside PortMaster's `patchscript`).
- `crates/toluca-d3d8` — Clean-room Direct3D 8 → OpenGL ES 3.0 renderer and shader translator.
- `crates/toluca-runtime` — Native host executable (4 GB virtual arena, AOT loader, Win32/DirectSound/DirectInput/Bink shims, SDL2 launcher).
- `portmaster/` — PortMaster packaging scripts (`Toluca.sh`, `tools/patchscript`, `port.json`).

See [PLAN.md](PLAN.md) for the full architecture and phased development roadmap.

## Legal

*Silent Hill 2* is a trademark of Konami Digital Entertainment. This project is an independent, clean-room reimplementation with no affiliation with Konami. No copyrighted game code or assets are included or distributed.
