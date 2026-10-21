use dbide_catalog_cache::{AdmissionError, CacheConfig, Clock, MetadataCache, MetadataLease};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
struct ManualClock(Arc<Mutex<Duration>>);
impl Clock for ManualClock {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap()
    }
}
impl ManualClock {
    fn advance(&self, duration: Duration) {
        let mut now = self.0.lock().unwrap();
        *now = now.checked_add(duration).unwrap();
    }
}
type Cache = MetadataCache<u32, Vec<u8>, ManualClock>;
fn weight(heap_bytes: usize) -> usize {
    Cache::accounted_size(heap_bytes).unwrap()
}
fn cache(bytes: usize, entries: usize) -> (Cache, ManualClock) {
    let clock = ManualClock::default();
    (
        Cache::with_clock(
            CacheConfig {
                max_accounted_bytes: bytes,
                max_live_entries: entries,
            },
            clock.clone(),
        )
        .unwrap(),
        clock,
    )
}
fn put(
    cache: &mut Cache,
    key: u32,
    byte: u8,
    length: usize,
) -> Result<MetadataLease<Vec<u8>>, AdmissionError> {
    let value = vec![byte; length];
    let heap = value.capacity();
    cache.insert(key, value, heap, Duration::from_secs(10))
}

#[test]
fn lru_touch_and_both_bounds_evict_only_oldest_reclaimable_entry() {
    let w = weight(4);
    let (mut cache, _) = cache(w * 2, 2);
    drop(put(&mut cache, 1, 1, 4).unwrap());
    drop(put(&mut cache, 2, 2, 4).unwrap());
    drop(cache.get(&1).unwrap());
    drop(put(&mut cache, 3, 3, 4).unwrap());
    assert!(cache.get(&2).is_none());
    assert_eq!(&**cache.get(&1).as_ref().unwrap(), &[1; 4]);
    assert_eq!(&**cache.get(&3).as_ref().unwrap(), &[3; 4]);
    let stats = cache.stats();
    assert_eq!(stats.evictions, 1);
    assert_eq!(stats.live_entries, 2);
    assert_eq!(stats.accounted_bytes, w * 2);
    assert_eq!(stats.pinned_bytes, 0);
}

#[test]
fn ttl_exact_deadline_is_miss_and_expired_lease_remains_charged() {
    let w = weight(4);
    let (mut cache, clock) = cache(w * 2, 2);
    let held = put(&mut cache, 1, 1, 4).unwrap();
    assert_eq!(held.expires_at(), Duration::from_secs(10));
    clock.advance(Duration::from_secs(9));
    drop(cache.get(&1).unwrap());
    clock.advance(Duration::from_secs(1));
    assert!(cache.get(&1).is_none());
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().misses, 1);
    assert_eq!(cache.stats().expirations, 1);
    assert_eq!(cache.stats().resident_entries, 0);
    assert_eq!(cache.stats().accounted_bytes, w);
    assert_eq!(cache.stats().pinned_bytes, w);
    assert_eq!(&*held, &[1; 4]);
    drop(held);
    assert_eq!(cache.stats().accounted_bytes, 0);
    assert_eq!(cache.stats().live_entries, 0);
}

#[test]
fn oversized_and_overflow_admissions_preserve_existing_snapshot() {
    let w = weight(4);
    let (mut cache, _) = cache(w * 2, 2);
    drop(put(&mut cache, 1, 1, 4).unwrap());
    assert!(matches!(
        put(&mut cache, 2, 2, w * 2),
        Err(AdmissionError::Oversized { .. })
    ));
    assert_eq!(
        cache
            .insert(2, vec![], usize::MAX, Duration::from_secs(1))
            .unwrap_err(),
        AdmissionError::WeightOverflow
    );
    assert_eq!(&*cache.get(&1).unwrap(), &[1; 4]);
    assert_eq!(cache.stats().resident_entries, 1);
    assert_eq!(cache.stats().evictions, 0);
    assert_eq!(cache.stats().admissions_rejected, 2);
}

#[test]
fn invalidated_snapshot_and_every_clone_hold_budget_until_last_drop() {
    let w = weight(4);
    let (mut cache, _) = cache(w, 1);
    let held = put(&mut cache, 1, 1, 4).unwrap();
    let clone = held.clone();
    assert!(cache.invalidate(&1));
    assert!(cache.get(&1).is_none());
    assert_eq!(cache.stats().pinned_bytes, w);
    assert_eq!(cache.stats().pinned_entries, 1);
    assert_eq!(cache.stats().resident_bytes, 0);
    assert!(
        matches!(put(&mut cache, 2, 2, 4), Err(AdmissionError::BudgetExceeded { pinned_bytes, live_entries: 1, .. }) if pinned_bytes == w)
    );
    drop(held);
    assert_eq!(cache.stats().accounted_bytes, w);
    drop(clone);
    assert_eq!(cache.stats().accounted_bytes, 0);
    drop(put(&mut cache, 2, 2, 4).unwrap());
    assert_eq!(cache.stats().accounted_bytes, w);
}

#[test]
fn replacement_failure_preserves_old_entry_and_original_ttl() {
    let w = weight(4);
    let (mut cache, clock) = cache(w, 1);
    let old = put(&mut cache, 1, 1, 4).unwrap();
    clock.advance(Duration::from_secs(5));
    assert!(matches!(
        put(&mut cache, 1, 9, 4),
        Err(AdmissionError::BudgetExceeded { .. })
    ));
    let current = cache.get(&1).unwrap();
    assert_eq!(&*current, &[1; 4]);
    assert_eq!(current.expires_at(), Duration::from_secs(10));
    drop(current);
    drop(old);
    let replacement = put(&mut cache, 1, 9, 4).unwrap();
    assert_eq!(&*replacement, &[9; 4]);
    assert_eq!(replacement.expires_at(), Duration::from_secs(15));
    assert_eq!(cache.stats().live_entries, 1);
    assert_eq!(cache.stats().accounted_bytes, w);
}

#[test]
fn replacement_can_retire_old_lease_and_evict_another_reclaimable_entry() {
    let w = weight(4);
    let (mut cache, _) = cache(w * 2, 2);
    let old = put(&mut cache, 1, 1, 4).unwrap();
    drop(put(&mut cache, 2, 2, 4).unwrap());
    let new = put(&mut cache, 1, 9, 4).unwrap();
    assert_eq!(&*old, &[1; 4]);
    assert_eq!(&*new, &[9; 4]);
    assert!(cache.get(&2).is_none());
    assert_eq!(cache.stats().resident_entries, 1);
    assert_eq!(cache.stats().live_entries, 2);
    assert_eq!(cache.stats().pinned_bytes, w * 2);
    assert_eq!(cache.stats().evictions, 1);
    drop(old);
    assert_eq!(cache.stats().accounted_bytes, w);
}

#[test]
fn failed_admission_does_not_partially_evict_reclaimable_candidates() {
    let w = weight(4);
    let (mut cache, _) = cache(w * 2, 3);
    let pinned = put(&mut cache, 1, 1, 4).unwrap();
    drop(put(&mut cache, 2, 2, 4).unwrap());
    assert!(matches!(
        put(&mut cache, 3, 3, w),
        Err(AdmissionError::BudgetExceeded { .. })
    ));
    assert_eq!(cache.stats().evictions, 0);
    assert_eq!(cache.stats().resident_entries, 2);
    assert_eq!(&*cache.get(&2).unwrap(), &[2; 4]);
    assert_eq!(&*pinned, &[1; 4]);
}

#[test]
fn clear_force_refresh_and_pruning_release_only_unleased_payloads() {
    let w = weight(4);
    let (mut cache, clock) = cache(w * 3, 3);
    let held = put(&mut cache, 1, 1, 4).unwrap();
    drop(put(&mut cache, 2, 2, 4).unwrap());
    assert!(cache.force_refresh(&2));
    assert!(!cache.force_refresh(&2));
    drop(put(&mut cache, 3, 3, 4).unwrap());
    clock.advance(Duration::from_secs(10));
    assert_eq!(cache.prune_expired(), 2);
    assert_eq!(cache.stats().accounted_bytes, w);
    drop(put(&mut cache, 4, 4, 4).unwrap());
    cache.clear();
    assert_eq!(cache.stats().resident_entries, 0);
    assert_eq!(cache.stats().accounted_bytes, w);
    assert_eq!(cache.stats().pinned_bytes, w);
    drop(held);
    assert_eq!(cache.stats().accounted_bytes, 0);
}

#[test]
fn live_entry_count_bound_applies_even_with_abundant_byte_budget() {
    let w = weight(4);
    let (mut cache, _) = cache(w * 100, 1);
    let held = put(&mut cache, 1, 1, 4).unwrap();
    cache.clear();
    assert!(matches!(
        put(&mut cache, 2, 2, 4),
        Err(AdmissionError::BudgetExceeded {
            live_entries: 1,
            ..
        })
    ));
    drop(held);
    for key in 2..102 {
        drop(put(&mut cache, key, 1, 4).unwrap());
    }
    assert_eq!(cache.stats().live_entries, 1);
    assert_eq!(cache.stats().evictions, 99);
}

#[test]
fn leases_are_thread_safe_when_values_are_and_survive_cache_drop() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MetadataLease<Vec<u8>>>();
    let w = weight(4);
    let (mut cache, _) = cache(w, 1);
    let held = put(&mut cache, 1, 1, 4).unwrap();
    cache.clear();
    std::thread::spawn(move || {
        assert_eq!(&*held, &[1; 4]);
    })
    .join()
    .unwrap();
    assert_eq!(cache.stats().accounted_bytes, 0);
    let held = put(&mut cache, 2, 2, 4).unwrap();
    drop(cache);
    assert_eq!(&*held, &[2; 4]);
    drop(held);
}

#[test]
fn misses_do_not_populate_or_fetch_and_keys_separate_endpoints_databases_and_scopes() {
    type Scoped = MetadataCache<(u64, String, String), String, ManualClock>;
    let mut cache = Scoped::with_clock(CacheConfig::default(), ManualClock::default()).unwrap();
    let key = (1, "db_a".into(), "public.tables".into());
    for _ in 0..100 {
        assert!(cache.get(&key).is_none());
    }
    assert_eq!(cache.stats().resident_entries, 0);
    assert_eq!(cache.stats().accounted_bytes, 0);
    let value = String::from("bounded table names");
    let heap = key.1.capacity() + key.2.capacity() + value.capacity();
    drop(
        cache
            .insert(key.clone(), value, heap, Duration::from_secs(1))
            .unwrap(),
    );
    for other in [
        (2, "db_a".into(), "public.tables".into()),
        (1, "db_b".into(), "public.tables".into()),
        (1, "db_a".into(), "private.tables".into()),
    ] {
        assert!(cache.get(&other).is_none());
    }
    assert_eq!(&*cache.get(&key).unwrap(), "bounded table names");
}

#[test]
fn invalid_config_zero_ttl_and_deadline_overflow_are_visible() {
    assert!(
        Cache::with_clock(
            CacheConfig {
                max_accounted_bytes: 0,
                max_live_entries: 1
            },
            ManualClock::default()
        )
        .is_err()
    );
    assert!(
        Cache::with_clock(
            CacheConfig {
                max_accounted_bytes: 1,
                max_live_entries: 0
            },
            ManualClock::default()
        )
        .is_err()
    );
    let (mut cache, clock) = cache(weight(4), 1);
    assert_eq!(
        cache.insert(1, vec![0; 4], 4, Duration::ZERO).unwrap_err(),
        AdmissionError::ZeroTtl
    );
    clock.advance(Duration::MAX);
    assert_eq!(
        cache
            .insert(1, vec![0; 4], 4, Duration::from_nanos(1))
            .unwrap_err(),
        AdmissionError::TtlOverflow
    );
    assert_eq!(cache.stats().accounted_bytes, 0);
    assert_eq!(cache.stats().admissions_rejected, 2);
    assert!(MetadataCache::<(), ()>::accounted_size(0).unwrap() > 0);
}
