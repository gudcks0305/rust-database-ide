# Database tool workflow comparison

Checked 2026-10-22 against official documentation. This is a feature/workflow benchmark, not a runtime or performance benchmark. DataGrip pages identify 2026.2 Help; DBeaver and Beekeeper pages are rolling documentation and do not establish installed-version behavior. No competitor application or user database was exercised for this comparison. Performance measurement remains [BENCHMARKS.md](BENCHMARKS.md).

## Findings and chosen scope

| Workflow | Official product contract | Rust Database IDE scope |
|---|---|---|
| Project organization | DataGrip groups data sources, consoles, attached directories and settings; project metadata lives under `.idea`. [Projects](https://www.jetbrains.com/help/datagrip/working-with-projects.html) | One explicit directory, `.dbide/project.json`, ordinary SQL files and a default target reference. |
| External folders | DataGrip attaches existing directories in place; Detach removes association while Delete affects disk. [User files](https://www.jetbrains.com/help/datagrip/working-with-user-files.html) | Browser roots remain in place; closing the project preserves source files and open tabs. Multiple attached roots remain pending. |
| SQL association | DataGrip Files supports dialect selection and attaching data sources to SQL files. [Files tool window](https://www.jetbrains.com/help/datagrip/files-tool-window.html) | Project default profile/database applied to new file tabs; unresolved targets remain unbound. Per-file bindings remain pending. |
| Transfer and resources | DBeaver projects organize connections, scripts, diagrams and resources, with project import/export. [Projects](https://dbeaver.com/docs/dbeaver/Projects/) | Portable folder metadata references separately managed profiles; no full project archive or connection import/export claim. |
| Existing SQL directories | DBeaver can link an external directory and use it as the default save location. [Script management](https://dbeaver.com/docs/dbeaver/Script-Management/) | `queries/` default at the project root, browsable subfolders and source files edited in place. No linked-root registry yet. |
| Automation entry points | DBeaver documents `-data`, `-link-project`, `-unlink-project`, file opening and task execution. Its command page marks `-runTask` as PRO. [Command line](https://dbeaver.com/docs/dbeaver/Command-Line/) | Shared Rust project model, explicit CLI roots, bounded JSON discovery, CAS writes and guarded execution plans. No saved task scheduler. |
| Saved queries | Beekeeper Saved Queries uses local SQLite storage; imported SQL is copied and later original-file edits are not reflected. [Saved Queries](https://docs.beekeeperstudio.io/user_guide/sql_editor/saving_queries/) | Ordinary `.sql` files remain the common source for GUI, CLI and external agents; content digests detect save conflicts. |
| Agent context | Beekeeper AI Shell documents schema/relations, saved queries and open tabs as context, with permission-based SQL execution. [AI Shell](https://docs.beekeeperstudio.io/user_guide/sql-ai-shell/) | Offline capabilities/context, explicit file reads and plan/execute separation. Existing read-only MCP remains separate; automatic disclosure of SQL/history/results is not added. |

The selected vertical slice is folder → SQL file → explicit saved target → reviewable argv → separate execution. It reuses the existing editor, profile store and query CLI while giving external agents the same file model as GUI users. [Workspace contracts and examples](WORKSPACES.md) define the implemented limits. The scope does not establish DataGrip/DBeaver parity, full project isolation or AI integration acceptance.

## Follow-on work

Next project improvements are per-file target bindings, multiple attached roots, isolated tab sets and explicit refresh/conflict workflows with optional filesystem/Git watching. Saved task definitions need target/file revision guards and clear execution effects before scheduling. Parameter binding and foreign-key row navigation remain separate database workflow gaps; folder management does not complete them.

An optional MCP project-context surface could expose bounded metadata for an explicitly supplied root. It would require a deliberate new contract and validation; current MCP tools have not gained filesystem access or write scope. Broader AI UI integration should preserve visible context selection and execution targets. Vendor AI provider/pricing claims are excluded because those details do not determine this implementation.

Validation status and native observations are tracked in [CURRENT-STATUS.md](CURRENT-STATUS.md). Documentation comparison cannot establish latency, memory use, cancellation correctness or live database compatibility.
