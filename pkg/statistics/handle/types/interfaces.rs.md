# `pkg/statistics/handle/types/interfaces.rs`

## 文件定位

该文件是 `astersql-statistics-handle-types` crate 的核心接口文件，由同目录 `lib.rs` 私有声明 `mod interfaces` 后整体 `pub use interfaces::*`，因此这里的公开结构、类型别名与 trait 构成 statistics handle 各子模块之间的稳定 Rust 契约。`Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `pkg/statistics/handle/types`，直接对应同路径 Go 文件 `interfaces.go`。

它处于 statistics handle 的“契约层”，不保存具体统计数据，也不执行 GC、ANALYZE、缓存刷新或 DDL 处理算法。具体实现分散在 `pkg/statistics/handle/cache/`、`storage/`、`syncload/`、`history/`、`usage/`、`autoanalyze/` 等 crate；当前 Rust 接线尚未完全收敛到本文件的全部 trait。例如 `pkg/statistics/handle/cache/statscache.rs` 已实现 `stats_types::StatsCache for StatsCacheImpl`，而 storage 与 syncload 目前仍分别保留自己的局部 `StatsHandler`/`StatsHandle` 抽象。

## 核心职责

1. 以细粒度 trait 划分统计子系统能力：`StatsGC`、`IndexUsage`/`StatsUsage`、`StatsHistory`、`StatsAnalyze`、`StatsCache`、`StatsLock`、`StatsReadWriter`、`StatsSyncLoad`、`StatsGlobal` 和 `DDL`。
2. 用组合 trait `StatsHandle` 汇总 handle 对调用方公开的能力，同时继承 util crate 提供的 `Pool`、`AutoAnalyzeProcIDGenerator`、`LeaseGetter` 和 `TableInfoGetter`。
3. 定义跨 crate 传递的数据形状，如 `CacheUpdate`、`NeededItemTask`、`GlobalStatsInfo`、`PartitionStatisticLoadTask` 和自动 ANALYZE 队列的 JSON 视图。
4. 重导出调用方常用的真实依赖类型，避免各子模块重复绑定底层 crate；典型例子是 `InfoSchema`、`StatementContext`、`StatsLoadResult`、statistics util 的 session/SQL 抽象以及 `JSONTable`。
5. 通过统一的 `Error(String)`/`Result<T>` 声明接口层错误边界，但不在本文件中执行错误分类、重试或日志记录。

## 主要符号

- `Error` 与 `Result<T>`：最小错误包装。`Display` 原样输出内部字符串，并实现 `std::error::Error`；它没有错误码、来源链或自动转换实现。
- `StatsGC`：声明无用统计和历史统计的清理，以及表/分区统计的软删除或硬删除入口。
- `IndexUsage` 与 `StatsUsage`：前者控制索引用量收集 worker，后者在其上增加列/谓词用量、session collector、delta 与用量落盘。`NewSessionStatsItem` 返回 `Box<dyn Any + Send>`，是为解除具体 collector 类型耦合保留的类型擦除边界。
- `ColStatsTimeInfo`：以两个 `Option<Time>` 表示最近使用和最近 ANALYZE 时间；`None` 对应 Go 的 nil 指针语义。
- `StatsHistory`：检查历史统计开关、记录 meta，并把指定物理表或分区的统计写入历史存储。
- `PriorityQueueSnapshot`、`AnalysisJobJSON`、`IndicatorsJSON`：自动 ANALYZE 优先队列的可序列化快照。显式 serde 字段名保持 Go JSON tag，例如 `current_jobs`、`partition_index_ids` 和 `change_percentage`。
- `StatsAnalyze`：覆盖 ANALYZE job 的插入、启动、进度、完成和损坏记录清理，以及自动分析、版本匹配与优先队列生命周期。
- `CacheUpdate` 与 `UpdateOptions`：批量携带缓存新增/替换、删除 ID 及版本游标控制；`SkipMoveForward` 默认值为 `false`。
- `StatsCache`：提供缓存刷新、查询、写入、替换、容量控制、健康指标、驱逐和异步写入屏障。`Arc<Table>` 让读者共享不可变表统计，trait 的 `Send + Sync + Any` 允许跨线程共享并支持运行时类型能力。
- `StatsLockTable` 与 `StatsLock`：描述表的全限定名和分区 ID 到名称的映射，并提供表/分区锁定、解锁、批量查询接口；返回的 `String` 用于承载被跳过对象的信息。
- `PartitionStatisticLoadTask`：一项分区 JSON 导入任务；`JSONTable: Option<Box<JSONTable>>` 保留 Go `*JSONTable` 可为 nil 的语义，`PhysicalID` 标识表或分区。
- `PersistFunc<'a>`：`PersistStatsBySnapshot` 使用的回调 trait object，接收执行上下文、可空 JSON 和物理 ID，并要求 `Send + Sync`。
- `MetaUpdate`：统计 meta 批量写入的单项，包含 `PhysicalID`、`Count` 和 `ModifyCount`。
- `StatsReadWriter`：定义存储读取、ANALYZE/meta 写入、JSON 导入导出、历史快照回退结果以及并发分区导入的完整存储边界。
- `NeededItemTask` 与 `StatsSyncLoad`：前者携带截止时间、结果 sender、加载项和重试次数；后者声明请求发送、语句级等待、队列追加和 worker 单步处理。
- `GlobalStatsInfo`、`AnalyzeOptions` 与 `StatsGlobal`：描述列/索引直方图 ID、统计版本和 ANALYZE 选项，并声明分区统计合并到全局统计的入口。
- `DDL`：消费 `SchemaChangeEvent` 并暴露 DDL 事件 receiver。
- `StatsHandle`：组合除 `StatsSyncLoad` 外的各能力 trait，并补充“缓存不存在时允许构造 pseudo”的 `GetPhysicalTableStats` 与“只返回非 pseudo 缓存项”的 `GetNonPseudoPhysicalTableStats`。

## 执行流程

本文件没有可独立运行的主流程；它约束实现者和调用者应如何拼接以下流程。

1. 查询规划或统计读取通过 `StatsHandle::GetPhysicalTableStats` / `GetNonPseudoPhysicalTableStats` 取得 `Arc<Table>`；缓存实现可经 `StatsCache::Get`、`Put`、`UpdateStatsCache` 和 `Update` 管理内容。`WaitForAsyncUpdates` 是写后读需要可见性时的显式屏障。
2. ANALYZE 路径用 `StatsAnalyze` 维护 job 生命周期，再由 `StatsReadWriter::SaveAnalyzeResultToStorage` 和 `SaveMetaToStorage` 持久化结果；`AnalyzeVersionMatchesForTable` 判断请求版本是否已满足。
3. JSON 导出路径从 `TableStatsToJSON`、`DumpStatsToJSON` 或快照方法产生 `JSONTable`。`PersistStatsBySnapshot` 可逐分区回调，且回调必须接受 `None`；导入路径则由 `LoadStatsFromJSON*` 落盘，`LoadStatsFromJSONConcurrently` 从 `Receiver<PartitionStatisticLoadTask>` 消费任务。
4. 同步加载路径由 `SendLoadRequests` 把 `StatsLoadItem` 关联到语句上下文，worker 使用 `AppendNeededItem` / `HandleOneTask` 处理队列，调用方再通过 `SyncWaitStatsLoad` 等待结果或错误。
5. schema 变化通过 `DDL::DDLEventCh` 进入 handle，`HandleDDLEvent` 接受执行上下文、session context 和 `SchemaChangeEvent` 更新相关统计；具体事件分支不在本文件实现。
6. 分区表 ANALYZE 完成后，`StatsGlobal::MergePartitionStats2GlobalStatsByTableID` 根据 `GlobalStatsInfo` 和 `AnalyzeOptions` 合并全局统计。

RustCodeGraph 对精确的 `StatsHandle`、`LoadStatsFromJSONConcurrently`、`SendLoadRequests` 和 `HandleDDLEvent` trait 声明未返回静态 callers/callees。这与 trait 仅声明签名且动态分派、当前实现接线分散的事实一致，不能据此宣称这些流程已经由统一 `StatsHandle` 实现贯通。直接文本证据显示 `pkg/statistics/handle/cache/statscache.rs` 消费本 crate 的 `StatsHandle`/`StatsCache`，`pkg/statistics/handle/syncload/stats_syncload.rs` 则实现同名局部接口和对应加载流程。

## 数据与状态

本文件的状态全部由 DTO 或 trait 参数描述，本身没有全局变量、静态缓存或内部锁。

- 所有表统计共享值用 `Arc<Table>` 表达；`CacheUpdate::Updated` 可一次提交多个共享表统计，`Deleted` 携带应移除的物理 ID。
- 表、分区、列和索引的身份主要使用 `i64`；`TableItemID` 专门标识统计加载项，避免把复合身份压成裸整数。
- `PriorityQueueSnapshot` 是观测快照而非队列本体；字符串化的 `IndicatorsJSON` 面向 JSON 展示，不能用于数值运算。
- `NeededItemTask::ToTimeout` 是绝对 `SystemTime`，`Retry` 是调用方维护的重试计数；结果通过单生产者 `Sender<StatsLoadResult>` 返回。
- `PartitionStatisticLoadTask` 通过拥有的 `Box<JSONTable>` 跨任务转移 JSON 所有权；通道 receiver 被按值传入加载方法，意味着该次消费拥有接收端。
- `UpdateOptions::SkipMoveForward` 为真时禁止缓存版本游标前移；默认派生保证未显式配置时仍推进版本。
- `GlobalStatsInfo::IsIndex` 沿用 Go 整数判别：0 表示 `HistIDs` 是列 ID，否则其语义为索引 ID。接口层没有把它收窄成 enum，也不负责校验长度。

## 依赖与调用关系

crate 边界由 `pkg/statistics/handle/types/Cargo.toml` 明确：DDL 事件来自 `astersql-ddl-notifier`，schema 和 meta 模型来自 `astersql-infoschema` / `astersql-meta-model`，统计实体来自 `astersql-statistics`，JSON 来自 `astersql-statistics-util`，语句加载状态来自 `astersql-sessionctx-stmtctx`，执行/session/SQL 抽象主要来自 `astersql-statistics-handle-util` 与 `astersql-util-sqlexec`。`chrono-tz` 仅用于列用量加载时区，`serde` 用于队列快照 DTO。

上游消费证据包括：

- `pkg/statistics/handle/cache/statscache.rs`：泛型 `StatsCacheHandle` 为 `T: stats_types::StatsHandle` 提供桥接，并为 `StatsCacheImpl` 实现本文件的 `stats_types::StatsCache`。
- `pkg/statistics/handle/cache/statscache_test.rs`：使用 `Arc<dyn stats_types::StatsHandle>` 和 `stats_types::StatsCache::Replace` 验证 trait-object 形状。
- `pkg/statistics/handle/usage/predicatecolumn/predicate_column.rs`：消费本 crate 重导出的 SQL value/row 类型，说明该 crate 同时承担共享类型门面的职责。
- `pkg/statistics/handle/types/interfaces_test.rs`：直接验证公开 DTO、依赖类型和 callback/trait 签名。

相邻实现并不等于已经实现本文件 trait：`storage/stats_read_writer.rs` 的 `StatsReadWriter` 是具体结构且依赖自己的 `StatsHandler`；`syncload/stats_syncload.rs` 的 `StatsHandle` 是同步加载所需的局部最小接口。扩展或接线时必须用完整限定名区分这些同名符号。

## 错误处理与边界

- 所有可能失败的接口统一返回 `Result<T, Error>`；`Error` 只有字符串消息，因而实现层若需保留结构化来源，必须在转换为该类型前决定如何编码上下文。
- 若干 job 状态方法（`StartAnalyzeJob`、`UpdateAnalyzeJobProgress`、`FinishAnalyzeJob`）刻意不返回 `Result`，对应 Go 契约中“记录失败不应影响 ANALYZE 主结果”的 best-effort 行为；真正的日志与容错由实现负责。
- `StatsHistory::RecordHistoricalStatsMeta` 同样没有返回错误，接口层不保证写入成功可被调用者同步观察。
- `Option<Arc<Table>>`、`Option<Box<JSONTable>>`、`Option<&JSONTable>` 和 `Option<NeededItemTask>` 都是有意义的缺失状态，不应被无条件 `unwrap`。尤其 Go 明确规定 `PersistStatsBySnapshot` 可能以 nil JSON 调用回调，Rust 测试也验证了 `None` 合法。
- `Receiver` 断开、超时、worker 退出及 callback 失败如何映射为 `Error` 未在本文件规定，应由具体实现和独立测试覆盖。
- 接口层没有验证负容量、零并发、无效 `IsIndex`、重复 ID 或时间倒退；调用者不能把“签名可传入”误认为“实现必然接受”。

## 并发与资源生命周期

大部分 trait 要求 `Send + Sync`，允许实现被 `Arc<dyn Trait>` 跨 worker 共享。`StatsCache` 返回 `Arc<Table>`，避免从共享缓存取值时复制完整统计对象；具体内部同步策略由实现选择。

`IndexUsage::StartWorker`/`Close`、`StatsAnalyze::ClosePriorityQueue`/`Close`、`StatsCache::Close` 以及 `StatsSyncLoad::SubLoadWorker` 构成显式资源生命周期。`ClosePriorityQueue` 只关闭优先队列，不等同于停止 ANALYZE worker；调用方需要按实现规定分别关闭。`WaitForAsyncUpdates` 用于等待异步缓存写入可见，不代表关闭缓存。

通道所有权在签名中可见：`LoadStatsFromJSONConcurrently` 和 `SubLoadWorker` 获取 `Receiver` 的所有权，`DDL::DDLEventCh` 返回共享借用的 receiver，`NeededItemTask` 拥有结果 sender。标准库 `mpsc::Receiver` 本身不是 `Sync`；实现若从实现了 `Sync` 的 handle 返回其引用，必须通过适当的内部封装满足 Rust 并发约束。`PersistFunc` 明确要求 `Send + Sync`，允许并发持久化实现安全共享回调。

## 与 Go 版本的对应关系

Rust 文件按 `interfaces.go` 的能力分组和方法名进行直接移植，并保留非 snake case 名称（由 `lib.rs` 的 `#![allow(non_snake_case)]` 放行）。主要语义映射如下：

- Go 的 `*T`/可能为 nil 的返回值映射为 Rust 的 `Option<T>` 或 `Option<Arc<T>>`；共享统计表从裸指针变为 `Arc<Table>`。
- Go 的 `context.Context` 分别映射到 util sqlexec 的 `ExecutionContext` 或 statistics handle util 的 `StatsExecutionContext` 重导出；它们不是同一类型，扩展时需依照现有签名选择。
- Go 的 variadic 参数映射为切片，如 `tableIDs ...int64` 对应 `&[i64]`，`metaUpdates ...MetaUpdate` 对应 `&[MetaUpdate]`。
- Go channel 映射到 `std::sync::mpsc::{Sender, Receiver}`；方向性由字段和参数类型表达，而不是 Go 的 channel direction 语法。
- Go `map[int64]struct{}` 映射为 `HashSet<i64>`，普通 map 映射为 `HashMap`。
- Go `PersistFunc` 的 nil JSON 能力由 `Option<&JSONTable>` 保留；`interfaces_test.rs` 有直接回归证据。
- Go `Get(tableID) (*Table, bool)` 与 `GetNonPseudoPhysicalTableStats (*Table, bool)` 分别压缩为 `Option<Arc<Table>>`；不存在状态由 `None` 表达。
- Rust 增加了 `Send + Sync`、生命周期和所有权约束，并用 `Result<T, Error>` 取代 Go 的多返回值 error；但当前 `Error` 比 Go error 链更弱。

Go 文件的行为注释仍提供若干 Rust 声明未展开的契约，例如 GC 版本推进、安全时间窗、历史统计快照回退表列表，以及缓存异步写入屏障的用途。本文将这些作为对应关系说明，不视为当前 Rust 具体实现已经完整落地。

## 扩展指南

- 新增 handle 能力时，优先判断它属于现有细粒度 trait 还是确需新 trait；只有所有完整 handle 都必须提供时才把它加入 `StatsHandle` 超 trait，避免扩大 mock 和动态对象的实现负担。
- 修改 DTO 字段、JSON 名称或公开签名时，同步更新 `interfaces.go` 对照、`interfaces_test.rs` 的类型/序列化断言，以及所有依赖该 crate 的 Cargo consumer。JSON 字段变更还需评估外部监控与 API 兼容性。
- 新增或改变 `StatsCache` 行为，应在独立的 `pkg/statistics/handle/cache/*_test.rs` 覆盖异步可见性、版本推进和容量/驱逐语义；不要把实现测试内嵌进 `interfaces.rs`。
- 修改同步加载任务时，在 `pkg/statistics/handle/syncload/stats_syncload_test.rs` 覆盖超时、通道断开、重复请求、退出和重试；同时确认局部 `syncload::StatsHandle` 是否也需调整。
- 修改 JSON 导入导出签名时，必须保留 `PersistFunc` 的 `None` 边界，检查 BR/metautil 调用方，并在 storage 的独立测试中覆盖并发数、部分失败和 receiver 生命周期。
- 接线某个尚未实现的接口前，先搜索同名局部 trait/结构，避免误把 `types::StatsReadWriter` 与 `storage::StatsReadWriter`、`types::StatsHandle` 与 `syncload::StatsHandle` 混为一体。迁移应复用现有实现，不为了满足 trait 而创建空桩。
- 性能风险主要集中在复制大型 `Vec`/`HashMap` DTO、缓存全量 `Values()`、同步等待异步写入以及无界/高并发 JSON 导入；兼容性风险主要是 Go nil/variadic/channel 语义和 JSON 字段名漂移。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`interfaces.rs` 被识别为 457 行、126 个符号，并显示被 47 个文件使用。
- RustCodeGraph 源码查询：读取了 `pkg/statistics/handle/types/interfaces.rs` 全部 1–457 行，以及独立测试 `interfaces_test.rs` 全部 1–171 行。
- RustCodeGraph 精确符号查询：`StatsHandle` 命中 `interfaces.rs::StatsHandle`；`LoadStatsFromJSONConcurrently`、`SendLoadRequests`、`HandleDDLEvent` 分别命中本文件对应 trait method。对这些声明运行 callers/callees 未得到静态调用边，因此本文未虚构动态分派关系。
- crate 与模块证据：读取 `pkg/statistics/handle/types/Cargo.toml` 和 `lib.rs`，确认依赖、Go 包映射、公开再导出和独立测试模块。
- Go 对照证据：通过 RustCodeGraph 读取 `pkg/statistics/handle/types/interfaces.go` 全部 1–537 行，逐组核对接口、nil、variadic、channel、JSON tag 和 best-effort 方法语义。
- 调用与迁移状态证据：用 `rg` 检查本 crate 的 Cargo consumers、`stats_types::StatsCache` 实现、`stats_types::StatsHandle` 使用处、同名局部 trait，以及关键方法在 statistics、domain、executor 和 BR 范围内的直接引用。
- 独立测试证据：`interfaces_test.rs` 覆盖默认版本推进、锁表 DTO、全局合并 DTO、真实依赖类型、supertrait、任意 error、执行上下文、同步加载签名、nil JSON callback 和 JSON 字段名。
- 本任务是纯文档分析，按计划不运行 Cargo；最终结构检查要求本文恰好包含上述 11 个固定二级标题。
