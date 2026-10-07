# `pkg/executor/typed_projection.rs`

## 文件定位

`typed_projection.rs` 属于 `astersql-executor` crate（`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/executor/lib.rs` 公开为 `typed_projection` 模块。它把规范物理计划中的 `PhysicalProjection` 和 `PhysicalTableDual` 落为统一 `ExecExecutor` 接口下的 typed 执行器，位于“物理计划构建 -> chunk 拉取执行”链路中。

直接装配入口在 `pkg/executor/builder.rs`：`build_typed_physical_plan` 为普通 typed 计划递归构建 `TypedProjection`/`TypedTableDual`，`wrap_typed_pushdown_plan` 还会在 index pushdown 子树外包装 `TypedProjection`。因此本文件不是通用 Go 执行器 `ProjectionExec` 的 Rust 逐字段翻译，而是当前 Rust typed runtime 的投影节点与 dual 叶子节点实现。

## 核心职责

- `TypedProjection`：从唯一子执行器分批取行，按 `expressions` 的顺序逐行、逐表达式求值，把结果写入输出 `Chunk`；这既可重排列，也可计算派生表达式（`TypedProjection::next_inner`）。
- 在输出分页与子节点分页不同步时保留未消费的源行，通过 `source` 和 `source_index` 跨多次 `Next` 继续处理（`TypedProjection::{source,source_index}`）。
- 将每个输入行对应的悲观锁 key 与投影结果同步分页，父节点通过 `TakeLockKeys` 一次性取走本页 keys（`source_keys`、`page_keys`）。
- 将 schema、chunk 容量、`CalculateNoDelay`、扫描行数和外键相关操作沿执行器链正确暴露或透传。
- `TypedTableDual`：为常量投影和 `DO` 一类无真实数据源的计划提供 0/1 行叶子；有 schema 时产生一行全 `NULL`，空 schema 时用 virtual row 表示一行。

## 主要符号

### `pub struct TypedProjection`

- `child: Box<dyn ExecExecutor>`：唯一输入执行器。
- `expressions: Vec<astersql_expression::ExprBox>`：按输出列序排列的规范表达式。
- `context: ContextRef`：表达式上下文；`Eval` 和构造 schema 时均使用 `context.GetExprCtx().GetEvalCtx()`。
- `schema: Vec<SchemaColumn>`：在 `new` 中由每个表达式的 `GetType` 固化，供父节点建 chunk 和返回元数据。
- `no_delay`：从 `PhysicalProjection.CalculateNoDelay` 传入并由 `CalculateNoDelay` 原样返回。
- `source`、`source_index`：复用的子 chunk 及下一个待投影行下标。
- `source_keys`、`page_keys`：分别对应当前子 chunk 和当前输出页的锁 key。
- `opened`、`closed`：本节点生命周期状态。

`TypedProjection::new(child, expressions, context, no_delay) -> Self` 预计算输出 schema，并通过 `child.NewChunk()` 创建可复用源 chunk；它不打开子节点，也不执行表达式。

`TypedProjection::next_inner(context, output) -> AdapterResult` 是 `Next` 与 `NextWithContext` 的共享实现。它检查生命周期、重置输出、按需拉取 child、校验锁 key 数量、执行表达式并组装本页锁 key。

`impl ExecExecutor for TypedProjection` 提供公开执行协议。除核心 `Open`/`Next`/`Close` 外：`ChunkConfig` 保留 child 的初始/最大容量但替换字段类型；外键检查、级联状态、锁等待时长和 `ScannedRows` 透传 child；`IsWriteExecutor` 为 `false`；`Detach` 返回 `None`，明确该节点不可脱离原会话。

### `pub(crate) struct TypedTableDual`

字段 `rows` 保存计划要求的 0/1 行，`returned` 防止重复输出，`schema` 和 `config` 定义返回列及 chunk 分配参数。`TypedTableDual::new` 只在 crate 内可见；正常入口 `build_typed_physical_plan` 先验证 `PhysicalTableDual.RowCount` 位于 `0..=1`。

`impl ExecExecutor for TypedTableDual` 是无 child 的叶子实现：`Open` 重置 `returned`，`Next` 最多产出一次，外键与扫描状态均为空/默认，`Detach` 同样返回 `None`。

本文件没有模块级常量、枚举、trait 或条件编译项。

## 执行流程

### 投影

1. `builder.rs::build_typed_physical_plan` 识别 `PhysicalProjection`，要求恰有一个 child，递归构建 child 后把 `Exprs`、计划 `ContextRef` 和 `CalculateNoDelay` 交给 `TypedProjection::new`。index pushdown 路径由 `wrap_typed_pushdown_plan` 做同样包装。
2. `new` 对每个表达式调用 `GetType(eval_ctx)` 形成输出 schema，并向 child 请求一个匹配 child schema/capacity 的源 chunk。
3. `Open` 先调用 `child.Open()`；成功后重置源 chunk、游标、两组锁 key，并把状态切换为 opened 且未 closed。child 打开失败时错误直接返回，本节点不会标为已打开。
4. `Next` 调用 `next_inner(None, output)`；`NextWithContext` 调用 `next_inner(Some(context), output)`，差别仅在拉 child 时选择 `child.Next` 或 `child.NextWithContext`。
5. `next_inner` 先拒绝未打开或已关闭状态，再 `Reset` 输出并清空上一页 `page_keys`。只要输出未满就继续：源 chunk 已耗尽时拉取下一 chunk，游标归零并从 child `TakeLockKeys`；child 返回零行即 EOF。
6. 若 child 提供锁 key，则其数量必须与源行数相同，否则返回 `lock keys do not match typed child rows`。该约束保证后续用同一行下标取 key 是安全且语义一致的。
7. 每消费一行，依输出列顺序调用 `ExprBox::Eval(eval_ctx, row.clone())`，再用 `AppendDatum(column, datum)` 写入。表达式列表为空时没有物理列可增长行数，因此显式将 virtual row 数加一。
8. 有源 keys 时，把当前行 key 克隆到 `page_keys`。输出满或 child EOF 后返回；调用方随后用 `TakeLockKeys` 取走且清空该页 keys。未消费完的 `source` 留到下次调用。
9. `Close` 首次调用先标记 closed 再关闭 child；之后重复调用直接成功。

### Dual

1. 构建器校验 `RowCount` 只能为 0 或 1，并从物理 schema 生成 `SchemaColumn` 后调用 `TypedTableDual::new`。
2. `Open` 把 `returned` 复位，因此同一实例可关闭后重新打开并再次输出。
3. `Next` 总是先清空输出。若已返回过或 `rows == 0`，直接给出空 chunk；否则空 schema 设置 `rows` 个 virtual rows，有 schema 时为每列追加一个 `NULL`，然后标记 `returned = true`。
4. 正常构建路径保证 `rows <= 1`；因此“有 schema 时每列只追加一个 NULL”与计划行数一致。直接绕过构建器传入大于 1 的值不属于已验证契约。

## 数据与状态

`TypedProjection` 是有状态的 pull operator。输出 schema 在构造时固定，但每次 `Open` 都重置执行游标与 keys。`source` 的容量和字段来自 child；输出 chunk 的字段来自投影表达式，而 `initial_capacity`、`maximum_chunk_size` 沿用 child 的 `ChunkConfig`。这保证上下游分页参数一致，同时允许投影改变列数与列类型。

一条关键不变量是：`source_keys` 要么为空（child 不产生锁 key），要么与 `source.NumRows()` 等长。`source_index` 同时索引 row 和 key；每次成功投影一行后才将对应 key 放入 `page_keys`。`TakeLockKeys` 使用 `std::mem::take` 转移所有权，重复调用不会重复返回旧 key。

表达式求值使用保存在节点中的 `ContextRef`，而 `ExecutionContext` 只向 child 传播 trace/RU/kill 等执行期信息；它不替换表达式 eval context。`TypedTableDual` 则没有 child 或表达式状态，只有是否已经输出的布尔状态。

## 依赖与调用关系

上游：

- `pkg/executor/builder.rs::build_typed_physical_plan`：`PhysicalProjection -> TypedProjection`、`PhysicalTableDual -> TypedTableDual` 的主要构建边。
- `pkg/executor/builder.rs::wrap_typed_pushdown_plan`：为 `PhysicalProjection` index pushdown 子树包装 `TypedProjection`。
- `pkg/executor/adapter.rs::recordSet::Next` 经 `ExecStmt::nextWithContext` 从根执行器逐页拉取；若根或中间节点是本文件类型，最终进入 `NextWithContext`/`next_inner`。

下游：

- `crate::adapter::ExecExecutor` 定义生命周期、chunk/schema、外键、锁 key、扫描统计与 detach 协议；`CascadeBatch`、`ExecutionContext`、`ChunkConfig`、`SchemaColumn`、`Key` 都来自同一适配层。
- `astersql-expression::ExprBox::{GetType,Eval}` 提供输出类型推导与逐行表达式求值。
- `astersql-planner-core-base::ContextRef` 提供会话绑定的表达式求值上下文。
- `astersql-util-chunk::Chunk` 承载输入和输出批次，使用 `Reset`、`IsFull`、`GetRow`、`AppendDatum`、`AppendNull` 和 virtual row API。
- `astersql-errors::New` 把生命周期、锁 key 不变量和表达式错误统一为 `AdapterResult` 的共享错误。

`pkg/executor/Cargo.toml` 明确声明以上 `astersql-errors`、`astersql-expression`、`astersql-planner-core-base`、`astersql-util-chunk` 为本 crate 直接路径依赖；本文件没有受 `nextgen` feature 控制的分支。

## 错误处理与边界

- 未 `Open` 或已经 `Close` 后调用投影 `Next`/`NextWithContext`，返回 `projection executor is not open`，不会触碰 child 或输出。
- child 的 `Open`、`Next`、`NextWithContext`、`Close`、`CheckForeignKeys` 错误通过 `?` 原样传播。
- `ExprBox::Eval` 的错误会以其字符串构造 `astersql_errors::New`；此前已经追加到输出的行/列不会在错误分支额外回滚，因此调用方应把出错的整个 `Next` 视为失败结果。
- 锁 key 非空但数量与 child 行数不等时立即报错，避免 key 与结果行错配。
- 零表达式是显式支持的边界：每个输入行产生一个 virtual row，而不是零行。
- `TypedTableDual` 自身不会校验 `rows`；0/1 限制由唯一生产构建入口 `build_typed_physical_plan` 强制，crate 内新增调用方必须维护该前置条件。
- `Close` 对已经 closed 的 `TypedProjection` 幂等；`TypedTableDual::Close` 始终成功。按 `ExecExecutor` 协议应先 `Open` 再 `Close`，尤其投影初始 `closed == false`，不应把“未打开即关闭”当作受测试行为。
- 两种执行器均 `Detach() -> None`，不能用于要求独立快照/跨会话存活的 detached result set。

## 并发与资源生命周期

本文件不创建线程、任务、锁或通道，也没有内部并行表达式执行。`TypedProjection` 通过 `&mut self` 串行推进，持有 `ContextRef`，并依 `ExecExecutor` 注释所述保持在原会话线程；它没有实现 detached/`Send` 执行路径。

主要资源是两个复用 chunk 与两组 key 向量：`source` 跨输出页复用，调用方提供的 `output` 每页重置；`source_keys` 随新 child chunk 替换，`page_keys` 每次 `next_inner` 清空并在 `TakeLockKeys` 时转移。表达式和 context 在执行器整个生命周期内持有。`Close` 负责向下关闭 child，但本文件没有 Go 版本中的 worker join、channel drain 或内存 tracker 解绑逻辑。

`TypedTableDual` 不拥有外部资源，`Open` 仅复位逻辑状态，`Close` 无操作。

## 与 Go 版本的对应关系

`TypedProjection` 对应 `pkg/executor/projection.go::ProjectionExec` 的关系代数语义：从 child 拉取 chunk，按表达式形成新列，并在 `Open`/`Next`/`Close` 生命周期中工作。Rust 当前路径使用同一会话的 eval context，语义上接近 Go 的 `unParallelExecute` 调用 `EvaluatorSuite.Run`，但实现粒度是逐行 `ExprBox::Eval`。

重要差异：Go `ProjectionExec` 支持 `numWorkers`、fetcher/worker channels、goroutine 回收、vectorizable 判断、requiredRows 传递、内存 tracker 和运行时并发统计；`TypedProjection` 目前均未实现，只按输出 chunk 的 `IsFull` 串行求值。Go 测试 `pkg/executor/executor_required_rows_test.go` 验证 requiredRows 和并行/非并行分页，其中并行用例目前还因调度不稳定而跳过；这些不能被当作 Rust 已支持能力。

`TypedTableDual` 对应 `pkg/executor/select.go::TableDualExec`：Go 同样在 `Open` 重置计数，0 行直接 EOF，空 schema 使用 virtual row，有 schema 追加 `NULL`，且计划约束 dual 行数只能为 0/1。Rust 用 `returned: bool` 表达一次性输出，而 Go 用 `numReturned`/`numDualRows`。

Rust 独立测试 `pkg/executor/typed_projection_test.rs` 是本移植路径的直接行为证据；Go 的投影测试更多覆盖通用执行器能力，不能反向推断 Rust typed runtime 已完整移植。

## 扩展指南

- 新增表达式批量/向量化执行时，主要接入点是 `TypedProjection::next_inner`。必须保留表达式顺序、零表达式 virtual row、跨页 `source_index` 和逐行锁 key 对齐；测试放在独立的 `pkg/executor/typed_projection_test.rs`，不要内嵌进源文件。
- 若实现 requiredRows，应同时明确 child 请求量、源 chunk 剩余行和 output 最大容量三者关系，并参考 Go `ProjectionExec::unParallelExecute` 及 `executor_required_rows_test.go`，不能只让测试凑齐行数。
- 若增加并行求值，必须先处理 `ContextRef`/表达式的线程绑定、输出顺序、错误取消、worker 生命周期、内存核算和 locks-to-rows 的稳定映射；当前 `ExecExecutor` 本身未要求 `Send`，不能直接照搬 Go goroutine 结构。
- 增加可 detach 能力时需修改 `Detach`，并证明表达式、会话参数、事务状态和 child 数据源都能独立拥有；仅让 child 可 detach 不足以保证 projection 安全。
- 改动锁 key 行为时，要扩展现有“重排列仍逐页保留 row key”的测试，并新增 key 数量不匹配错误测试；不可丢弃 `source_keys.len() == source.NumRows()` 不变量。
- 扩展 dual 行数前必须同时修改构建器 `0..=1` 校验和有 schema 输出逻辑；当前实现对大于 1 行只在空 schema 分支按 `rows` 设置 virtual rows。
- 调整 schema/type 推导时，应同步验证 `new` 的 `GetType`、`ChunkConfig` 和实际 `Eval` datum 类型一致，避免 chunk 列类型与运行值不匹配。
- 性能风险集中在逐行、逐表达式动态分派与 key 克隆；兼容风险集中在 session eval context、NULL/virtual-row 表示和 Go 执行器能力差异。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`explore "pkg/executor/typed_projection.rs TypedProjection TypedTableDual"` 返回目标文件源码；`query TypedProjection` 与 `query TypedTableDual` 分别定位到第 28、206 行；`node --file pkg/executor/typed_projection.rs --offset 200 --limit 260` 核对 dual 完整实现。`callers`/`callees` 对这些符号未返回边，因此装配关系按技能规则用下述精确文本搜索补齐。
- 生产源码：`pkg/executor/typed_projection.rs`（两个执行器全部 293 行）、`pkg/executor/builder.rs`（`wrap_typed_pushdown_plan`、`build_typed_physical_plan`）、`pkg/executor/adapter.rs`（`ExecExecutor` 与 `recordSet::Next`）、`pkg/executor/lib.rs`（模块声明）。目标包没有 `doc.go`。
- crate 边界：`pkg/executor/Cargo.toml` 的 package/lib、`nextgen` feature 和直接依赖声明。
- Rust 测试：`pkg/executor/typed_projection_test.rs::canonical_typed_projection_reorders_typed_columns_and_preserves_row_lock_keys` 验证 typed builder、列重排、逐页输出、锁 key、不可 detach 和关闭；`typed_dual_emits_zero_or_one_row_and_reopens` 验证 0/1 行、空/非空 schema、EOF 和 reopen。
- Go 对照：`pkg/executor/projection.go::ProjectionExec`、`pkg/executor/select.go::TableDualExec`、`pkg/executor/executor_required_rows_test.go`。对照仅用于确认共同语义与明确未移植能力。
- 文档结构以任务指定命令校验，要求目标文件存在且恰有 11 个固定二级标题；本任务为纯文档分析，未运行 Cargo 或代码测试。
