# `pkg/statistics/handle/autoanalyze/autoanalyze.rs`

## 文件定位

本文件是 `astersql-statistics-handle-autoanalyze` crate 的核心实现文件，由同目录 `lib.rs` 以 `pub mod autoanalyze` 声明并通过 `pub use autoanalyze::*` 全量再导出。它把 Go 版本 `pkg/statistics/handle/autoanalyze/autoanalyze.go` 中较适合独立验证的部分拆成三组 Rust 能力：维护 `mysql.analyze_jobs` 生命周期、根据统计摘要生成自动 `ANALYZE` 请求、通过 `StatsAnalyze<R>` 驱动优先级刷新器。

当前接线状态必须与 Go 版本区分：RustCodeGraph 没有找到本文件公开入口的生产调用者，只有同 crate 的 `autoanalyze_test.rs` 直接使用这些 API；`Cargo.toml` 中完整 TiDB 子系统依赖位于 `[target.'cfg(any())'.dependencies]`，恒假条件意味着它们不会进入正常构建。因而本文件目前是标准库加 trait/值对象构成的可测试移植层，不是 Go `statsAnalyze` 在 Domain 中的完整替代品。Go 的真实生产入口仍是 `statsAnalyze.HandleAutoAnalyze`，由 `pkg/domain/domain.go` 的 `autoAnalyzeWorker` 周期调用。

## 核心职责

- 用 `AnalyzeJob`、`AnalyzeProgress`、`JobStore` 和一组生命周期函数表示并持久化分析作业；状态依次为 `pending`、`running`、`finished`/`failed`，结束时清空 `process_id`。
- 用 `cleanup_corrupted_jobs_on_current_instance` 和 `cleanup_corrupted_jobs_on_dead_instances` 找出超过时限且已失去所属进程或实例的作业，并批量置为失败。
- 用 `need_analyze_table` 判断未分析表或修改比例超阈值的表；用 `random_pick_one_table_and_try_auto_analyze` 在时间窗口、锁、schema、视图和最小行数等约束下选择一个表或一组分区。
- 用 `AnalyzeRequest` 返回参数化 SQL 模板、标识符参数、目标统计版本和 snapshot 标志。本层只生成请求，不执行 `ANALYZE`。
- 用 `PriorityRefresher` 抽象优先级队列的外部实现，`StatsAnalyze<R>` 只规定调用次序和关闭语义。

## 主要符号

- 常量：`SELECT_ANALYZE_JOBS_ON_CURRENT_INSTANCE_SQL`、`SELECT_ANALYZE_JOBS_SQL` 和 `BATCH_UPDATE_ANALYZE_JOB_SQL` 定义坏作业查询/批量失败 SQL；`AUTO_ANALYZE_MIN_COUNT` 为 1,000 行；内部 `TEXT_MAX_LENGTH`、`MAX_PROGRESS_DELTA`、`DUMP_INTERVAL` 分别约束文本字节数、进度刷盘增量和最小刷盘间隔。
- 错误与存储：`Error(String)` 是简单错误包装；`SqlValue` 是 SQL 绑定值；`JobStore` 提供执行 SQL、读取自增 ID、查询过期作业和存活实例的最小接口。
- 作业模型：`JobType::{TableAnalysis, GlobalStatsMerge}` 区分是否需要在结束时补刷剩余行数；`AnalyzeProgress` 保存未刷盘增量和上次刷盘时间；`AnalyzeJob` 保存作业 ID、对象名、描述、起止时间和进度。
- 调度模型：`IndexInfo`、`TableStats`、`Partition`、`TableInfo`、`SchemaInfo` 是从完整 TiDB 元数据/统计对象压缩出的值对象；`PartitionPruneMode` 区分静态逐分区和动态批量；`AnalyzeRequest` 是生成结果。
- 公开算法：`need_analyze_table`、`within_day_time_period`、`parse_auto_analyze_window`、`random_pick_one_table_and_try_auto_analyze`、`analyze_version_matches_for_table`、`ten_minutes_ago`、`group_stats_by_partition`。
- 内部算法：`truncate_text`、`parse_hhmm`、`analyze_table`、`analyze_dynamic_partitions`、`partition_requests`、`analyze_version_matches`、`shuffle`、`epoch_seconds` 和 `batch_fail_jobs`。
- 队列门面：`PriorityRefresher` 定义分析、测试同步及关闭钩子；`StatsAnalyze<R>` 提供 `new`、`handle_priority_queue`、`close_priority_queue`、`close`。

## 执行流程

1. 作业创建时，`insert_analyze_job` 先用 `truncate_text` 将 `job_info` 截到 65,535 字节且保持 UTF-8 边界，再插入 `pending` 行并用 `last_insert_id` 回填 `AnalyzeJob.id`。插入或读取 ID 的错误通过 `Result` 返回。
2. `start_analyze_job` 对 `None` 或无 ID 作业静默返回；否则记录内存开始时间、初始化进度 dump 时间并尽力把数据库状态改为 `running`。`update_analyze_job_progress` 累加行数，只有累计值严格大于 10,000,000 且距上次 dump 严格超过 5 秒才增加 `processed_rows`。
3. `finish_analyze_job` 选择 `finished` 或 `failed`，记录结束时间、可选截断后的失败原因并清空 `process_id`；只有 `TableAnalysis` 把尚未刷盘的 `delta_count` 补入 `processed_rows`，`GlobalStatsMerge` 不补刷。
4. 坏作业清理分两条路径：当前实例路径保留 `process_id` 为空的记录，也保留仍在运行集合中的记录，只失败“有 process_id 但不再运行”的作业；死亡实例路径先读取存活实例集合，再失败归属不在集合中的过期作业。空 ID 集合不会执行更新。
5. 调度前可由 `parse_auto_analyze_window` 把 `HH:MM` 转为分钟，并由 `within_day_time_period` 判断普通或跨午夜窗口。`random_pick_one_table_and_try_auto_analyze` 先按确定性 xorshift 种子打乱 schema 和表；跳过系统/内存库、视图、锁表，并在每张表前重新调用 `window_open`，窗口关闭立即终止本轮。
6. 非分区表进入 `analyze_table`：缺失统计、伪统计或行数低于门槛时跳过；否则优先因未分析/修改比例生成整表请求，其次寻找 public、非 columnar 且尚未分析的索引。
7. 分区表在静态模式逐分区复用 `analyze_table`，命中第一项即返回一条请求；动态模式先收集所有需要重分析的分区并按 `partition_batch_size.max(1)` 分批，若没有脏分区再按索引收集缺失统计的分区。动态索引还跳过 `special_global` 索引。
8. `StatsAnalyze::handle_priority_queue` 在测试模式先处理 DML 变化、重排必须重试的作业，随后调用一次最高优先级分析，最后等待测试任务结束；非测试模式只调用分析入口并返回其布尔结果。

## 数据与状态

`AnalyzeJob.id` 是否存在是生命周期写操作的门槛；开始/进度/结束函数不会自行插入作业。`AnalyzeProgress.delta_count` 是尚未持久化的累计值，达到刷盘条件后归零；失败的持久化不会回滚这个内存变化，因为开始、进度和结束路径都忽略 `JobStore::execute` 的错误。

`TableStats.analyzed` 控制“从未分析”分支；已分析表使用 `analyze_row_count`（若大于零）作为修改率分母，否则使用 `realtime_count`。调用 `need_analyze_table` 本身没有最小行数/伪统计保护，这两项由 `analyze_table` 和动态分区过滤器负责。`ratio == 0` 只禁止已分析表按修改量触发，未分析表和缺失普通索引仍可触发。

`AnalyzeRequest.params` 按数据库、表、分区列表、可选索引的顺序保存标识符；动态分区 SQL 使用逗号分隔 `%n`。`snapshot` 在参与判断的既有统计版本与请求版本不一致时为真；缺失、伪统计或版本小于等于零按 Go 语义视为兼容。`group_stats_by_partition` 只收集 `stats.is_some()` 的分区并克隆统计对象。

## 依赖与调用关系

RustCodeGraph 的下游调用边显示：`insert_analyze_job` 调用 `JobStore::execute`、`last_insert_id`、`truncate_text`；`start_analyze_job` 调用进度初始化、`epoch_seconds` 和 `execute`；`update_analyze_job_progress` 调用 `AnalyzeProgress::update` 和 `execute`；`finish_analyze_job` 调用 `delta_count`、`truncate_text`、`epoch_seconds` 和 `execute`。两条清理入口分别调用 `JobStore` 查询方法和 `batch_fail_jobs`。调度入口调用 `shuffle`、`analyze_table`、`analyze_dynamic_partitions`，后两者再落到 `need_analyze_table`、版本判断与 `partition_requests`。

本 Rust 文件只直接依赖 `std::{collections, fmt, time}`。同目录 `Cargo.toml` 的正常依赖为空，dev-dependencies 提供 Domain、statistics、handle 与 testkit 测试支撑；列出的 infoschema、sessionctx、lockstats、exec、refresher 等完整生产依赖全部位于 `cfg(any())` 下。RustCodeGraph 对本文件公开函数没有返回生产 callers，这与 `lib.rs` 的测试模块直接调用和当前“独立移植层”定位一致。

Go 对照的真实上游链是 `pkg/domain/domain.go:autoAnalyzeWorker -> statsAnalyze.HandleAutoAnalyze -> statsAnalyze.handleAutoAnalyze -> RandomPickOneTableAndTryAutoAnalyze -> tryAutoAnalyzeTable`；优先级队列开启时，`handleAutoAnalyze` 改走 `refresher.AnalyzeHighestPriorityTables`。Go 文件还被 executor 的分析流程用于作业生命周期，而 Rust 版本尚未复刻这些 session pool、system process tracker、真实 SQL executor 和日志接线。

## 错误处理与边界

- `insert_analyze_job` 以及两条清理路径传播存储错误；`start_analyze_job`、`update_analyze_job_progress`、`finish_analyze_job` 则刻意 best-effort，忽略 SQL 更新错误。Rust 版本没有复刻 Go 的告警日志和 `DebugAnalyzeJobOperations` failpoint，因此调用者无法从返回值获知这些更新失败。
- `truncate_text` 按字节限制并回退到 UTF-8 字符边界，比 Go 当前直接字节切片更安全；但这也属于可观察的移植差异。
- `epoch_seconds` 对早于 Unix epoch 的时间返回 0；`ten_minutes_ago` 下溢时也返回 epoch。
- `parse_hhmm` 要求恰有冒号、数字小时/分钟且范围分别不超过 23/59；错误包含原输入。窗口端点是闭区间，起点大于终点表示跨午夜，起止相等只开放该分钟。
- 修改比例判断使用严格“大于阈值”：等于 `ratio` 时不分析。正常调度已排除小表，所以 `need_analyze_table` 中潜在的零分母不会成为常规入口的独立保护条件。
- 清理当前实例时 `process_id == NULL` 不会被选中，这与 Go 代码的空值检查一致；批量 ID 在 Rust trait 边界被作为逗号连接的单个 `SqlValue::Text` 交给存储实现，真实 SQL 转义/列表展开责任仍在未接入的 adapter。
- `partition_batch_size` 在公开调度入口被提升到至少 1，避免 `chunks(0)`；内部 `partition_requests` 假设调用者已满足该不变量。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、锁或事务实现。所有可变状态通过 `&mut JobStore`、`&mut AnalyzeJob`、`&mut [SchemaInfo]` 和 `&mut self` 串行传入，因此并发安全、事务边界、连接池归还和取消行为均由未来 adapter/调用方负责。

`StatsAnalyze<R>` 拥有 refresher；`close_priority_queue` 只关闭队列，`close` 才关闭整个 refresher，这一差异对应 Go 注释中的“关闭队列不会停止 analyze worker”。Rust 类型没有实现 `Drop`，资源不会在离开作用域时自动关闭，调用方必须显式遵守所提供的关闭协议。测试模式的 `wait_finished_for_test` 是确定性同步钩子，不代表生产执行会阻塞等待分析完成。

作业生命周期的数据库操作在 Rust trait 中没有显式事务包装。Go 的两条坏作业清理入口经 `CallWithSCtx(..., FlagWrapTxn)` 执行，而 Rust 版本只能依赖具体 `JobStore` 实现提供一致性；当前 `FakeJobStore` 仅记录调用，不模拟事务或并发更新。

## 与 Go 版本的对应关系

Rust `AnalyzeJob`/`AnalyzeProgress`、四个生命周期函数和三条 SQL 常量对应 Go `statistics.AnalyzeJob` 以及 `insertAnalyzeJob`、`startAnalyzeJob`、`updateAnalyzeJobProgress`、`finishAnalyzeJob`、两条 `CleanupCorrupted...`。主要状态选择、剩余进度只用于 TableAnalysis、文本上限和清空 `process_id` 的意图保持一致；Rust 用 `JobStore` 替代 Go 的 session context、`ExecRows`、infosync 和日志/failpoint。

Rust `need_analyze_table` 对应 Go `NeedAnalyzeTable`；`random_pick_one_table_and_try_auto_analyze`、`analyze_table`、`analyze_dynamic_partitions`、`partition_requests` 分别压缩了 Go 的随机 schema/table 扫描、`tryAutoAnalyzeTable` 和 `tryAutoAnalyzePartitionTableInDynamicMode`。Rust 保留了锁表/锁分区、系统库/视图、伪统计、小表、普通/列存/特殊全局索引、动态分批及版本 snapshot 等核心分支，但返回请求而非调用 `exec.AutoAnalyze`，也没有真实 infoschema、锁查询、日志和每三秒调度。

Rust `StatsAnalyze<R>` 对应 Go `statsAnalyze` 的优先队列片段，不包含 Go 的 `statsHandle`、`sysProcTracker`、`NewStatsAnalyze`、经典随机路径参数读取或 panic recovery。`parse_auto_analyze_window` 只接受 `HH:MM`，而 Go 生产路径通过 `exec.ParseAutoAnalysisWindow` 处理系统变量时间格式和时区；两者不可直接视为完整等价解析器。

独立 Rust 测试 `autoanalyze_test.rs` 明确区分两类覆盖：`FakeJobStore`/值对象测试直接验证本文件的纯逻辑；后半部分 testkit 用例验证更完整的 Rust Domain 行为，但不等于这些 Domain 路径已调用本文件所有 API。Go `autoanalyze_test.go` 则通过 mock TiDB store/domain、真实 SQL 和 failpoint 覆盖完整接线。

## 扩展指南

- 修改作业状态或 SQL 时，应同步检查 `insert_analyze_job`、`start_analyze_job`、`update_analyze_job_progress`、`finish_analyze_job`、`JobStore` 适配边界，以及 `autoanalyze_test.rs` 的 `FakeJobStore.executed` 精确断言；若要达到 Go 生产等价，还需补真实 session/事务/log/failpoint adapter，不能只扩展值对象。
- 修改挑表规则时，入口是 `random_pick_one_table_and_try_auto_analyze`；非分区逻辑在 `analyze_table`，动态分区逻辑在 `analyze_dynamic_partitions`，SQL 批次格式在 `partition_requests`。应同时覆盖静态/动态裁剪、锁表和锁分区、窗口中途关闭、伪统计、小表、普通/columnar/special-global 索引以及 batch size 边界。
- 修改统计版本策略时，应同步 `analyze_version_matches`、`analyze_version_matches_for_table` 与 request 的 `snapshot` 计算，并保持缺失/伪/未版本化统计的 Go 兼容语义。
- 新增并发或资源型 refresher 时，实现 `PriorityRefresher` 并明确 `close_priority_queue` 与 `close` 的幂等性、等待行为和调用顺序；相关测试继续放在独立的 `autoanalyze_test.rs`，不要嵌入生产源文件。
- 若将本 crate 接入生产主链，首先应把 `Cargo.toml` 中所需依赖移出 `cfg(any())`，建立 Go `statsAnalyze` 等价 adapter，再用 RustCodeGraph 确认 Domain/Executor 到本文件的 caller 边；在此之前不应宣称本文件已经执行真实自动分析。
- 性能风险主要在扫描/克隆全部 schema、表、分区和参数，以及为动态分区生成多个请求；兼容性风险主要在 SQL 占位符列表、时间/时区格式、文本截断、版本 snapshot 与锁信息新鲜度。

## 验证依据

- RustCodeGraph 索引状态：项目含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录列出了 `autoanalyze.rs`、`lib.rs`、Go 对照和独立测试。通过 `node --file` 完整读取了 `autoanalyze.rs` 1–754 行及 `lib.rs` 1–18 行。
- RustCodeGraph 查询：精确查询并核对了 `insert_analyze_job`、`start_analyze_job`、`update_analyze_job_progress`、`finish_analyze_job`、两条 `cleanup_corrupted_jobs...`、`random_pick_one_table_and_try_auto_analyze`、`analyze_version_matches_for_table` 和 `handle_priority_queue`；`callees` 证实上述内部调用边，`callers` 未返回本文件公开入口的生产调用者。
- crate 证据：`pkg/statistics/handle/autoanalyze/Cargo.toml` 声明包名和 `lib.rs` 入口；正常依赖为空，完整移植依赖位于 `target.'cfg(any())'`，测试依赖位于 `dev-dependencies`。`lib.rs` 全量再导出实现，并只在 `cfg(test)` 下装配 `autoanalyze_test.rs`。
- Go 对照：通过 RustCodeGraph 读取 `pkg/statistics/handle/autoanalyze/autoanalyze.go`，核对了 `statsAnalyze`、Domain 调用链、随机挑表、静态/动态分区、生命周期 SQL、坏作业清理和版本策略；读取 `autoanalyze_test.go` 核对完整 store/domain、failpoint、时间窗口、锁、索引和清理场景。
- Rust 测试：通过 RustCodeGraph 读取 `pkg/statistics/handle/autoanalyze/autoanalyze_test.rs`，核对 `NeedAnalyzeTable`、坏作业清理、锁、窗口、普通/columnar 索引、版本兼容、动态分区逗号与分批、时间解析、十分钟计算、分区映射以及 testkit 集成场景。
- 本任务按计划为纯文档分析，未运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核只新增本文档、未修改 Rust/Go/Cargo/`plan.md`。
