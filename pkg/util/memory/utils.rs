// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 内存子系统辅助工具：可复用链表、合并唤醒通知器、哈希分片与运行时内存采样。
//
// 对应 Go `pkg/util/memory/utils.go`。`wrapList` 用 slot 池避免频繁分配；
// `Notifer` 对多生产者唤醒做合并（coalesce），减少无意义的 channel 发送；
// 哈希/配额分片/比例换算供 Tracker 等配额管理使用；`SampleRuntimeMemStats`
// 将进程驻留内存映射为与 Go runtime 相近的统计视图。

use crossbeam_utils::CachePadded;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 字节单位：1 字节。
const byteSize: i64 = 1;
/// 字节单位：1 KiB。
const byteSizeKB: i64 = 1 << 10;
/// 字节单位：1 MiB。
const byteSizeMB: i64 = 1 << 20;
/// 字节单位：1 GiB。
const byteSizeGB: i64 = 1 << 30;
/// 千分比换算基数（比例以千分之一为单位）。
const kilo: i64 = 1000;

/// List with a cache to avoid allocating and deallocating elements repeatedly.
/// 带空闲槽缓存的链表：复用 slot，避免反复分配/释放节点。
pub(crate) struct wrapList<V> {
    /// 按槽位存放的元素；`None` 表示空闲。
    slots: Vec<Option<V>>,
    /// 活跃元素槽位的双端队列（保持插入/移动顺序）。
    active: VecDeque<usize>,
    /// 可复用的空闲槽位下标。
    free: Vec<usize>,
    /// 当前活跃元素个数。
    num: i64,
}

/// `wrapList` 中元素的句柄，持有槽位下标。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct wrapListElement {
    base: Option<usize>,
}

impl wrapListElement {
    /// 句柄是否仍指向有效槽位。
    pub(crate) fn valid(&self) -> bool {
        self.base.is_some()
    }

    /// 清空句柄，使其失效。
    pub(crate) fn reset(&mut self) {
        self.base = None;
    }

    /// 返回槽位下标；无效句柄会 panic。
    pub(crate) fn slot(&self) -> usize {
        self.base.expect("invalid wrapList element")
    }
}

impl<V> Default for wrapList<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> wrapList<V> {
    /// 创建空的可复用链表。
    pub(crate) fn new() -> Self {
        Self {
            slots: Vec::new(),
            active: VecDeque::new(),
            free: Vec::new(),
            num: 0,
        }
    }

    /// 清空全部槽位与计数，回到初始状态。
    pub(crate) fn init(&mut self) {
        self.slots.clear();
        self.active.clear();
        self.free.clear();
        self.num = 0;
    }

    /// 将已存在元素移到活跃队列前端（LRU 风格触达）。
    pub(crate) fn moveToFront(&mut self, e: wrapListElement) {
        let slot = e.slot();
        if let Some(position) = self.active.iter().position(|candidate| *candidate == slot) {
            self.active.remove(position);
            self.active.push_front(slot);
        }
    }

    /// 调整活跃元素计数。
    fn doAddNum(&mut self, x: i64) {
        self.num += x;
    }

    /// 从活跃队列移除元素，槽位回收到 free 列表。
    pub(crate) fn remove(&mut self, e: wrapListElement) {
        let slot = e.slot();
        let Some(position) = self.active.iter().position(|candidate| *candidate == slot) else {
            return;
        };
        self.active.remove(position);
        self.slots[slot] = None;
        self.free.push(slot);
        self.doAddNum(-1);
    }

    /// 查看队首元素（不弹出）。
    pub(crate) fn front(&self) -> Option<&V> {
        self.active
            .front()
            .and_then(|slot| self.slots[*slot].as_ref())
    }

    /// 弹出队首元素并回收槽位。
    pub(crate) fn popFront(&mut self) -> Option<V> {
        let slot = self.active.pop_front()?;
        let value = self.slots[slot].take();
        self.free.push(slot);
        self.doAddNum(-1);
        value
    }

    /// 当前活跃元素个数。
    pub(crate) fn size(&self) -> i64 {
        self.num
    }

    /// 是否无活跃元素。
    pub(crate) fn empty(&self) -> bool {
        self.size() == 0
    }

    /// 近似大小（当前等于精确 size）。
    pub(crate) fn approxSize(&self) -> i64 {
        self.size()
    }

    /// 近似是否为空。
    pub(crate) fn approxEmpty(&self) -> bool {
        self.empty()
    }

    /// 追加到队尾；优先复用 free 槽，否则扩容 slots。
    pub(crate) fn pushBack(&mut self, value: V) -> wrapListElement {
        // 优先从 free 取槽，避免 slots 持续增长。
        let slot = if let Some(slot) = self.free.pop() {
            self.slots[slot] = Some(value);
            slot
        } else {
            self.slots.push(Some(value));
            self.slots.len() - 1
        };
        self.active.push_back(slot);
        self.doAddNum(1);
        wrapListElement { base: Some(slot) }
    }

    /// 已分配槽位总数（含空闲槽），用于测试复用行为。
    pub(crate) fn allocated_len(&self) -> usize {
        self.slots.len()
    }
}

/// Multiple-producer, single-consumer notification with coalesced wakeups.
/// 多生产者单消费者通知器：合并连续唤醒，避免 channel 堆积。
pub struct Notifer {
    /// 容量为 1 的同步发送端；已 awake 时不再重复 send。
    pub C: SyncSender<()>,
    /// 接收端加锁，保证 Wait 串行消费。
    receiver: Mutex<Receiver<()>>,
    /// 是否已有未消费的唤醒（0/1）。
    awake: AtomicI32,
}

/// 构造容量为 1 的通知器。
pub fn NewNotifer() -> Notifer {
    let (sender, receiver) = sync_channel(1);
    Notifer {
        C: sender,
        receiver: Mutex::new(receiver),
        awake: AtomicI32::new(0),
    }
}

impl Notifer {
    /// 清除 awake 标志，返回清除前是否曾唤醒。
    fn clear(&self) -> bool {
        self.awake.swap(0, Ordering::SeqCst) != 0
    }

    /// 阻塞等待一次唤醒并清除 awake。
    pub fn Wait(&self) {
        self.receiver
            .lock()
            .expect("notifier receiver lock poisoned")
            .recv()
            .expect("notifier sender disconnected");
        self.clear();
    }

    /// 强制唤醒（若尚未 awake 则 send）。
    pub fn Wake(&self) {
        self.wake();
    }

    /// 仅在 awake 从 0→1 时向 channel 发送，实现唤醒合并。
    fn wake(&self) {
        if self.awake.swap(1, Ordering::SeqCst) == 0 {
            self.C.send(()).expect("notifier receiver disconnected");
        }
    }

    /// 当前是否处于已唤醒待消费状态。
    pub(crate) fn isAwake(&self) -> bool {
        self.awake.load(Ordering::SeqCst) != 0
    }

    /// 弱唤醒：仅在尚未 awake 时调用 wake。
    pub fn WeakWake(&self) {
        if !self.isAwake() {
            self.wake();
        }
    }
}

/// 对字符串做与 Go 一致的 64 位乘法哈希。
pub fn HashStr(key: &str) -> u64 {
    let mut hashKey = initHashKey;
    for c in key.chars() {
        hashKey = hashKey.wrapping_mul(prime64);
        hashKey ^= c as u64;
    }
    hashKey
}

/// 对偶数字键做分片友好的哈希（先混低 8 位再混高位）。
pub fn HashEvenNum(mut key: u64) -> u64 {
    const STEP: u32 = 8;
    const STEP_MASK: u64 = (1_u64 << STEP) - 1;
    let mut hashKey = initHashKey;
    hashKey ^= key & STEP_MASK;
    hashKey = hashKey.wrapping_mul(prime64);
    key >>= STEP;
    hashKey ^= key;
    hashKey.wrapping_mul(prime64)
}

/// 由 UID 计算分片下标：`HashEvenNum(key) & shardsMask`。
pub(crate) fn shardIndexByUID(key: u64, shardsMask: u64) -> u64 {
    HashEvenNum(key) & shardsMask
}

/// 按配额相对 `baseQuotaUnit` 的对数桶选取分片，并钳制到 `[0, maxQuotaShard)`。
pub(crate) fn getQuotaShard(quota: i64, maxQuotaShard: i32) -> i32 {
    let p = quota as u64 / baseQuotaUnit as u64;
    let pos = (u64::BITS - p.leading_zeros()) as i32;
    pos.min(maxQuotaShard - 1)
}

/// 当前 Unix 时间（毫秒）。
pub(crate) fn nowUnixMilli() -> i64 {
    now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// 当前 Unix 时间（秒）。
pub(crate) fn nowUnixSec() -> i64 {
    now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// 封装 `SystemTime::now`，便于测试替换。
fn now() -> SystemTime {
    SystemTime::now()
}

/// 返回不小于 n 的最小 2 的幂；n=0 时返回 1。
pub(crate) fn nextPow2(mut n: u64) -> u64 {
    if n == 0 {
        return 1;
    }
    n -= 1;
    n |= n >> 1;
    n |= n >> 2;
    n |= n >> 4;
    n |= n >> 8;
    n |= n >> 16;
    n |= n >> 32;
    n.wrapping_add(1)
}

/// 计算千分比：`x * 1000 / y`。
pub(crate) fn calcRatio(x: i64, y: i64) -> i64 {
    x.wrapping_mul(kilo) / y
}

/// 用千分比缩放：`x * yMilli / 1000`。
pub(crate) fn multiRatio(x: i64, yMilli: i64) -> i64 {
    x.wrapping_mul(yMilli) / kilo
}

/// 将浮点比例转为千分比整数。
pub(crate) fn intoRatio(x: f64) -> i64 {
    (x * kilo as f64) as i64
}

/// CPU 缓存行填充别名，减少伪共享。
type cpuCacheLinePad<T = ()> = CachePadded<T>;

/// 与 Go runtime 内存统计字段对齐的采样结果。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeMemStats {
    /// 已分配堆字节（Rust 侧用驻留内存近似）。
    pub HeapAlloc: u64,
    /// 在用堆字节。
    pub HeapInuse: u64,
    /// 已释放总量（Rust 无 GC 计数时恒为 0）。
    pub TotalFree: u64,
    /// 堆外/虚拟减驻留的近似。
    pub MemOffHeap: u64,
    /// GC 次数（Rust 侧通常为 0）。
    pub NumGC: u64,
}

/// Rust 侧原始内存统计字段，供转换为 `RuntimeMemStats`。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RustMemStats {
    pub HeapAlloc: u64,
    pub HeapInuse: u64,
    pub TotalAlloc: u64,
    pub Alloc: u64,
    pub Sys: u64,
    pub HeapSys: u64,
    pub NumGC: u32,
}

/// GC 时间跟踪状态（预留，对齐 Go gcTracker）。
struct gcTrackerState {
    lastGCTime: AtomicI64,
    lastNumGC: AtomicU64,
}

static gcTracker: gcTrackerState = gcTrackerState {
    lastGCTime: AtomicI64::new(0),
    lastNumGC: AtomicU64::new(0),
};

/// 读取最近一次近似 GC 时间戳。
fn approxLastGCTime() -> i64 {
    gcTracker.lastGCTime.load(Ordering::SeqCst)
}

/// Samples the current Rust process. Rust has no runtime GC counters, so NumGC
/// and TotalFree remain zero; resident memory is the closest portable heap view.
/// 采样当前进程内存：用物理驻留近似 HeapAlloc/HeapInuse，虚拟减驻留为 MemOffHeap。
pub fn SampleRuntimeMemStats() -> RuntimeMemStats {
    let Some(sample) = memory_stats::memory_stats() else {
        return RuntimeMemStats::default();
    };
    let resident = sample.physical_mem as u64;
    let virtual_mem = sample.virtual_mem as u64;
    RuntimeMemStats {
        HeapAlloc: resident,
        HeapInuse: resident,
        TotalFree: 0,
        MemOffHeap: virtual_mem.saturating_sub(resident),
        NumGC: 0,
    }
}

/// 将 `RustMemStats` 映射为与 Go 字段语义一致的 `RuntimeMemStats`。
pub fn IntoRuntimeMemStats(s: &RustMemStats) -> RuntimeMemStats {
    RuntimeMemStats {
        HeapAlloc: s.HeapAlloc,
        HeapInuse: s.HeapInuse,
        TotalFree: s.TotalAlloc.wrapping_sub(s.Alloc),
        MemOffHeap: s.Sys.wrapping_sub(s.HeapSys),
        NumGC: s.NumGC as u64,
    }
}
