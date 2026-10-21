-- Explicit seed for the project-owned loopback fixture only.
-- Existing rows are never replaced. Reapplying to a nonempty table is a no-op.
BEGIN;
CREATE SCHEMA IF NOT EXISTS dbide_ui_workbench;
CREATE TABLE IF NOT EXISTS dbide_ui_workbench.items (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL,
    notes text,
    amount numeric(12, 2) NOT NULL DEFAULT 0,
    active boolean NOT NULL DEFAULT true,
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO dbide_ui_workbench.items(name, notes, amount, payload)
SELECT 'QA item ' || n,
       CASE WHEN n = 1 THEN E'first line\nsecond line\r\nthird line'
            WHEN n = 2 THEN NULL WHEN n = 3 THEN '' ELSE 'note ' || n END,
       n * 1.25,
       jsonb_build_object('number', n, 'text', '한글')
FROM generate_series(1, 105) AS n
WHERE NOT EXISTS (SELECT 1 FROM dbide_ui_workbench.items);
COMMIT;
