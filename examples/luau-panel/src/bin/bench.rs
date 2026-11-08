//! `cargo run -p luau-panel --bin luau-bench --profile perf`: the Luau performance gates,
//! measured headless on the example panel. Prints a table, then the numbers as JSON.

fn main() {
    let report = luau_panel::bench::run(luau_panel::bench::Scale::Native);
    println!("{report}");
    println!("{}", report.json());
}
