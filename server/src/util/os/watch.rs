//! `watch`: change notifications for the files watcher, debounced by notify-debouncer-full
//! (docs/windows-port.md §2, "File watching"). The config folder's and the Workspace cards'
//! watchers use the same [`debouncer`].
//!
//! Unix: notify's inotify watcher, and the caller adds one non-recursive watch per directory
//! it shows: a recursive inotify watch would put one on every directory, `target/` and
//! `node_modules/` included, against a per-user limit.
//! Windows: one recursive `ReadDirectoryChangesW` watch on the root ([`RECURSIVE`]): an open
//! directory handle keeps the folders above it from being renamed, so a handle per directory
//! would pin every folder of the project. The caller drops what a per-directory watch would
//! not have seen. The watcher is ours, not notify's: notify 8 discards a buffer overflow
//! without a word (its rescan event comes with notify 9, still a release candidate). Here an
//! overflow is an event flagged `Flag::Rescan`, as inotify's queue overflow already is. There
//! is no file-id cache either: notify-debouncer-full's walks the whole tree, following links,
//! on every watch and every created folder.

use std::time::Duration;

use notify_debouncer_full::notify;
use notify_debouncer_full::{DebounceEventHandler, Debouncer as Debounced, new_debouncer_opt};

#[cfg(windows)]
pub use win::DirWatcher;

/// Whether one recursive watch on the root sees the whole tree (Windows) rather than one
/// watch per directory (Unix).
pub const RECURSIVE: bool = cfg!(windows);

/// Whether a folder is reported as modified when its entries change (Windows, besides the
/// entries themselves); inotify reports only the entries.
pub const FOLDERS_MODIFY: bool = cfg!(windows);

/// Whether an event flagged `Flag::Rescan` (the watcher lost events) is reported as an
/// overflow: Windows, whose one watch covers the tree. On Linux inotify's queue overflow
/// stays unreported, as it always was.
pub const RESCAN_IS_OVERFLOW: bool = cfg!(windows);

#[cfg(unix)]
type Watcher = notify::RecommendedWatcher;
#[cfg(unix)]
type Cache = notify_debouncer_full::RecommendedCache;
#[cfg(windows)]
type Watcher = DirWatcher;
#[cfg(windows)]
type Cache = notify_debouncer_full::NoCache;

/// A watcher whose events arrive debounced ([`debouncer`]).
pub type Debouncer = Debounced<Watcher, Cache>;

/// A debouncer that hands `handler` the events that settled for `timeout`: notify-debouncer-full's
/// `new_debouncer` on Unix, the same over [`DirWatcher`] on Windows.
pub fn debouncer<F: DebounceEventHandler>(timeout: Duration, handler: F) -> notify::Result<Debouncer> {
    new_debouncer_opt::<F, Watcher, Cache>(timeout, None, handler, Cache::new(), notify::Config::default())
}

/// Notification buffer: 64 KB, the most `ReadDirectoryChangesW` takes for a folder on a
/// network drive.
#[cfg(windows)]
const BUFFER: usize = 64 * 1024;

/// The `(action, name)` records of a `FILE_NOTIFY_INFORMATION` list: the name in UTF-16,
/// relative to the watched directory. A record that would run past the end ends the list.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
fn records(buf: &[u8]) -> Vec<(u32, Vec<u16>)> {
    let u32_at = |at: usize| buf.get(at..at.checked_add(4)?).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
    let mut out = vec![];
    let mut at = 0usize;
    // NextEntryOffset, Action and FileNameLength (bytes), then the name.
    while let (Some(next), Some(action), Some(len)) = (u32_at(at), u32_at(at.saturating_add(4)), u32_at(at.saturating_add(8))) {
        let start = at + 12;
        let Some(name) = start.checked_add(len as usize).and_then(|end| buf.get(start..end)) else { break };
        out.push((action, name.chunks_exact(2).map(|c| u16::from_ne_bytes([c[0], c[1]])).collect()));
        match (next, at.checked_add(next as usize)) {
            (0, _) | (_, None) => break,
            (_, Some(n)) => at = n,
        }
    }
    out
}

#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::OsStringExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use notify_debouncer_full::notify::event::{CreateKind, Flag, ModifyKind, RemoveKind, RenameMode};
    use notify_debouncer_full::notify::{self, Config, Event, EventHandler, EventKind, RecursiveMode, WatcherKind};
    use parking_lot::Mutex;
    use windows_sys::Win32::Foundation::{ERROR_IO_INCOMPLETE, ERROR_MORE_DATA, ERROR_NOTIFY_ENUM_DIR, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME,
        FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE,
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetLongPathNameW, OPEN_EXISTING, ReadDirectoryChangesW,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject};

    use super::{BUFFER, records};
    use crate::util::os::path::is_short_name;
    use crate::util::os::win32::{Handle, wide_path};

    /// Names created, deleted or renamed, and files written (size or last write time).
    const FILTER: FILE_NOTIFY_CHANGE =
        FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE;

    type Shared = Arc<Mutex<dyn EventHandler>>;

    /// `ReadDirectoryChangesW` watches, a thread each, as a notify watcher: create, remove,
    /// modify and the two halves of a rename, and `Flag::Rescan` when events were lost.
    pub struct DirWatcher {
        handler: Shared,
        /// Watched directory → the event that stops its thread.
        watches: HashMap<PathBuf, Arc<Handle>>,
        /// Notification buffer size in bytes.
        buffer: usize,
    }

    impl DirWatcher {
        fn with_buffer<F: EventHandler>(handler: F, buffer: usize) -> DirWatcher {
            DirWatcher { handler: Arc::new(Mutex::new(handler)), watches: HashMap::new(), buffer }
        }
    }

    impl notify::Watcher for DirWatcher {
        fn new<F: EventHandler>(event_handler: F, _config: Config) -> notify::Result<Self> {
            Ok(DirWatcher::with_buffer(event_handler, BUFFER))
        }

        /// Watch the directory `path` (not a file), its subtree too when recursive. Watching it
        /// again restarts its watch.
        fn watch(&mut self, path: &Path, recursive_mode: RecursiveMode) -> notify::Result<()> {
            let fail = |e: io::Error| notify::Error::io(e).add_path(path.to_path_buf());
            if !path.is_dir() {
                return Err(notify::Error::generic("only directories are watched on Windows").add_path(path.to_path_buf()));
            }
            let _ = self.unwatch(path);
            let mut reader = Reader::open(path, recursive_mode == RecursiveMode::Recursive, self.buffer).map_err(fail)?;
            let stop = Arc::new(event().map_err(fail)?);
            let (handler, thread_stop) = (self.handler.clone(), stop.clone());
            // Every request is made by the watch's thread: without a completion port, Windows
            // cancels a thread's pending I/O when it exits, and the caller may be a pooled
            // thread that exits when idle. The first one is waited for, so a folder that
            // cannot be watched (a file system without change notifications) fails the call.
            let (armed_tx, armed) = std::sync::mpsc::sync_channel(1);
            std::thread::Builder::new()
                .name("workbench-watch".into())
                .spawn(move || {
                    let first = reader.arm();
                    let ok = first.is_ok();
                    let _ = armed_tx.send(first);
                    if ok {
                        reader.run(&thread_stop, &handler);
                    }
                })
                .map_err(fail)?;
            armed.recv().unwrap_or_else(|_| Err(io::Error::other("the watch's thread ended"))).map_err(fail)?;
            self.watches.insert(path.to_path_buf(), stop);
            Ok(())
        }

        fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
            let stop = self.watches.remove(path).ok_or_else(notify::Error::watch_not_found)?;
            // SAFETY: a valid event handle; setting it ends the watch's thread, which closes the directory.
            unsafe { SetEvent(stop.0) };
            Ok(())
        }

        fn kind() -> WatcherKind {
            WatcherKind::ReadDirectoryChangesWatcher
        }
    }

    impl Drop for DirWatcher {
        fn drop(&mut self) {
            for stop in self.watches.values() {
                // SAFETY: as in `unwatch`.
                unsafe { SetEvent(stop.0) };
            }
        }
    }

    /// A manual-reset event, not set.
    fn event() -> io::Result<Handle> {
        // SAFETY: no security attributes and no name.
        Handle::new(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) }).ok_or_else(io::Error::last_os_error)
    }

    /// What a completed request brought.
    enum Outcome {
        Changes(Vec<(u32, Vec<u16>)>),
        /// Changes were lost: the buffer overflowed, or did not hold them all.
        Lost,
        Failed(io::Error),
    }

    /// `e` says changes were lost: the buffer overflowed (ERROR_NOTIFY_ENUM_DIR, from the
    /// request or from asking again) or is full (ERROR_MORE_DATA).
    fn lost(e: &io::Error) -> bool {
        matches!(e.raw_os_error().map(|c| c as u32), Some(ERROR_NOTIFY_ENUM_DIR | ERROR_MORE_DATA))
    }

    fn rescan() -> Event {
        Event::new(EventKind::Other).set_flag(Flag::Rescan)
    }

    /// One watched directory and its outstanding request.
    struct Reader {
        dir: Handle,
        path: PathBuf,
        recursive: bool,
        /// Set by the system when the request completes (manual reset).
        done: Handle,
        /// The request's state, boxed: the system writes to it until the request completes.
        ov: Box<OVERLAPPED>,
        /// DWORD-aligned, as `ReadDirectoryChangesW` requires.
        buf: Vec<u32>,
        /// A request is outstanding: `Drop` cancels it and waits for it before `ov` and `buf` go.
        pending: bool,
    }

    // SAFETY: `ov.hEvent` is `done`'s handle, valid in every thread, and a reader is used by
    // one thread at a time (it is moved into its watch's thread).
    unsafe impl Send for Reader {}

    impl Reader {
        fn open(path: &Path, recursive: bool, buffer: usize) -> io::Result<Reader> {
            let wide = wide_path(path)?;
            // SAFETY: `wide` is a NUL-terminated path; no security attributes and no template.
            // Every sharing mode: the watch never keeps others from writing, deleting or renaming.
            let h = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    FILE_LIST_DIRECTORY,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                    std::ptr::null_mut(),
                )
            };
            let dir = Handle::new(h).ok_or_else(io::Error::last_os_error)?;
            Ok(Reader {
                dir,
                path: path.to_path_buf(),
                recursive,
                done: event()?,
                ov: Box::default(),
                buf: vec![0; buffer.div_ceil(4)],
                pending: false,
            })
        }

        /// Ask for the next changes. Those made between two requests wait in a buffer the
        /// system keeps for the handle (as large as ours); when it overflowed, asking fails
        /// at once with ERROR_NOTIFY_ENUM_DIR ([`lost`]).
        fn arm(&mut self) -> io::Result<()> {
            *self.ov = OVERLAPPED { hEvent: self.done.0, ..Default::default() };
            // SAFETY: a valid event handle; the completion sets it again.
            unsafe { ResetEvent(self.done.0) };
            let len = (self.buf.len() * 4) as u32;
            let mut unused = 0u32;
            // SAFETY: `dir` is a directory opened with FILE_LIST_DIRECTORY for overlapped I/O;
            // `buf` (DWORD-aligned, `len` bytes) and `ov` are heap blocks that stay put, and
            // nothing touches or frees them before the request completes: `wait` or `Drop`
            // waits for it.
            let ok = unsafe {
                ReadDirectoryChangesW(self.dir.0, self.buf.as_mut_ptr().cast(), len, i32::from(self.recursive), FILTER, &mut unused, &mut *self.ov, None)
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            self.pending = true;
            Ok(())
        }

        /// Wait for the request to complete; `None` when `stop` is set first (or the wait
        /// fails): the caller returns, and `Drop` cancels the request.
        fn wait(&mut self, stop: &Handle) -> Option<Outcome> {
            let handles = [stop.0, self.done.0];
            // SAFETY: two valid event handles.
            if unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) } != WAIT_OBJECT_0 + 1 {
                return None;
            }
            let mut n = 0u32;
            // SAFETY: the request of `ov` on `dir`; its event is set, so it has completed.
            if unsafe { GetOverlappedResult(self.dir.0, &*self.ov, &mut n, 0) } == 0 {
                let e = io::Error::last_os_error();
                // Still running after all: `pending` stays, so `Drop` waits for it.
                self.pending = e.raw_os_error() == Some(ERROR_IO_INCOMPLETE as i32);
                return Some(if lost(&e) { Outcome::Lost } else { Outcome::Failed(e) });
            }
            self.pending = false;
            if n == 0 {
                // Success with nothing in the buffer: the changes did not fit.
                return Some(Outcome::Lost);
            }
            let bytes: Vec<u8> = self.buf.iter().flat_map(|w| w.to_ne_bytes()).take(n as usize).collect();
            Some(Outcome::Changes(records(&bytes)))
        }

        /// The watch's thread, after its first request: hand every change to `handler` until
        /// `stop` is set or the directory goes away. An error that ends it goes to `handler`.
        fn run(mut self, stop: &Handle, handler: &Shared) {
            let send = |ev: notify::Result<Event>| handler.lock().handle_event(ev);
            let end = |why: io::Error, path: &Path| {
                tracing::debug!("stopped watching {}: {why}", path.display());
                send(Err(notify::Error::io(why).add_path(path.to_path_buf())));
            };
            loop {
                let Some(outcome) = self.wait(stop) else { return };
                match outcome {
                    Outcome::Changes(list) => {
                        let mut unsure = false;
                        for (action, name) in list {
                            let Some(kind) = kind(action) else { continue };
                            let (path, sure) = long_path(&self.path, OsString::from_wide(&name));
                            unsure |= !sure;
                            send(Ok(Event::new(kind).add_path(path)));
                        }
                        if unsure {
                            send(Ok(rescan()));
                        }
                    }
                    Outcome::Lost if !self.path.is_dir() => return end(io::ErrorKind::NotFound.into(), &self.path),
                    Outcome::Lost => send(Ok(rescan())),
                    Outcome::Failed(e) => return end(e, &self.path),
                }
                // Ask again. Changes lost meanwhile make the request fail at once: report them,
                // and ask again (after a pause, should that repeat) until it waits.
                let mut tries = 0u32;
                loop {
                    match self.arm() {
                        Ok(()) => break,
                        Err(e) if lost(&e) => {
                            send(Ok(rescan()));
                            tries += 1;
                            // SAFETY: a valid event handle.
                            if tries > 1 && unsafe { WaitForSingleObject(stop.0, 50) } == WAIT_OBJECT_0 {
                                return;
                            }
                        }
                        Err(e) => return end(e, &self.path),
                    }
                }
            }
        }
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            if self.pending {
                let mut n = 0u32;
                // SAFETY: the outstanding request of `ov` on `dir`: cancel it, then wait until
                // the system is done with `ov` and `buf` (its event is still open: fields drop
                // after this).
                unsafe {
                    CancelIoEx(self.dir.0, &*self.ov);
                    GetOverlappedResult(self.dir.0, &*self.ov, &mut n, 1);
                }
            }
        }
    }

    fn kind(action: u32) -> Option<EventKind> {
        Some(match action {
            FILE_ACTION_ADDED => EventKind::Create(CreateKind::Any),
            FILE_ACTION_REMOVED => EventKind::Remove(RemoveKind::Any),
            FILE_ACTION_MODIFIED => EventKind::Modify(ModifyKind::Any),
            FILE_ACTION_RENAMED_OLD_NAME => EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            FILE_ACTION_RENAMED_NEW_NAME => EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            _ => return None,
        })
    }

    /// `dir` joined with `name` from a notification, which may spell a folder or the file
    /// with its 8.3 short name (`LONGFO~1`; Windows does not say which name it reports).
    /// A short name is made long again while the file exists; `false` when one may be left
    /// (the file is gone), so the caller asks for a rescan. Only names of that form count:
    /// `file.txt~` (vim's backup) and `~$doc.docx` (Office's lock) come and go on every save.
    fn long_path(dir: &Path, name: OsString) -> (PathBuf, bool) {
        let joined = dir.join(&name);
        if !Path::new(&name).components().any(|c| is_short_name(c.as_os_str())) {
            return (joined, true);
        }
        if let Some(long) = long_below(dir, &joined) {
            return (long, true);
        }
        // Gone: its folder may still be there.
        match (joined.parent().and_then(|p| long_below(dir, p)), joined.file_name()) {
            (Some(parent), Some(file)) => (parent.join(file), !is_short_name(file)),
            _ => (joined, false),
        }
    }

    /// `p` (below `dir`) with long names, spelled from `dir` as given.
    fn long_below(dir: &Path, p: &Path) -> Option<PathBuf> {
        let wide = wide_path(p).ok()?;
        let mut out = vec![0u16; 512];
        for _ in 0..3 {
            // SAFETY: `wide` is NUL-terminated and `out` has room for `out.len()` characters.
            let n = unsafe { GetLongPathNameW(wide.as_ptr(), out.as_mut_ptr(), out.len() as u32) } as usize;
            if n == 0 {
                return None;
            }
            if n < out.len() {
                let long = PathBuf::from(OsString::from_wide(&out[..n]));
                return crate::util::os::path::strip_prefix(&long, dir).map(|rest| dir.join(rest));
            }
            // Too small: `n` is the size needed, with the NUL.
            out.resize(n, 0);
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        use notify_debouncer_full::notify::Watcher;

        use super::*;

        type Rx = mpsc::Receiver<notify::Result<Event>>;

        fn watcher(buffer: usize) -> (DirWatcher, Rx) {
            let (tx, rx) = mpsc::channel();
            (DirWatcher::with_buffer(move |ev| drop(tx.send(ev)), buffer), rx)
        }

        /// Events until `done` holds for what arrived, or 10 s.
        fn until(rx: &Rx, mut done: impl FnMut(&[Event]) -> bool) -> Vec<Event> {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut got = vec![];
            while !done(&got) {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(Ok(ev)) => got.push(ev),
                    Ok(Err(e)) => panic!("watch error: {e}"),
                    Err(_) => panic!("timed out; got {got:?}"),
                }
            }
            got
        }

        fn saw(events: &[Event], p: &Path) -> bool {
            events.iter().any(|e| e.paths.iter().any(|q| q == p))
        }

        /// `std::fs::rename`, retried for 5 s: a virus scanner may hold a new file for a
        /// moment. A handle of the watch's own would outlast that.
        fn rename(from: &Path, to: &Path) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while let Err(e) = std::fs::rename(from, to) {
                assert!(Instant::now() < deadline, "rename {}: {e}", from.display());
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// One recursive watch reports changes deep below the root and leaves every folder
        /// under it free to be renamed (a handle per folder would pin them).
        #[test]
        fn a_recursive_watch_sees_the_tree_and_keeps_its_folders_renamable() {
            let dir = tempfile::tempdir().unwrap();
            let root = crate::util::os::path::canonicalize(dir.path()).unwrap();
            std::fs::create_dir_all(root.join("a").join("b")).unwrap();
            let (mut w, rx) = watcher(BUFFER);
            w.watch(&root, RecursiveMode::Recursive).unwrap();
            let file = root.join("a").join("b").join("c.txt");
            std::fs::write(&file, "x").unwrap();
            until(&rx, |got| saw(got, &file));
            rename(&root.join("a"), &root.join("a2"));
            let got = until(&rx, |got| saw(got, &root.join("a2")));
            assert!(got.iter().any(|e| e.kind == EventKind::Modify(ModifyKind::Name(RenameMode::To))), "{got:?}");
            assert!(w.watch(&root.join("a2").join("b").join("c.txt"), RecursiveMode::Recursive).is_err(), "files are not watched");
        }

        /// Changes that do not fit the buffer are lost: that is a rescan.
        #[test]
        fn an_overflow_is_a_rescan() {
            let dir = tempfile::tempdir().unwrap();
            let root = crate::util::os::path::canonicalize(dir.path()).unwrap();
            let gate = Arc::new(Mutex::new(()));
            let (entered_tx, entered) = mpsc::channel();
            let (tx, rx) = mpsc::channel();
            let g = gate.clone();
            // The handler holds the watch's thread while the test holds the gate, so the
            // changes pile up in the system's buffer (as small as ours).
            let mut w = DirWatcher::with_buffer(
                move |ev| {
                    let _ = entered_tx.send(());
                    let _open = g.lock();
                    let _ = tx.send(ev);
                },
                1024,
            );
            w.watch(&root, RecursiveMode::Recursive).unwrap();
            let held = gate.lock();
            std::fs::write(root.join("first.txt"), "x").unwrap();
            entered.recv_timeout(Duration::from_secs(10)).expect("the first change arrives");
            for i in 0..200 {
                std::fs::write(root.join(format!("file-with-a-longer-name-{i:03}.txt")), "x").unwrap();
            }
            drop(held);
            until(&rx, |got| got.iter().any(|e| e.need_rescan()));
        }

        /// Stopping a watch closes its handle: the folders above it can be renamed again.
        #[test]
        fn dropping_the_watcher_releases_the_folder() {
            let dir = tempfile::tempdir().unwrap();
            let outer = dir.path().join("outer");
            std::fs::create_dir_all(outer.join("watched")).unwrap();
            let (mut w, _rx) = watcher(BUFFER);
            w.watch(&outer.join("watched"), RecursiveMode::Recursive).unwrap();
            // Its thread closes the handle as it ends.
            drop(w);
            rename(&outer, &dir.path().join("renamed"));
        }

        /// The thread that asked for the watch may exit (a pooled thread going idle): the
        /// watch lives on, since its thread makes every request.
        #[test]
        fn a_watch_outlives_the_thread_that_made_it() {
            let dir = tempfile::tempdir().unwrap();
            let root = crate::util::os::path::canonicalize(dir.path()).unwrap();
            let (w, rx) = watcher(BUFFER);
            let w = Arc::new(Mutex::new(w));
            let (w2, root2) = (w.clone(), root.clone());
            std::thread::spawn(move || w2.lock().watch(&root2, RecursiveMode::Recursive).unwrap()).join().unwrap();
            // Let the system run down the exited thread's I/O first.
            std::thread::sleep(Duration::from_millis(200));
            let file = root.join("after.txt");
            std::fs::write(&file, "x").unwrap();
            until(&rx, |got| saw(got, &file));
        }

        /// Names with a tilde that are not 8.3 names (vim's backups, Office's locks) created
        /// and deleted at once are plain changes, not a rescan.
        #[test]
        fn backups_and_locks_are_no_rescan() {
            let dir = tempfile::tempdir().unwrap();
            let root = crate::util::os::path::canonicalize(dir.path()).unwrap();
            let (mut w, rx) = watcher(BUFFER);
            w.watch(&root, RecursiveMode::Recursive).unwrap();
            for name in ["x.txt~", "~$d.docx"] {
                std::fs::write(root.join(name), "x").unwrap();
                std::fs::remove_file(root.join(name)).unwrap();
            }
            let end = root.join("end.txt");
            std::fs::write(&end, "x").unwrap();
            let mut got = until(&rx, |got| saw(got, &end));
            // A rescan would follow the batch that carried the names.
            while let Ok(Ok(ev)) = rx.recv_timeout(Duration::from_millis(300)) {
                got.push(ev);
            }
            assert!(!got.iter().any(|e| e.need_rescan()), "{got:?}");
            assert!(saw(&got, &root.join("x.txt~")) && saw(&got, &root.join("~$d.docx")), "{got:?}");
        }

        #[test]
        fn short_names_are_made_long() {
            let dir = tempfile::tempdir().unwrap();
            let root = crate::util::os::path::canonicalize(dir.path()).unwrap();
            assert_eq!(long_path(&root, "plain.txt".into()), (root.join("plain.txt"), true));
            // A short name that is gone cannot be checked…
            assert!(!long_path(&root, "GONE~1.TXT".into()).1);
            assert!(!long_path(&root, r"GONE~1\x.txt".into()).1);
            // …but a tilde alone does not make one: editors' backups and locks come and go.
            for gone in ["file.txt~", "~$doc.docx", "~WRL0001.tmp", r"sub\x.txt~"] {
                assert_eq!(long_path(&root, gone.into()), (root.join(gone), true), "{gone}");
            }
            let long = root.join("a folder with a long name");
            std::fs::create_dir(&long).unwrap();
            std::fs::write(long.join("x.txt"), "x").unwrap();
            let mut short = vec![0u16; 1024];
            let w = wide_path(&long).unwrap();
            // SAFETY: `w` is NUL-terminated and `short` has room for its length.
            let n = unsafe { windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(w.as_ptr(), short.as_mut_ptr(), short.len() as u32) } as usize;
            let short = PathBuf::from(OsString::from_wide(&short[..n]));
            let Some(short_name) = short.file_name().filter(|n| is_short_name(n)) else {
                eprintln!("skipped: no 8.3 names on this volume");
                return;
            };
            let name: OsString = Path::new(short_name).join("x.txt").into();
            assert_eq!(long_path(&root, name), (long.join("x.txt"), true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::records;

    fn record(next: u32, action: u32, name: &str) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let mut b = vec![];
        b.extend(next.to_ne_bytes());
        b.extend(action.to_ne_bytes());
        b.extend(((units.len() * 2) as u32).to_ne_bytes());
        b.extend(units.iter().flat_map(|u| u.to_ne_bytes()));
        b
    }

    fn names(buf: &[u8]) -> Vec<(u32, String)> {
        records(buf).into_iter().map(|(a, n)| (a, String::from_utf16_lossy(&n))).collect()
    }

    #[test]
    fn notification_records_are_read_within_bounds() {
        // 12 bytes of header and 6 of name, padded to the next DWORD.
        let mut buf = record(20, 1, r"a\b");
        buf.resize(20, 0);
        buf.extend(record(0, 5, "é😀"));
        assert_eq!(names(&buf), [(1, r"a\b".to_string()), (5, "é😀".to_string())]);
        // A name running past the end, or an offset past it, ends the list.
        let whole = buf.len();
        assert_eq!(names(&buf[..whole - 1]), [(1, r"a\b".to_string())]);
        let mut bad = record(4096, 2, "x");
        bad.extend(record(0, 1, "never"));
        assert_eq!(names(&bad), [(2, "x".to_string())]);
        assert!(names(&[]).is_empty());
        assert!(names(&[0; 11]).is_empty());
        // A zeroed buffer reads as one empty record, which has no action to report.
        assert_eq!(names(&[0; 64]), [(0, String::new())]);
    }
}
