//! Files coming in: dropped and pasted paths and the walks that read them,
//! files that arrive by Open with, Send to, a second launch or Add files,
//! what waits while a panel or dialog is up, and the format choice a drop
//! or an arrival opens. `App` holds one `Incoming` and the methods here.

use super::*;

/// What is coming into the list, and what waits to come.
pub(super) struct Incoming {
    /// Walks of dropped and pasted paths under way, oldest first.
    pub(super) walks: Vec<Walk>,
    pub(super) next_walk: u64,
    /// New rows added before the engines' first answer, after the wait
    /// for it was given up: whether one lacks a helper app is told once
    /// it comes (`helper_check`).
    pub(super) helper_pending: Vec<PathBuf>,
    /// Files arriving by Open with, Send to, a second launch or Add files,
    /// until they stop.
    pub(super) arrivals: ways_in::Arrivals,
    /// The files in the current drag, read when it entered the window.
    pub(super) drag_paths: Vec<PathBuf>,
    /// Files dropped or sent from Explorer while a panel or dialog was up,
    /// added once the list is in view again.
    pub(super) queued: Vec<PathBuf>,
    /// The folder files were last added from in this run, where Add files
    /// opens.
    pub(super) last_add: Option<PathBuf>,
    /// Set while Kiln runs a drag of its own results, shared with the drop
    /// target, which then ignores it.
    pub(super) own_drag: Rc<Cell<bool>>,
}

/// How an add ended, for what waited on it (`App::after_add_with`).
#[derive(Debug, Clone, Default)]
pub(super) struct AddEnd {
    /// Its big-folder prompt was cancelled, or the list was cleared
    /// under it: nothing more of it was added.
    pub(super) cancelled: bool,
    /// The rows it added, in order.
    pub(super) fresh: Vec<PathBuf>,
}

/// What runs once an add is over, told how it ended.
pub(super) type AfterAdd = Box<dyn FnOnce(&mut App, &AddEnd)>;

/// A walk's batch, for `Event::Walked`: `id` names the walk.
pub(super) struct Walked {
    pub(super) id: u64,
    pub(super) batch: model::WalkBatch,
}

/// Whether a walk's files may join the list as they come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WalkAsk {
    /// Not yet known: the folders haven't been listed.
    Unknown,
    /// Few enough files: nothing to ask.
    No,
    /// The big-folder prompt is open; nothing is added meanwhile.
    Open,
    /// Add all was chosen.
    AddAll,
}

/// How many of a walk's files join the list in one go; the rest follow
/// after the window's other messages.
pub(super) const WALK_ADD_AT_ONCE: usize = 500;

/// A walk of dropped or pasted paths (`model::walk_dropped`): its own
/// thread reads the folders and files, and the window adds the rows from
/// what it sends, touching no disk.
pub(super) struct Walk {
    pub(super) id: u64,
    pub(super) cancel: CancelToken,
    /// A key asked for the files: the new focus row shows its ring.
    pub(super) keys: bool,
    pub(super) ask: WalkAsk,
    /// Read and not yet added.
    pub(super) files: Vec<model::ReadFile>,
    /// What the walk found besides, for the footer once it's over; its
    /// `files` stays empty.
    pub(super) found: model::Gathered,
    /// Files found so far, loose or from folders.
    pub(super) total: usize,
    pub(super) done: bool,
    /// A `WalkOn` is on its way for it.
    pub(super) soon: bool,
    /// What adding its files has done so far.
    pub(super) added: model::Added,
    /// The list's paths, kept from one share to the next.
    pub(super) listed: model::ListedPaths,
    /// Its new rows, in order.
    pub(super) fresh: Vec<PathBuf>,
    /// What runs once it's over, added or cancelled (`App::after_add`).
    pub(super) then: Vec<AfterAdd>,
}

impl Drop for Walk {
    /// A walk that goes, finished or not, stops its thread.
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl App {
    /// Add `paths` to the list. `keys` says a key asked for them (Ctrl+O,
    /// Ctrl+V), which shows the focus ring on the new focus row.
    ///
    /// Folders give their files, read on a thread of their own
    /// (`model::walk_dropped`), and the rows join the list as the walk
    /// sends them (`walked`). A big folder asks first, while the walk goes
    /// on, and nothing joins the list until Add all.
    pub(super) fn add(&mut self, paths: Vec<PathBuf>, keys: bool) {
        // What the InfoBar at the top said was about the files before.
        self.set_top_info(None);
        let id = self.incoming.next_walk;
        self.incoming.next_walk += 1;
        let cancel = CancelToken::new();
        self.incoming.walks.push(Walk {
            id,
            cancel: cancel.clone(),
            keys,
            ask: WalkAsk::Unknown,
            files: vec![],
            found: model::Gathered::default(),
            total: 0,
            done: false,
            soon: false,
            added: model::Added::default(),
            listed: model::ListedPaths::default(),
            fresh: vec![],
            then: vec![],
        });
        let wake = self.wake.clone();
        let spawned = model::walk_dropped(
            paths.clone(),
            self.kiln.registry().clone(),
            cancel.clone(),
            move |batch| {
                wake.send(Event::Walked(Walked { id, batch }));
            },
        );
        if let Err(e) = spawned {
            // No thread for it: the walk is made here rather than lost.
            log(&format!("couldn't start a folder walk: {e}"));
            let mut batches = vec![];
            model::walk_here(paths, self.kiln.registry(), &cancel, &mut |b| {
                batches.push(b)
            });
            for batch in batches {
                self.walked(Walked { id, batch });
            }
        }
    }

    /// Run `f` once the latest add is over, its rows added or its prompt
    /// cancelled; with no add under way, as `afterwards` does.
    pub(super) fn after_add(&mut self, f: impl FnOnce(&mut App) + 'static) {
        self.after_add_with(move |app, _| f(app));
    }

    /// As `after_add`, with `f` told how the add ended: what it added and
    /// whether it was cancelled. With no add under way, nothing was added.
    pub(super) fn after_add_with(&mut self, f: impl FnOnce(&mut App, &AddEnd) + 'static) {
        match self.incoming.walks.last_mut() {
            Some(w) => w.then.push(Box::new(f)),
            None => self.afterwards(move |app| f(app, &AddEnd::default())),
        }
    }

    /// A walk's batch has come: its files join the list unless a big
    /// folder's prompt is waiting, and the prompt is asked as soon as the
    /// folders are listed. A batch from a walk that was cancelled is
    /// dropped.
    pub(super) fn walked(&mut self, Walked { id, batch }: Walked) {
        let Some(w) = self.incoming.walks.iter_mut().find(|w| w.id == id) else {
            return;
        };
        let model::WalkBatch {
            mut part,
            found,
            listed,
            done,
            ..
        } = batch;
        w.files.append(&mut part.files);
        w.found.absorb(part);
        w.total = found;
        w.done |= done;
        if w.ask == WalkAsk::Unknown {
            if listed > model::FOLDER_ASK_ABOVE {
                w.ask = WalkAsk::Open;
                let prompt = model::big_folder_prompt(listed);
                self.ask(prompt, move |app, button| {
                    app.walk_answered(id, button == PromptButton::AddAll)
                });
            } else {
                w.ask = WalkAsk::No;
            }
        }
        self.walk_soon(id);
    }

    /// Have walk `id` add its next share in a message of its own, once
    /// the window's other messages are answered: one share a message, so
    /// a big folder never holds the window up.
    pub(super) fn walk_soon(&mut self, id: u64) {
        let Some(w) = self.incoming.walks.iter_mut().find(|w| w.id == id) else {
            return;
        };
        if !w.soon {
            w.soon = true;
            self.wake.send(Event::WalkOn(id));
        }
    }

    /// The big-folder prompt is answered: Add all adds what the walk has
    /// read and what it reads from now on; Cancel stops it, and nothing
    /// from that drop is added.
    pub(super) fn walk_answered(&mut self, id: u64, add_all: bool) {
        let Some(i) = self.incoming.walks.iter().position(|w| w.id == id) else {
            return;
        };
        if add_all {
            self.incoming.walks[i].ask = WalkAsk::AddAll;
            return self.walk_soon(id);
        }
        let w = self.incoming.walks.remove(i);
        w.cancel.cancel();
        self.say("Nothing added.");
        self.invalidate();
        self.walk_over(w, true);
    }

    /// Add what walk `id` has read, while nothing waits to be asked, a
    /// share at a time; once it's all added and the walk is over, say
    /// what the add did.
    pub(super) fn walk_on(&mut self, id: u64) {
        let Some(i) = self.incoming.walks.iter().position(|w| w.id == id) else {
            return;
        };
        // Rows wait for the engines, so they show what they offer from
        // their first paint; and while a panel or a dialog is up, as
        // other adds do (`adds_wait`). `resume_walks` goes on with them.
        let hold = !self.engines.ready() || self.adds_wait();
        let w = &mut self.incoming.walks[i];
        w.soon = false;
        if hold || !matches!(w.ask, WalkAsk::No | WalkAsk::AddAll) {
            return;
        }
        let share: Vec<model::ReadFile> = if w.files.len() > WALK_ADD_AT_ONCE {
            w.files.drain(..WALK_ADD_AT_ONCE).collect()
        } else {
            std::mem::take(&mut w.files)
        };
        let more = !w.files.is_empty();
        let keys = w.keys;
        if !share.is_empty() {
            let available = self.available().clone();
            let mut listed = std::mem::take(&mut self.incoming.walks[i].listed);
            let r = self
                .files
                .add_read_with(&mut listed, &self.kiln, &available, share);
            self.incoming.walks[i].listed = listed;
            // The new rows pour in on the end of the list, one after
            // another.
            let poured = self.files.len().saturating_sub(self.motion.rows.len());
            self.motion
                .rows_added(self.motion.rows.len(), poured, Instant::now());
            let fresh: Vec<PathBuf> = self.files.entries()[self.files.len() - r.added..]
                .iter()
                .map(|e| e.path.clone())
                .collect();
            let w = &mut self.incoming.walks[i];
            let first = w.added.added == 0 && r.added > 0;
            w.added.added += r.added;
            w.added.duplicates += r.duplicates;
            w.fresh.extend(fresh.iter().cloned());
            self.inspect(fresh);
            if first {
                // The list shows where the new files start, with that row
                // selected, and the keyboard is on it: Enter converts.
                self.button_focus = None;
                self.files.added(self.files.len() - r.added, keys);
                self.scroll_to_selection();
            } else if r.added > 0 {
                // Later shares of the walk join the selection: every file
                // the walk adds is selected (UX-05).
                self.files.also_added(self.files.len() - r.added);
            }
            self.animate();
        }
        if more {
            // The rest after the window's other messages.
            return self.walk_soon(id);
        }
        if self.incoming.walks[i].done {
            let w = self.incoming.walks.remove(i);
            self.walk_added(w);
        }
    }

    /// Go on with the walks that held their rows back (`walk_on`), once
    /// nothing makes them wait. Run after every message.
    pub(super) fn resume_walks(&mut self) {
        if !self.engines.ready() || self.adds_wait() {
            return;
        }
        let held: Vec<u64> = self
            .incoming
            .walks
            .iter()
            .filter(|w| {
                !w.soon
                    && matches!(w.ask, WalkAsk::No | WalkAsk::AddAll)
                    && (!w.files.is_empty() || w.done)
            })
            .map(|w| w.id)
            .collect();
        for id in held {
            self.walk_soon(id);
        }
    }

    /// Stop every walk, adding nothing more: the list was cleared. What
    /// waited on each runs, as after a Cancel.
    pub(super) fn stop_walks(&mut self) {
        for w in std::mem::take(&mut self.incoming.walks) {
            w.cancel.cancel();
            self.walk_over(w, true);
        }
    }

    /// A walk is over and its files are in the list: the footer says what
    /// the add did, as one line for the whole drop.
    pub(super) fn walk_added(&mut self, w: Walk) {
        let found = &w.found;
        let from = model::source_of(&found.folders, found.from_folders, w.total);
        let r = w.added;
        let skipped_count = found.unopened + found.skipped.len();
        let mut said = model::drop_line(
            self.files.len(),
            r.added,
            r.duplicates,
            from.as_deref(),
            &found.skipped,
        );
        if found.unopened > 0 {
            said = format!("{said} {} didn't open.", found.unopened);
        }
        self.say(said);
        // An add that added nothing says why, whatever is selected.
        if r.added == 0 {
            self.status.alert = Some(self.status.text.clone());
        }
        // Only the plain "Added 2 files." gives way to the idle hint; a line
        // about duplicates or skipped items stays until something replaces
        // it.
        self.status.added = ways_in::hint_follows(r.added, r.duplicates, skipped_count)
            .then(|| (self.status.text.clone(), Instant::now()));
        if self.status.added.is_some() {
            self.arm_hint(ways_in::ADDED_FOR);
        }
        if skipped_count == 0 {
            if self.engines.known() {
                self.helper_check(&w.fresh);
            } else {
                // Rows made after the wait was given up: whether one lacks
                // a helper app is told once the engines are really known.
                self.incoming.helper_pending.extend(w.fresh.iter().cloned());
            }
        }
        if let Some(last) = w.fresh.last() {
            self.incoming.last_add = last.parent().map(Path::to_path_buf);
        }
        self.animate();
        self.walk_over(w, false);
    }

    /// What waited on walk `w` runs, now it's over; `cancelled` says it
    /// was stopped before it added everything.
    pub(super) fn walk_over(&mut self, mut w: Walk, cancelled: bool) {
        let end = AddEnd {
            cancelled,
            fresh: std::mem::take(&mut w.fresh),
        };
        for f in std::mem::take(&mut w.then) {
            let end = end.clone();
            self.afterwards(move |app| f(app, &end));
        }
    }

    /// One calm notice per run that a helper app is missing, for the
    /// first of the new rows `fresh` that can't be converted for want of
    /// it; the rows carry the rest.
    pub(super) fn helper_check(&mut self, fresh: &[PathBuf]) {
        if self.helper_notice {
            return;
        }
        let lacking = fresh.iter().find_map(|p| {
            let e = self.files.entries().get(self.files.index_of(p)?)?;
            e.missing
                .as_ref()
                .filter(|_| e.targets.is_empty())
                .map(|m| m.backend.clone())
        });
        if let Some(backend) = lacking {
            self.helper_notice = true;
            self.alert(model::helper_notice(&backend));
        }
    }

    /// Whether files that arrive now wait to be added: a panel or one of
    /// Kiln's dialogs is up, and a row added beneath it would go unseen or
    /// change the list a dialog was asked about; or, at start, the look
    /// for what crashed runs left hasn't answered, and its prompt may be
    /// about to open.
    pub(super) fn adds_wait(&self) -> bool {
        system::modal_open() || self.over() == Over::Panel || self.work.scanning
    }

    /// Add `paths` now, or once the list is in view again.
    pub(super) fn add_when_live(&mut self, paths: Vec<PathBuf>) {
        if self.adds_wait() {
            self.incoming.queued.extend(paths);
        } else {
            self.add(paths, false);
        }
    }

    pub(super) fn dropped(&mut self, ev: &DragEvent) {
        self.drag.set(
            0.0,
            theme::fluent::motion::QUICK_MS,
            Instant::now(),
            theme::fluent::motion::animations(),
        );
        // A drop acts on the list, so the focus goes back to it.
        self.button_focus = None;
        if self.adds_wait() {
            self.incoming.queued.extend(ev.paths.iter().cloned());
            self.incoming.drag_paths.clear();
            self.invalidate();
            return;
        }
        // A drop opens the wheel where it landed.
        let at = self.screen_dips(ev.x, ev.y);
        self.set_top_info(None);
        if self.wheel.as_ref().is_some_and(|w| w.is_open()) {
            // A wheel open for other files never stays over a plain drop:
            // a pick on it would convert those files, not the drop's.
            self.close_wheel();
        }
        let dropped = ev.paths.clone();
        self.add(ev.paths.clone(), false);
        // Once the add is over: its folders walked, and a big folder's
        // prompt, if one asked, answered.
        self.after_add_with(move |app, end| app.drop_added(at, &dropped, end));
    }

    /// A drop on the window, once its files are added: it selects the
    /// drop's own rows, those it added and those of `dropped` already
    /// listed, and opens the format wheel at `at` for them. An add that was
    /// cancelled or met nothing it could list opens nothing, whatever is
    /// selected.
    pub(super) fn drop_added(&mut self, at: (f32, f32), dropped: &[PathBuf], end: &AddEnd) {
        if !end.cancelled {
            let rows = self.rows_of_drop(dropped, &end.fresh);
            if !rows.is_empty() {
                self.files.select_only(&rows);
                let paths = self.paths_of(&rows);
                self.choice_for_arrivals(paths, at, Pour::AtPointer);
                self.invalidate();
            }
        }
        self.incoming.drag_paths.clear();
    }

    /// The rows a drop of `dropped` stands for once its add is over: the
    /// rows it added, `fresh`, and the rows already listed for a dropped
    /// file, or for a file straight inside a dropped folder (a walk reads
    /// no deeper).
    pub(super) fn rows_of_drop(&self, dropped: &[PathBuf], fresh: &[PathBuf]) -> Vec<usize> {
        let fresh: HashSet<&Path> = fresh.iter().map(PathBuf::as_path).collect();
        let dropped: HashSet<&Path> = dropped.iter().map(PathBuf::as_path).collect();
        self.files
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                fresh.contains(e.path.as_path())
                    || dropped.contains(e.path.as_path())
                    || e.path.parent().is_some_and(|p| dropped.contains(p))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// `paths` arrived by Open with, Send to, a second launch or Add files:
    /// once they stop arriving, the format choice opens for them.
    pub(super) fn arrive(&mut self, paths: &[PathBuf]) {
        let now = Instant::now();
        self.set_top_info(None);
        self.incoming.arrivals.arrived(paths, now);
        self.arm_arrivals(now);
    }

    /// Wake when the files arriving are due, with a little slack.
    pub(super) fn arm_arrivals(&self, now: Instant) {
        if let Some(wait) = self.incoming.arrivals.wait(now) {
            let ms = (wait + HINT_SLACK).as_millis().min(u32::MAX as u128) as u32;
            // SAFETY: a timer on this thread's own window.
            unsafe {
                SetTimer(Some(self.hwnd), ARRIVALS_TIMER, ms, None);
            }
        }
    }

    /// The files that arrived have stopped arriving: once they are all
    /// listed and nothing is over the list, they are selected and the
    /// format choice opens centred over the list, by the rule a plain
    /// drop follows. Until then it waits.
    pub(super) fn arrivals_due(&mut self) {
        let now = Instant::now();
        // Nothing waits: taken, or acted on another way first.
        if self.incoming.arrivals.wait(now).is_none() {
            // SAFETY: a timer on this thread's own window.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), ARRIVALS_TIMER);
            }
            return;
        }
        let busy =
            self.adds_wait() || !self.incoming.walks.is_empty() || !self.incoming.queued.is_empty();
        if busy || !self.over().list_live() {
            // Look again once whatever is in the way has gone.
            if self.incoming.arrivals.wait(now).is_some() {
                // SAFETY: a timer on this thread's own window.
                unsafe {
                    SetTimer(Some(self.hwnd), ARRIVALS_TIMER, ARRIVALS_RETRY_MS, None);
                }
            }
            return;
        }
        let Some(arrived) = self.incoming.arrivals.due(now) else {
            return self.arm_arrivals(now);
        };
        // SAFETY: a timer on this thread's own window.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), ARRIVALS_TIMER);
        }
        // The rows they made: each file, and the files of each folder.
        let rows: Vec<usize> = self
            .files
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                arrived
                    .iter()
                    .any(|a| e.path == *a || e.path.starts_with(a))
            })
            .map(|(i, _)| i)
            .collect();
        if rows.is_empty() {
            return;
        }
        self.files.select_only(&rows);
        let paths = self.paths_of(&rows);
        let l = self.layout();
        let at = (l.list.x + l.list.w / 2.0, l.list.y + l.list.h / 2.0);
        self.choice_for_arrivals(paths, at, Pour::OverList);
        self.invalidate();
    }

    /// Where the rows of `paths` stand: each one's job, `None` for a file
    /// not listed.
    pub(super) fn jobs_of(&self, paths: &[PathBuf]) -> Vec<Option<JobState>> {
        paths
            .iter()
            .map(|p| {
                let i = self.files.index_of(p)?;
                self.files.entries().get(i).map(|e| e.job.clone())
            })
            .collect()
    }

    /// Files that just arrived, `paths`, selected: the format wheel opens
    /// for them, poured as `pour` says from `at`, when they are of one
    /// kind and it has something for them (spec 2.1). Kinds that share no
    /// format are told so in an InfoBar at the top of the list.
    /// Anything else is just listed.
    pub(super) fn choice_for_arrivals(&mut self, paths: Vec<PathBuf>, at: (f32, f32), pour: Pour) {
        if paths.is_empty() || self.wheel.as_ref().is_some_and(|w| w.is_open()) {
            return;
        }
        if !self.engines.ready() {
            // Only for files nothing was done to while the engines were
            // looked for: one converted another way meanwhile asks
            // nothing more.
            let before = self.jobs_of(&paths);
            return self.when_engines_known(move |app| {
                if app.jobs_of(&paths) == before {
                    app.choice_for_arrivals(paths, at, pour);
                }
            });
        }
        let kinds = wheel::kinds_in(&self.kiln, &paths);
        let offer = wheel::offer(WheelKind::Convert, &self.kiln, self.available(), &paths);
        if ways_in::drop_opens_wheel(&offer, kinds) {
            if self.files.convert_busy(&paths).is_none() {
                self.open_wheel(WheelKind::Convert, paths, at, false, pour);
            }
        } else if matches!(offer, wheel::Offer::Mixed(_)) {
            // Kinds that share nothing: said once, at the top of the list.
            // Kinds that share a format are listed, selected, as the side
            // pane offers what they share.
            self.set_top_info(Some(ways_in::mixed_drop_info(paths.len())));
        }
    }
}
