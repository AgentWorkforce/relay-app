//! Stopping the app while preserving its next-login startup choice.
use super::*;

impl App {
    /// Stops following the core. The core keeps agents connected after
    /// the app quits unless the person turned that off.
    pub fn shutdown(&self) {
        let keep_core = self.prefs().keeps_agents_connected_after_quit();
        self.inner
            .runtime
            .block_on(self.inner.bridge.quit(keep_core));
        if !keep_core {
            // The explicit quit preference stops this session's service,
            // while Open at login still governs the next login.
            let service = self.inner.service.clone();
            self.inner.runtime.block_on(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    if service.state().active {
                        service.stop()
                    } else {
                        Ok(())
                    }
                })
                .await;
            });
        }
    }
}
