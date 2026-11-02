# Fork changes

## Unreleased — application embedding API

These changes are local to this fork. The package version is still 0.3.0;
selecting the upstream version number alone does not select this implementation.
Use a local path dependency or a pinned fork commit containing these changes.

### Recommended application interface

- `solve(SolveProblem, SolveSettings, callback, &SolveControl)` imports external
  geometry or accepts a prepared Jagua instance, initializes randomness, and runs
  the existing packing heuristic synchronously.
- `SolveOutcome` keeps status, an optional best complete valid solution, actual
  seed when initialized, elapsed time, and statistics. It is `must_use` so ignored
  errors/results produce a compiler warning. Failure/cancellation and a retained
  valid solution can coexist; completion does not guarantee a solution.
- Borrowed progress events distinguish phase/statistics updates, improved valid
  solutions, optional explicitly labelled candidates, and a terminal event.
- Persistent cancellation is separate from skipping exploration. Cooperative
  checks extend into initial placement, strip-growth retries, worker item loops,
  sampling, and coordinate descent.
- Settings, input, and warm starts are validated before passing data to the relevant lower-level
  operations. Recoverable errors preserve their underlying sources.
  Worker-pool creation has a fallible path.
- Default timing includes the entire call (600 seconds), splitting the remaining
  post-initialization budget 80/20 between exploration and compression; compression
  can use unused exploration time. An explicit search-only mode preserves the
  legacy phase limits. Neither mode is a hard wall-clock limit.
- Explicit `u64` seeds avoid platform-size truncation and take precedence over the
  legacy seed. Solution export uses per-run timing and original-shape coordinates.
- Jagua is re-exported for compatible types; `SparrowConfig` implements `Default`.

### Compatibility and boundaries

The packing heuristic and default tuning remain shared with upstream code.
Existing optimizer signatures, listener traits, CLI/TUI tools, and IO helpers
remain available. CLI/TUI use shared orchestration and retain search-only timing,
flags, and output formats. The benchmark's phase-level orchestration remains.
The library facade performs no automatic file writes, logging setup, or signal
registration. Lower-level helpers retain their explicit side effects.

The README now targets library consumers. Legacy executable usage moved to
[docs/cli.md](docs/cli.md); upstream promotional/screenshots content was removed
from the main entry point. License and research attribution are retained.

See the [integration guide](docs/embedding.md) for the application contract,
threading, coordinate handling, budget precedence, outcomes, and warm-start code.
The guide is also included in generated API documentation; Rust snippets in the
README and guide are compiled as doctests. Runnable examples demonstrate basic
solving and bounded, nonblocking progress delivery from a background worker.

### Validation added

Coverage includes in-memory solves; valid/invalid warm starts; nonfinite and
malformed input; settings validation; seed precedence; sequential/concurrent
jobs; initial-placement and phase cancellation; separate skipping; candidate
opt-in; event ordering; budget semantics; retaining valid results after setup
failure; and cancellation checkpoints preserving worker/tracker state.

Maintainer checks cover existing regressions, CLI output/warm starts, optional
TUI/SVG features, nightly SIMD, and the WASM library build. A WASM build is a
compilation check, not an end-to-end browser integration test.
