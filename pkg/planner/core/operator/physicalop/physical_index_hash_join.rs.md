# `pkg/planner/core/operator/physicalop/physical_index_hash_join.rs`

源文件：[`physical_index_hash_join.rs`](physical_index_hash_join.rs)

## 文件定位

本文件位于 `astersql-planner-core-operator-physicalop` crate，定义索引哈希连接的两套 Rust 表示：当前参与统一物理计划接口的 `PhysicalIndexHashJoin`，以及供旧式轻量计划、缓存快照和相关测试使用的 `LegacyPhysicalIndexHashJoin`。crate 边界由同目录 `Cargo.toml` 的 `[package]`、`[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "pkg/planner/core/operator/physicalop"` 确认。

`lib.rs` 公开再导出 `PhysicalIndexHashJoin`，并用 `join_operator_core!(PhysicalIndexHashJoin, BasePhysicalJoin)` 将它接入 `ConcretePhysicalOperator` 和统一物理计划 trait。它是规划阶段的计划节点，不是执行器本身；执行侧会按动态类型或 `tp() == "IndexHashJoin"` 识别该节点，例如 `pkg/executor/statement_ru_plan_walk.rs`。真正的索引探测、哈希表构建和 worker 错误处理不在本文件内。

## 核心职责

1. `PhysicalIndexHashJoin` 在完整的 `PhysicalIndexJoin` 状态之上增加 `KeepOuterOrder`，保留 Go 同名算子的具体动态类型、外表顺序要求和基础索引连接契约。
2. `New` 把所接收 `PhysicalIndexJoin` 的算子类型改为 `"IndexHashJoin"`，并默认关闭外表保序；其余连接键、过滤器、子计划、统计信息和上下文都继续存放在嵌入的 `PhysicalIndexJoin` 中。
3. `Clone` 深度委托 `PhysicalIndexJoin::Clone(context)`，再复制 `KeepOuterOrder`，确保换上下文克隆时仍保持具体变体。
4. `MemoryUsage` 在基础索引连接的估算值上增加一个 `bool` 的静态大小。
5. `LegacyPhysicalIndexHashJoin` 提供可独立计算的简化节点：拼装 outer/inner 子树、估算两版成本、缓存 V1 成本并递归汇总内存。它没有被 `lib.rs` 公开再导出，当前直接用于 `cache_snapshot.rs`、`plan_clone_generated.rs` 和相邻独立测试。

文件前半第 21–108 行还有一份全部被注释掉的 Go 对齐草稿，列出了 `Init`、`Attach2Task`、`GetCost`、两版计划成本等预期接口。它不是可编译代码，不能当作当前 Rust 已支持行为。

## 主要符号

- `LegacyPhysicalIndexHashJoin`：包含 `outer`、`inner`、`keep_outer_order`、`concurrency` 和 `cached_cost`。派生 `Clone`、`Debug`、`PartialEq`，适合快照与确定性断言。
- `LegacyPhysicalIndexHashJoin::attach_to_task(&self) -> PhysicalPlanNode`：连接两侧 schema，以两子节点最大 ID 加一生成父 ID，节点种类固定为 `PhysicalKind::IndexHashJoin`，children 顺序固定为 outer 后 inner，统计信息复制 outer，必需属性初始化为空。
- `LegacyPhysicalIndexHashJoin::cost(...) -> f64`：返回 `outer_cost + inner_cost + (outer_count + inner_count) * cpu_factor / max(concurrency, 1) + inner_count * memory_factor`。`max(1)` 保证并发度为零时不除零。
- `plan_cost_v1(&mut self, cpu, memory) -> f64`：优先返回 `cached_cost`；未缓存时使用两侧 `stats.row_count` 和零子成本计算并写回缓存。
- `plan_cost_v2(&self, cpu, memory) -> f64`：按当前统计即时计算，不读取或写入缓存。
- `memory_usage(&self) -> i64`：结构自身静态大小加 outer、inner 节点的递归内存估算。
- `PhysicalIndexHashJoin { PhysicalIndexJoin, KeepOuterOrder }`：Go 兼容的生产计划类型。字段采用 Go 风格命名以维持移植契约。
- `Deref` / `DerefMut<Target = PhysicalIndexJoin>`：让调用方可直接访问基础索引连接的方法和字段；`lib.rs` 的 `join_operator_core!` 也因此能沿 `BasePhysicalJoin` 接入统一算子能力。
- `PhysicalIndexHashJoin::New(PhysicalIndexJoin) -> Self`：原地设置类型标签并构造默认非保序变体。
- `PhysicalIndexHashJoin::Clone(ContextRef) -> Result<Self, expression::Error>`：克隆完整基础连接并传播克隆错误。
- `PhysicalIndexHashJoin::MemoryUsage() -> i64`：委托基础结构后计入保序标志。

## 执行流程

生产类型的典型流程如下：调用方先构造完整 `PhysicalIndexJoin`，再调用 `PhysicalIndexHashJoin::New`；`New` 沿 `BasePhysicalJoin -> PhysicalSchemaProducer -> BasePhysicalPlan` 写入 `IndexHashJoin` 类型标签。此后 `lib.rs` 提供的公共再导出和统一 trait 实现使节点可放入 `Box<dyn base::PhysicalPlan>`，公共辅助函数 `index_join_base`/`index_join_base_mut` 能取回其基础 `PhysicalIndexJoin`。

当统一接线重写基础索引连接时，`preserve_index_join_variant` 按原动态类型重新包成 `PhysicalIndexHashJoin` 并复制 `KeepOuterOrder`。计划缓存路径中，`cache_snapshot.rs::CachedPhysicalPlan::try_capture` 将基础索引连接和保序位分别捕获，恢复时再构造相同具体类型。执行器的 RU 计划遍历则通过 `index_join_base` 获取公共状态，并对 `PhysicalIndexHashJoin` 使用 `OuterHashKeys.len()` 计算连接键槽位。

旧式类型的流程彼此独立：`attach_to_task` 生成一个新的轻量父节点；成本查询先由调用方选择 V1 或 V2，V1 首次计算后固定在 `cached_cost`，V2 每次读取当前统计；缓存克隆由 `plan_clone_generated.rs` 分别克隆 outer 和 inner，快照往返由 `cache_snapshot.rs::CachedIndexHashJoin` 保存全部五个字段。

## 数据与状态

`PhysicalIndexHashJoin` 自身只新增 `KeepOuterOrder`，主要可变状态都由 `PhysicalIndexJoin` 所有，包括基础物理连接、哈希键、比较过滤器、子节点、schema、上下文和成本信息。`DerefMut` 允许这些基础状态通过包装类型被修改，因此修改后仍须维持类型标签、子节点、schema 和连接键之间的一致性。

`LegacyPhysicalIndexHashJoin` 直接拥有两个 `PhysicalPlanNode` 副本。`attach_to_task` 克隆 schema、children 和 outer 统计，不会消费原节点；合并 schema 仅按 outer 后 inner 追加，不做去重。`cached_cost` 只缓存一个 `f64`，没有记录 cpu/memory 因子或统计版本，所以调用方一旦改变统计、因子或并发度，必须主动清空缓存，否则 V1 会返回旧值。`keep_outer_order` 在此轻量结构的本文件算法中不参与成本或组树，但会被快照和克隆完整保留。

## 依赖与调用关系

本文件直接依赖同 crate 的 `physical_common_plans::{PhysicalKind, PhysicalPlanNode}`、crate 根的 `PhysicalIndexJoin`，以及 `base::ContextRef`、`expression::Error`。这些依赖均由同目录 `Cargo.toml` 声明；其中 `base` 和 `expression` 是显式 path dependency，`PhysicalIndexJoin` 和公共节点类型来自本 crate 模块。

上游直接接线包括：`lib.rs` 的公开再导出、`join_operator_core!`、`index_join_base(_mut)` 和 `preserve_index_join_variant`；`cache_snapshot.rs` 的生产类型捕获/恢复与 legacy 快照；`plan_clone_generated.rs` 的 legacy 计划缓存克隆；`pkg/executor/statement_ru_plan_walk.rs` 的动态类型识别。RustCodeGraph 的文件节点报告该文件被 13 个文件使用，并精确检索到上述生产和测试引用。

下游方面，生产包装器把绝大多数行为委托给 `PhysicalIndexJoin`：`New` 调用 `SetTP`，`Clone` 调用基础 `Clone`，`MemoryUsage` 调用基础 `MemoryUsage`。legacy 类型只调用 `PhysicalPlanNode` 的 clone、stats 和 `memory_usage`。注释草稿中的 `utilfuncp` 成本与挂接函数没有编译，不能列为当前 Rust 调用边。

## 错误处理与边界

本文件唯一显式可失败的生产方法是 `PhysicalIndexHashJoin::Clone`；它使用 `?` 原样传播 `PhysicalIndexJoin::Clone` 的 `expression::Error`，包装层不吞错也不补充上下文。`New`、`MemoryUsage` 和全部 legacy 方法均为无错误返回接口。

边界条件包括：legacy 并发度为 0 时按 1 计算；空 schema 可正常拼接；父 ID 使用 `max(id) + 1`，本文件没有处理整数上界溢出；负数行数或负成本因子不会被校验；`f64` 的 NaN/无穷值会按浮点运算传播。`attach_to_task` 无条件清空 `required_properties` 并采用 outer 的统计，这只是当前轻量模型事实，不能外推为完整生产规划器的属性推导规则。

与 Go 实现相比，Rust `MemoryUsage` 没有 Go 的 nil receiver 分支；调用 Rust 方法必须已有有效引用。Rust `New` 也只设置类型标签，不像 Go `Init` 那样分配 PlanID、设置计划上下文和 `Self` 指针；这些能力目前由传入的基础连接和 crate 统一接线承担，不能假定 `New` 完成了 Go `Init` 的全部工作。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源。`LegacyPhysicalIndexHashJoin::concurrency` 只是成本公式的数值除数，并不启动 worker；真实 IndexHashJoin worker 生命周期属于执行器实现范围。

所有权方面，生产 `New` 消费一个 `PhysicalIndexJoin`，`Clone` 生成独立的基础连接并复制布尔值；legacy 的 `attach_to_task` 克隆两个子节点到新 children 中。`DerefMut` 暴露基础连接的可变访问，因此并发共享策略取决于外层计划所有权，文件自身没有同步保护。缓存 `Option<f64>` 的读写需要 `&mut self`，避免通过同一安全 Rust 引用并发修改，但不提供跨线程失效协议。

## 与 Go 版本的对应关系

Go 对照文件为 `physical_index_hash_join.go`。两端生产结构都以 `PhysicalIndexJoin` 为基础并增加 `KeepOuterOrder`；Rust `Clone` 与 Go `Clone` 都克隆基础连接并保留该布尔值，Rust `MemoryUsage` 与 Go 都在基础估算上增加布尔字段大小。Rust 的 `Deref/DerefMut` 对应 Go 的匿名嵌入字段提升效果。

当前差异必须显式保留：Go 有 `Init`、`Attach2Task`、`GetCost`、`GetPlanCostVer1` 和 `GetPlanCostVer2` 的同名实现，并通过 `utilfuncp` 接入完整成本/任务逻辑；Rust 文件中这些版本仅存在于注释草稿。可编译生产类型依靠 `lib.rs` 的统一 trait 将成本调用委托到嵌入的 `PhysicalIndexJoin`，而 legacy 类型实现的是较简化的本地公式，两者不可等同。Go `Init` 分配会话 PlanID、设置上下文和 Self；Rust `New` 只覆盖类型标签并保留传入基础状态。

Go 测试 `physical_utils_test.go` 证明哈希/合并变体因嵌入 `PhysicalIndexJoin` 而应被公共扫描逻辑识别。Rust 的对应直接证据是 `canonical_router_aster_unit_test.rs::canonical_router_recognizes_all_index_join_families`，以及 `cache_snapshot_test.rs` 中具体类型、`KeepOuterOrder`、键、过滤器和子节点的往返/克隆断言。

## 扩展指南

若扩展生产 `PhysicalIndexHashJoin` 字段，应同步检查 `New`、`Clone`、`MemoryUsage`，以及 `cache_snapshot.rs` 的 `CachedPhysicalPlan::IndexHashJoin` 捕获/恢复、`lib.rs::preserve_index_join_variant` 和计划缓存克隆路径，防止具体变体或状态丢失。新增基础索引连接行为应优先放在 `PhysicalIndexJoin`，仅哈希变体特有的状态才放在本结构中。

若完善 Go 对齐接口，不应直接把注释草稿视为实现；需要核对 Go `utilfuncp` 的挂接与两版成本语义，再接入 Rust 统一 trait，避免与 `LegacyPhysicalIndexHashJoin::cost` 的简化公式重复或冲突。类型标签必须继续为 `IndexHashJoin`，并保持 `index_join_base(_mut)`、RU 遍历、缓存快照和动态类型路由可识别。

测试必须放在独立文件。生产类型变更至少同步 `canonical_router_aster_unit_test.rs`、`cache_snapshot_test.rs` 和需要时的 `pkg/executor/statement_ru_plan_walk_test.rs`；legacy 组树、零并发、V1 缓存失效策略或内存公式变更，应在同目录独立测试文件增加针对性用例。性能风险主要来自成本公式和保序语义改变导致的计划选择变化；兼容风险主要来自 Go 字段语义、动态类型和缓存快照格式失配。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/planner/core/operator/physicalop/physical_index_hash_join.rs` 显示目标文件含 15 个符号；`node --file ... --offset 1 --limit 400` 读取完整 223 行并报告 13 个使用文件。精确 `query` 定位了 Rust/Go 的 `PhysicalIndexHashJoin`、legacy 结构、公共再导出、缓存和执行器入口；精确 `callers/callees` 未返回方法级边，因此未据此臆造调用关系。
- 源码与 crate：`physical_index_hash_join.rs`、同目录 `Cargo.toml`、`lib.rs`、`physical_index_join.rs`（经包装字段和公共接口引用）。
- 直接生产接线：`lib.rs` 的 `join_operator_core!`、`index_join_base(_mut)`、`preserve_index_join_variant`；`cache_snapshot.rs` 的生产/legacy 捕获恢复；`plan_clone_generated.rs` 的 legacy 克隆；`pkg/executor/statement_ru_plan_walk.rs` 的 RU 类型识别。
- 独立 Rust 测试：`canonical_router_aster_unit_test.rs::canonical_router_recognizes_all_index_join_families`；`cache_snapshot_test.rs::join_cached_round_trip_preserves_index_hash_children_and_properties`、`go_merge_187_join_aggregation_typed_index_cache`、`go_merge_187_join_aggregation_attach_keeps_concrete_variant`；`pkg/executor/statement_ru_plan_walk_test.rs` 的索引哈希连接槽位覆盖。
- Go 对照与测试：`physical_index_hash_join.go`、`physical_utils_test.go`，以及执行/规划测试中对 `IndexHashJoin` 计划形状的断言。上述执行测试证明算子标签进入应用链，但执行器 worker 内部行为不属于本文件实现，本文未将其归因于此处。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；交付只执行任务指定的 11 章节结构检查、Markdown 链接/事实复查和 diff 自审。
