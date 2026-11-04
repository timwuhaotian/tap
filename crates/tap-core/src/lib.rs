//! Tap's model and logic, ported from Search (docs/PLAN.md §4, §5.1).
//!
//! No AppKit, no WebKit, no main-thread requirement: everything here runs under plain
//! `cargo test`. Foundation is used only through [`fnd`], for URL semantics identical to
//! Search's (`URL(string:)` and `URLComponents`).
//!
//! The model never touches a view or a web view. It changes its own fields, marks which
//! ones changed, and pushes [`browser::Effect`]s that the shell carries out afterwards.

#![deny(unsafe_code)]

pub mod address;
pub mod browser;
pub mod crash;
pub mod engine;
pub mod fetching;
pub mod find;
pub mod fnd;
pub mod groups;
pub mod history;
pub mod icons;
pub mod identity;
pub mod internal;
pub mod keyword;
pub mod location;
pub mod session;
pub mod settings_index;
