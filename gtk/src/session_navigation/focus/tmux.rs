//! A pane selection changes server state; it becomes visible only through an
//! identified attached client. Detached sessions are explicitly unavailable.
use super::*;
const PANE_FORMAT: &str = "#{pane_id}\t#{pane_pid}\t#{pane_tty}\t#{session_id}\t#{window_id}";
const CLIENT_FORMAT: &str = "#{client_name}\t#{client_pid}\t#{client_tty}\t#{session_id}";

struct Target<'a> {
    session: &'a str,
    window: &'a str,
}

fn target<'a>(listing: &'a str, pane: &str, tty: Option<&str>) -> Result<Target<'a>, String> {
    let words: Vec<_> = listing.trim_end().split('\t').collect();
    if words.len() != 5
        || words[0] != pane
        || !numeric(words[1])
        || tty != Some(words[2])
        || !words[3].strip_prefix('$').is_some_and(numeric)
        || !words[4].strip_prefix('@').is_some_and(numeric)
    {
        return Err("The recorded terminal pane no longer belongs to this session.".into());
    }
    Ok(Target {
        session: words[3],
        window: words[4],
    })
}

fn command(endpoint: &str, args: impl IntoIterator<Item = String>) -> Invocation {
    Invocation::new(
        "tmux",
        ["-S".into(), endpoint.into()].into_iter().chain(args),
    )
}

fn clients(host: &dyn Host, endpoint: &str, session: &str) -> Result<String, String> {
    host.run(&command(
        endpoint,
        [
            "list-clients".into(),
            "-t".into(),
            session.into(),
            "-F".into(),
            CLIENT_FORMAT.into(),
        ],
    ))
}

fn attached_client<'a>(listing: &'a str, session: &str) -> Result<&'a str, String> {
    let candidates: Vec<_> = listing
        .lines()
        .filter(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            fields.len() == 4
                && fields[0] == fields[2]
                && fields[2].starts_with('/')
                && numeric(fields[1])
                && fields[3] == session
        })
        .collect();
    if candidates.len() == 1 {
        Ok(candidates[0])
    } else {
        Err("The original tmux session has no unique attached terminal client. Attach it explicitly before selecting this pane.".into())
    }
}

pub(super) fn focus(
    host: &dyn Host,
    location: &SessionLocation,
    pane: &str,
    endpoint: &str,
) -> Result<Outcome, String> {
    if !pane.strip_prefix('%').is_some_and(numeric) || !endpoint.starts_with('/') {
        return Err("The recorded tmux pane or socket is invalid.".into());
    }
    let listing = host.run(&command(
        endpoint,
        [
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            pane.into(),
            PANE_FORMAT.into(),
        ],
    ))?;
    let target = target(&listing, pane, location.tty.as_deref())?;
    let first = clients(host, endpoint, target.session)?;
    let client = attached_client(&first, target.session)?;
    verify(host, location)?;
    // Re-query the server so a disappeared/replaced client cannot silently
    // redirect this action to whichever terminal happens to be current.
    let current = clients(host, endpoint, target.session)?;
    if attached_client(&current, target.session)? != client {
        return Err("The original tmux client changed. Refresh and try again.".into());
    }
    verify(host, location)?;
    let client_name = client
        .split('\t')
        .next()
        .ok_or("The tmux client is missing.")?;
    host.run(&command(
        endpoint,
        [
            "switch-client".into(),
            "-c".into(),
            client_name.into(),
            "-t".into(),
            target.session.into(),
        ],
    ))?;
    host.run(&command(
        endpoint,
        ["select-window".into(), "-t".into(), target.window.into()],
    ))?;
    host.run(&command(
        endpoint,
        ["select-pane".into(), "-t".into(), pane.into()],
    ))?;
    Ok(Outcome::Dispatched)
}
