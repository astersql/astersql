# `pkg/expression/aggregation/bit_xor.rs`

## 文件定位

实现是直接的逐行状态机，不在本文件处理 `DISTINCT`、聚合阶段模式或多个参数。

## 核心职责

- 用 `bitXorFunction` 保存共享的 `aggFunction`，从其中取得第一个参数表达式及聚合描述符。
- 为每个聚合分组把累计值初始化或重置为无符号整数 `0`；`0` 是异或单位元，因此空输入组的结果也是 `0`。
- 在 `Update` 中求值第一个参数，忽略 `NULL`，把非 `u64` Datum 按语句类型上下文转换为 `i64` 后再按二进制位解释为 `u64`，最后与当前累计值异或。
- 以单个 `Datum` 同时提供最终结果和可继续传递的部分结果。

实现是直接的逐行状态机，不在本文件处理 `DISTINCT`、聚合阶段模式或多个参数。当前描述符构造会为 `BIT_XOR` 准备一个参数，而本实现直接索引 `Args[0]`，因此“至少一个参数”是调用方必须维持的不变量。

## 主要符号

- `pub struct bitXorFunction { pub aggFunction: aggFunction }`：唯一公开类型。字段包含 `AggFuncDesc`，因而间接持有参数表达式、函数名、模式和 `HasDistinct` 等描述信息。命名保留 Go 版本风格；crate 根允许 `non_camel_case_types` 与 `non_snake_case`。
- `impl Aggregation for bitXorFunction`：将类型接入统一行式聚合接口。
- `Update(&mut self, ctx, sc, row) -> Result<(), Error>`：计算一行的第一个参数并更新 `ctx.Value`。表达式求值或整数转换失败时返回错误。
- `GetResult(&self, ctx) -> types::Datum`：克隆当前累计 Datum，调用方获得独立值而不是状态引用。
- `GetPartialResult(&self, ctx) -> Vec<types::Datum>`：返回仅含最终累计值的单元素向量。
- `CreateContext(&self, ctx) -> AggEvaluateContext`：先调用 `aggFunction::CreateContext` 建立通用状态，再把 `Value` 设为 `u64(0)`。
- `ResetContext(&mut self, ctx, e)`：替换求值上下文并把 `Value` 设回 `u64(0)`；不会重置 `Count`、`DistinctChecker`、`Buffer`、`BufferInitialized` 或 `GotFirstRow`。

文件没有模块级常量、自由函数、条件编译项或自身定义的错误类型。

## 执行流程

1. 规划/构造阶段由 `AggFuncDesc::GetAggFunc` 识别 `ast::AggFuncBitXor`，或由 `NewDistAggFunc` 识别 `tipb::ExprType::AggBitXor`，把准备好的 `aggFunction` 装入 `bitXorFunction` 并作为 `Box<dyn Aggregation>` 返回。
2. 新分组开始时，上层调用 `CreateContext`。公共构造器建立 `AggEvaluateContext`，本实现随后令 `Value = 0`。
3. 每输入一行，上层调用 `Update`。它使用 `AggFuncDesc.Args[0].Eval(ctx.Ctx.as_ref(), row)` 求出 Datum。
4. 若 Datum 为 `NULL`，本次更新直接成功返回，累计值保持不变。
5. 若 Datum 的种类已经是 `types::KindUint64`，直接取 `GetUint64()`；否则调用 `ToInt64(sc.TypeCtx())`，再以 `as u64` 保留该有符号整数的二进制位模式。
6. 用 `ctx.Value.GetUint64() ^ value` 计算新值，并通过 `SetUint64` 写回。同一个值出现偶数次会相互抵消，这是 XOR 的代数性质而不是额外去重逻辑。
7. 需要输出时，`GetResult` 返回当前 Datum；分布式/部分聚合接口通过 `GetPartialResult` 得到单元素列表。
8. 状态对象复用于另一分组时，`ResetContext` 更换 `Ctx` 并恢复累计值 `0`。

## 数据与状态

真实累计状态位于 `AggEvaluateContext::Value`，在该实现中始终按 `u64` 读写。`bitXorFunction` 自身没有每组可变数据，只保存共享描述符；因此必须为不同分组提供不同的 `AggEvaluateContext`。

`AggEvaluateContext` 还含 `DistinctChecker`、`Count`、`Buffer`、`BufferInitialized` 和 `GotFirstRow`。`CreateContext` 会让公共构造器初始化这些字段，但 `BIT_XOR` 的更新路径只读取 `Ctx`、读写 `Value`。尤其是 `Update` 不访问 `DistinctChecker`，所以不能仅凭描述符中的 `HasDistinct` 推断本文件实现了去重。

`ResetContext` 有意保持除 `Ctx` 和 `Value` 外的字段不变；`pkg/expression/aggregation/bit_xor_test.rs::reset_context_matches_go_and_preserves_unrelated_state` 直接锁定了这一行为。若未来让该实现使用其他字段，必须同步重新评估这种局部重置是否仍然安全。

## 依赖与调用关系

上游构造边：

- `pkg/expression/aggregation/descriptor.rs::AggFuncDesc::GetAggFunc`：普通描述符路径在函数名为 `ast::AggFuncBitXor` 时实例化本类型。
- `pkg/expression/aggregation/aggregation.rs::NewDistAggFunc`：分布式 PB 路径先把 `tipb::ExprType::AggBitXor` 映射为 AST 名称，再实例化本类型并保留 PB 聚合模式。
- 上层执行器不依赖具体结构，而是通过 `Aggregation::{CreateContext, Update, GetPartialResult, GetResult, ResetContext}` 调用。

下游依赖：

- `expression::Expression::Eval`：对当前行求值；`use expression::Expression as _` 把 trait 方法引入作用域。
- `types::Datum`：承载输入、累计值和输出，并提供 `IsNull`、`Kind`、`GetUint64`、`SetUint64`、`ToInt64`。
- `stmtctx::StatementContext::TypeCtx`：为非 `u64` 输入的整数转换提供 SQL 类型转换上下文。
- `chunk::Row`：逐行输入载体。
- `Arc<dyn expression::EvalContext>`：在分组状态中共享表达式求值环境。

crate 直接依赖 `expression`、`chunk`、`stmtctx` 及由 `datum-dependency` 再导出的 Datum 类型；本文件通过 `use crate::*` 使用 crate 根的统一门面。

## 错误处理与边界

`Update` 的两个可失败点都使用 `?` 原样向上传播为 crate 的 `Error`（该别名等于 `expression::Error`）：参数表达式求值失败，以及非 `KindUint64` Datum 的 `ToInt64` 转换失败。失败发生在写回累计值之前，因此当前行不会造成半更新。

边界行为如下：

- 空组返回 `0`；这由 `CreateContext` 的单位元初始化保证。
- `NULL` 行被忽略，既不报错也不改变状态。
- `KindUint64` 绕过有符号转换，可完整保留 `u64` 范围。
- 其他种类必须满足 `ToInt64` 的转换规则；Decimal 的小数部分按该转换规则处理，测试中的 `1.234`、`1.012`、`2.12345678` 分别参与为整数并得到最终值 `2`。
- 负 `i64` 转为 `u64` 时使用 Rust 的 `as` 位模式转换，与 Go 的 `uint64(int64Value)` 对齐；这不是溢出检查。
- `Args[0]` 是直接索引。如果绕过描述符校验构造零参数对象，会发生 panic，而不是返回结构化错误。
- `GetPartialResult` 只包装当前值；本文件没有单独的“合并部分结果”分支，Final/Partial 模式如何提供输入由描述符与上层流程约束。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源，也没有显式分配可变长度的聚合缓冲。每行更新时间和额外空间均为常数级；`GetPartialResult` 每次分配一个单元素 `Vec`，`GetResult` 克隆一个 Datum。

求值上下文用 `Arc` 共享，但聚合状态通过 `&mut AggEvaluateContext` 独占更新，`bitXorFunction::Update` 本身也要求 `&mut self`。因此并发隔离责任在调用方：不同并发分组/worker 不应同时可变访问同一个状态对象。生命周期是“构造聚合器 → 每组创建上下文 → 多次逐行更新 → 读取结果 → 可选重置复用”；Reset 不释放或清空其他通用字段所持资源。

## 与 Go 版本的对应关系

- Go 的嵌入字段 `aggFunction` 在 Rust 中改为具名公开字段 `aggFunction: aggFunction`。
- 两版最终结果都是当前 Value，部分结果都是只含该值的单元素集合。

## 扩展指南

- 修改输入转换或异或规则时，入口是 `Aggregation for bitXorFunction::Update`。必须同时检查有符号、无符号、Decimal、NULL、负数及转换错误，避免破坏 Go 的 `ToInt64`/`uint64` 语义。
- 修改空组默认值或分组复用行为时，应同时调整 `CreateContext` 与 `ResetContext`，并更新 `bit_xor_test.rs` 以及共享聚合测试。不要把测试嵌入生产源文件。
- 若要支持或改变 `DISTINCT`，不能只设置 `AggFuncDesc::HasDistinct`；当前 `Update` 完全不读取 `DistinctChecker`。应先与 Go 当前行为和描述符路径核对，再为重复输入添加独立回归测试。
- 若部分聚合格式变化，需要同步审查 `GetPartialResult`、`AggFuncDesc` 的阶段拆分以及 `NewDistAggFunc` 的 PB 模式路径，确认下游把单个 `u64` Datum 解释一致。
- 新增参数校验应优先放在 `NewAggFuncDesc`/类型推断层，并为绕过构造器的路径保留清晰边界；直接在 `Update` 中扩展参数数量会改变热路径。
- 任何行为调整都应同步 Go 对照意图，而不是为 Rust 单独简化。性能上应保持逐行常数开销，避免在 `Update` 中引入堆分配；兼容性上重点关注 Datum 转换、负数位模式、空组默认值和部分结果编码。
