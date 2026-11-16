//! Host prototype of the intent shell (roadmap items S3 and S3b, ADR 0002,
//! 0007, 0011, 0014).
//!
//! The model proposes a plan; deterministic code validates, shows, asks and
//! only then runs. Everything outside `broker`, `plan` and `session` is an
//! adapter behind the traits in `ports`.

pub mod action;
pub mod broker;
pub mod host;
pub mod label;
pub mod log;
pub mod model;
pub mod plain;
pub mod plan;
pub mod ports;
pub mod reference;
pub mod resolve;
pub mod screen;
pub mod session;

#[cfg(feature = "tui")]
pub mod tui;

#[cfg(test)]
mod testing;

#[cfg(test)]
mod pipeline_tests;

#[cfg(test)]
mod resolution_tests;
