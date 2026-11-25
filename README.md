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
- `portmaster/` — PortMaster packaging scripts (`Toluca.sh`, `tools/patchscript`, `toluca.gptk`, `port.json`).

See [PLAN.md](PLAN.md) for the full architecture and phased development roadmap, and [portmaster/README.md](portmaster/README.md) for handheld controls, display modes, and `toluca.conf` options.

## Installing on a PortMaster Handheld

1. Download or build `dist/toluca-portmaster.zip` and extract it to your PortMaster `ports/` directory.
2. Copy your legal, unprotected vanilla North American v1.0 `sh2pc.exe` and `data/` folder into `ports/toluca/gamedata/` (or `ports/toluca/gamedata/Silent Hill 2/`).
3. Launch **Toluca** from PortMaster. On first boot, `patchscript` automatically verifies `sh2pc.exe` and compiles `cache/toluca_code.bin` on-device (~1–3 minutes, <100 MiB peak RAM).

## Building & Packaging

### Linux x86-64 Desktop

Requires a Rust toolchain (`cargo`), SDL2, OpenGL ES 3.0 / EGL, and FFmpeg (`libavformat`, `libavcodec`, `libavutil`) runtime libraries.

```bash
# Build host binaries (target/release/toluca-recomp and target/release/toluca-runtime)
cargo build --release -p toluca-recomp -p toluca-runtime   # or: make host

# Run the full workspace test suite
cargo test --workspace                                     # or: make test
```

To compile and run locally on desktop:

```bash
# 1. Translate sh2pc.exe into a native x86-64 AOT cache
./target/release/toluca-recomp compile "/path/to/Silent Hill 2/sh2pc.exe" -o /tmp/toluca_code.bin

# 2. Launch the native runtime
./target/release/toluca-runtime "/path/to/Silent Hill 2/sh2pc.exe" /tmp/toluca_code.bin
```

To package a self-contained clean-room desktop release zip (`dist/toluca-linux-x86_64.zip`):

```bash
# Build minimal Bink-only LGPL FFmpeg 7.1.1 shared libs into dist/x86_64/libs/
./scripts/build-desktop-ffmpeg.sh

# Package desktop launcher, stripped binaries, and FFmpeg libs (or: make package-desktop)
./scripts/package-desktop.sh
```

### Cross-Compiling for ARM64 & Generating a PortMaster Zip

Requires `podman` or `docker` and `zip`. Builds run inside [`Containerfile.aarch64`](Containerfile.aarch64) (Debian Bullseye / glibc 2.31 baseline) and bundle minimal Bink-only LGPL FFmpeg 7.1.1 shared libraries.

```bash
# Build stripped aarch64 binaries and minimal FFmpeg libs into dist/aarch64/
./scripts/build-aarch64.sh          # or: make build

# Optional: also run the full aarch64 test suite under qemu-aarch64-static
./scripts/build-aarch64.sh --test   # or: make test-aarch64

# Package the clean-room PortMaster bundle into dist/toluca-portmaster.zip
./scripts/package-portmaster.sh     # or: make package
```

`scripts/package-portmaster.sh` verifies that no copyrighted game binaries, assets, or compiled AOT/shader caches are included in the resulting zip.

For active development on a NextUI (`PORTS.pak`) handheld connected over ADB, the [`Makefile`](Makefile) also provides `make push` (deploys updated binaries and clears `cache/toluca_code.bin` so the handheld recompiles on next boot) and `make push-fast` (runtime-only deploy preserving the existing AOT cache and `toluca.conf`).

## Special Thanks

- **[Silent Hill 2: Enhanced Edition](https://github.com/elishacloud/Silent-Hill-2-Enhancements)** (Elisha Riedlinger & contributors) — For extensive public documentation and issue research on the vanilla 2002 PC port's engine quirks, including CRI-Ware ADX single-core thread-safety behavior, audio-thread `Sleep(0)` polling spins, and Direct3D 8 rendering oddities. *(All Toluca code is written clean-room from scratch; no Enhanced Edition code is included.)*
- **[`sh2-nx`](https://github.com/cutarev/sh2-nx)** (cutarev) — For inspiring this project by proving that statically recompiling the PC version of *Silent Hill 2* to native ARM64 is genuinely viable. *(Toluca is an independent clean-room implementation with no shared code.)*
- **[PortMaster](https://portmaster.games/)** and the **NextUI / `PORTS.pak`** communities — For the handheld runtime ecosystem, `gptokeyb2`, and on-device `patchscript` tooling that make clean-room ports practical on 1 GB ARM64 devices.
- **Open-Source Toolchains & Libraries** — [Cranelift](https://cranelift.dev/) (Bytecode Alliance), [`iced-x86`](https://github.com/icedland/iced), [FFmpeg](https://ffmpeg.org/) (LGPL Bink decoding), and [SDL2](https://www.libsdl.org/).

## Legal

*Silent Hill 2* is a trademark of Konami Digital Entertainment. This project is an independent, clean-room reimplementation with no affiliation with Konami. No copyrighted game code or assets are included or distributed.
