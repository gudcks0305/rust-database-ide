-- OPTIONAL idempotent setup for the project-owned PostgreSQL fixture only.
-- Parent integration runner may explicitly load this; workflows never execute setup SQL.
-- The nondeterministic collation case requires the fixture's ICU-enabled PostgreSQL build.
CREATE SCHEMA IF NOT EXISTS dbide_workflow_features;
CREATE TABLE IF NOT EXISTS dbide_workflow_features.fidelity_comments (
    id bigint PRIMARY KEY,
    value integer CHECK (value > 0),
    note text
);
COMMENT ON COLUMN dbide_workflow_features.fidelity_comments.note IS 'Owned fixture column comment';
CREATE TABLE IF NOT EXISTS dbide_workflow_features.fidelity_custom_collation (
    id text COLLATE "C" PRIMARY KEY
);
CREATE COLLATION IF NOT EXISTS dbide_workflow_features.fidelity_insensitive
    (provider=icu, locale='und-u-ks-level2', deterministic=false);
CREATE TABLE IF NOT EXISTS dbide_workflow_features.fidelity_nondeterministic (
    id text COLLATE dbide_workflow_features.fidelity_insensitive PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS dbide_workflow_features.fidelity_deferred (
    id bigint PRIMARY KEY DEFERRABLE INITIALLY DEFERRED,
    value integer UNIQUE DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE IF NOT EXISTS dbide_workflow_features.fidelity_unvalidated (value integer);
DO $dbide_fidelity_setup$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_catalog.pg_constraint con
        WHERE con.conrelid = 'dbide_workflow_features.fidelity_unvalidated'::pg_catalog.regclass
          AND con.conname = 'fidelity_unvalidated_check'
    ) THEN
        ALTER TABLE dbide_workflow_features.fidelity_unvalidated
            ADD CONSTRAINT fidelity_unvalidated_check CHECK (value > 0) NOT VALID;
    END IF;
END;
$dbide_fidelity_setup$;
CREATE TABLE IF NOT EXISTS dbide_workflow_features.fidelity_empty_edit (
    id bigint PRIMARY KEY,
    note text
);
