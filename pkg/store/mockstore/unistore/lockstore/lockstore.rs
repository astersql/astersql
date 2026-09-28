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

// 单写多读的 arena-backed 跳表锁存储（MemStore）。
//
// 跳表（skiplist）按 key 有序串接多层前向指针；写入假定单线程，读路径通过原子 next
// 观察结构。节点布局在 arena 字节块中，删除后可延迟复用内存，避免用量持续膨胀。

// 对应 lockstore.go，实现单写多读的 arena-backed 跳表锁存储。

use std::cmp::Ordering;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU64, Ordering as AtomicOrdering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::arena::{arena, arenaAddr, newArenaLocator, nullArenaAddr};
use rand::{RngCore, SeedableRng};

// MemStore is a skiplist variant used to store lock.
// Compares to normal skip list, it only supports single thread write.
// But it can reuse the memory, so that the memory usage doesn't keep growing.
// MemStore 对应 Go 的跳表变体：写入端假定单线程，读路径通过原子 next 指针观察结构。
/// 跳表锁存储：单写多读，arena 承载节点字节。
pub struct MemStore {
    pub height: AtomicI32, // Current height. 1 <= height <= maxHeight.
    pub head: *mut node,
    pub arenaPtr: AtomicPtr<arena>,
    retiredArenas: Vec<*mut arena>,

    // We only consume 2 bits for a random height call.
    pub rand: rand::rngs::StdRng,
    pub length: usize,
}

// Go exposes lockstore to concurrent readers while retaining a single writer.
// Rust mutation requires &mut MemStore; published next links and arena pointers
// use atomics, so immutable references may be shared across reader threads.
unsafe impl Send for MemStore {}
unsafe impl Sync for MemStore {}

impl Drop for MemStore {
    fn drop(&mut self) {
        let arena = self.arenaPtr.load(AtomicOrdering::SeqCst);
        if !arena.is_null() {
            unsafe {
                drop(Box::from_raw(arena));
            }
        }
        for arena in self.retiredArenas.drain(..) {
            unsafe {
                drop(Box::from_raw(arena));
            }
        }
    }
}

/// 跳表最大层高。
pub const maxHeight: usize = 16;
/// 节点固定头部字节数。
pub const nodeHeaderSize: usize = mem::size_of::<nodeHeader>();

// nodeHeader 对应 Go 中写入 arena 字节块开头的固定头部。
#[repr(C)]
/// 写入 arena 块开头的固定节点头。
pub struct nodeHeader {
    pub addr: arenaAddr,
    pub height: u16,
    pub keyLen: u16,
    pub valLen: u32,
}

// node 对应 Go 的变长节点。nextsBase 是 nexts 切片的第一个元素，后续元素通过指针运算访问。
#[repr(C)]
/// 变长跳表节点：头 + nexts 数组 + key/value。
pub struct node {
    pub nodeHeader: nodeHeader,
    // Height of the nexts.

    // node is a variable length struct.
    // The nextsBase is the first element of nexts slice,
    // it act as the base pointer we do pointer arithmetic in `next` and `setNext`.
    pub nextsBase: u64,
}

// entry 对应 Go 中嵌入 *node 的轻量返回值，同时缓存 key 切片以减少重复解析。
/// 查找返回值：节点指针与缓存的 key。
pub struct entry {
    pub node: *mut node,
    pub key: Vec<u8>,
}

impl entry {
    // getValue 对应 Go 的 entry.getValue；空 entry 返回 nil，这里用空 Vec 表达。
    /// 读取节点 value；空 entry 返回空 Vec。
    pub fn getValue(&self, arena: &arena) -> Vec<u8> {
        if !self.node.is_null() {
            unsafe {
                return (*self.node).getValue(arena);
            }
        }
        Vec::new()
    }

    // getNextAddr 保留 Go 嵌入 *node 后直接调用 getNextAddr 的语义。
    /// 读取指定层 next 地址。
    /// 原子加载指定层 next 地址。
    pub fn getNextAddr(&self, level: usize) -> arenaAddr {
        unsafe { (*self.node).getNextAddr(level) }
    }
}

impl node {
    // setNexts 对应 Go 的直接写 nextsBase 扩展区，用于新节点尚未发布前的初始化。
    /// 非原子写入 nexts（仅用于新节点发布前初始化）。
    pub fn setNexts(&mut self, level: usize, val: u64) {
        unsafe {
            *self.nextsAddr(level) = val;
        }
    }

    // nextsAddr 对应 Go 的 unsafe 指针运算，从 nextsBase 起按 uint64 步长取第 idx 个 next。
    /// 计算第 idx 个 next 槽的指针。
    pub unsafe fn nextsAddr(&self, idx: usize) -> *mut u64 {
        unsafe {
            let offset = idx * mem::size_of_val(&self.nextsBase);
            (&self.nextsBase as *const u64 as *mut u8).add(offset) as *mut u64
        }
    }

    // getNextAddr 对应 Go 的 atomic.LoadUint64，读者通过原子加载观察 next 指针。
    /// 原子加载指定层 next 地址。
    pub fn getNextAddr(&self, level: usize) -> arenaAddr {
        let next =
            unsafe { AtomicU64::from_ptr(self.nextsAddr(level)).load(AtomicOrdering::SeqCst) };
        arenaAddr(next)
    }

    // setNextAddr 对应 Go 的 atomic.StoreUint64，写入时按 level 发布新的后继地址。
    /// 原子发布指定层 next 地址。
    pub fn setNextAddr(&mut self, level: usize, addr: arenaAddr) {
        unsafe {
            AtomicU64::from_ptr(self.nextsAddr(level)).store(addr.0, AtomicOrdering::SeqCst);
        }
    }

    // nodeLen 对应 Go 的 node.nodeLen，包含可变高度 next 指针数组和固定头部。
    /// 头部 + nexts 数组占用字节数。
    pub fn nodeLen(&self) -> usize {
        self.nodeHeader.height as usize * 8 + nodeHeaderSize
    }

    // getKey 对应 Go 的 node.getKey，从 arena 中取出 header+nexts 后面的 key 区域。
    /// 从 arena 取出 key 区域。
    pub fn getKey(&self, a: &arena) -> Vec<u8> {
        let nodeLen = self.nodeLen();
        let entryData = a.get(
            self.nodeHeader.addr,
            nodeLen + self.nodeHeader.keyLen as usize,
        );
        entryData[nodeLen..].to_vec()
    }

    // getValue 对应 Go 的 node.getValue，从 key 之后截取 value 区域。
    /// 从 arena 取出 value 区域。
    pub fn getValue(&self, a: &arena) -> Vec<u8> {
        let nodeLenKeyLen = self.nodeLen() + self.nodeHeader.keyLen as usize;
        let entryData = a.get(
            self.nodeHeader.addr,
            nodeLenKeyLen + self.nodeHeader.valLen as usize,
        );
        entryData[nodeLenKeyLen..].to_vec()
    }

    // getNextNode 对应 Go 的 node.getNextNode，按 next 地址在 arena 中反解下一节点指针。
    /// 按 next 地址反解下一节点指针。
    pub fn getNextNode(&self, arena: &arena, level: usize) -> *mut node {
        let addr = self.getNextAddr(level);
        if addr == nullArenaAddr {
            return ptr::null_mut();
        }
        let data = arena.get(addr, nodeHeaderSize);
        data.as_ptr() as *mut node
    }
}

impl MemStore {
    // NewMemStore returns a new mem store.
    // NewMemStore 对应 Go 构造函数：创建 arena、随机源并放置 maxHeight 的头节点。
    /// 构造 MemStore：创建 arena、随机源与最大高度头节点。
    pub fn NewMemStore(arenaBlockSize: usize) -> Box<MemStore> {
        let arena = Box::new(newArenaLocator(arenaBlockSize));
        let mut ls = Box::new(MemStore {
            height: AtomicI32::new(1),
            arenaPtr: AtomicPtr::new(Box::into_raw(arena)),
            retiredArenas: Vec::new(),
            rand: rand::rngs::StdRng::seed_from_u64(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
            ),
            length: 0,
            head: ptr::null_mut(),
        });
        ls.setHeadNode();
        ls
    }

    // setHeadNode 对应 Go 的初始化头节点逻辑，头节点拥有最大高度且所有 next 初始化为空地址。
    /// 初始化哨兵头节点。
    pub fn setHeadNode(&mut self) {
        let n = self.newNode(&[], &[], maxHeight);
        for i in 0..maxHeight {
            unsafe {
                (*n).setNexts(i, 0);
            }
        }
        self.head = n;
    }

    // getHeight 对应 Go 的 atomic.LoadInt32。
    /// 原子读取当前跳表高度。
    pub fn getHeight(&self) -> usize {
        self.height.load(AtomicOrdering::SeqCst) as usize
    }

    // setHeight 对应 Go 的 atomic.StoreInt32。
    /// 原子写入当前跳表高度。
    pub fn setHeight(&self, height: usize) {
        self.height.store(height as i32, AtomicOrdering::SeqCst);
    }

    // Get gets a value with the key.
    // Get 对应 Go 的查找并拷贝 value；未命中时返回 None 表达 nil。
    /// 按 key 查找并拷贝 value；未命中返回 None。
    pub fn Get(&self, key: &[u8], buf: &mut Vec<u8>) -> Option<Vec<u8>> {
        let (e, matched) = self.findGreater(key, true);
        if !matched {
            return None;
        }
        let value = e.getValue(self.getArena());
        buf.clear();
        buf.extend_from_slice(&value);
        Some(buf.clone())
    }

    // getNext 对应 Go 的 MemStore.getNext，返回 entry 并附带解析出的 key。
    /// 取节点在指定层的后继 entry。
    pub fn getNext(&self, n: *mut node, level: usize) -> entry {
        let addr = unsafe { (*n).getNextAddr(level) };
        if addr == nullArenaAddr {
            return entry {
                node: ptr::null_mut(),
                key: Vec::new(),
            };
        }
        let arena = self.getArena();
        let data = arena.get(addr, nodeHeaderSize);
        let node = data.as_ptr() as *mut node;
        let key = unsafe { (*node).getKey(arena) };
        entry { node, key }
    }

    // findGreater 对应 Go 的查找 >= 或 > key 的路径搜索。
    /// 查找 >= 或 > key 的节点。
    pub fn findGreater(&self, key: &[u8], allowEqual: bool) -> (entry, bool) {
        let mut prev = entry {
            node: self.head,
            key: Vec::new(),
        };
        let mut level = self.getHeight() - 1;
        loop {
            let mut next = entry {
                node: ptr::null_mut(),
                key: Vec::new(),
            };
            let addr = prev.getNextAddr(level);
            if addr != nullArenaAddr {
                let arena = self.getArena();
                let data = arena.get(addr, nodeHeaderSize);
                next.node = data.as_ptr() as *mut node;
                next.key = unsafe { (*next.node).getKey(arena) };
                match next.key.as_slice().cmp(key) {
                    // next 仍小于 key，继续右移。
                    Ordering::Less => {
                        // next key is still smaller, keep moving.
                        prev = next;
                        continue;
                    }
                    // 命中相等键：允许相等则返回，否则降到底层继续。
                    Ordering::Equal => {
                        // prev.key < key == next.key.
                        if allowEqual {
                            return (next, true);
                        }
                        level = 0;
                        prev = next;
                        continue;
                    }
                    Ordering::Greater => {}
                }
            }
            // next is greater than key or next is nil. go to the lower level.
            if level > 0 {
                level -= 1;
                continue;
            }
            return (next, false);
        }
    }

    // findLess 对应 Go 的查找 <= 或 < key 的路径搜索，永远不会返回 head。
    /// 查找 <= 或 < key 的节点（永不返回 head）。
    pub fn findLess(&self, key: &[u8], allowEqual: bool) -> (entry, bool) {
        let mut prev = entry {
            node: self.head,
            key: Vec::new(),
        };
        let mut level = self.getHeight() - 1;
        loop {
            let next = self.getNext(prev.node, level);
            if !next.node.is_null() {
                match key.cmp(next.key.as_slice()) {
                    Ordering::Greater => {
                        // prev.key < next.key < key. We can continue to move right.
                        prev = next;
                        continue;
                    }
                    Ordering::Equal if allowEqual => {
                        // prev.key < key == next.key.
                        return (next, true);
                    }
                    _ => {}
                }
            }
            // get closer to the key in the lower level.
            if level > 0 {
                level -= 1;
                continue;
            }
            break;
        }
        // We are not going to return head.
        if prev.node == self.head {
            return (
                entry {
                    node: ptr::null_mut(),
                    key: Vec::new(),
                },
                false,
            );
        }
        (prev, false)
    }

    // findSpliceForLevel returns (outBefore, outAfter) with outBefore.key < key <= outAfter.key.
    // The input "before" tells us where to start looking.
    // If we found a node with the same key, then we return true.
    // findSpliceForLevel 对应 Go 的单层 splice 查找，是 Put/Delete hint 复用的核心。
    /// 单层 splice 查找：返回 before/after 及是否键相等。
    pub fn findSpliceForLevel(
        &self,
        arena: &arena,
        key: &[u8],
        mut before: *mut node,
        level: usize,
    ) -> (*mut node, *mut node, bool) {
        loop {
            // Assume before.key < key.
            let nextAddr = unsafe { (*before).getNextAddr(level) };
            if nextAddr == nullArenaAddr {
                return (before, ptr::null_mut(), false);
            }
            let data = arena.get(nextAddr, nodeHeaderSize);
            let next = data.as_ptr() as *mut node;
            let nextKey = unsafe { (*next).getKey(arena) };
            let cmp = nextKey.as_slice().cmp(key);
            if cmp != Ordering::Less {
                // before.key < key < next.key. We are done for this level.
                return (before, next, cmp == Ordering::Equal);
            }
            before = next; // Keep moving right on this level.
        }
    }

    // findLast returns the last element. If head (empty ls), we return nil. All the find functions
    // will NEVER return the head nodes.
    // findLast 对应 Go 的从最高层一路向右再下降的尾节点查找。
    /// 查找最大键节点；空表返回空 entry。
    pub fn findLast(&self) -> entry {
        let mut e = entry {
            node: self.head,
            key: Vec::new(),
        };
        let mut level = self.getHeight() - 1;
        loop {
            let next = self.getNext(e.node, level);
            if !next.node.is_null() {
                e = next;
                continue;
            }
            if level == 0 {
                if e.node == self.head {
                    return entry {
                        node: ptr::null_mut(),
                        key: Vec::new(),
                    };
                }
                return e;
            }
            level -= 1;
        }
    }

    // getNode 对应 Go 的按 arenaAddr 反解 node 指针。
    /// 按 arenaAddr 反解 node 指针。
    pub fn getNode(&self, arena: &arena, addr: arenaAddr) -> *mut node {
        let data = arena.get(addr, nodeHeaderSize);
        data.as_ptr() as *mut node
    }

    // Put puts the key-value pair, returns true if the key doesn't exist.
    /// 插入或替换；新键返回 true。
    pub fn Put(&mut self, key: &[u8], v: &[u8]) -> bool {
        self.PutWithHint(key, v, None)
    }

    // PutWithHint puts the key-value pair, returns true if the key doesn't exist.
    // PutWithHint 对应 Go 的插入/替换入口，hint 用于跳过仍有效的低层 splice 查找。
    /// 带 hint 的插入/替换，复用有效低层 splice。
    pub fn PutWithHint(&mut self, key: &[u8], v: &[u8], hint: Option<&mut Hint>) -> bool {
        let arena = self.getArena();
        let lsHeight = self.getHeight();
        let mut localHint;
        let hint = match hint {
            Some(hint) => hint,
            None => {
                localHint = Hint::new();
                &mut localHint
            }
        };
        let recomputeHeight = self.calculateRecomputeHeight(key, hint, lsHeight);
        let mut old: *mut node = ptr::null_mut();
        if recomputeHeight > 0 {
            for i in (0..recomputeHeight).rev() {
                // Use higher level to speed up for current level.
                let (prev, next, exists) = self.findSpliceForLevel(arena, key, hint.prev[i + 1], i);
                hint.prev[i] = prev;
                hint.next[i] = next;
                if exists {
                    old = hint.next[i];
                }
            }
        } else if !hint.next[0].is_null()
            && unsafe { (*hint.next[0]).getKey(arena) }.as_slice() == key
        {
            old = hint.next[0];
        }

        if !old.is_null() {
            self.replace(key, v, hint, old);
            return false;
        }
        let height = self.randomHeight();
        let x = self.newNode(key, v, height);
        if height > lsHeight {
            self.setHeight(height);
        }

        // 自底层向上插入：底层链好后高层才能安全发现该节点。
        // We always insert from the base level and up. After you add a node in base level, we cannot
        // create a node in the level above because it would have discovered the node in the base level.
        for i in 0..height {
            unsafe {
                if !hint.next[i].is_null() {
                    (*x).setNexts(i, (*hint.next[i]).nodeHeader.addr.0);
                } else {
                    (*x).setNexts(i, nullArenaAddr.0);
                }
                if hint.prev[i].is_null() {
                    hint.prev[i] = self.head;
                }
                (*hint.prev[i]).setNextAddr(i, (*x).nodeHeader.addr);
                hint.prev[i] = x;
            }
        }
        self.length += 1;
        true
    }

    // replace 对应 Go 的原地替换：新节点继承旧节点 nexts，再让前驱指向新节点，最后释放旧 arena 块引用。
    /// 原地替换：新节点继承 nexts，前驱改指向后释放旧块。
    pub fn replace(&mut self, key: &[u8], v: &[u8], hint: &mut Hint, old: *mut node) {
        let x = self.newNode(key, v, unsafe { (*old).nodeHeader.height as usize });
        let arena = self.getArena();
        for i in 0..unsafe { (*old).nodeHeader.height as usize } {
            let nextAddr =
                unsafe { AtomicU64::from_ptr((*old).nextsAddr(i)).load(AtomicOrdering::SeqCst) };
            unsafe {
                (*x).setNexts(i, nextAddr);
            }
            if nextAddr != nullArenaAddr.0 {
                hint.next[i] = self.getNode(arena, arenaAddr(nextAddr));
            } else {
                hint.next[i] = ptr::null_mut();
            }
            unsafe {
                (*hint.prev[i]).setNextAddr(i, (*x).nodeHeader.addr);
            }
            hint.prev[i] = x;
        }
        self.getArenaMut().free(unsafe { (*old).nodeHeader.addr });
    }

    // MaxEntrySize will return the maximum entry size for the MemStore
    // Any entry larger than this will likely result in failure.
    /// 单条目最大可分配字节数估计。
    pub fn MaxEntrySize(&self) -> usize {
        self.getArena().blockSize - nodeHeaderSize - self.getHeight() * 8
    }

    // newNode 对应 Go 的 arena 分配与字节布局写入逻辑，返回指向 arena 内部的 node。
    /// 在 arena 中分配并布局新节点。
    pub fn newNode(&mut self, key: &[u8], v: &[u8], height: usize) -> *mut node {
        // The base level is already allocated in the node struct.
        let nodeSize = nodeHeaderSize + height * 8 + key.len() + v.len();
        let mut addr = self.getArenaMut().alloc(nodeSize);
        // arena 不足时复制 locator、共享旧 block，再原子发布新 locator。
        if addr == nullArenaAddr {
            let grown = self.getArena().grow();
            self.setArena(Box::new(grown));
            // The new arena block must have enough memory to alloc.
            addr = self.getArenaMut().alloc(nodeSize);
        }
        let data = self.getArenaMut().get_mut(addr, nodeSize);
        let node = data.as_mut_ptr() as *mut node;
        unsafe {
            (*node).nodeHeader.addr = addr;
            (*node).nodeHeader.keyLen = key.len() as u16;
            (*node).nodeHeader.height = height as u16;
            (*node).nodeHeader.valLen = v.len() as u32;
            let nodeLen = (*node).nodeLen();
            data[nodeLen..nodeLen + key.len()].copy_from_slice(key);
            data[nodeLen + key.len()..nodeLen + key.len() + v.len()].copy_from_slice(v);
        }
        node
    }

    // getArena 对应 Go 的 atomic.LoadPointer，将 arenaPtr 重新解释为 arena 引用。
    /// 原子加载 arena 只读引用。
    pub fn getArena(&self) -> &arena {
        unsafe { &*self.arenaPtr.load(AtomicOrdering::SeqCst) }
    }

    // getArenaMut 是为写路径补出的可变引用，对应 Go 单写者直接修改 arena 的假设。
    /// 写路径可变 arena 引用。
    pub fn getArenaMut(&mut self) -> &mut arena {
        unsafe { &mut *self.arenaPtr.load(AtomicOrdering::SeqCst) }
    }

    // setArena 对应 Go 的 atomic.StorePointer，发布 grow 后的新 arena 定位器。
    /// 原子发布新的 arena 定位器。
    pub fn setArena(&mut self, al: Box<arena>) {
        let old = self
            .arenaPtr
            .swap(Box::into_raw(al), AtomicOrdering::SeqCst);
        if !old.is_null() {
            // Readers may still hold the old locator; reclaim it only when the store is dropped.
            self.retiredArenas.push(old);
        }
    }

    // randomHeight 对应 Go 的几何分布高度生成，每次大约用 2 bit 随机性。
    /// 几何分布随机层高。
    pub fn randomHeight(&mut self) -> usize {
        let mut h = 1;
        while h < maxHeight && self.rand.next_u64() < u64::MAX / 4 {
            h += 1;
        }
        h
    }

    // calculateRecomputeHeight 对应 Go 的 hint 有效性检查，返回需要自顶向下重算的高度。
    /// 检查 hint 有效性，返回需自顶向下重算的高度。
    pub fn calculateRecomputeHeight(
        &self,
        key: &[u8],
        hint: &mut Hint,
        listHeight: usize,
    ) -> usize {
        let mut recomputeHeight = 0;
        let arena = self.getArena();
        if hint.height < listHeight as i32 {
            // Either splice is never used or list height has grown, we recompute all.
            hint.prev[listHeight] = self.head;
            hint.next[listHeight] = ptr::null_mut();
            hint.height = listHeight as i32;
            recomputeHeight = listHeight;
        } else {
            while recomputeHeight < listHeight {
                let prevNode = hint.prev[recomputeHeight];
                let nextNode = hint.next[recomputeHeight];
                let prevNext = unsafe { (*prevNode).getNextNode(arena, recomputeHeight) };
                if prevNext != nextNode {
                    recomputeHeight += 1;
                    continue;
                }
                let keyBeforePrev = prevNode != self.head
                    && !prevNode.is_null()
                    && key <= unsafe { (*prevNode).getKey(arena) }.as_slice();
                if keyBeforePrev {
                    while prevNode == hint.prev[recomputeHeight] {
                        recomputeHeight += 1;
                    }
                    continue;
                }
                let keyAfterNext =
                    !nextNode.is_null() && key > unsafe { (*nextNode).getKey(arena) }.as_slice();
                if keyAfterNext {
                    while nextNode == hint.next[recomputeHeight] {
                        recomputeHeight += 1;
                    }
                    continue;
                }
                break;
            }
        }
        recomputeHeight
    }

    // DeleteWithHint deletes a value with the key and hint.
    // DeleteWithHint 对应 Go 的删除逻辑：先用 hint 找到目标，再从高层到低层改前驱 next。
    /// 带 hint 删除；成功返回 true。
    pub fn DeleteWithHint(&mut self, key: &[u8], hint: Option<&mut Hint>) -> bool {
        let listHeight = self.getHeight();
        let mut localHint;
        let hint = match hint {
            Some(hint) => hint,
            None => {
                localHint = Hint::new();
                &mut localHint
            }
        };
        let recomputeHeight = self.calculateRecomputeHeight(key, hint, listHeight);
        let arena = self.getArena();
        let mut keyNode: *mut node = ptr::null_mut();
        if recomputeHeight > 0 {
            for i in (0..recomputeHeight).rev() {
                // Use higher level to speed up for current level.
                let (prev, next, matched) =
                    self.findSpliceForLevel(arena, key, hint.prev[i + 1], i);
                hint.prev[i] = prev;
                hint.next[i] = next;
                if matched {
                    keyNode = hint.next[i];
                }
            }
        } else if !hint.next[0].is_null()
            && unsafe { (*hint.next[0]).getKey(arena) }.as_slice() == key
        {
            keyNode = hint.next[0];
        }
        if keyNode.is_null() {
            return false;
        }
        for i in (0..unsafe { (*keyNode).nodeHeader.height as usize }).rev() {
            // Change the nexts from higher to lower, so the data is consistent at any point.
            let addr = unsafe { (*keyNode).getNextAddr(i) };
            if addr != nullArenaAddr {
                hint.next[i] = self.getNode(arena, addr);
            } else {
                hint.next[i] = ptr::null_mut();
            }
            unsafe {
                (*hint.prev[i]).setNextAddr(i, (*keyNode).getNextAddr(i));
            }
        }
        self.getArenaMut()
            .free(unsafe { (*keyNode).nodeHeader.addr });
        self.length -= 1;
        true
    }

    // Delete deletes a value with the key.
    /// 删除指定 key。
    pub fn Delete(&mut self, key: &[u8]) -> bool {
        self.DeleteWithHint(key, None)
    }

    // Len returns the length of a mem store.
    /// 当前条目数。
    pub fn Len(&self) -> usize {
        self.length
    }
}

// Hint represents a hint.
// Hint 对应 Go 的插入/删除位置缓存；数组长度多一格用于 listHeight 哨兵。
/// 插入/删除位置缓存；多一格用于 listHeight 哨兵。
pub struct Hint {
    pub height: i32,
    pub prev: [*mut node; maxHeight + 1],
    pub next: [*mut node; maxHeight + 1],
}

impl Hint {
    // new 对应 Go 的 new(Hint)，所有指针初始为空，height 初始为 0。
    /// 构造空 hint。
    pub fn new() -> Hint {
        Hint {
            height: 0,
            prev: [ptr::null_mut(); maxHeight + 1],
            next: [ptr::null_mut(); maxHeight + 1],
        }
    }
}
