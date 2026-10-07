# [`pkg/expression/aggregation/avg.rs`](avg.rs)

## 文件定位

本文件实现行式 `AVG` 聚合器 `avgFunction`，属于 Cargo crate `astersql-expression-aggregation`。crate 入口 `pkg/expression/aggregation/lib.rs` 以 `mod avg` 纳入模块并通过 `pub use avg::*` 导出实现；`pkg/expression/aggregation/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认了这一边界。它不是执行器中的向量化 AVG 实现，而是实现 `aggregation.rs` 定义的 `Aggregation` trait，服务于按行更新的聚合路径以及 mock TiKV/PB 聚合构造路径。

两个直接工厂会创建该类型：`AggFuncDesc::GetAggFunc` 在描述符名称为 `ast::AggFuncAvg` 时装箱 `avgFunction`（`descriptor.rs:296-306`）；`NewDistAggFunc` 在 PB 类型为 `tipb::ExprType::Avg` 时创建它并保留 PB 聚合模式（`aggregation.rs:31-75`）。因此本文件位于“聚合描述符或 PB 表达式 → 具体 `Aggregation` 实现 → 按行维护状态 → 输出部分或最终结果”的运行链中。

## 核心职责

- 根据 `AggFuncDesc.Mode` 区分原始输入聚合与部分结果合并：`Partial1Mode`/`CompleteMode` 处理原始值，`Partial2Mode`/`FinalMode` 处理上游传来的 `(count, sum)`（`avgFunction::Update`）。
- 复用通用聚合器的求和、DISTINCT 与上下文管理能力，不在本文件重复实现：原始输入委托 `aggFunction::updateSum`，上下文创建和重置委托 `aggFunction::{CreateContext, ResetContext}`。
- 将累计的 `sum` 与 `count` 转成最终平均值：浮点直接相除；Decimal 先除法，再按返回类型的小数位以 `ModeHalfUp` 舍入（`avgFunction::GetResult`）。
- 暴露可供下一聚合阶段消费的两个 Datum：`[count, sum]`（`avgFunction::GetPartialResult`）。

本文件不负责类型推导、描述符拆分、表达式构建或执行器分组调度；这些分别由 `AggFuncDesc`、表达式层和调用 `Aggregation` trait 的上层完成。

## 主要符号

- `pub struct avgFunction { pub aggFunction: aggFunction }`：AVG 的具体运行时对象。唯一字段保存 `AggFuncDesc`，其参数、模式、DISTINCT 标志和返回类型均从该描述符取得。命名保留 Go 移植风格，crate 根允许 `non_camel_case_types`。
- `avgFunction::updateAvg(type_context, ctx, row) -> Result<(), Error>`：私有的二阶段合并入口。先求值 `Args[1]` 取得局部 sum；sum 为 NULL 时整行跳过，否则用 `calculateSum` 累加，再求值 `Args[0]` 并将其整数值加到 `ctx.Count`。
- `impl Aggregation for avgFunction`：对外运行时契约。
  - `Update`：按模式路由到 `aggFunction::updateSum` 或 `updateAvg`；`DedupMode` 明确 panic。
  - `GetResult`：从 `AggEvaluateContext::{Value, Count}` 计算最终 Datum。
  - `GetPartialResult`：返回 count 在前、sum 在后的固定二元布局。
  - `CreateContext` / `ResetContext`：委托通用 `aggFunction`，为每个分组创建或复用状态。

文件没有模块级常量、条件编译项或自定义 trait；使用的 `Aggregation`、`AggEvaluateContext`、模式枚举、Datum/Decimal API 均经 `crate::*` 引入。

## 执行流程

1. `AggFuncDesc::GetAggFunc` 或 `NewDistAggFunc` 根据 AVG 类型构造 `avgFunction`，内部携带完整描述符。
2. 上层为一个分组调用 `CreateContext`。通用实现建立 `AggEvaluateContext`；其中 `Value` 初始为 NULL、`Count` 为 0，DISTINCT 描述符会创建去重检查器（`aggregation.rs` 中 `AggEvaluateContext`、`aggFunction::CreateContext`/`ResetContext`）。
3. 对每一行调用 `Update`：
   - `Partial1Mode`/`CompleteMode`：`aggFunction::updateSum` 求值 `Args[0]`；NULL 不参与；DISTINCT 时跳过已见值；否则通过 `calculateSum` 累加并把 `Count` 加一（`aggregation.rs:259-277`）。
   - `Partial2Mode`/`FinalMode`：`updateAvg` 将 `Args[1]` 当作局部 sum、`Args[0]` 当作局部 count。NULL sum 使该部分结果整体被忽略；非 NULL sum 先累计，然后把局部 count 加入总计数。
   - `DedupMode`：当前不受支持，立即 panic。
4. 若还要进入下一聚合阶段，`GetPartialResult` 输出 `[NewIntDatum(Count), Value.clone()]`，该次序必须与二阶段的 `Args[0]`/`Args[1]` 约定一致。
5. 若要输出最终值，`GetResult` 检查 sum 的 Datum kind：
   - `KindFloat64`：计算 `sum / Count as f64`。
   - `KindMysqlDecimal`：用 `DecimalDiv(sum, NewDecFromInt(Count), ..., div_precision_increment)` 求商，再按 `RetTp.GetDecimal()` 指定的小数位做半入舍入；小数位为 `-1` 时采用 `mysql::MaxDecimalScale`，且最终上限同样为该常量。
   - 其他 kind（包括初始 NULL）：保持默认 Datum，即 NULL。
6. 分组状态复用时调用 `ResetContext`，通用实现重建/清除 DISTINCT 状态并重置 `Count`、`Value` 及通用缓冲字段。

## 数据与状态

`avgFunction` 自身没有逐组可变累计字段；逐组状态全部位于 `AggEvaluateContext`（定义于 `aggregation.rs:155-169`）：

- `Ctx: Arc<dyn expression::EvalContext>`：参数表达式求值以及除法精度配置的来源。
- `DistinctChecker: Option<distinctChecker>`：仅原始输入路径的 `aggFunction::updateSum` 使用；二阶段 `updateAvg` 不再次去重。
- `Count: i64`：原始路径为非 NULL、通过 DISTINCT 检查的输入数；合并路径为所有有效局部 sum 对应局部 count 之和。
- `Value: types::Datum`：累计 sum。初始 NULL；`calculateSum` 决定首次赋值以及后续按 MySQL 类型规则相加。

关键不变量是部分结果布局恒为 `(count, sum)` 且位置固定。Final/Partial2 描述符必须至少提供两个参数并保持该顺序；本文件直接索引 `Args[1]` 和 `Args[0]`，不在运行时检查长度。另一个不变量是 `Value` 的 kind 必须是 AVG 支持的 Float64 或 MySQL Decimal，才能得到非 NULL 最终结果。

## 依赖与调用关系

上游直接关系：

- `descriptor.rs::AggFuncDesc::GetAggFunc`：常规描述符工厂，`ast::AggFuncAvg` 分支实例化本类型。
- `aggregation.rs::NewDistAggFunc`：从 `tipb::ExprType::Avg` 和 PB 聚合模式实例化本类型，供分布式/mock TiKV 行式路径使用。
- `aggregation_aster_unit_test.rs` 中 `avg_and_sum_match_go_weighted_input_and_distinct_cases` 与 `avg_final_mode_combines_partial_count_and_sum`：RustCodeGraph 显示其直接调用本文件的 `Update`、`GetResult`、`GetPartialResult` 和 `CreateContext`。

下游直接关系：

- `aggFunction::updateSum`：原始输入累加、NULL 过滤、DISTINCT 检查与 `Count += 1`。
- `expression::Expression::Eval`：`updateAvg` 分别求值局部 sum 与 count。
- `calculateSum`：合并原始或局部 sum，并传播类型/溢出类错误。
- Decimal API：`DecimalDiv`、`NewDecFromInt`、`MyDecimal::Round`；以及 `EvalContext::GetDivPrecisionIncrement`。
- `chunk::Row` 是输入行，`types::Datum` 是状态和输出载体，`stmtctx::StatementContext::TypeCtx` 提供相加时的类型上下文。

Cargo 直接依赖由 crate 统一声明；本文件实际使用的表达式、chunk、stmtctx、parser/mysql 与 types 门面分别来自 `expression`、`chunk`、`stmtctx`、parser 依赖及 crate 根拼装的 datum/decimal/field 依赖。`Cargo.toml` 没有本文件专属 feature。

## 错误处理与边界

- 两条更新路径都返回 `Result<(), Error>`。表达式 `Eval` 失败和 `calculateSum` 失败通过 `?` 原样向上层传播；发生这些错误后调用方不得把当前聚合结果视为成功完成。
- `updateAvg` 若局部 sum 为 NULL，会在求值 count 之前返回成功并忽略整行。这与 Go 对照实现一致。
- `DedupMode` 不是可恢复错误，而是 `panic!("DedupMode is not supported now.")`；构造/规划层必须避免给 AVG 选择此模式。
- `GetResult` 对不支持的 Datum kind 静默返回默认 NULL，不返回错误。空输入因 `Value` 仍为 NULL，也得到 NULL；相关 Go/Rust聚合测试验证了这一点。
- Float64 且 `Count == 0` 时遵循 IEEE-754 除法。`avg_test.rs::zero_count_float_partial_state_matches_go_ieee_division` 人工构造 `sum=1.0,count=0`，验证结果为无穷大。
- Decimal 且 `Count == 0` 时，`DecimalDiv` 和 `Round` 的返回值都被 `let _ = ...` 忽略；`avg_test.rs::zero_count_decimal_partial_state_matches_go_logged_error_result` 验证当前结果是非 NULL 的零。此行为是兼容性边界，不应在没有同步 Go 语义和测试的情况下“清理”为错误返回。
- `RetTp` 缺失时 Decimal 舍入回退到 `MaxDecimalScale`；`RetTp.GetDecimal() == -1` 时也如此。
- `Args` 数量和顺序依赖构造层保证。Final/Partial2 若少于两个参数会因 `Args[1]` 越界而 panic；count Datum 类型不符合约定时 `GetInt64` 的具体行为由 Datum API 决定，本文件不另行校验。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或 I/O 资源。`avgFunction` 只保存不可在本文件内替换的描述符包装；可变聚合状态由调用方以 `&mut AggEvaluateContext` 逐组传入，`Update` 还要求 `&mut self`，因此同一个实例/上下文不会由 Rust 安全接口并发写入。

求值上下文通过 `Arc<dyn EvalContext>` 共享所有权，创建或重置分组状态时克隆/替换 Arc 而不手工管理释放。`Datum` 和部分结果在返回时 clone，生命周期不借用输入行。DISTINCT 检查器属于 `AggEvaluateContext`，在通用 `ResetContext` 中按描述符重新建立；分组复用不会沿用旧去重集合。内存计账、spill 和并行归并不由这个旧式行聚合实现直接管理。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/aggregation/avg.go`。Rust 基本逐段保留 Go 的结构与模式分支：

- Go `avgFunction` 嵌入 `aggFunction`；Rust 使用具名字段 `aggFunction`。
- 两边 `updateAvg` 都先求局部 sum，NULL 时忽略该行，再累计 sum，最后求局部 count 并加到计数。
- 两边 `Update` 对四种有效模式的路由一致，并都对 `DedupMode` panic。
- 两边部分结果顺序都是 `[count, sum]`；Float64 直接相除；Decimal 使用相同除法精度增量、返回类型 scale、`MaxDecimalScale` 上限和 `ModeHalfUp`。

可见差异与迁移事实：

- Go 在 `avg.go` 中单独实现 `ResetContext`；Rust 委托增强后的通用 `aggFunction::ResetContext`。Rust 通用实现除了重置 AVG 所需的 `Ctx`、`Value`、`Count` 和 DISTINCT 检查器，还清理共享上下文中的 Buffer/GotFirstRow 字段；对 AVG 的可观察状态仍满足 Go 意图。
- Go 用 `terror.Log(err)` 记录 `DecimalDiv` 和 `Round` 的错误；Rust 当前直接忽略返回值，不记录日志也不向上传播。零 count Decimal 测试固定了最终值兼容性，但未证明日志副作用等价，因此不能宣称错误可观测性完全一致。
- Rust 的 `RetTp` 是 `Option<FieldType>`，缺失时回退 `MaxDecimalScale`；Go 直接解引用 `RetTp`。这是 Rust 当前的防御性边界。

测试对应关系：Go `aggregation_test.go::{TestAvg, TestAvgFinalMode}` 覆盖空输入、加权输入、NULL、DISTINCT、部分结果和 FinalMode；Rust `aggregation_test.rs` 将同名测试转发到 `aggregation_aster_unit_test.rs` 的共享用例。Rust 另有 `avg_test.rs` 专门固定两个零 count 边界。

## 扩展指南

- 修改模式语义时，首选接入 `avgFunction::Update`；新增模式必须同时明确原始值还是 `(count,sum)` 输入、是否允许 DISTINCT，并同步描述符拆分/PB 模式映射。不要只在本文件增加分支而遗漏 `AggFuncDesc` 和 `NewDistAggFunc`。
- 修改二阶段协议时，必须成对修改 `updateAvg` 与 `GetPartialResult`，保持 count/sum 的位置和类型一致，并检查 planner/下推消费者的协议兼容性。
- 修改原始值过滤、DISTINCT 或 sum 累加规则时，应优先评估共享的 `aggFunction::updateSum` 和 `calculateSum`；这些符号也服务 SUM，改动风险会超出 AVG。
- 修改 Decimal 输出时，要同时核对 `GetDivPrecisionIncrement`、返回类型 scale、`MaxDecimalScale` 和 `ModeHalfUp`，并决定是否继续保持 Go 的“记录错误后返回结果”语义。把当前忽略错误改成传播错误会改变 `Aggregation::GetResult` 无 Result 返回值的接口，不能局部完成。
- 测试必须放在独立文件而非 `avg.rs`：AVG 通用/Final/DISTINCT 场景同步 `aggregation_aster_unit_test.rs` 及转发测试 `aggregation_test.rs`，精细边界同步 `avg_test.rs`；若声称 Go 等价，还需对照 `aggregation_test.go` 和 `avg.go`。
- 性能风险主要在每行表达式求值、Datum clone、Decimal 除法以及 DISTINCT 检查；不要在 `Update` 热路径引入额外分配。正确性风险集中在 NULL、零 count、部分结果顺序、Decimal scale/舍入和计数溢出；兼容性风险集中在 Go 的错误记录副作用与 PB 两阶段协议。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/expression/aggregation/avg.rs`（全部 117 行；`avgFunction`、`updateAvg`、`Aggregation` 实现）。
- crate 与模块边界：`pkg/expression/aggregation/Cargo.toml`、`pkg/expression/aggregation/lib.rs`。
- 工厂与共享状态/逻辑：`pkg/expression/aggregation/aggregation.rs`、`pkg/expression/aggregation/descriptor.rs`、`pkg/expression/aggregation/sum.rs`。
- Go 对照：`pkg/expression/aggregation/avg.go`、`pkg/expression/aggregation/aggregation_test.go::{TestAvg, TestAvgFinalMode}`。
- Rust 独立测试：`pkg/expression/aggregation/avg_test.rs`、`pkg/expression/aggregation/aggregation_test.rs`、`pkg/expression/aggregation/aggregation_aster_unit_test.rs::{avg_and_sum_match_go_weighted_input_and_distinct_cases, avg_final_mode_combines_partial_count_and_sum}`。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；文件查询确认 `avg.rs` 含 10 个符号；`node avgFunction` 给出 `NewDistAggFunc` 与 `GetAggFunc` 两条实例化入边；两个共享 Rust 测试的 node/trail 给出对 `avg.rs::{Update, GetResult, GetPartialResult, CreateContext}` 的直接调用边；`node updateSum` 和 `node AggEvaluateContext` 核对共享下游逻辑与状态字段。

任务为纯文档分析，按计划未运行 Cargo。结构完整性由任务指定的 11 标题检查命令验证；内容人工复核重点是文件存在理由、两阶段执行协议、安全扩展入口以及 Go/Rust 差异均有上述源码或调用边支撑。
