//! Kiln's own prompts and dialogs open in the window: the prompt being
//! asked and those waiting for it, and the Make smaller and Resize dialog
//! with the estimate it waits for. `App` holds one `Prompts` and the
//! methods here.

use super::*;

/// What is open in the window for the person to answer.
pub(super) struct Prompts {
    /// One of Kiln's prompts, open in the window (`Effect::Ask`).
    pub(super) ask_ui: Option<surfaces::Ask>,
    /// The Make smaller or Resize dialog, while it is open.
    pub(super) tool_ui: Option<surfaces::ToolUi>,
    /// The worker working out its estimate: stopped when another is asked
    /// or the dialog closes.
    pub(super) estimating: Option<CancelToken>,
    /// The thread that works out the Make smaller dialog's estimate,
    /// started when first needed.
    pub(super) estimator: Option<workers::Estimator>,
    /// Which tool dialog is open, counting up as each opens: an estimate
    /// for one closed since is dropped.
    pub(super) tool_dialog: u64,
    /// Prompts asked while one was open, each counted as open: they are
    /// asked in turn once it is answered.
    pub(super) asks_waiting: VecDeque<(
        model::Prompt,
        effects::Answer<PromptButton>,
        system::InWindow,
    )>,
}

impl App {
    /// Ask `prompt`, once `App` is let go; `then` gets the button chosen,
    /// with a fresh borrow. It opens inside the window (`open_ask`).
    pub(super) fn ask(
        &mut self,
        prompt: model::Prompt,
        then: impl FnOnce(&mut App, PromptButton) + 'static,
    ) {
        self.effect(Effect::Ask {
            prompt,
            then: Box::new(then),
        });
    }

    /// Whether a prompt can open inside the window: it draws. After the
    /// graphics device was lost again and again, a task dialog asks
    /// instead (`system::ask`).
    pub(super) fn asks_inside(&self) -> bool {
        #[cfg(test)]
        if self.draw_given_up_for_test {
            return false;
        }
        self.renderer.can_draw()
    }

    /// The window stopped drawing while a prompt was open in it, or
    /// waiting: no one may ever see it there. It and those waiting are
    /// asked again, in order, as `Effect::Ask`, which a window that can't
    /// draw hands to Windows' task dialog. Each stays counted as open
    /// from the queue on. Run after every message.
    pub(super) fn ask_elsewhere_if_undrawn(&mut self) {
        if self.prompts.ask_ui.is_none() || self.asks_inside() || self.ending() {
            return;
        }
        let Some(open) = self.prompts.ask_ui.take() else {
            return;
        };
        let (prompt, then) = open.into_parts();
        if let Some(then) = then {
            self.effect(Effect::Ask { prompt, then });
        }
        for (prompt, then, _counted) in std::mem::take(&mut self.prompts.asks_waiting) {
            self.effect(Effect::Ask { prompt, then });
        }
    }

    /// Open `prompt` inside the window, as `Effect::Ask` asked. One opens
    /// at a time: while another is open it waits, counted as open, and is
    /// asked once that one is answered. A window hidden in the
    /// notification area, or minimised, comes forward with it. Once the window has gone
    /// or the session is ending, nothing opens and `then` never runs:
    /// what is held stays.
    pub(super) fn open_ask(&mut self, prompt: model::Prompt, then: effects::Answer<PromptButton>) {
        if self.ending() {
            return;
        }
        if self.prompts.ask_ui.is_some() {
            self.prompts
                .asks_waiting
                .push_back((prompt, then, system::InWindow::open()));
            return;
        }
        if self.out_of_sight() {
            self.open_window();
        }
        // Nothing beneath a prompt acts: the right-click menu, which has
        // no loop of its own to hold the keys, gives way to it.
        self.close_wheel();
        self.close_menu();
        self.quiet_beneath();
        self.prompts.ask_ui = Some(surfaces::Ask::new(prompt, then));
        self.invalidate();
    }

    /// The open prompt was answered `button`: it closes, and is no longer
    /// counted, before what asked it hears the answer; then the prompts
    /// that waited are asked in turn.
    pub(super) fn answer_ask(&mut self, button: PromptButton) {
        let Some(mut open) = self.prompts.ask_ui.take() else {
            return;
        };
        let then = open.take_then();
        drop(open);
        let waiting = std::mem::take(&mut self.prompts.asks_waiting);
        if let Some(then) = then {
            then(self, button);
        }
        for (prompt, then, _counted) in waiting {
            self.effect(Effect::Ask { prompt, then });
        }
        self.invalidate();
    }

    /// The window is going or the session is ending: an open prompt and
    /// those waiting go unanswered, so nothing they would do is done.
    pub(super) fn drop_asks(&mut self) {
        self.prompts.ask_ui = None;
        self.prompts.asks_waiting.clear();
    }

    // Make smaller and Resize.

    /// Open `tool`'s dialog for `paths`, files of `kind`, its job to be
    /// started as `via` says. The rows' facts give each file's size and
    /// picture. `false` when the tool has no dialog or the files aren't
    /// ones it takes.
    pub(super) fn open_tool(
        &mut self,
        tool: Tool,
        kind: kiln_core::Kind,
        paths: Vec<PathBuf>,
        via: Via,
    ) -> bool {
        let files: Vec<crate::smaller::File> = paths
            .iter()
            .filter_map(|p| self.files.entries().get(self.files.index_of(p)?))
            .map(|e| {
                let facts = e.facts.as_ref();
                let ext = self
                    .kiln
                    .registry()
                    .format_of(&e.path)
                    .map_or_else(|| e.code.to_ascii_lowercase(), |f| f.ext.clone());
                crate::smaller::File {
                    path: e.path.clone(),
                    name: e.name.clone(),
                    ext,
                    bytes: e.size,
                    picture: facts
                        .and_then(|f| f.picture.as_ref())
                        .map(|p| (p.width, p.height)),
                    fps: facts.and_then(|f| f.media.as_ref()).and_then(|m| m.fps),
                }
            })
            .collect();
        let Some(model) = crate::smaller::ToolDialog::new(tool, files) else {
            return false;
        };
        let saving = crate::smaller::Saving::of(self.prefs.settings.save_to.is_some());
        // A job started is a choice made: nothing more is asked for what
        // arrived.
        self.incoming.arrivals.clear();
        self.close_wheel();
        self.close_menu();
        self.prompts.tool_dialog += 1;
        self.prompts.tool_ui = Some(surfaces::ToolUi::new(model, kind, via, saving));
        self.ask_estimate();
        self.animate();
        self.invalidate();
        true
    }

    /// The Make smaller or Resize dialog goes, and its worker stops.
    pub(super) fn close_tool(&mut self) {
        self.prompts.tool_ui = None;
        if let Some(c) = self.prompts.estimating.take() {
            c.cancel();
        }
        self.invalidate();
    }

    /// Ask a worker for the estimate the open dialog wants now, stopping
    /// the one asked before.
    pub(super) fn ask_estimate(&mut self) {
        let Some((path, opts)) = self
            .prompts
            .tool_ui
            .as_mut()
            .and_then(|t| t.take_estimate_wanted())
        else {
            return;
        };
        if let Some(c) = self.prompts.estimating.take() {
            c.cancel();
        }
        let cancel = CancelToken::new();
        self.prompts.estimating = Some(cancel.clone());
        let options = Options {
            tool: opts.clone(),
            ..self.prefs.settings.options()
        };
        let (kiln, wake) = (self.kiln.clone(), self.wake.clone());
        let estimator = self.prompts.estimator.get_or_insert_with(|| {
            workers::Estimator::start_with(move |a| {
                if let Some(e) = estimate_of(&kiln, a) {
                    wake.send(Event::Estimated(a.dialog, a.opts.clone(), e));
                }
            })
        });
        let ask = workers::EstimateAsk {
            dialog: self.prompts.tool_dialog,
            path,
            options,
            opts,
            cancel,
        };
        if let Err(a) = estimator.ask(ask) {
            // No thread for it: the dialog says the size asked for.
            log("no thread for the estimate; the dialog says the size asked for");
            let e = crate::smaller::Estimate::Plain;
            self.drive_tool(|t, cx| t.estimated(cx, a.opts, e));
        }
    }
}
