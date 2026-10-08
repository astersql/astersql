# `pkg/planner/core/operator/physicalop/physical_show.rs`

## 文件定位

本文件定义 Rust 侧的 `PhysicalShow` 物理计划节点，位于 `astersql-planner-core-operator-physicalop` crate。该 crate 由同目录 `Cargo.toml` 声明，`lib.rs` 以 `mod physical_show` 装入模块、以 `pub use physical_show::*` 对外再导出，并用 `impl_schema_leaf_operator!(PhysicalShow)` 将它接入通用物理计划接口。

它是 SHOW 类语句从逻辑描述走向物理计划和执行器之间的数据载体：保存输出 Schema、SHOW 参数以及可选的谓词抽取结果。本文件本身不读取系统表、不产生结果行，也不负责把 `LogicalShow` 转成 `PhysicalShow`。当前 Rust 仓库中，除模块接线、通用 trait 宏和独立测试外，没有检索到生产代码直接构造 `PhysicalShow`；完整的逻辑转物理和执行器构建主链仍可在 Go 对照实现中看到。因此它应被视为“已定义并接入物理计划抽象、但 Rust 生产构造链尚未在直接证据中确认”的移植状态。

## 核心职责

- `PhysicalShow`（`physical_show.rs:25`）把 `PhysicalSchemaProducer`、`logicalop::ShowContents` 和可选的 `logicalop::ShowPredicateExtractor` 聚合为一个叶子物理算子。
- `New` 与 `Init`（`:35`、`:48`）建立 `TypeShow` 类型的基础物理计划；`Init` 额外写入伪统计 `RowCount = 1.0`，使通用规划/代价路径拥有非空统计信息。该值不是 SHOW 实际返回行数。
- `Clone`（`:58`）在新的计划上下文中重建基础计划，并复制已经缓存的 Schema、SHOW 参数和抽取器，用于上下文切换场景。
- `ExplainInfo`（`:75`）把 EXPLAIN 文本生成委托给谓词抽取器；没有抽取器时返回空串。
- `MemoryUsage`（`:82`）按 Go 对照口径汇总 Schema 生产者、SHOW 参数以及 trait-object 接口槽位的估算占用。

本文件不实现 `PhysicalShowDDLJobs`、逻辑谓词抽取算法或 SHOW 执行逻辑；这些能力分别只在 Go 对照文件、Rust `logicalop/logical_show.rs` 和执行器代码中出现。

## 主要符号

### `pub struct PhysicalShow`

- `PhysicalSchemaProducer: PhysicalSchemaProducer`：内嵌基础物理计划和可选输出 Schema。字段公开是为了与同 crate 的宏和移植代码保持直接访问方式。
- `ShowContents: logicalop::ShowContents`：SHOW 的值语义参数。Rust 定义位于 `operator/logicalop/logical_show.rs:87`，包含 `ShowKind`、库名、分区、索引、资源组、作用域标志和任务 ID 等；它实现 `Clone + Default`。
- `Extractor: Option<Box<dyn logicalop::ShowPredicateExtractor>>`：可选谓词抽取器。trait 位于 `logical_show.rs:22`，通过 `CloneBox` 为 boxed trait object 提供深层克隆入口，并提供 `Extract`、`ExplainInfo`、`Field` 和 `FieldPatternLike` 查询。

### `pub fn New(ctx: ContextRef) -> Self`

以 `BasePhysicalPlan::New(ctx, plancodec::TypeShow, 0)` 构造 Schema 生产者，使用默认 `ShowContents`，并将 `Extractor` 设为 `None`。它只建立空 SHOW 节点，不设置伪统计，也不设置结果 Schema；若调用方需要完成初始化语义，应继续调用 `Init` 并显式写入 Schema。

### `pub fn Init(mut self, ctx: ContextRef) -> Self`

消费并返回 `self`。它重新创建 `BasePhysicalPlan`，把计划类型固定为 `TypeShow`、查询块偏移固定为 `0`，随后通过 `base::PhysicalPlan::set_stats` 写入行数为 `1.0` 的默认 `StatsInfo`。因为它会替换基础计划，调用方应在 `Init` 之后再设置 Schema 或其他基础计划状态。

### `pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error>`

先调用 `BasePhysicalPlan::CloneWithNewCtx`；该步骤失败时用 `?` 原样向上传播 `expression::Error`。若源 `PhysicalSchemaProducer::SchemaRef()` 已有缓存，则克隆 Schema 并写入新 producer；若源 Schema 尚未初始化，则目标也保持未缓存状态。最后克隆 `ShowContents` 与 boxed `Extractor`。

### `pub fn ExplainInfo(&self) -> String`

对 `Extractor` 使用 `map_or_else`：`None` 产生新空串，`Some` 调用动态分派的 `ShowPredicateExtractor::ExplainInfo`。该结果也被 `lib.rs:1666-1715` 的叶子算子宏同时用作普通和 normalized EXPLAIN 的 operator 信息。

### `pub fn MemoryUsage(&self) -> i64`

返回 `PhysicalSchemaProducer::MemoryUsage()`、`ShowContents::MemoryUsage()` 与 `size_of::<[usize; 2]>()` 之和。最后一项用两个机器字模拟 Go `interface{}`/Rust fat pointer 的接口槽位；它不递归统计抽取器实现内部堆分配。独立测试 `physical_show_test.rs:51` 锁定了这一计算式。

## 执行流程

1. 上游应先取得 `ContextRef`，用 `PhysicalShow::New` 创建默认节点，或组装 `ShowContents`/`Extractor` 后调用 `Init`。`Init` 把节点标记为 `plancodec::TypeShow` 并安装一行伪统计。
2. 上游为节点设置 SHOW 的业务参数和输出 Schema。Rust `PhysicalSchemaProducer::SetSchema` 会把 Schema 放进 `Arc`；若从未设置，叶子节点第一次走可变 `Schema()` 时会因没有孩子而惰性生成空 Schema。
3. `lib.rs` 中的 `impl_schema_leaf_operator!(PhysicalShow)` 把节点接到 `ConcretePhysicalOperator` 和通用 `PhysicalPlan`：Schema 访问委托给 producer，列下标解析委托给 `ResolveIndices`，代价委托给 `BasePhysicalPlan`，EXPLAIN 与内存统计分别回调本文件方法。
4. 需要换计划上下文时，`Clone` 重建基础计划；只有源节点已经缓存 Schema 时才复制 Schema，同时复制值参数并通过 `CloneBox` 克隆抽取器。
5. 下游展示计划时调用 `ExplainInfo`；真正执行 SHOW、访问元数据并生成行不在本文件中。

Go 主链用于说明设计意图而非证明 Rust 已接线：`findBestTask4LogicalShow`（`physical_show.go:91`）或 Cascades `ImplShow.OnImplement`（`implementation_rules.go:246`）把 `LogicalShow` 的内容、抽取器和 Schema 搬到 `PhysicalShow`，`pkg/executor/builder.go` 的 `buildShow` 再构建执行器。

## 数据与状态

- 上下文和通用计划状态保存在 `PhysicalSchemaProducer.BasePhysicalPlan` 中。`Init` 会整体替换这个基础计划，因此此前附着于旧基础计划的 ID、统计、孩子或代价缓存不能假定保留。
- Schema 是 `PhysicalSchemaProducer` 内部的 `Option<Arc<Schema>>`。本文件的 `Clone` 并非共享该 `Arc`：它读取 `&Schema` 后调用 `Schema::Clone`，再由 `SetSchema` 包装为新的 `Arc`。未初始化状态则保持未初始化。
- `ShowContents` 是拥有型值，字符串和标志随节点保存；其 `MemoryUsage` 当前只额外累计 DB、分区和索引相关字符串长度，未覆盖结构中的每个 `String` 字段，属于现有估算口径而非精确堆快照。
- `Extractor` 是可选 boxed trait object。节点拥有该对象，克隆通过对象安全的 `CloneBox` 完成；EXPLAIN 只读访问它。
- `RowCount = 1.0` 是避免规划路径缺少统计的伪值，不是执行结果基数承诺，也不应据此截断 SHOW 输出。

## 依赖与调用关系

直接依赖如下：

- `base::ContextRef` 提供计划上下文；`base::PhysicalPlan::set_stats` 写统计。
- 本 crate 的 `BasePhysicalPlan`、`PhysicalSchemaProducer` 提供通用物理计划、Schema、克隆与内存统计。
- `logicalop::ShowContents` 和 `logicalop::ShowPredicateExtractor` 承载从逻辑 SHOW 继承的参数与过滤信息；`physicalop/Cargo.toml` 以本地路径依赖 `astersql-planner-core-operator-logicalop`。
- `plancodec::TypeShow` 标识节点类型；`property::StatsInfo` 承载伪统计；`expression::Error` 是克隆失败通道。这些 crate 均由同目录 `Cargo.toml` 显式声明。

RustCodeGraph 将文件识别为 9 个符号，并确认 `ExplainInfo -> logicalop::ShowPredicateExtractor::ExplainInfo` 的下游调用边；`Clone` 的关键失败边落到 `BasePhysicalPlan::CloneWithNewCtx`，`MemoryUsage` 汇总 producer 与 contents。由于常见方法名在图查询中存在大量同名歧义，调用者结果再用限定路径文本检索复核：Rust 侧只发现 `lib.rs` 的模块/trait 宏接线以及 `physical_show_test.rs` 的直接 `New`/`MemoryUsage` 调用，没有发现生产构造调用。

Go 对照中的上游包括 `findBestTask4LogicalShow` 和 `ImplShow.OnImplement`；下游包括 executor builder 的 `buildShow`，字符串化路径 `pkg/planner/core/stringer.go:186` 也读取抽取器的 EXPLAIN 信息。这些是目标架构的直接证据，但不能替代 Rust 生产接线证据。

## 错误处理与边界

- `New`、`Init`、`ExplainInfo` 和 `MemoryUsage` 不返回错误。它们假设所依赖对象的方法遵守各自契约。
- `Clone` 是唯一显式可失败入口；`CloneWithNewCtx` 的 `expression::Error` 不包装、不吞掉，直接返回调用方。Schema、`ShowContents` 和抽取器克隆本身没有 `Result` 通道。
- 无抽取器是正常状态：`ExplainInfo` 返回空串，不报错。空串既可能表示 `Extractor == None`，也可能来自一个返回空信息的抽取器，调用者不能据此反推是否存在抽取器。
- 未设置 Schema 时，`Clone` 不会强制求值 Schema；对叶子 `PhysicalShow` 后续惰性求值会得到空 Schema。上游若要执行或展示真实结果列，必须在合适的构造阶段显式设置 Schema。
- 本文件不验证 `ShowContents` 字段组合，也不调用 `Extractor::Extract`；语法、兼容性和抽取是否有效由更早的逻辑规划阶段负责。
- `MemoryUsage` 是兼容性估算：不包含抽取器内部堆数据，也不保证等于分配器实际字节数。修改字段或 trait-object 表示时需同步评估该公式。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或 I/O。`PhysicalShow` 由所有权管理：`ShowContents` 与 `Extractor` 随节点释放，Schema 由 producer 内的 `Arc` 管理，计划上下文的共享策略由 `ContextRef` 的定义负责。

公开方法除 `Init` 消费并修改节点、`Clone` 创建新节点外均只借用 `&self`；代码本身不提供内部可变性或并发协调。是否可跨线程共享取决于 `ContextRef`、`BasePhysicalPlan` 和 `dyn ShowPredicateExtractor` 的 trait bound，而本文件没有声明 `Send`/`Sync` 保证，因此不得仅凭该结构推断可安全跨线程传递。

`Clone` 的生命周期边界尤其重要：新节点绑定 `new_ctx`，Schema 和 SHOW 参数形成独立值，抽取器由其实现决定如何深拷贝；旧节点仍保持有效，两者不应依赖共享的可变抽取状态。

## 与 Go 版本的对应关系

Rust `PhysicalShow` 三个字段对应 Go `physical_show.go:26-32` 的嵌入 `PhysicalSchemaProducer`、嵌入 `logicalop.ShowContents` 和 `Extractor base.ShowPredicateExtractor`。主要一致点如下：

- `Init` 都创建 `TypeShow` 基础计划并设置 `RowCount = 1` 的伪统计。
- `MemoryUsage` 都累计 Schema producer、ShowContents 和一个接口槽位；Rust 以两个 `usize` 明确模拟 Go 的 `size.SizeOfInterface`，独立 Rust 测试专门验证这一对齐。
- 谓词抽取器都服务于 SHOW 过滤和 EXPLAIN。Go 接口及基础实现可见 `base/misc_base.go:37-51` 与 `show_predicate_extractor.go:42-104`。

需要明确的差异和迁移状态：

- Go 文件还定义 `PhysicalShowDDLJobs`、两个 `findBestTask4LogicalShow*` 转换函数；Rust 本文件没有这些符号。
- Rust 增加了显式 `New`、可失败的跨上下文 `Clone` 和本地 `ExplainInfo`；Go 对照文件中的 `PhysicalShow` 没有对应的显式克隆方法，字符串化逻辑直接读取 extractor。
- Go 的 `Init` 返回指针，Rust `Init` 消费并返回值；Rust 调用顺序需遵守所有权与“Init 后设置 Schema”的约束。
- Go 的生产规划和 executor builder 已有明确调用链；Rust 限定路径检索未发现等价的生产构造调用。因此不能宣称 Rust 已经端到端执行 SHOW，只能确认节点定义和通用物理计划接口已接好。
- Rust `logicalop::ShowStatsMetaPredicateExtractor` 当前多个 trait 方法仍返回 `false`/空值（`logical_show.rs:49-64`）；这进一步说明不能从类型存在推断所有 Go 抽取语义已移植。

## 扩展指南

- 新增 SHOW 参数时，先在 `logicalop::ShowContents` 增加字段并同步其 `Default`、`Clone` 和 `MemoryUsage` 口径，再检查 `PhysicalShow::Clone` 是否仍能完整复制。Go 语义应与 `logical_show.go`/`physical_show.go` 对照。
- 新增或改变谓词抽取器时，实现 `ShowPredicateExtractor::CloneBox`、`Extract`、`ExplainInfo`、`Field` 和 `FieldPatternLike`；同时确认 boxed 对象的克隆语义和内部堆内存是否应计入 `PhysicalShow::MemoryUsage`。
- 改变初始化流程时，重点审查 `New` 与 `Init` 的重复构造关系、伪统计不变量以及 Schema 设置顺序。不要把伪行数改成执行结果限制。
- 将 Rust 接入完整规划链时，最可能新增的位置是逻辑 SHOW 到物理 SHOW 的实现规则/最佳任务选择，以及 executor builder 对 `PhysicalShow` 的分派。接线应复制 `ShowContents`、`Extractor` 和逻辑 Schema，并保持无排序要求、root task 等 Go 约束；这些改动不应塞入本文件来伪造端到端支持。
- 修改 EXPLAIN 时，同时检查 `impl_schema_leaf_operator!` 把同一文本用于普通和 normalized 输出的行为；如两者需要不同规范化结果，应调整通用接口而不是只改字符串内容。
- 测试必须保持独立文件。扩展本节点应同步 `pkg/planner/core/operator/physicalop/physical_show_test.rs`；若新增规划或执行接线，还应在对应独立 planner/executor 测试文件覆盖逻辑转物理、Schema、错误传播和用户可见结果，不要把 `#[cfg(test)]` 测试内嵌到 `physical_show.rs`。
- 兼容风险主要是 Go/Rust 字段遗漏、EXPLAIN 文本变化和内存统计口径漂移；正确性风险是未设置 Schema、克隆时丢字段或抽取器；性能风险集中在不必要的深克隆和新增堆分配。当前节点自身无 I/O 或并发热点。

## 验证依据

- 目标源码：`pkg/planner/core/operator/physicalop/physical_show.rs`，核对 `PhysicalShow` 及 `New`、`Init`、`Clone`、`ExplainInfo`、`MemoryUsage` 全部实现；文件无常量、enum、条件编译或其他本地 trait 定义。
- crate 与模块：`pkg/planner/core/operator/physicalop/Cargo.toml`；`pkg/planner/core/operator/physicalop/lib.rs:99,212,1666-1725`，核对模块装入、公开再导出和叶子物理算子宏接线。
- Rust 直接依赖：`physical_schema_producer.rs:28-94`；`operator/logicalop/logical_show.rs:21-129`，核对 Schema 生命周期、抽取器 clone 契约、SHOW 参数和内存口径。
- Rust 测试：`pkg/planner/core/operator/physicalop/physical_show_test.rs:51-60`，验证 `MemoryUsage` 包含两个机器字的 extractor 接口槽位；未发现本文件其他独立 Rust 测试。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_show.go:25-105`，核对字段、初始化、内存统计、`PhysicalShowDDLJobs` 和传统最佳任务转换；`pkg/planner/cascades/old/implementation_rules.go:235-259` 核对 Cascades 转换；`pkg/planner/core/stringer.go:181-190` 核对字符串化；`pkg/planner/core/base/misc_base.go:37-51` 与 `pkg/planner/core/show_predicate_extractor.go:42-104` 核对抽取器契约和边界。
- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点、1,848,419 边；`files --filter .../physical_show.rs` 命中目标并报告 9 个符号；`node physical_show.rs::PhysicalShow` 核对结构体；`explore`/`callers`/`callees` 核对 extractor 的 `ExplainInfo` 下游边以及构造调用缺失。因 `New`、`Init`、`Clone`、`MemoryUsage` 同名符号很多，另用限定目录的 `rg` 复核调用者，结论仅限当前工作树。
- 未运行 Cargo 或代码测试：本任务仅新增分析文档，计划明确禁止 Cargo。结构验证应以任务给定的 11 个固定二级标题命令为准。
