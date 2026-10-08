# `pkg/session/runtime/statistics.rs`

## 文件定位

本文件属于 `astersql-session` crate（`pkg/session/Cargo.toml` 的包名），由 `pkg/session/runtime.rs:117` 以私有模块 `statistics` 装配。它是会话层连接 SQL AST、Domain 统计上下文和执行器统计接口的适配层，集中承接四类工作：执行 `ANALYZE`、呈现 `SHOW STATS_*`/`SHOW ANALYZE STATUS`、生成常规 `SHOW COLUMNS/INDEX/CREATE TABLE/TABLE STATUS` 元数据，以及代理 `mysql.stats_*`、`mysql.analyze_jobs` 等受限系统表访问（`statistics.rs:614-1169, 1340-1567, 1569-3569`）。

上游主入口来自 `pkg/session/runtime/dispatch.rs`：普通 `ANALYZE` 调用 `execute_analyze`，`FLUSH STATS_DELTA` 调用 `execute_flush_stats_delta`，各类 `SHOW` 调用对应 `execute_show_*`，系统统计表 `SELECT` 调用 `execute_stats_system_select`；`pkg/session/runtime/control.rs` 分派统计表 `UPDATE`/`DELETE`。DDL 内部分析和类型化分析执行器还分别从 `pkg/session/runtime/ddl.rs:1879`、`pkg/session/runtime/typed_analyze_executor.rs:107` 进入本文件。

## 核心职责

1. 将 `ast::AnalyzeTableStmt` 解析为 `SessionAnalyzeInput`，解析/合并持久化分析选项，过滤锁定对象、视图、序列、分区、列和索引，并创建实现 `astersql_executor::analyze::CanonicalAnalyzeRuntime` 的 `SessionAnalyzeRuntime`（`execute_analyze_with_context`）。
2. 通过 `prepare_batch` 构建表、分区及动态裁剪模式下的全局统计；通过 `save_batch` 暴露保存阶段与失败点；通过 `publish_batch` 原子地发布缓存、列使用信息、持久化元数据和历史统计；失败时用 `record_failure` 记录任务状态。
3. 将 Domain 的目录、锁表、物理统计、列使用和任务状态适配为 `ShowStatsRuntime`，交给 `astersql_executor::show_stats::ShowExec` 生成 `SHOW STATS_META/HISTOGRAMS/TOPN/BUCKETS/HEALTHY/LOCKED` 等结果（`SessionShowStatsRuntime`、`execute_show_stats`）。
4. 补足会话拥有的展示与系统表兼容逻辑，包括标识符引用、时区化更新时间、`SHOW CREATE TABLE` 的区域拆分策略、`mysql.analyze_jobs` 的投影/过滤/排序/分页，以及受限统计 SQL 的字面量和 DML 路由。
5. 提供只用于独立测试的暂停、取消和保存错误注入守卫，复现 ANALYZE 构建/发布时序（`AnalyzePauseGuard`、`AnalyzeSaveErrorGuard`）。

## 主要符号

- `SessionAnalyzeInput`：一次表级输入，保存库表键、表元数据、会话可见行、目标分区、目标列/索引；列与索引顺序保持表定义顺序，以生成兼容的任务描述。
- `SessionAnalyzeRuntime`：ANALYZE 管线后端，持有 `Domain`、`SQLKiller`、输入、选项、时区、并发计数、内存配额和快照注入基线。它实现 `CanonicalAnalyzeRuntime::{check_killed, preflush_stats_delta, prepare_batch, save_batch, publish_batch, record_failure}`（`statistics.rs:614-1169`）。
- `SessionShowStatsRuntime`：只读 SHOW 适配器；实现 schema/表枚举、全局和分区物理统计读取、锁表集合、通配匹配、分析任务及列使用读取（`statistics.rs:1178-1567`）。
- `ConcreteSession::execute_analyze` / `execute_analyze_with_context`：SQL 入口及完整准备逻辑；前者区分手工与 restricted SQL 并记录成功/失败指标，后者驱动 canonical executor。
- `merge_previous_analyze_stats`：选择性 ANALYZE 时保留未重新分析的列/索引；只有新结果尚未分析/合成时才由旧条目补位。
- `format_table_update_time`：把 `TableInfo.UpdateTS` 的 TSO 物理毫秒部分转换为会话命名或固定时区的 `DATETIME`；零值或非法时间返回 `None`。
- `execute_show_stats`：按照 `ShowStmtType` 选择 executor fetch 方法，再在会话侧应用 `LIKE`/`WHERE` 过滤并构造固定列名。
- `execute_show_columns`、`execute_show_create_table`、`execute_show_index`、`execute_show_table_status`：从同一份运行时元数据生成 MySQL 兼容展示；`append_show_create_region_split_policies` 追加 TiDB 特有的区域拆分注释。
- `execute_stats_system_select`、`mysql_stats_system_update_sql`、`mysql_stats_system_delete_sql`：只识别白名单统计系统表和有限表达式，将访问交给 Domain 的 restricted statistics backend。
- `execute_column_usage_insert`、`execute_analyze_job_insert`、`execute_analyze_jobs_select`：为 Go 中持久化系统表语义提供当前 Rust runtime 的插入/物化桥接。

## 执行流程

ANALYZE 主流程如下：

1. `dispatch.rs` 或 DDL/typed 入口调用 `execute_analyze`；该函数建立默认 `analyzeContext`，执行完后仅为非 restricted SQL 增加手工 ANALYZE 指标。
2. `execute_analyze_with_context` 解析显式选项与 `DEFAULT` 重置，调用 planner 的 `handleAnalyzeOptions`/`fillAnalyzeOptions`，并从独立 restricted `ConcreteSession` 读取 `mysql.analyze_options`。它根据持久化开关、分析版本、动态/静态分区模式，把表级与分区级历史选项按 Go 的覆盖次序合并。
3. 对每张目标表解析元数据：拒绝视图和序列；跳过被统计锁覆盖的表/分区并发出 warning；验证列选择，报告缺失依赖列；忽略 columnar index；读取会话数据行，形成 `SessionAnalyzeInput`。无剩余输入时成功返回。
4. `AnalyzeExec::RunCanonicalWithContext` 回调 `preflush_stats_delta`，在构建前刷新涉及的逻辑表和分区统计增量，并在前后检查 context 与 `SQLKiller`。
5. `prepare_batch` 检查近似内存用量，分配统计版本，按分区表达式拆分行。单 worker 顺序构建；多 worker 使用 scoped threads、原子任务下标和结果互斥量并行构建。选择性分析通过 `merge_previous_analyze_stats` 保留旧统计；动态分区裁剪还合并本次与未触及分区的持久化统计，校正全局 `realtime_count`/`modify_count`，最后按 `physical_id` 排序。
6. `save_batch` 先记录 `running` job，执行保存 failpoint/暂停点；`publish_batch` 再发布统计缓存和 finished jobs，写列使用时间、统计元数据、必要的未分析直方图和历史统计。只有 canonical 管线成功后，会话才写回 `mysql.analyze_options`；失败由 `record_failure` 写 failed job，因此不会把失败分析的选项当成已生效。

SHOW 主流程是：`execute_show_stats` 以 Domain context 构造 `SessionShowStatsRuntime`，由 `ShowExec` 遍历 schema、逻辑表和物理分区并格式化统计行，再应用 AST 中的 pattern/谓词。动态裁剪模式会同时暴露分区表的 `global` 统计；伪统计不作为已分析统计展示。普通 SHOW 元数据不走该 executor，而直接使用 `ConcreteSession` 的 metadata catalog。

系统表流程是：先用表白名单和有限 AST 形状判定是否接管；读请求由 `restricted_stats_query`（`mysql.tidb` 在已有事务中例外走 `restricted_system_sql_in_transaction`）执行，写请求转为受控 SQL；`mysql.analyze_jobs` 则从 stats handle 中的 runtime jobs 物化，不依赖真实表扫描。

## 数据与状态

- 全局测试状态 `ANALYZE_PAUSE_STATE` 和 `ANALYZE_SAVE_ERROR_STATE` 都是 `LazyLock<Mutex<...>>`。键是 `Arc<SQLKiller>` 的指针值，因此注入只影响注册的会话，而非所有 ANALYZE。
- `SessionAnalyzeRuntime.inputs` 是管线的只读工作集；`options_by_physical_id` 明确区分逻辑表和各分区的最终选项；`topn`/`buckets` 是缺省回退值。
- `active_workers` 与 `max_active_workers` 使用原子变量记录当前/峰值并发；`AnalyzeActiveWorker::drop` 保证线程任意返回路径都递减活跃计数。
- 统计实体由 `TableStats` 表示，关键状态包括 `version`、`last_analyze_version`、`realtime_count`、`modify_count`、列/索引直方图、TopN、bucket 和加载状态。选择性分析不得覆盖未选择对象；动态全局统计不得丢失未选择分区。
- 分析任务经历 `running`、`finished` 或 `failed`；动态分区的 global merge job 只在物理分析成功后生成（`SessionAnalyzeRuntime::jobs`）。
- 列使用记录保留原 `last_used_at`，只更新本次被分析列的 `last_analyzed_at`；虚拟生成列跳过，stored generated column 可记录。
- `ConcreteSession.state` 使用 `RefCell`；本文件在 restricted 查询或读取运行时开关时短暂借用，避免把可变借用跨越外部调用。

## 依赖与调用关系

上游调用边由代码搜索确认：

- `runtime/dispatch.rs` → `execute_analyze`、`execute_flush_stats_delta`、各 `execute_show_*`、系统统计 SELECT/INSERT。
- `runtime/control.rs` → `mysql_stats_system_update_sql` / `mysql_stats_system_delete_sql`。
- `runtime/ddl.rs`、`runtime/typed_analyze_executor.rs` → `execute_analyze`。

主要下游依赖是：

- `astersql-executor`：定义 `CanonicalAnalyzeRuntime`/`AnalyzeExec` 与 `ShowStatsRuntime`/`ShowExec`，负责管线次序和 SHOW 行构造算法。
- `astersql-statistics`：`RuntimeStatsBuilder` 和分区统计合并；`astersql-statistics-handle`：统计版本、缓存、持久化表示、锁表、任务和列使用。
- `astersql-meta-model`：`TableInfo`、列、索引、分区及 region split policy。
- `Domain`/`DomainStatsContext`（经 `super::*`）：目录、统计 handle、restricted backend、delta flush、持久化和历史统计的生命周期所有者。
- `astersql-planner-core::planbuilder`：ANALYZE 选项验证、补默认值和合并。
- `chrono`/`chrono-tz`：TSO 时间展示；`astersql-testkit-testfailpoint`：独立测试使用的失败点守卫。

`pkg/session/Cargo.toml` 明确声明上述 workspace path 依赖，并声明 `chrono = 0.4`、`chrono-tz = 0.10`；本文件没有自己的 feature gate，随 `astersql-session` 库编译。

## 错误处理与边界

- 对外统一返回 `SessionResult`；下游错误使用 `session_error("操作", error)` 增加阶段上下文，canonical executor 错误转换为 `AnalyzeError` 后再回到 session error。
- `SQLKiller`、调用方 `analyzeContext` 错误在 preflush 和每个构建任务前后检查；worker panic 映射为明确的 `analyze build worker panicked`。
- 内存配额使用输入字符串字节数加固定 collector 基线的估算，而非精确 allocator 统计；超限返回与 Go 回归场景兼容的错误文本，不发布统计。
- 锁污染被视为内部不变量破坏，多个 `Mutex::lock` 用 `expect` 终止；普通用户输入错误（未知表/列、视图/序列、无效选项、受限 SHOW 表达式）返回可诊断错误或 warning。
- 分区表达式的本地求值仅显式处理 Hash、Key 和 Range；其他类型退化为保留行。该逻辑服务当前 runtime 数据模型，扩展分区类型时不能假设已自动支持。
- stats system SQL 不是通用 SQL 引擎：UPDATE 仅支持 `stats_buckets`/`stats_histograms`、简单赋值和可选等值 WHERE；DELETE 仅对白名单表保留有限等值条件；SELECT 投影仅支持列、`count(*)`、`hex`、`truncate` 等明确形状。无法识别时应让外层分派或错误路径处理，不应静默扩张语义。
- `show_predicate` 只覆盖展示语句所需的布尔、比较、LIKE、IS NULL 等子集；`execute_show_stats` 当前对谓词求值错误按不匹配过滤，而不是向用户传播。
- `format_table_update_time` 对零 TSO、溢出或非法时间返回空值，避免伪造更新时间。

## 并发与资源生命周期

- 多 worker ANALYZE 使用 `std::thread::scope`，因此 worker 不会逃逸 `SessionAnalyzeRuntime`；所有 join 在 `prepare_batch` 返回前完成。`AtomicUsize` 分配唯一任务下标，`Mutex<Vec<_>>` 汇总结果，最终排序消除并发完成顺序的非确定性。
- `Barrier` 让 worker 同时进入构建阶段，便于稳定验证并发；构建暂停点使用每会话 `Arc<(Mutex<AnalyzePausePoint>, Condvar)>`。`AnalyzePauseGuard::drop` 必定设置 resumed、唤醒 waiter、移除 target，并在最后一个 target 离开时释放 failpoint guard，防止测试挂死或泄漏全局注入。
- `AnalyzeSaveErrorGuard` 支持同一 killer 的嵌套计数；最后一个 guard 析构后才移除 target 和全局 failpoint。
- 统计 handle 的锁仅包围版本分配、发布或任务/使用记录等临界区；持久化调用通常在锁外执行，降低锁持有时间。异常的 poisoned lock 被视为不可恢复的进程内一致性失败。
- 发布顺序具有事务语义：先构建完整 batch，再经过 save 门槛，随后更新缓存并持久化；失败 job 会记录，但失败 batch 不应成为可见统计。选项写回更晚，保证失败分析不改变后续默认选项。

## 与 Go 版本的对应关系

crate 元数据把本 crate 对应到 Go `pkg/session`，但此文件的行为跨越多个 Go 文件，而不是存在一个同路径 `pkg/session/runtime/statistics.go`：

- Go `pkg/executor/analyze.go` 的 `AnalyzeExec` worker、结果处理、global stats 合并、job info 与 analyze option 保存，对应 Rust 的 `execute_analyze_with_context` 和 `CanonicalAnalyzeRuntime` 实现。Rust 注释还明确引用 Go `prepareAnalyzeColumnsJobInfo`、`prepareIndexes`、`prepareColumns` 语义。
- Go `pkg/executor/show_stats.go` 的 `fetchShowStatsMeta/Histogram/Buckets/TopN/Healthy/Locked/AnalyzeStatus/ColumnStatsUsage`，对应 `SessionShowStatsRuntime` 和 `execute_show_stats`。两边都区分动态分区模式下的 global 行、分区行、伪统计和初始化/加载状态。
- Go 通过真实系统表与 Domain/StatsHandle 持久化 analyze jobs、options 和统计记录；当前 Rust runtime 部分通过 stats handle 内存状态及 restricted backend 桥接，`execute_analyze_jobs_select` 明确物化 Go `metadef.CreateAnalyzeJobsTable` 的 13 列布局。
- Go 的并发由 goroutine、channel、errgroup 和 wait group 管理；Rust 使用 scoped OS threads、原子任务下标、mutex 和 barrier，但保留取消、失败不发布、等待全部 worker、动态全局合并等核心不变量。
- Rust 的本地会话行读取、分区表达式求值、内存估算及受限系统 SQL 是当前移植层的具体实现，不能据此宣称覆盖 Go 完整存储执行、表达式或 SQL 能力。

## 扩展指南

- 扩展 ANALYZE 输入或选项时，优先修改 `execute_analyze_with_context`、`SessionAnalyzeInput` 和 `SessionAnalyzeRuntime::option_counts/prepare_batch`；必须同时核对 Go `pkg/executor/analyze.go` 的覆盖顺序、动态/静态分区差异、锁表 warning 与 option 持久化时机。
- 增加新统计实体时，在 `merge_previous_analyze_stats`、动态 global merge、`publish_batch`、SHOW 映射四处检查“未选择对象不得丢失”的不变量，并评估 cache/persist/history 三层是否一致。
- 新增 SHOW STATS 类型时，应先扩展 `astersql-executor::show_stats::ShowStatsRuntime`/`ShowExec`，再在 `execute_show_stats` 添加固定列布局；不要把业务遍历算法复制到 session 层。
- 扩展统计系统表 SQL 时，修改白名单、字面量引用和 AST 形状检查，并明确事务路径；避免把用户表达式字符串直接拼接到 restricted SQL。
- 新增并发或失败阶段时，应扩展独立测试文件 `pkg/session/runtime/statistics_test.rs`（或相邻专用 `*_test.rs`），不要把测试嵌入本生产文件；暂停/失败守卫必须保持 RAII 清理和会话隔离。
- 兼容风险集中在 Go 输出列顺序/大小写/NULL 表示、warning/error 文本、global/partition 行选择和任务状态时序；性能风险集中在全量行复制、分区重复扫描、worker 数量以及持锁期间的数据转换。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/session/runtime/statistics.rs` 确认目标已索引；`node --file ...` 读取了目标文件全部 3,570 行并报告其被 `runtime/ddl.rs`、`runtime/dispatch.rs` 等 15 个文件使用。对常见方法名的直接 `callers/callees` 查询没有返回可区分的 impl-method 边，因此调用边用精确源码搜索补齐，未据此虚构图关系。
- 已读生产与装配文件：`pkg/session/runtime/statistics.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/control.rs`、`pkg/session/runtime/ddl.rs`、`pkg/session/runtime/typed_analyze_executor.rs`、`pkg/session/Cargo.toml`。
- 已读 Go 对照：`pkg/executor/analyze.go`、`pkg/executor/show_stats.go`、`pkg/statistics/analyze.go` 的相关符号/路径；仓库不存在同路径 Go `pkg/session/runtime/statistics.go`。
- 已读独立 Rust 测试 `pkg/session/runtime/statistics_test.rs`：覆盖 lite 与完整直方图加载、Unicode `_` LIKE、无符号边界和单行统计、默认选项重置、动态分区 global TopN/bucket、全局索引估算、大数据与 53 分区重复一致性。
- 本任务为纯文档分析，按任务要求不运行 Cargo。最终结构检查要求文档存在且恰好包含本页 11 个固定二级标题。
