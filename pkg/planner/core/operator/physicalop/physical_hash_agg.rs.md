# `pkg/planner/core/operator/physicalop/physical_hash_agg.rs`

## 文件定位

本文件说明的真实源码是 [`physical_hash_agg.rs`](./physical_hash_agg.rs)。它定义规划器中的物理哈希聚合节点 `PhysicalHashAgg`，位于 `astersql-planner-core-operator-physicalop` crate。模块由同目录 `lib.rs` 声明为私有模块并通过 `pub use physical_hash_agg::*` 再导出；`agg_operator_core!(PhysicalHashAgg)` 再把这里的方法接入统一的 `PhysicalPlan`/`ConcretePhysicalOperator` 接口。

它处在逻辑聚合与执行层之间：`base_physical_agg.rs::ExhaustPhysicalPlans4LogicalAggregation` 和旧 Cascades 规则 `implementation_rules.rs::ImplHashAgg::OnImplement` 从 `LogicalAggregation` 枚举/构造它；选中的计划随后由 Task 接线、TiPB 下推编码或 `pkg/executor/physical_plan_runtime.rs::execute_node` 消费。这里保存和转换计划元数据，不实现哈希表、分组桶或逐行聚合算法；本地 Rust 行级执行逻辑在 executor 运行时。

## 核心职责

- 用 `PhysicalHashAgg` 组合通用的 `BasePhysicalAgg` 与 TiFlash 专属的 `TiflashPreAggMode`。
- 用 `NewPhysicalHashAgg` 从 `LogicalAggregation` 复制分组表达式和聚合描述符，隔离后续物理优化的原地改写。
- 提供克隆、内存估算、旧版成本和 V1/V2 计划成本的接口，以及 CPU 成本并发摊销参数。
- 通过 `Attach2Task` 接入通用物理计划 Task 组装。
- 通过 `ToPB`/`aggregate_to_pb` 把分组项、聚合函数、TiFlash 子计划和细粒度 shuffle 参数编码为 `tipb::Executor`。

文件本身不负责候选枚举的完整策略。Root、Cop、MPP、一阶段/二阶段等候选及子属性主要在 `base_physical_agg.rs::ExhaustPhysicalPlans4LogicalAggregation` 中决定，再经 `BasePhysicalAgg::InitForHash` 补齐统计、Schema 和孩子物理属性。

## 主要符号

- `PhysicalHashAgg { BasePhysicalAgg, TiflashPreAggMode }`：具体计划节点。前者持有 Schema producer、聚合描述符、分组项、MPP 运行模式和分区列；后者为空时表示无需设置 TiFlash 预聚合模式。
- `GetPointer(&mut self)`：把可变访问转发给嵌入的聚合基座，供共享的聚合改写逻辑使用。
- `Clone(new_ctx)`：调用 `BasePhysicalAgg::CloneWithSelf` 深拷贝计划状态并换绑计划上下文，同时复制预聚合模式字符串。
- `MemoryUsage()`：转发给基座，统计计划、Schema、聚合描述符、分组表达式和 MPP 分区列的估算内存；不是运行时哈希表内存。
- `CPUCostDivisor(has_distinct)`：读取 final/partial hash-agg 并发系统变量；无效或非正配置回退到 `TiDBExecutorConcurrency`，再交给 `hash_agg_cpu_cost_divisor`。
- `GetCost(input_rows, ..., is_mpp, ...)`：按非负输入行数乘以 `BasePhysicalAgg::GetAggFuncCostFactor`；当前 Rust 实现没有在此处计入 Go `utilfuncp.GetCost4PhysicalHashAgg` 的全部 CPU/内存参数。
- `GetPlanCostVer1`、`GetPlanCostVer2`：当前转发到内层 `BasePhysicalPlan` 的通用成本缓存/计算入口。
- `Attach2Task(tasks)`：调用 `base::PhysicalPlan::attach_to_task` 做统一 Task 挂接。
- `ToPB(ctx, store)`：构造 `TypeAggregation` executor，并仅在 TiFlash 且模式非空时设置 `pre_agg_mode`。
- `NewPhysicalHashAgg(logical, producer)`：建立空基座，克隆 `GroupByItems` 与 `AggFuncs`，返回尚待 `InitForHash` 完成统计、Schema 和孩子属性初始化的节点。
- `clone_agg_funcs`：按值克隆整个 `AggFuncDesc` 列表；独立测试确认 mode、DISTINCT、GroupingID、返回类型和排序项不会丢失或共享可变状态。
- `hash_agg_cpu_cost_divisor`：DISTINCT 或 final/partial 都为 1 时返回 `(0, 0)`；否则返回 `(min(final, partial), final + partial)`。
- `tiflash_pre_agg_mode`：只接受 `force_preagg`、`auto`、`force_streaming` 对应的系统变量常量，其他字符串返回错误。
- `aggregate_to_pb`：完成表达式及聚合函数的 protobuf 转换，并组装 TiFlash 子 executor、executor id 与 shuffle 配置。

## 执行流程

1. 物理候选枚举器为逻辑聚合创建名为 `HashAgg` 的 `BasePhysicalPlan`/`PhysicalSchemaProducer`，调用 `NewPhysicalHashAgg` 复制逻辑计划内容。
2. `BasePhysicalAgg::InitForHash` 设置统计信息、输出 Schema 和孩子所需属性，并保证计划树中的类型名为 `HashAgg`。MPP 路径会在此前后设置 `MppRunMode`、`MppPartitionCols` 和孩子的分区要求。
3. 优化器通过统一计划接口读取成本。`CPUCostDivisor` 对无 DISTINCT 的并行 hash agg 给出成本摊销除数；DISTINCT 或完全串行时用 `(0, 0)` 表示不应用这项摊销。
4. 选中计划后，`Attach2Task` 把节点挂到孩子 Task；该文件把细节委托给通用接线，MPP 多阶段组装仍由基座/Task 层处理。
5. 下推编码时，`ToPB` 调用 `aggregate_to_pb`：先将所有 `GroupByItems` 转成 PB 表达式，再创建 push-down context 并逐一调用 `AggFuncToPBExpr`。
6. 对 TiFlash，编码器要求至少一个孩子，将第一个孩子递归编码进 `Aggregation.child`，写入 explain id；所有存储类型都会写入 executor 类型、聚合体和细粒度 shuffle 参数。
7. TiFlash 且 `TiflashPreAggMode` 非空时，`ToPB` 校验字符串并写入枚举值。非 TiFlash 路径不会携带该字段。
8. 本地执行时，`physical_plan_runtime.rs::execute_node` 识别 `PhysicalHashAgg`，执行唯一孩子，并走标量聚合快路径/流式路径/物化路径；该运行时当前明确限制为标量聚合，不能据此推断本文件实现了通用分组哈希执行器。

## 数据与状态

`PhysicalHashAgg` 自身只有两个字段。主要状态由 `BasePhysicalAgg` 持有：`AggFuncs` 描述聚合函数、模式、参数和返回类型，`GroupByItems` 描述分组键，`MppRunMode` 与 `MppPartitionCols` 描述分布式聚合布局，`PhysicalSchemaProducer` 进一步持有上下文、统计、Schema、孩子及 shuffle 配置。

构造函数刻意复制逻辑状态：分组表达式逐项 `CloneExpr`，聚合描述符经 `clone_agg_funcs` 完整克隆。因此物理优化改写 mode、排序项或 GroupingID 时不应污染逻辑计划。`Clone` 则用于已有物理节点换绑上下文，除上述集合外也保留 TiFlash 模式。

成本计算读取会话系统变量但不修改会话。PB 编码只创建局部 `Vec`、`Aggregation` 和 `Executor`；除递归读取孩子计划外不改变计划树。`BuildPBContext` 提供客户端、表达式上下文、告警处理器、 explain 标志和 shuffle batch size。

## 依赖与调用关系

上游直接关系：

- `base_physical_agg.rs::ExhaustPhysicalPlans4LogicalAggregation -> NewPhysicalHashAgg`：普通 Root/Cop 候选以及 MPP 一阶段、二阶段、TiDB-final 候选的主要入口。
- `cascades/old/implementation_rules.rs::ImplHashAgg::OnImplement -> NewPhysicalHashAgg`：旧 Cascades 将逻辑聚合转为 TiDB/TiKV 实现。
- `lib.rs::agg_operator_core!(PhysicalHashAgg)`：把 `MemoryUsage`、V1/V2 成本与 `ToPB` 暴露到统一动态计划接口。
- `pkg/executor/physical_plan_runtime.rs::execute_node`：对选中的节点执行 Rust 本地标量聚合路径。

下游直接关系：

- `BasePhysicalAgg::CloneWithSelf`、`MemoryUsage`、`GetAggFuncCostFactor` 提供共享聚合状态操作。
- `base::PhysicalPlan::attach_to_task` 提供通用 Task 接线。
- `expression::ExpressionsToPBList` 编码分组表达式；`aggregation::AggFuncToPBExpr` 编码聚合函数。
- `PhysicalPlan::Children`/`to_pb` 递归编码 TiFlash 的第一个孩子。
- `vardef` 提供 hash-agg 并发度、通用 executor 并发度和预聚合模式常量；`tipb` 提供目标协议类型。

`Cargo.toml` 证实该 crate 直接依赖 `base`、`costusage`、`expression`、`aggregation`、`kv`、`logicalop`、`property`、`vardef` 和带 `protobuf-codec` feature 的 `tipb`。模块无条件编译；只有独立测试模块受 `#[cfg(test)]` 控制。

## 错误处理与边界

- `Clone` 会传播基座克隆中的 `expression::Error`。
- `CPUCostDivisor` 对缺失或无法解析的系统变量使用默认值；配置值小于等于 0 时回退到通用 executor 并发度。辅助函数本身假定调用者已完成这种归一化。
- `GetCost` 用 `input_rows.max(0.0)` 防止负行数产生负成本。
- `aggregate_to_pb` 在 PB client 缺失时明确报错；分组表达式转换、聚合函数转换以及孩子递归编码的错误均用 `?` 原样传播。
- TiFlash 编码必须有至少一个孩子，否则返回 `TiFlash hash aggregation requires a child`；当前只取第一个孩子，符合一元聚合节点的不变量。
- 非法的 TiFlash 预聚合字符串返回 `unexpected tiflash pre agg mode: ...`，不会静默降级。空字符串是合法的“无需设置”哨兵。
- 非 TiFlash 编码不附加孩子到 `Aggregation.child`，executor id 保持空字符串；调用方不能把这一 PB 形态误当成 TiFlash 形态。
- 此文件不校验聚合函数是否可下推，也不决定 MPP 候选是否合法；这些前置约束由候选枚举和表达式/聚合 PB 转换层执行。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或运行时哈希表。`CPUCostDivisor` 中的 final/partial concurrency 只是成本模型输入，不代表这里启动 worker；实际并行执行由后续 executor 决定。

所有权边界清晰：构造和克隆后，物理节点拥有自己的表达式及聚合描述符；PB 编码器拥有新建的 protobuf 对象，并把递归生成的 TiFlash child 移入 `Aggregation`。共享的 planner context/client 仅以引用或 `Arc` 类句柄形式读取。函数没有显式清理步骤，资源释放依赖 Rust 所有权离开作用域。

需要关注的资源风险主要在规模上：PB 编码为全部分组项和聚合函数分配向量，`MemoryUsage` 只估算计划元数据，不包含 executor 执行时随分组基数增长的哈希状态。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/operator/physicalop/physical_hash_agg.go`。字段、`GetPointer`、克隆、内存估算、CPU 并发摊销、Task 挂接、TiPB 编码和构造器均有同名或等价入口。

已对齐的关键语义包括：物理优化前克隆聚合描述符；DISTINCT 与双并发为 1 时禁用 CPU 除数；其他情况使用较小并发度和并发总和；TiFlash PB 携带 child、explain id、预聚合模式和 shuffle 参数；非法预聚合模式报错。

当前实现结构与 Go 有几处重要差异：

- Go 的 `NewPhysicalHashAgg` 同时接收统计和孩子属性并调用 `InitForHash`；Rust 构造器只复制逻辑内容，调用方随后显式 `InitForHash`。
- Go `GetCost`、V1/V2 成本和 `Attach2Task` 调用 hash-agg 专用的 `utilfuncp`；Rust 当前分别使用简化的聚合函数因子、基座成本入口和通用 Task 接线。它们是当前代码事实，扩展成本或 Task 行为时必须重新核对 Go 专用路径，不能仅以名称相同认定完全等价。
- Go `ToPB` 直接使用 `ctx.GetClient()`，Rust 在 client 缺失时显式返回错误，并单独抽出 `aggregate_to_pb`/`tiflash_pre_agg_mode` 以便测试。
- Go 候选生成 `getHashAggs`/`tryToGetMppHashAggs` 与节点定义同文件；Rust 将主要候选逻辑放在 `base_physical_agg.rs`，本文件更聚焦节点与编码。

## 扩展指南

- 增加节点字段时，应同步更新 `PhysicalHashAgg::Clone`、`MemoryUsage`（若占用显著）、构造默认值、缓存快照/计划克隆逻辑和 Go 对照；若字段进入下推协议，还要更新 `ToPB`/`aggregate_to_pb`。
- 增加 TiFlash 预聚合模式时，应同时更新 `tiflash_pre_agg_mode`、`vardef` 常量、tipb 枚举映射及 `physical_hash_agg_aster_unit_test.rs::tiflash_pre_aggregation_mode_matches_go_validation`，并验证未知值仍报错。
- 修改成本语义时，优先核对 Go 的 `CPUCostDivisor`、`utilfuncp.GetCost4PhysicalHashAgg`、V1/V2 专用路径和会话变量回退规则；扩展 `hash_agg_cpu_divisor_matches_go_concurrency_rules`，特别覆盖 DISTINCT、串行和不对称并发。
- 修改逻辑到物理的复制规则时，扩展 `hash_agg_clones_complete_logical_descriptors`，确认嵌套排序项、返回类型及未来新增描述符字段不会共享可变状态。
- 修改 PB 形态时，需要分别验证 TiKV/非 TiFlash 与 TiFlash：client 缺失、表达式转换失败、无孩子、非法模式、executor id、child 和 shuffle 参数都是独立边界。
- 改变候选枚举、MPP 阶段或分区行为应在 `base_physical_agg.rs` 及其独立测试中完成，不应把整套策略塞回本文件。行级聚合能力则属于 `pkg/executor/physical_plan_runtime.rs` 及 executor 测试。
- Rust 测试继续放在独立文件 `physical_hash_agg_aster_unit_test.rs`（并由 `lib.rs` 的 `#[cfg(test)] mod` 注册），不要在生产源文件内新增测试模块。

兼容性风险集中在 Explain 类型名、成本选择、MPP 阶段/分区、TiPB 字段和聚合描述符深拷贝；性能风险集中在错误的并发摊销及 PB/计划克隆开销。任何改动都应与 Go 同路径实现逐项核对，而不是只验证编译成功。

## 验证依据

- 目标源码：`pkg/planner/core/operator/physicalop/physical_hash_agg.rs`，RustCodeGraph 显示 258 行、19 个符号，并列出 executor、Cascades 和 planner 等使用者。
- crate 与模块边界：`pkg/planner/core/operator/physicalop/Cargo.toml`；`pkg/planner/core/operator/physicalop/lib.rs` 的模块声明、再导出、`agg_operator_core!(PhysicalHashAgg)` 和独立测试注册。
- 构造与候选调用边：`pkg/planner/core/operator/physicalop/base_physical_agg.rs::ExhaustPhysicalPlans4LogicalAggregation`；`pkg/planner/cascades/old/implementation_rules.rs::ImplHashAgg::OnImplement`。RustCodeGraph 查询确认 `NewPhysicalHashAgg` 的调用者包括这两个入口。
- 本地执行边界：`pkg/executor/physical_plan_runtime.rs::execute_node` 对 `PhysicalHashAgg` 的 downcast 分支及标量聚合辅助逻辑。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_hash_agg.go` 的 `PhysicalHashAgg`、`getHashAggs`、`tryToGetMppHashAggs`、成本/Task/PB 方法和 `NewPhysicalHashAgg`。
- 独立 Rust 测试：`pkg/planner/core/operator/physicalop/physical_hash_agg_aster_unit_test.rs`，直接覆盖描述符克隆、CPU 除数和 TiFlash 模式；同目录 `canonical_router_aster_unit_test.rs` 与 `cache_snapshot_test.rs` 还覆盖候选路由和计划快照中的 `PhysicalHashAgg`。
- 未运行 Cargo：总计划和任务明确规定纯文档分析不运行 Cargo。本说明只做源码、代码图、Cargo/Go/测试证据核对以及固定章节结构验证。
