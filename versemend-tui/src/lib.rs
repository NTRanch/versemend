//! `versemend-tui` — operator console for the Versemend game host
//! (fleet TUI programme D-1562, rung 1; DESIGN §D2 adapted to a game
//! host: this is an operator console, not a player UI).
//!
//! The TUI programs against the narrow [`facade::VersemendFacade`]
//! trait. The real facade runs the host binary's own `--selftest` and
//! `--strand-proof` modes as bounded subprocesses with streamed
//! output; the host's `--screenshot` mode is reported, never run (it
//! drives the Bevy GUI platform and needs a display — see the Media
//! follow-on note in `facade::ScreenshotInfo`). The Bevy GUI itself is
//! never launched. There is no provider control: the host routes no
//! model calls, so the control is honestly absent, not faked.
//!
//! # Limits (all named, all enforced)
//!
//! - Log buffer: [`LOG_LINES_MAX`] lines.
//! - Audit buffer: [`AUDIT_LINES_MAX`] lines.
//! - One output line: [`LINE_BYTES_MAX`] bytes before truncation.
//! - One run's captured output: [`OUTPUT_BYTES_MAX`] bytes.
//! - Lua module listing: [`MODULES_MAX`] entries.
//! - Run deadline: [`DEADLINE_SECS_DEFAULT`] s default,
//!   [`DEADLINE_SECS_MAX`] s ceiling.
//! - Input line (line mode): [`INPUT_LINE_BYTES_MAX`] bytes.

#![forbid(unsafe_code)]

pub mod app;
pub mod facade;
pub mod panels;
pub mod screens;

/// Maximum lines retained in the scrolling Log view.
pub const LOG_LINES_MAX: usize = 512;
/// Maximum lines retained in the Fleet audit tail.
pub const AUDIT_LINES_MAX: usize = 512;
/// Maximum bytes kept from one streamed output line.
pub const LINE_BYTES_MAX: usize = 2_048;
/// Maximum bytes of captured output kept per run (both streams).
pub const OUTPUT_BYTES_MAX: usize = 262_144;
/// Maximum Lua modules listed from `host/game/`.
pub const MODULES_MAX: usize = 64;
/// Default subprocess deadline for a host mode run, in seconds.
pub const DEADLINE_SECS_DEFAULT: u64 = 120;
/// Hard ceiling for a run deadline, in seconds.
pub const DEADLINE_SECS_MAX: u64 = 600;
/// Maximum bytes accepted for one line-mode input line.
pub const INPUT_LINE_BYTES_MAX: usize = 1_024;
