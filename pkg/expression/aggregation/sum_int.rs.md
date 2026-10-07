# `pkg/expression/aggregation/sum_int.rs`

## 文件定位

`sum_int.rs` 位于 `astersql-expression-aggregation` crate，提供行式 `SUM_INT` 的运行时聚合器。模块入口 `pkg/expression/aggregation/lib.rs` 以 `mod sum_int` 装配该文件，并通过 `pub use sum_int::*` 将实现暴露给同 crate 的工厂。它不是 SQL 名称解析或类型推断入口：`base_func.rs::typeInfer4SumInt` 先约束参数必须恰好为一个整数并将返回类型设为 `LONG LONG`，本文件只消费已经构造好的 `AggFuncDesc`，逐行计算整数和。

该实现有两条直接构造路径。`aggregation.rs::NewDistAggFunc` 将下推的 `tipb::ExprType::SumInt` 转成 `ast::AggFuncSumInt` 并装箱 `sumIntFunction`，用于 PB/分布式行式聚合；`descriptor.rs::AggFuncDesc::GetAggFunc` 则从规划描述符按名称实例化同一实现。RustCodeGraph 对 `sumIntFunction` 的实例化边也指向这两个符号。

## 核心职责

- `sumIntFunction::Update` 对描述符第一个参数调用 `EvalInt`，跳过 `NULL`，并依据参数类型的 `UNSIGNED` 标志选择 `u64` 或 `i64` 累加路径。
- 当 `aggFunction.CreateContext` 因 `AggFuncDesc.HasDistinct` 创建了 `DistinctChecker` 时，`Update` 先以保持有符号性的 `Datum` 做去重；重复值不改变和值与计数。
- 累加使用 `types::AddUint64`/`types::AddInt64` 的检查加法，不以 Rust 的回绕或 panic 作为 SQL 结果。
- `GetResult` 返回当前 `Datum`，`GetPartialResult` 把同一个结果包装为单元素数组，供部分聚合结果传递。
- `CreateContext` 与 `ResetContext` 委托给公共 `aggFunction`，确保空输入结果为 `NULL`，并正确创建或重建 DISTINCT 状态。

## 主要符号

- `pub struct sumIntFunction { pub aggFunction: aggFunction }`：唯一的文件级类型。字段保存名称、参数、模式和 `HasDistinct` 等完整 `AggFuncDesc`；类型通过 `lib.rs` 再导出，但通常由工厂以 `Box<dyn Aggregation>` 使用。
- `impl Aggregation for sumIntFunction`：实现 `aggregation.rs::Aggregation` 的五个必需方法，没有文件级常量、自由函数、条件编译项或额外 trait。
- `Update(&mut self, ctx, _sc, row) -> Result<(), Error>`：核心状态转换。`_sc` 当前未参与整数求和；表达式求值使用 `ctx.Ctx`。
- `GetResult(&self, ctx) -> types::Datum`：克隆 `ctx.Value`，因此调用方获得独立的 `Datum`，不会借用或移走分组状态。
- `GetPartialResult(&self, ctx) -> Vec<types::Datum>`：返回 `[GetResult(ctx)]`。与 AVG 等多列部分结果不同，SUM_INT 的部分结果只有和值。
- `CreateContext`/`ResetContext`：分别转发到 `aggFunction::CreateContext` 和 `aggFunction::ResetContext`；后者会把 `Count` 清零、`Value` 设为 `NULL`、清空通用缓冲，并按 `HasDistinct` 重建去重器。

## 执行流程

1. `NewDistAggFunc` 或 `AggFuncDesc::GetAggFunc` 构造 `sumIntFunction`；执行器为每个聚合分组调用 `CreateContext`。公共上下文初始 `Count = 0`、`Value = NULL`，仅 DISTINCT 描述符带 `DistinctChecker`。
2. 每行进入 `Update` 后，代码从 `AggFuncDesc.Args[0]` 取参数并执行 `EvalInt(ctx.Ctx, row)`。求值错误以 `?` 立即返回；`is_null` 为真则成功跳过该行。
3. 代码读取同一参数的字段类型标志。若有 `mysql::UnsignedFlag`，把 `EvalInt` 返回的位模式转换为 `u64`；否则保持 `i64`。
4. 若上下文存在去重器，以 `NewUintDatum(value)` 或 `NewIntDatum(value)` 检查当前值。检查错误向上传播；已出现的值直接返回，不累计。
5. 若 `ctx.Value` 仍为 `NULL`，首个有效值直接成为总和；否则分别调用 `AddUint64` 或 `AddInt64` 做检查加法。成功后写回对应类型的 `Datum`。
6. 只有实际纳入和值的非空、非重复行才令 `ctx.Count += 1`。随后返回成功。
7. 最终结果直接取 `Value`；部分结果则返回单元素 `Vec`。空输入、全 NULL 或没有任何通过 DISTINCT 的值保持 `NULL`。

## 数据与状态

`sumIntFunction` 自身只持有不可分组的描述信息；每个分组的可变状态全部位于 `AggEvaluateContext`。本文件实际使用其中四项：`Ctx` 提供表达式求值环境，`DistinctChecker` 保存已见键集合，`Value` 保存带 signed/unsigned 种类的累计值，`Count` 记录已纳入的行数。`Buffer`、`BufferInitialized` 和 `GotFirstRow` 是公共上下文为其他聚合保留的字段，本实现不读取它们。

关键不变量是 `Value` 的 Datum 种类与参数无符号标志一致：无符号分支只调用 `GetUint64`/`SetUint64`，有符号分支只调用 `GetInt64`/`SetInt64`。首个有效输入前 `Value` 必须为 `NULL`，从而 SQL 空集语义不会被零值替代。`Count` 不参与 `GetResult`，但仍与公共聚合上下文契约一致，只统计真正累计的值。

## 依赖与调用关系

上游直接调用关系为：

- `aggregation.rs::NewDistAggFunc`：`tipb::ExprType::SumInt -> ast::AggFuncSumInt -> sumIntFunction`，保留 PB 聚合模式并返回 trait object 与描述符。
- `descriptor.rs::AggFuncDesc::GetAggFunc`：`Name == ast::AggFuncSumInt` 时构造该类型，服务本地行式聚合。
- `lib.rs`：声明并再导出模块；`Cargo.toml` 将 crate 命名为 `astersql-expression-aggregation`，且以 `package.metadata.porting.go-package = "pkg/expression/aggregation"` 标明 Go 对照包。

下游依赖为 `expression::Expression::EvalInt/GetType`、`mysql::HasUnsignedFlag`、`distinctChecker::Check`、`types::{NewIntDatum, NewUintDatum, AddInt64, AddUint64, Datum}`，以及公共 `aggFunction::{CreateContext, ResetContext}`。这些类型来自本 crate 的门面及 Cargo 中的 `expression`、`parser-mysql-dependency`、`datum-dependency`、`file-dependency`、`chunk`、`stmtctx` 等依赖；本文件没有网络、磁盘、事务或调度器依赖。

更外层的 PB 编码由 `agg_to_pb.rs` 将 `AggFuncSumInt` 映射为 `tipb::ExprType::SumInt`。`aggregation_aster_unit_test.rs` 和 `agg_to_pb_test.rs` 证明该描述符可下推到 TiKV/TiFlash；SQL 层 Go 用例 `pkg/executor/test/aggregate/aggregate_test.go` 验证 mock cop 计划中确实出现 `sum_int` 并得到整数结果。

## 错误处理与边界

- 参数表达式求值失败：`EvalInt(...)?` 原样向上传播 crate 的 `Error`；`NULL` 不是错误，且不改变任何状态。
- DISTINCT 检查失败：`checker.Check(...)?` 立即返回。检查成功但值重复也不改变 `Value` 或 `Count`。
- 溢出：有符号和无符号分别由 `AddInt64`、`AddUint64` 检测；底层错误被转换成 `expression::errors::New(error.to_string())`。因此超界不会提交新的总和值或递增计数。
- 参数形状：本文件直接索引 `Args[0]`，自身不防御空参数。正常入口依赖 `base_func.rs::typeInfer4SumInt` 保证一个整数参数；绕过描述符构造契约可能 panic，不能把该内部类型当成容错解析边界。
- signed/unsigned：无符号输入先从 `EvalInt` 的 `i64` 表示转换为 `u64`，再以无符号 Datum 去重和求和；不能把两条分支合并为有符号加法，否则会破坏高位值语义。
- 验证缺口：同目录没有专门调用 `sumIntFunction::Update` 来覆盖 signed/unsigned 溢出及上下文重置的独立 Rust 单测。现有 Rust 测试主要覆盖描述符、PB 映射和下推分类；SQL 行为的 signed/unsigned、DISTINCT、NULL 与 mock cop 证据主要来自 `pkg/executor/test/aggregate/aggregate_test.go`。因此文档不宣称溢出分支已有直接回归覆盖。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或外部资源。`Update` 需要 `&mut self` 与 `&mut AggEvaluateContext`，其设计单位是单个执行器拥有的单个分组状态；并行聚合必须由上层为不同 worker/分组提供独立实例或上下文，而不是并发修改同一个上下文。

求值环境以 `Arc<dyn expression::EvalContext>` 存在 `AggEvaluateContext.Ctx` 中，创建上下文时克隆所有权，重置时替换为新环境。DISTINCT 检查器与分组上下文同生命周期；`ResetContext` 丢弃旧去重集合并新建一个，避免跨分组泄漏已见值。结果 Datum 和单元素部分结果均为拥有型值，不延长对上下文的借用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/aggregation/sum_int.go`。Rust 的 `sumIntFunction`、`Update`、`GetResult`、`GetPartialResult` 与 Go 同名实现保持相同分支：先 `EvalInt` 并跳过 NULL，依据字段无符号标志选择整数域，DISTINCT 时用匹配 signedness 的 Datum 去重，使用 `types.AddUint64`/`AddInt64` 检查累加，最后只为接纳值增加 `Count`。

Rust 将 Go 的嵌入字段 `aggFunction` 改成显式公有字段，并因 Rust trait 要求在本文件补充 `CreateContext`/`ResetContext` 转发；Go 通过嵌入类型继承这些方法。Go 以 `sf.HasDistinct` 决定是否检查去重器，Rust 以 `ctx.DistinctChecker` 是否存在为准；正常上下文由公共创建/重置逻辑按同一 `HasDistinct` 构造，故正规入口语义一致。Rust 还显式把整数加法错误转成 expression error 文本，而 Go 直接返回 types 层错误；两者都在写回状态和增加计数前失败。

Go 测试 `pkg/expression/aggregation/aggregation_test.go::TestCheckAggPushDownSumInt` 与 Rust `aggregation_test.rs::TestCheckAggPushDownSumInt`/共享用例共同核对 TiKV、TiFlash 下推分类；两侧 `agg_to_pb_test` 都核对 DISTINCT 标志和 `tipb::SumInt` 编码。Go SQL 测试进一步覆盖 signed/unsigned DISTINCT、全 NULL 与 mock cop 下推，但当前 Rust 同名 SQL 测试使用简化聚合模型，不能替代对本文件所有内部分支的直接测试。

## 扩展指南

- 若改变逐行整数语义，应修改 `sumIntFunction::Update`，并保持 signed/unsigned 的 Datum、去重键和检查加法三者一致；同步新增同目录独立测试文件（例如 `sum_int_test.rs`），不要把测试内嵌进生产源文件。至少覆盖空输入、NULL、重复值、负数、`u64` 高位、两种边界溢出、错误后状态不变以及 `ResetContext` 后 DISTINCT 集合清空。
- 若改变参数数量、允许类型或返回类型，应从 `base_func.rs::typeInfer4SumInt` 入手，并同步 Go 对照语义与 `aggregation_aster_unit_test.rs::aggregate_type_inference_matches_go_contract`；不要仅在 `Update` 中做临时转换。
- 若改变构造或分布式阶段行为，应同时检查 `descriptor.rs::GetAggFunc`、`aggregation.rs::NewDistAggFunc`、`agg_to_pb.rs` 及其独立测试，保证本地构造、PB 编解码和 TiKV/TiFlash 下推仍一致。
- 若改变部分结果形状，必须同时调整 `GetPartialResult`、描述符拆分/Final 模式消费方和跨阶段协议。当前单元素 `[sum]` 是调用契约，不应只改一端。
- 性能敏感点是每行的类型标志读取、DISTINCT Datum/Vec 分配和溢出检查。优化时需要证明不会缓存失效的字段类型、不会混淆有符号键，并用直接基准或测试守住错误语义。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`node --file pkg/expression/aggregation/sum_int.rs` 展示完整 89 行源码，并报告该文件被 `aggregation.rs`、`descriptor.rs` 使用。`query/node sumIntFunction` 显示 Rust 实例化者为 `NewDistAggFunc` 与 `GetAggFunc`。
- 已读生产代码：`pkg/expression/aggregation/sum_int.rs`、`aggregation.rs`（`Aggregation`、`AggEvaluateContext`、`aggFunction`、`NewDistAggFunc`）、`descriptor.rs`（`AggFuncDesc::GetAggFunc`）、`base_func.rs` 的 SUM_INT 类型推断引用、`lib.rs` 与 `Cargo.toml`。
- Go 对照：`pkg/expression/aggregation/sum_int.go`，以及构造/类型/下推相关的 `aggregation.go`、`descriptor.go`、`base_func.go`、`agg_to_pb.go` 搜索证据。
- 已读测试：`pkg/expression/aggregation/aggregation_test.rs`、`aggregation_aster_unit_test.rs`、`agg_to_pb_test.rs` 及其 Go 对照；另读 `pkg/executor/test/aggregate/aggregate_test.go` 和 Rust 同名文件中的 SUM_INT 用例，以区分真实 SQL/mock cop 证据与简化 Rust 聚合模型。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的 `test -f` 与固定标题计数命令验证文档存在且恰有 11 个二级章节，并人工复核所有行为结论均可回指上述符号或文件。
