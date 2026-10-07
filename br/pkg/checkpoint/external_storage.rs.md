# `br/pkg/checkpoint/external_storage.rs`

## 文件定位

[`external_storage.rs`](external_storage.rs) 属于 `astersql-br-pkg-checkpoint` library crate；`br/pkg/checkpoint/Cargo.toml` 以 `lib.rs` 为 crate 根，`lib.rs` 将本模块声明为公开模块并通过 `pub use external_storage::*` 展平导出。它位于通用 `CheckpointRunner`（`checkpoint.rs`）与对象存储抽象 `Storage`（`stubs.rs`）之间：上层选择 checkpoint 目录和是否启用锁，本文件负责把数据分片、校验分片和锁文件落实为外部存储对象。

当前有两类直接装配入口：`backup.rs::{StartCheckpointBackupRunnerForTest, StartCheckpointRunnerForBackup}` 传入 `Some(timer)` 和备份锁路径，启用互斥锁；`manager.rs` 中 `StorageMetaManager` 的快照恢复、日志恢复 `StartCheckpointRunner` 传入 `None`，并使用 `flushPathForRestore` 产生的空 `CheckpointLockPath`，因此只使用分片落盘能力。该文件不是门面或生成代码，而是 `checkpointStorage` trait 的外部对象存储实现。

## 核心职责

本文件承担三组职责：

1. 定义 restore checkpoint 的目录/文件布局，并通过 `flushPathForRestore` 和五个 `getCheckpoint*ByName` 函数按 `taskName` 生成实际路径。
2. 由 `externalCheckpointStorage` 实现 `checkpointStorage`：将 Runner 交付的数据和 checksum 字节分别写入 UUID 命名的 `.cpt` 对象，并保留 `close` 生命周期钩子。
3. 为 backup 场景实现基于 TSO 的租约式互斥：构造时初始化锁，Runner 定时调用 `updateLock` 续期，`CheckpointLock` 以 JSON 保存持有者 ID 和过期物理时间。

它不负责 checkpoint 内容的序列化、加密、摘要计算或重试队列；这些由 `checkpoint.rs::{doFlush_shared, doChecksumFlush_shared, Flusher}` 完成。本文件也不删除历史 checkpoint 文件，restore 的加载、元数据保存和删除由 `manager.rs` 调用 `checkpoint.rs` 的辅助函数完成。

## 主要符号

- 六个公开路径格式常量 `CheckpointRestoreDirFormat`、`CheckpointDataDirForRestoreFormat`、`CheckpointChecksumDirForRestoreFormat`、`CheckpointMetaPathForRestoreFormat`、`CheckpointProgressPathForRestoreFormat`、`CheckpointIngestIndexPathForRestoreFormat` 固定 `checkpoints/restore-%s/...` 协议。Rust 使用 `%s` 作为可由调用方 `replace` 的展示模板；具体路径辅助函数直接用 `format!("checkpoints/restore-{taskName}/...")`。
- `flushPathForRestore(taskName: &str) -> flushPath` 组装 data/checksum 路径，并明确把 `CheckpointLockPath` 置为空串；它供 `manager.rs` 的两种外部 restore Runner 使用。
- `getCheckpointMetaPathByName`、`getCheckpointDataDirByName`、`getCheckpointChecksumDirByName`、`getCheckpointProgressPathByName`、`getCheckpointIngestIndexPathByName` 分别生成任务级 meta、分片目录、checksum 目录、日志恢复进度和 ingest-index 修复信息路径。
- `externalCheckpointStorage` 保存 `flushPath`、共享的 `Arc<dyn Storage>`、受 `Mutex<u64>` 保护的 `lockId`，以及可选的 `Arc<dyn GlobalTimer>`。结构本身不公开导出锁 ID 和 timer，强制锁状态经内部方法维护。
- `newExternalCheckpointStorage(...) -> Result<Arc<externalCheckpointStorage>>` 是唯一构造入口。只有 `timer.is_some()` 时才同步执行 `initialLock`；初始化失败不会返回半初始化实例。
- `getTS` 使用 `WithRetry(..., 32)` 获取 `(physical, logical)` TSO；`flushLock` 序列化并写锁；`checkLockFile` 读取、反序列化并验证锁；三者是锁协议的内部步骤。
- `checkpointStorage` 实现提供 `flushCheckpointData`、`flushCheckpointChecksum`、`initialLock`、`updateLock`、`close`。前两者每次通过 `Uuid::new_v4().simple()` 生成无连字符文件名，避免多个 Runner 分片覆盖同一固定对象。
- `CheckpointLock { LockId, ExpireAt }` 通过 serde 字段重命名为 JSON 的 `lock-id`、`expire-at`，与 Go 持久化格式兼容。
- `FAILED_AFTER_CHECKPOINT_UPDATES_LOCK` 与仅在 `cfg(test)` 下可见的 `set_failed_after_checkpoint_updates_lock_for_test` 模拟 Go 的 `failed-after-checkpoint-updates-lock` 故障点；触发位置在锁文件写成功之后。

## 执行流程

备份构造流程为：`StartCheckpointRunnerForBackup`/测试入口构造备份 `flushPath` → `newExternalCheckpointStorage` 保存依赖 → 因 timer 存在而调用 `initialLock`。`initialLock` 先由 `getTS` 获取 TSO，并以 `ComposeTS(physical, logical)` 形成当前实例 `lockId`；若锁对象存在则以当前物理时间校验；随后写入 `ExpireAt = physical + lockTimeToLive` 的新锁，阻塞等待 3 秒，再次读取同一锁文件，以发现初始化窗口中被另一 BR 覆盖的竞态。

Runner 写数据时，`checkpoint.rs::doFlush_shared` 已完成分组序列化、可选加密和明文 SHA-256，随后调用 `flushCheckpointData` 写入 `{CheckpointDataDir}/{uuid}.cpt`。checksum 路径类似：`doChecksumFlush_shared` 组装并序列化 `ChecksumInfo` 后调用 `flushCheckpointChecksum`，写入 `{CheckpointChecksumDir}/{uuid}.cpt`。底层 `Storage::WriteFile` 的结果原样成为本次调用结果。

备份 Runner 的 lock ticker 到期后，flush 线程在 `checkpoint.rs::startCheckpointFlushLoop` 调用 `updateLock`：重新取物理 TSO → 确认外部锁仍属于本实例 → 用相同 `lockId` 和新的过期时间覆写锁文件 → 执行写后故障注入检查。错误会被 Runner 注解为 `failed to update checkpoint lock.` 并作为首个致命错误上报。

restore 流程由 `manager.rs::StorageMetaManager::StartCheckpointRunner` 传 `None` timer，并把 lock tick 设为 `Duration::ZERO`；因此构造阶段跳过 `initialLock`，运行阶段也不产生 lock tick，只写 data/checksum 分片。结束时 `CheckpointRunner::WaitForFinish` 汇合线程并调用 `close`；当前实现为空操作。

## 数据与状态

持久化数据分为四类：data `.cpt` 分片、checksum `.cpt` 分片、`CheckpointLock` JSON，以及本文件只负责命名而由 manager/checkpoint 辅助函数读写的 meta/progress/ingest-index 文件。分片文件名不承载业务顺序或键；恢复时依赖目录遍历和分片内容，不能从 UUID 推导先后关系。

`lockId` 是一次 `externalCheckpointStorage` 实例生命周期内不变的持有者身份，值来自 `ComposeTS(p, l)`；`ExpireAt` 则每次写锁时更新为 TSO 的物理毫秒值加 5 分钟 `lockTimeToLive`。比较必须保持同一时间量纲，不能改用本地 wall clock。`timer: None` 表示“禁用锁初始化”，不是可在运行期补装 timer 的状态。

`Arc<dyn Storage>` 允许 Runner 线程共享底层存储；`Mutex<u64>` 保护锁 ID 的跨线程读取/写入。`FAILED_AFTER_CHECKPOINT_UPDATES_LOCK` 是进程级测试状态，使用 `SeqCst`，测试必须在结束前复位，避免影响同进程其他用例。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 文件索引与源码引用共同确认：`backup.rs` 两个启动函数以 `Some(timer)` 调用 `newExternalCheckpointStorage`；`manager.rs` 的 snapshot/log `StorageMetaManager::StartCheckpointRunner` 以 `None` 调用它，并共同使用 `flushPathForRestore`。同一 manager 还调用五个路径辅助函数加载、保存、探测或删除 restore checkpoint 相关对象。

下游关系中，`externalCheckpointStorage` 实现 `checkpoint.rs::checkpointStorage`，由 `newCheckpointRunner` 以 trait object 持有。`checkpoint.rs::{doFlush_shared, doChecksumFlush_shared, startCheckpointFlushLoop, CheckpointRunner::WaitForFinish}` 分别调用其 data 写入、checksum 写入、续锁和关闭方法。锁逻辑继续调用 `stubs.rs` 的 `GlobalTimer::GetTS`、`WithRetry`、`ComposeTS` 和 `Storage::{FileExists, ReadFile, WriteFile, URI}`。

crate 直接依赖方面，`Cargo.toml` 声明 `serde`/`serde_json` 用于锁 JSON，`uuid` 的 `v4` feature 用于分片名；本文件还使用标准库的 `Arc`、`Mutex`、`AtomicBool`、线程 sleep 和 `Duration`。具体对象存储 SDK 被隔离在 `Storage` trait 之后，不在本文件直接依赖。

RustCodeGraph 对 `external_storage.rs` 建立了 34 个符号并识别到 7 个使用文件；精确 `query` 区分出了 Rust 的 `newExternalCheckpointStorage`、`flushPathForRestore`、`externalCheckpointStorage`、`CheckpointLock` 与同名 Go 节点。精确 callers 命令在本次验证中持续无输出而被中止，因此调用边又通过上述 `rg` 引用位置和对应源码逐一核实，未把图中同名 Go 边当作 Rust 边。

## 错误处理与边界

`newExternalCheckpointStorage`、TSO 获取、对象存在性检查、读写和 JSON 编解码都使用 `Result` 与 `?` 向上传播。`getTS` 在 32 次策略调用后仍失败时返回最后错误；timer 缺失却直接调用锁方法时返回 `timer is nil`。restore 正常路径通过不启动 lock tick 避免这一错误，但扩展调用方不能在 `timer: None` 时主动调用 `initialLock` 或 `updateLock`。

`checkLockFile` 的判定分三种：锁已过期且对方 `LockId > my_id` 时拒绝覆盖，以防“更晚启动者先写锁”的异常顺序；锁未过期且持有者不同则返回剩余秒数、对方 ID 和手工删除 URI；其余情况（已过期且 ID 不大于本实例，或有效锁属于本实例）允许继续。存储读取失败、无效 JSON 和 URI/路径错误均直接失败，不会把损坏锁当作无锁。

初始化锁是“检查—写入—等待—复检”的竞争检测协议，并非对象存储原子 compare-and-swap；3 秒复检降低双写漏检，但不能提供强事务锁语义。`flushCheckpointData`/`flushCheckpointChecksum` 每次只执行一次 `WriteFile`，本文件没有写重试；失败分片的调度重试由 `checkpoint.rs::Flusher` 管理。UUID 冲突概率极低但没有显式存在性检查。

`lockId.lock().unwrap()` 在 mutex 被 poison 时会 panic，而不是返回 `Error`。`thread::sleep(3s)` 是同步阻塞，调用构造函数的线程必须接受该延迟。`close` 不删除锁文件；正常退出后依靠租约过期，而非主动释放。

## 并发与资源生命周期

构造成功后存储以 `Arc` 交给 `CheckpointRunner` 的共享状态，在主循环和 flush 线程间共享。data/checksum 写入可因不同批次发生多次，UUID 文件名使它们无需争用一个固定输出名；底层 `Storage` 是否允许并发访问由其 `Send + Sync` trait 边界保证。

锁初始化在 Runner 启动前同步完成，因而备份工作不会在初始互斥检查通过前开始。运行期主循环的 lock ticker 向 flush 线程的 lock channel 发信号，flush 线程串行执行 `updateLock`；默认 4 分钟续期间隔小于 5 分钟 TTL。任何续锁错误都会停止该 flush 循环分支并上报 Runner，而不是静默继续。

`WaitForFinish` 先通知结束、join 工作线程，再调用 `close`。本实现没有文件句柄、后台线程或显式锁删除需要释放，所以 `close` 为空；真正的线程、ticker 和 channel 生命周期由 `checkpoint.rs` 管理。restore 以 `Duration::ZERO` 创建无 channel 的 lock ticker，并因 timer 为空跳过初始锁，避免对空锁路径访问。

## 与 Go 版本的对应关系

Rust 文件逐项对应同目录 `external_storage.go`：路径常量与辅助函数、`externalCheckpointStorage` 字段、可选 timer 构造语义、UUID `.cpt` 分片、TSO 组合、5 分钟租约、已有锁判断、3 秒初始化复检、续锁和空 `close` 均保持相同主流程；`CheckpointLock` 的 JSON 字段名及两类冲突错误文案也保持兼容。

主要语言适配是 Go 的嵌入 `flushPath` 在 Rust 中变为命名字段，接口值变为 `Arc<dyn Storage>`/`Arc<dyn GlobalTimer>`，普通 `uint64` 锁 ID 变为 `Mutex<u64>`，Go error wrapping 变为 Rust `Result` 传播。Go 的 `utils.NewAggressivePDBackoffStrategy()` 在当前 Rust 桩接口中体现为 `WithRetry(..., 32)`；`external_storage_test.rs::get_ts_uses_go_aggressive_pd_attempt_count` 以 8 次失败后第 9 次成功验证会继续重试。

Go 使用 failpoint 框架注入 `failed-after-checkpoint-updates-lock`；Rust 使用 `AtomicBool` 测试钩子复现相同的“锁已写入、随后返回错误”时点，`external_storage_test.rs::update_lock_preserves_go_post_write_failpoint` 验证其错误文本。Go `checkpoint_test.go::TestCheckpointRunnerLock` 与 Rust `checkpoint_test.rs::test_checkpoint_runner_lock` 都覆盖旧锁冲突、过期锁接管和新实例冲突三阶段。

可观察差异是 Go `getTS` 会记录每次失败重试日志、`flushLock`/过期自有锁也会记录日志；当前 Rust 实现没有对应日志，仅保留控制流和错误结果。扩展监控时应补齐日志/指标而不改变锁判定。Rust 测试钩子是进程级原子状态，Go failpoint 则由 failpoint 运行时管理，测试隔离方式不同。

## 扩展指南

新增 restore 路径类型时，应同时更新格式常量、对应 `getCheckpoint*ByName` 函数、`manager.rs` 的读写入口及独立 `*_test.rs`；不要仅在调用点拼接路径，以免读取、保存、清理使用不同前缀。路径格式属于持久化兼容协议，改变 `restore-{taskName}` 或 JSON 字段名前需考虑已存在 checkpoint 的可恢复性。

调整锁策略时，最可能修改 `getTS`、`checkLockFile`、`initialLock`、`updateLock` 和 `CheckpointLock`。必须保持 TSO physical 与 `ExpireAt` 的毫秒量纲、4 分钟续期小于 5 分钟 TTL 的不变量，以及“写后故障仍表示外部状态已改变”的语义；对应测试应放在独立的 `external_storage_test.rs` 或 `checkpoint_test.rs`，不得内嵌进生产文件。若要强化竞态保证，应优先扩展 `Storage` 的条件写接口，而不是缩短 3 秒等待冒充原子性。

新增写入策略、压缩或命名规则时，应确认 `walkCheckpointFile` 对文件后缀和内容格式的读取假设，并为重复写、部分失败和对象存储最终一致性补独立测试。不要在本实现内部重复 Runner 已负责的内容序列化、加密和失败队列逻辑。增加 Cargo 依赖或 feature 时需同步 `br/pkg/checkpoint/Cargo.toml`，并评估分片写入路径的性能和二进制体积。

restore 当前依赖 `timer: None + Duration::ZERO + 空锁路径` 三者配合来禁用锁；若未来让 restore 使用锁，应一次性提供非空锁路径、timer 和非零续期间隔，并增加多实例冲突、过期接管、关闭后过期的测试，避免只改变其中一项而访问空对象名或在 TTL 后失锁。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/checkpoint` 确认目标、模块入口、Go 对照和独立测试均已索引；`node --file` 完整读取了 `external_storage.rs` 261 行及 `external_storage_test.rs` 70 行；精确 `query` 确认 Rust/Go 同名符号节点。callers 查询无输出并在等待后中止，故未将其作为完成证据。
- 生产源码：`br/pkg/checkpoint/external_storage.rs`（全部符号和控制流）、`br/pkg/checkpoint/checkpoint.rs`（`flushPath`、`lockTimeToLive`、`checkpointStorage`、Runner 写入/续锁/关闭调用）、`br/pkg/checkpoint/backup.rs`（带锁构造入口）、`br/pkg/checkpoint/manager.rs`（restore 路径和无锁构造入口）、`br/pkg/checkpoint/lib.rs`（模块装配和导出）。
- crate/对照：`br/pkg/checkpoint/Cargo.toml` 核对 crate 边界及 serde/uuid 依赖；`br/pkg/checkpoint/external_storage.go` 核对路径、落盘、TSO 锁协议、错误文案和 failpoint 时点。
- 测试：`br/pkg/checkpoint/external_storage_test.rs` 覆盖 TSO 重试次数和写锁后故障注入；`br/pkg/checkpoint/checkpoint_test.rs::test_checkpoint_runner_lock` 与 Go `br/pkg/checkpoint/checkpoint_test.go::TestCheckpointRunnerLock` 覆盖锁互斥三阶段。
- 调用边补证：对 `newExternalCheckpointStorage`、路径辅助函数、`checkpointStorage` 方法和锁相关符号执行限定 Rust 文件的 `rg`，再读取命中的调用片段，确认上游/下游关系；没有运行 Cargo，符合本任务纯文档约束。
