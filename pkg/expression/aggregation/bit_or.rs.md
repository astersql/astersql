# `pkg/expression/aggregation/bit_or.rs`

## 文件定位

本文件实现行式聚合框架中的 SQL `BIT_OR` 运行时聚合器。crate `astersql-expression-aggregation` 由 `pkg/expression/aggregation/Cargo.toml` 定义，入口 `pkg/expression/aggregation/lib.rs` 通过 `mod bit_or` 编译本模块并以 `pub use bit_or::*` 重导出其符号。

`bitOrFunction` 实现 `pkg/expression/aggregation/aggregation.rs` 中的 `Aggregation` trait。它有两条直接构造路径：常规表达式描述符由 `AggFuncDesc::GetAggFunc` 在名称为 `ast::AggFuncBitOr` 时实例化；下推的 tipb 聚合表达式由 `NewDistAggFunc` 在 `tipb::ExprType::AggBitOr` 分支实例化。因此本文件负责的是逐行求值和状态归并，不负责 SQL 解析、返回类型推导、PB 编解码或物理执行器调度。

## 核心职责

- 为每个聚合分组建立以无符号整数 `0` 为初值的状态；`0` 是按位或的单位元，也使空输入组得到 `0`（`bitOrFunction::CreateContext`）。
- 对每一行求值第一个参数，忽略 `NULL`，把非 `u64` Datum 按语句类型上下文转换为 `i64` 后再按位解释为 `u64`，最后与累计状态执行按位或（`bitOrFunction::Update`）。
- 以单个 Datum 同时提供最终结果和部分结果；部分结果可继续被上层分布式聚合阶段合并（`GetResult`、`GetPartialResult`）。
- 在状态对象复用到另一个分组时，先委托公共实现清理上下文，再恢复 `BIT_OR` 专属初值 `0`（`ResetContext`）。

## 主要符号

- `pub struct bitOrFunction { pub aggFunction: aggFunction }`：聚合器本体。唯一字段保存 `AggFuncDesc`，其中包括参数表达式、聚合名称、模式和 DISTINCT 标记等公共描述信息。类型虽为 `pub`，命名沿用 Go；crate 根允许非 CamelCase 名称。
- `impl Aggregation for bitOrFunction`：实现统一行式聚合协议。
  - `Update(&mut self, ctx, sc, row) -> Result<(), Error>`：读取 `AggFuncDesc.Args[0]`，执行表达式并更新 `ctx.Value`。
  - `GetResult(&self, ctx) -> types::Datum`：克隆当前累计 Datum，调用者拿到独立的返回值而不借用内部状态。
  - `GetPartialResult(&self, ctx) -> Vec<types::Datum>`：返回只含最终/累计值的一列部分状态。
  - `CreateContext(&self, eval_ctx) -> AggEvaluateContext`：建立公共状态并把 `Value` 设为 `Uint64(0)`。
  - `ResetContext(&mut self, eval_ctx, state)`：清理公共状态、替换求值上下文，并把 `Value` 重新设为 `Uint64(0)`。

本文件没有模块级常量、自由函数、条件编译项或自有错误类型。

## 执行流程

1. `AggFuncDesc::GetAggFunc`，或处理下推 PB 表达式的 `NewDistAggFunc`，依据 `BIT_OR` 类型创建 `bitOrFunction`。
2. 执行器为分组调用 `CreateContext`。公共 `aggFunction::CreateContext` 填充 `Ctx`、可选的 `DistinctChecker`、计数和缓冲字段；随后本实现令 `Value = 0`。
3. 每到一行，`Update` 调用唯一参数表达式 `Args[0].Eval(ctx.Ctx.as_ref(), row)`。表达式错误直接返回，状态不会进入后续按位或步骤。
4. 若结果为 `NULL`，该行不改变状态。若 Datum 的 kind 已是 `KindUint64`，直接读取 `GetUint64()`；否则调用 `ToInt64(sc.TypeCtx())`，再用 Rust 的 `as u64` 保留该有符号整数的二进制位模式。
5. 将转换值与 `ctx.Value.GetUint64()` 做 `|`，再由 `SetUint64` 写回。按位或具有结合律和交换律，因此逐行顺序不影响最终位集合。
6. 上层通过 `GetResult` 读取单值结果，或通过 `GetPartialResult` 得到一元素数组。复用状态前调用 `ResetContext`，新分组重新从 `0` 开始。

## 数据与状态

实际可变状态位于共享的 `AggEvaluateContext`，本实现只读写其中两个方面：`Ctx` 供参数表达式求值，`Value` 保存当前 `u64` 位掩码。`Count`、`Buffer`、`BufferInitialized` 和 `GotFirstRow` 不参与 `BIT_OR` 计算；公共创建/重置仍会初始化它们。公共层可按描述符建立 `DistinctChecker`，但 `bitOrFunction::Update` 不读取它，本文件没有 DISTINCT 去重分支。

初值和重置值均为 `Uint64(0)`。这意味着没有非 NULL 输入时，`GetResult` 与 `GetPartialResult()[0]` 都是 `0`，而不是 NULL。每个非 NULL 输入只会将位从 0 置为 1，不会清除已经置位的位。

聚合器持有的 `aggFunction`/`AggFuncDesc` 与每个分组的 `AggEvaluateContext` 分离：描述符和参数表达式属于聚合器，累计值属于分组状态。`GetResult` 克隆 Datum；`GetPartialResult` 每次新建一个 `Vec`。

## 依赖与调用关系

上游构造与调用关系：

- `pkg/expression/aggregation/descriptor.rs::AggFuncDesc::GetAggFunc`：常规 `ast::AggFuncBitOr` 描述符的直接工厂。
- `pkg/expression/aggregation/aggregation.rs::NewDistAggFunc`：把 `tipb::ExprType::AggBitOr` 映射为 `ast::AggFuncBitOr`，并构造本类型，供 mock TiKV 等行式下推路径使用。
- `pkg/expression/aggregation/aggregation.rs::Aggregation`：规定执行器按“创建状态 → 多次 Update → 读取结果/部分结果 → 可选重置”的方式驱动本类型。
- `pkg/expression/aggregation/lib.rs`：声明、编译并重导出本模块；同文件把各个内部类型 crate 组合为 Go 风格的 `types` 门面。

本文件的直接下游依赖是：`expression::Expression::Eval`（参数求值）、`stmtctx::StatementContext::TypeCtx`（转换规则）、`types::Datum` 的 kind/取值/转换/写值 API、`chunk::Row`（输入行）以及公共 `aggFunction::{CreateContext, ResetContext}`。Cargo 中对应的直接 crate 依赖包括 `expression`、`stmtctx`、`chunk` 和组成 `types` 门面的 datum/field 等内部 crate；本文件没有网络、磁盘或外部服务依赖。

描述符外围语义位于 `descriptor.rs`：`EvalNullValueInOuterJoin` 对 `BIT_OR` 复用 `evalNullValueInOuterJoin4BitOr`，无法折叠或为 NULL 时返回 0；`UpdateNotNullFlag4RetType` 将 `BIT_OR` 归入保持非空返回类型的一组。这些规则与本文件的状态初值相互一致，但不由本文件实现。

## 错误处理与边界

- 参数表达式求值失败时，`?` 原样传播 crate 的 `Error`；该行不执行状态合并。
- 非 `KindUint64` 值的 `ToInt64(sc.TypeCtx())` 失败时同样传播错误。具体截断、溢出或告警策略由 `StatementContext` 的类型上下文和 Datum 转换实现决定，本文件不吞掉或改写错误。
- `NULL` 明确跳过；空集及全 NULL 输入保持初值 0。
- 代码直接索引 `AggFuncDesc.Args[0]`，因此其不变量是描述符构造阶段必须保证至少一个参数。若绕过合法构造器创建空参数描述符，这里会 panic，而不是返回可恢复错误。
- 有符号转换结果随后 `as u64`，负数按二进制补码位模式参与按位或；这是对 Go `uint64(int64Value)` 的对应移植，不是数值范围检查。
- 本实现只识别 `KindUint64` 快速路径；Decimal 等其他 kind 均走 `ToInt64`。测试证明 Decimal `12.234、1.012、15.12345678、16.00` 转换后按位或为 `31`，但更广泛的转换边界属于 Datum/类型上下文测试范围。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或 I/O 资源。`Arc<dyn expression::EvalContext>` 允许求值上下文被共享所有权持有，但每个 `AggEvaluateContext` 仍通过 `&mut` 独占更新；本实现自身没有内部同步，也不声明同一分组状态可被并发写入。

生命周期以分组为单位：`CreateContext` 分配状态，若干次 `Update` 原地累计，结果读取只借用状态，`ResetContext` 则复用同一分配并替换 `Ctx`、清理公共字段、重置值。参数表达式和描述符随 `bitOrFunction` 存活；单行 `chunk::Row` 仅在一次 `Update` 调用期间消费。

## 与 Go 版本的对应关系

同路径 `pkg/expression/aggregation/bit_or.go` 是直接语义基线。Rust 与 Go 都：嵌入/持有公共 `aggFunction`；从参数 0 求值；跳过 NULL；对 `KindUint64` 直接取值，否则经 `ToInt64(sc.TypeCtx())` 转换；以 `uint64` 做按位或；空集与 Reset 后为 0；部分结果只包含最终值。

结构上的差异主要来自语言所有权：Go 返回 `*AggEvaluateContext` 并直接返回其中的 Datum，Rust 的 `CreateContext` 按值返回状态，`GetResult` 显式 clone Datum，求值上下文使用 `Arc<dyn EvalContext>`。Go 的 `ResetContext` 只替换 `Ctx` 并设置值；Rust 先调用公共 `aggFunction::ResetContext`，因此还会清理计数、缓冲、首行标志并按需重建 DISTINCT checker，再设置 0。对于本聚合直接使用的字段，两者结果一致；Rust 的公共清理更全面。

测试对应关系：Go 的 `pkg/expression/aggregation/aggregation_test.go::TestBitOr` 覆盖空值 0、输入 `1/NULL/1/3/2` 的累计结果、部分结果、Reset 和 Decimal 转换；Rust 的 `pkg/expression/aggregation/aggregation_aster_unit_test.rs::bit_aggregates_preserve_empty_values_nulls_and_reset` 用共享表驱动用例覆盖相同性质，并由 `aggregation_test.rs::TestBitOr` 调用。当前没有同名 `bit_or_test.rs`，测试逻辑仍保持在独立测试文件中，没有内嵌进生产源文件。

## 扩展指南

- 若改变输入转换或 NULL/空集规则，主要修改点是 `bitOrFunction::Update` 或创建/重置方法；必须同步 `aggregation_aster_unit_test.rs::bit_aggregates_preserve_empty_values_nulls_and_reset`，并与 Go `aggregation_test.go::TestBitOr` 的整数、NULL、Decimal 和 Reset 用例核对。
- 若改变构造或分布式模式接线，要同时检查 `descriptor.rs::AggFuncDesc::GetAggFunc`、`aggregation.rs::NewDistAggFunc`、`agg_to_pb.rs` 的 `AggBitOr` 映射，以及描述符的外连接空值折叠和返回类型非空规则。
- 若新增 BIT_OR 专属测试，遵守仓库规则放在独立测试文件并在 `lib.rs` 的 `#[cfg(test)]` 模块区接线；不要把测试写入 `bit_or.rs`。现有共享测试优先扩展，避免重复脚手架。
- 兼容风险集中在 MySQL/Go 一致的类型转换、负数的位模式、空输入非 NULL 语义和 Partial/Final 合并格式。性能敏感点是每行表达式求值和非 `u64` 转换；不要在热路径引入额外分配。`GetPartialResult` 当前固有地分配一元素 Vec，修改其布局会影响分布式聚合协议。
- 不应仅在本文件加入 DISTINCT 处理而不审查描述符合法性与 Go 行为，因为当前实现虽然公共上下文可能持有 checker，更新路径并未使用它。

## 验证依据

- 目标源码：`pkg/expression/aggregation/bit_or.rs`，逐项核对 `bitOrFunction` 及 `Aggregation` 的五个方法。
- crate 与模块边界：`pkg/expression/aggregation/Cargo.toml`、`pkg/expression/aggregation/lib.rs`。
- 运行时契约与 PB 构造：`pkg/expression/aggregation/aggregation.rs` 中的 `Aggregation`、`AggEvaluateContext`、`aggFunction::{CreateContext, ResetContext}` 和 `NewDistAggFunc`。
- 常规构造及外围语义：`pkg/expression/aggregation/descriptor.rs` 中的 `AggFuncDesc::GetAggFunc`、`EvalNullValueInOuterJoin`、`evalNullValueInOuterJoin4BitOr`、`UpdateNotNullFlag4RetType`。
- Go 对照：`pkg/expression/aggregation/bit_or.go`；Go 回归：`pkg/expression/aggregation/aggregation_test.go::TestBitOr`。
- Rust 回归：`pkg/expression/aggregation/aggregation_aster_unit_test.rs::bit_aggregates_preserve_empty_values_nulls_and_reset`，以及接线文件 `pkg/expression/aggregation/aggregation_test.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query bitOrFunction` 同时定位 Go/Rust 类型及 Go 方法；`explore`/文件节点确认 Rust `GetPartialResult → GetResult`、构造入口、公共上下文方法和上述测试调用关系。常见方法名存在大量同名节点，因此结论以目标文件节点和路径限定的调用证据为准。
- 本任务只新增文档，按计划不运行 Cargo；最终使用任务规定的 11 章节结构命令验证。
