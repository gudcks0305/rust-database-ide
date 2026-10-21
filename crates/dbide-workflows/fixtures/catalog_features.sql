-- OPTIONAL setup for the project-owned fixture only. The read-only workflow never runs this.
-- Parent integration runner may explicitly load it into dbide_fixture at 127.0.0.1:55439.
CREATE SCHEMA IF NOT EXISTS dbide_workflow_features;
CREATE TABLE IF NOT EXISTS dbide_workflow_features.parent (
    id bigint NOT NULL,
    code text NOT NULL,
    CONSTRAINT parent_composite_pk PRIMARY KEY (id,code)
);
CREATE TABLE IF NOT EXISTS dbide_workflow_features.child (
    id bigint PRIMARY KEY,
    parent_id bigint,
    parent_code text,
    CONSTRAINT child_parent_fk FOREIGN KEY (parent_id,parent_code)
      REFERENCES dbide_workflow_features.parent(id,code)
      ON DELETE CASCADE ON UPDATE RESTRICT DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE IF NOT EXISTS dbide_workflow_features.special (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    amount integer NOT NULL DEFAULT 1 CHECK (amount>0),
    doubled integer GENERATED ALWAYS AS (amount*2) STORED
);
CREATE INDEX IF NOT EXISTS special_expression ON dbide_workflow_features.special ((amount+1)) WHERE amount>1;
CREATE INDEX IF NOT EXISTS special_ordered ON dbide_workflow_features.special (amount DESC) INCLUDE (id);
CREATE OR REPLACE VIEW dbide_workflow_features.parent_view AS SELECT id,code FROM dbide_workflow_features.parent;
