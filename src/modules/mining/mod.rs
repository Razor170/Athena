//! Mining Supervisor.
//!
//! This folder holds the module's core logic, which does not depend on the UI:
//!
//! - [`gamelog`]: reads EVE game log files and turns mining lines into events.
//! - [`session`]: adds those events up into per-character mining sessions.
//!
//! The dashboard panel (the `Module` impl) comes in a later PR, once Kevin has
//! said what the module should show.

// Nothing outside the tests uses this code yet, so the compiler would warn
// about "dead code". Remove this line when the panel is wired in.
#![allow(dead_code)]

pub mod gamelog;
pub mod session;
