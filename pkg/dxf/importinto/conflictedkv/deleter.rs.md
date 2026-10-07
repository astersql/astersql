# `pkg/dxf/importinto/conflictedkv/deleter.rs` 逻辑说明

## 文件定位

本文件属于 `astersql-dxf-importinto-conflictedkv` crate；crate 由同目录 `Cargo.toml` 定义，`lib.rs` 将私有模块 `deleter` 的公开项重新导出。它处于 IMPORT INTO 的 `resolve-conflicts` 阶段：上游 `pkg/dxf/importinto/conflict_resolution.rs::ResolveConflictGroup` 从对象存储读取先前记录的冲突 KV，按 worker 分发后为每个接收端构造 `NewDeleter` 并调用 `Deleter::Run`。包级业务背景见 `pkg/dxf/importinto/conflictedkv/doc.go`：此阶段要删除冲突行关联且当前仍存在于集群中的全部 KV，使导入后的表重新一致。

该文件不是存储适配器，也不负责读取冲突文件。它位于 `Handler` 的“冲突 KV 解码/整行重编码”能力和 `ConflictStore` 的“快照读取/事务写入”抽象之间，负责存在性过滤、批量边界、删除 worker、事务重试和取消传播。

## 核心职责

1. `NewDeleter` 根据 KV 组选择 `DataKVHandler` 或 `IndexKVHandler`，并准备一份用于删除前存在性检查的 `LazyRefreshedSnapshot`。
2. `Deleter` 实现 `EncodedRowHandler`，接收 handler 重编码出的整行 `Pairs`，只缓冲快照中仍存在的键；已不存在的键不进入删除事务。
3. 缓冲达到 `BufferedKeySizeLimit`（默认 2 MiB）或 `BufferedKeyCountLimit`（默认 9600）时，将整个批次交给后台 `DeleteWorker`；输入结束后强制冲刷尾批。
4. `DeleteWorker` 为每批键开启事务，逐键 `Delete` 后 `Commit`，并对普通可重试存储错误及事务可重试错误执行有界指数退避。
5. 在快照读与删除写路径分别经 `TrafficRecorder` 记录集群流量；处理进度则由 handler 持有的 `ProgressCollector` 更新。

## 主要符号

- `storeOpMinBackoff`、`storeOpMaxBackoff`、`storeOpMaxRetryCnt`：重试策略常量，依次为 100 ms、1 s 和最多 10 次尝试。
- `BufferedKeySizeLimit`、`BufferedKeyCountLimit`：公开的原子全局阈值，测试可临时调整；生产读取使用 `Ordering::Acquire`。
- `Deleter`：主状态对象。`handler` 使用 `Option<Box<dyn Handler>>` 以便 `Run` 暂时取出 handler，同时把 `&mut self` 作为 `EncodedRowHandler` 传入；`store` 和 `traffic_recorder` 由删除 worker 共享；`snapshot` 做存在性过滤；`buffered_keys`/`buffered_size` 保存待删批次；`keys_sender` 只在 `Run` 生命周期内存在。
- `NewDeleter`：公开构造函数。`kv_group == DataKVGroup` 时创建 `DataKVHandler`，否则创建无 handle filter 的 `IndexKVHandler`；两条路径共用 `BaseHandler`。
- `Deleter::Run`：公开执行入口，建立容量为 0 的同步通道、启动 scoped 删除线程，依次执行 handler 的 `PreRun`、`Run`、`Close`，冲刷尾批，关闭发送端并等待 worker。
- `gatherAndDeleteKeysWithRetry` / `gatherKeysToDelete` / `sendKeysToDelete`：分别负责快照读取重试与阈值判断、存在键收集、批次转交。
- `Deleter::HandleEncodedRow`：`EncodedRowHandler` 实现；忽略已由 handler 解出的行键/Datum，只使用整行重编码后的 `Pairs.Pairs`。
- `DeleteWorker::deleteLoop` / `deleteBufferedKeys`：消费批次并执行事务删除。
- `retry` / `retryWithPolicy`：本文件私有的同步重试器；前者不额外接受事务冲突，后者由 `retry_transactions` 控制是否调用 `ConflictStore::IsTxnRetryableError`。

## 执行流程

1. `ResolveConflictGroup` 为一个 KV 组创建若干冲突输入通道。每个 worker 调用 `NewDeleter`，随后以对应 receiver 调用 `Run`。
2. `Run` 创建 `sync_channel(0)`。零容量意味着 handler 线程发送一个删除批次时，必须等待删除 worker 接收，从而限制在途批次数并形成背压。
3. handler 的具体流程位于 `handler.rs`：data 组直接解码行；索引组从唯一索引 KV 解出 handle、快照回查数据行；两者最终重编码整行，并回调 `Deleter::HandleEncodedRow`。
4. `gatherKeysToDelete` 把重编码结果中的字节键包装为 `Key`，调用惰性快照 `BatchGet`，仅将返回映射中的键计入 `buffered_keys` 和 `buffered_size`。因此删除目标以“调用时快照仍可见”为准，而不是无条件删除所有重编码键。
5. 任一阈值达到或超过上限时，`sendKeysToDelete` 先检查取消，再用 `mem::take` 转移缓冲并清零字节计数，通过同步通道发送批次。
6. 删除 worker 对每批执行 `retryWithPolicy(..., true, ...)`。一次尝试中先 `Begin`，逐键 `Delete`；任一 `Delete` 失败时尽力 `Rollback` 并返回原错误，全部成功后 `Commit(context)`。
7. handler 输入耗尽后，`Run` 仍调用 `Close`（索引 handler 会在此冲刷剩余 handle 并关闭 codec），再冲刷 deleter 的尾批。移除 `keys_sender` 会关闭删除通道；scoped thread join 后，将 `run_result`、`close_result`、`flush_result`、`delete_result` 按此顺序用 `Result::and` 合并返回。

## 数据与状态

- `buffered_keys` 与 `buffered_size` 必须同步变化：收集存在键时二者递增，成功准备发送时由 `mem::take` 与赋零一起重置。阈值按整个键字节长度与键数判断，不包含 value 大小。
- 快照由 `LazyRefreshedSnapshot` 持有；`handler.rs` 显示它首次访问或距离上次刷新至少 15 秒时以 `CurrentVersion` 获取新快照。索引 handler 自身另有一份快照用于按 handle 回查数据行，deleter 的快照则用于过滤最终重编码键，二者职责不同。
- `NewDeleter` 不为索引路径提供 `KeyFilter`。跨 worker 的一致性依赖上游 `conflictWorkerForPair` 的分发约束；尤其唯一多值索引可能让多个唯一键指向同一行，上游应将其路由到同一 deleter。
- `handler` 在 `Run` 内被 `take`，结束前放回，使同一个 `Deleter` 不会同时持有 handler 和作为可变回调借出。若进入 `Run` 时为空，返回 `deleter handler is already running`。
- 全局阈值是原子值而非实例配置。修改会影响同一进程中的所有 deleter；`deleter_test.rs` 因此用 `BUFFERED_KEY_COUNT_LOCK` 串行保护测试修改。

## 依赖与调用关系

上游生产调用边为 `pkg/dxf/importinto/conflict_resolution.rs::ResolveConflictGroup -> NewDeleter -> Deleter::Run`。上游负责对象存储 reader、按 KV 组和 worker 分发、关闭输入 sender，并在所有 worker join 后汇总读取错误或 worker 错误。

主要下游边如下：

- `NewDeleter -> NewBaseHandler -> NewDataKVHandler | NewIndexKVHandler`，定义冲突 KV 如何恢复成整行 KV。
- `NewDeleter -> NewLazyRefreshedSnapshot`，提供最终键存在性检查；`BatchGet` 还记录命中项的键和值读取字节。
- `Deleter::Run -> Handler::{PreRun, Run, Close}`；handler 经 `EncodedRowHandler::HandleEncodedRow` 回调回来。
- `DeleteWorker::deleteBufferedKeys -> ConflictStore::Begin -> ConflictTransaction::{Delete, Commit, Rollback}`。
- `retryWithPolicy -> ConflictStore::{IsRetryableError, IsTxnRetryableError}`；后者默认通过 `astersql_kv::TxnRetryableMark` 判断事务重试标记。

`Cargo.toml` 无 feature 条件，本文件直接使用的 crate 边界包括 `astersql-kv`、`astersql-lightning-backend-kv`、`astersql-lightning-verification`、`astersql-meta-model`、`astersql-types` 和进度 collector 所在的 `astersql-dxf-framework-taskexecutor-execute`。线程、原子量和同步通道均来自标准库。

## 错误处理与边界

- 快照读取使用 `retry(..., retry_transactions = false)`：只有 `ConflictStore::IsRetryableError` 判定为真的错误重试。事务删除使用 `retry_transactions = true`，还接受事务冲突错误。
- 每轮重试前检查 `ConflictContext::IsCancelled`；退避从 100 ms 倍增并封顶 1 s，最后一次失败直接返回。当前实现用阻塞 `thread::sleep`，睡眠期间不会再次观察取消，最长需等当前退避结束。
- 空重编码结果、快照零命中、空缓冲或空删除批次都是成功的无操作。
- `sendKeysToDelete` 在转移缓冲前检查取消；若 worker 已停止或 sender 未初始化，分别返回明确错误。发送失败发生在 `mem::take` 之后，因此该实例不会自动把该批次放回缓冲，但错误会向上传播。
- `Delete` 失败时调用 `Rollback`，但忽略 rollback 错误并保留原始删除错误；`Begin` 或 `Commit` 错误直接传播。与 Go 版本相比，Rust 没有 logger，因此无法记录 rollback 的次级错误。
- `Run` 即使 `PreRun` 或 handler `Run` 失败，仍尝试 `Close`、尾批冲刷和等待 worker；返回值按主处理、关闭、冲刷、worker 的顺序保留最早错误。若删除线程 panic，则转换为 `delete worker panicked`。
- 重复冲突投递可能生成重复候选，但快照存在性检查、上游单行路由和事务重试共同约束行为；本文件不在缓冲内显式去重。

## 并发与资源生命周期

`Deleter::Run` 使用 `std::thread::scope`，删除 worker 不会逸出调用栈；返回前必定 join。handler/输入消费运行在调用线程，删除事务运行在 scoped worker，两者通过容量为 0 的 `SyncSender<Vec<Key>>` 串联。该设计允许当前批次删除与后续冲突解码交叠，但发送点施加背压，内存中不会无限累积待消费批次。

关闭顺序很关键：handler `Close` 先冲刷索引 handle 并关闭 codec；deleter 再冲刷键；随后 `keys_sender.take()` 关闭唯一发送端，使 `deleteLoop` 的 `recv` 循环退出；最后 join。`Arc<dyn ConflictStore>` 和可选 `Arc<dyn TrafficRecorder>` 保证 worker 存活期间依赖有效。`ConflictContext` 是含共享取消标志的 clone，上游取消可被 handler、发送和每次存储重试观察。

事务粒度是一批 `buffered_keys`。阈值是发批触发条件而非严格上限：一次 `HandleEncodedRow` 可能加入多键，因此批次可能超过阈值。`BufferedKeySizeLimit` 的默认值与 Go 版本一样控制大事务风险，但扩展编码结果时仍需评估单行 KV 总量。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/dxf/importinto/conflictedkv/deleter.go`：相同的 100 ms/1 s/10 次重试参数、2 MiB/9600 缓冲阈值、data/index handler 选择、快照 `BatchGet` 存在性过滤、无缓冲通道背压、事务批量删除和结束冲刷。

重要实现差异如下：

- Go `Run` 用带 recover/context 的 error group 并发运行 handler 与 delete loop；Rust 用调用线程加 scoped worker，并显式 join。两者都在 handler 收尾后关闭删除通道并等待删除结束。
- Go 通过 `RunWithRetry`、`common.IsRetryableError` 与 `tidbkv.IsTxnRetryableError` 判定；Rust 将判定封装进 `ConflictStore`，以字符串错误为边界，并用私有 `retryWithPolicy` 实现相同的有界指数退避意图。
- Go 在 defer 中只在前序无错时 commit，否则 rollback；Rust 在逐键 `Delete` 失败时显式 rollback，全部成功后 commit。Rust 的 commit 失败由外层重试重新创建事务。
- Go rollback 失败会记录 warning；Rust 丢弃 rollback 错误。Go 构造函数持有 logger 和具体 TiDB storage/encoder，Rust 通过 trait object 注入 store/codec，且无 logger 字段。
- Rust 阈值采用 `AtomicUsize`，Go 是包变量。Rust 测试需串行保护全局修改；Go 包测试默认顺序行为不同。

测试意图保持对齐：Go `deleter_test.go::TestDeleter` 使用真实 mock TiDB 表验证 data/index 冲突修复并把条数阈值降为 2；Rust `deleter_test.rs::{test_deleter_data_kv_conflicts,test_deleter_index_kv_conflicts}` 用 fake codec/store 驱动真实 handler、快照和 worker 管线，保留每键重复投递及多次冲刷。Go `deleter_internal_test.go` 与 Rust 的 `propagates_commit_error_without_deleting_key`、`transaction_write_conflict_retries_and_commits_deletion` 对齐提交失败与写冲突重试边界。

## 扩展指南

- 新增 KV 组语义时，先决定它应复用 data 还是 index handler；若不能，仅修改 `NewDeleter` 的二分支不够，还需在 `handler.rs` 增加对应实现，并同步上游 `ResolveConflictGroup` 的路由证据。
- 调整批量策略时优先修改 `gatherAndDeleteKeysWithRetry`/`sendKeysToDelete`，并保持 `buffered_keys` 与 `buffered_size` 不变量。需评估事务大小、同步通道阻塞时间以及一次整行重编码导致的阈值超量。
- 改动重试分类或退避时修改 `retryWithPolicy` 及 `ConflictStore` 判定，区分快照读取与事务写入；补充取消、达到最大次数、普通可重试错误和事务可重试错误测试。
- 改动事务错误路径时修改 `deleteBufferedKeys`，明确 rollback/commit 错误优先级和可观测性；对应测试应继续放在独立的 `deleter_test.rs`，不要内嵌进生产文件。
- 增加并行度或缓冲通道容量前，应验证同一行（特别是唯一多值索引）的上游 worker 亲和性，并检查取消时发送方、接收方能否都退出，避免死锁或并发写冲突。
- 扩展流量统计时注意：读取字节在 `LazyRefreshedSnapshot::BatchGet` 统计命中键和值，写入字节在事务开始前按待删键长度统计，重试可能重复累计写流量；改变口径需同步 `TrafficRecorder` 契约和测试断言。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录的 Rust/Go 文件均已索引。
- RustCodeGraph 源码/符号查询：`deleter.rs` 全部 275 行；`NewDeleter` 的下游 `NewBaseHandler`、`NewDataKVHandler`、`NewIndexKVHandler`、`NewLazyRefreshedSnapshot`，以及测试调用者 `do_test_deleter`、提交错误测试和事务冲突测试。
- 生产调用证据：`pkg/dxf/importinto/conflict_resolution.rs::ResolveConflictGroup` 第 191 行起，尤其第 225–229 行的 worker 构造/运行，以及第 282–297 行的关闭、join 和错误汇总。
- 抽象与下游证据：`pkg/dxf/importinto/conflictedkv/handler.rs` 中 `ConflictContext`、`ConflictStore`、`ConflictTransaction`、`Handler`、`EncodedRowHandler`、data/index handler 和 `LazyRefreshedSnapshot`。
- crate 与包语义：`pkg/dxf/importinto/conflictedkv/Cargo.toml`、`lib.rs`、`doc.go`。
- Go 对照：`pkg/dxf/importinto/conflictedkv/deleter.go`、`deleter_test.go`、`deleter_internal_test.go`。
- Rust 独立测试：`pkg/dxf/importinto/conflictedkv/deleter_test.rs`。它覆盖 data/index KV、重复输入、低阈值多次冲刷、读写流量、提交错误不落地，以及事务写冲突重试成功。
- 本任务为纯文档分析，按任务约束未运行 Cargo；结构通过任务规定的 11 个固定二级标题命令验证，人工复核未把推测写成已支持行为，也未复制整段源码。
