# `br/pkg/checkpoint/checkpoint.rs`

源文件：[`checkpoint.rs`](./checkpoint.rs)

Go 对照：[`checkpoint.go`](./checkpoint.go)

独立 Rust 测试：[`checkpoint_test.rs`](./checkpoint_test.rs)

## 文件定位

`checkpoint.rs` 是 `astersql-br-pkg-checkpoint` crate 的通用 checkpoint 核心。crate 入口 [`lib.rs`](./lib.rs) 在 `stubs` 和 `ticker` 之后声明并重新导出本模块，再由 [`backup.rs`](./backup.rs)、[`restore.rs`](./restore.rs)、[`log_restore.rs`](./log_restore.rs) 和 [`manager.rs`](./manager.rs) 注入具体路径、值编码和存储后端。它不决定“备份还是恢复”，而是提供两类可复用能力：一类是 `CheckpointRunner<K, V>` 的异步聚合、周期刷盘、锁续期和失败重试；另一类是 checkpoint 数据、checksum、metadata 的序列化、读取、校验和清理。

[`Cargo.toml`](./Cargo.toml) 将该目录定义为独立 library crate `astersql-br-pkg-checkpoint`，并以 `package.metadata.porting.go-package = "br/pkg/checkpoint"` 明确对应 Go 包。这里直接使用 `crossbeam-channel`、`serde`/`serde_json` 和 `sha2`；AES/CTR、随机数、UUID 等依赖主要由同 crate 的存储与桩边界消费，而不是本文件直接调用。

## 核心职责

1. **聚合进度**：`Append` 把 `CheckpointMessage<K, V>` 投递给主循环；主循环按 `GroupKey` 合并 `Group`，保持同一消息及同一键后续追加的顺序。`FlushChecksum`/`FlushChecksumItem` 单独收集按表 checksum。
2. **调度持久化**：`startCheckpointMainLoop` 建立 meta、checksum、lock 三种 ticker，把内存快照交给 `startCheckpointFlushLoop`；后者通过 `checkpointStorage` 写入具体后端并续期锁。
3. **容忍瞬时写失败**：`Flusher` 保存写失败的 meta/checksum 批次，并在 retry tick 到达时从尾部重试；结束时尽力清空失败队列。
4. **定义跨语言格式**：`RangeGroup`、`RangeGroupData`、`CheckpointData`、`ChecksumItem(s)`、`ChecksumInfo` 的 serde 字段名与 Go JSON tag 对齐；data 分片可加密，并对加密前明文计算 SHA-256。
5. **恢复与清理**：`parseCheckpointData`/`walkCheckpointFile` 恢复分组数据，`parseCheckpointChecksum`/`loadCheckpointChecksum` 聚合表级 checksum，`loadCheckpointMeta`/`saveCheckpointMetadata` 处理单一 metadata，`removeCheckpointData` 并发删除 checkpoint 后缀文件。

该文件只依赖抽象 `checkpointStorage` 和 `stubs::Storage`，不直接绑定对象存储、SQL 表、PD 或具体业务路径。这些边界分别由 `external_storage.rs`、`storage.rs`、各场景适配层及 `stubs.rs` 提供。

## 主要符号

- 常量：`CheckpointDir = "checkpoints"`；`MaxChecksumTotalCost = 60.0`；默认 meta/checksum/lock/retry 周期分别为 30 秒、5 秒、4 分钟、3 秒，`lockTimeToLive` 为 5 分钟。锁续期周期必须短于 TTL。
- 泛型约束：`KeyType` 要求 `Eq + Hash + Clone + Send + Sync + 'static`；`ValueType` 要求 `Clone + Send + Sync + 'static`。序列化入口再按需要补充 `Default`、`PartialEq`、`Serialize` 和 `Deserialize`。
- 数据结构：`RangeType` 表示起止键及文件列表；`CheckpointMessage<K, V>` 是追加消息；`RangeGroup<K, V>` 是按键合并后的逻辑组；`RangeGroupData` 保存密文、明文 checksum、IV 和明文大小；`CheckpointData` 是一批数据组及其相对耗时；`ChecksumItem(s)` 和 `ChecksumInfo` 描述表级校验信息及其外层完整性校验。
- 边界 trait：`GlobalTimer` 桥接 `stubs::GlobalTimer`；`checkpointStorage` 定义 data/checksum 写入、初始锁、锁续期和关闭。`initialLock` 的调用位于存储/场景适配层，本文件运行循环主要调用 `updateLock` 和 `close`。
- `CheckpointRunner<K, V>`：公开操作为 `FlushChecksum`、`FlushChecksumItem`、`Append`、`WaitForFinish`、`startCheckpointMainLoop`、`doChecksumFlush` 和 `doFlush`；其 `RunnerShared` 保存聚合 map、checksum 列表、编码闭包、存储、cipher 和首错状态。
- 刷盘内部：`doFlush_shared`、`doChecksumFlush_shared` 执行真正编码和写入；`Flusher` 维护失败队列；`startCheckpointFlushLoop` 消费三类内部通道。
- 读取/维护 API：`parseCheckpointData`、`walkCheckpointFile`、`loadCheckpointMeta`、`parseCheckpointChecksum`、`loadCheckpointChecksum`、`saveCheckpointMetadata`、`removeCheckpointData`。
- 测试钩子：`FAILED_AFTER_CHECKPOINT_FLUSHES_CHECKSUM` 及仅在 `cfg(test)` 下可见的 `set_failed_after_checkpoint_flushes_checksum_for_test`，模拟 checksum 已写成功但调用返回失败。

## 执行流程

Runner 的主要生命周期如下：

1. 场景适配层构造实现 `checkpointStorage` 的后端，并向 `newCheckpointRunner` 传入可选 `CipherInfo` 与 `ValueMarshaler`。构造函数建立 append/checksum 无界通道、done/err 容量为 1 的通道，以及 meta/checksum/lock 内部通道；此时尚未启动线程。
2. 适配层调用 `startCheckpointMainLoop`。它一次性 `take` 三组内部发送端/接收端，先启动 `startCheckpointFlushLoop`，再启动主循环线程和 flush、checksum、lock 三个 ticker；同一 Runner 不应重复启动，否则 `expect` 会失败。
3. 业务调用 `Append` 或 `FlushChecksumItem`。两者先检查 `Context`，再观察刷盘错误和 `err_closed`，成功时向对应通道发送。主循环把 append 消息合并进 `HashMap<K, RangeGroup<K, V>>`，把 checksum 追加进 `ChecksumItems.Items`。
4. meta 或 checksum ticker 到达时，`flushMeta_internal`/`flushChecksum_internal` 用 `std::mem::take` 原子替换当前批次，然后把旧批次送往刷盘线程。lock ticker 只发送一个 `()` 脉冲，实际时间与互斥判断由存储实现负责。
5. 刷盘线程收到 meta 后调用 `doFlush_shared`：逐组调用场景注入的 marshaler，按可选 cipher 加密，计算明文 SHA-256，封装成 `CheckpointData` 后调用 `flushCheckpointData`。收到 checksum 后序列化 `ChecksumItems`、计算 SHA-256、封装 `ChecksumInfo`，再调用 `flushCheckpointChecksum`。空批次不产生文件。
6. 普通 data/checksum 写失败不会立即终止 Runner，而是进入 `Flusher.incompleteMetas` 或 `incompleteChecksums`；retry tick 每次优先重试最后一个 meta，否则重试最后一个 checksum。锁续期失败则通过错误通道上报为致命错误，主循环记录首错并退出。
7. `WaitForFinish(_, flush)` 只发送一次 done，随后 join 所有线程并关闭存储。主循环收到 done 后先排空尚未处理的 append/checksum 消息；若 `flush` 为 `true`，再交出最终内存批次；随后关闭内部发送端并等待刷盘线程结束。因此调用方必须遵守 Go 注释中的约束：不要让 `WaitForFinish` 与新的 `Append` 并行。

读取路径与写入路径相反：`walkCheckpointFile` 只读取 `.cpt` 文件并调用 `parseCheckpointData`；解析时先解密、再用明文 SHA-256 验证、再反序列化 `RangeGroup` 并逐值回调。`loadCheckpointChecksum` 遍历指定目录中的每个对象，由 `parseCheckpointChecksum` 验证外层 checksum 后按 `TableID` 覆盖合并，最后与 data 读取一样返回观察到的最大 `DureTime`。

## 数据与状态

- `RunnerShared.meta` 和 `RunnerShared.checksum` 分别由 `Mutex` 保护；每次周期 flush 通过 `mem::take` 转移所有权，使生产者可立即在空容器继续聚合。
- `valueMarshaler` 是 `Arc<dyn Fn(...) + Send + Sync>`，允许主循环和刷盘线程共享场景编码策略；`cipher` 创建后只读。
- `err: RwLock<Option<Error>>` 保存首个致命错误，`err_closed: Mutex<bool>` 保证只发布一次。`Append` 与 `FlushChecksumItem` 在关闭后返回带调用来源前缀的错误。
- `metaCh`、`checksumMetaCh`、`lockCh` 的两端包在 `Mutex<Option<_>>` 中，仅用于启动时转移；这是一项“一次启动”不变量。
- `wg` 保存主循环线程句柄；刷盘线程句柄先存入 `flush_done`，由主循环在 done 分支 join。存储只在外层 `WaitForFinish` join 完毕后关闭，避免仍有写入时释放资源。
- `CheckpointData.DureTime` 与 `ChecksumInfo.DureTime` 使用 `duration_ns` 以纳秒表示；读取多个分片时只保留最大值。`HashMap` 的迭代顺序不稳定，因此不同运行的分片内组顺序不是格式契约。
- `RangeGroup.GroupKey` 是归并键；默认值通过 `skip_serializing_if = "is_default"` 省略，以匹配 Go 的 `omitempty`。`RangeGroupsEncriptedData` 保留了 Go 结构中的历史拼写。
- `Flusher` 的失败队列没有容量上限。持续写失败会增长内存；成功重试从尾部截断，语义是后进先出而非严格按时间顺序。

## 依赖与调用关系

上游通过同 crate 的适配层间接进入本文件：

- `backup.rs` 的 `StartCheckpointRunnerForBackup`/测试构造器提供备份路径与 `RangeType` 编码，`AppendForBackup` 最终投递 `CheckpointMessage`。
- `restore.rs` 的 `StartCheckpointRunnerForRestore` 和 `log_restore.rs` 的 `StartCheckpointRunnerForLogRestore` 提供各自值类型及 marshaler；`manager.rs` 的 table/storage manager 将启动、加载、保存、删除和关闭操作统一成管理接口。
- RustCodeGraph 的调用面显示，测试构造器由 `checkpoint_test.rs` 的备份、恢复、日志恢复和锁测试调用；更上层生产接线包括 `br/pkg/restore/snap_client/client.rs::InitCheckpoint` 及日志恢复客户端的 manager 创建流程。当前索引同时表明部分备份调用经 `br/pkg/backup/stubs.rs` 桥接，因此不能仅凭本文件宣称所有生产后端都已脱离桩。

下游依赖分为三层：

- `ticker::dispatcherTicker` 为主循环和重试循环提供可停止的可选接收端。
- `stubs::{Context, Storage, CipherInfo, Encrypt, Decrypt, NowDureTime, Error}` 提供取消、对象存储、加密、计时和错误链边界。
- `checkpointStorage` 的具体实现位于 `external_storage.rs` 和表存储相关模块；本文件仅调用 `flushCheckpointData`、`flushCheckpointChecksum`、`updateLock` 与 `close`。

关键内部调用边为：`Append`/`FlushChecksumItem` → `startCheckpointMainLoop` 聚合 → `flushMeta_internal`/`flushChecksum_internal` → `startCheckpointFlushLoop` → `Flusher.doFlush`/`doChecksumFlush` → `doFlush_shared`/`doChecksumFlush_shared` → `checkpointStorage`。读取边为：场景/manager 加载 API → `walkCheckpointFile` 或 `loadCheckpointChecksum` → 对应 parse 函数 → `Storage::ReadFile` 与业务回调。

## 错误处理与边界

- `Context::Done` 在 append、checksum、内部转交通道及两个循环中都会被检查；存在上下文错误时沿 `Error` 返回或发布。`WaitForFinish` 自身不返回错误，join panic 与最终尽力刷盘错误也不会向调用者显式传播。
- `send_error_static`/`sendError` 只记录首个致命错误，并以容量 1 的错误通道通知外部调用。主循环会把刷盘线程报告的锁续期错误升级为致命错误；普通 data/checksum 写失败则进入重试队列。
- `doFlush_shared` 在 marshaler、加密、JSON 编码或存储写入失败时返回错误；`doChecksumFlush_shared` 对 JSON 与存储错误同样传播。两个写后 failpoint 都刻意在存储成功后返回错误，因此重试可能产生重复分片，读取方和业务语义必须允许幂等合并。
- `parseCheckpointData` 对外层 JSON 损坏返回 `Ok(())` 并跳过整个分片；明文 checksum 不匹配时跳过该组；但解密失败、组 JSON 失败和业务回调失败会中断并上抛。`parseCheckpointChecksum` 同样忽略损坏外层 JSON 或 checksum 不匹配，但合法外层中的 `ChecksumItems` 反序列化错误会返回失败。
- `walkCheckpointFile` 仅处理 `.cpt`；目录遍历、读文件或回调错误会终止加载。`loadCheckpointChecksum` 不按后缀过滤，因此指定 checksum 目录必须只包含预期对象或可被“损坏即跳过”规则安全忽略的对象。
- `removeCheckpointData` 先收集 `.cpt`、`.meta`、`.lock`，再用 4 个 scoped worker 删除；零星删除失败被忽略，累计到第 16 个失败时保存首个达到阈值的错误、停止领取新任务并返回错误。非目标后缀不会删除。
- 互斥锁协议的安全边界在 `checkpointStorage.initialLock`/`updateLock` 实现中；本文件只保证默认 4 分钟续期小于 5 分钟 TTL，并把续期失败视为致命。

## 并发与资源生命周期

线程拓扑是“调用线程 + 主循环线程 + 刷盘线程”，另有 ticker 自身资源。`crossbeam_channel::Select` 让主循环同时等待三种 ticker、两种业务消息、done 与刷盘错误；刷盘循环同时等待 meta、checksum、lock、retry，并以 50 毫秒 `select_timeout` 周期重新检查取消。主循环结束会停止三个 ticker，刷盘线程结束会停止 retry ticker。

append/checksum 使用无界通道，所以调用通常不会因主循环处理速度而背压，代价是极端生产速度下队列可持续增长；这与 Go 对照的无缓冲业务通道存在实现差异。Rust 在 done 分支显式 drain 两个队列，以补偿无界通道和结束竞态，保证 `WaitForFinish(flush=true)` 能看到 done 之前已成功发送的尾部消息，但调用方仍不得在关闭期间继续发送。

刷盘通道也为无界通道；meta/checksum 周期交出的是独立拥有的批次，后续聚合不会修改已排队批次。刷盘线程要等 meta、checksum、lock 三个发送端都关闭才正常退出，关闭前会分别尽力处理对应失败队列。`WaitForFinish` 的顺序是：发 done → 主循环排空和可选最终 flush → 关闭内部通道 → join 刷盘线程 → join 主线程 → `checkpointStorage.close()`。

`removeCheckpointData` 的共享迭代器由 `Mutex` 串行分配文件，实际删除在锁外并行；`AtomicI64` 和 `AtomicBool` 使用 `SeqCst` 统一失败计数和停止标志的可见性。相关测试要求并发峰值确实达到 4。

## 与 Go 版本的对应关系

Rust 文件按 [`checkpoint.go`](./checkpoint.go) 的类型和函数布局移植：同名常量、`RangeGroup*`/`CheckpointData`/`Checksum*` JSON 字段、`CheckpointRunner` 两级循环、`flusher` 失败队列、读写辅助函数和 16 个删除失败阈值均有直接对应。SHA-256 针对加密前明文，`DureTime` 使用纳秒表示，写后 failpoint、默认周期、4 工作者删除策略也与 Go 保持一致。

需要注意的实现差异如下：

- Go 用 goroutine、原生 channel、`select`、`sync.WaitGroup`；Rust 用 `std::thread`、crossbeam 通道、`Select`、`JoinHandle` 和锁。Rust 的 append/checksum 通道是无界的，而 Go 是无缓冲的；Rust 因此增加 done 前 drain。
- Go 的 `RangeGroup` 使用 `omitempty`，Rust 用 `Default + PartialEq` 与 `skip_serializing_if` 实现同样的零值省略。Rust 测试 `range_group_omits_zero_group_key_like_go_json` 固定这一兼容点。
- Go 依赖 `metautil.Encrypt`、`utils.Decrypt`、外部存储和 PD 时间接口；Rust 当前通过 `stubs` trait/函数隔离这些依赖。文档只能确认接口与算法对齐，不能据此推断所有后端均为完整生产实现。
- Go 在 flush 失败和损坏分片时记录日志；Rust 的部分对应分支只保留/跳过数据而不记录日志，因此可观测性弱于 Go，但控制流保持一致。
- Rust 清理使用 `thread::scope` 和共享迭代器模拟 Go `util.NewWorkerPool(4)` + `errgroup`。两者都在第 16 次失败时返回错误，但并发任务的具体停止时点可能不同。
- Go 的 `checkpoint-more-quickly-flush` failpoint 可在主循环启动时重写 tick；本文件的 `startCheckpointMainLoop` 没有对应分支，测试改由 `Start*ForTest` 显式注入短周期。这是当前可见差异，修改相关测试/运维行为时应重新评估。

[`checkpoint_test.rs`](./checkpoint_test.rs) 与 [`checkpoint_test.go`](./checkpoint_test.go) 保持场景对应：metadata、备份、storage/table 恢复、重试/不重试、日志恢复和锁测试均有同名或同意图覆盖。Rust 还增加了 checksum 写后 failpoint、4 工作者并发与零值 group-key JSON 的聚焦回归测试。

## 扩展指南

- 新增 checkpoint payload 字段时，先修改对应 serde 结构及 Go JSON 结构，明确缺省值与旧数据读取规则；同步检查 `doFlush_shared`/`parseCheckpointData` 或 checksum 写读两端，并在独立的 `checkpoint_test.rs` 增加跨版本 JSON、损坏输入和加密场景。不要把测试嵌入 `checkpoint.rs`。
- 新增业务场景时，优先在新的场景适配文件提供 `ValueMarshaler`、路径和 `checkpointStorage`，复用 `newCheckpointRunner`；只有通用调度或格式确需变化时才修改本文件。还应在 `lib.rs` 接线，并为 manager/场景入口建立独立测试。
- 修改 tick 或锁 TTL 时必须维持“续期周期严格短于 TTL”，同步 Go 常量、锁冲突/续期测试及测试构造器。若允许零周期，需确认 `dispatcherTicker` 的禁用语义。
- 修改通道容量、done 流程或错误发布时，要重点验证：done 前成功发送的消息不丢失、只启动一次、只关闭一次、刷盘完成后才关闭存储、首错不会被覆盖，以及取消不会留下未 join 线程。
- 修改重试策略时需决定是否继续允许重复分片；当前写后失败会重写同一批次，所以上层读取与合并不能假设 exactly-once。若加入上限或退避，还要处理当前无界失败队列的内存风险。
- 修改清理逻辑时只扩大明确授权的后缀集合，保留“先遍历再删除”以避免存储遍历期间变更，并用 `remove_checkpoint_data_uses_go_worker_pool_concurrency` 或新独立测试验证并发度、错误阈值和非目标文件保留。
- 任何修改都应同时核对 `checkpoint.go`。如果有意偏离 Go，应在测试和文档中说明兼容性、持久化格式、性能及回滚影响，而不是只让 Rust 测试通过。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件，目标 `br/pkg/checkpoint/checkpoint.rs` 已索引，共识别 109 个符号；使用 `files --filter br/pkg/checkpoint`、`explore "checkpoint.rs in br/pkg/checkpoint: public types traits constants and control flow"`、对 `newCheckpointRunner`/`parseCheckpointData`/`removeCheckpointData` 的 `query`，以及按文件分段的 `node --file` 阅读确认符号与调用面。
- 直接阅读：[`checkpoint.rs`](./checkpoint.rs) 全部 1,166 行；[`Cargo.toml`](./Cargo.toml)；[`lib.rs`](./lib.rs)；Go 对照 [`checkpoint.go`](./checkpoint.go)；Rust 独立测试 [`checkpoint_test.rs`](./checkpoint_test.rs)；Go 测试 [`checkpoint_test.go`](./checkpoint_test.go)。
- RustCodeGraph 调用证据：`StartCheckpointRunnerForBackup`、`StartCheckpointRunnerForRestore`、`StartCheckpointRunnerForLogRestore` 和 manager 层连接场景入口；`checkpoint_test.rs` 调用测试构造器并覆盖 backup/restore/log restore/lock；更上层恢复接线可追到 `br/pkg/restore/snap_client/client.rs::InitCheckpoint`。
- 关键测试证据：`checksum_flush_reports_go_post_write_failpoint` 验证先写后报错；`remove_checkpoint_data_uses_go_worker_pool_concurrency` 验证 4 路并发；`range_group_omits_zero_group_key_like_go_json` 验证 JSON 零值省略；`test_checkpoint_backup_runner` 验证追加、加密读取、checksum 与 metadata；`test_checkpoint_runner_retry*`/`test_checkpoint_runner_no_retry*` 验证允许重试重复和正常路径恰一次；`test_checkpoint_runner_lock` 验证锁冲突生命周期。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付结构检查要求文档存在且恰含十一个固定二级标题；具体命令和退出状态记录在最终交付说明中。
