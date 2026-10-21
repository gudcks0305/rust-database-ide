fn main() {
    let report = dbide_sqlite_spike::verify();
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("serialize fixture report")
    );
    if !report.passed {
        std::process::exit(1);
    }
}
