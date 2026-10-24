//! The window's own motion, as plain data: when each moment began and
//! where each eased value is heading. How each looks is worked out from
//! that when a frame is drawn (`anim`). The window reads the clock and
//! Windows' Animation effects setting and passes them in, so the tests
//! drive time directly. The wheel, the sheets and a drag keep their own
//! times.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use kiln_core::JobId;

use crate::anim::{self, elapsed_ms, Tween};
use crate::inspector as insp;
use crate::layout::{self, Control, DialogButton, SheetControl, ROW_HEIGHT};
use crate::model::{self, FocusButton, JobState};

use super::render::RowAnim;
use super::FACTS_MS;

/// A sheet's way in, `since` it opened: how far it has faded in, and how
/// far it still has to rise (`anim::rise`).
pub(super) fn sheet_rise(since: Instant, now: Instant, animations: bool) -> (f32, f32) {
    anim::rise(
        elapsed_ms(since, now),
        anim::SHEET_MS,
        anim::SHEET_RISE,
        animations,
    )
}

/// A row's times, which its motion is worked out from, and the job it
/// waits on.
pub(super) struct RowTimes {
    added: Instant,
    /// Its place in the batch it was added with: it pours in after the
    /// rows before it (`anim::pour`).
    pub(super) order: usize,
    /// When the row's last job finished well: its glaze cools from then.
    settled: Option<Instant>,
    /// When the row's last job failed: it cracks from then.
    cracked: Option<Instant>,
    /// The job the row is waiting on. Only it may change the row: a job
    /// stopped or replaced can still report late.
    pub(super) job: Option<JobId>,
}

impl RowTimes {
    fn new(added: Instant, order: usize) -> Self {
        RowTimes {
            added,
            order,
            settled: None,
            cracked: None,
            job: None,
        }
    }

    /// How long the row takes to pour in, its wait included.
    fn pour_ms(&self) -> u32 {
        let wait = (self.order as u64 * anim::POUR_STAGGER_MS as u64)
            .min(anim::POUR_STAGGER_CAP_MS as u64) as u32;
        wait + anim::POUR_MS
    }

    /// Whether the row is still pouring in, cooling or cracking at `now`.
    fn moving_at(&self, now: Instant, animations: bool) -> bool {
        let within =
            |t: Option<Instant>, ms: u32| t.is_some_and(|t| anim::within(t, ms, now, animations));
        within(Some(self.added), self.pour_ms())
            || within(self.settled, anim::COOL_MS)
            || within(self.cracked, anim::CRACK_MS)
    }
}

/// A button whose fill eases its hover and press over `anim::PRESS_MS`:
/// the window's own (the header's, the footer's and the inspector's), or
/// one on a sheet or dialog over it. Steppers, segments, tabs and links
/// keep their own feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ButtonId {
    Window(FocusButton),
    /// The options sheet's Cancel or Convert (`ButtonId::sheet`).
    Sheet(SheetControl),
    /// A button in Settings (`ButtonId::settings`).
    Settings(Control),
    /// Help's Close.
    Help,
    /// The details dialog's Copy or Close.
    Details(DialogButton),
}

impl ButtonId {
    /// The options sheet's control `c`, if it is a button.
    pub(crate) fn sheet(c: SheetControl) -> Option<Self> {
        matches!(c, SheetControl::Cancel | SheetControl::Convert).then_some(ButtonId::Sheet(c))
    }

    /// Settings' control `c`, if it is a button: not a tab, a segment, a
    /// stepper or a link.
    pub(crate) fn settings(c: Control) -> Option<Self> {
        use Control as C;
        matches!(
            c,
            C::Close
                | C::CheckUpdates
                | C::OutputChoose
                | C::OutputReset
                | C::EngineChoose(_)
                | C::EngineReset(_)
                | C::ForgetPicks
                | C::Key(_)
        )
        .then_some(ButtonId::Settings(c))
    }

    /// Whether the pointer lights it as the one it is on, among others like
    /// it: anything but the header's and footer's buttons, which each say
    /// for themselves.
    fn pointed(self) -> bool {
        !matches!(self, ButtonId::Window(b) if !matches!(b, FocusButton::Inspector(_)))
    }
}

/// What an inspector page shows, by the files themselves rather than by
/// where their rows sit: which of its contents it is (`Inspector`'s
/// variant), and the files it is about: the one file, or every selected
/// file, in list order. A row added or removed above the selection moves
/// its rows, not what it shows.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Shows {
    what: std::mem::Discriminant<insp::Inspector>,
    files: Vec<PathBuf>,
}

impl Shows {
    /// What `model` shows, built for the selection whose files are `files`.
    pub(super) fn of<'a>(
        model: &insp::Inspector,
        files: impl IntoIterator<Item = &'a Path>,
    ) -> Self {
        Shows {
            what: std::mem::discriminant(model),
            files: files.into_iter().map(Path::to_path_buf).collect(),
        }
    }

    /// Whether `rows`, which a page of it draws by their place in the list
    /// (`Panel::pictured`: the one file, or the first of several), still
    /// hold its files, `path_at` saying which file each row holds now.
    /// Once rows come or go above them they hold other files, and the page
    /// would show those.
    pub(super) fn still_at<'a>(
        &self,
        rows: &[usize],
        path_at: impl Fn(usize) -> Option<&'a Path>,
    ) -> bool {
        rows.iter()
            .zip(&self.files)
            .all(|(&i, f)| path_at(i) == Some(f.as_path()))
    }
}

/// Whether the inspector going from showing `before` to showing `now` is
/// new content, which cross-fades: other files, or another of its
/// contents for the same file (Waiting to Converting, say). The same
/// file's content changing in place, such as its facts arriving, its
/// figures ticking over or its result being saved, is laid out again
/// where it is, and so is a row moving in the list.
pub(super) fn new_content(before: &Shows, now: &Shows) -> bool {
    before != now
}

/// Whether the inspector as `built` (what it said, for the files it
/// showed) is already the page for `now`, so there is nothing to build:
/// the same words for the same files. The same words for other files,
/// such as the next file taking a removed one's row and saying just what
/// it said, is another page.
pub(super) fn same_page(
    built: (&insp::Inspector, &Shows),
    now: (&insp::Inspector, &Shows),
) -> bool {
    built == now
}

/// Which page fades out as new content rises in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Fading {
    /// The page that was fading out already, which still shows more than
    /// the page that was rising in.
    Before,
    /// The page that was showing, or rising in.
    Now,
}

/// The window's own motion: each row's times, the selection's glide, the
/// inspector's cross-fade and fold, the newest picture's fade and the
/// buttons' hover and press. It keeps only when each moment began and
/// where each eased value is heading.
pub(super) struct Motion {
    /// Each listed row's times, in list order.
    pub(super) rows: Vec<RowTimes>,
    /// Where the one selected row's wash and edge are drawn, in DIPs from
    /// the top of the list before scrolling. It glides when one row's
    /// selection moves to another nearby.
    selection_y: Tween,
    /// The one selected row as last seen, and how many rows there were
    /// then. `None` while several rows, or none, are selected.
    pub(super) glided: Option<(usize, usize)>,
    /// When the inspector's content last changed: the new content rises
    /// in from then (`anim::CROSSFADE_MS`).
    inspector_since: Option<Instant>,
    /// How much of the inspector is shown, from 0 folded away to 1. It
    /// follows the window's width (`layout::inspector_fits`).
    inspector_shown: Tween,
    /// The fold as read for the message being handled, once for all of
    /// it, or as the last message left it (`hold_fold`).
    held: f32,
    /// How many messages are being handled, one inside another: a message
    /// sent from inside another's handling is handled within it.
    depth: u32,
    /// When the newest picture in view arrived, so frames run while it
    /// fades in.
    pub(super) preview_shown: Option<Instant>,
    /// Each button's hover, 0 to 1. A button is added the first time it
    /// is hovered, never on a frame.
    button_hover: HashMap<ButtonId, Tween>,
    /// The same for a press.
    button_press: HashMap<ButtonId, Tween>,
}

impl Motion {
    /// Nothing moving, with the inspector `shown` (0 or 1).
    pub(super) fn new(now: Instant, shown: f32) -> Self {
        Motion {
            rows: vec![],
            selection_y: Tween::new(0.0, now),
            glided: None,
            inspector_since: None,
            inspector_shown: Tween::new(shown, now),
            held: shown,
            depth: 0,
            preview_shown: None,
            button_hover: HashMap::new(),
            button_press: HashMap::new(),
        }
    }

    /// `count` rows were added at row `at`, as one batch: each pours in
    /// after the ones before it.
    pub(super) fn rows_added(&mut self, at: usize, count: usize, now: Instant) {
        let at = at.min(self.rows.len());
        self.rows
            .splice(at..at, (0..count).map(|order| RowTimes::new(now, order)));
    }

    /// Row `i`'s job started: it has nothing left to cool or crack.
    pub(super) fn row_started(&mut self, i: usize) {
        if let Some(r) = self.rows.get_mut(i) {
            r.settled = None;
            r.cracked = None;
        }
    }

    /// Row `i`'s job finished well: it flashes in its glaze and cools.
    pub(super) fn row_finished(&mut self, i: usize, now: Instant) {
        if let Some(r) = self.rows.get_mut(i) {
            r.settled = Some(now);
            r.cracked = None;
        }
    }

    /// Row `i`'s job failed: it cracks.
    pub(super) fn row_failed(&mut self, i: usize, now: Instant) {
        if let Some(r) = self.rows.get_mut(i) {
            r.settled = None;
            r.cracked = Some(now);
        }
    }

    /// Whether anything here still moves at `now`, so frames are needed.
    /// With Animation effects off, nothing does.
    pub(super) fn moving_at(&self, now: Instant, animations: bool) -> bool {
        let within =
            |t: Option<Instant>, ms: u32| t.is_some_and(|t| anim::within(t, ms, now, animations));
        self.rows.iter().any(|r| r.moving_at(now, animations))
            || self.selection_y.running(now)
            || self.inspector_shown.running(now)
            || within(self.inspector_since, anim::CROSSFADE_MS)
            || within(self.preview_shown, anim::PREVIEW_MS)
            || self
                .button_hover
                .values()
                .chain(self.button_press.values())
                .any(|t| t.running(now))
    }

    /// Fold the inspector away, or bring it back, for a window `w` zoomed
    /// DIPs wide. It slides over `anim::FOLD_MS` while files are `listed`;
    /// with none there is no inspector to watch, and it is simply there.
    /// A window with no width, minimised, has nothing to follow: the fold
    /// stays as it is, so the window comes back as it went. Returns
    /// whether a slide began.
    pub(super) fn fold_to(&mut self, w: f32, listed: bool, now: Instant, animations: bool) -> bool {
        if w <= 0.0 {
            return false;
        }
        let to = if layout::inspector_fits(w) { 1.0 } else { 0.0 };
        if self.inspector_shown.target() == to {
            return false;
        }
        let slide = animations && listed;
        self.inspector_shown.set(to, anim::FOLD_MS, now, slide);
        slide
    }

    /// How much of the inspector shows at `now`, 0 to 1.
    pub(super) fn shown(&self, now: Instant) -> f32 {
        self.inspector_shown.value(now).clamp(0.0, 1.0)
    }

    /// A message begins at `now`: the fold is read once for all of it
    /// (`held_fold`), so everything the message works out sees the
    /// inspector at one place. Returns the fold the message found, which
    /// it gives to `release_fold` when it ends.
    pub(super) fn hold_fold(&mut self, now: Instant) -> f32 {
        self.depth += 1;
        let shown = self.shown(now);
        std::mem::replace(&mut self.held, shown)
    }

    /// The message that found `outer` ends at `now`. One handled inside
    /// another gives the outer message its own fold back. The outermost
    /// leaves the fold as it is now, for what runs outside any message:
    /// the window's start, and code resuming after a modal loop entered
    /// from there.
    pub(super) fn release_fold(&mut self, outer: f32, now: Instant) {
        self.depth = self.depth.saturating_sub(1);
        self.held = if self.depth > 0 {
            outer
        } else {
            self.shown(now)
        };
    }

    /// How much of the inspector shows for the message being handled, or
    /// as the last message left it: see `hold_fold`.
    pub(super) fn held_fold(&self) -> f32 {
        self.held
    }

    /// How much of the inspector shows once the fold has got where it is
    /// going: 0 or 1.
    pub(super) fn fold_target(&self) -> f32 {
        self.inspector_shown.target()
    }

    /// Whether the fold is still sliding at `now`.
    pub(super) fn folding(&self, now: Instant) -> bool {
        self.inspector_shown.running(now)
    }

    /// Whether the inspector is on its way to showing, or shows: where the
    /// fold is heading, whatever it shows yet.
    pub(super) fn inspector_staying(&self) -> bool {
        self.fold_target() > 0.5
    }

    /// The inspector's content's way in at `now`, since it last changed:
    /// how far it has faded in, and how far it still has to rise.
    pub(super) fn inspector_rise(&self, now: Instant, animations: bool) -> (f32, f32) {
        self.inspector_since.map_or((1.0, 0.0), |t| {
            anim::rise(
                elapsed_ms(t, now),
                anim::CROSSFADE_MS,
                anim::CROSSFADE_RISE,
                animations,
            )
        })
    }

    /// How far the page the inspector showed before still shows at `now`,
    /// having begun to fade out `from` that far in when the content last
    /// changed: it fades over the first half of the cross-fade. `None` once
    /// it has gone.
    pub(super) fn inspector_fading(&self, from: f32, now: Instant) -> Option<f32> {
        let t = self
            .inspector_since
            .map_or(1.0, |t| elapsed_ms(t, now) / anim::CROSSFADE_MS as f32);
        let a = from * (1.0 - 2.0 * t);
        (a > 0.0).then_some(a)
    }

    /// New content arrived at `now`, and rises in from here. Of the page
    /// already fading out, which still shows `fading`, and the page that
    /// was rising in, the more visible one fades out from here, from as
    /// far in as it shows, and the other goes: which, and how far in. With
    /// Animation effects off nothing fades (`None`): the new content is
    /// simply there.
    pub(super) fn content_changed(
        &mut self,
        fading: Option<f32>,
        now: Instant,
        animations: bool,
    ) -> Option<(Fading, f32)> {
        let (rising, _) = self.inspector_rise(now, animations);
        self.inspector_since = animations.then_some(now);
        if !animations {
            return None;
        }
        Some(match fading {
            Some(a) if a > rising => (Fading::Before, a),
            _ => (Fading::Now, rising),
        })
    }

    /// The selection is now `seen`: one row and how many rows there are, or
    /// `None` for several or none. From one row to another no more than
    /// `reach` rows away, with no rows come or gone, its wash glides there;
    /// any other way it is there at once. Returns whether a glide began.
    pub(super) fn glide(
        &mut self,
        seen: Option<(usize, usize)>,
        reach: usize,
        now: Instant,
        animations: bool,
    ) -> bool {
        let before = std::mem::replace(&mut self.glided, seen);
        let Some((row, rows)) = seen else {
            return false;
        };
        let near = matches!(before, Some((was, n)) if n == rows && was != row && was.abs_diff(row) <= reach);
        let glides = near && animations;
        let y = row as f32 * ROW_HEIGHT;
        if glides {
            self.selection_y.set(y, anim::GLIDE_MS, now, true);
        } else {
            self.selection_y = Tween::new(y, now);
        }
        glides
    }

    /// Where the one selected row's wash is drawn at `now`.
    pub(super) fn selection_at(&self, now: Instant) -> f32 {
        self.selection_y.value(now)
    }

    /// Ease button `b` toward being `hover`ed and `press`ed, over
    /// `anim::PRESS_MS`. A button not yet kept is added only when it is
    /// hovered or pressed. Returns whether either began to move.
    pub(super) fn ease_button(
        &mut self,
        b: ButtonId,
        hover: bool,
        press: bool,
        now: Instant,
        animations: bool,
    ) -> bool {
        let h = ease_toward(&mut self.button_hover, b, hover, now, animations);
        let p = ease_toward(&mut self.button_press, b, press, now, animations);
        h || p
    }

    /// Ease the buttons the pointer lights one at a time (the inspector's,
    /// and those on a sheet or dialog): the one it is on, `hover`, and the
    /// one it `press`ed, toward lit, and every other back.
    pub(super) fn ease_pointed(
        &mut self,
        hover: Option<ButtonId>,
        press: Option<ButtonId>,
        now: Instant,
        animations: bool,
    ) -> bool {
        let mut began = false;
        for (map, on) in [
            (&mut self.button_hover, hover),
            (&mut self.button_press, press),
        ] {
            for (b, t) in map.iter_mut().filter(|(b, _)| b.pointed()) {
                began |= ease_one(t, on == Some(*b), now, animations);
            }
            if let Some(b) = on {
                began |= ease_toward(map, b, true, now, animations);
            }
        }
        began
    }

    /// Button `b`'s hover and press at `now`, each 0 to 1.
    pub(super) fn button(&self, b: ButtonId, now: Instant) -> (f32, f32) {
        let at = |map: &HashMap<ButtonId, Tween>| {
            map.get(&b).map_or(0.0, |t| t.value(now).clamp(0.0, 1.0))
        };
        (at(&self.button_hover), at(&self.button_press))
    }

    /// Animation effects went off: every eased value is at its end now.
    pub(super) fn settle(&mut self, now: Instant) {
        let rest = |t: &mut Tween| *t = Tween::new(t.target(), now);
        rest(&mut self.selection_y);
        rest(&mut self.inspector_shown);
        self.button_hover.values_mut().for_each(rest);
        self.button_press.values_mut().for_each(rest);
        self.inspector_since = None;
        self.preview_shown = None;
    }

    /// Row `i`'s motion at `now`, with `e` its entry, for drawing.
    pub(super) fn row_anim(
        &self,
        i: usize,
        e: Option<&model::Entry>,
        now: Instant,
        animations: bool,
    ) -> RowAnim {
        let facts = e
            .and_then(|e| e.facts_at)
            .map_or(1.0, |t| anim::eased(t, FACTS_MS, now, animations));
        let Some(r) = self.rows.get(i) else {
            return RowAnim::at_rest(facts);
        };
        let (appear, rise) = anim::pour(elapsed_ms(r.added, now), r.order, animations);
        // A failed row's sentence turns to the error colour as it cracks; a
        // finished row's result cools into place.
        let settle = match (r.cracked, r.settled) {
            (Some(t), _) => anim::eased(t, anim::CRACK_MS, now, animations),
            (None, Some(t)) => anim::eased(t, anim::COOL_MS, now, animations),
            (None, None) => 1.0,
        };
        let done = e.is_some_and(|e| matches!(e.job, JobState::Done { .. }));
        let cool = r
            .settled
            .filter(|_| done)
            .map_or(0.0, |t| anim::cool(elapsed_ms(t, now), animations));
        let shake = r
            .cracked
            .map_or(0.0, |t| anim::crack(elapsed_ms(t, now), animations));
        RowAnim {
            appear,
            rise,
            settle,
            cool,
            shake,
            facts,
        }
    }
}

/// Ease `t` toward 1 when `on`, else toward 0. Returns whether it began
/// to move.
fn ease_one(t: &mut Tween, on: bool, now: Instant, animations: bool) -> bool {
    let to = if on { 1.0 } else { 0.0 };
    if t.target() == to {
        return false;
    }
    t.set(to, anim::PRESS_MS, now, animations);
    animations
}

/// Ease `map`'s value for `b` toward `on`, adding it, at rest at 0, only
/// when it isn't kept yet and is turning on.
fn ease_toward(
    map: &mut HashMap<ButtonId, Tween>,
    b: ButtonId,
    on: bool,
    now: Instant,
    animations: bool,
) -> bool {
    match map.get_mut(&b) {
        Some(t) => ease_one(t, on, now, animations),
        None if on => {
            let mut t = Tween::new(0.0, now);
            let began = ease_one(&mut t, true, now, animations);
            map.insert(b, t);
            began
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::inspector::{InspectorControl, Kept};

    fn ms(t: Instant, ms: u32) -> Instant {
        t + Duration::from_millis(ms as u64)
    }

    #[test]
    fn moving_stops_when_every_motion_has_ended() {
        let t = Instant::now();
        let mut m = Motion::new(t, 1.0);
        // Thirty rows added at once: the last of them waits out the
        // stagger's cap before it pours.
        m.rows_added(0, 30, t);
        assert!(m.moving_at(ms(t, 100), true));
        let end = anim::POUR_STAGGER_CAP_MS + anim::POUR_MS;
        assert!(m.moving_at(ms(t, end - 1), true));
        assert!(!m.moving_at(ms(t, end), true));
        // With Animation effects off nothing moves, not even at once.
        assert!(!m.moving_at(t, false));
    }

    #[test]
    fn the_fold_follows_the_width() {
        let t = Instant::now();
        let mut m = Motion::new(t, 1.0);
        assert!(m.fold_to(700.0, true, t, true));
        assert_eq!(m.fold_target(), 0.0);
        let mid = m.shown(ms(t, 100));
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert_eq!(m.shown(ms(t, anim::FOLD_MS)), 0.0);
        assert!(m.moving_at(ms(t, anim::FOLD_MS - 1), true));
        assert!(!m.moving_at(ms(t, anim::FOLD_MS), true));
        // Wide again, it slides back in; the same width again is no change.
        let t2 = ms(t, 1000);
        assert!(m.fold_to(1000.0, true, t2, true));
        assert!(!m.fold_to(1000.0, true, t2, true));
        assert_eq!(m.shown(ms(t2, anim::FOLD_MS)), 1.0);
        // With no files there is no inspector to watch, and with Animation
        // effects off nothing slides: either way it is there at once.
        assert!(!m.fold_to(700.0, false, t2, true));
        assert_eq!(m.shown(t2), 0.0);
        assert!(!m.fold_to(1000.0, true, t2, false));
        assert_eq!(m.shown(t2), 1.0);
    }

    #[test]
    fn minimising_leaves_the_fold_as_it_was() {
        let t = Instant::now();
        let mut m = Motion::new(t, 1.0);
        // Minimised, the window has no width: nothing folds, so coming
        // back shows the inspector as it was, with nothing sliding in.
        assert!(!m.fold_to(0.0, true, t, true));
        assert_eq!(m.fold_target(), 1.0);
        assert_eq!(m.shown(ms(t, anim::FOLD_MS)), 1.0);
        assert!(!m.moving_at(t, true));
        assert!(!m.fold_to(1300.0, true, t, true));
        // Folded away, the same: it stays folded.
        assert!(!m.fold_to(700.0, true, t, false));
        assert_eq!(m.fold_target(), 0.0);
        assert!(!m.fold_to(0.0, true, t, true));
        assert_eq!(m.fold_target(), 0.0);
    }

    #[test]
    fn the_fold_is_read_once_a_message_and_kept_between_them() {
        let t = Instant::now();
        let mut m = Motion::new(t, 1.0);
        assert_eq!(m.held_fold(), 1.0);
        assert!(m.fold_to(700.0, true, t, true));
        // A message partway through the slide reads the fold once...
        let outer = m.hold_fold(ms(t, 60));
        let held = m.held_fold();
        assert!(held > 0.0 && held < 1.0, "{held}");
        // ...and one sent from inside it reads its own, then gives the
        // outer message its own back.
        let inner = m.hold_fold(ms(t, 120));
        assert!(m.held_fold() < held);
        m.release_fold(inner, ms(t, 120));
        assert_eq!(m.held_fold(), held);
        // Once the outermost ends, the fold is as it is then, for what runs
        // outside any message: not as it was before that message.
        m.release_fold(outer, ms(t, anim::FOLD_MS));
        assert_eq!(m.held_fold(), 0.0);
    }

    #[test]
    fn a_finished_row_cools_and_a_failed_one_cracks() {
        let t = Instant::now();
        let mut m = Motion::new(t, 1.0);
        m.rows_added(0, 2, t.checked_sub(Duration::from_secs(5)).unwrap_or(t));
        assert!(!m.moving_at(t, true));
        m.row_failed(1, t);
        assert!(m.moving_at(ms(t, anim::CRACK_MS - 1), true));
        assert!(!m.moving_at(ms(t, anim::CRACK_MS), true));
        m.row_finished(0, t);
        assert!(m.moving_at(ms(t, anim::COOL_MS - 1), true));
        assert!(!m.moving_at(ms(t, anim::COOL_MS), true));
        assert!(!m.moving_at(t, false));
        // A row that starts again has nothing left to cool or crack.
        m.row_started(0);
        m.row_started(1);
        assert!(!m.moving_at(t, true));
    }

    #[test]
    fn the_selection_glides_only_from_one_row_to_one_nearby() {
        let t = Instant::now();
        let y = |row: usize| row as f32 * ROW_HEIGHT;
        let mut m = Motion::new(t, 1.0);
        // The first selection is drawn where it is.
        assert!(!m.glide(Some((2, 40)), 8, t, true));
        assert_eq!(m.selection_y.value(t), y(2));
        // One row to another within the list in view glides.
        assert!(m.glide(Some((6, 40)), 8, t, true));
        assert_eq!(m.selection_y.value(t), y(2));
        assert_eq!(m.selection_y.value(ms(t, anim::GLIDE_MS)), y(6));
        assert!(m.moving_at(ms(t, anim::GLIDE_MS - 1), true));
        // Farther than the list shows, it is there at once.
        let t2 = ms(t, 1000);
        assert!(!m.glide(Some((30, 40)), 8, t2, true));
        assert_eq!(m.selection_y.value(t2), y(30));
        // From several rows, or once rows came or went, at once too.
        assert!(!m.glide(None, 8, t2, true));
        assert!(!m.glide(Some((31, 40)), 8, t2, true));
        assert_eq!(m.selection_y.value(t2), y(31));
        assert!(!m.glide(Some((32, 41)), 8, t2, true));
        assert_eq!(m.selection_y.value(t2), y(32));
        // And with Animation effects off.
        assert!(!m.glide(Some((33, 41)), 8, t2, false));
        assert_eq!(m.selection_y.value(t2), y(33));
    }

    #[test]
    fn a_button_eases_its_hover_and_press_both_ways() {
        let t = Instant::now();
        let clear = ButtonId::Window(FocusButton::Clear);
        let mut m = Motion::new(t, 1.0);
        // Nothing hovered: nothing kept, nothing moving.
        assert!(!m.ease_button(clear, false, false, t, true));
        assert_eq!(m.button(clear, t), (0.0, 0.0));
        assert!(m.ease_button(clear, true, false, t, true));
        assert!(m.moving_at(ms(t, anim::PRESS_MS - 1), true));
        assert_eq!(m.button(clear, ms(t, anim::PRESS_MS)), (1.0, 0.0));
        // The same again changes nothing.
        let t2 = ms(t, 500);
        assert!(!m.ease_button(clear, true, false, t2, true));
        assert!(m.ease_button(clear, true, true, t2, true));
        assert_eq!(m.button(clear, ms(t2, anim::PRESS_MS)), (1.0, 1.0));
        // Let go, both ease back.
        let t3 = ms(t, 1000);
        assert!(m.ease_button(clear, false, false, t3, true));
        assert_eq!(m.button(clear, ms(t3, anim::PRESS_MS)), (0.0, 0.0));
        // Animation effects off: there at once, and nothing to wait for.
        let t4 = ms(t, 2000);
        let convert = ButtonId::Window(FocusButton::Convert);
        assert!(!m.ease_button(convert, true, false, t4, false));
        assert_eq!(m.button(convert, t4), (1.0, 0.0));
        assert!(!m.moving_at(t4, true));
    }

    #[test]
    fn the_button_under_the_pointer_lights_and_the_rest_let_go() {
        let t = Instant::now();
        let target = |k| ButtonId::Window(FocusButton::Inspector(InspectorControl::Target(k)));
        let cancel = ButtonId::sheet(SheetControl::Cancel).unwrap();
        let convert = ButtonId::sheet(SheetControl::Convert).unwrap();
        let mut m = Motion::new(t, 1.0);
        // The inspector's controls: the one under the pointer lights.
        assert!(m.ease_pointed(Some(target(0)), None, t, true));
        assert!(m.ease_pointed(Some(target(1)), None, t, true));
        let at = ms(t, anim::PRESS_MS);
        assert_eq!(m.button(target(0), at).0, 0.0);
        assert_eq!(m.button(target(1), at).0, 1.0);
        // A sheet's buttons ease the same way, and what the pointer left
        // lets go.
        let t2 = ms(t, 500);
        assert!(m.ease_pointed(Some(convert), Some(convert), t2, true));
        let at = ms(t2, anim::PRESS_MS);
        assert_eq!(m.button(convert, at), (1.0, 1.0));
        assert_eq!(m.button(target(1), at), (0.0, 0.0));
        assert_eq!(m.button(cancel, at), (0.0, 0.0));
        // Help's Close and the details dialog's buttons too.
        let t3 = ms(t, 1000);
        assert!(m.ease_pointed(Some(ButtonId::Help), None, t3, true));
        let copy = ButtonId::Details(DialogButton::Copy);
        assert!(m.ease_pointed(Some(copy), Some(copy), ms(t3, 500), true));
        let at = ms(t3, 500 + anim::PRESS_MS);
        assert_eq!(m.button(copy, at), (1.0, 1.0));
        assert_eq!(m.button(ButtonId::Help, at), (0.0, 0.0));
        // The header's and footer's buttons say for themselves: the
        // pointer leaving the others leaves them be.
        let clear = ButtonId::Window(FocusButton::Clear);
        let t4 = ms(t, 3000);
        assert!(m.ease_button(clear, true, false, t4, true));
        assert!(m.ease_pointed(None, None, t4, true));
        assert_eq!(m.button(clear, ms(t4, anim::PRESS_MS)).0, 1.0);
        assert!(!m.moving_at(ms(t4, anim::PRESS_MS), true));
    }

    #[test]
    fn only_buttons_ease_on_a_sheet_or_in_settings() {
        use crate::settings::{Engine, ThemeChoice};
        assert!(ButtonId::sheet(SheetControl::Cancel).is_some());
        assert!(ButtonId::sheet(SheetControl::Step(0)).is_none());
        for c in [
            Control::Close,
            Control::CheckUpdates,
            Control::OutputChoose,
            Control::OutputReset,
            Control::EngineChoose(Engine::ALL[0]),
            Control::EngineReset(Engine::ALL[0]),
            Control::ForgetPicks,
        ] {
            assert_eq!(ButtonId::settings(c), Some(ButtonId::Settings(c)), "{c:?}");
        }
        for c in [
            Control::Theme(ThemeChoice::default()),
            Control::Suggestions(true),
            Control::AutoSave(false),
            Control::EngineGet(Engine::ALL[0]),
        ] {
            assert_eq!(ButtonId::settings(c), None, "{c:?}");
        }
    }

    fn waiting(row: usize, facts: Vec<(&'static str, String)>) -> insp::Inspector {
        insp::Inspector::Waiting {
            row,
            what: None,
            facts,
            targets: vec![],
            more: false,
            helper: None,
        }
    }

    fn shows<const N: usize>(model: &insp::Inspector, files: [&str; N]) -> Shows {
        Shows::of(model, files.iter().map(Path::new))
    }

    #[test]
    fn content_is_new_by_what_it_shows_not_where_its_rows_are() {
        use insp::Inspector as I;
        // A row removed or put back above the one selected moves it, and
        // that is no new content.
        assert!(!new_content(
            &shows(&waiting(3, vec![]), ["b.png"]),
            &shows(&waiting(2, vec![]), ["b.png"])
        ));
        // Nor is the same file's content changing in place: its facts
        // arriving, its figures ticking over, its result being saved.
        let facts = vec![("Pixels", "640 × 480".to_owned())];
        assert!(!new_content(
            &shows(&waiting(2, vec![]), ["b.png"]),
            &shows(&waiting(2, facts), ["b.png"])
        ));
        let converting = |percent: Option<u32>, time: &str| I::Converting {
            row: 2,
            percent,
            time: time.to_owned(),
        };
        assert!(!new_content(
            &shows(&converting(None, "Converting to WEBP"), ["b.png"]),
            &shows(&converting(Some(11), "about 1 min"), ["b.png"])
        ));
        let done = |kept| I::Done {
            row: 2,
            result: "b.webp".to_owned(),
            change: None,
            kept,
        };
        assert!(!new_content(
            &shows(&done(Kept::Held), ["b.png"]),
            &shows(&done(Kept::InFolder), ["b.png"])
        ));
        // Another file is new content, and so is the same file's next state.
        assert!(new_content(
            &shows(&waiting(2, vec![]), ["b.png"]),
            &shows(&waiting(2, vec![]), ["c.png"])
        ));
        assert!(new_content(
            &shows(&waiting(2, vec![]), ["b.png"]),
            &shows(&converting(None, "x"), ["b.png"])
        ));
        assert!(new_content(
            &shows(&I::Nothing, []),
            &shows(&waiting(0, vec![]), ["a.png"])
        ));
        assert!(!new_content(
            &shows(&I::Nothing, []),
            &shows(&I::Nothing, [])
        ));
        // Several files: the selection is what it shows.
        let several = |kinds: &str| I::Several {
            rows: vec![0, 1],
            kinds: kinds.to_owned(),
            targets: vec![],
            none_shared: None,
            offered: vec![],
        };
        assert!(!new_content(
            &shows(&several("2 files"), ["a.png", "b.png"]),
            &shows(&several("2 images"), ["a.png", "b.png"])
        ));
        assert!(new_content(
            &shows(&several("2 images"), ["a.png", "b.png"]),
            &shows(&several("3 images"), ["a.png", "b.png", "c.png"])
        ));
    }

    #[test]
    fn the_same_words_for_another_file_are_another_page() {
        // The next file takes a removed one's row and says just what it
        // said: the page must still be built for it, or it would keep the
        // removed file's paths.
        let model = waiting(2, vec![]);
        let was = shows(&model, ["b.png"]);
        let next = shows(&model, ["c.png"]);
        assert!(!same_page((&model, &was), (&model, &next)));
        assert!(same_page((&model, &was), (&model, &was.clone())));
        let facts = waiting(2, vec![("Pixels", "640 × 480".to_owned())]);
        assert!(!same_page((&model, &was), (&facts, &was)));
    }

    #[test]
    fn a_page_whose_rows_moved_no_longer_shows_its_files() {
        let list = |names: &[&str]| -> Vec<PathBuf> { names.iter().map(PathBuf::from).collect() };
        let page = shows(&waiting(2, vec![]), ["c.png"]);
        let before = list(&["a.png", "b.png", "c.png"]);
        assert!(page.still_at(&[2], |i| before.get(i).map(PathBuf::as_path)));
        // "a.png" removed: row 2 is gone, and row 1 holds "c.png" now.
        let after = list(&["b.png", "c.png"]);
        assert!(!page.still_at(&[2], |i| after.get(i).map(PathBuf::as_path)));
        // A row put back above it: row 2 holds another file.
        let undone = list(&["a.png", "x.png", "b.png", "c.png"]);
        assert!(!page.still_at(&[2], |i| undone.get(i).map(PathBuf::as_path)));
        // Rows added after it leave it be.
        let added = list(&["a.png", "b.png", "c.png", "d.png"]);
        assert!(page.still_at(&[2], |i| added.get(i).map(PathBuf::as_path)));
    }

    #[test]
    fn a_third_content_keeps_the_more_visible_page_fading() {
        let t = Instant::now();
        let mut m = Motion::new(t, 1.0);
        // The first change: the page shown fades out from fully in.
        assert_eq!(m.content_changed(None, t, true), Some((Fading::Now, 1.0)));
        // Soon after, the page fading out still shows more than the new
        // one has risen in: it is the one that carries on fading, from
        // where it is, and the other goes.
        let t1 = ms(t, 20);
        let fading = m.inspector_fading(1.0, t1).unwrap();
        let (rising, _) = m.inspector_rise(t1, true);
        assert!(fading > rising, "{fading} {rising}");
        assert_eq!(
            m.content_changed(Some(fading), t1, true),
            Some((Fading::Before, fading))
        );
        assert_eq!(m.inspector_fading(fading, t1), Some(fading));
        assert_eq!(m.inspector_rise(t1, true).0, 0.0);
        // Later, the page rising in shows more: it fades out instead.
        let t2 = ms(t1, 60);
        let fading = m.inspector_fading(fading, t2).unwrap();
        let (rising, _) = m.inspector_rise(t2, true);
        assert!(rising > fading, "{fading} {rising}");
        assert_eq!(
            m.content_changed(Some(fading), t2, true),
            Some((Fading::Now, rising))
        );
        // The page fading out has gone by half the cross-fade.
        let half = anim::CROSSFADE_MS / 2;
        assert!(m.inspector_fading(rising, ms(t2, half - 1)).is_some());
        assert_eq!(m.inspector_fading(rising, ms(t2, half)), None);
        // With Animation effects off nothing fades: the new content is
        // there at once.
        assert_eq!(m.content_changed(Some(1.0), t2, false), None);
        assert_eq!(m.inspector_rise(t2, false), (1.0, 0.0));
        assert_eq!(m.inspector_fading(1.0, t2), None);
    }
}
