//! First-install consent; reconnects keep the saved sharing policy.
use super::{bridge, user_error};
use crate::history::delivery;
use crate::history::delivery_status::Config;
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{self, IsTerminal, Write};
use std::path::Path;

fn import_choice(answer: &str) -> Option<crate::contract::account::SharingMode> {
    match answer.trim() {
        "" | "1" => Some(crate::contract::account::SharingMode::Selected),
        "2" => Some(crate::contract::account::SharingMode::New),
        "3" => Some(crate::contract::account::SharingMode::All),
        _ => None,
    }
}

pub(super) fn choose_import() -> Result<Option<crate::contract::account::SharingMode>> {
    if !io::stdin().is_terminal() {
        return Err(user_error("Use an interactive terminal, or explicitly pass --selected-sessions-only, --new-sessions-only, or --include-existing."));
    }
    humanln!("Share coding session content with this Cloud workspace:");
    humanln!(
        "  1. Only sessions I select (default)\n  2. Automatically upload new sessions\n  3. Automatically upload all sessions\n  4. Cancel"
    );
    print!("Choose 1, 2, 3 or 4 [1]: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer)? == 0 {
        return Ok(None);
    }
    Ok(import_choice(&answer))
}
/// Applies the setup's sharing to the upload job: a selection keeps only
/// the selected sessions, and `withhold` (`--withhold-uploads`) pauses
/// uploads until the operator policy can be read.
pub(super) fn apply_to_uploads(
    directory: &Path,
    db_path: &Path,
    config: &Config,
    withhold: bool,
) -> Result<()> {
    bridge::enforce_selection(directory, config)?;
    if withhold {
        withhold_uploads(db_path, &config.job_id, |event| {
            if bridge::json_mode() {
                bridge::emit(event);
            }
        })?;
    }
    Ok(())
}

/// Pauses the upload job while the operator policy is unknown; reports
/// `withheld` when this paused it, so only that pause is lifted later. A
/// job already paused, by the person or an earlier setup, is left alone
/// and reports nothing.
fn withhold_uploads(db_path: &Path, job_id: &str, mut report: impl FnMut(Value)) -> Result<()> {
    let conn = delivery::open_db(db_path)?;
    if delivery::status(&conn, job_id)?.state == "paused" {
        return Ok(());
    }
    delivery::pause_job(&conn, job_id)?;
    report(json!({"event": "withheld"}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first setup that withholds uploads pauses the job and reports
    /// it once; a repeat setup on the paused job reports nothing.
    #[test]
    fn withholding_pauses_the_job_once_and_reports_only_that_pause() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("history.db");
        let conn = delivery::open_db(&db).unwrap();
        let job_config = crate::history::probe::live::tests::job_config();
        let job = delivery::create_session_job(&conn, &job_config, 0).unwrap();
        assert_ne!(
            delivery::status(&conn, &job.job_id).unwrap().state,
            "paused"
        );

        let mut events = Vec::new();
        withhold_uploads(&db, &job.job_id, |event| events.push(event)).unwrap();
        assert_eq!(
            delivery::status(&conn, &job.job_id).unwrap().state,
            "paused"
        );
        assert_eq!(events, vec![json!({"event": "withheld"})]);

        let mut repeat = Vec::new();
        withhold_uploads(&db, &job.job_id, |event| repeat.push(event)).unwrap();
        assert_eq!(
            delivery::status(&conn, &job.job_id).unwrap().state,
            "paused"
        );
        assert!(
            repeat.is_empty(),
            "a paused job reports nothing: {repeat:?}"
        );
    }
    #[test]
    fn interactive_install_defaults_to_selected_sessions() {
        use crate::contract::account::SharingMode::{All, New, Selected};
        assert_eq!(import_choice(""), Some(Selected));
        assert_eq!(import_choice(" 1\n"), Some(Selected));
        assert_eq!(import_choice("2"), Some(New));
        assert_eq!(import_choice("3"), Some(All));
        assert_eq!(import_choice("4"), None);
        assert_eq!(import_choice("invalid"), None);
    }
}
