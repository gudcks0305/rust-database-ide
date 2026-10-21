# Third-party code and license status

The project does not yet declare one repository-wide license. Individual package
manifests retain their existing declarations; this document does not grant a new
license to otherwise unlicensed project files.

## Vendored tokio-postgres

`vendor/tokio-postgres` is based on tokio-postgres 0.7.18 from
[rust-postgres](https://github.com/sfackler/rust-postgres). Its original
[MIT license](vendor/tokio-postgres/LICENSE-MIT) and
[Apache-2.0 license](vendor/tokio-postgres/LICENSE-APACHE) are retained.
The original crate SHA-256 and the small protocol-status patch are documented in
[PG-STATUS-PATCH.md](docs/PG-STATUS-PATCH.md). Original upstream source and license
notices must remain with this directory.

## Cargo dependencies

`Cargo.lock` pins dependency versions. Dependencies obtained through Cargo keep
their own licenses; they are not relicensed by this repository. Current UI package
manifests (`dbide-ui-spike`, `dbide-tools-ui`) declare Apache-2.0. Other project
packages currently have no package-level license declaration.

The local publication audit found no GPL-only dependency in the resolved metadata;
some dependencies offer GPL as one alternative in an OR expression. Three resolved
packages declare MPL-2.0 (`cbindgen`, `dwrote`, `option-ext`). This is a dependency
inventory observation, not a completed binary-distribution license audit. Before
publishing application binaries, collect notices for the actual target-specific
dependency set and satisfy its applicable license conditions.

## Reference product investigation

DataGrip is a JetBrains product. This repository contains independent Rust code,
research notes and artifact metadata. Reference application binaries, bytecode,
assets, credentials and raw local investigation outputs are excluded from Git.
Public metadata uses project-relative paths; retained local evidence hashes are
not a redistribution of the underlying reference artifacts.
