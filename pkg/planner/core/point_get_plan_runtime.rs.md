# `pkg/planner/core/point_get_plan_runtime.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate，由 `pkg/planner/core/lib.rs` 以公开模块 `point_get_plan_runtime` 导出，并依赖同一 crate 的逻辑计划类型、`astersql-planner-core-base` 的计划 trait、`astersql-expression` 的表达式/元数据以及 `astersql-planner-core-operator-physicalop` 的 `PointGetPlan`；这些边界由 `pkg/planner/core/Cargo.toml` 的 `base-dependency`、`expression-dependency`、`logicalop-dependency` 和 `physicalop-dependency` 声明确认。

它不是完整的 Rust 点查规则集：`pkg/planner/core/point_get_plan.rs` 另有面向简化 `FastQuery` 的 `TryFastPlan`。本文件的唯一公开入口 `TryFastIntegerPointGet` 则从已解析、已构建的逻辑 `SELECT` 树直接产生可执行的 `physicalop_dependency::PointGetPlan`。当前生产调用点只有 `pkg/session/runtime/planning.rs` 的 `ConcreteSession::OptimizeRootPlanIDForTest`，用于观测真实根计划 ID，所以它是测试/观测链路中的快路径桥接，不是普通 SQL 执行入口。

## 核心职责

`TryFastIntegerPointGet(context, logical) -> Option<Box<dyn PhysicalPlan>>` 承担三项联合职责：

1. 用严格的计划形状和元数据条件判定是否可以跳过通用物理优化；只接受 `LogicalProjection -> LogicalSelection -> DataSource` 且每层恰有一个子节点的树。
2. 从唯一的等值条件中提取整型主键 handle，并拒绝所有可能改变 Go 语义或吞掉剩余谓词的情形。
3. 创建真实 `PointGetPlan`，安装表元数据、handle、投影列、访问条件、schema 和输出名，并在创建前重置计划 ID。

失败不是错误；任一资格检查不满足都返回 `None`，让调用者继续走 `DoOptimize` 通用优化路径。

## 主要符号

- `TryFastIntegerPointGet`：文件中唯一自定义符号与唯一公开 API。它接收共享的 `ContextRef` 和借用的 `dyn LogicalPlan`，成功时返回装箱的 `dyn PhysicalPlan`。
- `LogicalProjection`、`LogicalSelection`、`DataSource`：通过 `LogicalPlan::as_any` 连续向下转型的三层输入节点，其 `Children`、`Conditions`、`TableInfo`、`AllConds` 和 `PushedDownConds` 构成资格检查的主体。
- `expression::ScalarFunction`、`expression::Column`、`expression::Constant`：将唯一条件限制为 `EQ(主键列, 常量)` 或交换参数顺序的对称形式。
- `expression::types::KindInt64` / `KindUint64`：唯一可接受的常量 Datum 类别；结合主键的 unsigned 标志完成无损转换。
- `physicalop_dependency::PointGetPlan::New`：创建 `TypePointGet` 物理计划，其类型定义在 `pkg/planner/core/operator/physicalop/physical_batch_point_get.rs`。
- `PlanContext::reset_plan_id`：在分配 `PointGetPlan` 的基类 ID 前将计数器归零，使快路径根节点 ID 为 1；会话实现见 `SessionPlanContext::reset_plan_id`。

本文件没有模块常量、自定义 struct/enum/trait/impl，也没有条件编译项。

## 执行流程

1. 将根节点转型为 `LogicalProjection`，要求其唯一子节点是 `LogicalSelection`，再要求 selection 的唯一子节点是 `DataSource`。任一转型或单子节解构失败即返回 `None`。
2. 检查表与数据源：表必须以主键作 handle，不能是分区表；`AllConds` 和 `PushedDownConds` 必须为空；所有列必须非生成列且处于 `StatePublic`；数据库不能是 `information_schema`、`performance_schema` 或 `metrics_schema`。
3. 在 `TableInfo.Columns` 中定位带主键标志的列，并要求 selection 恰有一个条件。该条件必须是名称为 `ast::EQ` 的 `ScalarFunction`，且恰有两个参数。
4. 用对称的 `Column + Constant` / `Constant + Column` 匹配抽取等值两边。列 ID 必须等于主键列 ID；常量不能是 deferred expression，也不能是预处理参数标记。
5. 按符号属性转换 Datum：有符号主键接受 `i64`，也接受不超过 `i64::MAX` 的 `u64`；无符号主键接受非负 `i64` 或任意 `u64`。其它 Datum 类型回退通用 range builder，保留溢出、截断和 `NULL` 语义。
6. 要求投影表达式全部是 `Column`。第一次遍历按列 ID 映射并克隆 `ColumnInfo`，第二次遍历克隆表达式列作为 `AccessColumns`；任一列无法映射就返回 `None`。
7. 调用 `context.reset_plan_id()`，再用 `PointGetPlan::New` 分配计划 ID。填充 `DBName`、`TblInfo`、`Handle`、`UnsignedHandle`、`Columns`、`AccessColumns`、`AccessConditions`和固定代价 `1.0`，浅拷贝输出名并克隆 schema，最后返回物理计划。

`ConcreteSession::OptimizeRootPlanIDForTest` 在调用前还会筛掉带锁、`SELECT INTO`、表/索引 hint、显式分区、stable-result mode、`sql_select_limit`、非 TiKV 读引擎以及 fix-control 52592 等情形；函数自身的注释明确这些语句级资格归调用者负责。

## 数据与状态

函数不修改输入逻辑树或表元数据。它从 `DataSource.TableInfo` 读取主键标志、分区、列状态和库名，从 `LogicalSelection.Conditions` 读取访问谓词，从 `LogicalProjection.Exprs/Schema/OutputNames` 读取输出布局。

成功输出中，`Handle: Option<i64>` 保存 Datum 的 64 位比特模式，`UnsignedHandle` 决定执行层是否按无符号解释；因此无符号 `u64 > i64::MAX` 被 `as i64` 保留比特位，不表示逻辑上变成负主键。`Columns` 是表列元数据的克隆，`AccessColumns` 是投影列表达式的克隆，`AccessConditions` 克隆唯一等值条件，schema 深克隆而输出名按 `Shallow()` 共享底层名称对象。

唯一显式可观测副作用是 `context.reset_plan_id()`。`PointGetPlan::New` 紧接着通过上下文分配新 ID；`SessionPlanContext` 使用 `AtomicI32` 的 `SeqCst` load/store/fetch-add 实现这个计数器。

## 依赖与调用关系

上游生产链为：

`pkg/planner/core/tests/pointget/point_get_plan_test.rs::TestPointGetId` → `pkg/testkit/testkit.rs::TestSession::OptimizeRootPlanIDForTest` → mockstore 的 `SessionRequest::OptimizeRootPlanID` → `pkg/session/runtime/planning.rs::ConcreteSession::OptimizeRootPlanIDForTest` → `TryFastIntegerPointGet`。

直接调用者先通过真实 parser、infoschema 和 `PlanBuilder::buildResultSetNode` 构造逻辑计划，再在语句级开关允许时尝试本快路径；返回 `None` 时它再次重置 plan ID 并调用 `astersql_planner_core::DoOptimize`。

下游依赖包括：

- `base_dependency::{ContextRef, Plan, PhysicalPlan}`：上下文副作用、schema/输出名访问以及 trait object 返回边界。
- `logicalop_dependency::{LogicalProjection, LogicalSelection, DataSource}`：输入树形状与元数据来源。
- `expression_dependency`：标量函数、列、常量、Datum kind、主键/无符号标志和列状态。
- `physicalop_dependency::PointGetPlan`：被返回的真实物理操作符，后续执行适配层可据其 handle 发起单行读取。

RustCodeGraph 精确 `query` 能定位该符号，但 `callers/callees` 查询在本次任务中长时间无输出而被中止；上述边因此由全仓精确文本搜索和直接源码读取交叉确认，不将图查询超时解读为“无边”。

## 错误处理与边界

返回类型是 `Option` 而非 `Result`：不符合快路径的 SQL 形状、元数据或常量类型都是正常的“不适用”，统一以 `None` 传播。`?`、slice 结构模式和 `collect::<Option<Vec<_>>>()` 使任一局部匹配失败立即回退，不产生半成品计划。

重要边界是：非 projection-selection-source 形状、多条件/剩余谓词、非 `EQ`、非主键列、表达式投影、分区表、系统库、生成列、非 public 列、deferred/parameter 常量、`NULL` 或非整数 Datum 都必须返回 `None`。负数不得用于 unsigned 主键；超过 `i64::MAX` 的 `u64` 不得用于 signed 主键。

本文件不执行权限、表锁、索引 hint、物化视图、最新 schema 索引状态、`LIMIT`/`ORDER BY`、行锁和隔离级别检查。这些在 Go `TryFastPlan`/`tryPointGetPlan` 中更完整，当前 Rust 调用者只在其观测用例所需的语句范围内做了前置筛选。

## 并发与资源生命周期

函数不创建线程、异步任务、通道、锁、事务或 I/O 资源，执行过程完全同步。输入 `logical` 仅在调用期间借用；输出通过 `Box<dyn PhysicalPlan>` 拥有新建计划，并通过 `ContextRef` 的引用计数克隆共享计划上下文。表元数据、schema、列与条件都被克隆到输出中，不依赖输入逻辑树的借用寿命。

`reset_plan_id` 是全函数唯一的共享状态写入。真实 `SessionPlanContext` 对计数器使用原子 `SeqCst` 操作；直接单元测试的 `ResettingContext` 则把 reset 委托为 `restore_plan_id_checkpoint(0)`。这个原子实现保证计数器读写不发生数据竞争，但不表示同一计划上下文可以被多个并发优化流程任意共享；调用者仍应维持“每次优化使用新鲜上下文”的会话约定。

## 与 Go 版本的对应关系

Go 对照入口是 `pkg/planner/core/point_get_plan.go::TryFastPlan` 和其 `tryPointGetPlan`。共同语义包括：在通用优化前尝试点查；只对单表精确等值访问生成计划；要求列非生成且为 public；构造 `PointGetPlan` 时保留 schema、输出名、表元数据、handle 符号属性和访问条件；快路径不适用时无错回退。Go `TryFastPlan` 也在尝试前将 `PlanID`/`PlanColumnID` 归零，对应 Rust 函数的 `context.reset_plan_id()` 以及调用者管理的列 ID 上下文。

当前 Rust 函数是刻意缩小的整型 PK-handle 子集，与 Go 版本存在明确差异：

- 不处理唯一索引、聚簇/公共 handle、`IN`/`OR` 批量点查、`UPDATE`/`DELETE`、`TableDual`、显式分区和全局索引。
- 不在内部处理 `LIMIT`、`ORDER BY`、hint、权限、表锁、行锁、连接隔离级别或最新索引可见性。
- 不接受参数标记或 deferred constant，也不记录 Go `HandleConstant`、`HandleFieldType`、`HandleColOffset`、`PartitionNames` 等额外字段。
- Go 的 `buildSchemaFromFields` 能处理更广的字段形态；Rust 函数要求每个 projection expression 都是直接列，因而 `select b + 1 ...` 必须回退。
- Go 新建计划设置估算行数 1、锁等待时间和 statement table 记录；本 Rust 函数仅将 `CostValue` 设为 1.0，未复制这些副作用。

因此，扩大本函数的支持面时必须以 Go 的分支顺序和回退语义为依据，不能把另一个 Rust `point_get_plan.rs` 的简化数据模型直接等同于完整 Go 实现。

## 扩展指南

- 扩展可接受的逻辑树形状时，首先修改 `TryFastIntegerPointGet` 前半部的转型/单子节检查，并保证任何被移除的 selection 条件都被完整编码到点查访问键或保留为 residual filter。
- 支持新的常量或类型强制转换时，应对照 Go range/Datum 的溢出、截断、`NULL`、signed/unsigned 规则，不应用 Rust `as` 强制转换静默放宽资格。
- 支持唯一索引或分区时，需要同步安装 `PointGetPlan` 的 `IndexInfo`/`IndexValues`/`IdxCols`/`IdxColLens`/`PartitionIdx` 等字段，并复制 Go 中索引状态、hint、分区过渡状态与隔离级别检查。
- 增加语句级快路径能力时，同步检查 `ConcreteSession::OptimizeRootPlanIDForTest` 的 `fast_statement` 门禁，避免函数内外的责任缝隙。
- 直接单元回归应放在独立文件 `pkg/planner/core/point_get_plan_runtime_test.rs`，使用 `build_logical_for_test` 覆盖新的成功和回退形态；端到端的计划 ID/会话门禁回归应扩展 `pkg/planner/core/tests/pointget/point_get_plan_test.rs::TestPointGetId`。
- 修改 `PointGetPlan` 字段时还要检查 `pkg/planner/core/operator/physicalop/physical_batch_point_get.rs` 的构造、克隆、代价和执行适配语义，并与 Go `physical_batch_point_get.go` 的字段保持对齐。

兼容性风险集中在错误放宽资格后吞掉谓词、绕过权限/锁/hint 检查或改变 signed/unsigned handle 语义；性能风险则包括在快路径中引入通用 range builder、重复克隆大型元数据或因过度保守而频繁回退通用优化器。

## 验证依据

- 目标源码：`pkg/planner/core/point_get_plan_runtime.rs`，逐分支核对唯一公开符号、所有资格检查和输出字段。
- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`query TryFastIntegerPointGet --kind function --limit 20 --json` 命中 `point_get_plan_runtime.rs::TryFastIntegerPointGet`、起始行 28 和签名。`files --filter` 未命中而精确 query 命中，`callers/callees` 长时间无输出后中止，因此调用边另用精确文本搜索核对。
- crate/模块边界：`pkg/planner/core/Cargo.toml` 和 `pkg/planner/core/lib.rs`。
- 真实返回类型：`pkg/planner/core/operator/physicalop/physical_batch_point_get.rs::PointGetPlan` 及其 `New`；Go 字段对照为同目录 `physical_batch_point_get.go::PointGetPlan`。
- 生产调用与 plan-ID 状态：`pkg/session/runtime/planning.rs::ConcreteSession::OptimizeRootPlanIDForTest`、`SessionPlanContext::{alloc_plan_id, reset_plan_id}`。
- Go 对照：`pkg/planner/core/point_get_plan.go::{TryFastPlan, tryPointGetPlan, newPointGetPlan}`；同时阅读 `pkg/planner/core/point_get_plan.rs::TryFastPlan` 以区分另一套 Rust 简化快路径模型。
- 独立 Rust 单元测试：`pkg/planner/core/point_get_plan_runtime_test.rs`。`integer_point_get_keeps_handle_projection_and_resets_id` 验证左/右两种等值顺序、handle 7、投影列与输出名以及重复构建时 ID 恒为 1；`integer_point_get_preserves_general_optimizer_fallbacks` 验证表达式投影、多条件、范围/非主键/`NULL` 谓词、order/limit、分区表与 unsigned 负数的回退。
- 会话级 Rust 测试：`pkg/planner/core/tests/pointget/point_get_plan_test.rs::TestPointGetId`，经 `pkg/testkit/testkit.rs` 与 `pkg/testkit/mockstore.rs` 请求桥接验证两种等值顺序反复规划时根 ID 为 1，以及 fix-control 52592 禁用快路径后转入通用优化。
- 结构验证按任务给定命令执行，要求文档存在且恰含十一个固定二级标题。本任务为纯文档分析，按总计划不运行 Cargo。
