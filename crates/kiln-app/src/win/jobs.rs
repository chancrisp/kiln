//! Jobs and their results: the jobs that run and the ones waiting for a
//! holding folder, the batches a drop or an Options pick makes, how the
//! work went for the finish notice, the taskbar button's progress, and the
//! hand-off of what earlier runs left. `App` holds one `Jobs` and the
//! methods here.

use super::*;

/// What is converting or about to, and how it went.
pub(super) struct Jobs {
    /// Starts jobs on its worker threads and hears how they go.
    pub(super) runner: Runner,
    pub(super) jobs: HashMap<JobId, JobInfo>,
    /// The rows redos from Options are converting, each with its old
    /// result, kept until the redo ends (`job_ended`) or a leave stops it
    /// (`give_back`).
    pub(super) redos: Redos,
    pub(super) batches: HashMap<u64, Batch>,
    /// Drag-wheel drops whose jobs are all done, waiting for their results
    /// to be saved next to their originals before they tell how it went.
    pub(super) waiting: HashMap<u64, Batch>,
    pub(super) next_batch: u64,
    /// Jobs waiting for a worker to open their drive's holding folder.
    pub(super) pending: Vec<PendingLaunch>,
    /// How work since Kiln was last idle went: ready, failed, cancelled,
    /// and the first result's name, for the finish notice.
    pub(super) tally: (usize, usize, usize, Option<String>),
    /// The saves next to the original the finish notice waits for, by
    /// export id, and where those that are in went.
    pub(super) tally_pending: HashSet<u64>,
    pub(super) tally_saves: model::Saves,
    /// The row the finish notice will open, while it is counted.
    pub(super) notice_row: Option<PathBuf>,
    /// Finished results' sizes, for the inspector's before and after. The
    /// runner's thread reads them as each job finishes, so the window
    /// never waits on the disk for them; a result not here has no size to
    /// show yet.
    pub(super) result_sizes: HashMap<PathBuf, u64>,
    /// The worker delivering what crashed, killed or older runs left
    /// (`look_for_stale`), while it runs: a quit cancels it, keeping what
    /// is left, whole, for the next start, and waits for it a moment.
    pub(super) delivering: Option<(CancelToken, std::thread::JoinHandle<()>)>,
    /// What the InfoBar says of the results delivered from last time,
    /// until the window is up and shown (`show_delivered`).
    pub(super) delivered: Option<String>,
    /// An InfoBar the delivery's one went in front of: it shows once that
    /// one goes.
    pub(super) info_after: Option<(String, String)>,
    /// A worker is looking for what crashed or killed runs left
    /// (`look_for_stale`): files that arrive meanwhile wait, for
    /// `STALE_WAIT_MS` at most.
    pub(super) scanning: bool,
    /// The taskbar button's progress, made the first time work starts.
    pub(super) taskbar: Option<system::Taskbar>,
    /// What the taskbar shows now, so a change of under a part in a
    /// thousand isn't sent again.
    pub(super) taskbar_shown: Option<(u8, u32)>,
}

impl App {
    // Jobs.

    /// Start `action` on `paths`. A convert runs at `quality`, or at the
    /// defaults from settings for its kind of file.
    pub(super) fn start(&mut self, action: Action, paths: Vec<PathBuf>, quality: Option<Quality>) {
        self.start_as(action, paths, quality, Via::Window);
    }

    /// `start`, where `via` says whether it came from the drag-wheel: a
    /// drop that took the window's path because its rows held results, or Options picked there. It is told by the window, but its
    /// results are saved next to their originals as any drop's are, and a
    /// drop's rows offer Options once done.
    pub(super) fn start_as(
        &mut self,
        action: Action,
        paths: Vec<PathBuf>,
        quality: Option<Quality>,
        via: Via,
    ) {
        // Make smaller and Resize ask first, in their own dialog: its
        // accent button starts the job (`start_tool`).
        if let Action::Tool(tool @ (Tool::Smaller | Tool::Resize), kind) = action {
            if self.open_tool(tool, kind, paths.clone(), via) {
                return;
            }
        }
        self.start_full(action, paths, quality, via, None);
    }

    /// Start `tool` on `paths` with the options its dialog chose.
    pub(super) fn start_tool(
        &mut self,
        action: Action,
        paths: Vec<PathBuf>,
        tool: ToolOptions,
        via: Via,
    ) {
        self.start_full(action, paths, None, via, Some(tool));
    }

    /// `start_as`, with a tool's options when its dialog chose them.
    pub(super) fn start_full(
        &mut self,
        action: Action,
        paths: Vec<PathBuf>,
        quality: Option<Quality>,
        via: Via,
        tool: Option<ToolOptions>,
    ) {
        // A job started is a choice made: nothing more is asked for the
        // files it takes. What arrived meanwhile and isn't in it still
        // gets its wheel.
        self.incoming.arrivals.forget(&paths);
        // Only files that made it into the list.
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| self.files.index_of(p).is_some())
            .collect();
        // Only the files the job would take are asked about. The notes of
        // those left out wait for the answer (`launch_with`), so a Cancel
        // leaves none behind.
        let convert = self.split(&action, paths.clone()).convert;
        if convert.is_empty() {
            // Nothing to ask about: each row says why it is left out.
            return self.launch_full(action, paths, quality, Origin::Window, via, tool);
        }
        // A new job replaces what a row holds: ask first.
        let rows = Rows::Files(convert);
        self.leave(
            Leave::Remove,
            rows,
            Then::Start(action, paths, quality, via, tool.map(Box::new)),
        );
    }

    /// Start `action` on `paths`, once nothing they hold is at stake, for a
    /// choice made on the wheel `origin` names.
    pub(super) fn launch_as(
        &mut self,
        action: Action,
        paths: Vec<PathBuf>,
        quality: Option<Quality>,
        origin: Origin,
    ) {
        let via = match origin {
            Origin::Drag => Via::Drop,
            Origin::Window => Via::Window,
        };
        self.launch_with(action, paths, quality, origin, via);
    }

    /// `paths` split for `action` (`wheel::split_for`): a conversion leaves
    /// out the files already in its format and those that can't become it
    /// with the engines found. Any other action takes them all.
    pub(super) fn split(&self, action: &Action, paths: Vec<PathBuf>) -> wheel::Split {
        let Action::Convert(ext) = action else {
            return wheel::Split {
                convert: paths,
                ..wheel::Split::default()
            };
        };
        let available = self.engines.known().then(|| self.available());
        let files = &self.files;
        wheel::split_for(&self.kiln, available, &paths, ext, &|p| {
            files.has_subtitles(p)
        })
    }

    /// `split`, with the rows of the files left out saying why ("Already
    /// JPG", "Can't become WEBP", "Needs FFmpeg to become MOV") and left
    /// as they are (UX-06). A row left out for a missing helper gets it
    /// through its link.
    pub(super) fn leave_alone(&mut self, action: &Action, paths: Vec<PathBuf>) -> wheel::Split {
        let split = self.split(action, paths);
        if let Action::Convert(ext) = action {
            for p in &split.already {
                self.notes
                    .insert(p.clone(), (model::already_line(ext), Tone::Muted));
                self.set_waits_on(p, None);
            }
            for p in &split.cant {
                self.notes
                    .insert(p.clone(), (wheel::cant_line(ext), Tone::Muted));
                self.set_waits_on(p, None);
            }
            for (p, helper) in &split.needs {
                self.notes
                    .insert(p.clone(), (wheel::needs_line(ext, helper), Tone::Muted));
                self.set_waits_on(p, Some(helper.clone()));
            }
        }
        split
    }

    /// Say which helper `path`'s row waits on, or that it waits on
    /// none.
    pub(super) fn set_waits_on(&mut self, path: &Path, helper: Option<String>) {
        if let Some(i) = self.files.index_of(path) {
            if self.files.entries()[i].waits_on != helper {
                self.files.entries_mut()[i].waits_on = helper;
            }
        }
    }

    /// `launch_as`, where `via` says whether it came from the drag-wheel,
    /// whichever wheel tells how it went.
    pub(super) fn launch_with(
        &mut self,
        action: Action,
        paths: Vec<PathBuf>,
        quality: Option<Quality>,
        origin: Origin,
        via: Via,
    ) {
        self.launch_full(action, paths, quality, origin, via, None);
    }

    /// `launch_with`, with a tool's options when its dialog chose them.
    pub(super) fn launch_full(
        &mut self,
        action: Action,
        paths: Vec<PathBuf>,
        quality: Option<Quality>,
        origin: Origin,
        via: Via,
        tool: Option<ToolOptions>,
    ) {
        // Nothing more is asked for the files a job takes, whichever way
        // it started (a redo, the drag-wheel).
        self.incoming.arrivals.forget(&paths);
        // An export it waited on may have taken a while.
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| self.files.index_of(p).is_some())
            .collect();
        let split = self.leave_alone(&action, paths);
        // The drag-wheel's window may be hidden, so its notice tells of the
        // rows left out too.
        let left = match &action {
            Action::Convert(ext) if origin == Origin::Drag => wheel::left_out_lines(&split, ext),
            _ => vec![],
        };
        if split.convert.is_empty() {
            if let Some(notice) = left_alone_notice(&split, &left) {
                self.drag_finished(notice, None);
            }
            return;
        }
        self.launch_now(PendingLaunch {
            action,
            paths: split.convert,
            quality,
            origin,
            via,
            left,
            tool,
        });
    }

    /// Start `p`'s jobs now, or once the holding folder they wait on is
    /// open (`held_opened`).
    pub(super) fn launch_now(&mut self, p: PendingLaunch) {
        let PendingLaunch {
            action,
            paths,
            quality,
            origin,
            via,
            left,
            tool,
        } = p;
        // Rows removed while it waited are left out.
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| self.files.index_of(p).is_some())
            .collect();
        if paths.is_empty() {
            return;
        }
        // A later start of the same files replaces one still waiting.
        for p in &mut self.work.pending {
            p.paths.retain(|x| !paths.contains(x));
        }
        self.work.pending.retain(|p| !p.paths.is_empty());
        let jobs = jobs_for(
            &self.kiln,
            &self.prefs.settings,
            &action,
            &paths,
            quality,
            tool.as_ref(),
        );
        // Results wait in the run folder for each job's first file's
        // drive. One not open yet is opened by a worker first, and the
        // jobs start once it is (`held_opened`).
        if let Some(h) = &mut self.held {
            let mut waits = false;
            for (group, _, _) in &jobs {
                let Some(first) = group.first() else {
                    continue;
                };
                if h.folder_now(first).is_none() {
                    h.open_for(first, &self.wake);
                    waits |= h.folder_now(first).is_none();
                }
            }
            if waits {
                // Its rows show as queued, as a job the runner hasn't
                // started yet does, and Cancel takes them off.
                for p in &paths {
                    if let Some(i) = self.files.index_of(p) {
                        start_row(&mut self.files.entries_mut()[i], &action, via == Via::Drop);
                        self.motion.row_started(i);
                    }
                }
                self.say_working(self.working_status());
                self.animate();
                self.invalidate();
                self.work.pending.push(PendingLaunch {
                    action,
                    paths,
                    quality,
                    origin,
                    via,
                    left,
                    tool,
                });
                return;
            }
        }
        for p in &paths {
            self.notes.remove(p);
            self.set_waits_on(p, None);
        }
        let batch = self.work.next_batch;
        self.work.next_batch += 1;
        self.work.batches.insert(
            batch,
            Batch {
                action: action.clone(),
                origin,
                finished: vec![],
                drops: vec![],
                left,
                files: paths.len(),
                ok: 0,
                failed: 0,
                cancelled: 0,
                groups: jobs.len(),
                pending: jobs.len(),
                pending_saves: HashSet::new(),
                saves: model::Saves::default(),
            },
        );
        for (group, kind, mut options) in jobs {
            // Results wait in this run's folder for the first file's drive
            // until they're exported.
            let out_dir = match (&self.held, group.first()) {
                (Some(h), Some(first)) => {
                    Some(h.folder_now(first).unwrap_or(h.local()).to_path_buf())
                }
                _ => None,
            };
            options.out_dir =
                written_into(out_dir.as_deref(), self.prefs.settings.save_to.as_deref());
            let id = self.work.runner.submit(kind, options);
            for p in &group {
                if let Some(i) = self.files.index_of(p) {
                    start_row(&mut self.files.entries_mut()[i], &action, via == Via::Drop);
                    self.motion.row_started(i);
                    if let Some(r) = self.motion.rows.get_mut(i) {
                        r.job = Some(id);
                    }
                }
            }
            self.work.jobs.insert(
                id,
                JobInfo {
                    paths: group,
                    action: action.clone(),
                    batch,
                    drops: vec![],
                    note: None,
                    out_dir,
                },
            );
        }
        self.say_working(self.working_status());
        self.sync_taskbar();
        self.animate();
    }

    pub(super) fn on_job(&mut self, ev: JobEvent) {
        match ev {
            JobEvent::Progress(id, p) => {
                let f = match p {
                    Progress::Fraction(f) => Some(f),
                    Progress::Indeterminate => None,
                    // A note for the job's details changes no bar: it is
                    // kept for the finished line.
                    Progress::Note(note) => {
                        if let Some(job) = self.work.jobs.get_mut(&id) {
                            job.note = Some(note);
                        }
                        return;
                    }
                };
                let Some(job) = self.work.jobs.get(&id) else {
                    return;
                };
                for path in &job.paths {
                    if let Some(i) = self.job_row(path, id) {
                        if let JobState::Running { fraction, .. } =
                            &mut self.files.entries_mut()[i].job
                        {
                            *fraction = f;
                        }
                    }
                }
                self.sync_taskbar();
            }
            // What the job leaves out: kept for the drop's notice, and said
            // on the rows it runs on, after their line.
            JobEvent::Notes(id, drops) => {
                let Some(job) = self.work.jobs.get_mut(&id) else {
                    return;
                };
                job.drops = drops;
                let note = model::job_note(&job.drops, job.note);
                row_noted(&mut self.files, &self.motion, id, &job.paths, note);
            }
            JobEvent::Finished(id, result) => {
                let Some(job) = self.work.jobs.remove(&id) else {
                    return;
                };
                // First, where each held result came from, so a crash from
                // here on still leaves it to be saved there.
                if let (Ok(outputs), Some(dir)) = (&result, &job.out_dir) {
                    let chosen = self.prefs.settings.save_to.as_deref();
                    for note in source_notes(dir, chosen, &job.paths, outputs) {
                        self.held_notes.send(note);
                    }
                }
                // A result may take the name of one a look found removed
                // by hand: no look asked before now speaks for it.
                if let Ok(outputs) = &result {
                    self.gone_seen.held_anew(outputs);
                }
                if let Err(e) = &result {
                    if !e.cancelled {
                        log(&failed_job_line(&job.paths, &job.action, e));
                    }
                }
                let origin = self
                    .work
                    .batches
                    .get(&job.batch)
                    .map_or(Origin::Window, |b| b.origin);
                let now = Instant::now();
                let state = match &result {
                    Ok(outputs) => JobState::Done {
                        action: job.action.clone(),
                        outputs: outputs.clone(),
                        // Without a holding folder, results were written
                        // next to their files: they're already saved.
                        saved: match &job.out_dir {
                            Some(_) => None,
                            None => outputs
                                .first()
                                .and_then(|p| p.parent())
                                .map(|p| model::Saved::Folder(p.to_path_buf())),
                        },
                    },
                    Err(e) if e.cancelled => JobState::Cancelled,
                    Err(e) => JobState::Failed {
                        // One plain sentence in the row; the engine's own
                        // words are in the details.
                        message: no_room(e, job.out_dir.as_deref())
                            .unwrap_or_else(|| e.message.clone()),
                        details: e.details.clone(),
                        engine: e.engine,
                        store_product: e.store_product,
                    },
                };
                // Rows a failed redo gave their old result back: the row
                // no longer says why, so the footer does.
                let mut back_failed = vec![];
                for path in &job.paths {
                    let ended = job_ended(
                        &mut self.files,
                        &mut self.notes,
                        &mut self.motion,
                        &mut self.work.redos,
                        (id, path),
                        (
                            &state,
                            model::job_note(&job.drops, job.note).filter(|_| result.is_ok()),
                        ),
                    );
                    match ended {
                        // Back to its old result: at rest, as it was.
                        Some((i, true)) => {
                            self.motion.row_started(i);
                            if let JobState::Failed { message, .. } = &state {
                                let name = self.files.entries()[i].name.clone();
                                back_failed.push((name, Some(message.clone())));
                            }
                        }
                        Some((i, false)) => self.row_ended(i, now),
                        None => {}
                    }
                }
                // Each finished row's results, all told, for its "31 MB →
                // 9.6 MB" (the sizes came with the news, `on_job_news`).
                // A merge's one result is of several files: no one file's
                // size to set it against.
                if let (Ok(outputs), [_]) = (&result, &job.paths[..]) {
                    let sizes: Option<u64> = outputs
                        .iter()
                        .map(|p| self.work.result_sizes.get(p).copied())
                        .sum();
                    for path in &job.paths {
                        if let Some(i) = self.files.index_of(path) {
                            let e = &mut self.files.entries_mut()[i];
                            if matches!(e.job, JobState::Done { .. }) {
                                e.result_bytes = sizes;
                            }
                        }
                    }
                }
                // A result going next to its original goes now, so the
                // notices can wait for it, and the footer doesn't ask for
                // it to be exported.
                let sent = match &result {
                    Ok(outputs) => self.result_ready(&job, outputs, origin),
                    Err(_) => model::AutoSave::Hold,
                };
                let mut drag_notice = None;
                if let Some(b) = self.work.batches.get_mut(&job.batch) {
                    let failed = match &state {
                        JobState::Failed { message, .. } => Some(Some(message.clone())),
                        JobState::Done { .. } => Some(None),
                        _ => None,
                    };
                    if let (Some(failed), Some(first)) = (failed, job.paths.first()) {
                        // A merge's one result is of all its files.
                        let name = match job.paths.len() {
                            1 => first
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            n => format!("{n} files"),
                        };
                        b.finished.push((first.clone(), name, failed));
                    }
                    match &result {
                        Ok(outputs) => {
                            b.ok += 1;
                            b.drops.push(job.drops.clone());
                            b.saves.held += sent.held(outputs.len());
                        }
                        Err(e) if e.cancelled => b.cancelled += 1,
                        Err(_) => b.failed += 1,
                    }
                    b.pending -= 1;
                    if b.pending == 0 {
                        let summary =
                            batch_summary(&b.action, b.files, b.ok, b.failed, b.cancelled);
                        let failed = b.failed > 0;
                        self.say(summary);
                        if failed {
                            self.status.alert = Some(self.status.text.clone());
                        }
                        drag_notice = self.batch_done(job.batch);
                    } else if !self.alerting() {
                        // Each finish is said once, with how far the batch is.
                        let name = job
                            .paths
                            .first()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let cancelled = matches!(&result, Err(e) if e.cancelled);
                        let said = model::job_outcome(&name, result.is_ok(), cancelled);
                        self.say_working(format!("{said} {}", self.working_status()));
                    }
                }
                // "Couldn't convert photo.png: {reason}", as the drag-wheel
                // says it, where "The row says why." would send the person
                // to a row that has its old result back.
                if let Some((_, line, _)) = model::drag_notice(&job.action, &back_failed) {
                    self.say(line);
                    self.status.alert = Some(self.status.text.clone());
                }
                // A drop on the drag-wheel says how it went itself, once
                // for the whole drop; the finish notice counts only the
                // rest.
                if let Some((notice, saved)) = drag_notice {
                    self.drag_finished(notice, saved);
                }
                self.tally_finish(&job, &result, origin == Origin::Window, sent);
                self.sync_taskbar();
                self.animate();
            }
        }
        self.invalidate();
    }

    /// A result is ready and held. With Save next to the original on, or
    /// for a drop on the drag-wheel (`origin`), it goes beside its file at
    /// once, never over anything there, and the notice that tells of it
    /// waits to say where it went. Kiln asks nothing here, and changes no
    /// setting: only Settings does. Says what became of it: `Beside` when
    /// it was sent there.
    pub(super) fn result_ready(
        &mut self,
        job: &JobInfo,
        outputs: &[PathBuf],
        origin: Origin,
    ) -> model::AutoSave {
        // Its batch's origin says which notice tells.
        match model::autosave_for(job.out_dir.is_some()) {
            model::AutoSave::Hold => model::AutoSave::Hold,
            model::AutoSave::Beside => {
                let feeds = match origin {
                    Origin::Drag => Feeds::Drop(job.batch),
                    Origin::Window => Feeds::Finish,
                };
                if self.save_beside(&job.paths, outputs, feeds) {
                    model::AutoSave::Beside
                } else {
                    model::AutoSave::Hold
                }
            }
        }
    }

    /// Save held `outputs` into the folder chosen in Settings (`save_to`),
    /// or else the folder of the first of `sources`, as an export does:
    /// under a free name, held still if it fails, the row saying it
    /// couldn't be saved there. The notice
    /// `feeds` names waits for it. False when there was nothing to save.
    pub(super) fn save_beside(
        &mut self,
        sources: &[PathBuf],
        outputs: &[PathBuf],
        feeds: Feeds,
    ) -> bool {
        // Only what a row still holds: the list can change meanwhile.
        let held = self.held_of(sources);
        let chosen = self.prefs.settings.save_to.as_deref();
        let Some((files, dir)) = beside(sources, outputs, &held, chosen) else {
            return false;
        };
        if let Some(id) = self.export_for(files, Dest::Into(dir), Some(feeds)) {
            match feeds {
                Feeds::Finish => {
                    self.work.tally_pending.insert(id);
                }
                Feeds::Drop(batch) => {
                    if let Some(b) = self.work.batches.get_mut(&batch) {
                        b.pending_saves.insert(id);
                    }
                }
            }
        }
        true
    }

    /// Where the notice `feeds` names keeps what became of its results
    /// saved next to their originals.
    pub(super) fn saves_of(&mut self, feeds: Feeds) -> Option<&mut model::Saves> {
        match feeds {
            Feeds::Finish => Some(&mut self.work.tally_saves),
            Feeds::Drop(batch) => match self.work.batches.get_mut(&batch) {
                Some(b) => Some(&mut b.saves),
                None => self.work.waiting.get_mut(&batch).map(|b| &mut b.saves),
            },
        }
    }

    /// The last job of batch `id` is in. A drop's notice goes now, unless
    /// results are still on their way next to their originals: then it
    /// waits for them (`save_done`). The notice, and where its results
    /// went when every one it tells of was saved.
    pub(super) fn batch_done(
        &mut self,
        id: u64,
    ) -> Option<((String, String, PathBuf), Option<model::Saves>)> {
        let b = self.work.batches.remove(&id)?;
        if b.origin == Origin::Drag && !b.pending_saves.is_empty() {
            self.work.waiting.insert(id, b);
            return None;
        }
        let saved = b.all_saved().then(|| b.saves.clone());
        b.drag_notice().map(|n| (n, saved))
    }

    /// A save next to the original, export `id`, is in: the notice `feeds`
    /// names goes once its jobs are done and it waits on no other save.
    pub(super) fn save_done(&mut self, feeds: Feeds, id: u64) {
        match feeds {
            Feeds::Finish => {
                self.work.tally_pending.remove(&id);
                self.finish_due();
            }
            Feeds::Drop(batch) => {
                if let Some(b) = self.work.batches.get_mut(&batch) {
                    // Its last job's finish tells.
                    b.pending_saves.remove(&id);
                    return;
                }
                let done = self.work.waiting.get_mut(&batch).is_some_and(|b| {
                    b.pending_saves.remove(&id);
                    b.pending_saves.is_empty()
                });
                if !done {
                    return;
                }
                if let Some(b) = self.work.waiting.remove(&batch) {
                    let saved = b.all_saved().then(|| b.saves.clone());
                    if let Some(notice) = b.drag_notice() {
                        self.drag_finished(notice, saved);
                    }
                }
            }
        }
    }

    /// Show how far the work has got on the taskbar button.
    pub(super) fn sync_taskbar(&mut self) {
        let total: usize = self.work.batches.values().map(|b| b.groups).sum();
        let done: usize = self
            .work
            .batches
            .values()
            .map(|b| b.groups - b.pending)
            .sum();
        let running: Vec<Option<f32>> = self
            .files
            .entries()
            .iter()
            .filter_map(|e| match &e.job {
                JobState::Running { fraction, .. } => Some(*fraction),
                _ => None,
            })
            .collect();
        let now = model::taskbar_progress(total, done, &running);
        let key = match now {
            model::TaskbarProgress::None => (0, 0),
            model::TaskbarProgress::Indeterminate => (1, 0),
            model::TaskbarProgress::Fraction(f) => (2, (f * 1000.0) as u32),
        };
        if self.work.taskbar_shown == Some(key) {
            return;
        }
        self.work.taskbar_shown = Some(key);
        let hwnd = self.hwnd;
        self.work
            .taskbar
            .get_or_insert_with(system::Taskbar::new)
            .set(hwnd, now);
    }

    /// A drop on the drag-wheel has finished all its jobs, and its results
    /// are saved next to their originals or kept: its one notification
    /// says how it went, and a click on it opens Kiln on the row it names
    /// (`Batch::drag_notice`), or with every result saved (`saved` holds
    /// where they went), shows them in their folder.
    pub(super) fn drag_finished(
        &mut self,
        (title, text, row): (String, String, PathBuf),
        saved: Option<model::Saves>,
    ) {
        // Show in folder only where they all went to one folder; otherwise
        // a click opens Kiln on the row.
        let reveal = saved.as_ref().and_then(model::Saves::together);
        let words = match (&saved, &reveal) {
            (Some(s), Some(_)) => self.saved_words(s, &row),
            _ => toast::plain(&title, &text),
        };
        let about = NoticeAbout {
            row: Some(row),
            saved: reveal.is_some(),
            reveal,
        };
        self.notify_about(&title, &text, words, about);
    }

    /// The toast's words for results `saves` says were all saved, the row
    /// `row` holding the first: with one, its sizes as the row has them.
    pub(super) fn saved_words(&self, saves: &model::Saves, row: &Path) -> toast::Words {
        let sizes = match saves.saved.len() {
            1 => self
                .files
                .index_of(row)
                .and_then(|i| self.files.entries().get(i))
                .and_then(|e| Some((e.size, e.result_bytes?))),
            _ => None,
        };
        toast::saved(&saves.saved, &saves.folders, sizes)
    }

    /// Count a finished job towards the finish notice, which goes once the
    /// last one is in (`finish_due`). `count` is false for a job that told
    /// its own news (a drag-wheel drop's): it isn't counted, but its finish
    /// may still be the last. `sent` says whether its results went next to
    /// their originals.
    pub(super) fn tally_finish(
        &mut self,
        job: &JobInfo,
        result: &std::result::Result<Vec<PathBuf>, JobError>,
        count: bool,
        sent: model::AutoSave,
    ) {
        match result {
            _ if !count => {}
            Ok(outputs) => {
                self.work.tally.0 += 1;
                self.work.tally_saves.held += sent.held(outputs.len());
                if self.work.tally.3.is_none() {
                    self.work.tally.3 = outputs
                        .first()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned());
                    self.work.notice_row = job.paths.first().cloned();
                }
            }
            Err(e) if e.cancelled => self.work.tally.2 += 1,
            Err(_) => {
                self.work.tally.1 += 1;
                if self.work.notice_row.is_none() {
                    self.work.notice_row = job.paths.first().cloned();
                }
            }
        }
        self.finish_due();
    }

    /// Once every job is done and every result the finish notice counts is
    /// saved next to its original or kept, tell the person if Kiln isn't in
    /// front: the taskbar button flashes (unless Windows has animations
    /// off) and a notice names the result, or says where it went.
    pub(super) fn finish_due(&mut self) {
        if !self.work.jobs.is_empty() || !self.work.tally_pending.is_empty() {
            return;
        }
        let (ok, failed, cancelled, first) = std::mem::take(&mut self.work.tally);
        let saves = std::mem::take(&mut self.work.tally_saves);
        if ok + failed + cancelled == 0 {
            // Nothing counted: the drag-wheel's own notice stands.
            return;
        }
        if system::in_front(self.hwnd) {
            self.work.notice_row = None;
            return;
        }
        let told = model::finish_notice_saved(ok, failed, cancelled, first.as_deref(), &saves);
        if let Some((title, text, saved)) = told {
            // One it couldn't save opens on that one's row.
            if let Some(row) = saves.opens().filter(|_| failed == 0) {
                self.work.notice_row = Some(row.to_path_buf());
            }
            if theme::fluent::motion::animations() {
                system::flash_until_front(self.hwnd);
            }
            let row = self.work.notice_row.take();
            // Show in folder only where they all went to one folder.
            let reveal = saves.together().filter(|_| saved);
            let words = match (&row, &reveal) {
                (Some(r), Some(_)) => self.saved_words(&saves, r),
                _ => toast::plain(&title, &text),
            };
            let about = NoticeAbout {
                row,
                saved: reveal.is_some(),
                reveal,
            };
            self.notify_about(&title, &text, words, about);
        }
    }

    /// What the footer says while jobs run: how many of them are done.
    /// How to stop the selected one is added as the footer is drawn, from
    /// the selection then (`ways_in::with_cancel_hint`).
    pub(super) fn working_status(&self) -> String {
        let total: usize = self.work.batches.values().map(|b| b.groups).sum();
        let pending: usize = self.work.batches.values().map(|b| b.pending).sum();
        model::batch_progress(total - pending, total)
    }

    /// Say `line` about the work under way, and remember it as that line,
    /// so the footer adds how to cancel the selected file while it still
    /// says it.
    pub(super) fn say_working(&mut self, line: String) {
        self.status.say_working(line);
    }

    /// The row for `path`, if it's waiting on job `id`: a row removed and
    /// added again, or given a new job, isn't the old job's any more.
    pub(super) fn job_row(&self, path: &Path, id: JobId) -> Option<usize> {
        self.files
            .index_of(path)
            .filter(|&i| self.motion.rows.get(i).is_some_and(|r| r.job == Some(id)))
    }

    /// Row `i`'s job has just ended, as its entry now says: a result cools
    /// in its wash, a failure shakes, and a cancelled job just stops.
    pub(super) fn row_ended(&mut self, i: usize, now: Instant) {
        match self.files.entries().get(i).map(|e| &e.job) {
            Some(JobState::Done { .. }) => self.motion.row_finished(i, now),
            Some(JobState::Failed { .. }) => self.motion.row_failed(i, now),
            _ => self.motion.row_started(i),
        }
    }

    // Results crashed, killed or older runs left.

    /// Look for what crashed, killed or older runs left, on every drive,
    /// and deliver every result, on a worker (`held::look_for_stale`),
    /// which answers with `Event::Swept` and then `Event::Delivered`.
    /// Files that arrive meanwhile wait for the look, at most
    /// `STALE_WAIT_MS`, never for the delivery.
    pub(super) fn look_for_stale(&mut self) {
        let Some(places) = self.places.clone() else {
            return;
        };
        let cancel = CancelToken::new();
        let worker = held::look_for_stale(
            self.held.as_ref(),
            places,
            held::volumes(),
            held::kiln_results(),
            cancel.clone(),
            &self.wake,
        );
        self.work.delivering = worker.map(|t| (cancel, t));
        self.work.scanning = true;
        unsafe {
            SetTimer(Some(self.hwnd), STALE_TIMER, STALE_WAIT_MS, None);
        }
    }

    /// The look for what earlier runs left is done: files that waited for
    /// it are added (`after`).
    pub(super) fn swept(&mut self) {
        self.work.scanning = false;
        self.invalidate();
    }

    /// Every result earlier runs left has been tried: the InfoBar says how
    /// many went where, once the window is up and shown.
    pub(super) fn delivered_from_last_time(&mut self, report: kiln_core::Report) {
        self.work.scanning = false;
        if let Some((_, t)) = self.work.delivering.take() {
            let _ = t.join();
        }
        let fallback = held::kiln_results();
        self.work.delivered = model::delivered_info(&report, fallback.as_deref());
        self.invalidate();
    }

    /// `STALE_TIMER`: the look hasn't answered in time. Files go ahead.
    pub(super) fn stale_waited_out(&mut self) {
        if self.work.scanning {
            log("still looking for results from last time; going on meanwhile");
            self.work.scanning = false;
        }
    }

    /// Say what became of last time's results, once: as soon as the window
    /// is up and shown. It goes in front of an InfoBar already showing,
    /// which comes back once it goes. Called after every message.
    pub(super) fn show_delivered(&mut self) {
        if self.work.delivered.is_none() || !self.started || !self.window_shown() {
            return;
        }
        let Some(text) = self.work.delivered.take() else {
            return;
        };
        self.work.info_after = self.top_info.take();
        self.set_top_info(Some((String::new(), text)));
    }
}
