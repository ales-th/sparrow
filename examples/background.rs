//! Run with: cargo run --release --example background -- data/input/swim.json
use sparrow::jagua_rs::probs::spp::io::ext_repr::ExtSPInstance;
use sparrow::{ProgressEvent, SolveControl, SolveProblem, SolveSettings, SolveStatus, solve};
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn main() -> anyhow::Result<()> {
    // File IO belongs to the application; the solver only sees in-memory data.
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "data/input/swim.json".into());
    let problem: ExtSPInstance = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let control = SolveControl::new();
    let worker_control = control.clone();
    let (tx, rx) = mpsc::sync_channel(8);
    let worker = thread::spawn(move || {
        solve(
            SolveProblem::External {
                instance: &problem,
                initial_solution: None,
            },
            SolveSettings::default(),
            |event| {
                // Forward compact owned updates. Clone a borrowed solution only if needed.
                if let ProgressEvent::BestSolution { solution, .. } = event {
                    // Progress can be dropped when the UI falls behind. Never block the solver.
                    if let Err(mpsc::TrySendError::Disconnected(_)) =
                        tx.try_send(solution.strip_width())
                    {
                        worker_control.cancel();
                    }
                }
            },
            &worker_control,
        )
    });
    // Illustrative application cancellation; no signal handlers are installed by Sparrow.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) && !worker.is_finished() {
        if let Ok(width) = rx.recv_timeout(Duration::from_millis(50)) {
            println!("Best width: {width}");
        }
    }
    control.cancel();
    let outcome = worker
        .join()
        .map_err(|_| anyhow::anyhow!("solver thread panicked"))?;
    for width in rx {
        println!("Best width: {width}");
    }
    println!(
        "Status: {:?}; retained solution: {}",
        outcome.status,
        outcome.best_solution.is_some()
    );
    // The final outcome is delivered independently of the bounded progress queue.
    if let Some(solution) = &outcome.best_solution {
        println!("Retained width: {}", solution.strip_width());
    }
    if let SolveStatus::Failed(error) = outcome.status {
        return Err(error.into());
    }
    Ok(())
}
