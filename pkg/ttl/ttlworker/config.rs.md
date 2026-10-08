# `pkg/ttl/ttlworker/config.rs`

## 文件定位

[`config.rs`](config.rs) 是 `astersql-ttl-ttlworker` crate 的配置值模块，由同目录 [`lib.rs`](lib.rs) 以 `pub mod config` 对外公开。它只依赖标准库的 `std::time::Duration`，集中保存 TTL worker 的调度周期、内部 SQL 超时、作业寿命和扫描拆分下限，并提供两个计算入口：`IntervalOverrides` 的有效间隔 getter，以及 `scan_split_count`。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，包名为 `astersql-ttl-ttlworker`、库入口为 `lib.rs`。当前 Rust 生产链中可确认的跨 crate 调用是 `pkg/session/runtime/ttl_metadata.rs::split_ttl_scan_ranges` 调用 `astersql_ttl_ttlworker::config::scan_split_count`，再把结果交给 TTL 表的主键或索引范围拆分逻辑。对全仓 Rust 引用的核查表明，`IntervalOverrides` 目前只被本 crate 的独立测试引用；不能据此宣称这些 override 已经接入 Rust Job/Task Manager 的生产循环。

## 核心职责

本文件承担三类职责：

1. 以 `Duration` 常量表达 Go TTL worker 中分散使用的默认周期，包括 Job Manager、Task Manager、心跳、缓存刷新、worker 扩缩容和 GC 周期，以及内部 SQL 超时和 job 超时。
2. 用 `IntervalOverrides` 将 Go 版 failpoint getter 的“覆盖值优先、否则返回默认值”语义建模为显式的 `Option<Duration>`。这使测试无需依赖 Go failpoint 运行时即可验证默认值和覆盖分支，但当前生产接线仍未由调用关系证实。
3. 用 `scan_split_count(is_tikv, tikv_store_count)` 保留 Go `getScanSplitCnt` 的核心边界：非 TiKV 始终拆成 64 份；TiKV 至少拆成 64 份，store 超过 64 时提升到 store 数量。

它不负责启动 ticker、读取集群拓扑、访问 RegionCache、执行 SQL、管理线程或提交扫描任务；这些动作属于上游运行时或其他 TTL worker 模块。

## 主要符号

- `JOB_MANAGER_LOOP_TICKER_INTERVAL = 10s`：Job Manager 检查 job 的默认周期，同时也是 `IntervalOverrides::heartbeat` 的 Go 对齐默认值。
- `UPDATE_INFO_SCHEMA_CACHE_INTERVAL = 120s`、`UPDATE_TTL_TABLE_STATUS_CACHE_INTERVAL = 120s`：InfoSchema 与 TTL 状态缓存的默认刷新周期。
- `TTL_INTERNAL_SQL_TIMEOUT = 30s`：Go 版 TTL 元数据 SQL 的超时常量。当前全仓 Rust 引用只找到定义，尚未证实生产消费方。
- `RESIZE_WORKERS_INTERVAL = 30s`：scan/delete worker 扩缩容检查周期。
- `SPLIT_SCAN_COUNT = 64`：扫描范围拆分数的固定下限。
- `TTL_JOB_TIMEOUT = 6h`：Go 版单个 TTL job 的最长存活时间。当前全仓 Rust 引用只找到定义。
- `TASK_MANAGER_LOOP_TICKER_INTERVAL = 60s`、`TTL_TASK_HEARTBEAT_TICKER_INTERVAL = 60s`：Task Manager 主循环与 task 心跳周期。
- `TTL_GC_INTERVAL = 600s`：TTL 元数据 GC 周期。
- `JOB_MANAGER_SYNC_TIMER_INTERVAL = 1s`、`TASK_MANAGER_CHECK_TASK_INTERVAL = 5s`、`CHECK_TRIGGERED_JOB_INTERVAL = 2s`：分别对应 Go getter `getJobManagerLoopSyncTimerInterval`、`getTaskManagerLoopCheckTaskInterval`、`getCheckJobTriggeredInterval` 的默认值。
- `IntervalOverrides`：公开、可克隆且实现 `Debug`/`Default` 的配置结构。其 11 个公开 `Option<Duration>` 字段分别覆盖 check-job、job 心跳、timer 同步、两类缓存刷新、worker 扩缩容、check-task、task 主循环、task 心跳、触发 job 检查和 GC。
- `IntervalOverrides::{check_job, heartbeat, sync_timer, update_info_schema, update_table_status, resize_workers, check_task, task_loop, task_heartbeat, check_triggered_job, gc}`：无副作用 getter；字段为 `Some` 时原样返回，为 `None` 时返回对应常量。
- `scan_split_count(is_tikv: bool, tikv_store_count: usize) -> usize`：公开纯函数。`is_tikv == true` 时返回 `max(64, tikv_store_count)`，否则忽略计数并返回 64。

文件没有 trait、enum、宏、条件编译块或私有辅助函数；所有配置常量、结构字段、getter 和拆分函数均为公开符号。

## 执行流程

间隔读取流程是：调用方先构造 `IntervalOverrides`；`Default` 会把全部字段置为 `None`；调用某个 getter 后，该 getter 对相应字段执行 `unwrap_or`，有覆盖值就直接返回覆盖值，没有覆盖值就返回编译期常量。各 getter 相互独立，不会修改结构，也不会联动其他间隔。一个需要特别保留的分支是 `heartbeat()`：它默认返回 `JOB_MANAGER_LOOP_TICKER_INTERVAL`（10 秒），而不是名字相近的 `TTL_TASK_HEARTBEAT_TICKER_INTERVAL`（60 秒），这与 Go `getHeartbeatInterval` 一致；task 心跳应通过 `task_heartbeat()` 获取。

扫描拆分流程是：`pkg/session/runtime/ttl_metadata.rs::split_ttl_scan_ranges` 根据 `store_count.is_some()` 判断是否拥有 TiKV store 计数，将布尔值与 `store_count.unwrap_or(0)` 传给 `scan_split_count`。非 TiKV 分支固定返回 64；TiKV 分支取 64 与 store 数量的较大者。上游随后将该值传给 `CachePhysicalTable::SplitIndexScanRanges` 或主键扫描范围拆分路径，因此它控制目标拆分粒度，而不亲自生成范围或创建任务。

## 数据与状态

所有默认值都是不可变的进程内常量。`Duration` 保留时间单位语义，避免裸整数在秒、纳秒之间混淆；三个扫描参数则使用 `usize`，与 Rust 集合计数和上游 store 数量类型一致。

`IntervalOverrides` 是普通值对象，不包含全局单例、锁、原子变量或内部可变性。克隆会复制 11 个 `Option<Duration>`；getter 只借用 `&self`。结构本身不验证间隔是否为零或是否过小，因此 `Some(Duration::ZERO)` 也会被原样接受；是否适合作为 ticker 周期必须由实际消费方保证。

`scan_split_count` 也不保存状态。它只区分“是否为 TiKV”与“已由上游获得的 store 数量”，不会自己访问 storage、PD 或 RegionCache。对 `is_tikv == false` 的调用，即使传入很大的 `tikv_store_count`，结果仍固定为 64。

## 依赖与调用关系

- 下游依赖：本文件仅使用 `std::time::Duration`，没有错误库、异步运行时或 TTL 子 crate 依赖。虽然 crate 的 [`Cargo.toml`](Cargo.toml) 声明了 `astersql-ttl-cache`，以及 Windows 目标下的一组移植依赖，但这些依赖没有被本文件直接导入。
- 模块出口：[`lib.rs`](lib.rs) 的 `pub mod config` 让其他 crate 可通过 `astersql_ttl_ttlworker::config` 使用公开符号；同一文件用 `#[cfg(test)] mod config_test` 将独立测试接入测试构建。
- 已确认的生产调用边：`pkg/session/runtime/ttl_metadata.rs::split_ttl_scan_ranges -> scan_split_count`。`pkg/session/Cargo.toml` 通过路径依赖 `../ttl/ttlworker` 引入该 crate。
- 已确认的测试调用边：`config_test.rs::interval_defaults_and_overrides_match_go_getters` 调用全部 11 个 getter；`config_test.rs::scan_split_count_matches_go_store_count_boundaries` 调用 `scan_split_count`；`timer_test.rs::interval_defaults_match_go_failpoint_fallbacks` 复核其中 4 个 Go fallback 值。
- 当前接线限制：RustCodeGraph 的精确符号结果与全仓 Rust 文本引用核查只证实 `scan_split_count` 的生产使用。其余常量和 `IntervalOverrides` 不能仅凭 Go 调用方推断为 Rust 生产链已使用。

## 错误处理与边界

本文件没有 `Result`、`Option` 返回值、panic 路径或日志。getter 的 `Option` 只表示“是否覆盖”，最终总能得到一个 `Duration`。`scan_split_count` 对零 store、恰好 64 个 store 和超过 64 个 store 都有确定结果，也不会发生减法或除法。

边界行为由 [`config_test.rs`](config_test.rs) 明确覆盖：非 TiKV 的计数 0 和 128 均得到 64；TiKV 的计数 0、64 得到 64，65 和 128 则原样提升。测试还用 7 纳秒验证每个 `Some` 覆盖值不会被截断或替换。Go `job_manager_test.go::TestSplitCnt` 进一步证明原实现对 nil/非 TiKV store 返回 64，并在 TiKV store 数从 1 增至 128 时以 64 为分界。

调用方需要自行处理两项边界：其一，`scan_split_count(true, 0)` 无法区分“集群确实没有 store”与“计数暂不可得”，两者都回退到 64；其二，override 不限制零时长或极短时长，若未来接入真实 ticker，必须核对所用计时 API 对零周期的约束。

## 并发与资源生命周期

本文件不创建线程、异步任务、channel、ticker、锁、session、事务或网络连接，也没有析构清理。常量和纯函数天然可被多个线程并发读取；`IntervalOverrides` 的并发共享策略由调用方决定，当前类型没有运行时热更新机制。

生命周期边界刻意停在“计算配置值”：Go 版 `job_manager.go` 会把 getter 结果交给 `time.Tick`、`time.NewTicker` 或 `context.WithTimeout`，并负责 manager context 和 cancel 的生命周期；Rust 本文件没有对应资源管理代码。`scan_split_count` 的上游负责获取 store 数量，下游负责创建扫描范围和任务，本函数两端都不持有资源。

## 与 Go 版本的对应关系

直接对照文件是 [`config.go`](config.go)。13 个 Rust 常量与 Go 的默认值一致：10 秒 Job Manager 周期、两个 2 分钟缓存周期、30 秒内部 SQL 超时与扩缩容周期、64 个扫描拆分下限、6 小时 job 超时、1 分钟 Task Manager/心跳周期、10 分钟 GC、1 秒 timer 同步、5 秒 check-task、2 秒已触发 job 检查。

Go 用 11 个 `get*Interval` 函数包裹 failpoint：注入存在时返回注入的纳秒 `Duration`，否则返回默认值。Rust 将相同选择语义集中到 `IntervalOverrides` 的 11 个字段和 getter 中。两者在默认值和显式覆盖值上由 `config_test.rs` 对齐，但机制不同：Go 从进程级 failpoint 动态取值，Rust 由调用方持有并传入一个普通结构；而当前仓库尚未显示 Rust 生产循环持有该结构。因此，“默认/覆盖计算已移植”是已验证事实，“运行时 failpoint 能影响 Rust worker”则未得到证据。

Go `getScanSplitCnt(store kv.Storage)` 自行做类型断言，并从 TiKV RegionCache 统计 TiKV store；Rust `scan_split_count` 将环境探测拆到上游，只接收 `is_tikv` 和 store 数。二者的计算规则相同，但 Rust 正确性也依赖上游传参能准确表达存储类型与 store 数量。`pkg/session/runtime/ttl_metadata.rs` 当前以 `store_count.is_some()` 表示 TiKV，并以 `None -> 0` 传入计数。

## 扩展指南

新增或修改调度间隔时，应同时更新默认常量、`IntervalOverrides` 字段与 getter，并扩展独立的 [`config_test.rs`](config_test.rs)，不要把测试内嵌回生产源文件。若该设置来自 Go 对齐，还应同步检查 `config.go` 的 getter、failpoint 名称和真实调用点；尤其不要混淆 job 心跳的 10 秒默认值与 task 心跳的 60 秒默认值。

将 override 接入生产循环时，最可能的修改点不是本文件，而是实际创建 ticker/超时的 Rust Job/Task Manager 或 session runtime。接线前应决定配置对象的所有权、是否支持运行时热更新、零间隔是否合法以及 worker 停止时如何释放计时资源，并增加对应模块的独立测试。不能用本文件现有 getter 测试代替生产接线测试。

修改 `scan_split_count` 时，应同步检查 `pkg/session/runtime/ttl_metadata.rs::split_ttl_scan_ranges`、Rust `config_test.rs::scan_split_count_matches_go_store_count_boundaries` 和 Go `job_manager_test.go::TestSplitCnt`。提高拆分数可能增加并发任务、调度与元数据开销，降低拆分数可能造成单个范围过大或热点；还需验证非 TiKV、未知计数、64 边界和超大 store 数。若改变存储探测语义，应在上游完成，而不是让这个纯函数重新依赖具体 storage 类型。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、索引中 `pkg/ttl/ttlworker/config.rs` 有 14 个符号；`files --filter pkg/ttl/ttlworker` 确认源码、Go 对照和独立测试位置；精确 `explore`/`query` 确认全部 getter 的测试调用以及 `scan_split_count` 到 `pkg/session/runtime/ttl_metadata.rs::split_ttl_scan_ranges` 的生产调用边。
- 源码：[`config.rs`](config.rs)（全部常量、`IntervalOverrides` 与 `scan_split_count`）；[`lib.rs`](lib.rs)（公开模块和独立测试装配）；`pkg/session/runtime/ttl_metadata.rs`（实际拆分调用）；`pkg/session/Cargo.toml`（反向路径依赖）。
- crate 声明：[`Cargo.toml`](Cargo.toml)（包名、库入口、端口元数据和依赖边界）。
- Go 对照：[`config.go`](config.go)（常量、failpoint getter、RegionCache store 计数）；[`job_manager_test.go`](job_manager_test.go) 的 `TestSplitCnt`；`job_manager_integration_test.go` 的调度加速 failpoint 设置与清理。
- Rust 测试：[`config_test.rs`](config_test.rs) 的 `interval_defaults_and_overrides_match_go_getters`、`scan_split_count_matches_go_store_count_boundaries`；[`timer_test.rs`](timer_test.rs) 的 `interval_defaults_match_go_failpoint_fallbacks`。
- 按任务约束未运行 Cargo 或代码测试；本次验证限于索引调用图、直接源码/清单/测试对照和文档结构检查。
