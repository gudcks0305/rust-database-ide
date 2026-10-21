#[tokio::test]
#[ignore = "requires DBIDE_TEST_PG_URL pointing to a disposable PostgreSQL fixture; explicit opt-in"]
async fn native_postgres_contract() {
    let url = std::env::var("DBIDE_TEST_PG_URL")
        .expect("DBIDE_TEST_PG_URL required; no fixture means failure, not pass");
    let evidence = tokio::time::timeout(
        std::time::Duration::from_secs(90),
        dbide_pg_spike::contract::verify(&url),
    )
    .await
    .expect("contract deadline")
    .expect("PostgreSQL driver contract");
    println!("{}", serde_json::to_string_pretty(&evidence).unwrap());
}
