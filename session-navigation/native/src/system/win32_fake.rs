//! A scripted [`Win32`] for tests: plain data in, every call recorded.
//!
//! The machine is a list of worlds (boot, table, windows, foreground). The
//! current world answers every read; a successful focus or spawn moves to
//! the next one (the last stays), as the real machine changes after an act.
//! A focus is checked against the current world like the real one checks
//! the live machine: the window must be listed with the named owner, and
//! the process must be in the table with the named creation time.
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::win32::{
    FocusRequest, FocusResult, RawWinProcess, RawWindow, SpawnRequest, Win32, Win32Error, WinBoot,
    WinDelegation, WinTerminalPackage,
};

/// One state of the scripted machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WinWorld {
    pub boot: Result<WinBoot, Win32Error>,
    pub processes: Result<Vec<RawWinProcess>, Win32Error>,
    pub windows: Result<Vec<RawWindow>, Win32Error>,
    pub foreground: Result<Option<RawWindow>, Win32Error>,
}

/// One recorded call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Win32Call {
    Boot,
    Processes,
    Windows,
    Foreground,
    Focus(FocusRequest),
    Delegation,
    TerminalPackage,
    UserEnvironment,
    Spawn(SpawnRequest),
}

struct State {
    worlds: Vec<WinWorld>,
    current: usize,
    focus: Result<FocusResult, Win32Error>,
    delegation: Result<WinDelegation, Win32Error>,
    terminal: Result<WinTerminalPackage, Win32Error>,
    environment: Result<Vec<(String, String)>, Win32Error>,
    spawn: Result<(), Win32Error>,
    calls: Vec<Win32Call>,
}

pub struct ScriptedWin32 {
    state: Mutex<State>,
}

impl ScriptedWin32 {
    /// The given worlds; the focus answers `Focused` when its checks pass,
    /// spawns succeed, the delegation is unset, Windows Terminal is not
    /// installed and the environment is empty.
    pub fn new(worlds: Vec<WinWorld>) -> Self {
        Self {
            state: Mutex::new(State {
                worlds,
                current: 0,
                focus: Ok(FocusResult::Focused),
                delegation: Ok(WinDelegation::default()),
                terminal: Ok(WinTerminalPackage {
                    installed: false,
                    version: None,
                }),
                environment: Ok(Vec::new()),
                spawn: Ok(()),
                calls: Vec::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What `SetForegroundWindow` and the read-back do once the identity
    /// checks pass (`Err`: the call fails before acting).
    pub fn set_focus(&self, result: Result<FocusResult, Win32Error>) {
        self.lock().focus = result;
    }

    pub fn set_delegation(&self, result: Result<WinDelegation, Win32Error>) {
        self.lock().delegation = result;
    }

    pub fn set_terminal(&self, result: Result<WinTerminalPackage, Win32Error>) {
        self.lock().terminal = result;
    }

    pub fn set_environment(&self, result: Result<Vec<(String, String)>, Win32Error>) {
        self.lock().environment = result;
    }

    pub fn set_spawn(&self, result: Result<(), Win32Error>) {
        self.lock().spawn = result;
    }

    /// Replace every world (and go back to the first).
    pub fn set_worlds(&self, worlds: Vec<WinWorld>) {
        let mut state = self.lock();
        state.worlds = worlds;
        state.current = 0;
    }

    pub fn calls(&self) -> Vec<Win32Call> {
        self.lock().calls.clone()
    }

    /// Only the spawns, in order.
    pub fn spawns(&self) -> Vec<SpawnRequest> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Win32Call::Spawn(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Only the focus requests, in order.
    pub fn focuses(&self) -> Vec<FocusRequest> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Win32Call::Focus(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    fn read<T>(
        &self,
        call: Win32Call,
        pick: impl Fn(&WinWorld) -> Result<T, Win32Error>,
    ) -> Result<T, Win32Error> {
        let mut state = self.lock();
        state.calls.push(call);
        let index = state.current.min(state.worlds.len().saturating_sub(1));
        state
            .worlds
            .get(index)
            .map_or(Err(Win32Error::new(1)), pick)
    }

    fn advance(state: &mut State) {
        if state.current + 1 < state.worlds.len() {
            state.current += 1;
        }
    }
}

fn matches_identity(world: &WinWorld, request: &FocusRequest) -> bool {
    let Ok(table) = &world.processes else {
        return false;
    };
    let live = |pid: u32, created: u64| {
        table
            .iter()
            .any(|p| p.pid == pid && p.created == Some(created))
    };
    let window = world.windows.as_ref().ok().and_then(|ws| {
        ws.iter()
            .find(|w| w.hwnd == request.hwnd && w.owner_pid == request.owner_pid)
    });
    window.is_some()
        && live(request.owner_pid, request.owner_created)
        && live(request.process_pid, request.process_created)
}

impl Win32 for ScriptedWin32 {
    fn boot(&self) -> Result<WinBoot, Win32Error> {
        self.read(Win32Call::Boot, |w| w.boot)
    }

    fn processes(&self) -> Result<Vec<RawWinProcess>, Win32Error> {
        self.read(Win32Call::Processes, |w| w.processes.clone())
    }

    fn windows(&self) -> Result<Vec<RawWindow>, Win32Error> {
        self.read(Win32Call::Windows, |w| w.windows.clone())
    }

    fn foreground(&self) -> Result<Option<RawWindow>, Win32Error> {
        self.read(Win32Call::Foreground, |w| w.foreground)
    }

    fn focus(&self, request: &FocusRequest) -> Result<FocusResult, Win32Error> {
        let mut state = self.lock();
        state.calls.push(Win32Call::Focus(*request));
        let scripted = state.focus?;
        let index = state.current.min(state.worlds.len().saturating_sub(1));
        let live = state
            .worlds
            .get(index)
            .is_some_and(|w| matches_identity(w, request));
        if !live {
            return Ok(FocusResult::Gone);
        }
        if scripted == FocusResult::Focused {
            Self::advance(&mut state);
        }
        Ok(scripted)
    }

    fn default_terminal_delegation(&self) -> Result<WinDelegation, Win32Error> {
        let mut state = self.lock();
        state.calls.push(Win32Call::Delegation);
        state.delegation.clone()
    }

    fn terminal_package(&self) -> Result<WinTerminalPackage, Win32Error> {
        let mut state = self.lock();
        state.calls.push(Win32Call::TerminalPackage);
        state.terminal.clone()
    }

    fn user_environment(&self) -> Result<Vec<(String, String)>, Win32Error> {
        let mut state = self.lock();
        state.calls.push(Win32Call::UserEnvironment);
        state.environment.clone()
    }

    fn spawn(&self, request: &SpawnRequest) -> Result<(), Win32Error> {
        let mut state = self.lock();
        state.calls.push(Win32Call::Spawn(request.clone()));
        state.spawn?;
        Self::advance(&mut state);
        Ok(())
    }
}
