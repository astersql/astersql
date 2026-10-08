# `pkg/session/plan_cache_runtime.rs`

## 文件定位

`plan_cache_runtime.rs` 属于 `astersql-session` crate；crate 根在 `pkg/session/Cargo.toml` 中以 `lib.rs` 声明，`pkg/session/lib.rs` 公开导出 `plan_cache_runtime` 模块，并把独立测试文件 `plan_cache_runtime_test.rs` 仅在测试构建中接入。该文件本身不执行 SQL，也不实现缓存算法，而是集中定义 Prepare/Execute 计划缓存运行时在 session 层交换的四个数据结构。

直接使用点位于 `pkg/session/runtime.rs` 及其子模块：`runtime/planning.rs` 负责创建和消费这些结构，`runtime/session.rs` 把预处理状态与最近一次计划快照保存在会话状态中，`runtime/typed_adapter_bridge.rs` 把同一份物理计划交给标准执行适配器。因而它位于“预处理语句状态/规划器计划缓存”与“KV 或适配器执行”之间的数据边界，而不是缓存实现的入口。

## 核心职责

- `ProcessPlanSnapshot` 保存一次已绑定物理计划的可观察摘要：算子名称和索引范围字符串。`runtime/planning.rs::collect_process_plan_snapshot` 负责遍历物理计划并填充它。
- `PreparedPlannedKVResult` 是物化执行 API 的返回值，把结果行、投影列名、缓存命中标志、告警和计划快照作为一个整体交给调用者。
- `PreparedKVPhysicalPlan` 表示“参数已绑定、但尚未读取 KV”的规范物理计划。它同时携带执行所需限制和缓存收尾状态，使直接 KV 路径和 typed adapter 路径共享同一套缓存键、参数类型推断、范围回退及缓存准入判断。
- `PreparedPlannedKVSelect` 是 session 内部的 Prepare 状态，保存 AST、绑定的 InfoSchema、`PlanCacheStmt` 以及参数个数；其可见性为 `pub(crate)`，不会成为 crate 外 API。

该文件刻意只承载状态契约：缓存键构造、命中恢复、优化、范围容量检查、实际扫描和运行时指标更新均由相邻模块完成。依据为四个结构均只有字段和派生实现，没有函数或 `impl` 块。

## 主要符号

- `pub struct ProcessPlanSnapshot`：派生 `Clone + Debug + Default + Eq + PartialEq`。`Operators: Vec<String>` 按计划树遍历顺序记录 `tp()` 或索引扫描的 `PlanCacheTP()`；`IndexRanges: Vec<String>` 只为可识别的 `PhysicalIndexScan` 记录 `PlanCacheRangeString()`，所以两个向量不保证等长。
- `pub struct PreparedPlannedKVResult`：派生 `Clone + Debug`。`Rows` 使用 `astersql_executor_sortexec::Row`；`Columns` 保持投影顺序；`FromPlanCache` 仅描述本次规划是否命中；`Warnings` 只收集本次规划开始之后新增的 statement warnings；`Plan` 是本次参数绑定后的快照。
- `pub struct PreparedKVPhysicalPlan`：不派生 `Clone`，因为 `Plan` 是独占的 `Box<dyn PhysicalPlan>`，并且 `PendingCache` 必须通过 `take()` 只消费一次。公开字段 `Plan`、`FromPlanCache`、`Warnings`、`Snapshot`、`SelectLimit`、`SQLText` 供执行层读取；crate 内字段 `CachedValue` 与 `PendingCache` 供收尾逻辑更新命中项或提交新缓存项。
- `pub(crate) struct PreparedPlannedKVSelect`：`Ast: ast::NodeRef` 和 `InfoSchema: Arc<dyn InfoSchema>` 允许规划时共享语法树及元数据快照；`Statement: PlanCacheStmt` 提供 SQL 文本、参数标记、schema version、数据库和可缓存属性；`ParameterCount` 在规划前执行严格数量校验。
- 文件级 `#![allow(non_snake_case)]` 保留从 Go 移植而来的公开字段命名风格；文件没有常量、trait、函数、`impl`、条件编译项或异步任务。

## 执行流程

1. `ConcreteSession::PreparePlannedKVSelect`（`pkg/session/runtime/planning.rs`）只接受恰好一个 `SELECT`，解析参数标记，构造 `PlanCacheStmt`，随后把 `PreparedPlannedKVSelect` 放入 `RuntimeSessionState::prepared_planned` 并返回递增的 statement id。
2. `ConcreteSession::PlanPreparedPlannedKVSelect` 按 id 取得 AST，注册 MDL，检查参数数量，选择事务可见的 InfoSchema，并从表元信息获得 table id/缓存表状态。未知 id、AST 不可用、参数数量不匹配或表不存在都会在产生物理计划前返回错误。
3. 规划函数根据数据库、schema version、分区裁剪模式、隔离读引擎、`sql_select_limit`、事务状态、字符集与排序规则等构造 `PlanCacheKeyContext`，由 `NewPlanCacheKey` 生成键；再从实参推断字段类型，并用十六进制键和参数类型查询实例计划缓存。
4. 命中时从 `PlanCacheValue.Plan` 恢复一棵绑定当前上下文和参数的独立物理计划；未命中时通过 `NewPlanBuilder`、`buildResultSetNode` 与 `DoOptimize` 构建新计划。随后 `collect_process_plan_snapshot` 生成 `ProcessPlanSnapshot`。
5. 规划函数检查物理计划是否可缓存以及 range 是否超过 `RangeMaxSize`。首次生成且满足条件时只把新条目放入 `PreparedKVPhysicalPlan::PendingCache`，不立即写缓存；命中项则放在 `CachedValue`。同时会话更新 `last_plan_from_cache` 和 `process_plan_snapshot`。
6. 直接路径 `ExecutePreparedPlannedKVSelect` 使用 `KVRetrieverTableSource` 执行计划，按 `SelectLimit` 截断行，调用 `FinishPreparedKVPhysicalPlan`，再组装 `PreparedPlannedKVResult`。
7. 标准适配器路径先由 `SessionBoundAdapterOwner::BindPreparedPlannedKVSelect` 保存 `BoundPhysicalPlan::Prepared` 并编码扫描范围；`ExecutePreparedPlannedKVSelectThroughAdapter` 从 `PreparedResultMetadata` 取得命中状态、告警与快照，执行 `ExecStmt` 并物化行。扫描结束时 `FinalizePreparedExecution` 仅在成功时调用 `FinishPreparedKVPhysicalPlan`，失败时丢弃 `PendingCache`。
8. `FinishPreparedKVPhysicalPlan` 对命中项调用 `PlanCacheValue::UpdateRuntimeInfo`；对首次计划用 `PendingCache.take()` 将条目写入实例缓存。此设计把缓存准入推迟到成功执行之后，并保证待写条目最多提交一次。

## 数据与状态

`PreparedPlannedKVSelect` 的生命周期从 Prepare 开始，归属于单个 `ConcreteSession` 的 `RuntimeSessionState::prepared_planned: HashMap<u64, ...>`。AST 与 InfoSchema 使用 `Arc`/引用计数共享，但 map、statement id、参数数量和 `PlanCacheStmt` 均由会话状态管理。

`PreparedKVPhysicalPlan` 是一次执行的所有权容器。`Plan` 是为当前参数恢复或新优化出的独占计划；`Snapshot` 是规划完成后的值快照；`SelectLimit` 和 `SQLText` 固化本次执行元信息。`CachedValue: Option<Arc<PlanCacheValue>>` 让多个会话命中同一实例缓存值并以原子方式累计运行信息；`PendingCache: Option<(String, PlanCacheValue)>` 则在首次执行成功前保持私有。

`ProcessPlanSnapshot` 还会被克隆到 `RuntimeSessionState::process_plan_snapshot`，由 `ConcreteSession::ProcessPlanSnapshot` 返回。因此结果里的 `Plan` 与会话“最近一次计划”是相同内容的独立值，不共享可变容器。

`PreparedPlannedKVResult` 是已经物化的返回对象，不持有执行器、KV snapshot、锁或 session 借用。其 `Rows`/`Columns`/`Warnings`/`Plan` 均由结果拥有，适合跨调用边界读取。

## 依赖与调用关系

crate 边界由 `pkg/session/Cargo.toml` 验证：本文件直接使用 `astersql-parser-ast`、`astersql-planner-core`、`astersql-planner-core-base`、`astersql-infoschema` 和 `astersql-executor-sortexec`，这些都在 `astersql-session` 的普通依赖中，而不是仅测试依赖；文件自身只额外依赖标准库 `Arc`。

主要上游为：

- `runtime/planning.rs::PreparePlannedKVSelect` 构造 `PreparedPlannedKVSelect`。
- `runtime/planning.rs::PlanPreparedPlannedKVSelect` 构造 `PreparedKVPhysicalPlan` 和 `ProcessPlanSnapshot`。
- `runtime/planning.rs::{ExecutePreparedPlannedKVSelect, ExecutePreparedPlannedKVSelectThroughAdapter}` 构造 `PreparedPlannedKVResult`。
- `runtime/typed_adapter_bridge.rs::{BoundPhysicalPlan, PreparedResultMetadata, BindPreparedPlannedKVSelect}` 保存或读取 `PreparedKVPhysicalPlan`。
- `runtime/scan_adapter_runtime.rs::FinalizePreparedExecution` 负责成功提交或失败丢弃待写缓存项。

主要下游不是由本文件调用，而是由这些字段表达：AST/InfoSchema 输入规划器；`PlanCacheStmt`、`PlanCacheValue` 与实例缓存组成缓存协议；`PhysicalPlan` 输入 KV executor 或标准 `ExecStmt`；`Row` 是物化结果格式。RustCodeGraph 将该文件的直接使用文件识别为 `plan_cache_runtime_test.rs`、`runtime.rs`、`runtime/planning.rs` 和 `runtime/typed_adapter_bridge.rs`；文本引用还表明状态字段与成功/失败收尾分别在 `runtime/session.rs`、`runtime/scan_adapter_runtime.rs`。

## 错误处理与边界

本文件没有 `Result` 返回值，也不主动生成错误；错误语义由构造这些结构的入口决定。已验证的边界包括：Prepare 必须是单条 `SELECT`；statement id 必须存在；AST 节点必须仍可访问；实参数量必须等于 `ParameterCount`；目标表必须存在于规划用 InfoSchema；缓存键、计划恢复、逻辑计划构建、优化和索引 range 字符串生成的错误都会被包装为带阶段上下文的 `SessionError`。

缓存命中不是执行成功的保证：命中计划仍可能在恢复、扫描、行转换或结果关闭时失败。适配器收尾因此只在 `success == true` 时提交 `PendingCache`，失败则显式 `take()` 丢弃；直接执行路径只在物理执行成功后调用完成函数。

range 超过 `tidb_opt_range_max_size` 是可恢复的缓存回退而非查询失败：规划器记录告警、关闭本次缓存准入并继续使用生成的物理计划。`PreparedPlannedKVResult::Warnings` 使调用者能够观察该决定。`usize` 扫描行数转为 `i64` 时饱和到 `i64::MAX`，`SelectLimit` 转为 `usize` 时也以 `usize::MAX` 为上限，避免窄平台溢出。

`ProcessPlanSnapshot::IndexRanges` 仅覆盖可下转为 `PhysicalIndexScan` 的节点，不能被当作完整执行计划序列化；它是诊断/测试快照。`PreparedKVPhysicalPlan::SQLText` 当前由规划阶段保存，但本文件不保证每条执行路径都会读取该字段。

## 并发与资源生命周期

此文件没有锁、线程、异步任务或通道。并发安全来自所有权边界：每次规划得到独占 `Box<dyn PhysicalPlan>`；命中缓存通过 `CachedPlan::restore` 产生当前参数的计划，避免多个会话共享可变 range。`plan_cache_runtime_test.rs::prepared_index_range_cache_rebuilds_without_fallback` 验证同一 Domain 的另一会话可命中实例缓存，同时用自己的参数重建 range。

共享的 InfoSchema 和命中缓存值使用 `Arc`。Go 对照的 `PlanCacheValue::UpdateRuntimeInfo` 使用原子计数；Rust 完成路径通过共享 `Arc<PlanCacheValue>` 调用对应更新接口。实例缓存本体在 `runtime.rs::runtime_instance_plan_cache` 中以 Domain 为键共享，本文件只携带引用，不管理缓存锁或淘汰。

资源阶段明确分为 Prepare、Plan/Bind、Execute、Finish：`PendingCache` 在成功完成时移动进缓存，在失败时丢弃；命中项在完成时累计扫描信息。typed adapter 会把 `PreparedKVPhysicalPlan` 保留到 result close/执行收尾，因此延迟读取期间计划和待写缓存项仍有效。`PreparedPlannedKVResult` 返回前已关闭或完成 record set，不延长底层执行资源生命周期。

## 与 Go 版本的对应关系

仓库不存在 `pkg/session/plan_cache_runtime.go`，因此不能声称 Rust 四个结构与某个 Go 文件逐项对应。`pkg/session/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/session"` 表明 crate 的整体 Go 来源；职责上的直接对照分散如下：

- Rust `PreparedPlannedKVSelect::Statement` 对应 Go `pkg/session/session.go::ExecutePreparedStmt` 取得并校验的 `*plannercore.PlanCacheStmt`，两者都把已 Prepare 描述交给执行路径。
- Rust `PlanPreparedPlannedKVSelect` 的“预处理、键生成、参数类型匹配、命中调整或生成新计划”流程，对应 Go `pkg/planner/core/plan_cache.go::GetPlanFromPlanCache`。Rust 当前聚焦计划型 KV `SELECT`，不是 Go 通用 prepared/non-prepared 执行入口的完整替代。
- Rust `CachedValue`/`PendingCache` 对应 Go `PlanCacheValue` 与 plan cache 的查找、生成和写入职责；Rust 把新条目延迟到执行成功后准入，这是本运行时适配层显式表达的生命周期。
- Rust `FinishPreparedKVPhysicalPlan` 对命中项调用 `UpdateRuntimeInfo`，语义可由 Go `pkg/planner/core/plan_cache_utils.go::PlanCacheValue.UpdateRuntimeInfo` 核对：累计执行次数、processed/total keys、延迟并更新时间。当前 Rust 调用把 scanned rows 同时作为 processed/total keys，latency 传 0，这是已实现事实，不代表 Go 完整统计粒度均已移植。
- `ProcessPlanSnapshot` 和 `PreparedPlannedKVResult` 是 Rust 运行时为了测试、适配器及诊断组合出的显式 DTO；没有发现 Go 同名结构。

因此扩展时应以行为契约对齐为准，不能仅按字段名称推断 Go 等价物；尤其不能把 Rust 的实例缓存专用路径描述成 Go session/instance 两种策略的完整覆盖。

## 扩展指南

新增影响缓存键的 session 环境时，应修改 `runtime/planning.rs::PlanPreparedPlannedKVSelect` 的 `PlanCacheKeyContext` 构造，并同步检查 `astersql-planner-core::NewPlanCacheKey`；遗漏字段会造成不兼容环境错误复用计划。测试应继续放在独立的 `pkg/session/plan_cache_runtime_test.rs` 或对应 `runtime_test` 文件，不要把测试内嵌到本源文件。

新增需要跨规划与执行传递的元数据时，优先判断它属于：Prepare 级状态（加到 `PreparedPlannedKVSelect`）、单次物理计划状态（加到 `PreparedKVPhysicalPlan`）、最终物化结果（加到 `PreparedPlannedKVResult`）还是诊断快照（加到 `ProcessPlanSnapshot`）。同时更新两条执行路径：直接 KV 路径和 `SessionBoundAdapterOwner` typed adapter 路径，防止行为分叉。

修改缓存准入必须保持“先执行成功、后提交”的不变量，并覆盖 `PendingCache.take()` 的单次消费和失败丢弃。修改命中计划时必须继续恢复/克隆计划，不能让多个会话共享可变 range。相关回归测试至少应覆盖首次 miss、后续 hit、跨会话参数重绑定、执行失败不准入、range 超额告警与不准入。

扩展计划快照时，应修改 `collect_process_plan_snapshot` 并说明遍历顺序和哪些节点产生额外数据；不要假设 `Operators` 与 `IndexRanges` 一一对应。新增公开字段还需评估 crate 外 API 兼容性及克隆成本，尤其是行集、字符串和大型计划诊断数据。

性能风险集中在计划克隆/恢复、快照字符串生成、参数类型匹配与大 range；正确性风险集中在缓存键遗漏、InfoSchema 版本、事务/隔离读环境和失败执行误准入；兼容性风险集中在 Go 完整计划缓存语义与 Rust 当前计划型 KV `SELECT` 子集之间的差异。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引可用（11,467 files、307,296 nodes、1,848,419 edges）；`node --file pkg/session/plan_cache_runtime.rs` 读取完整 76 行源码，并报告四个直接使用文件；对 `ProcessPlanSnapshot`、`PreparedPlannedKVResult`、`PreparedKVPhysicalPlan`、`PreparedPlannedKVSelect` 执行了 `query`、`callers`、`callees`。类型节点没有函数调用边，因此再按图未覆盖规则核对直接文本使用点。
- 源码与装配：`pkg/session/plan_cache_runtime.rs`；`pkg/session/lib.rs` 的模块声明与独立测试声明；`pkg/session/runtime.rs` 的导入；`pkg/session/runtime/session.rs::RuntimeSessionState`；`pkg/session/runtime/planning.rs::{collect_process_plan_snapshot, PreparePlannedKVSelect, PlanPreparedPlannedKVSelect, FinishPreparedKVPhysicalPlan, ExecutePreparedPlannedKVSelect, ExecutePreparedPlannedKVSelectThroughAdapter}`；`pkg/session/runtime/typed_adapter_bridge.rs::{BoundPhysicalPlan, PreparedResultMetadata, BindPreparedPlannedKVSelect}`；`pkg/session/runtime/scan_adapter_runtime.rs::FinalizePreparedExecution`。
- crate/依赖：`pkg/session/Cargo.toml` 的 `[package]`、`[lib]`、`[features]`、`[package.metadata.porting]` 及相关普通依赖。`nextgen` feature 不对本文件做条件编译。
- Rust 独立测试：`pkg/session/plan_cache_runtime_test.rs` 中覆盖索引 reader/lookup、跨会话缓存恢复、参数绑定不读取 KV、成功后延迟准入、range 超额跳过缓存、编译内存配额释放的测试；另核对 `pkg/session/runtime_test/typed_adapter_bridge.rs`、`pkg/session/tests/paging_rpc.rs`、`pkg/session/tests/system_session.rs` 和 `pkg/session/runtime/scan_adapter_runtime_test.rs` 的直接调用。
- Go 对照：`pkg/session/session.go::{GetSessionPlanCache, ExecutePreparedStmt, rebuildFromPrepareCache}`；`pkg/planner/core/plan_cache.go::{GetPlanFromPlanCache, lookupPlanCache, clonePlanForInstancePlanCache}`；`pkg/planner/core/plan_cache_utils.go::{PlanCacheValue, UpdateRuntimeInfo}`。没有发现同路径 `pkg/session/plan_cache_runtime.go`，故文档只作职责级对照。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工复核唯一新增生产物、无源代码修改、无整段源码复制以及所有“已支持”结论均有上述路径或符号依据。
