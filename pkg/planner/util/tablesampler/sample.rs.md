# `pkg/planner/util/tablesampler/sample.rs`

## 文件定位

[源文件 `sample.rs`](./sample.rs) 是 `astersql-planner-util-tablesampler` crate 的核心实现，定义物理表采样计划所携带的 `TableSampleInfo` 及其构造器。同目录 [`Cargo.toml`](./Cargo.toml) 将 crate 根设为 `lib.rs`；[`lib.rs`](./lib.rs) 把 `sample` 保持为私有模块，再用 `pub use sample::*` 对外公开本文件的类型和函数。

该文件是“采样元数据容器”，不执行扫描或抽样算法。Rust 物理算子 `pkg/planner/core/operator/physicalop/physical_table_sample.rs::PhysicalTableSample` 通过 `Option<Arc<tablesampler::TableSampleInfo>>` 持有它。目前精确引用搜索显示，Rust `NewTableSampleInfo` 的直接调用只出现在独立单元测试 `sample_test.rs`；因此不能将 Go 已完整接线的规划链路表述为 Rust 构造器已被生产主链调用。

## 核心职责

- `TableSampleInfo` 把解析器产生的 `TABLESAMPLE` AST、采样前的完整列 schema，以及需要参与的分区表集合成组保存。
- `NewTableSampleInfo` 将“没有采样 AST”解释为“不存在采样信息”，并在成功构造时克隆 AST 与 schema、接收分区向量的所有权。
- `MemoryUsage` 保持与 Go `sample.go::(*TableSampleInfo).MemoryUsage` 相同的计费项：两个可选指针槽位、切片头、按容量计算的分区接口槽位、可选 AST 和可选 schema。

本文件不校验 SQL 采样语法，不决定物理访问路径，也不负责从分区或 KV 层读取样本行。

## 主要符号

- `pub struct TableSampleInfo`
  - `AstNode: Option<TableSample>`：拥有一份 `parser_ast::TableSample` 克隆；`None` 在类型上可表示缺少 AST，虽然公开构造器成功时总是写入 `Some`。
  - `FullSchema: Option<Schema>`：拥有完整 schema 的克隆；构造器成功时写入 `Some(fullSchema.Clone())`。
  - `Partitions: Vec<Arc<dyn PartitionedTable>>`：拥有向量，向量元素通过 `Arc` 共享分区表 trait object。
- `TableSampleInfo::MemoryUsage(&self) -> i64`：返回近似内存账目。它先加固定槽位和 `Partitions.capacity()` 的计费，再根据 `AstNode` / `FullSchema` 是否存在条件加算。
- `NewTableSampleInfo(node: Option<&TableSample>, fullSchema: &Schema, partitions: Vec<Arc<dyn PartitionedTable>>) -> Option<TableSampleInfo>`：公开构造函数。`node?` 使 `None` 立即返回；成功路径深克隆 AST 和 schema，并移入 `partitions`。

文件内没有模块级常量、trait、异步函数或条件编译项。测试的 `#[cfg(test)]` 装配位于 `lib.rs`，测试逻辑位于独立的 `sample_test.rs`。

## 执行流程

1. 调用方将可选的 `TableSample` 引用、完整 `Schema` 引用和分区表向量传入 `NewTableSampleInfo`。
2. `let node = node?` 检查 AST；若为 `None`，函数在读取 schema 或移入分区之前返回 `None`。这与 Go 对 `node == nil` 的早返回对齐。
3. 若 AST 存在，函数调用 `node.clone()` 和 `fullSchema.Clone()`，然后将已传值进入的 `partitions` 直接存入新结构。
4. 上层物理采样算子可以用 `Arc<TableSampleInfo>` 共享该载荷。`PhysicalTableSample::MemoryUsage` 在载荷存在时向下调用本类型的 `MemoryUsage`。
5. `MemoryUsage` 不修改状态：它按 `size` crate 的 Go 兼容常量累加，对 AST 使用 Rust `size_of::<TableSample>()`，对 schema 委托 `Schema::MemoryUsage`。

Go 完整路径的直接证据是 `pkg/planner/core/logical_plan_builder.go`：它把 `NewTableSampleInfo` 结果写入 `DataSource.SampleInfo`；`pkg/planner/core/find_best_task.go::convertToSampleTable` 再将其传入 `physicalop.PhysicalTableSample`。Rust 侧已有持有该类型的物理算子，但本任务未找到 Rust 生产代码对 `NewTableSampleInfo` 的直接调用。

## 数据与状态

`TableSampleInfo` 是一个无内部可变性的值对象；字段虽为 `pub`，但本文件自身不提供后续更新方法。两个 `Option` 使调用者可以用结构体字面量创建部分缺失的值；`MemoryUsage` 因此必须对缺失的 AST/schema 安全返回不包含它们的账目。`sample_test.rs::memory_usage_omits_absent_optional_fields` 直接覆盖此边界。

构造器的所有权边界很明确：AST 由借用值克隆为自有值，schema 通过 TiDB 风格的 `Clone()` 得到独立副本，分区 `Vec` 被移入而不再复制。`sample_test.rs::new_table_sample_info_owns_an_independent_schema_clone` 证明构造后更改原 schema 不会改变已保存 schema。

`MemoryUsage` 是与 Go 计费规则对齐的估算，不是 Rust 对象递归、去重后的实际堆占用。分区部分使用 `capacity`而不是 `len`；它只按每个容量槽位的 `SizeOfInterface` 计费，不递归进入共享的分区表对象。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](./Cargo.toml) 明确声明，未定义 feature gate：

- `astersql-expression`（代码别名 `expression`）提供 `Schema` 及 `Schema::Clone` / `Schema::MemoryUsage`。
- `astersql-parser-ast`（`parser_ast`）提供 `TableSample`。
- `astersql-table`（`table`）提供 `PartitionedTable` trait。
- `astersql-util-size`（`size`）提供 `SizeOfPointer`、`SizeOfSlice` 和 `SizeOfInterface` 等 Go 兼容计费常量。

直接上游与使用者：

- `lib.rs` 再导出本文件的公开符号。
- `pkg/planner/core/operator/physicalop/Cargo.toml` 以 `tablesampler` 别名依赖本 crate；其 `physical_table_sample.rs::PhysicalTableSample` 保存 `Arc<TableSampleInfo>`，克隆时共享它，内存统计时调用 `TableSampleInfo::MemoryUsage`。
- `sample_test.rs` 是 `NewTableSampleInfo` 当前已确认的 Rust 直接调用者；`physical_table_sample_test.rs` 则直接创建 `TableSampleInfo` 字面量，验证上层算子的计费委托。

工作区根 `Cargo.toml` 以 `facade_planner_util_tablesampler` 别名登记此 crate。Go `BUILD.bazel` 只列出 `sample.go` 和对应 Go 依赖，不是 Rust 构建入口。

## 错误处理与边界

- `NewTableSampleInfo` 没有 `Result` 错误通道；唯一的失败/缺席分支是 `node == None`，并以 `None` 表示。
- 构造器要求 `fullSchema: &Schema` 始终有效，即使 `node` 是 `None`。不过函数体的早返回确保该分支不会克隆 schema。Rust 没有 Go 可传入 nil schema 指针的对应状态。
- `Partitions` 允许空向量；本文件不校验分区数、分区与 schema/AST 的一致性，也不去重。这些前置条件必须由规划主链保证。
- 由于字段公开，调用者可绕过构造器创建 `AstNode: None` 或 `FullSchema: None` 的结构；`MemoryUsage` 对此安全处理，但这不证明这种不完整载荷可被执行层使用。
- AST 计费用 `std::mem::size_of::<TableSample>()`，只覆盖该值的内联布局；如果 `TableSample` 以后引入间接堆数据，当前算法不会自动递归计费。
- 所有加法使用 `i64` 且没有显式溢出处理；一般容量下不构成实用风险，但这不是一个带上限检查的计费 API。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或 I/O 资源。`MemoryUsage` 只读取当前值。

AST 和 schema 的克隆使 `TableSampleInfo` 不借用构造输入，因此可超过输入引用的生命周期。`Partitions` 的 `Vec` 归 `TableSampleInfo` 所有；其元素的 `Arc` 仅管理引用计数，分区对象在最后一个强引用释放后销毁。

不应仅根据使用 `Arc` 就推断整个结构可跨线程共享；那还取决于 `PartitionedTable` trait object 及其他字段的 `Send` / `Sync` 实际约束，本文件没有显式声明或使用这些约束。

## 与 Go 版本的对应关系

直接对照文件是 [`sample.go`](./sample.go)。

- 字段语义对齐：Go `*ast.TableSample`、`*expression.Schema`、`[]table.PartitionedTable` 分别对应 Rust `Option<TableSample>`、`Option<Schema>`、`Vec<Arc<dyn PartitionedTable>>`。
- 缺少 AST 时，Go 返回 nil 指针，Rust 返回 `None`。
- 存在 AST 时，两边都克隆 schema，但 AST 所有权有差异：Go 直接保存传入指针，Rust 从借用引用克隆为自有 `TableSample`。因此 Rust 的 AST 不会因调用方后续更改原值而改变。
- Go 直接保存分区 slice；Rust 移入 `Vec`，而每个 trait object 以 `Arc` 共享。两边的内存计费都按容量而非长度计算分区槽位。
- Go `MemoryUsage` 支持 nil receiver 并返回零；Rust `&self` 无对应的 nil receiver，可选性由上层 `Option<TableSampleInfo>` 承担。
- Go 对非 nil AST 使用 `unsafe.Sizeof(ast.TableSample{})`，Rust 使用 `size_of::<TableSample>()`；其余固定计费项和 schema 委托逻辑一致。

Go 目录中没有 `sample_test.go`。相关 Go 规划测试 `pkg/planner/core/preprocess_test.go` 覆盖不合法 `TABLESAMPLE` 语法，但不直接测试本容器的构造或计费。本文件的直接契约覆盖来自 Rust `sample_test.rs`。

## 扩展指南

- 新增采样元数据字段时，需同时更新 `TableSampleInfo`、`NewTableSampleInfo` 和 `MemoryUsage`，明确新字段应深克隆、移入还是通过 `Arc` 共享。若要继续对齐 Go，还要同步检查 `sample.go` 的字段与计费项。
- 改变“无 AST 即无采样信息”契约时，修改 `NewTableSampleInfo` 并同步更新 `sample_test.rs::new_table_sample_info_requires_an_ast_node`；这会影响上层对 `Option` 的分支判断。
- 改变克隆策略时，保留 `sample_test.rs::new_table_sample_info_owns_an_independent_schema_clone` 这类所有权回归。如果 AST 也变为共享或可变对象，必须明确评估计划克隆的隔离性。
- 调整内存计费时，独立测试至少要继续覆盖：可选字段缺席、AST/schema 存在、以 `capacity` 而非 `len` 计分区槽位。直接测试应继续放在 `sample_test.rs`，不要内嵌到生产文件。
- 要完成 Rust 主链接线，不能只修改本容器；需从 Rust 逻辑数据源到 `PhysicalTableSample` 核对 Go `logical_plan_builder.go` / `find_best_task.go` 的传递语义，并增加独立的规划级回归测试。这属于后续接线任务，不是本文件说明任务的修改范围。

主要风险是所有权语义与 Go 指针语义偏离、新字段漏计内存，以及将元数据容器的存在误解为 Rust 端到端采样已接通。改动时应优先保持 Go 已有契约，再用 Rust 独立测试固化必要差异。

## 验证依据

- RustCodeGraph 索引：`status` 报告 11,467 个已索引文件；`files --filter pkg/planner/util/tablesampler` 列出 `lib.rs`、`sample.rs`、`sample.go`、`sample_test.rs`。
- RustCodeGraph 符号：`query TableSampleInfo --kind struct --json` 命中 Go/Rust 对照结构；`query NewTableSampleInfo --kind function --json` 命中 Go/Rust 构造器；`node pkg/planner/util/tablesampler/sample.rs::TableSampleInfo` 和 `::NewTableSampleInfo` 核对了字段、构造流程与测试调用边。`explore` 还识别出 `PhysicalTableSample::MemoryUsage` 对本类型的下游委托。
- Rust 源码与装配：`pkg/planner/util/tablesampler/sample.rs`、`Cargo.toml`、`lib.rs`；该目录下不存在 `doc.go`。
- 直接 Rust 测试：`pkg/planner/util/tablesampler/sample_test.rs`，覆盖缺少 AST、AST/schema 克隆、schema 副本独立性、可选字段缺席和分区容量计费。
- 上层 Rust 证据：`pkg/planner/core/operator/physicalop/physical_table_sample.rs` 及其独立测试 `physical_table_sample_test.rs::init_and_memory_usage_match_go_contract`。
- Go 对照与主链：`pkg/planner/util/tablesampler/sample.go`、`pkg/planner/core/logical_plan_builder.go`、`pkg/planner/core/find_best_task.go`、`pkg/planner/core/operator/physicalop/physical_table_sample.go`。相关语法边界见 `pkg/planner/core/preprocess_test.go`；同目录无 Go 直接单元测试。
- 精确引用搜索：在 `pkg/planner` 的 Rust/Go 文件中查找 `NewTableSampleInfo|TableSampleInfo|PhysicalTableSample`，用于补足 RustCodeGraph 未返回的 Go 调用者，并确认 Rust 构造器未出现在生产调用中。
- 未运行 Cargo 或代码测试：本任务仅新增说明文档，且任务计划明确禁止运行 Cargo。交付验证使用任务指定的 11 章节结构检查，并人工核对文档未将 Go 主链行为冒充为 Rust 已接线事实。
