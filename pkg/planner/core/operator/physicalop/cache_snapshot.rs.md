# `pkg/planner/core/operator/physicalop/cache_snapshot.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate；crate 根 `lib.rs` 以私有模块 `mod cache_snapshot` 装配它，再用 `pub use cache_snapshot::*` 暴露其公开类型。它位于物理计划与实例级计划缓存之间：`pkg/session/runtime/planning.rs` 在一次新的 SELECT 物理计划通过可缓存性与范围配额检查后调用 `CachedPlan::try_capture`，把结果交给 `pkg/planner/core/plan_cache_utils.rs::NewPlanCacheValue`；缓存命中时则从 `PlanCacheValue::Plan` 取得快照并用当前会话的 `ContextRef` 调用 `CachedPlan::restore`。

因此，这不是缓存容器、淘汰策略或可缓存性检查器，而是“把绑定旧会话的物理计划转为拥有型快照，并在新上下文中重建计划”的转换层。文件中的 DML 入口 `try_capture_plan`/`restore_plan` 也有独立单元测试，但当前搜索到的生产主链只直接使用物理计划入口 `try_capture`/`restore`。

## 核心职责

1. `CachedPlan` 作为递归枚举，对支持的 DML、扫描、点查、选择/投影、排序/限制、聚合、连接、Union 与 Reader 节点进行封闭分派；嵌套 child、probe parent、inner plan、partial plan 继续递归快照。
2. `CachedContext`、`CachedPlanBase`、`CachedSchemaProducer` 和 `CachedPhysicalProperty` 保存所有算子共享的计划元数据、schema、统计、代价、子节点、required property，并在恢复时绑定调用者提供的新上下文。
3. 表达式、列、schema、排序项、聚合描述与比较过滤器不保留原会话对象，而通过 `expression::CachedExpression`、`CachedColumn`、`CachedSchema` 等拥有型表示捕获并借助新 `BuildContext` 恢复。
4. 对无法安全跨会话复用的形态进行显式拒绝，而不是生成不完整快照，例如 DML 外键计划、Insert 的 session-bound runtime table、HashJoin runtime filters、TableScan columnar indexes，以及未知计划类型。
5. 在参数化 LIMIT 恢复时重新读取当前参数值，并保持 cop-side `OFFSET+COUNT` 的派生形状，防止缓存命中仍使用首次编译的参数界限。

## 主要符号

- `CacheSnapshotError(String)`：本模块统一的公开错误类型；实现 `Display` 与 `Error`。内部 `unsupported` 用于给不支持的节点/字段和表达式恢复类型错误附加可读原因。
- `CachedPlan`：主快照枚举。`try_capture_plan(&dyn base::Plan)` 接受 `Update`、`Delete`、`Insert`；`try_capture(&dyn base::PhysicalPlan)` 按具体类型向下转型并选择快照变体；`restore` 返回 `Box<dyn PhysicalPlan>`，`restore_plan` 同时处理 DML 和物理节点。
- `CachedContext`：保存 `StatsInfo`、plan type、ID、query block offset 与 noncacheable reason；`restore` 通过 `NewBasePlan` 重新注入 `ContextRef`。
- `CachedPlanBase`：保存 child required properties、递归 children、两版 cost、probe parents、TiFlash shuffle stream 数、schema、stats、stats table name 与 store type。`try_from_base`/`restore` 是大多数算子共享的递归骨架。
- `CachedSchemaProducer` 与私有 `CachedSimpleSchemaProducer`：分别快照物理 schema producer 与 DML 使用的 simple producer，恢复 schema 和 output names。
- `CachedPhysicalProperty`：覆盖排序、TaskType、ExpectedCnt、MPP 分区、CTE 状态、向量检索、IndexJoin runtime property、partial/advisory order 与 TiFlash 偏好；其内部哈希缓存不被保存，而由恢复后的 `PhysicalProperty::default()` 延迟重建。
- 节点快照类型：`CachedUpdate/Delete/Insert`，`CachedTableDual/TableScan/IndexScan/PointGet/BatchPointGet`，`CachedSelection/Projection/TopN/Limit`，`CachedStreamAgg/HashAgg`，`CachedUnionAll/UnionScan`，`CachedHashJoin/MergeJoin/IndexJoin`，以及四种 Reader。每类 `capture`/`restore` 只复制其对应运行时类型的语义字段。
- `CachedLocalIndexLookup` 与 `CachedIndexHashJoin`：直接克隆其底层 Rust 类型的独立辅助快照。目前它们不属于 `CachedPlan` 枚举的分派路径；其中 `CachedPlan::IndexHashJoin` 使用的是 `CachedIndexJoin + KeepOuterOrder`，不要把两套入口混为一谈。
- `capture_*`/`restore_*` 私有辅助函数：成对转换 assignments、sort/by items、expressions、columns 与 scalar functions；恢复 scalar function/constant 时还会校验动态类型。

## 执行流程

缓存写入路径如下：

1. `pkg/session/runtime/planning.rs` 生成物理计划并完成范围配额与 noncacheable reason 检查。
2. `CachedPlan::try_capture` 依次对已支持具体类型做 `as_any().downcast_ref`；命中后调用对应节点的 `capture`，未知类型返回 `plan <tp> has no cached snapshot variant`。
3. 节点 `capture` 先捕获 `CachedSchemaProducer`/`CachedPlanBase`，再转换本节点表达式、列和元数据；所有 child/inner/reader 子计划递归回到 `CachedPlan::try_capture`。任一层失败通过 `?` 终止整棵计划的缓存写入。
4. 成功快照作为 `PlanCacheValue::Plan` 存入实例计划缓存；失败原因由 session 路径写入 `PlanCacheTracker::SetSkipPlanCache`，本次计划仍可直接执行，只是不进入缓存。

缓存命中路径如下：

1. session 从 `PlanCacheValue::Plan` 取得 `CachedPlan`，传入当前 `plan_context` 调用 `restore`。
2. 枚举分派到具体节点的 `restore`；共享 base 先以新上下文重建 `Plan`，再恢复 required properties、children、probe parents、schema 与统计状态。
3. 表达式与列统一通过当前上下文的 `GetExprCtx()` 恢复，嵌套计划继续使用克隆的 `ContextRef`，最终返回与当前会话绑定的 `Box<dyn PhysicalPlan>`。
4. `CachedLimit::restore` 若记录了参数下标，则调用 `ContextRef::prepared_limit_value` 获取本次绑定值；普通 LIMIT 的 count 使用 `min(u64::MAX - offset)`，cop-side 派生 LIMIT 使用饱和加法。

DML 使用平行流程：顶层调用 `try_capture_plan`，其 select child 仍递归调用物理计划入口；恢复必须用 `restore_plan`。若把 DML 变体传给 `restore`，会得到明确错误。

## 数据与状态

快照以拥有数据为主：字符串、向量、range、表/索引/列元数据和 datum 被 clone；表达式相关对象变成独立的 cached 表示；递归节点用 `Box<CachedPlan>` 或 `Vec<CachedPlan>` 表示。不可变且设计为跨线程共享的对象继续使用引用计数句柄，例如向量检索的 `Arc<VectorFloat32>`、字段名的 `Arc<FieldName>` 与统计直方图句柄。

必须保持的关键不变量包括：

- 恢复后的 `Plan` 使用新 `ContextRef`，但 plan ID、type、query block offset、stats 和 noncacheable reason 与快照一致。
- child 顺序、child required property 顺序、probe parent 顺序及 reader 的 partial plan 顺序均保持不变。
- `CachedSchemaProducer` 同时保留 base schema 与 producer 的可选 schema，不能只恢复其中一层。
- DML 恢复时 `FKChecks`/`FKCascades` 必为空，因为非空形态在 capture 阶段已拒绝；Insert 的 runtime `Table` 同理恢复为 `None`。
- HashJoin 仅在 `RuntimeFilterList` 为空时捕获，恢复后该列表仍为空；TableScan 的 `UsedColumnarIndexes` 遵循相同规则。
- `PhysicalProperty` 的派生哈希缓存不跨上下文复制；恢复的是决定属性语义的字段。

## 依赖与调用关系

上游生产调用者是 `pkg/session/runtime/planning.rs`：未命中分支调用 `CachedPlan::try_capture`，命中分支调用 `CachedPlan::restore`。存储边界在 `pkg/planner/core/plan_cache_utils.rs`，其 `PlanCacheValue` 持有 `Option<physicalop_dependency::CachedPlan>`。crate 根 `lib.rs` 的再导出使 session 和 planner core 无需访问私有模块名。

下游依赖可分四层：

- `base`/`baseimpl`：`Plan`、`PhysicalPlan`、`ContextRef`、`NewBasePlan` 与共享计划行为。
- `expression`/`aggregation`：表达式、列、schema、assignment、scalar function 与聚合描述的上下文无关快照和恢复。
- `property`/`costusage`/`statistics`/`kv`：required property、统计、代价和存储类型。
- 当前 crate 的具体物理算子及 `planner_util`、`ranger`、`model`、`types`、`tipb`：重建节点字段与访问范围。

`Cargo.toml` 将该目录定义为独立 crate（`lib.rs`，`autotests = false`），上述依赖均为显式 workspace path 依赖，`tipb` 是固定 revision 的 Git 依赖。测试由 `lib.rs` 中的 `#[cfg(test)] mod cache_snapshot_test` 和 `cache_snapshot_contract_test` 接入，而不是 Cargo 自动发现。

## 错误处理与边界

所有可失败转换返回 `Result<_, CacheSnapshotError>`，递归与集合转换使用 `collect::<Result<...>>()`/`transpose()` 保证部分成功不会被当作完整快照。明确拒绝边界有：

- 未知 `Plan`/`PhysicalPlan` 具体类型；
- Update/Delete/Insert 含 foreign-key checks 或 cascades；
- Insert 含 session-bound runtime table；
- PhysicalHashJoin 含 runtime filter 实例；
- PhysicalTableScan 含 columnar index 状态；
- integer handle 没有列、common handle 缺少表/索引元数据；
- cached scalar function 或 comparison constant 恢复成错误的表达式动态类型；
- 当前上下文无法提供参数化 LIMIT 的绑定值；
- 调用错误恢复入口，例如对 DML 快照调用 `restore`。

边界之外的类型不是自动浅拷贝兜底；必须新增显式变体和双向转换。`CachedLocalIndexLookup`、`CachedIndexHashJoin` 虽有独立 round-trip API，但不表示 `CachedPlan::try_capture` 能分派到它们。代码也未在本文件内执行 plan cache eligibility、内存计量、缓存淘汰或 SQL 执行。

## 并发与资源生命周期

快照自身不创建锁、线程、任务、channel、事务或 I/O 资源。生命周期分为 capture 后的缓存驻留期和每次命中的 restore 期：驻留对象不持有原 session `ContextRef`，恢复时才接收并克隆当前上下文。`cache_snapshot_test.rs::cache_snapshot_base_types_are_send_and_sync` 对基础快照及主要节点做编译期 `Send + Sync` 约束，`PlanCacheValue` 的运行时计数则在另一文件用原子字段维护。

跨线程安全依赖两类约束：可变/会话绑定表达式被转换为拥有型 cached 表示；继续共享的 `Arc` 或统计句柄必须是不可变且 `Send + Sync`。扩展字段时，不能仅因其实现 `Clone` 就认定可跨 session 共享，还必须审查其内部上下文、可变缓存和资源句柄。

## 与 Go 版本的对应关系

直接 Go 基线是同目录 `plan_clone_generated.go` 的 25 个 `CloneForPlanCache(newCtx)` 实现。Rust `lib.rs` 中 `CACHE_SNAPSHOT_PLAN_CONTRACT` 逐项记录 Go 类型、Rust 类型和非浅拷贝字段，`cache_snapshot_contract_test.rs::go_cacheable_plan_inventory_is_complete` 直接读取生成的 Go 文件，验证数量、顺序和类型映射没有漂移。

语义上两边都以“换新上下文、深拷贝会话敏感字段、遇到不安全形态则拒绝”为核心。Rust 没有复用 Go 的对象内 `CloneForPlanCache` 方法，而是拆成不可变 `Cached*` 数据与 `restore` 两阶段，以便 `PlanCacheValue` 跨线程持有。常见对应包括：Go 的递归 `CloneForPlanCache(newCtx)` 对应 Rust 的递归 `CachedPlan::try_capture` + `restore(context)`；Go 的 `CloneExpressions/ColumnsForPlanCache` 对应 `CachedExpression/CachedColumn`；Go 返回 `(nil, false)` 的拒绝分支对应带原因的 `CacheSnapshotError`。

两端并非字段布局逐字相同。Go reader 恢复派生的 flatten 列表、自引用等对象内状态；Rust 只保存规范化的 raw 子计划，并由 Rust 类型当前字段布局重建。Rust 还支持 `LegacyPhysicalLock` 的 `SelectLock` 快照，而它不在这份 25 项 Go 生成清单中；反过来 Go 清单中的 local index lookup 在 Rust 当前仅有独立 `CachedLocalIndexLookup`，未进入 `CachedPlan` 枚举。新增或删除可缓存节点时必须同时审查这些差异，不能只让清单测试通过。

## 扩展指南

新增一种可缓存物理节点时，至少应：

1. 在本文件增加拥有型 `CachedXxx`，逐字段判定 clone、cached expression 转换、派生字段重建或显式拒绝；不要把原 `ContextRef` 放入快照。
2. 为 `CachedPlan` 增加变体，在 `try_capture`（DML 则是 `try_capture_plan`）和 `restore`/`restore_plan` 中完成对称分派；同时确认所有嵌套计划使用递归快照。
3. 若对应 Go `CloneForPlanCache` 清单发生变化，同步 `lib.rs::CACHE_SNAPSHOT_PLAN_CONTRACT` 及 `cache_snapshot_contract_test.rs`，并记录 Go/Rust 特有节点。
4. 在独立的 `cache_snapshot_test.rs` 增加 round-trip、拒绝边界、新上下文重绑定和 `Send + Sync` 断言；不要把测试写入本生产文件。参数化或派生字段必须使用不同于首次 capture 的恢复上下文值验证。
5. 检查 `pkg/session/runtime/planning.rs` 的入口是否确实能到达新类型，以及 `PlanCacheValue` 的内存估计/可缓存性检查是否需要配套调整。

主要风险是漏复制会影响执行或 explain 的字段、错误共享 session-bound 数据、遗漏递归 child、恢复派生字段不完整，以及 Rust 支持矩阵与 Go 生成清单漂移。范围/range、表达式、统计和 reader flatten 状态还可能带来内存与性能变化，应优先复用现有 cached 表示而不是引入整棵原计划克隆。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`query CacheSnapshot` 定位 `CacheSnapshotError`、`CachedPlan` 相关 capture；`node --file pkg/planner/core/operator/physicalop/cache_snapshot.rs` 分段核对了 1–2516 行的类型、分派和双向转换。方法 ID 的 `callers/callees` 查询出现名称误匹配且 callers 为空，因此按技能规则用文本搜索补足调用边，未采用错误图结果。
- 主链证据：`pkg/session/runtime/planning.rs` 的缓存未命中分支调用 `CachedPlan::try_capture`，命中分支调用 `CachedPlan::restore`；`pkg/planner/core/plan_cache_utils.rs::PlanCacheValue` 持有快照，`NewPlanCacheValue` 接收快照。
- crate 证据：`pkg/planner/core/operator/physicalop/Cargo.toml` 与 `lib.rs`；后者负责模块装配、公开再导出、25 项 Go/Rust 契约和独立测试模块接线。
- Go 对照：`pkg/planner/core/operator/physicalop/plan_clone_generated.go` 的 25 个 `CloneForPlanCache` 实现；契约测试 `cache_snapshot_contract_test.rs::go_cacheable_plan_inventory_is_complete` 校验该清单。
- Rust 测试：`cache_snapshot_test.rs` 覆盖 DML/外键拒绝、四类 Reader、leaf scan/point get、base/schema/context、join、selection、agg/union、参数/字段 round-trip 及 `Send + Sync`；`physical_table_scan_test.rs` 另验证 columnar indexes 被拒绝。
- 本任务是纯文档分析，按计划未运行 Cargo。交付结构检查要求本文恰有“文件定位”至“验证依据”11 个固定二级标题。
