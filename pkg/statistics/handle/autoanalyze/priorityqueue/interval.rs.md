# `pkg/statistics/handle/autoanalyze/priorityqueue/interval.rs`

## 文件定位

该文件属于 Cargo 包 `astersql-statistics-handle-autoanalyze-priorityqueue`，由同目录 `lib.rs` 以 `mod interval` 纳入，并通过 `pub use interval::*` 把公开常量、`AnalysisHistoryReader` trait 及两个查询函数重新导出。Cargo 清单 `pkg/statistics/handle/autoanalyze/priorityqueue/Cargo.toml` 将该 crate 对应到 Go 包 `pkg/statistics/handle/autoanalyze/priorityqueue`；本文件本身只使用标准库 `std::time::Duration` 和同 crate 的 `job::DEFAULT_FAILED_ANALYSIS_WAIT_TIME`，没有新增外部依赖或 feature 条件。

它不是“允许自动分析的每日时间窗口”实现；这里的 interval 指分析历史记录中的耗时/距上次失败的间隔。文件负责把 `mysql.analyze_jobs` 查询抽象成两类时长，供优先队列在调度分析作业前执行失败冷却判断。Go 生产链已经由 `job.go::isValidToAnalyze` 调用两个函数；精确 Rust 引用搜索显示当前 Rust 生产代码尚未调用它们，只有 `interval_test.rs` 和 `intervaltimezone/interval_timezone_test.rs` 使用该 API，因此当前 Rust 状态应描述为“查询适配层已实现并测试，但尚未接入生产调度链”。

## 核心职责

1. 用四个 SQL 常量区分普通表与指定分区集合，并分别查询最近成功 ANALYZE 的平均耗时、最近失败 ANALYZE 距当前时间的秒数。
2. 通过 `AnalysisHistoryReader` 隔离实际会话/SQL 执行机制，使本模块只负责选择 SQL、排列参数和映射结果，不依赖具体数据库客户端。
3. 将数据库结果转换为 `Option<Duration>`：`None` 表示无有效历史，`Some(Duration::ZERO)` 表示刚失败，正时长表示观测值；失败间隔为负时使用默认 30 分钟冷却时间。
4. 保留 Go 移植所需的查询筛选、最近五条规则、分区保守策略和异常数据防御语义，同时把 Go 的 `time.Duration(-1)` 哨兵改为 Rust 的 `None`。

## 主要符号

- `NO_RECORD: i64 = -1` 与 `JUST_FAILED: i64 = 0`：对应 Go 的 `NoRecord` 和 `justFailed` 数值语义。实际 Rust 返回 API 用 `Option<Duration>`/`Duration::ZERO` 表达这两种状态，函数体不直接返回这两个整数常量；它们主要保留移植语义和公开兼容标识。
- `AVG_DURATION_QUERY_FOR_TABLE`：筛选 schema、table、`state = 'finished'`、`fail_reason IS NULL`、空分区名，按 `id DESC` 取最近五条，再计算 `TIMESTAMPDIFF(SECOND, start_time, end_time)` 的平均值。
- `AVG_DURATION_QUERY_FOR_PARTITION`：与表级平均查询相同，但用 `partition_name in (%?)` 限制指定分区；“最近五条”是在所有指定分区合并后的结果中按 id 选取，不是每个分区各取五条。
- `LAST_FAILED_DURATION_QUERY_FOR_TABLE`：对空分区名的失败记录按 id 倒序取一条，计算 `start_time` 到 `CURRENT_TIMESTAMP` 的秒数。
- `LAST_FAILED_DURATION_QUERY_FOR_PARTITION`：先按分区分组取各自最大的失败记录 id，再连接原表并取这些最近失败记录距今秒数的最小值。最小值代表指定分区中最近发生的失败，采用更保守的冷却判断。
- `AnalysisHistoryReader`：公开同步 trait。`query_optional_f64` 服务平均值查询，`query_optional_i64` 服务失败间隔查询；二者接收 SQL 和有序字符串参数，返回 `Result<Option<_>, String>`。`None` 同时覆盖无行或 SQL 聚合结果为 NULL 的适配语义。
- `query_parts`：私有选择器。先放入 schema、table；无分区时返回表级 SQL，有分区时按调用者给定顺序逐个追加分区名并返回分区级 SQL。
- `GetAverageAnalysisDuration`：公开函数，查询可选浮点秒数；非负值先向下取整再转换为 `Duration`，无值或负值返回 `Ok(None)`，读取错误原样返回。
- `GetLastFailedAnalysisDuration`：公开函数，查询可选整数秒数；无值返回 `None`，零返回 `Duration::ZERO`，负数回退到 `DEFAULT_FAILED_ANALYSIS_WAIT_TIME`，正数转换为秒级 `Duration`。

## 执行流程

平均耗时流程从 `GetAverageAnalysisDuration` 开始：调用 `query_parts` 根据 `partition_names.is_empty()` 选择表级或分区级 SQL并构造参数；再调用 `AnalysisHistoryReader::query_optional_f64`。若 reader 返回错误，`?` 立即向上传播；若得到非负浮点秒数，函数取 `floor()` 并构造 `Duration`；若无记录或数值为负，则返回 `None`。SQL 已限定成功、无失败原因的记录，并只对最新五条求平均。

失败间隔流程由 `GetLastFailedAnalysisDuration` 以相同方式选择 SQL 和参数，然后调用 `query_optional_i64`。结果依次分流为无记录、刚失败、异常负间隔、正常正间隔四类。分区查询在 SQL 内先找每个分区最后一次失败，再取其中最短的距今间隔；这使任一分区刚失败时，整个指定分区集合都继续冷却。

在完整 Go 调度链中，`job.go::isValidToAnalyze` 先读取失败间隔，再读取平均耗时：刚失败直接拒绝；没有平均历史时要求至少等待默认 30 分钟；有平均历史时要求失败间隔至少达到平均耗时的两倍。Rust 的 `job.rs::is_valid_to_analyze` 已实现接收 `Option<Duration>` 后的判断逻辑，但当前没有调用本文件两个查询函数的生产接线，也没有生产 `AnalysisHistoryReader` 实现；扩展文档时应保持这一区分。

## 数据与状态

模块不保存全局或可变状态。每次调用临时分配一个 `Vec<String>`：前两个元素固定为 schema 和 table，随后是零个或多个分区名。分区顺序不会排序或去重，`interval_test.rs::partition_queries_preserve_names_and_order` 明确验证顺序原样传递；调用方或 reader 必须负责将多个参数安全绑定到 `%?` 占位语义，而不是字符串拼接。

返回状态使用 `Result<Option<Duration>, String>` 两层编码：`Err(String)` 是查询层失败，`Ok(None)` 是没有可用历史或平均耗时为负，`Ok(Some(_))` 是可参与冷却判断的时长。失败间隔的负值不被当作无记录，因为未来时间、时钟或时区异常若绕过冷却可能立即重试；它被钳制为 `DEFAULT_FAILED_ANALYSIS_WAIT_TIME`（`job.rs` 定义为 30 分钟）。

平均查询接收 `f64` 是为了适配 SQL `AVG`；仅保留整秒，且非负值向下取整。失败查询的正 `i64` 才转换为 `u64`，负值在转换前已分支处理，因此不会发生负数转无符号数的环绕。

## 依赖与调用关系

- 模块装配：`priorityqueue/lib.rs` 声明并公开重导出 `interval`；`Cargo.toml` 以 `lib.rs` 为 crate 根，并声明 Go 包迁移元数据。
- 本文件下游：`query_parts`、`AnalysisHistoryReader::{query_optional_f64, query_optional_i64}`、`std::time::Duration`，以及 `job.rs::DEFAULT_FAILED_ANALYSIS_WAIT_TIME`。
- Rust 图证据：RustCodeGraph 将目标文件识别为 7 个符号；对 `GetAverageAnalysisDuration`、`GetLastFailedAnalysisDuration` 和 `query_parts` 的 callers/callees 查询没有返回调用边，并只把 `interval_test.rs` 报告为使用文件。由于常见名称图查询存在噪声，随后用精确引用搜索核验。
- Rust 实际引用：`interval_test.rs` 直接覆盖两个函数；`intervaltimezone/interval_timezone_test.rs` 为 `GetLastFailedAnalysisDuration` 提供内存 reader，验证时区污染场景。除这些测试外，没有 Rust 生产调用者或生产 trait 实现。
- Go 生产上游：`job.go::isValidToAnalyze` 在决定是否分析表/分区前依次调用 `GetLastFailedAnalysisDuration` 和 `GetAverageAnalysisDuration`，然后执行冷却策略。Go 的 interval 测试还直接调用两个函数。
- 逻辑下游数据源：四个 SQL 查询 `mysql.analyze_jobs`。本模块不执行 SQL；具体连接、会话时区、占位符展开和行解码均属于 `AnalysisHistoryReader` 实现的职责。

## 错误处理与边界

- reader 的 `Err(String)` 经 `?` 不包装地传播；`interval_test.rs::query_errors_are_propagated_unchanged` 验证错误文本不变。当前错误类型缺少结构化分类和上下文，接入生产 reader 时需由实现端提供足够可诊断的信息。
- 无记录或聚合 NULL 由 reader 统一表达为 `Ok(None)`。平均耗时负数也映射为 `None`；失败间隔负数则映射为默认冷却，两者有意不同。
- `Some(0)` 对失败间隔表示刚失败；平均耗时的 `Some(0.0)` 则是合法的零时长，两者都转换为 `Duration::ZERO`，语义由调用的函数区分。
- `Duration::from_secs_f64` 对非有限值或超范围数值可能 panic；当前只防御 `< 0.0`，没有显式拒绝 `NaN`、正无穷或过大的平均值。生产 reader 应只返回数据库可表示的有限秒数；若未来输入边界扩大，应在函数内增加 `is_finite`/范围检查及独立回归测试。
- schema、table、分区列表在本层没有做空字符串、重复项或数量限制检查；安全性依赖参数绑定。不得把它们插值进 SQL 文本。
- 查询使用 `CURRENT_TIMESTAMP`，结果受执行会话的时区和时钟影响。负失败间隔有 30 分钟回退，但不等价于正确的会话时区；`intervaltimezone` 测试说明 session timezone 重置仍是上游资源管理责任。

## 并发与资源生命周期

所有 API 都是同步借用调用：reader 以 `&dyn AnalysisHistoryReader` 借用，schema/table/分区名在调用期间借用，返回值不持有这些引用。函数没有锁、线程、异步任务、通道、事务或连接生命周期，也没有静态可变状态；临时参数向量在函数返回时释放。

trait 没有要求 `Send` 或 `Sync`，因此本模块本身不保证同一 reader 能跨线程共享。`interval_test.rs::History` 用 `RefCell` 记录调用，证明实现允许通过内部可变性在 `&self` 方法中维护状态，但这种测试替身不是线程安全契约。数据库会话的获取、释放、事务边界及时区复位必须由生产 reader/调用者管理；本模块每次只发起一次 reader 查询，不做重试。

## 与 Go 版本的对应关系

四段 SQL 的筛选与排序意图和 `interval.go` 对齐：普通表用空分区名，分区表使用指定列表，成功平均值取跨目标集合的最近五条，失败分区查询取各分区最后一次失败后的最短间隔。Rust 将 Go 直接依赖的 `sessionctx.Context + util.ExecRows` 改为 `AnalysisHistoryReader`，便于不启动数据库地验证映射逻辑。

返回类型是主要表示差异。Go 用 `time.Duration(-1)` 表示无记录；Rust 用 `None`，避免构造不存在的负 `std::time::Duration`。Go 的刚失败值为零，Rust 用 `Some(Duration::ZERO)`。Go 的负失败间隔回退 `defaultFailedAnalysisWaitTime`，Rust 对应 `DEFAULT_FAILED_ANALYSIS_WAIT_TIME`。Go 把非负浮点平均值转换为整数秒时会截断小数；Rust 显式 `floor()`，在已拒绝负数的前提下结果一致。

参数形态存在适配边界：Go 把 `partitionNames` 切片作为一个参数传给支持 `%?` 列表展开的执行器；Rust `query_parts` 把每个分区名依次展开进 `Vec<String>`。因此生产 `AnalysisHistoryReader` 必须按该约定处理分区占位，不能机械照搬 Go 参数容器。

测试覆盖层级也不同：Go `interval_test.go` 通过 mock store 和真实 SQL 行验证空表、最近五条、表/多分区、负平均值、负失败间隔；Rust `interval_test.rs` 是 reader 契约单元测试，验证 SQL 选择、参数顺序、数值映射与错误传播。Rust `intervaltimezone/interval_timezone_test.rs` 使用内存作业存储验证失败间隔受正确会话时区驱动，但它不是对真实 SQL/会话池的集成测试。

## 扩展指南

- 接入 Rust 生产调度时，先实现基于实际统计会话的 `AnalysisHistoryReader`，明确 `%?` 分区列表展开、NULL/空行映射、时区复位和错误上下文；再在 `job.rs` 对应 Go `isValidToAnalyze` 的入口调用两个函数。不要在本文件复制会话池或 SQL 执行子系统。
- 修改历史选择规则时，应同步更新对应 SQL 常量、Go `interval.go`（若要求继续双实现对齐）以及独立的 `interval_test.rs`；涉及真实 SQL 行选择、NULL、小数或多分区行为时，还需同步/补充 `interval_test.go` 等集成证据。
- 修改失败冷却回退时，应同时审查 `GetLastFailedAnalysisDuration`、`job.rs::DEFAULT_FAILED_ANALYSIS_WAIT_TIME`、`job.rs::is_valid_to_analyze` 及各自独立测试，避免查询层回退值与策略层阈值分叉。
- 新增边界防御时，优先覆盖 `NaN`、正无穷、超大浮点秒数、空/重复分区及 reader 绑定错误；Rust 测试必须继续放在同目录独立测试文件，不能内嵌进 `interval.rs`。
- 若引入缓存或并发 reader，需先定义时效性、锁粒度、会话归还和时区清理契约；当前 trait 不承诺 `Send + Sync`，不能默认跨线程安全。
- 性能风险主要在 `mysql.analyze_jobs` 的筛选、排序、分组和 join，而非 Rust 映射代码。调整查询时应确认相关索引和大分区列表的执行计划，且保持“最新五条”和“最近失败优先”的语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter .../interval.rs` 报告该文件有 7 个符号；`node --file .../interval.rs` 读取 1–124 行；精确 `query` 找到 `AnalysisHistoryReader`、`query_parts`、两个公开函数；对三个函数执行 `callers`/`callees` 未得到边，目标文件节点只报告 `interval_test.rs` 使用。
- Rust 源与装配：`pkg/statistics/handle/autoanalyze/priorityqueue/interval.rs`、`lib.rs`、`job.rs`（默认等待时间和 `is_valid_to_analyze` 冷却逻辑）、`Cargo.toml`。
- Rust 独立测试：`interval_test.rs` 验证表/分区 SQL 选择、参数顺序、平均值向下取整、无值/负值/零值/正值映射及原样错误传播；`intervaltimezone/interval_timezone_test.rs::test_last_failed_analysis_duration_use_correct_timezone` 验证受污染系统时区下的正且小于一分钟的失败间隔。
- Go 对照与测试：`interval.go`、`job.go::isValidToAnalyze`、`interval_test.go`、`intervaltimezone/interval_timezone_test.go`。
- 精确引用核验：`rg` 在全部 Rust/Go 文件中确认 Go 生产调用边及 Rust 仅测试引用，弥补 RustCodeGraph 对本文件调用边为空的限制。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试；交付验证仅执行任务指定的 11 章节结构检查和文档范围/差异人工复核。
