# `pkg/statistics/handle/usage/predicate_column.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-statistics-handle-usage`（`pkg/statistics/handle/usage/Cargo.toml`），由同目录 `lib.rs` 以 `pub mod predicate_column` 声明并整体再导出。它位于会话统计采集与持久化存储之间：接收 `SessionStatsList` 聚合出的表增量和谓词列最近使用时间，通过可注入的 `UsageStore`、`SchemaState` 决定何时以及如何写入存储。

需要注意当前接线状态：仓库内对 `StatsUsageImpl` 及其方法的直接 Rust 引用只出现在 `predicate_column_test.rs` 和测试导出代码中；`Cargo.toml` 中面向完整 TiDB 子系统的依赖全部位于恒假条件 `target.'cfg(any())'` 下。因而这里是具有真实内存聚合、阈值判断和失败回补逻辑的可测试实现，但尚无证据表明它已接入 Rust 生产运行主链。生产侧类似调度目前由 `pkg/domain/domain.rs`、`pkg/statistics/handle/handle.rs` 的另一套接口承担。

## 核心职责

- 以 `StatsUsageImpl` 组合持久层、会话统计列表、schema 视图和索引使用率收集器，并设置 Go 对齐的默认刷盘比例与最长等待时间。
- 通过 `load_column_stats_usage`、`get_predicate_columns` 提供列使用时间和谓词列 ID 的只读查询门面。
- 通过 `new_session_stats_item` 为会话注册独立收集句柄；真正的会话局部累计和全局合并位于 `session_stats_collect.rs`。
- 通过 `dump_stats_delta_to_kv` 按目标集合、表存在性、强制标志、增量年龄和修改比例筛选表增量，并保证未写入或写入失败的增量重新回到全局 pending map。
- 通过 `dump_column_stats_usage_to_kv` 批量、确定性地保存谓词列最近使用时间；空集合不写存储，失败批次重新合并，避免静默丢失。

## 主要符号

- `DUMP_STATS_DELTA_RATIO: f64 = 1 / 10_000`：`TableDelta.count / stats_meta_count` 的默认刷盘下界，严格使用 `>` 比较。
- `DUMP_STATS_MAX_DURATION: Duration = 1h`：增量从 `init_time` 起超过此年龄即允许刷盘，同样是严格 `>`。
- `Error(String)`：本 crate 的轻量错误值；实现 `Clone + Eq` 便于失败注入和断言，但不保留结构化错误链。
- `ColumnTimeInfo`：同时表达列最近作为谓词使用的时间和最近 ANALYZE 时间；两个字段均可缺失。
- `UsageStore`：持久层端口，覆盖 stats meta 行数读取、delta 更新、列使用时间保存/加载、谓词列查询及索引使用率 GC。`StatsUsageImpl` 本文件只调用前五项中的相关方法；`gc_index_usage` 在本文件没有调用点。
- `SchemaState`：只读 schema 端口，提供 `table_exists` 和 `table_locked`。
- `StatsUsageImpl`：核心组合对象。`store`、`sessions`、`schema` 使用 `Arc` 共享；`dump_ratio`、`max_delta_age` 可由调用者调整；`index_usage` 在本文件仅构造和持有。
- `StatsUsageImpl::new`：构造默认 `SessionStatsList` 和 `IndexUsageCollector`，注入 store/schema，并应用两个默认阈值。
- `load_column_stats_usage`、`get_predicate_columns`：不做缓存或转换，直接转发到 `UsageStore`。
- `new_session_stats_item`：转发到 `SessionStatsList::new_item`，返回可克隆的会话句柄。
- `dump_stats_delta_to_kv`、`dump_column_stats_usage_to_kv`：两个有状态刷盘入口。

文件中没有宏、泛型定义、条件编译项或后台任务定义。

## 执行流程

`dump_stats_delta_to_kv(force, targets)` 的流程如下：

1. 调用 `sessions.sweep()`，把各会话局部 delta/usage 合并进全局容器。
2. 用 `table_delta().take()` 原子式取空当前全局 delta map；`collect_pending_stats_delta_table_ids` 根据 `targets` 选出待处理 ID。空 `targets` 表示处理全部 pending，非空集合只处理存在的目标，并由该辅助函数去重、排序。
3. 对每个 ID 从局部 `pending` 移除 delta。若 schema 中表不存在，则重新插入并跳过，保留给以后重试。
4. 缺失 `init_time` 时补为当前时间；读取 `stats_meta_count`。读取失败会先放回当前 delta，再提前返回错误。
5. 当 stats meta 缺失、行数不大于零，或 `delta.count / count > dump_ratio` 时比例条件成立；当 `now - init_time > max_delta_age` 时年龄条件成立。`force || old || ratio` 任一成立即调用 `update_delta`，同时传入 `table_locked(id)`。
6. 不满足条件的 delta 放回；写入失败的 delta 也先放回再返回错误。闭包结束后无论成功失败，都用 `merge(pending)` 把剩余内容并回全局容器，再返回原结果。

`dump_column_stats_usage_to_kv()` 先 `sweep()`，再 `take()` 全局 usage。空 map 直接成功；非空 map 转成 `ColStatsUsageEntry`，按 `(table_id, item.id)` 排序后一次调用 `save_column_usage`。失败时把整批条目还原为 map 并通过 `StatsUsage::merge` 回补；成功则由本次 take 消费掉该批次。

查询路径更短：`load_column_stats_usage` 与 `get_predicate_columns` 直接调用存储端口；会话注册则由 `new_session_stats_item -> SessionStatsList::new_item` 完成。

## 数据与状态

`StatsUsageImpl` 自身不持有事务或连接。可变运行状态主要在 `SessionStatsList` 内：每个 `SessionStatsItem` 有受 `Mutex` 保护的会话局部 delta/usage，全局 `TableDeltaMap` 与 `StatsUsage` 也各自用 `Mutex<HashMap<...>>` 保存聚合结果。`sweep` 会取走会话局部值并合并到全局；本文件的两个 dump 再对全局 map 执行 take/merge。

`TableDelta.count` 是比例判断的分子，而不是表示净行数变化的 `TableDelta.delta`；`predicate_column_test.rs::canonical_delta_dump_uses_modify_count_for_ratio` 专门固定这一不变量。`TableItemId` 同时带 `table_id`、列/索引 ID 与 `is_index`；本文件保存时不额外过滤，数据合法性由会话采集入口负责。重复使用时间在 `StatsUsage::merge` 中取较晚值，因此失败回补不会让旧时间覆盖并发到达的新时间。

`targets` 只限制本次尝试处理的 ID，不会丢弃其他 pending 项。排序提供确定性的表处理顺序和列批次顺序；本实现没有 Go 版的分批大小限制。

## 依赖与调用关系

直接内部依赖来自 crate 根再导出的 `ColStatsUsageEntry`、`SessionStatsItem`、`SessionStatsList`、`TableDelta`、`TableItemId`、`IndexUsageCollector` 和 `collect_pending_stats_delta_table_ids`。其中会话聚合类型定义在 `session_stats_collect.rs`，目标 ID 收集辅助函数定义在 `index_usage.rs`。标准库依赖仅为 `HashMap`、`Arc`、`Duration`、`SystemTime`。

上游方面，RustCodeGraph 为 `StatsUsageImpl` 和四个方法建立了符号节点，但精确 caller 查询未返回生产调用者；仓库级文本引用复核也只发现 `predicate_column_test.rs` 对 `new_session_stats_item`、两个 dump 方法的调用。`lib.rs` 会公开再导出这些 API，表示其他 crate 理论上可依赖它们，但“可见”不等于“已接线”。

下游调用边可由方法体直接确认：

- `new -> SessionStatsList::default + IndexUsageCollector::default`；
- `new_session_stats_item -> SessionStatsList::new_item`；
- 两个查询入口分别到 `UsageStore::load_column_usage`、`UsageStore::predicate_columns`；
- delta 刷盘到 `SessionStatsList::sweep`、`TableDeltaMap::{take,merge}`、`collect_pending_stats_delta_table_ids`、`SchemaState::{table_exists,table_locked}` 与 `UsageStore::{stats_meta_count,update_delta}`；
- 列使用率刷盘到 `SessionStatsList::sweep`、`StatsUsage::{take,merge}` 与 `UsageStore::save_column_usage`。

## 错误处理与边界

所有可恢复的持久层错误以 `Result<_, Error>` 原样传播。delta 路径在 `stats_meta_count` 或 `update_delta` 失败时保留当前项，并在函数末尾把尚未处理的其他项一并合回；列 usage 保存失败时回补整个批次。由此保证失败不造成内存队列中的已知数据丢失，但没有重试、退避、日志或错误分类，重试必须由上层再次调用 dump。

边界行为包括：空列 usage 不触发 store 写入；目标 ID 不在 pending 中时忽略；不存在的表不删除 delta；stats meta 为 `None`、零或负数时立即刷盘；`SystemTime::duration_since` 因时钟回拨失败时年龄条件视为 false；比例和年龄恰好等于阈值时不刷盘。`delta.count as f64` 可能为负，本文件不校验其业务合法性。

`SessionStatsList` 内的互斥锁使用 `expect`，锁中毒会 panic；`StatsUsageImpl` 不捕获 panic。trait 实现也可能在 `table_exists`、`table_locked` 中自行阻塞或 panic，本接口没有超时约束。

## 并发与资源生命周期

`UsageStore`、`SchemaState` 要求 `Send + Sync`，并由 `Arc<dyn ...>` 共享；`SessionStatsList` 也由 `Arc` 持有。因此 `StatsUsageImpl` 可共享访问其组件，但 dump 方法没有覆盖整个“sweep → take → store → merge”的全局互斥锁。

take/merge 模式使持久层 I/O 期间不占用全局 map 的 mutex，其他会话仍可汇入新数据。失败回补时，`TableDeltaMap::merge` 累加 delta/count 并保留更早 `init_time`，`StatsUsage::merge` 保留更晚时间戳，因此不会简单覆盖并发写入。不过若多个线程同时调用同一 dump 方法，它们可能分别取走不同批次并并行访问 store；是否允许并行事务、是否保持跨批次全局顺序由 store 实现和上层调度保证，本文件没有 single-flight 约束。

`SessionStatsItem` 的生命周期由 `SessionStatsList` 管理；会话调用 `delete` 后，要等后续 `sweep` 合并最后数据并从列表移除。`StatsUsageImpl` 不创建线程、定时器、通道或异步任务，也不拥有需要显式关闭的资源。

## 与 Go 版本的对应关系

Go 对照不是单一文件：`pkg/statistics/handle/usage/predicate_column.go` 定义 `statsUsageImpl`、构造函数及列 usage/谓词列查询门面；`session_stats_collect.go` 定义 delta 和列 usage 的 sweep、筛选与持久化。

已对齐的核心语义包括：默认比例 `1/10000`、最长一小时、按修改次数 `Count` 判断比例、缺失/空 stats 触发刷盘、空目标处理全部 pending、确定性排序、会话 sweep，以及写失败后保留待处理数据。Rust 测试分别覆盖低比例保持 pending、force 刷盘、缺 stats 刷盘、不存在表保留、使用 `count` 而非 `delta`、空 usage 跳过写入和失败回补。

Rust 当前是明显缩小后的端口化实现，不能等同于 Go 生产能力。Go 版还通过 session pool/事务执行 SQL，过滤内存库和系统库，按十万表分批，处理分区/全局统计与统计锁，记录历史 stats meta、指标和慢操作日志，并对 panic/failpoint 等做专门处理；列 usage 写入也包含节流和批量 SQL 细节。这些在本文件中由抽象 store 接口代替或完全未实现。此外，Go 的查询方法用 `predicatecolumn` 子包执行真实 SQL；Rust 这里只转发给 `UsageStore`。扩展或接线时必须重新评估这些差异，而不能仅凭现有单元测试宣称完整移植。

## 扩展指南

新增持久层时应实现 `UsageStore`，并把事务、SQL、批大小、锁和重试语义放在实现层或显式扩展接口；新增 schema 来源则实现 `SchemaState`。任何生产接线都应先确定唯一调度者，避免并发 dump 导致 store 侧顺序或事务假设失效。

调整 delta 策略时优先修改 `dump_stats_delta_to_kv` 及两个阈值字段，并同步 `predicate_column_test.rs` 的比例、年龄、目标过滤、表不存在、锁状态和错误回补用例。若要追平 Go，应逐项移植而非把复杂行为隐含在 `stats_meta_count`：至少需要覆盖系统/内存表过滤、批处理、分区与全局表更新、历史 meta 和可观测性。

调整列 usage 时应保持“空批次不写、稳定排序、失败不丢、同列取最新时间”的不变量，并在独立测试文件 `pkg/statistics/handle/usage/predicate_column_test.rs` 添加回归测试；不要把测试嵌入生产源文件。若 `ColumnTimeInfo` 或 `TableItemId` 的语义变化，还需同步 store 实现、`session_stats_collect.rs` 和 `predicatecolumn/` 子包的 SQL 映射。

性能风险集中在单次把全部 pending map 取出并排序、逐表调用 `stats_meta_count/update_delta` 以及一次性保存全部列条目。兼容风险集中在阈值的严格比较、`None/0` stats 的即时刷盘、锁标志传递和失败回补合并规则。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/statistics/handle/usage` 确认目标、同目录测试和 Go 对照均在图中；`node --file pkg/statistics/handle/usage/predicate_column.rs` 读取完整 176 行源码；`query` 确认 `StatsUsageImpl`、`dump_stats_delta_to_kv`、`dump_column_stats_usage_to_kv`、`get_predicate_columns`、`new_session_stats_item` 的符号节点。精确 caller/callee 子命令对内部函数 ID 返回了歧义结果，因此没有把该结果当作调用边证据，改以源码及引用搜索核验。
- Rust 源与模块边界：`pkg/statistics/handle/usage/predicate_column.rs`、`session_stats_collect.rs`、`index_usage.rs`、`lib.rs`。
- Cargo 边界：`pkg/statistics/handle/usage/Cargo.toml`；包名与 `lib.rs` 入口已核对，完整子系统依赖位于 `cfg(any())`。
- Go 对照：`pkg/statistics/handle/usage/predicate_column.go`、`session_stats_collect.go`；真实 SQL 子层位于 `predicatecolumn/predicate_column.go`。
- 独立测试：`pkg/statistics/handle/usage/predicate_column_test.rs`；同时参考 Go 行为测试 `predicate_column_test.go` 和 `session_stats_collect_test.go` 的覆盖范围，但未把 Go 集成测试通过状态作为本次证据。
- 仓库引用搜索确认当前 Rust 直接调用集中在上述独立测试；生产主链中同名 dump 调用落到 `domain.rs`/`handle.rs` 的另一实现。
- 本任务为纯文档分析，按计划不运行 Cargo；最终以固定 11 章节结构命令和人工事实复核验收。
