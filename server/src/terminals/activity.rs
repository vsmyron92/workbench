//! Output-activity heuristic for agent CLIs that report nothing (Kimi, custom CLIs):
//! *working* while output keeps flowing, *idle* after a quiet spell. It never claims a
//! finished answer is unread and never asks for attention: a quiet terminal may just as
//! well be waiting on a question. (The dialogs Workbench knows are recognized on screen
//! separately, `providers::dialog_on_screen`.)
//!
//! What does not count as the agent working:
//! * the echo of the user's own typing (output right after a keystroke);
//! * the repaint after a resize;
//! * a short burst (a single redraw) or a trickle (a cursor blink, a clock): output
//!   must span `SPAN` and add up to `MIN_BYTES` within `WINDOW`.

use std::collections::VecDeque;

use super::AgentState;

/// Output this soon after a keystroke is its echo.
pub const ECHO_MS: i64 = 350;
/// Output this soon after a resize is the repaint.
pub const RESIZE_MS: i64 = 700;
/// Recent output considered.
pub const WINDOW_MS: i64 = 3000;
/// Output must keep coming for this long to count as work.
pub const SPAN_MS: i64 = 800;
/// And add up to at least this much.
pub const MIN_BYTES: usize = 64;
/// Quiet this long: idle.
pub const QUIET_MS: i64 = 3000;
/// Startup ends at the first quiet spell after some output, or after this long.
pub const STARTUP_MAX_MS: i64 = 30_000;

#[derive(Debug)]
pub struct Activity {
    state: AgentState,
    started_at: i64,
    /// `(time, bytes)` of output that counts.
    recent: VecDeque<(i64, usize)>,
    last_output: i64,
    seen_output: bool,
}

impl Activity {
    pub fn new(now: i64) -> Self {
        Self { state: AgentState::Starting, started_at: now, recent: VecDeque::new(), last_output: 0, seen_output: false }
    }

    pub fn state(&self) -> AgentState {
        self.state
    }

    /// Output happened at `at` before the tracker was watching (it counts for startup only).
    pub fn saw_output(&mut self, at: i64) {
        self.seen_output = true;
        self.last_output = self.last_output.max(at);
    }

    /// A chunk of output at `now`; `typed_at` / `resized_at` are the latest keystroke
    /// and resize (ms, 0 = never).
    pub fn output(&mut self, now: i64, bytes: usize, typed_at: i64, resized_at: i64) {
        self.seen_output = true;
        self.last_output = now;
        if now - typed_at < ECHO_MS || now - resized_at < RESIZE_MS {
            return;
        }
        self.recent.push_back((now, bytes));
        while self.recent.front().is_some_and(|(t, _)| now - t > WINDOW_MS) {
            self.recent.pop_front();
        }
    }

    /// Re-evaluate at `now`. Returns the new state when it changed.
    pub fn tick(&mut self, now: i64) -> Option<AgentState> {
        while self.recent.front().is_some_and(|(t, _)| now - t > WINDOW_MS) {
            self.recent.pop_front();
        }
        let quiet = now - self.last_output >= QUIET_MS;
        let next = match self.state {
            AgentState::Starting => {
                // The splash screen is not work: wait for the first quiet spell.
                if (self.seen_output && quiet) || now - self.started_at >= STARTUP_MAX_MS {
                    self.recent.clear();
                    AgentState::Idle
                } else {
                    AgentState::Starting
                }
            }
            AgentState::Idle => {
                if self.flowing() {
                    AgentState::Working
                } else {
                    AgentState::Idle
                }
            }
            AgentState::Working => {
                let last_counted = self.recent.back().map(|(t, _)| *t).unwrap_or(0);
                if now - last_counted >= QUIET_MS {
                    AgentState::Idle
                } else {
                    AgentState::Working
                }
            }
            other => other,
        };
        (next != self.state).then(|| {
            self.state = next;
            next
        })
    }

    fn flowing(&self) -> bool {
        let (Some(first), Some(last)) = (self.recent.front(), self.recent.back()) else { return false };
        let bytes: usize = self.recent.iter().map(|(_, b)| b).sum();
        last.0 - first.0 >= SPAN_MS && bytes >= MIN_BYTES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Output every `step` ms from `from` to `to` (exclusive).
    fn flow(a: &mut Activity, from: i64, to: i64, step: i64, bytes: usize) -> Vec<AgentState> {
        let mut changes = vec![];
        let mut t = from;
        while t < to {
            a.output(t, bytes, 0, 0);
            changes.extend(a.tick(t));
            t += step;
        }
        changes
    }

    #[test]
    fn startup_ends_at_the_first_quiet_spell() {
        let mut a = Activity::new(1_000_000);
        assert!(flow(&mut a, 1_000_100, 1_002_000, 100, 200).is_empty(), "the splash is not work");
        assert_eq!(a.tick(1_004_500), None);
        assert_eq!(a.tick(1_005_000), Some(AgentState::Idle));
        // A program that prints nothing is idle after a while too.
        let mut quiet = Activity::new(0);
        assert_eq!(quiet.tick(10_000), None);
        assert_eq!(quiet.tick(STARTUP_MAX_MS), Some(AgentState::Idle));
    }

    #[test]
    fn working_while_output_flows_idle_after_quiet() {
        let mut a = Activity::new(0);
        a.output(100, 100, 0, 0);
        assert_eq!(a.tick(3_200), Some(AgentState::Idle));
        let changes = flow(&mut a, 10_000, 12_000, 100, 40);
        assert_eq!(changes, [AgentState::Working]);
        assert_eq!(a.tick(13_000), None);
        assert_eq!(a.tick(14_950), Some(AgentState::Idle));
    }

    #[test]
    fn echo_redraws_and_trickles_are_not_work() {
        let mut a = Activity::new(0);
        a.output(100, 100, 0, 0);
        a.tick(3_200);
        // Typing: every chunk is an echo of a keystroke.
        for t in (10_000..13_000).step_by(150) {
            a.output(t + 20, 30, t, 0);
            assert_eq!(a.tick(t + 20), None, "echo at {t}");
        }
        // A resize repaint.
        a.output(20_000, 5_000, 0, 19_900);
        a.output(20_300, 5_000, 0, 19_900);
        assert_eq!(a.tick(20_300), None);
        // A single burst of redraw (under SPAN_MS).
        a.output(30_000, 2_000, 0, 0);
        a.output(30_200, 2_000, 0, 0);
        assert_eq!(a.tick(30_200), None);
        // A cursor blink: long but tiny.
        let changes = flow(&mut a, 40_000, 46_000, 500, 6);
        assert!(changes.is_empty(), "{changes:?}");
        assert_eq!(a.state(), AgentState::Idle);
    }
}
