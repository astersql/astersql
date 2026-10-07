# `br/pkg/streamhelper/advancer_cliext.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-streamhelper`（包入口为 [`lib.rs`](lib.rs)，manifest 为 [`Cargo.toml`](Cargo.toml)），是日志备份 checkpoint advancer 与元数据存储之间的扩展层。模块由 `lib.rs` 以 `advancer_cliext` 公开，并通过 `pub use advancer_cliext::*` 扁平导出。

它承担两条彼此相关的路径：一条把任务与暂停键的初始快照和后续 watch 变化转换成 `TaskEvent`；另一条管理每个任务的 V3 全局检查点，包括读取、单调上传、清理和等待推进。生产侧适配见 [`advancer_env.rs`](advancer_env.rs) 的 `AdvancerExtEnv: StreamMeta`；其中同步环境接口目前调用 `BeginSnapshot`，而需要实时事件的调用方可直接调用 `AdvancerExt::Begin`。

## 核心职责

1. `AdvancerExt::getFullTasksAsEvent` 读取带 revision 的完整任务集合，并把每个任务转换为包含解析后 key ranges 的 `EventAdd`。
2. `AdvancerExt::Begin` 先发送快照，再从 `revision + 1` 同时监听任务前缀和暂停前缀，从而避免快照读取与 watch 建立之间漏事件。
3. `AdvancerExt::toTaskEvent` 将底层 `WatchEvent` 映射为新增、删除、暂停、恢复四种领域事件；转换失败由监听循环包装为 `EventErr`，不会因为单个畸形任务立即停止另一个 watch。
4. `GetGlobalCheckpointForTask`、`UploadV3GlobalCheckpointForTask` 和 `ClearV3GlobalCheckpointForTask` 管理 `GlobalCheckpointOf(taskName)` 键，并保证上传值不会主动回退。
5. `MetaDataClient::WaitGlobalCheckpointAdvance` 以“读取当前值及响应 revision，再从 `revision + 1` watch”的方式等待值严格大于调用者给定的 `current`，同时处理超时、取消、watch 重建与进度通知。
6. `runMetadataRequestWithRetry` 给元数据请求提供有界超时和选择性重试，并在返回前 join 工作线程，防止超时请求遗留后台 worker。

## 主要符号

- `METADATA_REQUEST_TIMEOUTS`：三次请求的超时预算，依次为 5、10、15 秒。
- `METADATA_WATCH_PROGRESS_INTERVAL_MILLIS` / `METADATA_WATCH_IDLE_TIMEOUT_MILLIS`：原子保存的 watch 主动进度请求间隔与无进展超时，默认分别为 30 秒和 90 秒；`setMetadataWatchProgressForTest` 原子替换并返回旧值，供独立测试恢复。
- `runMetadataRequestWithRetry<T>`：为每次尝试创建继承父 `WatchContext` 的 `MetadataRequestContext`，只对 `DeadlineExceeded` 和 `Unavailable` 重试；可在超时时调用 `on_timeout` 重置 watcher。
- `EventType`：`EventAdd`、`EventDel`、`EventErr`、`EventPause`、`EventResume`，判别值与 Go 的 `iota` 顺序一致；`Display` 输出简短名称。
- `TaskEvent`：承载事件类型、任务名、可选 `StreamBackupTaskInfo`、ranges 和可选错误字符串。`errorEvent` 构造只有错误字段有效的终止或转换错误事件。
- `parseGlobalCheckpointValue`：只接受恰好 8 字节的大端 `u64`。
- `AdvancerExt { meta: MetaDataClient }`：本文件的核心门面。它是 `Clone`，异步监听线程持有克隆值。
- `BeginSnapshot`：仅向可变 `Vec<TaskEvent>` 加入当前快照，是同步 `StreamMeta` 适配层的过渡入口。
- `Begin`：向 `mpsc::Sender<TaskEvent>` 同步发送快照并启动后台实时监听线程。
- `lastCheckpointMetric`：读取进程内、按任务名存储的最近成功上传值；它和 `LAST_CHECKPOINT_METRIC` 是 Rust 移植中的轻量指标替身。
- `getGlobalCheckpointWithRevision` / `WaitGlobalCheckpointAdvance`：定义在 `MetaDataClient` 上的内部读取原语和公开等待接口。

## 执行流程

任务事件路径如下：

1. `Begin` 调用 `getFullTasksAsEvent`，后者经 `MetaDataClient::GetAllTasksWithRevision` 获取同一快照中的任务和 revision，并逐项调用 `TaskInfo::Ranges`。
2. 初始任务作为 `EventAdd` 先写入调用者 channel；任一发送失败会使 `Begin` 直接返回错误，尚不会生成监听线程。
3. 以 `revision + 1` 分别调用 `KV.WatchPrefix(PrefixOfTask())` 和 `KV.WatchPrefix(PrefixOfPause())`，任一创建失败都会令 `Begin` 返回错误。
4. 后台线程轮询两个 receiver。普通事件经 `toTaskEvent` 转换；progress 事件只刷新活跃度，不向上游暴露。任务 `Put` 会反序列化 `StreamBackupTaskInfo` 并重新查询 ranges；任务 `Delete`、暂停 `Put/Delete` 不带 `Info` 和 ranges。
5. 周期到达时调用 `RequestWatchProgress`；超过 idle timeout 仍没有任何底层事件时发送 `EventErr` 并退出。父 context 取消时先 `try_iter` 排空两个 receiver 已送达的非 progress 事件，再发送取消错误。receiver 非取消断开则发送 `EOF`。

检查点路径如下：

1. `GetGlobalCheckpointForTask` 读取全局检查点键；键不存在（空值）视为 0，非空值交给 `parseGlobalCheckpointValue`。
2. `UploadV3GlobalCheckpointForTask` 先读取旧值。新值小于旧值时直接成功返回；否则将大端编码值通过 `PutWithRequestContext` 写入并按超时策略重试，成功后才更新 `LAST_CHECKPOINT_METRIC`。
3. `WaitGlobalCheckpointAdvance` 先通过 `getGlobalCheckpointWithRevision` 获取值和读取响应 revision。值已经严格大于 `current` 时立即返回；否则从 `revision + 1` 创建 watch。
4. watch 创建超时时由回调和请求闭包配合重置 watcher；三次仍超时会映射为 `PiTR checkpoint watch restart required`。watch 中只接受目标键的 `Put`，值严格大于 `current` 才完成；receiver 断开则重新进入“读取再监听”外层循环。

## 数据与状态

- 持久化键由 [`models.rs`](models.rs) 的 `PrefixOfTask`、`PrefixOfPause` 和 `GlobalCheckpointOf` 构造；检查点值由 `encodeUint64` 写成 8 字节大端格式。
- `TaskEvent` 是所有权消息：任务信息、ranges 和错误字符串随 channel 发送，不借用底层 watch buffer。
- `AdvancerExt` 本身只持有可克隆的 `MetaDataClient`；任务 watch 的游标来自快照 revision，检查点 watch 的游标来自读取响应 revision，而不是键自身的修改 revision。这一差异保证长期未修改的键即使旧 revision 已被压缩，也能从当前一致性位置继续观察。
- 两个 watch 时间参数使用 `AtomicU64`，读取为 `Acquire`、测试替换为 `AcqRel`。最近 checkpoint 指标使用 `OnceLock<Mutex<HashMap<String, u64>>>` 延迟初始化；它是进程局部观测状态，不是持久化真值。
- 上传的单调性是“先读后写”的客户端约束，并非原子 compare-and-set；代码能避免单个调用者显式写入较小值，但本文件没有证明多个并发 writer 间的全局线性化单调性。

## 依赖与调用关系

上游关系：

- [`lib.rs`](lib.rs) 声明并再导出本模块。
- [`advancer_env.rs`](advancer_env.rs) 的 `AdvancerExtEnv` 把 `BeginSnapshot` 和三个 checkpoint CRUD 方法接到 `StreamMeta`；`NewTiDBEnv` 构造 `AdvancerExt { meta }`。
- [`advancer.rs`](advancer.rs) 经 `StreamMeta` 在任务切换时读取/清理 checkpoint，并在 `importantTick` 中上传推进后的 checkpoint。实时 `Begin` 还被 [`integration_test.rs`](integration_test.rs) 和本模块独立测试直接验证。

下游关系：

- [`client.rs`](client.rs) 的 `MetaDataClient` 提供任务枚举、`TaskByInfo` 和底层 `KV`。
- [`models.rs`](models.rs) 提供元数据键与整数编码；[`stubs.rs`](stubs.rs) 定义 `EtcdKV` 风格接口、请求/取消上下文、watch 事件和任务 protobuf 替代类型。
- 标准库 `mpsc` 负责 worker 返回和任务事件传递，`std::thread` 负责每次带超时请求的 worker 及长期 watch 线程，`serde_json` 解码 Rust 元数据任务值。
- `Cargo.toml` 将 crate 定义为 `lib.rs` 入口；本文件直接使用的非标准能力主要来自 crate 内部模块和已声明的 `serde_json`。manifest 的 porting metadata 把它归属到 Go 包 `br/pkg/streamhelper`。

RustCodeGraph 的 `query AdvancerExt` 能定位 Rust struct（`advancer_cliext.rs:209`）及同路径 Go 类型，但当前索引的 `query` 对本文件 Rust impl 方法没有完整建边：例如 `WaitGlobalCheckpointAdvance` 只返回 Go 方法。因此上游 Rust 调用点使用 `rg` 补查，并在“验证依据”中明确记录。

## 错误处理与边界

- 请求重试只接受 `MetadataRequestError::DeadlineExceeded` 和 `Unavailable`；权限等 `Other` 错误立即返回。父 `WatchContext` 取消优先返回 `watch canceled`。
- 每次请求超时会取消子请求并 join worker；worker panic 转换为 `metadata request worker panicked`。若 `on_timeout` 重置 watcher 失败，该失败作为非重试的 `Other` 返回。
- `parseGlobalCheckpointValue` 严格拒绝非 8 字节值，并把实际长度和值写入错误。Rust 当前错误字符串会打印原始字节调试表示，见下节所述，这与 Go 的脱敏包装不同。
- `toTaskEvent` 拒绝不属于 task/pause 前缀的键和 progress 伪事件；任务 `Put` 的 JSON 或 ranges 解析错误被转换成单个 `EventErr`，监听循环继续处理后续事件。
- `Begin` 建立两个 watch 不是事务操作：第一个成功、第二个失败时方法返回错误，本文件没有显式释放第一个 receiver 对应的底层资源，具体释放依赖 `EtcdKV` 实现。
- checkpoint watch 忽略目标键以外、非 `Put` 和未超过阈值的事件；channel 断开会重新读取，以修复 watch 生命周期中可能发生的推进。
- `UploadV3GlobalCheckpointForTask` 对相等值仍会写入；只有严格较小才跳过。指标也只在最终写成功后更新，写失败或回退跳过不会改变指标。

## 并发与资源生命周期

- `runMetadataRequestWithRetry` 每次尝试创建一个线程和容量为 1 的同步 channel。主线程每最多 10ms 检查一次结果或 context；结束时取消请求并 `join`，测试 `metadata_deadlines_and_parent_cancellation_join_workers` 验证没有活跃 worker 遗留。
- `Begin` 返回后，发送端及两个 watch receiver 被后台线程拥有。调用者丢弃接收端时，下一次 `send` 失败会结束线程；父 context 取消、任一 receiver 断开、idle timeout 或进度请求失败也会结束线程。所有 sender 克隆随线程退出而释放，接收端随后观察到断开。
- 取消路径会先排空两个 receiver 当前已经缓冲的事件，然后发送终止错误；`canceled_listener_drains_both_closed_watches_before_cancel_error` 和 `cancellation_when_task_watch_closes_drains_pause_watch` 固化了“残余事件先于取消错误”的顺序。
- 监听采用 10ms 轮询加 sleep，而不是阻塞 select；增加新 watch 时应同时评估轮询成本、公平性、progress 活跃度定义和取消排空顺序。
- `LAST_CHECKPOINT_METRIC` 的 mutex poisoning 通过 `unwrap` 传播为 panic；本文件没有恢复 poisoned lock。测试专用时间参数是进程全局状态，并行测试必须成对保存/恢复，避免互相污染。

## 与 Go 版本的对应关系

对应源是 [`advancer_cliext.go`](advancer_cliext.go)。Rust 保留了 Go 的事件枚举顺序、快照后从 `rev + 1` 监听、任务/暂停双 watch、取消时排空、checkpoint 大端编码、严格推进判断、单调上传、请求重试和 watch progress/idle timeout 等核心语义。

当前实现差异必须在扩展时显式考虑：

- Go 直接使用 etcd v3 watcher、protobuf `StreamBackupTaskInfo`、gRPC status code、结构化日志、failpoint、PingCAP 错误类型、redaction 和 Prometheus 指标；Rust 通过 [`stubs.rs`](stubs.rs) 的抽象 transport、JSON 解码、字符串错误及进程内 `HashMap` 指标表达这些行为。因此 Rust 的错误分类、可观测性和敏感值脱敏尚不等价于 Go 生产实现。
- Go 将 `eventFromWatch`、`startListen` 和 `waitCheckpointEvent` 分成独立方法，并利用 channel/select/timer；Rust 把监听循环内联到 `Begin` / `WaitGlobalCheckpointAdvance`，用 `mpsc` 轮询实现。
- Go `Begin` 是主环境的实时入口并最终关闭输出 channel；Rust `StreamMeta` 的同步签名目前只能调用 `BeginSnapshot`，实时 `Begin` 是额外 API。不能把 snapshot 适配误述为已接通生产实时 watch。
- Go 在 checkpoint watch 压缩或关闭时返回专用可重启错误，再由外层重试；Rust 对普通 receiver 断开直接重读重建，对三次创建 deadline 才产生字符串形式的 restart-required 错误。
- Go 的上传接收调用者 context；Rust 上传内部创建新的 `WatchContext`，因此当前 API 没有让调用者取消上传的参数。

Go 的直接回归证据集中在 [`integration_test.go`](integration_test.go)：覆盖任务关闭事件、watch progress 超时、checkpoint 读写与防回退、读取/上传超时重试、提交后超时重试及 revision compaction。Rust 对应的细粒度测试位于独立文件 [`advancer_cliext_test.rs`](advancer_cliext_test.rs)。

## 扩展指南

- 新增任务事件种类时，应同时修改 `EventType`、`Display`、`TaskEvent` 语义、`toTaskEvent` 映射及 Go 对照；在 `advancer_cliext_test.rs` 增加独立测试，不要把测试嵌入生产文件。
- 改变 key 布局或序列化时，应从 `models.rs` / `stubs.rs` 的 canonical 定义接入，并同步验证 snapshot 与 live event 使用同一格式。特别要保留 `revision + 1` 不变量，避免建立 watch 时漏事件。
- 引入真实异步 runtime 或真实 etcd client 时，应保持取消排空、终止错误排序、watch progress 和 idle timeout 行为；还需替换字符串错误、补足日志/脱敏/指标，并证明 watch 创建失败不会泄漏资源。
- 强化并发单调性时，最可能修改 `UploadV3GlobalCheckpointForTask`，使用元数据存储事务/CAS 代替“读后写”；需要增加并发 writer 回归，并评估与 Go 行为及旧数据的兼容性。
- 改变重试策略时，应集中修改 `METADATA_REQUEST_TIMEOUTS` 和 `runMetadataRequestWithRetry`，保持只重试瞬态错误、每次取消并 join worker、watch deadline 时重置 transport 的约束。
- 扩充生产接线时，应评估 `StreamMeta::Begin` 的同步 `Vec` 签名是否需要演进，使 `AdvancerExtEnv` 能使用实时 `Begin`；这会影响 `advancer_env.rs`、`advancer.rs`、fake env 及其独立测试，不能只改本文件。
- 性能风险主要来自每次请求新建线程、10ms watch 轮询、任务 `Put` 时再次读取 ranges，以及进程全局 mutex 指标；优化时必须保留现有错误与顺序语义。

## 验证依据

- 生产源：[`advancer_cliext.rs`](advancer_cliext.rs)（全部 495 行），重点核对 `runMetadataRequestWithRetry`、`EventType`、`TaskEvent`、`AdvancerExt::{BeginSnapshot, Begin, toTaskEvent, GetGlobalCheckpointForTask, UploadV3GlobalCheckpointForTask, ClearV3GlobalCheckpointForTask}` 以及 `MetaDataClient::{getGlobalCheckpointWithRevision, WaitGlobalCheckpointAdvance}`。
- crate 边界与接线：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`advancer_env.rs`](advancer_env.rs) 的 `StreamMeta`/`AdvancerExtEnv`、[`advancer.rs`](advancer.rs) 的 checkpoint 读取、清理和上传调用。
- Rust 独立测试：[`advancer_cliext_test.rs`](advancer_cliext_test.rs) 覆盖 snapshot/live 事件、畸形事件隔离、等待推进与取消、上传单调性与指标、compaction 后 response revision、瞬态/永久错误重试、watch reset、worker join、取消排空和非取消 EOF；[`integration_test.rs`](integration_test.rs) 还覆盖公开路径的快照、实时监听和 checkpoint 生命周期。
- Go 对照：[`advancer_cliext.go`](advancer_cliext.go)；相关 Go 场景位于 [`integration_test.go`](integration_test.go)。仓库中不存在同名 `advancer_cliext_test.go`，所以未虚构该路径。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`query AdvancerExt --kind struct` 定位到 Rust `advancer_cliext.rs:209` 和 Go `advancer_cliext.go:72`。`explore`、`node --file` 以及 Rust impl 方法查询未返回完整边，故对未覆盖调用边使用 `rg` 核验，并发现 `advancer_env.rs` 的适配调用和 `advancer.rs` 的消费点。
- 本任务是纯文档分析，依计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核没有把 Go 的生产能力误写成 Rust 当前事实。
