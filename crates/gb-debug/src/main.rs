//! `gbdbg`: interactive TUI debugger and scriptable command runner for the emulator core.

mod command;
mod session;
mod ui;

use gb_core::GameBoy;
use session::Session;
use std::io::BufRead;
use std::process::ExitCode;

const USAGE: &str = "usage: gbdbg <rom.gb> [--script FILE|-]\n  \
    --script FILE   run commands (one per line) non-interactively and print results; '-' reads stdin";

fn main() -> ExitCode {
    let mut rom = None;
    let mut script = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--script" => script = args.next(),
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if rom.is_none() && !a.starts_with("--") => rom = Some(a),
            _ => {
                eprintln!("gbdbg: unknown argument '{a}'\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(rom) = rom else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let gb = match std::fs::read(&rom)
        .map_err(|e| e.to_string())
        .and_then(|d| GameBoy::new(d).map_err(|e| format!("{e:?}")))
    {
        Ok(gb) => gb,
        Err(e) => {
            eprintln!("gbdbg: {rom}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut session = Session::new(gb);
    match script {
        Some(path) => run_script(&mut session, &path),
        None => match ui::run(&mut session) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("gbdbg: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn run_script(session: &mut Session, path: &str) -> ExitCode {
    let reader: Box<dyn BufRead> = if path == "-" {
        Box::new(std::io::stdin().lock())
    } else {
        match std::fs::File::open(path) {
            Ok(f) => Box::new(std::io::BufReader::new(f)),
            Err(e) => {
                eprintln!("gbdbg: {path}: {e}");
                return ExitCode::FAILURE;
            }
        }
    };
    let mut failed = false;
    for line in reader.lines().map_while(Result::ok) {
        match command::parse(&line) {
            Ok(None) => {}
            Ok(Some(cmd)) => {
                println!("> {}", line.trim());
                for l in session.run_command(cmd) {
                    println!("{l}");
                }
                if session.quit {
                    break;
                }
            }
            Err(e) => {
                println!("> {}\nerror: {e}", line.trim());
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
