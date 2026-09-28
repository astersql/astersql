// Copyright 2026 AsterSQL.

use super::*;

fn invariants<K: Eq + Hash, V>(map: &SwissMap<K, V>) {
    assert_eq!(map.iter().count(), map.len());
    if map.directory.is_empty() {
        assert_eq!(map.tables.len(), 1);
        assert_eq!(map.tables[0].slots.len(), 8);
    } else {
        assert!(map.directory.len().is_power_of_two());
        for (id, table) in map.tables.iter().enumerate() {
            let references: Vec<_> = map
                .directory
                .iter()
                .enumerate()
                .filter_map(|(i, &t)| (t == id).then_some(i))
                .collect();
            assert_eq!(references.len(), map.directory.len() >> table.depth);
            assert!(references.windows(2).all(|w| w[1] == w[0] + 1));
        }
    }
    for table in &map.tables {
        let occupied = table
            .slots
            .iter()
            .filter(|s| !matches!(s, Slot::Empty))
            .count();
        let limit = if map.directory.is_empty() {
            8
        } else {
            table.slots.len() * 7 / 8
        };
        assert_eq!(occupied + table.growth_left, limit);
        for slot in &table.slots {
            if let Slot::Full { key, value, .. } = slot {
                assert!(std::ptr::eq(map.get(key).unwrap(), value));
            }
        }
    }
}

#[test]
fn hints_use_go_load_factor_and_directory_table_sizes() {
    for (hint, capacity, directory, bytes) in [
        (0, 8, 0, 184),
        (8, 8, 0, 184),
        (9, 16, 1, 360),
        (14, 16, 1, 360),
        (15, 32, 1, 632),
        (896, 1024, 1, 17496),
        (897, 1024, 2, 17536),
        (2000, 4096, 4, 69840),
    ] {
        let map = SwissMap::<i64, i64>::with_capacity(hint);
        assert_eq!(
            (
                map.capacity(),
                map.directory_len(),
                map.size(48, 32, 8, 136)
            ),
            (capacity, directory, bytes),
            "hint {hint}"
        );
        invariants(&map);
    }
    assert!(SwissMap::<i64, i64>::with_capacity(usize::MAX).is_empty());
}

#[test]
fn direct_mutations_split_clear_and_refill_preserve_go_accounting() {
    let mut map = SwissMap::default();
    map.set_seed(4992862800126241206);
    let mut expected = HashMap::new();
    for key in 0..6000 {
        assert_eq!(map.insert(key, key * 2), expected.insert(key, key * 2));
    }
    assert!(map.directory_len() > 1);
    invariants(&map);
    let bytes = map.size(48, 32, 8, 136);
    for key in (0..6000).step_by(3) {
        assert_eq!(map.remove(&key), expected.remove(&key));
        assert_eq!(map.remove(&key), None);
    }
    for key in 3000..9000 {
        assert_eq!(map.insert(key, -key), expected.insert(key, -key));
    }
    invariants(&map);
    assert_eq!(
        map.iter().map(|(&k, &v)| (k, v)).collect::<HashMap<_, _>>(),
        expected
    );
    assert!(map.size(48, 32, 8, 136) >= bytes);
    let allocated = map.size(48, 32, 8, 136);
    let seed = map.seed();
    map.clear();
    assert_eq!(map.clear_seq(), 1);
    assert_ne!(map.seed(), seed);
    assert_eq!(map.size(48, 32, 8, 136), allocated);
    map.clear();
    assert_eq!(map.clear_seq(), 1);
    map.set_seed(4992862800126241206);
    for key in 0..1024 {
        map.insert(key, key);
    }
    assert_eq!(map.size(48, 32, 8, 136), allocated);
    invariants(&map);
}

#[test]
fn table_probe_tombstone_reuse_and_pruning_follow_go() {
    let mut table = Table::new(32, 0, false);
    // Fill two groups, delete one whole group. No surviving probe crosses it.
    for i in 0..16 {
        table.unchecked_insert(Slot::Full {
            hash: ((i / 8) as u64) << 7,
            key: i,
            value: i,
        });
    }
    for i in 0..8 {
        table.slots[i] = Slot::Deleted;
    }
    let before = table.growth_left;
    table.prune_tombstones();
    assert_eq!(table.growth_left, before + 8);
    assert!(table.slots[..8].iter().all(|s| matches!(s, Slot::Empty)));

    let mut table = Table::new(32, 0, false);
    for i in 0..16 {
        table.unchecked_insert(Slot::Full {
            hash: 0,
            key: i,
            value: i,
        });
    }
    for i in 0..8 {
        table.slots[i] = Slot::Deleted;
    }
    let before = table.growth_left;
    table.prune_tombstones();
    assert_eq!(
        table.growth_left, before,
        "probe continuation needs the tombstones"
    );
    let slot = table.vacancy(0).unwrap();
    table.place(
        slot,
        Slot::Full {
            hash: 0,
            key: 99,
            value: 99,
        },
    );
    assert_eq!(
        table.growth_left, before,
        "reusing a tombstone consumes no growth"
    );
    assert!(table.find(0, &99).is_some());
}

#[test]
fn deletion_last_key_reseeds_and_retains_capacity() {
    let mut map = SwissMap::with_capacity(15);
    map.insert(1, 2);
    let seed = map.seed();
    assert_eq!(map.remove(&1), Some(2));
    assert_ne!(map.seed(), seed);
    assert_eq!(map.capacity(), 32);
    invariants(&map);
}

#[test]
#[should_panic(expected = "MockSeedForTest can only be called on empty map")]
fn seed_change_rejects_nonempty_maps() {
    let mut map = SwissMap::default();
    map.insert(1, 2);
    map.set_seed(42);
}

#[test]
fn borrowed_lookup_clone_and_drop_release_values_once() {
    use std::sync::Arc;
    let value = Arc::new(42);
    let mut map = SwissMap::default();
    map.insert("key".to_owned(), value.clone());
    assert_eq!(**map.get("key").unwrap(), 42);
    let copy = map.clone();
    assert_eq!(Arc::strong_count(&value), 3);
    map.clear();
    assert_eq!(Arc::strong_count(&value), 2);
    drop(copy);
    assert_eq!(Arc::strong_count(&value), 1);
}

#[test]
fn go_slot_layout_accounts_for_strings_indirection_and_zero_sized_fields() {
    assert_eq!(group_size::<String, i64>(), 200);
    assert_eq!(group_size::<i8, i64>(), 136);
    assert_eq!(group_size::<i64, ()>(), 136);
    assert_eq!(group_size::<i8, ()>(), 24);
    assert_eq!(group_size::<(), ()>(), 16);
    assert_eq!(group_size::<[u8; 129], i64>(), 136);
}
