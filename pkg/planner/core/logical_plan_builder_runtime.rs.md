# `pkg/planner/core/logical_plan_builder_runtime.rs`

## 文件定位

本文件是 `astersql-planner-core` crate 中把解析器结果集 AST 降低为真实 `logicalop::LogicalPlanRef` 的运行时构建器。模块由 `pkg/planner/core/lib.rs:153` 私有装配；其中 `resetCTECheckForSubQuery` 与 `appendDynamicVisitInfo` 又在 `lib.rs:217` 对 crate 外重导出。默认入口 `BuildResultSetNode` 在 `pkg/planner/core/planbuilder_runtime.rs:758` 注册进 `PlanBuilder::withResultSetBuilder`，而同文件的若干 DML/查询路径直接调用 `BuildBorrowedResultSetNode`（例如 `planbuilder_runtime.rs:1018,1035,1078,1202,1655`）。因此它位于“parser AST → PlanBuilder → 逻辑算子树 → 逻辑优化规则”的主链中，而不是优化器规则本身。

`pkg/planner/core/Cargo.toml` 声明 crate 名为 `astersql-planner-core`、库入口为 `lib.rs`、`autotests = false`。本文件直接使用该 crate 的 `PlanBuilder`、AST、表达式重写器与 CTE 检查逻辑，并依赖 `astersql-expression`、`astersql-expression-aggregation`、`astersql-planner-core-base`、`astersql-planner-core-operator-logicalop`、`astersql-planner-core-rule`、`astersql-util-hint`、`astersql-types` 等路径依赖。代码本身没有条件编译项；Cargo 的 `nextgen` feature 只向配置依赖透传，未在本文件形成分支。

RustCodeGraph 的文件节点报告该文件共 6264 行、约 1043 个索引符号，并列出 `pkg/server/runtime.rs`、`pkg/session/runtime/planning.rs`、`pkg/store/copr/coprocessor_test.rs` 三个文件级使用者。符号级 callers/callees 对动态 trait 回调和大量同名 Go 风格方法解析不完整，所以上述入口边同时以 `lib.rs` 与 `planbuilder_runtime.rs` 的直接引用复核。

## 核心职责

1. **结果集分派与作用域管理**：`BuildResultSetNode`/`BuildBorrowedResultSetNode` 接收 AST，保存并隔离 `runtimeCTEs`，建立 handle 映射，分派 `SELECT`、集合运算、`SHOW`、`DO`，最后执行 `RecheckLogicalCTE`。
2. **完整 SELECT 降低**：`build_select_runtime_inner` 依次处理 WITH、FROM、相关子查询预构建、WHERE、锁、聚合、HAVING、窗口、投影、DISTINCT、ORDER BY 与 LIMIT，并设置后续逻辑优化所需的 `optFlag`。
3. **CTE 与集合运算**：构建普通/递归 CTE，决定单消费者内联或物化，校验递归成员的限制；将 UNION、INTERSECT、EXCEPT 降低为 UnionAll、Distinct 与半连接/反半连接组合，并统一分支类型。
4. **关系源与连接**：把表、派生表、CTE 与 join AST 降低为 `DataSource`、`LogicalJoin` 或 `LogicalApply`；处理 NATURAL/USING 公共列、外连接可空性、lateral 可见性、访问路径、索引及存储 hint。
5. **表达式分层**：围绕 `expression_rewriter` 解析 WHERE/HAVING、聚合、窗口、投影和排序表达式，维护列 ID、Schema、输出名与辅助列。
6. **边界算子与公共辅助接口**：构建 LIMIT、DISTINCT、Apply/SemiJoin/MaxOneRow，并提供 CTE 子查询状态复位和动态权限访问信息追加。

文件不是门面或桩：它直接创建并连接 `LogicalSelection`、`LogicalAggregation`、`LogicalWindow`、`LogicalProjection`、`LogicalSort`、`LogicalLimit`、`LogicalJoin`、`LogicalApply`、`LogicalCTE`、`LogicalUnionAll`、`DataSource` 等运行时计划节点。

## 主要符号

- `CteBinding`（第 42 行）保存 CTE 类引用、Schema、输出名、名称、递归引用标志、共享种子统计 `Arc<RwLock<StatsInfo>>`、存储 ID 与是否内联；自定义 `Clone` 保留共享类与统计对象，但按现有语义克隆 Schema/名称。
- `CteEnvironment = HashMap<String, CteBinding>`（第 69 行）是当前查询层可见 CTE 的按小写名索引环境。
- `BuildResultSetNode`（第 74 行）从可消费的 `NodeRef` 借用 AST；节点已被消费时返回明确错误。`BuildBorrowedResultSetNode`（第 85 行）是真正的默认回调入口。
- `build_query_node_with_ctes`（第 107 行）按 AST 动态类型分派 `SelectStmt`、`SetOprStmt`、`ShowStmt`、`DoStmt` 和单元素 `SetOprSelectList`。
- `build_show_runtime`、`build_do_runtime`（第 141、286 行）分别构造 SHOW 的输出 Schema/过滤/投影和 DO 的表达式投影。
- `build_set_operation_runtime`、`build_set_semi_join`、`build_union_runtime`、`build_union_all_runtime`（第 394、480、520、668 行）实现集合运算优先级、去重、NULL-safe 比较、分支强制转换和 UnionAll。
- `build_with_runtime`（第 1022 行）构建 WITH 定义；配套的引用计数、递归 ORDER/LIMIT 检查、递归投影和 CTE 输出名函数位于第 759–1155 行。
- `build_select_runtime`/`build_select_runtime_inner`（第 1217、1377 行）是 SELECT 总入口和主体；前者负责 hint 栈与 STRAIGHT_JOIN 状态，后者负责算子流水线。
- 窗口组（第 2130–3363 行）包括窗口输入辅助列、参数校验、frame 边界、命名窗口继承、窗口分组与前后投影；`collect_window_expressions`（第 3364 行）是 `pub(crate)` 收集器。
- 聚合组（第 3394–3949 行）收集聚合 AST、找出 HAVING 中聚合外列、构造聚合前后投影和 `LogicalAggregation`。
- `build_select_source`、`build_result_set_runtime`、`build_join_runtime`、`build_lateral_join_runtime`、`build_table_source_runtime`（第 3950、4032、4070、4365、4569 行）负责 FROM、Join/Lateral 和表源。
- `install_declared_index_paths`/`install_hypothetical_indexes`（第 5006、5111 行）把 INDEX/MERGE/HYPO_INDEX 及 TiKV/TiFlash hint 落入 `DataSource`。
- `build_select_projection`（第 5415 行）生成最终选择列表、辅助排序列和输出名；别名解析/相关聚合子查询预构建位于第 5639–5858 行。
- `build_limit_runtime`、`read_limit_value`、`build_distinct_runtime`（第 5888、5958、5988 行）是可独立测试的运行时边界构造器。
- `impl PlanBuilder`（第 6033 行）提供 `buildLimit`、`buildDistinct`、`buildApplyWithJoinType`、`buildSemiApply`、私有 `buildSemiJoin` 与 `buildMaxOneRow`。
- `resetCTECheckForSubQuery`、`appendDynamicVisitInfo`（第 6248、6256 行）是重导出的尾部辅助 API。

## 执行流程

主入口流程如下：

1. `BuildResultSetNode` 通过 `NodeRef::with_node` 取得只读 AST；`BuildBorrowedResultSetNode` 调用 `handleHelper.pushMap()`，复制外层 `runtimeCTEs`，以 `mem::replace` 暂存旧环境。
2. `build_query_node_with_ctes` 选择语句类别。未知类别返回“default result-set builder supports SELECT and set operations”；括号集合列表在此入口只允许单输入。
3. SELECT 先由 `build_select_runtime` 解析 query-block hint，压入 `tableHintInfo`，设置 STRAIGHT_JOIN 与子查询 hint flags；无论内部成功或失败，随后弹出 hint、收集未命中警告并恢复 straight-join 状态。
4. `build_select_runtime_inner` 先建立 WITH 环境并设置列裁剪标志，再由 `build_select_source` 形成 FROM 树。它在 WHERE 前为相关聚合预构建子查询源，随后重写谓词；恒假条件降为零行 `LogicalTableDual`，普通条件产生 `LogicalSelection`，缓存表的 `LogicalUnionScan` 同时保留一份条件。
5. 锁读在 FROM/WHERE 上方、聚合/窗口/最终投影下方插入 `LogicalLock`；FOR UPDATE 同时递归标记所有 `DataSource::IsForUpdateRead`。UPDATE 使用的 SELECT 形 AST 走专用短路，只处理 ORDER/LIMIT，避免重复最终投影。
6. 普通 SELECT 校验 `ONLY_FULL_GROUP_BY` 与 `GROUPING()` 参数，收集聚合和窗口表达式。聚合阶段建立 group-by/aggregate mapper、HAVING；窗口阶段解析命名窗口继承和 frame，按规格分组构造窗口节点，并用前后投影安置参数与最终输出。
7. `build_select_projection` 建立 SELECT 列、别名和 ORDER BY 辅助列；之后按语句属性附加 DISTINCT、Sort/TopN 优化标志与 LIMIT，并清理仅在当前查询定义的 CTE 名称。
8. `BuildBorrowedResultSetNode` 恢复调用前的 CTE 环境，对完整树执行 `RecheckLogicalCTE` 后返回。

集合运算流程保留 Go 的优先级：先把 INTERSECT 立即折叠成 `SemiJoin`，再把 EXCEPT 前面的 UNION 段收束并生成 `AntiSemiJoin`；INTERSECT ALL 与 EXCEPT ALL 明确报不支持。UNION DISTINCT 用 `LogicalUnionAll + buildDistinct`，尾部 UNION ALL 保持不去重。每个 UnionAll 分支先投影到统一 Schema，必要时插入 cast；ORDER BY 的正整数被解释为一基输出列序号。

CTE 流程先统计主查询及后续 CTE 对当前定义的消费者数，避免跨构建累加旧计数；普通单消费者 CTE可内联，多消费者默认保留 `LogicalCTE`。递归 CTE分离 seed 和 recursive member，共享类、存储 ID 与统计信息，并把 seed/recursive 输出投影成一致类型。递归成员自身禁止 ORDER BY/LIMIT，但 lateral 派生表内部的 ORDER BY/LIMIT 被排除在该禁令之外。

FROM/JOIN 流程递归构建左右结果集。普通 join 按类型设置 Schema 可空性，重写 ON 条件，并对 NATURAL/USING 合并公共列；lateral 右侧在左侧 Schema/名称压栈后构建，产生携带相关列的 `LogicalApply`。表源路径解析 CTE、派生查询或真实表；真实表从 InfoSchema 构建列、隐藏 handle/commit-ts 列、伪统计与访问路径，并应用索引、假设索引和存储类型 hint。

## 数据与状态

- **Schema 与名称必须同步**：几乎每个新算子都成对调用 `SetSchema`/`SetOutputNames`，再 `SetChildren`。Union、CTE、外连接、Apply 会新分配列 ID或清除 NOT NULL；公共列合并和辅助列投影必须保持列数、顺序与名称一致。
- **PlanBuilder 可变状态**：本文件读写 `optFlag`、`curClause`、`runtimeCTEs`、`tableHintInfo`、`subQueryHintFlags`、`inStraightJoin`、`outerSchemas`、`outerNames`、`isForUpdateRead`、`visitInfo`、handle 映射和 CTE 信息。临时覆盖通常保存旧值并在子构建结束后恢复。
- **优化标志**：谓词产生 predicate pushdown/key info/simplification；Union/Distinct/Limit/Apply 等分别开启列裁剪、聚合下推、TopN 下推、decorrelate、常量传播、投影消除、semi-join rewrite 等规则。新算子若漏设标志，计划虽可构建但后续行为可能偏离 Go。
- **CTE 共享状态**：`Arc<RwLock<StatsInfo>>` 允许多个 CTE 读者共享 seed 统计；CTE 类本身经引用类型共享。`runtimeCTEs` 是按构建栈隔离的普通 `HashMap`，不是全局注册表。
- **列 ID 与计划 ID**：通过 expression/plan context 分配；源码特别保持 Go 的节点初始化顺序，因为 ID 会出现在裁剪与计划快照中。Union 先初始化 UnionAll 再建立各臂投影；相关聚合也会真实预构建子查询源，而不是仅做语义探测。
- **访问路径状态**：`DataSource` 同时保存全部/当前可能路径、handle、表统计、分区名、存储偏好和索引 hint。非聚簇表补真实 extra row handle，随后补隐藏 commit-ts；缓存表再包装 `LogicalUnionScan`。
- **Hint 警告**：`RuntimeHintWarnings` 收集解析警告，`set_hint_warning_once` 去重后写入 statement context；无效 HYPO_INDEX、冲突存储 hint、未命中 hint 均是警告而非必然失败。

## 依赖与调用关系

上游直接证据：

- `planbuilder_runtime.rs:758` 把 `BuildResultSetNode` 注册为默认 result-set builder。
- `planbuilder_runtime.rs:1018,1035,1078,1202,1655` 在 Explain/Execute/Insert/Update 等 SELECT 形路径直接调用 `BuildBorrowedResultSetNode`；`:1169` 调用 `collect_window_expressions`。
- `lib.rs:153` 声明模块，`:217` 重导出两个公共辅助函数，`:456–460` 装配两份独立测试模块。
- RustCodeGraph 文件级反向边还显示 server、session planning 与 coprocessor test 间接使用该模块；因符号图无法稳定解析动态回调，文档不把空的 symbol-callers 输出解释为“没有调用者”。

主要下游：

- `crate::expression_rewriter::{rewrite,rewriteWithPreprocess}`：把 AST 表达式绑定到当前计划 Schema，可能反向构建子查询并改变计划根。
- `logicalop_dependency`：全部逻辑节点、Schema/children 接口、统计与 CTE 类。
- `aggregation_dependency::NewAggFuncDesc`：聚合和 DISTINCT 的函数描述。
- `hint_dependency`：query-block、join、index、storage、aggregation 和 limit hint。
- `rule_dependency`：记录后续优化规则位；本文件只声明需求，不执行规则。
- `PlanBuilder` 的 InfoSchema/上下文方法：分配列 ID、读取 session SQL mode/prepared 参数、查表、填充 DataSource、记录权限与 statement 状态。
- `crate::recheck_cte::RecheckLogicalCTE`：在完整树建立后重新检查 CTE 消费者谓词。

crate 边界由 `pkg/planner/core/Cargo.toml` 复核：上述依赖均是显式路径依赖；`tipb` 是带固定 revision 的唯一相关 Git 依赖，但本文件不直接编码 protobuf。

## 错误处理与边界

所有主构建函数以 `Result<LogicalPlanRef, expression::Error>` 传播错误；表达式、聚合和元数据错误通常用 `?` 保留上游语义，局部结构错误用 `expression::errors::New`，GROUPING/ONLY_FULL_GROUP_BY 等兼容错误使用 `plannererrors` 的既有错误模板。

已确认的重要边界包括：

- 已消费的 AST 节点、未知结果集类型、空集合输入、集合输入/操作符数量不匹配、集合两侧列数不等均立即失败。
- INTERSECT ALL、EXCEPT ALL 保持 Go/TiDB“不支持”错误；INTERSECT/EXCEPT 用 `NullEQ`，使 NULL 集合语义不退化为普通等值连接。
- CTE 声明列数必须等于实际输出；递归 seed 与 recursive member 必须列数兼容，类型需统一；递归查询块自身的 ORDER BY/LIMIT 被拒绝，但 lateral 子查询是明确例外。
- 命名窗口禁止循环引用、非法继承与不合法 frame；ROWS/RANGE 边界、时间单位、无符号整数及窗口函数参数均在构造节点前校验。
- NATURAL/USING 需要真实公共列并维护歧义/隐藏列语义；RIGHT LATERAL 等不支持组合由 join 构建路径报错。外连接与 Apply 的内侧列会清除 NOT NULL。
- `read_limit_value` 只接受无符号整数、非负有符号整数，以及可解析为 `u64` 的 parser 字符串/十进制文本；其他表达式、负数、浮点报错。prepared marker 由 `read_bound_limit_value` 通过 SQL offset 查参数；offset 加 count 使用饱和运算，count 被截为 `u64::MAX - offset`，总量为零时返回零行 Dual。
- `build_distinct_runtime` 拒绝超过 child Schema 长度的键前缀；合法 DISTINCT 用键前缀分组，并为全部输出列生成 FirstRow。
- 表不存在、公共 handle 元数据缺失、表达式无法解析等是错误；无效 hint 多数通过 statement warning 降级。访问路径筛选仍须至少与后续优化器契约一致，修改时不可只为“能建树”吞掉错误。

本文件中少量 `expect` 依赖已经由前置不变量保证，例如非空 set operands、Union 目标类型存在、递归结构形状和缓存表 handle；扩展 AST 形状时必须重新验证这些不变量，不能把它们当作外部输入永远安全。

## 并发与资源生命周期

逻辑计划构建本身是同步、单 `&mut PlanBuilder` 流程，没有启动线程、异步任务或 channel。大多数计划节点由 `Box<dyn LogicalPlan>` 独占，表达式/名称/上下文使用 `Arc` 共享不可变或内部管理的数据。

唯一显式同步锁是 CTE seed 统计的 `Arc<RwLock<StatsInfo>>`；克隆 `CteBinding` 不复制统计快照，而是共享同一锁保护对象。文档未发现本文件持锁后调用外部重写器的路径，因而没有跨长调用持锁的证据。

需要按栈配对的生命周期有：`runtimeCTEs` 替换/恢复、`tableHintInfo.push/pop`、`outerSchemas` 与 `outerNames` 的 push/pop、当前 clause 与 straight-join 标志保存/恢复。`BuildBorrowedResultSetNode` 在构建结果返回前恢复旧 CTE 环境；`build_select_runtime` 在返回前恢复 hint/straight-join 状态。新增可失败分支时应沿用“先保存、集中恢复、最后返回结果”的结构，避免 `?` 提前返回留下污染状态。

`NodeRef::with_node` 反映 AST 有可消费生命周期；返回 `None` 必须视为错误。`resetCTECheckForSubQuery` 只复位传入 CTE 条目的 `enterSubquery`，不会全局扫描或删除 CTE。权限信息 `appendDynamicVisitInfo` 消费原向量、顺序追加一个元素后返回，保持 Go slice append 的顺序语义。

## 与 Go 版本的对应关系

主要对照文件是 `pkg/planner/core/logical_plan_builder.go`（8123 行），Cargo 的 `[package.metadata.porting] go-package = "pkg/planner/core"` 也明确了移植归属。Rust 不是逐行同名：它把 Go 文件的大型 `PlanBuilder` 方法群拆成自由函数和尾部 `impl PlanBuilder`，但关键阶段可对应如下：

- Rust `BuildResultSetNode`/`build_query_node_with_ctes` ↔ Go `(*PlanBuilder).buildResultSetNode`（Go 第 434 行）。
- Rust `build_select_runtime(_inner)` ↔ Go `buildSelect`（第 4290 行）；FROM/WHERE/聚合/窗口/投影/排序/LIMIT 的相对顺序和临时状态恢复均以 Go 为契约。
- Rust `build_select_aggregation`、`build_select_projection`、窗口函数组 ↔ Go `buildAggregation`（第 255 行）、`buildProjection`（第 1805 行）、`buildWindowFunctions` 及 frame 函数组（第 6792 行以后）。
- Rust 集合运算组 ↔ Go `buildSetOpr`、`buildUnion`、`buildUnionAll`、`buildSemiJoinForSetOperator`（第 2146–2411 行）。
- Rust `build_join_runtime`/`build_table_source_runtime` ↔ Go `buildJoin`（第 739 行）和 `buildDataSource`（第 4960 行）。Rust 还显式承载 lateral Apply、隐藏 handle/commit-ts、hint 访问路径等已经接线的运行时逻辑。
- Rust LIMIT/DISTINCT ↔ Go `buildLimit`（第 2607 行）和 `buildDistinct`（第 2004 行）。Rust 单测明确固定 offset/count 饱和、缺省 count 为零、FirstRow 聚合等 Go 可观察行为。
- Rust Apply/SemiJoin/MaxOneRow ↔ Go 第 5750–5840 行的同名方法；Rust 保持 decorrelate/outer-join/semi-rewrite 标志及标量半连接输出列形状。
- Rust `build_with_runtime`/CTE helpers ↔ Go `buildWith`、`buildProjection4CTEUnion`、`resetCTECheckForSubQuery`（第 8039 行以后）。
- Rust `appendDynamicVisitInfo` ↔ Go 第 7730 行同名函数，均按原顺序追加动态权限检查。

相关 Go 测试证据分散在 planner 测试面：`pkg/planner/core/lateral_join_test.go` 覆盖 LATERAL 生成 Apply、左侧别名可见性、外连接可空性、递归 CTE 中 lateral ORDER/LIMIT 例外及非 lateral 失败；`pkg/planner/core/logical_plans_test.go` 覆盖 ONLY_FULL_GROUP_BY、窗口和逻辑计划输出；`pkg/planner/core/preprocess_test.go` 覆盖 CTE 预处理和消费者计数幂等。Rust 侧的直接回归见下一节。文档只陈述已在当前 Rust 源码中接线的行为，不把 Go 文件其余 8123 行功能自动视为 Rust 已覆盖。

## 扩展指南

- **新增结果集 AST 类型**：在 `build_query_node_with_ctes` 增加分派，并同时定义 CTE 环境、handle map、Schema/名称和 RecheckCTE 语义；在独立 `*_test.rs` 中通过真实 parser/PlanBuilder 覆盖，禁止把测试嵌入生产文件。
- **改变 SELECT 阶段**：优先在 `build_select_runtime_inner` 对应阶段接线。必须核对 Go `buildSelect` 的算子顺序、临时 builder 状态恢复、列/计划 ID 分配顺序与 `optFlag`，并更新同目录独立测试。
- **扩展窗口/聚合**：修改第 2130–3949 行的参数校验、mapper、前后投影和输出类型；同时覆盖别名、HAVING/ORDER BY 辅助列、命名窗口继承、frame 边界与 DISTINCT 聚合，不应只让表达式重写通过。
- **扩展 Join/LATERAL/USING**：修改 `build_join_runtime`、`build_lateral_join_runtime`、`coalesce_common_columns`；同步检查 FullSchema/FullNames、外连接 NOT NULL、相关列提取、join hint 与 reorder/decorrelate 标志。直接测试面是 `logical_plan_builder_lateral_with_aster_unit_test.rs`，更广 Go 语义面是 `lateral_join_test.go`。
- **扩展 CTE**：修改 `CteBinding`、`build_with_runtime` 及引用计数/递归限制函数时，必须保证每次构建重算消费者、内外层环境隔离、seed/recur 类型与共享统计一致；同步独立 CTE/LATERAL 测试。
- **扩展表源或 hint**：在 `build_table_source_runtime` 和两个 index 安装函数接线；保持隐藏 handle/commit-ts、分区标志、pseudo stats、访问路径集合与 statement warning。新增真实错误不能降成 warning，反之亦然。
- **扩展 LIMIT/prepared 参数**：同时修改 `read_bound_limit_value`、`read_limit_value`、`build_limit_runtime` 和 `logical_plan_builder_runtime_aster_unit_test.rs`；明确负值、文本、溢出、缺省 count、参数索引和 plan cache 行为。
- **新增公共 API**：先判断是否只需 `pub(crate)`；若 crate 外使用，再在 `lib.rs` 重导出。保持 Go 风格名称是当前移植兼容选择，不要为局部风格统一任意改名。

主要风险：Schema 与 OutputNames 不同步会造成后续解析错列；列/计划 ID 顺序变化会破坏计划快照；遗漏优化标志会产生行为或性能回归；CTE/hint/outer schema 栈未恢复会污染后续查询；访问路径或类型合并简化会偏离 Go 兼容语义。任何行为修改均应先构造失败回归、再修复，并遵循仓库要求运行 `cargo fmt --all`，但本任务是纯文档分析且按计划不运行 Cargo。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11467 文件、307296 节点、1848419 边；`files --filter pkg/planner/core/logical_plan_builder_runtime.rs` 确认目标已索引；`node --file ...` 读取 1–6264 行并取得文件级使用者；`node --symbols-only` 与按区间节点读取核对主要类型、函数和实现位置。
- RustCodeGraph 精确调用查询：对 `BuildResultSetNode`、`BuildBorrowedResultSetNode`、`build_select_runtime`、`build_with_runtime`、`build_join_runtime`、`build_limit_runtime`、`build_distinct_runtime`、`appendDynamicVisitInfo` 运行 callers/callees。索引对动态回调只识别到有限边，因此再以直接引用补证，未把空结果外推为无调用关系。
- 源/装配/Cargo：`pkg/planner/core/logical_plan_builder_runtime.rs`、`pkg/planner/core/planbuilder_runtime.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/Cargo.toml`。
- Rust 独立测试：`pkg/planner/core/logical_plan_builder_runtime_test.rs`（INTERSECT/EXCEPT 与 ALL 错误）；`logical_plan_builder_runtime_aster_unit_test.rs`（LIMIT、字面量、DISTINCT）；`logical_plan_builder_lateral_with_aster_unit_test.rs`（LATERAL、递归 CTE、消费者计数）；`logical_plan_builder_test.rs` 是旧的轻量窗口结构测试，针对 `logical_plan_builder.rs`，不能替代本运行时文件的回归。
- Go 对照：`pkg/planner/core/logical_plan_builder.go`；相关测试证据来自 `pkg/planner/core/lateral_join_test.go`、`logical_plans_test.go`、`preprocess_test.go`。最近目录没有 `doc.go`，故不存在额外包级契约可读。
- 人工复核结论：本文件存在是为了提供已接入 `PlanBuilder` 的真实 AST→逻辑计划实现；运行方式是入口分派后按 SQL 子句逐层建树并维护 Schema/名称/状态；安全扩展必须在对应阶段保持 Go 顺序、状态栈、优化标志与独立回归测试。
- 按任务约束，本次只生成文档，不运行 Cargo；交付前执行任务指定的固定 11 章节结构校验，并检查唯一生产物及未修改 `plan.md`。
