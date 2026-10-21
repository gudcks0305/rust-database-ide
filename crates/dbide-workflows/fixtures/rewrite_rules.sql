-- Definitions only: the test captures metadata and never executes the DML that would fire rules.
CREATE RULE original_insert_also AS ON INSERT TO __DBIDE_RULE_SCHEMA__.original
    DO ALSO INSERT INTO __DBIDE_RULE_SCHEMA__.audit (id,payload,event) VALUES (NEW.id,NEW.payload,'insert:v1');
CREATE RULE original_update_also AS ON UPDATE TO __DBIDE_RULE_SCHEMA__.original
    DO ALSO INSERT INTO __DBIDE_RULE_SCHEMA__.audit (id,payload,event) VALUES (NEW.id,NEW.payload,'update');
CREATE RULE original_delete_also AS ON DELETE TO __DBIDE_RULE_SCHEMA__.original
    DO ALSO INSERT INTO __DBIDE_RULE_SCHEMA__.audit (id,payload,event) VALUES (OLD.id,OLD.payload,'delete');
CREATE RULE original_instead_insert AS ON INSERT TO __DBIDE_RULE_SCHEMA__.instead_table
    DO INSTEAD NOTHING;
