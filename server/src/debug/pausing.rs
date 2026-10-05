//! Live Watch for debug servers without a side channel (J-Link, pyOCD, st-util, QEMU…): the values
//! are read by stopping the program for a moment, reading the memory through the debugger and
//! resuming it. Every round is `pause`, `readMemory` for each value, `continue`.
//!
//! That disturbs the program, so it is the user's call, per session (`live::set_pausing`, off by
//! default), and never faster than every 100 ms. What keeps it from looking like debugging:
//!
//! * **The stop is not shown.** While a round is in flight the session's `quiet` state is set, and
//!   the `pause` stop it causes is swallowed by the event handler (`session::on_event`): the state
//!   stays `running`, the stop counter does not move, nothing is announced. Any other stop is
//!   real: a breakpoint hit during the round, or the user's own pause (which waits for the round
//!   to end, `Session::quiet_idle`), and a real stop is never resumed from here.
//! * **While the program is already stopped** (a breakpoint, a step) the memory is read at no cost
//!   and without a pause: the values keep moving as you step.
//! * **Nearby values are one read.** Values in ordinary memory within a few bytes of each other
//!   are read together (the program is stopped for less time); peripheral registers are always
//!   read one by one, at their exact size, because a read can change what a neighbouring register holds.
//! * **A round that fails** (the debugger would not stop the program, the memory could not be read)
//!   shows the reason on every value and makes the next rounds wait longer. If the program cannot be
//!   resumed after being stopped, the stop is shown as the real stop it now is.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::live::{self, Item, Sample};
use super::session::{self, REQUEST_TIMEOUT, Session, SessionState};
use crate::app::AppState;

/// How long the debugger has to stop the program once asked.
const PAUSE_WAIT: Duration = Duration::from_millis(2000);
/// Values this close together in ordinary memory are read in one request…
const MERGE_GAP: u64 = 16;
/// …as long as one request covers no more than this.
const MERGE_MAX: u64 = 256;

/// What one round came to.
#[derive(Debug)]
pub enum Outcome {
    /// Nothing to report (the session is not in a state to read, or the program was stopped by someone else).
    Skip,
    /// The readings, and how long the program was stopped for them (none if it was stopped anyway).
    Done(Vec<Sample>, Option<u32>),
    /// The round could not read: every value carries the reason.
    Failed(Vec<Sample>),
}

/// A request that reads a stretch of memory and the values in it.
#[derive(Debug, PartialEq)]
pub struct Group {
    pub start: u64,
    pub len: usize,
    /// Index into the items, and the offset of the value in the stretch.
    pub members: Vec<(usize, usize)>,
}

/// Which reads to make: values in ordinary memory that lie close together share one, peripheral registers each have their own.
pub fn plan_reads(items: &[Item]) -> Vec<Group> {
    let mut order: Vec<usize> = (0..items.len()).filter(|i| items[*i].address.is_some() && items[*i].size > 0).collect();
    order.sort_by_key(|i| items[*i].address);
    let mut groups: Vec<Group> = vec![];
    for i in order {
        let (address, size) = (items[i].address.unwrap(), u64::from(items[i].size));
        let alone = items[i].peripheral.is_some();
        if let Some(g) = groups.last_mut().filter(|g| !alone && g.members.iter().all(|(m, _)| items[*m].peripheral.is_none())) {
            let end = g.start + g.len as u64;
            if address <= end + MERGE_GAP && address.max(end) + size - g.start <= MERGE_MAX && address >= g.start {
                let new_end = end.max(address + size);
                g.members.push((i, (address - g.start) as usize));
                g.len = (new_end - g.start) as usize;
                continue;
            }
        }
        groups.push(Group { start: address, len: size as usize, members: vec![(i, 0)] });
    }
    groups
}

fn sample(item: &Item, t: i64, result: Result<serde_json::Value, String>) -> Sample {
    match result {
        Ok(v) => Sample { id: item.id, t, v: Some(v), e: None },
        Err(e) => Sample { id: item.id, t, v: None, e: Some(e) },
    }
}

/// Read every value now. The program must already be stopped.
async fn read_items(s: &Arc<Session>, items: &[Item], t: i64) -> Vec<Sample> {
    let mut out = Vec::with_capacity(items.len());
    for g in plan_reads(items) {
        if s.stopping() {
            break; // the session is ending: the program is resumed first
        }
        match s.read_memory(g.start, g.len).await {
            Ok(bytes) => {
                for (i, offset) in &g.members {
                    let item = &items[*i];
                    out.push(sample(item, t, Ok(live::decode(item.kind, item.size, &bytes[*offset..*offset + item.size as usize]))));
                }
            }
            // A stretch that failed as a whole: its values may still be readable one by one (one of them is not).
            Err(_) if g.members.len() > 1 => {
                for (i, _) in &g.members {
                    let item = &items[*i];
                    let r = s.read_memory(item.address.unwrap_or(0), item.size as usize).await.map(|b| live::decode(item.kind, item.size, &b)).map_err(|e| e.message);
                    out.push(sample(item, t, r));
                }
            }
            Err(e) => out.push(sample(&items[g.members[0].0], t, Err(e.message))),
        }
    }
    out
}

/// What became of the stop we asked for.
enum Stop {
    /// It arrived: this thread stopped.
    Ours(Option<i64>),
    /// The program is stopped for another reason (a breakpoint, the user): not ours to resume.
    Theirs,
    /// It did not arrive in time.
    Never,
}

/// Wait up to `limit` for the stop we asked for. The state is looked at first: a stop that was shown is somebody else's
/// (a breakpoint that hit as we asked), whatever else arrived with it, and a program stopped for real is never resumed here.
async fn wait_for_stop(s: &Session, limit: Duration) -> Stop {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if s.state() != SessionState::Running {
            return Stop::Theirs;
        }
        {
            let q = s.quiet.lock();
            if q.stopped {
                return Stop::Ours(q.thread);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Stop::Never;
        }
        let _ = tokio::time::timeout(Duration::from_millis(25), s.quiet_changed.notified()).await;
    }
}

/// Ends the round however it ends (an early return, a dropped future).
struct Round<'a>(&'a Session);

impl Drop for Round<'_> {
    fn drop(&mut self) {
        self.0.end_round();
    }
}

/// Resume the program that this round stopped. False when it should not be resumed (it was stopped for real meanwhile) or
/// could not be: then the stop is shown as the real stop it has become.
async fn resume(state: &AppState, s: &Arc<Session>, thread: i64) -> bool {
    // Somebody else's stop since (a breakpoint, the user's pause): the program stays stopped.
    if s.state() != SessionState::Running {
        return false;
    }
    match s.request("continue", json!({ "threadId": thread }), REQUEST_TIMEOUT).await {
        Ok(_) => true,
        Err(e) => {
            // Stopped, and nothing says so any more: the stop is real now, and the user hears about it (unless the session
            // is already ending, which must not be brought back to life).
            s.end_round();
            if s.is_live() && !s.stopping() && s.state() == SessionState::Running {
                let description = format!("A live read stopped the program and could not resume it: {}", e.message);
                session::on_event(state, s, json!({ "event": "stopped", "body": { "reason": "pause", "threadId": thread, "allThreadsStopped": true, "description": description } }));
            }
            false
        }
    }
}

/// One round: read every value, stopping the program for the moment it takes if it is running.
pub async fn read(state: &AppState, s: &Arc<Session>, items: &[Item]) -> Outcome {
    if s.stopping() {
        return Outcome::Skip;
    }
    let now = crate::util::now_ms();
    match s.state() {
        // Suspended anyway: the memory costs nothing to read, and the values follow every step.
        SessionState::Stopped => Outcome::Done(read_items(s, items, now).await, None),
        SessionState::Running => paused_read(state, s, items, now).await,
        _ => Outcome::Skip,
    }
}

async fn paused_read(state: &AppState, s: &Arc<Session>, items: &[Item], now: i64) -> Outcome {
    // Not while the user is stepping or has just pressed Pause: that stop is theirs, and one of ours would end their step, or be
    // mistaken for theirs and undone.
    if !s.begin_round() {
        return Outcome::Skip;
    }
    let _round = Round(s);
    let fail = |reason: String| Outcome::Failed(items.iter().map(|i| sample(i, now, Err(reason.clone()))).collect());
    let thread = s.first_thread();
    let began = std::time::Instant::now();
    let asked = s.request("pause", json!({ "threadId": thread }), REQUEST_TIMEOUT).await;
    // The stop may be on its way (or already here) even when the request failed or was slow to answer: look before giving up,
    // because a stop that was swallowed and then forgotten would leave the program stopped with nothing to say so.
    let stop = wait_for_stop(s, if asked.is_ok() { PAUSE_WAIT } else { Duration::from_millis(300) }).await;
    match (stop, asked) {
        (Stop::Theirs, _) => Outcome::Skip,
        (Stop::Never, Err(e)) => fail(format!("the debugger would not stop the program for a moment: {}", e.message)),
        (Stop::Never, Ok(_)) => fail("the program did not stop when asked".into()),
        (Stop::Ours(stopped), asked) => {
            let thread = stopped.unwrap_or(thread);
            // Our stop arrived. If the request failed all the same there is nothing to read from a program in an unknown state,
            // but it is stopped, and is resumed.
            let samples = if asked.is_ok() { read_items(s, items, now).await } else { vec![] };
            if !resume(state, s, thread).await {
                return Outcome::Done(samples, None);
            }
            let stopped_for = Some(began.elapsed().as_millis().min(60_000) as u32);
            if let Err(e) = asked {
                return fail(format!("the debugger would not stop the program for a moment: {}", e.message));
            }
            // Rounds in which nothing could be read stop the program for nothing: count them as failures, so they back off.
            if !samples.is_empty() && samples.iter().all(|x| x.e.is_some()) {
                return Outcome::Failed(samples);
            }
            Outcome::Done(samples, stopped_for)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::live::Kind;

    fn item(id: u32, address: u64, size: u32, peripheral: bool) -> Item {
        Item { id, expression: format!("v{id}"), address: Some(address), size, kind: Kind::Uint, type_name: "uint32_t".into(), peripheral: peripheral.then(|| "a peripheral register".into()), error: None }
    }

    #[test]
    fn values_close_together_in_ram_are_one_read_and_registers_are_never_merged() {
        // Out of order, three close RAM words, one far away, two registers next to each other.
        let items = vec![item(1, 0x2000_0008, 4, false), item(2, 0x2000_0000, 4, false), item(3, 0x2000_0004, 4, false), item(4, 0x2000_1000, 4, false), item(5, 0x5000_0014, 4, true), item(6, 0x5000_0018, 4, true)];
        let g = plan_reads(&items);
        assert_eq!(g.len(), 4, "{g:?}");
        assert_eq!(g[0], Group { start: 0x2000_0000, len: 12, members: vec![(1, 0), (2, 4), (0, 8)] });
        assert_eq!(g[1], Group { start: 0x2000_1000, len: 4, members: vec![(3, 0)] });
        assert_eq!((g[2].start, g[2].len, g[2].members.len(), g[3].start), (0x5000_0014, 4, 1, 0x5000_0018));
    }

    #[test]
    fn a_stretch_is_bounded_and_overlapping_values_share_bytes() {
        // A gap of 16 merges, 17 does not; a stretch stops growing at 256 bytes.
        assert_eq!(plan_reads(&[item(1, 0x2000_0000, 4, false), item(2, 0x2000_0014, 4, false)]).len(), 1);
        assert_eq!(plan_reads(&[item(1, 0x2000_0000, 4, false), item(2, 0x2000_0015, 4, false)]).len(), 2);
        let long: Vec<Item> = (0..40).map(|i| item(i + 1, 0x2000_0000 + u64::from(i) * 8, 4, false)).collect();
        let g = plan_reads(&long);
        assert!(g.len() >= 2 && g.iter().all(|g| g.len <= 256), "{:?}", g.iter().map(|g| g.len).collect::<Vec<_>>());
        assert_eq!(g.iter().map(|g| g.members.len()).sum::<usize>(), 40, "every value is in some read");
        // Two names for the same bytes, and a word inside a longer one.
        let g = plan_reads(&[item(1, 0x2000_0000, 8, false), item(2, 0x2000_0000, 4, false), item(3, 0x2000_0004, 2, false)]);
        assert_eq!(g, vec![Group { start: 0x2000_0000, len: 8, members: vec![(0, 0), (1, 0), (2, 4)] }]);
    }

    #[test]
    fn what_cannot_be_read_is_left_out() {
        let mut none = item(1, 0, 4, false);
        none.address = None;
        assert!(plan_reads(&[none, item(2, 0x2000_0000, 0, false)]).is_empty());
        assert!(plan_reads(&[]).is_empty());
    }
}
