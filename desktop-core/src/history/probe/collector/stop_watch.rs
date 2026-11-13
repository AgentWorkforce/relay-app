//! Poll the generation-scoped stop file independently of a long capture. The
//! hot record loops read an atomic flag rather than opening a file per record.
//! A collector's run also watches for being superseded (`superseded`): its
//! executable replaced on disk, or the saved sharing choice changed since it
//! read it.

use super::{stop_requested, superseded};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

/// How often the watch also asks whether the run was superseded: a metadata
/// read of the executable and of `config.json`.
const SUPERSEDED_CHECK_EVERY: Duration = Duration::from_secs(5);

pub(super) struct StopWatch {
    pub(super) cancelled: Arc<AtomicBool>,
    finish: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl StopWatch {
    /// A watch for stop requests alone.
    #[cfg(test)]
    pub(super) fn start(directory: &Path, startup_id: &str) -> Self {
        Self::watching(directory, startup_id, None)
    }

    /// A watch for stop requests and for the run `started` being superseded.
    pub(super) fn superseding(
        directory: &Path,
        startup_id: &str,
        started: superseded::Started,
    ) -> Self {
        Self::watching(directory, startup_id, Some(started))
    }

    fn watching(directory: &Path, startup_id: &str, started: Option<superseded::Started>) -> Self {
        let cancelled = Arc::new(AtomicBool::new(stop_requested(directory, startup_id)));
        let flag = cancelled.clone();
        let directory = directory.to_owned();
        let startup_id = startup_id.to_owned();
        let (finish, receive) = mpsc::channel();
        let mut next_check = Instant::now() + SUPERSEDED_CHECK_EVERY;
        let thread = std::thread::spawn(move || loop {
            if stop_requested(&directory, &startup_id) {
                flag.store(true, Ordering::Relaxed);
                break;
            }
            if let Some(started) = started.as_ref().filter(|_| Instant::now() >= next_check) {
                next_check = Instant::now() + SUPERSEDED_CHECK_EVERY;
                if let Some(reason) = started.superseded(&directory) {
                    eprintln!("{}", reason.message());
                    flag.store(true, Ordering::Relaxed);
                    break;
                }
            }
            match receive.recv_timeout(Duration::from_millis(100)) {
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                _ => break,
            }
        });
        Self {
            cancelled,
            finish,
            thread: Some(thread),
        }
    }
}

impl Drop for StopWatch {
    fn drop(&mut self) {
        let _ = self.finish.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
