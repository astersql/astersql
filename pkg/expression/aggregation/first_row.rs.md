# `pkg/expression/aggregation/first_row.rs`

## 文件定位

本文件实现行式聚合框架中的 `FIRST_ROW` 运行时聚合器。它属于 Cargo 包 `astersql-expression-aggregation`（`pkg/expression/aggregation/Cargo.toml`），由模块入口 `pkg/expression/aggregation/lib.rs` 以 `mod first_row` 纳入并通过 `pub use first_row::*` 在 crate 内外导出。

它不负责 SQL 解析、返回类型推断或聚合描述符构造；这些工作分别由上游 AST/表达式层和同 crate 的 `base_func.rs`、`descriptor.rs` 完成。本文件接收已经构造好的 `aggFunction`，实现 `aggregation.rs` 定义的 `Aggregation` trait，在执行器逐行送入数据时保存第一行参数值。

## 核心职责

- `firstRowFunction::Update` 对每个分组只求值一次：第一次调用保存唯一参数的值并置位 `GotFirstRow`，后续行直接成功返回。
- 首行值即使为 SQL `NULL` 也会被保存并锁定；`GotFirstRow` 而不是 `Datum` 是否为空负责区分“尚无输入”和“第一行恰为 NULL”。
- `GetResult` 返回已保存值，`GetPartialResult` 将同一个结果包装成单元素 `Vec<Datum>`，供统一的部分聚合接口使用。
- `CreateContext` 复用公共 `aggFunction::CreateContext` 初始化分组状态；`ResetContext` 则刻意只替换表达式上下文并清除首行标记，以匹配 Go 版本的状态复用行为。

## 主要符号

- `pub struct firstRowFunction { pub aggFunction: aggFunction }`：唯一的生产类型。字段保存 `AggFuncDesc`，其中包括函数名、参数表达式、聚合模式和 DISTINCT 标记。类型公开是为了由 `descriptor.rs` 和 `aggregation.rs` 的工厂构造；其具体行为通常经 `Box<dyn Aggregation>` 使用。
- `impl Aggregation for firstRowFunction`：实现行式聚合统一契约。
  - `Update(&mut self, &mut AggEvaluateContext, &StatementContext, Row) -> Result<(), Error>`：首行捕获入口。`StatementContext` 在本实现中未使用。
  - `GetResult(&AggEvaluateContext) -> Datum`：克隆上下文中的 `Value`，不暴露内部可变引用。
  - `GetPartialResult(&AggEvaluateContext) -> Vec<Datum>`：返回恰好一个元素，内容与 `GetResult` 相同。
  - `CreateContext(Arc<dyn EvalContext>) -> AggEvaluateContext`：委托公共基类创建完整状态。
  - `ResetContext(Arc<dyn EvalContext>, &mut AggEvaluateContext)`：更新 `Ctx` 并将 `GotFirstRow` 设为 `false`，不清空 `Value` 或其他公共字段。
- 本文件没有模块级常量、自由函数、条件编译项或自定义错误类型。

## 执行流程

1. 普通规划路径由 `AggFuncDesc::GetAggFunc`（`descriptor.rs`）在名称为 `ast::AggFuncFirstRow` 时构造 `firstRowFunction`；下推路径由 `NewDistAggFunc`（`aggregation.rs`）把 `tipb::ExprType::First` 映射为同一 AST 名称后构造它。
2. 调用方为新分组调用 `CreateContext`。公共实现将 `Value` 初始化为默认 `Datum`、将 `GotFirstRow` 初始化为 `false`，并保存共享的表达式求值上下文。
3. 每行到达 `Update` 时先检查 `GotFirstRow`。若已经处理过首行，立即返回 `Ok(())`，因此不会再次求值参数表达式，也不会让后续错误或值覆盖首行结果。
4. 首次更新要求描述符恰有一个参数；数量不为一时返回 `Wrong number of args for AggFuncFirstRow`。
5. 参数数量正确时调用 `Args[0].Eval(ctx.Ctx.as_ref(), row)`。求值成功后把所得 `Datum`（包括 NULL）写入 `ctx.Value`，再将 `GotFirstRow` 置为 `true`。
6. 最终读取通过 `GetResult` 完成；需要部分结果时，`GetPartialResult` 返回包含该值的单元素向量。
7. 上下文复用于下一分组时，`ResetContext` 先替换 `Ctx`、再清除 `GotFirstRow`。旧 `Value` 暂时保留，但下一次成功的 `Update` 会覆盖它；在新分组尚未更新前读取结果会看到旧值，这是与 Go 实现一致、要求调用方遵守“重置后先更新再读取”的生命周期约束。

## 数据与状态

`firstRowFunction` 自身只持有不可变使用的聚合描述符；每个分组的可变数据全部在 `AggEvaluateContext` 中。此实现直接读写的字段是：

- `Ctx: Arc<dyn expression::EvalContext>`：参数表达式求值所需环境；创建及重置时由调用方提供。
- `Value: types::Datum`：保存第一行唯一参数的求值结果。`GetResult` 克隆它，`GetPartialResult` 再把克隆值放进新向量。
- `GotFirstRow: bool`：真正的状态哨兵。它保证 NULL 首行也会阻止后续行覆盖结果。

公共上下文还包含 `DistinctChecker`、`Count`、`Buffer` 和 `BufferInitialized`，但 FIRST_ROW 的更新流程不使用它们。尤其是专用 `ResetContext` 不调用 `aggFunction::ResetContext`，所以这些字段及 `Value` 都被保留；`first_row_test.rs` 明确验证了 `Value` 的保留。

## 依赖与调用关系

上游接线有两条：

- `descriptor.rs::AggFuncDesc::GetAggFunc`：常规表达式描述符工厂，`ast::AggFuncFirstRow` 分支返回 `Box<dyn Aggregation>`。
- `aggregation.rs::NewDistAggFunc`：分布式/PB 工厂，先把 `tipb::ExprType::First` 映射成 `ast::AggFuncFirstRow`，再装箱 `firstRowFunction`；这是 mock TiKV 等消费下推聚合表达式的入口。

本文件的直接下游依赖是：`aggregation.rs` 中的 `Aggregation`、`AggEvaluateContext` 和 `aggFunction`；`expression::Expression::Eval`；`chunk::Row`；`stmtctx::StatementContext`；`types::Datum`；以及用于共享求值上下文所有权的 `std::sync::Arc`。这些本地 crate 依赖由 `Cargo.toml` 中的 `expression`、`chunk`、`stmtctx` 和 datum/type 门面依赖声明；`tipb` 只出现在上游 PB 工厂，并非本文件直接导入。

RustCodeGraph 的文件节点显示 `first_row.rs` 被 `aggregation.rs`、`descriptor.rs` 使用；精确名称查询同时定位到 Rust/Go 两个 `firstRowFunction`。由于 trait 对象动态分派不会把每次 `Update` 调用稳定归因到具体实现，工厂分支和独立测试是具体运行调用链的补充证据。

## 错误处理与边界

- 参数数不是 1 时，在索引参数之前返回 `expression::errors::New(...)`，避免越界；正常构造流程通常已约束参数，本检查仍是运行时防线。
- 唯一参数的 `Eval` 错误通过 `?` 原样向上传播。失败发生时 `Value` 不会赋新值，`GotFirstRow` 也不会置位，因此调用方若选择继续，可对另一行重试。
- 第一行求值得到 NULL 不是错误；NULL 被保存且 `GotFirstRow` 置位，后续非 NULL 行不会替换它。
- 没有输入时，公共新上下文中的默认 `Datum` 作为结果；它对应空状态的 NULL 表示。
- 已获得首行后，参数数量检查和表达式求值都会被短路。这是“第一行决定结果”的必要行为，也意味着后续行中的潜在表达式错误不会由此聚合器触发。
- `GetPartialResult` 每次分配一个单元素 `Vec`；值本身通过 `Datum::clone` 返回，不借用上下文。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部资源。`Arc<dyn EvalContext>` 只提供求值上下文的共享所有权；聚合状态仍通过 `&mut AggEvaluateContext` 串行修改，`firstRowFunction::Update` 也接收 `&mut self`。因此实现没有声明同一个分组上下文可被并发更新，调度层必须为每个活动分组维护并正确隔离其状态。

生命周期顺序是“工厂构造聚合器 → `CreateContext` 创建分组状态 → 零次或多次 `Update` → 读取结果 → 可选 `ResetContext` 复用状态”。重置不释放 `Value`、缓冲或 DISTINCT 状态，只更换 `Ctx` 并重新开放首行捕获；这减少了本实现的重置动作，但也要求调用方不要把重置后、首次成功更新前的旧值当作新分组结果。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/aggregation/first_row.go`。Rust 保留了 Go 的结构与方法边界：同名 `firstRowFunction` 包含公共 `aggFunction`；`Update` 先检查 `GotFirstRow`，再校验单参数、求值、保存并置位；`GetResult` 返回保存值；`GetPartialResult` 返回单元素切片/向量；`ResetContext` 仅更新求值上下文并清除首行标记。

实现语言层面的差异不改变语义：Go 通过 `value.Copy(&evalCtx.Value)` 显式复制 `Datum`，Rust 将求值返回的拥有值赋给 `ctx.Value`；Go 的 `GetResult` 按值返回，Rust 用 `clone` 达到相同的所有权隔离；Go 使用接口值保存上下文，Rust 使用 `Arc<dyn EvalContext>`。

测试对应关系如下：Go 的 `aggregation_test.go::TestFirstRow` 验证输入 1、2 后结果和部分结果仍为 1；Rust 的 `aggregation_test.rs::TestFirstRow` 转发到 `aggregation_aster_unit_test.rs::first_row_max_and_min_match_go_row_order_and_null_cases`，验证同一顺序行为及单元素部分结果。Rust 独立文件 `first_row_test.rs` 额外固定了 Go `ResetContext` 的细节：清除 `GotFirstRow` 但保留既有 `Value`。

## 扩展指南

- 修改“首行”判定、NULL 行处理或求值时机，应集中在 `firstRowFunction::Update`，并同步扩展 `aggregation_aster_unit_test.rs` 中的共享用例；特别要加入“第一行 NULL、第二行非 NULL 仍返回 NULL”和“首次求值失败后状态未置位”的回归覆盖。
- 修改部分聚合编码时应同时检查 `GetPartialResult`、`NewDistAggFunc` 的 `tipb::ExprType::First` 分支、`agg_to_pb.rs` 的双向映射及 `agg_to_pb_test.rs`。FIRST_ROW 当前只有一个部分结果列，改变形状会影响分布式阶段兼容性。
- 修改重置语义时必须同时更新 `ResetContext` 与独立的 `first_row_test.rs`，并先核对 Go 文件；不要无意改为公共 `aggFunction::ResetContext`，后者会清空更多字段和 `Value`。
- 若新增字段，应判断它属于聚合器级描述信息还是分组级可变状态。后者应进入 `AggEvaluateContext` 或专用状态，并确保不同分组隔离；不要把分组状态放在共享聚合器上。
- 测试逻辑应继续放在独立的 `first_row_test.rs` 或现有聚合测试文件中，不要内嵌进生产源文件。兼容风险主要是 Go 行为偏离和 PB 部分结果形状变化；性能风险主要是取消后续行短路、重复求值昂贵表达式或增加每行分配。

## 验证依据

- 源码与模块：`pkg/expression/aggregation/first_row.rs`、`aggregation.rs`、`descriptor.rs`、`lib.rs`、`Cargo.toml`。
- Go 对照：`pkg/expression/aggregation/first_row.go`、`aggregation_test.go::TestFirstRow`。
- Rust 测试：`pkg/expression/aggregation/first_row_test.rs::reset_context_matches_go_and_preserves_accumulated_value`、`aggregation_test.rs::TestFirstRow`、`aggregation_aster_unit_test.rs::first_row_max_and_min_match_go_row_order_and_null_cases`、`agg_to_pb_test.rs` 中的 FIRST_ROW/`tipb::ExprType::First` 映射用例。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query firstRowFunction --json` 定位 Rust 结构为 `first_row.rs::firstRowFunction`；目标文件节点报告由 `aggregation.rs` 与 `descriptor.rs` 使用；`query GetAggFunc --json` 和 `query NewAggFuncDesc --json` 定位描述符构造链。对同名 `Update`/`ResetContext` 的宽泛探索会混入仓库其他模块，因此具体动态分派关系以目标文件节点、精确工厂分支和测试调用为准。
- 人工复核结论：本文件存在是为了在统一行式聚合接口下保存每组第一行的参数值；安全扩展必须保持 NULL 也算首行、成功后短路、求值错误不置位、部分结果为单值以及 Go 式重置行为。
