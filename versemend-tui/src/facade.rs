//! The narrow host surface the console programs against.
//!
//! [`VersemendFacade`] has two implementations: [`HostFacade`] (the
//! real one — bounded subprocesses of the host binary's own modes,
//! plus read-only file facts about `host/game/*.lua`) and
//! [`FakeFacade`] (in-memory, scripted, call-recording; used by every
//! test). A run's verdict is classified from BOTH the exit code and
//! the mode's own success marker in its transcript: exit 0 without
//! the marker is `Malformed`, never `Passed` — a failing or garbled
//! selftest is never green-washed.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::{DEADLINE_SECS_MAX, LINE_BYTES_MAX, OUTPUT_BYTES_MAX};

/// Host crate version, from `host/Cargo.toml` at this rung.
pub const HOST_VERSION: &str = "1.0.0";
/// Jitrunner crate version, from `jitrunner/Cargo.toml` at this rung.
pub const JITRUNNER_VERSION: &str = "0.1.0";
/// This TUI crate's version.
pub const TUI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Typed facade failures. Expected failures are values, never panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacadeError {
    /// A deadline outside `1..=DEADLINE_SECS_MAX`.
    BadDeadline(u64),
    /// The game directory could not be read.
    GameDirUnreadable(String),
    /// A scripted transport failure (fake) or internal channel loss.
    Transport(String),
}

impl std::fmt::Display for FacadeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FacadeError::BadDeadline(secs) => {
                write!(f, "deadline {secs}s outside 1..={DEADLINE_SECS_MAX}s")
            }
            FacadeError::GameDirUnreadable(why) => write!(f, "game dir unreadable: {why}"),
            FacadeError::Transport(why) => write!(f, "transport failure: {why}"),
        }
    }
}

impl std::error::Error for FacadeError {}

/// One host mode the console can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `versemend --selftest` — exit code is the failure count.
    Selftest,
    /// `versemend --strand-proof` — dual-interpreter proof.
    StrandProof,
}

impl Mode {
    /// The CLI flag for this mode.
    pub fn arg(self) -> &'static str {
        match self {
            Mode::Selftest => "--selftest",
            Mode::StrandProof => "--strand-proof",
        }
    }

    /// Short label for panels and logs.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Selftest => "selftest",
            Mode::StrandProof => "strand-proof",
        }
    }

    /// Parse a user-typed mode name; unknown names are rejected.
    pub fn parse(text: &str) -> Option<Mode> {
        match text {
            "selftest" => Some(Mode::Selftest),
            "strand-proof" | "strand" => Some(Mode::StrandProof),
            _ => None,
        }
    }

    /// Both runnable modes, in display order.
    pub fn all() -> [Mode; 2] {
        [Mode::Selftest, Mode::StrandProof]
    }
}

/// Verdict of one run. Only `Passed` may render green.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Exit 0 AND the mode's success marker was present.
    Passed,
    /// The mode ran and reported failure (non-zero exit).
    Failed,
    /// The deadline hit; the child was killed and reaped.
    TimedOut,
    /// The subprocess could not be spawned at all.
    SpawnFailed,
    /// Exit 0 but the expected success marker was absent/garbled.
    Malformed,
}

impl RunStatus {
    /// One-word label.
    pub fn label(self) -> &'static str {
        match self {
            RunStatus::Passed => "PASS",
            RunStatus::Failed => "FAIL",
            RunStatus::TimedOut => "TIMEOUT",
            RunStatus::SpawnFailed => "SPAWN-FAILED",
            RunStatus::Malformed => "MALFORMED",
        }
    }
}

/// The bounded, classified result of one mode run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    /// Which mode ran.
    pub mode: Mode,
    /// Classified verdict (see [`classify`]).
    pub status: RunStatus,
    /// Process exit code, when the process exited on its own.
    pub exit_code: Option<i32>,
    /// Captured output lines (stdout and stderr interleaved, stderr
    /// lines prefixed `! `), each truncated to [`LINE_BYTES_MAX`].
    pub lines: Vec<String>,
    /// True when output was dropped past a bound.
    pub truncated: bool,
    /// Wall-clock duration of the run, in milliseconds.
    pub duration_ms: u64,
}

/// Classify a finished run. This is the anti-greenwash gate: exit 0
/// alone is never enough — the mode's own success marker must appear.
pub fn classify(mode: Mode, exit_code: Option<i32>, lines: &[String]) -> RunStatus {
    match exit_code {
        None => RunStatus::SpawnFailed,
        Some(0) => {
            let marked = match mode {
                Mode::Selftest => lines.iter().any(|l| l.trim() == "SELFTEST failures 0"),
                Mode::StrandProof => lines.iter().any(|l| {
                    l.contains("STRAND-PROOF")
                        && l.contains("spine-5.5=PASS")
                        && l.contains("strand-luajit=PASS")
                }),
            };
            if marked {
                RunStatus::Passed
            } else {
                RunStatus::Malformed
            }
        }
        Some(_) => RunStatus::Failed,
    }
}

/// Availability of the host's `--screenshot` mode. The mode exists in
/// the host binary but renders through the Bevy GUI platform, so it
/// does not run headlessly/cheaply from this console; the Media
/// screen is therefore a named follow-on, not a stub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotInfo {
    /// The host binary (which implements the mode) is present.
    pub mode_present: bool,
    /// Whether this console will run it headlessly. Always false here.
    pub headless_runnable: bool,
    /// Why, in operator language.
    pub note: String,
}

/// Point-in-time host facts for the Overview screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostStatus {
    /// Host crate version.
    pub host_version: String,
    /// Jitrunner crate version.
    pub jitrunner_version: String,
    /// Host binary path.
    pub host_bin: PathBuf,
    /// Host binary exists as a file.
    pub host_bin_present: bool,
    /// Jitrunner binary path, when configured.
    pub jitrunner_bin: Option<PathBuf>,
    /// Jitrunner binary exists as a file.
    pub jitrunner_present: bool,
    /// Game directory (`host/game`).
    pub game_dir: PathBuf,
    /// Screenshot mode availability.
    pub screenshot: ScreenshotInfo,
}

impl HostStatus {
    /// JSON view (the fleet facade shape: status crosses as a Value).
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "host_version": self.host_version,
            "jitrunner_version": self.jitrunner_version,
            "host_bin": self.host_bin.display().to_string(),
            "host_bin_present": self.host_bin_present,
            "jitrunner_present": self.jitrunner_present,
            "game_dir": self.game_dir.display().to_string(),
            "screenshot": {
                "mode_present": self.screenshot.mode_present,
                "headless_runnable": self.screenshot.headless_runnable,
                "note": self.screenshot.note,
            },
        })
    }
}

/// One Lua game module: read-only file facts, never executed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaModule {
    /// File name, e.g. `app.lua`.
    pub name: String,
    /// Size in bytes.
    pub size_bytes: u64,
}

/// The facade trait the UI programs against.
pub trait VersemendFacade {
    /// Host facts for Overview.
    fn status(&mut self) -> Result<HostStatus, FacadeError>;
    /// The mode list this facade can run (names, for Ops).
    fn tools(&mut self) -> Vec<String>;
    /// Lua modules in the game directory (read-only facts).
    fn lua_modules(&mut self) -> Result<Vec<LuaModule>, FacadeError>;
    /// Run one mode, streaming each output line into `sink` as it
    /// arrives, bounded by `deadline_secs` and the output bounds.
    fn run_mode(
        &mut self,
        mode: Mode,
        deadline_secs: u64,
        sink: &mut dyn FnMut(&str),
    ) -> Result<RunOutcome, FacadeError>;
}

/// Validate a deadline before any spawn.
fn check_deadline(deadline_secs: u64) -> Result<(), FacadeError> {
    if deadline_secs == 0 || deadline_secs > DEADLINE_SECS_MAX {
        return Err(FacadeError::BadDeadline(deadline_secs));
    }
    Ok(())
}

/// Truncate one output line to the byte bound, on a char boundary.
fn truncate_line(line: &str) -> (String, bool) {
    if line.len() <= LINE_BYTES_MAX {
        return (line.to_string(), false);
    }
    let mut end = LINE_BYTES_MAX;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}…", &line[..end]), true)
}

/// The real facade: subprocesses of the host binary + file facts.
#[derive(Debug, Clone)]
pub struct HostFacade {
    host_bin: PathBuf,
    jitrunner_bin: Option<PathBuf>,
    game_dir: PathBuf,
}

impl HostFacade {
    /// Build from explicit paths (resolved by `main`).
    pub fn new(host_bin: PathBuf, jitrunner_bin: Option<PathBuf>, game_dir: PathBuf) -> HostFacade {
        HostFacade {
            host_bin,
            jitrunner_bin,
            game_dir,
        }
    }

    fn screenshot_info(&self, host_present: bool) -> ScreenshotInfo {
        ScreenshotInfo {
            mode_present: host_present,
            headless_runnable: false,
            note: "mode exists in the host binary but renders via the Bevy GUI \
                   platform (needs a display); not run by this console — \
                   Media screen is a follow-on rung"
                .to_string(),
        }
    }
}

impl VersemendFacade for HostFacade {
    fn status(&mut self) -> Result<HostStatus, FacadeError> {
        let host_present = self.host_bin.is_file();
        let jit_present = self
            .jitrunner_bin
            .as_ref()
            .map(|p| p.is_file())
            .unwrap_or(false);
        Ok(HostStatus {
            host_version: HOST_VERSION.to_string(),
            jitrunner_version: JITRUNNER_VERSION.to_string(),
            host_bin: self.host_bin.clone(),
            host_bin_present: host_present,
            jitrunner_bin: self.jitrunner_bin.clone(),
            jitrunner_present: jit_present,
            game_dir: self.game_dir.clone(),
            screenshot: self.screenshot_info(host_present),
        })
    }

    fn tools(&mut self) -> Vec<String> {
        Mode::all().iter().map(|m| m.label().to_string()).collect()
    }

    fn lua_modules(&mut self) -> Result<Vec<LuaModule>, FacadeError> {
        let entries = std::fs::read_dir(&self.game_dir)
            .map_err(|e| FacadeError::GameDirUnreadable(e.to_string()))?;
        let mut modules = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| FacadeError::GameDirUnreadable(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("lua") {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            modules.push(LuaModule { name, size_bytes });
            if modules.len() >= crate::MODULES_MAX {
                break;
            }
        }
        modules.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(modules)
    }

    fn run_mode(
        &mut self,
        mode: Mode,
        deadline_secs: u64,
        sink: &mut dyn FnMut(&str),
    ) -> Result<RunOutcome, FacadeError> {
        check_deadline(deadline_secs)?;
        Ok(run_subprocess(
            &self.host_bin,
            mode,
            Duration::from_secs(deadline_secs),
            sink,
        ))
    }
}

/// Run `bin --<mode>` bounded: streamed lines, byte/line caps, a hard
/// deadline after which the child is killed and reaped. Spawn failure
/// is an outcome, not an error — the UI must render it as a failure.
fn run_subprocess(
    bin: &std::path::Path,
    mode: Mode,
    deadline: Duration,
    sink: &mut dyn FnMut(&str),
) -> RunOutcome {
    let started = Instant::now();
    let spawn = Command::new(bin)
        .arg(mode.arg())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawn {
        Ok(c) => c,
        Err(e) => {
            let line = format!("spawn failed: {e}");
            sink(&line);
            return RunOutcome {
                mode,
                status: RunStatus::SpawnFailed,
                exit_code: None,
                lines: vec![line],
                truncated: false,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
    };
    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        let tx2 = tx.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                match line {
                    Ok(l) => {
                        if tx2.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }));
    }
    if let Some(err) = child.stderr.take() {
        let tx2 = tx.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(err).lines() {
                match line {
                    Ok(l) => {
                        if tx2.send(format!("! {l}")).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }));
    }
    drop(tx);
    let mut lines: Vec<String> = Vec::new();
    let mut bytes_kept: usize = 0;
    let mut truncated = false;
    let mut timed_out = false;
    let exit_code: Option<i32>;
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => {
                let (kept, cut) = truncate_line(&line);
                truncated |= cut;
                if bytes_kept.saturating_add(kept.len()) <= OUTPUT_BYTES_MAX {
                    bytes_kept += kept.len();
                    sink(&kept);
                    lines.push(kept);
                } else {
                    truncated = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Ok(Some(status)) = child.try_wait() {
                    exit_code = status.code();
                    break;
                }
            }
        }
        if started.elapsed() > deadline {
            timed_out = true;
            let _ = child.kill();
            exit_code = child.wait().ok().and_then(|s| s.code());
            break;
        }
        if let Ok(Some(status)) = child.try_wait() {
            // Drain whatever the readers still deliver, briefly.
            while let Ok(line) = rx.try_recv() {
                let (kept, cut) = truncate_line(&line);
                truncated |= cut;
                if bytes_kept.saturating_add(kept.len()) <= OUTPUT_BYTES_MAX {
                    bytes_kept += kept.len();
                    sink(&kept);
                    lines.push(kept);
                } else {
                    truncated = true;
                }
            }
            exit_code = status.code();
            break;
        }
    }
    // Reader threads are deliberately NOT joined: after a deadline
    // kill, a grandchild can hold the pipe open and a join would wait
    // out its full lifetime, defeating the deadline. The threads are
    // detached and end at pipe EOF; their sends fail harmlessly once
    // `rx` is dropped at return.
    drop(readers);
    let status = if timed_out {
        RunStatus::TimedOut
    } else {
        classify(mode, exit_code, &lines)
    };
    RunOutcome {
        mode,
        status,
        exit_code,
        lines,
        truncated,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

/// In-memory scripted facade for tests and `--fake` smokes.
#[derive(Debug, Default)]
pub struct FakeFacade {
    /// Scripted status.
    pub status_result: Option<HostStatus>,
    /// Scripted module list.
    pub modules_result: Vec<LuaModule>,
    /// Outcomes returned by successive `run_mode` calls.
    pub run_results: VecDeque<RunOutcome>,
    /// Modes actually run, in order — the plan-mode no-exec proof
    /// asserts this stays empty until approval.
    pub run_calls: Vec<Mode>,
    /// When set, `run_mode` returns this error (server-death shape).
    pub run_error: Option<FacadeError>,
}

impl FakeFacade {
    /// A fake with a plausible present-host status.
    pub fn new() -> FakeFacade {
        FakeFacade {
            status_result: Some(HostStatus {
                host_version: HOST_VERSION.to_string(),
                jitrunner_version: JITRUNNER_VERSION.to_string(),
                host_bin: PathBuf::from("/fake/versemend"),
                host_bin_present: true,
                jitrunner_bin: Some(PathBuf::from("/fake/versemend-jitrunner")),
                jitrunner_present: true,
                game_dir: PathBuf::from("/fake/host/game"),
                screenshot: ScreenshotInfo {
                    mode_present: true,
                    headless_runnable: false,
                    note: "fake: GUI mode, not headless".to_string(),
                },
            }),
            modules_result: vec![
                LuaModule {
                    name: "app.lua".to_string(),
                    size_bytes: 32_899,
                },
                LuaModule {
                    name: "gate.lua".to_string(),
                    size_bytes: 5_325,
                },
            ],
            ..FakeFacade::default()
        }
    }

    /// Queue one outcome for the next run.
    pub fn queue_run(&mut self, outcome: RunOutcome) {
        self.run_results.push_back(outcome);
    }
}

impl VersemendFacade for FakeFacade {
    fn status(&mut self) -> Result<HostStatus, FacadeError> {
        self.status_result
            .clone()
            .ok_or_else(|| FacadeError::Transport("no scripted status".to_string()))
    }

    fn tools(&mut self) -> Vec<String> {
        Mode::all().iter().map(|m| m.label().to_string()).collect()
    }

    fn lua_modules(&mut self) -> Result<Vec<LuaModule>, FacadeError> {
        Ok(self.modules_result.clone())
    }

    fn run_mode(
        &mut self,
        mode: Mode,
        deadline_secs: u64,
        sink: &mut dyn FnMut(&str),
    ) -> Result<RunOutcome, FacadeError> {
        check_deadline(deadline_secs)?;
        self.run_calls.push(mode);
        if let Some(err) = self.run_error.clone() {
            return Err(err);
        }
        let outcome = self.run_results.pop_front().unwrap_or(RunOutcome {
            mode,
            status: RunStatus::Passed,
            exit_code: Some(0),
            lines: match mode {
                Mode::Selftest => vec!["SELFTEST failures 0".to_string()],
                Mode::StrandProof => {
                    vec!["STRAND-PROOF spine-5.5=PASS strand-luajit=PASS".to_string()]
                }
            },
            truncated: false,
            duration_ms: 1,
        });
        for line in &outcome.lines {
            sink(line);
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(mode: Mode, status: RunStatus, lines: Vec<&str>) -> RunOutcome {
        RunOutcome {
            mode,
            status,
            exit_code: Some(0),
            lines: lines.iter().map(|s| s.to_string()).collect(),
            truncated: false,
            duration_ms: 1,
        }
    }

    // --- validation ---

    #[test]
    fn classify_selftest_pass_needs_marker_and_zero_exit() {
        let lines = vec!["SELFTEST failures 0".to_string()];
        assert_eq!(classify(Mode::Selftest, Some(0), &lines), RunStatus::Passed);
    }

    #[test]
    fn classify_strand_pass_needs_both_verdicts() {
        let lines = vec!["STRAND-PROOF spine-5.5=PASS strand-luajit=PASS".to_string()];
        assert_eq!(
            classify(Mode::StrandProof, Some(0), &lines),
            RunStatus::Passed
        );
    }

    #[test]
    fn fake_default_outcomes_pass_and_record_calls() {
        let mut fake = FakeFacade::new();
        let mut seen = Vec::new();
        let out = fake
            .run_mode(Mode::Selftest, 10, &mut |l| seen.push(l.to_string()))
            .expect("run");
        assert_eq!(out.status, RunStatus::Passed);
        assert_eq!(fake.run_calls, vec![Mode::Selftest]);
        assert_eq!(seen, vec!["SELFTEST failures 0".to_string()]);
    }

    #[test]
    fn truncate_line_respects_byte_bound() {
        let long = "x".repeat(LINE_BYTES_MAX + 10);
        let (kept, cut) = truncate_line(&long);
        assert!(cut);
        assert!(kept.len() <= LINE_BYTES_MAX + 4);
    }

    // --- adversarial ---

    #[test]
    fn classify_selftest_failure_is_never_passed() {
        // Host exit code = failure count; 3 means three checks failed.
        let lines = vec!["SELFTEST failures 3".to_string()];
        assert_eq!(classify(Mode::Selftest, Some(3), &lines), RunStatus::Failed);
    }

    #[test]
    fn classify_exit_zero_without_marker_is_malformed() {
        let lines = vec!["garbage transcript, no verdict".to_string()];
        assert_eq!(
            classify(Mode::Selftest, Some(0), &lines),
            RunStatus::Malformed
        );
        let partial = vec!["STRAND-PROOF spine-5.5=PASS strand-luajit=FAIL".to_string()];
        assert_eq!(
            classify(Mode::StrandProof, Some(0), &partial),
            RunStatus::Malformed
        );
    }

    #[test]
    fn classify_strand_failure_exit_is_failed() {
        let lines = vec!["STRAND-PROOF spine-5.5=FAIL strand-luajit=PASS".to_string()];
        assert_eq!(
            classify(Mode::StrandProof, Some(6), &lines),
            RunStatus::Failed
        );
    }

    #[test]
    fn bad_deadline_rejected_before_any_run() {
        let mut fake = FakeFacade::new();
        let err = fake
            .run_mode(Mode::Selftest, 0, &mut |_| {})
            .expect_err("zero deadline");
        assert_eq!(err, FacadeError::BadDeadline(0));
        assert!(fake.run_calls.is_empty(), "no run may be recorded");
        let err = fake
            .run_mode(Mode::Selftest, DEADLINE_SECS_MAX + 1, &mut |_| {})
            .expect_err("oversized deadline");
        assert!(matches!(err, FacadeError::BadDeadline(_)));
    }

    #[test]
    fn fake_transport_error_propagates() {
        let mut fake = FakeFacade::new();
        fake.run_error = Some(FacadeError::Transport("host died mid-call".to_string()));
        let err = fake
            .run_mode(Mode::StrandProof, 10, &mut |_| {})
            .expect_err("transport");
        assert!(matches!(err, FacadeError::Transport(_)));
    }

    #[test]
    fn scripted_failure_outcome_keeps_failed_status() {
        let mut fake = FakeFacade::new();
        fake.queue_run(outcome(
            Mode::Selftest,
            RunStatus::Failed,
            vec!["SELFTEST failures 2"],
        ));
        let out = fake.run_mode(Mode::Selftest, 10, &mut |_| {}).expect("run");
        assert_eq!(out.status, RunStatus::Failed);
    }

    #[test]
    fn subprocess_missing_binary_is_spawn_failed_not_panic() {
        let out = run_subprocess(
            std::path::Path::new("/nonexistent/versemend"),
            Mode::Selftest,
            Duration::from_secs(5),
            &mut |_| {},
        );
        assert_eq!(out.status, RunStatus::SpawnFailed);
        assert_eq!(out.exit_code, None);
    }

    #[test]
    fn subprocess_exit_zero_without_marker_is_malformed() {
        // `true` exits 0 instantly with no marker -> Malformed, proving
        // the harness ran and classified rather than passing it.
        let out = run_subprocess(
            std::path::Path::new("/bin/true"),
            Mode::Selftest,
            Duration::from_secs(5),
            &mut |_| {},
        );
        assert_eq!(out.status, RunStatus::Malformed);
    }

    #[test]
    fn subprocess_deadline_kills_sleeper() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("versemend-tui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let script = dir.join("sleeper.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let started = Instant::now();
        let out = run_subprocess(&script, Mode::Selftest, Duration::from_secs(1), &mut |_| {});
        assert_eq!(out.status, RunStatus::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "deadline must kill well before the sleeper exits"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
