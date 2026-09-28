//! Console sessions and running SQL in them. A session is one connection kept per
//! console (so `BEGIN`, `SET` and temporary tables last across runs), closed after
//! 15 idle minutes. Queries use the simple protocol: every value comes back as text,
//! as psql shows it, and a script's statements each give a result. Rows past the cap
//! are not read: the query is cancelled instead.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use parking_lot::Mutex;
use serde::Serialize;
use tokio_postgres::SimpleQueryMessage;
use tokio_postgres::error::{ErrorPosition, SqlState};

use super::conn::{Canceller, Connected};

pub const DEFAULT_MAX_ROWS: usize = 500;
pub const MAX_ROWS: usize = 10_000;
/// Longest value kept per cell (the rest is cut, marked with …).
const MAX_CELL: usize = 10_000;
/// Text of all results together, at most.
const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const IDLE: Duration = Duration::from_secs(15 * 60);

/// Session key: project, source, console.
pub type Key = (String, String, String);

pub struct Session {
    pub client: tokio_postgres::Client,
    pub canceller: Canceller,
    pub notices: Arc<Mutex<Vec<String>>>,
    pub fingerprint: String,
    /// Held while a query runs: one at a time per console.
    pub busy: tokio::sync::Mutex<()>,
    pub last_used: Mutex<Instant>,
    /// A cancel sent for a query whose request went away. The server cancels whatever
    /// runs when it arrives, so the next query waits for it to land first.
    pending_cancel: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Session {
    pub fn new(c: Connected, fingerprint: String) -> Session {
        Session {
            client: c.client,
            canceller: c.canceller,
            notices: c.notices,
            fingerprint,
            busy: tokio::sync::Mutex::new(()),
            last_used: Mutex::new(Instant::now()),
            pending_cancel: Mutex::new(None),
        }
    }
}

#[derive(Default)]
pub struct Sessions {
    pub map: Mutex<HashMap<Key, Arc<Session>>>,
}

impl Sessions {
    pub fn get(&self, k: &Key) -> Option<Arc<Session>> {
        self.map.lock().get(k).cloned()
    }
    pub fn remove(&self, k: &Key) -> Option<Arc<Session>> {
        self.map.lock().remove(k)
    }
    /// Close sessions idle for `IDLE` (not while a query runs).
    pub fn reap(&self) {
        self.map.lock().retain(|_, s| s.last_used.lock().elapsed() < IDLE || s.busy.try_lock().is_err());
    }
    /// Close every session of a source (its settings changed or it was removed).
    pub fn close_source(&self, pid: &str, source: &str) {
        self.map.lock().retain(|k, _| !(k.0 == pid && k.1 == source));
    }
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSet {
    pub columns: Vec<String>,
    /// Text values as the server sends them; `null` for NULL.
    pub rows: Vec<Vec<Option<String>>>,
    /// More rows existed than were read.
    pub truncated: bool,
    /// Rows the statement returned or changed (its command tag's count).
    pub rows_affected: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlError {
    pub message: String,
    /// SQLSTATE.
    pub code: Option<String>,
    pub detail: Option<String>,
    pub hint: Option<String>,
    /// 1-based character position in the SQL sent.
    pub position: Option<u32>,
    pub severity: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryOutcome {
    pub results: Vec<ResultSet>,
    pub error: Option<SqlError>,
    pub notices: Vec<String>,
    pub ms: u64,
    /// Reading stopped at the row or size cap (the query was cancelled there).
    pub stopped: bool,
}

pub fn sql_error(e: &tokio_postgres::Error) -> SqlError {
    match e.as_db_error() {
        Some(db) => SqlError {
            message: db.message().to_string(),
            code: Some(db.code().code().to_string()),
            detail: db.detail().map(str::to_string),
            hint: db.hint().map(str::to_string),
            position: match db.position() {
                Some(ErrorPosition::Original(p)) => Some(*p),
                _ => None,
            },
            severity: Some(db.severity().to_string()),
        },
        None => SqlError { message: super::conn::connect_error(e), code: None, detail: None, hint: None, position: None, severity: None },
    }
}

fn cut(v: &str) -> String {
    if v.len() <= MAX_CELL {
        return v.to_string();
    }
    let mut end = MAX_CELL;
    while !v.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &v[..end])
}

/// Cancels the running query if the request goes away before it finishes.
struct CancelOnDrop<'a>(Option<&'a Session>);

impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        if let Some(s) = self.0.take() {
            let c = s.canceller.clone();
            let h = tokio::spawn(async move {
                let _ = c.cancel().await;
            });
            *s.pending_cancel.lock() = Some(h);
        }
    }
}

/// Past the row cap, rows are read and dropped for this long (or this many) before
/// the query is cancelled: a cancel lands on whatever runs when it arrives, so one
/// sent for a query that then finishes on its own would cancel the next.
const DRAIN_TIME: Duration = Duration::from_secs(2);
const DRAIN_ROWS: usize = 50_000;

/// Absorb a cancel that may still be on its way: PostgreSQL ignores one that arrives
/// while the session is idle, but not one that arrives during the next query.
async fn absorb_late_cancel(s: &Session) {
    let _ = s.client.simple_query("SELECT pg_sleep(0.3)").await;
}

/// Run `sql` in `s`, keeping at most `max_rows` rows per result.
pub async fn run(s: &Session, sql: &str, max_rows: usize) -> QueryOutcome {
    let started = Instant::now();
    let pending = s.pending_cancel.lock().take();
    if let Some(h) = pending {
        let _ = h.await;
        absorb_late_cancel(s).await;
    }
    s.notices.lock().clear();
    let mut guard = CancelOnDrop(Some(s));
    let mut cap_cancel: Option<tokio::task::JoinHandle<()>> = None;
    let mut cancelled_by_us = false;
    let mut results: Vec<ResultSet> = vec![];
    let mut current: Option<ResultSet> = None;
    let mut error = None;
    let mut stopped: Option<Instant> = None;
    let mut dropped = 0usize;
    let mut bytes = 0usize;
    match s.client.simple_query_raw(sql).await {
        Err(e) => error = Some(sql_error(&e)),
        Ok(stream) => {
            futures::pin_mut!(stream);
            loop {
                let deadline = stopped.filter(|_| cap_cancel.is_none()).map(|t| tokio::time::Instant::from_std(t + DRAIN_TIME));
                let next = tokio::select! {
                    m = stream.next() => m,
                    _ = async { tokio::time::sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                        let c = s.canceller.clone();
                        cap_cancel = Some(tokio::spawn(async move { let _ = c.cancel().await; }));
                        continue;
                    }
                };
                let Some(m) = next else { break };
                match m {
                    Ok(SimpleQueryMessage::RowDescription(cols)) => {
                        results.extend(current.take());
                        current = Some(ResultSet { columns: cols.iter().map(|c| c.name().to_string()).collect(), ..Default::default() });
                    }
                    Ok(SimpleQueryMessage::Row(row)) => {
                        let set = current.get_or_insert_with(|| ResultSet { columns: row.columns().iter().map(|c| c.name().to_string()).collect(), ..Default::default() });
                        if stopped.is_some() || set.rows.len() >= max_rows || bytes >= MAX_BYTES {
                            set.truncated = true;
                            stopped.get_or_insert_with(Instant::now);
                            dropped += 1;
                            if dropped > DRAIN_ROWS && cap_cancel.is_none() {
                                let c = s.canceller.clone();
                                cap_cancel = Some(tokio::spawn(async move {
                                    let _ = c.cancel().await;
                                }));
                            }
                            continue;
                        }
                        let values: Vec<Option<String>> = (0..row.len()).map(|i| row.get(i).map(cut)).collect();
                        bytes += values.iter().map(|v| v.as_ref().map_or(4, String::len)).sum::<usize>();
                        set.rows.push(values);
                    }
                    Ok(SimpleQueryMessage::CommandComplete(n)) => {
                        let mut set = current.take().unwrap_or_default();
                        set.rows_affected = Some(n);
                        results.push(set);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if cap_cancel.is_some() && e.code() == Some(&SqlState::QUERY_CANCELED) {
                            // Ours, at the cap: not an error.
                            cancelled_by_us = true;
                        } else {
                            error = Some(sql_error(&e));
                        }
                        break;
                    }
                }
            }
        }
    }
    results.extend(current.take());
    if let Some(h) = cap_cancel {
        let _ = h.await;
        if !cancelled_by_us {
            // The query ended before our cancel arrived: it may still land.
            absorb_late_cancel(s).await;
        }
    }
    guard.0 = None;
    *s.last_used.lock() = Instant::now();
    let notices = std::mem::take(&mut *s.notices.lock());
    QueryOutcome { results, error, notices, ms: started.elapsed().as_millis() as u64, stopped: stopped.is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_values_are_cut_on_a_char_boundary() {
        let v = "é".repeat(MAX_CELL);
        let c = cut(&v);
        assert!(c.ends_with('…') && c.len() <= MAX_CELL + '…'.len_utf8());
        assert_eq!(cut("short"), "short");
    }
}
