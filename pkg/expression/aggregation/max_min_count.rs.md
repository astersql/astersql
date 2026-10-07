# `pkg/expression/aggregation/max_min_count.rs`

## 文件定位

本文件实现表达式聚合 crate `astersql-expression-aggregation` 中的 `MAX_COUNT` / `MIN_COUNT` 运行时聚合器：它不返回极值本身，而是返回当前最大值或最小值出现的次数。crate 边界由 [`Cargo.toml`](Cargo.toml) 声明，模块由 [`lib.rs`](lib.rs) 的 `mod max_min_count` 纳入并通过 `pub use max_min_count::*` 导出。

它位于描述符和逐行执行之间。上游可由 [`descriptor.rs`](descriptor.rs) 的 `AggFuncDesc::GetAggFunc` 根据 AST 名称构造，也可由 [`aggregation.rs`](aggregation.rs) 的 `NewDistAggFunc` 从 TiPB `MaxCount` / `MinCount` 构造；下游则使用表达式求值、`Datum` 比较和排序规则完成状态更新。它不是 executor 中滑动窗口 `max/min count` 实现的替代品；本文件只实现 `expression::aggregation::Aggregation` 这一行式聚合接口。

## 核心职责

- `maxMinCountFunction` 同时承载两种行为：`isMax == true` 统计最大非 NULL 值出现次数，`isMax == false` 统计最小非 NULL 值出现次数。
- `Update` 支持两种输入形态：单参数 Complete/Partial1 输入一列原始值，每行权重为 1；双参数 Final/Partial2 输入 `[部分计数, 部分极值]`，把部分计数作为权重合并。
- `GetResult` 只输出计数；`GetPartialResult` 按固定顺序输出 `[计数, 极值]`，供后续阶段继续合并。
- `CreateContext` 与 `ResetContext` 复用公共 `aggFunction` 的上下文管理，确保同一个聚合器能为新分组创建或重置状态。

上述职责由 [`max_min_count.rs`](max_min_count.rs) 的 `impl Aggregation for maxMinCountFunction` 直接给出，并由 [`go_merge_44_test.rs`](go_merge_44_test.rs) 的 `go_merge_44_complete_and_final_count_extrema` 覆盖。

## 主要符号

- `pub struct maxMinCountFunction`：运行时聚合器。`aggFunction` 持有 `AggFuncDesc` 及参数表达式；`isMax` 选择最大/最小方向；`ctor: Box<dyn collate::Collator>` 保存按比较列类型选出的排序规则比较器。
- `Aggregation::Update(...) -> Result<(), Error>`：读取当前行并更新 `AggEvaluateContext::{Value, Count}`。表达式求值错误和 `Datum::Compare` 错误通过 `?` 原样向上传播。
- `Aggregation::GetResult(...) -> types::Datum`：用 `types::NewIntDatum(eval_ctx.Count)` 返回最终计数，因此空输入的结果是上下文初始值 0，而不是 NULL。
- `Aggregation::GetPartialResult(...) -> Vec<types::Datum>`：返回两个 Datum，顺序严格为计数在前、当前极值在后。
- `Aggregation::CreateContext(...)`：委托 `aggFunction::CreateContext` 创建状态；公共实现初始化 `Count = 0`、`Value = NULL`，并保留表达式求值上下文。
- `Aggregation::ResetContext(...)`：先委托公共实现清空值、缓冲等共享状态，再显式把 `Count` 设为 0。显式赋值与 Go 版本一致，也固定了该聚合器的重置不变量。

本文件没有模块级常量、自由函数、条件编译项或内部辅助类型；公开面只有结构体及其公开字段，行为通过 `Aggregation` trait 暴露。

## 执行流程

1. 构造阶段：`AggFuncDesc::GetAggFunc` 对 `AggFuncMaxCount` / `AggFuncMinCount` 创建本结构；`NewDistAggFunc` 对 TiPB `MaxCount` / `MinCount` 做同样工作。两处都在 Final/Partial2 且参数数大于 1 时用参数 1 的类型选择 collator，否则用参数 0。
2. 输入解码：`Update` 查看 `AggFuncDesc.Args`。参数数大于 1 时，先通过 `Args[0].EvalInt` 取部分计数；计数为 NULL 或 0 时整行不参与聚合；再通过 `Args[1].Eval` 取部分极值，值为 NULL 也跳过。单参数时直接求值，NULL 跳过，并令计数为 1。
3. 初始化状态：若 `eval_ctx.Value` 仍为 NULL，则当前非 NULL 值成为极值，`Count` 设为当前行或部分结果携带的计数，随后立即返回。
4. 比较：已有极值通过 `Datum::Compare(sc.TypeCtx(), &value, ctor)` 与候选值比较。最大值模式在现值小于候选值时替换；最小值模式在现值大于候选值时替换。替换时计数也被候选计数覆盖。
5. 平局累加：比较结果为 0 时，把候选计数加到当前 `Count`。非极值且不相等的候选不改变状态。
6. 输出：完整结果只取 `Count`；部分结果输出 `[Count, Value]`。因此多阶段聚合可把各分区极值及其频次再次送入同一 `Update` 逻辑。

例如部分结果 `(2, 3)` 与 `(1, 3)` 在最大值模式合并为计数 3；后续 `(4, 2)` 不影响最大值结果。相同输入在最小值模式最终会选择更小的值并重新累计其计数。该事实由 Rust 和 Go 的完整/Final 用例共同验证。

## 数据与状态

持久状态实际位于共享 `AggEvaluateContext`：`Value` 保存当前极值，`Count: i64` 保存该极值的累计出现次数，`Ctx` 提供参数表达式求值环境。`maxMinCountFunction` 本身保存只随描述符变化的配置，不保存分组结果，所以一个函数对象可配合不同上下文处理不同分组。

关键不变量如下：

- `Value == NULL` 表示尚未接纳有效值；此时正常上下文的 `Count` 为 0。
- 接纳有效值后，`Value` 是所有已接纳输入的目标极值，`Count` 是与该极值比较相等的权重之和。
- 单阶段输入权重恒为 1；多阶段输入权重来自第一列。NULL、零计数或 NULL 极值的部分结果不会污染状态。
- 部分结果的列序 `[count, value]` 是与 `AggFuncDesc::Split`、构造阶段比较列选择及 Final/Partial2 解码共同遵守的协议，不能独立改变。
- 字符串等需要排序规则的值由构造时选定的 `collate::Collator` 比较；`StatementContext::TypeCtx` 提供语句级类型比较上下文。

`Count += count` 使用普通 `i64` 加法；本文件没有单独的溢出检查或饱和策略。扩展或改变计数范围时必须同时审查 Go 行为及构建配置下整数溢出语义。

## 依赖与调用关系

上游入口：

- [`descriptor.rs`](descriptor.rs) 的 `AggFuncDesc::GetAggFunc` 是本地描述符入口，并依据 `Name` 设置 `isMax`。
- [`aggregation.rs`](aggregation.rs) 的 `NewDistAggFunc` 是 TiPB/分布式入口，把 protobuf 子表达式解码成参数并依据 TiPB 类型设置方向。
- 同文件的 `CheckAggPushDown` 规定 Max/Min Count 只在 TiFlash、单参数且非 Dedup 模式下可下推；`NeedCount` 和 `NeedValue` 都把这两个函数列为需要相应状态的聚合。

下游依赖：

- `expression::Expression::{EvalInt, Eval}` 读取计数列或值列。
- `types::Datum::{IsNull, Compare}` 实现 NULL 判断与带类型上下文、collator 的值比较；`types::NewIntDatum` 包装输出计数。
- `aggFunction::{CreateContext, ResetContext}` 管理共享状态；`stmtctx::StatementContext::TypeCtx` 提供比较语义。
- `chunk::Row` 是逐行输入载体，`Arc<dyn expression::EvalContext>` 管理共享求值上下文所有权。

`Cargo.toml` 直接声明了本文件用到的 `chunk`、`collate`、`expression`、`stmtctx` 等工作区依赖，并通过 `package.metadata.porting.go-package = "pkg/expression/aggregation"` 标明 Go 对照包。

## 错误处理与边界

- `EvalInt`、`Eval` 和 `Compare` 的错误均由 `?` 返回给调用者；本文件不吞错、不包装错误，也不在失败后继续更新。计数表达式成功但值表达式失败时，状态尚未改变。
- 单参数分支直接索引 `Args[0]`，双参数分支直接索引 `Args[1]`；参数形状由 `AggFuncDesc` 构造和 Split 协议保证。若绕过描述符构造提供空参数，当前代码会 panic，而不是返回结构化错误。
- Final/Partial2 的识别在构造处影响 collator 的比较列，但 `Update` 仅以 `args.len() > 1` 选择双参数协议。因此扩展参数列表时不能把无关参数附加到该聚合器，否则会被误解为 `[count, value]`。
- NULL 原始值被忽略；部分计数为 NULL 或 0、部分极值为 NULL 时也被忽略。负计数未被拒绝，当前实现会接纳并可能减少累计值；正常调用链必须保证部分计数合法。
- 空输入与全部被忽略输入返回整数 0；部分结果此时为 `[0, NULL]`。Rust 独立测试明确验证重置后的这一状态。
- 比较相等按 `Datum::Compare` 与 collator 的语义判定，不一定等同于字节相等；变更 collator 选择会同时改变“极值”和“平局计数”。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部 I/O。`Update` 要求 `&mut self` 和 `&mut AggEvaluateContext`，因此同一函数对象和同一分组状态的更新在 Rust 类型层面是串行可变访问；跨线程共享与调度由上层执行器负责。

求值上下文通过 `Arc<dyn expression::EvalContext>` 传入并存入 `AggEvaluateContext`，创建或重置时更换引用计数所有权；本文件不手工释放资源。`ctor` 由函数对象独占，生命周期与函数对象一致。`chunk::Row` 按值传入，必要时仅在先求计数、后求值的双参数路径中克隆一次行句柄，不保存到聚合状态中。

状态生命周期是“构造函数对象 → 为分组 `CreateContext` → 多次 `Update` → `GetResult`/`GetPartialResult` → 可选 `ResetContext` 后复用”。重置测试证明 `Count` 回到 0 且 `Value` 回到 NULL，防止不同分组之间串值。

## 与 Go 版本的对应关系

直接对照文件是 [`max_min_count.go`](max_min_count.go)。Rust 版本保留了 Go 的结构字段、五个聚合接口方法、单/双参数分支、NULL 与零计数过滤、首次值初始化、极值替换、平局累加及部分结果顺序。

可见的语言层差异不改变算法：Go 使用嵌入的 `aggFunction`，Rust 使用具名字段；Go 用 `value.Copy(&evalCtx.Value)`，Rust 把拥有所有权的 `Datum` 赋给 `eval_ctx.Value`；Go 将求值和比较错误显式判断后返回，Rust 使用 `?`；Go 比较替换条件写成 `cmp == -1` / `cmp == 1`，Rust 接受任意负值 / 正值，语义更贴合一般三路比较结果。

Go 的 [`aggregation_test.go`](aggregation_test.go) 中 `TestMaxMinCount` 验证原始值 `[2,3,3,1,1,NULL]` 得到最大/最小次数均为 2，并验证 Final 模式把部分结果合并为最大值次数 3、最小值次数 5。Rust 的 [`go_merge_44_test.rs`](go_merge_44_test.rs) 迁移了这些核心断言，并额外明确覆盖计数 0、计数 NULL、值 NULL 的忽略行为以及 Reset。Rust [`aggregation_test.rs`](aggregation_test.rs) 当前没有同名 `TestMaxMinCount` 转发，实际 Rust 回归入口是 `lib.rs` 注册的 `go_merge_44_test` 独立测试模块。

## 扩展指南

- 改变输入协议或部分结果布局时，至少同步修改本文件的 `Update`/`GetPartialResult`、`descriptor.rs::AggFuncDesc::Split`、`descriptor.rs::GetAggFunc`、`aggregation.rs::NewDistAggFunc`，以及 TiPB 转换逻辑；保持 `[count, value]` 的生产端与消费端一致。
- 新增支持的比较类型或改变排序规则时，重点审查两处构造入口的 `compare_index` 和 `collate::GetCollator`，并增加字符串/排序规则及 Final/Partial2 测试，避免用计数列类型选择 collator。
- 改变 NULL、零计数、负计数或溢出策略时，应先与 `max_min_count.go` 对齐，再扩展独立 Rust 测试 `go_merge_44_test.rs`；测试逻辑不要内嵌回生产文件。
- 改变下推能力时，还需同步 `aggregation.rs::CheckAggPushDown`、`agg_to_pb.rs` 及其独立测试，而不能只修改本运行时聚合器。
- 性能修改应保留单行至多一次值比较、双参数路径必要的行克隆和 `Datum` 所有权边界；对大分组而言，比较与 collator 调用是主要逐行成本。
- 兼容性审查应覆盖 Complete、Partial1、Partial2、Final 四种相关模式、空输入、全部 NULL、并列极值、多个部分结果和重置复用。Dedup 当前被下推检查拒绝，不应在没有描述符与执行链证据时宣称支持。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；查询时目标 Rust 文件在索引中。
- RustCodeGraph `node --file pkg/expression/aggregation/max_min_count.rs`：确认文件共 90 行、结构字段和五个 trait 方法，并显示该文件被 `descriptor.rs` 使用。
- RustCodeGraph `node maxMinCountFunction`：确认 Rust 定义由 `descriptor.rs::GetAggFunc` 实例化；`node GetAggFunc` 显示 AST 名称到 `isMax`、collator 和具体结构的构造边。
- RustCodeGraph `node aggregation.rs::NewDistAggFunc`：确认 TiPB `MaxCount` / `MinCount` 的分布式构造入口、模式映射和比较列选择，并显示 Rust 测试调用边。
- 已核对源码/配置：[`max_min_count.rs`](max_min_count.rs)、[`aggregation.rs`](aggregation.rs)、[`descriptor.rs`](descriptor.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。
- 已核对 Go 对照与测试：[`max_min_count.go`](max_min_count.go)、[`aggregation_test.go`](aggregation_test.go) 的 `TestMaxMinCount`。
- 已核对独立 Rust 测试：[`go_merge_44_test.rs`](go_merge_44_test.rs) 的 `go_merge_44_descriptor_and_pushdown`、`go_merge_44_complete_and_final_count_extrema`、`go_merge_44_outer_join_count_defaults_and_not_null_flag`；由 `lib.rs` 的 `#[path = "go_merge_44_test.rs"]` 注册。
- 本任务按计划只做文档分析，未运行 Cargo。交付前另以固定正则验证本文恰好包含规定的 11 个二级章节，并人工检查未修改 Rust、Go、Cargo 或只读总计划。
