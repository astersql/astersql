# `pkg/ingestor/ingestctrl/duplicate.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate；crate 入口 `pkg/ingestor/ingestctrl/lib.rs` 以 `pub mod duplicate` 暴露它。它位于 Lightning local backend 的 ingest 控制面中，承接“从本地或远端来源读取重复 KV，再依处理策略报错、记录或删除”的工作。当前 Rust 应用侧的直接组装入口是 `Backend::GetDupeController`（`pkg/ingestor/ingestctrl/local.rs:676`）：该入口创建 `DupeDetector`，并把 `EngineManager` 提供的重复 KV 集合及 `KeyAdapter` 装入 `DupeController`。

`pkg/ingestor/ingestctrl/Cargo.toml` 将该目录声明为独立 library crate（`[lib] path = "lib.rs"`），并用 `[package.metadata.porting] go-package = "pkg/ingestor/ingestctrl"` 标记 Go 来源。`duplicate.rs` 本身只直接依赖标准库、同 crate 的 `iterator::KeyAdapter`，以及 crate 根定义的 `CancellationToken`、`DuplicateResolution`、`Error`、`KeyRange`、`KvPair` 和 `Result`；它没有直接调用 Cargo 中列出的外部 crate。

## 核心职责

1. 用 `DupKVStream` 统一本地内存重复 KV 与远端分页重复 KV 的顺序读取/关闭接口。
2. 用 `DupeDetector` 执行冲突处理策略：`Error` 在首条冲突处返回原始 KV；其他策略分批写入 `ErrorManager`；`Remove` 还会在事务中仅删除值仍未变化的冲突键。
3. 用 `PendingKeyRanges` 记录远端扫描尚未完成的半开范围，并让 `processRemoteDupTask` 对可重试失败进行有限重试。
4. 用 `DupeController` 把检测器连接到本地重复 KV 快照；辅助类型 `pendingIndexHandles`、`PendingIndexHandle` 和 `FoundDuplicateKeys` 为索引冲突及错误解包保留数据结构。

这里的 Rust 实现是一个抽象化、范围较窄的移植层，不具备同路径 Go 文件中的完整表/索引元数据解码、Region 拆分、worker pool 并发扫描和完整冲突替换流程。

## 主要符号

- `MAX_DUP_COLLECT_ATTEMPTS = 5`：远端扫描连续无进展时的最大失败轮数。`DEFAULT_RECORD_CONFLICT_BATCH = 1024`：写入冲突及条件删除的批大小。
- `DataConflictInfo`：保存表名、原始 key/value 及当前 Rust 实现生成的可读 `row` 字符串。`PendingIndexHandle` 额外保存索引名、编码 handle 与原始 handle。
- `pendingIndexHandles`：内部 `Vec<PendingIndexHandle>` 容器；`append`、`truncate`、`Len` 和 `sort` 分别负责追加、保留容量式清空、计数及按 `raw_handle` 排序。构造函数为 `makePendingIndexHandlesWithCapacity`。
- `PendingKeyRanges`：以结束键为 `BTreeMap` 键保存待扫描范围；`list` 展平快照，`empty` 判断完成，`finish` 从所有重叠范围中扣除已完成区间。构造函数为 `newPendingKeyRanges`。
- `DupKVStream`：提供 `Next() -> Result<Option<KvPair>>` 与 `Close()`；`None` 表示正常结束。
- `DupKVStreamImpl` / `NewLocalDupKVStream`：本地实现，构造时按 `[start, end)` 过滤并按编码 key 排序，读取时通过 `KeyAdapter::Decode` 暴露用户 key。
- `RemoteDuplicateSource`、`RemoteDupKVStream` / `NewRemoteDupKVStream`：远端数据源抽象及分页缓冲流；`scan_duplicates` 返回一页 KV 和 continuation，空 continuation 表示最后一页。
- `ErrorManager`：数据冲突和索引冲突的持久化边界。`Transaction` / `TransactionFactory`：`BatchGet`、`Delete`、`Commit` 组成条件删除边界。
- `DupeDetector` / `NewDupeDetector`：核心协调器；公开方法包括 `HasDuplicate`、`RecordDataConflictError`、`RecordIndexConflictError` 和 `processRemoteDupTask`，内部方法为 `collect_stream`、`write_conflicts`、`delete_conflicts`。
- `RetrieveKeyAndValueFromErrFoundDuplicateKeys`：只接受 `Error::Conflict` 并克隆其中 key/value，其他错误返回 `Error::InvalidArgument`。
- `DupeController::new`、`CollectLocalDuplicateRows`：当前 Rust 本地应用入口；后者对共享 KV 加锁、克隆快照、构造本地流、调用检测器，并返回检测器累计的 `HasDuplicate` 状态。

## 执行流程

本地流程从 `Backend::GetDupeController` 开始。它用表名、错误管理器和事务工厂创建检测器，并取得 `EngineManager::getDuplicateData()` 与 `getKeyAdapter()`。调用 `DupeController::CollectLocalDuplicateRows` 时，控制器先检查 `Mutex` 加锁结果并克隆整个 KV 向量，然后释放锁；`NewLocalDupKVStream` 对快照做 `[start, end)` 过滤与 key 排序；每次 `Next` 解码 key 并保持 value 不变。

`RecordDataConflictError` 与 `RecordIndexConflictError` 都进入 `collect_stream`。循环每取得一项，先检查取消令牌，再以 Release 顺序把 `has_duplicate` 设为真。若策略为 `DuplicateResolution::Error`，立即返回 `Error::Conflict { key, value }`。否则构造 `DataConflictInfo`；批大小达到 1024 或到达流尾时，依据 `index` 参数调用对应的 `ErrorManager` 方法。策略为 `Remove` 时，记录每批后再调用 `delete_conflicts`。无论循环成功还是读取、记录、删除失败，函数最后都会调用一次 `stream.Close()`，但关闭错误被有意忽略并保留原操作结果。

条件删除流程先通过 `TransactionFactory::Begin` 开启事务，再一次性 `BatchGet` 本批 key。只有事务快照中的当前 value 与扫描时记录的 value 完全相等，才调用 `Delete`；每个删除前再次检查取消令牌，最后 `Commit`。这条“比较后删除”不变量避免删除扫描后被并发更新的新值。

远端流程由 `processRemoteDupTask` 建立初始 `PendingKeyRanges`。每轮为所有待处理范围新建 `RemoteDupKVStream`，后者缓冲分页结果并在缓冲耗尽时调用 `RemoteDuplicateSource::scan_duplicates`。某范围完整处理成功后由 `finish` 扣除；`Error::Retryable` 保留该范围，其余错误立即返回。只要本轮至少一个范围完成，连续无进展计数归零；否则递增，达到 5 次时返回新的 `Error::Retryable`。

## 数据与状态

- `KeyRange` 来自 crate 根，语义是半开区间 `[start, end)`，空 `end` 表示无上界。本地过滤、`PendingKeyRanges::finish` 和远端扫描都依赖这一约定。
- `PendingKeyRanges::finish` 对每个重叠范围最多生成左、右两个残余；无界 `finished.end` 不会重新产生右侧工作，空区间也不会重新插入。容器按 end key 排序，但同一 end 可保存多个范围。
- `DupKVStreamImpl` 的 `cursor` 只增不减；`Close` 设置 `closed` 并清空 pairs，关闭后的 `Next` 返回 `Error::Closed`。
- `RemoteDupKVStream` 的 `buffer/cursor` 表示当前页，`continuation` 表示下一页起点，`finished` 表示数据源已返回空 continuation。`Close` 只标记完成并清空缓冲，不调用数据源的关闭操作。
- `DupeDetector::has_duplicate` 是累计状态：一旦观察到 KV 就保持为真，不会在一次收集开始时重置。读取采用 Acquire、写入采用 Release，因此可跨线程观察，但检测器没有“一次调用独立计数”的语义。
- `record_batch_size` 构造时固定为 1024；当前无公开调节入口。
- `DupeController::local_pairs` 是 `Arc<Mutex<Vec<KvPair>>>`，收集时克隆全量快照，检测过程不继续持锁，也不会从该内存向量移除已处理项。

## 依赖与调用关系

上游链路为 `local.rs::Backend::GetDupeController` → `NewDupeDetector` + `DupeController::new` → `DupeController::CollectLocalDuplicateRows` → `NewLocalDupKVStream` → `DupeDetector::RecordDataConflictError`。仓库 Rust 搜索未发现生产代码直接调用 `processRemoteDupTask`、`RecordIndexConflictError` 或 `NewRemoteDupKVStream`；这些公开能力当前更像为后续接线保留的模块 API，不能据此宣称远端/索引路径已进入完整应用主链。

下游边界包括：`KeyAdapter::Decode`（`iterator.rs`）负责去除本地重复库的编码包装；`CancellationToken::check` 提供协作式取消；`ErrorManager` 隔离冲突持久化；`RemoteDuplicateSource` 隔离远端分页协议；`TransactionFactory` 与 `Transaction` 隔离删除事务。`KeyRange::overlaps`、`DuplicateResolution` 和统一 `Error` 均定义在 `lib.rs`。

`Cargo.toml` 的大量 Windows 条件依赖属于整个 ingestctrl crate，而不是本文件直接依赖。当前 `duplicate.rs` 的抽象 trait 也没有在本文件中绑定到具体 TiKV、error manager 或 SQL 表解码实现。

## 错误处理与边界

- 本地流关闭后读取返回 `Error::Closed`；`KeyAdapter::Decode` 错误原样传播。远端流每次循环先检查取消，数据源错误原样传播。
- `collect_stream` 在读取、取消检查、冲突写入、事务操作或提交失败时立即返回。关闭始终尝试，但 `Close` 的错误被丢弃；若未来关闭具有必须报告的失败语义，应重新设计错误合并规则。
- `DuplicateResolution::Error` 不写错误管理器，也不执行删除；它在第一条 KV 处返回携带原始字节的 `Error::Conflict`。`None` 与 `Record` 在当前 Rust 实现中走相同的“记录但不删除”路径；这点与枚举注释中 `None` 的“不处理”字面含义存在语义张力，扩展时不可假定 `None` 会跳过记录。
- `Remove` 先持久化冲突，再尝试删除；写入成功而删除/提交失败时会留下已记录但未删除的数据，没有跨错误管理器与 KV 事务的原子性。
- `delete_conflicts` 只删除仍匹配旧 value 的键；不存在或已变化的键被跳过。事务对象被消费式 `Commit`，接口没有显式 rollback；失败后的清理依赖具体实现。
- `PendingKeyRanges` 的正确性依赖调用方提供规范半开范围。`processRemoteDupTask` 只把 `Error::Retryable` 视为可重试；任意非重试错误直接终止。
- `RetrieveKeyAndValueFromErrFoundDuplicateKeys` 不解析包装错误，只匹配本 crate 的精确 `Error::Conflict` 变体。

## 并发与资源生命周期

`DupeDetector` 通过 `AtomicBool` 暴露线程可见的累计重复状态，其依赖 trait 要求 `ErrorManager`、`TransactionFactory` 和 `RemoteDuplicateSource` 为 `Send + Sync`，事务为 `Send`。但 `DupKVStream` 本身没有 `Send` 约束，`processRemoteDupTask` 也是顺序遍历范围；当前文件不创建线程或异步任务。

本地共享数据的锁只用于获取一致快照：`CollectLocalDuplicateRows` 在锁内克隆 `Vec<KvPair>`，随后流的排序、解码和冲突处理均在锁外进行。代价是 O(n) 快照内存及 O(n log n) 排序时间，且快照之后的新重复项不会出现在本轮。

流的生命周期由 `collect_stream` 收口：所有正常与异常退出均尝试 `Close`。远端流的关闭只丢弃本地缓冲和阻止继续拉页；取消则由共享 `CancellationToken` 在拉页前、每项处理时和每个条件删除前检查。事务从 `Begin` 持续到整批 `BatchGet`、逐键条件删除及 `Commit` 完成。

`PendingKeyRanges` 没有内部锁，适用于当前顺序重试流程；若将 Region 范围改为并行处理，必须在调用层串行合并完成区间，或为该结构增加同步并重新验证范围切分不变量。

## 与 Go 版本的对应关系

同路径 `duplicate.go` 是来源和完整行为基准。两版都保留最大重试次数 5、默认冲突批大小 1024、流抽象、待处理范围、重复状态和本地/远端检测概念；Rust 独立测试也沿用 Go 的边界意图。不过当前并非逐项等价：

- Go `pendingIndexHandles` 是四个等长 slice 的 struct-of-arrays，并实现完整 `sort.Interface::Swap`；Rust 是 `Vec<PendingIndexHandle>` 的 array-of-structs，排序结果一致但布局与批量 BatchGet 方式不同。
- Go 本地流直接迭代 Pebble duplicate DB；Rust 接收内存 `Vec<KvPair>` 快照。Go 远端流封装 ImportSST DuplicateDetect gRPC、Region leader 与请求上下文；Rust 用 `RemoteDuplicateSource` + continuation 抽象，尚无这些具体接线。
- Go `dupeDetector` 持有 table、decoder、TiKV codec/client、split client、并发度和日志，可构造表记录/唯一索引任务并解码人类可读行。Rust `DupeDetector` 只持表名、抽象错误管理器/事务工厂和布尔状态，`row` 只是 key/value 的 Debug 文本。
- Go 索引路径按同 key 分组，只把不同 value 视为真实冲突，忽略相同 key/value 的重入重复，并批量查询对应行；Rust `RecordIndexConflictError` 与数据冲突共用 `collect_stream`，每个 KV 都直接视为冲突，没有索引分组、handle 解码或对应行查询。
- Go 远端路径先按 Region 切分，使用 task/region worker pools 并区分 Region error 重试；Rust 顺序扫描调用方给定的范围，仅按 `Error::Retryable` 和“本轮是否有进展”控制五次重试。
- Go 的 `DupeController` 分别提供本地收集、远端收集与 `ResolveDuplicateRows`，替换策略在后续阶段读取最新值并批量删除。Rust 把 `Remove` 的比较后删除直接放在收集批次内，且当前控制器只公开本地收集。
- Go `RetrieveKeyAndValueFromErrFoundDuplicateKeys` 解析 `common.ErrFoundDuplicateKeys` 及其 `terror` 参数；Rust 只解析本 crate 的 `Error::Conflict`。

因此扩展 Rust 时应以 Go 的实际分组、解码、重试及生命周期为语义参考，但不能把 Go 已有功能描述成 Rust 当前已支持。

## 扩展指南

- 接入真正远端扫描时，应实现 `RemoteDuplicateSource`，并从应用入口调用 `processRemoteDupTask`；同时补齐 Region 边界变化、空页但 continuation 非空、取消、部分范围成功后重试及五轮无进展等独立测试。
- 完善索引冲突时，修改重点是 `RecordIndexConflictError`/`collect_stream` 的分流，而不是只丰富 `PendingIndexHandle`。应同步 Go 的“同 key 不同 value 才算冲突”、handle 解码、对应 row 查询与批量写入语义，并在 `duplicate_test.rs` 增加相同 KV、不同 value、批次边界和错误传播测试。
- 改动处理策略时，应明确 `None`、`Record`、`Remove` 的契约及记录/删除顺序；若要与 Go 的独立 Resolve 阶段一致，需要调整 `DupeController` API，而不能仅改枚举分支。
- 优化本地内存或锁占用时，入口是 `DupeController::CollectLocalDuplicateRows` 与 `DupKVStreamImpl`。流式替代全量 clone/sort 前必须保持半开范围、有序输出、KeyAdapter 解码及锁外处理等可观察行为。
- 改动 `PendingKeyRanges::finish` 时，应覆盖相离、完全覆盖、左右截断、中间切分、空 end 无上界及多个重叠范围；并行化则还需同步策略和竞态测试。
- 改动删除路径时，应保持“当前 value 等于扫描 value 才删除”的并发安全条件，并测试 `Begin`、`BatchGet`、`Delete`、取消和 `Commit` 各阶段失败。Rust 单元测试必须继续放在独立的 `pkg/ingestor/ingestctrl/duplicate_test.rs`，不要内嵌到生产文件。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/ingestor/ingestctrl` 确认目标 Rust/Go/测试均已索引，`duplicate.rs` 有 58 个符号。
- RustCodeGraph `node --file pkg/ingestor/ingestctrl/duplicate.rs --offset 1 --limit 500` 及 `--offset 489 --limit 100`：核对了全部 520 行生产源码；`query` 核对 `DupeDetector`、`processRemoteDupTask`、`DupeController` 与 `NewLocalDupKVStream` 的定义候选。`callers/callees` 精确查询连续超时，故调用边由下列源码搜索补证，而未把超时结果当作调用关系结论。
- RustCodeGraph `node --file pkg/ingestor/ingestctrl/duplicate_test.rs --offset 1 --limit 500`：核对本地流的范围过滤/排序/关闭、原始冲突解包、累计重复标志、范围切分、无界完成范围、句柄排序清空及读取失败仍关闭流。
- `pkg/ingestor/ingestctrl/lib.rs`：核对模块暴露、统一错误、半开 `KeyRange`、四种 `DuplicateResolution` 和 `CancellationToken`；`pkg/ingestor/ingestctrl/local.rs:675-687`：核对唯一生产组装入口和本地依赖来源。
- `pkg/ingestor/ingestctrl/Cargo.toml`：核对 crate 边界、Go package 映射及依赖条件；`pkg/ingestor/ingestctrl/duplicate.go`：核对 Go 的本地/远端流、数据/索引冲突、Region 重试、控制器及 Resolve 流程；`pkg/ingestor/ingestctrl/duplicate_test.go`：核对 Go 的任务构造、keyspace 编码、错误解包及行/索引冲突可读错误意图。
- 仓库 `rg` 调用点搜索：Rust 生产代码只在 `local.rs` 导入并组装这些类型；测试路径为独立的 `duplicate_test.rs`。本任务是纯文档分析，按计划未运行 Cargo 或代码测试。
