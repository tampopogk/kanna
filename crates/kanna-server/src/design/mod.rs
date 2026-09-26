//! App Design: design-first workflows (docs/specs/app-design.md).
//!
//! A design task keeps one live session and workspace from its first design
//! position to the hand-off (§3). This module owns what that session works
//! on beside its terminal: the live document ([`document`]).

pub(crate) mod document;
