// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// MVMap：同 key 可存多 value 的哈希表，尽量降低 GC/分配开销。
//
// 对应 Go `pkg/util/mvmap`。key/value 连续写入 data 分片，元数据写入 entry
// 分片；hash 表仅存桶头 `entryAddr`。非线程安全，应只在单执行流中使用。

use std::collections::HashMap;

/// FNV-1 64 位哈希子模块。
mod fnv;

use fnv::fnv_hash64;

/// hash 桶链表节点：数据地址、key/value 长度及同桶下一节点。
// entry 对应 Go 的 entry 结构体，保存数据地址、key/value 长度以及同 hash 桶里的下一项。
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct entry {
    addr: dataAddr,
    keyLen: u32,
    valLen: u32,
    next: entryAddr,
}

/// entry 分片存储：按固定容量切片追加，降低单 Vec 扩容压力。
// entryStore 对应 Go 的 entryStore，按固定容量分片追加 entry，减少单个 Vec 的扩容压力。
#[derive(Default)]
struct entryStore {
    slices: Vec<Vec<entry>>,
    sliceIdx: u32,
    sliceLen: u32,
}

/// data 分片存储：key 与 value 连续写入同一字节分片。
// dataStore 对应 Go 的 dataStore，key 和 value 连续写入同一字节分片。
#[derive(Default)]
struct dataStore {
    slices: Vec<Vec<u8>>,
    sliceIdx: u32,
    sliceLen: u32,
}

/// entry 在分片中的地址（分片号 + 分片内偏移）。
// entryAddr 对应 Go 的 entryAddr，使用分片编号和分片内偏移定位 entry。
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct entryAddr {
    sliceIdx: u32,
    offset: u32,
}

/// key/value 原始字节在 data 分片中的地址。
// dataAddr 对应 Go 的 dataAddr，使用分片编号和分片内偏移定位 key/value 原始字节。
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct dataAddr {
    sliceIdx: u32,
    offset: u32,
}

/// 单个 data 分片的目标最大长度（64KiB）。
const maxDataSliceLen: u32 = 64 * 1024;
/// 单个 entry 分片的最大条数（8Ki）。
const maxEntrySliceLen: u32 = 8 * 1024;

impl dataStore {
    /// 把 key 和 value 连续追加到当前分片，返回起始 `dataAddr`。
    // put 对应 Go 的 (*dataStore).put，把 key 和 value 连续追加到当前分片并返回起始地址。
    fn put(&mut self, key: &[u8], value: &[u8]) -> dataAddr {
        // Go 代码把 len(key)+len(value) 转成 uint32；这里保留 u32 形状，超大输入的截断风险与语义一并保留。
        let dataLen = (key.len() + value.len()) as u32;
        if self.sliceLen != 0 && self.sliceLen + dataLen > maxDataSliceLen {
            // 当前分片已容不下完整 key/value 时，新建分片；容量沿用 max(maxDataSliceLen, dataLen)。
            let capacity = std::cmp::max(maxDataSliceLen, dataLen) as usize;
            self.slices.push(Vec::with_capacity(capacity));
            self.sliceLen = 0;
            self.sliceIdx += 1;
        }
        let addr = dataAddr {
            sliceIdx: self.sliceIdx,
            offset: self.sliceLen,
        };
        let slice = &mut self.slices[self.sliceIdx as usize];
        slice.extend_from_slice(key);
        slice.extend_from_slice(value);
        self.sliceLen += dataLen;
        addr
    }

    /// 校验 entry 中保存的 key 字节，匹配则返回对应 value 切片。
    // get 对应 Go 的 (*dataStore).get，先校验 entry 里保存的 key 字节，再返回 value。
    fn get<'a>(&'a self, e: entry, key: &[u8]) -> Option<&'a [u8]> {
        let slice = &self.slices[e.addr.sliceIdx as usize];
        let keyOffset = e.addr.offset as usize;
        let valOffset = keyOffset + e.keyLen as usize;
        // Go 里 bytes.Equal 失败时返回 nil；用 None 表达“该 hash 链节点不是目标 key”。
        if key != &slice[keyOffset..valOffset] {
            return None;
        }
        Some(&slice[valOffset..valOffset + e.valLen as usize])
    }

    /// 按 entry 元数据切出 key/value 字节对，供迭代器使用。
    // getEntryData 对应 Go 的 (*dataStore).getEntryData，迭代器使用它取回一组 key/value 切片。
    fn getEntryData<'a>(&'a self, e: entry) -> (&'a [u8], &'a [u8]) {
        let slice = &self.slices[e.addr.sliceIdx as usize];
        let keyOffset = e.addr.offset as usize;
        let key = &slice[keyOffset..keyOffset + e.keyLen as usize];
        let valOffset = e.addr.offset as usize + e.keyLen as usize;
        let value = &slice[valOffset..valOffset + e.valLen as usize];
        (key, value)
    }
}

/// 零值 entryAddr，表示 hash 链表结束（null）。
// nullEntryAddr 对应 Go 的零值 entryAddr，用于表示 hash 链表结束。
const nullEntryAddr: entryAddr = entryAddr {
    sliceIdx: 0,
    offset: 0,
};

impl entryStore {
    /// 向 entry 分片追加一个 entry，返回其 `entryAddr`。
    // put 对应 Go 的 (*entryStore).put，向 entry 分片追加一个 entry 并返回其地址。
    fn put(&mut self, e: entry) -> entryAddr {
        if self.sliceLen == maxEntrySliceLen {
            // 分片满时创建下一片；没有并发同步，沿用 MVMap 只在单 goroutine 使用的前提。
            self.slices
                .push(Vec::with_capacity(maxEntrySliceLen as usize));
            self.sliceLen = 0;
            self.sliceIdx += 1;
        }
        let addr = entryAddr {
            sliceIdx: self.sliceIdx,
            offset: self.sliceLen,
        };
        self.slices[self.sliceIdx as usize].push(e);
        self.sliceLen += 1;
        addr
    }

    /// 根据 `entryAddr` 取出 entry 值副本。
    // get 对应 Go 的 (*entryStore).get，根据 entryAddr 返回一个 entry 值。
    fn get(&self, addr: entryAddr) -> entry {
        self.slices[addr.sliceIdx as usize][addr.offset as usize]
    }
}

// MVMap stores multiple value for a given key with minimum GC overhead.
// A given key can store multiple values.
// It is not thread-safe, should only be used in one goroutine.
/// 多值哈希表：hash 桶头 + entry/data 分片 + value 计数。
// MVMap 按 Go 结构保存 hash 表、entry 分片、data 分片和 value 总数。
// Rust 不实现锁，也不声明 Send/Sync；调用方仍应按 Go 注释只在单执行流中使用。
pub struct MVMap {
    hashTable: HashMap<u64, entryAddr>,
    entryStore: entryStore,
    dataStore: dataStore,
    length: usize,
}

// NewMVMap creates a new multi-value map.
/// 构造空 MVMap：初始化首个 entry/data 分片，并写入占位空 entry。
// NewMVMap 对应 Go 的构造函数，初始化 hash 表、entry/data 首分片，并写入首个空 entry。
pub fn NewMVMap() -> MVMap {
    let mut m = MVMap {
        hashTable: HashMap::new(),
        entryStore: entryStore {
            slices: vec![Vec::with_capacity(64)],
            sliceIdx: 0,
            sliceLen: 0,
        },
        dataStore: dataStore {
            slices: vec![Vec::with_capacity(1024)],
            sliceIdx: 0,
            sliceLen: 0,
        },
        length: 0,
    };
    // Append the first empty entry, so the zero entryAddr can represent null.
    // 首个空 entry 占住 (0,0)，让零值 entryAddr 可以安全表示链表结束。
    m.entryStore.put(entry::default());
    m
}

impl MVMap {
    // Put puts the key/value pairs to the MVMap, if the key already exists, old value will not be overwritten,
    // values are stored in a list.
    /// 追加一对 key/value；同 key 不覆盖旧值，新 entry 插到 hash 链头。
    // Put 对应 Go 的 (*MVMap).Put：同 key 多次写入时新 entry 插到 hash 链表头部，旧值不会覆盖。
    pub fn Put(&mut self, key: &[u8], value: &[u8]) {
        let hashKey = fnv_hash64(key);
        // Go map 缺失键会返回 entryAddr 零值；显式回退到 nullEntryAddr。
        let oldEntryAddr = self
            .hashTable
            .get(&hashKey)
            .copied()
            .unwrap_or(nullEntryAddr);
        let dataAddr = self.dataStore.put(key, value);
        let e = entry {
            addr: dataAddr,
            keyLen: key.len() as u32,
            valLen: value.len() as u32,
            next: oldEntryAddr,
        };
        let newEntryAddr = self.entryStore.put(e);
        self.hashTable.insert(hashKey, newEntryAddr);
        self.length += 1;
    }

    // Get gets the values of the "key" and appends them to "values".
    /// 沿 hash 链收集同 key 的 value，追加到传入 `values` 后整体反转以恢复 Put 顺序。
    // Get 对应 Go 的 (*MVMap).Get，沿 hash 链查找相同 key，并把匹配 value 追加到传入 values。
    pub fn Get<'a>(&'a self, key: &[u8], mut values: Vec<&'a [u8]>) -> Vec<&'a [u8]> {
        let hashKey = fnv_hash64(key);
        let mut entryAddr = self
            .hashTable
            .get(&hashKey)
            .copied()
            .unwrap_or(nullEntryAddr);
        while entryAddr != nullEntryAddr {
            let e = self.entryStore.get(entryAddr);
            entryAddr = e.next;
            let val = self.dataStore.get(e, key);
            if val.is_none() {
                // hash 相同但 key 字节不同，对应 Go 中 val == nil 时 continue 的碰撞过滤分支。
                continue;
            }
            values.push(val.unwrap());
        }
        // Keep the order of input.
        // Go 插入时新值在链表头，因此遍历结果是倒序；这里反转整个 values，机械保留原实现。
        values.reverse();
        values
    }

    // Len returns the number of values in th mv map, the number of keys may be less than Len
    // if the same key is put more than once.
    /// 返回已存 value 条数；同 key 多次 Put 会使 Len 大于 key 种类数。
    // Len 返回 value 数量；同一个 key 多次 Put 会让长度继续增长。
    pub fn Len(&self) -> usize {
        self.length
    }

    // NewIterator creates a iterator for the MVMap.
    /// 创建按 entryStore 物理顺序遍历的迭代器。
    // NewIterator 对应 Go 的 (*MVMap).NewIterator，创建按 entryStore 物理顺序遍历的迭代器。
    pub fn NewIterator(&self) -> Iterator<'_> {
        // The first entry is empty, so init entryCur to 1.
        // 第 0 个 entry 是 null 占位，迭代时从 1 开始跳过。
        Iterator {
            m: self,
            sliceCur: 0,
            entryCur: 1,
        }
    }
}

// Iterator is used to iterate the MVMap.
/// 按 entry 分片物理顺序遍历 MVMap；生命周期绑定被遍历的 map。
// Iterator 保存当前 entry 分片和分片内游标；生命周期绑定到被遍历的 MVMap。
pub struct Iterator<'a> {
    m: &'a MVMap,
    sliceCur: usize,
    entryCur: usize,
}

impl<'a> Iterator<'a> {
    // Next returns the next key/value pair of the MVMap.
    // It returns (nil, nil) when there is no more entries to iterate.
    /// 返回下一组 key/value；耗尽时为 `(None, None)`，对齐 Go 的 `(nil, nil)`。
    // Next 对应 Go 的 (*Iterator).Next；耗尽时用 (None, None) 表达 Go 的 (nil, nil)。
    pub fn Next(&mut self) -> (Option<&'a [u8]>, Option<&'a [u8]>) {
        loop {
            if self.sliceCur >= self.m.entryStore.slices.len() {
                return (None, None);
            }
            let entrySlice = &self.m.entryStore.slices[self.sliceCur];
            if self.entryCur >= entrySlice.len() {
                // 当前分片耗尽时切到下一分片，保持 Go for 循环里的 continue 控制流。
                self.sliceCur += 1;
                self.entryCur = 0;
                continue;
            }
            let entry = entrySlice[self.entryCur];
            let (key, value) = self.m.dataStore.getEntryData(entry);
            self.entryCur += 1;
            return (Some(key), Some(value));
        }
    }
}
