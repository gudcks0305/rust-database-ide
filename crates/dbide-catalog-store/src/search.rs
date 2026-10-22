//! Read-only, bounded historical object discovery. No directory walk or database I/O.
use super::*;
use std::collections::HashSet;

/// Captured current workspace identity. `None` permits every saved database on this endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchProfile {
    pub profile_id: String,
    pub fingerprint: String,
    pub profile_name: String,
    pub engine: String,
    pub database: Option<String>,
}
impl SearchProfile {
    fn contains(&self, key: &Key) -> bool {
        self.profile_id == key.profile_id
            && self.fingerprint == key.fingerprint
            && self.database.as_ref().is_none_or(|db| db == &key.database)
    }
}
#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
    pub text: String,
    pub kind: Option<String>,
}
#[derive(Clone, Copy, Debug)]
pub struct SearchLimits {
    pub max_results: usize,
    pub max_shards: usize,
    pub max_bytes: usize,
}
impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_results: 100,
            max_shards: 64,
            max_bytes: 4 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub profile: SearchProfile,
    pub database: String,
    pub relation: Relation,
    /// Historical capture time, never evidence that the object currently exists.
    pub captured_unix_ms: u64,
}
#[derive(Clone, Debug, Default)]
pub struct SearchPage {
    pub matches: Vec<SearchMatch>,
    pub scanned_shards: usize,
    pub bytes_read: usize,
    /// Budget reached, a source snapshot was partial, or an unreadable shard was skipped.
    pub partial: bool,
    pub skipped_shards: usize,
    pub cache_present: bool,
}
impl Store {
    /// Reads the bounded manifest, then only Objects payloads within supplied workspace identities.
    /// Columns/Details entries already identify their objects; their payloads are never loaded.
    /// Limits are per search and hard-capped. Incomplete searches must be labeled as a subset.
    pub fn search_objects(
        &self,
        profiles: &[SearchProfile],
        query: &SearchQuery,
        limits: SearchLimits,
    ) -> Result<SearchPage, String> {
        let mut page = SearchPage::default();
        if !self.ensure_root(false)? {
            return Ok(page);
        }
        let mut entries = self.read_index()?.entries;
        entries.retain(|entry| profiles.iter().any(|p| p.contains(&entry.key)));
        page.cache_present = !entries.is_empty();
        // Search all manifest-only identities before spending the payload budget.
        // Prefer fresh entries within each group, independent of insertion order.
        entries.sort_by(|a, b| {
            matches!(a.key.scope, Scope::Objects(_))
                .cmp(&matches!(b.key.scope, Scope::Objects(_)))
                .then_with(|| b.captured_unix_ms.cmp(&a.captured_unix_ms))
                .then_with(|| a.key.filename().cmp(&b.key.filename()))
        });
        let limits = SearchLimits {
            max_results: limits.max_results.min(100),
            max_shards: limits.max_shards.min(64),
            max_bytes: limits.max_bytes.min(4 * 1024 * 1024),
        };
        let needle = query.text.trim().to_lowercase();
        let mut seen = HashSet::new();
        for entry in entries {
            let key = &entry.key;
            let Some(profile) = profiles.iter().find(|p| p.contains(key)) else {
                continue;
            };
            match &key.scope {
                Scope::Columns(relation) | Scope::Details(relation) => {
                    if matches_query(relation, &needle, query.kind.as_deref())
                        && !insert_match(
                            &mut page,
                            &mut seen,
                            profile,
                            key,
                            relation.clone(),
                            entry.captured_unix_ms,
                            limits.max_results,
                        )
                    {
                        break;
                    }
                }
                Scope::Objects(_) => {
                    if page.scanned_shards >= limits.max_shards {
                        page.partial = true;
                        break;
                    }
                    // Reserved manifest bytes may exceed actual size; use this conservative budget
                    // before opening a payload. A concurrent writer cannot bypass the read bound.
                    if entry.bytes as usize > limits.max_bytes.saturating_sub(page.bytes_read) {
                        page.partial = true;
                        continue;
                    }
                    page.scanned_shards += 1;
                    let remaining = limits
                        .max_bytes
                        .saturating_sub(page.bytes_read)
                        .min(MAX_SHARD_BYTES);
                    let bytes = match read_optional(&self.root.join(key.filename()), remaining) {
                        Ok(Some(bytes)) => bytes,
                        Ok(None) | Err(_) => {
                            page.skipped_shards += 1;
                            page.partial = true;
                            continue;
                        }
                    };
                    page.bytes_read += bytes.len();
                    let envelope: Envelope = match serde_json::from_slice(&bytes) {
                        Ok(envelope) => envelope,
                        Err(_) => {
                            page.skipped_shards += 1;
                            page.partial = true;
                            continue;
                        }
                    };
                    if envelope.version != VERSION
                        || envelope.key != *key
                        || !envelope.snapshot.matches(key)
                    {
                        page.skipped_shards += 1;
                        page.partial = true;
                        continue;
                    }
                    page.partial |= envelope.snapshot.truncated || envelope.snapshot.fields_partial;
                    if let Data::Objects(objects) = envelope.snapshot.data {
                        for object in objects {
                            let relation = Relation {
                                schema: object.schema,
                                name: object.name,
                                kind: object.kind,
                            };
                            if matches_query(&relation, &needle, query.kind.as_deref())
                                && !insert_match(
                                    &mut page,
                                    &mut seen,
                                    profile,
                                    key,
                                    relation,
                                    envelope.snapshot.captured_unix_ms,
                                    limits.max_results,
                                )
                            {
                                return Ok(page);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(page)
    }
}
fn matches_query(relation: &Relation, needle: &str, kind: Option<&str>) -> bool {
    kind.is_none_or(|kind| relation.kind == kind)
        && (needle.is_empty()
            || relation.name.to_lowercase().contains(needle)
            || format!("{}.{}", relation.schema, relation.name)
                .to_lowercase()
                .contains(needle))
}
fn insert_match(
    page: &mut SearchPage,
    seen: &mut HashSet<(String, String, String, Relation)>,
    profile: &SearchProfile,
    key: &Key,
    relation: Relation,
    captured_unix_ms: u64,
    max_results: usize,
) -> bool {
    let identity = (
        key.profile_id.clone(),
        key.fingerprint.clone(),
        key.database.clone(),
        relation.clone(),
    );
    if !seen.insert(identity) {
        return true;
    }
    if page.matches.len() >= max_results {
        page.partial = true;
        return false;
    }
    page.matches.push(SearchMatch {
        profile: profile.clone(),
        database: key.database.clone(),
        relation,
        captured_unix_ms,
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> SearchProfile {
        SearchProfile {
            profile_id: "test".into(),
            fingerprint: "endpoint".into(),
            profile_name: "Fixture".into(),
            engine: "PostgreSQL".into(),
            database: None,
        }
    }
    fn key(scope: Scope) -> Key {
        Key {
            profile_id: "test".into(),
            fingerprint: "endpoint".into(),
            database: "fixture".into(),
            scope,
        }
    }
    fn objects(store: &mut Store, schema: &str, names: &[(&str, &str)]) -> Key {
        let key = key(Scope::Objects(schema.into()));
        store
            .save(
                key.clone(),
                Snapshot {
                    captured_unix_ms: 3,
                    truncated: false,
                    fields_partial: false,
                    data: Data::Objects(
                        names
                            .iter()
                            .map(|(name, kind)| Object {
                                schema: schema.into(),
                                name: (*name).into(),
                                kind: (*kind).into(),
                            })
                            .collect(),
                    ),
                },
            )
            .unwrap();
        key
    }
    fn search(store: &Store, text: &str) -> SearchPage {
        store
            .search_objects(
                &[profile()],
                &SearchQuery {
                    text: text.into(),
                    kind: None,
                },
                SearchLimits::default(),
            )
            .unwrap()
    }
    #[test]
    fn unexpanded_objects_unicode_and_all_saved_identity_scopes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::new(tmp.path().canonicalize().unwrap().join("private"));
        objects(
            &mut store,
            "공개",
            &[
                ("고객😊", "table"),
                ("CustomerView", "view"),
                ("Totals", "materialized view"),
            ],
        );
        let relation = Relation {
            schema: "other".into(),
            name: "OnlyColumns".into(),
            kind: "table".into(),
        };
        store
            .save(
                key(Scope::Columns(relation.clone())),
                Snapshot {
                    captured_unix_ms: 4,
                    truncated: false,
                    fields_partial: false,
                    data: Data::Columns(vec![]),
                },
            )
            .unwrap();
        let detail_relation = Relation {
            schema: "other".into(),
            name: "OnlyDetails".into(),
            kind: "view".into(),
        };
        let view = dbide_schema::View {
            name: detail_relation.name.clone(),
            definition: "SELECT 1".into(),
            materialized: false,
        };
        store
            .save(
                key(Scope::Details(detail_relation)),
                Snapshot {
                    captured_unix_ms: 4,
                    truncated: false,
                    fields_partial: false,
                    data: Data::Details {
                        table: None,
                        view: Some(Box::new(view)),
                    },
                },
            )
            .unwrap();
        assert_eq!(search(&store, "").matches.len(), 5);
        assert_eq!(search(&store, "공개.고객😊").matches.len(), 1);
        assert_eq!(search(&store, "customerview").matches.len(), 1);
        let filtered = store
            .search_objects(
                &[profile()],
                &SearchQuery {
                    text: String::new(),
                    kind: Some("materialized view".into()),
                },
                SearchLimits::default(),
            )
            .unwrap();
        assert_eq!(filtered.matches.len(), 1);
    }
    #[test]
    fn exact_workspace_endpoint_database_and_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::new(tmp.path().canonicalize().unwrap().join("private"));
        objects(&mut store, "public", &[("one", "table")]);
        store
            .save(
                key(Scope::Columns(Relation {
                    schema: "public".into(),
                    name: "one".into(),
                    kind: "table".into(),
                })),
                Snapshot {
                    captured_unix_ms: 9,
                    truncated: false,
                    fields_partial: false,
                    data: Data::Columns(vec![]),
                },
            )
            .unwrap();
        assert_eq!(search(&store, "one").matches.len(), 1);
        for p in [
            SearchProfile {
                fingerprint: "changed".into(),
                ..profile()
            },
            SearchProfile {
                profile_id: "deleted".into(),
                ..profile()
            },
            SearchProfile {
                database: Some("different".into()),
                ..profile()
            },
        ] {
            assert!(
                store
                    .search_objects(&[p], &SearchQuery::default(), SearchLimits::default())
                    .unwrap()
                    .matches
                    .is_empty()
            );
        }
    }
    #[test]
    fn bounded_results_shards_bytes_and_read_only_absence() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().canonicalize().unwrap().join("absent");
        let store = Store::new(missing.clone());
        assert!(!search(&store, "").cache_present);
        assert!(!missing.exists());
        let mut store = Store::new(tmp.path().canonicalize().unwrap().join("private"));
        objects(&mut store, "a", &[("one", "table"), ("two", "table")]);
        objects(&mut store, "b", &[("three", "view")]);
        for limits in [
            SearchLimits {
                max_results: 1,
                ..SearchLimits::default()
            },
            SearchLimits {
                max_shards: 1,
                ..SearchLimits::default()
            },
            SearchLimits {
                max_bytes: 1,
                ..SearchLimits::default()
            },
        ] {
            let page = store
                .search_objects(&[profile()], &SearchQuery::default(), limits)
                .unwrap();
            assert!(page.partial);
            assert!(page.matches.len() <= limits.max_results);
            assert!(page.scanned_shards <= limits.max_shards);
            assert!(page.bytes_read <= limits.max_bytes);
        }
        fs::remove_file(store.root.join(WRITER_LOCK)).unwrap();
        search(&store, "one");
        assert!(!store.root.join(WRITER_LOCK).exists());
    }
    #[test]
    fn missing_corrupt_future_and_mismatched_objects_are_honestly_partial() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::new(tmp.path().canonicalize().unwrap().join("private"));
        let key = objects(&mut store, "a", &[("one", "table")]);
        let path = store.root.join(key.filename());
        let original = fs::read(&path).unwrap();
        for mutation in ["corrupt", "future", "identity", "missing"] {
            let bytes = match mutation {
                "future" | "identity" => {
                    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
                    if mutation == "future" {
                        value["version"] = 999.into();
                    } else {
                        value["key"]["fingerprint"] = "other".into();
                    }
                    serde_json::to_vec(&value).unwrap()
                }
                _ => b"invalid".to_vec(),
            };
            if mutation == "missing" {
                fs::remove_file(&path).unwrap();
            } else {
                fs::write(&path, bytes).unwrap();
            }
            let page = search(&store, "one");
            assert!(page.matches.is_empty());
            assert!(page.partial);
            assert_eq!(page.skipped_shards, 1);
        }
    }
    #[test]
    fn result_ceiling_is_hard_and_manifest_identity_search_never_loads_columns() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::new(tmp.path().canonicalize().unwrap().join("private"));
        let relation = Relation {
            schema: "main".into(),
            name: "ManifestOnly".into(),
            kind: "table".into(),
        };
        let column_key = key(Scope::Columns(relation));
        store
            .save(
                column_key.clone(),
                Snapshot {
                    captured_unix_ms: 1,
                    truncated: false,
                    fields_partial: false,
                    data: Data::Columns(vec![]),
                },
            )
            .unwrap();
        // A manifest identity is historical evidence even if its detail payload is absent.
        // No Columns payload is consulted by object-name discovery.
        fs::remove_file(store.root.join(column_key.filename())).unwrap();
        let page = store
            .search_objects(
                &[profile()],
                &SearchQuery::default(),
                SearchLimits {
                    max_results: 100,
                    max_shards: 0,
                    max_bytes: 0,
                },
            )
            .unwrap();
        assert_eq!(page.matches.len(), 1);
        assert_eq!(page.bytes_read, 0);
        assert_eq!(page.scanned_shards, 0);

        let names = (0..110)
            .map(|i| format!("object{i:03}"))
            .collect::<Vec<_>>();
        let refs = names
            .iter()
            .map(|name| (name.as_str(), "view"))
            .collect::<Vec<_>>();
        objects(&mut store, "main", &refs);
        let page = store
            .search_objects(
                &[profile()],
                &SearchQuery::default(),
                SearchLimits {
                    max_results: usize::MAX,
                    max_shards: usize::MAX,
                    max_bytes: usize::MAX,
                },
            )
            .unwrap();
        assert_eq!(page.matches.len(), 100);
        assert!(page.partial);
        assert!(page.bytes_read <= 4 * 1024 * 1024);
        assert!(page.scanned_shards <= 64);
    }
}
