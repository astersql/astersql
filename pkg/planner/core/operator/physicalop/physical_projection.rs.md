# `pkg/planner/core/operator/physicalop/physical_projection.rs`

## 文件定位

本文件定义 SQL 物理计划中的投影节点 `PhysicalProjection`：它保存 SELECT 列表一类输出表达式，并负责把逻辑投影枚举成 Root、TiKV cop 或 TiFlash MPP 所需的物理候选。它属于 `astersql-planner-core-operator-physicalop` crate；同目录 `Cargo.toml` 以 `lib.rs` 为 crate 根，并声明了本文件直接使用的 `base`、`costusage`、`expression`、`kv`、`logicalop`、`property`、`planner_util`、`plancodec`、`tipb` 和 `vardef` 等依赖。

`physicalop/lib.rs` 以 `mod physical_projection` 装配本模块，并通过 `pub use physical_projection::*` 再导出公开项。该文件还为 `PhysicalProjection` 手写 `ConcretePhysicalOperator`，把本文的 Explain、索引解析、相关列提取、任务挂接、成本和 protobuf 转换接入统一 `Plan` / `PhysicalPlan` 动态分发；这一步很重要，因为 `Attach2Task` 与 `ToPB` 不是基础节点的默认实现。

本文件是规划期数据结构与转换逻辑，不执行行级表达式。执行器如何逐行求值、如何调度投影并发不在这里；本文能确认的运行边界止于物理树、任务树和下推 protobuf 的构造。

## 核心职责

本文件承担四组职责：

1. 定义投影节点的表达式、schema、统计信息及两个执行期开关，并提供初始化、深克隆、相关列提取、Explain、索引解析和内存估算。
2. 用 `policy_field_type`、`policy_expression` 和 `can_projection_push_to_store` 将运行时表达式映射到 `expression::infer_pushdown` 的策略模型，统一判断 TiKV/TiFlash 是否可执行该投影。
3. 在 `ExhaustPhysicalPlans4LogicalProjection` 中把父物理属性变换为孩子属性，枚举普通、MPP 和 cop 单读候选，同时维护 CTE、半连接、排序边界、JSON 聚合及 IndexJoin 等当前 Rust 路由需要的限制。
4. 在 `Attach2Task` 中选择把投影嵌入 Reader 内侧还是保留为 Root 节点，并在 `ToPB` 中编码为 `tipb::Projection`。

投影下推不是“表达式可编码”这一个条件：候选枚举还受会话开关、任务类型和收益判断控制；实际挂接又受 Reader 种类、存储类型及孩子属性控制。扩展时必须同时检查枚举、挂接和 protobuf 三个阶段。

## 主要符号

- `policy_field_type(&FieldType) -> infer_pushdown::FieldType`：把 MySQL 字段类型归类成策略层的 Bit、Set、Enum、Geometry、Json、Decimal、Duration、Year、Real、String、Vector、Time、Unspecified 或默认 Int，并复制长度、小数位、unsigned、hybrid、字符集和排序规则。
- `policy_expression(&dyn Expression, &dyn EvalContext) -> infer_pushdown::Expression`：常量只保留“常量”身份并用 Null 占位；列保留唯一 ID、非负索引、类型及虚拟列可编码性；标量函数递归转换参数。regexp 系列的返回字符集/排序规则特意跟随第一个参数。其他表达式变为 `Unsupported(ExplainInfo)`。
- `CanProjectionPushToTiFlash(&PhysicalProjection)` 与 `can_projection_push_to_store(...)`：前者是 TiFlash 专用入口，后者拒绝虚拟列和 TiFlash 上隐式 JSON cast，再调用 `infer_pushdown::can_exprs_push_down` 做最终策略判断。
- `pub struct PhysicalProjection`：`PhysicalSchemaProducer` 保存基础计划、schema、孩子和统计信息；`Exprs` 是输出表达式列表；`CalculateNoDelay` 要求后续阶段不要延迟计算；`AvoidColumnEvaluator` 是 Union 子投影规避 column evaluator 的兼容开关。后两个开关在本文件中只被保存和克隆，不在这里执行其运行时策略。
- `New` / `Init`：`New` 创建 `TypeProj` 空节点；`Init` 消费节点并设置上下文、节点类型、查询块偏移、统计信息和孩子要求属性。
- `Clone`：在新上下文中克隆基础物理计划、schema 和全部表达式，并复制两个布尔开关；失败返回 `expression::Error`。
- `ExtractCorrelatedCols`：遍历每个表达式并汇总其相关列；本方法不去重。
- `ExplainInfo` / `ExplainNormalizedInfo`：前者按日志脱敏和共享 `StmtCtx` 的 Explain 格式输出表达式，必要时追加 TiFlash 细粒度 shuffle 流数；后者根据全局 `IgnoreInlistPlanDigest` 选择是否忽略 IN 列表内容后排序归一化。
- `ResolveIndices`：先解析 `PhysicalSchemaProducer`，再以第一个孩子 schema 解析每个投影表达式；缺少孩子时成功返回。
- `Attach2Task`：根据孩子属性和 Reader 类型选择存储侧投影或 Root 投影，所有成功路径都构造新的计划对象而不原地改写输入 Reader。
- `GetPlanCostVer1` / `GetPlanCostVer2`：本地入口委托 `BasePhysicalPlan`；经 `lib.rs` 的 `PhysicalPlan` trait 调用时，会优先走 planner core 安装的全局成本路由器。
- `ToPB`：仅接受 TiKV/TiFlash，编码表达式、递归编码唯一孩子并设置 `TypeProjection` 与 executor ID。
- `MemoryUsage`：估算 schema producer、每个表达式及两个 bool 的内存，不是分配器级精确统计。
- `ExhaustPhysicalPlans4LogicalProjection`：本文件的物理候选枚举入口，返回 `Vec<Box<dyn PhysicalPlan>>`；无上下文、属性无法穿过投影或不满足强制任务条件时可返回空集合。

## 执行流程

传统枚举路径从 `ExhaustPhysicalPlans4LogicalProjection` 开始：

1. 从 `LogicalProjection` 取得上下文，并用 `TryToGetChildProp` 把父属性中的输出列映射回孩子列；失败即无候选。
2. 继承 `NoCopPushDown`；若禁止 cop 下推则强制 Root。带排序项的孩子允许补 enforcer，之后通过 `AdmitIndexJoinProp` 做 IndexJoin 属性准入。
3. 按父 `ExpectedCnt` 缩放统计信息。函数临时构造 `policy_probe` 计算 TiFlash/TiKV 下推资格，并用计划 ID checkpoint 恢复这次探测产生的 ID 副作用。
4. 递归识别孩子中的 CTE、半连接/半 Apply 和偏好 Root IndexJoin 的结构，并识别直接孩子是否为 TableDual、JSON 聚合、标量聚合或 Root 排序边界。
5. 如果请求 MPP 但孩子包含半连接，直接拒绝；否则结合会话 MPP 许可、TiFlash 可用性、表达式资格及上述边界计算 `can_use_mpp`。强制 MPP 时，符合条件的非 MPP 候选会被移除。
6. 在允许时追加由已变换孩子属性克隆出的 MPP 候选；Root TopN/Sort 边界、偏好 Root IndexJoin 和半连接会阻止这条额外候选。
7. 在投影下推开关启用、TiKV 表达式可执行且下推能减少/改变有效投影工作时，追加 `CopSingleReadTaskType` 候选。
8. 每个孩子属性生成一个 `PhysicalProjection`，克隆逻辑表达式、复制 `CalculateNoDelay`、设置逻辑 schema，并通过 `Init` 安装统计和属性。

旧 Cascades 路径在 `pkg/planner/cascades/old/implementation_rules.rs::ImplProjection`：它同样调用 `TryToGetChildProp`，然后构造并初始化 `PhysicalProjection`，但该规则本身只生成对应给定属性的实现，不复刻本文传统入口的整套额外 MPP/cop 枚举。

候选被选中后，`Attach2Task` 先尝试三种 Reader 内嵌路径：cop 属性且有收益时嵌入 `PhysicalIndexReader` 的 index plan；嵌入 `PhysicalIndexLookUpReader` 的 table plan；或依据 `PhysicalTableReader::StoreType` 嵌入 table plan。每条路径都会克隆投影、被包裹的内部计划和 Reader，更新 Reader schema；TableReader 路径还复制输出名。不能下推时，函数克隆全部孩子计划，把第一孩子统计复制给投影，安装孩子后返回以投影为根的新 `RootTask`。

`ToPB` 是存储侧终点：检查 store，要求 PB client，调用 `ProjectionExpressionsToPBList`，递归编码第一个孩子，然后组装带 explain ID 的 `tipb::Executor`。

## 数据与状态

核心持久状态是 `Exprs`、输出 schema、孩子要求属性和统计信息。`Exprs[i]` 与输出 schema 的第 `i` 列存在语义对应，但本文件不强制两者长度相等；构造者必须维护该不变量。索引解析使用第一个孩子 schema，说明投影是一元节点；`ToPB` 也明确要求一个孩子。

`ResolveIndices` 用解析后的新表达式逐项替换原表达式。失败时，错误文本会附加孩子 schema 中每列的可读名与 `UniqueID`，便于定位列绑定问题；但由于逐项写回，后面表达式失败时，前面表达式已经解析完成，因此该方法不是事务式回滚。

候选探测会创建临时 `PhysicalProjection`，可能分配计划 ID；`plan_id_checkpoint` / `restore_plan_id_checkpoint` 用于避免纯策略探测永久消耗 ID。只有上下文支持 checkpoint 时才恢复。

`Attach2Task` 通过深克隆建立新的所有权树，避免直接修改传入 Reader；计划上下文仍以 `ContextRef` 共享。推入 Reader 后，Reader 的输出 schema 被替换为投影 schema，这是上层看到正确列集合的关键状态更新。

本文件没有成本缓存字段的直接管理。常规 trait 成本调用由 `physicalop/lib.rs` 优先路由到 `pkg/planner/core/plan_cost_ver1.rs` / `plan_cost_ver2.rs`；本文件的两个方法只是无全局路由时的基础计划回退入口。

## 依赖与调用关系

上游与装配关系：

- `physicalop/lib.rs` 再导出本文符号，并通过 `ConcretePhysicalOperator for PhysicalProjection` 将通用 trait 的 `attach_to_task`、`to_pb`、`resolve_indices`、成本、Explain、克隆和内存方法路由到本文。
- `ExhaustPhysicalPlans4LogicalProjection` 是逻辑投影到传统物理候选的直接转换入口；`cascades/old/implementation_rules.rs::ImplProjection` 是旧 Cascades 的平行构造入口。
- `base_physical_plan.rs` 在任务路由、投影注入、Reader 边界、MPP 下推和计划改写的多个位置识别或构造 `PhysicalProjection`；其中 aggregation cop-reader 准入调用 `can_projection_push_to_store`，TiFlash Reader 附着调用 `CanProjectionPushToTiFlash`。
- `pkg/planner/core/plan_cost_ver1.rs` 和 `plan_cost_ver2.rs` 通过全局路由识别该类型，递归计算孩子成本并加入投影自身成本。
- 执行侧如 `pkg/executor/statement_ru_plan_walk.rs` 会遍历该节点；这属于物理计划的消费方，而非本文的表达式执行实现。

主要下游依赖：

- `expression` 提供表达式 RTTI、类型、克隆、相关列、索引解析、Explain、PB 编码、虚拟列判断和下推策略。
- `property` 定义 Root、CopSingleRead、MPP 等任务类型及物理属性；`logicalop` 提供逻辑投影和用于边界识别的逻辑节点类型。
- `PhysicalSchemaProducer` / `BasePhysicalPlan` 提供 schema、上下文、孩子、统计信息和通用计划行为；三个 Reader 类型与 `RootTask` 实现任务树重组。
- `planner_util::ShouldCheckTiFlashPushDown` 与 `logicalop::GetHasTiFlash` 共同限定是否检查 TiFlash 路径；会话变量控制 MPP 与 projection pushdown。
- `tipb` 是下推执行器的序列化边界。

RustCodeGraph 的文件查询确认目标包含 30 个符号，并报告被 8 个文件使用；索引的文件视图明确列出 executor、旧 Cascades 和 logicalop 等使用方。对精确 Rust 方法运行 `callers` / `callees` 时当前索引未返回边，因此本文没有把缺失结果解释为“无调用者”，而是用模块路由源码和精确引用搜索补证。

## 错误处理与边界

- `Clone`、`ResolveIndices`、两版成本入口和 `ToPB` 使用 `Result<_, expression::Error>` 传播可恢复错误。
- `ResolveIndices` 无孩子时返回成功；这便于尚未接线的节点存在，但不表示该节点可被序列化或执行。它对 schema 缺列补充上下文后返回错误。
- `ToPB` 拒绝 TiDB 与 `UnSpecified` store，缺 PB client、缺孩子、表达式编码失败或孩子编码失败都会返回错误；只有 TiKV/TiFlash 是合法序列化目标。
- `Attach2Task` 假定至少存在一个任务；空向量不会立刻 panic，但会走 Root 回退并产生零孩子投影。该节点随后不能通过 `ToPB` 的“一孩子”检查。克隆失败均通过 `expect` panic，而不是返回错误。
- Reader 下推的若干 `Clone` 也以 `expect` 处理失败，表明规划器把已建计划可克隆视为内部不变量。
- `policy_expression` 对未知表达式返回 `Unsupported`，让策略层拒绝下推，而不是误判为可执行。常量使用 Null 占位意味着该策略只判断表达式种类/类型兼容性，不验证常量具体值。
- 虚拟列一律阻止存储侧投影；TiFlash 还额外拒绝对 JSON 参数的隐式 cast，以保持 AVG、SUM、GROUP_CONCAT 等场景由 TiDB 计算。
- MPP 枚举对半连接、CTE、标量聚合、Root 排序边界等采用有意的保守限制；这些是当前 Rust 任务/成本桥接的兼容边界，不应概括成 TiFlash 永远不支持相关算子。
- `MemoryUsage` 未计入 `Vec` 容器本身的所有布局细节，也不包含共享上下文的完整内存；只能用于一致的估算口径。

## 并发与资源生命周期

本类型没有锁、通道、后台线程、文件句柄、网络连接或事务。`ProjectionConcurrency` 只在 planner core 的成本模型中参与估算，实际 worker 生命周期不由本文件管理。`CalculateNoDelay` 与 `AvoidColumnEvaluator` 也是传递给后续阶段的元数据，而非本文创建的并发资源。

唯一可见的全局并发敏感状态是 `vardef::IgnoreInlistPlanDigest`，`ExplainNormalizedInfo` 只原子读取它；独立测试用互斥锁串行修改该全局开关，锁属于测试而非生产类型。会话上下文与表达式上下文通过共享引用访问，本文件不修改其生命周期。

克隆是任务树资源边界：`Clone` 深克隆表达式和 schema，`Attach2Task` 深克隆孩子/Reader，再由返回的 `RootTask` 独占新树。PB 构建只借用 `BuildPBContext`；`PushDownClient` 不归投影所有。计划 ID checkpoint 是短生命周期的逻辑状态快照，并在策略探测后立即恢复。

## 与 Go 版本的对应关系

直接对照为同目录 `physical_projection.go`，其字段和 `Clone`、`ExtractCorrelatedCols`、`MemoryUsage`、Explain、`Init`、`ResolveIndices`、成本、`ToPB`、`Attach2Task` 均有 Rust 对应。两边都保持 `TypeProj`、表达式深克隆、相关列汇总、TiKV/TiFlash PB 限制、executor ID 和 IN 列表归一化开关等核心语义。

需要明确的当前差异：

- Go 的 `ExhaustPhysicalPlans4LogicalProjection` 主要基于 `TryToGetChildProp`、MPP/TiKV 表达式资格、会话开关和收益判断枚举候选；Rust 增加了 `NoCopPushDown`、强制 MPP、CTE、半连接、JSON 聚合、标量聚合、Root TopN/Sort 和 IndexJoin 偏好等适配分支。这些是当前 Rust 完整路由的实际行为，不可用较短 Go 函数覆盖描述。
- Go 直接使用 `expression.CanExprsPushDown`；Rust 先把表达式转换为 `infer_pushdown` 策略 AST，并显式拒绝虚拟列和 TiFlash JSON cast。regexp 返回类型的字符集/排序规则也在转换中补正。
- Go `attach2Task4PhysicalProjection` 复制通用 `CopTask` / `MppTask`，允许直接把节点附到相应任务，再回退 Root；Rust 当前针对 `PhysicalIndexReader`、`PhysicalIndexLookUpReader` 和 `PhysicalTableReader` 克隆并重组具体 Reader，最终都返回 `RootTask`。两者目标同为“可下推则靠近存储，否则留在 Root”，但中间任务表示并非逐语句移植。
- Go `resolveIndices4PhysicalProjection` 假定第一个孩子存在，并在相邻投影时调用 `refine4NeighbourProj` 保持重复输入索引关系；本文 Rust 对缺孩子安全返回，但没有调用对应 refine 逻辑。不能宣称相邻投影的索引细化已在本文件等价实现。
- Go v1 成本是孩子成本加按投影并发折算的 CPU/并发开销；v2 还按表达式数估算过滤式成本。Rust 经 `lib.rs` 全局路由进入 planner core 的对应 Rust 成本实现；只有路由未安装时才回退 `BasePhysicalPlan`，因此审查成本一致性必须同时看路由安装与 core 实现。
- Go `MemoryUsage` 支持 nil 接收者且按 Go 对象布局估算；Rust 不存在 nil `&self`，其数值不能与 Go 逐字节比较。
- Rust `ToPB` 对缺 client、缺孩子给出显式错误；Go 直接取 client 并索引 `Children()[0]`，前置不变量更强。

独立 Rust 测试位于 `physical_projection_test.rs`，覆盖 PB executor ID、IndexReader 内嵌与 Root 边界、普通 cast 不下推、IN 列表 digest 开关以及 Explain 格式。未发现同目录独立 `physical_projection_test.go`；Go 语义依据生产文件及 `task.go`、`resolve_indices.go`、两版成本文件核对。

## 扩展指南

- 新增字段时同步更新 `New`、`Clone`、`MemoryUsage`、逻辑到物理的两个构造入口，以及真正消费字段的执行器构建代码；测试继续放在独立 `physical_projection_test.rs`，不要内嵌进生产文件。
- 新增可下推表达式或类型时，同时审查 `policy_field_type`、`policy_expression`、`can_projection_push_to_store` 和 `ToPB`。至少增加 TiKV/TiFlash 正反例、虚拟列、字符集/排序规则、JSON cast 及 PB 编码失败测试，避免“策略允许但编码失败”或相反。
- 修改物理候选枚举时，优先在 `ExhaustPhysicalPlans4LogicalProjection` 保持父属性到孩子属性的列映射、`NoCopPushDown`、计划 ID checkpoint、MPP 强制语义和 Root 排序边界；还要核对旧 Cascades `ImplProjection` 是否应同步。
- 修改任务附着时，必须覆盖 IndexReader、IndexLookUpReader、TiKV/TiFlash TableReader、Root 属性、无收益投影和不可下推表达式。保持输入 Reader 不被原地修改，并同步 schema 与 output names。
- 修改索引解析时，应增加无孩子、缺列、相邻投影重复索引和部分解析失败用例，并对照 Go 的 `refine4NeighbourProj` 判断是否需要移植关系细化。
- 修改成本时，不能只改本文的 `GetPlanCostVer1/Ver2`；还需检查 `physicalop/lib.rs` 路由、`pkg/planner/core/plan_cost_ver1.rs`、`plan_cost_ver2.rs` 及旧 Cascades 的 `ProjectionCostPlan`。
- 修改 Explain 时，应保持脱敏、`StmtCtx` 格式、细粒度 shuffle stream count 和 digest 归一化稳定性，并同步现有独立测试。

主要兼容风险是错误下推造成存储端无法执行或结果类型/排序规则变化；正确性风险集中在属性列映射、schema 同步和索引解析；性能风险集中在误判“下推有收益”、错误枚举全 MPP 路径以及投影成本低估。任何扩展都应先用上述入口把这三类风险分别覆盖。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/planner/core/operator/physicalop/physical_projection.rs` 确认目标文件和 30 个符号；`node --file ... --offset 1 --limit 760` 完整读取 729 行源码，并得到 8 个使用文件的索引摘要。
- 主要符号查询：`query PhysicalProjection`、`query ExhaustPhysicalPlans4LogicalProjection --kind function`、`query CanProjectionPushToTiFlash --kind function` 确认 Rust/Go 对应定义及嵌套辅助函数。精确 `callers` / `callees` 对这些 Rust 符号未返回边，故调用关系改由索引源码视图与精确引用搜索核验，未将空结果当成无调用证据。
- Rust 生产源码：`physical_projection.rs`；模块装配与 trait 路由：`physicalop/lib.rs`；传统物理路由直接证据：`physicalop/base_physical_plan.rs`；旧 Cascades 构造与成本：`pkg/planner/cascades/old/implementation_rules.rs`。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml`，其中 `package.metadata.porting.go-package` 指向同路径 Go 包，且测试由 `lib.rs` 的 `#[cfg(test)] mod physical_projection_test` 独立装配。
- Go 对照：`physical_projection.go`、`pkg/planner/core/task.go::attach2Task4PhysicalProjection`、`pkg/planner/core/resolve_indices.go::resolveIndices4PhysicalProjection`、`pkg/planner/core/plan_cost_ver1.go::getPlanCostVer14PhysicalProjection`、`pkg/planner/core/plan_cost_ver2.go::getPlanCostVer24PhysicalProjection`。
- Rust 测试：`physical_projection_test.rs` 的 `projection_protobuf_carries_explain_id_like_go`、`cop_projection_attaches_inside_index_reader_but_root_projection_stays_outside`、`ordinary_cast_projection_stays_above_tikv_reader_like_go`、`normalized_explain_honors_ignore_inlist_plan_digest_like_go`、`projection_explain_reads_shared_statement_format`。
- 人工复核：本文分别回答了文件存在原因、逻辑候选如何形成、任务如何附着、状态和错误边界、Go 差异以及安全扩展位置；没有把运行时执行器、并发 worker 或缺失的 RustCodeGraph 调用边写成已验证事实。

本任务只新增说明文档，未修改 Rust、Go、Cargo 或 `plan.md`，并按计划不运行 Cargo。交付结构以计划规定的十一节标题命令验证。
