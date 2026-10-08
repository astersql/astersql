# `pkg/planner/core/operator/physicalop/physical_cte.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate，是 CTE（Common Table Expression，公用表表达式）的物理计划模型。`pkg/planner/core/operator/physicalop/Cargo.toml` 将 crate 根指定为 `lib.rs`，后者以 `pub mod physical_cte` 声明并以 `pub use physical_cte::*` 再导出这里的公开类型。

当前代码包含两组用途不同、不能混为一谈的模型：

- `PhysicalCTEDefinition`、`PhysicalCTE`、`PhysicalCteScan` 接入 `base::PhysicalPlan` trait，参与规范优化器、计划遍历和执行资源统计等真实主链。
- `PhysicalCte`、`PhysicalCteStorage`、`PhysicalCteSink`、`PhysicalCteSource`、`CteExecutor` 和 `exhaust_physical_cte` 基于 `physical_common_plans::PhysicalPlanNode`，是保留 Go 语义的兼容数据模型。它覆盖关联列、EXPLAIN、属性传递和简化 sink/source 编码，但部分能力仍是占位或简化实现。

文件开头第 21—104 行还有一段被注释掉的早期迁移草图；它不是可执行代码，理解现状应以第 107 行之后的定义以及 `lib.rs` 中的 trait 接线为准。

## 核心职责

1. 用 `PhysicalCTEDefinition` 持有一个共享 CTE 的 storage ID、种子物理计划和可选递归物理计划；用 `Arc<PhysicalCTEDefinition>` 让多个 `PhysicalCTE` 引用同一份定义。
2. 用 `PhysicalCteScan` 表示主计划树中的 CTE 消费叶子。规范寻优路由在 `base_physical_plan.rs` 中为 `LogicalCTE` 和 `LogicalCTETable` 构造它，而共享生产者树单独呈现。
3. 用兼容模型 `PhysicalCte` 表达种子/递归树、`distinct` 和三段统计信息，并提供关联列提取、EXPLAIN、内存估算和访问对象文本。
4. 用 `PhysicalCteSink`/`PhysicalCteSource` 表达 TiFlash MPP 中对同一临时存储的写端和读端，携带 fragment 切分后得到的本地 source/sink 数量，并生成简化的 `CteExecutor`。
5. 用 `exhaust_physical_cte` 生成一个 `CteStorage` 候选并原样保存所需物理属性和统计信息。

## 主要符号

- `PhysicalCTEDefinition`：typed 共享定义。`New(ctx, storage_id, seed, recursive)` 创建 `TypeCTEDefinition` 节点；`Clone(ctx)` 克隆 schema producer、种子计划及递归计划，保留 storage ID。
- `PhysicalCTE`：typed 主树引用。`New` 创建 `TypeCTE` 节点并保存 `Arc`；`Clone` 只克隆自身 producer，上游共享定义继续通过 `Arc::clone` 共享。
- `PhysicalCteScan`：typed 消费叶子。`New(ctx, tp, explain, schema)` 安装类型、说明文本和 schema；`Clone` 复制 schema 与文本；`ResolveIndices`、两版代价计算委托给 `PhysicalSchemaProducer`/`BasePhysicalPlan`；它没有关联列。
- `PhysicalCte`：兼容生产者模型，包含 `id_for_storage`、可选 seed/recur 树、递归去重标志以及 seed/recursive/result 三份统计。
- `collect_correlated`、`collect_correlated_expr`：深度遍历计划孩子以及 Selection/Projection 表达式；标量函数参数也递归检查，最终收集 `CorrelatedColumn` ID。函数不做去重，重复出现会重复返回。
- `CteDefinition`：兼容 EXPLAIN 视图，按是否存在递归计划返回 `Recursive CTE` 或 `Non-Recursive CTE`，ID 为 `CTE_<storage-id>`。
- `PhysicalCteStorage`：兼容 producer 包装；`explain_info` 固定为 `Non-Recursive CTE Storage`，`attach_to_task` 当前直接返回输入任务。
- `PhysicalCteSink`：MPP 写端。自定义 `Clone` 保留配置、计数和 child，但清空 `self_tasks` 与 `target_tasks`，避免复制旧任务拓扑；`to_pb` 从 child schema 和自身字段构造 sink 描述。
- `PhysicalCteSource`：MPP 读端，无 child；`to_pb` 从自身 schema 和计数字段构造 source 描述，压缩模式留空。
- `CteExecutor`：仅覆盖 sink/source、storage ID、局部计数、字段列 ID 和压缩模式的简化执行描述，并非完整 `tipb::Executor`。
- `exhaust_physical_cte`：返回恰好一个无孩子、空 schema 的 `PhysicalKind::CteStorage` 节点以及 `true`，把传入 `Stats` 和 `PhysicalProperty` 完整放入候选。

## 执行流程

typed 路径的主流程如下：

1. 上游创建 `PhysicalCTEDefinition::New`，把 seed 与可选 recursive 计划作为独立根保存，并为定义分配 `TypeCTEDefinition` 基座。
2. 每个 CTE 引用用 `PhysicalCTE::New` 持有同一个 `Arc<PhysicalCTEDefinition>`；因此多个消费者共享定义和 storage ID，而不是把生产者树复制进每个主计划分支。
3. `base_physical_plan.rs` 的规范任务路由遇到 `LogicalCTE`/`LogicalCTETable` 时，检查任务类型和排序约束，组装包含 CTE 名、别名及 storage ID 的 EXPLAIN 文本，创建 `PhysicalCteScan`，设置统计并放入 `RootTask` 和规范任务缓存。
4. `lib.rs` 中的 `ConcretePhysicalOperator` 实现把这三类 typed 节点接到统一 `PhysicalPlan` trait：scan 支持索引解析和 producer 代价；引用与定义节点提供 EXPLAIN、内存和 producer 代价委托。
5. 下游计划遍历可识别共享定义。例如 `pkg/executor/statement_ru_plan_walk.rs` 要求 `PhysicalCTEDefinition` 有一至两个孩子，第一项标为 seed，第二项（若存在）标为 recursive。

兼容路径的流程是：`exhaust_physical_cte` 生成 storage 候选；`PhysicalCte` 保存 seed/recur 结构并可提取相关列；MPP fragment 侧使用 sink/source 共享 storage ID 和局部数量；最后各自 `to_pb` 生成 `CteExecutor`。注意这里生成的是本文件的简化结构，不等价于 Go 端递归调用 child `ToPB` 并构造完整 tipb executor 的过程。

## 数据与状态

- `IDForStorage`/`id_for_storage` 是定义、引用、sink、source 之间的关联键。兼容编码将 `i64` 直接转换为 `u32`，本文件没有负数或溢出校验。
- typed 定义拥有 `Box<dyn base::PhysicalPlan>` seed 和可选 recur；typed 引用通过 `Arc` 共享定义。`PhysicalCTEDefinition::Clone` 深克隆计划，`PhysicalCTE::Clone` 则刻意保持共享定义。
- 兼容 `PhysicalCte` 里的三个 `Stats` 字段彼此独立；当前本文件只存储并比较它们，没有在这里推导或合并统计。
- `PhysicalCteSink.self_tasks` 和 `target_tasks` 是拓扑状态。克隆时两者归零，而 `cte_sink_num`、`cte_source_num` 保留，测试把这一行为作为不变量。
- `PhysicalCteScan.ExplainText` 是预组装文本；clone 复制它，`ExplainInfo` 原样返回。
- 内存估算并不统一：兼容节点显式累计结构体、vector capacity 和子树；typed `PhysicalCTE`/`PhysicalCTEDefinition` 在 `lib.rs` 的 trait 接线中目前只报告 producer 的内存，没有累计共享定义或 seed/recur 树。

## 依赖与调用关系

上游与接线：

- `physicalop/lib.rs` 公开本模块，并为 `PhysicalCteScan`、`PhysicalCTE`、`PhysicalCTEDefinition` 实现 `ConcretePhysicalOperator` 后通过宏实现 `PhysicalPlan`。
- `base_physical_plan.rs` 的规范寻优路径创建 `PhysicalCteScan`；它是当前最直接的生产调用者。
- `pkg/executor/statement_ru_plan_walk.rs` 对 `PhysicalCTEDefinition` 的 seed/recursive 孩子形状进行资源使用遍历。
- `pkg/planner/core/flat_plan.rs`、`pkg/planner/core/optimizer_runtime.rs` 等文件也按具体类型识别 CTE 节点；RustCodeGraph 报告目标文件被 8 个文件使用。

下游依赖：

- `BasePhysicalPlan`、`PhysicalSchemaProducer` 提供统一计划 ID、上下文、schema、代价、clone 和索引解析能力。
- `base::PhysicalPlan` 是 typed seed/recur 的动态接口；`expression::Error` 是 typed clone/解析/代价委托的错误类型。
- `physical_common_plans::{PhysicalPlanNode, PhysicalKind, PhysicalExpr, PhysicalProperty, Stats}` 支撑兼容模型。
- `plancodec::TypeCTE` 与 `TypeCTEDefinition` 决定 typed 节点类型标签；`property::TaskType`、`costusage::{PlanCostOption, CostVer2}` 进入代价接口。

crate 依赖由 `pkg/planner/core/operator/physicalop/Cargo.toml` 声明；本文件直接涉及的 `base`、`costusage`、`expression`、`property`、`plancodec` 均为 workspace path 依赖。Cargo 无针对本模块的 feature gate，测试由 `lib.rs` 中 `#[cfg(test)] mod physical_cte_test;` 独立装配。

## 错误处理与边界

- typed `Clone`、`ResolveIndices`、代价方法返回 `expression::Error` 并用 `?` 原样传播；递归计划的 `Option<Result<_>>` 通过 `transpose` 转换，任一子计划克隆失败都会使整个定义克隆失败。
- `PhysicalCteSink::to_pb` 的签名是 `Result<CteExecutor, String>`，但当前函数体没有可能失败的分支；`PhysicalCteSource::to_pb` 直接返回值。这与 Go 版字段类型转换和 child PB 构造可能失败的行为不同。
- `PhysicalCte::plan_cost_v2` 固定返回 `0.0`，不能作为完整成本模型；typed 路径则委托基座代价接口。
- `PhysicalCteStorage::attach_to_task` 不构造或改写计划，只透传输入任务，属于兼容占位行为。
- 关联列扫描只识别 Selection/Projection 中的 `PhysicalExpr`，其他 `PhysicalKind` 自有表达式若未来加入，不会自动被扫描。
- sink 假定 `child` 已存在；source 按设计无 child。兼容 `to_pb` 对 storage ID 的 `i64 as u32` 转换要求调用者保证值域合法。
- `exhaust_physical_cte` 本身不拒绝排序、任务类型或分区属性，而是完整保存属性；真实 typed 路由的约束检查位于 `base_physical_plan.rs`。

## 并发与资源生命周期

- `Arc<PhysicalCTEDefinition>` 提供线程安全引用计数并表达定义共享所有权；本文件不在定义内部使用锁或可变共享状态。多个 `PhysicalCTE` clone 继续指向同一对象。
- seed/recur 是定义拥有的 boxed 计划根，随定义最后一个 `Arc` 释放；调用 `PhysicalCTEDefinition::Clone` 会产生独立的新计划树，而不是共享原 boxed 子树。
- 兼容 MPP 任务 vector 由 sink 独占。自定义 clone 清空任务 ID，防止一个新节点沿用旧 fragment 的瞬态拓扑；局部 source/sink 数量作为已计算配置保留。
- 文件没有启动异步任务、线程或通道，也没有直接持有实际 CTE 临时存储。真正的运行时存储初始化、fragment 数量填充和执行生命周期位于其他模块；本文件只保存计划期标识与描述。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/physicalop/physical_cte.go`。

- Go `PhysicalCTE` 将 seed、recur、`CTEClass`、名称和别名放在同一结构中；Rust typed 路径拆成共享 `PhysicalCTEDefinition` 与主树引用 `PhysicalCTE`，并另设消费叶子 `PhysicalCteScan`。这是所有权模型上的重构，不是字段逐一同构。
- Go `PhysicalCTE.Clone` 深克隆 seed/recur、共享 `CTEClass`；Rust 将相应语义分开：定义 clone 深克隆计划，引用 clone 保持 `Arc` 共享。
- Go `CTEDefinition.ExplainInfo` 还处理 LIMIT 和日志脱敏；兼容 Rust `CteDefinition` 目前只报告是否递归。Go 的访问对象使用 CTE 名/别名，兼容 Rust `PhysicalCte::access_object` 只显示 storage ID；typed scan 的名称/别名文本在路由处补足。
- Go `PhysicalCTEStorage.Attach2Task` 调用专用挂接函数，Rust 兼容 `attach_to_task` 仅透传；Go 成本委托 `GetPlanCostVer24PhysicalCTE`，Rust 兼容成本固定为零。这两处均是明确的迁移缺口。
- Go sink/source `ToPB` 构造完整 `tipb::Executor`、转换字段类型，并由 sink 递归构造 child PB；Rust 兼容 `CteExecutor` 只保存字段 ID 和少量元数据。两者的 local source/sink 计数、storage ID、sink 压缩信息意图一致。
- `PhysicalCteStorage::explain_info` 的固定字符串、`CteDefinition` 的递归判断及 `CTE_<id>` 格式由独立 Rust 测试与 Go 实现互相核对。

## 扩展指南

- 若扩展真实优化/执行主链，优先修改 typed 类型及 `physicalop/lib.rs` 的 `ConcretePhysicalOperator` 接线，并同步检查 `base_physical_plan.rs` 的规范路由、flat plan 和执行器遍历；不要只增强兼容 `PhysicalPlanNode` 模型。
- 若增加需要提取关联列的算子表达式，应扩展 `collect_correlated` 的 `PhysicalKind` 分支，并在独立的 `physical_cte_test.rs` 添加嵌套及重复列用例。
- 若补全兼容成本或 `attach_to_task`，应对齐 Go 的 `GetPlanCostVer24PhysicalCTE` 和 `Attach2Task4PhysicalCTEStorage`，不能以常量成本或无操作挂接替代；还需验证任务类型、排序、MPP 分区与 enforcer 边界。
- 若把 `CteExecutor` 升级为真实 tipb 编码，需要加入字段类型转换、child executor、executor ID 及错误传播，并验证 storage ID 值域。Cargo 已声明 `tipb` 和 `protobuf`，但当前结构并未直接使用它们。
- 修改 clone 时必须保持所有权约定：typed 引用共享定义、typed 定义深克隆计划、兼容 sink 清空瞬态任务列表。相应回归测试应继续放在同目录独立文件 `physical_cte_test.rs`，不要嵌入生产源文件。
- 兼容模型与 typed 模型存在语义重复；扩展前先确定调用主链，避免只更新其中一套而造成 EXPLAIN、成本或 PB 行为漂移。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引时间戳为 `1791342965170`；`files --filter pkg/planner/core/operator/physicalop` 确认目标、Go 对照和独立测试均已索引。
- RustCodeGraph：`explore "pkg/planner/core/operator/physicalop/physical_cte.rs PhysicalCTE PhysicalCTETable PhysicalCTETableScan"` 给出目标上下文和调用爆炸半径；`node --file .../physical_cte.rs` 核对全部 502 行；`query` 精确定位 `PhysicalCTEDefinition`、`PhysicalCteScan` 和 `exhaust_physical_cte`。单独的 `callers` 查询在本次运行中未在 30 秒内返回结果，因此调用关系又由已索引文件的精确符号引用核验。
- Rust 源与接线：`pkg/planner/core/operator/physicalop/physical_cte.rs`、`physical_common_plans.rs`、`base_physical_plan.rs`、`lib.rs`。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml`。目标包没有 `doc.go`；最近的有效模块契约是 Rust crate 根 `lib.rs`，已读取。
- Rust 独立测试：`pkg/planner/core/operator/physicalop/physical_cte_test.rs`，覆盖共享定义、递归说明、嵌套标量关联列、属性保留以及 sink PB/clone 行为；`pkg/executor/statement_ru_plan_walk_test.rs` 另有 typed 定义的集成式使用证据。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_cte.go`；MPP fragment 的 Go 流程和测试文件进一步说明 local sink/source 数量由 fragment split 后填充。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构通过任务指定的 11 章节检查验证。
