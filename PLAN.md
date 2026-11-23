# toluca-portmaster — Architecture & Development Plan

## 1. Core Constraints & Principles

1. **Zero PC steps for end users:** Users drop their own legal, unprotected vanilla North American v1.0 `sh2pc.exe` and `data/` folder onto the handheld. PortMaster's `patchscript` invokes `toluca-recomp` on-device to produce `toluca_code.bin`.
2. **100% Clean-Room Repository (MIT):** Every component is written from scratch using public documentation (PE32/COFF, Intel x86-32 SDM, Win32/Direct3D 8/DirectSound/DirectInput docs, OpenGL ES 3.0 spec, FFmpeg/SDL2 APIs). No copyrighted game bytes and no unlicensed third-party port code are copied into this repository.
3. **1 GB RAM Hardware Budget:** Designed for ARM64 Linux handhelds with 1 GB unified RAM (e.g., Allwinner H700 / Mali-G31, Rockchip RK3566 / Mali-G52, RK3326 / Mali-G31, PowerVR GE8300):
   - Recompilation stays under **100 MB peak RAM** by compiling one function at a time and streaming machine code out to disk.
   - Runtime avoids wasteful power-of-two/quarantine allocators and targets vanilla assets first.
4. **No External Linker or Toolchain On-Device:** `toluca-recomp` emits a self-contained AOT code cache (`toluca_code.bin`) rather than C source or ELF objects requiring `gcc`/`ld`.

---

## 2. High-Level Architecture

```text
[User's vanilla sh2pc.exe]
          │
          ▼  (First launch inside PortMaster patchscript — ~15-45s, <100 MB RAM)
   ┌─────────────┐
   │  toluca-recomp │  PE32 parser → iced-x86 CFG discovery → Cranelift streaming codegen
   └──────┬──────┘
          │ writes
          ▼
   [toluca_code.bin]   Header + source SHA-1 + raw AArch64/x86_64 code + dispatch/reloc tables
          │
          ▼  (Every launch)
   ┌─────────────┐
   │ toluca-runtime │  mmap(PROT_READ|PROT_EXEC) + W^X bridge GOT + 4 GB PROT_NONE guest arena
   ├─────────────┤
   │  Win32/SDL2 │  kernel32, user32, msvcrt, DirectSound (SDL2 audio), DirectInput, Bink (FFmpeg)
   ├─────────────┤
   │  toluca-d3d8   │  D3D8 COM vtables → GLSL ES 3.00 shaders + software DXT fallback
   └─────────────┘
```

### Why `toluca_code.bin` Instead of an ELF `.so`?
- **No linker in `patchscript`:** Cranelift produces raw machine code + relocation records per function. Writing our own flat AOT cache removes any dependency on `ld`/`lld` on the handheld.
- **Bounded memory:** `toluca-recomp` compiles one function, appends its bytes and relocations to `toluca_code.bin`, clears Cranelift's `Context`, and moves on. Peak RAM stays low even on 1 GB devices.
- **Desktop/handheld parity:** The exact same loader and binary format run on `x86_64` Linux (fast local debugging + Unicorn differential testing) and `aarch64` Linux (PortMaster handhelds).
- **W^X clean:** At startup, `toluca-runtime` maps the code blob read-only + executable (`PROT_READ | PROT_EXEC`) and resolves helper/import addresses into a separate writable jump/pointer table page.

---

## 3. Target Binary Scope (Initial MVP)

- **Target executable:** Unprotected North American v1.0 `sh2pc.exe` (decrypted / no SafeDisc DRM).
- **Detection:** `toluca-recomp` inspects the PE header and SHA-1 hash up front, printing a clear error if a SafeDisc-encrypted or unsupported regional executable is supplied.
- **Assets:** Vanilla `data/` directory only for MVP (original 640×480 FMVs, original textures and menus). Optional Enhanced Edition support and EU v1.1 / Director's Cut profiles are deferred to Phase 7.

---

## 4. Phased Roadmap

### Phase 0 — Workspace & ABI Contract (`toluca-core`)
- [x] Initialize clean-room MIT Cargo workspace (`toluca-core`, `toluca-recomp`, `toluca-d3d8`, `toluca-runtime`).
- [x] Define `GuestCpuState` layout (`eax..edi`, `esp`, `eip`, lazy/materialized flags, x87 FP stack + control/status words, MMX/SSE registers, guest arena base pointer).
- [x] Define `toluca_code.bin` header, section directory, VA→offset dispatch table, and relocation entry formats with versioned magic + checksums.
- [x] Define the `HostBridge` function table ABI for calling native Win32/D3D8/libc shims and handling guest indirect calls (`call reg` / `call [mem]`).

### Phase 1 — PE32 Loader & Function Discovery (`toluca-recomp` Front-End)
- [x] Parse PE32 headers, section tables, import address table (IAT), and entry point cleanly using `object` or a small dedicated parser.
- [x] Verify unprotected v1.0 `sh2pc.exe` and detect SafeDisc-encrypted `.text` sections with clear user diagnostics.
- [x] Implement `iced-x86` disassembly over `.text`: recursive descent from entry point, indirect jump-table pattern recognition, alignment/prologue heuristics, and data-section function-pointer scanning.
- [ ] Emit a summary report (discovered functions, byte coverage, jump tables, imported Win32 DLL/symbol list) and validate against vanilla `sh2pc.exe`. *(Summary report CLI implemented; awaiting real vanilla `sh2pc.exe` validation.)*

### Phase 2 — Streaming Cranelift Lifter & Verification (`toluca-recomp` Back-End)
- [x] Implement the x86-32 → Cranelift IR lifter in small, modular files (`integer.rs`, `flags.rs`, `control_flow.rs`, `fpu.rs`, `simd.rs`, `string_ops.rs`).
- [x] Implement streaming compilation to `toluca_code.bin` (resetting Cranelift context per function and logging peak RSS).
- [x] Build a desktop-only differential test harness (`unicorn-engine` dev-dependency) that runs unit tests and sampled `sh2pc.exe` functions side-by-side against a reference x86 emulator, asserting identical register/flag/memory output. *(Unit differential suites verified across integer, flags, branches, strings, x87, and SSE; sampled `sh2pc.exe` verification pending retail binary.)*
- [ ] Benchmark full `sh2pc.exe` recompilation time and peak RAM on both x86-64 and ARM64.

### Phase 3 — Host Runtime & Boot to `Direct3DCreate8` (`toluca-runtime`)
- [x] Implement 4 GB `PROT_NONE` virtual memory arena (`mmap`) with on-demand page commit and a compact guest heap allocator suited to 1 GB unified RAM.
- [x] Implement `toluca_code.bin` loader (`mmap` RX + bridge pointer table + indirect-call dispatcher).
- [x] Implement Win32 shims required for startup:
  - `kernel32`: file I/O with case-insensitive Windows path resolution, heap/VirtualAlloc, critical sections, events, threads, high-res timers.
  - `user32`: SDL2 window creation and message pump.
  - `msvcrt`: `setjmp`/`longjmp`, formatting, math, and memory routines imported by `sh2pc.exe`.
- [ ] **Milestone Gate:** Vanilla `sh2pc.exe` boots natively on Linux x86-64, loads `data/` files, and reaches `Direct3DCreate8`. *(Verified end-to-end on synthetic PE32 with case-insensitive file I/O, guest `WndProc` callback, and `Direct3DCreate8(220)`; awaiting unprotected vanilla NA v1.0 `sh2pc.exe`.)*

### Phase 4 — Clean-Room Direct3D 8 → OpenGL ES 3.0 Renderer (`toluca-d3d8`)
- [ ] Implement guest-memory COM vtables for `IDirect3D8`, `IDirect3DDevice8`, `IDirect3DTexture8`, `IDirect3DSurface8`, `IDirect3DVertexBuffer8`, and `IDirect3DIndexBuffer8`.
- [ ] Implement D3D8 fixed-function pipeline + `vs_1_1` / `ps_1_x` → `#version 300 es` GLSL shader generator:
  - Handle Y-axis flip and `[0, 1]` → `[-1, 1]` depth remapping in vertex shader epilogues (avoiding desktop-only `glClipControl`).
  - Swizzle `D3DCOLOR` `.bgra` vertex attributes in shaders instead of desktop `GL_BGRA` vertex format.
  - Present via fullscreen textured quad (compatible with 16-bit handheld displays where `glBlitFramebuffer` fails).
- [ ] Implement software S3TC/DXT1/3/5 decompression fallback when `GL_EXT_texture_compression_s3tc` is absent (Mali/PowerVR blobs) and enforce GPU `GL_MAX_TEXTURE_SIZE`.
- [ ] Add a portable shader recipe cache (`toluca-progs.bin`) to eliminate repeat area-load stutters.
- [ ] **Milestone Gate:** Menus and in-game 3D scenes render accurately using an OpenGL ES 3.0 context on Linux desktop.

### Phase 5 — Audio, Input & FMV Playback (`toluca-runtime`)
- [ ] Implement `dsound` software mixer outputting to SDL2 audio.
- [ ] Implement `dinput` over `SDL_GameController` + rumble, honoring PortMaster's `SDL_GAMECONTROLLERCONFIG`.
- [ ] Implement `binkw32` playback via FFmpeg (`libavformat`/`libavcodec`/`libswscale`/`libswresample`) tuned for original 640×480 vanilla FMVs.
- [ ] **Milestone Gate:** Playable vanilla *Silent Hill 2* on Linux x86-64 with sound, controls, saving/loading, and cutscenes.

### Phase 6 — ARM64 Cross-Compilation & PortMaster Packaging (`portmaster/`)
- [ ] Set up reproducible `aarch64-unknown-linux-gnu` cross-compilation (Container/Toolbox targeting PortMaster's baseline glibc).
- [ ] Author `portmaster/tools/patchscript` (validates `sh2pc.exe`, runs `toluca-recomp`, reports progress cleanly, verifies peak RAM <100 MB).
- [ ] Author `portmaster/Toluca.sh`, `port.json`, and resolution/aspect defaults (`640x480` 4:3 and `1280x720` 16:9).
- [ ] Verify end-to-end on real 1 GB ARM64 handheld hardware (cold first launch via `patchscript` → gameplay at steady 30 FPS).

### Phase 7 — Post-MVP Enhancements
- [ ] Support EU v1.1 / Director's Cut unprotected executables.
- [ ] Opt-in Enhanced Edition asset/patch support when `sh2e/` is present and device RAM permits.
- [ ] Widescreen / aspect-ratio options and handheld launcher UI.
