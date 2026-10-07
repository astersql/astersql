# `pkg/planner/core/logical_plan_builder.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-core`（`pkg/planner/core/Cargo.toml`），由 crate 根 `pkg/planner/core/lib.rs` 以公开模块 `pub mod logical_plan_builder` 挂载。它在 `PlanBuilder` 上实现一套以轻量 `task::{PlanNode, Expression, FieldType}` 为数据模型的逻辑计划构建骨架，并定义与该骨架配套的 SELECT、JOIN、集合运算、窗口、CTE、UPDATE 和 DELETE 输入类型。

需要特别区分两条路径：本文件头注释与 `lib.rs` 都将它标为“简化/骨架实现”；解析真实 parser AST、构造 `logicalop` 计划的完整运行时实现位于同 crate 的 `logical_plan_builder_runtime.rs` 和 `planbuilder_runtime.rs`。因此，本文件可由公开 API、同 crate 的 `integration_test.rs` 以及若干单元测试直接调用，但不能据此认定其中每个轻量分支都是服务器 SQL 入口当前采用的完整实现。Go 的语义来源是同路径 `pkg/planner/core/logical_plan_builder.go`，Rust 运行时实现是另一个需要同步核对的落点。

## 核心职责

- 定义轻量输入 AST：`SelectStmt`、`ResultSet`、`JoinSpec`、`TableSource`、`SetOprType`、`WindowSpec`、`CteDef`、`UpdateStmt`、`DeleteStmt` 等；这些结构不是 `parser::ast` 类型，而是本文件骨架自己的可测试表示。
- 把输入递归组装成 `PlanNode` 树。核心分派是 `buildResultSetNode`，主 SELECT 管线是 `buildSelect`，DML 入口是 `buildUpdate` 与 `buildDelete`。
- 在构建时维护 `PlanBuilder` 状态：通过七个 `Flag*` 位累加优化需求，通过 `visitInfo` 记录表级 SELECT 权限，通过 `outerCTEs` 维护 CTE 可见性/递归状态，通过 `warnings` 和 `unusedViewHints` 保存提示相关信息。
- 维护 schema、输出名、粗略基数与算子属性，包括 JOIN schema 合并、UNION 类型归并、窗口输出追加、LIMIT 行数上界、存储偏好和 UPDATE/DELETE 的列/handle 位置信息。
- 提供与 Go 同名的解析/校验辅助函数，例如 `extractLimitCountOffset`、`buildWindowSpecs`、`mergeWindowSpec`、`CheckUpdateList`、`appendVisitInfo` 和 `resetCTECheckForSubQuery`。

## 主要符号

- 优化标志：`FlagPredicatePushDown`、`FlagBuildKeyInfo`、`FlagDecorrelate`、`FlagConstantPropagation`、`FlagPushDownAgg`、`FlagPushDownTopN`、`FlagEliminateOuterJoin`。`buildSelection`、`buildAggregation`、`buildLimit`、`buildApplyWithJoinType` 等按需置位，供后续优化阶段判断。
- 结果集模型：`ResultSet::{Table, Join, Select, SetOperation, Values, Cte}` 是 `buildResultSetNode` 的封闭分派集合；`SelectStmt` 汇集 FROM、投影、过滤、分组、窗口、CTE、排序和 LIMIT。
- 计划主入口：`PlanBuilder::buildSelect`、`buildResultSetNode`、`buildJoin`、`buildDataSource`、`buildSetOpr`、`buildUpdate`、`buildDelete`。
- 关系算子构建：`buildSelection`、`buildProjection`、`buildAggregation`/`buildExpand`、`buildDistinct`、`buildSort`、`buildLimit`、`buildApplyWithJoinType`、`buildSemiJoin`、`buildUnionAll`。
- 窗口处理：`buildWindowSpecs` 解析命名窗口引用，`mergeWindowSpec` 执行继承约束，`groupWindowFuncs` 按规格分组，`buildWindowFunctions` 为每组先补投影再追加 `Window` 节点；`specEqual` 比较分区键、排序键和完整 frame。
- CTE 处理：`buildWith`/`buildCte`/`buildRecursiveCTE` 构建定义，`tryBuildCTE` 解析可见引用，`prepareCTECheckForSubQuery` 与 `resetCTECheckForSubQuery` 管理递归子查询保护，`tryToBuildSequence` 为需要物化的 CTE 增加 `Sequence`/`CteStorage`。
- DML 辅助：`buildUpdateLists` 校验列范围与重复赋值；`buildUpdateTblColPosInfos`、`pruneAndBuildColPositionInfoForDelete`、`resolveIndicesForTblID2Handle` 维护目标表、可写列和 handle 下标。
- 遍历辅助器：`aggOrderByResolver`、`userVarTypeProcessor`、`resolveGroupingTraverseAction`、`correlatedAggregateResolver`、`gbyResolver` 及各类列名/表名 resolver。它们以 `Enter`/`Leave`/`Transform` 形式保存遍历状态，但遍历框架本身不在本文件中实现。
- 私有基础函数：`schema_names`、`column_exprs`、`merge_schema`、`expr_matches_column`；末尾私有 trait `SaturatingSubF64` 仅为 LIMIT 基数计算提供浮点饱和减法。

## 执行流程

`buildSelect` 的固定顺序是：先由 `buildWith` 登记并构建 CTE；`buildTableRefs` 将 FROM 交给 `buildResultSetNode`，缺省时生成单行 `TableDual`；随后依次处理 WHERE、聚合/GROUP BY、投影、DISTINCT、HAVING、窗口、ORDER BY、LIMIT；最后由 `tryToBuildSequence` 把物化 CTE 存储节点放在查询计划之前。该顺序解释了测试中 `Projection -> HashAgg -> Limit -> Projection -> Selection -> IndexScan` 等由根到叶的计划形状。

`buildResultSetNode` 对基表调用 `buildDataSource`，对 JOIN 递归构建左右输入，对子查询递归调用 `buildSelect`，对集合运算调用 `buildSetOpr`，对 VALUES 以行数标注 `TableDual`，对 CTE 调用 `tryBuildCTE`。`buildJoin` 在右侧含 LATERAL 时改走 `buildLateralJoin`/`Apply`；普通 JOIN 合并 schema、估算行数，再处理 NATURAL 或 USING 等值条件。

聚合路径先用 `buildExpand` 识别 `grouping_set:` 表达式并增加 grouping-id 列，再由 `buildAggregation` 构造 `HashAgg`。DISTINCT 由 `buildDistinct` 转为按可见列分组、对每列执行 `first_row` 的聚合。集合运算先分别构建各 SELECT；UNION ALL 合并子计划并以 `unionJoinFieldType` 统一每列类型，UNION 再叠加 DISTINCT，INTERSECT/EXCEPT 分别转为 Semi/AntiSemi Join。

窗口路径由 `buildWindowSpecs` 递归展开命名规格，拒绝环和非法覆盖；`groupWindowFuncs` 把等价规格的函数归组；每组经 `buildProjectionForWindow` 补齐分区、排序和参数表达式，再创建 `Window` 节点。UPDATE/DELETE 则共同执行“源结果集 -> WHERE -> ORDER BY -> LIMIT”，随后分别校验赋值列表或目标表列位置，并包装为名为 `Update`/`Delete` 的 `PlanKind::Other`。

## 数据与状态

`PlanNode` 是所有构建函数之间传递的所有权对象。各函数通常消费子计划，将其放入 `children`，并同步复制或重建 `schema`、`stats`、`conditions`、`expressions`、`by_items`、`labels` 与 `store`。`labels` 在骨架中承载列名到下标、索引首列、CTE 名、赋值数、提示结果等轻量元数据；扩展标签时必须检查既有字符串约定，例如 `column:`、`index-leading:`、`cte:`。

`PlanBuilder` 的相关可变状态定义于 `planbuilder.rs`：`optFlag` 是累计位图；`visitInfo` 与 `warnings` 按构建顺序追加；`outerCTEs` 中的 `cteInfo` 保存 `nonRecursive`、`useRecursive`、`isBuilding`、`enterSubquery`；`noDecorrelate` 决定 Apply 的缓存标志；`unusedViewHints` 同时被 `pushTableHints`/`popTableHints` 当栈使用。复用构建器前应走 `PlanBuilder::ResetForReuse`，它会清空上述请求级状态。

类型归并有明确不变量：外连接的可空侧通过 `merge_schema` 清除 `unsigned` 标志以容纳 NULL；UNION/递归 CTE 逐列调用 `unionJoinFieldType`，列数不等直接报错；投影、窗口、视图和 DML 的列下标都必须落在当前 schema 范围内。基数只是骨架估算，例如普通过滤乘 `0.8`、可用索引乘 `0.1`、聚合按分组数收缩，不能当作完整统计模块的代价结论。

## 依赖与调用关系

直接 Rust 依赖只有同 crate 的 `planbuilder`（`PlanBuilder`、错误、权限、表/值/CTE 状态）和 `task`（表达式、类型、计划节点、统计与存储类型），外加标准库 `HashMap`/`HashSet`。更宽的 planner crate 依赖由 `Cargo.toml` 声明，包括 parser AST、expression、logicalop/physicalop、statistics、property、rule、infoschema、table、kv 和 session context；本文件本身没有直接引用这些外部 crate 名称。

RustCodeGraph 对 `buildSelect` 给出的本文件内部下游边包括 `buildWith`、`buildTableRefs`、`buildSelection`、`extractAggFuncsInSelectFields`、`buildAggregation`、`buildProjection`、`buildDistinct`、`resolveHavingAndOrderBy`、`resolveWindowFunction`、`buildWindowFunctions`、`buildSort`、`buildLimit` 和 `tryToBuildSequence`。源码中的递归上游包括 `buildResultSetNode`、`buildSetOpr`、视图构建、CTE 构建/合并；仓库文本搜索还确认 `integration_test.rs` 直接调用 `buildSelect`、`buildResultSetNode`、`buildUpdate` 与 `buildDelete`。

完整应用主链另由 `planbuilder_runtime.rs` 接收真实语句并调用运行时 `buildUpdate`/`buildDelete` 等实现；本骨架模块虽为 `pub`，却不是该真实 AST 调用边的替代品。新增行为若面向服务器 SQL，必须同时定位 `logical_plan_builder_runtime.rs`/`planbuilder_runtime.rs` 和 Go 实现，不能只修改本文件后宣称运行时已支持。

## 错误处理与边界

可失败入口统一返回 `planbuilder::Result<T>`，错误是字符串包装 `BuilderError`，并用 `?` 向上传播。主要边界包括：USING 缺列、投影/赋值/handle 下标越界；UNION、集合运算、视图或递归 CTE 列数不一致；空集合输入；DISTINCT 键过长；ORDER BY 不在 DISTINCT 可见列中；负数或非整数 LIMIT；GROUP BY 位置越界及 ONLY_FULL_GROUP_BY 违规；窗口参数数目、frame 偏移、继承、重名、未知引用和循环引用错误；递归 CTE 的非法子查询引用。

常量过滤有专门退化：WHERE 为 `true` 原样返回，`false` 返回同 schema 的零行 `TableDual`；LIMIT 的 offset/count 用饱和逻辑避免 `u64` 溢出，结果总量为零时也退化为 Dual。存储提示同时偏好 TiFlash 与 TiKV 时不失败，而是向 `warnings` 追加冲突信息并按 `buildDataSource` 当前分支选择 TiFlash；调用方若需要严格提示语义必须读取警告。

有一处值得扩展时特别谨慎：`buildIntersect` 的 reduce 闭包对 `buildSemiJoinForSetOperator` 使用 `unwrap()`，与其余 `Result` 传播风格不同；在列数不匹配时可能 panic。本文只记录现状，不把它解释为安全错误边界，也未在纯文档任务中修改实现。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄或网络资源；所有计划、表达式和 schema 都是同步、内存内的拥有值或借用切片。`PlanBuilder` 通过 `&mut self` 串行修改请求级状态，因此同一个实例不应被并发共享构建；类型本身也没有在此声明并发保证。

需要成对维护的生命周期主要是逻辑栈：CTE 在 `buildCte` 开始时以 `isBuilding=true` 推入 `outerCTEs`，成功构建并调整输出名后才改为 false；子查询检查由 `prepareCTECheckForSubQuery` 返回被修改下标，调用侧应以 `resetCTECheckForSubQuery` 恢复；提示和权限列表分别有 push/pop/truncate 辅助函数。当前 `buildCte` 在中途 `?` 返回错误时不会执行尾部的 `isBuilding=false`，因此错误后复用同一 builder 必须依赖外层丢弃或 `ResetForReuse`，不能假设状态已自动回滚。

计划树由值所有权维持资源生命周期。大部分节点移动子计划避免共享可变状态；少数路径（如 `tryToBuildSequence`）会 clone 结果计划。对大计划增加新 clone 会有内存和 CPU 风险，应优先保持现有“消费并挂到 children”的模式。

## 与 Go 版本的对应关系

`Cargo.toml` 的 `[package.metadata.porting]` 明确把本 crate 对应到 Go 包 `pkg/planner/core`。Go 同路径文件中存在对应入口：`buildAggregation`、`buildJoin`、`buildSelection`、`buildSetOpr`、`extractLimitCountOffset`/`buildLimit`、`buildSelect`、`buildDataSource`、`buildUpdate`/`buildUpdateLists`、`buildDelete`、`buildWindowFunctions`、`mergeWindowSpec`、`buildWindowSpecs`、`appendDynamicVisitInfo`、`appendVisitInfo`、`buildWith` 和 `getResultCTESchema`。

Rust 骨架保持了 Go 的高层顺序和若干可观察规则：窗口继承不能继承带 frame 的父规格，子规格不能覆盖 PARTITION BY；命名窗口大小写不敏感且禁止环；LIMIT 只接受非负整数并饱和处理 offset+count；集合运算按列数与类型对齐；LATERAL 转 Apply；UPDATE/DELETE 在修改前应用过滤、排序和限制；权限信息维持追加顺序。`logical_plan_builder_test.rs` 明确以 “like_go” 测试窗口继承、排序项和 frame 等价性。

但两者并非等量实现。Go 文件接收完整 parser AST、表达式/Schema/统计/提示/权限上下文并构造正式 logical plan；本文件使用字符串前缀（如 `agg:`、`window:`、`grouping_set:`）和简化基数来模拟语义。实际 Rust 运行时移植位于 `logical_plan_builder_runtime.rs`。因此 Go 新增逻辑的正确迁移路径是：先判断行为属于骨架测试面、正式运行时面还是两者，再分别保持输入模型与错误语义一致，禁止把字符串骨架当成完整 AST 实现。

## 扩展指南

新增 ResultSet/语句形态时，先扩展对应 enum/struct，再同步 `buildResultSetNode` 或顶层 DML 入口，并检查 `ExtractTableList`/`collectTableName`、LATERAL 检测和表权限收集是否也需要覆盖新变体。新增算子必须设置正确的 `schema`、`stats`、`children` 及必要优化标志；JOIN、UNION、窗口和 DML 尤其要验证列下标重映射与可空性。

修改 SELECT 阶段顺序时以 `buildSelect` 为唯一骨架主线，确认 HAVING、窗口、DISTINCT、ORDER BY、LIMIT 的相对位置，并在独立测试中断言完整计划形状而非只断言构建成功。窗口扩展应同步 `specEqual`、`mergeWindowSpec`、`resolveWindowSpec`、默认 frame 和参数校验；CTE 扩展应同时检查可见性、递归保护、物化/内联选择及错误后的状态清理。

测试不得内嵌到本源文件。骨架级回归优先放在同目录独立文件 `logical_plan_builder_test.rs`，跨模块轻量计划用例可沿用 `integration_test.rs` 的现有 typed-plan 区域；正式 parser AST/`logicalop` 行为则应扩展 `logical_plan_builder_runtime_test.rs`、相关 `*_aster_unit_test.rs` 或 `casetest/logicalplan/logical_plan_builder_test.rs`，并与 Go 的 `logical_plan_builder_test.go`/`planbuilder_test.go` 对照。服务器可见行为还必须同步审查运行时实现，而不是仅覆盖骨架 API。

兼容风险集中在错误文本、schema 类型/列序、优化标志和 CTE 状态；性能风险集中在计划 clone、窗口分组的重复扫描与大 UNION 的逐子计划投影。扩展前应通过 RustCodeGraph 查询目标符号 callers/callees，并搜索 `PlanKind`、标签字符串和 `optFlag` 消费者，避免产生无人读取的元数据。

## 验证依据

- 源码全量阅读：`pkg/planner/core/logical_plan_builder.rs`（2699 行）；构建器状态与错误类型：`pkg/planner/core/planbuilder.rs`；轻量计划模型：`pkg/planner/core/task.rs`。
- crate 边界：`pkg/planner/core/Cargo.toml`（包名、依赖、`nextgen` feature、Go porting 元数据）与 `pkg/planner/core/lib.rs`（公开骨架模块、私有运行时模块及测试挂载）。目标包没有顶层 `pkg/planner/core/doc.go`；检索到的 `base/doc.go` 属于子目录 `base`，不作为本模块契约替代品。
- RustCodeGraph：`status` 显示索引含 11467 文件、307296 节点、1848419 边；`node --file ... --offset 1 --limit 400` 核对文件开头、类型和首批实现；`query PlanBuilder` 定位 `planbuilder.rs:568`；`query buildSelect` 与 `callees buildSelect` 核对本文件 `buildSelect` 及其 13 条主要下游边；`query buildResultSetNode` 定位 Go 同名入口，图对本骨架部分外部 caller 覆盖有限，故按技能规则用仓库文本搜索补充直接测试调用证据。
- Go 对照：`pkg/planner/core/logical_plan_builder.go` 与 `pkg/planner/core/planbuilder.go`；同名函数位置通过 `rg '^func ...'` 核对。Go 测试参考 `pkg/planner/core/logical_plan_builder_test.go`、`pkg/planner/core/planbuilder_test.go` 和 `pkg/planner/core/casetest/logicalplan/logical_plan_builder_test.go`。
- Rust 测试：`pkg/planner/core/logical_plan_builder_test.rs`（窗口继承/比较/frame）；`logical_plan_builder_subquery_join_aster_unit_test.rs`（Apply schema 与标志）；`logical_plan_builder_lateral_with_aster_unit_test.rs`（正式运行时 LATERAL/递归 CTE）；`logical_plan_builder_runtime_test.rs` 与 `logical_plan_builder_runtime_aster_unit_test.rs`（集合运算、LIMIT、DISTINCT）；`integration_test.rs`（typed SELECT/UNION/UPDATE/DELETE、索引选择、HAVING 等直接骨架调用）。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务规定的章节结构命令，并人工复核：恰有 11 个固定二级标题、没有把骨架实现表述为完整运行时、所有扩展建议指向独立测试文件。
