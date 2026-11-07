//! Nest outstanding demand into a finite sheet without moving locked copies.
use crate::{
    SolveControl, SparrowConfig,
    eval::{lbf_evaluator::LBFEvaluator, sample_eval::SampleEval},
    sample::search::search_placement_with_stop,
};
use anyhow::{Result, ensure};
use jagua_rs::{
    entities::{Item, Layout},
    geometry::DTransformation,
    io::{
        export::export_layout_snapshot,
        ext_repr::{ExtContainer, ExtLayout, ExtPlacedItem, ExtQualityZone, ExtShape},
        import::{Importer, ext_to_int_transformation},
    },
    probs::spp::io::ext_repr::ExtSPInstance,
};
use rand::{SeedableRng, rngs::Xoshiro256PlusPlus, seq::SliceRandom};
use std::{collections::HashMap, sync::Arc, time::Duration};

pub struct FixedSheet<'a> {
    pub width: f32,
    pub height: f32,
    pub edge_margin: f32,
    pub exclusion_allowance: f32,
    pub exclusions: &'a [ExtShape],
    pub locks: &'a [ExtPlacedItem],
}

pub struct SheetSearch {
    pub seed: u64,
    pub time_limit: Option<Duration>,
    pub attempt_limit: Option<usize>,
    pub config: SparrowConfig,
}

pub enum SheetOutcome {
    Complete(SheetFit),
    NoCompleteFit,
    Cancelled(Option<SheetFit>),
}

pub struct SheetFit {
    pub layout: ExtLayout,
    /// Rightmost occupied coordinate, including the half-clearance on parts.
    pub used_length: f32,
}

pub struct PreparedSheet {
    initial: Layout,
    items: Vec<Arc<Item>>,
    remaining: Vec<usize>,
    by_id: HashMap<u64, usize>,
    locks: Vec<ExtPlacedItem>,
    stock_area: f64,
}

impl PreparedSheet {
    pub fn new(
        problem: &ExtSPInstance,
        sheet: FixedSheet<'_>,
        config: SparrowConfig,
    ) -> Result<Self> {
        let gap = problem.min_item_separation;
        ensure!(
            [
                sheet.width,
                sheet.height,
                sheet.edge_margin,
                sheet.exclusion_allowance,
                gap
            ]
            .iter()
            .all(|n| n.is_finite() && *n >= 0.0),
            "Invalid stock dimensions or clearance"
        );
        let edge = sheet.edge_margin;
        ensure!(
            sheet.width > 2.0 * edge + gap && sheet.height > 2.0 * edge + gap,
            "Margins leave no usable stock"
        );
        let importer = Importer::new(config.cde_config, None, None);
        let item_importer = importer.with_min_item_separation(gap)?;
        let container_importer =
            importer.with_min_item_separation(gap + 2.0 * sheet.exclusion_allowance)?;
        // The container importer offsets both the exterior and holes. Compensate
        // the exterior for the extra allowance, which applies only to holes.
        let inset = edge - sheet.exclusion_allowance;
        let container = container_importer.import_container(&ExtContainer {
            id: 0,
            shape: ExtShape::Rectangle {
                x_min: inset,
                y_min: inset,
                width: sheet.width - 2.0 * inset,
                height: sheet.height - 2.0 * inset,
            },
            zones: sheet
                .exclusions
                .iter()
                .cloned()
                .map(|shape| ExtQualityZone { quality: 0, shape })
                .collect(),
        })?;
        let mut prepared = Self {
            initial: Layout::new(container),
            items: Vec::new(),
            remaining: Vec::new(),
            by_id: HashMap::new(),
            locks: sheet.locks.to_vec(),
            stock_area: f64::from(sheet.width) * f64::from(sheet.height),
        };
        for item in &problem.items {
            let idx = prepared.items.len();
            ensure!(
                prepared.by_id.insert(item.base.id, idx).is_none(),
                "Duplicate item ID {}",
                item.base.id
            );
            prepared
                .items
                .push(item_importer.import_item(&item.base, idx)?);
            prepared.remaining.push(usize::try_from(item.demand)?);
        }
        for lock in sheet.locks {
            Self::place(
                &mut prepared.initial,
                &prepared.items,
                &prepared.by_id,
                &mut prepared.remaining,
                lock,
            )?;
        }
        ensure!(
            prepared.initial.is_collision_free(),
            "Locked placements overlap or violate the sheet or an exclusion"
        );
        Ok(prepared)
    }

    fn place(
        layout: &mut Layout,
        items: &[Arc<Item>],
        by_id: &HashMap<u64, usize>,
        remaining: &mut [usize],
        placement: &ExtPlacedItem,
    ) -> Result<()> {
        let idx = *by_id
            .get(&placement.item_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown item {}", placement.item_id))?;
        let item = &items[idx];
        ensure!(
            remaining[idx] > 0,
            "Placements exceed demand for item {}",
            placement.item_id
        );
        let pose = &placement.transformation;
        ensure!(
            pose.rotation.is_finite()
                && pose.translation.0.is_finite()
                && pose.translation.1.is_finite(),
            "Invalid placement coordinates"
        );
        let pose = DTransformation::from(pose.clone());
        ensure!(
            item.allowed_orientations().allows(&pose),
            "Disallowed orientation for item {}",
            placement.item_id
        );
        let pose = ext_to_int_transformation(&pose, &item.shape_orig().pre_transform);
        layout.place_item(item, pose);
        remaining[idx] -= 1;
        Ok(())
    }

    /// The lock prefix is preserved verbatim in every result.
    pub fn validate(&self, solution: &ExtLayout) -> Result<(f32, f32)> {
        ensure!(
            solution.container_id == 0 && solution.placed_items.len() >= self.locks.len(),
            "Invalid sheet layout"
        );
        for (actual, lock) in solution.placed_items.iter().zip(&self.locks) {
            let a = &actual.transformation;
            let b = &lock.transformation;
            ensure!(
                actual.item_id == lock.item_id
                    && a.translation == b.translation
                    && a.rotation == b.rotation
                    && a.reflected == b.reflected,
                "Layout does not preserve every lock"
            );
        }
        let mut layout = self.initial.clone();
        let mut remaining = self.remaining.clone();
        for placement in solution.placed_items.iter().skip(self.locks.len()) {
            Self::place(
                &mut layout,
                &self.items,
                &self.by_id,
                &mut remaining,
                placement,
            )?;
        }
        ensure!(
            remaining.iter().all(|&n| n == 0),
            "Layout does not satisfy outstanding demand"
        );
        ensure!(
            layout.is_collision_free(),
            "Layout violates sheet boundaries or clearances"
        );
        let density = (f64::from(layout.placed_item_area()) / self.stock_area) as f32;
        Ok((density, used_length(&layout)))
    }
}

fn used_length(layout: &Layout) -> f32 {
    layout
        .placed_items()
        .values()
        .map(|part| part.shape().bbox().x_max)
        .fold(0.0, f32::max)
}

pub fn solve_fixed_sheet(
    problem: &ExtSPInstance,
    sheet: FixedSheet<'_>,
    settings: SheetSearch,
    control: &SolveControl,
    mut progress: impl FnMut(usize, usize, usize, Duration, Option<&SheetFit>),
) -> Result<SheetOutcome> {
    let start = jagua_rs::Instant::now();
    ensure!(
        settings.time_limit.is_some() || settings.attempt_limit.is_some(),
        "A sheet search needs a stopping limit"
    );
    let stopped = || {
        control.is_cancelled()
            || settings
                .time_limit
                .is_some_and(|limit| start.elapsed() >= limit)
    };
    let stop_outcome = |best| {
        if control.is_cancelled() {
            SheetOutcome::Cancelled(best)
        } else if let Some(fit) = best {
            SheetOutcome::Complete(fit)
        } else {
            SheetOutcome::NoCompleteFit
        }
    };
    if stopped() {
        return Ok(stop_outcome(None));
    }
    let prepared = PreparedSheet::new(problem, sheet, settings.config)?;
    let mut rng = Xoshiro256PlusPlus::seed_from_u64(settings.seed);
    let mut order = Vec::new();
    for (idx, &count) in prepared.remaining.iter().enumerate() {
        order.try_reserve(count)?;
        order.extend(std::iter::repeat_n(idx, count));
    }
    order.sort_by(|&a, &b| {
        prepared.items[b]
            .area()
            .total_cmp(&prepared.items[a].area())
    });
    let required = order.len() + prepared.locks.len();
    let mut best_count = prepared.locks.len();
    let mut attempts = 0;
    let mut best: Option<SheetFit> = None;
    while !stopped() && settings.attempt_limit.is_none_or(|limit| attempts < limit) {
        if attempts > 0 {
            order.shuffle(&mut rng);
        }
        attempts += 1;
        let mut layout = prepared.initial.clone();
        let mut placed = prepared.locks.len();
        progress(attempts, best_count, required, start.elapsed(), None);
        for &idx in &order {
            if stopped() {
                return Ok(stop_outcome(best));
            }
            let item = &prepared.items[idx];
            let evaluator = LBFEvaluator::new(&layout, item);
            let (candidate, _) = search_placement_with_stop(
                &layout,
                item,
                None,
                evaluator,
                settings.config.expl_cfg.separator_config.sample_config,
                &mut rng,
                &stopped,
            );
            let Some((pose, SampleEval::Clear { .. })) = candidate else {
                break;
            };
            layout.place_item(item, pose);
            placed += 1;
            if placed > best_count {
                best_count = placed;
                progress(attempts, best_count, required, start.elapsed(), None);
            }
        }
        if placed == required
            && !stopped()
            && best
                .as_ref()
                .is_none_or(|fit| used_length(&layout) < fit.used_length)
        {
            let mut result = export_layout_snapshot(&layout.save(), 0);
            result.placed_items[..prepared.locks.len()].clone_from_slice(&prepared.locks);
            let Ok((density, used_length)) = prepared.validate(&result) else {
                continue;
            };
            result.density = density;
            if best
                .as_ref()
                .is_none_or(|fit| used_length < fit.used_length)
            {
                best = Some(SheetFit {
                    layout: result,
                    used_length,
                });
                progress(
                    attempts,
                    best_count,
                    required,
                    start.elapsed(),
                    best.as_ref(),
                );
            }
            if order.is_empty() {
                break;
            }
        }
    }
    Ok(stop_outcome(best))
}
