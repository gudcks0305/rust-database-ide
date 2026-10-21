-- Project-owned loopback fixture only. Apply to dbide_console_b, not dbide_fixture.
-- Idempotent marker data for native cross-database table-tab acceptance.
CREATE TABLE IF NOT EXISTS public.dbide_ui_tabs_b (
    id integer PRIMARY KEY,
    label text NOT NULL
);
INSERT INTO public.dbide_ui_tabs_b (id, label)
VALUES (1, 'B marker 1'), (2, 'B marker 2'), (3, 'B marker 3')
ON CONFLICT (id) DO NOTHING;
