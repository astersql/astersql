# `pkg/planner/core/operator/logicalop/hash64_equals_generated.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-logicalop` crate。crate 入口 `pkg/planner/core/operator/logicalop/lib.rs` 以私有模块 `mod hash64_equals_generated;` 编入它，并把各逻辑算子类型公开导出；本文件本身不声明新类型，而是为这些类型追加固有的 `Hash64` / `Equals` 方法。

文件名沿用 Go 侧生成物 `pkg/planner/core/operator/logicalop/hash64_equals_generated.go`，但当前 Rust 文件不是逐字生成的 Go 代码：它使用 Rust `std::hash::Hasher`、表达式规范哈希和若干精简字段表示来实现结构指纹与相等判断。其用途是为逻辑计划节点提供可复用的“相等对象必须得到相同哈希输入”契约；是否把这些方法接入某个优化器容器，应由调用方代码另行确认。

`Cargo.toml` 的 `[package.metadata.porting]` 将本 crate 对应到 Go 包 `pkg/planner/core/operator/logicalop`。本文件通过 `use crate::*` 使用 crate 入口重导出的表达式、列、排序项、Handle 列及各逻辑算子类型，没有单独 feature 或条件编译项。

## 核心职责

核心职责分为两层：

1. 六组私有辅助函数把常见值稳定地写入调用者提供的 `dyn Hasher`，或按相同投影比较：`hash_bytes` / `hash_exprs` / `hash_columns` / `hash_column_groups` / `hash_sort_items` / `hash_by_items` 及对应的 `equal_*` 函数。
2. 为 18 类逻辑对象定义公开固有方法：`LogicalSchemaProducer`、`LogicalJoin`、`LogicalAggregation`、`LogicalApply`、`LogicalExpand`、`LogicalLimit`、`LogicalMaxOneRow`、`DataSource`、`LogicalMemTable`、`LogicalUnionAll`、`LogicalPartitionUnionAll`、`LogicalProjection`、`LogicalSelection`、`LogicalSequence`、`LogicalShow`、`LogicalShowDDLJobs`、`LogicalSort`、`LogicalTableDual`、`LogicalTopN`、`LogicalUnionScan`、`LogicalWindow` 和 `LogicalLock`。其中 `LogicalWindow` 在本文件仅补 `Equals`，它的无参数 `Hash64() -> u64` 位于 `logical_window.rs`。

这里的“相等”是每种节点选定字段上的结构相等，不是 Rust `PartialEq`，也不保证比较所有运行时字段。例如 `LogicalShow` 只比较 schema，`LogicalShowDDLJobs` 不比较 `JobNumber`；相应独立测试明确锁定了这种排除行为。

## 主要符号

- `hash_bytes(h, value)`：先写 `usize` 长度，再写字节，避免仅拼接字节造成边界歧义。
- `hash_exprs` / `equal_exprs`：有序处理 `Expression` 切片；表达式身份取 `CanonicalHashCode()`，因此列表长度、顺序和每项规范哈希都参与结果。
- `hash_columns` / `equal_columns`：有序处理 `Column`，只投影 `UniqueID` 与物理 `ID`；`Index`、类型及其他列元数据不在此辅助函数的契约内。
- `hash_column_groups` / `equal_column_groups`：用于 `LogicalAggregation::PossibleProperties` 的二维有序列组。
- `hash_sort_items` / `equal_sort_items`：比较 `SortItem.Col.UniqueID` 和 `Desc`。
- `hash_by_items` / `equal_by_items`：比较排序表达式的规范哈希和 `Desc`。
- `LogicalSchemaProducer::{Hash64, Equals}`：哈希使用列投影，比较则委托 `Schema::Equal`；扩展时必须确认两者仍保持一致。
- `LogicalJoin`：覆盖 `JoinType`、schema 和五组条件（等值、null-aware 等值、左、右、其他）。
- `LogicalAggregation`：覆盖 schema、聚合函数名/参数/模式/DISTINCT/函数内排序、分组表达式和可能属性；相等对聚合函数委托 `AggFuncDesc::Equals`。
- `LogicalApply`：在 `LogicalJoin` 投影之上加入关联列 `UniqueID`、`NoDecorrelate` 与 `IsLateral`。
- `LogicalExpand`：覆盖 distinct 分组列/表达式、大小、rollup 的列 ID、各层表达式以及可选 GID/GPos 列的 `UniqueID`。
- `LogicalLimit` / `LogicalTopN`：覆盖 schema、分区排序、偏移和数量；TopN 另覆盖排序项与 `PreferLimitToCop`。
- `DataSource` / `LogicalMemTable`：覆盖选定的表身份、别名、条件、物理表/存储偏好或数据库名；并不深比较完整 `TableInfo`。
- `LogicalProjection`：覆盖 schema、投影表达式和 `CalculateNoDelay`、`Proj4Expand`。
- `LogicalSelection`：直接以节点 `HashCode()` 同时作为哈希输入和相等依据。
- `LogicalUnionScan`：覆盖条件与 `HandleCols` 迭代产生的列 `UniqueID`。
- `LogicalLock`：覆盖锁类型与 `TblID2Handle` 的表 ID 键集合；哈希先排序，比较使用 `BTreeSet`，均忽略 map 值。

## 执行流程

调用者先创建具体逻辑节点并提供一个 `Hasher`。调用 `Hash64(&mut hasher)` 时，节点实现按固定字段次序写入基础 schema/内嵌节点，再写本节点字段。切片通常先写长度；嵌套对象通过辅助函数或其自身 `Hash64` 递归投影。调用 `Equals(&other)` 时，同样按固定顺序短路比较对应投影：长度不同立即为假，随后逐项比较。

典型组合链为：`LogicalApply::Hash64` 先调用 `LogicalJoin::Hash64`，再写关联列与两个标志；`LogicalPartitionUnionAll` 完全委托内嵌 `LogicalUnionAll`；多数 schema producer 节点先调用 `LogicalSchemaProducer::Hash64/Equals`。这种委托使基类投影只维护一处，但也意味着修改下层投影会同时改变所有委托者的身份语义。

特殊路径有三类：`LogicalSelection` 不遍历 `Conditions`，而是使用其 `HashCode()`；`LogicalWindow::Equals` 先比较 `logical_window.rs` 提供的 64 位值，再逐项复核函数名与参数，以降低仅凭哈希判等的风险；`LogicalLock` 通过排序/集合化消除哈希映射遍历顺序的影响。

## 数据与状态

本文件没有模块级常量、静态变量、缓存或持久状态。所有函数只读取接收者与参数，并把字节增量写入外部 `Hasher`；因此最终 `u64` 由调用者选择的 hasher 算法和此前已写入的状态共同决定，本文件不创建或重置 hasher。

重要不变量是：在同一实现和同一初始 hasher 状态下，若某类型的 `Equals` 返回真，则其对应哈希写入应一致。反向不成立，哈希相同仍可能碰撞。列表普遍有序且长度敏感；`LogicalLock::TblID2Handle` 键是例外，按集合语义处理。

可选列 `GID` / `GPos` 在哈希中以缺失值写 `0`，而相等区分 `None` 与 `Some(UniqueID = 0)`。这会产生允许的哈希碰撞，不破坏“相等蕴含同哈希”，但扩展者不能把哈希相同当作相等。

## 依赖与调用关系

直接依赖来自 `crate::*` 与标准库：`Expression::CanonicalHashCode`、`Column::{UniqueID, ID}`、`Schema::Equal`、`AggFuncDesc::Equals`、`HandleCols::{IterColumns, NumCols}`、`LogicalPlan::Schema`、`LogicalSelection::HashCode`、`LogicalWindow::Hash64`，以及 `std::hash::Hasher`、`std::any::Any`、`BTreeSet`。

crate 边界由同目录 `Cargo.toml` 确认：相关类型主要来自本 logicalop crate 及其 `expression`、`aggregation`、`planner_util`、`property`、`model`、`parser_ast`、`kv` 等路径依赖。目标文件没有直接 `use` 外部三方 crate。

RustCodeGraph 对目标文件给出的文件级使用者为 `logical_selection.rs`、`physicalop/enforce_test.rs` 和 `physicalop/task_base.rs`，但精确方法调用图没有给出可靠的生产调用边；仓库文本检索到的明确 Rust 调用主要在 `hash64_equals_generated_test.rs`、`logical_generated_aster_unit_test.rs` 及独立测试 crate `logicalop_test/hash64_equals_test.rs`。因此可以确认方法已编入并经过调用测试，但不能仅凭文件注释断言它已成为 Rust Cascades memo 去重的生产入口；`pkg/planner/cascades/memo/group_expr.rs` 有自己独立的逻辑计划哈希/相等路线。

## 错误处理与边界

所有方法返回 `bool` 或写入 hasher，不返回 `Result`、不显式产生业务错误。索引访问均由先比较长度再 `zip`，或由安全迭代器完成；正常输入下没有越界分支。表达式的 `CanonicalHashCode()` 和下游 `Equals` 的具体行为属于被委托类型的边界。

Rust 引用消除了 Go 接收者的 nil/type assertion 分支；Rust `Vec` 也不区分 Go 的 nil slice 与空 slice。本实现通常只写长度，所以两者迁移到 Rust 后自然合并。`LogicalExpand` 的 rollup 外层没有显式写每层长度，但相等仍逐层比较 `ColumnIDs`，不同分组边界存在哈希碰撞可能，不能只比较哈希。

字段选择是兼容性边界：`LogicalShow` 的 payload、`LogicalShowDDLJobs::JobNumber`、`LogicalLock` 的 handle map 值等被有意排除；新增字段不会自动参与身份。若字段影响逻辑语义却漏加到两种方法，会让不同计划被视为相等；若只改哈希或只改相等，会破坏契约。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或文件/网络资源。函数只进行同步只读遍历；唯一可变对象是调用者独占传入的 `&mut dyn Hasher`。Rust 借用保证单次调用期间该 hasher 不被并发写入，但类型是否可跨线程共享取决于具体节点字段和 hasher 实现，本文件没有声明额外的 `Send`/`Sync` 保证。

临时资源均为栈上值或短生命周期集合：`LogicalLock::Hash64` 收集并排序键向量，`Equals` 构建两个 `BTreeSet`，调用结束即释放。其时间复杂度分别包含键排序/集合构建的 `O(n log n)`，其他列表投影通常为线性复杂度；表达式规范哈希的成本由表达式实现决定。

## 与 Go 版本的对应关系

Go 对照是同目录 `hash64_equals_generated.go`，测试对照是 `logicalop_test/hash64_equals_test.go`。两侧总体结构一致：按节点类型选择语义字段，递归哈希并逐项判等；TopN、TableDual、Sort、Show、Selection、Projection、MemTable、Limit、Expand、Apply、Join、Aggregation、UnionAll 等均有相应测试意图。

但当前 Rust 不是完全等价生成物，已核实的差异包括：

- Go 每个节点先写 `plancodec.Type*` 类型标签，Rust 多数实现没有节点类型标签；跨类型复用同一哈希域时更易碰撞。
- Go 显式区分 nil/非 nil 切片并写嵌套长度；Rust `Vec` 无 nil，部分嵌套结构的边界编码也更精简。
- Go 对 `TableInfo`、`CIStr`、列、HandleCols、锁信息等调用完整 `Hash64/Equals`；Rust 常只比较 ID、lowercase 名或键集合。
- Go `LogicalWindow` 比较 schema、完整窗口函数描述、分区/排序和 frame；Rust 本文件的 `Equals` 先依赖另一文件的 `Hash64()`，再仅复核窗口函数名和参数。
- Go `LogicalMaxOneRow`、`LogicalSequence`、`LogicalLock` 使用 `BaseLogicalPlan`/完整锁对象；Rust 分别投影 schema、schema、锁类型加表 ID 键集合。
- Rust 独立回归测试新增并锁定 `PossibleProperties`、投影标志、`PreferLimitToCop` 参与身份，以及 Show payload 与 DDL job 数量不参与身份。

因此“与 Go 对齐”应理解为迁移相同能力与测试意图，不能声称当前字段级编码完全一致。任何要求跨语言哈希值相同的功能都需要额外协议设计和测试，当前证据只支持各语言内部的一致性。

## 扩展指南

新增逻辑算子时，应在其独立源文件定义类型，在本文件同时实现成对的 `Hash64` 和 `Equals`，并在独立测试文件中覆盖默认相等、每个语义字段变更、不参与字段保持相等，以及“相等则哈希相同”。不要把测试内嵌到本生产文件。

给已有算子增加语义字段时，先对照 Go 同名结构与生成文件确认字段是否属于身份，再同步更新哈希和相等投影。列表需同时编码长度、顺序和元素；无序映射应像 `LogicalLock` 一样先建立稳定顺序；可选值应编码存在标志，避免 `None` 与合法零值无谓碰撞。若改变共享辅助函数，要审计所有使用者。

最直接的 Rust 测试入口是同目录 `hash64_equals_generated_test.rs`；覆盖面更广的迁移测试在 `logicalop_test/hash64_equals_test.rs`，该文件属于独立测试 crate；`logical_generated_aster_unit_test.rs` 还覆盖 DataSource 与 Projection。Go 意图由 `logicalop_test/hash64_equals_test.go` 提供。性能敏感扩展需关注表达式重复生成 `CanonicalHashCode()`、深层列表遍历以及锁键排序的成本。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter .../hash64_equals_generated.rs` 命中目标；`node --file ... --offset 1 --limit 500` 读取目标 439 行及 56 个符号，并报告 3 个文件级使用者；`explore` 与 `query LogicalJoin`/`query Hash64` 用于核对符号范围。精确 `callers/callees` 查询因同名方法歧义未形成可靠边，本文没有据此虚构生产调用关系。
- 源与模块：`pkg/planner/core/operator/logicalop/hash64_equals_generated.rs`、`lib.rs`、`base_logical_plan.rs`、`logical_window.rs`。
- crate/移植元数据：`pkg/planner/core/operator/logicalop/Cargo.toml`。
- Go 对照：`pkg/planner/core/operator/logicalop/hash64_equals_generated.go`。
- Rust 测试：`pkg/planner/core/operator/logicalop/hash64_equals_generated_test.rs`、`logical_generated_aster_unit_test.rs`、`logicalop_test/hash64_equals_test.rs`；Go 测试：`logicalop_test/hash64_equals_test.go`。
- 调用边补充检查：仓库范围检索 `.Hash64(` / `.Equals(`，并阅读 `pkg/planner/cascades/memo/group_expr.rs` 的独立 memo 路线。未运行 Cargo，符合该纯文档任务要求。
