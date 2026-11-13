//! Replayable upload-mode changes preserve individual user denials.
use super::*;

pub(super) fn make_plan(
    conn: &Connection,
    directory: &Path,
    mut config: Config,
    requested: Option<SharingMode>,
    identities_requested: &[SessionIdentity],
    include: bool,
) -> Result<ChangePlan> {
    validate_mutation_identities(conn, identities_requested, include)?;
    let old = delivery::status(conn, &config.job_id)?;
    let mut selected = selected(directory)?;
    let mut user_excluded: HashSet<_> = delivery::user_exclusions(conn)?.into_iter().collect();
    let incremental = requested.is_none() && mode(&config) == SharingMode::Selected;
    let mut withheld = if incremental {
        HashSet::new()
    } else {
        excluded(conn)?
    };
    if let Some(mode) = requested {
        config.sharing_mode = Some(mode);
        config.include_existing = mode == SharingMode::All;
        withheld = if mode == SharingMode::All {
            user_excluded.clone()
        } else {
            delivery::session_identities(conn)?
                .into_iter()
                .filter(|id| !selected.contains(id))
                .chain(user_excluded.iter().cloned())
                .collect()
        };
    } else {
        for id in identities_requested.iter().cloned() {
            if include {
                withheld.remove(&id);
                user_excluded.remove(&id);
                if !selected.contains(&id) {
                    selected.push(id);
                }
            } else {
                selected.retain(|v| v != &id);
                user_excluded.insert(id.clone());
                withheld.insert(id);
            }
        }
    }
    // A subagent is part of its session, never a selection of its own: one
    // the manifest kept from before it was known to be one is dropped, and is
    // shared through its session.
    let mut kept = Vec::with_capacity(selected.len());
    for id in selected {
        if delivery::delegating_session(conn, &id)?.is_none()
            && !delivery::native_grok_subagent(conn, &id)?
        {
            kept.push(id);
        }
    }
    let selected = kept;
    let mut job = old.config;
    job.selection = collector::selection(mode(&config) == SharingMode::All);
    Ok(ChangePlan {
        config,
        job,
        selected,
        excluded: withheld.into_iter().collect(),
        user_excluded: Some(user_excluded.into_iter().collect()),
        paused: old.state == "paused",
        delta: incremental.then(|| (identities_requested.to_vec(), include)),
        cloud_selection: None,
    })
}

/// The replayed intent records whether an exclusion was automatic or explicit.
pub(super) fn apply_exclusions(conn: &Connection, plan: &ChangePlan) -> Result<()> {
    let mut wanted: HashSet<_> = plan.excluded.iter().cloned().collect();
    if plan.user_excluded.is_none() {
        // An intent written by an older build may have cleared every deny
        // for All. Recovery must not replay that obsolete widening behavior.
        wanted.extend(delivery::user_exclusions(conn)?);
    }
    for id in excluded(conn)? {
        if !wanted.contains(&id) {
            delivery::set_session_excluded(conn, &id, false)?;
        }
    }
    for id in &wanted {
        if plan
            .user_excluded
            .as_ref()
            .is_none_or(|denied| denied.contains(id))
        {
            delivery::set_session_excluded(conn, id, true)?;
        } else {
            delivery::set_policy_excluded(conn, id, true)?;
        }
    }
    Ok(())
}
