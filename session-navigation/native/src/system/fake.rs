//! A scripted machine: program runs replay responses keyed by program and
//! argv, files live in memory, and every call is recorded in order so tests
//! can assert exactly what ran. Never touches the real machine.
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::{FsError, Metadata, Output, Run, RunError, Secret, System};

/// One recorded interaction. `stdin` stays secret-typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    Run {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
        stdin: Option<Secret>,
    },
    ReadFile(String),
    ReadPrefix(String),
    ListDir(String),
    Metadata(String),
    Canonicalize(String),
    CreateDir(String),
    WriteAtomic(String),
    RemoveFile(String),
    ReadLink(String),
    /// A [`System::spawn_detached`] (its responses come from the same
    /// table as runs).
    Spawn {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
}

#[derive(Clone, Debug)]
struct FakeFile {
    bytes: Vec<u8>,
    mtime: u64,
    executable: bool,
}

type Key = (String, Vec<String>);

#[derive(Default)]
struct State {
    responses: HashMap<Key, VecDeque<Result<Output, RunError>>>,
    files: BTreeMap<String, FakeFile>,
    dirs: BTreeSet<String>,
    unreadable: BTreeSet<String>,
    /// Symlink → target (an absolute path).
    links: BTreeMap<String, String>,
    read_only: bool,
    calls: Vec<Call>,
    now: u64,
}

#[derive(Default)]
pub struct FakeSystem {
    state: Mutex<State>,
    /// The scripted Win32, when a test gives one ([`FakeSystem::set_win32`]).
    win32: Option<super::ScriptedWin32>,
    /// The scripted kernel, when a test gives one ([`FakeSystem::set_kernel`]).
    kernel: Option<std::sync::Arc<dyn super::Kernel + Send>>,
    /// [`System::concurrent`]; off unless a test turns it on.
    concurrent: bool,
}

fn parent(path: &str) -> Option<&str> {
    let (dir, _) = path.rsplit_once('/')?;
    Some(if dir.is_empty() { "/" } else { dir })
}

impl FakeSystem {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue a response for `program args…`. Responses are consumed in
    /// order; the last one repeats for every later identical call.
    pub fn on_run(&self, program: &str, args: &[&str], response: Result<Output, RunError>) {
        let key = (
            program.to_owned(),
            args.iter().map(|a| (*a).to_owned()).collect(),
        );
        self.lock()
            .responses
            .entry(key)
            .or_default()
            .push_back(response);
    }

    /// Same as [`Self::on_run`] with owned arguments.
    pub fn on_run_owned(
        &self,
        program: &str,
        args: Vec<String>,
        response: Result<Output, RunError>,
    ) {
        self.lock()
            .responses
            .entry((program.to_owned(), args))
            .or_default()
            .push_back(response);
    }

    /// Replace every queued response for `program args…` with this one.
    pub fn set_run(&self, program: &str, args: Vec<String>, response: Result<Output, RunError>) {
        self.lock()
            .responses
            .insert((program.to_owned(), args), VecDeque::from([response]));
    }

    /// Queue responses for a [`Run`] built by the adapters' own helpers.
    pub fn on(&self, run: &Run, responses: Vec<Result<Output, RunError>>) {
        self.lock()
            .responses
            .insert((run.program.clone(), run.args.clone()), responses.into());
    }

    /// Answer [`System::win32`] with `win32` (the Windows adapter's fast
    /// path); without one it is `None`.
    pub fn set_win32(&mut self, win32: super::ScriptedWin32) {
        self.win32 = Some(win32);
    }

    /// Answer [`System::kernel`] with `kernel` (the macOS adapter's fast
    /// path); without one it is `None` and the tools are run.
    pub fn set_kernel(&mut self, kernel: std::sync::Arc<dyn super::Kernel + Send>) {
        self.kernel = Some(kernel);
    }

    /// Let the adapters run independent reads on several threads. Calls are
    /// then recorded in whatever order they happen.
    pub fn set_concurrent(&mut self, concurrent: bool) {
        self.concurrent = concurrent;
    }

    /// The scripted Win32, to read its calls.
    pub fn scripted_win32(&self) -> Option<&super::ScriptedWin32> {
        self.win32.as_ref()
    }

    pub fn set_now(&self, now: u64) {
        self.lock().now = now;
    }

    fn add_dirs(state: &mut State, path: &str) {
        let mut current = Some(path);
        while let Some(p) = current {
            if p.is_empty() || !state.dirs.insert(p.to_owned()) {
                break;
            }
            current = parent(p).filter(|q| *q != p);
        }
    }

    pub fn add_file(&self, path: &str, bytes: impl Into<Vec<u8>>, mtime: u64) {
        let mut state = self.lock();
        if let Some(dir) = parent(path) {
            Self::add_dirs(&mut state, dir);
        }
        state.files.insert(
            path.to_owned(),
            FakeFile {
                bytes: bytes.into(),
                mtime,
                executable: false,
            },
        );
    }

    pub fn add_executable(&self, path: &str, mtime: u64) {
        self.add_file(path, b"#!fake".to_vec(), mtime);
        if let Some(file) = self.lock().files.get_mut(path) {
            file.executable = true;
        }
    }

    pub fn add_dir(&self, path: &str) {
        Self::add_dirs(&mut self.lock(), path);
    }

    /// Make `link` resolve to `target` for [`System::canonicalize`] and
    /// [`System::read_link`] (which returns `target` as is). Other calls on
    /// `link` see only what was added at that path itself.
    pub fn add_symlink(&self, link: &str, target: &str) {
        self.lock().links.insert(link.to_owned(), target.to_owned());
    }

    /// Every file-system call on this exact path fails with `PermissionDenied`.
    pub fn make_unreadable(&self, path: &str) {
        self.lock().unreadable.insert(path.to_owned());
    }

    /// Writes and removals fail.
    pub fn make_read_only(&self) {
        self.lock().read_only = true;
    }

    pub fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }

    /// Only the program runs, as (program, args).
    pub fn runs(&self) -> Vec<(String, Vec<String>)> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Run { program, args, .. } => Some((program, args)),
                _ => None,
            })
            .collect()
    }

    /// Only the detached spawns, as (program, args).
    pub fn spawns(&self) -> Vec<(String, Vec<String>)> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Spawn { program, args, .. } => Some((program, args)),
                _ => None,
            })
            .collect()
    }

    pub fn clear_calls(&self) {
        self.lock().calls.clear();
    }

    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.lock().files.get(path).map(|f| f.bytes.clone())
    }

    pub fn files_under(&self, dir: &str) -> Vec<String> {
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        self.lock()
            .files
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect()
    }

    fn check(state: &State, path: &str) -> Result<(), FsError> {
        if state.unreadable.contains(path) {
            Err(FsError::PermissionDenied)
        } else {
            Ok(())
        }
    }
}

impl System for FakeSystem {
    fn run(&self, run: &Run) -> Result<Output, RunError> {
        let mut state = self.lock();
        state.calls.push(Call::Run {
            program: run.program.clone(),
            args: run.args.clone(),
            env: run.env.clone(),
            stdin: run.stdin.clone(),
        });
        let key = (run.program.clone(), run.args.clone());
        let Some(queue) = state.responses.get_mut(&key) else {
            return Err(RunError::NotFound);
        };
        let response = if queue.len() > 1 {
            queue.pop_front()
        } else {
            queue.front().cloned()
        };
        let response = response.unwrap_or(Err(RunError::NotFound))?;
        if response.stdout.len() > run.max_output || response.stderr.len() > run.max_output {
            return Err(RunError::TooLarge);
        }
        Ok(response)
    }

    fn read_file(&self, path: &str, cap: usize) -> Result<Vec<u8>, FsError> {
        let mut state = self.lock();
        state.calls.push(Call::ReadFile(path.to_owned()));
        Self::check(&state, path)?;
        let file = state.files.get(path).ok_or(FsError::NotFound)?;
        if file.bytes.len() > cap {
            return Err(FsError::TooLarge);
        }
        Ok(file.bytes.clone())
    }

    fn read_prefix(&self, path: &str, len: usize) -> Result<Vec<u8>, FsError> {
        let mut state = self.lock();
        state.calls.push(Call::ReadPrefix(path.to_owned()));
        Self::check(&state, path)?;
        let file = state.files.get(path).ok_or(FsError::NotFound)?;
        Ok(file.bytes.iter().take(len).copied().collect())
    }

    fn list_dir(&self, path: &str) -> Result<Vec<String>, FsError> {
        let mut state = self.lock();
        state.calls.push(Call::ListDir(path.to_owned()));
        Self::check(&state, path)?;
        if !state.dirs.contains(path) {
            return Err(if state.files.contains_key(path) {
                FsError::Other
            } else {
                FsError::NotFound
            });
        }
        let prefix = format!("{}/", path.trim_end_matches('/'));
        let mut names = BTreeSet::new();
        for key in state.files.keys().chain(state.dirs.iter()) {
            if let Some(rest) = key.strip_prefix(&prefix) {
                if let Some(name) = rest.split('/').next().filter(|n| !n.is_empty()) {
                    names.insert(name.to_owned());
                }
            }
        }
        Ok(names.into_iter().collect())
    }

    fn metadata(&self, path: &str) -> Result<Metadata, FsError> {
        let mut state = self.lock();
        state.calls.push(Call::Metadata(path.to_owned()));
        Self::check(&state, path)?;
        if let Some(file) = state.files.get(path) {
            return Ok(Metadata {
                is_dir: false,
                is_file: true,
                len: u64::try_from(file.bytes.len()).unwrap_or(u64::MAX),
                mtime: file.mtime,
                executable: file.executable,
            });
        }
        if state.dirs.contains(path) {
            return Ok(Metadata {
                is_dir: true,
                is_file: false,
                len: 0,
                mtime: 0,
                executable: true,
            });
        }
        Err(FsError::NotFound)
    }

    fn canonicalize(&self, path: &str) -> Result<String, FsError> {
        let mut state = self.lock();
        state.calls.push(Call::Canonicalize(path.to_owned()));
        let mut current = path.to_owned();
        // Bounded like the kernel's own symlink limit.
        for _ in 0..40 {
            Self::check(&state, &current)?;
            match state.links.get(&current) {
                Some(target) => current = target.clone(),
                None if state.files.contains_key(&current) || state.dirs.contains(&current) => {
                    return Ok(current)
                }
                None => return Err(FsError::NotFound),
            }
        }
        Err(FsError::Other)
    }

    fn create_private_dir(&self, path: &str) -> Result<(), FsError> {
        let mut state = self.lock();
        state.calls.push(Call::CreateDir(path.to_owned()));
        Self::check(&state, path)?;
        if state.files.contains_key(path) {
            return Err(FsError::Other);
        }
        if state.read_only && !state.dirs.contains(path) {
            return Err(FsError::PermissionDenied);
        }
        Self::add_dirs(&mut state, path);
        Ok(())
    }

    fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<(), FsError> {
        let mut state = self.lock();
        state.calls.push(Call::WriteAtomic(path.to_owned()));
        Self::check(&state, path)?;
        let dir_exists = parent(path).is_some_and(|d| state.dirs.contains(d));
        if state.read_only || !dir_exists {
            return Err(FsError::PermissionDenied);
        }
        let mtime = state.now;
        state.files.insert(
            path.to_owned(),
            FakeFile {
                bytes: bytes.to_vec(),
                mtime,
                executable: false,
            },
        );
        Ok(())
    }

    fn remove_file(&self, path: &str) -> Result<(), FsError> {
        let mut state = self.lock();
        state.calls.push(Call::RemoveFile(path.to_owned()));
        Self::check(&state, path)?;
        if state.read_only {
            return Err(FsError::PermissionDenied);
        }
        state
            .files
            .remove(path)
            .map(|_| ())
            .ok_or(FsError::NotFound)
    }

    fn now(&self) -> u64 {
        self.lock().now
    }

    fn win32(&self) -> Option<&dyn super::Win32> {
        self.win32.as_ref().map(|w| w as &dyn super::Win32)
    }

    fn kernel(&self) -> Option<&dyn super::Kernel> {
        self.kernel.as_deref().map(|k| k as &dyn super::Kernel)
    }

    fn concurrent(&self) -> bool {
        self.concurrent
    }

    fn read_link(&self, path: &str) -> Result<String, FsError> {
        let mut state = self.lock();
        state.calls.push(Call::ReadLink(path.to_owned()));
        Self::check(&state, path)?;
        state.links.get(path).cloned().ok_or(FsError::NotFound)
    }

    fn spawn_detached(&self, run: &Run) -> Result<(), RunError> {
        let mut state = self.lock();
        state.calls.push(Call::Spawn {
            program: run.program.clone(),
            args: run.args.clone(),
            env: run.env.clone(),
        });
        let key = (run.program.clone(), run.args.clone());
        let Some(queue) = state.responses.get_mut(&key) else {
            return Err(RunError::NotFound);
        };
        let response = if queue.len() > 1 {
            queue.pop_front()
        } else {
            queue.front().cloned()
        };
        response.unwrap_or(Err(RunError::NotFound)).map(|_| ())
    }
}
