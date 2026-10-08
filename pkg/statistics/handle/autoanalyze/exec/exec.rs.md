# `pkg/statistics/handle/autoanalyze/exec/exec.rs`

## 文件定位

本文件是 crate `astersql-statistics-handle-autoanalyze-exec` 的业务实现，由同目录 `lib.rs` 通过 `mod exec; pub use exec::*;` 导出。它对应 Go 包 `pkg/statistics/handle/autoanalyze/exec`，把自动 ANALYZE 的语句转义与执行、系统进程登记、全局参数筛选、时间窗解析及窗口外中断集中在一个边界层中。

当前 Rust 接线状态必须与 Go 主链区分：RustCodeGraph 显示本文件由 `exec_test.rs` 使用；上层 `autoanalyze` 和 `refresher` 的 Cargo 依赖位于 `target.'cfg(any())'.dependencies`，不会在正常配置中启用。因此这里是已实现且有独立测试覆盖的移植模块，还不是 Rust 自动分析生产调度链的已启用入口。Go 版本则由 `autoanalyze.go`、`refresher/refresher.go`、priority queue 和 `Domain.CheckAutoAnalyzeWindows` 实际调用。

## 核心职责

- 用 `EscapeSQL` 展开 `%n` 等占位符，保证日志和会话执行器看到同一条已转义 ANALYZE SQL；转义失败时保留原 SQL。
- 强制自动分析使用 `statistics::Version2`，并在调用方指出 legacy 版本重写时记录告警。
- 为每次执行分配自动分析进程 ID，同时登记到全局进程列表和注入的 `SysProcTracker`；无论登记或执行成功与否都清理登记并释放 ID。
- 将执行错误转换为 `AutoAnalyze` 的布尔结果和错误日志，或由 `RunAnalyzeStmt` 原样返回给需要错误细节的调用方。
- 从调用方给出的全局变量快照中只提取自动分析修改率、开始时间和结束时间，并解析修改率与带时区时间窗。
- 按 UTC 的时、分判断当前时刻是否处于普通或跨午夜窗口；窗口外中断所有全局登记的自动分析进程。

这些职责分别由 `execOptionForAnalyzeVersion`、`RunAnalyzeStmt`、`AutoAnalyze`、`GetAutoAnalyzeParameters`、`ParseAutoAnalyzeRatio`、`ParseAutoAnalysisWindow`、`CheckAutoAnalyzeWindow` 和 `KillAutoAnalyzeOutsideWindow` 承担。

## 主要符号

- 常量 `TIDB_AUTO_ANALYZE_RATIO`、`TIDB_AUTO_ANALYZE_START_TIME`、`TIDB_AUTO_ANALYZE_END_TIME` 以及三个 `DEF_*` 常量直接复用 `astersql_sessionctx_vardef` 的变量名和默认值，避免在此 crate 维护第二套配置契约。
- `AnalyzeError(String)` 是本文件的轻量错误边界，实现 `Display` 和 `std::error::Error`；执行器、跟踪器及时间解析都通过它传递失败。
- `AnalyzeExecutor: Send + Sync` 把具体会话执行抽象为 `Execute(&str) -> Result<(), AnalyzeError>`。它不暴露结果行，因为此模块只关心 ANALYZE 是否成功。
- `SysProcTracker: Send + Sync` 抽象 `Track`、`UnTrack`、`KillSysProcess`，使系统进程登记与 kill 机制可由会话/Domain 侧实现，也便于独立测试注入。
- `StatsHandleOps` 组合 `AutoAnalyzeProcIdGenerator + Send + Sync`，并用 `AutoAnalyzeProcID`、`ReleaseAutoAnalyzeProcID` 保留 Go 风格 API。
- 私有 `escape_analyze_sql` 构造 `Vec<SqlArg::String>` 并调用 `EscapeSQL`；失败回退原 SQL。
- `execOptionForAnalyzeVersion` 先断言 `stats_ver == Version2`，再返回已转义 SQL；`need_warn` 为真时记录 legacy v1 被改写为 v2 的告警。
- `AutoAnalyze` 是只返回成功/失败的便捷入口；`RunAnalyzeStmt` 是保留 `AnalyzeError` 的底层执行入口。
- 私有 `ProcIdGuard` 在 `Drop` 中依次从 `GLOBAL_AUTO_ANALYZE_PROCESS_LIST` 移除、调用 `tracker.UnTrack`、释放 Handle 分配的 ID。
- `GetAutoAnalyzeParameters` 从 `HashMap<String, String>` 输入中复制三个受支持的键；它不自行访问 `mysql.global_variables`。
- `ParseAutoAnalyzeRatio` 将合法浮点数限制为不小于零，解析失败采用默认值，并显式保留 NaN，以对齐 Go `math.Max(NaN, 0)` 的结果。
- `AnalysisTime { Hour, Minute, OffsetMinutes }` 是解析后的时间值；私有 `parse_time` 只接受固定宽度 `HH:MM ±HHMM`。
- `ParseAutoAnalysisWindow` 对空起止值应用默认值；`analysis_time_as_utc` 将墙上时间和偏移转换为 UTC instant。
- `CheckAutoAnalyzeWindow` 返回格式化后的起止时分及当前是否在窗口内；`KillAutoAnalyzeOutsideWindow` 在窗口外逐个告警、kill 并移除全局登记。
- `new_test_handle_ops` 创建顺序分配 ID 的测试实现，只由独立测试使用，不是生产 Handle 工厂。

## 执行流程

1. 上层调用 `AutoAnalyze` 或直接调用 `RunAnalyzeStmt`，传入会话执行器、统计 Handle、系统进程跟踪器、统计版本、SQL 模板和标识符参数。
2. `RunAnalyzeStmt` 调用 `execOptionForAnalyzeVersion`：断言版本为 v2，经 `escape_analyze_sql` 得到可执行 SQL，并按 `need_warn` 写 legacy 重写告警。
3. Handle 分配 `auto_analyze_proc_id`；函数先把 ID 写入 `GLOBAL_AUTO_ANALYZE_PROCESS_LIST`，随即创建 `ProcIdGuard`。因此后面的 `tracker.Track` 即使失败，退出路径仍会释放全部资源。
4. `tracker.Track` 保存 `TrackProc`。其 `database`、`table` 目前为空，`statement` 是已转义 SQL；登记失败通过 `?` 返回，且不会调用执行器。
5. `sctx.Execute(&escaped)` 执行语句。返回或提前退出时 guard 析构，清理全局列表、外部 tracker 和 ID 分配器。
6. 若入口是 `AutoAnalyze`，成功映射为 `true`；失败则再次转义 SQL，记录 `sql` 与 `error` 字段后返回 `false`。

时间窗路径独立运行：`CheckAutoAnalyzeWindow` 读取参数 map，空缺按默认值解析，将起止点转换到 UTC，再调用 `WithinDayTimePeriod`。该函数对 `end >= start` 使用闭区间比较，对结束时间早于开始时间的情况使用“当前不晚于 end 或不早于 start”，因而支持跨午夜窗口。`KillAutoAnalyzeOutsideWindow` 仅在检查结果为假时遍历全局 ID，发出告警并调用 `KillSysProcess`。

## 数据与状态

- 输入 SQL 与 `params` 只在本次调用中转换为拥有所有权的 `String`/`SqlArg`，不会保存在模块级状态；跟踪期间 `TrackProc.statement` 持有一份已转义 SQL 副本。
- 共享可变状态位于依赖 crate 的 `GLOBAL_AUTO_ANALYZE_PROCESS_LIST`。本文件在执行前 `track(id)`，guard 析构和窗口外 kill 路径执行 `untrack(id)`；具体并发安全由该全局列表实现保证。
- 进程 ID 生命周期由调用方的 `StatsHandleOps` 管理。本文件只保证一次分配对应退出时一次 `ReleaseAutoAnalyzeProcID` 调用。
- 自动分析参数以调用方提供的 `HashMap` 快照传入。`GetAutoAnalyzeParameters` 返回新的 map，不保存引用，也不会写回全局变量表。
- `AnalysisTime` 保存输入墙上时间与 UTC 偏移分钟。窗口展示字符串使用输入的 `Hour`/`Minute`，窗口比较使用换算后的 UTC 时分；日期固定为 1970-01-01，仅日内时分参与判断。
- `AutoAnalyze` 测量 `Instant::elapsed()`，但当前 `_duration` 没有输出到指标或日志，不能据此声称 Rust 已对齐 Go 的耗时直方图。

## 依赖与调用关系

RustCodeGraph 给出的核心调用边为：`AutoAnalyze -> RunAnalyzeStmt`，`AutoAnalyze -> escape_analyze_sql/StatsLogger`；`RunAnalyzeStmt -> execOptionForAnalyzeVersion -> escape_analyze_sql/EscapeSQL`，并调用 `StatsHandleOps::AutoAnalyzeProcID`、`SysProcTracker::Track`、`AnalyzeExecutor::Execute`；`KillAutoAnalyzeOutsideWindow -> CheckAutoAnalyzeWindow -> ParseAutoAnalysisWindow`，窗口比较下沉到 `astersql_util_timeutil::time_zone::WithinDayTimePeriod`。

crate 边界由同目录 `Cargo.toml` 定义。生产依赖分别提供系统变量常量（`astersql-sessionctx-vardef`）、`Version2`（`astersql-statistics`）、统计日志（`astersql-statistics-handle-logutil`）、进程 ID 与全局列表（`astersql-statistics-handle-util`）、SQL 转义（`astersql-util-sqlescape`）、时间窗比较（`astersql-util-timeutil`），另直接使用 `chrono` 处理时区。`domain`、`session`、`statistics-handle`、`testkit`、`sqlkiller` 等只列为 dev-dependencies，用于 `exec_test.rs` 的真实会话测试。

仓库级反向引用显示，普通 Rust 源码没有调用这些导出函数；同目录 `exec_test.rs` 是当前直接使用者。根 `Cargo.toml` 提供 facade 别名，上层 `autoanalyze/Cargo.toml` 与 `refresher/Cargo.toml` 虽声明本 crate，但相关依赖被 `cfg(any())` 永久关闭。因此安全扩展时不能假设修改会立即影响 Rust 服务运行时；启用生产接线需要单独处理调用方及其会话适配。

## 错误处理与边界

- `execOptionForAnalyzeVersion` 用 `assert_eq!` 强制 v2，传入其他版本会 panic，而不是返回 `AnalyzeError`。调用者必须在进入本层前完成 legacy v1 到 v2 的选择；`need_warn` 仅控制告警。
- SQL 转义失败不会中止分析：`escape_analyze_sql` 回退原 SQL。它同时用于执行和失败日志，意味着异常模板可能被原样交给执行器。
- `tracker.Track` 与 `AnalyzeExecutor::Execute` 的错误原样传播；`AutoAnalyze` 把它们降为 `false` 并记错误日志。`RunAnalyzeStmt` 调用方可保留错误细节。
- `ProcIdGuard` 在 `Track` 前创建，专门覆盖登记失败的清理边界。Rust 测试验证此时执行器不会运行、tracker 会 UnTrack、Handle 会释放 ID、全局列表不残留。
- `parse_time` 拒绝非 11 字节格式、非两位时分、超出 23:59 的时分、缺少或非法符号、非五字符/超范围时区。错误文本按失败阶段区分为 analysis time、hour、minute 或 timezone，但不附带原始输入。
- `CheckAutoAnalyzeWindow` 遇到任何解析错误会记录错误，并返回 `("00:00", "00:00", false)`。调用 `KillAutoAnalyzeOutsideWindow` 时，这一保守结果会进入窗口外 kill 分支。
- `analysis_time_as_utc` 对无法构造的偏移或日期采用 UTC 零点回退；在 `parse_time` 已校验的正常路径上不应触发，但这是防御性退路。
- `GetAutoAnalyzeParameters` 不报告缺键；缺少的时间值在窗口解析时视为空并使用默认值，缺少比率值交给 `ParseAutoAnalyzeRatio` 后也回退默认比率。

## 并发与资源生命周期

三个注入 trait 都要求执行相关对象满足 `Send + Sync`，允许自动分析任务和窗口检查位于不同线程。`exec_test.rs::test_kill_in_windows` 正是在线程中运行 `RunAnalyzeStmt`，等待执行到暂停点后，从另一线程路径调用 `KillAutoAnalyzeOutsideWindow` 并由 `SQLKiller` 发出中断。

资源顺序是不变量：分配 ID，加入全局列表，安装 guard，登记 tracker，执行 SQL，最后由 guard 清理。窗口外 kill 会提前把 ID 从全局列表移除；执行线程结束时 guard 再次 `untrack`，因此全局列表与 tracker 的实现需要允许这种竞态下的重复移除。窗口 kill 只发送信号，不等待执行线程退出，也不直接释放 ID；ID 最终仍由执行线程的 guard 释放。

本文件没有自行创建异步任务、线程、锁或 channel。并发控制、实际 SQL 会话锁、kill 信号传递和全局列表同步都由注入实现及依赖 crate 负责。`new_test_handle_ops` 使用 `AtomicU64` 的 `SeqCst` 顺序分配测试 ID，但释放是空操作，不能替代生产 Handle。

## 与 Go 版本的对应关系

- `AutoAnalyze`/`RunAnalyzeStmt` 保留 Go 的主分层：外层记录失败并返回布尔值，内层执行并返回错误；SQL 转义、v2 断言、legacy 重写告警、进程 ID 分配与释放也有直接对应。
- Rust 用三个 trait 注入会话、Handle 和 sys-proc tracker；Go 直接依赖 `sessionctx.Context`、`StatsHandle`、`sysproctrack.Tracker` 及 `statsutil.ExecWithOpts`。
- Go `RunAnalyzeStmt` 还传入 analyze snapshot、partition prune mode、当前 session、sys-proc tracking 等 `sqlexec` options，并返回 rows/fields；Rust 只执行已经转义的 SQL 字符串并返回 `Result<(), AnalyzeError>`。这些会话 option 和结果行语义尚未移植到本文件。
- Go 外层上报 `AutoAnalyzeHistogram`、成功/失败 counter，并在 panic 时恢复后释放 ID；Rust 当前不写 metrics，也没有 `catch_unwind`。Rust guard 能覆盖普通 `Result` 退出和栈展开时的析构，但进程 abort 不在保证范围内。
- Go `GetAutoAnalyzeParameters` 自行查询 `mysql.global_variables`，查询失败返回空 map；Rust 接受已取得的 map 并只做白名单复制，因此数据库读取职责仍在调用方。
- Go 的窗口解析使用 `time.ParseInLocation(FullDayTimeFormat, ..., UTC)`；Rust 固定解析 `HH:MM ±HHMM` 并显式保存偏移。二者随后都转成 UTC 时分，并通过等价的闭区间/跨午夜算法比较。
- Go 窗口外 kill 位于 `Domain.CheckAutoAnalyzeWindows`：获取系统 session，调用 `autoanalyze.CheckAutoAnalyzeWindow`，再遍历全局列表并 kill；Rust 把检查与遍历封装为本文件的两个函数，而且 Rust 路径会在 kill 后主动从全局列表移除 ID。
- `exec_test.go` 验证真实 ANALYZE、legacy v2 重写和窗口外中断；`exec_test.rs` 保留这些意图，并额外验证 NaN、固定宽度时间输入及 Track 失败时的资源清理。分区 legacy 测试因窄 Rust 会话不支持直接更新 `mysql.stats_histograms`，通过统计 Handle 发布 API 构造等价状态，且只断言全局表和目标分区达到 v2；这是与 Go 断言所有相关 ID 的已记录差异。

## 扩展指南

- 若增加 ANALYZE 执行选项，优先扩展 `AnalyzeExecutor` 的输入契约或引入明确的 options 类型，并在 `RunAnalyzeStmt` 统一组装；需要逐项对照 Go 的 snapshot、partition prune mode、current session 和 sys-proc track 语义，不能只让测试通过。
- 若接入 Rust 生产自动分析主链，需要同时解除上层 Cargo 的 `cfg(any())` 隔离，选择真实 `AnalyzeExecutor`/`StatsHandleOps`/`SysProcTracker` 实现，并新增上层独立测试；不要把 `new_test_handle_ops` 用于生产。
- 若改变进程登记流程，必须保留“guard 在第一个可失败的 Track 前已安装”的不变量，并同步 `exec_test.rs::test_run_analyze_stmt_releases_process_id_when_tracking_fails`。涉及 kill 时还应覆盖登记、kill、执行退出的竞态和重复 UnTrack。
- 若修改 SQL 模板或参数类型，应从 `escape_analyze_sql` 与 `execOptionForAnalyzeVersion` 接入，并同步普通表、分区表以及转义告警断言，防止日志 SQL 与实际执行 SQL 分叉。
- 若扩展全局参数，更新常量、`GetAutoAnalyzeParameters` 白名单、默认值处理和上层查询职责；同时评估 Go `mysql.global_variables` 查询及 priority queue/refresher 调用方的兼容性。
- 若修改时间格式或窗口算法，同步 `parse_time`、`analysis_time_as_utc`、`CheckAutoAnalyzeWindow` 以及 `pkg/util/timeutil/time_zone.rs::WithinDayTimePeriod` 的契约，并添加普通窗口、跨午夜、边界等于起止点、非零偏移、非法输入的独立测试。
- Rust 测试继续放在同目录 `exec_test.rs`，Go 对照测试位于 `exec_test.go`；不要把测试逻辑内嵌回 `exec.rs`。兼容风险主要是 Go/Rust 选项差异和时间偏移语义，性能风险主要是重复 SQL 分配、全局列表遍历及窗口外批量 kill。

## 验证依据

- 目标实现：`pkg/statistics/handle/autoanalyze/exec/exec.rs`；crate 导出：`pkg/statistics/handle/autoanalyze/exec/lib.rs`；依赖边界与移植元数据：同目录 `Cargo.toml`。
- Rust 独立测试：`pkg/statistics/handle/autoanalyze/exec/exec_test.rs`，覆盖真实自动分析、legacy v2 告警、NaN、固定宽度时间格式、Track 失败清理以及并发窗口外中断。
- Go 对照：同目录 `exec.go` 与 `exec_test.go`；生产调用证据来自 `pkg/statistics/handle/autoanalyze/autoanalyze.go`、`refresher/refresher.go`、priority queue 文件及 `pkg/domain/domain.go::CheckAutoAnalyzeWindows`。
- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边。文件查询识别 `exec.rs` 的 47 个符号；`explore` 与 `callers/callees` 核对了 `AutoAnalyze -> RunAnalyzeStmt`、`RunAnalyzeStmt` 的 executor/tracker/ID/版本处理调用，以及 `KillAutoAnalyzeOutsideWindow -> CheckAutoAnalyzeWindow -> ParseAutoAnalysisWindow`。
- `rustcodegraph node WithinDayTimePeriod` 核实 Rust 实现把三个时间转换为 UTC 分钟，普通窗口使用闭区间，跨午夜窗口使用析取条件；其调用轨迹包含本文件的 `CheckAutoAnalyzeWindow`。
- 仓库反向搜索确认当前 Rust 直接调用只在 `exec_test.rs`，并确认上层 `autoanalyze`/`refresher` 对本 crate 的依赖位于 `cfg(any())`；因此文档没有宣称生产 Rust 主链已经接通。
- 本任务是纯文档分析，按任务约束不运行 Cargo。交付前使用任务给定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核以上每项结论均可回指到符号、调用边、Cargo、Go 对照或独立测试。
