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
        // Any mode the person sets is their choice. The flag is cleared here,
        // after a replayed older plan, so it is saved only with the change.
        config.sharing_unchosen = false;
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

/// Whether the install already carries this request, which then replies
/// without a new delivery generation. A new generation restarts the upload
/// from the first revision, so re-applying the mode the install already
/// shares keeps its job, as does a broad-mode include already accepted. A
/// pending change, a missing or cancelled job, or sessions deleted from Cloud
/// that the request restores still take the normal path.
pub(super) fn already_applied(
    directory: &Path,
    config: &Config,
    keys: &[String],
    identities: &[SessionIdentity],
    requested: Option<SharingMode>,
    include: bool,
) -> Result<bool> {
    let deleted = deleted_uploads(directory)?;
    if directory.join("sharing-change.json").exists() {
        return Ok(false);
    }
    let conn = read_db(directory)?;
    let Some(requested) = requested else {
        if keys.iter().any(|key| deleted.contains_key(key))
            || !selection_validation::mutations_unchanged(
                &conn, directory, config, identities, include,
            )?
        {
            return Ok(false);
        }
        let side = if include { "included" } else { "excluded" };
        emit(json!({side: keys, "regenerated": false}));
        return Ok(true);
    };
    let kept = mode(config) == requested
        && (requested != SharingMode::All || deleted.is_empty())
        && delivery::status(&conn, &config.job_id).is_ok_and(|job| job.state != "cancelled");
    if kept {
        emit(json!({"sharing_mode": requested, "regenerated": false}));
    }
    Ok(kept)
}

/// Re-applying the mode already shared is still the person's choice, so the
/// install stops counting as unchosen. The caller holds desktop.lock, as every
/// config.json writer does; a failed request never reaches here.
pub(super) fn record_choice(
    directory: &Path,
    mut config: Config,
    requested: Option<SharingMode>,
) -> Result<()> {
    if requested.is_some() && config.sharing_unchosen {
        config.sharing_unchosen = false;
        save_json(&directory.join("config.json"), &config)?;
    }
    Ok(())
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
