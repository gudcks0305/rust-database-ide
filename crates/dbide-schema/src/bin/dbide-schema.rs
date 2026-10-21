use dbide_schema::{DatabaseIdentity, SchemaError, SchemaSnapshot, ddl, diff, er, explain};
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

fn read(path: &str) -> Result<String, SchemaError> {
    if fs::metadata(path)
        .map_err(|e| SchemaError(e.to_string()))?
        .len()
        > 32_000_000
    {
        return Err(SchemaError("input exceeds 32 MB cap".into()));
    }
    fs::read_to_string(path).map_err(|e| SchemaError(e.to_string()))
}
fn write_new(path: &str, data: &str) -> Result<(), SchemaError> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| SchemaError(format!("cannot create output {path}: {e}")))?;
    output
        .write_all(data.as_bytes())
        .map_err(|e| SchemaError(e.to_string()))
}
fn run() -> Result<(), SchemaError> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["ddl-import",input,output] | ["ddl-import",input,output,_] => {
            let default_schema=args.get(3).map(String::as_str).unwrap_or("public");
            let snapshot=ddl::import_postgres_ddl(&read(input)?,DatabaseIdentity { engine:"postgresql".into(),identity:format!("offline:{}",Path::new(input).display()),name:"offline".into(),server_version:None },default_schema,input)?;
            write_new(output,&snapshot.to_json()?)
        }
        ["ddl-export",input,output] => write_new(output,&ddl::export_postgres_ddl(&SchemaSnapshot::from_json(&read(input)?)?)?),
        ["diff",desired,actual,output] => {
            let plan=diff(&SchemaSnapshot::from_json(&read(desired)?)?,&SchemaSnapshot::from_json(&read(actual)?)?)?;
            write_new(output,&serde_json::to_string_pretty(&plan)?)
        }
        ["er-svg",input,output] => write_new(output,&er::export_svg(&SchemaSnapshot::from_json(&read(input)?)?,er::ErLimits::default())?),
        ["explain",input,output,mode] => {
            let mode=match *mode { "plan"=>explain::ExecutionMode::PlanOnly,"analyze-executed"=>explain::ExecutionMode::AnalyzeExecuted,"unknown"=>explain::ExecutionMode::Unknown,_=>return Err(SchemaError("explain mode must be plan/analyze-executed/unknown".into())) };
            write_new(output,&serde_json::to_string_pretty(&explain::parse_postgres_explain(&read(input)?,mode)?)?)
        }
        _ => Err(SchemaError("Offline only. Usage: dbide-schema ddl-import INPUT.sql OUTPUT.json [DEFAULT_SCHEMA] | ddl-export INPUT.json OUTPUT.sql | diff DESIRED.json ACTUAL.json PLAN.json | er-svg INPUT.json OUTPUT.svg | explain INPUT.json OUTPUT.json plan|analyze-executed|unknown. Output files must not exist. No apply or database connection command.".into())),
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
