use std::{path::PathBuf, process::ExitCode};

const USAGE: &str = "Usage: dbide-mcp --profile-file <explicit-json-path> --profile-id <exact-id>\nOptional password: DBIDE_MCP_PASSWORD environment variable. No default file, Keychain or profile discovery.";

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dbide-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args_os().skip(1);
    let mut path = None;
    let mut id = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--profile-file") if path.is_none() => {
                path = Some(PathBuf::from(args.next().ok_or(USAGE)?))
            }
            Some("--profile-id") if id.is_none() => {
                id = Some(
                    args.next()
                        .and_then(|v| v.into_string().ok())
                        .ok_or(USAGE)?,
                )
            }
            Some("--help" | "-h") => {
                eprintln!("{USAGE}");
                return Ok(());
            }
            _ => return Err(USAGE.into()),
        }
    }
    let path = path.ok_or(USAGE)?;
    let id = id.ok_or(USAGE)?;
    let password = match std::env::var("DBIDE_MCP_PASSWORD") {
        Ok(v) => v,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(_) => return Err("DBIDE_MCP_PASSWORD must be UTF-8".into()),
    };
    let request = dbide_mcp::request_from_file(&path, &id, password)?;
    dbide_mcp::serve_stdio(request).await
}
