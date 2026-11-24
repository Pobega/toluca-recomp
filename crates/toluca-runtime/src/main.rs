use std::path::{Path, PathBuf};
use std::process::ExitCode;

use toluca_core::cpu::ExitReason;
use toluca_d3d8::Gles3Backend;
use toluca_runtime::{ImportRegistry, Runtime, TrapInfo, Win32Config, register_all_win32};

struct CliArgs {
    exe_path: PathBuf,
    aot_path: PathBuf,
    game_dir: Option<PathBuf>,
    shader_cache: Option<PathBuf>,
    max_frames: Option<u64>,
    dump_frame: Option<PathBuf>,
    headless: bool,
    trace_imports: bool,
    trace_d3d8: bool,
}

fn main() -> ExitCode {
    let raw_args: Vec<String> = std::env::args().collect();
    let Some(opts) = parse_args(&raw_args) else {
        print_usage(&raw_args);
        return ExitCode::FAILURE;
    };

    let game_root = opts.game_dir.unwrap_or_else(|| {
        opts.exe_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    });

    let exe_name = opts
        .exe_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("sh2pc.exe")
        .to_owned();

    let shader_cache_path = opts
        .shader_cache
        .or_else(|| (!opts.headless).then(|| game_root.join("toluca-progs.bin")));

    let config = Win32Config {
        game_root,
        headless: opts.headless,
        trace_imports: opts.trace_imports,
        trace_d3d8: opts.trace_d3d8,
        shader_cache_path: shader_cache_path.clone(),
        max_frames: opts.max_frames,
        command_line: exe_name.clone(),
        exe_name,
    };

    let mut registry = ImportRegistry::new();
    register_all_win32(&mut registry);

    let mut runtime = match Runtime::load_with_config(&opts.exe_path, &opts.aot_path, registry, config)
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("Failed to load runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    let outcome = match runtime.run_entry() {
        Ok(out) => out,
        Err(err) => {
            eprintln!("Execution error: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "Exit reason: {:?} (detail=0x{:08x}, EAX=0x{:08x}, EIP=0x{:08x}, ESP=0x{:08x})",
        outcome.exit_reason, outcome.exit_detail, outcome.eax, outcome.eip, outcome.esp
    );
    let (d3d8_create_calls, d3d8_present_calls) = {
        let win32 = runtime.win32();
        (win32.d3d8_create_calls.clone(), win32.d3d8_present_calls)
    };
    if !d3d8_create_calls.is_empty() {
        println!(
            "Direct3DCreate8 calls: {d3d8_create_calls:?}, Present frames: {d3d8_present_calls}"
        );
    }
    if let Some(gles) = runtime
        .win32()
        .d3d8
        .backend()
        .as_any()
        .downcast_ref::<Gles3Backend>()
    {
        println!(
            "GLES3 stats: draw_calls={}, present_calls={}, last_gl_error=0x{:04x}",
            gles.draw_calls,
            gles.present_calls,
            gles.last_gl_error()
        );
        if let Some(cache_path) = shader_cache_path.as_deref() {
            if let Err(err) = gles.save_recipe_cache(cache_path) {
                eprintln!("Failed to write shader cache {}: {err}", cache_path.display());
            } else {
                println!("Saved shader recipe cache to {}", cache_path.display());
            }
        }
        if let Some(dump_path) = opts.dump_frame.as_deref() {
            if let Some((w, h, rgba)) = gles.readback_rgba8() {
                if let Err(err) = write_ppm_rgba8(dump_path, w, h, &rgba) {
                    eprintln!("Failed to write frame dump {}: {err}", dump_path.display());
                } else {
                    println!("Dumped {}x{} frame to {}", w, h, dump_path.display());
                }
            }
        }
    }
    if let Some(code) = runtime.win32().exit_code {
        println!("Guest ExitProcess code: {code}");
    }

    if let Some(trap) = &outcome.trap {
        match trap {
            TrapInfo::UnresolvedImport {
                index,
                dll_name,
                symbol,
            } => {
                eprintln!("Trap: unresolved import #{index} ({dll_name}!{symbol:?})");
            }
            TrapInfo::UnresolvedIndirectCall(va) => {
                eprintln!("Trap: unresolved indirect call target 0x{va:08x}");
            }
            TrapInfo::UnimplementedCom {
                interface_id,
                method_index,
            } => {
                eprintln!("Trap: unimplemented COM call interface={interface_id} method={method_index}");
            }
            TrapInfo::UnhandledInstruction { guest_va, reason } => {
                eprintln!("Trap: unhandled instruction at 0x{guest_va:08x} (reason={reason})");
            }
        }
        return ExitCode::FAILURE;
    }

    match outcome.exit_reason {
        ExitReason::Returned | ExitReason::Halt => {
            let code = runtime.win32().exit_code.unwrap_or(0);
            if code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from((code & 0xFF) as u8)
            }
        }
        _ => ExitCode::FAILURE,
    }
}

fn write_ppm_rgba8(path: &Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    let header = format!("P6\n{width} {height}\n255\n");
    let mut out = Vec::with_capacity(header.len() + (width as usize) * (height as usize) * 3);
    out.extend_from_slice(header.as_bytes());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&px[0..3]);
    }
    std::fs::write(path, out)
}

fn parse_args(args: &[String]) -> Option<CliArgs> {
    let mut positional: Vec<PathBuf> = Vec::new();
    let mut game_dir: Option<PathBuf> = None;
    let mut shader_cache: Option<PathBuf> = None;
    let mut max_frames: Option<u64> = None;
    let mut dump_frame: Option<PathBuf> = None;
    let mut headless = false;
    let mut trace_imports = false;
    let mut trace_d3d8 = false;

    let mut idx = 1usize;
    while idx < args.len() {
        let arg = args[idx].as_str();
        match arg {
            "--headless" => headless = true,
            "--trace-imports" => trace_imports = true,
            "--trace-d3d8" => trace_d3d8 = true,
            "--game-dir" => {
                idx += 1;
                let val = args.get(idx)?;
                game_dir = Some(PathBuf::from(val));
            }
            "--shader-cache" => {
                idx += 1;
                let val = args.get(idx)?;
                shader_cache = Some(PathBuf::from(val));
            }
            "--max-frames" => {
                idx += 1;
                let val = args.get(idx)?;
                max_frames = Some(val.parse::<u64>().ok()?);
            }
            "--dump-frame" => {
                idx += 1;
                let val = args.get(idx)?;
                dump_frame = Some(PathBuf::from(val));
            }
            _ if arg.starts_with('-') => {
                eprintln!("Unknown option: {arg}");
                return None;
            }
            _ => positional.push(PathBuf::from(arg)),
        }
        idx += 1;
    }

    if positional.len() != 2 {
        return None;
    }

    Some(CliArgs {
        exe_path: positional.remove(0),
        aot_path: positional.remove(0),
        game_dir,
        shader_cache,
        max_frames,
        dump_frame,
        headless,
        trace_imports,
        trace_d3d8,
    })
}

fn print_usage(args: &[String]) {
    let bin = args.first().map(String::as_str).unwrap_or("toluca-runtime");
    eprintln!("Usage:");
    eprintln!(
        "  {bin} <exe-path> <aot-path> [--game-dir <dir>] [--shader-cache <path>] [--max-frames <N>] [--dump-frame <path.ppm>] [--headless] [--trace-imports] [--trace-d3d8]"
    );
}
