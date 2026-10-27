# `pkg/session/runtime/planning.rs`

## 文件定位

真正的建树、规则优化、物理计划与缓存计划捕获分别在 `pkg/planner/core`、`pkg/planner/core/operator/*` 中实现；本文件负责把这些能力接到具体会话生命周期。

## 核心职责

1. 通过 `SessionPlanContext` 实现 planner 所需的 `PlanContext`，把参数、列 ID、计划 ID、表达式构建上下文、下推能力探测和统计同步加载连接到当前会话（`SessionPlanContext`、`plan_context_with_params_and_explain`）。
2. 把 `Domain` 的统计缓存适配为同步加载器所需的 `StatsStorage`/`StatsHandle`，并按 Domain 共享有界任务队列及 worker（`DomainStatsSyncLoadHandle`、`DomainStatsLoadWorkers`、`SessionStatsSyncLoadAdapter`）。
3. 为逻辑 `DataSource` 注入真实或伪统计、列 NDV、直方图集合和可访问路径，同时遵守强制索引、隔离读引擎及过期统计策略（`populate_session_data_source`、两个 `DataSourceProvider::Populate` 实现）。
4. 支撑普通 SELECT、prepared SELECT 和测试观察入口的解析、建树、优化、执行及计划快照（`ExecutePlannedKVSelect`、`OptimizeParsedSelect`、`OptimizeRootPlanIDForTest`、`IndexMergePathDigestForTest`）。
5. 管理 prepared statement 的参数标记、参数边界校验、实例计划缓存 key、缓存恢复/准入、range 配额、运行信息回写及 `last_plan_from_cache` 状态（`PreparePlannedKVSelect` 到 `FinishPreparedKVPhysicalPlan`）。
6. 在直接 SELECT 路径建立与 Go 会话一致的 statement context 边界，包括 hint、时区、SQL mode、TaskID、CTE、killer 与统计同步等待设置（`StatementContextRuntime::reset_statement_context`）。

## 主要符号

- `SessionPlanColumnIDAllocator`：将表达式层 `PlanColumnIDAllocator` 转发给 `SessionVars::AllocPlanColumnID`，并以 `SeqCst` 读取最后分配的列 ID。
- `SessionPlanContext`：文件内部的 planner 上下文。`AtomicI32 plan_id` 支持分配、checkpoint、恢复和重置；其余字段保存会话变量、表达式/ranger/PB 构建上下文、内建函数计数器、可选统计等待器及 prepared 参数在原 SQL 中的 offset。
- `SessionPushdownCapabilityClient`：只实现请求类型能力探测。`IsRequestTypeSupported` 转发给 `RequestTypeSupportedChecker`；`Send` 必然 panic，形成“规划阶段不得发请求”的硬边界。
- `DomainStatsSyncLoadHandle`：在 session 与统计同步加载器之间转换列/索引直方图、TopN、加载状态和版本，并把加载结果写回 Domain 统计 cache。锁中毒统一映射为 syncload 的 `Error::Poisoned`。
- `DomainStatsLoadWorkers`：持有共享 loader、退出原子位和线程句柄；`Drop` 先以 `Release` 发布退出，再 join 所有 worker。
- `SessionStatsSyncLoadAdapter`：实现 `StatsLoadWaiter`。`new` 用于直接 handle 并由等待方消费任务；`new_with_domain` 按 Domain 指针共享队列和后台 worker。
- `SessionKVDataSourceProvider`：用调用方给出的单一行数/统计版本填充数据源，主要服务 prepared 与测试型 KV 规划。
- `SessionDomainDataSourceProvider`：从真实 Domain 统计缓存按物理表取统计，服务普通多表 SELECT、EXPLAIN 等直接规划路径。
- `should_use_pseudo_for_outdated_stats`：仅当系统变量开启、已有 analyze 基数且 `modify_count/analyze_count` 超过 `RatioOfPseudoEstimate()` 时判定过期；空表统计由 `Populate` 另行按 pseudo 处理。
- `estimated_table_records` / `estimated_table_stats`：先读 stats handle cache，再回退 `stats_context().physical_stats`，最终回退 `PseudoRowCount` 和版本 0；不为估算扫描用户表。
- `PlannedKVResult`：公开执行结果，包含行、物理算子名、扫描行数、代价和 TopN 局部有序索引开关快照。
- `parameter_markers` / `bind_parameter_markers` / `validate_prepared_limit_arguments`：识别不在引号或注释中的 `?`，校验参数数量，并在文本替换前为 LIMIT/OFFSET 保留 TiDB 风格错误。
- `named_prepared_partial_index_cacheable`、`negative_unsigned_equality_parameter`、`parameter_shape_class`：根据 AST、表元数据、部分索引条件及参数值识别不能安全复用同一计划的情况，供 `runtime/dispatch.rs` 的 named prepared 路径调用。
- `ExecutePlannedKVSelect`：一次性 SELECT 的完整 parse/build/optimize/execute 入口。
- `PreparePlannedKVSelect`：创建并登记 `PreparedPlannedKVSelect`，保存 AST、参数 marker、schema/version 和语句文本。
- `PlanPreparedPlannedKVSelect`：只绑定参数并生成/恢复物理计划，不读取数据；返回 `PreparedKVPhysicalPlan`。
- `ExecutePreparedPlannedKVSelect` / `ExecutePreparedPlannedKVSelectThroughAdapter`：分别经快照 retriever 或 typed adapter 执行 prepared 计划。
- `FinishPreparedKVPhysicalPlan`：命中缓存时更新运行行数；首次计划执行完成后才把 `PendingCache` 放入实例缓存。
- `OptimizeParsedSelect`：对已解析 SELECT 使用当前 Domain 的统计与 InfoSchema 直接优化；调用方必须先重置 statement context。
- `StatementContextRuntime for ConcreteSession`：为 executor 的直接 SELECT 接口实现语句上下文重置。

文件没有模块级业务常量、宏或 `#[cfg]` 条件项；公开面主要是 `PlannedKVResult` 和 `ConcreteSession` 上的方法，其余多为 `pub(crate)`、`pub(super)` 或私有实现。

## 执行流程

### 一次性 KV SELECT

`ExecutePlannedKVSelect` 解析并要求恰好一个 SELECT，定位表及当前 InfoSchema，读取估算行数/统计版本，创建 `SessionPlanContext` 与 `SessionKVDataSourceProvider`，再经 `NewPlanBuilder().Init`、`buildResultSetNode` 和 `DoOptimize` 得到物理计划。随后它递归收集算子，构造 KV 数据源并执行，最终返回行、代价、扫描行数和规划期开关状态。非法语句、缺表、建树、优化或执行错误都附加阶段上下文后返回。

### 普通已解析 SELECT

executor 先通过 `StatementContextRuntime::reset_statement_context` 完成 statement boundary；`OptimizeParsedSelect` 再验证 AST 类型、将统计等待上限限制在 session 值与 `MAX_EXECUTION_TIME` hint 的较小值、重置 `PlanColumnID`，并构造带 Domain 统计等待器的上下文。`SessionDomainDataSourceProvider` 注入各表统计。非 restricted SQL 额外开启 predicate-column 收集与同步统计加载规则，随后执行 `DoOptimize`。

### Prepared 计划生命周期

1. `PreparePlannedKVSelect` 解析单条 SELECT，提取/sort 参数 marker，完成预处理所需的 schema 绑定，并把 `PreparedPlannedKVSelect` 存入会话 map，返回 statement ID。
2. `PlanPreparedPlannedKVSelect` 校验 statement ID 与参数数量，刷新语句级状态和 warnings，按 MDL 决定使用 prepare 时或最新 InfoSchema，识别目标表与 table-cache 标记。
3. 它从当前库、schema version、只读属性、分区裁剪模式、隔离读引擎、`SQL_SELECT_LIMIT`、事务状态、连接字符集/排序规则等建立 `PlanCacheKeyContext`；参数 datum 同时推导为 field types，参与实例缓存匹配。
4. 命中时，`CachedPlan::restore` 用新的 `SessionPlanContext` 和参数重建可变 range；未命中时重新 build/optimize。
5. 物理计划递归生成 `ProcessPlanSnapshot`，并检查每个索引 range 是否超过 `RangeMaxSize`、算子或部分索引条件是否不可缓存。range 回退会写入 `StmtCtx` tracker/warning。
6. 可缓存的首次计划由 `CachedPlan::try_capture` 生成 `PendingCache`，但此时不立即写缓存；执行成功/结果关闭后 `FinishPreparedKVPhysicalPlan` 才准入。缓存命中则在 finish 时写回 runtime rows。
7. `ExecutePreparedPlannedKVSelect` 是“plan -> 构建快照数据源 -> 执行 -> finish”的同步封装；typed adapter 路径把相同 finalize 动作放在结果关闭处（直接调用见 `runtime/scan_adapter_runtime.rs`）。

### 统计与数据源注入

`SyncWaitStatsLoad` 从 `StmtCtx.StatsLoad.NeededItems` 下转为 syncload item，严格沿用 statement timeout（零表示立即超时），发送批量请求后：直接 handle 模式在当前线程消费恰好所需任务；Domain 模式由共享 worker 消费；最后等待所有结果。`new_with_domain` 用 `OnceLock<Mutex<HashMap<domain_ptr, Weak<...>>>>` 复用每个 Domain 的 loader，并清除已失效 weak entry。

`SessionDomainDataSourceProvider::Populate` 将缓存统计转成 planner `HistColl`，按 analyzed rows 对 NDV 缩放；空的已分析统计或被判定过期的统计使用 pseudo row count/version 0。`populate_session_data_source` 再生成 table/index/TiFlash access path，并通过 `FilterPathByIsolationRead` 应用会话隔离读限制；显式 `USE/FORCE INDEX` 没有匹配路径时返回错误。

## 数据与状态

- `SessionPlanContext.plan_id` 与 `SessionVars.PlanColumnID` 分属计划节点和表达式列 ID。前者每个上下文从 0 开始，支持优化分支 checkpoint；后者在普通直接规划前显式归零。两者均使用顺序一致原子操作，避免 ID 观察到乱序状态。
- prepared statement 定义保存在 `ConcreteSession` 的 prepared map；实例计划缓存由 `runtime.rs` 中 `RUNTIME_INSTANCE_PLAN_CACHES` 按 Domain 共享，因此不同会话可以命中同一不可变捕获计划，但恢复出的上下文、参数 range 和运行态彼此独立。
- `SessionState.last_plan_from_cache` 和 `process_plan_snapshot` 是最近一次 prepared 规划的会话可观察状态；`LastPlanFromCache` 与 `ProcessPlanSnapshot` 只读返回它们。
- `StmtCtx` 承载 warnings、统计加载 item/timeout、plan-cache tracker、TaskID、时区、SQL mode 派生 flags、CTE map、动态裁剪开关和 `NotFillCache`。每条直接 SELECT 都重新初始化这些语句级字段，同时保留时区。
- 统计缓存转换保留 NDV、null count、总列大小、correlation bit pattern、版本、bucket 和 TopN。CMSketch 当前明确返回/存储为 `None`；列 `primary_key`/`is_handle` 在此适配器中为 false，这是当前实现事实，不应推断为完整统计语义。
- `DomainStatsSyncLoadHandle` 优先读取 `Domain::persisted_table_stats`，否则读取 handle cache；写回时创建缺失 table entry，并逐列/索引更新 cache。
- `planned.PendingCache.take()` 保证首次计划最多准入一次；命中缓存时持有 `CachedValue`，finish 用扫描行数更新 runtime info。

## 依赖与调用关系

上游装配和调用：

- `pkg/session/runtime.rs` 声明模块、再导出规划适配器和结果类型，并提供 `ConcreteSession`、共享实例缓存及 `ProcessPlanSnapshot` 等相邻定义。
- `pkg/session/runtime/dispatch.rs` 调用参数校验、部分索引/unsigned 参数判断和 prepared adapter 执行入口，承接文本 SQL 的 PREPARE/EXECUTE 分派。
- `pkg/session/runtime/scan_adapter_runtime.rs` 在 lazy typed execution 关闭时调用 `FinishPreparedKVPhysicalPlan`，确保缓存准入和 runtime info 发生在执行终点。
- `pkg/session/runtime/relational_scan.rs`、`explain_query.rs`、`control.rs` 复用 plan context、数据源 provider 或统计等待器；`typed_adapter_bridge.rs` 暴露绑定 prepared 计划及缓存状态。
- `astersql_executor::select` 通过 `StatementContextRuntime` 调用 `reset_statement_context` 与 `OptimizeParsedSelect`，形成直接 SELECT 的正式上游。

下游依赖：

- parser/AST：解析 SELECT、识别参数和遍历谓词表达式。
- planner core/base/rule/operator：`PlanContext`、`PlanBuilder`、`DoOptimize`、数据源填充、物理树递归、计划捕获与恢复。
- domain/infoschema/statistics：schema version、表元数据、实时统计、直方图与同步加载队列。
- expression/types：参数 datum、参数类型推断、表达式构建及字段类型转换。
- kv/executor/tablecodec：能力探测、快照读取、物理计划到实际行扫描。

RustCodeGraph 的文件节点列出 `runtime/scan_adapter_runtime_test.rs`、`runtime/system_session.rs`、`session/test/variable/variable_test.rs`、`sessionctx/stmtctx/stmtctx.rs` 和 `sessionctx/variable/session.rs` 等索引关联；对具体运行时调用关系，模块内搜索进一步确认了上述 `dispatch`、adapter、relational scan、explain 和 control 接线。

## 错误处理与边界

- 所有面向会话的失败使用 `SessionResult`/`SessionError`，下游 planner/expression/syncload 错误通过 `session_error("阶段", error)` 或字符串转换保留阶段信息。
- 入口严格限制 SELECT：一次性、prepared、直接 AST 和 statement reset 各自拒绝多语句或非 SELECT；缺失 InfoSchema 表也不会退化成无元数据计划。
- 参数 marker 数量必须与 arguments 相等；prepared statement ID 不存在、缓存 key 不可建立、缓存计划无法恢复、LIMIT/OFFSET 参数不是 `u64` 均直接报错。
- `parameter_markers` 跳过单/双引号、反引号、转义、`#`/`--` 行注释和 `/* */` 块注释，防止把文本中的问号当参数；它是轻量扫描器，扩展 SQL lexical 规则时必须同步验证。
- 强制索引过滤后无访问路径会报 `table ... has no requested access path`；隔离读过滤错误继续向上传播。
- 同步统计加载区分 poisoned item lock、错误类型的缓存值、超时、读取/写回失败；测试 failpoint 可强制 timeout。零 timeout 有意不回退 session 默认值。
- `SessionPushdownCapabilityClient::Send` panic 是不变量保护：本对象只能用于能力查询。若未来 planner 在该上下文中发送请求，必须改用真实 storage client，而不是吞掉调用。
- 共享 `Rc<SessionInner>` 或 `Arc<SessionVars>` 无法取得唯一可变引用时，直接规划/reset 拒绝继续，避免在前一语句仍被持有时修改 statement state。
- bucket count 的整数转换采用饱和/默认策略；直方图 JSON bound 转换失败的 bucket 被跳过。该容错可能降低估算精度，但不会伪造可执行数据。
- cache capture 失败会设置 skip reason 而不是使查询失败；缓存写入返回值当前被忽略，因此计划缓存是性能优化，不改变查询正确性。

## 并发与资源生命周期

- 每个 Domain 的统计 loader/worker 通过进程级 `OnceLock` map 共享，map 只保存 `Weak<DomainStatsLoadWorkers>`，不会仅因注册表而延长 Domain 生命周期；会话 adapter 的 `_workers: Arc<_>` 在使用期间保活 worker。
- `DomainStatsLoadWorkers::drop` 发布退出位并 join 全部线程，保证队列 owner 最后释放时后台线程退出。worker 数来自全局配置；0 使用 CPU 推导值，队列容量要求非负。
- 统计 handle 由 `Arc<Mutex<_>>` 保护；读取 persisted stats 使用 `Weak<Domain>`，避免 loader 反向强持有 Domain。静态共享队列 map 也由 `Mutex` 串行化创建与清理。
- planner context 可由 `Arc<dyn PlanContext>` 下传，但其会话变量大多按语句生命周期读取；计划 ID/列 ID 使用原子量。`ConcreteSession` 的可变状态仍按单会话、单线程 `Rc`/`RefCell` 模型工作，不能把这些方法解释为允许同一 session 并发规划。
- prepared 缓存采用 capture/restore：缓存存放可恢复的计划形状，参数绑定后的 range 和 context 每次重建；首次执行完成前只持有 `PendingCache`，避免失败或未消费结果污染共享缓存。
- snapshot/retriever 与 typed adapter 的资源由调用方关闭；lazy adapter 在 close/finalize 时回调 finish。因此新增早退路径必须仍保证 finish 只在成功执行语义下发生。

## 与 Go 版本的对应关系

- `pkg/planner/optimize.go` 的 `Optimize`/`buildLogicalPlan` 与 `pkg/planner/core/optimizer.go` 的 `DoOptimize`、`adjustOptimizationFlags` 对应 Rust 的 builder + `DoOptimize` 流程。Rust `OptimizeParsedSelect` 显式补入 `FLAG_COLLECT_PREDICATE_COLUMNS_POINT` 和 `FLAG_SYNC_WAIT_STATS_LOAD_POINT`，注释说明这是对齐 Go 正常 SELECT 的 flags 调整。
- Go prepared plan cache 会把 schema、session variables、参数类型、range 配额和运行反馈纳入可复用性判断；Rust 的 `PlanCacheKeyContext`、`CachedPlan::restore`、`physical_plan_cache_ranges_fit_quota` 与延迟 `FinishPreparedKVPhysicalPlan` 复刻这组行为，而不是仅按 SQL 文本缓存最终 plan 对象。
- Go 使用 Domain statistics handle、pseudo estimate 与 isolation read 选择访问路径；Rust 的 `SessionDomainDataSourceProvider` 和 `populate_session_data_source` 提供相应接线。当前 Rust 适配对 CMSketch、主键 handle 等统计元数据仍是受限映射，文档不将其描述为 Go 全量统计结构的完全等价实现。
- Go 正常 session 的 statement reset 涉及更广泛事务/执行状态；本文件只实现 executor direct-SELECT 所需子集。该边界由 `runtime.rs` 顶部说明确认：`ConcreteSession` 是窄运行时，不冒充完整 `sessionapi::Session` ABI。

## 扩展指南

- 新增 planner 会话能力时，优先扩展 `SessionPlanContext` 的 trait 实现及 `plan_context_with_params_and_explain` 的统一构造，避免在一次性、prepared、EXPLAIN 和 direct SELECT 路径分别拼装不一致上下文。
- 新增统计字段时同时检查 `histogram_from_column/index`、`StatsHandle::Get`、`UpdateStatsCache` 与 `planner_histogram_collection` 的双向转换；特别关注 loaded status、版本、TopN、bucket count、主键/handle 和 CMSketch 语义。
- 修改访问路径选择时在 `populate_session_data_source` 保持 `USE/FORCE INDEX`、TiFlash replica 与 `FilterPathByIsolationRead` 的顺序；变更 pseudo 策略时同步核对空统计与 outdated 两条分支。
- 扩展 prepared 可缓存范围时，至少同步修改 `physical_plan_noncacheable_reason`、range 配额递归、`CachedPlan::try_capture/restore` 支持和 cache key 上下文；不要在 `PlanPreparedPlannedKVSelect` 中提前写缓存。
- 扩展参数表达式识别时同步维护 `collect_parameterized_columns`、`expression_contains_parameter` 和 `collect_expression_columns`，覆盖新的 AST kind；若修改 marker lexer，增加引号、注释、转义和 LIMIT/OFFSET 的独立用例。
- 修改直接 SELECT statement boundary 时，以 `reset_statement_context` 为唯一集中入口，并验证时区保留、hint restore/start、TaskID、CTE、killer、SQL mode flags 与动态分区裁剪不会跨语句泄漏。
- Rust 单元测试必须继续放在独立文件。首选扩展 `pkg/session/runtime_test/planning.rs`（普通 planning/statement boundary）、`pkg/session/plan_cache_runtime_test.rs`（prepared cache/range/snapshot）、`pkg/session/runtime/scan_adapter_runtime_test.rs`（typed adapter 与执行终点）、`pkg/session/runtime_test/session.rs`（统计同步加载）、`pkg/session/cached_table_runtime_test.rs`（table-cache 标志）或 `pkg/session/runtime_pessimistic_test.rs`（事务导致的缓存失效），不要在 `planning.rs` 内嵌测试。
- 兼容性风险主要是错误文本、cache key 漏项、schema/MDL 版本选择和 Go flags 不一致；性能风险主要是错误回退 pseudo、重复启动统计 worker、提前扫描表做估算、range 爆炸或不可缓存计划误准入。


## 2026-10-24 预处理语句身份承接

negative_unsigned_equality_parameter 对预处理 SQL 的再解析承接所属连接方言，避免负整数参数检查退回默认 MySQL；其余规划能力不扩展。

PreparePlannedKVSelect 作为普通 SQL PREPARE 表查询的实际计划准备入口，同样按连接方言解析模板；不改变规划上下文或缓存键架构。


## 2026-10-24 规划求值身份与缓存隔离

统一规划工厂 plan_context_with_params / plan_context_with_params_and_explain 显式接收所属 SqlDialect，保存到 expression 的静态 EvalContext；派生 Ranger 与 BuildPB 使用同一身份。Prepared lookup、restore 与延迟 cache admission 通过 dialect_plan_cache_key 隔离实际共享键，缓存准入、范围重建与参数类型决策不变。相邻普通扫描、EXPLAIN、typed adapter、MLog 调用仅补传连接身份。

## 整数参数元数据接线

`ConcreteSession::protocol_integer_parameter_types` 在 PostgreSQL 方言中解析共享 AST，仅收集 SELECT 投影、WHERE/HAVING、GROUP/ORDER 表达式内直接 int4/int8 Cast 的 ParamMarker（允许括号），按原 offset 对应 engine marker；返回每个出现位置的 OID 23/20，未约束位置为0。嵌套 cast 由最内侧直接参数约束源类型，PG 消费者按原编号折叠首个直接约束。它不执行SQL、不注册statement，不实现跨DML/子查询/模式的一般推导。

## 2026-10-25 PG scalar 01

PG参数约束保留供给OID16，并从带bool目标身份的CAST推导16，复用原有参数源位置映射。

### PostgreSQL限定布尔显式转换（任务3）

protocol_parameter_types对直接TEXT目标的参数约束推导OID25，供$1::text::boolean在未提供OID时保留源text身份；只补现有直接CAST约束，不扩展通用推导矩阵。

## PostgreSQL signed16（任务5）

protocol_parameter_types保留显式OID21并由PostgreSQLInt2推断未指定参数为21；复用现有参数发生位置及约束合并逻辑。

## PostgreSQL 有限 numeric 承载

从 PostgreSQLNumeric CAST 推导参数 OID1700。


## PostgreSQL float4 文本输入（任务16）

PG OID700和PostgreSQLFloat4约束进入已有参数类型推断，不影响MySQL参数推断。

## PostgreSQL float8 文本参数（任务17）

PG显式OID701与700一同进入现有参数类型约束，避免准备阶段退为text。

## PostgreSQL 任务18浮点声明参数

按PostgreSQLFloat8 AST身份推导OID701，与既有PostgreSQLFloat4/OID700对称，使float(25/53)、float、double precision及float8的无显式OID参数描述稳定；不按参数值或格式推断。

### 2026-10-26 浮点参数显式转换

显式协议源OID优先于目标CAST推导，避免float8参数转整数前被重新定型而套用numeric舍入；未知参数继续由CAST约束。

## 2026-10-26 文本族身份边界（任务21）

参数约束保留 OID25/1042/1043，显式文本 CAST 从 FieldType 推断正确 OID。

## UUID显式转换局部接线

明确UUID cast约束可推导未知参数为2950；已经声明的text/bytea参数身份不被cast覆盖。

## JSON显式转换与类型交接

仅在明确json cast上下文将OID0推导为114，已声明text/bytea/json不因外观改型。


## 2026-10-27 JSONB显式转换接线

明确jsonb转换约束推导OID3802；已声明参数OID仍保留，禁止以结果转换覆盖输入身份。


## int4 数组共享求值与协议

`protocol_parameter_types` 保留声明1007，并在明确的 int4[] cast 上下文为 OID0 参数推导1007；不从 Bind 内容或首行推导数组族。

## 2026-10-27 数组显式转换（任务23）

仅为显式int8[]目标推断数组OID1016；未新增1016供给参数白名单。int4数组输入OID1007仍可显式转换到1016结果。
