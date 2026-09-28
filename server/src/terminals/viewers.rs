//! Who decides a terminal's size when several views show it at once (a desktop panel
//! and a phone, or a panel and the bottom tool window).
//!
//! The policy is tmux's `window-size latest`: the PTY follows the most recently active
//! view. A view becomes active when it reports its size, when the user types in it, and
//! when the user clicks into it (the client then re-reports its size). When the active
//! view goes away, the most recently active remaining view's size comes back, so a
//! phone that looked at a session does not leave the desktop at phone width.
//!
//! Every attached socket is told the PTY size (`{"t":"size"}`), so views that lost the
//! size can render the program's output at the size it was drawn for.

/// The views attached to one terminal and the size each one asked for.
#[derive(Debug, Default)]
pub struct Viewers {
    next_id: u64,
    clock: u64,
    list: Vec<Viewer>,
}

#[derive(Debug)]
struct Viewer {
    id: u64,
    /// `(cols, rows)` the view fits, once it reported one (hidden views never do).
    size: Option<(u16, u16)>,
    /// Logical time of the last activity; higher is more recent.
    active: u64,
}

impl Viewers {
    /// A view attached. Returns its id; it counts as least recently active until it acts.
    pub fn join(&mut self) -> u64 {
        self.next_id += 1;
        self.list.push(Viewer { id: self.next_id, size: None, active: 0 });
        self.next_id
    }

    pub fn leave(&mut self, id: u64) {
        self.list.retain(|v| v.id != id);
    }

    /// The view reported the size it fits (it is active now).
    pub fn report(&mut self, id: u64, cols: u16, rows: u16) {
        self.clock += 1;
        let now = self.clock;
        if let Some(v) = self.list.iter_mut().find(|v| v.id == id) {
            v.size = Some((cols, rows));
            v.active = now;
        }
    }

    /// The user acted in the view (typed, clicked a program's mouse area).
    pub fn touch(&mut self, id: u64) {
        self.clock += 1;
        let now = self.clock;
        if let Some(v) = self.list.iter_mut().find(|v| v.id == id) {
            v.active = now;
        }
    }

    /// The size the PTY should have: that of the most recently active view that reported
    /// one. `None` when no attached view has reported a size (keep the current one).
    pub fn preferred(&self) -> Option<(u16, u16)> {
        self.list.iter().filter(|v| v.size.is_some()).max_by_key(|v| v.active).and_then(|v| v.size)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.list.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_view_wins_and_the_previous_size_comes_back_when_it_leaves() {
        let mut v = Viewers::default();
        let desk = v.join();
        v.report(desk, 120, 32);
        assert_eq!(v.preferred(), Some((120, 32)));
        // A phone attaches and reports its size: it wins.
        let phone = v.join();
        assert_eq!(v.preferred(), Some((120, 32)), "a view that has not reported does not change the size");
        v.report(phone, 50, 20);
        assert_eq!(v.preferred(), Some((50, 20)));
        // Typing on the desktop takes the size back without a new report.
        v.touch(desk);
        assert_eq!(v.preferred(), Some((120, 32)));
        v.touch(phone);
        assert_eq!(v.preferred(), Some((50, 20)));
        // The phone leaves: the desktop's size returns.
        v.leave(phone);
        assert_eq!(v.preferred(), Some((120, 32)));
        v.leave(desk);
        assert_eq!(v.preferred(), None);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn hidden_views_that_never_reported_do_not_count() {
        let mut v = Viewers::default();
        let hidden = v.join();
        v.touch(hidden);
        assert_eq!(v.preferred(), None);
        let shown = v.join();
        v.report(shown, 100, 30);
        v.touch(hidden);
        assert_eq!(v.preferred(), Some((100, 30)));
    }
}
