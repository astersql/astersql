// Copyright 2026 AsterSQL.
// Copyright 2025 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in GO-LICENSE in this directory.

//! Safe storage following Go 1.25/1.26 internal/runtime/maps map/table algorithms.
//! Allocation accounting describes the Go ABI (header, directory, tables, groups),
//! not Rust enum/Vec overhead. Rust keys use their Hash/Eq implementations; no
//! Rust allocation is reinterpreted as a Go runtime object.

use std::borrow::Borrow;
use std::collections::{HashMap, hash_map::RandomState};
use std::hash::{BuildHasher, DefaultHasher, Hash, Hasher};
use std::ops::Index;

const SLOTS: usize = 8;
const MAX_TABLE: usize = 1024;

#[derive(Clone, Debug)]
enum Slot<K, V> {
    Empty,
    Deleted,
    Full { hash: u64, key: K, value: V },
}

#[derive(Clone, Debug)]
struct Table<K, V> {
    slots: Vec<Slot<K, V>>,
    depth: u32,
    growth_left: usize,
}

impl<K, V> Table<K, V> {
    fn new(capacity: usize, depth: u32, small: bool) -> Self {
        let capacity = capacity.max(SLOTS).next_power_of_two();
        Self {
            slots: std::iter::repeat_with(|| Slot::Empty)
                .take(capacity)
                .collect(),
            depth,
            growth_left: if small { SLOTS } else { capacity * 7 / 8 },
        }
    }

    fn find<Q: Eq + ?Sized>(&self, hash: u64, key: &Q) -> Option<usize>
    where
        K: Borrow<Q>,
    {
        let mask = self.slots.len() / SLOTS - 1;
        let mut group = (hash >> 7) as usize & mask;
        for step in 0..=mask {
            let start = group * SLOTS;
            let mut empty = false;
            for i in start..start + SLOTS {
                match &self.slots[i] {
                    Slot::Full {
                        hash: h, key: k, ..
                    } if *h == hash && k.borrow() == key => return Some(i),
                    Slot::Empty => empty = true,
                    _ => {}
                }
            }
            if empty {
                return None;
            }
            group = (group + step + 1) & mask;
        }
        None
    }

    // Go PutSlot remembers the first tombstone and terminates at an empty slot.
    fn vacancy(&self, hash: u64) -> Option<usize> {
        let mask = self.slots.len() / SLOTS - 1;
        let mut group = (hash >> 7) as usize & mask;
        let mut deleted = None;
        for step in 0..=mask {
            for i in group * SLOTS..(group + 1) * SLOTS {
                match self.slots[i] {
                    Slot::Deleted => {
                        deleted.get_or_insert(i);
                    }
                    Slot::Empty => return Some(deleted.unwrap_or(i)),
                    _ => {}
                }
            }
            group = (group + step + 1) & mask;
        }
        deleted
    }

    fn place(&mut self, index: usize, slot: Slot<K, V>) {
        if !matches!(self.slots[index], Slot::Deleted) {
            self.growth_left -= 1;
        }
        self.slots[index] = slot;
    }

    fn unchecked_insert(&mut self, slot: Slot<K, V>) {
        if let Slot::Full { hash, .. } = &slot {
            let index = self.vacancy(*hash).expect("rehash has room");
            self.place(index, slot);
        }
    }

    // Go pruneTombstones never moves live entries and requires reclaiming 10%.
    fn prune_tombstones(&mut self) {
        let deleted = self
            .slots
            .iter()
            .filter(|s| matches!(s, Slot::Deleted))
            .count();
        if deleted * 10 < self.slots.len() {
            return;
        }
        let groups = self.slots.len() / SLOTS;
        let mask = groups - 1;
        let mut needed = vec![false; groups];
        for (index, slot) in self.slots.iter().enumerate() {
            match slot {
                Slot::Empty => needed[index / SLOTS] = true,
                Slot::Full { hash, .. } => {
                    let mut group = (*hash >> 7) as usize & mask;
                    for step in 0..groups {
                        if group == index / SLOTS {
                            break;
                        }
                        needed[group] = true;
                        group = (group + step + 1) & mask;
                    }
                }
                _ => {}
            }
        }
        let reclaim = self
            .slots
            .iter()
            .enumerate()
            .filter(|(i, s)| !needed[i / SLOTS] && matches!(s, Slot::Deleted))
            .count();
        if reclaim * 10 < self.slots.len() {
            return;
        }
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if !needed[i / SLOTS] && matches!(slot, Slot::Deleted) {
                *slot = Slot::Empty;
                self.growth_left += 1;
            }
        }
    }
}

/// A mutable map whose Go Swiss-table allocation history survives deletion and
/// clear. All structural mutations pass through this type; there is no mutable
/// dereference to a HashMap that could silently bypass the accounting.
#[derive(Clone, Debug)]
pub struct SwissMap<K, V> {
    tables: Vec<Table<K, V>>,
    directory: Vec<usize>,
    used: usize,
    seed: u64,
    clear_seq: u64,
    tombstone_possible: bool,
}

fn fresh_seed() -> u64 {
    RandomState::new().build_hasher().finish()
}

impl<K: Eq + Hash, V> SwissMap<K, V> {
    pub fn with_capacity(hint: usize) -> Self {
        let mut map = Self {
            tables: vec![Table::new(SLOTS, 0, true)],
            directory: Vec::new(),
            used: 0,
            seed: fresh_seed(),
            clear_seq: 0,
            tombstone_possible: false,
        };
        if hint > SLOTS {
            // Match Go NewMap's hint overflow fallback to an empty small map.
            let Some(target) = hint.checked_mul(SLOTS).map(|v| v / 7) else {
                return map;
            };
            let Some(directory_len) = target.div_ceil(MAX_TABLE).checked_next_power_of_two() else {
                return map;
            };
            let capacity = (target / directory_len).max(SLOTS).next_power_of_two();
            map.tables = (0..directory_len)
                .map(|_| Table::new(capacity, directory_len.ilog2(), false))
                .collect();
            map.directory = (0..directory_len).collect();
        }
        map
    }

    fn hash<Q: Hash + ?Sized>(&self, key: &Q) -> u64 {
        let mut hash = DefaultHasher::new();
        hash.write_u64(self.seed);
        key.hash(&mut hash);
        hash.finish()
    }

    fn table_index(&self, hash: u64) -> usize {
        if self.directory.len() <= 1 {
            return 0;
        }
        self.directory[(hash >> (64 - self.directory.len().ilog2())) as usize]
    }

    pub fn len(&self) -> usize {
        self.used
    }
    pub fn is_empty(&self) -> bool {
        self.used == 0
    }
    pub fn seed(&self) -> u64 {
        self.seed
    }
    pub fn clear_seq(&self) -> u64 {
        self.clear_seq
    }
    pub fn directory_len(&self) -> usize {
        self.directory.len()
    }
    pub fn capacity(&self) -> usize {
        self.tables.iter().map(|t| t.slots.len()).sum()
    }

    pub fn set_seed(&mut self, seed: u64) -> u64 {
        assert!(
            self.is_empty(),
            "MockSeedForTest can only be called on empty map"
        );
        std::mem::replace(&mut self.seed, seed)
    }

    fn reset_seed(&mut self) {
        let old = self.seed;
        while self.seed == old {
            self.seed = fresh_seed();
        }
    }

    pub fn get<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        let hash = self.hash(key);
        let table = &self.tables[self.table_index(hash)];
        let index = table.find(hash, key)?;
        match &table.slots[index] {
            Slot::Full { value, .. } => Some(value),
            _ => unreachable!(),
        }
    }

    pub fn get_mut<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        let hash = self.hash(key);
        let index = self.table_index(hash);
        let table = &mut self.tables[index];
        let slot = table.find(hash, key)?;
        match &mut table.slots[slot] {
            Slot::Full { value, .. } => Some(value),
            _ => unreachable!(),
        }
    }

    pub fn contains_key<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.get(key).is_some()
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let hash = self.hash(&key);
        loop {
            let index = self.table_index(hash);
            let table = &mut self.tables[index];
            if let Some(slot) = table.find(hash, &key) {
                if let Slot::Full { value: old, .. } = &mut table.slots[slot] {
                    return Some(std::mem::replace(old, value));
                }
            }
            if let Some(slot) = table.vacancy(hash) {
                if table.growth_left == 0 && !matches!(table.slots[slot], Slot::Deleted) {
                    table.prune_tombstones();
                }
                if table.growth_left > 0 || matches!(table.slots[slot], Slot::Deleted) {
                    table.place(slot, Slot::Full { hash, key, value });
                    self.used += 1;
                    return None;
                }
            }
            self.grow(index);
        }
    }

    fn grow(&mut self, index: usize) {
        let capacity = self.tables[index].slots.len();
        let depth = self.tables[index].depth;
        if capacity < MAX_TABLE {
            let old = std::mem::replace(
                &mut self.tables[index],
                Table::new(capacity * 2, depth, false),
            );
            for slot in old.slots {
                self.tables[index].unchecked_insert(slot);
            }
            if self.directory.is_empty() {
                self.directory.push(0);
            }
            return;
        }
        let global_depth = self.directory.len().ilog2();
        if depth == global_depth {
            self.directory = self.directory.iter().flat_map(|&id| [id, id]).collect();
        }
        let old = std::mem::replace(
            &mut self.tables[index],
            Table::new(MAX_TABLE, depth + 1, false),
        );
        let right = self.tables.len();
        self.tables.push(Table::new(MAX_TABLE, depth + 1, false));
        let mask = 1_u64 << (63 - depth);
        for slot in old.slots {
            if let Slot::Full { hash, .. } = &slot {
                let target = if hash & mask == 0 { index } else { right };
                self.tables[target].unchecked_insert(slot);
            }
        }
        let shift = self.directory.len().ilog2() - depth - 1;
        for (i, table) in self.directory.iter_mut().enumerate() {
            if *table == index && ((i >> shift) & 1) != 0 {
                *table = right;
            }
        }
    }

    pub fn remove<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let hash = self.hash(key);
        let table_index = self.table_index(hash);
        let table = &mut self.tables[table_index];
        let index = table.find(hash, key)?;
        let start = index / SLOTS * SLOTS;
        let empty = self.directory.is_empty()
            || table.slots[start..start + SLOTS]
                .iter()
                .any(|s| matches!(s, Slot::Empty));
        let replacement = if empty {
            table.growth_left += 1;
            Slot::Empty
        } else {
            self.tombstone_possible = true;
            Slot::Deleted
        };
        let removed = std::mem::replace(&mut table.slots[index], replacement);
        self.used -= 1;
        if self.used == 0 {
            self.reset_seed();
        }
        match removed {
            Slot::Full { value, .. } => Some(value),
            _ => unreachable!(),
        }
    }

    pub fn clear(&mut self) {
        if self.used == 0 && !self.tombstone_possible {
            return;
        }
        for table in &mut self.tables {
            for slot in &mut table.slots {
                *slot = Slot::Empty;
            }
            table.growth_left = if self.directory.is_empty() {
                SLOTS
            } else {
                table.slots.len() * 7 / 8
            };
        }
        self.used = 0;
        self.tombstone_possible = false;
        self.clear_seq = self.clear_seq.wrapping_add(1);
        self.reset_seed();
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            tables: self.tables.iter(),
            slots: [].iter(),
        }
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, value)| value)
    }
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(key, _)| key)
    }

    pub fn size(&self, header: u64, table: u64, pointer: u64, group: u64) -> u64 {
        header
            + pointer * self.directory.len() as u64
            + if self.directory.is_empty() {
                group
            } else {
                self.tables
                    .iter()
                    .map(|t| table + group * (t.slots.len() / SLOTS) as u64)
                    .sum()
            }
    }
}

pub struct Iter<'a, K, V> {
    tables: std::slice::Iter<'a, Table<K, V>>,
    slots: std::slice::Iter<'a, Slot<K, V>>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            for slot in self.slots.by_ref() {
                if let Slot::Full { key, value, .. } = slot {
                    return Some((key, value));
                }
            }
            self.slots = self.tables.next()?.slots.iter();
        }
    }
}

impl<'a, K: Eq + Hash, V> IntoIterator for &'a SwissMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<K: Eq + Hash, V> Default for SwissMap<K, V> {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl<K: Eq + Hash, V> From<HashMap<K, V>> for SwissMap<K, V> {
    fn from(map: HashMap<K, V>) -> Self {
        // HashMap import is a Rust convenience; its opaque allocation cannot be
        // used as a Go capacity hint. Reinsert the current entries into Go storage.
        let mut result = Self::with_capacity(map.len());
        for (key, value) in map {
            result.insert(key, value);
        }
        result
    }
}

impl<K: Eq + Hash + Borrow<Q>, V, Q: Hash + Eq + ?Sized> Index<&Q> for SwissMap<K, V> {
    type Output = V;
    fn index(&self, key: &Q) -> &V {
        self.get(key).expect("no entry found for key")
    }
}

// Rust String and byte-string keys represent Go strings, whose header has two
// words. Other types retain their native layout; fields >128 bytes are indirect
// in a Go map slot. Preserve Go's trailing zero-sized field padding as well.
fn go_field_layout<T>() -> (usize, usize) {
    match std::any::type_name::<T>() {
        "alloc::string::String" | "&str" | "alloc::vec::Vec<u8>" => (
            2 * std::mem::size_of::<usize>(),
            std::mem::align_of::<usize>(),
        ),
        _ if std::mem::size_of::<T>() > 128 => {
            (std::mem::size_of::<usize>(), std::mem::align_of::<usize>())
        }
        _ => (std::mem::size_of::<T>(), std::mem::align_of::<T>()),
    }
}

pub(crate) fn group_size<K, V>() -> u64 {
    let (key_size, key_align) = go_field_layout::<K>();
    let (elem_size, elem_align) = go_field_layout::<V>();
    let alignment = key_align.max(elem_align);
    let elem_offset = key_size.next_multiple_of(elem_align);
    let tail = elem_offset + elem_size + usize::from(elem_offset > 0 && elem_size == 0);
    let slot = tail.next_multiple_of(alignment);
    // group = struct { ctrls uint64; slots [8]slot }; the final zero-sized field
    // must have an address inside the allocated object.
    (8 + slot * SLOTS + usize::from(slot == 0)).next_multiple_of(alignment.max(8)) as u64
}

#[cfg(test)]
#[path = "swiss_map_test.rs"]
mod tests;
