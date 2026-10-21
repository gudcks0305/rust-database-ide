//! Bounded metadata snapshots. No fetching, database scanning, filesystem or network I/O.
//! The cache has one mutable owner; immutable leases may be cloned and, when V is Send+Sync,
//! sent across threads.
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheConfig {
    pub max_accounted_bytes: usize,
    /// Counts resident AND evicted/invalidated entries whose snapshots remain leased.
    pub max_live_entries: usize,
}
impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_accounted_bytes: 16 * 1024 * 1024,
            max_live_entries: 1024,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigError;
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cache byte and live-entry limits must be positive")
    }
}
impl std::error::Error for ConfigError {}

/// Monotonic elapsed time in one clock epoch. Custom clocks must never move backward.
pub trait Clock {
    fn now(&self) -> Duration;
}
#[derive(Debug)]
pub struct MonotonicClock {
    start: Instant,
}
impl Default for MonotonicClock {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}
impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Charged payload bytes, including retired snapshots until their final lease drops.
    pub accounted_bytes: usize,
    pub resident_bytes: usize,
    /// All leased resident payloads plus all retired payloads. Count each payload once.
    pub pinned_bytes: usize,
    pub live_entries: usize,
    pub resident_entries: usize,
    pub pinned_entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub expirations: u64,
    pub invalidations: u64,
    pub admissions_rejected: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    ZeroTtl,
    TtlOverflow,
    WeightOverflow,
    Oversized {
        requested_bytes: usize,
        max_bytes: usize,
    },
    BudgetExceeded {
        requested_bytes: usize,
        accounted_bytes: usize,
        pinned_bytes: usize,
        live_entries: usize,
        max_bytes: usize,
        max_live_entries: usize,
    },
}
impl fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroTtl => f.write_str("metadata TTL must be positive"),
            Self::TtlOverflow => f.write_str("metadata TTL deadline overflow"),
            Self::WeightOverflow => f.write_str("metadata accounted size overflow"),
            Self::Oversized {
                requested_bytes,
                max_bytes,
            } => write!(
                f,
                "metadata requires {requested_bytes} accounted bytes; budget is {max_bytes}"
            ),
            Self::BudgetExceeded {
                pinned_bytes,
                max_bytes,
                max_live_entries,
                ..
            } => write!(
                f,
                "metadata admission blocked by live snapshots: pinned {pinned_bytes} bytes; limits {max_bytes} bytes/{max_live_entries} entries"
            ),
        }
    }
}
impl std::error::Error for AdmissionError {}

#[derive(Default)]
struct LiveCounts {
    bytes: usize,
    entries: usize,
}
#[derive(Default)]
struct Accounting {
    live: Mutex<LiveCounts>,
}
impl Accounting {
    fn counts(&self) -> std::sync::MutexGuard<'_, LiveCounts> {
        self.live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
struct Payload<V> {
    value: Option<V>,
    weight: usize,
    expires_at: Duration,
    accounting: Arc<Accounting>,
}
impl<V> Drop for Payload<V> {
    fn drop(&mut self) {
        // Free nested owned value allocations before releasing their budget charge.
        drop(self.value.take());
        let mut live = self.accounting.counts();
        live.bytes -= self.weight;
        live.entries -= 1;
    }
}
/// Immutable point-in-time snapshot. It may outlive TTL/invalidation or the entire cache.
/// Holding/cloning this handle keeps the original entry's full budget charge alive.
pub struct MetadataLease<V> {
    payload: Arc<Payload<V>>,
}
impl<V> Clone for MetadataLease<V> {
    fn clone(&self) -> Self {
        Self {
            payload: self.payload.clone(),
        }
    }
}
impl<V> Deref for MetadataLease<V> {
    type Target = V;
    fn deref(&self) -> &V {
        self.payload
            .value
            .as_ref()
            .expect("a live lease always owns its value")
    }
}
impl<V> MetadataLease<V> {
    pub fn accounted_bytes(&self) -> usize {
        self.payload.weight
    }
    /// Deadline in the cache clock's epoch. Existing leases are snapshots, not fresh lookups.
    pub fn expires_at(&self) -> Duration {
        self.payload.expires_at
    }
}
impl<V: fmt::Debug> fmt::Debug for MetadataLease<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetadataLease")
            .field("value", &**self)
            .field("accounted_bytes", &self.accounted_bytes())
            .field("expires_at", &self.expires_at())
            .finish()
    }
}
struct Record<V> {
    payload: Arc<Payload<V>>,
    recency: u64,
}

/// Caller keys MUST identify the full endpoint fingerprint, database, scope and relevant
/// metadata generation/permissions. This generic cache cannot invent connection identity.
/// Fetch/generation validation and SQL work belong to the caller; get() never calls a loader.
pub struct MetadataCache<K, V, C = MonotonicClock> {
    config: CacheConfig,
    clock: C,
    entries: HashMap<K, Record<V>>,
    accounting: Arc<Accounting>,
    resident_bytes: usize,
    sequence: u64,
    counters: CacheStats,
}
impl<K: Eq + Hash, V> MetadataCache<K, V, MonotonicClock> {
    pub fn new(config: CacheConfig) -> Result<Self, ConfigError> {
        Self::with_clock(config, MonotonicClock::default())
    }
}
impl<K: Eq + Hash, V, C: Clock> MetadataCache<K, V, C> {
    pub fn with_clock(config: CacheConfig, clock: C) -> Result<Self, ConfigError> {
        if config.max_accounted_bytes == 0 || config.max_live_entries == 0 {
            return Err(ConfigError);
        }
        Ok(Self {
            config,
            clock,
            entries: HashMap::new(),
            accounting: Arc::new(Accounting::default()),
            resident_bytes: 0,
            sequence: 0,
            counters: CacheStats::default(),
        })
    }
    pub fn config(&self) -> CacheConfig {
        self.config
    }
    /// Weight includes fixed key/value/entry bookkeeping plus caller-declared retained heap
    /// capacities for BOTH key and value, recursively. Count spare String/Vec capacity and
    /// retained shared allocations conservatively. This is accounted payload, never RSS.
    /// Allocation overhead, HashMap spare buckets, lease handle allocations and caller-held
    /// copies are not an allocator/RSS cap. Unknown weights must not be silently passed as 0.
    pub fn accounted_size(owned_heap_bytes: usize) -> Result<usize, AdmissionError> {
        owned_heap_bytes
            .checked_add(std::mem::size_of::<K>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Payload<V>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Record<V>>()))
            .and_then(|bytes| bytes.checked_add(4 * std::mem::size_of::<usize>()))
            .ok_or(AdmissionError::WeightOverflow)
    }
    /// Returns only an unexpired snapshot. Expired entries become misses and lose residency;
    /// any existing leases retain their budget charge until the last lease is dropped.
    pub fn get(&mut self, key: &K) -> Option<MetadataLease<V>> {
        let now = self.clock.now();
        let expired = self
            .entries
            .get(key)
            .is_some_and(|record| record.payload.expires_at <= now);
        if expired {
            self.remove_resident(key);
            self.counters.expirations = self.counters.expirations.saturating_add(1);
        }
        let recency = self.next_recency();
        match self.entries.get_mut(key) {
            Some(record) => {
                record.recency = recency;
                self.counters.hits = self.counters.hits.saturating_add(1);
                Some(MetadataLease {
                    payload: record.payload.clone(),
                })
            }
            None => {
                self.counters.misses = self.counters.misses.saturating_add(1);
                None
            }
        }
    }
    /// Admission is atomic from the owner's perspective: failure preserves all existing
    /// resident entries, including a replaced key. Evict only reclaimable (unleased) entries,
    /// expired ones first, then least recently used. Pinned leases never fake free capacity.
    pub fn insert(
        &mut self,
        key: K,
        value: V,
        owned_heap_bytes: usize,
        ttl: Duration,
    ) -> Result<MetadataLease<V>, AdmissionError> {
        let result = self.prepare_admission(&key, owned_heap_bytes, ttl);
        let (weight, expires_at, evict) = match result {
            Ok(plan) => plan,
            Err(error) => {
                self.counters.admissions_rejected =
                    self.counters.admissions_rejected.saturating_add(1);
                return Err(error);
            }
        };
        let now = self.clock.now();
        let mut expired = 0_u64;
        let mut evicted = 0_u64;
        self.entries.retain(|_, record| {
            if evict.contains(&record.recency) {
                self.resident_bytes -= record.payload.weight;
                if record.payload.expires_at <= now {
                    expired += 1;
                } else {
                    evicted += 1;
                }
                false
            } else {
                true
            }
        });
        self.counters.evictions = self.counters.evictions.saturating_add(evicted);
        self.counters.expirations = self.counters.expirations.saturating_add(expired);
        self.remove_resident(&key);
        {
            let mut live = self.accounting.counts();
            live.bytes += weight;
            live.entries += 1;
        }
        let payload = Arc::new(Payload {
            value: Some(value),
            weight,
            expires_at,
            accounting: self.accounting.clone(),
        });
        let recency = self.next_recency();
        self.resident_bytes += weight;
        self.entries.insert(
            key,
            Record {
                payload: payload.clone(),
                recency,
            },
        );
        Ok(MetadataLease { payload })
    }
    /// Invalidation prevents fresh lookups immediately. It cannot revoke existing snapshots.
    pub fn invalidate(&mut self, key: &K) -> bool {
        let removed = self.remove_resident(key);
        if removed {
            self.counters.invalidations = self.counters.invalidations.saturating_add(1);
        }
        removed
    }
    /// Remove matching resident keys without touching unrelated recency or TTL. The scan
    /// is bounded by resident entry count; retired leased payloads remain budget-charged.
    pub fn invalidate_where(&mut self, mut predicate: impl FnMut(&K) -> bool) -> usize {
        let before = self.entries.len();
        self.entries.retain(|key, record| {
            if predicate(key) {
                self.resident_bytes -= record.payload.weight;
                false
            } else {
                true
            }
        });
        let removed = before - self.entries.len();
        self.counters.invalidations = self.counters.invalidations.saturating_add(removed as u64);
        removed
    }
    /// Same local invalidation contract; the caller schedules/validates any subsequent fetch.
    pub fn force_refresh(&mut self, key: &K) -> bool {
        self.invalidate(key)
    }
    pub fn clear(&mut self) {
        self.counters.invalidations = self
            .counters
            .invalidations
            .saturating_add(self.entries.len() as u64);
        self.entries = HashMap::new();
        self.resident_bytes = 0;
    }
    pub fn prune_expired(&mut self) -> usize {
        let now = self.clock.now();
        let before = self.entries.len();
        self.entries.retain(|_, record| {
            if record.payload.expires_at <= now {
                self.resident_bytes -= record.payload.weight;
                false
            } else {
                true
            }
        });
        let removed = before - self.entries.len();
        self.counters.expirations = self.counters.expirations.saturating_add(removed as u64);
        removed
    }
    pub fn stats(&self) -> CacheStats {
        let live = self.accounting.counts();
        let mut stats = self.counters;
        stats.accounted_bytes = live.bytes;
        stats.live_entries = live.entries;
        stats.resident_bytes = self.resident_bytes;
        stats.resident_entries = self.entries.len();
        stats.pinned_bytes = live.bytes - self.resident_bytes;
        stats.pinned_entries = live.entries - self.entries.len();
        for record in self.entries.values() {
            if Arc::strong_count(&record.payload) > 1 {
                stats.pinned_bytes += record.payload.weight;
                stats.pinned_entries += 1;
            }
        }
        stats
    }
    fn remove_resident(&mut self, key: &K) -> bool {
        if let Some(record) = self.entries.remove(key) {
            self.resident_bytes -= record.payload.weight;
            true
        } else {
            false
        }
    }
    fn next_recency(&mut self) -> u64 {
        if self.sequence == u64::MAX {
            let mut order: Vec<u64> = self.entries.values().map(|record| record.recency).collect();
            order.sort_unstable();
            order.dedup();
            for record in self.entries.values_mut() {
                record.recency = order
                    .binary_search(&record.recency)
                    .expect("existing recency") as u64;
            }
            self.sequence = order.len() as u64;
        }
        self.sequence += 1;
        self.sequence
    }
    fn prepare_admission(
        &self,
        key: &K,
        owned_heap_bytes: usize,
        ttl: Duration,
    ) -> Result<(usize, Duration, Vec<u64>), AdmissionError> {
        if ttl.is_zero() {
            return Err(AdmissionError::ZeroTtl);
        }
        let weight = Self::accounted_size(owned_heap_bytes)?;
        if weight > self.config.max_accounted_bytes {
            return Err(AdmissionError::Oversized {
                requested_bytes: weight,
                max_bytes: self.config.max_accounted_bytes,
            });
        }
        let now = self.clock.now();
        let expires_at = now.checked_add(ttl).ok_or(AdmissionError::TtlOverflow)?;
        let (mut bytes, mut count) = {
            let live = self.accounting.counts();
            (live.bytes, live.entries)
        };
        if let Some(record) = self.entries.get(key)
            && Arc::strong_count(&record.payload) == 1
        {
            bytes -= record.payload.weight;
            count -= 1;
        }
        let fits = |bytes: usize, count: usize| {
            count < self.config.max_live_entries
                && bytes <= self.config.max_accounted_bytes - weight
        };
        let mut evict = Vec::new();
        let mut candidates: Vec<_> = self
            .entries
            .iter()
            .filter(|(candidate, record)| {
                *candidate != key && Arc::strong_count(&record.payload) == 1
            })
            .map(|(_, record)| {
                (
                    record.payload.expires_at > now,
                    record.recency,
                    record.payload.weight,
                )
            })
            .collect();
        candidates.sort_unstable();
        for (_, recency, candidate_weight) in candidates {
            if fits(bytes, count) {
                break;
            }
            bytes -= candidate_weight;
            count -= 1;
            evict.push(recency);
        }
        if !fits(bytes, count) {
            let stats = self.stats();
            return Err(AdmissionError::BudgetExceeded {
                requested_bytes: weight,
                accounted_bytes: stats.accounted_bytes,
                pinned_bytes: stats.pinned_bytes,
                live_entries: stats.live_entries,
                max_bytes: self.config.max_accounted_bytes,
                max_live_entries: self.config.max_live_entries,
            });
        }
        Ok((weight, expires_at, evict))
    }
}
