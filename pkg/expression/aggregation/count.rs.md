# `pkg/expression/aggregation/count.rs`

## 文件定位

本文件实现行式 `COUNT` 聚合器 `countFunction`，属于 Cargo crate `astersql-expression-aggregation`（`pkg/expression/aggregation/Cargo.toml`），由 `pkg/expression/aggregation/lib.rs` 的 `mod count` 纳入并通过 `pub use count::*` 导出。它不是描述符构造或类型推断入口；规划阶段的 `AggFuncDesc` 在执行前由 `AggFuncDesc::GetAggFunc`（`descriptor.rs`）实例化成本类型，分布式 PB 路径则由 `NewDistAggFunc`（`aggregation.rs`）实例化。

在完整执行链中，本文件处于“参数表达式已经构造、聚合模式已经确定”之后：调用者逐行调用 `Aggregation::Update` 更新某一分组的 `AggEvaluateContext`，再通过 `GetPartialResult` 输出可合并的局部计数，或通过 `GetResult` 输出最终计数。`COUNT` 的返回类型推断、Complete/Partial 拆分及外连接空值折叠位于 `base_func.rs`、`descriptor.rs`，不在本文件内完成。

## 核心职责

- 对每行依次求值 `AggFuncDesc.Args`；任一参数为 NULL 时整行不计数（`countFunction::Update`）。这同时覆盖单参数 `COUNT(expr)` 与多参数 DISTINCT 元组的 NULL 语义。
- 在 `CompleteMode`、`Partial1Mode` 中，对通过 NULL 与 DISTINCT 检查的输入把 `AggEvaluateContext::Count` 加一。
- 在 `FinalMode`、`Partial2Mode` 中，把输入 Datum 当作上游局部计数并累加到 `Count`，从而支持两阶段聚合。
- 在 `HasDistinct` 为真时收集本行全部参数并调用 `distinctChecker::Check`，只让首次出现的编码元组通过。
- 把计数状态包装为 `types::Datum`，并复用 `aggFunction` 创建、重置每个分组的上下文。

本文件不负责 SQL 语法、返回类型推断、聚合下推决策、分组表管理或执行器调度，也不自行拥有线程、任务或 I/O 资源。

## 主要符号

- `pub struct countFunction { pub aggFunction: aggFunction }`：COUNT 的运行时对象。唯一字段保存 `AggFuncDesc`，其中包含参数表达式、`Mode` 与 `HasDistinct`。类型由 crate 根重新导出，但命名保持与 Go 移植代码一致。
- `impl Aggregation for countFunction`：实现 `aggregation.rs` 定义的行式聚合契约。
  - `Update(&mut self, ctx, _sc, row) -> Result<(), Error>`：本文件的核心状态迁移；`StatementContext` 参数当前未使用，表达式求值使用的是 `ctx.Ctx`。
  - `GetResult(&self, ctx) -> types::Datum`：返回 `NewIntDatum(ctx.Count)`；空输入上下文初值为 0，因此 COUNT 空集结果为 0 而非 NULL。
  - `GetPartialResult(&self, ctx) -> Vec<types::Datum>`：返回仅含最终计数 Datum 的单元素向量，供下一聚合阶段消费。
  - `CreateContext` / `ResetContext`：委托 `aggFunction`。共享实现负责建立可选 DISTINCT 检查器、清零 `Count` 并重置上下文其他通用字段。

文件没有模块级常量、自有枚举、条件编译项或私有辅助函数；模式常量和 `Aggregation` trait 均定义在 `aggregation.rs`。

## 执行流程

1. `AggFuncDesc::GetAggFunc` 遇到 `ast::AggFuncCount` 时构造 `countFunction`；来自 tipb 的分布式表达式也可由 `NewDistAggFunc` 构造，并把 PB 模式转换后保存在描述符中。
2. 执行器为每个分组调用 `CreateContext`。`aggFunction::CreateContext` 令 `Count = 0`；若 `HasDistinct`，同时创建空的 `distinctChecker`。
3. 每次 `Update` 预留参数值缓冲区并按描述符顺序执行每个参数的 `Expression::Eval`。求值错误立即通过 `?` 返回；任一结果为 NULL 则立即返回 `Ok(())`。
4. 若模式为 `FinalMode` 或 `Partial2Mode`，每个非 NULL 参数的 `GetInt64()` 被直接加到 `ctx.Count`。参数遍历结束后函数立即成功返回，不再执行逐行加一逻辑。
5. 若启用 DISTINCT，步骤 3 同时保存各参数 Datum；参数全部非 NULL 后，`distinctChecker::Check(values)` 编码整个元组。重复元组直接返回，首次出现的元组被记录并继续。
6. 若模式为 `CompleteMode` 或 `Partial1Mode`，`ctx.Count += 1`。`DedupMode` 不命中任何增加计数的分支，因此本文件当前只完成求值/可选去重而保持计数不变；`count_test.rs::dedup_mode_does_not_produce_a_count` 固化了这一事实。
7. 消费结果时，`GetResult` 返回当前计数；部分阶段调用 `GetPartialResult` 得到一个计数列，供 Final/Partial2 阶段相加。分组复用时调用 `ResetContext`，计数归零且 DISTINCT 集合重建。

## 数据与状态

`countFunction` 本身只保存不可变语义上的聚合描述符；运行中变化的数据位于调用方提供的 `AggEvaluateContext`：

- `Count: i64` 是唯一影响 COUNT 输出的字段。Complete/Partial1 以每个合格输入增加 1，Final/Partial2 以局部结果的整数值增加。
- `Ctx: Arc<dyn expression::EvalContext>` 为参数表达式求值及 DISTINCT 编码错误策略提供上下文。
- `DistinctChecker: Option<distinctChecker>` 仅在 `HasDistinct` 时创建；其 `MVMap` 保存已见编码键，`key` 缓冲在检查间复用（`util.rs`）。
- `Value`、`Buffer`、`BufferInitialized`、`GotFirstRow` 是通用聚合上下文的其他字段，COUNT 不读取它们；共享 Reset 会一并恢复，以保证上下文可安全改作新分组。

关键不变量是：一次普通阶段更新最多增加 1；遇到任一 NULL 或重复 DISTINCT 元组增加 0；合并阶段增加输入携带的局部计数。`GetPartialResult` 始终恰好返回一个 Datum。计数采用有符号 64 位整数，与 `typeInfer4Count` 生成的非 NULL `TypeLonglong` 返回类型一致。

## 依赖与调用关系

上游构造边由 RustCodeGraph 确认：

- `aggregation.rs::NewDistAggFunc -> countFunction`：mock TiKV/分布式 PB 表达式入口，把 `tipb::ExprType::Count` 映射为 COUNT 实现。
- `descriptor.rs::AggFuncDesc::GetAggFunc -> countFunction`：由已经完成类型推断和模式设置的描述符构造常规运行时对象。

运行时通过 `Box<dyn Aggregation>` 间接调用本文件的方法。`aggregation.rs` 定义 trait、`AggEvaluateContext`、`AggFunctionMode` 与共享 `aggFunction::{CreateContext, ResetContext}`；`descriptor.rs::Split` 为两阶段 COUNT 生成 Partial1/Final 描述符及 Final 输入列。

主要下游依赖为：`expression::Expression::Eval`（逐参数求值）、`chunk::Row`（当前输入行）、`types::Datum`（参数和输出载体）、`distinctChecker::Check`（DISTINCT 元组判重）以及 `stmtctx::StatementContext`（trait 形参，本实现未使用）。Cargo 清单把这些能力分别接到本地 `expression`、`chunk`、`codec`、`mvmap`、`stmtctx` 和 datum/field 类型 crate；PB 构造路径还依赖固定 revision 的 `tipb`。

## 错误处理与边界

- 参数表达式求值失败时，`Update` 原样传播 `Expression::Eval` 返回的聚合错误，不修改后续参数或执行后续计数步骤。
- DISTINCT 编码失败由 `distinctChecker::Check` 交给 `EvalContext::ErrCtx().HandleError`：若策略返回错误则转换为 `expression::Error`；若策略吞掉错误，检查器继续以当次得到的键判重。该行为来自 `util.rs`，本文件只用 `?` 传播最终结果。
- NULL 是正常控制流而非错误。任一参数为 NULL 都会提前结束本行；在约定的 Final COUNT 单参数形态下不会产生部分更新。若非标准地给 Final/Partial2 配置多个参数，当前代码会边遍历边累加，后续参数为 NULL 时不会回滚此前已加值；描述符拆分正常只生成一个 Final 计数参数（`aggregation_aster_unit_test.rs::split_count_builds_owned_partial_and_final_descriptors`）。
- Final/Partial2 直接调用 `Datum::GetInt64`，本文件不做类型转换或合法性校验；正确类型由上游描述符和部分结果契约保证。
- `DedupMode` 当前不会递增 `Count`，并有独立 Rust 回归测试。新增模式时不能依赖兜底分支，必须明确决定它应当“加一、合并还是只去重”。
- 计数和局部计数合并使用普通 `i64 +=`，本文件没有显式溢出处理；扩展超大计数策略时需同时核对 Go 兼容性和构建配置下的整数溢出行为。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务或外部句柄。`Update` 需要 `&mut self` 和 `&mut AggEvaluateContext`，单个聚合器/分组状态应由执行器串行独占更新；代码本身不为多个线程并发更新同一上下文提供同步。

`EvalContext` 通过 `Arc` 共享，COUNT 只借用它进行表达式求值；可变计数和 DISTINCT 集合仍归每个 `AggEvaluateContext` 独占。DISTINCT 的 `MVMap` 生命周期与分组上下文一致：`CreateContext` 创建，连续 Update 累积，`ResetContext` 丢弃旧检查器并重建，随后随上下文释放。普通 COUNT 不创建检查器，参数 Datum 与 `values` 向量仅存活于单次 Update。

## 与 Go 版本的对应关系

直接对照 `pkg/expression/aggregation/count.go`，Rust 保留了 Go 的核心顺序和语义：逐参数 Eval、任一 NULL 跳过、Final/Partial2 累加 `GetInt64`、DISTINCT 对完整参数元组判重、Complete/Partial1 加一、空集返回 0、部分结果为单元素切片/向量。字段嵌入在 Rust 中改为显式 `aggFunction` 字段，trait 实现对应 Go 的 `Aggregation` 接口方法。

可见实现差异有两点。第一，Go 仅在 DISTINCT 时分配 `datumBuf`，Rust 当前无条件以参数个数创建 `Vec`，但只有 DISTINCT 时才 push，属于分配策略差异而非结果语义差异。第二，Go 的 COUNT Reset 只恢复 DISTINCT、Ctx 和 Count；Rust 委托共享 `aggFunction::ResetContext`，还清理通用 Value/Buffer/GotFirstRow 字段。这些字段不参与 COUNT 结果，因而对合法 COUNT 上下文等价，同时让通用上下文恢复更彻底。

测试对照也保持一致：Go `aggregation_test.go::TestCount` 验证初值 0、加权输入 5050、NULL 不改变结果、部分结果 5050 与 DISTINCT 结果 100；Rust `aggregation_aster_unit_test.rs::count_matches_go_null_distinct_and_final_mode_cases` 覆盖相同核心场景并额外验证 FinalMode 合并为 31，`aggregation_test.rs::TestCount` 转发该共享用例。`count_test.rs` 另验证 Rust 当前 DedupMode 行为。

## 扩展指南

- 修改计数规则、模式分支或 NULL/DISTINCT 顺序时，入口是 `countFunction::Update`。保持 Go `count.go` 同步，并优先扩展独立测试 `count_test.rs`；通用兼容场景同步更新 `aggregation_aster_unit_test.rs`，必要时由 `aggregation_test.rs::TestCount` 继续转发。
- 修改输出布局时需同时更新 `GetResult`、`GetPartialResult`、`AggFuncDesc::Split` 及 Final/Partial2 消费规则；当前跨阶段契约是“单个非 NULL INT64 计数列”。
- 修改上下文初始化或复用行为应优先评估共享 `aggFunction::{CreateContext, ResetContext}`，并确认不会影响 AVG、SUM、GROUP_CONCAT 等同用 `AggEvaluateContext` 的聚合器。
- 修改 DISTINCT 表示或比较语义应在 `util.rs::distinctChecker` 完成，并增加多参数、NULL、编码错误和排序规则相关的独立测试；不要在 COUNT 内另建一套判重规则。
- 增加聚合模式时，应在 `AggFunctionMode`、PB 映射、描述符 Split、本文件 Update 和测试中形成闭环，特别验证新模式是否输出局部值、合并值或只执行去重。
- 性能修改需关注非 DISTINCT 路径当前的临时 Vec 分配、每参数表达式求值和 DISTINCT 编码/MVMap 内存增长；任何优化都必须保持参数求值次序、错误传播和 Go 兼容语义。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`files --filter pkg/expression/aggregation/count.rs` 确认目标有 8 个符号，`node --file ...` 读取完整 83 行实现。
- RustCodeGraph 调用证据：`node countFunction` 给出 Rust 构造调用者 `aggregation.rs::NewDistAggFunc` 与 `descriptor.rs::GetAggFunc`；目标文件的文件级使用者还包括 `avg.rs`，其共享的是 COUNT 类型/逻辑组合关系，而 COUNT 的两个直接构造边以上述调用者为准。
- 已核对生产代码：`count.rs`、`aggregation.rs`、`descriptor.rs`、`base_func.rs`、`util.rs`、`lib.rs`。
- 已核对 crate 配置：`pkg/expression/aggregation/Cargo.toml`，包括本地表达式、行、编码、MVMap、语句上下文与类型依赖，以及 `package.metadata.porting.go-package = "pkg/expression/aggregation"`。
- 已核对 Go 对照：`pkg/expression/aggregation/count.go`、`pkg/expression/aggregation/aggregation_test.go::TestCount`。
- 已核对 Rust 独立测试：`count_test.rs::dedup_mode_does_not_produce_a_count`、`aggregation_aster_unit_test.rs::count_matches_go_null_distinct_and_final_mode_cases`、`split_count_builds_owned_partial_and_final_descriptors`、`aggregate_type_inference_matches_go_contract`，以及转发入口 `aggregation_test.rs::TestCount`。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证本文恰好包含 11 个固定二级章节，并人工复核关键陈述均能回溯到上述符号或文件。
