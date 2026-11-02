//! I/O-free implementation of the tmux control-mode protocol: framing and
//! notification parsing ([`protocol`]), wire escaping ([`escape`]), layout
//! trees ([`layout`]), command builders and query parsers ([`commands`]) and
//! a reducer mirroring one attached session ([`model`]).

pub mod commands;
pub mod escape;
pub mod ids;
pub mod layout;
pub mod model;
pub mod protocol;
pub mod snapshot;

pub use ids::{PaneId, SessionId, WindowId};
pub use layout::{CellKind, LayoutCell};
pub use model::{Effect, SessionModel};
pub use protocol::{Event, Notification, Parser, Response};
