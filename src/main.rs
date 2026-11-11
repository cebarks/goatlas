use goatlas::{exit_code, scan_file};
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

fn run() -> Result<u8, Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next().ok_or("Usage: goatlas <FILE>")?;
    if args.next().is_some() {
        return Err("Usage: goatlas <FILE>".into());
    }
    let mut out = io::stdout().lock();
    if first == "--help" {
        writeln!(
            out,
            "Usage: goatlas <FILE>\n\nInventory recorded Go build identity without executing FILE.\nExit codes: 0 decoded, 2 partial/unavailable, 1 no valid report."
        )?;
        out.flush()?;
        return Ok(0);
    }
    if first == "--version" {
        writeln!(out, "goatlas {}", env!("CARGO_PKG_VERSION"))?;
        out.flush()?;
        return Ok(0);
    }
    let report = scan_file(Path::new(&first))?;
    serde_json::to_writer(&mut out, &report)?;
    writeln!(out)?;
    out.flush()?;
    Ok(exit_code(&report))
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("goatlas: {error}");
            ExitCode::from(1)
        }
    }
}
