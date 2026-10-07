# `pkg/executor/internal/applycache/apply_cache.rs`

## 文件定位

本文件实现 Apply 算子的结果缓存：用编码后的相关外层行作为键，保存该键对应的内层执行结果 `chunk::List`。它属于 workspace crate `astersql-executor-internal-applycache`，由同目录 `lib.rs` 作为私有模块载入并将全部公开符号重新导出；crate 的边界和依赖见 `pkg/executor/internal/applycache/Cargo.toml`。

在完整 SQL 执行链中，这类缓存服务于相关子查询的 Nested Loop Apply，避免相同相关列值反复执行内层计划。Go 生产路径已经在 `pkg/executor/parallel_apply.go` 的 `ParallelNestedLoopApplyExec.Open` 和 `pkg/executor/join/hash_join_v1.go` 的 `NestedLoopApplyExec.Open` 中构造缓存，并在工作循环中调用 `Get`/`Set`。截至本次检索，Rust workspace 和 `pkg/executor`、`pkg/executor/join` 的 Cargo 清单虽已声明该 crate，但 Rust 生产源码没有直接调用 `NewApplyCache`、`ApplyCache::Get` 或 `ApplyCache::Set`；直接 Rust 使用者仅见同目录独立测试。因此，本文件是已实现且有测试的迁移组件，不能据此声称 Rust Apply 主链已接入它。

## 核心职责

- `ApplyCache` 封装非线程安全的 `kvcache::SimpleLRUCache`，让每次底层 LRU 操作都经过互斥锁。
- `ApplyCacheKey` 保留 Go 命名 `[]byte` 键的类型身份，并通过 `kvcache::Key::Hash` 提供按字节内容查找的哈希键。
- `applyCacheKVMem` 按“键字节长度 + `chunk::List` 自身 Tracker 当前消耗”估算条目内存。
- `NewApplyCache` 从窄接口 `ApplyCacheContext` 读取 `MemQuotaApplyCache`，创建独立内存 Tracker，并关闭底层 LRU 自带的实际容量控制。
- `Get` 读取并刷新 LRU 近因顺序；`Set` 在会话配额内插入，必要时逐项淘汰最久未使用条目。
- `GetMemTracker` 暴露缓存 Tracker，让执行器把它挂到语句/执行器的父 Tracker；Go 两个 `Open` 路径均展示了这一生命周期接线。

## 主要符号

- `trait ApplyCacheContext { fn MemQuotaApplyCache(&self) -> i64; }`：构造函数所需的最小会话边界。它没有直接依赖尚未统一的 Rust `sessionctx::Context`，调用方需提供适配实现。
- `struct ApplyCache`：持有 `Mutex<Box<SimpleLRUCache>>`、独立 `memory::Tracker` 和构造时固定的 `mem_capacity`。字段均为私有，外部只能通过公开方法维持不变量。
- `struct ApplyCacheKey(Vec<u8>)`：公开 newtype、私有载荷；支持从 `Vec<u8>` 和 `&[u8]` 构造，也支持 `AsRef<[u8]>`。其固有方法 `Hash` 返回底层字节的克隆，`kvcache::Key` 实现转发到该方法。
- `applyCacheKVMem(&ApplyCacheKey, &chunk::List) -> i64`：公开的记账辅助函数。它不计算 `List` 结构体自身、`Arc`、LRU 节点等管理开销，与 Go 当前算法一致。
- `NewApplyCache(&C) -> Result<ApplyCache, Infallible>`：以 `usize::MAX`、guard `0.1`、quota `0` 创建底层 LRU。quota 为 `0` 表示底层只按条目数控制，而近乎无限的条目容量使实际淘汰由 `ApplyCache::Set` 的 `mem_capacity` 决定。
- 私有 `get`、`put`、`removeOldest`：分别对单次 `SimpleLRUCache::Get`、`Put`、`RemoveOldest` 加锁。
- `Get(ApplyCacheKey) -> Result<Option<Arc<chunk::List>>, Infallible>`：未命中为 `Ok(None)`；命中时将类型擦除值 downcast 回共享的 `chunk::List`。
- `Set(ApplyCacheKey, Arc<chunk::List>) -> Result<bool, Infallible>`：条目过大或无法腾出空间时返回 `Ok(false)`；插入成功返回 `Ok(true)`。
- `GetMemTracker(&self) -> &memory::Tracker`：返回借用，不转移 Tracker 所有权。

文件没有模块级业务常量、枚举、异步函数或条件编译项；`#![allow(dead_code, non_snake_case)]` 用于保留 Go API 命名和当前尚未完整接线的符号。

## 执行流程

构造流程如下：调用方实现 `ApplyCacheContext`；`NewApplyCache` 读取一次 `MemQuotaApplyCache`；随后建立容量为 `usize::MAX` 的 `SimpleLRUCache` 和标签为 `LabelForApplyCache`、自身无硬限制（`-1`）的 Tracker。配额值保存在 `mem_capacity`，后续不会随会话变量变化。

读取流程为：`Get` 把键借给私有 `get`；`get` 锁住 LRU 并调用其 `Get`；底层命中会把节点移到 LRU 的最近使用端；未命中直接返回 `None`。命中值是 `Arc<dyn Any + Send + Sync>`，随后 downcast 为 `Arc<chunk::List>`，因此调用者拿到的是写入时同一共享对象，而不是复制出的列表。

写入流程为：

1. `Set` 用 `applyCacheKVMem` 计算新条目大小。
2. 若单条大小大于 `mem_capacity`，不修改缓存和 Tracker，返回 `false`。
3. 若“新条目大小 + 当前 Tracker 消耗”超限，反复调用 `removeOldest`。
4. 每次淘汰都从类型擦除的键和值恢复 `ApplyCacheKey` 与 `Arc<chunk::List>`，并从 Tracker 扣除该条目的估算值。
5. 若 LRU 已空却仍无法满足条件，返回 `false`；否则先给 Tracker 增加新条目估算值，再调用 `put` 插入并返回 `true`。

底层 `SimpleLRUCache::Put` 对同哈希键执行原地替换并刷新近因性，而不是新增节点。当前 `Set` 在调用 `Put` 前无条件增加新条目内存，且没有先扣除旧值；所以重复设置同一键会使本层 Tracker 高估占用。这与 Go 文件当前流程一致，但扩展或调用时不应把它误解成精确的替换记账。

## 数据与状态

键的逻辑身份是完整字节序列：`SimpleLRUCache` 以 `Key::Hash()` 返回的 `Vec<u8>` 作为 `HashMap` 键。`ApplyCacheKey::Hash` 每次克隆字节，因此查询、插入和淘汰记账均会产生临时分配；它不是密码学哈希。

值以 `Arc<chunk::List>` 进入接口，再被隐式提升为类型擦除的 `kvcache::Value`。缓存和命中调用者共享同一个 `List` 所有权；本文件不复制行数据，也不冻结列表内容。

`mem_capacity` 是构造快照。`mem_tracker.BytesConsumed()` 只由成功插入时的正向 `Consume(mem)` 和显式 LRU 淘汰时的负向 `Consume(...)` 更新。本文件没有清空/关闭方法，也没有为缓存析构时显式归零 Tracker；对象释放依靠 Rust 字段和 `Arc` 的析构。由于记账取自写入/淘汰时 `List` Tracker 的当前值，若共享 `List` 在缓存期间继续增减内存，其变化不会由本文件自动校正。

`SimpleLRUCache::Get` 会刷新近因性，`RemoveOldest` 从最久未使用端移除。这一不变量由 `migration_aster_unit_test.rs` 的 `get_refreshes_lru_before_memory_driven_eviction` 直接覆盖。

## 依赖与调用关系

直接下游依赖如下：

- `crate::kvcache`：由 `lib.rs` 重新导出 `astersql-util-kvcache`；提供 `Key`、`KeyRef`、`Value`、`SimpleLRUCache` 和 `NewSimpleLRUCache`。`pkg/util/kvcache/simple_lru.rs` 明确说明该缓存非线程安全，并定义命中刷新、同键替换和最旧项淘汰语义。
- `crate::chunk`：由 `lib.rs` 重新导出 `astersql-util-chunk::List`；是缓存值及其内存 Tracker 的来源。
- `crate::memory`：由 `lib.rs` 重新导出 `astersql-util-memory` 的 Tracker API 和 `LabelForApplyCache`。
- `crate::syncutil::Mutex`：实际为 `parking_lot::Mutex`，保护底层 LRU。
- 标准库 `Arc` 和 `Infallible`：分别承担共享所有权/类型擦除 downcast，以及保留与 Go `(value, error)` 形状相近但当前不产生可恢复错误的 API。

上游证据分两层：RustCodeGraph 定位到本文件 18 个符号，并确认 `NewApplyCache`、`applyCacheKVMem` 等定义；限定 Rust 源码搜索只找到 `apply_cache_test.rs` 与 `migration_aster_unit_test.rs` 的直接调用。Go 对照的生产调用者则是 `ParallelNestedLoopApplyExec` 和 `NestedLoopApplyExec`：它们在 `Open` 时构造缓存并把缓存 Tracker 挂到执行器 Tracker，在处理相关外层行时按编码键查询，未命中执行内层计划后写回。

## 错误处理与边界

公开构造、读、写接口的错误类型均为 `Infallible`，所以当前正常路径只通过 `Option` 或 `bool` 表达未命中/拒绝写入，不会返回业务错误。这样保留了 Go API 的错误槽位，但调用者不能依赖它报告分配失败或类型错误。

以下情况具有明确边界：单条估算值严格大于配额时返回 `false`；等于配额时允许写入并可能淘汰全部旧项；负配额会拒绝所有非负大小条目；零配额只有估算值为零的条目可能通过。缓存空而 Tracker 状态仍显示无法容纳新项时，淘汰失败并返回 `false`。

两个 `expect` 链是内部类型不变量的强制检查，不是可恢复错误：命中或淘汰若缺少值会 panic；缓存若混入非 `chunk::List` 值也会 panic。字段私有且 `put` 只接收 `Arc<chunk::List>`，正常公开 API 能维持该不变量。

内存估算并非真实堆占用：它遗漏容器/节点开销，且 Go 测试也保留了 `chunk::List` Tracker 精度不足的 TODO。因此配额是与 Go 兼容的逻辑配额，不是严格的进程内存上限。

## 并发与资源生命周期

`SimpleLRUCache` 的每次 `Get`、`Put`、`RemoveOldest` 都由同一个 `parking_lot::Mutex` 串行化；返回值使用 `Arc`，所以缓存释放或淘汰后，已命中的调用者仍可持有列表。测试 `TestApplyCacheConcurrent` 和 `concurrent_get_and_set_preserve_go_handoff_behavior` 用两个线程反复交接两个键，证明该使用模式没有数据竞争并保持最终记账。

锁的粒度是“单次底层操作”，不是整个 `Set` 事务：大小计算、读取 Tracker、可能的多次淘汰、Tracker 更新和最终 `Put` 之间会释放锁。多个线程同时 `Set` 时可能基于相同旧消耗作决定，因而不能把 `mem_capacity` 视为强并发上限；这一结构与 Go 版本一致。若未来要求严格配额，应把 LRU 状态和记账决策放入同一临界区，并新增并发同写/不同键竞争测试。

缓存不创建线程、任务、通道或事务，也没有显式 `Close`。其生命周期由拥有 `ApplyCache` 的执行器控制；底层条目随缓存析构释放，条目值则在所有 `Arc` 引用释放后才真正析构。父子 Tracker 的挂接不在本文件完成，而应由上层执行器在构造后调用 `GetMemTracker` 处理。

## 与 Go 版本的对应关系

Rust 结构逐项对应 `pkg/executor/internal/applycache/apply_cache.go`：Go 的 `cache`、`memTracker`、`memCapacity`、`lock` 分别对应 Rust 的加锁 `cache`、`mem_tracker`、`mem_capacity`（锁合并包裹缓存）；Go `applyCacheKey []byte` 对应 Rust newtype；`applyCacheKVMem`、构造、三个私有 LRU 包装器和三个公开方法保持相同算法顺序。

有意的语言适配包括：Rust 用 `ApplyCacheContext` 代替完整 `sessionctx.Context`；返回拥有所有权的 `ApplyCache` 而不是指针；键按值传入并使用 `Arc<chunk::List>`；动态值通过 `Arc::downcast` 恢复类型；Go 的 `(nil, nil)` 未命中对应 `Ok(None)`；当前永不产生的 Go `error` 对应 `Infallible`。

语义证据也保持一致：`apply_cache_test.rs` 翻译 Go 的容量淘汰和双 goroutine 交接测试；`migration_aster_unit_test.rs` 额外覆盖键哈希、超大条目拒绝、Tracker 数值和 Get 刷新 LRU。Rust 的测试没有内嵌在生产文件，符合仓库要求。

当前迁移差异是接线状态而非缓存算法：Go 两条 Apply 执行路径直接使用该包；Rust `parallel_apply.rs` 通过 `ParallelApplyRuntimeContext::{InitializeApplyCache, ApplyCacheGet, ApplyCacheSet}` 抽象缓存能力，尚未直接引用本 crate，`join/hash_join_v1.rs` 中相关构造仅见注释。因此接入 Rust 主链前仍需提供上下文适配与值形态转换，不能仅添加一个 import 就视为完成。

## 扩展指南

若调整配额或淘汰策略，主要修改点是 `NewApplyCache` 和 `Set`，并同步 `apply_cache_test.rs` 与 `migration_aster_unit_test.rs` 的超大条目、精确容量、LRU 刷新和并发用例。若改变内存估算，必须同步 `applyCacheKVMem`、淘汰扣减逻辑及 Go 对照，特别核对 `chunk::List` Tracker 是否仍是兼容口径。

若要支持严格并发配额、同键替换的准确记账或缓存值在插入后增长，应先定义清楚记账不变量，再让决策、LRU 修改和 Tracker 修改共享一个临界区；相应测试应放在独立 `*_test.rs` 文件，覆盖并发双写、同键替换、值 Tracker 变化和空缓存/异常 Tracker 状态。

若接入 Rust Apply 主链，应在运行时上下文实现处适配 `ApplyCacheContext`，把编码后的相关列 `Vec<u8>` 转为 `ApplyCacheKey`，把执行器的 Chunk 集合转换为本 crate 使用的 `chunk::List`，并把 `GetMemTracker` 挂到执行器父 Tracker。需要同步验证缓存开关、命中率统计、执行器 Open/Close 生命周期，以及 `parallel_apply.rs` 的抽象接口与该具体实现之间不会重复记账。

若扩大动态值类型，必须重构当前 `Arc::downcast::<chunk::List>` 的强不变量；不要仅绕过 `expect`，否则会把编程错误静默变成缓存未命中。若要让构造或操作真正返回错误，再将 `Infallible` 替换为仓库统一错误类型并补齐上层传播测试。

## 验证依据

- 目标源码：`pkg/executor/internal/applycache/apply_cache.rs`，核对全部 18 个索引符号、公开/私有边界、流程和 panic 点。
- crate 边界：`pkg/executor/internal/applycache/Cargo.toml` 与 `lib.rs`；根 `Cargo.toml`、`pkg/executor/Cargo.toml`、`pkg/executor/join/Cargo.toml` 用于确认 workspace 与依赖声明。
- 下游实现：RustCodeGraph `query SimpleLRUCache`、`query NewSimpleLRUCache` 及 `node --file pkg/util/kvcache/simple_lru.rs`，核对非线程安全、命中刷新、同键替换和最旧项淘汰。
- 调用图：RustCodeGraph `query ApplyCache`、`query NewApplyCache`、`query applyCacheKVMem`、`query ApplyCacheKey`；精确 `callers`/`callees` 对 impl 方法未返回边，继而用限定 `rg` 搜索确认 Rust 直接使用仅存在于独立测试，并记录未接线事实。
- Go 对照及生产入口：`pkg/executor/internal/applycache/apply_cache.go`、`pkg/executor/parallel_apply.go`、`pkg/executor/join/hash_join_v1.go`。
- 测试证据：`pkg/executor/internal/applycache/apply_cache_test.rs`、`migration_aster_unit_test.rs` 及 Go `apply_cache_test.go`；`main_test.rs`/`main_test.go` 仅提供包测试环境，不改变缓存算法。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务给定命令验证本文恰有 11 个固定二级标题，并人工复核“存在目的、运行过程、安全扩展”三类问题均可由上述路径反查。
