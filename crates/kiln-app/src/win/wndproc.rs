//! The window's procedure and the message loop's turns: how each message is
//! answered with `App` borrowed, what UI Automation is told once it is let
//! go, how a drag and a paint are handled, and the handler for every
//! message `App` answers.

use super::*;

/// The window's procedure. Its work runs inside `crash::WINDOW`: after a
/// panic, `App` is never touched again, and Kiln says it has to close and
/// ends, keeping what it holds.
pub(super) extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if crash::WINDOW.tripped() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    crash::WINDOW
        .run(|| answer_message(hwnd, msg, wparam, lparam))
        .unwrap_or_else(|| crash::close_after_panic(hwnd))
}

/// The window's answer to one message. `App` is borrowed for `handle` and
/// `after` only; once it is let go, UI Automation is told what changed,
/// and the effects the handler queued run. A message that finds `App`
/// borrowed is answered without it (`nested`).
pub(super) fn answer_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if let Some(answer) = frame_message(hwnd, msg, wparam, lparam) {
        return answer;
    }
    let Some(cell) = AppCell::of(hwnd) else {
        // SAFETY: the message's own arguments, handed on unchanged.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    // UI Automation asks for the window's tree. It is handed over with
    // `App` let go: the handover can deliver the window's messages.
    if msg == WM_GETOBJECT && lparam.0 as i32 == uia::ROOT_OBJECT_ID {
        let Some(root) = turn(cell, App::uia_root, |said| sync_kind(msg, wparam.0, said)) else {
            return nested(hwnd, cell, msg, wparam, lparam);
        };
        // Kept first: a client that asks again while the effects run
        // finds it.
        cell.keep_root(&root);
        run_effects(hwnd, cell);
        return uia::answer(hwnd, wparam, lparam, root);
    }
    let Some(answer) = turn(
        cell,
        |app| handle(app, msg, wparam, lparam),
        |said| sync_kind(msg, wparam.0, said),
    ) else {
        return nested(hwnd, cell, msg, wparam, lparam);
    };
    run_effects(hwnd, cell);
    // A modal loop's frame of the floating wheels, which need no `App`.
    if msg == WM_TIMER && wparam.0 == MODAL_FRAME_TIMER {
        float_wheel::pump_frames();
    }
    // The session ended while `App` was borrowed: what it holds is kept,
    // and the window goes, as `WM_ENDSESSION` does.
    if cell.take_ending() {
        turn(cell, App::end_session, |_| SyncKind::Skip);
        run_effects(hwnd, cell);
        cell.flush_at_session_end();
    }
    // Signing out or shutting down: once the message has done all it
    // does, the settings saved so far and the holding folders' notes are
    // written, last, before Windows ends Kiln.
    if msg == WM_ENDSESSION && wparam.0 != 0 {
        cell.flush_at_session_end();
    }
    let answer = match answer {
        Some(r) => r,
        // SAFETY: the message's own arguments, handed on unchanged.
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    };
    if msg == WM_DESTROY {
        // UI Automation lets go of the providers, with `App` let go: a
        // client may still be asking.
        cell.drop_root();
        uia::disconnect(hwnd);
    }
    if msg == WM_NCDESTROY {
        // The window has gone; its cell lives until the loop ends.
        AppCell::forget(hwnd);
    }
    answer
}

/// One borrow of the window's `App`: `f`, then what follows every message
/// (`after`), with `sync` saying when the tree catches up. Once `App` is
/// let go, UI Automation is told what changed. `None`, with nothing run,
/// while `App` is already borrowed.
pub(super) fn turn<R>(
    cell: &AppCell,
    f: impl FnOnce(&mut App) -> R,
    sync: impl FnOnce(bool) -> SyncKind,
) -> Option<R> {
    let (answer, events) = cell.with(|app| {
        // The fold is read once for the whole message, so everything it
        // works out sees the inspector at one place (`Motion::hold_fold`).
        let outer = app.motion.hold_fold(Instant::now());
        // A client was given the tree while `App` was borrowed: if it is
        // the first, it is set up now, as one asking now would be.
        if cell.take_client_asked() && !app.uia.active() {
            let _ = app.uia_root();
        }
        let answer = f(app);
        after(app, sync);
        app.motion.release_fold(outer, Instant::now());
        // A hit-test, which is answered without `App`, finds Back while
        // Settings is open.
        cell.set_settings_open(app.settings_ui.is_some());
        // SetFocus sends focus messages straight back to this window: it
        // is made with `App` let go, after what this message queued.
        if std::mem::take(&mut app.take_focus) {
            app.effect(Effect::Focus);
        }
        (answer, std::mem::take(&mut app.a11y_events))
    })?;
    // With `App` let go: a client told something may call straight back in.
    uia::raise(events);
    Some(answer)
}

/// A message that came while `App` was borrowed: from a call made with it
/// borrowed that sends straight back to the window. It never touches
/// `App`, and it isn't lost:
/// - a paint is answered by Windows, and asked for again once the borrow
///   ends;
/// - a posted message (a timer, a wake, Kiln's own posts, a close) is
///   posted again once it ends;
/// - UI Automation asking for the tree gets the root, which reads the
///   tree's own copy;
/// - the end of a session is kept in the cell, and acted on once it ends,
///   and a shutdown query is answered with what Windows was last told;
/// - anything else gets Windows' default answer.
///
/// The log says so once a borrow for each kind of message, and debug
/// builds count them (`AppCell::nested`).
pub(super) fn nested(
    hwnd: HWND,
    cell: &AppCell,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if cell.note_nested(msg) {
        log(&format!(
            "message {msg:#06x} came while the window's state was in use"
        ));
    }
    match msg {
        WM_PAINT => {
            cell.repaint_after();
            // SAFETY: the message's own arguments, handed on unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        m if posted_again(m, lparam) => {
            cell.post_later(msg, wparam, lparam);
            LRESULT(0)
        }
        // A client asks for the tree: the root reads the tree's own copy,
        // not `App`. Once `App` is free, a first client is set up as one
        // asking then would be (`App::uia_root`).
        WM_GETOBJECT if lparam.0 as i32 == uia::ROOT_OBJECT_ID => match cell.root() {
            Some(root) => {
                cell.client_asked();
                uia::answer(hwnd, wparam, lparam, root)
            }
            // SAFETY: the message's own arguments, handed on unchanged.
            None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        },
        // While results are held, ask Windows to wait, as `handle` does.
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                cell.session_ended();
                // Windows may end Kiln before the borrow does: what was
                // saved, and where each held result came from, is written
                // now, through the cell.
                cell.flush_at_session_end();
            }
            LRESULT(0)
        }
        // SAFETY: the message's own arguments, handed on unchanged.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Whether a message that finds `App` borrowed is posted again: timers,
/// wakes, Kiln's own posts and the tray's, and a close, which Windows'
/// default answer would carry out without asking about what's held.
pub(super) fn posted_again(msg: u32, lparam: LPARAM) -> bool {
    #[cfg(debug_assertions)]
    if msg == crash::WM_KILN_TEST_PANIC {
        return true;
    }
    matches!(
        msg,
        WM_TIMER
            | WM_CLOSE
            | WM_KILN_WAKE
            | WM_KILN_NOTIFY
            | WM_KILN_STARTED
            | WM_KILN_HOOK
            | tray::WM_KILN_TRAY
    ) || (msg == WM_COMMAND && lparam.0 == 0)
        || msg == tray::taskbar_created()
}

/// The drop target's answer to one drag event: it reaches `App` as a
/// message does, inside `crash::WINDOW`, and the effects it queued run
/// once `App` is let go. `None`, no drop, while `App` is borrowed or the
/// window has none.
pub(super) fn answer_drag(hwnd: HWND, kind: DragKind, ev: &DragEvent) -> Option<LRESULT> {
    if crash::WINDOW.tripped() {
        return None;
    }
    crash::WINDOW
        .run(|| {
            let cell = AppCell::of(hwnd)?;
            let answer = turn(
                cell,
                |app| on_drag(app, kind, ev),
                |said| drag_sync(kind, said),
            )?;
            run_effects(hwnd, cell);
            Some(answer)
        })
        .unwrap_or_else(|| crash::close_after_panic(hwnd))
}

/// After each message, tell UI Automation each status set, at once, then
/// bring its tree up to date, even while minimised, when nothing paints.
/// `sync` says when, from whether a status was set; the events a wake
/// handled may ask for sooner (`App::woke`).
pub(super) fn after(app: &mut App, sync: impl FnOnce(bool) -> SyncKind) {
    let woke = std::mem::replace(&mut app.woke, SyncKind::Skip);
    // A prompt the window can no longer draw goes to Windows' own dialog.
    app.ask_elsewhere_if_undrawn();
    app.run_due();
    // Once the window is up, what became of last time's results is said.
    app.show_delivered();
    // Files that arrived under a panel or a dialog join the list once it's
    // back in view.
    if !app.incoming.queued.is_empty() && !app.adds_wait() {
        let paths = std::mem::take(&mut app.incoming.queued);
        app.add(paths, false);
    }
    // Walks that held their rows back under a panel or dialog go on.
    app.resume_walks();
    // Whatever the message moved (the scroll, the size, the selection,
    // the files, the pointer, the screen), the motion, the inspector and
    // the previews follow it. Nothing is built or asked when none of that
    // changed.
    app.follow();
    app.refresh_panel();
    app.look_if_helper_selected();
    app.ask_previews();
    let sets = app.status.take_sets();
    let said = !sets.is_empty();
    let active = app.uia.active();
    for (text, alert) in app.status.teller.tell(sets, active) {
        let event = app.uia.status(text, alert);
        app.a11y_events.push(event);
    }
    if !active {
        return;
    }
    match sync(said).max(woke) {
        SyncKind::Skip => {}
        SyncKind::Now => app.sync_a11y(true),
        SyncKind::Soon => app.sync_a11y_soon(),
    }
}

/// When a message's changes reach UI Automation's tree, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SyncKind {
    /// It changes nothing the tree shows, or builds the tree itself.
    Skip,
    /// At most every `a11y::PROGRESS_EVERY`.
    Soon,
    Now,
}

/// Input, a status set and the window's own news go at once; anything
/// else, such as animation frames and job progress, a few times a second
/// at most. Painting never builds the tree. `timer` is a WM_TIMER's id.
/// A wake is what its events need (`event_sync`).
pub(super) fn sync_kind(msg: u32, timer: usize, said: bool) -> SyncKind {
    match msg {
        _ if said => SyncKind::Now,
        WM_PAINT | WM_ERASEBKGND | WM_GETOBJECT | WM_NCHITTEST | WM_SETCURSOR | WM_DESTROY
        | WM_NCDESTROY | WM_KILN_WAKE => SyncKind::Skip,
        WM_TIMER if timer == A11Y_TIMER => SyncKind::Skip,
        WM_TIMER if timer == HINT_TIMER || timer == COPIED_TIMER => SyncKind::Now,
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK
        | WM_CONTEXTMENU | WM_MOUSEWHEEL | WM_SETFOCUS | WM_KILLFOCUS | WM_ACTIVATE
        | WM_COPYDATA | WM_SIZE | WM_DPICHANGED | WM_KILN_NOTIFY => SyncKind::Now,
        _ => SyncKind::Soon,
    }
}

/// Run `handle` on `state`, then fold `kind` into its `woke`. After, not
/// before: a handler that runs a modal loop has each nested message's
/// `after` take `woke`, which would otherwise erase this event's kind
/// before the wake's own `after` reads it.
pub(super) fn fold_after<S>(
    state: &mut S,
    kind: SyncKind,
    woke: impl Fn(&mut S) -> &mut SyncKind,
    handle: impl FnOnce(&mut S),
) {
    handle(state);
    let w = woke(state);
    *w = (*w).max(kind);
}

/// What an event from the channel needs of the tree, as its message did:
/// a preview changes nothing the tree shows; job news and animation-like
/// news go a few times a second at most; the rest at once.
pub(super) fn event_sync(event: &Event) -> SyncKind {
    match event {
        Event::Preview(..) => SyncKind::Skip,
        Event::Job(_)
        | Event::Copying(_)
        | Event::TextScale
        | Event::Float(_)
        | Event::Walked(_)
        | Event::WalkOn(_) => SyncKind::Soon,
        Event::Update(_)
        | Event::Probed(_)
        | Event::Cards(_)
        | Event::Extensions(_)
        | Event::Inspected(_)
        | Event::Exported(_)
        | Event::HeldOpened(_)
        | Event::Swept
        | Event::Delivered(_)
        | Event::Mf(_)
        | Event::Engines(_)
        | Event::Gone(_)
        | Event::SettingsWritten
        | Event::Estimated(..)
        | Event::A11y(_) => SyncKind::Now,
    }
}

/// What a drag event needs of the tree, as its message did: a drop at
/// once, the rest a few times a second at most.
pub(super) fn drag_sync(kind: DragKind, said: bool) -> SyncKind {
    match kind {
        _ if said => SyncKind::Now,
        DragKind::Drop => SyncKind::Now,
        DragKind::Enter | DragKind::Over | DragKind::Leave => SyncKind::Soon,
    }
}

/// A drag event over the window, as its messages were answered: always
/// zero, as the drop target answers copy itself.
pub(super) fn on_drag(app: &mut App, kind: DragKind, ev: &DragEvent) -> LRESULT {
    // Kiln's own drag back over Kiln: the drop target ignores it too.
    if app.incoming.own_drag.get() {
        return LRESULT(0);
    }
    match kind {
        DragKind::Enter => {
            app.incoming.drag_paths = ev.paths.clone();
            app.drag.set(
                1.0,
                theme::fluent::motion::QUICK_MS,
                Instant::now(),
                theme::fluent::motion::animations(),
            );
            app.animate();
        }
        DragKind::Over => {}
        DragKind::Leave => {
            app.drag.set(
                0.0,
                theme::fluent::motion::QUICK_MS,
                Instant::now(),
                theme::fluent::motion::animations(),
            );
            app.incoming.drag_paths.clear();
            app.close_wheel();
            app.animate();
        }
        DragKind::Drop => {
            app.dropped(ev);
            app.effect(Effect::Foreground);
        }
    }
    LRESULT(0)
}

/// Draw the window now, and mark it drawn. A frame lost with the graphics
/// device asks to be drawn again on a new one, after the ValidateRect that
/// would swallow the request. Hidden or minimised, nothing is drawn, for
/// a job's progress or anything else: the window draws as it comes back
/// (`WM_SIZE`, `show_first_frame`).
pub(super) fn paint_now(app: &mut App) {
    paint_as(app, false);
}

/// A frame of the message loop's or a modal loop's clock, after the
/// animations stepped: while only a running job's progress moves, and
/// nothing else has asked for the window to be drawn, only the running
/// rows are drawn and presented.
pub(super) fn paint_frame(app: &mut App) {
    let progress_only = app.pace() == anim::Pace::Progress
        && app.last_pace.get() == anim::Pace::Progress
        && !app.whole.get()
        && !update_pending(app.hwnd);
    paint_as(app, progress_only);
}

/// Whether the window has an update region: it was invalidated in a way
/// `App::invalidate` didn't see, such as Windows exposing it.
pub(super) fn update_pending(hwnd: HWND) -> bool {
    // SAFETY: asks only about this thread's own window.
    unsafe { GetUpdateRect(hwnd, None, false) }.as_bool()
}

/// `paint_now`, drawing just the running rows when `progress_only`.
pub(super) fn paint_as(app: &mut App, progress_only: bool) {
    let pace = app.pace();
    if app.on_screen() {
        let before = app.renderer.presented();
        app.paint(progress_only);
        // A frame that never reached the screen leaves the next one whole.
        if app.renderer.presented() == before {
            app.whole.set(true);
        }
    }
    app.last_pace.set(pace);
    // SAFETY: a plain call on this thread's own window.
    unsafe {
        let _ = ValidateRect(Some(app.hwnd), None);
    }
    if app.renderer.take_lost() {
        app.invalidate();
    }
}

pub(super) fn handle(app: &mut App, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let hwnd = app.hwnd;
    match msg {
        // Debug builds: a test's panic, with `App` borrowed as for any
        // message (`crash`).
        #[cfg(debug_assertions)]
        crash::WM_KILN_TEST_PANIC if crash::test_panic_armed() => {
            if wparam.0 == crash::TEST_PANIC_AFTER_QUIT {
                unsafe { PostQuitMessage(0) };
            }
            panic!("a test asked the window code to panic")
        }
        WM_PAINT => {
            paint_now(app);
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => Some(LRESULT(1)),
        // Files from a second launch, such as a drop on the app icon.
        WM_COPYDATA => {
            // A notification's link, from a second launch: only this run's
            // notices are acted on.
            // SAFETY: this is a WM_COPYDATA's lparam, read for the message.
            if let Some(id) = unsafe { handoff::received_notice(lparam) } {
                return Some(LRESULT(isize::from(app.open_notice(id))));
            }
            let files = unsafe { handoff::received(lparam) }?;
            // Even with no files, a second launch opens a hidden Kiln.
            app.open_window();
            if !files.is_empty() {
                app.arrive(&files);
                app.add_when_live(files);
            }
            Some(LRESULT(1))
        }
        // Shown or hidden: the inspector's clock runs only while shown.
        WM_SHOWWINDOW => {
            app.sync_clock(wparam.0 != 0);
            None
        }
        WM_SIZE => {
            app.sync_clock(wparam.0 != SIZE_MINIMIZED as usize && app.window_shown());
            // Minimised, the client area is empty: keep the swap chain, the
            // zoom and the scroll for when the window comes back, so it
            // comes back showing its last frame.
            if wparam.0 == SIZE_MINIMIZED as usize {
                return Some(LRESULT(0));
            }
            app.renderer.resize();
            app.rezoom();
            app.clamp_scroll();
            app.close_menu();
            // Back from minimised, what moves moves again.
            app.animate();
            // Drawn at the new size at once: with no redirection bitmap,
            // what the swap chain doesn't cover yet shows through, so the
            // frame isn't left for WM_PAINT. A hidden window draws when it
            // is shown.
            // SAFETY: asks only whether this thread's own window is shown.
            if unsafe { IsWindowVisible(hwnd) }.as_bool() {
                paint_now(app);
            }
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            app.dpi = (wparam.0 & 0xFFFF) as f32;
            app.renderer.set_dpi(app.unit_dpi());
            if let Some(t) = &app.tips {
                t.set_dpi(app.unit_dpi());
            }
            app.tip_cache = None;
            // SAFETY: WM_DPICHANGED's lparam is the suggested window
            // rectangle, valid for the length of the message; it is copied
            // here.
            let r = unsafe { system::lparam_ref::<RECT>(lparam) }.copied()?;
            // (`place_now` keeps it inside the test switch's area.)
            app.effect(Effect::Place(Place {
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: r.bottom - r.top,
                flags: SWP_NOZORDER | SWP_NOACTIVATE,
            }));
            app.invalidate();
            Some(LRESULT(0))
        }
        // The smallest window holds the smallest layout at the Text size:
        // its client area is `layout::min_window`, kept within the screen.
        WM_GETMINMAXINFO => {
            // SAFETY: WM_GETMINMAXINFO's lparam is the system's MINMAXINFO
            // to fill in, valid and unshared for the length of the
            // message.
            let info = unsafe { system::lparam_mut::<MINMAXINFO>(lparam) }?;
            let (w, h) = window_size(hwnd, app.dpi, layout::min_window(app.text_scale));
            info.ptMinTrackSize = POINT { x: w, y: h };
            Some(LRESULT(0))
        }
        // "Added 2 files." has been read: the idle hint can show. A timer
        // that fires early waits out the rest instead.
        // The same timer lets a result or an alert go after its few
        // seconds: the frame it asks for does that.
        WM_TIMER if wparam.0 == HINT_TIMER => {
            let status_rest = app
                .status
                .seen
                .as_ref()
                .and_then(|(since, _)| ways_in::status_wait(since.elapsed()));
            match app.status.hint_wait().into_iter().chain(status_rest).min() {
                Some(rest) => app.arm_hint(rest),
                None => unsafe {
                    let _ = KillTimer(Some(hwnd), HINT_TIMER);
                },
            }
            app.invalidate();
            Some(LRESULT(0))
        }
        // Files that arrived have stopped arriving.
        WM_TIMER if wparam.0 == ARRIVALS_TIMER => {
            app.arrivals_due();
            Some(LRESULT(0))
        }
        // "Copied" has been read: the button says "Copy" again.
        WM_TIMER if wparam.0 == COPIED_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), COPIED_TIMER);
            }
            app.invalidate();
            Some(LRESULT(0))
        }
        // A change held back is due. A timer that fires early waits out
        // the rest: building early would hold a percent again.
        WM_TIMER if wparam.0 == A11Y_TIMER => {
            let now = Instant::now();
            match app.a11y_due {
                Some(due) if now < due => unsafe {
                    SetTimer(Some(hwnd), A11Y_TIMER, a11y::timer_ms(now, due), None);
                },
                _ => app.sync_a11y(true),
            }
            Some(LRESULT(0))
        }
        // A second has passed while the inspector shows a file converting:
        // it is built again after this message, its time left with it, and
        // the row's own line is drawn again beside it.
        WM_TIMER if wparam.0 == CLOCK_TIMER => {
            frame_log::clock_ticked();
            app.panel_stale();
            app.invalidate();
            Some(LRESULT(0))
        }
        // One frame of the animations, drawn now: the message loop sent it
        // when the display could take it, with every message handled.
        WM_KILN_FRAME => {
            app.tick();
            paint_frame(app);
            Some(LRESULT(0))
        }
        // A modal loop's frame: drawn if the swap chain can take it now,
        // or hasn't for a while, so the dialog's loop never waits on it.
        // The floating wheels' frames follow, with `App` let go
        // (`answer_message`), hidden in the tray or not.
        WM_TIMER if wparam.0 == MODAL_FRAME_TIMER => {
            if app.pace() != anim::Pace::Idle && app.renderer.frame_due(STALL_MS) {
                app.tick();
                paint_frame(app);
            }
            app.modal_frames();
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            if !app.tracking_mouse {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut tme);
                }
                app.tracking_mouse = true;
            }
            let at = (lo(lparam), hi(lparam));
            if app.last_mouse != Some(at) {
                app.last_mouse = Some(at);
                let (x, y) = app.dips(at.0, at.1);
                // Back lights under the pointer; off the bar's own buttons,
                // Windows' go dark.
                let back = app.back_at(x, y);
                if app.caption.pressed.is_none_or(|p| Some(p) == back) && app.caption.hover(back) {
                    app.invalidate();
                }
                app.mouse_move(x, y, wparam.0 as u32);
            }
            Some(LRESULT(0))
        }
        // Windows' buttons on the title bar: lit while the pointer is over
        // them, acting on release over the one pressed.
        WM_NCMOUSEMOVE => {
            if !app.tracking_nc {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: a local structure of its own size, for this
                // thread's own window.
                unsafe {
                    let _ = TrackMouseEvent(&mut tme);
                }
                app.tracking_nc = true;
            }
            if app.caption.hover(caption_part(wparam.0)) {
                app.invalidate();
            }
            None
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
            // The bar itself and the edges are Windows' to move and size.
            let part = caption_part(wparam.0)?;
            app.caption.press(Some(part));
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_NCLBUTTONUP => {
            let over = caption_part(wparam.0);
            let pressed = app.caption.pressed.is_some();
            if let Some(part) = app.caption.release(over) {
                app.title_act(part);
            }
            if !pressed && over.is_none() {
                return None;
            }
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_NCMOUSELEAVE => {
            app.tracking_nc = false;
            if app.caption.leave() {
                app.invalidate();
            }
            None
        }
        // A tooltip about to show asks what to say.
        WM_NOTIFY => {
            let id = app.tips.as_ref()?.asking(lparam)?;
            let text = app.tip_text(id);
            if let Some(t) = &mut app.tips {
                t.answer(lparam, &text);
            }
            Some(LRESULT(0))
        }
        // Another window took the mouse mid-press: no drag or collapse is
        // left waiting for a release that won't come.
        WM_CAPTURECHANGED => {
            if lparam.0 != hwnd.0 as isize {
                if app.caption.pressed == Some(Part::Back) && app.caption.leave() {
                    app.invalidate();
                }
                app.list.drop_press();
            }
            None
        }
        WM_MOUSELEAVE => {
            app.tracking_mouse = false;
            app.last_mouse = None;
            if app.caption.hover == Some(Part::Back) && app.caption.pressed.is_none() {
                app.caption.leave();
            }
            app.dispatch(Input::Leave);
            app.invalidate();
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN => {
            app.effect(Effect::Focus);
            let (x, y) = app.dips(lo(lparam), hi(lparam));
            // Back is over every layer: it closes Settings, whatever is
            // open in it.
            if let Some(back) = app.back_at(x, y) {
                app.caption.press(Some(back));
                app.effect(Effect::Capture);
                app.invalidate();
                return Some(LRESULT(0));
            }
            app.mouse_down(x, y, wparam.0 as u32);
            Some(LRESULT(0))
        }
        // The second click of a double-click: on a held row it opens the
        // result; anywhere else it's a click like any other.
        WM_LBUTTONDBLCLK => {
            app.effect(Effect::Focus);
            let (x, y) = app.dips(lo(lparam), hi(lparam));
            let keys = wparam.0 as u32;
            // With Ctrl or Shift it's a second selection click, not an open.
            // Unmodified, a link acted on the first click; a second would
            // open it twice.
            let modified = keys & (MK_SHIFT_FLAG | MK_CONTROL_FLAG) != 0;
            // An inspector control acted on the first click; a second
            // could act again on whatever took its place.
            let inspector = app.on_inspector(x, y);
            if !inspector && (modified || !app.on_link(x, y)) {
                app.mouse_down(x, y, keys);
            }
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            let (x, y) = app.dips(lo(lparam), hi(lparam));
            if app.caption.pressed == Some(Part::Back) {
                let over = app.back_at(x, y);
                app.effect(Effect::ReleaseCapture);
                if let Some(part) = app.caption.release(over) {
                    app.title_act(part);
                }
                app.invalidate();
                return Some(LRESULT(0));
            }
            app.mouse_up(x, y);
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            let raw = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
            // In proportion, as the list scrolls: a precise touchpad sends
            // many small steps of less than a notch.
            app.dispatch(Input::Scroll(raw as f32 / WHEEL_DELTA as f32));
            Some(LRESULT(0))
        }
        // The Apps key, Shift+F10 or a right click: the right-click menu.
        // On an open wheel, the options for the format under the pointer.
        WM_CONTEXTMENU => {
            if app.wheel.as_ref().is_some_and(|w| w.is_open()) {
                let spot = if lparam.0 == -1 {
                    app.wheel.as_ref().and_then(|w| w.lit())
                } else {
                    let at = app.screen_dips(lo(lparam), hi(lparam));
                    let w = app.wheel.as_ref().expect("checked above");
                    w.spot_at(at.0, at.1)
                };
                app.open_sheet(spot);
                return Some(LRESULT(0));
            }
            // From the keyboard, it closes a menu that is up (Shift+F10),
            // or one the Menu key's press just closed. With the pointer,
            // a menu already up gives way to the new one.
            let by_key = std::mem::take(&mut app.menu_closed_by_key);
            if lparam.0 == -1 && (app.row_menu.is_some() || by_key) {
                app.close_menu();
                return Some(LRESULT(0));
            }
            app.close_menu();
            if !app.over().list_live() {
                return Some(LRESULT(0));
            }
            let at = (lparam.0 != -1).then(|| (lo(lparam), hi(lparam)));
            app.context_menu(at);
            Some(LRESULT(0))
        }
        WM_KEYDOWN => {
            // Bit 30: the key was already down, and this is its repeat.
            app.key_repeat = lparam.0 & (1 << 30) != 0;
            // Only the Menu key's own press, closing the menu, is followed
            // by the WM_CONTEXTMENU that must not open it again.
            app.menu_closed_by_key = false;
            let taken = app.key(VIRTUAL_KEY(wparam.0 as u16));
            app.key_repeat = false;
            taken.then_some(LRESULT(0))
        }
        // Typing reaches the Make smaller or Resize dialog's Custom box.
        WM_CHAR if app.prompts.tool_ui.is_some() && app.prompts.ask_ui.is_none() => {
            if let Some(c) = char::from_u32(wparam.0 as u32) {
                app.drive_tool(|t, cx| t.char(cx, c));
            }
            Some(LRESULT(0))
        }
        // Alt+Enter arrives as a system key.
        WM_SYSKEYDOWN if wparam.0 == VK_RETURN.0 as usize => {
            app.key(VK_RETURN);
            Some(LRESULT(0))
        }
        // Its character would otherwise go looking for a menu mnemonic,
        // which beeps.
        WM_SYSCHAR if wparam.0 == '\r' as usize => Some(LRESULT(0)),
        // There's no menu bar, so a lone Alt shouldn't put the window into
        // menu mode and swallow the next key.
        WM_SYSCOMMAND if (wparam.0 & 0xFFF0) == SC_KEYMENU as usize && lparam.0 == 0 => {
            Some(LRESULT(0))
        }
        // A contrast theme, Animation effects, Text size or PATH may have
        // changed: each is cheap to read again, and PATH is looked at on
        // the engines thread. Text size also has its own
        // event (`Event::TextScale`); this catches it if that never comes.
        WM_SETTINGCHANGE => {
            // SAFETY: for WM_SETTINGCHANGE, lparam is null or a NUL-ended
            // string that lasts for the message.
            let what = unsafe { system::lparam_text(lparam) };
            if what.as_deref() == Some("ImmersiveColorSet") {
                app.prefs.system_light = system::system_light();
            }
            app.environment_changed(what.as_deref());
            app.display_settings_changed();
            app.text_scale_changed();
            // A taskbar set to hide itself, or not: a maximised window
            // leaves room for it again (`autohide_of`).
            if wparam.0 as u32 == windows::Win32::UI::WindowsAndMessaging::SPI_SETWORKAREA.0 {
                refresh_bars(app.hwnd);
                app.effect(Effect::FrameChanged);
            }
            None
        }
        // What the screen holds may not be the last frame: draw it all.
        WM_DISPLAYCHANGE | WM_DWMCOMPOSITIONCHANGED => {
            // Monitors came, went or moved: their taskbars are asked again.
            if msg == WM_DISPLAYCHANGE {
                refresh_bars(hwnd);
            }
            app.invalidate();
            None
        }
        WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
            app.display_settings_changed();
            None
        }
        // Events wait on the channel: the wake itself carries nothing.
        WM_KILN_WAKE => {
            app.woken();
            Some(LRESULT(0))
        }
        // The finish notice: a click opens Kiln on the result; anything
        // else means it has gone.
        WM_KILN_NOTIFY => {
            match (lparam.0 & 0xFFFF) as u32 {
                NIN_BALLOONUSERCLICK => {
                    system::notify_done(hwnd);
                    app.notice_clicked();
                }
                NIN_BALLOONTIMEOUT | NIN_BALLOONHIDE => system::notify_done(hwnd),
                _ => {}
            }
            Some(LRESULT(0))
        }
        WM_KILN_STARTED => {
            app.started = true;
            Some(LRESULT(0))
        }
        WM_KILN_HOOK => {
            app.verdict(hooks::decode(wparam, lparam));
            Some(LRESULT(0))
        }
        // A release over a lit slice brought no drop: the wheel closes.
        WM_TIMER if wparam.0 == FLOAT_TIMER => {
            app.close_float();
            Some(LRESULT(0))
        }
        // Back from sleep: Windows may have taken the hooks away.
        WM_POWERBROADCAST if wparam.0 == PBT_APMRESUMEAUTOMATIC as usize => {
            if app.prefs.settings.drag_wheel {
                app.arm_hooks();
            }
            None
        }
        // Coming back to Kiln: a held result may have been removed by hand
        // meanwhile, or a helper app or the Media Feature Pack added.
        WM_ACTIVATE => {
            // The title bar's text follows.
            app.active = (wparam.0 & 0xFFFF) as u32 != WA_INACTIVE;
            // A menu closes when the window loses the keyboard.
            if !app.active {
                app.close_menu();
            }
            app.invalidate();
            if app.active {
                app.ask_gone();
                app.look_again_on_activate(Instant::now());
                app.recheck_media_foundation();
                app.recheck_extensions();
            }
            None
        }
        // Closing with results held or jobs running asks first. Cancel, or
        // cancelling the folder picker, keeps the window open; so does an
        // export that didn't save everything.
        // While the drag-wheel is on, closing hides the window instead,
        // once held results are asked about (`close_window`).
        WM_CLOSE => {
            app.close_window();
            Some(LRESULT(0))
        }
        // The tray icon: a click opens Kiln, a right click shows its menu,
        // and a notification shown from it was clicked or went.
        WM_KILN_TRAY => {
            match tray::read(wparam, lparam) {
                TrayEvent::Open => app.open_window(),
                // With a dialog of Kiln's open, the menu would start
                // nothing: the dialog is brought forward instead.
                TrayEvent::Menu(_) if system::modal_open() => app.open_window(),
                // The menu has the window, not the `Tray`: a message its
                // loop delivers may take the icon away.
                // With the test placement switch on a capture has no icon to
                // click: the message alone opens the menu.
                TrayEvent::Menu(at) if app.tray.is_some() || placement::quiet_on() => {
                    app.effect(Effect::TrayMenu {
                        at,
                        drag_wheel_on: app.prefs.settings.drag_wheel,
                        palette: Box::new(app.prefs.palette),
                        text_scale: app.text_scale,
                    });
                }
                TrayEvent::Menu(_) => {}
                TrayEvent::NoticeClicked => app.notice_clicked(),
                TrayEvent::NoticeGone | TrayEvent::Other => {}
            }
            Some(LRESULT(0))
        }
        // The tray menu's commands, as a menu without TPM_RETURNCMD would
        // send them; Kiln posts Quit to itself this way.
        WM_COMMAND if lparam.0 == 0 => {
            let c = TrayCommand::from_id(wparam.0 & 0xFFFF)?;
            app.tray_command(c);
            Some(LRESULT(0))
        }
        // Nothing is held for the person to save: Windows may end the
        // session. What isn't saved yet is kept for the next start.
        WM_QUERYENDSESSION => Some(LRESULT(1)),
        // Signing out or shutting down: Windows ends Kiln once this
        // returns. Its icon goes first, so none is left behind.
        // Kiln ends for real, as when the Restart Manager closes it for an
        // update (ENDSESSION_CLOSEAPP): its jobs stop and the window goes,
        // so the usual teardown takes the icon away. The holding folder is
        // kept, results and all, and the next start offers them (UX-02).
        // Nothing here waits or asks.
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                app.end_session();
            }
            Some(LRESULT(0))
        }
        // Explorer restarted, and the taskbar with it: the icon comes back,
        // or tries again in a moment if Explorer isn't ready for it.
        m if m == tray::taskbar_created() => {
            // A new taskbar may hide itself where the old one didn't.
            refresh_bars(hwnd);
            app.effect(Effect::FrameChanged);
            if !app.restore_tray() {
                unsafe {
                    SetTimer(Some(hwnd), TRAY_TIMER, TRAY_RETRY_MS, None);
                }
            }
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == STALE_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), STALE_TIMER);
            }
            app.stale_waited_out();
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == ENGINES_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), ENGINES_TIMER);
            }
            app.engines_waited_out();
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == TRAY_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TRAY_TIMER);
            }
            if !app.restore_tray() {
                log("couldn't put the tray icon back after Explorer restarted");
            }
            Some(LRESULT(0))
        }
        // Moving, sizing and the system menu run modal loops of their own.
        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            system::window_loop(true);
            app.modal_frames();
            None
        }
        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            system::window_loop(false);
            app.modal_frames();
            None
        }
        WM_DESTROY => {
            app.destroyed = true;
            // Gone with the window, unanswered.
            app.drop_asks();
            system::notify_done(hwnd);
            if app.notify_by == toast::Channel::Toast {
                toast::forget();
            }
            app.tray = None;
            // The drag-wheel goes with the window, and its hooks: a wheel
            // fading out goes at once, its timers with its window.
            app.close_float();
            app.float_fading = None;
            if let Some(h) = app.hooks.take() {
                h.stop();
            }
            app.drop_float_notes();
            // UI Automation lets go of the tree first; the providers go
            // once `App` is let go (`answer_message`).
            app.uia.shutdown();
            // Stop the inspect thread with the window, without waiting
            // for a decode under way: nothing is reported after this.
            app.inspector.shutdown();
            // The same for the thumbnail thread: nothing is posted after this.
            app.thumbs.shutdown();
            unsafe {
                let _ = RevokeDragDrop(hwnd);
                PostQuitMessage(0);
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}
