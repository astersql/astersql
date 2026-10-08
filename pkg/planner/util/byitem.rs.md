# `pkg/planner/util/byitem.rs`

## 文件定位

`byitem.rs` 属于 `astersql-planner-util` crate，由 [`pkg/planner/util/lib.rs`](lib.rs) 的 `mod byitem` 纳入并通过 `pub use byitem::*` 对外再导出。它位于 SQL 规划的通用数据模型层：把一个表达式与排序方向绑成 `ByItems`，供逻辑 `Sort`/`TopN`、物理 `Sort`/`TopN`、聚合函数的 `ORDER BY` 以及 Cascades 指纹比较共用。它不解析 SQL，也不自己执行排序。

crate 边界由 [`pkg/planner/util/Cargo.toml`](Cargo.toml) 确认：直接使用 `astersql-expression`、`astersql-planner-cascades-base` 和 `astersql-util-size`；该 manifest 无 feature 开关，`autotests = false`，测试由 `lib.rs` 显式挂载。目录下没有 `doc.go`，因此包边界以 Cargo manifest、`lib.rs` 与同路径 Go 文件为准。

## 核心职责

- `ByItems` 保存排序/分组表达式 `Expr` 和方向位 `Desc`，使表达式语义与 `ASC`/`DESC` 成为不可分割的规划属性。
- 实现 `cascades_base::Hash64` 和 `Equals`，使方向及表达式都参与 Memo/计划指纹和结构相等判定。
- 提供带上下文的语义相等、字符串化、深拷贝和内存估算，供上层计划结构递归处理。
- `StringifyByItemsWithCtx` 将整个切片格式化为方括号包围、空格分隔的调试文本。当前 Rust 源码搜索只找到定义，未找到生产调用者；计划 `EXPLAIN` 主路使用的是相邻 [`explain_misc.rs`](explain_misc.rs) 中的 `ExplainByItems`。

## 主要符号

- `pub struct ByItems { pub Expr: expression::ExprBox, pub Desc: bool }`：`ExprBox` 是 `Box<dyn expression::Expression>`，因此每个项拥有一个动态表达式；`Desc == true` 表示降序。两个字段均为公开可变字段，上层优化规则会替换 `Expr` 或改写方向。
- `impl Hash64::Hash64`：先写入 `Expr.Hash64`，再写入 `Desc`；顺序是指纹协议的一部分。
- `impl Equals::Equals`：先将 `dyn Any` 下转为 `ByItems`，类型不匹配时返回 `false`；匹配后同时比较 `Desc` 与表达式的结构 `Equals`。
- `ByItems::StringWithCtx`：调用表达式的 `StringWithCtx(context, redact)`；降序时追加字面量 ` true`，而非 SQL 文本 `DESC`，这是与 Go 实现保持一致的内部表示。
- `ByItems::Clone`：返回 `self.clone()`。`ByItems` 的派生 `Clone` 会调用 `ExprBox::clone`，而 [`pkg/expression/expression.rs`](../../expression/expression.rs) 将其实现为 `CloneExpr()`，所以是表达式深拷贝，不是共享同一个 `Box`。
- `ByItems::Equal`：在 `EvalContext` 下调用表达式的语义 `Equal`，并比较 `Desc`。这与接受 `Any` 的结构 `Equals` 是两条不同协议。
- `ByItems::MemoryUsage`：返回 `size::SizeOfBool + Expr.MemoryUsage()`，表示该项的布尔字段和表达式自报内存，不包含外层 `Vec` 容量或其他拥有者开销。
- `StringifyByItemsWithCtx`：逐项使用禁止脱敏的 `expression::errors::RedactLogDisable`，用空格连接后包装为 `[...]`；空切片结果为 `[]`。

## 执行流程

1. 规划构建器把 AST 中的 `ORDER BY`/`GROUP BY` 表达式转换为 `ExprBox`，并与方向位组成 `ByItems`。Rust 直接实例化可见 [`pkg/planner/core/logical_plan_builder_runtime.rs`](../core/logical_plan_builder_runtime.rs)。
2. 逻辑优化阶段由 `LogicalSort.ByItems` 或 `LogicalTopN.ByItems` 持有列表；替换列、剪枝常量排序项、推导有序属性或下推 TopN 时读写 `Expr` 和 `Desc`。
3. Cascades 和计划缓存相关路径通过 `Hash64`/`Equals` 判断两个排序要求是否同构；表达式一样但方向不同必须产生不同结果。
4. 逻辑计划转物理计划时，`Clone` 在不共享可变表达式的前提下传递排序项。聚合函数描述符则把它保存在 `AggFuncDesc.OrderByItems`中，并继续纳入指纹、相等、文本和内存计算。
5. 物理执行入口如 [`pkg/executor/physical_plan_runtime.rs`](../../executor/physical_plan_runtime.rs) 的 `sort_keys` 将每项解析为列下标和升/降序 `SortKey`；到这一层时，表达式必须已解析为列引用。

## 数据与状态

`ByItems` 只有两个实例字段，没有全局可变状态、缓存、隐式 ID 或条件编译分支。`Expr` 拥有表达式 trait object；`Desc` 是值语义布尔量。结构本身可 `Clone`，但未派生 `Default`、`Debug` 或 Rust `PartialEq`，相等性必须明确选用 `Equals` 或 `Equal`。

重要不变式是：排序方向必须与表达式同时参与克隆、哈希和相等比较。如果优化规则替换 `Expr`，必须保留原 `Desc`；如果方向变化，指纹也必须变化。[`hash64_equals_test.rs`](../core/operator/logicalop/logicalop_test/hash64_equals_test.rs) 已对 `LogicalSort` 的这一不变式提供间接回归证据。

## 依赖与调用关系

- 下游依赖：`expression::ExprBox` 提供克隆、哈希、结构/语义相等、字符串化和内存估算；`expression::exprctx::{ParamValues, EvalContext}` 携带参数或求值语义；`cascades_base::{Hash64, Equals, Hasher}` 定义指纹协议；`size::SizeOfBool` 提供与 Go 移植口径一致的布尔大小。
- 上游构建者：RustCodeGraph 对 `ByItems` 的 trail 显示 `logical_selection.rs::DeriveTopN` 和 `cache_snapshot.rs::restore_by_items` 直接实例化该类型；搜索还确认 `logical_plan_builder_runtime.rs` 多处构建逻辑 Sort/TopN 的项。
- 上游持有者：[`logical_sort.rs`](../core/operator/logicalop/logical_sort.rs) 与 [`logical_top_n.rs`](../core/operator/logicalop/logical_top_n.rs) 保存和改写 `Vec<ByItems>`；[`pkg/expression/aggregation/descriptor.rs`](../../expression/aggregation/descriptor.rs) 在 `AggFuncDesc` 中对其迭代执行哈希、相等、字符串化和内存累加。
- 上游消费者：[`physical_plan_runtime.rs`](../../executor/physical_plan_runtime.rs) 把 `ByItems` 转为执行器 `SortKey`；物理 Sort/TopN 和 Cascades implementation rules 则克隆或检查它们以生成物理属性。
- RustCodeGraph 的文件节点报告 `byitem.rs` 被 34 个文件使用；对 `StringifyByItemsWithCtx` 的 callees 结果确认它只下调同文件的 `StringWithCtx`。该函数的 Rust 调用者搜索为空，不应把 Go 中的存在性误写成 Rust 主路已调用。

## 错误处理与边界

本文件的 API 都不返回 `Result`，也没有显式 I/O。`Equals` 对错误的动态类型返回 `false`，而非 panic。由于 Rust 字段是非空 `Box<dyn Expression>`，`Clone`、`Hash64`、`MemoryUsage` 等不需要处理 Go 的 nil receiver 或 nil expression；这也意味着从外部快照/协议恢复时，必须先构造有效表达式才能创建 `ByItems`。

`StringWithCtx` 和 `Equal` 将上下文原样传给表达式；如果具体表达式对参数或求值上下文有额外约束，由调用者保证。`StringifyByItemsWithCtx` 固定关闭脱敏，因此不应直接用于可能暴露敏感常量的日志通道；需要可配置脱敏时应使用单项 `StringWithCtx` 并传入合适策略。

执行层还有更窄的边界：`physical_plan_runtime.rs::sort_keys` 只接受已解析为列的 `Expr`，非列表达式会返回 `PhysicalRuntimeError`；这个错误不是由 `byitem.rs` 产生，但是安全扩展 `ByItems` 消费链时必须保留的下游前置条件。

## 并发与资源生命周期

`byitem.rs` 不创建锁、线程、异步任务、通道、事务或文件句柄。`ExprBox` 拥有表达式，`ByItems` 被 drop 时一并释放；`Clone` 通过 `CloneExpr` 创建独立表达式对象，以免优化规则改写一份计划时意外污染另一份。

`expression::Expression` 协议承担具体表达式的可发送/可共享约束，但 `ByItems` 的公开字段本身不提供内部同步。共享计划时应遵守上层计划的不可变/外部同步规则；不应在多线程中无保护地同时改写 `Expr` 或 `Desc`。

## 与 Go 版本的对应关系

Rust 文件逐项对应 [`pkg/planner/util/byitem.go`](byitem.go) 的 `ByItems`、`Hash64`、`Equals`、`StringWithCtx`、`Clone`、`Equal`、`MemoryUsage` 和 `StringifyByItemsWithCtx`。核心顺序与格式均保留：哈希是“表达式后方向”，降序字符串追加 ` true`，切片文本为方括号加空格分隔。

语言模型带来三个可见差异：

1. Go 用 `*ByItems` 和接口字段，因此 `Equals` 显式拒绝 nil，`MemoryUsage` 允许 nil receiver 和 nil `Expr`；Rust 用值类型加非空 `ExprBox`，这些 nil 分支不可表示。
2. Go `Clone` 显式调用 `by.Expr.Clone()`；Rust 通过派生 `Clone` 到 `ExprBox::CloneExpr()` 达到同样的深拷贝语义。
3. Go `StringifyByItemsWithCtx` 使用 `strings.Builder`；Rust 收集为 `Vec<String>` 后 `join(" ")`。输出一致，但 Rust 路径会为中间向量和每项字符串分配内存，如果未来成为高频热点，可考虑直接写入 `String`。

同目录未发现专门的 `byitem_test.rs`/`byitem_test.go`。Go 的 [`hash64_equals_test.go`](../core/operator/logicalop/logicalop_test/hash64_equals_test.go) 和 Rust 的同名独立测试都通过 `LogicalTopN`/`LogicalSort` 间接校验表达式及方向对哈希/相等的影响。

## 扩展指南

- 增加新字段时，必须同步决定它是否参与 `Hash64`、`Equals`、`Equal`、`Clone`、`StringWithCtx` 和 `MemoryUsage`；任何哈希/相等不对称都可能破坏 Memo 去重和计划缓存。
- 改变 `Desc` 语义或增加 NULL 排序等方向属性时，需要同时检查逻辑 Sort/TopN 剪枝与下推、物理属性匹配、PB 转换、`executor::sort_keys` 和 explain 输出，不能只改本结构。
- 改变文本格式时应区分 `StringWithCtx` 的内部/Go 兼容形式与 `ExplainByItems` 的用户可见 `:desc` 形式，并明确脱敏策略。
- 新增本类型的直接单元测试时，按仓库规则放在独立 `pkg/planner/util/byitem_test.rs` 中，并由 `lib.rs` 的 `#[cfg(test)] #[path = "byitem_test.rs"] mod byitem_test;` 挂载，不要把测试内嵌到 `byitem.rs`。
- 最小回归集应覆盖：不同动态类型的 `Equals == false`；相同表达式在 `Desc` 不同时哈希与相等均不同；克隆后更改表达式不影响原值；空/多项字符串化；上下文相等；内存估算包含表达式。同时保留 [`logicalop_test/hash64_equals_test.rs`](../core/operator/logicalop/logicalop_test/hash64_equals_test.rs)、[`logical_sort_test.rs`](../core/operator/logicalop/logical_sort_test.rs)、[`logical_top_n_test.rs`](../core/operator/logicalop/logical_top_n_test.rs) 和 [`physical_plan_runtime_test.rs`](../../executor/physical_plan_runtime_test.rs) 的集成语义验证。
- 兼容风险集中在 Go/Rust 指纹和输出差异；正确性风险集中在优化改写时丢失方向或共享表达式；性能风险集中在高频克隆及 `StringifyByItemsWithCtx` 的中间分配。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/planner/util` 确认 Rust/Go 目标和独立测试文件边界。
- `rustcodegraph node --file pkg/planner/util/byitem.rs --offset 1 --limit 240`：读取全部 94 行，确认 1 个公开结构、2 个 trait 实现、4 个关联方法和 1 个自由函数，且无条件编译项。
- `rustcodegraph query ByItems --kind struct --json` 与 `node pkg/planner/util/byitem.rs::ByItems`：定位 Rust/Go 两个结构，trail 显示 `DeriveTopN`、`TestLogicalSortHash64Equals`、`restore_by_items` 的实例化边。
- `rustcodegraph query StringifyByItemsWithCtx --json` 及 callees 结果：确认 Rust/Go 同名符号，并确认 Rust 函数下调 `StringWithCtx`。精确 callers 查询未返回调用边，随后用 `rg` 核对 Rust 生产文件，仅有定义。
- 已读源与边界文件：[`byitem.rs`](byitem.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`byitem.go`](byitem.go)、[`pkg/expression/expression.rs`](../../expression/expression.rs)、[`pkg/expression/aggregation/descriptor.rs`](../../expression/aggregation/descriptor.rs)、[`explain_misc.rs`](explain_misc.rs)、[`physical_plan_runtime.rs`](../../executor/physical_plan_runtime.rs)。
- 已读测试证据：[`logicalop_test/hash64_equals_test.rs`](../core/operator/logicalop/logicalop_test/hash64_equals_test.rs) 校验 `LogicalSort` 的方向参与哈希/相等；[`logical_sort_test.rs`](../core/operator/logicalop/logical_sort_test.rs) 和 [`logical_top_n_test.rs`](../core/operator/logicalop/logical_top_n_test.rs) 校验表达式替换及降序 explain；[`physical_plan_runtime_test.rs`](../../executor/physical_plan_runtime_test.rs) 校验多列 TopN 的实际排序消费；Go [`hash64_equals_test.go`](../core/operator/logicalop/logicalop_test/hash64_equals_test.go) 提供移植意图对照。同目录无专用 byitem 测试。
- 本任务为纯文档分析，按计划不运行 Cargo；完成判定以固定十一章结构检查和上述源码/调用边人工复核为准。
