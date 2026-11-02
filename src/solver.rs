//! Synchronous, side-effect-free embedding API. See `examples/` for usage.
#![doc = include_str!("../docs/embedding.md")]
use crate::config::{ShrinkDecayStrategy, SparrowConfig};
use crate::util::{
    listener::{
        OptimizationPhase, ReportType, SeparationProgress, SeparationResult, SolutionListener,
    },
    terminator::{StopSignal, Terminator},
};
use jagua_rs::probs::spp::{
    entities::{SPInstance, SPSolution},
    io::{
        self,
        ext_repr::{ExtSPInstance, ExtSPSolution},
    },
};
use jagua_rs::{Instant, entities::Layout, io::import::Importer};
use rand::{SeedableRng, rngs::Xoshiro256PlusPlus};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// Input geometry uses Jagua's types. Imported instances must use compatible geometry settings.
pub enum SolveProblem<'a> {
    /// Import external/original geometry with the supplied settings. Recommended for applications.
    External {
        instance: &'a ExtSPInstance,
        initial_solution: Option<&'a ExtSPSolution>,
    },
    /// Use prepared geometry as-is; changing import settings does not rebuild it.
    Imported {
        instance: SPInstance,
        initial_solution: Option<&'a SPSolution>,
    },
}

/// Selects which work consumes the time limit. Defaults to whole-run timing in settings.
#[derive(Debug, Clone, Copy)]
pub enum BudgetMode {
    /// Includes import, construction, callbacks, and search. Checked cooperatively.
    WholeRun(Duration),
    /// Uses the configuration's separate exploration and compression durations.
    SearchOnly,
}

/// Per-job options. Prefer defaults, then set a budget, full-width seed, and both worker counts.
/// All fields are validated, even when unused by the selected budget/input mode.
#[derive(Debug, Clone, Copy)]
pub struct SolveSettings {
    /// Algorithm tuning and external-geometry import settings. Import settings do not rebuild prepared geometry.
    pub config: SparrowConfig,
    /// Overrides `config.rng_seed`, without truncating to the platform's usize.
    pub seed: Option<u64>,
    /// Defaults to 600 seconds including initialization. Whole-run mode overrides phase durations.
    pub budget: BudgetMode,
    /// Opt in to visualization snapshots, which may be infeasible. Defaults to false.
    pub live_candidates: bool,
}
impl Default for SolveSettings {
    fn default() -> Self {
        Self {
            config: SparrowConfig::default(),
            seed: None,
            budget: BudgetMode::WholeRun(Duration::from_secs(600)),
            live_candidates: false,
        }
    }
}

/// Clone to request interruption from another thread. Use a fresh handle per run.
#[derive(Clone, Default)]
pub struct SolveControl {
    cancelled: Arc<AtomicBool>,
    skip: Arc<AtomicBool>,
}
impl SolveControl {
    /// Create fresh flags for one job. Clones share these flags; they do not create new jobs.
    pub fn new() -> Self {
        Self::default()
    }
    /// Request cooperative whole-run cancellation. This flag is never reset.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    /// Whether cancellation was requested, not whether the worker has finished.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    /// Advances exploration to compression. During initialization, the request is retained.
    /// Requests made during compression have no effect.
    pub fn skip_to_compression(&self) {
        self.skip.store(true, Ordering::Relaxed);
    }
}

/// Current stage; initialization can consume the entire budget before a solution exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolvePhase {
    /// Validation, import, warm-start preparation, and initial placement.
    Initialization,
    /// Search across layouts and widths.
    Exploration,
    /// Compress the best exploration layout.
    Compression,
}
/// Why a normally completed run stopped. Neither reason proves optimality or a solution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionReason {
    /// Whole-run deadline expired, or at least one search-only phase exhausted its allocation.
    BudgetExhausted,
    /// The heuristic stopped without exhausting the applicable budget.
    AlgorithmStopped,
}
/// Job status, independent of whether a valid solution is retained in the outcome.
#[derive(Debug)]
pub enum SolveStatus {
    /// Normal stop; a budget may have expired before a complete solution existed.
    Completed(CompletionReason),
    /// Whole-job cancellation was requested before the outcome was finalized.
    Cancelled,
    /// Recoverable failure; inspect the outcome's best solution separately.
    Failed(SolveError),
}

/// Search counters exclude import and initial placement; unavailable measurements are `None`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SolveStatistics {
    /// Number of items placed during initialization; not a feasibility guarantee.
    pub placed_items: usize,
    /// Total quantity demanded by the imported instance.
    pub required_items: usize,
    /// Cumulative separator evaluations, updated at separation completion.
    pub evaluations: usize,
    /// Cumulative separator moves, updated at separation completion.
    pub moves: usize,
    /// Cumulative separator iterations, also updated within separation.
    pub iterations: usize,
    /// Last reported search width; the current candidate may be infeasible.
    pub current_width: Option<f32>,
    /// Last reported search loss, not a replacement for a best-solution event.
    pub current_loss: Option<f32>,
    /// Smallest width of a retained complete valid solution.
    pub best_width: Option<f32>,
    /// Retained solution's density as a fraction, not a percentage.
    pub best_density: Option<f32>,
}

/// Borrowed events are delivered serially on the calling thread. Clone a solution to retain it.
/// Candidate snapshots may be infeasible and must not be treated as the best valid solution.
#[derive(Debug)]
pub enum ProgressEvent<'a> {
    /// A phase transition. Callback execution counts toward the budget.
    Phase {
        phase: SolvePhase,
        elapsed: Duration,
    },
    /// Measurements available at this algorithm checkpoint, not a fixed-rate heartbeat.
    Statistics {
        statistics: SolveStatistics,
        elapsed: Duration,
    },
    /// Improved complete valid solution; clone it only when retaining it beyond this callback.
    BestSolution {
        solution: &'a SPSolution,
        elapsed: Duration,
    },
    /// Optional visualization snapshot. Even a feasible candidate need not improve the best.
    Candidate {
        solution: &'a SPSolution,
        feasible: bool,
        elapsed: Duration,
    },
    /// Exactly one terminal event per normal return. Panic/unrecoverable exit may prevent it.
    Finished { outcome: &'a SolveOutcome },
}

/// Final job state. Always inspect status and solution availability independently.
#[must_use = "Inspect status and best_solution; an error or cancellation can retain a valid solution"]
#[derive(Debug)]
pub struct SolveOutcome {
    /// Completion, cancellation, or a structured error.
    pub status: SolveStatus,
    /// Best complete valid layout, if found. Internal placements use preprocessed coordinates.
    /// Use [`Self::export_solution`] for transformations of original input shapes.
    pub best_solution: Option<SPSolution>,
    /// Actual RNG seed, or None if the job ended before RNG initialization.
    pub seed: Option<u64>,
    /// Time from API entry to outcome creation; excludes the terminal callback.
    pub elapsed: Duration,
    /// Last available measurements, including counters from completed separation work.
    pub statistics: SolveStatistics,
}
impl SolveOutcome {
    /// Export original-shape placements with this call's elapsed time in whole seconds.
    /// Returns None when no complete solution exists. Does not write a file or use global EPOCH.
    pub fn export_solution(&self) -> Option<ExtSPSolution> {
        self.best_solution.as_ref().map(|solution| {
            let mut exported = io::export(solution, solution.time_stamp);
            exported.run_time_sec = self.elapsed.as_secs();
            exported
        })
    }
}

/// Recoverable failure category. Wrapped errors preserve their source chain.
#[derive(Debug)]
pub enum SolveError {
    /// Invalid tuning, geometry configuration, workers, or unsupported duration arithmetic.
    InvalidSettings(String),
    /// Invalid external input or prepared instance.
    Input(anyhow::Error),
    /// Incomplete, incompatible, or infeasible warm start.
    WarmStart(anyhow::Error),
    /// Initial placement failed; not proof of problem infeasibility.
    Construction(anyhow::Error),
    /// Runtime resources such as the local worker pool could not be created.
    RuntimeSetup(anyhow::Error),
}
impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSettings(message) => write!(f, "invalid settings: {message}"),
            Self::Input(source) => write!(f, "invalid input: {source}"),
            Self::WarmStart(source) => write!(f, "invalid warm start: {source}"),
            Self::Construction(source) => write!(f, "initial placement failed: {source}"),
            Self::RuntimeSetup(source) => write!(f, "runtime setup failed: {source}"),
        }
    }
}
impl std::error::Error for SolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidSettings(_) => None,
            Self::Input(e) | Self::WarmStart(e) | Self::Construction(e) | Self::RuntimeSetup(e) => {
                Some(e.as_ref())
            }
        }
    }
}

impl SolveSettings {
    /// Checks tuning and geometry settings before any problem import or worker creation.
    pub fn validate(&self) -> Result<(), SolveError> {
        let c = &self.config;
        let check = |ok: bool, message: &str| {
            if ok {
                Ok(())
            } else {
                Err(SolveError::InvalidSettings(message.into()))
            }
        };
        let unit = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
        check(
            c.expl_cfg.shrink_step.is_finite()
                && c.expl_cfg.shrink_step > 0.0
                && c.expl_cfg.shrink_step < 1.0,
            "exploration shrink_step must be in (0, 1)",
        )?;
        let (max, min) = c.cmpr_cfg.shrink_range;
        check(
            max.is_finite() && min.is_finite() && min > 0.0 && min <= max && max < 1.0,
            "compression shrink_range must satisfy 0 < min <= max < 1",
        )?;
        if let ShrinkDecayStrategy::FailureBased(r) = c.cmpr_cfg.shrink_decay {
            check(
                r.is_finite() && r > 0.0 && r < 1.0,
                "failure decay must be in (0, 1)",
            )?;
        }
        check(
            c.expl_cfg.solution_pool_distribution_stddev.is_finite()
                && c.expl_cfg.solution_pool_distribution_stddev > 0.0,
            "solution pool standard deviation must be positive and finite",
        )?;
        check(
            unit(c.expl_cfg.large_item_ch_area_cutoff_percentile),
            "large-item percentile must be in [0, 1]",
        )?;
        check(
            c.expl_cfg.max_conseq_failed_attempts != Some(0),
            "failure-attempt limit must be positive",
        )?;
        for sep in [c.expl_cfg.separator_config, c.cmpr_cfg.separator_config] {
            check(
                sep.n_workers > 0 && sep.n_workers <= rayon::max_num_threads(),
                "worker count must be within Rayon's supported range",
            )?;
            check(
                sep.iter_no_imprv_limit > 0 && sep.strike_limit > 0,
                "iteration and strike limits must be positive",
            )?;
            check(
                sep.sample_config.n_container_samples > 0 && sep.sample_config.n_coord_descents > 0,
                "container samples and coordinate descents must be positive",
            )?;
        }
        // Bound the exponentially sized quadtree to the documented practical range.
        check(
            c.cde_config.quadtree_depth <= 10,
            "quadtree depth must be at most 10",
        )?;
        let sur = c.cde_config.item_surrogate_config;
        check(
            unit(sur.ff_pole_area_ratio) && sur.n_pole_limits.iter().all(|(_, ratio)| unit(*ratio)),
            "surrogate area ratios must be in [0, 1]",
        )?;
        for value in c.poly_simpl_tolerance.into_iter().chain(
            c.narrow_concavity_cutoff_ratio
                .into_iter()
                .flat_map(|(a, b)| [a, b]),
        ) {
            check(
                value.is_finite() && value >= 0.0,
                "geometry tolerances must be finite and nonnegative",
            )?;
        }
        let now = Instant::now();
        for duration in [c.expl_cfg.time_limit, c.cmpr_cfg.time_limit]
            .into_iter()
            .chain(match self.budget {
                BudgetMode::WholeRun(d) => Some(d),
                BudgetMode::SearchOnly => None,
            })
        {
            check(
                now.checked_add(duration).is_some() && duration.as_secs_f32().is_finite(),
                "time budget exceeds supported duration arithmetic",
            )?;
        }
        Ok(())
    }
}

/// Solve without files, signal handlers, terminal setup, or global logger initialization.
/// A fixed seed does not imply identical results under wall-clock time limits.
/// Callback panics propagate to the caller; they are not converted into failed outcomes.
///
/// This blocks the caller. In a UI or async application, schedule it on a background/blocking
/// worker and forward owned updates. Use a fresh [`SolveControl`] per job. A completed job
/// can have no solution; inspect status and `best_solution` separately.
///
/// # Panics
/// User callback panics propagate and can prevent the terminal event. Unrecoverable failures
/// such as an out-of-memory abort are outside the returned-outcome guarantee.
pub fn solve(
    problem: SolveProblem<'_>,
    settings: SolveSettings,
    mut on_progress: impl FnMut(ProgressEvent<'_>),
    control: &SolveControl,
) -> SolveOutcome {
    let start = Instant::now();
    let mut bridge = ProgressBridge::new(start, settings.live_candidates, &mut on_progress);
    bridge.phase(SolvePhase::Initialization);
    let mut seed = None;
    let mut term = RunTerminator::new(start, settings.budget, control.clone());
    let result = (|| {
        settings.validate()?;
        if term.run_stopped() {
            return Ok(None);
        }
        let (instance, initial) = match problem {
            SolveProblem::External {
                instance,
                initial_solution,
            } => {
                validate_external(instance).map_err(SolveError::Input)?;
                let c = settings.config;
                let importer = Importer::new(
                    c.cde_config,
                    c.poly_simpl_tolerance,
                    c.narrow_concavity_cutoff_ratio,
                );
                let imported =
                    io::import_instance(&importer, instance).map_err(SolveError::Input)?;
                if term.run_stopped() {
                    return Ok(None);
                }
                let initial = initial_solution
                    .map(|s| import_warm_start(&imported, s))
                    .transpose()
                    .map_err(SolveError::WarmStart)?;
                (imported, initial)
            }
            SolveProblem::Imported {
                instance,
                initial_solution,
            } => {
                validate_instance(&instance).map_err(SolveError::Input)?;
                let initial = initial_solution
                    .map(|s| validate_initial(&instance, s))
                    .transpose()
                    .map_err(SolveError::WarmStart)?;
                (instance, initial)
            }
        };
        validate_instance(&instance).map_err(SolveError::Input)?;
        bridge.report_initialization(
            initial
                .as_ref()
                .map_or(0, |s| s.layout_snapshot().placed_items().len()),
            instance.total_item_qty(),
        );
        if let Some(s) = &initial {
            bridge.report(ReportType::ExplFeas, s);
        }
        if term.run_stopped() {
            return Ok(None);
        }
        let actual_seed = settings
            .seed
            .or(settings.config.rng_seed.map(|s| s as u64))
            .unwrap_or_else(rand::random);
        seed = Some(actual_seed);
        crate::optimizer::optimize_with(
            instance,
            Xoshiro256PlusPlus::seed_from_u64(actual_seed),
            &mut bridge,
            &mut term,
            &settings.config.expl_cfg,
            &settings.config.cmpr_cfg,
            initial.as_ref(),
        )
    })();
    let status = match result {
        Err(error) => SolveStatus::Failed(error),
        Ok(_) if control.is_cancelled() => SolveStatus::Cancelled,
        Ok(_) => SolveStatus::Completed(if term.exhausted() {
            CompletionReason::BudgetExhausted
        } else {
            CompletionReason::AlgorithmStopped
        }),
    };
    let outcome = SolveOutcome {
        status,
        best_solution: bridge.best.take(),
        seed,
        elapsed: start.elapsed(),
        statistics: bridge.statistics,
    };
    (bridge.callback)(ProgressEvent::Finished { outcome: &outcome });
    outcome
}

// Jagua validates polygon topology, but assumes finite values in several constructors.
fn validate_external(instance: &ExtSPInstance) -> anyhow::Result<()> {
    use jagua_rs::io::ext_repr::ExtShape;
    anyhow::ensure!(
        instance.strip_height.is_finite() && instance.strip_height > 0.0,
        "strip height must be positive and finite"
    );
    anyhow::ensure!(
        instance.min_item_separation.is_finite()
            && instance.min_item_separation >= 0.0
            && instance.min_item_separation < instance.strip_height,
        "invalid item separation for strip height"
    );
    let mut total = 0usize;
    for item in &instance.items {
        total = total
            .checked_add(usize::try_from(item.demand)?)
            .ok_or_else(|| anyhow::anyhow!("item demand overflow"))?;
        if item.demand == 0 {
            continue;
        }
        let vertices = match &item.base.shape {
            ExtShape::Rectangle {
                x_min,
                y_min,
                width,
                height,
            } => {
                anyhow::ensure!(
                    [
                        *x_min,
                        *y_min,
                        *width,
                        *height,
                        x_min + width,
                        y_min + height
                    ]
                    .iter()
                    .all(|v| v.is_finite())
                        && *width > 0.0
                        && *height > 0.0,
                    "invalid rectangle for item {}",
                    item.base.id
                );
                continue;
            }
            ExtShape::SimplePolygon(polygon) => &polygon.0,
            ExtShape::Polygon(polygon) => {
                anyhow::ensure!(
                    polygon.inner.is_empty(),
                    "item {} has unsupported holes",
                    item.base.id
                );
                &polygon.outer.0
            }
            ExtShape::MultiPolygon(_) => anyhow::bail!("multipolygon items are unsupported"),
        };
        anyhow::ensure!(
            vertices.iter().all(|(x, y)| x.is_finite() && y.is_finite()),
            "nonfinite coordinates for item {}",
            item.base.id
        );
    }
    anyhow::ensure!(total > 0, "positive item demand required");
    Ok(())
}

fn validate_instance(instance: &SPInstance) -> anyhow::Result<()> {
    anyhow::ensure!(
        instance.base_strip.fixed_height.is_finite()
            && instance.base_strip.fixed_height > 0.0
            && instance.base_strip.width.is_finite()
            && instance.base_strip.width > 0.0,
        "strip dimensions must be positive and finite"
    );
    let config = SparrowConfig {
        cde_config: instance.base_strip.cde_config,
        ..Default::default()
    };
    SolveSettings {
        config,
        ..Default::default()
    }
    .validate()?;
    let mut ids = std::collections::HashSet::new();
    for (idx, (item, _)) in instance.items.iter().enumerate() {
        anyhow::ensure!(
            item.idx() == idx && ids.insert(item.external_id()),
            "invalid item index or duplicate ID"
        );
        anyhow::ensure!(
            item.shape_cd().area().is_finite() && item.shape_cd().area() > 0.0,
            "item area must be positive and finite"
        );
    }
    let total = instance
        .items
        .iter()
        .try_fold(0usize, |n, (_, qty)| n.checked_add(*qty))
        .ok_or_else(|| anyhow::anyhow!("item demand overflow"))?;
    anyhow::ensure!(total > 0, "positive item demand required");
    Ok(())
}

/// Import only after demand checks: Jagua's low-level placement API panics on excess demand.
fn import_warm_start(
    instance: &SPInstance,
    external: &ExtSPSolution,
) -> anyhow::Result<SPSolution> {
    anyhow::ensure!(
        external.strip_width.is_finite() && external.strip_width > 0.0,
        "warm-start strip width must be positive and finite"
    );
    anyhow::ensure!(
        external.layout.container_id == 0,
        "warm-start container ID must be zero"
    );
    let mut quantities = vec![0usize; instance.items.len()];
    for placement in &external.layout.placed_items {
        let idx = instance
            .item_idx(placement.item_id)
            .ok_or_else(|| anyhow::anyhow!("unknown item ID {}", placement.item_id))?;
        quantities[idx] += 1;
        anyhow::ensure!(
            quantities[idx] <= instance.item_qty(idx),
            "excess demand for item {}",
            placement.item_id
        );
        // Jagua's transformation constructor rejects NaN by panicking, so check
        // external components before constructing its internal representation.
        let transform = &placement.transformation;
        anyhow::ensure!(
            transform.rotation.is_finite()
                && transform.translation.0.is_finite()
                && transform.translation.1.is_finite(),
            "nonfinite transformation for item {}",
            placement.item_id
        );
        let dt = jagua_rs::geometry::DTransformation::from(transform.clone());
        anyhow::ensure!(
            instance.item(idx).allowed_orientations().allows(&dt),
            "invalid placement orientation or translation for item {}",
            placement.item_id
        );
    }
    anyhow::ensure!(
        quantities
            .iter()
            .enumerate()
            .all(|(i, qty)| *qty == instance.item_qty(i)),
        "warm start must meet all item demands"
    );
    let solution = io::import_solution(instance, external)?;
    anyhow::ensure!(
        Layout::from_snapshot(solution.layout_snapshot()).is_collision_free(),
        "warm start is not collision-free or lies outside the strip"
    );
    Ok(solution)
}

pub(crate) fn validate_initial(
    instance: &SPInstance,
    solution: &SPSolution,
) -> anyhow::Result<SPSolution> {
    anyhow::ensure!(
        solution.strip().fixed_height == instance.base_strip.fixed_height,
        "warm-start strip height differs from instance"
    );
    import_warm_start(instance, &io::export(solution, solution.time_stamp))
}

struct RunTerminator {
    control: SolveControl,
    deadline: Option<Instant>,
    phase_deadline: Option<Instant>,
    whole_run: bool,
    phase_count: usize,
    timed_out: Arc<AtomicBool>,
}
impl RunTerminator {
    fn new(start: Instant, budget: BudgetMode, control: SolveControl) -> Self {
        Self {
            control,
            deadline: match budget {
                BudgetMode::WholeRun(d) => start.checked_add(d),
                BudgetMode::SearchOnly => None,
            },
            phase_deadline: None,
            whole_run: matches!(budget, BudgetMode::WholeRun(_)),
            phase_count: 0,
            timed_out: Arc::new(AtomicBool::new(false)),
        }
    }
    fn exhausted(&self) -> bool {
        if self.whole_run {
            self.deadline.is_some_and(|d| Instant::now() >= d)
        } else {
            self.timed_out.load(Ordering::Relaxed)
        }
    }
}
impl Terminator for RunTerminator {
    fn phase_time_limit(&self, configured: Duration) -> Duration {
        if self.whole_run {
            self.phase_deadline.map_or(Duration::ZERO, |d| {
                d.saturating_duration_since(Instant::now())
            })
        } else {
            configured
        }
    }

    fn run_stopped(&self) -> bool {
        self.control.is_cancelled() || self.deadline.is_some_and(|d| Instant::now() >= d)
    }
    fn kill(&self) -> bool {
        let now = Instant::now();
        let timeout = self.deadline.is_some_and(|d| now >= d)
            || self.phase_deadline.is_some_and(|d| now >= d);
        if timeout {
            self.timed_out.store(true, Ordering::Relaxed);
        }
        self.control.is_cancelled()
            || timeout
            || (self.phase_count == 1 && self.control.skip.load(Ordering::Relaxed))
    }
    fn new_timeout(&mut self, timeout: Duration) {
        let now = Instant::now();
        let remaining = self.deadline.map(|d| d.saturating_duration_since(now));
        let duration = if self.whole_run {
            let remaining = remaining.unwrap_or(Duration::ZERO);
            if self.phase_count > 0 {
                remaining
            } else {
                remaining.mul_f32(crate::consts::DEFAULT_EXPLORE_TIME_RATIO)
            }
        } else {
            timeout
        };
        self.phase_deadline = Some(now.checked_add(duration).unwrap_or(now));
        self.phase_count += 1;
        if self.phase_count == 2 {
            self.control.skip.store(false, Ordering::Relaxed);
        }
    }
    fn timeout_at(&self) -> Option<Instant> {
        self.phase_deadline
    }
    fn stop_signal(&self) -> Option<StopSignal> {
        let control = self.control.clone();
        let deadline = self.deadline;
        let phase_deadline = self.phase_deadline;
        let skip_enabled = self.phase_count == 1;
        let timed_out = self.timed_out.clone();
        Some(Arc::new(move || {
            let now = Instant::now();
            let timeout =
                deadline.is_some_and(|d| now >= d) || phase_deadline.is_some_and(|d| now >= d);
            if timeout {
                timed_out.store(true, Ordering::Relaxed);
            }
            control.is_cancelled()
                || timeout
                || (skip_enabled && control.skip.load(Ordering::Relaxed))
        }))
    }
}

struct ProgressBridge<'a, F> {
    start: Instant,
    live_candidates: bool,
    callback: &'a mut F,
    best: Option<SPSolution>,
    statistics: SolveStatistics,
    iteration_base: usize,
}
impl<'a, F: FnMut(ProgressEvent<'_>)> ProgressBridge<'a, F> {
    fn new(start: Instant, live_candidates: bool, callback: &'a mut F) -> Self {
        Self {
            start,
            live_candidates,
            callback,
            best: None,
            statistics: SolveStatistics::default(),
            iteration_base: 0,
        }
    }
    fn phase(&mut self, phase: SolvePhase) {
        (self.callback)(ProgressEvent::Phase {
            phase,
            elapsed: self.start.elapsed(),
        });
    }
    fn stats(&mut self) {
        (self.callback)(ProgressEvent::Statistics {
            statistics: self.statistics,
            elapsed: self.start.elapsed(),
        });
    }
}
impl<F: FnMut(ProgressEvent<'_>)> SolutionListener for ProgressBridge<'_, F> {
    fn report_initial_solution(&mut self, solution: &SPSolution) {
        self.report(ReportType::ExplFeas, solution);
    }
    fn wants_candidates(&self) -> bool {
        self.live_candidates
    }
    fn report_initialization(&mut self, placed: usize, required: usize) {
        self.statistics.placed_items = placed;
        self.statistics.required_items = required;
        self.stats();
    }
    fn report(&mut self, report: ReportType, solution: &SPSolution) {
        let feasible = matches!(
            report,
            ReportType::ExplFeas | ReportType::CmprFeas | ReportType::Final
        );
        if feasible
            && self
                .best
                .as_ref()
                .is_none_or(|b| solution.strip_width() < b.strip_width())
        {
            self.statistics.best_width = Some(solution.strip_width());
            self.statistics.best_density = Some(solution.density());
            self.best = Some(solution.clone());
            (self.callback)(ProgressEvent::BestSolution {
                solution,
                elapsed: self.start.elapsed(),
            });
        } else if self.live_candidates && report != ReportType::Final {
            (self.callback)(ProgressEvent::Candidate {
                solution,
                feasible,
                elapsed: self.start.elapsed(),
            });
        }
    }
    fn report_phase(&mut self, phase: OptimizationPhase) {
        self.phase(match phase {
            OptimizationPhase::Exploration => SolvePhase::Exploration,
            OptimizationPhase::Compression => SolvePhase::Compression,
        });
    }
    fn report_separation_progress(&mut self, progress: SeparationProgress) {
        self.statistics.current_width = Some(progress.strip_width);
        self.statistics.current_loss = Some(progress.min_loss);
        self.statistics.iterations = self.iteration_base + progress.iteration;
        self.stats();
    }
    fn report_separation_result(&mut self, result: SeparationResult) {
        self.statistics.evaluations += result.total_evals;
        self.statistics.moves += result.total_moves;
        self.iteration_base = self.statistics.iterations;
        self.stats();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem() -> ExtSPInstance {
        serde_json::from_value(serde_json::json!({"name":"unit", "strip_height":10,
            "items":[{"id":17,"demand":2,
                "orientation":{"rotation":{"mode":"discrete","angles":[0]}},
                "shape":{"type":"simple_polygon","data":[[0,0],[3,0],[3,2],[0,2]]}}]
        }))
        .unwrap()
    }
    fn settings() -> SolveSettings {
        let mut s = SolveSettings {
            seed: Some(42),
            budget: BudgetMode::SearchOnly,
            ..Default::default()
        };
        s.config.expl_cfg.time_limit = Duration::ZERO;
        s.config.cmpr_cfg.time_limit = Duration::ZERO;
        s.config.expl_cfg.separator_config.n_workers = 1;
        s.config.cmpr_cfg.separator_config.n_workers = 1;
        s
    }

    #[test]
    fn failure_after_construction_retains_best_solution() {
        let problem = problem();
        let mut terminals = 0;
        let out = solve(
            SolveProblem::External {
                instance: &problem,
                initial_solution: None,
            },
            settings(),
            |event| match event {
                ProgressEvent::Phase {
                    phase: SolvePhase::Compression,
                    ..
                } => {
                    crate::optimizer::separator::FAIL_NEXT_SETUP.with(|fail| fail.set(true));
                }
                ProgressEvent::Finished { .. } => terminals += 1,
                _ => {}
            },
            &SolveControl::new(),
        );
        assert!(matches!(
            out.status,
            SolveStatus::Failed(SolveError::RuntimeSetup(_))
        ));
        assert_eq!(terminals, 1);
        let best = out.best_solution.unwrap();
        assert_eq!(best.layout_snapshot().placed_items().len(), 2);
        assert!(Layout::from_snapshot(best.layout_snapshot()).is_collision_free());
    }

    #[test]
    fn phase_deadlines_and_global_cancel_are_independent() {
        let control = SolveControl::new();
        let mut term = RunTerminator::new(
            Instant::now(),
            BudgetMode::WholeRun(Duration::from_secs(100)),
            control.clone(),
        );
        term.new_timeout(Duration::ZERO);
        control.skip_to_compression();
        assert!(term.kill());
        assert!(!term.run_stopped());
        term.new_timeout(Duration::ZERO);
        control.skip_to_compression();
        assert!(!term.kill(), "skip during compression has no effect");
        term.deadline = Some(Instant::now());
        assert!(term.run_stopped());
        assert!(term.kill());
        assert!((term.stop_signal().unwrap())());
        assert!(term.exhausted());
        control.cancel();
        term.new_timeout(Duration::from_secs(100));
        assert!(term.run_stopped(), "new phase cannot clear cancellation");
    }

    #[test]
    fn exploration_timeout_does_not_exhaust_the_whole_run_budget() {
        let mut term = RunTerminator::new(
            Instant::now(),
            BudgetMode::WholeRun(Duration::from_secs(100)),
            SolveControl::new(),
        );
        term.new_timeout(Duration::ZERO);
        assert!(term.phase_deadline < term.deadline);
        term.phase_deadline = Some(Instant::now());
        assert!(term.kill());
        assert!(!term.exhausted());
        term.new_timeout(Duration::ZERO);
        assert_eq!(term.phase_deadline, term.deadline);
        assert!(!term.kill());
    }

    #[test]
    fn live_candidates_are_opt_in_and_labelled() {
        for enabled in [false, true] {
            let p = problem();
            let mut candidates = 0;
            let mut settings = settings();
            settings.live_candidates = enabled;
            let out = solve(
                SolveProblem::External {
                    instance: &p,
                    initial_solution: None,
                },
                settings,
                |event| {
                    if let ProgressEvent::Candidate {
                        solution, feasible, ..
                    } = event
                    {
                        candidates += 1;
                        assert_eq!(
                            feasible,
                            Layout::from_snapshot(solution.layout_snapshot()).is_collision_free()
                        );
                    }
                },
                &SolveControl::new(),
            );
            assert!(out.best_solution.is_some());
            assert_eq!(candidates > 0, enabled);
        }
    }

    #[test]
    fn deadline_expiry_preserves_only_completed_initial_solutions() {
        // A callback can consume the budget; expiry must be rechecked before placing another item.
        let p = problem();
        let mut settings = settings();
        settings.budget = BudgetMode::WholeRun(Duration::from_secs(1));
        let start = Instant::now();
        let out = solve(
            SolveProblem::External {
                instance: &p,
                initial_solution: None,
            },
            settings,
            |event| {
                if let ProgressEvent::Statistics { statistics, .. } = event {
                    if statistics.placed_items == 1 {
                        while start.elapsed() < Duration::from_millis(1100) {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
            },
            &SolveControl::new(),
        );
        assert!(matches!(
            out.status,
            SolveStatus::Completed(CompletionReason::BudgetExhausted)
        ));
        assert!(out.best_solution.is_none());
    }
}
