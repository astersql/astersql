# `pkg/executor/typed_selection.rs`

## 文件定位

`typed_selection.rs` 位于 `astersql-executor` crate，是 typed 物理执行器链中的行过滤节点。crate 由 `pkg/executor/Cargo.toml` 定义，根模块在 `pkg/executor/lib.rs` 中以 `pub mod typed_selection` 暴露本模块，并仅在 `cfg(test)` 下装配独立测试 `typed_selection_test.rs`。

上游 `pkg/executor/builder.rs` 将 canonical 物理计划中的 `PhysicalSelection` 构造成 `TypedSelection`；它也会在表扫描、索引扫描/回表以及非 Full Outer Join 的左右输入存在过滤条件时，用本节点包装已经构造好的子执行器。因此，本文件不是 SQL 解析或谓词生成处，而是把 planner 产生的 `ExprBox` 条件应用到 typed `Chunk` 行上的执行期适配层。

## 核心职责

- `TypedSelection::new` 接收一个 `Box<dyn ExecExecutor>` 子节点、canonical 表达式列表和会话计划上下文，把表达式包装为 `CNFExprs`，并按子节点的 chunk 配置创建可复用输入缓冲。
- `TypedSelection::next_inner` 按需从子节点拉取批次，对每行调用 `astersql_expression::EvalBool`，只把结果为真的行追加到输出 chunk。
- 当子节点同时返回逐行锁键时，过滤器保持“输出行与锁键一一对应”：未通过谓词的行对应锁键不会通过 `TakeLockKeys` 暴露给上层。
- `ExecExecutor` 实现负责生命周期检查，并把 schema、chunk 配置、扫描计数及外键相关能力透明委托给子节点。

它当前明确不承担向量化过滤、内存 tracker、异步预取或可分离执行；这些不能从 Go `SelectionExec` 的能力反推为本实现已支持。

## 主要符号

- `pub struct TypedSelection`：本文件唯一的模块级类型，也是唯一公开 API。字段均为私有：
  - `child` 是唯一子执行器。
  - `conditions: CNFExprs` 保存合取范式过滤条件。
  - `context: ContextRef` 提供会话绑定的表达式求值上下文。
  - `source`、`source_index` 保存当前子批次及下一待处理行位置，支持一次子批次跨多次 `Next` 消费。
  - `source_keys` 保存当前子批次的锁键，`page_keys` 保存当前输出页对应的锁键。
  - `opened`、`closed` 维护显式生命周期状态。
- `pub fn TypedSelection::new(...) -> Self`：公开构造入口。构造时调用 `child.NewChunk()`，但不打开或拉取子节点。
- `fn TypedSelection::next_inner(...) -> AdapterResult`：私有的统一拉取实现；`context: Option<&ExecutionContext>` 决定对子节点调用 `Next` 还是 `NextWithContext`。
- `impl ExecExecutor for TypedSelection`：实现 `Open`、`Close`、`Next`、`NextWithContext`、元数据/外键委托、锁键转移、扫描计数和 `Detach`。本文件没有常量、trait、条件编译项或其他自由函数。

## 执行流程

1. `TypedSelection::new` 从子执行器创建 `source` chunk，将条件转成 `CNFExprs`，并把索引、键缓存和生命周期标志初始化为空/未打开状态。
2. `Open` 先调用 `child.Open()`；成功后重置输入 chunk、行索引和两级锁键缓存，再置 `opened = true`、`closed = false`。若子节点打开失败，状态不会被标记为已打开。
3. `Next` 或 `NextWithContext` 进入 `next_inner`。函数首先拒绝未打开或已经关闭的调用，然后重置调用方的输出 chunk，并清空上一输出页的 `page_keys`。
4. 在输出未满时，如果当前 `source` 已消费完，则向子节点拉取下一批；有 `ExecutionContext` 时保留该上下文并调用 `child.NextWithContext`，否则调用 `child.Next`。随后从子节点通过 `TakeLockKeys` 接管本批锁键。
5. 子批次为空表示 EOF，当前调用返回一个空或尚未填满的输出 chunk。若锁键非空但数量不等于子批次行数，则立即报错，防止行键错配。
6. 每次先推进 `source_index`，再取该行并调用 `EvalBool(context.GetExprCtx().GetEvalCtx(), &conditions, row)`。只有 `keep == true` 时才用 `Chunk::Append` 追加该行；若本批存在锁键，也按同一行下标克隆到 `page_keys`。
7. 输出达到容量或子节点 EOF 后返回。调用方随后可用 `TakeLockKeys` 一次性取走本输出页的键；下一次 `Next` 会从未消费完的 `source_index` 继续。
8. `Close` 首次调用时先标记本节点关闭，再关闭子节点；重复关闭直接成功。关闭之后不能再次 `Next`，但重新 `Open` 会重置本节点状态并重新打开子节点。

## 数据与状态

本节点是有状态、分页式的拉取算子。`source` 的生命周期覆盖多次 `Next`，`source_index` 是当前批次游标；这保证当大量输入行通过过滤而输出 chunk 提前填满时，剩余输入不会丢失。`output.Reset()` 与 `page_keys.clear()` 表明每次调用产生的是独立输出页，而非增量追加到调用方旧内容。

锁键有两个阶段：`source_keys` 与子批次按行对齐，`page_keys` 与当前输出页按行对齐。空 `source_keys` 表示子节点没有提供逐行锁键，属于合法状态；非空时必须满足 `source_keys.len() == source.NumRows()`。`TakeLockKeys` 使用 `std::mem::take` 转移所有权，调用一次后本页缓存为空。

条件和 `ContextRef` 在构造后保持不变。`EvalBool` 按 CNF 顺序求值；普通 NULL 条件和假值都会淘汰行，来自 `IN` 等价条件的延迟 NULL 会通过其第二返回值表达。这里故意忽略 `_deferred_null`，因为 Selection 的最终职责只需要决定该行是否保留。

## 依赖与调用关系

- 上游构造：`pkg/executor/builder.rs` 的 `BuildTypedPhysicalPlan`、内部递归构建与 `wrap_typed_pushdown_plan` 在 `PhysicalSelection`、扫描 `FilterCondition`、索引 lookup 两阶段过滤和 Join 侧条件处调用 `TypedSelection::new`。
- 统一执行接口：`crate::adapter::ExecExecutor` 定义 `Open/Next/Close`、chunk/schema、外键、锁键、扫描计数及 detach 契约；本类型通过 trait object 嵌入任意 typed 子执行器。
- 表达式：`astersql-expression` 提供 `ExprBox`、`CNFExprs` 和 `EvalBool`。RustCodeGraph 的调用边确认 `TypedSelection::next_inner -> pkg/expression/expression.rs::EvalBool`。
- 会话上下文：`astersql-planner-core-base::ContextRef` 经 `GetExprCtx().GetEvalCtx()` 提供 canonical 求值环境；因此表达式语义与会话绑定。
- 批数据：`astersql-util-chunk` 提供 `Chunk`、`Row`、`Reset`、`Append`、容量及行访问。输出 schema 与 chunk 配置不在本文件复制，而由 `Schema`、`ChunkConfig`、`NewChunk` 委托给 child。
- 错误：`AdapterResult` 来自 `crate::adapter`，底层类型为 `Result<_, astersql_errors::SharedError>`；本节点也用 `astersql-errors` 将生命周期、键对齐及表达式错误统一到该边界。

上述四个直接 crate 依赖均在 `pkg/executor/Cargo.toml` 中声明：`astersql-errors`、`astersql-expression`、`astersql-planner-core-base`、`astersql-util-chunk`。

## 错误处理与边界

- 未成功 `Open`，或已 `Close` 后调用 `Next`/`NextWithContext`，返回 `selection executor is not open`。
- `child.Open`、`child.Next`、`child.NextWithContext`、`child.Close` 的错误通过 `?` 原样传播；`EvalBool` 的表达式错误被转换为 `astersql_errors::New(error.to_string())`，会保留文字信息，但会丢失原错误的具体类型链。
- 子节点提供非空锁键时，如果键数与行数不相等，返回 `lock keys do not match typed child rows`，且不会继续暴露可能错配的锁键。
- 空条件列表按 `EvalBool` 语义保留所有行。空子批次是 EOF，不是错误。
- `Close` 是幂等的；但它在调用 `child.Close()` 前已经设置 `closed = true`，所以子节点关闭失败后再次调用不会重试子关闭。
- `CalculateNoDelay` 和 `IsWriteExecutor` 固定为 `false`；外键检查、级联批次、锁等待时长和扫描行数均透传给子节点，过滤器自身不新增这些统计或副作用。

## 并发与资源生命周期

`TypedSelection` 通过 `&mut self` 驱动，没有内部锁、线程、任务、通道或原子状态；代码也没有声明其可跨线程共享。其正确使用模型是单执行器链上的串行 `Open -> Next* -> Close`。输入 chunk 与两个锁键向量均在执行器内复用，减少逐页重新分配；保留的锁键需要克隆，是过滤时与行对齐所需的显式所有权成本。

`ContextRef` 与 canonical 表达式求值环境绑定会话，所以 `Detach` 无条件返回 `None`。`NextWithContext` 中的 `ExecutionContext` 仅继续传递给 child，本文件自身不读取 trace、RU 或 kill signal；因此取消/计量能力取决于下游执行器。资源关闭由 child 的 `Close` 实现承担，本节点没有独立外部句柄。

## 与 Go 版本的对应关系

直接 Go 对照为 `pkg/executor/select.go` 的 `SelectionExec`。两者都以一个子执行器为输入，复用子 chunk，按谓词过滤行，在 EOF 时返回空 chunk，并传播子执行器或表达式错误。Rust 的 `EvalBool` 位于 `pkg/expression/expression.rs`，其 NULL、假值和 `IN` 等价条件延迟 NULL 规则与 `pkg/expression/expression.go` 的 canonical `EvalBool` 保持对应。

当前 Rust typed 实现是聚焦的行式版本，与 Go 版本仍有明确差异：Go 会依据 `expression.Vectorizable` 选择 `VectorizedFilter` 或逐行路径，跟踪 chunk 内存占用，并尊重 required rows；Rust 始终逐行求值，只以 `output.IsFull()` 控制页大小，也没有本地内存 tracker。Go `SelectionExec.Detach` 会在表达式不需要可选求值属性时复制分离；Rust 因持有会话 `ContextRef` 而总是返回 `None`。Rust 还承担 typed locking 链中的逐行 `TakeLockKeys` 过滤，这是本文件测试明确覆盖的契约，不能仅按 Go `SelectionExec` 的字段结构理解。

Go 侧 `pkg/executor/executor_required_rows_test.go::TestSelectionRequiredRows` 验证 required-row 拉取策略，`pkg/executor/detach_integration_test.go::TestDetachSelection` 验证可分离条件；它们说明的是 Go 完整执行器能力，不是 Rust 当前已具备能力。

## 扩展指南

- 修改谓词语义时，首要接入点是 `next_inner` 对 `EvalBool` 返回值的处理；必须与 `pkg/expression/expression.rs::EvalBool` 及 Go `pkg/expression/expression.go::EvalBool` 同步核对 NULL、短路、错误和副作用顺序。
- 新增向量化或 required-row 支持时，应保留 `source_index` 跨页续读及锁键逐行筛选不变量，避免输出行和 `page_keys` 错位；性能比较应覆盖高选择率、低选择率和跨多个子 chunk 的情况。
- 若要支持 Detach，不能只把 `Detach` 改成返回 child；需要证明 `conditions` 和求值上下文不再引用会话可选属性，并构造独立上下文，参照 Go `pkg/executor/detach.go::SelectionExec.Detach` 的属性检查意图。
- 生命周期或错误策略变化应覆盖未打开拉取、关闭后拉取、重复关闭、child 打开/关闭失败，以及键数不匹配。测试逻辑必须继续放在独立的 `pkg/executor/typed_selection_test.rs`，不要内嵌到生产文件。
- builder 新增过滤包装位置时，应复用 `TypedSelection::new`，并检查过滤发生在索引行、回表行还是 Join 输入上；放置阶段会影响可用列、锁键和执行代价。
- 兼容性风险主要是 SQL NULL/副作用求值顺序和 Go 语义漂移；正确性风险主要是游标或锁键错配；性能风险主要是逐行表达式调用与锁键克隆。任何优化都不应以删除 canonical 分支或简化 Go 行为来换取测试通过。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点；`node --file pkg/executor/typed_selection.rs` 读取了完整 163 行，并显示该文件被多个执行器/测试文件引用。
- RustCodeGraph 符号与调用查询：`query TypedSelection` 定位到 `typed_selection.rs:16`；`query next_inner` 定位到本文件 `:48`；`callees next_inner` 给出本文件到 `EvalBool`、`Next`、`NextWithContext`、`TakeLockKeys` 和 `GetExprCtx` 的边；`node EvalBool` 进一步确认 `pkg/expression/expression.rs:383` 的实现及其由本文件调用。
- 已读生产证据：`pkg/executor/typed_selection.rs`、`pkg/executor/builder.rs`、`pkg/executor/adapter.rs`、`pkg/executor/lib.rs`、`pkg/executor/Cargo.toml`、`pkg/expression/expression.rs`（经 RustCodeGraph 源码节点）。`pkg/executor` 下未发现包级 `doc.go`。
- 已读 Rust 测试：`pkg/executor/typed_selection_test.rs::canonical_typed_selection_filters_rows_and_excludes_discarded_record_locks`。它验证 `PhysicalSelection + PhysicalTableScan` 构建、Open 不提前迭代、仅保留值 30、仅返回该行锁键、`Detach == None`、EOF 空页和 Close 成功。
- 已读 Go 对照：`pkg/executor/select.go::SelectionExec`、`Open/open/Next/unBatchedNext/Close`，`pkg/executor/detach.go::SelectionExec.Detach`，以及 `pkg/executor/executor_required_rows_test.go::TestSelectionRequiredRows`、`pkg/executor/detach_integration_test.go::TestDetachSelection`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核唯一新增生产物、源码链接和无“已支持”臆测。
