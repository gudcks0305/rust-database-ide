# 출처

확인일: 2026-10-21. 공식 문서/현재 저장소 내용은 향후 바뀔 수 있다.
패키지 증거는 특정 build에 고정; 웹 문서는 runtime 관찰을 대신하지 않는다.

## DataGrip

- D01: [Data sources and drivers](https://www.jetbrains.com/help/datagrip/data-sources-and-drivers-dialog.html), [지원 DB/등급](https://www.jetbrains.com/help/datagrip/managing-data-sources.html)
- D02: [SSH and SSL](https://www.jetbrains.com/help/datagrip/configuring-ssh-and-ssl.html)
- D03: [Sessions](https://www.jetbrains.com/help/datagrip/managing-connection-sessions.html)
- D04: [Connection / transaction options](https://www.jetbrains.com/help/datagrip/configuring-database-connections.html)
- D05: [Introspection](https://www.jetbrains.com/help/datagrip/introspection.html)
- D06: [Run queries](https://www.jetbrains.com/help/datagrip/run-a-query.html)
- D07: [Data editor and viewer](https://www.jetbrains.com/help/datagrip/data-editor-and-viewer.html)
- D08: [Import](https://www.jetbrains.com/help/datagrip/import-data.html), [Export](https://www.jetbrains.com/help/datagrip/export-data.html)
- D09: [IntelliJ Platform 구조](https://plugins.jetbrains.com/docs/intellij/intellij-platform.html); 편집 기능 세부 parity는 runtime fixture로 추가 확인
- D10: [SQL refactoring](https://www.jetbrains.com/help/datagrip/refactoring-source-code.html)
- D11: [Schema comparison/migration](https://www.jetbrains.com/help/datagrip/schema-comparison-and-migration.html)
- D12: [Compare data](https://www.jetbrains.com/help/datagrip/compare-data.html)
- D13: [Database diagrams](https://www.jetbrains.com/help/datagrip/creating-diagrams.html)
- D14: [Query execution plan](https://www.jetbrains.com/help/datagrip/query-execution-plan.html)
- D15: [DDL data sources](https://www.jetbrains.com/help/datagrip/ddl-data-sources.html)
- D16: [MongoDB](https://www.jetbrains.com/help/datagrip/mongodb.html), [Redis](https://www.jetbrains.com/help/datagrip/redis.html)
- D17: [Debugger](https://www.jetbrains.com/help/datagrip/debugging-code.html)

D12–D17의 세부 기능은 구현 단계에서 해당 DB/version과 실제 동작을 재검증한다.
문서에 없는 UX 세부사항은 계획상의 제품 요구로 표시하며 공식 동작이라고 단정하지 않는다.

## Rust 후보 / 계약

- U01: [GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md), [manifest](https://github.com/zed-industries/zed/blob/main/crates/gpui/Cargo.toml), [Zed editor manifest](https://github.com/zed-industries/zed/blob/main/crates/editor/Cargo.toml)
- U02: [Iced](https://github.com/iced-rs/iced), [TextEditor API](https://docs.rs/iced/latest/iced/widget/text_editor/struct.TextEditor.html)
- U03: [egui](https://github.com/emilk/egui), [accessibility](https://github.com/emilk/egui/blob/main/docs/accessibility.md)
- R01: [SQLx](https://github.com/transact-rs/sqlx), [raw_sql](https://docs.rs/sqlx/latest/sqlx/fn.raw_sql.html)
- R02: [PostgreSQL wire protocol flow](https://www.postgresql.org/docs/current/protocol-flow.html)
- R03: [JDBC Statement](https://docs.oracle.com/en/java/javase/25/docs/api/java.sql/java/sql/Statement.html)
- R04: [Tree-sitter](https://tree-sitter.github.io/tree-sitter/), [SQL parser](https://github.com/apache/datafusion-sqlparser-rs)

## 고정된 패키지 / 도구

- P01: `evidence/package-inventory.json`, `evidence/product-summary.json`; DataGrip build `262.10968.148`
- P02: [JetBrains release API](https://data.services.jetbrains.com/products/releases?code=DG&latest=true&type=release); 응답은 `evidence/jetbrains-release.json` 보존
- P03: [공식 macOS ARM64 배포본](https://download.jetbrains.com/datagrip/datagrip-2026.2.6-aarch64.dmg), [checksum](https://download.jetbrains.com/datagrip/datagrip-2026.2.6-aarch64.dmg.sha256)
- T01: [REA](https://github.com/morluto/rea), 실제 실행 npm package `rea-agents@6.7.0`
- T02: 설치 skill `reverse-engineer-anything` v44; release CLI가 보고한 expected skill v42와 차이 있음. 실제 CLI `--help/--schema`를 우선 적용.
