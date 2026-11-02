//! Muxscape application core, independent of any UI toolkit.
//!
//! Modules map to the product's layers:
//! * [`hosts`], [`ssh_config`], [`vault`] — SSH host & credential management
//! * `mux_ssh` (crate) — SSH connection lifecycle and transport
//! * [`control`], [`remote`] — tmux control-mode integration
//! * [`attachment`] — session restoration and state synchronisation
//! * [`ports`] — what's listening on a server and what it serves
//! * [`tunnels`] — port forwarding and "a server just started" detection
//! * [`core`], [`api`] — connection manager and the UI contract
//!   (rendering/workspace lives in the frontend)

pub mod api;
pub mod attachment;
pub mod control;
pub mod core;
pub mod hosts;
pub mod ports;
pub mod remote;
pub mod ssh_config;
pub mod tunnels;
pub mod vault;

pub use crate::core::Core;
