# `pkg/session/runtime/mlog_purge.rs`

## 文件定位

本文件属于 `astersql-session` crate（见 `pkg/session/Cargo.toml`），由 `pkg/session/runtime.rs` 以私有模块 `mlog_purge` 装配。它位于物化视图日志（Materialized View Log，MLog）的消费端：`runtime/mlog.rs` 在基础表 DML 时写入日志，本文件依据所有相关物化视图已经安全读取到的 TSO 清除旧日志，并维护 `mysql.tidb_mlog_purge_info` 与 `mysql.tidb_mlog_purge_hist`。

入口有两条。用户执行 `PURGE MATERIALIZED VIEW LOG` 或 `CANCEL MATERIALIZED VIEW ...` 后，`ConcreteSession::execute_statement` 在 `runtime/dispatch.rs:2050-2062` 分派到本文件的方法；后台入口由 `runtime/session.rs:1085` 在会话工厂初始化时调用 `start_domain_mlog_purge_worker`，周期性执行到期任务。`runtime.rs:67-68` 另外在 crate 内重导出底层批删除函数和单次 tick，供 DML/runtime 测试及局部接线使用。

本文件没有 feature gate。文件尾通过 `#[path = "mlog_purge_test.rs"] mod tests` 把测试保持在独立文件，符合生产逻辑与测试分离的仓库约定。`pkg/session` 下不存在可供本任务读取的 `doc.go`。

## 核心职责

- 建立清理安全栅栏：`execute_purge_materialized_view_log` 以 purge 事务的 `StartTS` 为上界，再取已公开和正在构建的依赖物化视图的 `LAST_SUCCESS_READ_TSO` 最小值，得到 `safe_tso`。只有提交时间位于 `(LAST_PURGED_TSO, safe_tso]` 的日志记录可以删除。
- 串行化同一日志的清理：对 `tidb_mlog_purge_info` 的目标行执行 `FOR UPDATE NOWAIT`，避免两个清理任务同时推进相同 MLog checkpoint。
- 分批、可节流地删除：先尽力统计候选行与整数 row ID 边界，再按至多 16 段的线性/分片范围扫描；每批使用独立 KV 事务提交，并根据下一调度时刻、最低速率和预算比例控制批量与睡眠。
- 维护可观察状态：写入 running/success/failed 历史、累计删除行数、更新心跳、响应取消请求、推进 `LAST_PURGED_TSO`，自动任务还更新 `NEXT_PURGE_UNIX_SECONDS`。
- 管理后台调度：`run_mlog_purge_tick` 查找到期 MLog，恢复基础表与 schema 后复用同一清理主流程；`start_domain_mlog_purge_worker` 把 tick 注册为 Domain 拥有的周期工作线程。

文件不负责生成 MLog、创建/删除 MLog 元数据表或刷新物化视图；这些分别位于 `runtime/mlog.rs`、MLog DDL 路径和物化视图刷新路径。

## 主要符号

- `DEFAULT_MLOG_PURGE_BATCH_SIZE = 10_000`：会话变量不存在、解析失败或非正时的批大小回退值。
- `MLOG_PURGE_ADAPTIVE_MAX_BUDGET`、`MLOG_PURGE_ADAPTIVE_BATCH_WINDOW`、`MLOG_PURGE_ADAPTIVE_MIN_BATCH_SIZE`：分别限定最长节流预算为 590 秒、目标批窗口为 200 毫秒、适应性批量下限为 8,000 行。
- `MLogPurgeThrottlePlan { target_rate, pending_rows, min_rate, deadline, no_wait_streak }`：一次清理的内存态速率计划。`new` 拒绝零行、已过期/零预算、非正 ratio 或 min rate；`batch_size` 取配置上限与 200ms 目标量的较小值；`sleep_duration` 计算累计进度所需等待，并在连续两次无需等待后按剩余行和剩余预算重估速率。
- `MLogPendingRowStats` 与 `MLogRowIDRange`：候选行数及可选整数句柄边界、闭区间扫描范围。`mlog_purge_row_id_ranges` 根据 `pending_rows / 8000` 生成 1 至 16 段；有 `ShardRowIDBits` 时对齐分片桶的 2 次幂分组，否则在线性 span 上均分。
- `pending_mlog_rows(snapshot, log_id, last_purged_tso, safe_purge_tso)`：在 30 秒上限内扫描 MLog record prefix，统计 TSO 合格行；只有所有候选记录都是整数 handle 时才保留 row ID 边界。
- `purge_mlog_snapshot_batch`：crate 内可见的无游标批删除门面，返回删除数。实际实现 `purge_mlog_snapshot_batch_from` 还接收上一 key 与可选 row ID 范围，并返回 `(deleted, last_seen, exhausted)`。
- `purge_sql_row` / `purge_sql_rows`：通过 `ConcreteSession::execute` 消费内部 SQL 结果，并显式关闭 record set。前者最多返回首行，后者排空所有行。
- `purge_sql_string`、`purge_history_time`、`purge_history_duration`：分别处理动态 SQL 字符串、UTC 微秒时间和秒.微秒耗时格式。
- `resolve_mlog_database`：显式 schema 为空时使用当前库；两者都为空则返回 `No database selected`。
- `finalize_purge_history`：历史终态更新失败时原样重试一次，两次失败才返回合并错误。
- `derive_mlog_next_purge_seconds`：把 MLog 元数据中的 `PurgeNext` 包装成 `select <expr>` 解析，并按保存的 SQL mode 调用 `mview_ddl::mlog_schedule_unix_seconds_with_mode` 求下一 UTC 秒；空表达式返回 `None`。
- `ConcreteSession::check_mlog_operate_privilege`：对已登录用户校验目标日志表上的 `OPERATE VIEW`；没有登录身份的内部会话直接通过。
- `ConcreteSession::execute_purge_materialized_view_log`：手工与自动清理共用的核心编排器，`automatic` 决定历史 method 和是否重算下一调度时间。
- `ConcreteSession::execute_cancel_materialized_view_job`：校验 job 类型、状态、目标和权限，然后以条件 UPDATE 写取消时间与请求者。
- `run_mlog_purge_tick(domain, now)`：执行一次到期任务扫描，返回成功执行的任务数。
- `start_domain_mlog_purge_worker(domain)`：以 10 秒间隔向 Domain 注册闭包；返回值沿用 Domain 的“是否本次真正启动”语义。

## 执行流程

手工/自动清理的主流程如下：

1. `execute_purge_materialized_view_log` 拒绝显式事务，解析目标数据库和基础表，按 `MaterializedViewLogTableName` 找到日志表，校验 `BaseTableID` 反向关系与 `OPERATE VIEW` 权限。
2. 创建彼此独立的 `maintenance` 与 `history` 会话。前者持有 purge-info 悲观事务及 checkpoint 更新，后者让历史和取消状态不依赖该事务提交。
3. `maintenance` 执行 `begin pessimistic`，对目标 purge-info 行 `FOR UPDATE NOWAIT`；解析可选 `LAST_PURGED_TSO`、下一执行时间，并以当前事务 `StartTS` 作为 job ID 和初始 `safe_tso`。
4. 查询正在创建的物化视图 DDL job，并联合基础表元数据中的公开 MView ID。对每个相关视图读取 `tidb_mview_refresh_info.LAST_SUCCESS_READ_TSO`，不断缩小 `safe_tso`；公开视图缺少刷新行是硬错误，构建中视图缺行则跳过。
5. 读取该 MLog 最新非空历史 cutoff，计算 `max(last_purged_tso, latest_cutoff)`。若新 `safe_tso` 小于此栅栏，整个任务不新增历史、不删数据、不回退 checkpoint；否则先插入 running 历史。
6. 当 `safe_tso > 0` 且严格前进时，通过当前存储快照调用 `pending_mlog_rows`。统计失败只写 warning 并回退到无范围、无节流的全前缀批扫描，不中止清理。
7. 若统计成功，`mlog_purge_row_id_ranges` 生成扫描范围，`MLogPurgeThrottlePlan::new` 根据下次调度（自动任务重新计算，手工任务使用锁定的时间）、最低速率和预算比例建立计划；到期或无有效计划时不节流。
8. 每个范围从空 cursor 开始循环。每轮先检查取消并刷新心跳，再开启独立 KV 事务，调用 `purge_mlog_snapshot_batch_from`，提交后累计 `purged` 并通过 `Domain::record_stats_mutation` 扣减行数、增加 modify count。
9. 需要节流时按最多 200ms 的小段睡眠，每段后重新检查取消；范围耗尽后进入下一范围。全部完成后在 maintenance 事务更新 `LAST_PURGED_TSO`；自动任务同时计算并更新下一执行秒，最后提交。
10. 任一步失败都尝试回滚 maintenance 事务。若 running 历史已建立，将其终结为 failed；若失败发生更早但已取得 job ID，则插入 failed 历史。成功提交后将 running 历史终结为 success；此终结两次都失败仅向原会话追加 warning，因为数据和 checkpoint 已提交。

底层批处理 `purge_mlog_snapshot_batch_from` 先验证 `batch_size > 0`；`safe_purge_tso == 0` 直接视为耗尽。它把可选整数 handle 区间编码为 KV 边界，从 cursor 的下一 key 继续，读取每条记录的 MVCC `CommitTs`，收集至多 batch size 个满足栅栏的 key，关闭 iterator 后再调用事务 `Delete`。cursor 使用“最后检查过的 key”而非“最后删除的 key”，因此不会在不合格记录上反复扫描。

取消流程先要求 `LogPurge` 类型和正 job ID。外部登录用户还要从 running 历史解析 MLog、在 InfoSchema 反查 schema，并通过 `OPERATE VIEW`。随后再次确认状态为 running 且尚无取消时间，最后用带同样条件的 UPDATE 登记请求；实际 purge 循环在批次前后及节流睡眠期间消费该请求。

自动流程由 `run_mlog_purge_tick` 查询 `NEXT_PURGE_UNIX_SECONDS <= now` 的 ID，跳过已从 InfoSchema 消失、已不是 MLog 或基础表已消失的记录；可恢复的条目被构造成 `PurgeMaterializedViewLogStmt` 并以 `automatic = true` 调用主流程。任何实际执行错误会终止本次 tick 并由 worker 记录错误。

## 数据与状态

- 持久状态以 `tidb_mlog_purge_info` 为每个 MLog 的调度/checkpoint 真源，以 `tidb_mlog_purge_hist` 记录 job 生命周期。`PURGE_JOB_ID` 取 purge 悲观事务的 StartTS，因而同时是非零、随事务分配的任务标识。
- `safe_tso` 是删除上界；`last_purged_tso` 是已完成区间的下界。实际过滤条件为 `commit_ts <= safe_tso && (last_purged_tso 不存在或 commit_ts > last_purged_tso)`。
- 历史 cutoff 是防倒退的第二道持久栅栏。即使 purge-info checkpoint 缺失或较旧，也不会在存在更大历史 cutoff 时建立一个更旧任务。
- `purged` 只在一个批事务成功提交后累加；统计更新紧随提交。若后续批次失败，先前批次不会回滚，但 checkpoint 不推进，下一次仍能安全重扫已删除区间。
- `cursor`、range index、throttle plan、取消轮询结果和当前耗时只存在于一次函数调用内，不持久化；崩溃恢复依赖 MVCC 过滤、checkpoint 和历史栅栏，而不是内存游标。
- 非整数 handle 会令 `row_id_bounds = None`，从而使用完整 record prefix 扫描；这保留正确性，只失去范围并行/定位能力。当前代码仍是逐范围串行处理，不会并行执行这些范围。

## 依赖与调用关系

RustCodeGraph 将本文件标为被 `pkg/ddl/job_worker.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/system_session.rs`、`pkg/session/tests/system_session.rs` 使用；精确静态边还确认：

- `runtime/dispatch.rs::ConcreteSession::execute_statement` → `execute_purge_materialized_view_log(..., false)` / `execute_cancel_materialized_view_job`。
- `runtime/session.rs::SessionFactory` → `start_domain_mlog_purge_worker`；`pkg/domain/domain.rs::start_mlog_purge_worker` 保证每个 Domain 只启动一次名为 `mlog-purge-worker` 的线程。
- `start_domain_mlog_purge_worker` → `run_mlog_purge_tick` → `execute_purge_materialized_view_log(..., true)`。
- `execute_purge_materialized_view_log` → SQL helpers、`pending_mlog_rows`、`mlog_purge_row_id_ranges`、`MLogPurgeThrottlePlan`、`purge_mlog_snapshot_batch_from`、`Domain::record_stats_mutation`。
- `purge_mlog_snapshot_batch` → `purge_mlog_snapshot_batch_from`；`pkg/session/dml_runtime_test.rs` 直接调用前者验证 MVCC fence。

crate 依赖由 `pkg/session/Cargo.toml` 明确声明：KV/表 key 来自 `astersql-kv` 与 `astersql-tablecodec`，表/MLog/DDL job 元数据来自 `astersql-meta-model`，语法与 AST 来自 `astersql-parser`/`astersql-parser-ast`，schema/权限/统计分别来自 `astersql-infoschema`、`astersql-privilege-privileges`、`astersql-statistics-handle`，系统变量来自 `astersql-sessionctx-vardef`，UTC 时间格式与调度比较使用 `chrono`。本文件通过 `use super::*` 取得这些别名和 `ConcreteSession`、`Domain`、错误及日志适配器。

## 错误处理与边界

- 输入边界：无数据库、基础表/MLog 不存在、MLog 反向元数据不匹配、显式事务、权限不足、非法取消类型/job ID 均在产生删除前失败。
- 锁与 TSO 边界：purge-info 行不存在、NOWAIT 锁冲突、StartTS 为零、checkpoint/刷新 TSO/历史 cutoff 无法解析都会停止任务；`safe_tso == 0` 不删除。
- KV 边界：批大小为零是错误；iterator 创建、Get、Next、row key 解码、Delete、Commit 均带操作上下文映射为 `SessionError`。iterator 无论扫描成功或失败都会调用 `Close`。
- 记录若没有可用 `CommitTs`（值为 0）会报错，而不是假定其可删除。所有删除必须同时满足上、下 TSO 栅栏。
- 统计扫描的 30 秒超时及其他统计错误属于 best effort：只禁用自适应计划，不影响正确的无节流删除。相反，真实批删除、统计 mutation 记录、checkpoint 或 maintenance commit 失败属于任务失败。
- 历史终结最多尝试两次。失败任务若连 failed 历史也无法落盘，会返回该终结错误；成功数据提交后的 success 历史失败只产生 warning，避免把已经提交的 purge 伪装成可回滚失败。
- 动态 SQL中的 schema、表名、错误和请求者通过 `purge_sql_string` 转义；数字来自已解析整数。当前 helper 是手工构造 SQL，扩展字段时必须继续区分字符串和数字，不能直接插入未经转义文本。
- `run_mlog_purge_tick` 对无效 ID 字符串返回错误，对元数据已消失的到期项静默跳过；一个任务错误会阻止本次 tick 继续处理后续行。

## 并发与资源生命周期

同一 Domain 的 worker 由 `Domain::start_mlog_purge_worker` 中的原子 `mlog_purge_worker_started.swap` 去重。Domain 持有 stop flag 和 join handle；线程每轮执行 tick 后 `park_timeout(10s)`，Domain 关闭/stop flag 被设置后退出。闭包只持有 `Weak<Domain>`，不会因 worker 自身延长 Domain 生命周期。

同一 MLog 的业务并发由 purge-info 行的悲观 NOWAIT 锁串行化；不同 MLog 可以由其他调用会话各自运行。主 maintenance 事务从加锁持续到 checkpoint/next schedule 提交，但数据删除使用一批一个短 KV 事务，降低单事务体积。history 会话独立，使 running、heartbeat、cancel 和终态对其他会话可见。

当前实现的范围是串行的，后台 tick 也是逐行串行的；`std::thread::sleep` 会阻塞当前 purge 执行线程。睡眠拆为最多 200ms 片段，以缩短取消响应时间。取消是协作式的：请求方只写历史字段，执行方在批次前、批次后和每个睡眠片段后检查；正在执行的单个 KV 批次不会被强制中断。

结果集由 SQL helper 显式 `close`，KV iterator 也显式 `Close`。批事务只有成功执行 `Commit` 后才累计删除数；错误时 transaction 对象离开作用域，外层 maintenance 事务还会显式 rollback。

## 与 Go 版本的对应关系

主要对照是 `pkg/executor/mlog_purge.go`，其核心由提交 `21f9ba4ee2 executor, planner: support materialized view log purge (#71118)` 引入；`pkg/session/session.go` 仅包含语句校验接线，不是清理实现。Rust 对应关系如下：

- Go `PurgeMaterializedViewLogExec.executePurgeMaterializedViewLog` 对应 Rust `ConcreteSession::execute_purge_materialized_view_log`；两者都校验权限、使用 purge-info NOWAIT 锁、以 StartTS 建 job ID、计算依赖视图最小 TSO、使用历史 cutoff、防 checkpoint 倒退、分批删除并写 success/failed 历史。
- Go `mlogPurgeThrottlePlan` / pending stats / row-ID range 与 Rust 同名语义结构对应；常量 30 秒、200ms、8,000、16 段和 590 秒预算一致。`mlog_purge_test.rs` 的测试名带 `go_merge_49`，直接验证这批移植的速率预算、配置批量上限及线性/分片范围。
- Go 通过 TiFlash/SQL 统计和带 `_tidb_commit_ts`、`_tidb_rowid` 条件的 DELETE 执行；Rust 直接在 KV snapshot 上读取 commit ts 并删除编码 row key。因此 Rust 保留相同 fence 与范围意图，但执行层、计划选择和错误形态并不逐行等同。
- Go 用 `startMVTaskMonitor` 独立监测取消/心跳，并能利用 context 取消睡眠/执行；Rust 在清理循环和分段睡眠中同步轮询历史表。Rust 当前没有 Go 的 maintenance session memory quota、isolation read engine、TiFlash thread 配置应用/恢复，也没有相同 failpoint 集合。
- Go 在成功后设置 affected rows 和 SQL message；当前 Rust 编排器只维护本地 `purged`、统计元数据与历史，没有在此文件写回语句 affected rows/message。扩展或兼容性评估不得假定这些用户可见细节已完全对齐。
- Go 的 checkpoint UPDATE 带单调条件，Rust 依靠持有 purge-info 行锁及 cutoff fence 后直接赋值；在现有锁生命周期内目标相同，但若未来缩短锁范围，必须重新加入存储层单调保护。

Go 测试 `pkg/executor/test/materializedviewlog/materialized_view_purge_test.go` 提供更广契约，包括无库、权限、显式事务、NOWAIT 冲突、批删除、范围条件、checkpoint/cutoff 短路、部分失败、取消/心跳、历史重试、统计行数和自动调度。它们是移植语义参考，不等于这些分支都已由 Rust 测试直接覆盖。

## 扩展指南

- 修改安全范围时，首先审查 `execute_purge_materialized_view_log` 中 public/building MView 收集、`safe_tso`、历史 cutoff 和 checkpoint 四者的不变量；同步扩展 `pkg/session/dml_runtime_test.rs`，至少覆盖旧记录删除、新记录保留、checkpoint 不倒退和中途失败可重试。
- 修改扫描/批删除时，集中在 `pending_mlog_rows`、`mlog_purge_row_id_ranges`、`purge_mlog_snapshot_batch_from`。必须保持 cursor 前进、闭区间编码、非整数 handle 回退、iterator 关闭和“提交后才计数”；纯算法单元测试放在独立的 `runtime/mlog_purge_test.rs`。
- 修改节流策略时，保持无效配置/统计失败回退为正确但不节流的路径，并测试 deadline、最小速率、配置 batch 上限、连续 no-wait 重估与取消延迟。性能风险主要是全表 snapshot 扫描、过小批量的事务开销和阻塞式 sleep。
- 修改历史或取消协议时，同时核对 `tidb_mlog_purge_hist` schema、`finalize_purge_history`、三个取消检查位置及权限目标解析。兼容性风险是 running 行永久残留、已提交任务被错误标 failed、或请求者绕过 `OPERATE VIEW`。
- 修改后台调度时，保留 Domain 单例 worker、Weak 生命周期和关闭检测；若要并行处理到期 MLog，需要明确并发上限、逐任务错误隔离以及同一 MLog 的 NOWAIT 行锁行为。
- 追求 Go 完整兼容时，应逐项对照 `pkg/executor/mlog_purge.go` 及其测试，而不是简化为“最终日志被删掉”。特别关注 maintenance session vars、context 取消、affected rows/message、历史字段条件和 Go 的内部/外部 SQL差异。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；本文件被识别为 73 个符号。
- RustCodeGraph 源码读取：`node --file pkg/session/runtime/mlog_purge.rs` 覆盖 1-992 行；`node --file pkg/session/runtime/mlog_purge_test.rs` 覆盖 1-53 行。
- RustCodeGraph 精确查询：`query` 定位 `purge_mlog_snapshot_batch`、`execute_purge_materialized_view_log`、`execute_cancel_materialized_view_job`、`run_mlog_purge_tick`、`start_domain_mlog_purge_worker`、`pending_mlog_rows`、`mlog_purge_row_id_ranges`；`callees` 验证 tick 调用 purge 主流程、worker 调用 tick、批删除门面调用游标实现，以及主流程对 SQL helper、权限检查和批删除的依赖。
- RustCodeGraph `node start_mlog_purge_worker`：核对 `pkg/domain/domain.rs:5757-5795` 的单例原子标志、命名线程、stop flag、park interval 和 worker handle 生命周期。
- 已读 Rust/Cargo 路径：`pkg/session/runtime.rs`、`pkg/session/runtime/dispatch.rs:2035-2063`、`pkg/session/runtime/session.rs:1060-1091`、`pkg/session/Cargo.toml`、`pkg/session/runtime/mlog_purge_test.rs`、`pkg/session/dml_runtime_test.rs`。
- 已读 Go 对照/测试：`pkg/executor/mlog_purge.go`、`pkg/executor/test/materializedviewlog/materialized_view_purge_test.go`；并以 `rg` 确认 parser/planner/executor 构建链和系统表引用。Go 测试明确覆盖 NOWAIT 冲突、批删除、row ID 范围、checkpoint/cutoff、失败历史、节流回退、统计更新和自动 next time。
- Rust 独立测试证据：`go_merge_49_adaptive_purge_respects_rate_budget_and_batch_limit`、`go_merge_49_adaptive_purge_splits_linear_and_sharded_row_ids`；集成式 runtime 测试证据包括 `go_merge_49_mlog_purge_batch_respects_commit_fence`、`go_merge_49_scheduled_mlog_purge_uses_auto_history` 及取消权限场景。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前只执行任务指定的 11 章节结构验证，并人工复核文档覆盖“为何存在、如何运行、如何安全扩展”。
