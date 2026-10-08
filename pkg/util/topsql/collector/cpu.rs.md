# `pkg/util/topsql/collector/cpu.rs`

## 文件定位

本文件是 `astersql-util-topsql-collector` crate 的 CPU 采集实现。crate 入口 `pkg/util/topsql/collector/lib.rs` 将私有 `cpu` 模块的公开项全部再导出；`pkg/util/topsql/collector/Cargo.toml` 表明它直接依赖进程级 `cpuprofile`、TopSQL 开关 `topsql_state`、pprof protobuf、`crossbeam-channel`、`hex` 和日志库。

它处在两条路径的交点：上游由 `pkg/util/topsql/topsql.rs` 的 `AttachAndRegisterSQLInfo`、`AttachSQLAndPlanInfo`、`AttachAndRegisterProcessInfo` 写入采样标签；下游由 `pkg/util/topsql/reporter/reporter.rs::NewRemoteTopSQLReporter` 创建 `SQLCPUCollector`，把解析出的 `SQLCPUTimeRecord` 送入 reporter。进程 CPU 数据则经可选的 `ProcessCPUTimeUpdater` 回调交给绑定方。

## 核心职责

- `SQLCPUCollector` 管理一个后台线程，根据 `topsql_state::TopSQLEnabled()` 动态向全局 `cpuprofile` 注册或注销消费者，接收每个剖析区间完成后的 `ProfileData`。
- `handleProfileData` 解码 pprof protobuf，同一份 profile 分两路处理：按 `sql_digest`/`plan_digest` 聚合 TopSQL CPU 毫秒记录；按 `sql_global_uid` 聚合连接当前 SQL 的进程 CPU 纳秒数。
- `sqlStats::tune` 把只有 SQL 标签、尚无计划标签的优化阶段耗时归入空 plan digest，并保持 Go 的无计划、单计划和多计划分支语义。
- `ProfileContext` 与三个 `CtxWith*` 函数提供 Rust 侧标签载体。它们是 Go `context.Context`/`runtime/pprof` 边界的替代接口，不等同于 Go 的 goroutine-local 标签实现。

## 主要符号

- `ProcessCPUTimeUpdater: Send + Sync`：以 `(connID, sqlID, Duration)` 回写进程 CPU 时间。该 trait 用于切断 collector 与具体会话/进程统计实现之间的依赖环。
- `Collector: Send + Sync`：接收一个采样区间内形成的 `Vec<SQLCPUTimeRecord>`。
- `SQLCPUTimeRecord`：公开输出记录，包含二进制 SQL digest、可为空的二进制 plan digest，以及截断到毫秒的 `u32` CPU 时间。
- `SQLCPUCollector`：持有 collector、可选 updater、取消发送端、后台线程句柄、启动状态、跨线程注册状态和采集检查间隔。`NewSQLCPUCollector` 默认间隔为 1 秒；`Start`、`Stop` 和 `Drop` 管理生命周期。
- `collectSQLCPULoop`、`doRegister`、`doUnregister`：后台循环及全局 profiler 消费者的幂等注册管理。
- `handleProfileData`：单份 profile 的解码与双路分发入口。
- `parseCPUProfileBySQLLabels`、`createSQLStats`、`sqlStats::tune`：SQL/计划维度的聚合、digest 解码和优化阶段耗时校正。
- `parseCPUProfileForProcess`、`processCPUTimeRecord`：连接/SQL ID 维度的聚合。
- `sampleLabelValues`、`durationFromNanos`：安全读取 pprof 字符串表标签，以及把负纳秒压为零后构造 `Duration`。
- `ProfileContext`、`current_thread_profile_labels`、`CtxWithSQLDigest`、`CtxWithSQLAndPlanDigest`、`CtxWithProcessInfo`：标签保存、查询与线程本地快照 API。

仅 `ProcessCPUTimeUpdater`、`Collector`、`SQLCPUTimeRecord`、`SQLCPUCollector`、构造/配置方法和 ProfileContext 相关 API 对 crate 使用者公开；解析、聚合与注册函数保持模块私有。`set_collect_interval`、`is_started` 也是私有方法，主要供同模块独立测试使用。

## 执行流程

1. `reporter.rs::NewRemoteTopSQLReporter` 用 `WeakCPUCollector` 适配器调用 `NewSQLCPUCollector`。弱引用避免 collector 与 reporter 形成强引用环；reporter 的 `Start`/`Close` 分别启动和停止它。
2. `SQLCPUCollector::Start` 以 `started` 防止重复启动，建立容量为 1 的取消通道，克隆回调与原子注册状态并启动 `collectSQLCPULoop`。
3. 循环每轮先读取 TopSQL 开关：开启时调用 `doRegister`，关闭时调用 `doUnregister`。随后在取消信号、周期 tick 和 profile 数据三者之间选择；tick 用来重新检查开关，profile 数据交给 `handleProfileData`。
4. `handleProfileData` 对上游已标错的 `ProfileData` 直接返回；protobuf 解码失败时记录错误并返回。成功后总会调用 `Collector::Collect`，即使 SQL 聚合结果为空。
5. SQL 聚合选用 `profile.sample_type` 的最后一个值类型。无 `sql_digest` 的样本被忽略；每个 SQL 累加全部样本值，同时按 plan digest 累加。`tune` 后十六进制 digest 被解码为字节，CPU 纳秒除以 `1_000_000` 形成毫秒记录。
6. 进程聚合倒序扫描样本，将 `sql_global_uid` 按 `connID_sqlID` 拆分。每个连接只保留数值最大的 sqlID，并累加该 ID 的样本值；存在结果时要求 updater 已设置，再逐项回调。
7. `Stop` 发送取消信号并 `join` 后台线程。循环无论正常退出还是被捕获的 panic，都会执行 `doUnregister`；`Drop` 再调用幂等的 `Stop`，防止对象析构时遗留消费者。

标签路径独立于上述消费循环：两个 SQL/计划 helper 只更新传入的 `ProfileContext`；`CtxWithProcessInfo` 还把完整标签表复制到 `CURRENT_THREAD_PROFILE_LABELS`。`topsql.rs` 的 Attach API 负责在业务边界调用这些 helper。

## 数据与状态

- 三个标签键固定为 `sql_digest`、`plan_digest` 和 `sql_global_uid`，与 `cpu.go` 一致。digest 输入是十六进制字符串，公开记录保存解码后的字节；空 plan digest 表示无计划或优化阶段样本。
- `sqlStats.total` 是一个 SQL 的全部样本值，`plans` 是带 plan 标签部分。无 plan 时全部总量写入空键；单 plan 时该 plan 被校正为总量；多 plan 时仅正的 `total - planTotal` 余量写入空键。
- profile 的最后一个 sample type 被视为 CPU 纳秒。空 profile 不进入样本循环，可返回空结果；只要存在样本，样本 `value` 必须包含对应索引。
- `processCPUTimeRecord` 的状态按连接 ID 分区，保留最新的 sqlID 与累计纳秒数。输出来自 `HashMap`，SQL 记录与进程记录的顺序均不稳定，调用者和测试不应依赖顺序。
- `started` 仅由拥有 `&mut SQLCPUCollector` 的控制线程访问；`registered` 是 `Arc<AtomicBool>`，供控制对象和 worker 共享。`Ordering::SeqCst` 保证注册状态切换具有全序语义。
- `ProfileContext.labels` 是普通 `HashMap`；`CURRENT_THREAD_PROFILE_LABELS` 是每线程独立的 `RefCell<HashMap<...>>`。返回当前标签时会克隆快照，不暴露内部可变借用。

## 依赖与调用关系

主要上游调用边：

- `reporter.rs::NewRemoteTopSQLReporter -> collector::NewSQLCPUCollector`；`RemoteTopSQLReporter::Start -> SQLCPUCollector::Start`；`RemoteTopSQLReporter::Close -> SQLCPUCollector::Stop`。
- `RemoteTopSQLReporter::BindProcessCPUTimeUpdater -> SQLCPUCollector::SetProcessCPUUpdater`。
- `topsql.rs::AttachAndRegisterSQLInfo -> CtxWithSQLDigest`，`AttachSQLAndPlanInfo -> CtxWithSQLAndPlanDigest`，`AttachAndRegisterProcessInfo -> CtxWithProcessInfo`。

主要下游调用边：

- `collectSQLCPULoop -> topsql_state::TopSQLEnabled`，以及 `cpuprofile::Register`/`Unregister`。`cpuprofile::ProfileConsumer` 是向 `Arc<ProfileData>` 非阻塞投递的发送端；本文件保留其接收端。
- `handleProfileData -> pprof::protos::Profile::decode -> parseCPUProfileBySQLLabels -> Collector::Collect`，随后 `parseCPUProfileForProcess -> ProcessCPUTimeUpdater::UpdateProcessCPUTime`。
- reporter 的 `WeakCPUCollector::Collect` 把 CPU 记录转交 `RemoteTopSQLReporter::Collect`；后者使用有界通道非阻塞入队，满载时计数并丢弃，实际 TopN/上报处理位于 reporter 而非本文件。

Cargo 边界没有 feature 条件；测试依赖 `serial_test` 和 `testsetup`。本文件只有两个 `#[cfg(test)]` 模块声明，测试逻辑分别保存在独立的 `main_test.rs` 与 `migration_aster_unit_test.rs`，未与生产实现混写。

## 错误处理与边界

- `ProfileData.Error` 非空和 protobuf 解码失败都不会调用任何回调；后者会记录错误日志。
- SQL 或 plan digest 不是合法十六进制时，仅跳过对应输出并记录日志：坏 SQL digest 会丢弃该 SQL 的全部 plan 记录，坏 plan digest 只丢弃该 plan。
- `sampleLabelValues` 对负索引、越界 string table 索引或键不匹配返回无值，不会 panic。
- 空 profile 可安全返回空集合；但有样本而 `sample_type` 为空、或样本 `value` 不足时会索引越界。格式不含下划线的 `sql_global_uid` 也会在访问第二段时 panic。后台循环用 `catch_unwind` 捕获这些 panic、注销消费者并记录错误，但直接调用私有解析函数时没有这一保护。
- `sql_global_uid` 两段的十进制解析失败会按 `0` 处理，与 Go 忽略 `ParseUint` 错误一致。额外的下划线段被忽略。
- profile 含进程记录却未调用 `SetProcessCPUUpdater` 时，`handleProfileData` 会因 `expect` panic；正常集成必须在可能产生 `sql_global_uid` 样本前绑定 updater。
- 负的进程纳秒通过 `durationFromNanos` 变为零。SQL 毫秒由整数除法截断，再转为 `u32`；异常负值或超范围值没有显式拒绝，扩展时应保持与 Go 转换语义兼容或同步修改两侧契约。
- 取消通道发送失败被忽略，worker panic 的 join 失败只记录日志。`Start`/`Stop` 明确不是并发安全 API；调用方必须串行持有可变 collector。

## 并发与资源生命周期

每个已启动的 `SQLCPUCollector` 最多拥有一个 worker。`started` 守卫使重复 `Start` 不新建线程，重复 `Stop` 不重复取消；取消通道容量为 1，足以表达一次终止。worker 的 profile 通道容量同样为 1，与 `cpuprofile` 的非阻塞投递策略共同形成背压：消费者落后时上游可以丢弃区间，而不会阻塞全局 profiler。

注册状态通过 `AtomicBool::swap` 实现幂等。TopSQL 开关变化最迟在下一个 tick 或其他 select 事件后的下一轮循环生效。退出时先注销再结束；`Stop` 等待 join，因此返回后本 collector 不再注册。`Drop` 是最后一道资源清理保证。reporter 使用 `WeakCPUCollector`，避免 reporter 持有 collector、collector 回调又强持有 reporter 的生命周期环。

`Collector` 和 `ProcessCPUTimeUpdater` 必须 `Send + Sync`，因为回调发生在 worker。它们的具体实现负责自身同步；本文件不会序列化其他调用来源。线程本地标签只对调用 `CtxWithProcessInfo` 的当前 OS 线程生效，不能自动跨线程传播。

## 与 Go 版本的对应关系

`pkg/util/topsql/collector/cpu.go` 是逐项语义基线：公开接口、三种标签、启动守卫、按开关注册/注销、取最后 sample type、`sqlStats.tune` 四类结果、倒序处理连接最新 sqlID，以及三个 context helper 均有 Rust 对应物。

Rust 的结构性替换包括：`Arc<dyn Trait>` 代替 Go interface，`crossbeam_channel` 代替 channel/context/ticker，`JoinHandle` 代替 `WaitGroup`，`catch_unwind` 代替 `util.Recover`，`prost` 解码的 `pprof::protos::Profile` 代替 `google/pprof/profile.ParseData`。Go 的 `defer` 清理在 Rust 循环闭包之后显式执行，`Drop` 又补充了 RAII 清理。

标签实现存在重要差异：Go helper 使用 `pprof.WithLabels`，且 `CtxWithProcessInfo` 调用 `pprof.SetGoroutineLabels`；Rust 使用自定义 `ProfileContext` 和 OS 线程本地快照。`pkg/util/topsql/topsql_test.rs::test_top_sql_cpu_profile` 明确记录 pprof-rs 不承载 Go goroutine labels，因此测试在 collector 边界注入记录。不能据此宣称 Rust 已具备与 Go 完全相同的运行时标签传播能力。

Rust 迁移测试比 Go `main_test.go` 额外固定了非法 protobuf 不分发、空 profile、安全 digest 解码、最新 sqlID 累加、启停幂等和 ProfileContext 标签覆盖等边界；Go 测试主要验证真实 profiler 开关切换、进程 updater 恢复和 `tune` 的单/多计划分支。

## 扩展指南

- 新增采样标签时，应同时修改标签常量、ProfileContext helper、对应聚合函数及 `topsql.rs` 的业务接入点，并核对 `cpu.go` 是否需要同步；不要只在解析端增加理想化字段。
- 调整 SQL/plan 聚合规则时，优先修改 `parseCPUProfileBySQLLabels`、`sqlStats::tune` 或 `createSQLStats`，并在独立的 `migration_aster_unit_test.rs` 增加无计划、单计划、多计划、非法 digest、无标签及数值边界测试。
- 调整连接最新 SQL 规则时，修改 `parseCPUProfileForProcess`，测试同连接乱序、多连接、重复 sqlID、畸形 UID 和负值。若要把畸形输入从 panic 改为跳过，须明确评估与 Go 当前索引行为的兼容差异。
- 改动启停、开关或注册策略时，保持 `Start`/`Stop` 幂等、Stop 返回前完成注销、panic 后清理和容量为 1 的非阻塞消费契约；同步扩展 `main_test.rs` 的真实 profiler 测试。不要把测试写回 `cpu.rs`。
- 若要补齐 Go goroutine label 等价能力，接入点是 `ProfileContext`、三个 `CtxWith*` helper 以及 `topsql.rs` 的 `set_goroutine_labels`，并需要真实采样的端到端证据；当前线程本地快照不能作为等价证明。
- 新增公开 API 时确认 `lib.rs` 的全量再导出是否合适；新增依赖时更新 `Cargo.toml`。输出排序若成为协议要求，应在明确的边界排序，避免无意依赖 `HashMap` 迭代顺序并承担额外开销。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/topsql/collector` 确认目标及测试文件被索引；`query CpuCollector` 定位 Rust/Go 的 `SQLCPUCollector` 与 `NewSQLCPUCollector`；`node --file pkg/util/topsql/collector/cpu.rs` 核对完整 464 行源码及 46 个符号。图的 callers/callees 查询未返回可用文本，故调用边再由下列直接源码引用核验。
- 生产源码：`pkg/util/topsql/collector/cpu.rs`、`pkg/util/topsql/collector/lib.rs`、`pkg/util/topsql/reporter/reporter.rs`、`pkg/util/topsql/topsql.rs`、`pkg/util/cpuprofile/cpuprofile.rs`、`pkg/util/topsql/state/state.rs`。
- crate 配置：`pkg/util/topsql/collector/Cargo.toml`。
- Go 对照：`pkg/util/topsql/collector/cpu.go`；Go 测试：`pkg/util/topsql/collector/main_test.go`。
- 独立 Rust 测试：`pkg/util/topsql/collector/main_test.rs`、`pkg/util/topsql/collector/migration_aster_unit_test.rs`；上层接线限制证据：`pkg/util/topsql/topsql_test.rs::test_top_sql_cpu_profile`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付时运行任务文件指定的 11 章节结构检查，并人工复核本文能回答文件存在原因、运行流程、边界、真实接线及安全扩展位置。
