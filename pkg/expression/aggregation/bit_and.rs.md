# `pkg/expression/aggregation/bit_and.rs`

## 文件定位

本文件属于 `astersql-expression-aggregation` crate，提供旧式行式聚合接口 `Aggregation` 的 `BIT_AND` 具体实现。crate 由 `pkg/expression/aggregation/Cargo.toml` 定义，入口是同目录 `lib.rs`；`lib.rs` 以私有模块 `mod bit_and` 编译本文件，再通过 `pub use bit_and::*` 导出 `bitAndFunction`。

它位于“聚合描述符已经建立，执行器按分组逐行求值”的运行阶段，不负责 SQL 解析、返回类型推断或物理执行器调度。普通描述符入口 `AggFuncDesc::GetAggFunc`（`descriptor.rs:297`）在名称为 `ast::AggFuncBitAnd` 时构造它；从 tipb 恢复分布式聚合的入口 `NewDistAggFunc`（`aggregation.rs:31`）在类型为 `tipb::ExprType::AggBitAnd` 时构造它。执行方随后通过 `Box<dyn Aggregation>` 的统一接口创建分组状态、逐行更新并读取结果。

## 核心职责

1. 为每个新分组把累计值初始化为 `u64::MAX`，即按位与运算的单位元；因此空输入组以及只有 NULL 的输入组都保留全 1 结果。
2. 对每一行求值描述符的第一个参数，忽略 NULL，把非 `KindUint64` 的值按语句类型上下文转换为 `i64` 后重解释为 `u64`，再与当前累计值做按位与。
3. 以单个 `Datum` 同时提供最终结果和单列部分结果，使 Complete/Partial/Final 聚合阶段可以复用同一位表示。
4. 在分组状态复用时先执行公共清理，再恢复 `u64::MAX`，防止前一分组的位结果泄漏。

本文件只实现运行时折叠算法。`BIT_AND` 的类型推断、空输入默认值、外连接常量折叠、PB 映射与下推判断分别位于 `base_func.rs`、`descriptor.rs`、`agg_to_pb.rs` 和 `aggregation.rs`，不应误认为由本文件完成。

## 主要符号

- `pub struct bitAndFunction { pub aggFunction: aggFunction }`：唯一模块级类型，也是公开导出的具体聚合器。字段保存 `AggFuncDesc`，其中 `Args[0]` 是每行要求值的输入表达式。命名保留 Go 移植风格，并由 crate 级 lint 配置允许非 CamelCase。
- `impl Aggregation for bitAndFunction`：实现统一的五个生命周期方法；本文件没有模块级常量、自由函数、额外 trait 或条件编译项。
- `Update(&mut self, ctx, sc, row) -> Result<(), Error>`：求值首参数；NULL 时不改变状态；`KindUint64` 直接取 `u64`，其他类型调用 `Datum::ToInt64(sc.TypeCtx())`，最后执行 `old & value`。
- `GetResult(&self, ctx) -> Datum`：克隆并返回当前 `ctx.Value`，调用方获得独立的 `Datum` 值，不暴露内部可变引用。
- `GetPartialResult(&self, ctx) -> Vec<Datum>`：返回仅含 `GetResult` 的一列向量；位与的部分结果仍可作为下一聚合阶段的输入。
- `CreateContext(&self, eval_ctx) -> AggEvaluateContext`：委托 `aggFunction::CreateContext` 建立公共字段和可选 DISTINCT checker，再把 `Value` 写成 `u64::MAX`。
- `ResetContext(&mut self, eval_ctx, state)`：委托 `aggFunction::ResetContext` 更新求值上下文、重建去重器并清理公共字段，再恢复位与单位元。

## 执行流程

构造阶段有两条直接入口。`AggFuncDesc::GetAggFunc` 将描述符克隆进 `aggFunction`，对 `ast::AggFuncBitAnd` 返回 `Box<bitAndFunction>`；`NewDistAggFunc` 先用 `expression::PBToExprs` 恢复参数、把 `AggBitAnd` 映射为 `AggFuncBitAnd`、恢复聚合 mode，再装箱同一类型。这两条边均由 RustCodeGraph 的 `bitAndFunction` 节点和入口节点确认。

一个分组的运行步骤如下：

1. 调用 `CreateContext`。公共基类设置 `Ctx`、可选 `DistinctChecker`、计数与缓冲等字段，本实现随即把 `Value` 设为 `u64::MAX`。
2. 每行调用 `Update`。`AggFuncDesc.Args[0].Eval` 使用状态中的 `EvalContext` 求值得到 `Datum`；表达式错误立即返回。
3. 若值为 NULL，本行被跳过。若是 `KindUint64`，直接读取无符号值；否则按 `StatementContext::TypeCtx` 执行 `ToInt64`，成功后用 Rust 的 `as u64` 保留二进制补码位模式。
4. 以 `ctx.Value.GetUint64() & value` 更新累计值。按位与只会清除已有位，不会重新置位；一旦某位归零，后续输入不能恢复它。
5. 需要跨阶段传递时调用 `GetPartialResult` 获得单列 `Datum`；完成时调用 `GetResult`。状态供下一分组复用时调用 `ResetContext`，结果重新回到全 1。

例如输入 `1, 3, 2` 的累计轨迹是 `u64::MAX -> 1 -> 1 -> 0`；在任意位置插入 NULL 不改变轨迹。Decimal 输入先遵循 `ToInt64` 规则，测试中的 `1.234, 3.012, 2.12345678` 最终得到 0。

## 数据与状态

本实现自身只有不可独立变更的公共描述符字段 `aggFunction`；真正按分组变化的数据位于 `AggEvaluateContext::Value`。该 `Datum` 在创建和重置后为 `KindUint64(u64::MAX)`，每个有效输入将它替换为新的无符号位与结果。`Count`、`Buffer`、`BufferInitialized` 和 `GotFirstRow` 不参与本算法。

公共上下文仍可能因描述符的 `HasDistinct` 创建 `DistinctChecker`，但 `bitAndFunction::Update` 不读取或调用它；当前位与算法对重复值天然幂等，因此重复输入不会改变结果。描述符必须至少含一个参数，这是 `Update` 直接索引 `Args[0]` 的前置不变量，正常路径由 `NewAggFuncDesc`/PB 构造保证。

部分结果布局固定为一列无符号 `Datum`。这与位与的结合律及单位元配套：各分区结果可继续按位与，空分区的 `u64::MAX` 不影响其他分区。改变初值、结果类型或部分列数都会破坏分布式合并兼容性。

## 依赖与调用关系

直接上游构造者是 `descriptor.rs::AggFuncDesc::GetAggFunc` 和 `aggregation.rs::NewDistAggFunc`。前者用于已存在 Rust 描述符的执行路径；后者用于 tipb 表达式恢复路径。crate 的 `Aggregation` trait 定义执行器所见的五个方法，具体执行器只依赖 trait object，不需要识别 `bitAndFunction`。

直接下游依赖如下：

- `aggFunction::CreateContext` / `ResetContext`：建立或清理 `AggEvaluateContext` 的公共字段。
- `AggFuncDesc.Args[0]` 与 `expression::Expression::Eval`：从 `chunk::Row` 求值输入。
- `types::Datum`：保存空值、类型标记、转换结果与无符号累计值。
- `stmtctx::StatementContext::TypeCtx`：为非无符号输入提供与 SQL 语义一致的整数转换上下文。
- `std::sync::Arc`：共享 `dyn expression::EvalContext` 的所有权。

`Cargo.toml` 表明相关直接 crate 依赖包括 `expression`、`chunk`、`stmtctx` 与多个 `types` 子 crate；`lib.rs` 中的 `types` 门面统一再导出 Datum/字段/数值能力。类型推断在 `base_func.rs::TypeInfer` 中把三种位聚合分派到 `typeInfer4BitFuncs`；`base_func.rs::GetDefaultValue` 对 `AggFuncBitAnd` 返回 `NewUintDatum(u64::MAX)`，与本文件运行时初值一致。

## 错误处理与边界

`Update` 有两个可传播的错误源：参数表达式的 `Eval` 失败，以及非 `KindUint64` 值的 `ToInt64(sc.TypeCtx())` 转换失败。两者都使用 `?` 在写入累计值之前返回，因此失败行不会产生半更新。`GetResult`、`GetPartialResult`、`CreateContext` 和 `ResetContext` 本身不返回错误。

NULL 被显式忽略；空输入、全 NULL 输入和刚重置的分组结果都是 `u64::MAX`。无符号输入不经过有符号转换，避免超过 `i64::MAX` 的合法 `u64` 被误报溢出；其他类型必须先满足 `ToInt64` 规则。转换出的负 `i64` 通过 `as u64` 保留其二进制补码位型，这与 Go 的 `uint64(int64Value)` 对应。

本文件不校验参数个数；若调用方绕过正常描述符构造并传入零参数，`Args[0]` 会越界 panic。它也不自行检查 mode 或输入返回类型，而是依赖描述符、类型推断和 PB 解码层建立合法契约。部分结果和最终结果均克隆当前 `Datum`，不会因调用后继续更新状态而发生别名变化。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源。聚合器持有描述符，执行器为每个聚合分组持有一份 `AggEvaluateContext`；`CreateContext` 开始该状态生命周期，`Update` 原地修改，读取结果后可销毁或用 `ResetContext` 复用。

`Arc<dyn EvalContext>` 仅管理共享求值上下文的引用计数。虽然上下文可共享，`Update` 同时要求 `&mut self` 和 `&mut AggEvaluateContext`，同一个聚合器/分组状态没有内部并发同步，调用方不能无同步并发更新。该算法的状态大小恒定，不随行数增长；如果描述符错误地开启 DISTINCT，公共层可能分配 checker，但本实现不会向 checker 写入值。

重置先由公共基类替换 `Arc`、重建可选 checker、清空公共值/计数/缓冲/标志，然后本文件恢复 `u64::MAX`。这一顺序确保复用后的状态等价于新建状态，同时可能保留公共 `Vec` 的容量；位与自身不使用该缓冲。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/aggregation/bit_and.go`。Rust 保留了 Go 的类型角色和五个接口方法：两者都以 `math.MaxUint64`/`u64::MAX` 初始化和重置；都求值首参数、跳过 NULL；都为 `KindUint64` 提供直接路径，否则执行 `ToInt64` 后转为无符号数；都返回当前值，并把单个最终值作为部分结果。

语言层差异主要是所有权和错误表达：Go 的 `*AggEvaluateContext`/interface 对应 Rust 的引用与 `Box<dyn Aggregation>`；Go 返回 `error`，Rust 返回 `Result<(), Error>`；Go 直接返回 `evalCtx.Value`，Rust 显式 `clone`；Go 的 `uint64(int64Value)` 对应 Rust 的 `as u64`。Rust `ResetContext` 先调用更完整的公共 `aggFunction::ResetContext`，而 Go 版本直接替换 `Ctx` 并设置 `Value`；对本聚合的可观察值语义相同，Rust 同时清理了公共辅助字段。

Go 回归 `aggregation_test.go::TestBitAnd` 是移植语义基准。Rust 没有同名 `bit_and_test.rs`，而是在独立文件 `aggregation_test.rs::TestBitAnd` 中转发到 `aggregation_aster_unit_test.rs::bit_aggregates_preserve_empty_values_nulls_and_reset`。该测试覆盖空集、整数轨迹、NULL、部分结果、Reset 与 Decimal 转换，并与 Go 用例保持一致。

## 扩展指南

修改本算法时，最可能需要接触的符号是 `Update`（输入转换/累计规则）、`CreateContext` 与 `ResetContext`（单位元和复用规则）、`GetPartialResult`（分布式状态布局）。必须同时保持 `base_func.rs::GetDefaultValue` 的空输入值、`descriptor.rs` 的构造/外连接折叠和 `agg_to_pb.rs`/`NewDistAggFunc` 的 PB 映射一致。

回归测试应继续放在独立 Rust 测试文件，不要嵌入 `bit_and.rs`。现有入口是 `aggregation_test.rs` 与共享实现 `aggregation_aster_unit_test.rs`；新增边界可在共享函数中扩充，或在需要隔离时新建并从 `lib.rs` 的 `#[cfg(test)]` 模块区接入。至少应覆盖：空集和全 NULL、`u64::MAX` 与超过 `i64::MAX` 的无符号值、负有符号值、转换错误不提交状态、Reset 后跨组隔离、部分阶段再次合并。

兼容风险集中在空集返回值、负数/大无符号数的位模式和单列部分结果格式；正确性风险集中在绕过 `KindUint64` 快路径或在转换失败后错误写状态；性能风险较低，但不应为每行增加堆分配或克隆完整描述符。若更改参数约束或返回类型，还应同步 Go 对照、类型推断和独立测试，而不能只修改本文件。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/aggregation` 确认目标及相邻 Rust/Go/测试文件均已索引。
- RustCodeGraph `node --file pkg/expression/aggregation/bit_and.rs --offset 1 --limit 240`：完整读取 62 行目标源码，并报告使用文件 `aggregation.rs`、`descriptor.rs`、`go_merge_44_test.rs`；`node bitAndFunction --file ...` 确认两条实际构造边来自 `NewDistAggFunc` 和 `GetAggFunc`。
- RustCodeGraph `node NewDistAggFunc`、`node GetAggFunc`、`node Aggregation`、`node aggFunction`、`node AggEvaluateContext`：核对 tipb/描述符入口、trait 契约、公共状态布局及创建/重置语义。对过宽的 `explore` 结果未用作结论，避免把仓库内其他同名 `Update` 混入本文件调用链。
- 已读 crate/生产边界：`pkg/expression/aggregation/Cargo.toml`、`lib.rs`、`aggregation.rs`、`descriptor.rs` 的图节点、`base_func.rs` 相关分支，以及 Go 对照 `bit_and.go`。
- 已读独立测试：`aggregation_test.rs::TestBitAnd`、`aggregation_aster_unit_test.rs::bit_aggregates_preserve_empty_values_nulls_and_reset` 和 `aggregation_test.go::TestBitAnd`。`rg` 确认当前没有独立 `bit_and_test.rs`，BIT_AND 的 Rust 行为测试由上述聚合测试文件承载。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核没有修改 Rust、Go、Cargo 或只读的 `plan.md`。
