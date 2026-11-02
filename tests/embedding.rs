use sparrow::{
    jagua_rs::{
        entities::Layout,
        io::import::Importer,
        probs::spp::io::{self, ext_repr::ExtSPInstance},
    },
    *,
};
use std::time::Duration;

fn problem() -> ExtSPInstance {
    serde_json::from_value(serde_json::json!({
        "name": "embedding", "strip_height": 10.0,
        "items": [
            {"id": 17, "demand": 2, "orientation": {"rotation": {"mode":"discrete", "angles":[0,180]}},
             "shape": {"type":"simple_polygon", "data":[[0,0],[3,0],[3,2],[0,2]]}},
            {"id": 91, "demand": 1, "orientation": {"rotation": {"mode":"discrete", "angles":[0]}},
             "shape": {"type":"simple_polygon", "data":[[0,0],[2,0],[2,3],[0,3]]}}
        ]
    })).unwrap()
}
fn settings() -> SolveSettings {
    let mut s = SolveSettings {
        seed: Some(u64::MAX),
        budget: BudgetMode::SearchOnly,
        ..Default::default()
    };
    s.config.expl_cfg.time_limit = Duration::ZERO;
    s.config.cmpr_cfg.time_limit = Duration::ZERO;
    s.config.expl_cfg.separator_config.n_workers = 1;
    s.config.cmpr_cfg.separator_config.n_workers = 1;
    s
}
fn run(instance: &ExtSPInstance) -> SolveOutcome {
    solve(
        SolveProblem::External {
            instance,
            initial_solution: None,
        },
        settings(),
        |_| {},
        &SolveControl::new(),
    )
}
fn assert_valid(out: &SolveOutcome) {
    let solution = out.best_solution.as_ref().expect("complete solution");
    let layout = Layout::from_snapshot(solution.layout_snapshot());
    assert!(layout.is_collision_free());
    assert_eq!(layout.placed_items().len(), 3);
    for (_, item) in layout.placed_items() {
        assert!(item.item().allowed_orientations().allows(&item.d_transf()));
    }
    let exported = out.export_solution().unwrap();
    assert_eq!(
        exported
            .layout
            .placed_items
            .iter()
            .filter(|p| p.item_id == 17)
            .count(),
        2
    );
    assert_eq!(
        exported
            .layout
            .placed_items
            .iter()
            .filter(|p| p.item_id == 91)
            .count(),
        1
    );
    assert_eq!(exported.run_time_sec, out.elapsed.as_secs());
}

#[test]
fn solves_in_memory_and_reports_ordered_events() {
    let p = problem();
    let mut elapsed = Duration::ZERO;
    let mut iterations = 0;
    let mut evaluations = 0;
    let mut terminal = 0;
    let mut phases = vec![];
    let out = solve(
        SolveProblem::External {
            instance: &p,
            initial_solution: None,
        },
        settings(),
        |event| {
            let time = match event {
                ProgressEvent::Phase { phase, elapsed } => {
                    phases.push(phase);
                    elapsed
                }
                ProgressEvent::Statistics {
                    statistics,
                    elapsed,
                } => {
                    assert!(statistics.iterations >= iterations);
                    assert!(statistics.evaluations >= evaluations);
                    iterations = statistics.iterations;
                    evaluations = statistics.evaluations;
                    elapsed
                }
                ProgressEvent::BestSolution { solution, elapsed } => {
                    assert_eq!(solution.layout_snapshot().placed_items().len(), 3);
                    assert!(Layout::from_snapshot(solution.layout_snapshot()).is_collision_free());
                    elapsed
                }
                ProgressEvent::Candidate { .. } => panic!("candidates disabled"),
                ProgressEvent::Finished { outcome } => {
                    terminal += 1;
                    outcome.elapsed
                }
            };
            assert!(time >= elapsed);
            elapsed = time;
        },
        &SolveControl::new(),
    );
    assert!(matches!(
        out.status,
        SolveStatus::Completed(CompletionReason::BudgetExhausted)
    ));
    assert_eq!(out.seed, Some(u64::MAX));
    assert_eq!(terminal, 1);
    assert_eq!(
        phases,
        [
            SolvePhase::Initialization,
            SolvePhase::Exploration,
            SolvePhase::Compression
        ]
    );
    assert_valid(&out);
}

#[test]
fn zero_whole_run_budget_and_pre_cancel_have_no_partial_solution() {
    let p = problem();
    let s = SolveSettings {
        budget: BudgetMode::WholeRun(Duration::ZERO),
        ..settings()
    };
    let out = solve(
        SolveProblem::External {
            instance: &p,
            initial_solution: None,
        },
        s,
        |_| {},
        &SolveControl::new(),
    );
    assert!(matches!(
        out.status,
        SolveStatus::Completed(CompletionReason::BudgetExhausted)
    ));
    assert!(out.best_solution.is_none());
    let control = SolveControl::new();
    control.cancel();
    let mut terminals = 0;
    let out = solve(
        SolveProblem::External {
            instance: &p,
            initial_solution: None,
        },
        settings(),
        |event| {
            if matches!(event, ProgressEvent::Finished { .. }) {
                terminals += 1;
            }
        },
        &control,
    );
    assert!(matches!(out.status, SolveStatus::Cancelled));
    assert!(out.best_solution.is_none());
    assert_eq!(terminals, 1);
}

#[test]
fn cancellation_during_initial_placement_does_not_expose_partial_layout() {
    let p = problem();
    let control = SolveControl::new();
    let out = solve(
        SolveProblem::External {
            instance: &p,
            initial_solution: None,
        },
        settings(),
        |event| {
            if let ProgressEvent::Statistics { statistics, .. } = event {
                if statistics.placed_items == 1 {
                    control.cancel();
                }
            }
        },
        &control,
    );
    assert!(matches!(out.status, SolveStatus::Cancelled));
    assert_eq!(out.statistics.placed_items, 1);
    assert!(out.best_solution.is_none());
}

#[test]
fn cancellation_in_either_phase_retains_best_and_does_not_reset() {
    for phase_to_cancel in [SolvePhase::Exploration, SolvePhase::Compression] {
        let p = problem();
        let control = SolveControl::new();
        let mut phases = vec![];
        let out = solve(
            SolveProblem::External {
                instance: &p,
                initial_solution: None,
            },
            settings(),
            |event| {
                if let ProgressEvent::Phase { phase, .. } = event {
                    phases.push(phase);
                    if phase == phase_to_cancel {
                        control.cancel();
                    }
                }
            },
            &control,
        );
        assert!(matches!(out.status, SolveStatus::Cancelled));
        assert!(control.is_cancelled());
        assert_valid(&out);
        if phase_to_cancel == SolvePhase::Exploration {
            assert!(!phases.contains(&SolvePhase::Compression));
        }
    }
}

#[test]
fn skip_during_initialization_advances_to_compression_without_cancelling() {
    let p = problem();
    let control = SolveControl::new();
    let mut reached_compression = false;
    let mut s = settings();
    s.config.expl_cfg.time_limit = Duration::from_secs(60);
    let out = solve(
        SolveProblem::External {
            instance: &p,
            initial_solution: None,
        },
        s,
        |event| {
            if let ProgressEvent::Phase { phase, .. } = event {
                if phase == SolvePhase::Initialization {
                    control.skip_to_compression();
                }
                if phase == SolvePhase::Compression {
                    reached_compression = true;
                    control.skip_to_compression();
                }
            }
        },
        &control,
    );
    assert!(reached_compression);
    assert!(!control.is_cancelled());
    assert!(matches!(out.status, SolveStatus::Completed(_)));
    assert_valid(&out);
}

#[test]
fn external_and_imported_warm_starts_are_validated() {
    let p = problem();
    let original = run(&p);
    assert_valid(&original);
    let exported = original.export_solution().unwrap();
    let out = solve(
        SolveProblem::External {
            instance: &p,
            initial_solution: Some(&exported),
        },
        settings(),
        |_| {},
        &SolveControl::new(),
    );
    assert_valid(&out);
    let config = settings().config;
    let importer = Importer::new(
        config.cde_config,
        config.poly_simpl_tolerance,
        config.narrow_concavity_cutoff_ratio,
    );
    let imported = io::import_instance(&importer, &p).unwrap();
    let out = solve(
        SolveProblem::Imported {
            instance: imported,
            initial_solution: original.best_solution.as_ref(),
        },
        settings(),
        |_| {},
        &SolveControl::new(),
    );
    assert_valid(&out);
    for bad in 0..9 {
        let mut invalid = exported.clone();
        match bad {
            0 => {
                invalid.layout.placed_items.pop();
            }
            1 => invalid
                .layout
                .placed_items
                .push(invalid.layout.placed_items[0].clone()),
            2 => invalid.layout.placed_items[0].item_id = 999,
            3 => {
                invalid.layout.placed_items[0].transformation =
                    invalid.layout.placed_items[1].transformation.clone()
            }
            4 => invalid.strip_width = f32::NAN,
            5 => invalid.layout.container_id = 7,
            6 => invalid.layout.placed_items[0].transformation.rotation = 45.0,
            7 => invalid.layout.placed_items[0].transformation.reflected = true,
            _ => invalid.layout.placed_items[0].transformation.translation.0 = f32::NAN,
        }
        let out = solve(
            SolveProblem::External {
                instance: &p,
                initial_solution: Some(&invalid),
            },
            settings(),
            |_| {},
            &SolveControl::new(),
        );
        assert!(
            matches!(out.status, SolveStatus::Failed(SolveError::WarmStart(_))),
            "bad case {bad}: {:?}",
            out.status
        );
        assert!(out.best_solution.is_none());
    }
}

#[test]
fn validates_settings_and_input() {
    let p = problem();
    for bad in 0..9 {
        let mut s = settings();
        match bad {
            0 => s.config.expl_cfg.separator_config.n_workers = 0,
            1 => s.config.expl_cfg.shrink_step = f32::NAN,
            2 => s.config.cmpr_cfg.shrink_range = (0.001, 0.5),
            3 => s.config.poly_simpl_tolerance = Some(-1.0),
            4 => {
                s.config
                    .expl_cfg
                    .separator_config
                    .sample_config
                    .n_coord_descents = 0
            }
            5 => s.budget = BudgetMode::WholeRun(Duration::MAX),
            6 => s.config.cde_config.quadtree_depth = 255,
            7 => s.config.expl_cfg.solution_pool_distribution_stddev = 0.0,
            _ => s.config.cmpr_cfg.separator_config.n_workers = usize::MAX,
        }
        let mut terminal = 0;
        let out = solve(
            SolveProblem::External {
                instance: &p,
                initial_solution: None,
            },
            s,
            |event| {
                if matches!(event, ProgressEvent::Finished { .. }) {
                    terminal += 1;
                }
            },
            &SolveControl::new(),
        );
        assert!(matches!(
            out.status,
            SolveStatus::Failed(SolveError::InvalidSettings(_))
        ));
        assert_eq!(terminal, 1);
    }
    let mut invalid = p.clone();
    invalid.strip_height = f32::INFINITY;
    assert!(matches!(
        run(&invalid).status,
        SolveStatus::Failed(SolveError::Input(_))
    ));
    invalid = p.clone();
    invalid.items[0].base.id = invalid.items[1].base.id;
    assert!(matches!(
        run(&invalid).status,
        SolveStatus::Failed(SolveError::Input(_))
    ));
    invalid = p;
    invalid.items.clear();
    assert!(matches!(
        run(&invalid).status,
        SolveStatus::Failed(SolveError::Input(_))
    ));
}

#[test]
fn malformed_polygon_geometry_returns_an_input_error() {
    for nonfinite in [false, true] {
        let mut p = problem();
        if let sparrow::jagua_rs::io::ext_repr::ExtShape::SimplePolygon(shape) =
            &mut p.items[0].base.shape
        {
            if nonfinite {
                shape.0[0].0 = f32::NAN;
            } else {
                shape.0.truncate(2);
            }
        }
        assert!(matches!(
            run(&p).status,
            SolveStatus::Failed(SolveError::Input(_))
        ));
    }
}

#[test]
fn independent_runs_and_seed_precedence() {
    let handles: Vec<_> = (0..2)
        .map(|_| std::thread::spawn(|| run(&problem())))
        .collect();
    for handle in handles {
        assert_valid(&handle.join().unwrap());
    }
    let p = problem();
    let mut s = settings();
    s.config.rng_seed = Some(7);
    let out = solve(
        SolveProblem::External {
            instance: &p,
            initial_solution: None,
        },
        s,
        |_| {},
        &SolveControl::new(),
    );
    assert_eq!(out.seed, Some(u64::MAX));
    s.seed = None;
    let out = solve(
        SolveProblem::External {
            instance: &p,
            initial_solution: None,
        },
        s,
        |_| {},
        &SolveControl::new(),
    );
    assert_eq!(out.seed, Some(7));
    assert_valid(&out);
}

#[test]
fn unplaceable_item_is_a_construction_error_with_source() {
    let mut p = problem();
    p.strip_height = 1.0;
    let out = run(&p);
    match out.status {
        SolveStatus::Failed(SolveError::Construction(source)) => {
            assert!(
                source
                    .downcast_ref::<sparrow::optimizer::lbf::ConstructionError>()
                    .is_some()
            );
        }
        status => panic!("unexpected status: {status:?}"),
    }
    assert!(out.best_solution.is_none());
}
