// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 按表划分的内存 KV 缓存（cacheDB）：在事务缓冲与底层快照之间减少回源。
//
// 每个 table ID 独占一个固定容量（约 100 MiB）的缓存实例。`UnionGet`
// 先查缓存，未命中再读快照并写回；`Delete` 按表整体失效。对齐 Go
// `pkg/kv/cachedb.go` 的并发语义（读写锁）与失败传播规则。

// 对齐 pkg/kv/cachedb.go 的按表缓存、回源和清理语义。

use std::collections::HashMap;
use std::sync::RwLock;

/// cacheDB 对应 Go 的私有缓存实现；每个表 ID 独占一个固定容量缓存。
/// Go 将互斥锁与 map 分开存放，这里用 RwLock 包住 map，保持并发读和独占写语义。
pub struct cacheDB {
    /// table ID → 与 freecache v1.2.1 等价的固定分段环形缓存。
    memTables: RwLock<HashMap<i64, TableCache>>,
}

/// 单表缓存容量上限（字节），与 Go 的 100 MiB 一致。
const TABLE_CACHE_CAPACITY: usize = 100 * 1024 * 1024;
/// freecache 的 key 长度存储为 uint16。
const FREECACHE_MAX_KEY_LENGTH: usize = u16::MAX as usize;
/// freecache 每个 entry 的固定头大小。
const FREECACHE_ENTRY_HEADER_SIZE: usize = 24;
/// freecache 将缓存分成 256 段，并把单 entry 限制为段容量的四分之一。
const FREECACHE_MAX_KEY_VALUE_LENGTH: usize =
    TABLE_CACHE_CAPACITY / 1024 - FREECACHE_ENTRY_HEADER_SIZE;

/// MemManager 在事务缓冲区与底层存储之间增加缓存，以减少发往存储的请求。
/// 分区表同样使用 table ID；即使物理 ID 相同与否，分区键本身仍保证唯一。
pub trait MemManager: Send + Sync {
    /// UnionGet 先查询 cacheDB；未命中时读取快照，再把结果写回缓存。
    fn UnionGet(
        &self,
        ctx: &context::Context,
        tid: i64,
        snapshot: &dyn Snapshot,
        key: &Key,
    ) -> Result<Vec<u8>, Error>;

    /// Delete 按 table ID 释放缓存。
    fn Delete(&self, tableID: i64);
}

impl cacheDB {
    /// set 对应 Go 的 cacheDB.set：必要时为表创建 100 MiB 缓存，然后写入永不过期的条目。
    fn set(&self, tableID: i64, key: &Key, value: &[u8]) -> Result<(), Error> {
        // 写锁覆盖“查找或创建”和 Set，避免两个线程为同一 table ID 建立不同缓存。
        let mut memTables = self
            .memTables
            .write()
            .map_err(|err| errors::New(err.to_string()))?;
        let table = memTables.entry(tableID).or_insert_with(TableCache::new);

        // freecache 先检查 uint16 key 上限，再检查单 entry 的分片容量上限。
        if key.0.len() > FREECACHE_MAX_KEY_LENGTH {
            return Err(errors::New("The key is larger than 65535"));
        }
        if key.0.len().saturating_add(value.len()) > FREECACHE_MAX_KEY_VALUE_LENGTH {
            return Err(errors::New(
                "The entry size is larger than 1/1024 of cache size",
            ));
        }

        // Go 的过期秒数为 0，表示该条目不会因 TTL 自动过期。
        table.insert(&key.0, value);
        Ok(())
    }

    /// get 对应 Go 的 cacheDB.get；缓存缺失折叠为 None。
    fn get(&self, tableID: i64, key: &Key) -> Option<Vec<u8>> {
        // 读锁在缓存查询完成后自动释放，等价于 Go 的 RLock/defer RUnlock。
        let memTables = self.memTables.read().ok()?;
        let table = memTables.get(&tableID)?;
        // freecache.Get 用 nil 缓冲读取，零长度 value 仍返回 nil；UnionGet 必须回源。
        table.get(key.as_ref()).filter(|value| !value.is_empty())
    }
}

impl MemManager for cacheDB {
    /// UnionGet 实现 Go MemManager.UnionGet 的“缓存与快照并集读取”语义。
    fn UnionGet(
        &self,
        ctx: &context::Context,
        tid: i64,
        snapshot: &dyn Snapshot,
        key: &Key,
    ) -> Result<Vec<u8>, Error> {
        if let Some(value) = self.get(tid, key) {
            return Ok(value);
        }

        // 未命中才调用外部 GetValue 回源快照。
        let value = GetValue(ctx, snapshot, key.clone())?;
        // 与 Go 一致，写缓存失败会使本次读取整体失败，不返回已经取得的 value。
        self.set(tid, key, &value)?;
        Ok(value)
    }

    /// Delete 清空并移除指定表缓存；不存在的 table ID 不产生错误。
    fn Delete(&self, tableID: i64) {
        // 独占写锁保证 Clear 与 remove 不会和并发 get/set 交错。
        if let Ok(mut memTables) = self.memTables.write() {
            // Go Clear 后移除；这里没有逸出的表引用，drop 同步释放环形缓冲和索引。
            memTables.remove(&tableID);
        }
    }
}

/// NewCacheDB 对应 Go 构造函数，返回隐藏具体实现的 MemManager trait object。
pub fn NewCacheDB() -> Box<dyn MemManager> {
    Box::new(cacheDB {
        memTables: RwLock::new(HashMap::new()),
    })
}

// The private backend ports the Set(key, value, 0), Get and lifetime semantics
// used by cacheDB from coocood/freecache v1.2.1 (MIT, Copyright (c) 2015 Ewan Chou.).
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

const CACHE_SEGMENTS: usize = 256;

struct TableCache {
    segments: Vec<std::sync::Mutex<CacheSegment>>,
}

impl TableCache {
    fn new() -> Self {
        Self {
            segments: (0..CACHE_SEGMENTS)
                .map(|_| {
                    std::sync::Mutex::new(CacheSegment::new(TABLE_CACHE_CAPACITY / CACHE_SEGMENTS))
                })
                .collect(),
        }
    }

    fn now() -> u32 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32
    }

    fn insert(&self, key: &[u8], value: &[u8]) {
        let hash = xxhash_rust::xxh64::xxh64(key, 0);
        let mut segment = self.segments[hash as usize & 255].lock().unwrap();
        segment.set(key, value, hash, Self::now());
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let hash = xxhash_rust::xxh64::xxh64(key, 0);
        let mut segment = self.segments[hash as usize & 255].lock().unwrap();
        segment.get(key, hash, Self::now())
    }
}

#[derive(Clone, Copy, Default)]
struct CacheEntryPtr {
    offset: u64,
    hash16: u16,
    key_len: u16,
    // Preserve freecache's 16-byte pointer size and contiguous slot allocation.
    _reserved: u32,
}

#[derive(Default)]
struct CacheHeader {
    access_time: u32,
    key_len: u16,
    hash16: u16,
    value_len: u32,
    value_cap: u32,
    deleted: bool,
    slot: u8,
}

impl CacheHeader {
    fn size(&self) -> usize {
        FREECACHE_ENTRY_HEADER_SIZE + self.key_len as usize + self.value_cap as usize
    }
}

// Fixed ring and 256 sorted hash slots, matching segment.go. Deleted entries
// remain in the ring (and in total_time/count) until evacuation reaches them.
// No TTL API is exposed: cacheDB exclusively calls freecache.Set with TTL zero.
struct CacheSegment {
    data: Vec<u8>,
    end: u64,
    vacuum: usize,
    total_time: i64,
    total_count: i64,
    slot_cap: usize,
    slot_lens: [usize; 256],
    slots: Vec<CacheEntryPtr>,
}

impl CacheSegment {
    fn new(size: usize) -> Self {
        Self {
            data: vec![0; size],
            end: 0,
            vacuum: size,
            total_time: 0,
            total_count: 0,
            slot_cap: 1,
            slot_lens: [0; 256],
            slots: vec![CacheEntryPtr::default(); 256],
        }
    }

    fn read(&self, offset: u64, output: &mut [u8]) {
        let index = (offset % self.data.len() as u64) as usize;
        let first = output.len().min(self.data.len() - index);
        output[..first].copy_from_slice(&self.data[index..index + first]);
        let rest = output.len() - first;
        output[first..].copy_from_slice(&self.data[..rest]);
    }

    fn write(&mut self, offset: u64, bytes: &[u8]) {
        let index = (offset % self.data.len() as u64) as usize;
        let first = bytes.len().min(self.data.len() - index);
        self.data[index..index + first].copy_from_slice(&bytes[..first]);
        self.data[..bytes.len() - first].copy_from_slice(&bytes[first..]);
    }

    fn header(&self, offset: u64) -> CacheHeader {
        let mut bytes = [0; FREECACHE_ENTRY_HEADER_SIZE];
        self.read(offset, &mut bytes);
        CacheHeader {
            access_time: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            // Bytes 4..8 are expireAt, always zero for this API.
            key_len: u16::from_le_bytes(bytes[8..10].try_into().unwrap()),
            hash16: u16::from_le_bytes(bytes[10..12].try_into().unwrap()),
            value_len: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            value_cap: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            deleted: bytes[20] != 0,
            slot: bytes[21],
        }
    }

    fn write_header(&mut self, offset: u64, header: &CacheHeader) {
        let mut bytes = [0; FREECACHE_ENTRY_HEADER_SIZE];
        bytes[0..4].copy_from_slice(&header.access_time.to_le_bytes());
        bytes[8..10].copy_from_slice(&header.key_len.to_le_bytes());
        bytes[10..12].copy_from_slice(&header.hash16.to_le_bytes());
        bytes[12..16].copy_from_slice(&header.value_len.to_le_bytes());
        bytes[16..20].copy_from_slice(&header.value_cap.to_le_bytes());
        bytes[20] = u8::from(header.deleted);
        bytes[21] = header.slot;
        self.write(offset, &bytes);
    }

    fn lookup(&self, slot: usize, hash16: u16, key: &[u8]) -> (usize, bool) {
        let start = slot * self.slot_cap;
        let entries = &self.slots[start..start + self.slot_lens[slot]];
        let mut index = entries.partition_point(|entry| entry.hash16 < hash16);
        while index < entries.len() && entries[index].hash16 == hash16 {
            let entry = entries[index];
            if entry.key_len as usize == key.len() {
                let offset = ((entry.offset + FREECACHE_ENTRY_HEADER_SIZE as u64)
                    % self.data.len() as u64) as usize;
                let first = key.len().min(self.data.len() - offset);
                if self.data[offset..offset + first] == key[..first]
                    && self.data[..key.len() - first] == key[first..]
                {
                    return (index, true);
                }
            }
            index += 1;
        }
        (index, false)
    }

    fn remove(&mut self, slot: usize, index: usize) {
        let start = slot * self.slot_cap;
        let offset = self.slots[start + index].offset;
        let mut header = self.header(offset);
        header.deleted = true;
        self.write_header(offset, &header);
        self.slots.copy_within(
            start + index + 1..start + self.slot_lens[slot],
            start + index,
        );
        self.slot_lens[slot] -= 1;
    }

    fn insert_ptr(&mut self, slot: usize, index: usize, ptr: CacheEntryPtr) {
        if self.slot_lens[slot] == self.slot_cap {
            let mut expanded = vec![CacheEntryPtr::default(); self.slot_cap * 2 * 256];
            for (slot, len) in self.slot_lens.iter().copied().enumerate() {
                let start = slot * self.slot_cap;
                expanded[start * 2..start * 2 + len]
                    .copy_from_slice(&self.slots[start..start + len]);
            }
            self.slot_cap *= 2;
            self.slots = expanded;
        }
        let start = slot * self.slot_cap;
        self.slots.copy_within(
            start + index..start + self.slot_lens[slot],
            start + index + 1,
        );
        self.slots[start + index] = ptr;
        self.slot_lens[slot] += 1;
    }

    fn evacuate(&mut self, required: usize, inserting_slot: u8) -> bool {
        let mut slot_modified = false;
        let mut consecutive = 0;
        while self.vacuum < required {
            let old_offset = self.end + self.vacuum as u64 - self.data.len() as u64;
            let header = self.header(old_offset);
            let size = header.size();
            if header.deleted {
                consecutive = 0;
                self.total_time -= i64::from(header.access_time);
                self.total_count -= 1;
                self.vacuum += size;
                continue;
            }
            let slot = header.slot as usize;
            let start = slot * self.slot_cap;
            let index = self.slots[start..start + self.slot_lens[slot]]
                .iter()
                .position(|ptr| ptr.hash16 == header.hash16 && ptr.offset == old_offset)
                .expect("live ring entry must have a slot pointer");
            let least_recent = i64::from(header.access_time) * self.total_count <= self.total_time;
            if least_recent || consecutive > 5 {
                self.remove(slot, index);
                slot_modified |= header.slot == inserting_slot;
                consecutive = 0;
                self.total_time -= i64::from(header.access_time);
                self.total_count -= 1;
                self.vacuum += size;
            } else {
                // Copy before wrapping so overlapping source/destination is safe.
                // The temporary is bounded by one entry (one quarter segment).
                let mut bytes = vec![0; size];
                self.read(old_offset, &mut bytes);
                self.write(self.end, &bytes);
                self.slots[start + index].offset = self.end;
                self.end += size as u64;
                consecutive += 1;
            }
        }
        slot_modified
    }

    fn set(&mut self, key: &[u8], value: &[u8], hash: u64, now: u32) {
        let slot = (hash >> 8) as u8;
        let hash16 = (hash >> 16) as u16;
        let (index, found) = self.lookup(slot as usize, hash16, key);
        let mut header = CacheHeader {
            access_time: now,
            key_len: key.len() as u16,
            hash16,
            value_len: value.len() as u32,
            value_cap: value.len().max(1) as u32,
            slot,
            ..Default::default()
        };
        if found {
            let offset = self.slots[slot as usize * self.slot_cap + index].offset;
            let old = self.header(offset);
            header.value_cap = old.value_cap;
            if header.value_cap >= header.value_len {
                self.total_time += i64::from(now) - i64::from(old.access_time);
                self.write_header(offset, &header);
                self.write(
                    offset + FREECACHE_ENTRY_HEADER_SIZE as u64 + key.len() as u64,
                    value,
                );
                return;
            }
            self.remove(slot as usize, index);
            while header.value_cap < header.value_len {
                header.value_cap *= 2;
            }
            header.value_cap = header
                .value_cap
                .min((self.data.len() / 4 - FREECACHE_ENTRY_HEADER_SIZE - key.len()) as u32);
        }
        // Go preserves the original insertion index for equal-hash keys on
        // growth unless evacuation deleted a live pointer in this same slot.
        let index = if self.evacuate(header.size(), slot) {
            let (index, found) = self.lookup(slot as usize, hash16, key);
            debug_assert!(!found);
            index
        } else {
            index
        };
        self.insert_ptr(
            slot as usize,
            index,
            CacheEntryPtr {
                offset: self.end,
                hash16,
                key_len: key.len() as u16,
                _reserved: 0,
            },
        );
        self.write_header(self.end, &header);
        self.write(self.end + FREECACHE_ENTRY_HEADER_SIZE as u64, key);
        self.write(
            self.end + FREECACHE_ENTRY_HEADER_SIZE as u64 + key.len() as u64,
            value,
        );
        self.end += header.size() as u64;
        self.vacuum -= header.size();
        self.total_time += i64::from(now);
        self.total_count += 1;
    }

    fn get(&mut self, key: &[u8], hash: u64, now: u32) -> Option<Vec<u8>> {
        let slot = (hash >> 8) as u8 as usize;
        let (index, found) = self.lookup(slot, (hash >> 16) as u16, key);
        if !found {
            return None;
        }
        let offset = self.slots[slot * self.slot_cap + index].offset;
        let mut header = self.header(offset);
        // Go subtracts uint32 timestamps before converting to int64 in get.
        self.total_time += i64::from(now.wrapping_sub(header.access_time));
        header.access_time = now;
        self.write_header(offset, &header);
        let mut value = vec![0; header.value_len as usize];
        self.read(
            offset + FREECACHE_ENTRY_HEADER_SIZE as u64 + key.len() as u64,
            &mut value,
        );
        Some(value)
    }
}
