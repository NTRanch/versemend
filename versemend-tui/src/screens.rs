//! Screen text models: pure functions from console state to bounded
//! display lines. Both the ratatui panels and line mode render from
//! these lines, so the two surfaces cannot diverge.

use crate::app::{ConsoleState, Screen};
use crate::facade::{Mode, RunStatus};

/// Maximum characters kept on one rendered screen line.
pub const SCREEN_LINE_CHARS_MAX: usize = 160;

/// Truncate a line to the screen bound, on a char boundary.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!("{kept}…")
}

fn line(text: String) -> String {
    truncate_chars(&text, SCREEN_LINE_CHARS_MAX)
}

/// Render the active screen's lines from state.
pub fn screen_lines(state: &ConsoleState) -> Vec<String> {
    match state.screen {
        Screen::Overview => overview_lines(state),
        Screen::Ops => ops_lines(state),
        Screen::Lua => lua_lines(state),
        Screen::Log => state.log.iter().cloned().collect(),
        Screen::Fleet => fleet_lines(state),
    }
}

/// Overview: versions, mode availability, last run states.
pub fn overview_lines(state: &ConsoleState) -> Vec<String> {
    let mut out = Vec::new();
    match &state.status {
        Some(s) => {
            out.push(line(format!(
                "versemend-tui {} — host console",
                crate::facade::TUI_VERSION
            )));
            out.push(line(format!("host versemend {}", s.host_version)));
            out.push(line(format!(
                "host bin {} [{}]",
                s.host_bin.display(),
                if s.host_bin_present {
                    "present"
                } else {
                    "MISSING"
                }
            )));
            out.push(line(format!(
                "jitrunner versemend-jitrunner {}",
                s.jitrunner_version
            )));
            out.push(line(format!(
                "jitrunner bin [{}]",
                if s.jitrunner_present {
                    "present"
                } else {
                    "MISSING"
                }
            )));
            out.push(line(format!("game dir {}", s.game_dir.display())));
            out.push(line(format!(
                "modes: --selftest [{}]  --strand-proof [{}]  --screenshot [{}]",
                if s.host_bin_present {
                    "available"
                } else {
                    "unavailable"
                },
                if s.host_bin_present {
                    "available"
                } else {
                    "unavailable"
                },
                if s.screenshot.mode_present {
                    "present, not run"
                } else {
                    "absent"
                },
            )));
            out.push(line(format!("screenshot: {}", s.screenshot.note)));
        }
        None => out.push("status: not loaded yet (refresh)".to_string()),
    }
    out.push(line(format!("run mode: {}", state.run_mode.label())));
    out.push("provider: none — the host routes no model calls".to_string());
    if state.last_runs.is_empty() {
        out.push("last runs: none this session".to_string());
    } else {
        for run in &state.last_runs {
            out.push(line(format!(
                "last run: {} {} (exit {}, {} ms)",
                run.mode.label(),
                run.status.label(),
                run.exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".into()),
                run.duration_ms
            )));
        }
    }
    out
}

/// Ops: runnable modes, staged invocation, last outcomes.
pub fn ops_lines(state: &ConsoleState) -> Vec<String> {
    let mut out = vec![
        format!("Ops — run mode: {}", state.run_mode.label()),
        "plan = stage the exact invocation, wait for approval".to_string(),
        "auto = execute directly".to_string(),
    ];
    for mode in Mode::all() {
        out.push(line(format!("  {}  (host {})", mode.label(), mode.arg())));
    }
    match &state.staged {
        Some(staged) => {
            out.push(line(format!("STAGED: {}", staged.invocation)));
            out.push("approve with `approve` / key a — abort with `abort` / key x".to_string());
        }
        None => out.push("staged: none".to_string()),
    }
    for run in &state.last_runs {
        let marker = if run.status == RunStatus::Passed {
            "PASS"
        } else {
            "FAIL"
        };
        out.push(line(format!(
            "  [{marker}] {} -> {} (exit {})",
            run.mode.label(),
            run.status.label(),
            run.exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".into())
        )));
    }
    out
}

/// Lua view: module names + sizes (read-only file facts).
pub fn lua_lines(state: &ConsoleState) -> Vec<String> {
    let mut out = vec!["Lua game modules (host/game/*.lua, read-only)".to_string()];
    if state.modules.is_empty() {
        out.push("  (none loaded — refresh, or game dir missing)".to_string());
    }
    let mut total: u64 = 0;
    for module in &state.modules {
        total = total.saturating_add(module.size_bytes);
        out.push(line(format!(
            "  {:<16} {:>8} bytes",
            module.name, module.size_bytes
        )));
    }
    out.push(line(format!(
        "  total {} bytes across {} modules",
        total,
        state.modules.len()
    )));
    out
}

/// Fleet: no trigger rules tonight; the audit tail is present.
pub fn fleet_lines(state: &ConsoleState) -> Vec<String> {
    let mut out = vec![
        "Fleet / triggers".to_string(),
        "content lane: restricted (BLOCH) — this console's work stays in the versemend repo".to_string(),
        "trigger rules: 0 (none declared tonight; nothing fires that is not listed)".to_string(),
        "audit tail:".to_string(),
    ];
    if state.audit.is_empty() {
        out.push("  (audit empty)".to_string());
    }
    for entry in &state.audit {
        out.push(line(format!("  {entry}")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Console;
    use crate::facade::{FakeFacade, RunOutcome};

    #[test]
    fn overview_shows_versions_and_screenshot_note() {
        let console = Console::new(FakeFacade::new());
        let text = overview_lines(console.state()).join("\n");
        assert!(text.contains("host versemend 1.0.0"));
        assert!(text.contains("versemend-jitrunner 0.1.0"));
        assert!(text.contains("provider: none"));
    }

    #[test]
    fn lua_lines_list_modules_with_sizes() {
        let console = Console::new(FakeFacade::new());
        let text = lua_lines(console.state()).join("\n");
        assert!(text.contains("app.lua"));
        assert!(text.contains("32899"));
    }

    #[test]
    fn fleet_lines_state_zero_rules() {
        let console = Console::new(FakeFacade::new());
        let text = fleet_lines(console.state()).join("\n");
        assert!(text.contains("trigger rules: 0"));
    }

    // --- adversarial ---

    #[test]
    fn ops_lines_render_failure_as_fail_never_pass() {
        let mut console = Console::new(FakeFacade::new());
        console.state_mut().last_runs.push(RunOutcome {
            mode: Mode::Selftest,
            status: RunStatus::Failed,
            exit_code: Some(3),
            lines: vec!["SELFTEST failures 3".to_string()],
            truncated: false,
            duration_ms: 5,
        });
        let text = ops_lines(console.state()).join("\n");
        assert!(text.contains("[FAIL] selftest -> FAIL (exit 3)"));
        assert!(!text.contains("[PASS] selftest"));
    }

    #[test]
    fn truncate_chars_bounds_oversized_lines() {
        let long = "y".repeat(SCREEN_LINE_CHARS_MAX + 50);
        let kept = truncate_chars(&long, SCREEN_LINE_CHARS_MAX);
        assert_eq!(kept.chars().count(), SCREEN_LINE_CHARS_MAX + 1);
    }

    #[test]
    fn overview_missing_host_bin_is_shown_missing() {
        let mut fake = FakeFacade::new();
        if let Some(status) = fake.status_result.as_mut() {
            status.host_bin_present = false;
        }
        let console = Console::new(fake);
        let text = overview_lines(console.state()).join("\n");
        assert!(text.contains("MISSING"));
        assert!(text.contains("unavailable"));
    }
}
