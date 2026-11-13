//! The account's service callers (`docs/service-callers.md`): the
//! callers and the core host they read, made with the account, tied to
//! the core when it attaches and watched once the daemon runs.

use crate::service_callers::host::CoreHost;
use crate::service_callers::ServiceCallers;
use agent_relay_node::relay::daemon::{Core, SpawnCallers};
use std::sync::Arc;

pub(crate) struct Callers {
    host: Arc<CoreHost>,
    pub(crate) callers: Arc<ServiceCallers>,
}

impl Callers {
    pub(crate) fn new() -> Callers {
        let host = Arc::new(CoreHost::default());
        let callers = Arc::new(ServiceCallers::new(host.clone()));
        Callers { host, callers }
    }

    /// Ties the host to the core the account serves.
    pub(crate) fn attach(&self, core: &Arc<Core>) {
        self.host.attach(core);
    }

    /// Starts the callers' watch for the life of the daemon.
    pub(crate) fn watch(&self) {
        tokio::spawn(self.callers.clone().watch());
    }

    /// The core's spawn-callers plugin.
    pub(crate) fn plugin(&self) -> Arc<dyn SpawnCallers> {
        self.callers.clone()
    }
}
