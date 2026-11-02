# CLI and terminal tools (maintainers)

These commands run the legacy executable. For application development, use the
[library API](embedding.md). The CLI installs logging and Ctrl+C handling and
writes files relative to its working directory. These are executable behaviors;
`solve` does not do them. CLI time limits cover search only, excluding import and
initial placement. Do not use the CLI as the integration contract for an app.

## Running the CLI

**General usage:**
```bash
cargo run --release  -- \
    -i [path to input JSON, or a solution JSON] \
    -t [timelimit in seconds (default is 600s)]
```
The optimization process contains two distinct phases: exploration & compression.
By default 80% of the timelimit is spent exploring and 20% is spent compressing.
Pressing 'Ctrl + C' immediately moves the algorithm to the next phase, or terminates it.

**All CLI options:**
```bash
-i, --input <INPUT>              Path to the input JSON file, or a solution JSON file for warm starting
-t, --global-time <GLOBAL_TIME>  Set a global time limit (in seconds)
-e, --exploration <EXPLORATION>  Set the exploration phase time limit (in seconds)
-c, --compression <COMPRESSION>  Set the compression phase time limit (in seconds)
-x, --early-termination          Enable early termination of the optimization process
-s, --rng-seed <RNG_SEED>        Fixed seed for the random number generator
--workers <WORKERS>           Number of worker threads used by the separator
-h, --help                       Print help
```

**Concrete example**:
```bash
cargo run --release -- \
    -i data/input/swim.json
```

## Live visualization

`sparrow` can show the search in a terminal dashboard alongside a live browser view of the current solution.
The dashboard reports the current phase, progress, throughput, and logs. It continuously updates the SVG used by
[live_viewer.html](../data/live/live_viewer.html):

```bash
cargo run --release --features=tui -- \
    -i data/input/swim.json
```

For the browser viewer without the dashboard, open
[live_viewer.html](../data/live/live_viewer.html) and enable the `live_svg` feature:

```bash
cargo run --release --features=live_svg -- \
    -i data/input/swim.json
```

## Input

The CLI reads a JSON instance or a previously exported output JSON for warm
starting. Schema, supported geometry, units, and orientation permissions are
shared with the library's external representation; see the
[integration guide](embedding.md#problem-model-units-and-placement-coordinates).
Importing SVG/DXF is the responsibility of other tooling.

## Output

Solutions are exported as SVG files in the `output` folder. 
The final SVG solution is saved as `output/final_{name}.svg`.

The SVG files serve both as a visual and exact representation of the solution.
All original shapes and their exact transformations applied to them are defined within the SVG:
```html
    ...
    <g id="items">
        <defs>
            <g id="item_0">...</g>
        </defs>
        <use transform="translate(1289.9116 1828.7717), rotate(-90)" xlink:href="#item_0">...</use>
    </g>
    ...
```
The [SVG spec](https://stackoverflow.com/questions/18582935/the-applying-order-of-svg-transforms) defines that the transformations are applied from right to left.
So here the item is always first rotated and then translated.

By default, a range of intermediate (and infeasible) solutions will be exported in `output/sols_{name}`.
To disable this and export only a single final solution, compile with the `only_final_svg` feature:
```bash
cargo run --release --features=only_final_svg -- \
    -i data/input/swim.json
```
The final solution is saved both in SVG and JSON format in `output/final_{name}.svg` and `output/final_{name}.json`, respectively.

