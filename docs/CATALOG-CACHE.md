# Bounded catalog snapshot cache

`dbide-catalog-cache` provides an in-memory cache foundation for **metadata only**
(table/schema/object/column lists). It does not implement a database index, scan a
database, fetch rows, watch DDL, read files, open connections or schedule work.
There is no eager metadata load and no implicit loader on cache misses. Integration
with the bridge/explorer is separate work; this crate alone does not make app
metadata indexing complete.

## Limits and ownership

Default `CacheConfig`: **16 MiB accounted payload**, **1,024 live entries**. Both
are explicit, positive limits. A live entry includes a resident cache value and
an evicted, expired, replaced or invalidated value retained by any lease. An entry
is counted once regardless of how many lease handles share it.

`MetadataCache<K,V,C = MonotonicClock>` has one mutable owner; put it behind an
external mutex/actor if more than one caller needs to mutate it. The cache has no
background thread. `MetadataLease<V>` is an immutable `Arc` snapshot, implements
`Deref<Target=V>` and Clone, and is Send/Sync when V is Send/Sync. It can outlive its
TTL, invalidation or the entire cache. Existing leases are point-in-time snapshots;
only a fresh `get()` establishes that the entry has not expired at lookup time.

Weights include key/value inline storage and fixed per-entry bookkeeping. The
caller supplies **retained heap bytes for both key and value**, recursively,
including spare String/Vec capacities and retained shared allocations. Prefer
conservative overestimates when allocations are shared; do not silently use zero
for unknown heap ownership. `MetadataCache::accounted_size(owned_heap_bytes)`
returns the complete charge and detects arithmetic overflow. Zero retained heap
is valid for inline-only values, which still incur nonzero bookkeeping charge.

This is **accounted payload, not RSS or an allocator cap**. HashMap spare buckets,
allocator overhead, lease handles, external copies, caller fetch buffers and
unreported allocations are not accurately bounded by this number. `clear()` also
drops the cache's map allocation; individual eviction need not shrink buckets.
The entry-count bound independently limits retained entries/key/value objects.
Callers must still bound source query pagination, result size, in-flight requests
and UI model copies. Constructing an oversized result before insert is already
outside the cache's retention budget.

A payload's full byte and entry charge remains alive until the **last cache or
lease owner drops it**. Removing a pinned entry does not claim to release that
payload. The value's owned allocations are dropped before its charge is released.
Replacements keep old leased snapshots charged independently from the new value.

## API

```rust,ignore
MetadataCache::<Key, Value>::new(CacheConfig::default()) -> Result<_, ConfigError>
MetadataCache::with_clock(config, custom_clock) -> Result<_, ConfigError>
cache.get(&key) -> Option<MetadataLease<Value>>
cache.insert(key, value, owned_heap_bytes, ttl: Duration)
    -> Result<MetadataLease<Value>, AdmissionError>
cache.invalidate(&key) -> bool
cache.invalidate_where(|key: &Key| key.inside(&scope)) -> usize
cache.force_refresh(&key) -> bool
cache.prune_expired() -> usize
cache.clear()
cache.stats() -> CacheStats
```

`insert` validates TTL/weight/deadline and plans admission before changing existing
entries. An oversized item or pinned-budget rejection preserves every current
resident entry, including the old value/TTL for a failed replacement. On success,
expired reclaimable entries are preferred, then the least recently used
**unleased** entries. Cache hits update recency. A pinned payload cannot supply
free capacity, so such entries are skipped when choosing reclaimable victims.
If enough bytes or live-entry slots cannot be reclaimed, admission returns a
visible `AdmissionError::BudgetExceeded` with requested/current/pinned byte counts
and configured limits. No silent fallback allocation or unbounded secondary cache.

`get` treats `now >= expires_at` as expired, removes residency and returns a miss.
It never returns expired data as a fresh hit and never extends TTL on access.
`prune_expired`, `invalidate`, `force_refresh` and `clear` remove lookup visibility;
held snapshots retain their charges. `force_refresh` is only an invalidation
alias: the caller explicitly starts and validates the next metadata request.
`invalidate_where` scans only bounded resident keys and removes matching entries
for a schema/database subtree refresh. It increments invalidations by the number
removed, preserves unrelated values/TTL/LRU order and keeps held matching leases
charged until their final Drop. The predicate owns scope semantics; this method
does not load children or expose payload ownership. A default cache scans at most
1,024 resident keys (or its explicit configured live-entry bound).

`Clock::now()` uses elapsed Duration in one monotonically increasing epoch. The
default clock wraps Instant. Manual clocks allow exact-deadline tests without
sleeping. Zero TTL, deadline overflow, weight overflow and zero config limits are
errors. Expired entries can remain bounded residents until get/prune/admission;
applications may prune on their existing idle/timer tick without a new worker.

`CacheStats` includes accounted/resident/pinned bytes; live/resident/pinned entry
counts; hits, misses, LRU evictions, expirations, invalidations and rejected
admissions. Leased resident bytes and retired leased bytes count once in pinned
bytes. Concurrent lease drops can make a stats observation immediately outdated,
but a final lease still holding payload is never reported as freed capacity.
Stats counters saturate rather than overflow.

## Caller identity and refresh contract

The generic key must contain the complete **password-free endpoint fingerprint**,
engine/database identity, requested scope, authenticated role/permissions identity
where relevant and the caller's metadata generation/revision. A profile display
name or database name alone is insufficient. Keep credentials out of keys/logs.
The cache cannot infer whether arbitrary caller keys are safe or equivalent.

Example scopes: database list, one database's schema list, one schema's object
page, one table's columns. Fetch on expansion/selection with source bounds; do
not walk every database/schema/table to populate this cache. A cache miss is an
instruction for the caller to decide whether to request metadata, not an automatic
network operation. Incoming results must still match the initiating endpoint,
request ID and generation before admission. Request cancellation, stale-response
rejection and atomic UI refresh remain caller-owned. DDL/profile/role changes
require appropriate invalidation or a new generation. TTL is bounded reuse, not
proof that server metadata has not changed.

## Verification

`cargo test -p dbide-catalog-cache`: **14 offline tests**, no sleeps or DB access:
weighted LRU touch/order, TTL exact deadline, overflow/oversized rejection, pinned
lease/clone retention, replacement admission/failure preservation, no partial
candidate eviction on failure, clear/refresh/prune, live-entry budget independent
of bytes, cross-thread lease drop/cache destruction, endpoint/database/scope key
separation, no miss population, invalid config/TTL and schema-scoped invalidation
with held/unheld entries, blocked admission until lease release, unrelated TTL
and LRU preservation.

`cargo clippy -p dbide-catalog-cache --all-targets -- -D warnings`,
`cargo fmt -p dbide-catalog-cache -- --check` and `git diff --check` pass. No
throughput or RSS measurement was performed. End-to-end explorer reuse, refresh
routing and memory behavior must be verified after integration before claiming
app-level metadata indexing or a measured memory reduction.

## Local catalog shards (2026-10-22)

`dbide-catalog-store` adds an optional **historical metadata** disk layer to the
Navigator. The app passes the selected profiles file's parent directory plus
`catalog-cache/` to `Navigator::with_store`; `Navigator::new` remains memory-only.
Constructing either `Navigator` or `Store` performs no filesystem reads or writes.
Expanding a missing memory branch queues a bounded manifest lookup and exactly
that branch's payload file lookup on one
dedicated disk worker. A miss or rejected shard falls back to the existing bounded
live metadata queue; a disk hit remains visibly historical until explicit refresh.
There is no startup directory scan, full catalog preload, or database connection
from a cache lookup. GPUI rendering/polling only enqueues work and consumes replies.

Layout is deliberately inspectable:

```text
<selected-profiles-parent>/catalog-cache/
  .dbide-catalog-v1           # ownership marker
  .writer.lock               # private advisory lock shared by app instances
  index-v1.json              # bounded branch identities, sizes and capture times
  <sha256-stable-key>.json   # version + exact stable key + one metadata snapshot
```

The stable key includes profile ID, endpoint fingerprint, database and typed scope
(database/schema/object/columns/details); column/detail scopes also capture schema, object name
and kind. It excludes the process-local generation, which changes across restarts.
Every read validates the full stored key and scope, not just the hashed filename.
The manifest gates lookup, so invalidation can unlist an unsupported shard while
preserving its file; a direct key cannot revive an unlisted file. A shard records format version, UNIX capture time in milliseconds, truncated and
partial flags, and database/schema/object/column descriptors. Column defaults and
extra descriptors round-trip as metadata strings. Dedicated detail shards preserve complete typed table columns, primary/foreign keys,
indexes (expressions, predicates, includes and unsupported descriptors) and view
names, definitions and materialized status. Schema definition SQL is local metadata;
it is never executed by the cache. Credentials, SQL editor/query buffers, query
result rows and table data are not persisted.

Restored snapshots set `historical = true`; `stale()` remains true regardless of
capture age. They describe a prior observation and cannot establish current table
identity, authorize SQL execution, or satisfy current editable-grid identity checks.
Live SQL targets continue to come from the independently validated profile/session
path. Refreshing a branch invalidates its disk descendants; connection refresh or
profile replacement/removal invalidates matching endpoint branches. Clear removes
all indexed cache branches. In-flight obsolete live results fail the existing
profile-generation/ticket check before any save. The serial disk queue places
invalidation after earlier saves, so an older queued save cannot resurrect a
refreshed branch. If control work fills the queue, pending writes are dropped and
invalidation coalesces to an all-cache clear. Unrelated valid branches otherwise
remain available.

Limits: **1 MiB per JSON shard**, **128 MiB indexed shard payload**, **2,048 shards**,
**2 MiB manifest**, **16 KiB stable identity**, **32 queued disk commands** and
**64 reply slots**. A save evicts oldest captured indexed shards until it fits;
manifest listing reads identities only. Eviction/invalidation validates bounded
ownership headers rather than loading every metadata payload. A single atomic
write can temporarily add one shard's size above the steady-state payload budget;
manifest/ownership files are separately bounded. Manifest byte counts reserve the
larger of prior/current payloads across failed replacements, so they may
conservatively exceed current file sizes. JSON serialization and all file
operations execute on the disk worker. Cache budgets do not measure process RSS.

Directories are created as `0700`; owned marker, manifest, shard and temporary files
as `0600`. Reads reject non-private files, symlinks, non-regular files, oversize
payloads, corrupt/truncated JSON, future versions and mismatched identity. Writes
use exclusive private temporary files, flush and atomic rename. Save and
invalidation hold a private advisory writer lock across manifest re-read, eviction
and complete publication, serializing independent app instances sharing this
cache. Initial manifest creation uses the same lock; read-only lookups never
create or acquire it. The manifest is published before the payload and reserves
both prior/new payload sizes, leaving at worst a budgeted missing/older shard
after a failure or crash, rather than an unindexed published payload. Unknown directories/files and unsupported replacement shards are preserved;
errors become diagnostics/cache misses. No cleanup scans user directories. Tests
use newly created, canonicalized temporary roots only.

For a read-only inspector, `Store::list()` returns bounded manifest summaries
(`shard_id`, full stable key, bytes, capture time). `Store::read_by_id(id)` reads
one exact manifest-listed shard and returns `(Key, Snapshot)`; arbitrary paths and
unknown IDs return no entry. Both methods leave missing directories missing and
perform no database I/O. Human-readable shard JSON can also be inspected directly.
