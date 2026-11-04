//! What Tap brings in from elsewhere and sends out (docs/PLAN.md §4, §7): so far, a whole
//! profile as one offline file (Search #845), and Search's own profile on this Mac packed
//! as one. No AppKit, no WebKit: everything here runs
//! under plain `cargo test`.

#![deny(unsafe_code)]

pub mod from_search;
pub mod profile;
