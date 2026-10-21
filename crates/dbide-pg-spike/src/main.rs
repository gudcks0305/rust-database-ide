#[tokio::main]
async fn main() {
    let Ok(url) = std::env::var("DBIDE_TEST_PG_URL") else {
        eprintln!(
            "DBIDE_TEST_PG_URL is required; use only a disposable isolated PostgreSQL fixture"
        );
        std::process::exit(2);
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(90),
        dbide_pg_spike::contract::verify(&url),
    )
    .await
    {
        Ok(Ok(evidence)) => println!("{}", serde_json::to_string_pretty(&evidence).unwrap()),
        Ok(Err(error)) => {
            eprintln!("PostgreSQL contract FAILED: {error}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("PostgreSQL contract FAILED: 90 second deadline");
            std::process::exit(1);
        }
    }
}
