# `pkg/util/globalconn/pool.rs`

## 文件定位

本文件属于 `astersql-util-globalconn` crate，crate 入口 [`lib.rs`](lib.rs) 将私有模块 `pool` 的公开项全部重导出。它提供连接 ID 分配器的底层本地 ID 池：[`globalconn.rs`](globalconn.rs) 中的 `SimpleAllocator` 使用 `AutoIncPool`，`GlobalAllocator` 同时使用 `LockFreeCircularPool`（32 位本地 ID）和 `AutoIncPool`（64 位本地 ID）。因此本文件不负责 GCID 的位编码或 ServerID 管理，而负责本地 ID 的申请、归还和并发复用。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：包名为 `astersql-util-globalconn`，库入口是 `lib.rs`，运行时外部依赖仅有 `log`；本文件本身只使用 Rust 标准库。根工作区 `Cargo.toml` 将 `pkg/util/globalconn` 列为 workspace member，并以 `facade_util_globalconn` 映射该包。

## 核心职责

- 以 `IDPool` 统一两种池的初始化、长度、容量、归还和获取接口，并用 `Display` 对齐 Go 的 `fmt.Stringer`。
- `AutoIncPool` 通过原子递增生成 ID；可选维护已占用集合，在有限次冲突重试后报告耗尽。它适合容量很大的 64 位空间，也用于未开启 GlobalKill 时的简单连接号分配。
- `LockFreeCircularPool` 以 `head`、`tail` 和每槽 `seq` 实现多生产者、多消费者 FIFO；它只保存 32 位值，用于 `GlobalAllocator` 的 32 位本地连接号池。
- 用 `IDPoolInvalidValue == u64::MAX` 表示无锁池取空时的无效值；调用者仍必须以返回的 `bool` 判断成功与否。

这些职责可由 `pool.rs` 的 `IDPool`、`AutoIncPool`、`LockFreeCircularPool` 以及 `globalconn.rs` 的 `NewSimpleAllocator`、`GlobalAllocator::NewGlobalAllocator`、`Allocate`、`Release` 交叉验证。

## 主要符号

- `IDPoolInvalidValue: u64`：空池哨兵，值为 `u64::MAX`，对应 Go `math.MaxUint64`。
- `IDPool: fmt::Display`：公开 trait。`Init(&mut self, u64)` 负责初始化；`Len` 返回当前元素/占用数，若实现不支持则为 `-1`；`Cap` 返回可用容量；`Put`、`Get` 以布尔值报告满或空。
- `AutoIncPool`：包含原子 `lastID`、模空间 `cap`、冲突重试次数 `tryCnt`，以及可选的 `Mutex<HashSet<u64>>` 已占用集合。公开固有方法为 `Init`、`InitExt`、`Get`、`Put`、`Len`、`Cap`，并实现 `Default`、`IDPool` 和 `Display`。
- `LockFreeCircularPool`：包含原子 `head`/`tail`、槽位数 `cap` 和 `Vec<LockFreePoolItem>`；两个填充字段用于让头尾布局保持 64 位边界并减少伪共享。公开固有方法为 `Init`、`InitExt`、测试辅助方法 `InitForTest`、`Len`、`Cap`、`Put`、`Get`，并实现 `Default`、`IDPool` 和 `Display`。
- `LockFreePoolItem`：每个槽位包含原子 `value: AtomicU32` 与状态序号 `seq: AtomicU32`。构造函数 `new` 为文件内部实现细节；类型虽为公开，但字段不公开。

本文件没有条件编译项。测试由 `lib.rs` 在 `#[cfg(test)]` 下以独立文件 `pool_test.rs` 和 `migration_aster_unit_test.rs` 挂载，生产源码与测试逻辑没有混放。

## 执行流程

`AutoIncPool` 的流程如下：

1. `Init` 委托 `InitExt(size, false, 1)`；`InitExt` 保存容量和非负重试次数，并按 `checkExisted` 创建或关闭已占用集合。
2. `Get` 每次尝试先对 `lastID` 做 `SeqCst` 原子加一。只要 `cap < u64::MAX`，就对结果取模，因此达到容量后回绕到零。
3. 未启用去重时立即返回新 ID。启用去重时，在同一个互斥临界区检查并插入 ID；若已占用则释放锁并继续下一次尝试。
4. 所有尝试均冲突时返回 `(0, false)`。`Put` 从已占用集合删除指定 ID；未启用集合时它是恒成功的空操作。

`LockFreeCircularPool` 的流程如下：

1. `Init` 创建空池；`InitExt` 把 `fillCount` 截到 `cap - 1`，将前若干槽初始化为值 `1..=fillCount` 且标为可读，其余槽填 `u32::MAX` 并标为可写，然后把 `head` 设为 0、`tail` 设为填充数。
2. `Put` 先按顺序读取 `tail` 再读取 `head`；若 `tail - head == cap - 1`，池满并返回 `false`。否则通过 CAS 抢占一个逻辑 `tail`，定位 `tail & (cap - 1)` 对应槽位，等待 `slot.seq == tail`，写入值后将序号推进为 `tail + 1`，使槽位可读。
3. `Get` 在 `head == tail` 时返回 `(IDPoolInvalidValue, false)`。否则通过 CAS 抢占一个逻辑 `head`，等待 `slot.seq == head + 1`，读取并清空值，再把序号推进到 `head + cap`，使槽位在下一圈可写。
4. CAS 竞争失败会重试；已抢占逻辑位置但槽状态尚未就绪时调用 `thread::yield_now()` 主动让出调度机会。

`globalconn.rs` 的上层流程是：构造全局分配器时把 32 位池填满、把 64 位自增池设为去重模式；`Allocate` 优先从 32 位池 `Get`，空时升级到 64 位并从自增池获取；`Release` 解析 GCID 后把本地号归还对应池，并可能在 32 位池空闲量恢复后降级回 32 位模式。

## 数据与状态

`AutoIncPool.lastID` 是单调按 `u64` 回绕的逻辑计数器，并不会因 `Put` 回退；释放只改变 `existed`。因此启用去重后，被释放的 ID 只有在计数器再次遍历到它时才会重新发放。`Len` 在启用集合时返回当前占用数，否则返回 `-1`；`Cap` 将 `u64` 容量转换为 `i32`，大容量可能按 Rust `as` 语义截断，这与上层是否使用该值有关。

无锁池保留一个空槽来区分满与空，所以槽位数是 `cap`，对外可用容量是 `cap - 1`。`head` 和 `tail` 是持续增长并允许 `u32` 回绕的逻辑位置，`Len` 使用 `tail.wrapping_sub(head)`。每槽 `seq` 编码生命周期：逻辑位置 `i` 时 `seq == i` 表示可写，写完变为 `i + 1` 表示可读，读完变为 `i + cap` 表示下一圈可写。`InitForTest` 只用于把这些序号整体移到接近溢出的位置。

槽索引使用 `logical_position & (cap - 1)`，所以当前实现隐含 `cap` 为非零的 2 的幂；生产调用 `1 << LocalConnIDBits32`、相关测试也都按 `1 << sizeInBits` 初始化。代码没有在 API 边界验证这一前置条件。

## 依赖与调用关系

下游依赖均来自标准库：`AtomicU64` 用于自增号，`AtomicU32` 用于无锁队列位置、槽值和序号，`Mutex<HashSet<u64>>` 用于可选去重，`fmt` 提供诊断字符串，`thread::yield_now` 对齐 Go 的 `runtime.Gosched`。所有原子操作均使用 `Ordering::SeqCst`。

RustCodeGraph 将 `pool.rs` 标为被 `globalconn.rs`、`globalconn_test.rs`、`migration_aster_unit_test.rs`、`pool_test.rs` 和 `pkg/util/security_2_aster_unit_test.rs` 五个文件使用。核心生产调用边由源码确认：

- `NewSimpleAllocator -> AutoIncPool::Init`，`SimpleAllocator::NextID -> AutoIncPool::Get`，`SimpleAllocator::Release -> AutoIncPool::Put`。
- `GlobalAllocator::NewGlobalAllocator -> LockFreeCircularPool::InitExt / AutoIncPool::InitExt`。
- `GlobalAllocator::Allocate -> LockFreeCircularPool::Get / AutoIncPool::Get`。
- `GlobalAllocator::Release -> AutoIncPool::Put / LockFreeCircularPool::Put`，并读取无锁池 `Len`、`Cap` 决定是否降级。

RustCodeGraph 对带类型限定的方法名执行 `callers/callees` 时未返回方法级边，因此这里没有把缺失的图边误写成已验证事实；上述边来自索引展示的直接使用文件和 `globalconn.rs` 中的实际调用表达式。

## 错误处理与边界

两种池用返回布尔值表达正常的容量边界：无锁池满时 `Put` 返回 `false`，空时 `Get` 返回哨兵和 `false`；自增池在 `tryCnt` 次冲突后返回 `(0, false)`。调用者不能单独依赖数值，因为零是有效的回绕 ID，而哨兵值也应结合 `false` 判断。

当前实现有若干由 Go 语义直接保留、但没有主动校验的前置条件：

- `AutoIncPool::Get` 在 `cap == 0` 且 `tryCnt > 0` 时执行模零并 panic；默认值未初始化时 `tryCnt == 0`，会直接返回失败。
- 无锁池要求非零、2 的幂容量；否则按位与不能正确替代取模。未初始化或零容量时，`Put`、`Get`、`Display` 还可能产生错误索引或算术边界行为。
- `LockFreeCircularPool::Put` 将 `u64` 直接转换成 `u32`，高 32 位会被截断；类型注释和上层用途明确该池只支持 32 位 ID。
- `AutoIncPool` 的互斥锁若中毒，`Get`、`Put`、`Len` 会通过 `expect` panic，而不是返回可恢复错误。
- 两个 `Display` 实现都只用于诊断。无锁版本分别加载多个原子，不能提供同一时刻的一致快照；在池未正确初始化时还可能索引失败。

## 并发与资源生命周期

池必须先通过 `Init`/`InitExt` 完成单线程初始化，再共享给并发调用者；这些初始化方法需要 `&mut self`，会重建槽位或去重集合，不应与 `Get`/`Put` 并发执行。初始化后，`Get`、`Put`、`Len` 和 `Cap` 接收 `&self`，可以通过共享引用调用。

`AutoIncPool` 的 ID 生成由原子计数器并发安全地推进；启用去重时，检查和插入处于同一锁区，避免两个线程同时占有同一 ID。`Put` 与 `Len` 使用同一把锁维护/观察集合。未启用去重时，`Put` 不承担复用管理，`Len == -1` 明示无法统计占用。

无锁池先用 CAS 为每个生产者或消费者分配唯一逻辑位置，再用 `seq` 在该位置的生产者和消费者之间发布状态。保留槽位避免 `head == tail` 同时表示满和空；`u32` 回绕通过 `wrapping_*` 与序号协议覆盖。等待是自旋加 `yield_now`，没有阻塞原语、超时或取消：如果已抢占位置的线程永久停顿，后继在相关槽位上可能持续等待。`SeqCst` 提供最强的全序内存语义，但也意味着扩展或优化时不能未经并发证明就削弱内存序。

槽位和集合都由池对象拥有，随对象析构自动释放；没有后台线程、通道、文件描述符或显式 `Drop`。测试通过 `Arc<dyn IDPool + Send + Sync>` 在多线程间共享初始化后的池，并在结束时 join 所有线程。

## 与 Go 版本的对应关系

Rust 文件逐项对应同目录 [`pool.go`](pool.go)：`IDPoolInvalidValue`、`IDPool`、`AutoIncPool`、`LockFreeCircularPool` 及其初始化、容量、获取、归还和字符串接口均保留。主要机械映射为：Go `atomic.AddUint64` 对应 `AtomicU64::fetch_add(...).wrapping_add(1)`；`sync.Mutex + map` 对应 `Mutex<HashSet>`；`atomic.Uint32` 与槽位原子函数对应 `AtomicU32`；`runtime.Gosched` 对应 `thread::yield_now`；无符号溢出通过 `wrapping_add`、`wrapping_sub` 显式表达。

可见差异包括：Rust 用 `(value, bool)` 代替 Go 多返回值，用 trait 加 `Display` 代替 interface 加 `fmt.Stringer`；Go 的独立 `mu` 和 `existed` 字段合并为 `Option<Mutex<HashSet<_>>>`；Rust 把槽值也做成 `AtomicU32`；Rust 的 `tryCnt: i32` 在初始化时被截为非负 `usize`，因此 Go 中负数迭代次数等价于零次尝试。Go 在文件中用编译期赋值断言两个实现满足接口，Rust 则通过显式 `impl IDPool` 在编译期保证。

测试意图也保持对应：[`pool_test.rs`](pool_test.rs) 对齐 [`pool_test.go`](pool_test.go) 的顺序分配、回绕、耗尽、释放复用、满空 FIFO、空池初始化、多生产者/消费者总和和接近 `u32` 溢出场景；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 另以较小数据集检查 Go 行为及并发值集合完整性。Rust 的 `BenchmarkPoolConcurrency` 目前只是未接入 bench harness 的普通函数，而 Go 版本是实际 benchmark，这一点不能视为 Rust 已具备等价基准入口。

## 扩展指南

- 若增加新的池实现，应实现 `IDPool` 和 `Display`，明确 `Len` 的语义、空/满返回约定与可并发性，并在独立测试文件中加入 trait-object、满空、复用和并发测试。
- 若修改自增分配策略，主要接入点是 `AutoIncPool::InitExt`、`Get`、`Put`；必须同步验证零值、容量回绕、冲突次数、释放后再次命中以及 `globalconn.rs` 中 64 位池耗尽的 panic 假设。
- 若修改无锁算法，主要接入点是 `InitExt`、`Put`、`Get` 和 `LockFreePoolItem.seq` 协议。需要同步 `pool_test.rs` 的基本流程、溢出测试和五类并发场景，并与 `pool_test.go` 复核；任何内存序、索引公式或槽状态变化都应给出线性化点和回绕证明。
- 若希望支持任意容量，应把当前 `& (cap - 1)` 假设改为经验证的索引策略，并新增非 2 的幂、零容量和最小容量测试；不能只改 `Cap`。
- 若希望无锁池支持 64 位值，需要同时扩大槽值类型、重新评估内存布局/伪共享和性能，并确认 `GlobalAllocator` 的 32/64 位职责是否仍合理。
- 测试应继续放在同目录独立文件 `pool_test.rs` 或 `migration_aster_unit_test.rs`，不要嵌入生产源文件；Go 对齐行为变化还应同步检查 `pool.go`、`pool_test.go` 与上层 `globalconn.rs`/`globalconn_test.rs`。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/globalconn` 列出本 crate 的 Rust/Go 实现与测试。
- RustCodeGraph `node --file pkg/util/globalconn/pool.rs --offset 1 --limit 500`：读取目标文件 437 行全貌，并得到其五个直接使用文件。
- RustCodeGraph 精确查询 `AutoIncPool`、`LockFreeCircularPool`、`InitExt`：确认 Rust/Go 同名符号、测试入口和 `SimpleAllocator` 使用关系；对 `AutoIncPool::Get`、`LockFreeCircularPool::Put/Get` 的 `callers/callees` 查询未返回方法级结果，故调用关系进一步由直接入口源码核验。
- RustCodeGraph 文件节点：读取 `globalconn.rs` 的构造、申请、释放主链，读取 `pool.go`、`pool_test.rs`、`pool_test.go` 和 `migration_aster_unit_test.rs` 的对应实现与边界测试。
- 原始文件核验：`pkg/util/globalconn/Cargo.toml`、`pkg/util/globalconn/lib.rs` 与根 `Cargo.toml`，确认 crate 名称、入口、依赖、公开重导出和 workspace 接线；目标目录没有 `doc.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认本文档存在且恰好包含 11 个固定二级章节，并人工复核唯一新增生产物为本文件。
