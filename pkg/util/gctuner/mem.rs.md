# `pkg/util/gctuner/mem.rs`

## 文件定位

本文件是 `astersql-util-gctuner` crate 的进程堆内存适配层，由 [`lib.rs`](lib.rs) 公开为 `mem` 模块。它把 [`task-memory`](../memory/memstats.rs) 提供的跨平台内存快照收敛成调谐器需要的单一 `heap_inuse` 指标，并为 Rust 周期驱动补充“请求系统分配器归还空闲页”的平台边界。crate 归属、依赖别名和 Go 包映射见 [`Cargo.toml`](Cargo.toml)：`task-memory` 实际指向同仓库的 `astersql-util-memory`，迁移来源是 Go `pkg/util/gctuner`。

文件只有两个公开函数 `readMemoryInuse` 与 `releaseUnusedMemory`，以及按目标平台条件编译的三个 C 分配器符号；不保存调谐策略，也不直接设置 GOGC 或 memory limit。策略分别位于 [`tuner.rs`](tuner.rs) 和 [`memory_limit_tuner.rs`](memory_limit_tuner.rs)。

## 核心职责

1. `readMemoryInuse` 每次强制取得新快照并返回 `MemStats.heap_inuse`，避免调谐决策复用旧缓存。这个数值作为 GOGC 阈值计算和 memory-limit 触发判定的共同输入。
2. `releaseUnusedMemory` 在支持的平台调用进程系统分配器的回收接口：macOS 使用 malloc zone pressure relief，glibc Linux 使用 `malloc_trim(0)`；其他目标明确返回 `false`，表示没有接入真实回收边界。
3. 隔离平台差异。上层 finalizer 只调用布尔接口，不需要持有 C 指针或了解 libc ABI；内存采样的 macOS、glibc Linux 与其他平台差异则继续由 `task_memory::memstats::ForceReadMemStats` 负责。

## 主要符号

- `pub fn readMemoryInuse() -> u64`（[`mem.rs`](mem.rs)）：调用 `ForceReadMemStats()`，读取返回值的 `heap_inuse`。名称保留 Go 风格；crate 根通过 `#![allow(non_snake_case)]` 接受这种迁移期 API 命名。
- `pub fn releaseUnusedMemory() -> bool`（[`mem.rs`](mem.rs)）：执行平台分配器回收并报告“当前目标是否支持”。这里的布尔值不是分配器实际释放字节数，也不是回收成功码。
- `malloc_default_zone() -> *mut c_void` 与 `malloc_zone_pressure_relief(zone, goal) -> usize`：仅在 macOS 编译。实现向默认 malloc zone 传入目标值 `0`，请求尽可能释放空闲页；返回的字节数当前未使用。
- `malloc_trim(pad: usize) -> i32`：仅在 `target_os = "linux"` 且 `target_env = "gnu"` 时编译。实现传入 `0`，不要求保留额外的顶部空闲空间；返回码当前未使用。
- `ForceReadMemStats`：来自 `task_memory::memstats` 的下游函数。它调用平台采样逻辑并更新进程级缓存，然后把本次 `MemStats` 快照按值返回。

## 执行流程

内存读取链如下：

1. [`tuner.rs`](tuner.rs) 的 `Tuner::tuning` 或 [`memory_limit_tuner.rs`](memory_limit_tuner.rs) 的 `MemoryLimitTuner::tuning` 进入一次调谐。
2. 上层调用 `readMemoryInuse`。
3. `readMemoryInuse` 调用 `ForceReadMemStats`；后者在 [`../memory/memstats.rs`](../memory/memstats.rs) 中执行 `sample_heap`、刷新缓存并返回快照。
4. 本文件只取 `heap_inuse`。GOGC 调谐器把它传给 `calcGCPercent(inuse, threshold)`；memory-limit 调谐器用 `heap_inuse * (1 + GOGC / 100)` 与当前 limit 比较。

空闲页回收链独立于上述读链：[`finalizer.rs`](finalizer.rs) 的运行时驱动每 300 ms 超时唤醒一次，先调用 `releaseUnusedMemory`，再调用 `Finalizer::run` 执行调谐回调。macOS 分支依次取得默认 zone 并调用 pressure relief；glibc Linux 分支调用 `malloc_trim(0)`；其他平台不执行 FFI 并返回 `false`。`return true` 位于各条件编译块内，末尾的 `false` 通过 `#[allow(unreachable_code)]` 同时兼容支持和不支持的平台编译结果。

## 数据与状态

本文件没有静态可变状态、结构体或长期持有的资源。`readMemoryInuse` 返回字节数快照，生命周期止于调用者的当轮计算；它不返回对 `task-memory` 全局缓存的引用。

`heap_inuse` 的平台含义由 [`../memory/memstats.rs`](../memory/memstats.rs) 定义：macOS 以所有 malloc zones 的 `size_allocated` 近似分配器保留堆，glibc Linux 以 `arena + hblkhd` 表示保留区域，其他目标退化为进程 RSS，并同时填入 `heap_alloc` 与 `heap_inuse`。因此它是对 Go `runtime.MemStats.HeapInuse` 的尽可能接近的跨平台指标，不保证不同分配器之间逐字节等价。

`releaseUnusedMemory` 的返回值只编码能力：支持分支固定为 `true`，不支持分支固定为 `false`。macOS API 返回的释放量和 glibc API 返回的成功码均被丢弃，所以调用者不能从该返回值判断本轮是否真的释放了页面。

## 依赖与调用关系

RustCodeGraph 对 `readMemoryInuse` 的直接生产调用边显示：

- [`tuner.rs`](tuner.rs) 的 `Tuner::tuning -> readMemoryInuse`：根据当前占用与阈值重算 GOGC。
- [`memory_limit_tuner.rs`](memory_limit_tuner.rs) 的 `MemoryLimitTuner::tuning -> readMemoryInuse`：估算下一次 GC 的堆触发量，并决定是否进入 memory-limit 的两阶段调整。

测试调用者包括 [`mem_test.rs`](mem_test.rs) 的 `test_mem`、`memory_inuse_matches_force_read_mem_stats` 与 `allocator_collection_boundary_is_connected`，[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `migration_memory_probe_reports_live_process_memory`，以及 [`tuner_test.rs`](tuner_test.rs) 的 `test_tuner`。

`releaseUnusedMemory` 的生产调用边是 [`finalizer.rs`](finalizer.rs) 的 `Finalizer::startRuntimeDriver -> releaseUnusedMemory`。它向下调用 macOS 的 `malloc_default_zone`、`malloc_zone_pressure_relief`，或 glibc 的 `malloc_trim`。`Cargo.toml` 中没有为这些符号声明额外 Rust crate；它们通过目标系统 C ABI 链接。采样侧唯一直接 crate 依赖是 `task-memory`。

## 错误处理与边界

- 两个公开函数都不返回 `Result`，也没有业务错误传播。`readMemoryInuse` 继承 `ForceReadMemStats` 的降级规则：非 macOS/非 glibc Linux 读取不到进程时以 `0` 构造快照，而不是报错。
- `releaseUnusedMemory` 不检查底层返回值。即使 `malloc_trim` 返回 `0` 或 pressure relief 释放 `0` 字节，只要平台分支已编译，函数仍返回 `true`；该契约应解释为“边界已接线”。
- musl Linux、Windows 及其他目标不会调用上述 C API，返回 `false`。上层当前忽略此布尔值，因此调谐回调仍会继续运行。
- FFI 调用封装在小范围 `unsafe` 块中。macOS 指针只在同一调用中传给 zone API，glibc 调用不保存参数；本文件不取得内存所有权，也不负责释放指针。
- 快照是瞬时观测。采样完成后其他线程仍可分配或释放内存，调用者不能把返回值当成事务一致的上限。

## 并发与资源生命周期

本文件自身不创建线程、不持锁。`readMemoryInuse` 间接通过 `task-memory` 的进程级 `RwLock<Option<GlobalMemStats>>` 更新缓存；其写锁中毒时会恢复内部值而不是 panic，因此并发调用被序列化为各自的新快照。

实际周期生命周期由 [`finalizer.rs`](finalizer.rs) 管理：`newFinalizer` 启动后台线程，线程以 `Weak<Finalizer>` 避免延长所有者生命周期，收到 `stop` 通知或无法升级弱引用时退出；每个超时周期先请求分配器回收，再在 finalizer 的回调互斥锁下运行调谐。系统分配器操作作用于进程级分配器状态，不绑定某个 `Finalizer` 对象，且本文件不会保留 zone 指针。

扩展平台实现时必须维持并发可调用性：上层可能存在 GOGC 与 memory-limit 两套 finalizer 驱动，不能引入需要调用者串行化的局部资源，也不能让回收路径长期阻塞回调线程。

## 与 Go 版本的对应关系

Go [`mem.go`](mem.go) 的 `readMemoryInuse` 调用 `memory.ForceReadMemStats()` 并返回 `HeapInuse`；Rust 同名函数逐项保持这一语义。Go [`mem_test.go`](mem_test.go) 分配约 100 MiB 并断言读数至少为 100 MiB；Rust [`mem_test.rs`](mem_test.rs) 同样分配大块内存，但先记录基线、按页触碰，并允许约 5 MiB 误差，以适配真实进程与多平台分配器观测。

Go 文件没有 `releaseUnusedMemory`。这个函数是 Rust runtime 适配所需的局部接线：Go [`finalizer.go`](finalizer.go) 借助 tracing GC finalizer 获得每次 GC 后的回调，Rust 没有等价钩子，因此 [`finalizer.rs`](finalizer.rs) 用周期线程主动请求系统分配器回收后再触发回调。它不应被描述为 Go `mem.go` API 的直接移植，也不能据此认为 Rust 已复刻 Go runtime 的精确 GC 时序。

另一个可见差异是底层指标来源：Go 由运行时直接提供 `runtime.MemStats.HeapInuse`；Rust 由 `task-memory` 按平台读取 malloc 统计或回退 RSS。上层公式保持一致，但采样口径和观测时间可能不同。

## 扩展指南

- 新增内存指标时，优先扩展 [`../memory/memstats.rs`](../memory/memstats.rs) 的 `MemStats` 与平台采样，再决定本文件是否需要为调谐器暴露窄接口；不要在两个 tuner 中复制平台探测。
- 新增分配器或目标平台回收能力时，在 `releaseUnusedMemory` 增加精确的 `cfg` 分支、最小 FFI 声明和安全性说明，并在 [`mem_test.rs`](mem_test.rs) 扩展能力断言。必须确认目标 ABI、线程安全性、返回值语义和链接方式。
- 若要把返回值升级为“本轮确实释放了内存”，必须同步修改 `releaseUnusedMemory` 的契约、[`finalizer.rs`](finalizer.rs) 的调用处理和独立测试；不能继续无条件返回 `true`。
- 改动采样语义时，应同步检查 `Tuner::tuning`、`MemoryLimitTuner::tuning` 及其独立测试，因为把 `heap_alloc`、RSS 或缓存值误当成 `heap_inuse` 会改变 GOGC 和 memory-limit 的触发点。
- Rust 单元测试继续放在独立的 [`mem_test.rs`](mem_test.rs)，不要内嵌回生产文件。性能风险主要是强制采样和分配器回收的调用频率；兼容风险主要是不同 libc/allocator 的符号可用性与统计口径。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点；目标目录查询确认 `mem.rs` 有 8 个符号，并被 `mem_test.rs`、`memory_limit_tuner.rs`、`migration_aster_unit_test.rs`、`tuner.rs`、`tuner_test.rs` 使用。
- RustCodeGraph `node readMemoryInuse`：确认 Rust 定义位于 `mem.rs:36`，直接调用者包含两个 `tuning` 方法及三个测试文件；`node releaseUnusedMemory` 与 `callees`：确认其调用三个平台 FFI 符号，直接调用者包含 finalizer 模块导入及 `allocator_collection_boundary_is_connected`。生产调用点由 `finalizer.rs` 的 `startRuntimeDriver` 源码核实。
- 已核读 Rust 文件：[`mem.rs`](mem.rs)、[`lib.rs`](lib.rs)、[`mem_test.rs`](mem_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、[`tuner.rs`](tuner.rs)、[`memory_limit_tuner.rs`](memory_limit_tuner.rs)、[`finalizer.rs`](finalizer.rs)、[`../memory/memstats.rs`](../memory/memstats.rs)。
- 已核读配置与 Go 对照：[`Cargo.toml`](Cargo.toml)、[`mem.go`](mem.go)、[`mem_test.go`](mem_test.go)、[`tuner.go`](tuner.go)、[`memory_limit_tuner.go`](memory_limit_tuner.go)、[`finalizer.go`](finalizer.go)。
- 独立测试所证明的边界：100 MiB 活动分配相对基线可见；`readMemoryInuse` 与刚刷新后缓存的 `heap_inuse` 相等；macOS 与 glibc Linux 的回收边界返回支持；迁移聚合测试再次覆盖活动分配可见性。
- 本任务是纯文档分析，按计划不运行 Cargo。最终结构检查要求本文恰有十一个固定二级标题；链接、重要符号与平台条件另做人工复核。
