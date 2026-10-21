//! Typed app-owned tools messages. SQL, identifiers and external diagnostics are not translated.
use dbide_i18n::{Locale, Message, tr};
pub(crate) fn key(source: &str) -> Option<Message> {
    match source {
        "No profile captured · offline tools available" => {
            Some(Message::ToolsNoProfileCapturedOfflineToolsAvailable)
        }
        "Ready · database workflows use dedicated read-only sessions" => {
            Some(Message::ToolsReadyDatabaseWorkflowsUseDedicatedRead)
        }
        "Not loaded" => Some(Message::ToolsNotLoaded),
        "Action needs attention" => Some(Message::ToolsActionNeedsAttention),
        "Catalog captured · complete within explicit schema scope" => {
            Some(Message::ToolsCatalogCapturedCompleteWithinExplicitSchema)
        }
        "Desired schema loaded · compare direction: desired → captured actual" => {
            Some(Message::ToolsDesiredSchemaLoadedCompareDirectionDesired)
        }
        "Migration preview ready · no SQL executed" => {
            Some(Message::ToolsMigrationPreviewReadyNoSQLExecuted)
        }
        "Plan only · ANALYZE never requested · no actual timing metrics" => {
            Some(Message::ToolsPlanOnlyANALYZENeverRequestedNo)
        }
        "Tagged JSON loaded · select explicit unique non-NULL keys" => {
            Some(Message::ToolsTaggedJSONLoadedSelectExplicitUnique)
        }
        "Exact typed data comparison ready · before → after" => {
            Some(Message::ToolsExactTypedDataComparisonReadyBefore)
        }
        "SQL draft ready · copy/save or open a new console for explicit review; nothing executed" => {
            Some(Message::ToolsSQLDraftReadyCopySaveOr)
        }
        "Export saved atomically · existing files are never overwritten" => {
            Some(Message::ToolsExportSavedAtomicallyExistingFilesAre)
        }
        "File selection cancelled" => Some(Message::ToolsFileSelectionCancelled),
        "Operation failed · previous successful inputs retained" => {
            Some(Message::ToolsOperationFailedPreviousSuccessfulInputsRetained)
        }
        "Read-only worker stopped before returning a result" => {
            Some(Message::ToolsReadOnlyWorkerStoppedBeforeReturning)
        }
        "Cancelled · pending read-only session retiring; stale result discarded" => {
            Some(Message::ToolsCancelledPendingReadOnlySessionRetiring)
        }
        "Schema capture currently requires a PostgreSQL profile captured when this panel opens" => {
            Some(Message::ToolsSchemaCaptureCurrentlyRequiresAPostgreSQL)
        }
        "Capturing scoped catalog…" => Some(Message::ToolsCapturingScopedCatalog),
        "EXPLAIN currently requires a PostgreSQL profile captured when this panel opens" => {
            Some(Message::ToolsEXPLAINCurrentlyRequiresAPostgreSQLProfile)
        }
        "Planning exactly one statement…" => Some(Message::ToolsPlanningExactlyOneStatement),
        "Load desired schema and capture actual catalog first" => {
            Some(Message::ToolsLoadDesiredSchemaAndCaptureActual)
        }
        "Comparing desired → actual…" => Some(Message::ToolsComparingDesiredActual),
        "Open both before and after tagged JSON files first" => {
            Some(Message::ToolsOpenBothBeforeAndAfterTagged)
        }
        "Comparing typed rows by explicit identity…" => {
            Some(Message::ToolsComparingTypedRowsByExplicitIdentity)
        }
        "Choose local input…" => Some(Message::ToolsChooseLocalInput),
        "Open local schema or tagged JSON" => Some(Message::ToolsOpenLocalSchemaOrTaggedJSON),
        "No input file selected" => Some(Message::ToolsNoInputFileSelected),
        "Native file dialog failed" => Some(Message::ToolsNativeFileDialogFailed),
        "Could not open selected file" => Some(Message::ToolsCouldNotOpenSelectedFile),
        "File worker stopped" => Some(Message::ToolsFileWorkerStopped),
        "Create the requested output first" => Some(Message::ToolsCreateTheRequestedOutputFirst),
        "Generate an object SQL draft first" => Some(Message::ToolsGenerateAnObjectSQLDraftFirst),
        "Choose new export destination…" => Some(Message::ToolsChooseNewExportDestination),
        "Native save dialog failed" => Some(Message::ToolsNativeSaveDialogFailed),
        "Export worker stopped" => Some(Message::ToolsExportWorkerStopped),
        "Capture a complete PostgreSQL catalog within 1–32 explicitly selected schemas." => {
            Some(Message::ToolsCaptureACompletePostgreSQLCatalogWithin)
        }
        "Schema scope, comma separated" => Some(Message::ToolsSchemaScopeCommaSeparated),
        "Capture schema" => Some(Message::ToolsCaptureSchema),
        "Save snapshot JSON…" => Some(Message::ToolsSaveSnapshotJSON),
        "Open desired JSON / DDL…" => Some(Message::ToolsOpenDesiredJSONDDL),
        "PostgreSQL-only live catalog" => Some(Message::ToolsPostgreSQLOnlyLiveCatalog),
        "Schema capture, object browsing/drafts and EXPLAIN require a PostgreSQL profile. This captured engine is not supported by these live workflows; offline desired-schema and data compare remain available." => {
            Some(Message::ToolsSchemaCaptureObjectBrowsingDraftsAnd)
        }
        "Captured actual" => Some(Message::ToolsCapturedActual),
        "{} schemas · {tables} tables · {columns} columns" => {
            Some(Message::ToolsSchemasTablesTablesColumnsColumns)
        }
        "Scope: {} · complete: {}" => Some(Message::ToolsScopeComplete),
        "Database: {} · {:?}" => Some(Message::ToolsDatabase),
        "Desired schema" => Some(Message::ToolsDesiredSchema),
        "{} · {} tables" => Some(Message::ToolsTables),
        "Scope: {} · {:?}" => Some(Message::ToolsScope),
        "Catalog and plan requests have 15 s statement limits; parent profile changes do not retarget this panel. Files are processed in background workers. SQLite/MySQL live tools remain unavailable here." => {
            Some(Message::ToolsCatalogAndPlanRequestsHave15)
        }
        "Desired → captured actual" => Some(Message::ToolsDesiredCapturedActual),
        "Preview operations TO actual to make it match desired." => {
            Some(Message::ToolsPreviewOperationsTOActualToMake)
        }
        "Preview only. No database apply action is exposed; exported SQL requires separate review and execution." => {
            Some(Message::ToolsPreviewOnlyNoDatabaseApplyAction)
        }
        "Compare schemas" => Some(Message::ToolsCompareSchemas),
        "Export SQL preview…" => Some(Message::ToolsExportSQLPreview),
        "{} operations · destructive: {} · unsupported: {}" => {
            Some(Message::ToolsOperationsDestructiveUnsupported)
        }
        "Desired: {} → Actual: {} · implicit commit: {}" => {
            Some(Message::ToolsDesiredActualImplicitCommit)
        }
        "Migration operations, desired to actual" => {
            Some(Message::ToolsMigrationOperationsDesiredToActual)
        }
        "Destructive: {} · data validation: {} · blocked: {}" => {
            Some(Message::ToolsDestructiveDataValidationBlocked)
        }
        "Preview capped at 300 operations. Export contains the complete bounded plan." => {
            Some(Message::ToolsPreviewCappedAt300OperationsExport)
        }
        "Capture actual catalog and load desired schema in Schema tab, then compare." => {
            Some(Message::ToolsCaptureActualCatalogAndLoadDesired)
        }
        "Export ER SVG…" => Some(Message::ToolsExportERSVG),
        "80 tables · 24 columns/table · 200 edges maximum" => {
            Some(Message::Tools80Tables24ColumnsTable200)
        }
        "{} tables · {} foreign keys" => Some(Message::ToolsTablesForeignKeys),
        "Foreign keys from captured catalog. No inferred relationships. SVG preview and accessible table share the same bounded graph." => {
            Some(Message::ToolsForeignKeysFromCapturedCatalogNo)
        }
        "Foreign key relationships" => Some(Message::ToolsForeignKeyRelationships),
        "Outside displayed scope" => Some(Message::ToolsOutsideDisplayedScope),
        "unnamed foreign key" => Some(Message::ToolsUnnamedForeignKey),
        "Target in scope: {} · column endpoints visible: {}" => {
            Some(Message::ToolsTargetInScopeColumnEndpointsVisible)
        }
        "ER generation failed: {e}" => Some(Message::ToolsERGenerationFailedE),
        "Capture a schema first. Relationships and SVG will be generated from that exact snapshot." => {
            Some(Message::ToolsCaptureASchemaFirstRelationshipsAnd)
        }
        "Plan only · no ANALYZE" => Some(Message::ToolsPlanOnlyNoANALYZE),
        "Exactly one parsed SQL statement, planned in a dedicated read-only transaction." => {
            Some(Message::ToolsExactlyOneParsedSQLStatementPlanned)
        }
        "Estimated costs/rows are planner estimates. Planning may take locks; statement and lock timeouts are enforced." => {
            Some(Message::ToolsEstimatedCostsRowsArePlannerEstimates)
        }
        "Exactly one SQL statement for plan-only EXPLAIN" => {
            Some(Message::ToolsExactlyOneSQLStatementForPlan)
        }
        "EXPLAIN plan only" => Some(Message::ToolsEXPLAINPlanOnly),
        "EXPLAIN unavailable for captured engine" => {
            Some(Message::ToolsEXPLAINUnavailableForCapturedEngine)
        }
        "PostgreSQL plan-only workflow is disabled. Reopen tools with a PostgreSQL profile to request a live plan." => {
            Some(Message::ToolsPostgreSQLPlanOnlyWorkflowIsDisabled)
        }
        "SQL captured for this plan" => Some(Message::ToolsSQLCapturedForThisPlan),
        "SQL input changed; displayed plan belongs to the captured SQL above. Run EXPLAIN again to refresh." => {
            Some(Message::ToolsSQLInputChangedDisplayedPlanBelongs)
        }
        "Expand or collapse nodes · preview capped at 300 visible nodes." => {
            Some(Message::ToolsExpandOrCollapseNodesPreviewCapped)
        }
        "Estimated query plan tree" => Some(Message::ToolsEstimatedQueryPlanTree),
        "{} · estimated cost {} · estimated rows {}" => {
            Some(Message::ToolsEstimatedCostEstimatedRows)
        }
        "cost {}…{} · rows {} · width {}" => Some(Message::ToolsCostRowsWidth),
        "Complete tagged JSON inputs only · exact cell kinds and values · 10,000 rows / 16 MiB per input. Before → after; no positional or guessed identity." => {
            Some(Message::ToolsCompleteTaggedJSONInputsOnlyExact)
        }
        "Open before JSON…" => Some(Message::ToolsOpenBeforeJSON),
        "Open after JSON…" => Some(Message::ToolsOpenAfterJSON),
        "Comparison inputs" => Some(Message::ToolsComparisonInputs),
        "Before: {} · {} rows" => Some(Message::ToolsBeforeRows),
        "After: {} · {} rows" => Some(Message::ToolsAfterRows),
        "Explicit key column names, comma separated" => {
            Some(Message::ToolsExplicitKeyColumnNamesCommaSeparated)
        }
        "Compare by explicit keys" => Some(Message::ToolsCompareByExplicitKeys),
        "Available columns: {}" => Some(Message::ToolsAvailableColumns),
        "{} inserted · {} deleted · {} changed · {} unchanged" => {
            Some(Message::ToolsInsertedDeletedChangedUnchanged)
        }
        "Compared complete inputs: {} → {} rows · key column indices: {:?}" => {
            Some(Message::ToolsComparedCompleteInputsRowsKeyColumn)
        }
        "Exact typed row differences, before to after" => {
            Some(Message::ToolsExactTypedRowDifferencesBeforeTo)
        }
        "{}. {} · key {}" => Some(Message::ToolsKey),
        "Before: {}" => Some(Message::ToolsBefore),
        "After: {}" => Some(Message::ToolsAfter),
        "Row preview capped at 100 changes; summary counts cover all loaded rows." => {
            Some(Message::ToolsRowPreviewCappedAt100Changes)
        }
        "Database tools" => Some(Message::ToolsDatabaseTools),
        "Connection fixed at panel open · dedicated read-only sessions" => {
            Some(Message::ToolsConnectionFixedAtPanelOpenDedicated)
        }
        "Cancel" => Some(Message::ToolsCancel),
        "Capture a PostgreSQL schema before generating object drafts" => {
            Some(Message::ToolsCaptureAPostgreSQLSchemaBeforeGenerating)
        }
        "Validating exactly one object SQL draft…" => {
            Some(Message::ToolsValidatingExactlyOneObjectSQLDraft)
        }
        "Select a captured table for the index draft" => {
            Some(Message::ToolsSelectACapturedTableForThe)
        }
        "Select a captured ordinary view to draft its removal" => {
            Some(Message::ToolsSelectACapturedOrdinaryViewTo)
        }
        "Capture a PostgreSQL schema first" => Some(Message::ToolsCaptureAPostgreSQLSchemaFirst),
        "Validating destructive DROP INDEX draft…" => {
            Some(Message::ToolsValidatingDestructiveDROPINDEXDraft)
        }
        "Index not present in captured table" => Some(Message::ToolsIndexNotPresentInCapturedTable),
        "SQL draft sent to a new console for review · not executed" => {
            Some(Message::ToolsSQLDraftSentToANew)
        }
        "Captured PostgreSQL objects" => Some(Message::ToolsCapturedPostgreSQLObjects),
        "Browse actual table/view metadata, then generate one SQL statement for review." => {
            Some(Message::ToolsBrowseActualTableViewMetadataThen)
        }
        "Drafts never execute here. Copy, save or open a new console with the captured profile. Run remains an explicit workbench action." => {
            Some(Message::ToolsDraftsNeverExecuteHereCopySave)
        }
        "Capture schema in Schema tab to load actual objects." => {
            Some(Message::ToolsCaptureSchemaInSchemaTabTo)
        }
        "Live object capture and PostgreSQL object drafts are disabled for this captured engine. Reopen tools with a PostgreSQL profile." => {
            Some(Message::ToolsLiveObjectCaptureAndPostgreSQLObject)
        }
        "{total_tables} tables · {total_views} views · captured scope {}" => {
            Some(Message::ToolsTotalTablesTablesTotalViewsViews)
        }
        "Table {}{}" => Some(Message::ToolsTable),
        "Materialized view" => Some(Message::ToolsMaterializedView),
        "View" => Some(Message::ToolsView),
        "{} unsupported objects in {} · showing first 100 markers; full details in snapshot JSON" => {
            Some(Message::ToolsUnsupportedObjectsInShowingFirst100)
        }
        "Unsupported catalog object in {}: {}" => Some(Message::ToolsUnsupportedCatalogObjectIn),
        "{} columns · {} foreign keys · {} indexes" => {
            Some(Message::ToolsColumnsForeignKeysIndexes)
        }
        "Unsupported table feature: {}" => Some(Message::ToolsUnsupportedTableFeature),
        "Captured columns and defaults" => Some(Message::ToolsCapturedColumnsAndDefaults),
        "Nullable: {} · default: {} · generated/identity: {}" => {
            Some(Message::ToolsNullableDefaultGeneratedIdentity)
        }
        "Column preview capped at 300. Full bounded capture remains available in snapshot JSON." => {
            Some(Message::ToolsColumnPreviewCappedAt300Full)
        }
        "Primary key · {}" => Some(Message::ToolsPrimaryKey),
        "Foreign key · {}" => Some(Message::ToolsForeignKey),
        "{} → {} ({}) · DELETE {} · UPDATE {} · deferrable {}" => {
            Some(Message::ToolsDELETEUPDATEDeferrable)
        }
        "Index {} · {}{}" => Some(Message::ToolsIndex),
        "Columns: {} · expressions: {} · INCLUDE: {} · predicate: {}" => {
            Some(Message::ToolsColumnsExpressionsINCLUDEPredicate)
        }
        "Constraint owned: {} · unsupported metadata: {}" => {
            Some(Message::ToolsConstraintOwnedUnsupportedMetadata)
        }
        "Advanced definition shown from captured metadata; expression/predicate/INCLUDE/opclass/collation recreation is unsupported by this basic draft form." => {
            Some(Message::ToolsAdvancedDefinitionShownFromCapturedMetadata)
        }
        "Draft DROP INDEX · destructive" => Some(Message::ToolsDraftDROPINDEXDestructive),
        "Foreign key/index detail preview capped at 200 each; complete captured metadata remains in snapshot JSON." => {
            Some(Message::ToolsForeignKeyIndexDetailPreviewCapped)
        }
        "Create index draft · ordinary btree only" => {
            Some(Message::ToolsCreateIndexDraftOrdinaryBtreeOnly)
        }
        "New index name" => Some(Message::ToolsNewIndexName),
        "Exact captured column names for index, comma separated" => {
            Some(Message::ToolsExactCapturedColumnNamesForIndex)
        }
        "Unique: yes" => Some(Message::ToolsUniqueYes),
        "Unique: no" => Some(Message::ToolsUniqueNo),
        "Generate CREATE INDEX draft" => Some(Message::ToolsGenerateCREATEINDEXDraft),
        "Captured {} {}" => Some(Message::ToolsCaptured),
        "materialized view" => Some(Message::ToolsMaterializedView2),
        "Materialized view refresh/storage/drop/create is unsupported in this basic view form." => {
            Some(Message::ToolsMaterializedViewRefreshStorageDropCreate)
        }
        "Definition captured from PostgreSQL. View owner, ACL, reloptions/security settings and triggers are outside this basic form." => {
            Some(Message::ToolsDefinitionCapturedFromPostgreSQLViewOwner)
        }
        "Ordinary view SQL drafts" => Some(Message::ToolsOrdinaryViewSQLDrafts),
        "Schema and view name are quoted as identifiers. Definition must parse as one query; review functions and dependencies before running. Existing ordinary views require explicit destructive Replace." => {
            Some(Message::ToolsSchemaAndViewNameAreQuoted)
        }
        "View schema in captured scope" => Some(Message::ToolsViewSchemaInCapturedScope),
        "View name" => Some(Message::ToolsViewName),
        "View definition, exactly one query" => Some(Message::ToolsViewDefinitionExactlyOneQuery),
        "Draft CREATE VIEW" => Some(Message::ToolsDraftCREATEVIEW),
        "Draft REPLACE VIEW · destructive" => Some(Message::ToolsDraftREPLACEVIEWDestructive),
        "Draft DROP VIEW · destructive" => Some(Message::ToolsDraftDROPVIEWDestructive),
        "DESTRUCTIVE SQL draft · review required" => {
            Some(Message::ToolsDESTRUCTIVESQLDraftReviewRequired)
        }
        "SQL draft · review required" => Some(Message::ToolsSQLDraftReviewRequired),
        "This draft belongs to the captured target above. It is unchanged by later form edits; regenerate to reflect edits. No SQL has been executed." => {
            Some(Message::ToolsThisDraftBelongsToTheCaptured)
        }
        "Copy SQL draft" => Some(Message::ToolsCopySQLDraft),
        "SQL draft copied · nothing executed" => Some(Message::ToolsSQLDraftCopiedNothingExecuted),
        "Save SQL draft…" => Some(Message::ToolsSaveSQLDraft),
        "Open draft in new console" => Some(Message::ToolsOpenDraftInNewConsole),
        "Identifiers require 1–63 UTF-8 bytes and no control characters" => {
            Some(Message::ToolsIdentifiersRequire163UTF8)
        }
        "Object SQL draft exceeds 64 KB" => Some(Message::ToolsObjectSQLDraftExceeds64KB),
        "Object draft must parse as PostgreSQL SQL" => {
            Some(Message::ToolsObjectDraftMustParseAsPostgreSQL)
        }
        "Object draft requires exactly one statement" => {
            Some(Message::ToolsObjectDraftRequiresExactlyOneStatement)
        }
        "Unsupported object draft statement" => Some(Message::ToolsUnsupportedObjectDraftStatement),
        "Table no longer exists in the captured snapshot; select a table again" => {
            Some(Message::ToolsTableNoLongerExistsInThe)
        }
        "View no longer exists in the captured snapshot; select a view again" => {
            Some(Message::ToolsViewNoLongerExistsInThe)
        }
        "Index name conflicts with a relation in the captured schema" => {
            Some(Message::ToolsIndexNameConflictsWithARelation)
        }
        "Choose distinct exact captured column names, comma separated; expressions/predicates/INCLUDE are unsupported" => {
            Some(Message::ToolsChooseDistinctExactCapturedColumnNames)
        }
        "Create ordinary btree index · review locking, uniqueness validation and name conflicts" => {
            Some(Message::ToolsCreateOrdinaryBtreeIndexReviewLocking)
        }
        "Constraint-owned index cannot be dropped independently; manage its constraint separately" => {
            Some(Message::ToolsConstraintOwnedIndexCannotBeDropped)
        }
        "DESTRUCTIVE · remove index · RESTRICT prevents dependent-object cascade" => {
            Some(Message::ToolsDESTRUCTIVERemoveIndexRESTRICTPreventsDependent)
        }
        "View schema must belong to the explicit captured scope" => {
            Some(Message::ToolsViewSchemaMustBelongToThe)
        }
        "View name conflicts with a captured relation; choose a new name or explicit Replace for an ordinary view" => {
            Some(Message::ToolsViewNameConflictsWithACaptured)
        }
        "Materialized view management is unsupported; no ordinary view draft generated" => {
            Some(Message::ToolsMaterializedViewManagementIsUnsupportedNo)
        }
        "Captured view already exists; choose explicit Replace view draft" => {
            Some(Message::ToolsCapturedViewAlreadyExistsChooseExplicit)
        }
        "Replace requires an existing ordinary view in the captured snapshot" => {
            Some(Message::ToolsReplaceRequiresAnExistingOrdinaryView)
        }
        "View definition must be exactly one SELECT/query statement" => {
            Some(Message::ToolsViewDefinitionMustBeExactlyOne)
        }
        "DESTRUCTIVE · replace ordinary view definition · review column/dependency compatibility" => {
            Some(Message::ToolsDESTRUCTIVEReplaceOrdinaryViewDefinitionReview)
        }
        "Create ordinary view · query functions and dependencies require review" => {
            Some(Message::ToolsCreateOrdinaryViewQueryFunctionsAnd)
        }
        "Materialized view management is unsupported" => {
            Some(Message::ToolsMaterializedViewManagementIsUnsupported)
        }
        "DESTRUCTIVE · remove ordinary view · RESTRICT prevents dependent-object cascade" => {
            Some(Message::ToolsDESTRUCTIVERemoveOrdinaryViewRESTRICTPrevents)
        }
        "Schema" => Some(Message::ToolsSchema),
        "Objects" => Some(Message::ToolsObjects),
        "Migration" => Some(Message::ToolsMigration),
        "Relationships" => Some(Message::ToolsRelationships),
        "EXPLAIN" => Some(Message::ToolsEXPLAIN),
        "Data compare" => Some(Message::ToolsDataCompare),
        "Choose 1–32 distinct user schemas, comma separated (system schemas excluded)" => {
            Some(Message::ToolsChoose132DistinctUserSchemas)
        }
        "Enter distinct, unambiguous column names separated by commas; names must match exactly" => {
            Some(Message::ToolsEnterDistinctUnambiguousColumnNamesSeparated)
        }
        "Operation cancelled" => Some(Message::ToolsOperationCancelled),
        "Could not start read-only tools runtime" => Some(Message::ToolsCouldNotStartReadOnlyTools),
        "Operation cancelled; dedicated read-only session retired" => {
            Some(Message::ToolsOperationCancelledDedicatedReadOnlySession)
        }
        "Tools operation timed out; dedicated read-only session retired" => {
            Some(Message::ToolsToolsOperationTimedOutDedicatedRead)
        }
        "Could not open selected input" => Some(Message::ToolsCouldNotOpenSelectedInput),
        "Could not read selected input" => Some(Message::ToolsCouldNotReadSelectedInput),
        "Schema input exceeds 16 MB limit" => Some(Message::ToolsSchemaInputExceeds16MBLimit),
        "Schema input must be UTF-8" => Some(Message::ToolsSchemaInputMustBeUTF8),
        "No file selected" => Some(Message::ToolsNoFileSelected),
        "unnamed" => Some(Message::ToolsUnnamed),
        "none" => Some(Message::ToolsNone),
        "Tools operation failed: {error}" => Some(Message::ToolsToolsOperationFailedError),
        "GPUI built-in component text may remain English." => {
            Some(Message::ToolsGPUIBuiltInComponentTextMay)
        }
        _ => None,
    }
}
pub(crate) fn text(locale: Locale, source: &str) -> &str {
    key(source)
        .map(|message| tr(locale, message))
        .unwrap_or(source)
}
/// Display summaries are finite previews; never use translated output as SQL or a file payload.
pub(crate) fn format(locale: Locale, message: Message, args: &[(&str, String)]) -> String {
    let values = args
        .iter()
        .map(|(name, value)| {
            let value = if value.len() > dbide_i18n::MAX_ARGUMENT_BYTES {
                let mut end = dbide_i18n::MAX_ARGUMENT_BYTES - 3;
                while !value.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}…", &value[..end])
            } else {
                value.clone()
            };
            (*name, value)
        })
        .collect::<Vec<_>>();
    let refs = values
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect::<Vec<_>>();
    dbide_i18n::format(locale, message, &refs)
        .expect("tools catalog and call-site placeholders agree")
}

/// Origin is explicit: a server diagnostic matching an app key must still remain verbatim.
#[derive(Clone)]
pub(crate) enum Diagnostic {
    Owned(String),
    Raw(String),
}
impl Diagnostic {
    pub(crate) fn body(&self, locale: Locale) -> &str {
        match self {
            Self::Owned(value) => text(locale, value),
            Self::Raw(value) => value,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_diagnostic_never_uses_matching_app_translation() {
        let raw = tr(Locale::En, Message::ToolsNoFileSelected).to_owned();
        let diagnostic = Diagnostic::Raw(raw.clone());
        for &locale in Locale::ALL {
            assert_eq!(diagnostic.body(locale), raw);
        }
    }
}
