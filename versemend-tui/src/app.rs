//! Console state machine + event loops (full-screen and line mode).
//!
//! State transitions follow validate → prepare → commit → observe:
//! in plan mode a run is *staged* (exact invocation recorded, audit
//! entry written) and the facade is not called until `approve`;
//! `abort` drops the staged run and a later `approve` is a no-op.

use std::collections::VecDeque;
use std::io::Write;
use std::time::Duration;

use crate::facade::{HostStatus, LuaModule, Mode, RunOutcome, VersemendFacade};
use crate::{AUDIT_LINES_MAX, DEADLINE_SECS_DEFAULT, INPUT_LINE_BYTES_MAX, LOG_LINES_MAX};

/// The console's screens. There is deliberately no Media screen:
/// `--screenshot` needs the Bevy GUI platform (see facade docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    /// Versions, availability, last runs.
    #[default]
    Overview,
    /// Selftest / strand-proof runs with plan/auto.
    Ops,
    /// Lua game modules (read-only).
    Lua,
    /// Bounded scrolling run output.
    Log,
    /// Trigger rules (none tonight) + audit tail.
    Fleet,
}

impl Screen {
    /// All screens in Tab order.
    pub fn all() -> [Screen; 5] {
        [
            Screen::Overview,
            Screen::Ops,
            Screen::Lua,
            Screen::Log,
            Screen::Fleet,
        ]
    }

    /// Tab title.
    pub fn title(self) -> &'static str {
        match self {
            Screen::Overview => "Overview",
            Screen::Ops => "Ops",
            Screen::Lua => "Lua",
            Screen::Log => "Log",
            Screen::Fleet => "Fleet",
        }
    }

    /// Next screen in Tab order.
    pub fn next(self) -> Screen {
        let all = Screen::all();
        let idx = all.iter().position(|s| *s == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }

    /// Parse a screen name from line mode.
    pub fn parse(text: &str) -> Option<Screen> {
        match text {
            "overview" => Some(Screen::Overview),
            "ops" => Some(Screen::Ops),
            "lua" => Some(Screen::Lua),
            "log" => Some(Screen::Log),
            "fleet" => Some(Screen::Fleet),
            _ => None,
        }
    }
}

/// Plan stages; Auto executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunMode {
    /// Stage the exact invocation; no facade call before approval.
    #[default]
    Plan,
    /// Execute declared operations directly.
    Auto,
}

impl RunMode {
    /// One-word label.
    pub fn label(self) -> &'static str {
        match self {
            RunMode::Plan => "plan",
            RunMode::Auto => "auto",
        }
    }
}

/// A staged (not yet executed) run, with its exact invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedRun {
    /// Mode to run on approval.
    pub mode: Mode,
    /// Deadline that will be used, in seconds.
    pub deadline_secs: u64,
    /// The exact invocation text shown to the operator.
    pub invocation: String,
}

/// Everything the screens render. Pure data; panels never mutate it.
#[derive(Debug, Default)]
pub struct ConsoleState {
    /// Active screen.
    pub screen: Screen,
    /// Plan/auto run mode (starts in Plan).
    pub run_mode: RunMode,
    /// Last loaded host status.
    pub status: Option<HostStatus>,
    /// Lua modules from the last refresh.
    pub modules: Vec<LuaModule>,
    /// Staged run awaiting approval, if any.
    pub staged: Option<StagedRun>,
    /// Bounded log lines.
    pub log: VecDeque<String>,
    /// Bounded audit lines.
    pub audit: VecDeque<String>,
    /// Outcomes of runs this session, oldest first.
    pub last_runs: Vec<RunOutcome>,
}

/// The console: a facade plus its state.
pub struct Console<F: VersemendFacade> {
    facade: F,
    state: ConsoleState,
    host_bin_label: String,
}

impl<F: VersemendFacade> Console<F> {
    /// New console in plan mode on Overview; loads status + modules.
    pub fn new(facade: F) -> Console<F> {
        let mut console = Console {
            facade,
            state: ConsoleState::default(),
            host_bin_label: "versemend".to_string(),
        };
        console.refresh();
        console
    }

    /// Read-only state for screens/panels.
    pub fn state(&self) -> &ConsoleState {
        &self.state
    }

    /// Mutable state (tests and the event loops).
    pub fn state_mut(&mut self) -> &mut ConsoleState {
        &mut self.state
    }

    /// The facade (tests assert recorded calls through this).
    pub fn facade(&self) -> &F {
        &self.facade
    }

    /// Reload status + Lua modules; failures land in the log.
    pub fn refresh(&mut self) {
        match self.facade.status() {
            Ok(status) => {
                self.host_bin_label = status.host_bin.display().to_string();
                self.state.status = Some(status);
            }
            Err(e) => self.push_log(&format!("status failed: {e}")),
        }
        match self.facade.lua_modules() {
            Ok(modules) => self.state.modules = modules,
            Err(e) => self.push_log(&format!("lua list failed: {e}")),
        }
    }

    fn push_log(&mut self, line: &str) {
        if self.state.log.len() >= LOG_LINES_MAX {
            self.state.log.pop_front();
        }
        self.state.log.push_back(line.to_string());
    }

    fn push_audit(&mut self, line: &str) {
        if self.state.audit.len() >= AUDIT_LINES_MAX {
            self.state.audit.pop_front();
        }
        self.state.audit.push_back(line.to_string());
    }

    /// The exact invocation a run of `mode` will use.
    pub fn invocation_for(&self, mode: Mode, deadline_secs: u64) -> String {
        format!(
            "{} {} (deadline {}s, bounded output {} bytes)",
            self.host_bin_label,
            mode.arg(),
            deadline_secs,
            crate::OUTPUT_BYTES_MAX
        )
    }

    /// Stage a run without executing it (plan mode's prepare step).
    pub fn stage(&mut self, mode: Mode, deadline_secs: u64) -> String {
        let invocation = self.invocation_for(mode, deadline_secs);
        self.state.staged = Some(StagedRun {
            mode,
            deadline_secs,
            invocation: invocation.clone(),
        });
        self.push_audit(&format!("staged {}", mode.label()));
        format!("staged: {invocation}\nrun `approve` to execute, `abort` to drop")
    }

    /// Execute a mode honouring plan/auto: plan stages, auto runs.
    pub fn request_run(&mut self, mode: Mode, deadline_secs: u64) -> String {
        match self.state.run_mode {
            RunMode::Plan => self.stage(mode, deadline_secs),
            RunMode::Auto => self.execute(mode, deadline_secs),
        }
    }

    /// Approve the staged run: the only path from plan to execution.
    pub fn approve(&mut self) -> String {
        match self.state.staged.take() {
            Some(staged) => {
                self.push_audit(&format!("approved {}", staged.mode.label()));
                self.execute(staged.mode, staged.deadline_secs)
            }
            None => "nothing staged".to_string(),
        }
    }

    /// Drop the staged run. A later approve is a no-op.
    pub fn abort(&mut self) -> String {
        match self.state.staged.take() {
            Some(staged) => {
                self.push_audit(&format!("aborted {}", staged.mode.label()));
                format!("aborted staged {}", staged.mode.label())
            }
            None => "nothing staged".to_string(),
        }
    }

    fn execute(&mut self, mode: Mode, deadline_secs: u64) -> String {
        self.push_audit(&format!("run started {}", mode.label()));
        self.push_log(&format!(
            "--- {} started (deadline {}s)",
            mode.label(),
            deadline_secs
        ));
        let mut lines: Vec<String> = Vec::new();
        let result = {
            let mut sink = |line: &str| lines.push(line.to_string());
            self.facade.run_mode(mode, deadline_secs, &mut sink)
        };
        for line in &lines {
            self.push_log(line);
        }
        match result {
            Ok(outcome) => {
                let summary = format!(
                    "{} {} (exit {}, {} ms{})",
                    outcome.mode.label(),
                    outcome.status.label(),
                    outcome
                        .exit_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    outcome.duration_ms,
                    if outcome.truncated {
                        ", output truncated"
                    } else {
                        ""
                    }
                );
                self.push_log(&format!("--- {summary}"));
                self.push_audit(&format!("run finished {summary}"));
                self.state.last_runs.push(outcome);
                summary
            }
            Err(e) => {
                let summary = format!("{} ERROR: {e}", mode.label());
                self.push_log(&format!("--- {summary}"));
                self.push_audit(&format!("run failed {summary}"));
                summary
            }
        }
    }

    /// Execute one line-mode command; returns the text to print.
    /// This is the shared execute path for line mode and the tests.
    pub fn execute_line(&mut self, input: &str) -> String {
        if input.len() > INPUT_LINE_BYTES_MAX {
            return "input too long".to_string();
        }
        let parts: Vec<&str> = input.split_whitespace().collect();
        let Some((cmd, rest)) = parts.split_first() else {
            return String::new();
        };
        match *cmd {
            "help" => "commands: status | ops | lua | log | fleet | screen <name> | \
                 mode plan|auto | plan <mode> | run <mode> | approve | abort | \
                 refresh | quit"
                .to_string(),
            "status" | "overview" => {
                self.state.screen = Screen::Overview;
                crate::screens::overview_lines(&self.state).join("\n")
            }
            "ops" => {
                self.state.screen = Screen::Ops;
                crate::screens::ops_lines(&self.state).join("\n")
            }
            "lua" => {
                self.state.screen = Screen::Lua;
                crate::screens::lua_lines(&self.state).join("\n")
            }
            "log" => {
                self.state.screen = Screen::Log;
                self.state
                    .log
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            "fleet" => {
                self.state.screen = Screen::Fleet;
                crate::screens::fleet_lines(&self.state).join("\n")
            }
            "screen" => match rest.first().and_then(|s| Screen::parse(s)) {
                Some(screen) => {
                    self.state.screen = screen;
                    format!("screen: {}", screen.title())
                }
                None => "unknown screen (overview|ops|lua|log|fleet)".to_string(),
            },
            "mode" => match rest.first() {
                Some(&"plan") => {
                    self.state.run_mode = RunMode::Plan;
                    "run mode: plan".to_string()
                }
                Some(&"auto") => {
                    self.state.run_mode = RunMode::Auto;
                    "run mode: auto".to_string()
                }
                _ => "usage: mode plan|auto".to_string(),
            },
            "plan" => match rest.first().and_then(|s| Mode::parse(s)) {
                Some(mode) => self.stage(mode, DEADLINE_SECS_DEFAULT),
                None => "usage: plan selftest|strand-proof".to_string(),
            },
            "run" => match rest.first().and_then(|s| Mode::parse(s)) {
                Some(mode) => self.request_run(mode, DEADLINE_SECS_DEFAULT),
                None => "usage: run selftest|strand-proof".to_string(),
            },
            "approve" => self.approve(),
            "abort" => self.abort(),
            "refresh" => {
                self.refresh();
                "refreshed".to_string()
            }
            "quit" | "exit" => "bye".to_string(),
            other => format!("unknown command: {other} (try `help`)"),
        }
    }
}

/// Line mode: read commands from `input`, write answers to `output`.
/// Shares `execute_line` with the full-screen path's semantics.
pub fn run_line_mode<F: VersemendFacade>(
    console: &mut Console<F>,
    input: impl std::io::BufRead,
    output: &mut impl Write,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.len() > INPUT_LINE_BYTES_MAX {
            writeln!(output, "input too long")?;
            continue;
        }
        let answer = console.execute_line(&line);
        let trimmed = line.trim();
        if !answer.is_empty() {
            writeln!(output, "{answer}")?;
        }
        if trimmed == "quit" || trimmed == "exit" {
            break;
        }
    }
    Ok(())
}

/// Full-screen ratatui loop. Keys: Tab/1-5 screens, `m` plan/auto,
/// `s` selftest, `t` strand-proof (both honour plan/auto), `a`
/// approve, `x` abort, `r` refresh, `q` quit.
pub fn run_fullscreen<F: VersemendFacade>(console: &mut Console<F>) -> std::io::Result<()> {
    use crossterm::event::{self, Event, KeyCode};
    use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
    use crossterm::ExecutableCommand;
    use ratatui::backend::CrosstermBackend;
    use ratatui::Terminal;

    let mut stdout = std::io::stdout();
    terminal::enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;
    let result = (|| -> std::io::Result<()> {
        loop {
            term.draw(|frame| {
                let area = frame.area();
                frame.render_widget(crate::panels::screen_panel(console.state()), area);
            })?;
            if event::poll(Duration::from_millis(250))? {
                if let Event::Key(key) = event::read()? {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Tab => {
                            console.state.screen = console.state.screen.next();
                        }
                        KeyCode::Char(c @ '1'..='5') => {
                            let idx = (c as usize) - ('1' as usize);
                            console.state.screen = Screen::all()[idx];
                        }
                        KeyCode::Char('m') => {
                            console.state.run_mode = match console.state.run_mode {
                                RunMode::Plan => RunMode::Auto,
                                RunMode::Auto => RunMode::Plan,
                            };
                        }
                        KeyCode::Char('s') => {
                            console.request_run(Mode::Selftest, DEADLINE_SECS_DEFAULT);
                        }
                        KeyCode::Char('t') => {
                            console.request_run(Mode::StrandProof, DEADLINE_SECS_DEFAULT);
                        }
                        KeyCode::Char('a') => {
                            console.approve();
                        }
                        KeyCode::Char('x') => {
                            console.abort();
                        }
                        KeyCode::Char('r') => console.refresh(),
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    })();
    terminal::disable_raw_mode()?;
    term.backend_mut().execute(LeaveAlternateScreen)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facade::{FacadeError, FakeFacade, RunOutcome, RunStatus};

    fn failed_outcome() -> RunOutcome {
        RunOutcome {
            mode: Mode::Selftest,
            status: RunStatus::Failed,
            exit_code: Some(3),
            lines: vec!["SELFTEST failures 3".to_string()],
            truncated: false,
            duration_ms: 7,
        }
    }

    // --- validation ---

    #[test]
    fn plan_mode_stages_exact_invocation() {
        let mut console = Console::new(FakeFacade::new());
        let answer = console.execute_line("run selftest");
        assert!(answer.contains("staged:"));
        assert!(answer.contains("--selftest"));
        assert!(answer.contains("deadline 120s"));
        let staged = console.state().staged.as_ref().expect("staged");
        assert_eq!(staged.mode, Mode::Selftest);
    }

    #[test]
    fn approve_executes_staged_run() {
        let mut console = Console::new(FakeFacade::new());
        console.execute_line("run strand-proof");
        let answer = console.approve();
        assert!(answer.contains("PASS"));
        assert_eq!(console.facade().run_calls, vec![Mode::StrandProof]);
        assert!(console.state().staged.is_none());
    }

    #[test]
    fn auto_mode_runs_directly() {
        let mut console = Console::new(FakeFacade::new());
        console.execute_line("mode auto");
        let answer = console.execute_line("run selftest");
        assert!(answer.contains("PASS"));
        assert_eq!(console.facade().run_calls.len(), 1);
    }

    #[test]
    fn audit_records_stage_approve_finish() {
        let mut console = Console::new(FakeFacade::new());
        console.execute_line("run selftest");
        console.approve();
        let audit = console
            .state()
            .audit
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(audit.contains("staged selftest"));
        assert!(audit.contains("approved selftest"));
        assert!(audit.contains("run finished selftest PASS"));
    }

    #[test]
    fn line_mode_status_and_lua_answer() {
        let mut console = Console::new(FakeFacade::new());
        assert!(console
            .execute_line("status")
            .contains("host versemend 1.0.0"));
        assert!(console.execute_line("lua").contains("app.lua"));
    }

    // --- adversarial ---

    #[test]
    fn plan_mode_no_exec_proof() {
        let mut console = Console::new(FakeFacade::new());
        console.execute_line("plan selftest");
        console.execute_line("run selftest");
        assert!(
            console.facade().run_calls.is_empty(),
            "plan mode must never call the facade before approval"
        );
    }

    #[test]
    fn abort_then_approve_executes_nothing() {
        let mut console = Console::new(FakeFacade::new());
        console.execute_line("run selftest");
        assert!(console.abort().contains("aborted"));
        assert_eq!(console.approve(), "nothing staged");
        assert!(console.facade().run_calls.is_empty());
    }

    #[test]
    fn selftest_failure_renders_as_failure() {
        let mut fake = FakeFacade::new();
        fake.queue_run(failed_outcome());
        let mut console = Console::new(fake);
        console.execute_line("mode auto");
        let answer = console.execute_line("run selftest");
        assert!(answer.contains("FAIL"), "answer was: {answer}");
        assert!(
            !answer.contains("PASS"),
            "failure must not green-wash: {answer}"
        );
        let log = console
            .state()
            .log
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(log.contains("selftest FAIL"));
    }

    #[test]
    fn facade_error_is_rendered_not_panicked() {
        let mut fake = FakeFacade::new();
        fake.run_error = Some(FacadeError::Transport("host died mid-call".to_string()));
        let mut console = Console::new(fake);
        console.execute_line("mode auto");
        let answer = console.execute_line("run selftest");
        assert!(answer.contains("ERROR"));
        assert!(console.state().last_runs.is_empty());
    }

    #[test]
    fn unknown_commands_and_modes_are_rejected() {
        let mut console = Console::new(FakeFacade::new());
        assert!(console.execute_line("run bogus").contains("usage:"));
        assert!(console
            .execute_line("frobnicate")
            .contains("unknown command"));
        assert!(console.facade().run_calls.is_empty());
    }

    #[test]
    fn oversized_input_line_rejected() {
        let mut console = Console::new(FakeFacade::new());
        let long = "x".repeat(INPUT_LINE_BYTES_MAX + 1);
        assert_eq!(console.execute_line(&long), "input too long");
    }

    #[test]
    fn log_buffer_is_bounded() {
        let mut fake = FakeFacade::new();
        fake.queue_run(RunOutcome {
            mode: Mode::Selftest,
            status: RunStatus::Passed,
            exit_code: Some(0),
            lines: (0..(LOG_LINES_MAX + 100))
                .map(|i| format!("line {i}"))
                .collect(),
            truncated: false,
            duration_ms: 1,
        });
        let mut console = Console::new(fake);
        console.execute_line("mode auto");
        console.execute_line("run selftest");
        assert!(console.state().log.len() <= LOG_LINES_MAX);
    }
}
