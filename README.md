# Sparrow — Rust embedding fork

Sparrow solves **2D irregular strip packing**: the strip height is fixed and the
solver minimizes its width. This fork adds a convenient Rust API for embedding
Sparrow in applications, while retaining the upstream packing heuristic.
Multiple fixed-size sheets, file-format conversion, rendering, and application
job management are responsibilities of the consuming application.

## Start here

- Review the [fork changes](CHANGELOG.md).
- Read the [application integration guide](docs/embedding.md) before connecting a UI.
- Use `sparrow::solve` as the application entry point.
- Try the [in-memory example](examples/in_memory.rs) and
  [background worker example](examples/background.rs).
- Generate the API reference with `cargo doc --lib --no-deps --open`.

### Depend on this fork

Use Rust 1.90 or newer. For local development, point the dependency at this fork's
checkout, relative to the **consuming crate's `Cargo.toml`**:

```toml
[dependencies]
sparrow = { path = "../sparrow" }
```

Adjust that path to the checkout location. For a checkout stored as a submodule,
use its actual relative path. A dependency such as `sparrow = "0.3"` does **not**
select this fork or guarantee this API. For a Git dependency, use this fork's Git
URL and pin a reviewed commit containing these changes; uncommitted local edits
are available only through a path dependency. This crate pins Jagua to 1.1.0 and
re-exports its types under `sparrow::jagua_rs`.

### Solve and inspect the outcome

```rust,no_run
use std::time::Duration;
use sparrow::{solve, BudgetMode, SolveControl, SolveProblem, SolveSettings, SolveStatus};
use sparrow::jagua_rs::probs::spp::io::ext_repr::ExtSPInstance;

fn nest(problem: &ExtSPInstance) {
    let outcome = solve(
        SolveProblem::External { instance: problem, initial_solution: None },
        SolveSettings {
            seed: Some(42),
            budget: BudgetMode::WholeRun(Duration::from_secs(30)),
            ..Default::default()
        },
        |_| {},
        &SolveControl::new(),
    );
    match &outcome.status {
        SolveStatus::Completed(reason) => println!("Completed: {reason:?}"),
        SolveStatus::Cancelled => println!("Cancelled"),
        SolveStatus::Failed(error) => eprintln!("Failed: {error}"),
    }
    // Cancellation or failure can still retain a usable solution.
    if let Some(solution) = outcome.export_solution() {
        println!("Best width: {}", solution.strip_width);
    }
}
```

`solve` blocks its calling thread. Run it on an application-owned background
worker for a UI. The default budget is **600 seconds for the whole call**,
including initialization. Cancellation is cooperative. A completed outcome can
have no solution if the budget expired during initialization; completion does
not mean a proven optimum. Always inspect both status and solution availability.

Use `export_solution()` when applying placements to your original input shapes.
Raw internal placements use Jagua's preprocessing coordinates. See the guide for
units, orientation permissions, progress ownership, budgets, and safe warm starts.

## Development and compatibility

Build the consuming application in release mode for meaningful optimization
performance. Cargo profiles in a dependency are not inherited by the application;
configure release tuning in the consuming workspace if needed. Stable Rust is
the default. The optional `simd` feature requires nightly Rust.

```powershell
cargo test --all-targets
cargo test --doc
cargo check --all-targets --features tui,live_svg
cargo check --all-targets --features only_final_svg
cargo +nightly test --all-targets --features simd
cargo +nightly build --target wasm32-unknown-unknown --lib
```

The SIMD and WASM commands require the corresponding toolchain/target. `live_svg`
and `only_final_svg` are mutually exclusive. Neither is needed for the embedding
API; they and `tui` control executable/exporter workflows, not application UI
progress. A WASM build check does not establish browser worker/thread integration.

Legacy optimizer modules, CLI/TUI, exporters, and benchmarking assets remain for
compatibility and maintenance. They are lower-level tools with different setup
and side effects. [CLI instructions](docs/cli.md) are separate from application
integration. Do not call `CtrlCTerminator`, `init_logger`, or `SvgExporter` merely
to run an embedded job; the facade needs none of them.

## Origin and license

Based on [JeroenGar/sparrow](https://github.com/JeroenGar/sparrow), built on
[Jagua](https://github.com/JeroenGar/jagua-rs), accompanying the paper
[An open-source heuristic to reboot 2D nesting research](https://doi.org/10.48550/arXiv.2509.13329).
Upstream research results and benchmark reproduction material remain under
[data/experiments](data/experiments/README.md).

The original project began at the CODeS research group of NUMA, KU Leuven, with
funding from Research Foundation – Flanders (grant 1S71222N) until October 2025.
See [LICENSE](./LICENSE) for the MIT license and copyright notice.
