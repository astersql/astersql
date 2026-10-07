# `dumpling/export/status.rs`

## 文件定位

[`status.rs`](status.rs) 属于 `astersql-dumpling-export` library crate；[`lib.rs`](lib.rs) 通过 `include!("status.rs")` 将它并入与 Go `dumpling/export` 包相似的单包命名空间，而不是建立独立 Rust module。其上游主入口位于 [`dump.rs`](dump.rs) 的 `Dumper::Dump`：表清单准备完成后先用 `calculateTableCount` 写入 `Dumper.totalTables`，再以 `startLogProgress` 启动后台状态采样，writer 完成任务时更新指标，最后停止并等待采样线程发布终态。缓存的 `DumpStatus` 同时供进度日志和 [`http_handler.rs`](http_handler.rs) 的 `/status` 响应读取。

该文件不是单纯的数据类型定义：它集中实现了导出状态快照、周期刷新与日志、线程停止协议、基础表计数、JSON 表示以及最近吞吐速率。crate 边界由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 和 `package.metadata.porting.go-package = "dumpling/export"` 确认；本文件直接使用的上下文、日志和其他包内类型经 `lib.rs` 的共享导入或 `include!` 获得。

## 核心职责

1. `Dumper::RefreshStatus` 从 Prometheus 风格 counter/gauge、`totalTables` 和 chunk 原子计数采样，计算 `CurrentSpeedBPS` 与 chunk 百分比，再一次性替换 `Dumper.status` 中的快照。
2. `Dumper::GetStatus` 克隆已发布快照，不读取实时指标，也不推进 `SpeedRecorder` 的采样窗口；因此 HTTP 轮询不会改变吞吐统计。
3. `Dumper::startLogProgress`/`runLogProgress` 管理后台线程：默认每 5 秒刷新、每 2 分钟记录一次进度，取消时再刷新一次并退出。
4. `LogProgressGuard` 把“取消并等待线程”封装为显式 `stop` 和 `Drop` 清理协议，保证 `Dumper::Dump` 返回前能看到最终状态。
5. `calculateTableCount` 只统计 `TableType::TableTypeBase`，排除视图和序列；结果既是进度中的总表数，也是 `prepareColumnProjection` 容量预估依据。
6. `SpeedRecorder` 根据累计完成字节的增量和采样间隔计算 bytes/s；`DumpStatus::toJSON` 为 Rust HTTP 服务生成与 Go JSON 字段名一致的响应。

## 主要符号

- `statusRefreshTick: Duration = 5s`：快照刷新周期；独立于 HTTP 请求频率和进度日志周期。
- `logProgressTick: Duration = 120s`：常规进度日志周期。`EnableLogProgress` failpoint 生效时，`runLogProgress` 将日志周期改为 1 秒。
- `LogProgressGuard { cancel, worker }`：持有上下文取消函数和可选 `JoinHandle`。`stop(&mut self)` 先取消，再仅对尚未取走的线程句柄执行一次 `join`；`Drop` 复用同一路径。
- `Dumper::progressView(&self) -> Dumper`：构造供后台线程拥有的轻量视图。它共享 `conf`、`metrics`、`speedRecorder`、`status`、`totalTables`，但将数据库、外部存储、HTTP、PD client 和自身 cancel 置空，避免后台线程拥有无关资源。
- `Dumper::startLogProgress(&self, tctx) -> LogProgressGuard`：派生可取消上下文，创建 `progressView`，并启动执行 `runLogProgress` 的线程。
- `Dumper::runLogProgress` 与私有 `runLogProgressWithTicks`：前者固定生产周期和 failpoint；后者容纳确定性测试可注入的周期，执行初始刷新、定时刷新、日志及取消后的最终刷新。
- `Dumper::GetStatus(&self) -> DumpStatus`：对互斥锁中的快照做深拷贝式 `clone`；`ProgressPercent: Option<f64>` 也随值复制。
- `Dumper::RefreshStatus(&self)`：唯一集中采样入口，构造新 `DumpStatus` 后整体发布。
- `DumpStatus`：包含 `CompletedTables`、`FinishedBytes`、`FinishedRows`、`EstimateTotalRows`、`TotalTables`、`CurrentSpeedBPS`、面向人的 `Progress` 和面向程序的 `ProgressPercent`。
- `DumpStatus::setProgress`：同步维护字符串与数值两种百分比；完成时字符串为 `"100 %"`，未完成时用两位小数和固定宽度格式。
- `DumpStatus::toJSON`：输出 lower-camel-case JSON；空 `Progress` 和 `None` 的 `ProgressPercent` 被省略，非有限浮点值返回错误。
- `calculateTableCount(&DatabaseTables) -> i32`：遍历数据库到表列表的映射，只累计基础表。
- `SpeedRecorder`、`NewSpeedRecorder`、`GetSpeed`：保存上次累计字节、采样时刻及上次速率；构造时速率和完成字节均为 0。

## 执行流程

1. `Dumper::Dump` 在准备好 `conf.Tables` 后调用 `calculateTableCount`，以 `SeqCst` 写入共享 `totalTables`；随后调用 `startLogProgress`。
2. `startLogProgress` 从传入的 `tcontext::Context` 派生 `(ctx, cancel)`，将共享状态装入 `progressView`，线程入口调用 `view.runLogProgress(&ctx)`，主线程持有 `LogProgressGuard`。
3. `runLogProgressWithTicks` 进入循环前立即 `RefreshStatus`，记录日志区间起点与 `last_bytes`，并计算下一次状态和日志截止时间。
4. 循环首先检查 `tctx.Done()`；若已取消，则记录 debug 日志、执行最终 `RefreshStatus` 后返回。未取消时，越过状态截止点便刷新，并用 `while` 推进截止点以跳过已错过的周期，而非补发多次采样。
5. 越过日志截止点时，普通路径直接读取上一次缓存状态；failpoint 加速路径先刷新，确保短任务的一秒日志包含 chunk 进度。区间平均 MiB/s 使用实时 `finishedSizeGauge` 与 `last_bytes`，`recent speed bps` 则来自缓存快照中的 `CurrentSpeedBPS`，两种速度的采样窗口有意分离。
6. 每轮最多休眠到下一截止点与 10ms 中的较小值，以便较快观察取消。`Dumper::Dump` 完成 writer 消费后调用 `progress.stop()`，取消线程并等待最终快照发布。
7. `RefreshStatus` 读取总表数及四个工作量指标，再锁住 `SpeedRecorder` 更新速度。只有 `progressReady` 为真时才发布 chunk 百分比：`totalChunks == 0` 视为 100%；否则计算 `completed/total`，超过 1 时钳制为 100% 并告警。
8. `/status` 路径在 `http_handler.rs::serveHTTPConnection` 中克隆同一个 `Arc<Mutex<DumpStatus>>` 快照并调用 `toJSON`；它不会调用 `RefreshStatus`，因而不会改变速度基线。

## 数据与状态

`DumpStatus` 是一次采样的不可变语义快照，而实时数据源分散在 `Dumper` 中：`metrics.rs::metrics` 的 gauge/counter 保存已完成大小、行数、表数和估算总行数；`AtomicI64 totalTables` 保存准备阶段统计出的基础表总数；`metrics.totalChunks`、`completedChunks` 和 `progressReady` 是进度条专用的原子状态。所有这些原子读写在本路径使用 `Ordering::SeqCst`。

`progressReady == false` 表示 chunk 分母尚未知，不等同于 0%：此时 `Progress` 保持空字符串，`ProgressPercent` 保持 `None`，JSON 中两个字段均省略。ready 后，零 chunk 明确表示没有上游数据并作为完成处理；`completedChunks > totalChunks` 被视为可能的计数竞态，输出仍限制在 100%。源码未对负 chunk 计数做额外约束；安全扩展时应保持生产者只做非负计数的不变量。

`SpeedRecorder` 只在 `RefreshStatus` 下由 `Arc<Mutex<_>>` 串行访问。若 `finished <= last_finished` 或 elapsed 为零，返回上一次速率且不移动基线；只有累计字节增长且时间推进时才更新三项内部状态。`GetStatus` 只锁定并复制 `DumpStatus`，所以调用者修改返回值不会污染缓存。

## 依赖与调用关系

- 上游生产主链：`dump.rs::Dumper::Dump` 调用 `calculateTableCount` 和 `Dumper::startLogProgress`；writer 的 finish callbacks 更新 `finishedTablesCounter`、`completedChunks` 等指标，`Dump` 在消费完任务后调用 `LogProgressGuard::stop`。
- 另一处表计数调用：`dump.rs::prepareColumnProjection` 用 `calculateTableCount` 预分配 `columnProjection`，但随后仍遍历所有表信息；计数函数本身的语义仍是“基础表数”。
- HTTP 消费链：`http_handler.rs::startDumplingServiceWithDumper` 克隆 `d.status`，服务线程经 `serveHTTPConnection` 读取快照并调用 `DumpStatus::toJSON`。没有 Dumper 时 `/status` 返回 503。
- 指标依赖：`RefreshStatus` 调用 `metrics.rs` 中的 `ReadCounter`/`ReadGauge`，并读取 `metrics` 的三个 chunk 原子字段；`Dumper::Dump`、writer 与任务创建路径负责递增这些数据源。
- 上下文与日志依赖：`startLogProgress` 使用 `astersql-dumpling-context::Context::WithCancel`，日志通过 `tctx.L()`/`Dumper::L()` 和 `astersql-dumpling-log::Field` 输出。
- 包内类型依赖：`DatabaseTables`、`TableInfo`、`TableType` 定义在 [`prepare.rs`](prepare.rs)，`Dumper` 定义在 `dump.rs`。由于 `lib.rs` 的 `include!` 单包结构，本文件可直接引用这些符号。
- RustCodeGraph 对目标文件报告直接使用者为 `dump.rs`、`dump_test.rs`、`http_handler.rs`、`prepare_test.rs`；其 flow 查询确认 `startLogProgress → runLogProgress → runLogProgressWithTicks → RefreshStatus`，并确认 `calculateTableCount` 的生产调用点位于 `dump.rs`。

## 错误处理与边界

本文件的定时采样和大部分计算不返回业务错误。互斥锁中毒通过 `lock().unwrap()` 触发 panic；后台线程 panic 会在 `LogProgressGuard::stop` 的 `join().expect("dump progress worker panicked")` 中传播。此行为让采样线程故障不会被静默吞掉，但也意味着若以后在持锁区增加可 panic 逻辑，会影响导出收尾。

`DumpStatus::toJSON` 在任何浮点状态或可选百分比为 NaN/无穷时返回 `unsupported non-finite JSON status value`；`http_handler.rs` 记录告警并以空 body 结束已经确定为 200 的响应。当前 JSON 是手工拼接，数值字段受有限值检查保护，`Progress` 只由内部格式化函数生成；若未来允许外部字符串进入该字段，必须先补 JSON 转义或改用序列化器。

表进度日志在 `TotalTables == 0` 时执行浮点除法，可能产生 NaN 展示，但不会 panic。chunk 百分比只对大于 100% 做上限钳制，未显式钳制负值。速率把“累计值不增长或回退”解释为沿用旧值，而不是 0；这是当前 Go/Rust 共同语义，不应在没有兼容性评估时改动。

## 并发与资源生命周期

状态线程不借用原始 `Dumper`，而是拥有 `progressView`；其中关键状态都以 `Arc` 共享。快照和速率分别由不同 `Mutex` 保护，`RefreshStatus` 先释放 speed recorder 锁，再获取 status 锁，没有与 `GetStatus` 形成反向锁序。计数和 ready 标志使用原子类型；快照发布是一次互斥锁保护的整体替换，读者不会看到半填充对象。

`LogProgressGuard::stop` 可重复调用：第一次 `take()` 并 join worker，之后只再次发出取消，不再 join。`Drop` 提供遗漏显式 stop 时的兜底。取消路径在线程返回前执行最终刷新，且 stop 等待 join，因此调用者观察到 guard 停止后，最终状态已经发布。线程轮询睡眠被限制为最多 10ms，这是取消响应延迟与空转开销之间的实现选择；增加采样频率或改变轮询机制时应同时评估 CPU 开销和测试时序。

HTTP 服务只持有 `status` 的 `Arc`，不会延长数据库或存储句柄生命周期。`progressView` 同样故意不持有这些资源，避免后台进度线程阻碍其释放。

## 与 Go 版本的对应关系

直接对照文件是 [`status.go`](status.go)，Rust 保留了 `statusRefreshTick`、`logProgressTick`、`DumpStatus`、`GetStatus`、`RefreshStatus`、`calculateTableCount`、`SpeedRecorder`、`NewSpeedRecorder` 和 `GetSpeed` 的命名与核心分支。两端都在启动时刷新、取消退出时刷新；正常每 5 秒采样，每 2 分钟记录日志；`EnableLogProgress` 将日志缩短到 1 秒并先刷新；进度未知时省略进度字段，零 chunk 为完成，超过 100% 时钳制并告警。

实现机制存在语言差异：Go 用 `atomic.Pointer[DumpStatus]` 发布快照，并在 `GetStatus` 中复制结构体及百分比指针；Rust 用 `Arc<Mutex<DumpStatus>>` 加 `Clone`。Go 的 `SpeedRecorder` 自带 `sync.Mutex`，Rust 将无锁字段结构包在 `Dumper.speedRecorder: Arc<Mutex<SpeedRecorder>>` 中。Go `startLogProgress` 返回闭包并通过 `WaitGroupWrapper` 等待，Rust 返回带 `Drop` 的 `LogProgressGuard`。Go 用 `time.Ticker/select`，Rust 用 `Instant` 截止点、短睡眠和上下文轮询模拟相同生命周期。

Rust 还包含服务自身需要的私有 `toJSON`；Go 依赖 `encoding/json` 和结构体 tag。Rust 手工 JSON 会拒绝非有限数值，并在字段为空/缺失时实现 `omitempty` 效果。以上差异是实现手段而非行为简化；新增字段时需同时更新 Go tag 语义与 Rust JSON 输出。

## 扩展指南

- 新增状态字段：在 `DumpStatus`、`RefreshStatus` 的数据来源和 `toJSON` 中同步接线；若字段面向 Go 兼容 API，还需同步 `status.go::DumpStatus` 的 JSON tag。更新独立的 [`status_test.rs`](status_test.rs) 和 [`http_handler_test.rs`](http_handler_test.rs)，验证初始值、刷新后值、快照独立性和 HTTP 字段；不要把测试内嵌到生产文件。
- 改变进度算法：优先修改 `RefreshStatus`/`setProgress`，保持 `Progress` 与 `ProgressPercent` 同时更新，并覆盖 not-ready、0/0、部分完成、恰好完成、completed 超过 total 的边界。生产者侧变更还需核对 `dump.rs` 中 `progressReady`、`totalChunks`、`completedChunks` 的更新时机。
- 改变采样或日志周期：生产常量在本文件顶部，确定性测试入口是私有 `runLogProgressWithTicks`。必须保持 HTTP 读取与采样解耦、日志区间平均使用实时字节、取消后最终刷新并等待的协议，否则短导出或高频轮询会改变结果。
- 改变线程模型：保留 `progressView` 的最小资源所有权，避免将 DB/storage/HTTP/PD 句柄带入后台线程；维持 stop 幂等与 join 后可见性。若改为 channel/condvar，可减少 10ms 轮询，但要覆盖取消竞态和线程 panic 传播。
- 改变速率策略：修改 `SpeedRecorder::GetSpeed` 时同步 Go 版本和 `status_test.rs::test_speed_recorder`，并评估累计指标回退、零间隔、长时间无进展时是否继续展示旧速率的兼容性。
- 改变表计数：`calculateTableCount` 同时影响状态分母与列投影容量；若把视图或序列纳入，必须检查实际 task 生成与完成计数是否一致，防止长期无法达到 100%。

主要风险是：状态字段/JSON 不同步造成 API 兼容性变化；采样锁或周期变化造成并发时序回归；更频繁采样增加锁与指标读取开销；表/chunk 分母与完成计数口径分裂导致错误进度。这些修改均应保持 Rust 与 Go 的逻辑和独立测试尽可能一致。

## 验证依据

- RustCodeGraph：`status` 检查确认索引含 11467 个文件、其中 Rust 7032 个；`node --file dumpling/export/status.rs` 覆盖目标文件全部 314 行，并报告直接使用文件为 `dump.rs`、`dump_test.rs`、`http_handler.rs`、`prepare_test.rs`；`query` 定位了 `startLogProgress`、`RefreshStatus`、`GetStatus`、`calculateTableCount`、`NewSpeedRecorder`、`GetSpeed`；`explore` 给出 Rust 调用流 `startLogProgress → runLogProgress → runLogProgressWithTicks → RefreshStatus` 和生产 `calculateTableCount` 调用位置。
- 源码与 crate：已读 [`status.rs`](status.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`dump.rs`](dump.rs)、[`metrics.rs`](metrics.rs)、[`http_handler.rs`](http_handler.rs) 以及 `prepare.rs` 中 `DatabaseTables`/`TableType` 的定义与使用点。
- Go 对照：已读 [`status.go`](status.go)、[`status_test.go`](status_test.go) 和 [`http_handler.go`](http_handler.go)，核对快照、周期刷新、failpoint、最终发布、进度边界、速率及 HTTP JSON 语义。
- Rust 独立测试：[`status_test.rs`](status_test.rs) 覆盖指标刷新与缓存隔离、快照独立、进度 optional/钳制、可重复 stop、刷新/日志采样窗口、生产 5 秒周期、failpoint 和速率；[`http_handler_test.rs`](http_handler_test.rs) 覆盖缓存状态 JSON 与并发轮询不改变速度；[`dump_test.rs`](dump_test.rs) 的 `dump_publishes_final_nonempty_sql_export_status` 覆盖主流程结束后的非空最终状态。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定 shell 命令验证目标文件存在且恰含 11 个固定二级章节，并人工复核文档只描述有上述源码、图查询或测试支撑的行为。
