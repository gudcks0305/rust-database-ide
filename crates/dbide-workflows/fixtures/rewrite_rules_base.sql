-- Test-only template. The ignored owned-fixture test quotes a unique generated schema name.
-- No application data is modified. Every object is cleaned up by that test.
CREATE TABLE __DBIDE_RULE_SCHEMA__.audit (id bigint, payload text, event text);
CREATE TABLE __DBIDE_RULE_SCHEMA__.original (id bigint PRIMARY KEY, payload text);
CREATE TABLE __DBIDE_RULE_SCHEMA__.instead_table (id bigint PRIMARY KEY, payload text);
CREATE VIEW __DBIDE_RULE_SCHEMA__.ordinary_view AS SELECT id, payload FROM __DBIDE_RULE_SCHEMA__.audit;
