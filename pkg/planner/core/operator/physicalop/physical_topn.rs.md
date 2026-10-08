# `pkg/planner/core/operator/physicalop/physical_topn.rs`

## 文件定位

本文件实现逻辑 `LogicalTopN` 的 Rust 物理算子、候选计划枚举和存储下推序列化。它属于 `astersql-planner-core-operator-physicalop` crate；crate 的 [`Cargo.toml`](Cargo.toml) 以 `lib.rs` 为入口，并通过 `lib.rs` 中的 `mod physical_topn` 与 `pub use physical_topn::*` 对外暴露本文件符号。`impl_sort_operator!(PhysicalTopN, PhysicalSchemaProducer, {})` 又把本文件的方法接入 `ConcretePhysicalOperator`/`PhysicalPlan` 的统一动态分派。

在传统物理计划枚举主链中，`base_physical_plan.rs` 将 `LogicalTopN` 路由到 `ExhaustPhysicalPlans4LogicalTopN`；该函数返回两组候选：真正需要排序的 `PhysicalTopN`，以及子节点已能满足顺序时的 `PhysicalLimit`。运行时的 `pkg/executor/physical_plan_runtime.rs` 识别 `PhysicalTopN`，递归执行唯一子节点，再以 `sortexec::TopNExec` 执行 offset/count TopN。另一路 `pkg/planner/cascades/old/implementation_rules.rs::ImplTopN` 也会直接构造本类型。

## 核心职责

- `PhysicalTopN` 保存排序键、分区键、`OFFSET`/`LIMIT` 和前缀索引优化元数据，并借 `PhysicalSchemaProducer` 持有 schema、统计信息、上下文、子节点及所需物理属性。
- `ExhaustPhysicalPlans4LogicalTopN` 在父属性与逻辑排序项匹配时，分别生成 TopN 候选和保序 Limit 候选，以便每组先选优后再按代价比较；不匹配时没有候选。
- `getPhysTopN` 枚举 CopSingleRead、CopMultiRead、Root，以及会话允许时的 MPP 任务属性；它还生成前缀索引部分有序、IndexMerge advisory sort 和向量 TopK 的专用候选。
- `getPhysLimits` 在 `GetPropByOrderByItems` 能把排序表达式转成物理顺序属性时，用 `PhysicalLimit` 要求子节点直接供给有序数据，避免本节点排序。
- `ToPB` 把排序、分区和数量编码成 `tipb::TopN`；TiFlash 路径额外递归嵌入子执行器并设置 executor id。
- `Clone`、`ResolveIndices`、Explain、内存与代价方法使该节点可以安全进入计划复制、列索引绑定、展示、缓存和优化器成本比较流程。

## 主要符号

- `pub struct PhysicalTopN`：公开物理节点。`ByItems: Vec<planner_util::ByItems>` 表示表达式及升降序；`PartitionBy` 表示分区内 TopN；`Offset`/`Count` 表示跳过与保留行数；`PrefixCol`/`PrefixLen` 记录前缀索引短路信息。
- `PhysicalTopN::New(ctx, offset, count)`：建立带 `TypeTopN` 基础计划的空骨架；排序/分区键为空，前缀优化关闭。
- `PhysicalTopN::Init(self, ctx, stats, offset, props)`：重建基础计划，写入统计信息、query block offset 和每个子节点所需的 `PhysicalProperty`。
- `Clone(new_ctx)`：通过 `CloneWithNewCtx` 换绑上下文，并逐项深拷贝 schema、排序表达式、分区列和前缀列；错误来自基础计划/表达式复制。
- `ExtractCorrelatedCols`：只从 `ByItems` 的表达式抽取相关列；当前不从 `PartitionBy` 或 `PrefixCol` 抽取。
- `ExplainInfo` / `ExplainNormalizedInfo`：前者尊重日志脱敏的 disable/marker/enable 三态并显示 offset/count/可选前缀信息；后者仅保留规范化后的分区和排序表达式，不输出具体分页数值。
- `GetCost(count, root)`：以 `max(offset + count, 2)` 为堆大小，估算 `count * log2(heap) * CPUFactor + heap * MemoryFactor`；root 与 cop 使用不同 CPU 因子。加法采用 `wrapping_add`，这是当前代码事实。
- `ResolveIndices`：先解析 producer，再以第一个子节点 schema 重绑排序表达式、分区列和前缀列；无子节点时 producer 成功后直接返回。
- `Attach2Task`：转交 `base::PhysicalPlan::attach_to_task`，本文件不自行实现 root/cop/mpp 挂接细节。
- `GetPlanCostVer1` / `GetPlanCostVer2`：转交 `BasePhysicalPlan` 的统一成本接口。
- `ToPB(ctx, store)`：构造 `tipb::Executor(TypeTopN)`；表达式不可下推、缺少 PB client 或 TiFlash 缺少子节点时返回错误。
- `ExhaustPhysicalPlans4LogicalTopN`：本文件唯一公开的候选枚举函数。
- 私有辅助函数 `new_topn`、`getPhysTopN`、`getPhysLimits`、`canUsePartialOrder4TopN`、`checkPartialOrderPattern`、`interpret_vector_search`：分别负责节点复制初始化、两类候选生成、部分有序形态检查和向量距离表达式识别。

## 执行流程

1. `base_physical_plan.rs` 发现 `LogicalTopN` 后调用 `ExhaustPhysicalPlans4LogicalTopN(logical, required)`。如果 `MatchItems(required, logical.ByItems)` 失败，立即返回空候选。
2. `getPhysTopN` 首先检查部分有序前缀索引机会。会话开关必须开启，`ByItems` 非空，且子树只能是以 `DataSource` 结尾、途中仅含单子节点 `LogicalSelection`/`LogicalProjection` 的链；所有排序表达式还必须能下转为简单 `Column`。满足时优先加入带 `CopMultiReadTaskType` 和 `PartialOrderInfo` 的候选。
3. 函数按固定顺序加入 CopSingleRead、CopMultiRead、Root 候选；MPP 允许时追加 MPP。每个子属性把 `ExpectedCnt` 设为 `f64::MAX`，并继承 `CTEProducerStatus`、`NoCopPushDown`。直接子节点是 `DataSource` 且排序项能转换为属性时，CopMultiRead 再得到一个带 `AdvisorySortItems` 的额外候选。
4. MPP 开启且排序恰为单个升序向量距离表达式、直接子节点是无下推过滤的 `DataSource` 时，`interpret_vector_search` 验证函数名、一个 vector-float32 列参数和一个 vector-float32 常量参数，随后生成 `VectorProp`，其中 `TopK = (Count + Offset) as u32`。
5. `new_topn` 从当前第一个逻辑子节点统计重新调用 `DeriveLimitStats(child_stats, Count)`，避免改写后沿用陈旧逻辑统计；缺少子统计时回退逻辑节点统计，最后回退默认值。它深拷贝逻辑字段和 schema，再以单个 child property 初始化物理节点。
6. `getPhysLimits` 将排序项转成 `SortItems`，为 CopSingleRead、CopMultiRead、Root 各生成一个 `PhysicalLimit`。子属性 `ExpectedCnt` 为 `Count + Offset`，Limit 同步参数化 offset/count 与 `PartitionBy`；此路径不枚举 MPP。
7. 选中的 `PhysicalTopN` 在 root Rust 运行时由 `TopNExec` 消费；下推时由 `ToPB` 编码给存储层。TiFlash PB 采用树形 executor，因而必须有一个可递归序列化的子节点；非 TiFlash PB 只携带 TopN 本身。

## 数据与状态

节点自身是可变计划数据，不含全局单例。`PhysicalSchemaProducer` 是主要组合对象，承载 `BasePhysicalPlan`、schema、统计和 children；本文件的字段决定排序语义和候选优化提示。`ByItems` 中表达式使用 trait object，克隆时由各表达式的 `Clone` 实现深拷贝；`PartitionBy` 与 `PrefixCol` 同样显式克隆，避免不同计划候选共享可变列索引状态。

`MemoryUsage` 汇总 producer、每个排序项、每个分区项、可选前缀列，以及两个 `u64` 和一个 `usize` 的标量大小。它没有计入 `Vec` 容量/分配头的独立固定成本，因此与 Go 版本使用 slice capacity 和指针大小的估算不是逐字节等价。

缓存生命周期由相邻 `cache_snapshot.rs::CachedTopN::{capture, restore}` 负责：它保存 producer、排序/分区项、offset/count、前缀列与长度，并在恢复时用新 context 重建表达式。这证明上述字段都属于计划缓存正确性所需状态，而非仅 Explain 元数据。

## 依赖与调用关系

上游入口包括：

- `base_physical_plan.rs` 的逻辑计划路由调用 `ExhaustPhysicalPlans4LogicalTopN`，并把两组候选展平交给统一优化流程。
- `pkg/planner/cascades/old/implementation_rules.rs::ImplTopN::OnImplement` 通过 `New`/`Init` 构造 TiDB 或 TiKV implementation；其 `TopNCostPlan` 适配器调用 `GetCost`。
- `cache_snapshot.rs::CachedTopN::restore` 直接重建 `PhysicalTopN`；`cache_snapshot_test.rs` 验证往返后 offset/count、前缀长度和排序方向保持不变。

主要下游依赖包括：

- `property`：统计裁剪、任务类型、顺序/部分顺序/advisory/vector 属性，以及分区 Explain。
- `expression`：相关列抽取、索引解析、Explain、列/常量/标量函数识别和 PB 排序项转换。
- `planner_util`：`ByItems` 的复制、内存计算和 Explain。
- `base`/本 crate：计划上下文、统一 trait、任务挂接、schema producer 与 `PhysicalLimit`。
- `tipb`/`protobuf`/`kv`：TopN executor protobuf、枚举转换和 TiFlash/TiKV 存储类型。
- `pkg/executor/physical_plan_runtime.rs`：root 路径把 `ByItems` 转为运行时 sort keys，并以 `Offset`/`Count` 创建 `TopNExec`。当前这段本地执行代码没有消费 `PartitionBy`、`PrefixCol` 或 `PrefixLen`；这些字段主要服务计划属性、下推与优化流程，若要宣称 root 运行时支持其语义需另行验证。

RustCodeGraph 对 `PhysicalTopN` 给出的显式实例化边包括 `cache_snapshot.rs::restore`；对动态 trait/宏分派的 callers/callees 覆盖不完整，因此以上其余关系由具体路由、宏展开入口和消费点源码核对。

## 错误处理与边界

- 候选枚举多用“无候选”表达不适用：父顺序不匹配、缺少 plan context、排序项不能转列、向量表达式形态/类型不符、子树形态不符时返回空集合或跳过专用候选，而不是报错。
- `new_topn` 缺上下文时返回 `None`；缺统计时使用默认统计。这保证枚举继续，但默认统计可能降低成本判断精度。
- `ResolveIndices` 会传播 producer、排序表达式、分区列或前缀列的解析错误；若没有第一个子节点则不解析自身字段并返回成功，因此调用方仍应维持 TopN 的一元节点不变量。
- `ToPB` 显式拒绝缺少 client、不可下推的 order/partition 表达式和 TiFlash 无子节点。非 TiFlash 路径不读取 child。
- `Offset + Count` 在成本、Limit expected count 和 vector TopK 处分别使用 wrapping 加法/普通 Rust 加法后转换；极端 `u64` 值可能回绕、在带溢出检查构建中失败或截断为 `u32`。本文件未额外校验这些边界，修改时应与 Go 的无符号加法/转换语义一起评估。
- `interpret_vector_search` 只接受四种距离函数名，并要求恰能找到一个 vector-float32 列和一个 vector-float32 常量；额外的非列/常量参数不会单独触发拒绝，但最终仍必须同时得到列和向量。过滤条件非空、降序或非直接 `DataSource` 都禁用向量候选。
- Explain 的脱敏分支是用户可见兼容面；新增字段时必须同步普通、marker 和完全脱敏三种输出，不能泄露值。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。`Arc` 只用于向量候选中共享常量向量数据；plan context 本身也以 `ContextRef` 共享，其并发保证由上下文实现提供，本文件不加锁。

计划对象的生命周期是“枚举候选 → 成本比较/改写与索引解析 → root 执行或 PB 下推”。候选之间通过显式 Clone 隔离表达式和列索引状态。PB 构建只分配 protobuf 对象；TiFlash 子 executor 的所有权被移入 `tipb::TopN`。root 执行器生命周期由 `physical_plan_runtime.rs` 管理，本文件只提供计划描述。

## 与 Go 版本的对应关系

直接对照文件是同目录 [`physical_topn.go`](physical_topn.go)。Rust 保留了 Go 的字段集合、两组候选设计、固定 task-type 枚举顺序、部分有序前缀索引、IndexMerge advisory 属性、向量 TopK、Explain 脱敏、成本公式、PB 结构和 TopN-as-Limit 思路。

已确认的实现差异：

- Go `ExhaustPhysicalPlans4LogicalTopN` 还返回 `ok` 与 `error`；Rust 以空/非空 `Vec<Vec<_>>` 表达结果。
- Go 候选直接复用 `lt.StatsInfo()`；Rust `new_topn`/`getPhysLimits` 优先根据当前 child stats 重新 `DeriveLimitStats`，独立 Rust 测试明确覆盖了这一迁移语义。
- Go 的成本、ResolveIndices 与 Attach2Task 经 `utilfuncp` 函数指针回调 core 包；Rust 本文件分别转交 `BasePhysicalPlan` 或通用 `attach_to_task`，自身 `ResolveIndices` 则直接实现字段解析。不能仅凭同名方法假设回调内部逐分支完全一致。
- Go `ToPB` 默认假设 client/表达式转换可用；Rust 将缺 client 和不可下推表达式显式转为 `expression::Error`，并检查 TiFlash child。
- Go `MemoryUsage` 计入 slice capacity/指针等，Rust 按元素及标量估算；两者只保证用途一致，不保证数值一致。
- Go 的向量识别调用公共 `expression.InterpretVectorSearchExpr`；Rust 在本文件用 `interpret_vector_search` 实现受限识别逻辑。扩展支持的函数或表达式形态时必须两边同步。
- Go `Clone` 的可见代码主要显式复制 `ByItems`、`PartitionBy`；Rust 还明确克隆 `PrefixCol` 并保留 `PrefixLen`。相邻 Rust cache snapshot 测试覆盖前缀字段持久化。

## 扩展指南

- 新增 TopN 状态字段时，应同步 `PhysicalTopN::{New, Clone, MemoryUsage, ExplainInfo}`、必要的 `ResolveIndices`/`ToPB`，以及 `cache_snapshot.rs::CachedTopN` 的 capture/restore 和 `lib.rs` 中的缓存契约描述；如果源自逻辑节点，还要同步 `new_topn` 与 TopN-as-Limit 路径。
- 新增候选策略应放在 `getPhysTopN` 或 `getPhysLimits`，并保持候选分组语义与 task 枚举顺序；同时检查 `base_physical_plan.rs` 的 attach/pushdown 规则和 cascades `ImplTopN` 是否需要等价接线。
- 扩展向量检索时，优先修改 `interpret_vector_search` 的函数白名单、参数类型/唯一性验证，并在 `physical_topn_test.rs` 增加成功及拒绝用例；还需与 Go 公共解释函数的行为对齐。
- 修改下推编码时，在 `ToPB` 保留错误上下文和 TiFlash 一元子节点要求，并扩充 `physical_topn_test.rs`；若字段参与 plan cache，再扩充 `cache_snapshot_test.rs`。
- 修改 root 执行语义时应同步检查 `pkg/executor/physical_plan_runtime.rs` 与 sortexec 的独立测试，尤其是 `PartitionBy`、前缀优化、空输入、offset 超过输入、count 为零以及溢出边界。
- 测试必须继续放在独立文件；本模块现有直接测试是 `physical_topn_test.rs`，缓存复制测试在 `cache_snapshot_test.rs`。不要把 `#[cfg(test)]` 测试内嵌到生产源文件。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；查询定位到 Rust/Go 两个 `PhysicalTopN` 定义，并显示 Rust 类型被 `cache_snapshot.rs::restore` 实例化。读取了索引中的 `physical_topn.rs` 全部 631 行和 `physical_topn_test.rs` 全部 165 行；图对宏与动态分派调用边无完整结果，已用下列源码补证。
- 目标源码：`physical_topn.rs` 的 `PhysicalTopN`、全部公开方法、`ExhaustPhysicalPlans4LogicalTopN` 与六个私有辅助函数。
- crate/trait 接线：`Cargo.toml`、`lib.rs` 的模块声明、公开重导出、`impl_sort_operator!` 及 TopN 缓存契约。
- 上下游证据：`base_physical_plan.rs` 的 `LogicalTopN` 路由与 reader pushdown，`pkg/planner/cascades/old/implementation_rules.rs` 的 TopN implementation/cost adapter，`pkg/executor/physical_plan_runtime.rs` 的 `TopNExec` 消费点，以及 `cache_snapshot.rs::CachedTopN`。
- Go 对照：同目录 `physical_topn.go`，覆盖字段、候选生成、部分有序、向量、Explain、PB、成本、索引解析与任务挂接接口。
- 独立 Rust 测试：`physical_topn_test.rs::tiflash_protobuf_carries_explain_id_like_go` 验证 TiFlash PB executor id；`mpp_allowed_enumerates_all_go_task_types_for_a_root_request` 验证四种任务类型顺序及重算后的统计。`cache_snapshot_test.rs` 还验证 TopN 字段缓存往返。
- 本任务是纯文档分析，按任务约束不运行 Cargo。交付前以任务指定命令验证目标文件存在且恰有十一个固定二级标题，并人工复核本文能回答文件存在原因、主流程、安全扩展点及已知验证边界。
