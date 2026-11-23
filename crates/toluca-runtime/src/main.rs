use std::path::{Path, PathBuf};
use std::process::ExitCode;

use toluca_core::cpu::ExitReason;
use toluca_runtime::{ImportRegistry, Runtime, TrapInfo, Win32Config, register_all_win32};

struct CliArgs {
    exe_path: PathBuf,
    aot_path: PathBuf,
    game_dir: Option<PathBuf>,
    headless: bool,
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

    let config = Win32Config {
        game_root,
        headless: opts.headless,
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
    if !runtime.win32().d3d8_create_calls.is_empty() {
        println!("Direct3DCreate8 calls: {:?}", runtime.win32().d3d8_create_calls);
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

fn parse_args(args: &[String]) -> Option<CliArgs> {
    let mut positional: Vec<PathBuf> = Vec::new();
    let mut game_dir: Option<PathBuf> = None;
    let mut headless = false;

    let mut idx = 1usize;
    while idx < args.len() {
        let arg = args[idx].as_str();
        match arg {
            "--headless" => headless = true,
            "--game-dir" => {
                idx += 1;
                let val = args.get(idx)?;
                game_dir = Some(PathBuf::from(val));
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
        headless,
    })
}

fn print_usage(args: &[String]) {
    let bin = args.first().map(String::as_str).unwrap_or("toluca-runtime");
    eprintln!("Usage:");
    eprintln!("  {bin} <exe-path> <aot-path> [--game-dir <dir>] [--headless]");
}
