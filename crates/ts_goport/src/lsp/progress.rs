//! Go `internal/lsp/progress.go`.
//!
//! PORT: the progress goroutine becomes a thread. It touches only `Send`
//! data: the outgoing queue and the write-once server state behind
//! `ServerShared`. Go `...any` message arguments are `Vec<String>` (built
//! with `args![..]`), as in `project::Client`.

use crate::lsp::prelude::*;

use crate::gostd::context::Done;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

// Go: lsp/progress.go:13 progressEvent
pub struct ProgressEvent {
    pub message: &'static crate::diagnostics::Message,
    pub args: Vec<String>,
    pub finish: bool,
}

// Go: lsp/progress.go:22 progressReporter
// progressReporter abstracts the LSP transport operations needed by
// projectLoadingProgress so the progress logic can be tested without a
// full Server instance.
pub trait ProgressReporter: Send + Sync {
    // done returns a channel that is closed when the server is shutting down.
    // PORT: `None` is Go's nil channel (never closed).
    fn done(&self) -> Option<Done>;
    // localize converts a diagnostic message to a display string.
    fn localize(&self, msg: &'static crate::diagnostics::Message, args: Vec<String>) -> String;
    // createWorkDoneProgress asks the client to create a progress token.
    fn create_work_done_progress(&self, token: &str);
    // sendProgress sends a $/progress notification.
    fn send_progress(&self, token: &str, value: lsproto::WorkDoneProgressBeginOrReportOrEnd);
}

// Go: lsp/progress.go:34 serverProgressReporter
// serverProgressReporter adapts *Server to the progressReporter interface.
pub struct ServerProgressReporter {
    pub server: Arc<ServerShared>,
}

impl ProgressReporter for ServerProgressReporter {
    // Go: lsp/progress.go:38 serverProgressReporter.done
    fn done(&self) -> Option<Done> {
        self.server.background_ctx().done()
    }

    // Go: lsp/progress.go:42 serverProgressReporter.localize
    fn localize(&self, msg: &'static crate::diagnostics::Message, args: Vec<String>) -> String {
        crate::diagnostics_loc::message_localize(msg, &self.server.locale(), &args)
    }

    // Go: lsp/progress.go:46 serverProgressReporter.createWorkDoneProgress
    fn create_work_done_progress(&self, token: &str) {
        let _ = send_client_request_fire_and_forget(
            &self.server,
            &lsproto::WINDOW_WORK_DONE_PROGRESS_CREATE_INFO,
            lsproto::WorkDoneProgressCreateParams {
                token: lsproto::IntegerOrString {
                    string: Some(token.to_string()),
                    ..Default::default()
                },
            },
        );
    }

    // Go: lsp/progress.go:52 serverProgressReporter.sendProgress
    fn send_progress(&self, token: &str, value: lsproto::WorkDoneProgressBeginOrReportOrEnd) {
        let _ = send_notification(
            &self.server,
            &lsproto::PROGRESS_INFO,
            lsproto::ProgressParams {
                token: lsproto::IntegerOrString {
                    string: Some(token.to_string()),
                    ..Default::default()
                },
                value,
            },
        );
    }
}

// Go: lsp/progress.go:70 projectLoadingProgress
// projectLoadingProgress manages LSP WorkDoneProgress indicators for
// long-running operations. A single persistent goroutine processes
// start/finish events, maintains a ref-counted map of active operations,
// and sends progress messages in order.
//
// To avoid flickering on fast operations, the indicator is not shown
// until progressDelay has elapsed since the first start event. If all
// operations complete before then, no progress UI is displayed.
//
// start/finish may block if the internal buffer (64 events) is full,
// but will bail out if the server's background context is cancelled.
//
// PORT: Go `ch chan progressEvent`. The channel carries `Option`: `None`
// is a wake-up that a waker on `reporter.done()` sends, so `run` can wait
// on the channel alone (Go `select`s on the channel, the delay timer and
// `done()`). The receiving end goes to the `run` thread.
pub struct ProjectLoadingProgress {
    pub reporter: Arc<dyn ProgressReporter>,
    pub ch: SyncSender<Option<ProgressEvent>>,
    pub delay: Duration, // time to wait before showing progress UI
}

// Go: lsp/progress.go:76 newProjectLoadingProgress
pub fn new_project_loading_progress(
    server: Arc<ServerShared>,
    delay: Duration,
) -> Arc<ProjectLoadingProgress> {
    new_project_loading_progress_from_reporter(Arc::new(ServerProgressReporter { server }), delay)
}

// Go: lsp/progress.go:80 newProjectLoadingProgressFromReporter
pub fn new_project_loading_progress_from_reporter(
    reporter: Arc<dyn ProgressReporter>,
    delay: Duration,
) -> Arc<ProjectLoadingProgress> {
    let (ch, rx) = sync_channel(64);
    let p = Arc::new(ProjectLoadingProgress {
        reporter,
        ch,
        delay,
    });
    // Go: go p.run()
    let run = p.clone();
    std::thread::Builder::new()
        .name("lsp-progress".to_string())
        .spawn(move || {
            if let Err(payload) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.run(rx)))
            {
                go_crash(payload);
            }
        })
        .expect("failed to spawn lsp-progress thread");
    p
}

impl ProjectLoadingProgress {
    // Go: lsp/progress.go:90 start
    pub fn start(&self, message: &'static crate::diagnostics::Message, args: Vec<String>) {
        self.send_or_drop(ProgressEvent {
            message,
            args,
            finish: false,
        });
    }

    // Go: lsp/progress.go:99 finish
    pub fn finish(&self, message: &'static crate::diagnostics::Message, args: Vec<String>) {
        self.send_or_drop(ProgressEvent {
            message,
            args,
            finish: true,
        });
    }

    // Go: the `select` of start and finish:
    //
    //	select {
    //	case p.ch <- ev:
    //		// Sent successfully.
    //	case <-p.reporter.done():
    //		// Server shutting down; drop the event.
    //	}
    //
    // PORT: a `try_send` loop that checks `done()` while the buffer is full.
    fn send_or_drop(&self, ev: ProgressEvent) {
        let done = self.reporter.done();
        let mut ev = Some(ev);
        loop {
            match self.ch.try_send(ev.take()) {
                // Sent successfully.
                Ok(()) => return,
                Err(TrySendError::Full(back)) => {
                    // Server shutting down; drop the event.
                    if done.as_ref().is_some_and(Done::is_closed) {
                        return;
                    }
                    ev = back;
                    std::thread::sleep(Duration::from_millis(1));
                }
                // PORT: the run goroutine has stopped (the server is done).
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }

    // Go: lsp/progress.go:110 run
    // run is the persistent goroutine that processes all progress events.
    // It owns all mutable state: no external synchronization needed.
    // PORT: `ch` is the receiving end of `p.ch`.
    pub fn run(&self, ch: Receiver<Option<ProgressEvent>>) {
        // Go: collections.OrderedMap[string, int]
        let mut loading: IndexMap<String, i32> = IndexMap::new();
        let mut token = String::new(); // current token; empty if no progress active
        let mut token_id = 0;
        let mut begun = false; // whether "begin" has been sent for the current token

        // PORT: Go `delay *time.Timer`; here its deadline. A fired timer is
        // cleared, because Go never receives from its channel again.
        let mut delay: Option<Instant> = None;
        let mut delay_fired = false; // true after the delay timer fires

        // PORT: the `<-p.reporter.done()` case wakes the channel wait.
        let done = self.reporter.done();
        if let Some(done) = &done {
            let wake = self.ch.clone();
            if done
                .register_waker(move || {
                    let _ = wake.try_send(None);
                })
                .is_none()
            {
                // Already closed.
                return;
            }
        }
        let is_done = || done.as_ref().is_some_and(Done::is_closed);

        enum Selected {
            Event(ProgressEvent),
            Delay,
            Done,
        }

        loop {
            let selected = match delay {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        Selected::Delay
                    } else {
                        match ch.recv_timeout(deadline - now) {
                            Ok(Some(ev)) => Selected::Event(ev),
                            Ok(None) => Selected::Done,
                            Err(RecvTimeoutError::Timeout) => Selected::Delay,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                }
                None => match ch.recv() {
                    Ok(Some(ev)) => Selected::Event(ev),
                    Ok(None) => Selected::Done,
                    Err(_) => return,
                },
            };

            match selected {
                Selected::Event(ev) => {
                    let text = self.reporter.localize(ev.message, ev.args);
                    if !ev.finish {
                        let count = loading.get(&text).copied().unwrap_or(0);
                        loading.insert(text.clone(), count + 1);
                        if token.is_empty() {
                            token_id += 1;
                            token = format!("tsgo-loading-{token_id}");
                            begun = false;
                            if self.delay.is_zero() {
                                delay_fired = true;
                                self.reporter.create_work_done_progress(&token);
                            } else {
                                delay_fired = false;
                                delay = Some(Instant::now() + self.delay);
                            }
                        }
                        if delay_fired {
                            begun = self.begin_or_report(&token, &text, begun);
                        }
                    } else {
                        let count = loading.get(&text).copied().unwrap_or(0);
                        if count <= 1 {
                            loading.shift_remove(&text);
                        } else {
                            loading.insert(text.clone(), count - 1);
                        }
                        if !token.is_empty() {
                            if loading.is_empty() {
                                if begun {
                                    self.reporter.send_progress(
                                        &token,
                                        lsproto::WorkDoneProgressBeginOrReportOrEnd {
                                            end: Some(lsproto::WorkDoneProgressEnd::default()),
                                            ..Default::default()
                                        },
                                    );
                                }
                                // Go: stopDelay()
                                delay = None;
                                token = String::new();
                            } else if delay_fired {
                                // Go: core.FirstOrNilSeq(loading.Keys())
                                let first = loading.keys().next().cloned().unwrap_or_default();
                                self.reporter.send_progress(
                                    &token,
                                    lsproto::WorkDoneProgressBeginOrReportOrEnd {
                                        report: Some(lsproto::WorkDoneProgressReport {
                                            message: Some(first),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    },
                                );
                            }
                        }
                    }
                }

                Selected::Delay => {
                    delay = None;
                    delay_fired = true;
                    if !token.is_empty() && !loading.is_empty() {
                        self.reporter.create_work_done_progress(&token);
                        let first = loading.keys().next().cloned().unwrap_or_default();
                        begun = self.begin_or_report(&token, &first, begun);
                    }
                }

                Selected::Done => {
                    if is_done() {
                        // Go: stopDelay()
                        return;
                    }
                }
            }

            // PORT: when the buffer was full, the waker's wake-up was
            // dropped; check `done()` here too.
            if is_done() {
                return;
            }
        }
    }

    // Go: lsp/progress.go:200 beginOrReport
    // beginOrReport sends WorkDoneProgressBegin if not yet begun, otherwise
    // sends WorkDoneProgressReport. Returns true to indicate begun state.
    pub fn begin_or_report(&self, token: &str, text: &str, begun: bool) -> bool {
        if !begun {
            let title = self.reporter.localize(diag::Loading, Vec::new());
            self.reporter.send_progress(
                token,
                lsproto::WorkDoneProgressBeginOrReportOrEnd {
                    begin: Some(lsproto::WorkDoneProgressBegin {
                        title,
                        message: Some(text.to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            );
        } else {
            self.reporter.send_progress(
                token,
                lsproto::WorkDoneProgressBeginOrReportOrEnd {
                    report: Some(lsproto::WorkDoneProgressReport {
                        message: Some(text.to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            );
        }
        true
    }
}
