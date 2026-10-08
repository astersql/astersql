# `pkg/planner/core/operator/physicalop/physical_selection.rs` 逻辑说明

## 文件定位

本文件实现物理计划层的一元过滤算子 `PhysicalSelection`，对应 SQL 中经逻辑优化后仍需执行的 `WHERE`、`HAVING` 等谓词。它属于 Cargo 包 `astersql-planner-core-operator-physicalop`；该包以 `lib.rs` 为库入口，`lib.rs` 声明 `mod physical_selection`、公开再导出其符号，并以 `direct_operator_core!(PhysicalSelection, PhysicalSchemaProducer)` 把它接入统一的 `base::PhysicalPlan` 接口。

在优化主链中，`pkg/planner/core/operator/physicalop/base_physical_plan.rs` 的物理计划枚举路由把 `logicalop::LogicalSelection` 交给本文件的 `ExhaustPhysicalPlans4LogicalSelection`。枚举出的节点随后可由任务附着、索引解析、代价计算和 PB 编码流程消费；本地执行侧则由 `pkg/executor/builder.rs` 构造 `TypedSelection`，或由 `pkg/executor/physical_plan_runtime.rs` 递归/流式过滤行。

## 核心职责

- 用 `PhysicalSelection` 保存过滤谓词 `Conditions`、通用物理计划与输出 Schema 状态，以及兼容代价模型的 `FromDataSource` 标志。
- 从 `LogicalSelection` 和所需 `PhysicalProperty` 枚举物理候选，并决定是否增加 MPP/TiFlash 子属性候选。
- 为计划接口提供克隆、相关列提取、EXPLAIN、列下标解析、任务附着、代价查询、PB 序列化和内存估算。
- 在 `ResolveIndices` 中对 CTE 等场景提供一次受约束的列重映射回退，使谓词最终绑定到唯一的孩子 Schema 列。

本文件描述的是计划节点及其转换逻辑，不直接扫描数据。实际逐行谓词求值见 `pkg/executor/typed_selection.rs` 和 `pkg/executor/physical_plan_runtime.rs`。

## 主要符号

- `resolve_cte_column_ids(context, expression, schema)`：私有辅助函数。先直接调用表达式的 `ResolveIndices`；失败后提取列，优先沿用有效下标所指的 Schema 列，否则仅在字符串表示唯一匹配时替换，再执行 `ColumnSubstitute` 和第二次解析。若回退仍失败，返回第一次错误。
- `PhysicalSelection`：公开结构体。
  - `PhysicalSchemaProducer`：承载 `BasePhysicalPlan`、输出 Schema、孩子、统计信息、计划上下文和 TiFlash 细粒度 shuffle 配置。
  - `Conditions: Vec<ExprBox>`：合取语义的过滤表达式列表；执行侧要求全部谓词匹配才保留行。
  - `FromDataSource: bool`：标记 Selection 是否来自数据源侧；Go 注释明确其用途是代价模型兼容。
- `New` / `Init`：前者创建 `TypeSel` 空节点，后者绑定上下文、统计信息、查询块偏移和孩子所需属性。
- `Clone`：用新上下文克隆基础计划和 Schema，深克隆表达式，并保留 `FromDataSource`。
- `ExtractCorrelatedCols`：从所有条件中收集外层查询相关列。
- `ExplainInfo` / `ExplainNormalizedInfo`：分别生成可读和稳定归一化的谓词文本；前者追加非零 stream count，后者受全局 `vardef::IgnoreInlistPlanDigest` 控制。
- `ResolveIndices`：先解析 `PhysicalSchemaProducer`，再把条件绑定到第一个孩子 Schema。
- `Attach2Task`、`GetPlanCostVer1`、`GetPlanCostVer2`：将统一接口委托给基础计划实现。
- `ToPB`：生成 `tipb::Executor(TypeSelection)`；TiFlash 路径递归嵌入孩子并写入 executor ID。
- `MemoryUsage`：合计 Schema 生产者、所有条件表达式和布尔标志的估算内存。
- `ExhaustPhysicalPlans4LogicalSelection`：逻辑 Selection 的物理候选枚举入口；其局部函数 `contains_runtime_scalar_subquery` 识别不可进入 MPP 片段的运行时标量子查询表达式。

文件中没有条件编译项、模块级常量或单独 trait 定义；测试模块的 `#[cfg(test)]` 声明位于 `lib.rs`。

## 执行流程

1. `base_physical_plan.rs` 识别 `LogicalSelection`，调用 `ExhaustPhysicalPlans4LogicalSelection(logical, property)`。
2. 枚举函数取得逻辑节点上下文；上下文缺失时直接返回空候选集。
3. 递归检查条件中是否含运行时标量子查询：`Constant.SubqueryRefID > 0`、`Column.UniqueID == 0`、名称以 `ScalarQueryCol#` 开头或未知表达式实现都视为 root-only；相关列本身不视为此类表达式。排序后的 EXPLAIN 文本检查作为 `ScalarQueryCol#` 的补充证据。
4. 若请求本身是 MPP，而条件含运行时标量子查询，或当前逻辑子树不应检查 TiFlash 下推，则拒绝生成候选。
5. 克隆父属性的必要字段作为普通孩子属性，并继承 `CanAddEnforcer`。对于非 MPP 请求，在会话允许 MPP、存在可检查的 TiFlash 路径、无虚拟列且无运行时标量子查询时，再加入一个 `MppTaskType` 孩子属性。
6. `crate::AdmitIndexJoinProps` 对候选孩子属性执行 Index Join 场景的准入调整；统计信息按 `ExpectedCnt` 缩放。
7. 每个孩子属性生成一个 `PhysicalSelection`，深克隆逻辑条件，复制逻辑 Schema，并通过 `Init` 写入上下文、统计信息、查询块偏移和孩子属性。
8. 后续计划定型时，`ResolveIndices` 将条件绑定到真实孩子 Schema；`Attach2Task` 将节点挂入 Root/Cop/MPP 任务树；代价接口参与候选比较。
9. 执行有两条可见路径：`builder.rs` 要求恰好一个孩子并构造 `TypedSelection`；`physical_plan_runtime.rs` 的流式路径逐行调用 `row_matches_conditions`，非流式路径先执行孩子再用 `filter_rows` 过滤。
10. 下推时 `ToPB` 序列化条件；TiFlash 还递归序列化唯一孩子，TiKV 路径不把孩子嵌入 Selection PB。

## 数据与状态

`PhysicalSelection` 自身拥有条件表达式对象和 `FromDataSource` 值；通用计划状态由内嵌的 `PhysicalSchemaProducer`/`BasePhysicalPlan` 管理。`Clone` 对条件调用 `CloneExpr`，并克隆 Schema，因此候选之间不会共享可变表达式或 Schema 容器；上下文替换为调用方传入的 `new_ctx`。

`Init` 会覆盖上下文、类型、查询块偏移、统计信息和孩子需求属性。`ExhaustPhysicalPlans4LogicalSelection` 使用 `ScaleByExpectCnt` 生成候选统计信息；如果逻辑节点无统计信息，则使用默认值。`Conditions` 为空在类型层面合法，`ToPB` 会生成无条件的 Selection 执行器。

`ExplainNormalizedInfo` 读取进程级原子配置 `vardef::IgnoreInlistPlanDigest`。它不在节点内缓存结果；开关变化会影响随后生成的归一化文本。`MemoryUsage` 是估算值，只计入本节点拥有/封装的结构和表达式，不代表孩子子树总内存。

## 依赖与调用关系

上游直接证据：

- `pkg/planner/core/operator/physicalop/base_physical_plan.rs`：`LogicalSelection -> ExhaustPhysicalPlans4LogicalSelection` 的枚举路由。
- `pkg/planner/core/operator/physicalop/lib.rs`：模块声明、公开再导出、`direct_operator_core!` 接口接线。
- `pkg/planner/core/operator/physicalop/task.rs`：在 Root 条件物化时构造 Selection。
- `pkg/planner/core/operator/physicalop/index_join_probe.rs`、`base_physical_plan.rs`：其他规划重写会直接构造或识别该节点。
- `pkg/executor/builder.rs`、`pkg/executor/physical_plan_runtime.rs`：把计划节点转换为执行器或直接执行过滤语义。

主要下游依赖：

- `base`：`ContextRef`、`PhysicalPlan`、`Task`、`BuildPBContext` 及通用计划接口。
- `logicalop`、`property`、`planner_util`：逻辑 Selection、物理属性/统计信息、TiFlash 下推资格判断和 Index Join 属性准入。
- `expression`：表达式克隆、列提取/替换、索引解析、EXPLAIN、虚拟列检测与 PB 转换。
- `kv`、`tipb`：目标存储类型和下推执行器协议。
- `costusage`、`plancodec`、`vardef`：代价结果/选项、`TypeSel` 类型码和计划摘要配置。

上述依赖均由 `pkg/planner/core/operator/physicalop/Cargo.toml` 声明；该 manifest 还以 `[package.metadata.porting] go-package = "pkg/planner/core/operator/physicalop"` 标明 Go 对照包。

## 错误处理与边界

- `ResolveIndices` 没有孩子时仅完成基础解析后返回成功；有孩子时只使用第一个孩子 Schema，符合一元算子约束。真正执行与 TiFlash PB 编码会显式要求一个孩子。
- CTE 回退只接受两种安全替换：有效下标，或 `Column::String()` 在孩子 Schema 中恰好唯一匹配。零匹配或多匹配保持原列，第二次解析失败时返回原始错误，避免把歧义列静默绑定到错误位置。
- `ResolveIndices` 会给条件绑定错误增加孩子 Schema 的 `String#UniqueID` 列表，便于定位；基础计划解析错误直接用 `?` 传播。
- `ToPB` 仅在条件非空时索取 PB client，因此空条件 TiKV Selection 可在无 client 时编码；非空条件缺 client 返回 `PB client is required`。表达式转换和孩子 PB 错误继续传播。
- TiFlash `ToPB` 要求第一个孩子，缺失时返回 `selection requires one child`；TiKV 路径不会检查孩子。执行器构建和运行时另行以 `PhysicalSelection requires one child`/`one_child` 守住一元结构。
- MPP 枚举保守拒绝未知表达式实现，因为规划器私有表达式未必能由存储表达式 crate 序列化。此选择偏向正确性而非尽量下推。
- `Clone`、EXPLAIN 和 `MemoryUsage` 假定表达式实现遵守各自 trait 合约；本文件不捕获 panic，也不为异常表达式提供降级执行。

## 并发与资源生命周期

节点不创建线程、异步任务、通道、锁、事务或网络请求。它随物理计划树创建、克隆并释放，孩子关系及表达式由 `Box`/`Vec` 所有权管理；上下文通过 `ContextRef` 共享。

唯一可见的跨节点全局状态是 `vardef::IgnoreInlistPlanDigest` 的原子读取。对应测试 `physical_selection_test.rs::normalized_explain_honors_ignore_inlist_plan_digest_like_go` 使用互斥锁串行修改测试进程中的该开关，并在结束前恢复旧值；生产方法自身只读，不负责配置生命周期。

PB 转换只构造协议对象。测试用 `PushDownClient::Send` 会 panic，以证明本路径不应发送 KV 请求；实际网络请求生命周期不属于本文件。TiFlash 路径递归构造孩子 PB，但不持有额外运行时资源。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/operator/physicalop/physical_selection.go`。Rust 已覆盖 Go 的核心字段和方法：候选枚举、初始化、克隆、相关列提取、内存估算、两种 EXPLAIN、索引解析、任务附着、两版代价接口和 PB 序列化。`TypeSel`、TiFlash child/executor ID、stream count 以及 ignore-IN-list 摘要开关的语义一致。

已确认的实现差异：

- Go 枚举 MPP 候选时计算 `canPushDownToTiFlash`，包含 `expression.CanExprsPushDown(..., kv.TiFlash)`；当前 Rust 枚举没有调用同名通用可推检查，而是显式拒绝虚拟列和运行时标量子查询，并检查 TiFlash 路径。不能据此宣称两者对所有表达式的 MPP 候选集合完全相同。
- Rust 对“请求本身已是 MPP”的属性增加了显式早退守卫；所示 Go 函数只控制是否额外创建 MPP 子属性。
- Rust `Clone` 显式保留 `FromDataSource`，且 `unary_aster_unit_test.rs` 验证这一点；所示 Go `Clone` 未显式复制该字段。该差异应在代价兼容行为变更时重点核对。
- Rust `ToPB` 对空条件不要求 client，并显式检查 TiFlash 孩子；Go 先无条件取得 client，并直接索引 `Children()[0]`。Rust 的错误边界更显式，但协议结果保持对应。
- Go `ResolveIndices` 委托 `utilfuncp.ResolveIndices4PhysicalSelection`；Rust 本类型直接实现表达式解析，并带有按列字符串唯一匹配的 CTE 回退。仓库另有 `pkg/planner/core/resolve_indices.rs` 的简化 `SelectionPlan` 路径，但它不是本方法的直接实现。

Go 独立测试 `pkg/planner/core/operator/physicalop/physical_utils_test.go` 还验证 Selection 在扁平化下推计划和单扫描 Index Join 检测中作为透明一元包装节点的角色；Rust 对应周边行为由 `task_test.rs`、`canonical_router_aster_unit_test.rs` 和执行器测试覆盖。

## 扩展指南

- 新增 Selection 持久字段时，应同步 `Clone`、`MemoryUsage`、`cache_snapshot.rs` 的捕获/恢复结构、`lib.rs` 的 cache snapshot contract，以及 `cache_snapshot_test.rs`；若字段影响计划摘要或下推，还需同步 EXPLAIN/PB 方法。
- 修改谓词绑定时，优先改 `ResolveIndices`/`resolve_cte_column_ids`，并在独立的 `physical_selection_test.rs` 或 planner 的 `resolve_indices_test.rs` 增加唯一匹配、歧义、缺列和 CTE/generated-column 场景。不要把测试内嵌进生产 `.rs`。
- 扩展 MPP 准入时，应修改 `ExhaustPhysicalPlans4LogicalSelection` 及其局部表达式分类，并与 Go 的 `CanExprsPushDown`、`ContainVirtualColumn`、TiFlash 可用性语义逐项核对。风险是生成存储端无法编码的计划，或过度保守导致 MPP 性能回退。
- 修改 PB 形状时，应同步 `physical_selection_test.rs` 的 TiKV/TiFlash 分支，并核对 `pkg/executor/builder.rs` 和存储协议消费者；TiFlash 的孩子嵌入与 executor ID 属于兼容性要求。
- 修改运行时过滤语义通常不应只改本文件；还需同步 `pkg/executor/typed_selection.rs`、`physical_plan_runtime.rs` 及其独立测试。该节点的 `Conditions` 是合取列表，改变空列表或 NULL/错误处理会直接影响 SQL 兼容性。
- 修改 `FromDataSource` 时应先核对 v1/v2 代价实现及 Go issue 注释所述兼容目的；字段看似简单，但可能改变计划选择。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标 Rust 文件；`query PhysicalSelection` 定位 Rust/Go 结构体，`query ExhaustPhysicalPlans4LogicalSelection` 定位 Rust/Go 枚举入口；`node --file pkg/planner/core/operator/physicalop/physical_selection.rs --offset 1 --limit 500` 读取目标文件全部 390 行。对枚举入口和 `resolve_cte_column_ids` 执行了 `callers`/`callees`，命令未返回边，因此调用关系用下列源码搜索补证，没有把缺失图边写成已验证结果。
- 已读生产与配置：`pkg/planner/core/operator/physicalop/physical_selection.rs`、`physical_selection.go`、`Cargo.toml`、`lib.rs`、`base_physical_plan.rs`、`task.rs`，以及执行侧 `pkg/executor/builder.rs`、`pkg/executor/physical_plan_runtime.rs`。
- 已读直接测试：`pkg/planner/core/operator/physicalop/physical_selection_test.rs`，覆盖 ignore-IN-list 归一化、TiFlash executor ID/孩子 PB、空 TiKV 条件无 client；`unary_aster_unit_test.rs` 覆盖 trait 接线和 `FromDataSource` 克隆；`task_test.rs` 覆盖 Root 条件物化；`cache_snapshot_test.rs` 覆盖条件和标志快照；Go 的 `physical_utils_test.go` 提供一元包装行为对照。
- 关键边通过 `rg` 定位：`base_physical_plan.rs` 调用枚举入口，`lib.rs` 接入统一接口，`builder.rs`/`physical_plan_runtime.rs` 消费节点，表达式/PB/属性依赖与 `Cargo.toml` 声明一致。
- 本任务仅新增文档，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工检查没有把未验证的 Go/Rust等价性或理想架构表述为当前事实。
