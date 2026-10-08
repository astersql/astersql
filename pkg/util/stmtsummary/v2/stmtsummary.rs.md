# `pkg/util/stmtsummary/v2/stmtsummary.rs` 逻辑说明

## 文件定位

`stmtsummary.rs` 是 crate `astersql-util-stmtsummary-v2` 的窗口聚合与生命周期核心。crate 根 `pkg/util/stmtsummary/v2/lib.rs` 将本模块整体再导出，并与 `record.rs`、`logger.rs`、`reader.rs`、`column.rs` 共同组成 v2 语句摘要：本文件负责接收单次执行、按摘要键聚合、LRU 驱逐、窗口轮转、文件追加写以及 v1/v2 全局代理；记录字段怎样累加由 `record.rs::StmtRecord` 实现，查询行怎样生成则由 `reader.rs` 和 `column.rs` 实现。

生产入口有两条直接证据：`cmd/tidb-server/main.rs::setupStmtSummary` 根据实例配置构造本文件的 `Config` 并调用 `Setup`；`pkg/session/runtime/scan_adapter_runtime.rs` 在语句完成时构造 `StmtExecInfo`，补入 RU 明细后调用 crate 级 `Add`。`reader.rs::MemorySummarySource` 则是当前内存窗口的读取边界，本文件为 `StmtSummary` 实现该 trait。

`pkg/util/stmtsummary/v2/Cargo.toml` 声明该目录是独立 library crate，入口为 `lib.rs`、禁用自动测试发现；主要直接依赖是 `lru`、`crossbeam-channel`、`parking_lot`、`chrono`/`chrono-tz`，以及提供 `StmtExecInfo`、摘要键池和 v1 回退对象的 `astersql-util-stmtsummary`（代码别名 `task_stmtsummary`）。Cargo 的 `package.metadata.porting.go-package` 指向同目录 Go 包，说明这是 `stmtsummary.go` 的移植边界。

## 核心职责

1. 维护一个以 `digestKey` 结果为键的有界 `LruCache`，同键执行合并到 `StmtRecord`，容量不足时通过 `onEvict` 生成驱逐统计。
2. 用 `stmtWindow` 表示单个刷新周期；周期结束时将当前 LRU 记录和未被逐条日志覆盖的驱逐聚合快照交给 `stmtStorage::persist`。
3. 在 `PersistEvicted` 开启时，将被驱逐的单条记录非阻塞地送入有界通道，由 `evictedLogLoop` 批量落盘；通道满时不阻塞写入热路径，而是增加丢弃计数。
4. 暴露运行时选项及清理操作，包括启用开关、内部 SQL、容量、SQL 长度、刷新周期、逐条驱逐持久化和按用户分组。
5. 管理生产实例的后台线程和关闭顺序，并通过 `Setup`/`Close` 管理全局 v2 实例；全局代理在 v2 未激活或初始化失败时回退到 v1 `StmtSummaryByDigestMap`。
6. 为 `reader.rs::MemReader` 提供一致的 `MemWindowSnapshot`，使当前窗口记录及驱逐“其他”聚合可以映射到查询结果。

本文件不负责解析 SQL、生成 digest、定义记录的全部统计字段，也不负责历史日志查询；这些分别来自调用方的 `StmtExecInfo`、`record.rs` 和 `reader.rs`。

## 主要符号

- 配置和默认值：`Config` 提供 `Filename`、`FileMaxSize`、`FileMaxDays`、`FileMaxBackups`；`defaultMaxStmtCount` 为 3000、`defaultMaxSQLLength` 为 32768、`defaultRefreshInterval` 为 1800 秒。当前 Rust `fileStmtStorage::new` 只消费 `Filename`，后三个滚动参数尚未接入本文件的存储构造。
- 存储抽象：私有 trait `stmtStorage` 定义 `persist`、`logEvicted`、`sync` 和测试观测方法 `persistedEvictedCount`。`fileStmtStorage` 以追加模式打开文件并写 JSON Lines；`mockStmtStorage` 保存窗口和驱逐记录，供 `NewStmtSummary4Test` 使用。
- 窗口结构：`stmtWindow` 保存开始时间、LRU、`stmtEvicted` 和累计驱逐次数；`WindowSnapshot` 是脱离窗口锁后交给存储的克隆快照。`stmtEvicted` 同时维护去重键集合、全部驱逐的 `other`、以及仅包含未逐条入队记录的 `otherForPersist`。
- 主对象：`StmtSummaryInner` 聚合原子选项、窗口锁、存储、驱逐通道、停止/关闭标志；公开 `StmtSummary` 再持有后台 `JoinHandle` 列表。`StmtSummary::create` 是共同构造器。
- 写入和驱逐：`StmtSummary::Add`、`digestKey`、`onEvict`、`evictedLogLoop`、`flushBatch` 组成写入链；`SetMaxStmtCount` 缩容也复用 `onEvict`。
- 轮转和关闭：`rotateLoop`、`rotateWindow`、`StmtSummary::Close`、`Drop` 管理窗口与线程生命周期。
- 读取边界：`StmtSummary::Evicted` 返回 `[BEGIN_TIME, END_TIME, EVICTED_COUNT]`；`MemorySummarySource::currentWindowSnapshot` 返回记录和聚合驱逐记录；`Len`、`EvictedCount`、`DroppedEvictedCount` 等提供状态观测。
- 全局 API：`NewStmtSummary`、`Setup`、`Close`、`Add`、`Enabled`、各 `Set*` 函数，以及 `GLOBAL_STMT_SUMMARY`、`GLOBAL_PERSISTENT_ENABLED`。`NewStmtSummary4Test` 和带 `cfg(test)` 的辅助方法属于测试入口。

## 执行流程

初始化流程如下：`cmd/tidb-server/main.rs::setupStmtSummary` 读取实例配置并调用 `Setup`；`Setup` 先调用 `NewStmtSummary`，后者校验文件名、创建 `fileStmtStorage`，再由 `StmtSummary::create` 建立 LRU、容量为 1024 的驱逐通道、驱逐日志线程和轮转线程。只有完整构造成功后，`Setup` 才替换全局实例并将 `GLOBAL_PERSISTENT_ENABLED` 设为真；旧实例在替换后关闭。若文件打开失败，当前已安装实例不会被替换，但持久模式标志会关闭，全局代理转向 v1。

一次语句写入经过以下阶段：

1. `pkg/session/runtime/scan_adapter_runtime.rs` 形成 `StmtExecInfo` 并调用全局 `Add`。
2. 全局 `Add` 通过 `activeGlobal` 选择 v2；未激活时调用 v1 `StmtSummaryByDigestMap.AddStatement`。
3. `StmtSummary::Add` 先检查 `closed`，在窗口锁内根据 `GroupByUser` 决定用户维度，并调用 `digestKey`。键包含 schema、当前/前一 SQL digest、plan digest、资源组以及可选用户。
4. 已存在的键由 LRU `get` 提升热度；新键用 `NewStmtRecord` 创建记录并插入。如果插入造成淘汰，`onEvict` 在窗口锁内处理旧记录。
5. 释放窗口锁后，取得该记录自己的 `parking_lot::Mutex` 并调用 `StmtRecord::Add` 累加执行数据。

驱逐流程中，`onEvict` 增加窗口累计次数并克隆记录。开启逐条驱逐持久化时，它补上窗口起点和当前终点，调用 `try_send`；成功入队的记录只合并到 `stmtEvicted.other`，未开启、通道满或断开的记录还会合并到 `otherForPersist`，从而在窗口落盘时兜底且避免成功入队记录被重复聚合持久化。`evictedLogLoop` 收集通道记录，达到 64 条或经过 100 毫秒就调用 `logEvicted`；停止时先排空通道再刷最后一批。

轮转线程每秒获得一次 tick，并以最多 100 毫秒的接收等待检查停止标志。`rotateWindow` 仅在 `elapsed > RefreshInterval` 时替换窗口；锁外持久化旧快照。`Close` 以原子交换保证幂等，先阻止新写入并停止、join 后台线程，再替换当前窗口、持久化非空记录并 `sync`。`Drop` 仅停止和 join 线程，不执行当前窗口持久化，因此需要持久保证的调用方应显式 `Close`。

## 数据与状态

运行时选项使用原子变量，布尔值和整数读取采用 Acquire、写入采用 Release；`closed` 的首次关闭使用 AcqRel。结构性状态由两层锁保护：`StmtSummaryInner.window` 的 `Mutex<stmtWindow>` 保护当前窗口、LRU 成员和顺序、驱逐聚合；每个 `LockedRecord = Arc<Mutex<StmtRecord>>` 保护单条记录的可变统计字段。代码遵循先窗口锁、再记录锁的顺序，`Add` 在确定记录后释放窗口锁才合并，以缩短全局临界区。

`stmtEvicted.keys` 按键字节去重，因此 `Evicted` 的数量是当前窗口被驱逐的不同摘要键数；`evictedCount` 则是实际 LRU 淘汰事件总数，两者语义不同。`Clear` 同时清空 LRU、驱逐集合和次数；窗口轮转通过新建 `stmtWindow` 达到同样的重置效果。`ClearInternal` 只删除当前记录最终标记为 `IsInternal` 的键；混合了内部与外部执行的记录不会删除，这由 `stmtsummary_test.rs::go_merge_38_internal_cleanup_keeps_mixed_record_and_capacity` 固化。

按用户分组关闭时，用户不进入键，但 `StmtRecord` 仍可聚合授权用户集合；开启后，同 digest 的不同用户形成不同 LRU 项。`SetGroupByUser` 在窗口锁内切换并清空旧窗口，防止两种键语义混存。`SetMaxStmtCount` 将小于 1 的值钳制为 1，缩容时主动从 LRU 尾部弹出并产生正常驱逐统计，然后调整 cache 容量。

全局状态分为“已保存的实例”和“当前是否激活持久模式”两部分。多数代理使用 `activeGlobal`，只在激活标志为真时读取 v2；`SetMaxSQLLength`、`SetPersistEvicted`、`SetGroupByUser` 则会直接查看已保存实例，后者还总是先同步 v1。这个差异是当前代码的真实兼容策略，扩展代理时不能默认所有函数选择后端的规则一致。

## 依赖与调用关系

上游调用关系：

- `cmd/tidb-server/main.rs::setupStmtSummary → stmtsummary.rs::Setup → NewStmtSummary → StmtSummary::create`，建立全局生产实例；`closeStmtSummary → Close → StmtSummary::Close` 做进程收尾。
- `pkg/session/runtime/scan_adapter_runtime.rs → stmtsummary.rs::Add → StmtSummary::Add/StmtSummaryByDigestMap.AddStatement`，是目前搜索到的 Rust 生产写入入口。
- 系统变量或配置接线可调用本文件的 `SetEnabled`、`SetEnableInternalQuery`、`SetRefreshInterval`、`SetMaxStmtCount`、`SetMaxSQLLength`、`SetPersistEvicted`、`SetGroupByUser`；这些代理再选择 v1/v2。
- `pkg/util/stmtsummary/v2/reader.rs::MemReader::Rows → MemorySummarySource::currentWindowSnapshot`，读取 v2 当前窗口并按权限、digest 和时间过滤。

下游依赖关系：

- `task_stmtsummary::{StmtExecInfo, StmtDigestKeyPool}` 提供输入和摘要键构造；`StmtSummaryByDigestMap` 是 v1 回退后端。
- `crate::{NewStmtRecord, StmtRecord}` 负责初始化和合并统计；`marshalStmtRecord`、`marshalEvictedStmtRecord` 负责 JSON 编码；`setGlobalMaxSQLLength` 同步记录层的 SQL 截断上限。
- `lru::LruCache` 实现容量与热度顺序，`crossbeam_channel` 实现有界非阻塞驱逐队列及 tick，`parking_lot::Mutex` 保护窗口、文件和记录。
- `chrono` 与 `chrono-tz` 将 `SystemTime` 转成兼容的 timestamp Datum；文件持久化使用标准库 `OpenOptions` 的 create+append 语义。

RustCodeGraph 对目标文件的索引显示 115 个符号；精确查询确认 `rotateLoop`、`evictedLogLoop`、`Setup` 及 Rust 服务端 `setupStmtSummary` 的位置。图的通用 `explore` 对 `Add`/`Setup` 存在大量跨仓同名结果，因此本文只采用路径消歧后的节点和源码直连调用，不将无关同名边计入本模块调用图。

## 错误处理与边界

`NewStmtSummary` 对空文件名返回字符串错误，文件创建/打开错误也转成字符串；它在启动线程前完成存储创建，因此失败路径不遗留后台线程。`Setup` 失败时将持久模式关闭并返回带“falling back to v1”上下文的错误；`stmtsummary_test.rs::go_merge_37_setup_failure_reports_fallback_and_keeps_v1_available` 和 `go_merge_38_failed_setup_keeps_previous_instance_and_falls_back` 分别验证 v1 可用及旧实例不被关闭。

持久化是 best-effort：`fileStmtStorage::persist`、`logEvicted` 忽略每条 `writeRecord` 的错误，`Close` 也忽略 `sync` 错误；JSON 序列化错误仅在 `writeRecord` 内转成 `io::Error`，但上层不传播。文档和新功能不能宣称运行期落盘失败会反馈给调用者。通道满时逐条驱逐日志被丢弃并增加 `evictedDropped`，但记录仍进入 `otherForPersist`，在窗口结束时以聚合行补偿；通道断开也走聚合兜底。当前 Rust 的 30 秒报告 tick 只更新局部 `last_report`，没有像 Go 版本一样真正记录警告或指标。

`rotateWindow` 使用 `duration_since(...).unwrap_or_default()`，系统时钟回拨被视为零经过时间，不触发轮转；到达恰好等于刷新间隔时也不轮转，必须严格超过。`unixSeconds` 对 UNIX epoch 之前的时间返回 0。容量始终至少为 1；刷新间隔也至少为 1 秒。`Evicted` 在没有不同驱逐键时返回 `None`，即使其他计数接口仍存在。

`SetHistorySize` 在 v2 激活时是明确的 no-op，因为 v2 没有内存 history size 概念；未激活时才转发 v1。`Config` 中的文件大小、天数和备份数目前未使用，文件实现也不滚动。`persist` 只有在快照 `records` 非空时才被调用，因此如果窗口只剩需要兜底的驱逐聚合而没有 LRU 记录，该聚合不会由 `rotateWindow`/`Close` 单独触发落盘；这是按当前条件分支得出的边界，修改时应补专门回归测试。

## 并发与资源生命周期

生产实例拥有两个线程：驱逐日志线程总会启动，轮转线程仅在 `create(..., rotate=true)` 时启动。测试构造器关闭轮转并把刷新间隔设为一年，但仍启动驱逐线程。所有线程共享 `Arc<StmtSummaryInner>`；`workers` 自身由 Mutex 保护，便于 `Close` 或 `Drop` 排空并 join。

`Add` 在窗口锁内二次检查 `closed`，与 `Close` 的原子关闭和窗口替换配合，防止关闭开始后插入新记录。`ClearInternal` 持窗口锁逐条获取记录锁；`stmtsummary_test.rs::go_merge_38_internal_cleanup_waits_for_record_update` 验证清理会等待进行中的记录更新，`go_merge_38_evicted_and_cleanup_are_safe_during_updates` 验证并发检查/清理与大量写入不会突破容量。`evicted_rows_are_safe_during_concurrent_window_rotation` 验证读取驱逐行和窗口轮转并发安全。

逐条日志队列的资源上界由 `evictedLogChanCap=1024` 给出，batch 上界通常为 64；`try_send` 保证 LRU 淘汰不等待文件 I/O。线程收到 `stop` 后排空队列并刷批次，随后 `Close` 才持久化当前窗口和 fsync，确保成功入队的逐条记录先完成。不过，文件写由 `fileStmtStorage.file` 的 Mutex 串行化，窗口持久化和逐条驱逐写不会交错破坏单行写入。

显式 `Close` 是完整资源生命周期的一部分：它幂等、停止线程、刷新窗口并同步文件。仅依赖 `Drop` 会 join 线程但不刷新当前窗口；全局 `Setup` 替换实例时和全局 `Close` 卸载实例时都显式调用了 `Close`。

## 与 Go 版本的对应关系

Rust 的默认值、运行时选项、LRU 键维度、缩容驱逐、内部记录清理、按用户分组切换清窗、逐条驱逐的非阻塞队列与批处理、`other`/`otherForPersist` 去重语义、v1/v2 全局代理，均直接对应 `pkg/util/stmtsummary/v2/stmtsummary.go` 的同名结构和函数。Rust 测试 `test_stmt_window`、`test_stmt_summary`、`test_stmt_summary_persist_evicted`、`test_stmt_summary_persist_evicted_does_not_persist_logged_records_as_aggregate`、`test_stmt_summary_group_by_user`、`test_window_evicted_count_reset_on_rotate`、`test_stmt_summary_flush`、`test_default_config` 与 Go 测试的同名场景相互对照。

当前仍有以下可观察差异：

- Go 用 `newStmtLogStorage` 接入完整日志配置和滚动；Rust 的 `fileStmtStorage` 只是一个追加文件，忽略 `FileMaxSize`、`FileMaxDays`、`FileMaxBackups`。
- Go 窗口轮转的持久化另起 goroutine，并由 `closeWg` 等待；Rust `rotateWindow` 在线程自身同步调用 `persist`，实现更简单但慢磁盘会推迟后续轮转检查。
- Go `updateMetrics` 更新当前窗口记录数和驱逐数，并在通道满时记录丢弃指标、周期性告警；本 Rust 文件没有对应指标写入，报告 tick 也没有外部输出。
- Go 对 `storage.sync` 失败写错误日志；Rust 忽略 sync 与逐条写错误。两者公开 setter 的错误类型也不同：Go 返回 `error`，Rust 使用 `Result<T, String>`。
- Go `cloneRecordForLog` 显式深拷贝两个可变 map；Rust 依赖 `StmtRecord: Clone`。是否等价取决于 `record.rs` 中集合字段的值语义；当前 Rust 容器类型的 `Clone` 会复制其内容。
- Rust 全局状态用 `OnceLock<RwLock<Option<Arc<_>>>>` 加独立激活标志，而 Go 使用包级指针和全局配置开关。Rust 初始化失败会保留旧实例对象但将其停用；这是 `go_merge_38_failed_setup_keeps_previous_instance_and_falls_back` 明确验证的行为。

这些差异应作为迁移状态记录，不能因测试场景大体对齐就宣称实现完全等价。

## 扩展指南

新增摘要键维度时，应修改 `digestKey`，同时审查 Go 的 `StmtDigestKey.Init` 对应调用、`SetGroupByUser` 一类键策略切换是否必须清空窗口，并在 `stmtsummary_test.rs` 增加“同 digest 不同新维度”的容量和合并测试。键改变会直接影响内存量、命中率、驱逐率和历史兼容性。

新增动态选项时，应把字段放入 `StmtSummaryInner`，在 `create` 初始化，并明确全局代理是遵循 `activeGlobal` 还是对已保存 v2 实例也生效；后者目前只适用于少数 setter。若选项改变聚合键或记录语义，必须像 `SetGroupByUser` 一样在同一窗口锁临界区切换并清理，避免混合状态。

增强持久化时，优先扩展 `stmtStorage`/`fileStmtStorage`，补齐 `Config` 的滚动字段和错误观测；需要同步 `logger.rs` 的编码契约以及 `stmtsummary_test.rs` 的临时文件断言。改变 `persist` 触发条件时，应特别覆盖“所有普通记录均已驱逐、仅 `otherForPersist` 非空”的窗口。不能在持有窗口锁时做阻塞 I/O。

调整并发结构时，必须保持窗口/LRU 锁与记录锁的顺序，继续保证 `Add` 不受驱逐文件 I/O 阻塞，并维护 `Close` 的“停止接收新写入 → 排空逐条队列 → 刷当前窗口 → sync”顺序。相关回归集中在 `pkg/util/stmtsummary/v2/stmtsummary_test.rs`；吞吐或锁竞争变化可同步 `stmtsummary_benchmark_test.rs`。Rust 单元测试应继续保留在独立测试文件，不嵌入本生产文件。

若补齐 Go 的 metrics、日志滚动或异步轮转，不应只为通过现有 Rust 测试做简化：需以 `stmtsummary.go` 的 `updateMetrics`、`newStmtLogStorage`、`rotate`/`closeWg` 为语义基线，并评估兼容性（日志格式与全局代理）、正确性（重复/遗漏持久化）和性能（热路径锁、通道容量、磁盘背压）。

## 验证依据

- RustCodeGraph：`status` 确认索引可用（目标目录已索引，`stmtsummary.rs` 为 115 个符号）；`files --filter pkg/util/stmtsummary/v2` 核对同目录 Rust/Go/测试集合；`query`/`node` 精确定位并读取 `StmtSummary`、`Setup`、`rotateLoop`、`evictedLogLoop` 及完整目标源码；`query setupStmtSummary` 定位 Rust 服务入口。对 `callers`/`callees` 的精确命令曾执行但在当前 CLI/索引上超时无输出，因此关键边均另由路径限定的调用点源码核验，没有采用模糊同名结果。
- 生产源码：`pkg/util/stmtsummary/v2/stmtsummary.rs`；模块边界 `pkg/util/stmtsummary/v2/lib.rs`；记录、编码与读取接口分别核对 `record.rs`、`logger.rs`、`reader.rs`；生产调用点核对 `cmd/tidb-server/main.rs::setupStmtSummary`、`closeStmtSummary` 和 `pkg/session/runtime/scan_adapter_runtime.rs` 的全局 `Add` 调用。
- crate 配置：`pkg/util/stmtsummary/v2/Cargo.toml`，用于确认 crate 名称、入口、依赖、测试装配和 Go 移植元数据。
- Go 对照：`pkg/util/stmtsummary/v2/stmtsummary.go`，逐项核对默认值、公开方法、锁顺序、LRU 驱逐、轮转、异步驱逐日志、全局代理和迁移差异。
- 测试证据：`pkg/util/stmtsummary/v2/stmtsummary_test.rs` 与 `stmtsummary_benchmark_test.rs`；Go 对照测试为 `stmtsummary_test.go` 与 `stmtsummary_benchmark_test.go`。重点场景包括容量与 LRU、轮转重置、逐条驱逐无重复、按用户分组、关闭刷盘、默认配置、初始化失败回退及并发清理/轮转。
- 人工复核结论：本文件存在的原因是把语句完成事件变成有界、可读取、可持久化的时间窗口，并在 v2 不可用时维持 v1 服务；安全扩展的核心约束是键语义一致、锁顺序固定、热路径非阻塞、驱逐不重不漏、显式关闭完成持久化。本文没有把尚未接入的文件滚动、指标或错误上报写成已支持。
- 任务要求的结构验证命令应在文档落盘后执行；本节不以 Cargo 测试作为证据，因为任务明确是纯文档分析且禁止运行 Cargo。
