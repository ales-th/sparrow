//! Run with: cargo run --release --example in_memory
use sparrow::jagua_rs::probs::spp::io::ext_repr::ExtSPInstance;
use sparrow::{BudgetMode, SolveControl, SolveProblem, SolveSettings, SolveStatus, solve};
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let problem: ExtSPInstance = serde_json::from_str(
        r#"{
        "name":"rectangles", "strip_height":10,
        "items":[{"id":42,"demand":3,
            "orientation":{"rotation":{"mode":"discrete","angles":[0,90]}},
            "shape":{"type":"simple_polygon","data":[[0,0],[3,0],[3,2],[0,2]]}}]
    }"#,
    )?;
    let settings = SolveSettings {
        seed: Some(42),
        budget: BudgetMode::WholeRun(Duration::from_secs(1)),
        ..Default::default()
    };
    let outcome = solve(
        SolveProblem::External {
            instance: &problem,
            initial_solution: None,
        },
        settings,
        |_| {},
        &SolveControl::new(),
    );
    println!(
        "Status: {:?}, elapsed: {:?}",
        outcome.status, outcome.elapsed
    );
    if let Some(solution) = outcome.export_solution() {
        println!("Best width: {}", solution.strip_width);
    }
    if let SolveStatus::Failed(error) = outcome.status {
        return Err(error.into());
    }
    Ok(())
}
