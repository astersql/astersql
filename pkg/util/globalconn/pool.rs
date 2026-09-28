// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 连接/本地 ID 分配池：自增池 `AutoIncPool` 与无锁环形池 `LockFreeCircularPool`。
//
// 供全局连接 ID 分配器复用本地号段。自增池可选用去重集合；无锁池仅支持 32 位 ID，
// 用 head/tail + slot.seq 实现无锁 FIFO，空/满判定保留一个空槽位。

use std::collections::HashSet;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::thread;

/// 池为空时 Get 返回的无效哨兵值（对齐 Go `math.MaxUint64`）。
// IDPoolInvalidValue indicates invalid value from IDPool.
// IDPoolInvalidValue 对应 Go 的 math.MaxUint64，用于无锁池取空时返回的无效 ID。
pub const IDPoolInvalidValue: u64 = u64::MAX;

/// ID 池接口：初始化、容量/长度、Put/Get；同时要求 `Display`（对齐 `fmt.Stringer`）。
// IDPool is the pool allocating & deallocating IDs.
// IDPool trait 机械对应 Go interface：同时要求 Display 来表达 fmt.Stringer。
pub trait IDPool: fmt::Display {
    /// 按给定容量初始化池。
    // Init initiates pool.
    fn Init(&mut self, size: u64);
    /// 当前可用/已占用长度；不支持时返回 -1。
    // Len returns length of available id's in pool.
    // Note that Len() would return -1 when this method is NOT supported.
    fn Len(&self) -> i32;
    /// 池容量。
    // Cap returns the capacity of pool.
    fn Cap(&self) -> i32;
    /// 归还 ID；池满时返回 false。
    // Put puts value to pool. "ok" is false when pool is full.
    fn Put(&self, val: u64) -> bool;
    /// 取出 ID；池空时返回 false（无锁池同时带回 Invalid 哨兵）。
    // Get gets value from pool. "ok" is false when pool is empty.
    fn Get(&self) -> (u64, bool);
}

/// 自增分配池：原子递增 `lastID`，可选对已发放 ID 去重，支持回绕。
// AutoIncPool simply do auto-increment to allocate ID. Wrapping will happen.
// AutoIncPool 保留 Go 的自增 ID 分配策略；lastID 用原子值表达 sync/atomic.AddUint64。
pub struct AutoIncPool {
    /// 最近一次发放的 ID（原子自增）。
    lastID: AtomicU64,
    /// 模运算容量；小于 `u64::MAX` 时对 ID 取模回绕。
    cap: u64,
    /// 冲突时最大尝试次数。
    tryCnt: usize,

    // existed 为 Some 时对应 Go 里的 map+Mutex 去重模式；Mutex guard 离开作用域即对应 Unlock。
    /// 可选的“已占用”集合；启用时 Put/Get 需加锁维护。
    existed: Option<Mutex<HashSet<u64>>>,
}

impl Default for AutoIncPool {
    fn default() -> Self {
        Self {
            lastID: AtomicU64::new(0),
            cap: 0,
            tryCnt: 0,
            existed: None,
        }
    }
}

impl AutoIncPool {
    /// 默认初始化：不去重，尝试次数为 1。
    // Init initiates AutoIncPool.
    // Init 保留 Go 默认参数：不检查已存在 ID，尝试次数为 1。
    pub fn Init(&mut self, size: u64) {
        self.InitExt(size, false, 1);
    }

    /// 扩展初始化：可开启去重集合与自定义尝试次数。
    // InitExt initiates AutoIncPool with more parameters.
    // InitExt 对应 Go 的扩展初始化；checkExisted 为真时才创建去重集合和锁。
    pub fn InitExt(&mut self, size: u64, checkExisted: bool, tryCnt: i32) {
        self.cap = size;
        if checkExisted {
            self.existed = Some(Mutex::new(HashSet::new()));
        } else {
            self.existed = None;
        }
        self.tryCnt = tryCnt.max(0) as usize;
    }

    /// 原子自增取 ID；启用去重时冲突则重试，耗尽尝试次数返回 `(0, false)`。
    // Get id by auto-increment.
    // Get 通过原子自增获取 ID；当 cap 小于 MaxUint64 时沿用 Go 的取模回绕语义。
    pub fn Get(&self) -> (u64, bool) {
        for _ in 0..self.tryCnt {
            let mut id = self.lastID.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
            if self.cap < u64::MAX {
                // Go 代码在 cap 为 0 时会触发除零 panic；不改变这个边界行为。
                id %= self.cap;
            }
            if let Some(existed) = &self.existed {
                // 加锁检查和插入必须是同一临界区，避免两个线程拿到同一个 ID。
                let mut existed = existed.lock().expect("AutoIncPool existed mutex poisoned");
                if existed.contains(&id) {
                    // guard 在 continue 前释放，对应 Go 分支里的显式 Unlock。
                    continue;
                }
                existed.insert(id);
            }
            return (id, true);
        }
        (0, false)
    }

    /// 归还 ID：启用去重时从集合移除；否则恒为 true。
    // Put id back to pool.
    // Put 在启用去重集合时删除占用记录；未启用时与 Go 一样直接返回 true。
    pub fn Put(&self, id: u64) -> bool {
        if let Some(existed) = &self.existed {
            let mut existed = existed.lock().expect("AutoIncPool existed mutex poisoned");
            existed.remove(&id);
        }
        true
    }

    /// 已占用集合大小；未启用去重时返回 -1。
    // Len implements IDPool interface.
    // Len 对 AutoIncPool 返回已占用集合大小；未启用 existed 时 Go 语义是 -1。
    pub fn Len(&self) -> i32 {
        if let Some(existed) = &self.existed {
            let existed = existed.lock().expect("AutoIncPool existed mutex poisoned");
            return existed.len() as i32;
        }
        -1
    }

    /// 返回初始化时设置的容量。
    // Cap implements IDPool interface.
    pub fn Cap(&self) -> i32 {
        self.cap as i32
    }
}

impl IDPool for AutoIncPool {
    fn Init(&mut self, size: u64) {
        AutoIncPool::Init(self, size);
    }

    fn Len(&self) -> i32 {
        AutoIncPool::Len(self)
    }

    fn Cap(&self) -> i32 {
        AutoIncPool::Cap(self)
    }

    fn Put(&self, val: u64) -> bool {
        AutoIncPool::Put(self, val)
    }

    fn Get(&self) -> (u64, bool) {
        AutoIncPool::Get(self)
    }
}

// String implements IDPool interface.
impl fmt::Display for AutoIncPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lastID: {}", self.lastID.load(Ordering::SeqCst))
    }
}

/// 无锁环形 ID 池（仅 32 位 ID），用 head/tail 与每槽 seq 协调读写。
// LockFreeCircularPool is a lock-free circular implementation of IDPool.
// Note that to reduce memory usage, LockFreeCircularPool supports 32bits IDs ONLY.
// LockFreeCircularPool 机械保留 Go 的无锁环形队列设计，head/tail 和每个 slot 的 seq 都用原子表达。
pub struct LockFreeCircularPool {
    _align_before_head: u64, // align to 64bits
    /// 第一个可读槽位下标（环形逻辑序号）。
    head: AtomicU32, // first available slot
    _padding_head: u32,      // padding to avoid false sharing
    /// 第一个可写空槽；`head==tail` 表示空。
    tail: AtomicU32, // first empty slot. `head==tail` means empty.
    _padding_tail: u32,      // padding to avoid false sharing

    /// 槽位数（实际容量为 cap-1，留一空位区分空/满）。
    cap: u32,
    /// 环形槽位数组。
    slots: Vec<LockFreePoolItem>,
}

impl Default for LockFreeCircularPool {
    fn default() -> Self {
        Self {
            _align_before_head: 0,
            head: AtomicU32::new(0),
            _padding_head: 0,
            tail: AtomicU32::new(0),
            _padding_tail: 0,
            cap: 0,
            slots: Vec::new(),
        }
    }
}

/// 无锁池单个槽位：值 + 读写状态序号。
pub struct LockFreePoolItem {
    /// 槽内存放的 32 位 ID。
    value: AtomicU32,

    // seq indicates read/write status
    // Sequence:
    //   seq==tail: writable ---> doWrite,seq:=tail+1 ---> seq==head+1:written/readable ---> doRead,seq:=head+size
    // ^ |
    //         +----------------------------------------------------------------------------------------+
    //   slot[i].seq: i(writable) ---> i+1(readable) ---> i+cap(writable) ---> i+cap+1(readable) ---> i+2*cap ---> ...
    // seq 是无锁队列区分可写/可读状态的关键字段；用 AtomicU32 表达 Go 的 atomic.Load/Store。
    /// 与 head/tail 比对以判定可写/可读。
    seq: AtomicU32,
}

impl LockFreePoolItem {
    fn new(value: u32, seq: u32) -> Self {
        Self {
            value: AtomicU32::new(value),
            seq: AtomicU32::new(seq),
        }
    }
}

impl LockFreeCircularPool {
    /// 默认初始化：`fillCount=0`（空池）。
    // Init implements IDPool interface.
    pub fn Init(&mut self, size: u64) {
        self.InitExt(size as u32, 0);
    }

    /// 初始化槽位；前 `fillCount` 个填入可读初值，其余为可写空槽。
    // InitExt initializes LockFreeCircularPool with more parameters.
    // fillCount: fills pool with [1, min(fillCount, 1<<(sizeInBits-1)]. Pass "math.MaxUint32" to fulfill the pool.
    // InitExt 初始化环形槽位；前 fillCount 个槽位标记为可读，其余槽位标记为可写。
    pub fn InitExt(&mut self, size: u32, fillCount: u32) {
        self.cap = size;
        self.slots.clear();
        self.slots.reserve(self.cap as usize);

        // Go 使用 mathutil.MinUint32(p.cap-1, fillCount)；cap 为 0 时保持 uint32 回绕语义。
        let fillCount = fillCount.min(self.cap.wrapping_sub(1));
        let mut i = 0;
        while i < fillCount {
            self.slots.push(LockFreePoolItem::new(i + 1, i + 1));
            i += 1;
        }
        while i < self.cap {
            self.slots.push(LockFreePoolItem::new(u32::MAX, i));
            i += 1;
        }

        self.head.store(0, Ordering::SeqCst);
        self.tail.store(fillCount, Ordering::SeqCst);
    }

    /// 测试专用：把 head/tail 推到接近溢出的位置以覆盖回绕。
    // InitForTest used to unit test overflow of head & tail.
    // InitForTest 只服务 Go 单测里的 head/tail 溢出场景，保持传入 head 的序列号偏移。
    pub fn InitForTest(&mut self, head: u32, fillCount: u32) {
        let fillCount = fillCount.min(self.cap.wrapping_sub(1));
        let mut i = 0;
        while i < fillCount {
            self.slots[i as usize] =
                LockFreePoolItem::new(i + 1, head.wrapping_add(i).wrapping_add(1));
            i += 1;
        }
        while i < self.cap {
            self.slots[i as usize] = LockFreePoolItem::new(u32::MAX, head.wrapping_add(i));
            i += 1;
        }

        self.head.store(head, Ordering::SeqCst);
        self.tail
            .store(head.wrapping_add(fillCount), Ordering::SeqCst);
    }

    /// 当前元素个数（`tail - head`，u32 回绕语义）。
    // Len implements IDPool interface.
    pub fn Len(&self) -> i32 {
        self.tail
            .load(Ordering::SeqCst)
            .wrapping_sub(self.head.load(Ordering::SeqCst)) as i32
    }

    /// 可用容量为 `cap - 1`（留一空位区分空/满）。
    // Cap implements IDPool interface.
    pub fn Cap(&self) -> i32 {
        self.cap.wrapping_sub(1) as i32
    }

    /// CAS 抢占 tail 后写入；满则 false，等待可写时 `yield_now`。
    // Put implements IDPool interface.
    // Put 先 CAS 抢占 tail，再等待对应 slot 进入 writable 状态；等待时用 yield_now 对应 runtime.Gosched。
    pub fn Put(&self, val: u64) -> bool {
        loop {
            let tail = self.tail.load(Ordering::SeqCst); // `tail` should be loaded before `head`, to avoid "false full".
            let head = self.head.load(Ordering::SeqCst);

            if tail.wrapping_sub(head) == self.cap.wrapping_sub(1) {
                return false;
            }

            if self
                .tail
                .compare_exchange(
                    tail,
                    tail.wrapping_add(1),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_err()
            {
                continue;
            }

            let slot = &self.slots[(tail & self.cap.wrapping_sub(1)) as usize];
            loop {
                let seq = slot.seq.load(Ordering::SeqCst);

                if seq == tail {
                    slot.value.store(val as u32, Ordering::SeqCst);
                    slot.seq.store(tail.wrapping_add(1), Ordering::SeqCst);
                    return true;
                }

                thread::yield_now();
            }
        }
    }

    /// CAS 抢占 head 后读取；空则返回 Invalid 哨兵与 false。
    // Get implements IDPool interface.
    // Get 先 CAS 抢占 head，再等待 slot 从 written/readable 状态转为可读；读完后推进 seq 让该槽位可复用。
    pub fn Get(&self) -> (u64, bool) {
        loop {
            let head = self.head.load(Ordering::SeqCst);
            let tail = self.tail.load(Ordering::SeqCst);
            if head == tail {
                return (IDPoolInvalidValue, false);
            }

            if self
                .head
                .compare_exchange(
                    head,
                    head.wrapping_add(1),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_err()
            {
                continue;
            }

            let slot = &self.slots[(head & self.cap.wrapping_sub(1)) as usize];
            loop {
                let seq = slot.seq.load(Ordering::SeqCst);

                if seq == head.wrapping_add(1) {
                    let val = slot.value.load(Ordering::SeqCst) as u64;
                    slot.value.store(u32::MAX, Ordering::SeqCst);
                    slot.seq
                        .store(head.wrapping_add(self.cap), Ordering::SeqCst);
                    return (val, true);
                }

                thread::yield_now();
            }
        }
    }
}

impl IDPool for LockFreeCircularPool {
    fn Init(&mut self, size: u64) {
        LockFreeCircularPool::Init(self, size);
    }

    fn Len(&self) -> i32 {
        LockFreeCircularPool::Len(self)
    }

    fn Cap(&self) -> i32 {
        LockFreeCircularPool::Cap(self)
    }

    fn Put(&self, val: u64) -> bool {
        LockFreeCircularPool::Put(self, val)
    }

    fn Get(&self) -> (u64, bool) {
        LockFreeCircularPool::Get(self)
    }
}

// String implements IDPool interface.
// Notice: NOT thread safe.
// Display 保留 Go String 输出格式；原注释说明它不是线程安全快照，同样不提供一致性保证。
impl fmt::Display for LockFreeCircularPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let head = self.head.load(Ordering::SeqCst);
        let tail = self.tail.load(Ordering::SeqCst);
        let headSlot = &self.slots[(head & self.cap.wrapping_sub(1)) as usize];
        let tailSlot = &self.slots[(tail & self.cap.wrapping_sub(1)) as usize];
        let length = tail.wrapping_sub(head);

        write!(
            f,
            "cap:{}, length:{}; head:{:x}, slot:{{{:x},{:x}}}; tail:{:x}, slot:{{{:x},{:x}}}",
            self.cap,
            length,
            head,
            headSlot.value.load(Ordering::SeqCst),
            headSlot.seq.load(Ordering::SeqCst),
            tail,
            tailSlot.value.load(Ordering::SeqCst),
            tailSlot.seq.load(Ordering::SeqCst),
        )
    }
}
