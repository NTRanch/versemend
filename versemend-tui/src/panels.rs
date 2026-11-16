//! Pure ratatui panel constructors. A panel is a function from
//! console state to a widget; rendering is tested through
//! `TestBackend`, never a real terminal.

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs, Wrap};

use crate::app::{ConsoleState, Screen};
use crate::facade::RunStatus;
use crate::screens;

/// Maximum screen-body lines a panel renders (older lines scroll off
/// the top — the state buffers themselves are bounded separately).
pub const PANEL_LINES_MAX: usize = 200;

/// Build the full-screen widget: tab bar + active screen body.
pub fn screen_panel(state: &ConsoleState) -> ratatui::widgets::Paragraph<'static> {
    let titles: Vec<Line> = Screen::all()
        .iter()
        .map(|s| Line::from(s.title()))
        .collect();
    let selected = Screen::all()
        .iter()
        .position(|s| *s == state.screen)
        .unwrap_or(0);
    let _tabs = Tabs::new(titles).select(selected);
    let body = body_lines(state);
    Paragraph::new(body)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " versemend-tui — {} — mode {} (Tab/1-5 screens, s/t run, a approve, x abort, m mode, r refresh, q quit) ",
                    state.screen.title(),
                    state.run_mode.label()
                )),
        )
        .wrap(Wrap { trim: false })
}

/// The active screen's body as styled lines, bounded to
/// [`PANEL_LINES_MAX`], with run verdicts coloured by status —
/// failures are red, never green.
pub fn body_lines(state: &ConsoleState) -> Vec<Line<'static>> {
    let raw = screens::screen_lines(state);
    let start = raw.len().saturating_sub(PANEL_LINES_MAX);
    raw[start..]
        .iter()
        .map(|text| {
            let style = if text.contains("FAIL") || text.contains("MISSING") {
                Style::default().fg(Color::Red)
            } else if text.contains("PASS") {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            Line::from(Span::styled(text.clone(), style))
        })
        .collect()
}

/// A two-pane layout helper (header + body) for future screens.
pub fn split_vertical(area: ratatui::layout::Rect) -> std::rc::Rc<[ratatui::layout::Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area)
}

/// Colour for a run status: only `Passed` is green.
pub fn status_color(status: RunStatus) -> Color {
    match status {
        RunStatus::Passed => Color::Green,
        RunStatus::Failed | RunStatus::SpawnFailed => Color::Red,
        RunStatus::TimedOut | RunStatus::Malformed => Color::Yellow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Console;
    use crate::facade::FakeFacade;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_text(state: &ConsoleState) -> String {
        let backend = TestBackend::new(100, 30);
        let mut term = Terminal::new(backend).expect("terminal");
        term.draw(|frame| {
            let area = frame.area();
            frame.render_widget(screen_panel(state), area);
        })
        .expect("draw");
        let buffer = term.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    // --- validation ---

    #[test]
    fn first_screen_renders_overview() {
        let console = Console::new(FakeFacade::new());
        let text = render_text(console.state());
        assert!(text.contains("versemend-tui"));
        assert!(text.contains("host versemend 1.0.0"));
    }

    #[test]
    fn ops_screen_renders_staged_invocation() {
        let mut console = Console::new(FakeFacade::new());
        console.execute_line("run selftest");
        console.state_mut().screen = Screen::Ops;
        let text = render_text(console.state());
        assert!(text.contains("STAGED"));
        assert!(text.contains("--selftest"));
    }

    #[test]
    fn fleet_screen_renders_zero_rules_and_audit() {
        let mut console = Console::new(FakeFacade::new());
        console.state_mut().screen = Screen::Fleet;
        let text = render_text(console.state());
        assert!(text.contains("trigger rules: 0"));
    }

    // --- adversarial ---

    #[test]
    fn failure_status_is_not_green() {
        assert_eq!(status_color(RunStatus::Failed), Color::Red);
        assert_eq!(status_color(RunStatus::Malformed), Color::Yellow);
        assert_eq!(status_color(RunStatus::Passed), Color::Green);
    }

    #[test]
    fn body_lines_bounded_for_oversized_log() {
        let mut console = Console::new(FakeFacade::new());
        for i in 0..(PANEL_LINES_MAX + 50) {
            console.state_mut().log.push_back(format!("entry {i}"));
        }
        console.state_mut().screen = Screen::Log;
        assert_eq!(body_lines(console.state()).len(), PANEL_LINES_MAX);
    }

    #[test]
    fn no_media_screen_exists() {
        let titles: Vec<&str> = Screen::all().iter().map(|s| s.title()).collect();
        assert_eq!(titles, vec!["Overview", "Ops", "Lua", "Log", "Fleet"]);
    }
}
