# `pkg/expression/aggregation/max_min.rs`

## 文件定位

本文件属于 `astersql-expression-aggregation` crate（见同目录 `Cargo.toml`），实现行式聚合运行时中的 `MAX`/`MIN`。模块入口 `lib.rs` 以 `mod max_min` 装载它，并通过 `pub use max_min::*` 将实现提供给 crate 内的描述符构造和分布式聚合构造流程。

它不是规划器中的类型推断实现，也不是执行器的分组容器：`AggFuncDesc::GetAggFunc`（`descriptor.rs`）和 `NewDistAggFunc`（`aggregation.rs`）负责根据函数名或 tipb 表达式创建本文件的对象；执行器通过 `Aggregation` trait 驱动对象逐行更新。RustCodeGraph 对 `maxMinFunction` 的反向关系也只列出这两个实例化入口。

## 核心职责

- `maxMinFunction` 统一承载 `MAX` 和 `MIN`，用 `isMax` 选择方向，避免复制两套聚合逻辑。
- `Update` 对第一个参数求值，忽略 SQL `NULL`，并在当前状态为空时接纳首个非空值。
- 后续值通过 `Datum::Compare` 和构造时选定的 `collate::Collator` 比较；`MAX` 只接受更大值，`MIN` 只接受更小值，相等时保留已有值。
- `GetResult`、`GetPartialResult`、`CreateContext` 和 `ResetContext` 补齐 `Aggregation` 的结果读取及分组状态生命周期契约。

本实现只维护一个当前极值，不排序、不缓存全部输入，因此每行更新所需额外空间为常数；比较成本由具体 `Datum` 类型和字符串排序规则决定。

## 主要符号

- `pub struct maxMinFunction`
  - `aggFunction: aggFunction`：持有 `AggFuncDesc`；本文件从 `AggFuncDesc.Args[0]` 取得唯一参与比较的表达式，也复用其上下文创建/重置逻辑。
  - `isMax: bool`：`true` 表示 `MAX`，`false` 表示 `MIN`。两个构造入口都从函数种类明确设置该字段。
  - `ctor: Box<dyn collate::Collator>`：字符串及字节比较采用的 collation 实现。`descriptor.rs` 和 `aggregation.rs` 都从比较参数的 `FieldType::GetCollate()` 选择它。
- `impl Aggregation for maxMinFunction`
  - `Update(&mut self, ctx, sc, row) -> Result<(), Error>`：求值并更新极值，是文件内唯一会改变聚合状态的方法。
  - `GetResult(&self, ctx) -> types::Datum`：克隆并返回 `ctx.Value`，因此调用者取得独立的结果值而不借用分组状态。
  - `GetPartialResult(&self, ctx) -> Vec<types::Datum>`：返回只含最终/中间极值的一列，供分阶段聚合继续合并。
  - `CreateContext`、`ResetContext`：直接委托给 `aggFunction`，创建或复用 `AggEvaluateContext`。

文件没有模块级常量、自由函数、条件编译项或自有错误类型。

## 执行流程

1. `AggFuncDesc::GetAggFunc` 接收 `max`/`min` 描述符，或 `NewDistAggFunc` 接收 tipb `ExprType::Max`/`Min`，据此构造 `maxMinFunction`。两条路径都按参数类型的 collation 创建 `ctor`。
2. 新分组调用 `CreateContext`。`aggFunction::CreateContext` 将 `AggEvaluateContext.Value` 初始化为默认 `Datum`（即 `NULL`），并保存共享的表达式求值上下文。
3. 每行调用 `Update`：先执行 `AggFuncDesc.Args[0].Eval(ctx.Ctx.as_ref(), row)`。求值错误立即沿 `Result` 返回。
4. 输入值为 `NULL` 时直接成功返回，不改变当前状态。这保证空输入或全 `NULL` 输入的结果仍为 `NULL`。
5. 当前 `ctx.Value` 为 `NULL` 时，把首个非空输入移入状态并返回，不进行无意义的比较。
6. 已有极值时执行 `value.Compare(sc.TypeCtx(), &ctx.Value, ctor)`。比较结果大于零表示新值更大，小于零表示新值更小；仅当方向与 `isMax` 相符时替换 `ctx.Value`。
7. 调用者以 `GetResult` 读取一列结果，或以 `GetPartialResult` 得到单元素数组。在 Final/Partial 合并阶段，该单个 `Datum` 可作为普通输入再次经过相同比较流程。
8. 状态对象复用于下一分组时调用 `ResetContext`；共享实现会把 `Value` 重新置为 `NULL`，并清空其他通用聚合字段。

## 数据与状态

真正参与本算法的可变状态只有 `AggEvaluateContext.Value`：它从 `NULL` 转为首个非空值，之后始终保存截至当前行的极值。由 `Update` 的替换条件可得不变量：处理完任意输入前缀后，`MAX` 状态不小于该前缀中的每个非空值，`MIN` 状态不大于它们；没有非空值时状态仍为 `NULL`。

`Count`、`Buffer`、`BufferInitialized` 和 `GotFirstRow` 是通用上下文字段，本文件不读写。`CreateContext` 可能因描述符的 `HasDistinct` 创建 `DistinctChecker`，但 `Update` 不访问它；这不会改变结果语义，因为重复值不会影响最大值或最小值，但也意味着新增与去重状态相关的统计时不能假定本实现会更新该检查器。

状态中的 `Datum` 由值拥有：首次接纳或替换时直接赋值，读取结果时克隆。文件本身没有全局状态、静态缓存或跨分组共享的可变集合。

## 依赖与调用关系

上游实例化关系：

- `aggregation.rs::NewDistAggFunc`：将 tipb `Max`/`Min` 映射为 AST 名称并实例化 `maxMinFunction`，用于 mock TiKV 等分布式 PB 表达式路径。
- `descriptor.rs::AggFuncDesc::GetAggFunc`：规划器/执行侧按 `AggFuncDesc.Name` 实例化对象，是普通描述符路径。
- `aggregation.rs::Aggregation`：规定执行器可见的逐行更新、结果、部分结果和上下文生命周期接口。

主要下游依赖：

- `expression::Expression::Eval`：计算参数表达式，输入不一定只是裸列。
- `types::Datum::{IsNull, Compare}`：承载 SQL 值、NULL 判定及跨支持类型的比较。
- `stmtctx::StatementContext::TypeCtx`：为类型转换、截断等比较行为提供语句级上下文。
- `collate::Collator`：决定字符串/字节值的排序规则。该依赖在 `Cargo.toml` 中来自工作区的 `astersql-util-collate`；`expression`、`chunk`、`stmtctx` 和 datum 类型也均由同一 manifest 明确声明。
- `aggFunction::{CreateContext, ResetContext}`：管理通用分组状态。

RustCodeGraph 将源文件标记为被 `aggregation.rs` 和 `descriptor.rs` 使用，并把上述两个构造符号列为 `maxMinFunction` 的实例化调用方；没有发现本文件直接调用异步、网络或存储接口。

## 错误处理与边界

- 参数表达式求值失败时，`Update` 用 `?` 原样传播错误，且在求值成功前不会修改极值状态。
- `Datum::Compare` 失败时同样传播错误；替换发生在比较成功之后，因此比较失败不会用当前输入覆盖已有结果。
- `NULL` 永远不参与比较：空集、全 `NULL` 集合返回 `NULL`，混合输入忽略其中的 `NULL`。
- 相等值不会替换现有状态；这对标量结果没有差异，但若未来为 `Datum` 附加来源身份，必须意识到当前实现稳定地保留先到值。
- 本文件直接索引 `Args[0]`。参数数量和类型合法性由 `AggFuncDesc` 构造/类型推断阶段保证；若绕过这些入口制造空参数描述符，会发生索引 panic，而不是返回业务错误。
- collation 在对象构造时依据参数字段类型固定下来，不能在某一行临时更换。字符串比较扩展必须保持构造入口与 `Datum::Compare` 使用同一排序规则契约。

## 并发与资源生命周期

`maxMinFunction` 和每个 `AggEvaluateContext` 由聚合执行流程按分组持有，`Update` 要求 `&mut self` 与 `&mut AggEvaluateContext`，本文件没有内部锁，也没有自行提供同一状态的并发更新。并行聚合应由上层为 worker/分组创建独立状态，再通过部分结果合并，而不是共享一个可变 `ctx.Value`。

表达式求值上下文以 `Arc<dyn EvalContext>` 保存，`CreateContext` 接收并共享它；`ResetContext` 可替换为新的 `Arc`。collator 由 `Box<dyn Collator>` 独占在聚合对象中，生命周期覆盖该对象的全部比较。代码不启动任务、不使用 channel、不持有事务/文件/网络资源，也没有显式清理阶段；`Datum`、collator 和 `Arc` 均依靠 Rust 所有权在对象释放时回收。

## 与 Go 版本的对应关系

Go 对照文件是同目录 `max_min.go`。Rust 保留了相同的 `maxMinFunction` 三项概念字段、五个聚合接口方法，以及“求值—初始化—忽略 NULL—按 collation 比较—按方向替换”的语义。

两版有两处写法差异但结果等价：Go 在当前结果为 `NULL` 时先把输入复制到状态，然后若输入为 `NULL` 再返回；Rust 先跳过输入 `NULL`，只把首个非空值写入状态。Go 比较 `current.Compare(new)`，在 `MAX` 得到 `-1` 或 `MIN` 得到 `1` 时替换；Rust比较 `new.Compare(current)`，分别检查正数或负数。Rust 用拥有型赋值/`clone` 代替 Go 的 `Datum.Copy`。

测试对应关系也保持明确：Go `aggregation_test.go::TestMaxMin` 验证初始 NULL、输入 `2/3/1/NULL` 后的最大值 3、最小值 1，以及单元素部分结果；Rust `aggregation_test.rs::TestMaxMin` 转发到独立的 `aggregation_aster_unit_test.rs::first_row_max_and_min_match_go_row_order_and_null_cases`，覆盖相同序列和断言。Rust 当前没有单独的 `max_min_test.rs`，测试仍与源文件分离，符合仓库测试组织要求。

## 扩展指南

- 修改极值选择规则时，首要接入点是 `Update`。必须同时保持 NULL、首值初始化、相等值、比较错误不污染状态这四项不变量，并同步 Go 对照语义。
- 支持新的可比较 `Datum` 类型通常应扩展 `Datum::Compare` 及其类型上下文，而不是在本文件添加类型分支；字符串类类型还要确认 `descriptor.rs::GetAggFunc` 和 `aggregation.rs::NewDistAggFunc` 仍从正确参数取得 collation。
- 若部分结果从一列变为多列，必须同步 `GetPartialResult`、描述符的 `Split`/Final 参数布局和两个构造路径；这会影响分布式兼容性，不能只改本文件。
- 若增加专属状态，优先在独立生产类型或通用 `AggEvaluateContext` 中明确生命周期，并同步 `CreateContext`/`ResetContext`；不要把测试逻辑内嵌进 `max_min.rs`。
- 回归测试应扩展独立的 `aggregation_aster_unit_test.rs`，并保留 `aggregation_test.rs::TestMaxMin` 的对外测试名。建议新增字符串不同 collation、相等值、求值/比较错误以及 Reset 后重新聚合的用例；同时核对 Go `aggregation_test.go::TestMaxMin`，避免 Rust 特有简化。
- 性能审查应关注 `Datum` 克隆和 collation 比较成本。当前算法的常数空间与单遍扫描特性不应因功能扩展而退化为保存全部输入。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`files --filter pkg/expression/aggregation/max_min.rs` 确认目标文件含 7 个符号；`node --file ...` 读取完整 69 行实现。
- RustCodeGraph 符号证据：`node pkg/expression/aggregation/max_min.rs::maxMinFunction` 显示其定义及两条实例化反向边：`aggregation.rs::NewDistAggFunc`、`descriptor.rs::GetAggFunc`。
- 源码证据：`max_min.rs::{maxMinFunction, Update, GetResult, GetPartialResult, CreateContext, ResetContext}`；`aggregation.rs::{Aggregation, AggEvaluateContext, aggFunction::CreateContext, aggFunction::ResetContext, NewDistAggFunc}`；`descriptor.rs::AggFuncDesc::GetAggFunc`；`lib.rs` 的模块声明和再导出。
- crate 证据：`pkg/expression/aggregation/Cargo.toml` 的 `[lib]`、`[dependencies]` 与 `[package.metadata.porting] go-package = "pkg/expression/aggregation"`。
- Go 对照：`pkg/expression/aggregation/max_min.go` 与 `pkg/expression/aggregation/aggregation_test.go::TestMaxMin`。
- Rust 测试：`pkg/expression/aggregation/aggregation_test.rs::TestMaxMin` 和 `pkg/expression/aggregation/aggregation_aster_unit_test.rs::first_row_max_and_min_match_go_row_order_and_null_cases`。本任务为纯文档分析，按计划未运行 Cargo；验证限于静态源码、索引调用边、Go/Rust 测试意图与文档结构。
