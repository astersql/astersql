# `pkg/planner/core/operator/logicalop/logical_mock.rs` 逻辑说明

## 文件定位

`logical_mock.rs` 位于 `astersql-planner-core-operator-logicalop` crate 内，是逻辑算子测试使用的最小数据源节点实现。模块由 `pkg/planner/core/operator/logicalop/lib.rs` 以 `mod logical_mock` 纳入 crate，并通过 `pub use logical_mock::*` 对外导出 `MockDataSource`；它不是 SQL 构建或真实表扫描主链中的生产数据源，真实数据源语义位于同 crate 的 `logical_datasource.rs` 等文件。

该文件虽然处于生产源码目录，却明确承担测试支撑职责：源码注释称其为“测试用 Mock 数据源”，当前 Rust 直接引用只出现在 `logical_union_all_test.rs` 和 `logical_partition_union_all_test.rs`。它让逻辑规则测试可以构造一个满足 `LogicalPlan` 契约、但不引入表元数据、访问路径或执行器依赖的叶节点。

## 核心职责

文件只有三项职责：

1. 用 `MockDataSource` 嵌入一份 `BaseLogicalPlan`，承载逻辑节点共有的上下文、节点 ID、类型名、子节点、Schema、统计信息等状态。
2. 用 `MockDataSource::Init` 把默认对象初始化成类型名为 `"mockDS"`、查询块偏移为 `0` 的逻辑计划节点。
3. 实现 `LogicalPlan` 所要求的向下转型和基类访问入口，使通用优化代码能够通过 trait 默认方法操作该节点。

它不负责读取数据、构造物理扫描、推导访问路径或连接执行器。规则测试把它当作“足够真实的逻辑叶子/分支占位符”，被测行为属于其父算子，而不是 `MockDataSource` 自身。

## 主要符号

- `MockDataSource`：公开结构体，唯一字段是公开的 `BaseLogicalPlan: BaseLogicalPlan`。`#[derive(Default)]` 使测试既能创建未初始化占位节点，也能随后调用 `Init` 获得有上下文和计划 ID 的节点。
- `MockDataSource::Init(self, ctx: base::ContextRef) -> Self`：按值接收并返回节点。它调用 `crate::NewBaseLogicalPlan(ctx, "mockDS", 0)` 覆盖基类字段；没有额外的 Schema、统计或子节点初始化逻辑。
- `impl LogicalPlan for MockDataSource`：实现 `as_any`、`as_any_mut`、`base`、`base_mut` 四个必需方法。其余计划操作（例如 `TP`、`ID`、`Schema`、`SetSchema`、`SetChildren`、`PredicatePushDown`、`PruneColumns`、`PushDownTopN`）均继承 `base_logical_plan.rs` 中 `LogicalPlan` 的默认实现。

文件中没有模块级常量、额外 trait、条件编译项或私有辅助函数。上述结构体和方法经 `lib.rs` 的通配再导出成为 crate 公共 API。

## 执行流程

典型测试流程如下：

1. 测试构造实现 `base::PlanContext` 的上下文，并以 `MockDataSource::default()` 得到空基类节点。
2. 调用 `Init(ctx)`；`NewBaseLogicalPlan` 通过 `ctx.alloc_plan_id()` 分配节点 ID，保存上下文，设置类型名 `mockDS` 和查询块偏移 `0`，其余字段沿用 `BaseLogicalPlan::default()`。
3. 测试可通过 `LogicalPlan` 默认方法补充状态，例如 `logical_union_all_test.rs::child` 调用 `SetSchema` 后将节点装箱为 `LogicalPlanRef`。
4. 父算子的规则沿 `Children`、`Schema`、`PredicatePushDown` 或 `PushDownTopN` 等通用接口访问该节点；需要判断具体类型时，通过 `as_any`/`as_any_mut` 做运行时向下转型。

`logical_partition_union_all_test.rs::push_down_top_n_clones_limit_and_order_for_every_partition` 使用两个已初始化 mock 分支验证 TopN 克隆；`logical_union_all_test.rs` 的多个用例把已初始化并配置 Schema 的 mock 作为 UnionAll 子节点，验证列裁剪、谓词下推和 TopN 下推。`push_down_without_top_n_preserves_partition_union` 则放入未调用 `Init` 的默认 mock，但该路径只验证 `None` 输入时保留父节点，没有读取 mock 的上下文或 ID。

## 数据与状态

`MockDataSource` 自身没有业务数据，全部状态都在 `BaseLogicalPlan` 中。相关字段由 `base_logical_plan.rs::BaseLogicalPlan` 定义，包括规划上下文、类型名、计划 ID、查询块偏移、任务缓存、子节点、Schema、输出列名、统计信息、`max_one_row`、函数依赖集合和 TiFlash 标志等。

调用 `Init` 后可确认的不变量是：上下文存在；类型名为 `mockDS`；ID 来自传入上下文的 `alloc_plan_id`；查询块偏移为 `0`；未显式设置的 Schema、子节点、统计等仍是基类默认值。直接使用 `MockDataSource::default()` 时，上下文为空、类型名为空、ID 为 `0`，因此只有不依赖这些初始化字段的测试路径才应这样使用。

`Init` 按值更新并返回 `Self`，与当前 Rust 算子的链式初始化风格一致。`base()` 与 `base_mut()` 分别暴露共享状态的只读和可变访问；具体变更通常通过 `LogicalPlan` 默认方法完成。

## 依赖与调用关系

直接依赖关系为：

- `use crate::{BaseLogicalPlan, LogicalPlan}`：依赖同 crate 的逻辑计划基类和公共 trait。
- `base::ContextRef`：来自 `Cargo.toml` 中别名为 `base` 的路径依赖 `astersql-planner-core-base`，为共享规划上下文引用。
- `std::any::Any`：支撑 trait 对象的运行时向下转型。
- `crate::NewBaseLogicalPlan`：初始化下游；该函数进一步调用 `base::PlanContext::alloc_plan_id`。

已验证的上游使用者为 `logical_union_all_test.rs` 和 `logical_partition_union_all_test.rs`。模块入口 `lib.rs` 负责装配与再导出；`Cargo.toml` 的 `[package.metadata.porting] go-package = "pkg/planner/core/operator/logicalop"` 明确该 crate 对应的 Go 包边界。RustCodeGraph 将目标文件识别为 7 个符号，并报告上述两个直接使用文件；同名 `Init` 在全仓库中高度重载，精确调用边因此又用源码引用搜索核对。

## 错误处理与边界

本文件没有返回 `Result` 的函数，也不主动产生或转换错误。`Init` 的唯一外部动作是调用上下文分配计划 ID；`PlanContext::alloc_plan_id` 的接口本身不返回错误，因此初始化没有可传播的失败分支。

边界来自其“最小 mock”定位：

- 它未覆盖 `LogicalPlan` 的优化方法，所有行为都来自 `BaseLogicalPlan` 默认实现；默认叶节点遇到需要真实数据源特性的规则时，不能替代 `DataSource`。
- 未调用 `Init` 的默认实例缺少有效上下文和类型信息。若后续路径调用 `SCtx`、依赖唯一 ID 或按类型名工作，应先初始化。
- 默认 Schema 为空。需要验证列传播或列裁剪时，调用方必须像 `logical_union_all_test.rs::child` 一样显式 `SetSchema`。
- 文件不定义真实扫描结果、错误注入点或物理计划转换；新增此类需求应使用更贴近真实算子的测试夹具，而不是悄悄扩大该 mock 的语义。

## 并发与资源生命周期

`MockDataSource` 不创建线程、异步任务、锁、通道、事务、文件或网络资源，也没有自定义 `Drop`。节点拥有自己的 `BaseLogicalPlan`；其生命周期随值移动或装箱后的 `LogicalPlanRef` 所有权结束。

规划上下文通过 `base::ContextRef` 共享。现有两个测试中的上下文以 `Arc` 承载，并用 `AtomicI32` 分配计划 ID，这是测试上下文的并发安全策略，不是 `MockDataSource` 自身实现的同步机制。目标文件没有声明额外的 `Send`/`Sync` 保证；是否能跨线程使用取决于其字段和 trait 对象边界，不应从本 mock 的简单结构推导新的并发承诺。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/operator/logicalop/logical_mock.go`。两侧都定义仅供测试使用的 `MockDataSource`，都只嵌入/保存 `BaseLogicalPlan`，初始化时都传入规划上下文、使用类型名 `mockDS` 和查询块偏移 `0`。

主要语言差异如下：

- Go 的嵌入字段自动提供基类方法；Rust 显式实现 `LogicalPlan` 的 `base`/`base_mut` 与 `Any` 转型入口，从而复用 trait 默认方法。
- Go 的 `Init` 返回 `*MockDataSource`，并把 `&ds` 作为 self 参数交给 Go 版 `NewBaseLogicalPlan`；Rust 的 `Init` 返回拥有所有权的 `Self`，Rust 版基类通过 trait 访问具体节点，不保存该 self 指针。
- Go 文件只声明测试用途，没有同目录 Go 测试直接引用该类型；当前 Rust 已有两个独立测试模块直接使用它。这说明 Rust 侧的实际验证面来自 UnionAll 和 PartitionUnionAll 规则测试，而不是同名 mock 专项测试。

这些差异是对象模型和所有权表达的差异；当前可观察的初始化语义保持一致。不能据此声称 Rust mock 已覆盖 Go `DataSource` 的生产行为，因为两侧这个文件本身都不是生产数据源实现。

## 扩展指南

若新增规则测试只需要一个空逻辑叶节点，优先复用 `MockDataSource::default().Init(ctx)`，并按测试所需显式设置 Schema、输出名或统计信息。若新路径依赖某项通用逻辑计划行为，先确认 `LogicalPlan`/`BaseLogicalPlan` 的默认实现是否已满足，不要在 mock 中复制父类逻辑。

修改该文件时最可能涉及的接入点是 `MockDataSource::Init` 和 `impl LogicalPlan for MockDataSource`。新增状态应先判断它是否属于所有逻辑算子；若属于，应修改 `BaseLogicalPlan` 而非只加到 mock。若属于真实数据源扫描或访问路径语义，应修改 `logical_datasource.rs` 及对应测试，而非扩展本占位节点。

测试应继续放在独立 `*_test.rs` 文件中。影响 UnionAll 通用规则时同步检查 `logical_union_all_test.rs`；影响分区 UnionAll 的 TopN 下推时同步检查 `logical_partition_union_all_test.rs`。兼容风险主要是更改 `mockDS` 类型名、初始化 ID/上下文语义或默认 trait 行为导致测试树行为漂移；性能风险很低，但给 mock 引入真实元数据或重型依赖会提高规则单测的构造成本并削弱隔离性。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件已索引。
- RustCodeGraph `files --filter pkg/planner/core/operator/logicalop/logical_mock.rs`：确认目标文件含 7 个符号。
- RustCodeGraph `node --file .../logical_mock.rs --offset 1 --limit 260`：核对完整目标源码，并报告直接使用文件为 `logical_partition_union_all_test.rs` 与 `logical_union_all_test.rs`。
- RustCodeGraph 对 `MockDataSource`、`Init` 的查询及调用边尝试：确认 Go/Rust 同名符号；因全仓库存在大量同名 `Init`，调用方/被调用方未能精确消歧，随后以直接引用搜索补证。
- `pkg/planner/core/operator/logicalop/base_logical_plan.rs`：核对 `LogicalPlan` 必需方法、默认行为、`BaseLogicalPlan` 状态和 `NewBaseLogicalPlan → alloc_plan_id` 初始化链。
- `pkg/planner/core/operator/logicalop/lib.rs`：核对模块装配、公开再导出和独立测试模块声明。
- `pkg/planner/core/operator/logicalop/Cargo.toml`：核对 crate 名、`base` 路径依赖和 Go 包映射。
- `pkg/planner/core/operator/logicalop/logical_mock.go`：核对 Go 版本的结构与初始化语义。
- `pkg/planner/core/operator/logicalop/logical_union_all_test.rs`、`logical_partition_union_all_test.rs`：核对真实使用方式、初始化边界和被测规则；未发现同名专项 Rust 测试或直接引用该类型的同目录 Go 测试。
- 本任务只新增说明文档；按任务约束不运行 Cargo。交付前以任务指定命令验证恰好包含 11 个固定二级章节，并人工复核文件定位、运行方式和安全扩展入口均有上述源码证据。
