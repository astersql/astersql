# `pkg/planner/core/operator/physicalop/physical_table_reader.rs`

## 文件定位

本文件属于 Cargo crate `astersql-planner-core-operator-physicalop`（同目录 `Cargo.toml`），定义根任务中的物理表读取边界 `PhysicalTableReader`：它把一棵可下推到 TiKV/TiFlash 的 `TablePlan` 包装成 planner 可统一处理的物理算子，并把输出 schema、统计、代价、解释信息和 protobuf 编码能力暴露给优化器与执行器。

`lib.rs` 以 `mod physical_table_reader` 和 `pub use physical_table_reader::*` 导出本文件，并通过 `ConcretePhysicalOperator for PhysicalTableReader` 与 `impl_concrete_physical_plan!` 接入 `base::PhysicalPlan`。这里的真实孩子保存在 `TablePlan`，不是 `BasePhysicalPlan.Children`；适配层的 `children_operator`、`set_children_operator` 和 `attach_operator_to_task` 负责让通用计划树操作仍看到这棵子树。

## 核心职责

- 保存存储侧下推树 `TablePlan`，并使 reader 的 schema 与子树根同步。
- 用 `StoreType` 与子树形状选择 `Cop`、`BatchCop` 或 `MPP` 请求类型；TiFlash `PhysicalExchangeSender` 对应 MPP，含聚合/TopN 且无有序扫描的 TiFlash 子树可选择 BatchCop。
- 遍历下推树定位 `PhysicalTableScan`，为访问对象、单扫描约束、MPP 标志检查等功能提供依据。
- 提供 clone、相关列提取、列下标解析、EXPLAIN、cost、PB 编码、网络数据量与内存估算等计划接口。
- 作为优化器与执行器之间的类型化边界：优化器先创建 reader 模板并在挂接 task 时装入实际子树；执行器从 `GetTablePlan` 继续递归构建存储侧执行计划。

当前实现含明显的迁移中行为，不能据此声称 Go 版本能力已全部等价：`LoadTableStats` 只遍历 scan，没有加载统计；动态分区访问对象、会话级 BatchCop 开关、专用 reader cost 和真实平均行宽估算尚未完整对应。

## 主要符号

### `ReadReqType`

公开枚举有 `Cop`、`BatchCop`、`MPP` 三种值，默认值为 `Cop`。`Name(self)` 分别返回 `cop`、`batchCop`、`mpp`，用于计划展示和诊断；与 Go 不同，Rust 对未知枚举值无需提供默认分支，因为枚举取值封闭。

### `PhysicalTableReader`

- `PhysicalSchemaProducer`：保存 `BasePhysicalPlan`、正式输出 schema、context、plan id/type、统计和孩子物理属性要求。
- `TablePlan: Option<Box<dyn PhysicalPlan>>`：存储侧下推树的唯一根；空值表示 reader 尚未完成接线。
- `StoreType`：目标存储类型，`New` 默认为 TiKV。
- `ReadReqType`：实际读请求模式，`New` 默认为 Cop。
- `IsCommonHandle`：记录 clustered index/common handle 语义；本文件只保存和克隆该标志。
- `PlanPartInfo`：分区裁剪相关计划信息；clone、计划缓存和内存估算会处理它，但本文件不执行分区裁剪。

### 构造、克隆与工厂

- `New(ctx)` 创建 `TypeTableReader`、query block offset 为 0、TiKV/Cop、无子计划的 reader。
- `Init(self, ctx, offset)` 重设 context、类型和查询块偏移；有 `TablePlan` 时同步 schema 并调用 `adjust_read_request_type`。该私有方法只识别 TiFlash + `PhysicalExchangeSender` 为 MPP，其余为 Cop。
- `Clone(&self, new_ctx)` 克隆基座、schema、整棵动态子计划、存储/请求标志和分区信息；子计划克隆失败会返回 `expression::Error`。
- `GetPhysicalTableReader(ctx, schema, stats, props)` 创建优化器使用的 reader 模板，只设置 schema、统计和孩子属性要求，不创建 `TablePlan`，也不携带 Go 工厂从 `TiKVSingleGather` 取得的分区信息。

### 子树与计划接口

- `SetChildren` 只取传入列表的第一个孩子作为 `TablePlan`，同步 schema，并按 TiFlash 子树形状更新请求类型；额外孩子会被静默丢弃。
- `GetTableScans` 通过 `collect_table_scans` 深度优先收集所有 `PhysicalTableScan`；`GetTableScan` 要求结果恰好一个，否则返回 `the count of table scan != 1`。
- `ExplainInfo` 输出 `data:<id>`，MPP 时前置全局最新 MPP 版本；`OperatorInfo` 始终只输出 data 部分；`ExplainNormalizedInfo` 当前为空字符串。
- `ExtractCorrelatedCols`、`ResolveIndices`、`ToPB` 分别委托子树提取相关列、解析列下标和生成 `tipb::Executor`。
- `GetPlanCostVer1`/`GetPlanCostVer2` 委托 `BasePhysicalPlan`；`GetAvgRowSize` 以“schema 列数 × 8”粗估，`GetNetDataSize` 再乘子树 `stats_count`。
- `MemoryUsage` 汇总 producer、`TablePlan.memory_usage()` 和 `PlanPartInfo`；`LoadTableStats` 当前仅调用 `GetTableScans` 后丢弃结果。

## 执行流程

1. 旧 cascades 路径 `pkg/planner/cascades/old/implementation_rules.rs::ImplTiKVSingleReadGather::OnImplement` 在非索引 gather 分支调用 `GetPhysicalTableReader`，用 logical group 的 context、schema、缩放统计和 child property 创建 reader 模板；传统物理枚举路径也在 `base_physical_plan.rs` 中调用该工厂。
2. `lib.rs::ConcretePhysicalOperator::attach_operator_to_task` 克隆各 child task 的计划，再克隆 reader 模板；为对齐 Go 在 cop 子节点建成后才物化 reader 的 ExplainID 顺序，它重新分配 reader id，然后调用 `SetChildren`。
3. `SetChildren` 保存首个下推树、复制根 schema。TiFlash 下若根是 `PhysicalExchangeSender`，请求类型设为 MPP；否则递归检查子树：出现 `PhysicalHashAgg`、`PhysicalStreamAgg` 或 `PhysicalTopN` 且任何 `PhysicalTableScan` 都未要求 `KeepOrder` 时使用 BatchCop，否则使用 Cop。非 TiFlash 不在此分支改写请求类型。
4. 后续准备阶段可调用 `ResolveIndices`，先解析 reader 的 producer，再解析整棵 `TablePlan`；任一步失败立即向上传播。
5. EXPLAIN、cost、访问对象、网络量与 PB 编码从 reader 读取或委托 `TablePlan`。`pkg/executor/builder.rs` 对 `PhysicalTableReader` 向下转型，要求 `GetTablePlan()` 非空，再递归构造实际类型化执行器。
6. 计划缓存通过 `cache_snapshot.rs::CachedTableReader::{capture,restore}` 保存和恢复 producer、子树、存储/请求类型、common-handle 标志及分区信息；新增状态若未同步该处会在缓存恢复后丢失。

工厂返回值不是可执行 reader：在 `SetChildren` 或等价接线完成前，`TablePlan` 为空，执行器构建和 `ToPB` 都会明确失败。

## 数据与状态

核心不变量是 `TablePlan` 与对外 schema 一致，并由 trait 适配层把 `TablePlan` 暴露为 reader 的单一孩子。直接查看 `BasePhysicalPlan.Children` 不能代表 reader 是否有子计划；通用代码应调用 `PhysicalPlan::children()`。

`TablePlan` 用 `Box<dyn PhysicalPlan>` 独占动态分派的计划树。`SetChildren` 取得所有权，`Clone` 通过 `clone_physical(new_ctx)` 深克隆；`GetTableScans` 返回借用，生命周期不会越过 reader。`PlanPartInfo` 在 clone 时复制，缓存快照则显式 capture/restore。

请求类型存在两个计算入口且语义不同：`Init` 的 `adjust_read_request_type` 只识别 ExchangeSender/MPP；`SetChildren` 还会识别 BatchCop 候选。直接赋值 `TablePlan` 或只调用 `SetTablePlanForTest` 不会自动同步 schema/请求类型，因此生产接线应走 `SetChildren`，测试辅助方法不能替代完整初始化。

当前 BatchCop 判定只依据存储类型和子树形状，没有读取 Go 使用的 `SessionVars.AllowBatchCop`；同时它不会在本文件内把所有 scan 的 `IsMPPOrBatchCop` 改为 true。专属测试明确由 scan 候选构造阶段预先设置此标志，这是一项跨阶段不变量。

## 依赖与调用关系

直接依赖由 `Cargo.toml` 声明：`base` 提供 `ContextRef`、`PhysicalPlan`、计划 context 与 PB 构建上下文；`expression` 提供 schema、相关列和错误；`property` 提供统计、task 类型和物理属性；`costusage` 提供两版 cost 类型；`kv` 提供存储类型和 MPP 版本；`tipb` 是下推 executor 编码边界；`plancodec` 提供 TableReader 类型常量。

RustCodeGraph 的 `node` 结果确认 `GetPhysicalTableReader` 被 `implementation_rules.rs::OnImplement` 与 `base_physical_plan.rs` 中的传统路径调用；`PhysicalTableReader` 还被 `cache_snapshot.rs::restore` 直接实例化。源码补充核验的主要上游/消费者包括：

- `pkg/planner/cascades/old/implementation_rules.rs`：从 `TiKVSingleGather` 生成 table reader implementation，并通过 `ReaderCostPlan` 使用其网络/行宽语义。
- `pkg/planner/core/operator/physicalop/lib.rs`：提供统一 trait、孩子模型与 attach-to-task 生命周期。
- `pkg/planner/core/operator/physicalop/base_physical_plan.rs`：传统优化/任务物化、MPP 投影插入及多处 reader 分类逻辑。
- `pkg/executor/builder.rs`：消费完整 `TablePlan` 构建执行器；空计划报 `PhysicalTableReader has no TablePlan`。
- `pkg/planner/core/operator/physicalop/cache_snapshot.rs`：计划缓存状态序列化与恢复。

主要下游边是 `PhysicalSchemaProducer`/`BasePhysicalPlan` 的 context、schema、统计、属性、clone 和 cost API，以及动态 `PhysicalPlan` 的 `schema`、`children`、`stats_count`、`extract_correlated_cols`、`resolve_indices`、`to_pb`、`memory_usage` 与 `explain_id`。

## 错误处理与边界

- `Clone`、`ResolveIndices`、cost 和 `ToPB` 以 `Result<_, expression::Error>` 传播下游错误；没有在 reader 中吞掉子计划错误。
- `ToPB` 对空子树返回 `table reader has no table plan`；执行器 builder 也独立检查空值。相反，explain、行宽、网络量、相关列、访问对象、统计加载和内存估算对空计划返回空/零或无操作，不能用这些宽容结果证明 reader 已完整接线。
- `GetTableScan` 明确拒绝零个或多个 scan；MPP 子树允许 join/union 等形状并可能包含多个 scan，应使用 `GetTableScans`。
- `SetChildren` 不验证数量：空列表会清空子树，多个孩子只保留第一个。新增调用点应在上游保证单孩子，避免静默丢计划。
- `AccessObject` 将各 scan 的字符串用 `, ` 连接，忽略传入 context，也不实现 Go 的动态分区访问对象类型与别名/裁剪逻辑。
- `ExplainInfo` 对直接 `PhysicalTableScan` 使用其专用 `ExplainID`，其他根使用动态 trait 的 `explain_id`；MPP 版本取 `kv::GetNewestMppVersion()`，并非 Go 的会话选择值。
- `collect_table_scans` 与两项 BatchCop 辅助检查使用递归；异常深的计划树会带来递归栈风险，遍历复杂度与访问节点数线性相关。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或网络连接。所有子计划和分区状态由 `Box`、`Option` 与普通值拥有，reader 释放时随之释放；scan 列表仅保存临时借用，不延长资源生命周期。

`ContextRef` 是共享 context 引用；clone 时调用者提供的新 context 会进入基座和整棵子计划。计划改写方法使用 `&mut self`，本文件不提供并发写同步，外层 planner 必须保证构造/重写阶段的独占访问。

`MemoryUsage` 是估值而非释放操作，且只调用根 `TablePlan.memory_usage()`；是否包含全子树取决于各算子的 trait 约定。`LoadTableStats` 名称虽然暗示资源加载，Rust 当前没有 I/O 或缓存副作用，不能依赖它完成 Go 的统计预热。

## 与 Go 版本的对应关系

同路径 `physical_table_reader.go` 是语义基准，`Cargo.toml` 的 `[package.metadata.porting]` 也把 Go package 指向 `pkg/planner/core/operator/physicalop`。主要对应与差异如下：

- 两边都有 `TablePlan`、`StoreType`、`ReadReqType`、`IsCommonHandle` 和 `PlanPartInfo`；Go 另有扁平 `TablePlans` 与多 scan 分区映射 `TableScanAndPartitionInfos`，Rust 改用递归遍历且没有后一状态。
- Go `Init` 会扁平化计划、同步 schema、选择请求类型，并为 MPP/BatchCop 递归标记 scan；Rust `Init` 只选择 Cop/MPP，Rust `SetChildren` 才含 BatchCop 形状判定，scan 标记由更早的候选构建阶段负责。
- Go BatchCop 判定尊重 `AllowBatchCop` 的 0/1/2 配置；Rust 当前不读取该会话变量，表现更接近“含 agg/TopN 时允许”。这是兼容与执行策略风险，不应描述为完整对齐。
- Go `GetAvgRowSize`/`GetNetDataSize` 调用 cardinality 并使用表统计与列类型；Rust固定每列 8 字节。Go cost 调用 TableReader 专用函数，Rust转发 `BasePhysicalPlan`，估算精度不同。
- Go `LoadTableStats` 通过首个扁平 scan 的表和物理表 ID 真正加载统计；Rust只收集 scan。Go `AccessObject` 实现动态分区裁剪、别名和多 scan 分区对象；Rust只连接各 scan 的字符串描述。
- clone、相关列、单 scan 约束、explain/operator info、下标解析的大体意图一致；Rust对可选子树更宽容。Go `SetChildren` 直接索引第一个孩子，空输入会 panic；Rust空输入会清空子树。
- Go 工厂接收 `TiKVSingleGather` 并填充 `PlanPartInfo`、query block offset；Rust工厂直接接收 context/schema/stats/props，默认 offset 为 0，也不创建分区信息。调用链若需要这些语义必须另行补齐。

独立 Rust 测试 `physical_table_reader_test.rs` 覆盖 TiFlash + ExchangeSender 选择 MPP、单 scan 的 MPP 标志，以及 `OperatorInfo` 不含 MPP 版本前缀。它没有覆盖 BatchCop、空/多 scan、clone、PB、cost、分区和统计加载。Go 没有同名 `_test.go`；相关 Go 行为只在 `physical_utils_test.go` 等邻近测试中零散覆盖 reader 分类。

## 扩展指南

- 新增 reader 字段时同步修改 `New`、`Clone`、`MemoryUsage`、`cache_snapshot.rs::CachedTableReader::{capture,restore}`，并核对计划缓存测试；否则缓存恢复会静默丢状态。
- 修改孩子模型时同时更新本文件 `SetChildren`、`lib.rs::ConcretePhysicalOperator` 的 children/set-child/attach 实现、executor builder 和传统 task 物化逻辑，不要只写 `BasePhysicalPlan.Children`。
- 调整请求类型时，把 `Init` 与 `SetChildren` 的判定收敛到一致规则，并明确 Go 的 `AllowBatchCop`、scan `KeepOrder`、`IsMPPOrBatchCop` 标记和会话 MPP 版本契约；在独立 `physical_table_reader_test.rs` 增加 Cop、BatchCop、MPP、禁用 BatchCop、有序 scan 与嵌套树用例。
- 完成 Go 对齐时，统计加载、动态分区访问对象、真实平均行宽和专用 cost 应分别接入已有 Rust 子系统，不能用当前无操作或固定 8 字节估算冒充完整能力。
- 扩展多 scan/MPP 时保留 `GetTableScan` 的单扫描显式错误，并为 `GetTableScans` 的遍历顺序、访问对象组合、分区信息映射和 PB 编码定义清楚规则。
- 更改构造工厂时检查 cascades 与传统 planner 两条调用链，尤其是 query block offset 与 `PlanPartInfo`；更改执行边界时同步 `pkg/executor/builder.rs` 的空计划和子树类型验证。
- Rust 测试必须继续放在独立测试文件，不应内嵌进生产源文件；涉及计划缓存、规范路由或执行器接线时分别补充 `cache_snapshot_test.rs`、`canonical_router_aster_unit_test.rs` 和 executor 的相关独立测试。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter` 确认目标文件已索引；`node --file ... --offset 1 --limit 1000` 读取完整 367 行并报告被 76 个文件使用；`query`/`node` 定位 Rust/Go 类型、工厂及上游调用边。`explore`、`callers`、`callees` 本次未返回明细，相关关系进一步由精确源码搜索核验。
- 目标源码：`pkg/planner/core/operator/physicalop/physical_table_reader.rs`，核对枚举、结构体、公开方法、三个局部/私有递归辅助逻辑和工厂。
- crate 与模块：`pkg/planner/core/operator/physicalop/Cargo.toml`、`pkg/planner/core/operator/physicalop/lib.rs`，核对依赖、Go porting 元数据、导出、统一 trait、孩子模型和 attach 生命周期；包内不存在 `doc.go`。
- 上下游源码：`pkg/planner/cascades/old/implementation_rules.rs`、`pkg/planner/core/operator/physicalop/base_physical_plan.rs`、`pkg/planner/core/operator/physicalop/cache_snapshot.rs`、`pkg/executor/builder.rs`。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_table_reader.go`；邻近 Go 测试证据来自 `pkg/planner/core/operator/physicalop/physical_utils_test.go`。
- Rust 测试：`pkg/planner/core/operator/physicalop/physical_table_reader_test.rs`；补充接线证据来自 `cache_snapshot_test.rs`、`canonical_router_aster_unit_test.rs`、`physical_utils_test.rs` 和 `readers_scans_aster_unit_test.rs`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构命令，并人工复核唯一生产物、源码引用、Go 差异和无运行时代码修改。
