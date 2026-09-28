// Copyright 2019-present PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// MemStore 有序双向迭代器。
//
// 在跳表锁存储上提供 Seek/Next/Prev 等定位与遍历；当前 key/value 以拷贝缓存，
// 避免外部长期持有 arena 内部切片。

// 对应 iterator.go，实现 MemStore 的双向有序迭代与定位。

use super::lockstore::{MemStore, entry};

// Iterator iterates the entries in the MemStore.
// Iterator 对应 Go 的同名结构，缓存当前 key/value 的拷贝，避免外部直接持有 arena 内部切片。
/// MemStore 有序迭代器，缓存当前 key/value 拷贝。
pub struct Iterator<'a> {
    pub ls: &'a MemStore,
    pub key: Vec<u8>,
    pub val: Vec<u8>,
}

impl MemStore {
    // NewIterator returns a new Iterator for the lock store.
    // NewIterator 对应 Go 的 MemStore.NewIterator，初始 key 为空，因此 Valid 返回 false。
    /// 创建未定位的迭代器（Valid 为 false）。
    pub fn NewIterator(&self) -> Iterator<'_> {
        Iterator {
            ls: self,
            key: Vec::new(),
            val: Vec::new(),
        }
    }
}

impl<'a> Iterator<'a> {
    // Valid returns true iff the iterator is positioned at a valid node.
    // Valid 对应 Go 的有效性判断；空 key 表示未定位或越界。
    /// 是否定位在有效节点。
    pub fn Valid(&self) -> bool {
        !self.key.is_empty()
    }

    // Key returns the key at the current position.
    /// 当前键。
    pub fn Key(&self) -> &[u8] {
        &self.key
    }

    // Value returns value.
    /// 当前值。
    pub fn Value(&self) -> &[u8] {
        &self.val
    }

    // Next moves the iterator to the next entry.
    /// 前进到下一更大键。
    pub fn Next(&mut self) {
        let (e, _) = self.ls.findGreater(&self.key, false);
        self.setKeyValue(e);
    }

    // Prev moves the iterator to the previous entry.
    /// 回退到上一更小键。
    pub fn Prev(&mut self) {
        let (e, _) = self.ls.findLess(&self.key, false); // find <. No equality allowed.
        self.setKeyValue(e);
    }

    // Seek locates the iterator to the first entry with a key >= seekKey.
    /// 定位到第一个 key >= seekKey。
    pub fn Seek(&mut self, seekKey: &[u8]) {
        let (e, _) = self.ls.findGreater(seekKey, true); // find >=.
        self.setKeyValue(e);
    }

    // SeekForPrev locates the iterator to the last entry with key <= target.
    /// 定位到最后一个 key <= target。
    pub fn SeekForPrev(&mut self, target: &[u8]) {
        let (e, _) = self.ls.findLess(target, true); // find <=.
        self.setKeyValue(e);
    }

    // SeekForExclusivePrev locates the iterator to the last entry with key < target.
    /// 定位到最后一个 key < target。
    pub fn SeekForExclusivePrev(&mut self, target: &[u8]) {
        let (e, _) = self.ls.findLess(target, false);
        self.setKeyValue(e);
    }

    // SeekToFirst locates the iterator to the first entry.
    /// 定位到最小键。
    pub fn SeekToFirst(&mut self) {
        let e = self.ls.getNext(self.ls.head, 0);
        self.setKeyValue(e);
    }

    // SeekToLast locates the iterator to the last entry.
    /// 定位到最大键。
    pub fn SeekToLast(&mut self) {
        let e = self.ls.findLast();
        self.setKeyValue(e);
    }

    // setKeyValue 对应 Go 的私有方法，复用已有 Vec 容量模拟 append(it.key[:0], ...).
    /// 用 entry 覆盖缓存的 key/value（复用 Vec 容量）。
    fn setKeyValue(&mut self, e: entry) {
        self.key.clear();
        self.key.extend_from_slice(&e.key);
        self.val.clear();
        self.val.extend_from_slice(&e.getValue(self.ls.getArena()));
    }
}
