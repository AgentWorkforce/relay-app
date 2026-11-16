//! Closing a startup scan that ended paused, once a later round succeeds.
use super::{LocalProgress, Progress, WRITES};
use std::{path::Path, sync::Mutex};

/// Rewrite a capture that ended paused as complete, once a later pass
/// succeeds: its monitor has already finished, so this writes the closing
/// report a scan that succeeded writes. Only the paused report itself is
/// rewritten: once delivery has published over it (uploading, watching, a
/// stall), that is the newer state and stays, checked under the lock every
/// `progress.json` write takes. The paused scan's file counts carry over.
pub fn capture_resumed(directory: &Path) {
    let Some(paused) = paused_report(directory) else {
        return;
    };
    let count = |field: &str| paused[field].as_u64().and_then(|n| usize::try_from(n).ok());
    let mut progress = Progress {
        phase: "scanning".into(),
        source: "local history".into(),
        processed_files: count("processedFiles").unwrap_or(0),
        total_files: count("totalFiles"),
        ..Default::default()
    };
    // Count before taking the write lock: counting reads the catalog.
    progress.count(&directory.join("history.db"), None, true, None);
    let local = LocalProgress {
        directory: directory.to_owned(),
        closed: Mutex::new(false),
        delivery: false,
    };
    // Checked again under the lock: a delivery report published since the
    // first check is the newer state and stays.
    let _writing = WRITES.lock().unwrap_or_else(|e| e.into_inner());
    if paused_report(directory).is_some() {
        local.write_locked(&progress);
    }
}

/// The saved progress, while it still reports capture as paused.
fn paused_report(directory: &Path) -> Option<serde_json::Value> {
    let saved = std::fs::read(directory.join("progress.json")).ok()?;
    let mut saved: serde_json::Value = serde_json::from_slice(&saved).ok()?;
    let progress = saved.get_mut("progress")?.take();
    (progress["phase"] == "capture_paused").then_some(progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(directory: &Path) -> serde_json::Value {
        let bytes = std::fs::read(directory.join("progress.json")).unwrap();
        let mut saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        saved["progress"].take()
    }

    fn write(directory: &Path, progress: &Progress) {
        LocalProgress {
            directory: directory.to_owned(),
            closed: Mutex::new(false),
            delivery: false,
        }
        .write(progress);
    }

    #[test]
    fn a_paused_scan_is_reported_complete_with_its_file_counts() {
        let directory = tempfile::tempdir().unwrap();
        crate::history::probe::history::open(&directory.path().join("history.db")).unwrap();
        write(
            directory.path(),
            &Progress {
                phase: "capture_paused".into(),
                processed_files: 40,
                total_files: Some(40),
                ..Default::default()
            },
        );
        capture_resumed(directory.path());
        let progress = saved(directory.path());
        assert_eq!(progress["phase"], "scanning");
        assert_eq!(progress["processedFiles"], 40);
        assert_eq!(progress["totalFiles"], 40);
    }

    /// Delivery publishes once the paused scan's monitor is gone; a later
    /// successful round must not blank its report.
    #[test]
    fn delivery_state_published_since_the_pause_is_kept() {
        let directory = tempfile::tempdir().unwrap();
        let watching = Progress {
            phase: "watching".into(),
            backlog_complete: true,
            records_uploaded: 100,
            ..Default::default()
        };
        write(directory.path(), &watching);
        capture_resumed(directory.path());
        let progress = saved(directory.path());
        assert_eq!(progress["phase"], "watching");
        assert_eq!(progress["recordsUploaded"], 100);
    }

    #[test]
    fn nothing_is_written_without_a_saved_report() {
        let directory = tempfile::tempdir().unwrap();
        capture_resumed(directory.path());
        assert!(!directory.path().join("progress.json").exists());
    }
}
