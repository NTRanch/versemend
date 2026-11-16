//! `versemend-tui` binary: parse args, build the real facade, run.
//!
//! Usage:
//!   versemend-tui [--root DIR] [--host-bin PATH] [--jitrunner-bin PATH]
//!                 [--game-dir PATH] [--line] [--fake]
//!
//! Defaults under `--root` (the versemend repo root, default `.`):
//! host bin `host/target/debug/versemend`, jitrunner bin
//! `jitrunner/target/debug/versemend-jitrunner`, game dir
//! `host/game`. Full-screen runs when stdout is a TTY; otherwise
//! line mode reads commands from stdin. The Bevy GUI is never
//! launched; `--screenshot` is reported, not run.

#![forbid(unsafe_code)]

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use versemend_tui::app::{run_fullscreen, run_line_mode, Console};
use versemend_tui::facade::{FakeFacade, HostFacade};

struct Args {
    root: PathBuf,
    host_bin: Option<PathBuf>,
    jitrunner_bin: Option<PathBuf>,
    game_dir: Option<PathBuf>,
    line: bool,
    fake: bool,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut args = Args {
        root: PathBuf::from("."),
        host_bin: None,
        jitrunner_bin: None,
        game_dir: None,
        line: false,
        fake: false,
    };
    let mut iter = argv.iter().skip(1);
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--line" => args.line = true,
            "--fake" => args.fake = true,
            "--root" => {
                args.root = PathBuf::from(iter.next().ok_or("--root needs a value")?);
            }
            "--host-bin" => {
                args.host_bin = Some(PathBuf::from(
                    iter.next().ok_or("--host-bin needs a value")?,
                ));
            }
            "--jitrunner-bin" => {
                args.jitrunner_bin = Some(PathBuf::from(
                    iter.next().ok_or("--jitrunner-bin needs a value")?,
                ));
            }
            "--game-dir" => {
                args.game_dir = Some(PathBuf::from(
                    iter.next().ok_or("--game-dir needs a value")?,
                ));
            }
            "--help" | "-h" => return Err("help".to_string()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) if e == "help" => {
            println!("versemend-tui [--root DIR] [--host-bin PATH] [--jitrunner-bin PATH] [--game-dir PATH] [--line] [--fake]");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("versemend-tui: {e}");
            return ExitCode::from(2);
        }
    };
    if args.fake {
        let console = Console::new(FakeFacade::new());
        return run(console, args.line);
    }
    let host_bin = args
        .host_bin
        .unwrap_or_else(|| args.root.join("host/target/debug/versemend"));
    let jitrunner_bin = Some(
        args.jitrunner_bin
            .unwrap_or_else(|| args.root.join("jitrunner/target/debug/versemend-jitrunner")),
    );
    let game_dir = args.game_dir.unwrap_or_else(|| args.root.join("host/game"));
    let facade = HostFacade::new(host_bin, jitrunner_bin, game_dir);
    let console = Console::new(facade);
    run(console, args.line)
}

fn run<F: versemend_tui::facade::VersemendFacade>(
    mut console: Console<F>,
    force_line: bool,
) -> ExitCode {
    let interactive = !force_line && std::io::stdout().is_terminal();
    let result = if interactive {
        run_fullscreen(&mut console)
    } else {
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        run_line_mode(&mut console, stdin.lock(), &mut stdout)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("versemend-tui: {e}");
            ExitCode::FAILURE
        }
    }
}
