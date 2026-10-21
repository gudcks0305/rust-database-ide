//! This explicitly ignored fixture test creates only a unique owned schema, then cleans it up.
//! It never executes INSERT/UPDATE/DELETE against tables that have the rules under examination.
use dbide_pg_spike::{CommitPolicy, Event, Limits, Outcome, Session};
use dbide_profiles::{ConnectionProfile, ConnectionRequest};
use dbide_workflows::{
    CaptureLimits, capture_schema, capture_target_table_for_edit,
    metadata_features::USER_REWRITE_RULES,
};
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture() -> ConnectionRequest {
    assert_eq!(
        std::env::var("DBIDE_WORKFLOW_TEST_URL").expect("explicit owned fixture URL required"),
        "postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture",
        "rewrite fixture test only permits the exact project-owned loopback database"
    );
    ConnectionRequest {
        profile: ConnectionProfile::fixture(),
        password: "dbide_fixture_only".into(),
    }
}
async fn sql(session: &Session, text: &str) -> Result<dbide_pg_spike::ExecutionReport, String> {
    let report = session
        .execute(text, CommitPolicy::Auto)
        .await
        .map_err(str::to_owned)?;
    if report.outcome != Outcome::Completed
        || report.terminal_error.is_some()
        || report.truncated
        || report.connection_retired
    {
        return Err(format!(
            "owned fixture SQL failed (SQLSTATE {})",
            report
                .terminal_error
                .as_ref()
                .and_then(|e| e.sqlstate.as_deref())
                .unwrap_or("unknown")
        ));
    }
    Ok(report)
}
fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

#[tokio::test]
#[ignore = "explicitly creates/drops one unique schema on the owned fixture; metadata reads only between rule DDL changes"]
async fn live_original_table_rules_are_unsupported_and_definition_change_invalidates_hash() {
    let request = fixture();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let schema = format!("dbide_workflow_rules_{}_{stamp}", std::process::id());
    assert!(schema.starts_with("dbide_workflow_rules_") && schema.len() <= 63);
    let quoted = dbide_schema::quote_ident(&schema);
    let session = Session::connect_request(
        &request,
        Limits {
            retained_bytes: 2_000_000,
            events: 256,
            sql_bytes: 64_000,
        },
    )
    .await
    .unwrap();
    // Confirm ownership before any cleanup path: never drop a coincidentally pre-existing schema.
    if let Err(error) = sql(&session, &format!("CREATE SCHEMA {quoted}")).await {
        session.close();
        panic!("unique owned rewrite fixture schema creation failed: {error}");
    }
    // All fallible checks inside work return errors, so cleanup happens before the test assertion.
    let work=async {
        sql(&session,"SET statement_timeout='5s'; SET lock_timeout='2s'").await?;
        let setup=include_str!("../fixtures/rewrite_rules_base.sql").replace("__DBIDE_RULE_SCHEMA__",&quoted);
        sql(&session,&setup).await?;
        let before=capture_target_table_for_edit(&request,&schema,"original",CaptureLimits::default()).await.map_err(|e|e.to_string())?;
        require(!before.snapshot.schemas[0].tables[0].unsupported_features.iter().any(|f| f==USER_REWRITE_RULES),"plain original table was falsely marked as owning rules")?;
        let rules=include_str!("../fixtures/rewrite_rules.sql").replace("__DBIDE_RULE_SCHEMA__",&quoted);
        sql(&session,&rules).await?;
        let original=capture_target_table_for_edit(&request,&schema,"original",CaptureLimits::default()).await.map_err(|e|e.to_string())?;
        require(original.relation_oid==before.relation_oid,"rule installation unexpectedly replaced the original table")?;
        require(original.snapshot.schemas[0].tables[0].unsupported_features.iter().any(|f| f==USER_REWRITE_RULES),"ALSO INSERT/UPDATE/DELETE rules did not mark original table unsupported")?;
        let instead=capture_target_table_for_edit(&request,&schema,"instead_table",CaptureLimits::default()).await.map_err(|e|e.to_string())?;
        require(instead.snapshot.schemas[0].tables[0].unsupported_features.iter().any(|f| f==USER_REWRITE_RULES),"INSTEAD NOTHING rule did not mark ordinary table unsupported")?;
        let full=capture_schema(&request,std::slice::from_ref(&schema),CaptureLimits::default()).await.map_err(|e|e.to_string())?;
        require(full.schemas[0].views.iter().any(|v| v.name=="ordinary_view"),"normal view was not captured")?;
        require(!full.schemas[0].unsupported_objects.iter().any(|f| f=="view ordinary_view: user rewrite rules"),"view _RETURN rule produced ordinary-table rewrite marker")?;
        let definition=sql(&session,&dbide_pg_spike::table_metadata_query(original.relation_oid)).await?;
        let json=definition.events.iter().find_map(|e| if let Event::Row {values}=e { values.first().and_then(|s|s.as_deref()) } else { None }).ok_or("canonical rule JSON missing")?;
        let raw:serde_json::Value=serde_json::from_str(json).map_err(|e|e.to_string())?;
        let raw_rules=raw["rules"].as_array().ok_or("canonical metadata omitted raw rule array")?;
        for name in ["original_insert_also","original_update_also","original_delete_also"] {
            require(raw_rules.iter().any(|r| r["rulename"]==name && r["ev_action"].as_str().is_some_and(|s| !s.is_empty())),"canonical metadata omitted rule definition/action")?;
        }
        let replace=format!("CREATE OR REPLACE RULE original_insert_also AS ON INSERT TO {quoted}.original DO ALSO INSERT INTO {quoted}.audit (id,payload,event) VALUES (NEW.id,NEW.payload,'insert:v2');");
        sql(&session,&replace).await?;
        let changed=capture_target_table_for_edit(&request,&schema,"original",CaptureLimits::default()).await.map_err(|e|e.to_string())?;
        require(changed.relation_oid==original.relation_oid,"rule replacement changed relation OID")?;
        require(changed.snapshot.content_fingerprint().map_err(|e|e.to_string())?==original.snapshot.content_fingerprint().map_err(|e|e.to_string())?,"unchanged typed table marker unexpectedly depended on raw rule definition")?;
        require(changed.metadata_fingerprint!=original.metadata_fingerprint,"same-OID rule definition change did not invalidate canonical fingerprint")?;
        let counts=sql(&session,&format!("SELECT (SELECT count(*) FROM {quoted}.original),(SELECT count(*) FROM {quoted}.audit),(SELECT count(*) FROM {quoted}.instead_table)")).await?;
        require(counts.events.iter().any(|e| matches!(e,Event::Row {values} if values==&vec![Some("0".into()),Some("0".into()),Some("0".into())])),"rule fixture executed unexpected application DML")?;
        Ok::<(),String>(())
    }.await;
    let cleanup = sql(&session, &format!("DROP SCHEMA IF EXISTS {quoted} CASCADE")).await;
    session.close();
    cleanup.expect("unique owned rewrite fixture cleanup failed");
    work.expect("owned rewrite-rule metadata contract failed (fixture cleaned up)");
}
