# [`pkg/executor/statement_ru_result.rs`](statement_ru_result.rs)

## 文件定位

`classify_statement_ru_plan` 解析真正被执行的计划，区分写入、提交、分析、点查和普通计划，并生成指标使用的 SQL 类型。

## 核心职责

1. `classify_statement_ru_plan` 解析真正被执行的计划，区分写入、提交、分析、点查和普通计划，并生成指标使用的 SQL 类型。
2. `new_statement_ru_calculation_setup`、`install_statement_ru_owner_at_boundary` 与 `install_statement_ru_owner` 在执行前判断语句是否有资格计量，并把不可变安装快照交给 `StatementRUOwner`。
3. `statement_ru_frontend_compile_bytes` 计算前端编译输入规模：计划缓存命中时为零，否则优先使用去除特定 `EXPLAIN ANALYZE` 前缀后的规范化 SQL。
4. `classify_scan_evidence` 把 Reader 的三个整数计数转成 `Invalid`、`Unavailable` 或估算后的 `Valid(f64)`，供计划遍历层统一处理证据质量。
5. `StatementRUCalculator::finalize` 读取当前配置权重，计算语句总 RU 和分引擎 RU，应用 TiFlash 倍率，校验数值，并冻结可选 full-report。
6. `publish_statement_ru_finalized_snapshot` 和 `StatementRUContextSink` 将资源组消费、汇总指标、full-report 指标及校准发布彼此隔离，使某个外部消费者 panic 不穿透语句完成路径。

## 主要符号

- `StatementRUPlanKind::{Other, Write, Commit, Analyze, PointLookup}`：RU 计算关心的顶层计划分类。`Other` 默认按 `select` 标签发布；写计划再细分 `insert`、`replace`、`update`、`delete`。
- `StatementRUPlanInfo<'a>`：借用已解包的真实计划，同时携带 `kind` 和静态 `sql_type`。借用关系确保分类不会复制或长期持有计划。
- `classify_statement_ru_plan(&dyn Plan) -> StatementRUPlanInfo`：反复解开 `RuntimeExecute`；只在 `RuntimeExplain.Analyze` 为真时解开目标计划，普通 `EXPLAIN` 保留自身身份，因为它只渲染而不执行目标。
- `StatementRUCalculationSetup`：一次合格语句的安装配置，包含 `frontend_compile_bytes` 和是否生成 `full_report`。
- `StatementRUInstallState`：从 session/statement context 提取的值对象，记录只读/SELECT、restricted SQL、TTL 身份、游标及 flat-plan 缓存等资格事实。
- `StatementRUCalculator`：终结阶段的值累加器；`units` 是全语句 `StmtUnits`，`compute[3]` 对应三个执行引擎的计算单位，`report` 仅在 full 模式存在。
- `StatementRUFinalizedSnapshot`：发布所需的冻结载荷，包含单位、总结果、分引擎结果、可选明细、校准状态和 SQL 类型。
- `ScanEvidence` 与 `classify_scan_evidence`：Reader 扫描证据的三态分类及按 `processed_bytes / processed_keys * total_keys` 的估算。
- `StatementRUPublicationSink`：发布端口，拆分为 `consumption`、`results`、`unit`、`statement` 和 `calibration` 五类副作用。
- `StatementRUContextSink`：生产实现，通过 `AdapterRuntime` 上报资源组和校准信息，通过 `astersql-metrics` 更新汇总、TTL、单位及语句状态指标。

## 执行流程

执行前，`install_statement_ru_owner` 从 `AdapterRuntime::StatementRUInstallState` 取得会话边界快照，并读取全局 `ruv2.report_mode`。它先用零编译字节做一次资格探测，只有合格时才调用 `StatementRUFrontendCompileBytes`，避免为必然跳过的语句做额外工作。随后 `install_statement_ru_owner_at_boundary` 生成 `StatementRUOwner` 并存入 `StatementCtx.statement_ru_owner`；full 模式下若是非 restricted 的不合格用户语句，则调用 `StatementRUIneligible` 记录一次跳过。

资格判定先由 `classify_statement_ru_plan` 解包并分类。只读语句、SELECT 上下文，以及 `Analyze`、`Write`、`Commit` 可进入；statement context 缺失、不合格计划、非 TTL 的 restricted SQL、已有游标或已缓存 flat plan 都被排除。TTL 例外必须同时满足 restricted、`request_source_type == astersql_kv::InternalTxnTTL` 和非空 job id。

执行中，`statement_ru_plan_walk.rs` 使用这里的分类和扫描证据规则，把计划、运行统计与写入快照投影为 `StatementRUCalculator` 中的标量单位。RustCodeGraph 显示 `classify_scan_evidence` 的生产调用者包括 `collect_statement_ru_reader_scan_bytes` 与 `collect_statement_ru_point_lookup_evidence`；`classify_statement_ru_plan` 还被 forest 计算和 `adapter.rs::ClassifiedTypedPlan` 使用。

终结时，`StatementRUCalculator::finalize` 每次读取最新全局权重，调用 `ruv2::model::calculate` 得到总结果，同时调用 `statement_ru_engine_result` 得到 TiDB/TiKV/TiFlash 拆分。总 RU 已含一份 TiFlash RU，因此仅补上 `STATEMENT_RU_TIFLASH_MULTIPLIER - 1` 份，并把 TiFlash 分量本身乘以该倍率。任何总量或分引擎值为负数、NaN 或无穷时返回 `None`。full 模式会克隆报告，再把语句级单位加入克隆体，保证已发布快照不随原累加器变化。

发布时，`ExecStmt::FinishExecuteStmt` 保存 `Arc<StatementRUFinalizedSnapshot>` 并构造 `StatementRUContextSink`。`publish_statement_ru_finalized_snapshot` 依次尝试资源组消费、结果/full-report 指标、校准；三段分别用 `catch_unwind` 隔离。只有分引擎结果至少一项为正才调用 `consumption`，只有 full-report 存在才发布单位明细和校准。

## 数据与状态

所有计算核心都是值语义。`StatementRUPlanInfo` 只在分类调用期间借用计划；`StatementRUInstallState` 在安装边界复制会话事实；`StatementRUCalculator` 只保存 `StmtUnits`、固定长度三引擎数组和可选有界报告；`StatementRUFinalizedSnapshot` 则是可克隆的最终载荷。该设计明确禁止计划或运行统计引用越过终结阶段。

`StatementRUCalibrationState` 有 `Unknown`、`Complete`、`Incomplete` 三态，并映射为同名小写标签。当前 `finalize` 产生的快照固定为 `Incomplete`；完整性可由更上层证据处理更新。默认 SQL 类型是 `select`，计划遍历/终结路径可按分类改成 `insert`、`replace`、`update`、`delete`、`analyze` 或 `commit`。

`ScanEvidence` 的不变量是：任一输入为负即 `Invalid`；`processed_keys == 0 && processed_bytes == 0` 是合法的零扫描；无处理键却有处理字节是矛盾数据；有处理键但总键数或处理字节为零表示证据不足；其余情况必须得到有限且非负的估算值。

全局配置并未缓存在 calculator 内。`current_statement_ru_weights` 在每次 `finalize` 时读取 `astersql_config::get_global_config().ruv2.stmt_weights`，因此同一 calculator 在配置变化后再次终结会采用新权重。full-report 快照反过来会在终结时克隆，防止后续累加污染已发布数据。

## 依赖与调用关系

上游安装链为 `compiler.rs -> install_statement_ru_owner`：编译器先构造 `ExecStmt`、设置 `TypedPlan`（必要时补成 `RuntimeExecute`），再安装 RU owner。完成链为 `adapter.rs::ExecStmt::FinishExecuteStmt -> finishStatementRU -> StatementRUCalculator::finalize -> publish_statement_ru_finalized_snapshot`。RustCodeGraph 还确认 `FinishExecuteStmt` 由 `CloseRecordSet` 调用，说明发布位于 result set 关闭后的语句完成阶段。若没有 owner，完成路径仍尝试快照证据，但不会把本文件的资格规则绕开为可发布结果。

横向依赖中，`statement_ru_plan_walk.rs` 使用 `StatementRUCalculator`、`classify_scan_evidence` 和计划分类完成森林遍历；`statement_ru_reporting.rs` 提供 `StatementRUComputeUnits`、`StatementRUEngineResult`、`StatementRUFullReport`、引擎汇总及 full-report 指标发布；`adapter.rs` 提供 `ExecStmt`、`StatementNode` 和生产运行时接口。

下游数据依赖包括：`astersql-resourcegroup::ruv2::model` 负责权重模型计算；planner core/base/physicalop 和 parser AST 用于运行时类型分类；`astersql-config` 提供权重及报告模式；`astersql-kv::InternalTxnTTL` 定义 TTL 请求来源；`astersql-metrics::ru_v2` 接收最终汇总和标签化计数。

生产 sink 读取资源组名并检查 reporter 可用性；两者满足后以参数顺序 `(tikv, tidb, tiflash)` 调用 `ReportRUV2Consumption`。指标更新还会在安装时快照出的 `ttl_job` 为真时增加 `RUV2TTLTotal`，然后调用 `AddRUV2Results`；full-report 的逐单位计数通过 trait 的 `unit` 回调完成。

## 错误处理与边界

本文件没有 `Result` 型业务错误传播；资格不满足和数值无效以 `Option::None` 表达，扫描证据问题以显式枚举表达。`new_statement_ru_calculation_setup` 对缺失计划、缺失安装状态或任一排除条件返回 `None`。`StatementRUCalculator::finalize` 对模型拒绝的单位或非有限/负 RU 返回 `None`，因此调用者不能发布半初始化快照。

编译字节计算在计划缓存命中时严格为零。有 `original_sql` 时优先使用规范化 SQL，并且只识别 `explain analyze format = ? ` 与 `explain analyze format = ru ` 两种带尾随空格的前缀；规范化文本为空才回退到节点原文，最后回退到节点 text。返回的是 Rust 字符串字节长度，与 Go 的字符串长度语义一致，不是 Unicode 字符数。

发布函数把资源组、指标和校准分别放在 panic 边界内。指标/full-report 段 panic 且确有 full-report 时，会额外安全发布 `failed:panic`；资源组或校准自身 panic 不触发这个指标失败标签。result-only 模式没有 full-report，所以不发布单位、语句状态和校准，也不会因 results sink panic 再发布 failure。`StatementRUContextSink` 对 poisoned 指标初始化锁通过 `into_inner` 恢复，但全局指标尚未初始化时仍会在 `expect` 处 panic，由外层发布隔离捕获。

## 并发与资源生命周期

一次语句的 owner 使用 `Arc<StatementRUOwner>` 存放在 statement context 中，允许执行与完成边界共享所有权；本文件只负责创建该 owner，原子式“仅终结一次”的消费逻辑位于 `statement_ru_plan_walk.rs`。最终快照也以 `Arc` 缓存在 `StatementCtx.statement_ru_finalized`，使日志、慢查询和其他完成阶段消费者复用同一冻结结果，而不是重新读取可变执行证据。

`StatementRUCalculator` 本身不含锁、通道、后台任务或 I/O 资源，预期是终结调用栈内的局部值。配置在终结瞬间读取，报告在该时刻克隆；发布完成后不再借用 calculator。`StatementRUContextSink` 仅在调用期间借用 `AdapterRuntime`。

指标全局值受 `astersql_metrics::metrics::PACKAGE_INIT_LOCK` 保护：`results` 在锁内读取 TTL counter 并更新汇总指标；`unit` 和 `statement` 在锁内克隆具体 counter，离开锁后再按标签递增，缩短临界区。panic 隔离使用同步 `catch_unwind`，不会生成异步任务，也不会重试失败的副作用，因此每个 sink 方法必须自行保证操作的幂等需求。

## 与 Go 版本的对应关系

Rust 命名改为 snake_case/PascalCase，但主要数据形状和分支保持一一对应：`statementRUPlanKind`/`statementRUPlanInfo`、`statementRUCalculationSetup`、`statementRUCalculator`、扫描证据三态、TTL 三字段身份、当前权重读取和最终快照均有对应实现。

终结公式保持 Go 注释中的关键不变量：总结果先含原始 TiFlash RU，只额外增加倍率减一的部分；分引擎 TiFlash 再乘完整倍率。

发布结构在 Rust 中进一步抽象为 `StatementRUPublicationSink`，便于不依赖全局指标的独立测试；生产 `StatementRUContextSink` 仍复现 Go 的资源组、TTL、总结果、full-report 和校准顺序。Rust 当前把 TTL 身份放在 sink 构造参数中，而 Go 的 finalized snapshot 自带 `ttlJob`，这是载荷位置差异，不改变生产发布结果。

## 扩展指南

新增计划类别或 SQL 类型时，应同时修改 `StatementRUPlanKind`、`classify_statement_ru_plan` 和资格匹配，并核对 `statement_ru_plan_walk.rs` 如何为该类别采集单位及设置 `sql_type`。测试应扩展独立文件 `pkg/executor/statement_ru_result_test.rs` 的 `go_merge_197_setup_eligibility_plan_branches_and_owner`，并同步核对 Go 文件及 `pkg/executor/statement_ru_result_test.go`；不要把测试嵌入生产源文件。

新增计量单位或引擎时，不能只改 `StmtUnits`。需要同步 `StatementRUCalculator::compute` 的引擎维度、`statement_ru_engine_result`、full-report 结构和发布标签；尤其不能继续假设固定数组长度为 3。若新增引擎有特殊倍率，应明确总 RU 已含多少原始分量，避免重复计费。必须增加总量守恒、非有限值拒绝和冻结快照测试。

调整资格时，应优先修改 `StatementRUInstallState` 与 `new_statement_ru_calculation_setup` 的值边界，而不是让 calculator 读取 live session。任何新增 session 字段都应在安装时快照，并覆盖 restricted SQL、TTL、cursor、flat-plan 缓存和缺失 context 组合。full 模式“不合格用户工作计数、restricted 工作不计入”的现有行为也要维持或明确迁移。

新增发布消费者时，应先扩展 `StatementRUPublicationSink`，再实现生产 sink 与测试 sink。要明确它属于资源组、指标还是校准 panic 域，以及 result-only 模式是否应调用；不要让外部回调 panic 越过 `FinishExecuteStmt`。性能上应避免在默认 result-only 模式克隆 full-report，也不要扩大全局指标锁的持有范围。
