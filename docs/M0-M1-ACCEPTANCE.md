# M0 / M1 acceptance checklist

Updated 2026-10-22. **Neither milestone is declared complete.** Later feature
components exist, but their existence does not advance the milestone status.
The user requested prioritizing basic database navigation and completing the
foundation. M3–M6 remains in scope and follows these core workflow repairs;
existing advanced work is preserved for later integration and acceptance.

## M0: framework and execution risks

| Gate from PLAN.md | Evidence / current result | Status |
|---|---|---|
| Korean composition, undo, selection | Unicode paste and undo observed; automated key injection also produced separated Jamo in native NSTextView, so it cannot establish physical IME behavior | Partial: physical marked-text/candidate interaction unverified |
| VoiceOver / accessibility | Native AX exposes editor, buttons and actual table cells; fake filler rows removed | Partial: complete screen-reader and keyboard workflow unverified |
| 1M logical rows × 200 columns | Lazy grid/far corner observed in initial native spike; no eager million-row storage | Passed for logical fixture; latest UI needs regression |
| Exact cancel and terminal state | Owned PG/SQLite/MySQL contract tests, late cancellation keys, backend cleanup and network fault cases | Passed for documented adapter contracts |
| Pinned sessions / temp tables / manual transactions | Actual PG ReadyForQuery state; dedicated A/B sessions, different database selection, TEMP retention, cancellation isolation and rollback2→1 passed native QA | Passed for documented PG behavior |
| Arbitrary SQL independent of parser success | Raw script path remains separate from formatting/analysis and current-statement selection | Implemented and tested contracts; native regression in M1 flow |
| Measurement protocol and first results | BENCHMARKS.md and hashed native smoke evidence exist; root callback is explicitly not first paint | Initial measurement exists; input/frame and matched DataGrip baseline remain unverified |

Historical evidence: [M0 results](M0-RESULTS.md),
[phase3 native QA](../evidence/ui-native-phase3-qa.json),
[core/recovery QA](../evidence/ui-native-core-recovery-qa.json),
[console/locale QA](../evidence/ui-console-locale-qa.json),
[integrated test checkpoint](../evidence/current-validation.json).
Unverified tests are not passed tests and are not proof of a framework defect.

## M1: one coherent PostgreSQL workflow

The acceptance run must use a single identified release binary and an isolated
profile store. It must not inspect user connection settings or databases.

- [x] General profiles, TLS validation, exact Keychain API, explicit session password.
- [x] Native connection failure feedback, edit settings and reconnect-on-Run.
- [x] Schema groups, searchable table/view list and expandable field metadata in the main explorer. Lazy connection/database/schema hierarchy and bounded scoped probes passed native PG/SQLite/MySQL navigation checks.
- [x] Structure view: names, types, NULL, defaults, PK/FK and available index definitions observed on owned PG/SQLite/MySQL fixtures; unsupported engine-specific details remain explicit.
- [x] SQL tabs, native file open/save and dirty-tab protection observed.
- [ ] Latest file shortcuts and physical external drag acceptance. Shared loader tests and native Open pass; cross-app drag is unverified.
- [x] Native PG query, multi-result, error, cancel and transaction state workflows observed on earlier release.
- [ ] Repeat the complete connection → explorer → SQL → result/error/cancel → history workflow on the basic-explorer release.
- [x] GUI and CLI concurrent setting changes preserve unrelated profiles; shared locale persistence preserves unknown settings.
- [x] Core Korean/English switching, Cmd+, and locale persistence after restart. Tools runtime switching also preserves form input; table-view runtime switching also preserved page2 and search text. Shared low-level result captions and some GPUI text remain English.
- [x] Per-console configured DB restoration without opening connections; native PG A/B isolation.
- [ ] Complete keyboard/focus/AX regression on the same release.
- [ ] Matched DataGrip task baseline; no speedup claim before it exists.

File recovery is a useful foundational improvement but remains tracked separately
from completed file-open/save acceptance. SQL restoration must not connect or run.

## Deferred work and visible limits

M2 row editing is **not accepted**. A verified namespace rename/rebinding race
can bypass the current captured-table guard. The reviewable UI must keep Apply
disabled; read-only table data, structure, paging and SQL console remain available.
See [PG blocked acceptance](PG-SPIKE.md#blocked-m2-acceptance-namespace-rebinding).
Later AI/advanced tool components remain preserved and are not counted as phase
completion. No acceptance requirement is removed to make a milestone appear done.
