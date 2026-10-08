# `pkg/planner/core/operator/physicalop/physical_shuffle.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-core-operator-physicalop`，其 crate 根为同目录的 [`lib.rs`](lib.rs)。`lib.rs` 以 `pub mod physical_shuffle` 公开本模块，并在 `#[cfg(test)]` 下挂接独立测试文件 [`physical_shuffle_test.rs`](physical_shuffle_test.rs)。包清单 [`Cargo.toml`](Cargo.toml) 通过 `[package.metadata.porting]` 把本 crate 对应到 Go 包 `pkg/planner/core/operator/physicalop`。

需要特别区分两套当前并存的 Rust 表示：本文件定义基于 `physical_common_plans::{PhysicalPlanNode, PhysicalExpr, Stats, PhysicalProperty}` 的轻量值模型；[`physical_window.rs`](physical_window.rs) 另有接入 `base::PhysicalPlan` trait 的 `PhysicalShuffle` 与 `PhysicalShuffleReceiverStub`，并由 [`lib.rs`](lib.rs) 中的 `ConcretePhysicalOperator` 实现接入规划主链。精确引用搜索显示，本文件的两个轻量结构目前只被本模块方法和独立测试直接使用；主链只复用这里的 `PartitionSplitterType`。因此本文不会把 Go 执行器的完整运行接线归属于这套轻量结构。

源文件保留了一段注释化的早期 Go 直译草案；实际可执行定义从 `use crate::physical_common_plans` 开始。草案不是编译单元，也不是“已支持”证据。

## 核心职责

本文件有三项可执行职责：

1. `PartitionSplitterType` 表示 Shuffle 的分区策略，包含默认的 `Hash` 和 `Range`。
2. `PhysicalShuffle` 保存并发度、worker 尾计划、数据源、分区策略、逐数据源分区表达式及输出 schema；它能估算部分内存、生成 EXPLAIN 文本、校验表达式列引用，并折叠为一个通用 `PhysicalPlanNode`。
3. `PhysicalShuffleReceiverStub` 保存 receiver 下标、schema 和可选数据源；它能估算内存并生成一个无孩子的 `ShuffleReceiver` 通用节点。

Go 中 Shuffle 的目的，是把 `Shuffle -> Window/StreamAgg/MergeJoin -> Sort -> DataSource` 拆成主线程边界、worker 尾和取数线程数据源。该意图见 [`physical_shuffle.go`](physical_shuffle.go) 的类型注释；本文件保留相同的数据分组概念，但不创建线程、通道、splitter 或 executor。

## 主要符号

- `PartitionSplitterType::{Hash, Range}`：可复制的策略枚举；`#[default]` 选择 `Hash`。枚举本身只携带策略标签，不实现哈希或范围算法。
- `PhysicalShuffle`：公开字段值对象。`concurrency` 是 worker 数；`tails` 是各 worker 内的末端计划；`data_sources` 是 splitter 的输入计划；`by_item_arrays[i]` 应相对 `data_sources[i].schema` 解析；`schema` 是输出列 UniqueID。
- `PhysicalShuffle::memory_usage(&self) -> i64`：以 `size_of::<Self>()` 为基数，递归加入 `tails`、`data_sources` 中节点的 `PhysicalPlanNode::memory_usage`，并加入每个分区表达式向量按 capacity 计算的元素存储。
- `PhysicalShuffle::explain_info(&self) -> String`：按 `data_sources` 顺序读取 `id`，以空格连接，生成 `execution info: concurrency:N, data sources:[...]`。
- `PhysicalShuffle::resolve_indices(&mut self) -> Result<(), String>`：拒绝分区表达式组多于数据源的情况，然后逐组与逐数据源 `zip`，递归调用 `PhysicalExpr::resolve_indices`。
- `PhysicalShuffle::into_plan(self, stats, property) -> Result<PhysicalPlanNode, String>`：要求并发度非零；先放入所有数据源，再追加所有 tails；新节点种类为 `PhysicalKind::Shuffle`，只保存一个 required property。
- `PhysicalShuffleReceiverStub`：`receiver_index` 在此模型中兼作转换后节点 ID；`data_source` 用于所有权和内存统计，不会成为转换后节点的 child。
- `PhysicalShuffleReceiverStub::{memory_usage, into_plan}`：前者包括自身、schema capacity 及可选数据源子树；后者生成 `PhysicalKind::ShuffleReceiver` 的叶节点。

文件内没有 trait、模块级常量、自由函数或条件编译项；两个结构及其方法均为公开 API，方法内部没有额外私有辅助函数。

## 执行流程

`PhysicalShuffle` 的典型轻量流程如下：

1. 调用方构造数据源 `PhysicalPlanNode`、worker tails、分区表达式和输出 schema，并选择 `Hash` 或 `Range`。
2. 在折叠计划前调用 `resolve_indices`。若 `by_item_arrays.len() > data_sources.len()`，函数立即返回 `shuffle by-item arrays exceed data sources`；否则第 `i` 组表达式只用第 `i` 个数据源 schema 校验。`PhysicalExpr` 会递归检查 `Column`、`CorrelatedColumn` 和 `Scalar.args`，常量与 `Default` 不需要列解析。
3. `explain_info` 可在仍保留独立数据源列表时输出并发度及数据源 ID；空数据源产生空方括号。
4. `into_plan` 消耗结构体。并发度为零时不构造节点；成功时孩子顺序固定为 `data_sources` 后接 `tails`，节点 ID 为所有孩子最大 ID 加一（无孩子时从 1 开始），并透传调用者提供的 `Stats`、输出 `schema` 和单个 `PhysicalProperty`。
5. receiver 侧的 `into_plan` 同样消耗 stub，但生成无孩子、无 required property 的 `ShuffleReceiver` 节点；其 `stats` 来自调用者，ID 来自 `receiver_index` 的整数转换。

Go 的真实运行链更长：[`pkg/planner/core/plan.go`](../../plan.go) 的 `optimizeByShuffle` 针对 Window、StreamAgg、MergeJoin 选择插入 `PhysicalShuffle`；[`pkg/executor/builder.go`](../../../../executor/builder.go) 的 `buildShuffle` 创建 splitter、数据源 executor、每个 worker 的 receiver，并临时把 receiver stub 接到 tails。那些步骤是 Go 对照证据，不是本文件函数的隐式行为。

## 数据与状态

所有状态都由普通拥有型字段承载，没有全局变量或内部缓存。两个结构派生 `Clone`、`Debug`、`PartialEq`；`PhysicalShuffle` 还派生 `Default`，因此默认并发度是 `0`，它只是可构造的中间状态，调用 `into_plan` 时会被拒绝。`PartitionSplitterType` 默认是 `Hash`。

关键对应不变量是 `by_item_arrays[i]` 与 `data_sources[i]` 配对。当前实现只检查“表达式组不能更多”，没有要求两者数量完全相等：表达式组更少时，尾部数据源不会参与索引解析。这比 [`physical_window.rs`](physical_window.rs) 的主链实现宽松，后者要求两者长度相等。扩展者不能假定本文件已经强制一一等长。

`into_plan` 会丢弃 `splitter_type`、`by_item_arrays` 和 `concurrency` 的显式表示，因为通用 `PhysicalPlanNode` 只有 `kind/schema/children/stats/required_properties` 字段。这意味着转换结果只表达“这是 Shuffle 及其孩子”，不能单独恢复具体切分配置。receiver 转换也不会保留 `data_source` 或 receiver 通道对象。

内存统计是估算而非 allocator 精确账单：`PhysicalPlanNode::memory_usage` 统计节点本体、schema capacity 和孩子递归值；Shuffle 另外统计每个分区表达式 Vec 的 capacity。它没有递归统计 `PhysicalExpr::Scalar` 参数或字符串堆内存，也没有单独加入 `tails`/`data_sources` 外层 Vec 的预留 capacity；与 Go `MemoryUsage` 的完整接口对象逐项口径并不完全等价。

## 依赖与调用关系

本文件唯一的编译期导入来自同 crate 的 [`physical_common_plans.rs`](physical_common_plans.rs)：

- `PhysicalExpr::resolve_indices` 提供列 ID 对 schema 的递归校验及错误字符串。
- `PhysicalPlanNode::memory_usage` 提供计划子树内存递归。
- `PhysicalKind::{Shuffle, ShuffleReceiver}` 标记转换后的节点类型。
- `Stats` 和 `PhysicalProperty` 由调用者传入并直接写入通用节点。

crate 边界由 [`Cargo.toml`](Cargo.toml) 确认；本文件没有直接使用该清单中的外部 crate。上游精确引用包括 [`physical_shuffle_test.rs`](physical_shuffle_test.rs) 对轻量结构的直接构造，以及 [`physical_window.rs`](physical_window.rs)、[`pkg/planner/core/optimizer_runtime.rs`](../../optimizer_runtime.rs) 和若干 Rust 测试对 `PartitionSplitterType` 的复用。

RustCodeGraph 对 `PhysicalShuffle` 的查询同时返回本文件、`physical_window.rs` 和 Go 文件的同名类型；对本文件方法的 blast-radius 结果主要是自调用/独立测试，没有显示规划主链调用。相反，`physical_window.rs` 中的同名结构由 `lib.rs` 实现 `ConcretePhysicalOperator`，并被 `optimizer_runtime.rs` 构造。这个分叉是理解调用关系时必须保留的限制。

## 错误处理与边界

- `resolve_indices` 使用 `Result<(), String>`。表达式组多于数据源时返回固定错误；列或相关列不在对应 schema 时，下游返回 `column {id} is absent from child schema`；标量函数参数遇到首个错误即停止。
- 表达式组少于数据源不会报错，这是当前代码事实，不应文档化为严格一一校验。
- `into_plan` 只显式校验 `concurrency != 0`，错误为 `shuffle concurrency must be positive`。它不校验 tails 与数据源数量、分区键数量、Range 输入有序性或 schema 一致性。
- 节点 ID 使用“最大孩子 ID + 1”；此文件没有冲突检测或溢出处理。调用者需要确保输入 ID 范围合理。
- `PhysicalShuffleReceiverStub::into_plan` 对 `receiver_index as i64` 不做范围语义校验，也不验证可选数据源与 receiver schema 一致。
- 方法没有 panic 分支、I/O 或外部错误类型；但容量乘法和整数转换仍服从 Rust 的平台整数与构建模式规则，不能当作持久化格式。

## 并发与资源生命周期

尽管名称与字段描述并行执行，本文件不包含线程、锁、原子、异步任务、通道或 `unsafe`。`concurrency` 只是配置值；`PartitionSplitterType` 只是标签；receiver stub 也只有索引，没有实际 receiver 指针。

资源生命周期由 Rust 所有权决定。`memory_usage`、`explain_info` 借用不可变状态；`resolve_indices` 独占借用并原位遍历表达式；两个 `into_plan` 消耗 `self`，将 schema、孩子等拥有值移动进新节点，随后未被移动或保留的配置正常释放。`data_source: Option<Box<PhysicalPlanNode>>` 在 receiver stub 转换时不会移动进结果节点，因此随被消费的 stub 一起释放。

Go 的 `PhysicalShuffleReceiverStub` 含 `unsafe.Pointer Receiver`，执行器构建时将其指向 worker receiver；本文件没有等价的指针与生命周期协议。Rust 主链版本也把 DataSource 放在 `Children()` 之外，并由执行/遍历层特殊处理，但该行为位于 `physical_window.rs`、`lib.rs` 和 executor 代码，不由本轻量 stub 实现。

## 与 Go 版本的对应关系

直接对照文件是 [`physical_shuffle.go`](physical_shuffle.go)：

- `PartitionHashSplitterType` / `PartitionRangeSplitterType` 对应 Rust `Hash` / `Range`。
- Go `PhysicalShuffle` 的 `Concurrency`、`Tails`、`DataSources`、`SplitterType`、`ByItemArrays` 在轻量结构中都有字段；Rust 另显式保存通用节点需要的 `schema`。
- `explain_info` 保留“并发度 + 数据源 ExplainID”的意图和测试字符串形状。
- `resolve_indices` 同样按每个 DataSource 的 schema 解析对应 ByItems，而非使用普通 `children[0]`。
- 两侧内存函数都递归考虑 tails、数据源与分区表达式，但 Rust 轻量表达式的统计口径更窄，且 Go 会先解析 `BasePhysicalPlan` 并统计接口/切片头。
- Go `Init` 建立 `BasePhysicalPlan`、required properties 与 stats；本文件以 `into_plan(stats, property)` 完成较简化的组装，没有 PlanContext、query block offset 或基础计划方法。
- Go receiver stub 保存 `unsafe.Pointer Receiver` 和 `DataSource`，执行器可由指针返回实际 receiver；本文件用 `receiver_index` 替代指针，转换后仅得到无孩子通用节点，不能执行 Go 的 worker 对接。

Go 优化器只在并发度大于 1、输入形状合适且估算 NDV 大于 1 等条件下插入 Shuffle，并把并发度限制到 NDV；本文件不实现这些策略。Go builder 根据 splitter type 实例化具体 hash/range splitter；本文件也不实现数据重分区算法。因此它应被描述为可测试的计划值骨架，而不是 Go Shuffle 子系统的完整移植。

## 扩展指南

- 新增 splitter 策略时，先扩展 `PartitionSplitterType`，再同步主链 [`physical_window.rs`](physical_window.rs)、Rust optimizer/executor 消费点及 Go 语义；仅增加枚举项不会产生执行能力。补充独立测试，至少覆盖默认值、EXPLAIN/转换保真以及不支持策略的处理。
- 若要求严格保持 `ByItemArrays` 与 `DataSources` 一一对应，应在 `resolve_indices` 把当前单边检查改为等长检查，并为“更少”和“更多”两种不匹配分别增加回归断言；同时核对主链版本已有的等长契约。
- 若通用节点需要在转换后保留并发度、splitter 和分区表达式，应先扩展 `PhysicalPlanNode`/`PhysicalKind` 的数据模型，而不是静默依赖原结构仍然存在；这会影响序列化、相等性、内存统计和所有构造点。
- 修改孩子顺序时必须保留执行语义并同步 `into_plan_preserves_every_worker_tail_after_data_sources`；Go builder 对 `DataSources`、`Tails` 和普通 child 的角色不同，不能把它们任意扁平化。
- 完善内存估算时，应统一 `PhysicalExpr` 深层堆分配、外层 Vec capacity 与可选数据源的计费口径，并避免与 `size_of::<Self>()` 重复计数。
- receiver 扩展如需真实通道或共享执行状态，应明确线程安全、关闭顺序和所有权；不能从 Go 的 `unsafe.Pointer` 直接机械翻译。测试仍应放在独立的 `physical_shuffle_test.rs`，不要嵌入生产源文件。

## 验证依据

- 源码：[`physical_shuffle.rs`](physical_shuffle.rs)；逐项核对了 1 个枚举、2 个结构、7 个可执行方法以及注释化草案与实际代码的边界。
- 公共值模型：[`physical_common_plans.rs`](physical_common_plans.rs)；核对 `PhysicalExpr::resolve_indices`、`PhysicalKind`、`PhysicalPlanNode` 与其 `memory_usage`。
- crate 与模块接线：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)；确认包名、Go package 映射、公开模块、独立测试模块和主链 trait 实现位置。
- Rust 主链对照：[`physical_window.rs`](physical_window.rs)、[`pkg/planner/core/optimizer_runtime.rs`](../../optimizer_runtime.rs)、[`pkg/executor/statement_ru_plan_walk.rs`](../../../../executor/statement_ru_plan_walk.rs)；确认同名主链结构与本文件轻量结构并存，主链复用 `PartitionSplitterType`。
- Go 对照：[`physical_shuffle.go`](physical_shuffle.go)、[`pkg/planner/core/plan.go`](../../plan.go)、[`pkg/executor/builder.go`](../../../../executor/builder.go)；确认优化器插入条件、计划切分角色、splitter/worker/receiver 构建与错误边界。
- 独立 Rust 测试：[`physical_shuffle_test.rs`](physical_shuffle_test.rs)；覆盖 EXPLAIN 数据源 ID、逐数据源 schema 解析、数据源在前/tails 在后的孩子顺序，以及 receiver 可选数据源内存计入。
- Go 测试引用：`pkg/planner/core/plan_test.go`、`pkg/planner/core/integration_test.go`、`pkg/executor/statement_ru_plan_walk_test.go` 与 `pkg/executor/benchmark_test.go`；它们证明 Go 主链和执行器使用 Shuffle，但不直接证明本文件轻量结构已接线。
- RustCodeGraph：`status` 显示索引含 11,467 个文件；`explore/query/callers/callees` 查询了 `PhysicalShuffle`、`PartitionSplitterType`、`PhysicalShuffleReceiverStub` 及各方法。由于同名符号造成图查询歧义，调用结论再由精确 Rust 引用搜索核验。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认目标文档存在且恰有 11 个固定二级标题，并人工检查只新增本文件、链接指向真实相邻源码、没有把 Go 或另一套 Rust 类型的能力误写成本文件能力。
