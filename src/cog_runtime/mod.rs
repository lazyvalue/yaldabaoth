//! Capability-gated Cog runtime-delivery protocol and coordinator.
//!
//! The public library owns exact wire types and transport-independent client
//! behavior. `yalda-session-server` supplies the provider bridge and supervises
//! the coordinator so no second ACP owner is created.

pub mod coordinator;
pub mod journal;
pub mod provider;
pub mod transport;
pub mod wire;

pub use coordinator::*;
pub use journal::*;
pub use provider::*;
pub use transport::{CapabilityProbe, ClientError, CogClient, CogRuntimeTransport};
pub use wire::*;
