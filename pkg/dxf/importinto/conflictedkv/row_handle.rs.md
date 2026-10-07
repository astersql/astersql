# `pkg/dxf/importinto/conflictedkv/row_handle.rs`

## 文件定位

本文件属于 `astersql-dxf-importinto-conflictedkv` crate；crate 入口 `pkg/dxf/importinto/conflictedkv/lib.rs` 通过 `mod row_handle` 加载它并用 `pub use row_handle::*` 导出其公开项。它服务于 IMPORT INTO 的 collect-conflicts 阶段：处理唯一索引冲突时，同一物理行可能被多个索引 KV 指向，因而需要在内存预算内记录已经成功处理的完整数据行键，避免重复计算原行 checksum 和重复写出冲突行。包级背景及该去重目的见 `pkg/dxf/importinto/conflictedkv/doc.go`。

直接生产调用链是 `pkg/dxf/importinto/collect_conflicts.rs::CollectConflictGroup` → `pkg/dxf/importinto/conflictedkv/collector.rs::NewCollector` → `pkg/dxf/importinto/conflictedkv/handler.rs::IndexKVHandler`。本文件只维护行键集合和过滤判定，不负责解析索引、读取快照、解码行、计算 checksum 或写对象存储。

## 核心职责

- `BoundedKeySet` 保存完整的 `astersql_kv::Key` 字节。使用完整行键而不是只存 handle，确保分区表中“相同 handle、不同物理表 ID”的行不会被错误合并（`BoundedKeySet::Add`、`Contains`）。
- 多个集合通过同一个 `Arc<AtomicI64>` 共享近似内存计数；达到 `size_limit` 后，后续 `Add` 静默跳过，避免索引冲突行去重集合无限增长（`NewBoundedKeySet`、`BoundExceeded`）。
- `KeyFilter` 分离两种作用域：只读的 global 集合代表更早 KV 组已经处理的行；互斥保护的 local 集合代表当前 worker 在此前批次中成功处理的行（`KeyFilter::{isHandledGlobally,isHandledLocally,addLocal}`）。
- `Merge` 把 worker 的已保留行键并入更大作用域，但不重复收费；调用侧在 `ConflictCollector::MergeRowKeysInto` 和 `CollectConflictGroup` 汇总 worker 结果。

内存上限只控制后续记录能力，不保证所有重复行都被过滤。上限到达后，调用链仍可继续处理行；`collectConflictsStepExecutor::onFinished` 会把 `BoundExceeded()` 传给 `applyCollectResult`，让上层标记 checksum 去重能力已受限。

## 主要符号

- `pub struct BoundedKeySet { shared_size, size_limit, keys }`：非线程安全的本地 `HashSet<Vec<u8>>` 加跨集合共享的原子字节计数。集合所有权本身不共享可变访问；需要共享可变访问时由调用方包裹 `Mutex`。
- `pub fn NewBoundedKeySet(shared_size: Arc<AtomicI64>, limit: i64) -> BoundedKeySet`：构造集合。共享计数已达到上限时以容量 0 建表，否则用 128 作为容量提示；容量提示不改变逻辑上限。
- `pub fn BoundedKeySet::Add(&mut self, key: &Key)`：若预算已耗尽则直接返回；否则按“键字节数 + Go string header 大小 + bool 大小”增加共享计数，并克隆完整键字节插入集合。
- `pub fn BoundedKeySet::Contains(&self, key: &Key) -> bool`：按完整字节精确查询。
- `pub fn BoundedKeySet::Merge(&mut self, other: Option<&Self>)`：`None` 为空操作；`Some` 时克隆并扩展所有键，不再次调整共享计数。
- `pub fn BoundedKeySet::BoundExceeded(&self) -> bool`：以 Acquire 顺序读取共享计数，判定 `shared_size >= size_limit`。
- `pub fn BoundedKeySet::{Len,SharedSize}`：分别暴露去重后的键数和跨集合共享计数，当前主要用于独立测试和外部验证。
- `pub struct KeyFilter { global, local }`：global 为 `Arc<BoundedKeySet>`，local 为 `Arc<Mutex<BoundedKeySet>>`。
- `pub fn NewKeyFilter(...) -> KeyFilter`：组装两个作用域。
- `KeyFilter::{isHandledGlobally,isHandledLocally,addLocal}`：包内方法，分别查询 global、加锁查询 local、加锁写入 local。

文件没有 trait、模块常量、条件编译项或异步函数。公开命名保留 Go 风格，crate 根的 `#![allow(non_snake_case)]` 明确允许这种移植接口。

## 执行流程

1. `collectConflictsStepExecutor::resetForNewSubtask` 创建共享 `AtomicI64` 预算和 global `BoundedKeySet`，上限为子任务内存容量的一半。
2. `CollectConflictGroup` 为每个 worker 创建共享同一计数器的 local `BoundedKeySet`；`NewCollector` 对 data KV 组直接建立 `DataKVHandler`，只对 index KV 组用 global/local 集合建立 `KeyFilter` 并传给 `NewIndexKVHandler`。
3. `IndexKVHandler::HandleOne` 从索引 KV 解出 table ID 和 handle，再编码出包含物理表身份的 row key。若 `isHandledGlobally` 为真，说明更早的 KV 组已处理该行，当前索引项直接成功返回。
4. 未被 global 过滤的 row key 被缓冲；`handleBufferedHandles` 批量从快照读取对应数据行。对每个实际返回的行，先用 `isHandledLocally` 跳过当前 worker 之前批次已处理的键。
5. 未跳过的行完成解码，并由 `encodeAndHandleRow` 调用下游回调。只有回调成功后才执行 `addLocal`；若解码或回调报错，该键不会登记，后续重试仍可处理它。此顺序由 `handler_test.rs::index_key_is_registered_only_after_successful_row_callback` 直接验证。
6. worker 结束时，`ConflictCollector::MergeRowKeysInto` 把其 local 集合复制到返回集合；`CollectConflictGroup` 再把各 worker 集合合并。组间串行执行时，调用方可把完成组的键集合并到 global，使后续唯一索引组跳过同一行。
7. 任意 `Add` 若观察到共享预算已达到上限，就不再保存新键。已有键仍保留并可参与查询和合并。

## 数据与状态

`keys: HashSet<Vec<u8>>` 的身份语义是完整编码数据行键；同一字节键重复 `Add` 不增加 `Len`，但当前实现会在插入前增加 `shared_size`，因此重复登记仍会收费。这与 Go `addStr` 先执行 `sharedSize.Add(delta)`、再写 map 的顺序一致，是保守预算而非精确的 `HashSet` 堆内存测量。

单次收费 `delta = key.len() + size_of::<&str>() + 1`。`size_of::<&str>()` 在 64 位目标上对应 Go string 的两字长 header，额外 1 字节对应 Go `bool`；`row_handle_test.rs::entry_shallow_size` 固定用 17 验证 Go 兼容预算，避免误用 Rust `String` 的三字长布局。该估算不包含 Rust `HashSet` bucket、分配器或 `Vec` capacity 的真实开销。

`shared_size` 是多个 local/global 集合可共同观察的单调计数；本文件没有减法或释放时回退计数的逻辑。`Merge` 不收费的前提是被合并的键先前已通过共享计数器收费；生产路径为所有 worker 传入同一个 `Arc<AtomicI64>`。若扩展调用方合并来自不同计数器的集合，`SharedSize` 将不再代表合并后集合的预算，调用方必须明确处理这一不变量。

## 依赖与调用关系

直接外部依赖只有 `astersql_kv::Key`；`Cargo.toml` 将其声明为同 workspace 路径依赖 `../../../kv`。其余依赖均来自标准库：`HashSet`、`Arc`、`Mutex`、`AtomicI64` 和 `Ordering`。crate 的 Cargo 元数据声明 Go 对照包为 `pkg/dxf/importinto/conflictedkv`。

主要上游：

- `collect_conflicts.rs::collectConflictsStepExecutor` 持有 global 集合、共享计数和上限，并根据 `BoundExceeded` 生成最终截断状态。
- `collect_conflicts.rs::CollectConflictGroup` 创建每 worker local 集合、启动 scoped worker，并在 join 后合并集合。
- `collector.rs::NewCollector` 创建 `KeyFilter`；`ConflictCollector::MergeRowKeysInto` 导出 worker 已成功处理的键。
- `handler.rs::IndexKVHandler::{HandleOne,handleBufferedHandles}` 分别消费 global 和 local 查询，并在成功回调后写 local。

主要下游：`BoundedKeySet` 只调用 `Key` 的公开字节字段、`HashSet` 操作和原子计数；`KeyFilter` 只调用 `BoundedKeySet` 并管理 local mutex。RustCodeGraph 的文件节点还报告目标文件被 `scheduler.rs` 和 `conflict_resolution_test.rs` 使用；精确源码引用显示实际生产构造集中在 `collect_conflicts.rs`/`collector.rs`，`conflict_resolution_test.rs` 则是跨模块流程验证。

## 错误处理与边界

本文件 API 不返回 `Result`。达到或超过预算时 `Add` 静默不写；`Merge(None)` 静默不操作；空集合查询返回 false。上限为 0 或负数时，初始 `shared_size` 通常已满足 `>= limit`，因此集合从创建起拒绝新增键。

`KeyFilter` 的 local mutex 使用 `lock().unwrap()`：若另一个持锁线程 panic 并毒化 mutex，查询或新增会继续 panic，而不是转成业务错误。global 集合通过不可变 `Arc` 分享，构造 `KeyFilter` 后无法由该引用修改。

预算检查与计数增加是两个独立原子操作：多个 worker 可同时在检查时看到“未超限”，随后各自 `fetch_add`，所以最终计数允许越过上限一个或多个条目的大小。这是软上限；一旦后续调用观察到 `>= limit`，新增就停止。`Merge` 不检查上限，因为它汇总已经保留且已经收费的键。

## 并发与资源生命周期

共享预算由 `Arc<AtomicI64>` 管理，读取使用 Acquire，增加使用 AcqRel；它负责跨 worker 可见性和无锁计数，但不把“检查上限 + 预留预算”变成一个不可分割事务。`BoundedKeySet` 自身没有内部锁且要求可变引用才能 `Add`/`Merge`，与 Go 注释“set is not goroutine safe”一致。

global 集合以 `Arc<BoundedKeySet>` 只读共享；worker local 集合以 `Arc<Mutex<BoundedKeySet>>` 同时供 handler 登记和 collector 在结束时导出。`CollectConflictGroup` 使用 scoped threads，关闭输入、等待所有 worker join 后才合并返回集合，因此不会在 worker 仍写 local 时把它提升到下一组。`Arc` 和容器析构负责释放内存；共享计数不会随集合析构递减，因为它记录子任务期间累计预算，而非当前存活分配。

本文件不开线程、不持有 I/O、事务、通道或快照资源；这些生命周期分别由 `CollectConflictGroup`、`ConflictCollector` 和 `IndexKVHandler` 管理。

## 与 Go 版本的对应关系

Rust 文件直接移植自 `pkg/dxf/importinto/conflictedkv/row_handle.go`：`KeyFilter`、`BoundedKeySet`、128 的初始容量提示、完整 row key 身份、共享原子预算、重复 Add 收费、无二次收费的 Merge 及 `>=` 上限判定均保持一致。

有以下明确差异：

- Go 允许在 nil `*KeyFilter` 接收者上调用三个方法并返回 false/空操作；Rust 不允许空对象，调用侧以 `Option<KeyFilter>` 和 `is_some_and` 表达同一语义。`row_handle_test.rs::test_key_filter` 验证 `None` 不过滤。
- Go local 集合字段为普通指针并依赖外部并发约束；Rust `KeyFilter` 用 `Arc<Mutex<_>>` 显式同步 handler 与 collector 的共享可变访问。
- Go 同时提供 string 形式的 `addStr`/`containsStrKey`；Rust 调用链统一传 `Key`，未暴露这两个辅助方法。
- Go 在首次跨越上限时记录包含大小信息的日志，并保留刚导致越界的键；Rust 同样保留导致计数达到/越过上限的键，但没有 logger 和该日志。
- Go `BoundExceeded` 带 `mockKeySetSizeLimit` failpoint；Rust 没有此 failpoint。Rust 独立测试通过显式传入小 limit 覆盖边界。
- Rust 额外提供 `Len`、`SharedSize` 观测方法，主要支撑独立测试和跨 crate 流程断言。

Go 测试还直接验证分区 handle 编码为不同物理 row key；Rust 本文件的身份存储语义支持该性质，但对应编码行为属于 codec/tablecodec，不在本文件实现。

## 扩展指南

- 改变键身份时，应修改 `Add`/`Contains` 作为一个整体，并确认仍包含物理表 ID；同步更新独立文件 `row_handle_test.rs` 的 global/local 身份测试，以及 Go 对照测试中的分区行键案例。不要把测试内嵌回生产源文件。
- 改变预算算法时，应同时审查 `NewBoundedKeySet`、`Add`、`BoundExceeded` 和 `collectConflictsStepExecutor::onFinished`；保持 Go header 兼容还是改为真实 Rust 堆内存估算必须作为兼容决策，并增加重复键、恰好达到上限和并发越界测试。
- 若要让上限成为硬上限，需要用 CAS/预留循环把检查与增加合并，并定义大于剩余预算的单键是否保留；这会改变当前 Go 软上限语义和最终 checksum 截断行为，不能只改原子 Ordering。
- 若改变合并来源，必须保证 `Merge` 两侧共享同一计数器，或为跨预算合并设计重新计费；否则 `BoundExceeded` 会低估实际保留键。
- 若增加可恢复的锁错误处理，应调整 `KeyFilter` 方法签名和 `IndexKVHandler` 的错误传播；当前 `unwrap` 的 panic 契约不能在调用侧无感改变。
- 行处理成功时机相关修改应同步 `handler_test.rs::index_key_is_registered_only_after_successful_row_callback`，并保留“失败回调不登记、成功后才登记”的不变量。

## 验证依据

- RustCodeGraph `status`：索引可用，共 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件节点显示 96 行、18 个符号，并报告由 `collect_conflicts.rs`、`collector.rs`、`scheduler.rs`、`conflict_resolution_test.rs` 使用。
- RustCodeGraph `query`：核对了 Rust/Go 两侧 `BoundedKeySet`、`NewBoundedKeySet`、`KeyFilter`、`NewKeyFilter`、`isHandledGlobally`、`isHandledLocally`、`addLocal` 的定义位置；精确 callers/callees 请求未在 30 秒内返回，因此调用边再用局部源码引用核验，未把超时结果当作完整调用图。
- 生产源码：`row_handle.rs`；直接入口与调用边：`lib.rs`、`collector.rs::{NewCollector,MergeRowKeysInto}`、`handler.rs::IndexKVHandler::{HandleOne,handleBufferedHandles}`、`collect_conflicts.rs::{collectConflictsStepExecutor,CollectConflictGroup}`。
- crate 边界：`pkg/dxf/importinto/conflictedkv/Cargo.toml`，其中 `[lib] path = "lib.rs"`、porting 元数据和 `astersql-kv` 路径依赖与本文描述一致。
- Go 对照：`pkg/dxf/importinto/conflictedkv/row_handle.go`；包级业务语义：同目录 `doc.go`。
- 独立测试：`row_handle_test.rs` 验证 None 过滤器、精确达到上限、超限静默跳过、空合并、共享计数和 global/local 分离；`handler_test.rs::index_key_is_registered_only_after_successful_row_callback` 验证登记时机；`conflict_resolution_test.rs` 覆盖 `CollectConflictGroup` 的多 worker 非空快照流程。Go 对照边界来自 `row_handle_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；最终仅运行任务指定的 11 章节结构检查，并人工检查关键陈述均可回溯到上述符号或文件。
