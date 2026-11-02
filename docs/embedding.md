# Application integration guide

Use `sparrow::solve` for application jobs. It accepts an in-memory problem,
settings, a progress callback, and a cancellation handle. It is synchronous and
returns a `SolveOutcome`, rather than `Result<SPSolution, _>`, because a failed or
cancelled job can still have a valid solution worth keeping.

For dependency setup, see the repository README. Use the fork checkout or a pinned
commit of the fork, rather than selecting a similarly named crates.io package.
Jagua types are available through `sparrow::jagua_rs`; this avoids accidentally
introducing an incompatible Jagua version.

## A complete call and outcome handling

```rust,no_run
use std::time::Duration;
use sparrow::{solve, BudgetMode, SolveControl, SolveProblem, SolveSettings, SolveStatus};
use sparrow::jagua_rs::probs::spp::io::ext_repr::ExtSPInstance;

fn nest(problem: &ExtSPInstance) {
    let control = SolveControl::new();
    let mut settings = SolveSettings {
        seed: Some(42),
        budget: BudgetMode::WholeRun(Duration::from_secs(30)),
        ..Default::default()
    };
    // Configure both phases when choosing the job's parallelism.
    settings.config.expl_cfg.separator_config.n_workers = 2;
    settings.config.cmpr_cfg.separator_config.n_workers = 2;

    let outcome = solve(
        SolveProblem::External { instance: problem, initial_solution: None },
        settings, |_| {}, &control,
    );
    match &outcome.status {
        SolveStatus::Completed(reason) => println!("Completed: {reason:?}"),
        SolveStatus::Cancelled => println!("Cancelled"),
        SolveStatus::Failed(error) => eprintln!("Failed: {error}"),
    }
    // Inspect this independently of status: even failure may retain a result.
    match outcome.export_solution() {
        Some(solution) => println!("Best width: {}", solution.strip_width),
        None => println!("No complete valid solution was found"),
    }
}
```

The [in-memory example](https://github.com/ales-th/sparrow/blob/main/examples/in_memory.rs) includes problem construction.
The [background example](https://github.com/ales-th/sparrow/blob/main/examples/background.rs) demonstrates nonblocking
progress forwarding and cancellation. Both are executable from the fork root:

```powershell
cargo run --release --example in_memory
cargo run --release --example background -- data/input/swim.json
```

| Outcome | How the application should interpret it |
| --- | --- |
| `Completed(AlgorithmStopped)` | The heuristic stopped naturally. This is not proof of optimality. |
| `Completed(BudgetExhausted)` | The applicable budget was consumed. A solution is optional. |
| `Cancelled` | Cancellation was observed. Keep a retained solution if useful. |
| `Failed(error)` | Show or record the structured error; inspect the retained solution separately. |
| `best_solution: None` | No complete valid solution was retained. Never assume completion implies `Some`. |

`SolveOutcome` is marked `must_use`. Do not ignore it, unwrap its optional solution
unconditionally, or turn an error status into an early return before deciding
what to do with its retained solution. `export_solution()` returns `None` when
there is no solution and does not write a file. Its runtime is this call's elapsed
time, rounded down to whole seconds; it is not cumulative across warm starts.
`outcome.elapsed` retains finer duration information. `seed` can be `None` if the
job ended before RNG setup.

## Problem model, units, and placement coordinates

Sparrow minimizes the **width** of a strip with fixed **height**. It does not
pack across multiple finite sheets or automatically reject a result against an
application-specific maximum sheet width. Such limits and additional constraints
must be modeled or checked by the application.

`SolveProblem::External` is the recommended starting point. Construct an
`ExtSPInstance` in memory, or deserialize it in the application. The facade does
not load SVG, DXF, or JSON files itself. It currently uses Jagua 1.1.0's schema:

```json
{
  "name": "rectangles",
  "strip_height": 10,
  "min_item_separation": 0,
  "items": [{
    "id": 42,
    "demand": 3,
    "orientation": { "rotation": { "mode": "discrete", "angles": [0, 90] } },
    "shape": { "type": "simple_polygon", "data": [[0,0], [3,0], [3,2], [0,2]] }
  }]
}
```

- Use one consistent length unit for coordinates, strip height, and separation.
  There is no implicit millimeter or pixel conversion.
- External angles are in **degrees**. Internal Jagua transformations use radians.
  Rotation permissions are required; use `discrete` angles, a supported `stepped`
  rotation, or `continuous` mode. Old orientation fields are unsupported.
- Item IDs must be unique, can be nonconsecutive, and are preserved in exported
  placements. Demand is a quantity, not an ID. Zero-demand items are ignored by
  import; there must be positive total demand.
- Coordinates and dimensions must be finite; rectangles must have positive size.
  Polygons must have valid topology. Item holes and multipolygons are unsupported.
- Separation belongs on `ExtSPInstance.min_item_separation`, not solver tuning.
  It must be finite, nonnegative, and smaller than strip height. Import handles
  collision geometry offsets; do not apply the same separation a second time.
- Density values are **fractions** (for example, `0.8` means 80%), not percentages.

Use `outcome.export_solution()` to place **the original input shapes** in your UI
or export pipeline. Jagua centers and may simplify/inflate collision geometry.
Raw `SPSolution` snapshot transformations belong to that internal representation;
applying them directly to original vertices can produce misplaced parts.
Exported transformations undo preprocessing. Apply the exported local reflection
(if `reflected`), then rotation in degrees, then translation. Perform any canvas
axis conversion consistently in the application.

`SolveProblem::Imported` is an advanced path accepting an owned `SPInstance` and
optional borrowed `SPSolution`. Its geometry is already prepared. Changing
`config.cde_config`, simplification, or concavity settings does not rebuild that
instance. Re-import from the original external problem to change those settings.
Keep an immutable input snapshot and a job ID/version alongside each solve so a
late result is applied to the shapes that produced it, rather than newer edits.

## Settings and timing precedence

| Setting | Meaning and default |
| --- | --- |
| `budget` | Defaults to `WholeRun(600 seconds)`. Includes initialization. `WholeRunWithPhaseLimits` also enforces the configured exploration and compression caps while retaining one overall deadline. |
| `config.expl_cfg.time_limit`, `config.cmpr_cfg.time_limit` | Used only by `SearchOnly`; ignored as phase allocations in `WholeRun`. |
| `seed` | Optional full `u64`; overrides `config.rng_seed`. Otherwise the legacy seed or a generated seed is used. |
| `config.*.separator_config.n_workers` | Separate worker count for each phase; both default to 3. This is not the number of application jobs. |
| `live_candidates` | Defaults to `false`; enables extra candidate snapshots for visualization. |
| Geometry import settings in `config` | Applied to `External` input; do not rebuild `Imported` geometry. |

`settings.validate()` can be called before launching a worker. `solve` also
validates settings and returns `Failed(InvalidSettings(...))` on failure. All
supplied configuration is validated, including fields unused by the selected
budget/input mode. Preserve defaults unless deliberately tuning the algorithm.
Tune the consuming application's release profile: dependency Cargo profiles are
not inherited. A fixed seed does not guarantee identical results under a
wall-clock deadline or across versions/platforms.

In `WholeRun`, time starts at API entry and includes validation, import, initial
placement, callbacks, and search. Exploration receives 80% of the time remaining
after initialization. Compression receives the entire remaining global budget
when it begins, including unused exploration time. Expiring exploration's
allocation alone does not exhaust the whole-run budget. `WholeRunWithPhaseLimits` uses
that same global deadline while also capping each phase at its configured duration.

`SearchOnly` gives exploration and compression their configured independent
limits, excluding initialization. Its exhaustion reason means at least one phase
used its allocation. The legacy optimizer and CLI/TUI retain search-only timing.
Do not copy their `-t` expectations into an application using `WholeRun`.

Zero budgets are valid. A zero whole-run budget can return no solution without
importing the problem. Zero search-only phase durations still allow initial
construction, but skip search. Neither budget is a hard wall-clock bound:
checks are cooperative, and an indivisible import, callback, allocation, or
geometry operation can finish after expiry. No further search starts after
expiry is observed. The terminal callback runs after the recorded elapsed time.

## Background work, progress, and cancellation

`solve` blocks its calling thread and does not spawn an application job thread.
Run it on a dedicated worker or the async runtime's blocking-work facility.
Calling it directly in a UI handler or ordinary async task can block that thread.
The solver also uses parallel placement workers; budget their count across jobs.

Callbacks run serially on the **solve caller's thread**, never concurrently from
placement workers. Return promptly, avoid blocking channel sends, and forward
owned compact updates to the UI. Callback execution counts toward the budget.
For bounded queues, drop/coalesce progress updates when full; deliver the final
outcome through the worker completion path independently. Do not drop the final
outcome along with expendable visualization updates.

| Event | Contract |
| --- | --- |
| `Phase` | Initialization, exploration, or compression, with elapsed time. |
| `Statistics` | Placed/required item counts during construction; cumulative search counters and available width/loss/best measurements. `None` means unavailable, not zero. |
| `BestSolution` | An improved complete valid solution. Clone only if retaining it outside the callback. |
| `Candidate` | Optional visualization snapshot with an explicit feasibility flag. Even a feasible candidate need not improve the retained best. |
| `Finished` | Exactly one terminal event for each normally returned outcome, including validation failure and cancellation. The outcome reference is borrowed. |

Search evaluations/moves exclude initial placement and update at separation
completion. Iterations also update within separation. Events arrive at algorithm
checkpoints, not a fixed frame rate; import may produce no intermediate updates.
Use a UI timer if a smooth elapsed-time display is needed. A candidate is never
a substitute for the retained best or a complete exported outcome. Never send an
infeasible candidate to production output or use it as a warm start.

Each `solve` needs a **fresh `SolveControl`**. Clones share the same flags; they
are handles for that job, not independent controllers. There is no reset method.
Do not share a control across unrelated jobs or reuse a cancelled control.

- `cancel()` persists across phases and stops the entire job, including initial
  placement. A complete best solution is retained when one exists.
- `skip_to_compression()` waits for initialization to finish, skips the remainder
  of exploration, and does nothing during compression. It is not cancellation.
- A request after a job has returned cannot change its recorded outcome.

Checkpoints include initial placements, strip-growth retries, separator
iterations, worker item loops, sampling, and coordinate descent. After requesting
cancellation, keep the worker and its input alive until completion. Do not assume
the handle forcibly terminates a thread or makes `join()` nonblocking. In a UI,
join/consume the outcome when the worker is finished. Caller callback panics
propagate and can prevent `Finished`; thread panic handling belongs to the host.

## Warm starts and error handling

Warm starts must meet all demand exactly, preserve compatible item IDs and
orientations, have finite transformations and a positive finite strip width, and
be collision-free inside the strip. External layout `container_id` must be zero.
A previous solution from modified geometry, changed demand, or changed permissions
must be revalidated; a saved result is not automatically compatible with new input.
The safest external warm-start source is `export_solution()`:

```rust,no_run
use sparrow::{solve, SolveControl, SolveOutcome, SolveProblem, SolveSettings};
use sparrow::jagua_rs::probs::spp::io::ext_repr::ExtSPInstance;

fn resume(problem: &ExtSPInstance, previous: &SolveOutcome) -> anyhow::Result<SolveOutcome> {
    let saved = previous.export_solution()
        .ok_or_else(|| anyhow::anyhow!("previous run has no complete valid solution"))?;
    Ok(solve(
        SolveProblem::External { instance: problem, initial_solution: Some(&saved) },
        SolveSettings::default(), |_| {}, &SolveControl::new(),
    ))
}
```

`SolveError` distinguishes invalid settings, input, warm-start, construction, and
runtime setup failures. Underlying sources remain available through
`std::error::Error::source`, including the construction error's external item ID.
Construction failure means the heuristic failed to construct a solution; it is
not proof that the problem is infeasible. Process termination, out-of-memory
abort, and other unrecoverable failures are outside the outcome guarantee.

## Library and maintainer boundaries

The facade does not write files, install a logger or Ctrl+C handler, configure a
terminal, or manage application lifecycle. Ordinary `log` messages go to the
host's logger if one exists. Serialization, rendering, persistence, job queues,
and window shutdown handling belong to the application.

Legacy `optimizer::optimize`, listener/terminator traits, CLI/TUI, benchmarks,
and IO utilities remain available for compatibility and maintenance. Prefer
`solve` for new application code: lower-level APIs bypass portions of its
validation, budgets, progress, and outcome retention. `CtrlCTerminator::new()`
installs a process-wide handler, `io::init_logger()` configures global logging,
and `SvgExporter::new()` can delete SVG files in its configured intermediate
directory. These are explicit opt-in utilities, not prerequisites for embedding.
See `docs/cli.md` in the checkout for legacy executable instructions.
