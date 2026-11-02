//! Muxscape application core, independent of any UI toolkit.
//!
//! Modules map to the product's layers:
//! * [`hosts`], [`ssh_config`] — SSH host & credential management
//! * `mux_ssh` (crate) — SSH connection lifecycle and transport
//! * [`control`], [`remote`] — tmux control-mode integration
//! * [`attachment`] — session restoration and state synchronisation
//! * [`core`], [`api`] — connection manager and the UI contract
//!   (rendering/workspace lives in the frontend)

pub mod api;
pub mod attachment;
pub mod control;
pub mod core;
pub mod hosts;
pub mod remote;
pub mod ssh_config;

pub use crate::core::Core;
