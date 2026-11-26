//! Durable launch reservations: one JSON file per session in a private
//! directory. The file name is the provider plus the hex of the id bytes, so
//! no id (however hostile) can name a path outside the directory. Writes
//! are atomic (temp file and rename in the same directory).
use session_navigation::evidence::{Fact, Reservation};
use session_navigation::model::{Provider, SessionRef};
use session_navigation::ports::{Reservations, StoreError};

use crate::system::{join, FsError, System};

/// Bound on an id we will name a file after (hex doubles it; 255-byte names).
const MAX_ID: usize = 100;
const MAX_FILE: usize = 64 * 1024;

fn provider_tag(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::OpenCode => "opencode",
        Provider::Grok => "grok",
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        for nibble in [b >> 4, b & 0x0f] {
            out.push(char::from(
                *DIGITS.get(usize::from(nibble)).unwrap_or(&b'0'),
            ));
        }
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    text.as_bytes()
        .chunks(2)
        .map(|pair| match pair {
            [hi, lo] => Some(digit(*hi)? << 4 | digit(*lo)?),
            _ => None,
        })
        .collect()
}

/// `<provider>-<hex id>.json`, or `None` for an empty or oversized id.
pub fn file_name(session: &SessionRef) -> Option<String> {
    let id = session.id.as_bytes();
    (!id.is_empty() && id.len() <= MAX_ID)
        .then(|| format!("{}-{}.json", provider_tag(session.provider), hex(id)))
}

/// The session a file name stands for.
pub fn session_of(name: &str) -> Option<SessionRef> {
    let stem = name.strip_suffix(".json")?;
    let (tag, encoded) = stem.split_once('-')?;
    let provider = Provider::ALL
        .into_iter()
        .find(|p| provider_tag(*p) == tag)?;
    let id = String::from_utf8(unhex(encoded)?).ok()?;
    (!id.is_empty() && id.len() <= MAX_ID).then(|| SessionRef::new(provider, id))
}

/// This session's reservation. Missing directory or file is `Known(None)`;
/// anything unreadable, corrupt or naming another session is `Unknown`
/// (which blocks launches, as intended).
pub fn read<S: System>(sys: &S, dir: &str, session: &SessionRef) -> Fact<Option<Reservation>> {
    let Some(name) = file_name(session) else {
        return Fact::Unknown;
    };
    match sys.read_file(&join(dir, &name), MAX_FILE) {
        Err(FsError::NotFound) => Fact::Known(None),
        Err(_) => Fact::Unknown,
        Ok(bytes) => match serde_json::from_slice::<Reservation>(&bytes) {
            Ok(r) if r.session == *session => Fact::Known(Some(r)),
            _ => Fact::Unknown,
        },
    }
}

/// Other OpenCode sessions with a reservation file. Hidden files (atomic
/// write temporaries) are skipped; any other name we did not write is
/// `Unknown`.
pub fn other_opencode<S: System>(sys: &S, dir: &str, except: &str) -> Fact<Vec<String>> {
    let names = match sys.list_dir(dir) {
        Err(FsError::NotFound) => return Fact::Known(Vec::new()),
        Err(_) => return Fact::Unknown,
        Ok(names) => names,
    };
    let mut out = Vec::new();
    for name in names.iter().filter(|n| !n.starts_with('.')) {
        let Some(session) = session_of(name) else {
            return Fact::Unknown;
        };
        if session.provider == Provider::OpenCode && session.id != except {
            out.push(session.id);
        }
    }
    Fact::Known(out)
}

/// [`Reservations`] over a [`System`].
pub struct FileReservations<S> {
    sys: S,
    dir: String,
}

impl<S: System> FileReservations<S> {
    pub fn new(sys: S, dir: &str) -> Self {
        Self {
            sys,
            dir: dir.to_owned(),
        }
    }

    pub fn read(&self, session: &SessionRef) -> Fact<Option<Reservation>> {
        read(&self.sys, &self.dir, session)
    }
}

impl<S: System> Reservations for FileReservations<S> {
    fn reserve(&mut self, reservation: Reservation) -> Result<(), StoreError> {
        let name = file_name(&reservation.session).ok_or(StoreError)?;
        let bytes = serde_json::to_vec(&reservation).map_err(|_| StoreError)?;
        self.sys
            .create_private_dir(&self.dir)
            .map_err(|_| StoreError)?;
        self.sys
            .write_atomic(&join(&self.dir, &name), &bytes)
            .map_err(|_| StoreError)
    }

    fn release(&mut self, session: &SessionRef) -> Result<(), StoreError> {
        let name = file_name(session).ok_or(StoreError)?;
        match self.sys.remove_file(&join(&self.dir, &name)) {
            Ok(()) | Err(FsError::NotFound) => Ok(()),
            Err(_) => Err(StoreError),
        }
    }
}
