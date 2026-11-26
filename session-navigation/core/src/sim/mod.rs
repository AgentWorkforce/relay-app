//! A deterministic fake machine. `World` holds the truth; it implements
//! [`crate::ports::Observe`] by deriving snapshots through lossy channels and
//! [`crate::ports::Execute`] by mutating truth with the modelled real
//! behaviour, hazards included. [`oracle`] judges runs against truth.
pub mod agents;
pub mod apps;
pub mod codex;
pub mod defaults;
pub mod effects;
pub mod events;
pub mod execute;
pub mod gen;
pub mod grok;
pub mod hosts;
pub mod hosts_exec;
pub mod hosts_observe;
pub mod labels;
pub mod links;
pub mod observe;
pub mod oracle;
pub mod oracle_hosts;
pub mod preference;
pub mod rng;
pub mod runner;
pub mod starts;
pub mod stores;
pub mod tmux;
pub mod truth;
pub mod vars;
pub mod world;

pub use events::{Channel, Event, OpenEvent};
pub use vars::WorldVars;
pub use world::World;
