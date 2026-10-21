-- Generated fixture only. Run against the isolated dbide_fixture database.
-- Dedicated namespace: no DROP, no access to user schemas.
CREATE SCHEMA IF NOT EXISTS dbide_benchmark;
CREATE TABLE IF NOT EXISTS dbide_benchmark.customers (
    id bigint PRIMARY KEY,
    name text NOT NULL,
    email text,
    status text NOT NULL,
    balance numeric(18, 4),
    display_name text,
    created_at timestamptz,
    active boolean,
    metadata jsonb,
    optional_text text
);
INSERT INTO dbide_benchmark.customers
SELECT n,
       'Customer ' || n,
       'customer-' || n || '@example.invalid',
       CASE WHEN n % 3 = 0 THEN 'inactive' ELSE 'active' END,
       (n % 100000)::numeric / 100,
       '사용자 ' || n,
       timestamptz '2026-01-01 00:00:00+00' + n * interval '1 second',
       n % 2 = 0,
       jsonb_build_object('segment', n % 7),
       CASE WHEN n % 5 = 0 THEN NULL ELSE '' END
FROM generate_series(1, 1000000) AS series(n)
ON CONFLICT (id) DO NOTHING;
ANALYZE dbide_benchmark.customers;
