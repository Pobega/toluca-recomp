use std::process::ExitCode;

use toluca_recomp::cfg::discover_functions;
use toluca_recomp::pe::PeImage;
use toluca_recomp::verify::verify_pe;

const EXIT_REJECTED_BINARY: u8 = 2;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || args[1] != "inspect" {
        print_usage(&args);
        return ExitCode::FAILURE;
    }

    let mut path: Option<&str> = None;
    let mut include_imports = false;

    for arg in &args[2..] {
        if arg == "--imports" {
            include_imports = true;
        } else if arg.starts_with('-') {
            eprintln!("Unknown option: {arg}");
            print_usage(&args);
            return ExitCode::FAILURE;
        } else if path.is_none() {
            path = Some(arg.as_str());
        } else {
            eprintln!("Unexpected argument: {arg}");
            print_usage(&args);
            return ExitCode::FAILURE;
        }
    }

    let Some(exe_path) = path else {
        print_usage(&args);
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

fn print_usage(args: &[String]) {
    let bin = args.first().map(String::as_str).unwrap_or("toluca-recomp");
    eprintln!("Usage: {bin} inspect <path-to-exe> [--imports]");
}
