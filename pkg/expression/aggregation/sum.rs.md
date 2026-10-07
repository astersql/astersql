# `pkg/expression/aggregation/sum.rs`

## 文件定位

本文件是 `astersql-expression-aggregation` crate 中行式 `SUM` 聚合的薄运行时适配层。源码见 [`sum.rs`](sum.rs)；[`lib.rs`](lib.rs) 以私有模块 `mod sum` 装入它，再通过 `pub use sum::*` 导出 `sumFunction`；crate 边界和依赖见同目录 [`Cargo.toml`](Cargo.toml)。它不负责类型推断、表达式构造或具体数值相加，而是实现 [`aggregation.rs`](aggregation.rs) 定义的 `Aggregation` trait，并把这些公共职责委托给内嵌的 `aggFunction`。

运行时有两条直接构造路径：`aggregation.rs::newDistAggFunc` 在下推表达式类型为 `tipb::ExprType::Sum` 时构造 `sumFunction`；`descriptor.rs::AggFuncDesc::GetAggFunc` 在描述符名称为 `ast::AggFuncSum` 时构造它。因而本文件位于“聚合描述符/下推表达式已经完成解析”与“逐行维护分组状态”之间，而不是 SQL 语法解析入口。

## 核心职责

- 用 `sumFunction` 把一个 `aggFunction` 聚合描述符包装为 `dyn Aggregation` 可调用对象。
- 在 `Update` 中从 `StatementContext` 取得类型转换上下文，委托 `aggFunction::updateSum` 完成参数求值、NULL 跳过、DISTINCT 判重、类型提升与累加。
- 让最终结果和单列部分结果都直接读取 `AggEvaluateContext::Value`。
- 复用 `aggFunction::CreateContext` 与 `ResetContext` 管理每个分组的状态，而不在本文件复制状态初始化逻辑。

本文件没有自定义常量、枚举、条件编译项或自由函数；其唯一生产类型是公开结构体 `sumFunction`，唯一实现块是 `impl Aggregation for sumFunction`。

## 主要符号

- `pub struct sumFunction { pub aggFunction: aggFunction }`：`SUM` 的运行时对象。字段公开是当前 crate 对象构造方式的一部分；描述符、参数、模式和 DISTINCT 标志实际保存在 `aggFunction::AggFuncDesc` 中。
- `Update(&mut self, ctx, sc, row) -> Result<(), Error>`：每输入一行调用一次。它把 `sc.TypeCtx()`、当前分组的 `AggEvaluateContext` 和行交给 `aggFunction::updateSum`。
- `GetResult(&self, ctx) -> types::Datum`：克隆并返回当前 `ctx.Value`。克隆避免把状态中的 Datum 所有权移出。
- `GetPartialResult(&self, ctx) -> Vec<types::Datum>`：返回恰好一个元素的向量，元素与 `GetResult` 相同；这与 AVG 的 `(count, sum)` 双列部分结果不同。
- `CreateContext(&self, Arc<dyn expression::EvalContext>) -> AggEvaluateContext`：委托基类创建新分组状态；若描述符启用 DISTINCT，同时创建去重器。
- `ResetContext(&mut self, Arc<dyn expression::EvalContext>, &mut AggEvaluateContext)`：委托基类清空并复用分组状态。

这些方法都是 trait 实现；文件没有额外的固有公开 API。

## 执行流程

1. `aggregation.rs::newDistAggFunc` 或 `descriptor.rs::AggFuncDesc::GetAggFunc` 根据 `Sum`/`AggFuncSum` 选择 `sumFunction`，并把已构造的 `aggFunction` 放入其中。
2. 执行器为一个分组调用 `CreateContext`。`aggFunction::CreateContext` 保存共享的表达式求值上下文，初始化 `Count = 0`、空/NULL `Value` 及其他通用字段，并按 `HasDistinct` 决定是否创建 `distinctChecker`。
3. 每行进入 `sumFunction::Update`，后者调用 `aggFunction::updateSum`：
   - 通过 `AggFuncDesc.Args[0].Eval` 求出第一个参数；
   - 参数为 NULL 时立即成功返回，状态不变；
   - DISTINCT 模式下将 Datum 编码为键，重复值立即跳过；
   - 调用 `util.rs::calculateSum`。整数和无符号整数先转 Decimal，Decimal 保持 Decimal，其他非 NULL 值转 `f64`，再与旧值相加；
   - 累加成功后将 `Count` 加一。`SUM` 的输出不直接使用该计数，但共享更新逻辑也服务 AVG。
4. 普通完成阶段调用 `GetResult` 取得单个 Datum；需要可继续传输/合并的表示时调用 `GetPartialResult`，得到单元素 `[sum]`。
5. 状态对象复用到下一分组时调用 `ResetContext`，把 `Value` 设为 NULL、`Count` 清零，并重建 DISTINCT 检查器。

## 数据与状态

`sumFunction` 本身只持有不可分离的描述符包装 `aggFunction`；真正随分组变化的数据位于调用方持有的 `AggEvaluateContext`。与本文件直接相关的字段如下：

- `Ctx: Arc<dyn expression::EvalContext>`：参数表达式求值和错误策略所需的共享上下文。
- `DistinctChecker: Option<distinctChecker>`：仅 `SUM(DISTINCT ...)` 使用，保存已经出现的编码键。
- `Value: types::Datum`：当前和。空输入及全 NULL 输入保持 NULL；首个有效值决定进入 Decimal 或 Double 累加路径。
- `Count: i64`：每个真正纳入求和的非 NULL、非重复输入加一。当前 `sumFunction::GetResult` 不读取它。

`GetResult` 返回 `Value.clone()`，`GetPartialResult` 再把该克隆值包装为新 `Vec`，因此读取结果不会清空或借出内部状态。Rust 共享求值上下文使用 `Arc` 管理所有权；可变聚合状态则显式通过 `&mut AggEvaluateContext` 传入。

## 依赖与调用关系

上游接线：

- `aggregation.rs::newDistAggFunc`：`tipb::ExprType::Sum -> Box<sumFunction>`，用于从下推表达式建立聚合对象。
- `descriptor.rs::AggFuncDesc::GetAggFunc`：`ast::AggFuncSum -> Box<sumFunction>`，用于从逻辑描述符建立运行时对象。
- `aggregation_aster_unit_test.rs::avg_and_sum_match_go_weighted_input_and_distinct_cases`：经 `GetAggFunc` 获得 trait 对象，再调用本文件实现的 `CreateContext`、`Update`、`GetResult` 和 `GetPartialResult`。

下游委托：

- `sumFunction::Update -> aggFunction::updateSum -> AggFuncDesc.Args[0].Eval / distinctChecker::Check / calculateSum`。
- `sumFunction::{CreateContext, ResetContext} -> aggFunction::{CreateContext, ResetContext}`。
- `GetPartialResult -> GetResult -> AggEvaluateContext::Value::clone`。

`Cargo.toml` 将本文件归入 `astersql-expression-aggregation`；直接出现于签名或共享逻辑中的 crate 别名包括 `expression`、`stmtctx`、`chunk`，而 Datum/Decimal 等通过 crate 根重导出和本地类型依赖提供。RustCodeGraph 的文件节点记录 `sum.rs` 被 `aggregation.rs`、`descriptor.rs` 两个文件使用；对同名方法的精确 callers/callees 查询受 Go/Rust 同名符号消歧限制，以上边由目标文件及两个构造点源码核验。

## 错误处理与边界

- 参数表达式求值失败时，`AggFuncDesc.Args[0].Eval` 的错误经 `updateSum` 和 `Update` 原样向上传播。
- DISTINCT 键编码或错误上下文处理失败时，`distinctChecker::Check` 返回错误，当前行不进入累加。
- 数值转换及 `ComputePlus` 失败时，`calculateSum` 将其转换为 crate 的表达式错误并向上传播；只有累加成功后才递增 `Count`。
- NULL 输入是正常边界而非错误：直接跳过。没有有效输入时 `Value` 保持 NULL，所以 `GetResult` 和唯一部分结果也为 NULL。
- 重复 DISTINCT 输入是正常跳过，不修改 `Value` 或 `Count`。
- `updateSum` 固定访问 `Args[0]`；“至少一个参数”的前置条件由 `AggFuncDesc` 构造/校验层保证，本文件不重复检查。绕开描述符构造直接制造空参数对象会越界 panic。
- `calculateSum` 只接受 NULL、Float64 或 Decimal 作为已有累计值；其他累计值种类返回 `invalid value ... for aggregate`。整数输入会先转 Decimal，因此正常 `SUM` 路径不会保留整数 Datum 作为和。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部 I/O。`sumFunction` 的方法按调用方驱动同步执行。

每个聚合分组应拥有独立的 `AggEvaluateContext`，并通过 `&mut` 更新，Rust 借用规则阻止同一状态在同一时刻被多个调用并发修改。`Arc<dyn EvalContext>` 只共享求值配置/上下文；它不把 `AggEvaluateContext::Value` 变成共享可变状态。DISTINCT 去重器及其 MVMap/编码缓冲随该分组上下文存活；`ResetContext` 会重建去重器，从而不会把上一分组的已见键泄漏到下一分组。结果 Datum 和部分结果 Vec 由克隆产生，其生命周期独立于后续状态重置。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/aggregation/sum.go`。两端都有同名 `sumFunction`，`Update` 都把 `StatementContext.TypeCtx()`、分组上下文和当前行交给 `updateSum`；`GetResult` 都返回上下文的 `Value`；`GetPartialResult` 都返回单元素结果。

Rust 版额外显式实现 `CreateContext` 和 `ResetContext`，只是因为 Rust trait 实现不能像 Go 的匿名嵌入那样自动提升 `aggFunction` 方法，行为仍委托给同一层公共逻辑。所有权上的差异是 Go 返回 Datum 值，而 Rust 明确 `clone`；Go 上下文使用接口/指针，Rust 使用 `Arc<dyn EvalContext>` 与可变借用。

语义证据也对齐：Go `aggregation_test.go::TestSum` 验证初始 NULL、加权输入和为 `338350`、NULL 不改变结果、单列部分结果及 DISTINCT 和 `5050`；Rust `aggregation_test.rs::TestSum` 转发到 `aggregation_aster_unit_test.rs::avg_and_sum_match_go_weighted_input_and_distinct_cases`，验证相同结果与边界。Rust 共享用例还同时证明 SUM 与 AVG 复用求和逻辑。

## 扩展指南

- 若只改变 `SUM` 的 trait 表面行为（例如部分结果列形状），修改本文件的相应 `Aggregation` 方法，并同步 `aggregation_aster_unit_test.rs` 中的 SUM 用例；不要把测试内嵌回 `sum.rs`。
- 若改变 NULL、DISTINCT、参数求值或计数规则，应修改 `aggregation.rs::aggFunction::updateSum`，并同时评估 AVG，因为它在 Complete/Partial1 模式也调用该方法。
- 若改变数值类型提升、溢出或转换策略，应修改 `util.rs::calculateSum`，同步覆盖 Decimal、Double、整数、转换错误及 AVG 兼容性。
- 若新增构造模式或聚合名称，需要同时检查 `aggregation.rs::newDistAggFunc`、`descriptor.rs::GetAggFunc`、`agg_to_pb.rs` 和描述符返回类型推断；仅新增结构体而不接线不会进入运行时。
- 若调整状态生命周期或 DISTINCT 实现，保持“每组独立状态、Reset 后无旧键、失败时不递增 Count”的不变量，并在独立测试文件中覆盖状态复用。
- 兼容风险主要是 MySQL 数值类型提升、空集 NULL、DISTINCT 编码语义和分布式部分结果形状；性能风险主要来自每次 `GetResult` 的 Datum 克隆以及 DISTINCT 的键编码与集合增长。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标仓库；`files --filter pkg/expression/aggregation/sum.rs` 显示该文件含 7 个符号；`node --file ... --offset 1 --limit 400` 完整读取 52 行并报告使用方为 `aggregation.rs`、`descriptor.rs`。
- 目标与模块边界：`pkg/expression/aggregation/sum.rs`、`lib.rs`、`Cargo.toml`。
- 公共状态和执行逻辑：`aggregation.rs::{Aggregation, AggEvaluateContext, aggFunction::CreateContext, aggFunction::ResetContext, aggFunction::updateSum, newDistAggFunc}`。
- 构造入口：`descriptor.rs::AggFuncDesc::GetAggFunc`。
- 数值与 DISTINCT 逻辑：`util.rs::{calculateSum, distinctChecker::Check}`。
- Rust 独立测试：`aggregation_test.rs::TestSum` 与 `aggregation_aster_unit_test.rs::avg_and_sum_match_go_weighted_input_and_distinct_cases`。
- Go 对照及测试：`sum.go`、`aggregation.go::aggFunction.updateSum`、`aggregation_test.go::TestSum`。
- 人工复核结论：该文件存在是为了把共享的 `aggFunction` 求和逻辑适配到 `Aggregation` trait；运行时由两个构造入口选择，逐行更新分组状态，安全扩展点按“trait 适配 / 共享更新 / 数值运算 / 构造接线”分层。
