//! Host prototype of the intent shell (roadmap item S3, ADR 0002, 0007, 0011).
//!
//! The model proposes a plan; deterministic code validates, shows, asks and
//! only then runs. Everything outside `broker`, `plan` and `session` is an
//! adapter behind the traits in `ports`.

pub mod action;
pub mod broker;
pub mod host;
pub mod log;
pub mod model;
pub mod plan;
pub mod ports;
pub mod session;

#[cfg(test)]
mod testing;

#[cfg(test)]
mod pipeline_tests;
