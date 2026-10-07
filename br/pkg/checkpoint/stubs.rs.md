# `br/pkg/checkpoint/stubs.rs`

## 文件定位

`stubs.rs` 是 `astersql-br-pkg-checkpoint` crate 的本地依赖边界层。crate 入口 `br/pkg/checkpoint/lib.rs` 先以 `pub mod stubs` 挂载它，再用 `pub use stubs::*` 展平导出；因此 `checkpoint.rs`、`external_storage.rs`、`storage.rs`、`manager.rs` 以及备份、恢复和日志恢复适配层都可以复用这里的类型。

文件名虽为 stubs，但并非单一测试 mock：它同时承载 checkpoint Rust 移植目前所需的错误、取消上下文、对象存储、加密、TSO、SQL/Domain 边界、序列化适配和 failpoint。其实现刻意缩小了 Go 依赖面的能力，只适合当前 crate 的本地运行和对等测试；`MemStorage`、`MockTimer` 以及 Session/Domain traits 不能等同于真实 TiDB、PD 或对象存储客户端。

crate 边界由 `br/pkg/checkpoint/Cargo.toml` 确认：包名是 `astersql-br-pkg-checkpoint`，库入口为 `lib.rs`，移植元数据指向 Go 包 `br/pkg/checkpoint`。本文件直接使用的外部依赖主要是 `aes`、`ctr`、`rand`、`serde` 和 `serde_json`。

## 核心职责

本文件把 Go checkpoint 实现依赖的多个外部包收敛成可注入的 Rust 接口和小型本地实现：

- 以 `Error`、`Result<T>` 和 `Context` 提供最小错误传播与取消检查能力。
- 以 `Storage` trait 定义 checkpoint 文件的遍历、读写、删除、存在性检查和 URI 查询，并以 `MemStorage` 提供线程安全的内存实现。
- 以 `EncryptionMethod`、`CipherInfo`、`Encrypt`、`Decrypt` 实现 checkpoint 数据使用的 AES-CTR 子集。
- 以 `GlobalTimer`、`MockTimer` 和 `ComposeTS` 隔离 PD TSO 获取及混合时间戳合成。
- 以 `Session`、`RestrictedSQLExecutor`、`Glue`、`Domain`、`InfoSchema`、`SqlValue`、`SqlRow` 描述表后端所需的最小 TiDB 边界。
- 以 `File`、`KeyRange`、`Range`、`ClusterConfig`、`CIStr`、`TiFlashReplicaInfo` 和 `duration_ns` 保持 checkpoint JSON 的关键跨语言形状。
- 以 `failpoint` 模块模拟一条刷盘后失败注入点，供独立回归测试验证 Go 行为。

这些职责是“移植期适配”而非完整基础设施实现。例如 `Context` 没有 deadline/value，`WithRetry` 没有退避等待或多错误聚合，`Error::Trace` 不记录栈，`MemStorage` 不持久化也不模拟权限和网络失败。

## 主要符号

- `pub type Result<T> = std::result::Result<T, Error>` 与 `Error { msg }`：crate 内的轻量错误协议。`Errorf` 是构造别名，`Annotate`/`Annotatef` 添加 `"context: original"` 前缀，`Trace` 原样返回；同时实现 `Display`、`std::error::Error` 以及从 JSON/IO 错误的转换。
- `Context`：内部是 `Arc<Mutex<Option<Error>>>`。`Background` 创建未取消上下文，`cancel` 写入原因，`Err` 返回原因副本，`Done` 判断是否取消。`cancel` 不是 public，当前 crate 外不能主动取消它。
- `CrypterIvLen = 16`、`PhysicalShiftBits = 18` 与 `ComposeTS`：分别固定 CTR IV 长度和 TiDB TSO 的物理位移；`ComposeTS` 计算 `(physical << 18) | logical`。
- `EncryptionMethod`、`CipherInfo`、`Encrypt`、`Decrypt`、私有 `aes_crypt_ctr`：支持明文及 AES-128/192/256 CTR；AES 变体实际按密钥长度选择，并要求 16 字节 IV。
- `Storage` 与 `MemStorage`：前者是 `Send + Sync` 的对象存储最小接口；后者以 `Mutex<HashMap<String, Vec<u8>>>` 保存对象。`paths` 返回排序后的路径快照，便于确定性断言。
- `WalkOption { SubDir }`：目前唯一遍历选项。`MemStorage::WalkDir` 将首尾 `/` 去除后做路径段近似过滤，并按路径排序回调。
- `NowDureTime`：依赖进程级 `SUMMARY_START: Mutex<Option<Instant>>`，第一次调用建立起点，以后返回从该起点开始的经过时间。
- `WithRetry`：最多执行 `max_attempts.max(1)` 次闭包；每次执行前检查 `Context::Err`，成功立即返回，耗尽后返回最后一次错误。
- `GlobalTimer` 与 `MockTimer`：前者抽象 `GetTS`，后者始终返回构造时的 `(p, l)`，不检查上下文。
- `Session`、`RestrictedSQLExecutor`、`Glue`、`Domain`、`InfoSchema`：表 checkpoint 的依赖倒置接口。它们只覆盖内部 SQL、受限查询、Session 创建、Store/InfoSchema 访问、表存在性与库内表枚举。
- `SqlValue` 与 `SqlRow`：SQL 参数和结果的简化表示。`GetBytes` 只接受 Bytes/Str，`GetUint64` 只接受 U64/I64；不匹配时返回空切片或 0，但越界索引会 panic。
- `duration_ns`：把 `Duration` 作为纳秒 `i64` 序列化；反序列化时把 `i64` 强转为 `u64`。
- `failpoint::{Enable, Disable, failed_after_checkpoint_flushes}`：只识别完整名称 `github.com/pingcap/tidb/br/pkg/checkpoint/failed-after-checkpoint-flushes`，状态保存在 `AtomicBool` 中。

## 执行流程

外部对象存储流程中，`backup.rs`、`checkpoint.rs` 和 `manager.rs` 接收 `Arc<dyn Storage>`。写入时，`checkpoint.rs::doFlush_shared` 将范围组序列化，调用 `Encrypt` 生成密文和 IV，再经 checkpoint storage 写入；读取时，`checkpoint.rs::parseCheckpointData` 调用 `Decrypt` 还原内容。枚举与清理路径通过 `Storage::WalkDir`、`ReadFile`、`DeleteFile` 等接口完成；测试通常注入 `MemStorage`。

锁流程中，`external_storage.rs::getTS` 使用本文件的 `WithRetry` 调用 `GlobalTimer::GetTS`，`initialLock` 再用 `ComposeTS` 合成锁拥有者 ID。生产语义需要由调用方注入真实 timer；测试以 `MockTimer` 或自定义 `EventuallySuccessfulTimer` 控制失败次数和返回值。

表存储流程中，`manager.rs` 通过 `Glue::CreateSession` 获取 `Session`，`storage.rs` 用 `ExecuteInternal` 写表、用 `RestrictedSQLExecutor::ExecRestrictedSQL` 读取数据，并通过 `Domain::InfoSchema` 判断表或数据库是否仍可安全清理。`SqlValue`/`SqlRow` 是这条路径上的最小参数与结果载体。

刷盘错误注入流程中，测试调用 `failpoint::Enable`；`checkpoint.rs::doFlush_shared` 在完成存储写入后读取 `failed_after_checkpoint_flushes()`，开启时返回指定失败。这使回归测试能验证“写入已经发生、随后报告失败”的时序。

## 数据与状态

`File`、`KeyRange`、`Range`、`ClusterConfig`、`CIStr` 和 `TiFlashReplicaInfo` 都是跨模块传递或 JSON 持久化的数据形状。字段上的 `serde` rename/flatten/skip 规则是兼容协议的一部分：例如 `Range` 将 `KeyRange` 展平，`CIStr` 保留 `O`/`L`，`File` 使用 `name`、`start_key`、`end_key`。修改字段名、默认值或省略规则会影响已有 checkpoint 的可读性。

共享可变状态有三处：`Context.cancelled` 在所有 clone 间共享；`MemStorage.files` 为实例级互斥映射；`SUMMARY_START` 和 failpoint 的 `FAILED_AFTER_FLUSHES` 是进程级全局状态。`MemStorage::ReadFile` 和 `paths` 返回数据副本，因此调用方不会持有映射内借用。`WalkDir` 也先在锁内生成条目快照、释放锁后再执行用户回调，避免回调重入时持有 storage mutex。

`Encrypt` 每次 AES 加密都生成随机 16 字节 IV；IV 必须与密文一起保存，并在 `Decrypt` 时使用同一密钥。CTR 模式不提供认证，本文件本身不检测密文篡改；checkpoint 上层另行保存并校验 SHA-256。

## 依赖与调用关系

上游入口由 `br/pkg/checkpoint/lib.rs` 导出。本文件的主要直接调用关系经 RustCodeGraph 和源码核对如下：

- `checkpoint.rs::doFlush_shared` 调用 `Encrypt`、`NowDureTime` 和 `failpoint::failed_after_checkpoint_flushes`。
- `checkpoint.rs::parseCheckpointData` 调用 `Decrypt`；`checkpoint.rs` 还使用 `Storage`、`Context`、`WalkOption` 与 `duration_ns`。
- `external_storage.rs::getTS` 调用 `WithRetry`，`initialLock` 调用 `ComposeTS`，外部存储实现持有 `Arc<dyn Storage>` 与可选 `Arc<dyn GlobalTimer>`。
- `backup.rs` 使用 `CipherInfo`、`File`、`Storage` 和 `GlobalTimer` 组装备份 checkpoint。
- `storage.rs` 使用 SQL/Session traits 实现表后端；`manager.rs` 使用 `Glue`、`Domain`、`InfoSchema` 选择、创建和清理表后端或存储后端。
- `log_restore.rs` 使用 `CIStr`、`Session` 和 `TiFlashReplicaInfo` 保存日志恢复元数据。

向下依赖方面，AES-CTR 由 `aes` 和 `ctr` 实现，IV 由 `rand::thread_rng` 生成，JSON 数据形状由 `serde`/`serde_json` 支持；同步完全使用标准库 `Arc`、`Mutex` 和原子变量。本文件没有真实 PD、TiKV、SQL 引擎、网络或文件系统客户端依赖。

## 错误处理与边界

错误只保留文本，不能承载 Go `pingcap/errors` 的 cause、错误类别或栈信息。`Mutex::lock().unwrap()` 在锁中毒时会 panic，而不是返回 `Error`。`SqlRow` 访问越界也会 panic；匹配错误的列类型则静默返回空值，扩展查询逻辑时必须避免把类型错误误判成真实零值。

存储边界中，读取不存在路径返回 `file not found: <path>`；删除不存在路径仍成功；写入覆盖既有值。`WalkDir` 的过滤是本地近似语义，既匹配根下前缀，也匹配中间 `/<prefix>/`，并不实现真实对象存储的全部 prefix 规则。传入回调的首个错误会中止遍历并向上传播。

加密边界中，空内容或 `None` cipher 直接透传，`PLAINTEXT` 同样不生成 IV，`UNKNOWN` 返回 invalid argument。AES 密钥长度只能是 16/24/32，IV 必须是 16 字节。算法枚举与密钥长度若互相矛盾，当前实现仍按密钥长度选择算法，这是相对 Go protobuf 枚举语义需要特别留意的差异。

重试边界中，即使 `max_attempts` 为 0 也执行一次；取消在每次调用闭包前检查；没有 sleep/backoff，也不聚合历次错误。`duration_ns::deserialize` 没有拒绝负纳秒，负数会强转为很大的 `u64`。failpoint 对未知名称返回错误，表达式参数被忽略。

## 并发与资源生命周期

所有跨线程抽象 traits 均要求 `Send + Sync`。`Context` clone 共享同一取消槽；`MemStorage` 用单个 mutex 串行化映射操作；`MockTimer` 只有不可变整数。`SUMMARY_START` 首次调用时在 mutex 保护下初始化，随后所有 checkpoint runner 共享同一计时基准，而不是每个 runner 单独计时。

`WalkDir` 有意在调用回调之前释放 `files` 锁，因此回调可以再次访问同一个 storage，不会因本文件的锁重入而死锁。代价是遍历的是快照：回调期间的并发写删不会改变当前轮条目列表，条目 size 也是快照时的值。

failpoint 使用 `SeqCst` 原子顺序，状态跨线程、跨测试共享；测试必须成对 Disable 或使用显式复位，避免污染后续用例。`Session::Close` 的真实资源释放由实现者负责，本 trait 没有 RAII 保证；`manager.rs` 和独立测试负责在生命周期末显式关闭 session/manager。

## 与 Go 版本的对应关系

这里不存在一一对应的 Go `stubs.go`；Rust 文件把多个 Go 包的依赖集中到了一个兼容层。`GlobalTimer` 对应 `br/pkg/checkpoint/checkpoint.go` 中的同名接口，`ComposeTS` 对应 TiDB TSO 编码规则；`File`/`CipherInfo` 等替代 Go 使用的 kvproto/rtree 类型，`ClusterConfig` 对应 `br/pkg/pdutil` 的调度配置概念。

`Encrypt` 的主分支对齐 `br/pkg/metautil/metafile.go::Encrypt`，`Decrypt` 对齐 `br/pkg/utils/encryption.go::Decrypt`：nil/空内容透传，PLAINTEXT 不变换，AES-CTR 使用 16 字节随机 IV，未知算法报参数错误。Rust 版本的错误类型、随机源错误暴露方式和“按密钥长度选择 AES 变体”仍是简化实现。

`NowDureTime` 对应 `br/pkg/summary/summary.go::NowDureTime`，但 Go 由 summary collector 管理起点并支持调整；Rust 只有不可调整的进程级 `Instant`。`WithRetry` 对应 `br/pkg/utils/retry.go` 的重试意图，但没有 `BackoffStrategy`、等待或 multierr，因此只能说明当前 checkpoint 调用点所需的最小行为。

`Glue`/`Session` 对应 `br/pkg/glue/glue.go` 中更大的接口子集；`Domain`、`InfoSchema`、`RestrictedSQLExecutor` 同样只保留 checkpoint 表后端实际使用的方法。`Storage` 对应 Go `storeapi.Storage` 的 checkpoint 所需子集，`MemStorage` 则是 Rust 本地测试实现，不是 Go 生产存储的移植。

## 扩展指南

新增 checkpoint 数据字段时，先判断它属于真正的 checkpoint 模型还是外部依赖适配。只有被多个 checkpoint 模块共享、且用于隔离外部 crate 的小型边界才应加入本文件；业务算法应留在 `checkpoint.rs`、`manager.rs` 或具体场景模块。序列化字段必须同步核对 Go JSON 名称、零值省略和历史数据兼容性。

扩展 `Storage`、`Session`、`Domain` 等 trait 会要求所有实现同步更新，包括 `MemStorage`、`storage.rs::MemSession`、`checkpoint_test.rs`、`storage_test.rs` 及其他自定义测试 double。若要接入真实生产客户端，应优先新增独立适配实现，而不是把网络、磁盘或 TiDB 细节塞进 `MemStorage`。

修改加密逻辑时应保持算法枚举、密钥长度、IV 长度和密文/IV 配对不变量，并在独立测试中覆盖空内容、PLAINTEXT、三种 AES 密钥长度、错误 IV、错误密钥长度和 UNKNOWN。若要求认证加密或严格按枚举选算法，属于协议变更，必须同时评估 Go 互操作和旧 checkpoint 可读性。

修改共享状态或并发策略时，应保留 `WalkDir` 回调不持锁、failpoint 测试复位和 Session 显式关闭。测试逻辑应继续放在独立文件：主要同步 `br/pkg/checkpoint/checkpoint_test.rs`、`external_storage_test.rs`、`storage_test.rs`、`parity_test.rs`，不要把测试内嵌进 `stubs.rs`。

## 验证依据

- RustCodeGraph：索引状态为 7,032 个 Rust 文件；读取了 `br/pkg/checkpoint/stubs.rs` 全部 613 行及 `lib.rs`，并核对了 `Encrypt`、`Decrypt`、`NowDureTime`、`WithRetry`、`failed_after_checkpoint_flushes` 的直接调用边。
- Rust 源码：`br/pkg/checkpoint/checkpoint.rs`、`external_storage.rs`、`storage.rs`、`manager.rs`、`backup.rs`、`log_restore.rs` 的导入与调用点证明本文件处于 checkpoint 核心、外部存储锁和表后端之间。
- crate 配置：`br/pkg/checkpoint/Cargo.toml` 证明库入口、Go 包映射和 AES/CTR/rand/serde 依赖边界。
- Go 对照：`br/pkg/checkpoint/checkpoint.go`、`br/pkg/metautil/metafile.go`、`br/pkg/utils/encryption.go`、`br/pkg/utils/retry.go`、`br/pkg/summary/summary.go`、`br/pkg/glue/glue.go`。
- 独立 Rust 测试：`br/pkg/checkpoint/parity_test.rs` 覆盖 AES-256 CTR 往返；`external_storage_test.rs` 覆盖 timer 重试；`checkpoint_test.rs` 覆盖 MemStorage、MockTimer、Session/Glue/Domain、加密和刷盘 failpoint；`storage_test.rs` 覆盖 SQL 参数与 InfoSchema 边界。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的结构命令确认文件存在且恰有 11 个固定二级章节，并人工复核没有把本地桩描述为完整生产能力。
