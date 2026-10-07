# `pkg/planner/core/flat_plan.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate。`pkg/planner/core/Cargo.toml` 将该 crate 的入口设为 `lib.rs`；`pkg/planner/core/lib.rs` 以私有模块 `mod flat_plan` 装入本文件，再通过 `pub use flat_plan::*` 对外导出其公开符号。它处在“物理计划树 → 可线性遍历的计划表示 → 编码、EXPLAIN 或 statement RU 统计”的转换边界。

文件同时维护两套用途不同的表示：`TypedFlatPhysicalPlan`/`TypedFlatOperator` 借用真实 `base::Plan` trait object，供执行器按具体算子类型计算 statement RU；`FlatPhysicalPlan`/`FlatOperator` 拥有轻量 `PlanNode` 摘要，供 Rust 侧计划编码、摘要和 EXPLAIN 展示。两套表示不能互换：前者保留可 downcast 的具体算子和请求类型，后者保留展示字段并可克隆。

## 核心职责

1. `FlattenTypedPhysicalPlan` 对真实计划做前序深度优先展开，识别 reader、join、DML、外键、shuffle receiver、Execute/Explain 包装等特殊结构，并保留对原算子的借用。
2. `FlattenTypedPhysicalPlanForest` 在主树之外收集 CTE 定义和注册的标量子查询，形成 statement RU 所需的完整森林；CTE 按 `IDForStorage` 去重。
3. `FlattenPhysicalPlan` 对 `PlanNode` 摘要树做深度优先展开，为每个节点计算子节点下标、层级、Build/Probe 或 Seed/Recursive 标签、root/store 信息和文本树兄弟位置。
4. `GetSelectPlan` 从 DML 主树中切出真正的 SELECT 区段，排除前置 Insert/Update/Delete 和后置 FKCheck/FKCascade，供规范化编码使用。
5. `ExplainFlatPlanInRowFormat` 和 `ExplainFlatPlanInRUFormat` 把摘要表示转为 EXPLAIN 行；`NewExplainRUResult` 为每次算子出现创建按位置对齐的 RU 结果，避免用重复 plan ID 合并不同出现位置。

## 主要符号

- `TypedFlatPhysicalPlan<'a>`：真实计划森林，含 `Main`、多棵 `CTEs` 和多棵 `ScalarSubQueries`。生命周期 `'a` 表明所有算子均借用调用方持有的计划与标量子查询注册表。
- `TypedFlatOperator<'a>`：保存 `Origin: &'a dyn base::Plan`、直接子节点起始下标、整个子树终点 `ChildrenEndIdx`、执行位置的 `StoreType`/`ReqType` 及 join/CTE/文本树元数据。
- `TypedOperatorLabel`：typed 路径的 `Empty`、`BuildSide`、`ProbeSide`、`SeedPart`、`RecursivePart` 标签。
- `FlattenTypedPhysicalPlan(plan)`：真实算子单树入口。遇到 `RuntimeExecute` 或 `RuntimeExplain` 时直接展开内部目标；无法识别为物理计划或允许的特殊叶节点时返回 `None`。
- `FlattenTypedPhysicalPlanForest(plan, scalar_subqueries)`：森林入口。它从主树发现 root 位置的 `PhysicalCTE`，递归追加 seed/recur 分支，并把能 downcast 为 `ScalarSubqueryEvalCtx` 的注册项转成独立树。
- `FlatPlanTree = Vec<FlatOperator>`、`FlatPhysicalPlan`：摘要路径的线性树和容器。容器包含 `Main`、单个 `CTE`、单个 `ScalarSubQ` 以及展示标志；当前 `FlattenPhysicalPlan` 本身只填充 `Main`。
- `FlatOperator`：拥有克隆的 `PlanNode`，记录直接子节点下标、`NeedReverseDriverSide`、标签、root/store、层级和最后兄弟标记；`ExplainID` 由计划种类名称和 ID 拼成。
- `OperatorLabel`：摘要路径标签；其 `Display` 是 EXPLAIN 的稳定文本契约，例如 `(Build)`、`(Probe)`。
- `ExplainRUOperatorResult`/`ExplainRUResult`：把每个 `FlatOperator` 出现位置与 self/cumulative RU 绑定，并保存全语句 `TotalRU`。
- `FlattenPhysicalPlan`、内部 `child_labels` 与 `flatten_recursively`：摘要树展开入口、标签决策和递归实现。
- `ExplainFlatPlanInRowFormat`、`ExplainFlatPlanInRUFormat`、`format_bytes`：普通/verbose/analyze/RU 行输出及字节格式化辅助函数。

## 执行流程

typed 主流程从 `FlattenTypedPhysicalPlanForest` 调用 `FlattenTypedPhysicalPlan` 得到主树。递归 `append` 先把当前节点写入向量，再计算 reader 上下文和 join 标签，随后按原 children 顺序前序展开；reader 的孩子切换为非 root 并继承 TiKV/TiFlash 与 Cop/BatchCop/MPP 请求类型。`PhysicalIndexLookUpReader` 和 `PhysicalIndexMergeReader` 还标记 INL probe 子树。普通 physical children 之后，代码追加 Insert/Update/Delete 的 SELECT、外键检查与级联，以及 `PhysicalShuffleReceiverStub::DataSource` 等不在 `PhysicalPlan::children()` 中的逻辑孩子，最后写回直接孩子下标与子树终点。

森林阶段扫描主树中 `IsRoot` 的 `PhysicalCTE`，以队列处理嵌套定义并用 `IDForStorage` 去重。每棵定义树先展开定义节点，再由局部 `attach` 追加 seed 和可选 recursive 分支；`attach` 会把分支下标整体加上当前偏移。标量子查询注册项必须能 downcast 为 `ScalarSubqueryEvalCtx`，否则被跳过；合法项先建立上下文根节点，再附加实际子查询计划。

摘要主流程由 `FlattenPhysicalPlan` 初始化 level 0/root/TiDB/last-child 上下文，调用 `flatten_recursively`。递归函数先追加当前节点，再由 `child_labels` 根据 `PlanKind` 分配标签；当 `build_side_first` 为真时 Build 子节点排序到前，否则对 Probe/Build 原顺序设置 `NeedReverseDriverSide`。每个孩子的 `Level` 加一；Table/Index reader 类节点之下切换为非 root。递归返回的索引依次写入父节点 `ChildrenIdx`，因此数组维持前序深度优先顺序。

展示阶段中，行格式只遍历 `flat.Main`：依据 `Level` 和 `IsLastChild` 生成 `├─`/`└─` 前缀，基础列为 ID、估计行数、store、访问对象和算子信息；`analyze` 插入实际行数并追加执行、内存、磁盘列，`verbose` 插入代价与公式。RU 格式优先消费 occurrence-aligned 的 `ExplainRUResult`；结果为空、主树为空或任一条目缺少 operator 时，回退到当前 flat 森林并将三列 RU 留空。

## 数据与状态

两种扁平树都遵守前序布局：父节点出现在其所有后代之前，`ChildrenIdx` 只保存直接孩子的起始位置。typed 表示额外用 `ChildrenEndIdx` 标出整个后代区间；摘要表示用每个节点的 `Level` 支持文本缩进。扩展递归逻辑时必须同时维持这些下标不变量。

`IsRoot` 不是“数组第一个元素”的同义词，而是执行站点。reader 的远端计划子树会成为非 root，DML 的 SELECT/外键子计划和 shuffle receiver 数据源则作为新的 root 子树。`StoreType` 与 typed 路径的 `ReqType` 必须在跨 reader 边界时一并切换。

`NeedReverseDriverSide` 只描述两个孩子以 Probe、Build 顺序保存且调用方没有要求 build-first 的情形；已经在展开时重排后不应再反转。`NewExplainRUResult` 克隆摘要算子到结果中，以数组坐标而非 plan ID 对齐 RU，因此重复 ID 仍是不同 occurrence。

`FlatPhysicalPlan::InExecute`、`TryFastPlan` 和 `BuildSideFirst` 是摘要容器字段；本文件当前只在 `FlattenPhysicalPlan` 初始化 `BuildSideFirst`。typed 路径会主动解包 Execute/Explain，但不会在返回结构中保存相应布尔状态。

## 依赖与调用关系

直接 crate 依赖来自 `pkg/planner/core/Cargo.toml`：`base-dependency` 提供 `Plan`/join 类型，`physicalop-dependency` 提供具体物理算子和 `ReadReqType`，`kv-dependency` 提供存储类型；本 crate 自身提供 `PlanKind`、`PlanNode`、运行时 Execute/Explain/Analyze/Simple 和 `ScalarSubqueryEvalCtx`。

生产上游之一是 `pkg/executor/adapter.rs`：`finishStatementRU` 和 `SnapshotStatementRUEvidence` 调用 `FlattenTypedPhysicalPlanForest`，分别用于计算完整 statement RU 和冻结各算子 ID 的运行时证据；`ExecStmt::TypedFlatPlan` 调用 `FlattenTypedPhysicalPlan` 暴露真实算子线性视图。下游是 `pkg/executor/statement_ru_plan_walk.rs` 的森林/单树 RU 遍历逻辑。

摘要路径由 `pkg/planner/core/common_plans.rs::Explain::RenderResult` 调用：先 `FlattenPhysicalPlan`，再按格式路由到 row 或 RU 渲染。`pkg/planner/core/encode.rs` 的 `EncodePlan`、`NormalizePlan`/`NormalizeFlatPlan` 也依赖该表示和 `GetSelectPlan`。模块入口在 `pkg/planner/core/lib.rs` 公开导出全部符号。

RustCodeGraph 将本文件标为被 `pkg/executor/adapter.rs`、`pkg/executor/statement_ru_plan_walk.rs`、其测试及 `pkg/planner/core/common_plans.rs` 等文件使用；对 `FlattenTypedPhysicalPlanForest` 的 callee 图确认其调用局部 `attach` 和 `FlattenTypedPhysicalPlan`，对 `FlattenTypedPhysicalPlan` 的 callee 图确认其进入局部 `append`。

## 错误处理与边界

本文件不构造业务错误类型。两个 flatten 入口使用 `Option` 表达“没有计划”或“无法完整展开”：`FlattenPhysicalPlan(None, ..)` 返回 `None`；typed `append` 遇到既非物理计划、也非 Insert/Update/Delete/FKCheck/FKCascade/RuntimeAnalyze/RuntimeSimple 的节点时返回 `None`，并通过 `?` 使整棵树失败。Execute/Explain 包装没有内部目标也会沿调用链失败。

join 标签只在孩子数量符合预期时设置；typed 路径还检查 `inner < 2`，避免错误下标。标量注册表中类型不匹配的项目被安静跳过，而不是让整个森林失败。CTE 用 storage ID 去重，避免嵌套引用重复展开或循环追加。

`GetSelectPlan` 对空树和只有 DML 节点的树返回空切片与偏移 0；只在看到 DML 前缀后才把 FKCheck/FKCascade 当作 SELECT 结束边界。RU 渲染只有在主结果非空且三棵结果向量中的所有条目都有 operator 时才接受结果，否则整批回退为空 RU 列，避免部分或错位数据混入输出。总 RU 非正数时百分比固定为 `0.00%`。`format_bytes` 对负数输出 `N/A`，不足 1024 的值用 Bytes，其余只换算到 KB。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道或 I/O。所有展开都在调用线程同步完成，临时 `Vec`/`HashSet` 随调用结束释放。

typed 结构通过生命周期借用原始计划，不能比 `plan`、CTE 定义或标量注册表中的 `Rc` 活得更久；`FlattenTypedPhysicalPlanForest` 特意接收调用方持有的 registry snapshot，避免把临时 `Rc` 内部引用放进自引用结果。它不克隆具体算子，也不取得所有权。摘要结构则克隆 `PlanNode`，`NewExplainRUResult` 再克隆 `FlatOperator`，从而能保存一次确定的 occurrence 快照，但这也意味着调用方应关注大计划的分配与复制成本。

代码只读取传入计划。CTE 队列和 `seen` 集合是函数局部状态；statement RU 的证据发布、并发保护和终态管理发生在上游 `pkg/executor/adapter.rs`，不属于本文件职责。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/flat_plan.go`，展示与 RU 对照还涉及 `pkg/planner/core/common_plans.go`。两端共同遵守前序展开、children 下标、reader 执行站点、join Build/Probe、CTE Seed/Recursive、DML SELECT 与外键尾部以及 Execute/Explain 解包等核心语义。

typed Rust 路径最接近 Go 的 `FlatPhysicalPlan`：它保留真实算子，支持多棵 CTE/标量子查询树，并覆盖 reader、DML、外键、shuffle 等特殊孩子。Rust 摘要路径则是额外的轻量表示；其 `FlatPhysicalPlan` 当前把 `CTE`、`ScalarSubQ` 各表示为单个向量，且 `FlattenPhysicalPlan` 只自动生成 `Main`，所以不能声称与 Go 的 `CTEs []FlatPlanTree`、`ScalarSubQueries []FlatPlanTree` 完全等价。

仍需注意的可见差异包括：Go `FlatOperator` 保存 `ReqType`、`ChildrenEndIdx`、`TextTreeIndent`、`IsINLProbeChild` 和 `IsPhysicalPlan`，Rust 摘要算子没有这些字段；Go row EXPLAIN 遍历 main、所有 CTE 与所有标量子查询，Rust `ExplainFlatPlanInRowFormat` 当前只遍历 `Main`；Go 字节格式化和运行时统计由更完整的 formatter/provider 处理，Rust 摘要实现只提供 Bytes/KB 与已存入 `PlanNode` 的字段。文档将这些视为当前迁移边界，而不是预期设计。

`pkg/planner/core/flat_plan_test.rs` 验证 DML/FK 切片、标签字符串、right outer merge join、root 继承、RU 有值/无值和 RuntimeAnalyze/RuntimeSimple typed 叶节点。`pkg/planner/core/casetest/flatplan/flat_plan_test.rs` 以手工 `PlanNode` 树覆盖单表、HashJoin 重排、CTE 标签、嵌套层级和文本前缀；对应 Go `flat_plan_test.go` 则通过 parser→resolve→optimizer→flatten 的真实 SQL/golden 链路验证主树及多棵 CTE。

## 扩展指南

- 新增物理算子时，先判断它是普通 `PhysicalPlan::children()` 节点还是拥有隐藏孩子的容器。后者必须在 `FlattenTypedPhysicalPlan::append` 的 special-children 分支接线，并在需要时同步 reader store/request、root 和 INL probe 语义。
- 新增 join 或改变 build/probe 约定时，要同步 typed `append` 与摘要 `child_labels`；同时覆盖 `build_side_first=false/true`、`NeedReverseDriverSide`、孩子实际顺序和标签。Go 对照逻辑位于 `flat_plan.go::flattenRecursively`。
- 扩展 CTE/标量子查询时，要保持 CTE storage ID 去重、`attach` 下标偏移和 borrowed registry 生命周期。若让摘要路径自动形成多棵森林，需要先调整其数据模型及 `common_plans.rs`、`encode.rs` 消费者，不能把多棵树直接拼成一棵而破坏局部下标。
- 扩展 EXPLAIN 列时，要明确 row/analyze/verbose/RU 的列顺序，并同步 `Explain::RenderResult`、编码消费者与 Go 输出契约。RU 仍应按 occurrence 对齐，不能退化成以 plan ID 为 key。
- 测试逻辑应放在独立文件：核心回归优先补 `pkg/planner/core/flat_plan_test.rs`；SQL/golden 语义补 `pkg/planner/core/casetest/flatplan/flat_plan_test.rs` 及对应测试数据；typed statement RU 的 reader/CTE/标量子查询覆盖补 `pkg/executor/statement_ru_plan_walk_test.rs`。不要把测试内嵌回本生产文件。
- 性能风险集中于大树递归深度、`Vec` 多次增长、摘要 `PlanNode` 克隆以及 `build_side_first` 排序；兼容风险集中于 EXPLAIN 文本、下标稳定性、Go 标签/顺序和远端执行站点判定。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；读取 `node --file pkg/planner/core/flat_plan.rs` 得到完整 824 行源码。
- RustCodeGraph 查询：`explore "pkg/planner/core/flat_plan.rs symbols responsibilities callers callees FlatPlan FlatOperator"`；`callees FlattenTypedPhysicalPlanForest`；`callees FlattenTypedPhysicalPlan`；`callees ExplainFlatPlanInRUFormat`；并尝试对六个公开入口执行 callers 查询。精确 callers 无输出时，以索引的 “used by 6 files” 结果和仓库引用搜索补齐调用点。
- 已读 Rust 生产路径：`pkg/planner/core/flat_plan.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/common_plans.rs`、`pkg/planner/core/encode.rs`、`pkg/executor/adapter.rs`；调用关系还由 RustCodeGraph 指向 `pkg/executor/statement_ru_plan_walk.rs`。
- 已读边界声明：`pkg/planner/core/Cargo.toml`；该包不存在 `pkg/planner/core/doc.go`。
- 已读 Go 对照：`pkg/planner/core/flat_plan.go`、`pkg/planner/core/common_plans.go`、`pkg/planner/core/casetest/flatplan/flat_plan_test.go`。
- 已读独立 Rust 测试：`pkg/planner/core/flat_plan_test.rs`、`pkg/planner/core/common_plans_test.rs`、`pkg/planner/core/casetest/flatplan/flat_plan_test.rs`；引用搜索还确认 `pkg/planner/core/plan_test.rs`、`encode_test.rs`、`logical_plans_test.rs` 和 `pkg/executor/statement_ru_plan_walk_test.rs` 的覆盖点。
- 本任务是纯文档分析，依计划不运行 Cargo。交付前使用任务规定的 `rg -c` 命令验证恰有 11 个固定二级章节，并人工复核所有“已支持”描述均能回指上述源码、调用边、Go 对照或独立测试。
