# `pkg/expression/aggregation/explain.rs`

## 文件定位

本说明对应源文件 [`explain.rs`](./explain.rs)。它属于 Cargo 包 `astersql-expression-aggregation`（见同目录 `Cargo.toml`），负责把规划期的单个 `AggFuncDesc` 格式化为聚合算子的 EXPLAIN 片段。模块在 `lib.rs` 中以 `mod explain` 装配，并通过 `pub use explain::*` 将两个公开函数重导出到 crate 根，因此调用方使用 `aggregation::ExplainAggFunc`，无需知道私有模块名。

当前 Rust 生产代码中，直接调用点是 `pkg/planner/core/operator/physicalop/base_physical_agg.rs` 的 `BasePhysicalAgg::explain`：它先输出可选的 `group by:`，再逐个调用 `ExplainAggFunc` 输出 `funcs:`，最后追加聚合结果列。由此，本文件位于“物理计划生成/展示 → 聚合函数描述文本化”链路上，不负责解析 SQL、构造聚合描述、执行聚合或输出完整 EXPLAIN 行。

## 核心职责

- `ExplainAggFunc` 根据 `AggFuncDesc` 的名称、模式、`HasDistinct`、参数和 `OrderByItems` 生成稳定的单函数文本，例如普通形式 `count(distinct col)`，以及 `GROUP_CONCAT` 特有的 `group_concat(value order by key desc separator sep)`。
- `normalized` 决定所有参数及排序表达式走 `Expression::ExplainNormalizedInfo` 还是 `Expression::ExplainInfo(ctx)`；前者用于规范化计划文本，后者保留求值上下文相关的可读信息。
- `SetExplainAggModeForTest` 控制是否把 `AggFunctionMode::ToString()` 插入函数名后的第一个位置。该开关是 Go `show-agg-mode` failpoint 的 Rust 等价测试接口，不是常规会话配置。
- 本文件只拼接描述文本，不改变 `AggFuncDesc`，也不触发聚合求值。

## 主要符号

- `static SHOW_AGG_MODE: AtomicBool`：进程级测试开关，初始为 `false`。读取和写入都使用 `Ordering::SeqCst`。
- `pub fn SetExplainAggModeForTest(enabled: bool)`：覆盖上述全局开关。启用后，开头由 `name(` 变为 `name(mode,`；模式字符串来自 `pkg/expression/aggregation/aggregation.rs` 的 `AggFunctionMode::ToString`，可能为 `complete`、`final`、`partial1`、`partial2` 或 `deduplicate`。
- `pub fn ExplainAggFunc(ctx: &dyn expression::EvalContext, aggregate: &AggFuncDesc, normalized: bool) -> String`：本文件的主入口。它借助 `use expression::Expression as _` 调用 trait 方法，并通过 `AggFuncDesc` 对 `baseFuncDesc` 的 `Deref` 访问 `Name` 和 `Args`。
- `AggFuncDesc`（定义于 `descriptor.rs`）：输入描述符；本函数读取 `Name`、`Mode`、`HasDistinct`、`Args`、`OrderByItems`，不读取 `GroupingID` 或返回类型。
- `planner_util::ByItems`（定义于 `pkg/planner/util/byitem.rs`）：`OrderByItems` 元素，提供 `Expr` 与 `Desc`。本文件自行规定 EXPLAIN 中的逗号和 ` desc` 文案。

文件没有类型、trait、条件编译项或可失败的返回值；除全局原子开关外也没有模块级状态。

## 执行流程

1. `ExplainAggFunc` 读取 `SHOW_AGG_MODE`。关闭时以 `aggregate.Name + "("` 开始；开启时再写入 `aggregate.Mode.ToString()` 和逗号，例如 `count(partial1,`。
2. 若 `aggregate.HasDistinct` 为真，紧接着写入 `distinct `。因此模式开启时顺序是 `name(mode,distinct ...)`，与 Go 对照实现一致。
3. 按原顺序遍历 `aggregate.Args`。普通参数从第二个起先写 `, `，再按 `normalized` 选择表达式格式化方法。
4. 当且仅当名称等于 `ast::AggFuncGroupConcat` 且当前参数是最后一个参数时，把该参数视为 separator 参数：
   - 若 `OrderByItems` 非空，先写 ` order by `，逐项格式化排序表达式；多个排序项以 `, ` 分隔，`Desc == true` 时追加 ` desc`。
   - 随后无条件写 ` separator `，再格式化最后一个参数本身。
   - 前面的参数仍按普通参数规则输出，所以 `GROUP_CONCAT` 的值参数、排序子句和 separator 保持 Go 版布局。
5. 追加右括号并返回 `String`。函数不排序参数或 `OrderByItems`；其输出顺序完全由描述符决定。

一个重要结构约定是：`GROUP_CONCAT` 的最后一个 `Args` 元素必须已经是 separator。该约定由描述符构造/拆分链维护，本文件只消费它；若输入描述符不满足约定，本函数仍会把最后一个参数标成 separator，而不会报错。

## 数据与状态

输入均为共享借用：`ctx` 是动态 `EvalContext`，`aggregate` 是不可变 `AggFuncDesc`。输出是函数内新建并逐步增长的拥有型 `String`，没有缓存，也不修改表达式或描述符。

`normalized == true` 时，参数和排序项调用无 `ctx` 参数的 `ExplainNormalizedInfo`；此分支虽仍要求调用者传入 `ctx`，但函数不会使用它。`normalized == false` 时，每个相关表达式都共享同一个 `ctx` 调用 `ExplainInfo(ctx)`。

唯一持久状态是 `SHOW_AGG_MODE`。它不是线程局部、会话局部或描述符字段，而是整个进程共享的布尔值。`SeqCst` 保证各线程观察到单一全序，但不提供“仅对某一次调用生效”的隔离。

## 依赖与调用关系

上游：

- `pkg/planner/core/operator/physicalop/base_physical_agg.rs::BasePhysicalAgg::explain` 是 RustCodeGraph 与 `rg` 均确认的当前 Rust 生产调用者。它由 `ExplainInfo` 和 `ExplainNormalizedInfo` 分别以 `normalized = false/true` 进入。
- `pkg/expression/aggregation/lib.rs` 声明并重导出本模块；`pkg/planner/core/Cargo.toml` 以 `aggregation-dependency` 依赖本 crate。
- 全仓库 Rust 搜索只发现 `SetExplainAggModeForTest` 的定义，未发现调用者，因此当前 Rust 树中该测试开关尚未被测试或其他代码接线。

下游：

- `AggFuncDesc` 与其 `Deref<Target = baseFuncDesc>`（`descriptor.rs`）提供名称、参数、模式、DISTINCT 和排序项。
- `AggFunctionMode::ToString`（`aggregation.rs`）提供稳定的小写模式名。
- `expression::Expression::{ExplainInfo, ExplainNormalizedInfo}` 格式化参数及排序表达式。
- `ast::AggFuncGroupConcat` 是启用特殊布局的名称常量；`planner_util::ByItems::{Expr, Desc}` 提供 ORDER BY 内容。
- 标准库 `String`/`format!` 负责缓冲，`AtomicBool` 负责测试开关。

`Cargo.toml` 没有为本文件设置 feature 门控；`expression`、`planner-util` 和 `parser-ast` 都是该 crate 的常规路径依赖。目标包没有 `doc.go`，因此没有额外的包级契约需要合并。

## 错误处理与边界

`ExplainAggFunc` 返回 `String` 而非 `Result`，文件内没有显式错误传播。表达式的两个 EXPLAIN trait 方法同样返回 `String`，所以格式化阶段没有可恢复错误通道。内存分配失败等进程级异常不在此 API 的建模范围内。

边界行为包括：

- 空 `Args` 得到 `name()`；若模式开关开启则得到带悬空逗号的 `name(mode,)`。代码没有为零参数模式单独修正标点。
- `HasDistinct` 在空参数时仍会产生 `name(distinct )`；合法描述符约束由上游负责。
- 只有严格等于 `ast::AggFuncGroupConcat` 的名称触发 ORDER BY/separator 布局；其他聚合即使携带 `OrderByItems` 也不会在这里输出这些项。
- `GROUP_CONCAT` 没有参数时不会进入 separator 分支；只有一个参数时该参数会被当作 separator。正确参数布局是调用方必须维持的不变量。
- 降序显式输出 ` desc`，升序不输出 `asc`，与 Go 实现一致。
- 函数不做转义、重排或去重；表达式文本的引用、脱敏及规范化语义完全委托给 `Expression` 实现。

## 并发与资源生命周期

主函数只读借用输入并拥有局部 `String`，无锁、无异步任务、无通道、无事务或外部资源；返回后局部缓冲所有权转给调用者，输入借用结束。

`SHOW_AGG_MODE` 可被多线程安全读写，不存在数据竞争。需要注意的是，`SetExplainAggModeForTest(true)` 与随后恢复 `false` 之间是进程级影响窗口：并行生成 EXPLAIN 的线程都会看到可能变化的模式。`SeqCst` 只保证原子可见性，不保证测试作用域隔离，也没有 RAII guard 自动恢复旧值。因此新增测试应串行化该全局状态、在所有退出路径恢复原值，并避免与其他 EXPLAIN 断言并发执行。

性能上，函数使用一个增长中的 `String`，但开头的 `format!` 和每个表达式返回的临时 `String` 都会产生分配；这是展示路径而非聚合热执行路径。若优化分配，必须保持字节级文本兼容并用独立测试覆盖普通、规范化与 `GROUP_CONCAT` 分支。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/aggregation/explain.go`。Rust `ExplainAggFunc` 保留了 Go 版的核心顺序和标点：可选 mode、`distinct `、参数逗号、`GROUP_CONCAT` 的 ` order by `、可选 ` desc`、` separator `，以及 normalized/非 normalized 两套表达式格式化。

主要实现差异：

- Go 用 `bytes.Buffer` 与 `fmt.Fprintf`，Rust 用 `String`、`format!` 和 `push_str`。
- Go 每次调用通过 failpoint `show-agg-mode` 读取局部 `showMode`；Rust 用进程级 `AtomicBool`，并提供 `SetExplainAggModeForTest`。两者输出语义对齐，但启停机制与测试隔离方式不同。
- Go 当前还有 `pkg/planner/core/operator/logicalop/logical_aggregation.go` 和物理聚合两个生产调用点；全仓库 Rust 搜索只找到物理聚合调用点。因此不能据本文件声称 Rust 已覆盖 Go 的逻辑聚合 EXPLAIN 接线。

Go 测试 `pkg/executor/test/tiflashtest/tiflash_test.go::TestMppAggShouldAlignFinalMode` 启用 failpoint 后，断言 `complete`、`partial1`、`partial2`、`final` 出现在 EXPLAIN 中，是模式文本及插入位置的直接对照证据。Rust 当前没有对 `SetExplainAggModeForTest` 的直接测试。

## 扩展指南

- 若新增聚合通用修饰词，优先在 `ExplainAggFunc` 的“mode → distinct → args”顺序中确定稳定位置，并同步核对 Go `ExplainAggFunc`，避免计划文本、归一化摘要或 golden 结果漂移。
- 若新增仅特定聚合使用的 ORDER BY/separator 类语法，不应仅靠名称分支猜测参数布局；先在 `AggFuncDesc` 构造与拆分逻辑中确认不变量，再扩展本函数。特别要复查 `descriptor.rs::Split` 对最终阶段参数的安排。
- 若修改模式展示，需同步 `aggregation.rs::AggFunctionMode::ToString`、Rust 测试开关语义和 Go failpoint 输出；模式名可能被 EXPLAIN 测试或诊断工具依赖。
- 建议在同目录新增独立 `explain_test.rs`，并从 `lib.rs` 以 `#[cfg(test)] #[path = "explain_test.rs"] mod explain_test;` 装配；不要把测试写进 `explain.rs`。至少覆盖普通参数/DISTINCT、normalized 分支、多个 ORDER BY 项及 DESC、separator、各 mode、空参数边界，并妥善恢复全局开关。
- 端到端文本变化还应同步检查 `pkg/planner/core/tests/null/null_test.rs` 的 `GROUP_CONCAT` EXPLAIN 断言，以及物理计划相关测试数据。若补齐 Rust 模式测试，应以 Go `TestMppAggShouldAlignFinalMode` 为行为基准，但不必复制其完整 TiFlash 环境才能验证纯字符串函数。
- 兼容风险主要是 EXPLAIN 文本和 normalized 计划摘要变化；性能风险主要来自在计划展示路径增加额外表达式格式化或分配。函数本身不执行聚合，因此不应在这里加入运行时求值副作用。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；`files --filter pkg/expression/aggregation` 确认目标 Rust/Go 文件、描述符、模块入口与邻近独立测试均已索引。
- RustCodeGraph `explore "pkg/expression/aggregation/explain.rs ExplainAggFunc"`：读取了目标文件完整源码，并给出 Rust 调用者 `pkg/planner/core/operator/physicalop/base_physical_agg.rs::explain`。随后 `query ExplainAggFunc --kind function --json` 区分出 Go/Rust 两个同名符号。限定名称的 `callers/callees` 命令未在 30 秒内返回结果，因此调用边又以精确 `rg` 搜索和调用点源码核验。
- 已读 Rust 生产路径：`pkg/expression/aggregation/explain.rs`、`lib.rs`、`Cargo.toml`、`descriptor.rs`、`aggregation.rs`，`pkg/planner/util/byitem.rs`，以及 `pkg/planner/core/operator/physicalop/base_physical_agg.rs`。
- 已读 Go 对照与测试：`pkg/expression/aggregation/explain.go`、`pkg/executor/test/tiflashtest/tiflash_test.go::TestMppAggShouldAlignFinalMode`；Go 调用搜索还确认了逻辑与物理聚合两个入口。
- 已读/搜索 Rust 测试：`pkg/planner/core/tests/null/null_test.rs` 直接断言 `group_concat(... order by ... separator ...)` 的完整 EXPLAIN 片段；`pkg/planner/core/integration_test.rs::test_agg_with_json_push_down_to_ti_flash` 覆盖含 `GROUP_CONCAT` 的聚合规划，但不直接断言本函数全部字符串分支；同目录测试搜索未找到 `ExplainAggFunc` 或 `SetExplainAggModeForTest` 的直接用例。
- 本任务只新增说明文档，未运行 Cargo。结构验证按任务文件给定命令执行；其退出码记录在任务完成交付中。
