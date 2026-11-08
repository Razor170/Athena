//! Mining Supervisor.
//!
//! This folder holds the module's core logic, which does not depend on the UI:
//!
//! - [`gamelog`]: reads EVE game log files and turns mining lines into events.
//! - [`session`]: adds those events up into per-character mining sessions.
//! - [`history`]: one History entry per "End session" click, with all pilots.
//! - [`prices`]: Jita prices from ESI (refreshed hourly) and ore values.
//! - [`scan`]: parses pasted Mining Survey Scanner results and works out how
//!   fast a belt is being mined.
//!
//! The dashboard panel (the `Module` impl) comes in a later PR, once Kevin has
//! approved the mockup.
//!
//! Notes for wiring this up:
//! - Pass EVE time (UTC) to `CharacterTracker::tick`, never local time.
//! - On startup Athena reads today's logs from the start. During that
//!   catch-up, don't show alerts, and skip finished sessions that are already
//!   in the history (same character and start time), or they are stored twice.
//! - The hold contents must be saved across restarts, since an unload at a
//!   station leaves no trace in the log.

// Nothing outside the tests uses this code yet, so the compiler would warn
// about "dead code". Remove this line when the panel is wired in.
#![allow(dead_code)]

pub mod gamelog;
pub mod history;
pub mod prices;
pub mod scan;
pub mod session;
