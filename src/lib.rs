//! # Sparrow embedding fork
//!
//! Use [`solve`] with [`SolveProblem`], [`SolveSettings`], and a fresh [`SolveControl`].
//! Always inspect [`SolveOutcome::status`] and [`SolveOutcome::best_solution`] independently.
//! The complete application integration guide, including coordinate conversion, budgets,
//! progress ownership, cancellation, and warm starts, is available in [`solver`].
//! Lower-level optimizer and IO modules remain for compatibility and maintainer tooling.
#![cfg_attr(feature = "simd", feature(portable_simd))]
#![allow(const_item_mutation)]
#![allow(unused_imports)]

use jagua_rs::Instant;
use numfmt::{Formatter, Precision, Scales};
use std::sync::LazyLock;

pub mod config;
pub mod consts;
pub mod eval;
pub mod fixed_sheet;
pub mod optimizer;
pub mod quantify;
pub mod sample;
pub mod solver;
pub mod util;

pub use config::SparrowConfig;
pub use jagua_rs;
pub use solver::{
    BudgetMode, CompletionReason, ProgressEvent, SolveControl, SolveError, SolveOutcome,
    SolvePhase, SolveProblem, SolveSettings, SolveStatistics, SolveStatus, solve,
};

pub static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

static FMT: fn() -> Formatter = || -> Formatter {
    Formatter::new()
        .scales(Scales::short())
        .precision(Precision::Significance(3))
};

#[cfg(feature = "live_svg")]
pub const EXPORT_LIVE_SVG: bool = true;

#[cfg(not(feature = "live_svg"))]
pub const EXPORT_LIVE_SVG: bool = false;

#[cfg(feature = "only_final_svg")]
pub const EXPORT_ONLY_FINAL_SVG: bool = true;

#[cfg(not(feature = "only_final_svg"))]
pub const EXPORT_ONLY_FINAL_SVG: bool = false;

#[cfg(all(feature = "live_svg", feature = "only_final_svg"))]
compile_error!("The features `live_svg` and `only_final_svg` are mutually exclusive.");

// Compile README examples without copying repository-relative navigation into generated docs.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme_examples {}
