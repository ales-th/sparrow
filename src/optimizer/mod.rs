use crate::config::*;
use crate::consts::LBF_SAMPLE_CONFIG;
use crate::optimizer::compress::compression_phase;
use crate::optimizer::explore::exploration_phase;
use crate::optimizer::lbf::LBFBuilder;
use crate::optimizer::separator::Separator;
use crate::util::listener::{OptimizationPhase, ReportType, SolutionListener};
use crate::util::terminator::Terminator;
use jagua_rs::geometry::geo_enums::RotationRange;
use jagua_rs::geometry::geo_traits::TransformableFrom;
use jagua_rs::geometry::Transformation;
use jagua_rs::probs::spp::entities::{SPInstance, SPProblem, SPSolution};
use log::info;
use rand::{Rng, SeedableRng};
use std::time::Duration;
use rand::rngs::Xoshiro256PlusPlus;

pub mod lbf;
pub mod separator;
mod worker;
pub mod explore;
pub mod compress;

/// Low-level compatibility entry point. Prefer [`crate::solve`] for new application code.
///
/// Callers own import/configuration validation, RNG, listener and terminator setup.
/// This returns an error instead of an outcome retaining the best solution on failure,
/// and uses phase/search-only budgets. See [`crate::solver`] for the application contract.
///
/// Algorithm 11 from https://doi.org/10.48550/arXiv.2509.13329
///
/// Returns a construction error if no initial solution is supplied and the
/// initial-placement heuristic cannot build one within its strip growth limit.
pub fn optimize(
    instance: SPInstance,
    rng: Xoshiro256PlusPlus,
    sol_listener: &mut impl SolutionListener,
    terminator: &mut impl Terminator,
    expl_config: &ExplorationConfig,
    cmpr_config: &CompressionConfig,
    initial_solution: Option<&SPSolution>
) -> anyhow::Result<SPSolution> {
    optimize_with(instance, rng, sol_listener, terminator, expl_config, cmpr_config, initial_solution)
        .map_err(|error| match error {
            crate::solver::SolveError::InvalidSettings(message) => anyhow::anyhow!(message),
            crate::solver::SolveError::Input(source)
            | crate::solver::SolveError::WarmStart(source)
            | crate::solver::SolveError::Construction(source)
            | crate::solver::SolveError::RuntimeSetup(source) => source,
        })?
        .ok_or_else(|| anyhow::anyhow!("optimization stopped before initial placement completed"))
}

/// Shared orchestration for legacy callers and the embedding facade.
pub(crate) fn optimize_with(
    instance: SPInstance,
    mut rng: Xoshiro256PlusPlus,
    sol_listener: &mut impl SolutionListener,
    terminator: &mut impl Terminator,
    expl_config: &ExplorationConfig,
    cmpr_config: &CompressionConfig,
    initial_solution: Option<&SPSolution>,
) -> Result<Option<SPSolution>, crate::solver::SolveError> {
    use crate::solver::SolveError;
    let mut next_rng = || Xoshiro256PlusPlus::seed_from_u64(rng.next_u64());
    let start_prob = match initial_solution {
        None => {
            if terminator.run_stopped() { return Ok(None); }
            let builder = LBFBuilder::new(instance, next_rng(), LBF_SAMPLE_CONFIG)
                .map_err(SolveError::Construction)?
                .construct_with(terminator, sol_listener).map_err(SolveError::Construction)?;
            match builder { Some(builder) => builder.prob, None => return Ok(None) }
        }
        Some(initial) => {
            let initial = crate::solver::validate_initial(&instance, initial).map_err(SolveError::WarmStart)?;
            let mut prob = SPProblem::new(instance).map_err(SolveError::Construction)?;
            prob.restore(&initial);
            prob
        }
    };
    let initial = start_prob.save();
    sol_listener.report_initial_solution(&initial);
    if terminator.run_stopped() {
        sol_listener.report(ReportType::Final, &initial);
        return Ok(Some(initial));
    }

    sol_listener.report_phase(OptimizationPhase::Exploration);
    if terminator.run_stopped() {
        sol_listener.report(ReportType::Final, &initial);
        return Ok(Some(initial));
    }
    terminator.new_timeout(expl_config.time_limit);
    let mut expl_separator = Separator::try_new(start_prob, next_rng(), expl_config.separator_config)
        .map_err(SolveError::RuntimeSetup)?;
    let solutions = exploration_phase(&mut expl_separator, sol_listener, terminator, expl_config);
    let final_explore_sol = solutions.into_iter().last().expect("initial solution is always retained");
    if terminator.run_stopped() {
        sol_listener.report(ReportType::Final, &final_explore_sol);
        return Ok(Some(final_explore_sol));
    }

    sol_listener.report_phase(OptimizationPhase::Compression);
    if terminator.run_stopped() {
        sol_listener.report(ReportType::Final, &final_explore_sol);
        return Ok(Some(final_explore_sol));
    }
    terminator.new_timeout(cmpr_config.time_limit);
    let mut config = *cmpr_config;
    // Time-based compression must use its effective allocation in whole-run mode.
    config.time_limit = terminator.phase_time_limit(cmpr_config.time_limit);
    let mut cmpr_separator = Separator::try_new(expl_separator.prob, next_rng(), config.separator_config)
        .map_err(SolveError::RuntimeSetup)?;
    let solution = compression_phase(&mut cmpr_separator, &final_explore_sol, sol_listener, terminator, &config);
    sol_listener.report(ReportType::Final, &solution);
    Ok(Some(solution))
}

/// Conservative lower bound on strip width, including the container's inset.
/// Leave relative slack for f32 rotation and bounding-box rounding near exact fits.
fn strip_width_lower_bound(prob: &SPProblem) -> f32 {
    const REL_TOL: f32 = 1e-6;
    let container = prob.layout().container().outer_cd().bbox();
    let width_inset = prob.strip_width() - container.width();
    let area: f64 = prob.instance().items.iter()
        .map(|(item, qty)| f64::from(item.shape_cd().area()) * *qty as f64)
        .sum();
    let mut width_lower_bound = (area / f64::from(container.height())) as f32 * (1.0 - REL_TOL);

    for (item, _) in &prob.instance().items {
        let rotations = match item.allowed_orientations().rotations() {
            RotationRange::None => &[0.0][..],
            RotationRange::Discrete(rotations) => rotations.as_slice(),
            // ponytail: continuous rotations use only the area bound; add exact rotational bounds if this is too weak.
            RotationRange::Continuous => continue,
        };
        let tolerance = item.shape_cd().diameter() * REL_TOL;
        let mut shape = item.shape_cd().as_ref().clone();
        let item_width = rotations.iter()
            .filter_map(|&rotation| {
                let bbox = shape.transform_from(item.shape_cd().as_ref(), &Transformation::from_rotation(rotation)).bbox();
                (bbox.height() <= container.height() + tolerance)
                    .then_some((bbox.width() - tolerance).max(0.0))
            })
            .fold(f32::INFINITY, f32::min);
        width_lower_bound = width_lower_bound.max(item_width);
    }
    width_lower_bound + width_inset
}
