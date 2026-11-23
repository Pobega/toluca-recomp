use std::path::Path;
use std::process::ExitCode;

use toluca_core::aot::TargetArch;
use toluca_recomp::cfg::discover_functions;
use toluca_recomp::compiler::{CompileError, CompileOptions, compile_file};
use toluca_recomp::pe::PeImage;
use toluca_recomp::verify::verify_pe;

const EXIT_REJECTED_BINARY: u8 = 2;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(cmd) = args.get(1).map(String::as_str) else {
        print_usage(&args);
        return ExitCode::FAILURE;
    };

    match cmd {
        "inspect" => run_inspect(&args),
        "compile" => run_compile(&args),
        _ => {
            print_usage(&args);
            ExitCode::FAILURE
        }
    }
}

fn run_inspect(args: &[String]) -> ExitCode {
    let mut path: Option<&str> = None;
    let mut include_imports = false;

    for arg in &args[2..] {
        if arg == "--imports" {
            include_imports = true;
        } else if arg.starts_with('-') {
            eprintln!("Unknown option: {arg}");
            print_usage(args);
            return ExitCode::FAILURE;
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            eprintln!("Unexpected argument: {arg}");
            print_usage(args);
            return ExitCode::FAILURE;
        }
    }

    let Some(exe_path) = path else {
        print_usage(args);
        return ExitCode::FAILURE;
    };

    let bytes = match std::fs::read(exe_path) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("Failed to read '{exe_path}': {err}");
            return ExitCode::FAILURE;
        }
    };

    let image = match PeImage::parse(bytes) {
        Ok(pe) => pe,
        Err(err) => {
            eprintln!("Failed to parse PE32 '{exe_path}': {err}");
            return ExitCode::FAILURE;
        }
    };

    let verification = verify_pe(&image);
    print!("{}", verification.format_summary());

    if verification.is_rejected() {
        return ExitCode::from(EXIT_REJECTED_BINARY);
    }

    let discovery = discover_functions(&image);
    print!("{}", discovery.format_summary(include_imports));

    ExitCode::SUCCESS
}

fn run_compile(args: &[String]) -> ExitCode {
    let mut input_path: Option<&str> = None;
    let mut output_path: Option<&str> = None;
    let mut arch: Option<TargetArch> = TargetArch::current();
    let mut strict_verify = false;

    let mut idx = 2usize;
    while idx < args.len() {
        let arg = args[idx].as_str();
        match arg {
            "-o" | "--output" => {
                idx += 1;
                let Some(val) = args.get(idx).map(String::as_str) else {
                    eprintln!("Missing path after {arg}");
                    print_usage(args);
                    return ExitCode::FAILURE;
                };
                output_path = Some(val);
            }
            "--arch" => {
                idx += 1;
                let Some(val) = args.get(idx).map(String::as_str) else {
                    eprintln!("Missing architecture after --arch");
                    print_usage(args);
                    return ExitCode::FAILURE;
                };
                arch = match parse_arch(val) {
                    Some(parsed) => Some(parsed),
                    None => {
                        eprintln!("Unsupported architecture '{val}' (expected x86_64 or aarch64)");
                        return ExitCode::FAILURE;
                    }
                };
            }
            "--strict" => strict_verify = true,
            _ if arg.starts_with('-') => {
                eprintln!("Unknown option: {arg}");
                print_usage(args);
                return ExitCode::FAILURE;
            }
            _ if input_path.is_none() => input_path = Some(arg),
            _ => {
                eprintln!("Unexpected argument: {arg}");
                print_usage(args);
                return ExitCode::FAILURE;
            }
        }
        idx += 1;
    }

    let (Some(in_path), Some(out_path), Some(target_arch)) = (input_path, output_path, arch) else {
        print_usage(args);
        return ExitCode::FAILURE;
    };

    let options = CompileOptions {
        arch: target_arch,
        strict_verify,
    };

    match compile_file(Path::new(in_path), Path::new(out_path), options) {
        Ok((verification, report)) => {
            print!("{}", verification.format_summary());
            print!("{}", report.format_summary());
            ExitCode::SUCCESS
        }
        Err(CompileError::RejectedBinary(summary)) => {
            eprintln!("{summary}");
            ExitCode::from(EXIT_REJECTED_BINARY)
        }
        Err(err) => {
            eprintln!("Compilation failed: {err}");
            ExitCode::FAILURE
        }
    }
}

fn parse_arch(text: &str) -> Option<TargetArch> {
    match text.to_ascii_lowercase().as_str() {
        "x86_64" | "x64" | "amd64" => Some(TargetArch::X86_64),
        "aarch64" | "arm64" => Some(TargetArch::AArch64),
        _ => None,
    }
}

fn print_usage(args: &[String]) {
    let bin = args.first().map(String::as_str).unwrap_or("toluca-recomp");
    eprintln!("Usage:");
    eprintln!("  {bin} inspect <path-to-exe> [--imports]");
    eprintln!(
        "  {bin} compile <path-to-exe> -o <output.bin> [--arch x86_64|aarch64] [--strict]"
    );
}
