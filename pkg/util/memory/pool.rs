// Copyright 2025 PingCAP, Inc.
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

// 资源池（ResourcePool）与预算（Budget）配额树。
//
// 对应 Go `pool.go`：父子 pool 链表、对齐分配、超限/超容量回调，以及 Budget 的
// Grow/Shrink/Clear。本文件已有逐段中文说明；模块文档汇总职责便于导航。

// memory 包里的资源池配额树：ResourcePool 负责记录已分配字节、预算 Budget、
// 父子 pool 链表和超限回调，Budget 负责从来源 pool 申请、增长、释放容量。
// 锁保护边界、错误返回和父子资源池链表维护的代码形状，方便人工逐行审阅。
// Go 同包依赖:
// - DefMaxLimit 在 arbitrator.go 中定义；本文件只保留引用点，不在里解决跨文件接线。

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use std::ptr::NonNull;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

// MemoryResult 对应 Go 函数返回的 error。这里用 String 作为占位错误类型，
// 只表达“成功/失败”和错误文本，不声明已经接入 pingcap/errors 或 anyhow 等真实错误栈。
type MemoryResult = Result<(), String>;

// PoolLink 对应 Go 的 *ResourcePool。Go 允许 nil receiver 和链表裸指针；
// Rust 用 Option<NonNull<ResourcePool>> 保留“可为空的非拥有指针”语义。
type PoolLink = Option<NonNull<ResourcePool>>;

// Go: var resourcePoolID = int64(-1)
// 该 ID 从 -1 开始递减，转成 uint64 后表示内部 pool；这里保留相同的原子递减形状。
static resourcePoolID: AtomicI64 = AtomicI64::new(-1);

// DefPoolAllocAlignSize indicates the default allocation alignment size
// DefPoolAllocAlignSize 是默认分配对齐大小；Go int64 在 实现中用 i64 表达。
/// 默认分配对齐粒度（字节）。
pub const DefPoolAllocAlignSize: i64 = 10 * 1024;

// DefMaxUnusedBlocks indicates the default maximum unused blocks*alloc-align-size of the resource pool
// DefMaxUnusedBlocks 控制预算空闲块数量超过多少后触发 shrink。
/// 预算允许保留的最大未用对齐块数，超出则 shrink。
pub const DefMaxUnusedBlocks: i64 = 10;

// DefMaxLimit is defined by arbitrator.go. Keeping the numeric value here makes
// this file independently testable without manufacturing a substitute module.
const DefMaxLimit: i64 = 5_000_000_000_000_000;

// ResourcePool manages a set of resource quota
// ResourcePool 对应 Go 的同名结构体，字段顺序保持 actions、parentMu、name、mu、uid、reserved、limit 等。
// parentMu 里的 prev/next 只应由父 pool 持有锁时访问；mu 里的字段由本 pool 的 Mutex 保护。
/// 资源配额池：记录已分配字节、父子链表、Budget 与超限回调。
pub struct ResourcePool {
    // actions to be taken when the pool meets certain conditions
    pub actions: PoolActions,
    // accessible by parent pool only
    pub parentMu: ResourcePoolParentMu,
    // name of pool
    pub name: String,
    pub mu: Mutex<ResourcePoolMu>,
    // unique ID of the resource pool: uid <= 0 indicates that it is a internal pool
    pub uid: u64,
    // quota from other sources
    pub reserved: i64,
    // limit of the resource quota
    pub limit: i64,
    // each allocation size must be a multiple of it
    pub allocAlignSize: i64,
    // max unused-blocks*alloc-align size before shrinking budget
    pub maxUnusedBlocks: i64,
}

// ResourcePoolParentMu 对应 Go 里匿名的 parentMu struct。
// Go 代码没有单独命名；拆出来只是为了表达字段结构和父链表指针。
/// 父 pool 持有锁时维护的兄弟链表指针（prev/next）。
#[derive(Default)]
pub struct ResourcePoolParentMu {
    pub prevChildren: PoolLink,
    pub nextChildren: PoolLink,
}

// ResourcePoolMu 对应 Go 里匿名的 mu struct，并把嵌入的 sync.Mutex 放在外层 Mutex<ResourcePoolMu>。
// headChildren 维护子 pool 单链表头；allocated/maxAllocated/budget/stopped 都需要在锁内读写。
/// 本 pool Mutex 保护的状态：子链表头、Budget、已分配与 stopped。
pub struct ResourcePoolMu {
    // head of the children pools chain
    pub headChildren: PoolLink,
    // budget of the resource quota
    pub budget: Budget,
    // allocated bytes of the resource quota
    pub allocated: i64,
    // maximum allocated bytes
    pub maxAllocated: i64,
    // number of children pools
    pub numChildren: usize,
    pub stopped: bool,
}

impl Default for ResourcePoolMu {
    fn default() -> Self {
        Self {
            headChildren: None,
            budget: Budget::default(),
            allocated: 0,
            maxAllocated: 0,
            numChildren: 0,
            stopped: false,
        }
    }
}

// OutOfCapacityActionArgs wraps the arguments for out of capacity action
// OutOfCapacityActionArgs 保留 Go 回调入参：触发容量不足的 pool 指针和这次缺口 request。
/// 容量不足回调参数：触发 pool 与本次缺口 request。
#[derive(Clone, Copy)]
pub struct OutOfCapacityActionArgs {
    pub Pool: PoolLink,
    pub Request: i64,
}

// Go 的函数类型 func(...) error 在 实现中以 Arc<dyn Fn(...) -> MemoryResult> 占位。
// 使用 Arc 是为了近似 Go 里 PoolActions 被值复制时函数值仍可共享的语义。
/// 容量不足时的回调类型。
pub type OutOfCapacityAction = Arc<dyn Fn(OutOfCapacityActionArgs) -> MemoryResult + Send + Sync>;
/// 超过硬 limit 时的回调类型。
pub type OutOfLimitAction = Arc<dyn Fn(PoolLink) -> MemoryResult + Send + Sync>;

// PoolActions represents the actions to be taken when the resource pool meets certain conditions
// PoolActions 保存资源池达到特定条件时的回调。None 对应 Go nil func。
/// 资源池条件触发动作集合（容量不足 / 超限）。
#[derive(Clone, Default)]
pub struct PoolActions {
    // Called when the resource pool is out of capacity
    pub OutOfCapacityActionCB: Option<OutOfCapacityAction>,
    // Called when the resource pool is out of limit
    pub OutOfLimitActionCB: Option<OutOfLimitAction>,
}

// ResourcePoolState represents the state of a resource pool
// ResourcePoolState 是 Traverse 暴露给监控回调的快照，字段顺序与 Go 完全一致。
/// Traverse 暴露给监控回调的资源池快照。
pub struct ResourcePoolState {
    pub Name: String,
    pub Level: i32,
    pub ID: u64,
    pub ParentID: u64,
    pub Used: i64,
    pub Reserved: i64,
    pub Budget: i64,
}

impl ResourcePool {
    // Traverse the resource pool and calls the callback function
    // Traverse 从当前 pool 开始做深度遍历；Go 代码先调用内部 traverse(0, cb)。
    pub fn Traverse<F>(&self, mut stateCb: F) -> MemoryResult
    where
        F: FnMut(ResourcePoolState) -> MemoryResult,
    {
        self.traverse(0, &mut stateCb)
    }

    // traverse 对应 Go 的私有递归方法。它在锁内复制当前状态和 children 列表，
    // 然后释放锁再执行回调和递归，避免回调或子树遍历期间长期持有当前 pool 的锁。
    fn traverse<F>(&self, level: i32, stateCb: &mut F) -> MemoryResult
    where
        F: FnMut(ResourcePoolState) -> MemoryResult,
    {
        let mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during traverse");
        if mu.stopped {
            // Go 分支：stopped pool 不再向回调暴露状态，也不遍历其子节点。
            return Ok(());
        }

        let monitorState = ResourcePoolState {
            Level: level,
            Name: self.name.clone(),
            ID: self.uid,
            // Go 调用 p.mu.budget.pool.UID()；nil parent 通过 UID() 返回 0。
            ParentID: pool_uid(mu.budget.pool),
            Used: mu.allocated,
            Reserved: self.reserved,
            Budget: mu.budget.cap,
        };

        let mut children = Vec::with_capacity(mu.numChildren);
        let mut c = mu.headChildren;
        while let Some(child) = c {
            children.push(child);
            // Go 直接沿 c.parentMu.nextChildren 走链表；Rust 裸指针读取需要 unsafe。
            // 这里不取得所有权，只复制子 pool 指针，保持释放当前锁后递归的 Go 语义。
            c = unsafe { child.as_ref().parentMu.nextChildren };
        }
        drop(mu);

        stateCb(monitorState)?;
        for child in children {
            // 子 pool 的生命周期由原 Go 指针结构保证；这里用 unsafe 表达这类外部不变量。
            unsafe {
                child.as_ref().traverse(level + 1, stateCb)?;
            }
        }
        Ok(())
    }

    // SetAllocAlignSize sets the allocation alignment size and returns the original value of allocAlignSize
    // SetAllocAlignSize 在 Go 中持有 p.mu 后修改锁外字段 allocAlignSize；这里保留同一互斥边界。
    pub fn SetAllocAlignSize(&mut self, size: i64) -> i64 {
        let _guard = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during SetAllocAlignSize");
        let ori = self.allocAlignSize;
        self.allocAlignSize = size;
        ori
    }

    // NewResourcePoolInheritWithLimit creates a new resource pool inheriting from the parent pool
    // NewResourcePoolInheritWithLimit 从父 pool 复制对齐大小、unused blocks 和回调配置，limit 使用入参。
    pub fn NewResourcePoolInheritWithLimit(&self, name: String, limit: i64) -> Box<ResourcePool> {
        NewResourcePool(
            newPoolUID(),
            name,
            limit,
            self.allocAlignSize,
            self.maxUnusedBlocks,
            self.actions.clone(),
        )
    }

    // StartNoReserved creates a new resource pool with no reserved quota
    // StartNoReserved 是 Start(parent, 0) 的薄封装，表示子 pool 没有额外保留额度。
    pub fn StartNoReserved(&mut self, pool: PoolLink) {
        self.Start(pool, 0)
    }

    // UID returns the unique ID of the resource pool
    // UID 对应 Go 的 nil-safe 方法；Rust 的 None 情况由 pool_uid 辅助函数处理。
    pub fn UID(&self) -> u64 {
        self.uid
    }

    // Start starts the resource pool with a parent pool and reserved quota
    // Start 初始化 pool 的 Budget，并在父 pool 的 children 链表头插入当前 pool。
    // Go 允许 parentPool 为 nil，此时 CreateBudget 返回来源 pool 为 nil 的 Budget，表示 root pool。
    pub fn Start(&mut self, parentPool: PoolLink, reserved: i64) {
        let mut mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during Start");
        if mu.allocated != 0 {
            panic!(
                "{}: started with {} bytes left over",
                self.name, mu.allocated
            );
        }
        if let Some(started_pool) = mu.budget.pool {
            panic!(
                "{}: already started with pool {}",
                self.name,
                pool_name(started_pool)
            );
        }

        mu.allocated = 0;
        mu.maxAllocated = 0;
        mu.budget = create_budget_from_pool(parentPool);
        mu.stopped = false;
        self.reserved = reserved;
        drop(mu);

        if let Some(mut parent) = parentPool {
            // 父子关系由父 pool 的 mu 保护。Go 代码只在父锁内改 headChildren 和子节点 parentMu 指针。
            unsafe {
                let parent_ref = parent.as_mut();
                let mut parent_mu = parent_ref
                    .mu
                    .lock()
                    .expect("parent ResourcePool.mu poisoned during Start");
                if let Some(mut s) = parent_mu.headChildren {
                    s.as_mut().parentMu.prevChildren = Some(NonNull::from(&mut *self));
                    self.parentMu.nextChildren = Some(s);
                }
                parent_mu.headChildren = Some(NonNull::from(&mut *self));
                parent_mu.numChildren += 1;
            }
        }
    }

    // Name returns the name of the resource pool
    // Name 返回资源池名称；Go 返回 string 值，这里返回 &str 以避免不必要复制。
    pub fn Name(&self) -> &str {
        &self.name
    }

    // Limit returns the limit of the resource pool
    // Limit 直接返回资源配额上限。
    pub fn Limit(&self) -> i64 {
        self.limit
    }

    // IsStopped checks if the resource pool is stopped
    // IsStopped 在锁内读取 stopped 标记，保持 Go 的并发可见性语义。
    pub fn IsStopped(&self) -> bool {
        let mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during IsStopped");
        mu.stopped
    }

    // Stop stops the resource pool and releases the budget & returns the quota released
    // Stop 停止 pool、释放已分配 quota、释放 Budget，并从父 pool children 链表中摘除当前节点。
    pub fn Stop(&mut self) -> i64 {
        let self_ptr = self as *mut Self;
        let mut mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during Stop");
        mu.stopped = true;

        if mu.allocated != 0 {
            // Go 在持有 p.mu 时调用 doRelease；显式传入锁内状态，标明该方法不再二次加锁。
            let allocated = mu.allocated;
            unsafe { (*self_ptr).doRelease(&mut mu, allocated) };
        }

        let released = mu.budget.cap;
        unsafe { (*self_ptr).releaseBudget(&mut mu) };

        if let Some(mut parent) = mu.budget.pool {
            // Go 这里用一个闭包和 defer parent.mu.Unlock()；显式 drop guard 表达资源收尾。
            unsafe {
                let parent_ref = parent.as_mut();
                let mut parent_mu = parent_ref
                    .mu
                    .lock()
                    .expect("parent ResourcePool.mu poisoned during Stop");
                let prev = self.parentMu.prevChildren;
                let next = self.parentMu.nextChildren;
                if parent_mu.headChildren == Some(NonNull::new(self_ptr).unwrap()) {
                    parent_mu.headChildren = next;
                }
                if let Some(mut prev) = prev {
                    prev.as_mut().parentMu.nextChildren = next;
                }
                if let Some(mut next) = next {
                    next.as_mut().parentMu.prevChildren = prev;
                }
                parent_mu.numChildren -= 1;
            }
        }

        mu.budget.pool = None;
        released
    }

    // MaxAllocated returns the maximum allocated bytes
    // MaxAllocated 在锁内读取历史最大已分配字节数。
    pub fn MaxAllocated(&self) -> i64 {
        let mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during MaxAllocated");
        mu.maxAllocated
    }

    // Allocated returns the allocated bytes
    // Allocated 在锁内调用 allocated，保持 Go 代码的读路径结构。
    pub fn Allocated(&self) -> i64 {
        let mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during Allocated");
        self.allocated(&mu)
    }

    // CreateBudget creates a new budget from the resource pool
    // CreateBudget 对应 Go 方法，返回来源 pool 指向当前 ResourcePool 的 Budget。
    pub fn CreateBudget(&mut self) -> Budget {
        Budget {
            pool: Some(NonNull::from(&mut *self)),
            cap: 0,
            used: 0,
            explicitReserved: 0,
        }
    }

    // doAlloc 对应 Go 的私有方法，调用方已经持有 p.mu。
    // 它先检查 limit，再按需向父 Budget 增加容量，最后更新 allocated/maxAllocated。
    fn doAlloc(&mut self, mu: &mut ResourcePoolMu, request: i64) -> MemoryResult {
        if mu.allocated > self.limit - request {
            if let Some(cb) = self.actions.OutOfLimitActionCB.clone() {
                // Go 回调拿到 *ResourcePool；传 NonNull 指针，真实迁移需处理借用和回调重入。
                cb(Some(NonNull::from(&mut *self)))?;
            } else {
                return Err(newBudgetExceededError(
                    "out of limit",
                    self,
                    request,
                    mu.allocated,
                    self.limit,
                ));
            }
        }

        // Check whether we need to request an increase of our budget.
        // delta > 0 表示当前 reserved + budget.used 不足以覆盖新申请，需要向上层 Budget 要额度。
        let delta = request + mu.allocated - mu.budget.used - self.reserved;
        if delta > 0 {
            self.increaseBudget(mu, delta)?;
        }
        mu.allocated += request;
        if mu.maxAllocated < mu.allocated {
            mu.maxAllocated = mu.allocated;
        }

        Ok(())
    }

    // ExplicitReserve reserves the budget explicitly
    // ExplicitReserve 显式预留 Budget；Go 在当前 pool 锁内调用 Budget.Reserve。
    pub fn ExplicitReserve(&mut self, request: i64) -> MemoryResult {
        let mut mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during ExplicitReserve");
        mu.budget.Reserve(request)
    }

    // allocate 对应 Go 的私有方法：获取当前 pool 锁，调用 doAlloc，然后释放锁返回错误。
    fn allocate(&mut self, request: i64) -> MemoryResult {
        let self_ptr = self as *mut Self;
        let mut mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during allocate");
        unsafe { (*self_ptr).doAlloc(&mut mu, request) }
    }

    // allocated 对应 Go 的私有读方法，调用者负责持锁。
    fn allocated(&self, mu: &ResourcePoolMu) -> i64 {
        mu.allocated
    }

    // capacity 对应 Go 的私有读方法，返回当前 Budget cap。
    fn capacity(&self, mu: &ResourcePoolMu) -> i64 {
        mu.budget.cap
    }

    // ApproxCap returns the approximate capacity of the resource pool
    // ApproxCap 在 Go 中不加锁读取 capacity；仍通过 Mutex 读取以表达字段封装，
    // 语义上仍是“近似容量”，不应作为强一致状态判断。
    pub fn ApproxCap(&self) -> i64 {
        let mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during ApproxCap");
        self.capacity(&mu)
    }

    // Capacity returns the capacity of the resource pool
    // Capacity 在锁内返回 Budget cap，是精确读路径。
    pub fn Capacity(&self) -> i64 {
        let mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during Capacity");
        self.capacity(&mu)
    }

    // SetLimit sets the limit of the resource pool
    // SetLimit 在锁内更新 limit；Go 代码同样使用 p.mu 保护这个锁外字段。
    pub fn SetLimit(&mut self, newLimit: i64) {
        let _guard = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during SetLimit");
        self.limit = newLimit;
    }

    // release 对应 Go 的私有方法：持锁后调用 doRelease。
    fn release(&mut self, sz: i64) {
        let self_ptr = self as *mut Self;
        let mut mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during release");
        unsafe { (*self_ptr).doRelease(&mut mu, sz) };
    }

    // doRelease 对应 Go 的锁内释放逻辑：扣减 allocated，然后根据空闲 budget 调整容量。
    fn doRelease(&mut self, mu: &mut ResourcePoolMu, mut sz: i64) {
        if mu.allocated < sz {
            sz = mu.allocated;
        }
        mu.allocated -= sz;

        self.doAdjustBudget(mu);
    }

    // SetOutOfCapacityAction sets the out of capacity action
    // It is called when the resource pool is out of capacity
    // SetOutOfCapacityAction 在锁内替换容量不足回调。
    pub fn SetOutOfCapacityAction(&mut self, f: Option<OutOfCapacityAction>) {
        let self_ptr = self as *mut Self;
        let mut mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during SetOutOfCapacityAction");
        unsafe { (*self_ptr).doSetOutOfCapacityAction(&mut mu, f) };
    }

    // doSetOutOfCapacityAction 对应 Go 的锁内赋值辅助函数。
    fn doSetOutOfCapacityAction(
        &mut self,
        _mu: &mut ResourcePoolMu,
        f: Option<OutOfCapacityAction>,
    ) {
        self.actions.OutOfCapacityActionCB = f;
    }

    // SetOutOfLimitAction sets the out of limit action
    // It is called when the resource pool is out of limit
    // SetOutOfLimitAction 在锁内替换 limit 超限回调。
    pub fn SetOutOfLimitAction(&mut self, f: Option<OutOfLimitAction>) {
        let _guard = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during SetOutOfLimitAction");
        self.actions.OutOfLimitActionCB = f;
    }

    // increaseBudget 对应 Go 的私有方法，调用方已经持有 p.mu。
    // root pool 没有父 pool，需要走 OutOfCapacityAction 或返回 out of quota；非 root 则向父 Budget Grow。
    fn increaseBudget(&mut self, mu: &mut ResourcePoolMu, request: i64) -> MemoryResult {
        if mu.budget.pool.is_none() {
            // Root Pool
            let need = request - mu.budget.available();
            if need <= 0 {
                mu.budget.used += request;
                return Ok(());
            }

            if let Some(cb) = self.actions.OutOfCapacityActionCB.clone() {
                cb(OutOfCapacityActionArgs {
                    Pool: Some(NonNull::from(&mut *self)),
                    Request: need,
                })?;

                mu.budget.used += request;
                return Ok(());
            }
            return Err(newBudgetExceededError(
                "out of quota",
                self,
                request,
                mu.budget.used,
                mu.budget.cap,
            ));
        }

        mu.budget.Grow(request)
    }

    // roundSize 对齐到 allocAlignSize 的整数倍；alignSize <= 1 时保持原值。
    fn roundSize(&self, sz: i64) -> i64 {
        let alignSize = self.allocAlignSize;
        if alignSize <= 1 {
            return sz;
        }
        (sz + alignSize - 1) / alignSize * alignSize
    }

    // releaseBudget 对应 Go 的私有方法，只调用 Budget.Clear。
    // Clear 会把 cap/used 清零并把容量释放回来源 pool，但不会把 budget.pool 本身置空。
    fn releaseBudget(&mut self, mu: &mut ResourcePoolMu) {
        mu.budget.Clear();
    }

    // AdjustBudget adjusts the budget of the resource pool
    // AdjustBudget 暴露给外部的预算调整入口；先加锁，再调用锁内实现。
    pub fn AdjustBudget(&mut self) {
        let self_ptr = self as *mut Self;
        let mut mu = self
            .mu
            .lock()
            .expect("ResourcePool.mu poisoned during AdjustBudget");
        unsafe { (*self_ptr).doAdjustBudget(&mut mu) };
    }

    // doAdjustBudget 根据 allocated-reserved 计算实际需要的预算，空闲容量超过阈值时 Shrink。
    fn doAdjustBudget(&mut self, mu: &mut ResourcePoolMu) {
        let mut needed = mu.allocated - self.reserved;
        if needed <= 0 {
            needed = 0;
        } else {
            needed = self.roundSize(needed);
        }
        if self.allocAlignSize * self.maxUnusedBlocks <= mu.budget.used - needed {
            let delta = mu.budget.used - needed;
            mu.budget.Shrink(delta);
        }
    }

    // forceAddCap 对应 Go 的测试/内部辅助：调用方已持有 pool 锁，直接修改
    // Budget cap，不做向父池申请或释放，也不能再次获取同一把非重入锁。
    pub(crate) fn forceAddCap(&mut self, c: i64) {
        if c == 0 {
            return;
        }
        let mu = self
            .mu
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        mu.budget.cap += c;
    }
}

// NewResourcePoolDefault creates a new resource pool
// NewResourcePoolDefault 用内部 uid、无限制 limit、默认 maxUnusedBlocks 和空回调创建 pool。
/// 以默认 maxUnusedBlocks 创建资源池（尚未 Start）。
pub fn NewResourcePoolDefault(name: String, allocAlignSize: i64) -> Box<ResourcePool> {
    NewResourcePool(
        newPoolUID(),
        name,
        0,
        allocAlignSize,
        DefMaxUnusedBlocks,
        PoolActions::default(),
    )
}

// newPoolUID 对应 Go 的 atomic.AddInt64(&resourcePoolID, -1) 再转 uint64。
// fetch_sub 返回旧值，所以这里减 1 后再转换，保持第一次调用得到 -2 的 Go 行为。
fn newPoolUID() -> u64 {
    let next = resourcePoolID.fetch_sub(1, Ordering::SeqCst) - 1;
    next as u64
}

// NewResourcePool creates a new resource pool
// NewResourcePool 只构造 ResourcePool，不自动 Start；allocAlignSize/limit 的默认值与 Go 一致。
/// 创建资源池并指定 maxUnusedBlocks。
pub fn NewResourcePool(
    uid: u64,
    name: String,
    mut limit: i64,
    mut allocAlignSize: i64,
    maxUnusedBlocks: i64,
    actions: PoolActions,
) -> Box<ResourcePool> {
    if allocAlignSize <= 0 {
        allocAlignSize = DefPoolAllocAlignSize;
    }
    if limit <= 0 {
        // DefMaxLimit 来自 Go 同包 arbitrator.go。这里保留原符号引用，真实 Rust 接线时应由同模块提供。
        limit = DefMaxLimit;
    }
    Box::new(ResourcePool {
        name,
        uid,
        limit,
        allocAlignSize,
        actions,
        maxUnusedBlocks,
        parentMu: ResourcePoolParentMu::default(),
        mu: Mutex::new(ResourcePoolMu::default()),
        reserved: 0,
    })
}

// Budget represents the budget of a resource pool
// Budget 保存从来源 pool 申请到的容量、已使用量和显式保留量。
/// 从某 ResourcePool 申请的预算句柄：cap/used 与对齐块管理。
pub struct Budget {
    // source pool
    pub pool: PoolLink,
    // capacity of the budget
    pub cap: i64,
    // used bytes
    pub used: i64,
    // explicit reserved size which can not be shrunk
    pub explicitReserved: i64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            pool: None,
            cap: 0,
            used: 0,
            explicitReserved: 0,
        }
    }
}

impl Budget {
    // Used returns the used bytes of the budget
    // Used 对应 Go 的 nil-safe 方法；Rust 只有 Some(&Budget) 时调用，nil 情况由调用点用 Option 处理。
    pub fn Used(&self) -> i64 {
        self.used
    }

    // Pool returns the resource pool of the budget
    // Pool 返回来源 ResourcePool 的非拥有指针，None 对应 Go nil。
    pub fn Pool(&self) -> PoolLink {
        self.pool
    }

    // Capacity returns the capacity of the budget
    // Capacity 返回 Budget 当前容量 cap。
    pub fn Capacity(&self) -> i64 {
        self.cap
    }

    // available 对应 Go 私有方法：cap-used 即尚未消耗的预算。
    fn available(&self) -> i64 {
        self.cap - self.used
    }

    // Reserve reserves the budget through the allocate aligned given size; update the explicit reserved size;
    // Reserve 向来源 pool 申请按 allocAlignSize 对齐后的容量，并增加 explicitReserved。
    pub fn Reserve(&mut self, request: i64) -> MemoryResult {
        let Some(mut pool) = self.pool else {
            // Go 中 nil Budget receiver 或 nil pool 都直接返回 nil；这里保留空操作语义。
            return Ok(());
        };
        unsafe {
            let minExtra = pool.as_ref().roundSize(request);
            pool.as_mut().allocate(minExtra)?;
            self.cap += minExtra;
            self.explicitReserved += request;
        }
        Ok(())
    }

    // Empty releases the used budget
    // Empty 清空 used，并在可释放容量超过一个 allocAlignSize 时把多余容量还给来源 pool。
    pub fn Empty(&mut self) {
        let Some(mut pool) = self.pool else {
            return;
        };
        self.used = 0;
        unsafe {
            let release = self.available() - pool.as_ref().allocAlignSize;
            if release > 0 {
                pool.as_mut().release(release);
                self.cap -= release;
            }
        }
    }

    // Clear releases the budget and resets
    // Clear 把 cap 和 used 清零，并把原 cap 全量释放回来源 pool；pool 指针保持不变，匹配 Go 行为。
    pub fn Clear(&mut self) {
        let release = self.cap;
        self.used = 0;
        self.cap = 0;

        let Some(mut pool) = self.pool else {
            return;
        };
        if release > 0 {
            unsafe {
                pool.as_mut().release(release);
            }
        }
    }

    // resize 对应 Go 私有方法，根据 new-old 的 delta 选择 Grow、Shrink 或无操作。
    fn resize(&mut self, oldSz: i64, newSz: i64) -> MemoryResult {
        let delta = newSz - oldSz;
        match delta {
            d if d > 0 => self.Grow(d),
            d if d < 0 => {
                self.Shrink(-d);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    // ResizeTo resizes the budget to the new size
    // ResizeTo 把 used 调整到新大小；新旧相等时直接返回。
    pub fn ResizeTo(&mut self, newSz: i64) -> MemoryResult {
        if newSz == self.used {
            return Ok(());
        }
        self.resize(self.used, newSz)
    }

    // Grow the budget by the given size
    // Grow 增加 used；如果 available 不足，则按来源 pool 的对齐大小申请额外 cap。
    pub fn Grow(&mut self, request: i64) -> MemoryResult {
        let Some(mut pool) = self.pool else {
            return Ok(());
        };
        let extra = request - self.available();
        if extra > 0 {
            unsafe {
                let minExtra = pool.as_ref().roundSize(extra);
                pool.as_mut().allocate(minExtra)?;
                self.cap += minExtra;
            }
        }
        self.used += request;
        Ok(())
    }

    // Shrink the budget and reduce the given size
    // Shrink 减少 used，并在空闲预算超过一个对齐块且未违反 explicitReserved 时释放给来源 pool。
    pub fn Shrink(&mut self, mut delta: i64) {
        if delta == 0 {
            return;
        }
        if self.used < delta {
            delta = self.used;
        }
        self.used -= delta;

        let Some(mut pool) = self.pool else {
            return;
        };

        unsafe {
            let release = self.available() - pool.as_ref().allocAlignSize;
            if release > 0
                && (self.explicitReserved == 0
                    || self.used + pool.as_ref().allocAlignSize > self.explicitReserved)
            {
                pool.as_mut().release(release);
                self.cap -= release;
            }
        }
    }
}

// create_budget_from_pool 对应 Go 的 parentPool.CreateBudget()。
// Go 可以对 nil *ResourcePool 调用方法并得到 Budget{pool:nil}；这个辅助函数显式保留 nil 情况。
fn create_budget_from_pool(pool: PoolLink) -> Budget {
    Budget {
        pool,
        cap: 0,
        used: 0,
        explicitReserved: 0,
    }
}

// pool_uid 对应 Go 的 (*ResourcePool).UID nil-safe 语义。
fn pool_uid(pool: PoolLink) -> u64 {
    match pool {
        Some(pool) => unsafe { pool.as_ref().UID() },
        None => 0,
    }
}

// pool_name 只用于复刻 Go panic("%s: already started with pool %s") 的错误文本。
fn pool_name(pool: NonNull<ResourcePool>) -> String {
    unsafe { pool.as_ref().name.clone() }
}

// newBudgetExceededError 对应 Go 的 fmt.Errorf 包装，保留原始错误文本和参数顺序。
fn newBudgetExceededError(
    reason: &str,
    root: &ResourcePool,
    requestedBytes: i64,
    allocatedBytes: i64,
    limitBytes: i64,
) -> String {
    format!(
        "resource pool `{}` meets `{}`: requested({}) + allocated({}) > limit({})",
        root.name, reason, requestedBytes, allocatedBytes, limitBytes
    )
}
