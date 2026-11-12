//! What the footer says: the status line and the results, alerts and
//! hints that come and go in it, with the clocks that let a result give
//! way to the count and the hint. `App` holds one and gives it the facts
//! it can't know (how many files are listed, whether work is under way).

use crate::{a11y, model, ways_in};
use std::time::{Duration, Instant};

/// The footer's text and what decides how long each line stays.
pub(super) struct Status {
    /// The status line itself.
    pub(super) text: String,
    /// What the footer says whenever it has nothing more pressing: why
    /// results are saved next to each file, when there's no holding folder.
    pub(super) sticky: Option<&'static str>,
    /// What adding files last said, and when, so the footer's idle hint
    /// can take its place once it has been read.
    pub(super) added: Option<(String, Instant)>,
    /// The line last said about the work under way, so the footer can add
    /// how to cancel the selected file while it still says it.
    pub(super) working: Option<String>,
    /// When the status was first drawn, and the selection then, so a result
    /// or an alert can give way to the count and the hint once the selection
    /// changes or it has been up a few seconds. `None` until drawn.
    pub(super) seen: Option<(Instant, Vec<usize>)>,
    /// The last status that reported something going wrong or not
    /// happening. While the status still says it, it stands over the
    /// selection count.
    pub(super) alert: Option<String>,
    /// Each status set during this message, in order: see `say`.
    pub(super) sets: Vec<String>,
    /// Which status sets are announced.
    pub(super) teller: a11y::StatusTeller,
}

impl Status {
    /// An empty status line, with `sticky` as what the footer falls back on.
    pub(super) fn new(sticky: Option<&'static str>) -> Self {
        Status {
            text: String::new(),
            sticky,
            added: None,
            working: None,
            seen: None,
            alert: None,
            sets: Vec::new(),
            teller: a11y::StatusTeller::default(),
        }
    }

    /// Say `text` in the footer as something that went wrong or didn't
    /// happen, which the selection count doesn't hide.
    pub(super) fn alert(&mut self, text: String) {
        self.alert = Some(text.clone());
        self.say(text);
    }

    /// Set the status line. Each set is told to UI Automation on its own,
    /// once the message is done, so an alert set again with the same words
    /// is said again.
    pub(super) fn say(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.seen = None;
        self.sets.push(self.text.clone());
    }

    /// Say `line` about the work under way, and remember it as that line,
    /// so the footer adds how to cancel the selected file while it still
    /// says it.
    pub(super) fn say_working(&mut self, line: String) {
        self.say(line);
        self.working = Some(self.text.clone());
    }

    /// Whether the status is an alert that still stands.
    pub(super) fn alerting(&self) -> bool {
        self.alert.as_deref() == Some(self.text.as_str())
    }

    /// Whether the status is a result or an alert that may give way to the
    /// plain count `plain`: not the work under way, not the count itself,
    /// and not while `busy` (a removal that can still be undone, a burn or
    /// an export).
    pub(super) fn can_expire(&self, plain: &str, busy: bool) -> bool {
        !self.text.is_empty()
            && self.text != plain
            && self.working.as_deref() != Some(self.text.as_str())
            && !busy
    }

    /// Let a result or an alert go once the selection has changed since it
    /// was first drawn, or it has been up [`ways_in::STATUS_FOR`]: the
    /// status goes back to the count `plain`. Run as each frame starts.
    /// Returns how much longer a result that stays has to, for the timer
    /// that wakes for the rest.
    pub(super) fn expire(
        &mut self,
        plain: String,
        busy: bool,
        selection: impl FnOnce() -> Vec<usize>,
    ) -> Option<Duration> {
        if !self.can_expire(&plain, busy) {
            self.seen = None;
            return None;
        }
        let selection = selection();
        let (since, then) = self
            .seen
            .get_or_insert_with(|| (Instant::now(), selection.clone()));
        let (shown, changed) = (since.elapsed(), *then != selection);
        if ways_in::status_stale(shown, changed) {
            self.text = plain;
            self.alert = None;
            self.seen = None;
            None
        } else {
            ways_in::status_wait(shown)
        }
    }

    /// How much longer "Added 2 files." has to stay, if it's still what the
    /// footer says and its two seconds aren't up.
    pub(super) fn hint_wait(&self) -> Option<Duration> {
        self.added
            .as_ref()
            .filter(|(said, _)| *said == self.text)
            .and_then(|(_, at)| ways_in::hint_wait(at.elapsed()))
    }

    /// What the footer says, the open wheel's hint aside. `plain` is the
    /// count; `files` how many are listed, `running` whether work is under
    /// way, `pane` whether the side pane is up, `selected` how many rows
    /// are selected, and `space_cancels` whether Space would cancel the
    /// selected file (asked only when it matters).
    pub(super) fn footer_text(
        &self,
        plain: &str,
        files: usize,
        running: bool,
        pane: bool,
        selected: usize,
        space_cancels: impl FnOnce() -> bool,
    ) -> String {
        let added = self
            .added
            .as_ref()
            .filter(|(said, _)| *said == self.text)
            .map(|(_, at)| at.elapsed());
        // Once the status has nothing more to say, a warning that stands
        // comes first, then the hint on what to do next.
        let status = match ways_in::idle_hint(files, running, &self.text, plain, added, pane) {
            Some(hint) => self.sticky.unwrap_or(hint),
            None => model::footer_status(&self.text, plain, self.sticky),
        };
        // While the work under way is what it says, it says how to cancel
        // a selected file that is converting, as the selection is now: only
        // while Space does it. The selection is looked through only then.
        let status = ways_in::with_cancel_hint(status, self.working.as_deref(), space_cancels);
        // While several rows are selected, their count stands over all of
        // that, unless something is running or went wrong.
        let count = model::selected_line(selected);
        ways_in::footer_line(&status, count.as_deref(), running, self.alerting()).to_owned()
    }

    /// The empty window has no footer: what the status says, past the
    /// plain "0 files.", sits under its Add files button.
    pub(super) fn empty_text(&self, plain: &str) -> String {
        match self.text.as_str() {
            s if s == plain => String::new(),
            s => s.to_owned(),
        }
    }

    /// The sets made so far, each with whether it was an alert, taken for
    /// the announcements to be made once the message is done.
    pub(super) fn take_sets(&mut self) -> Vec<(String, bool)> {
        let alert = self.alert.clone();
        self.sets
            .drain(..)
            .map(|t| {
                let alert = alert.as_deref() == Some(t.as_str());
                (t, alert)
            })
            .collect()
    }

    /// The sets made before the first client came, dropped unannounced.
    pub(super) fn forget_sets(&mut self) {
        let sets = std::mem::take(&mut self.sets);
        self.teller
            .tell(sets.into_iter().map(|t| (t, false)), false);
    }
}
