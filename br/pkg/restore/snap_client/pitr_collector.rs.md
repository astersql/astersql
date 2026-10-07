# `br/pkg/restore/snap_client/pitr_collector.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-restore-snap-client`（见同目录 `Cargo.toml`），由 `lib.rs` 以 `pub mod pitr_collector` 装配并整体重导出。它位于快照恢复的 ingest 前后：`client.rs::SnapClient::InstallPiTRSupport` 创建收集器，在 importer 上注册 `pitrCollector::onBatchOwned` 作为 ingest 前回调，并注册 `pitrCollector::close` 作为关闭回调。其目的不是执行 SST ingest，而是把即将 ingest 的 SST 复制到日志备份存储，并生成以后 PiTR 可以识别的额外备份元数据。

当前 Rust crate 的 `Cargo.toml` 明确说明使用本地 traits/stubs；本文件是 Go `pitr_collector.go` 的可运行语义移植，但尚不是 Go 生产接线的完整替代：`newPiTRColl` 直接转调测试友好的构造器，没有查询 etcd 日志备份任务，也没有创建真实对象存储；`prepareMig` 只把元数据路径写入进程内 `migration_paths`；元数据通过 `serde_json` 而非 Go 的 protobuf `Marshal` 写出。阅读和扩展时必须保留这条能力边界。

## 核心职责

- `pitrCollector` 持有恢复源存储、日志备份目标存储、恢复 UUID、TSO 和成功判定器，并在启用时协调 SST 复制、表 ID 改写收集和元数据提交。
- `verifyCompatibilityFor` 在产生复制副作用前拒绝 PiTR 当前无法表达的 keyspace 改写、时间戳改写及时间范围裁剪。
- `onBatchOwned` 为 `client.rs` 的实际回调入口：立即为 SST 和表 ID 映射启动线程，返回的闭包负责等待全部工作、传播首个错误并持久化批次元数据。
- `CopyConcurrency` 用 permit/RAII 将并发复制限制为配置值；`persistLock` 串行化快照生成与写入，避免旧快照覆盖新快照。
- `prepareMigIfNeeded` 保证每个 collector 只准备一次 migration；`commit` 和 `close` 决定最终元数据是完成态还是半完成态。

## 主要符号

- `DefaultMaxConcurrentCopy: i32 = 1024`：未显式配置时的复制并发上限，与 Go 常量一致。
- `CopyConcurrency` / `CopyPermit`：内部信号量。状态元组分别保存活跃数和上限；`acquire` 在满载时每 10ms 检查一次 `Context`，permit 的 `Drop` 自动减计数并唤醒等待者。
- `ingestedSSTsMeta`：受互斥锁保护的提交快照，包含 `backuppb::IngestedSSTs` 和 `old_table_id -> new_table_id` 映射。`toProtoMessage` 克隆消息并把映射展开为 `RewrittenTableID`。
- `pitrCollector`：核心状态机。公开方法包括批次入口、兼容性检查、路径构造、复制/改写收集、持久化、migration 准备、提交和测试观察接口；字段本身保持私有。
- `PiTRCollDep`：构造依赖容器。`LoadMaxCopyConcurrency` 通过 `PdClient::GetAllStores` 按“每 TiKV 并发数 × store 数”计算总上限。
- `newPiTRCollForTest`：当前真正完成字段接线的构造器；禁用分支不要求存储，启用分支采用传入的 stub trait 对象，并为 UUID、TSO、成功判定和名称提供默认值。
- `newPiTRColl`：保留 Go 命名的公开入口，但当前只忽略 `ctx` 并委托 `newPiTRCollForTest`，不具备 Go 版本的日志任务发现逻辑。
- `onBatch`：借用 `&self` 的顺序执行兼容/测试入口；它在返回闭包前只做 migration 准备与兼容性验证，真正复制在闭包执行时发生。生产接线使用的是 `onBatchOwned`。

## 执行流程

1. `SnapClient::InstallPiTRSupport` 先调用 `PiTRCollDep::LoadMaxCopyConcurrency`，再以 `newPiTRColl` 构造并用 `Arc` 托管 collector。若未启用则不注册回调；若是增量恢复则先关闭 collector，再返回“不安全”错误。
2. 首个 ingest 批次进入 `onBatchOwned`。函数先调用 `prepareMigIfNeeded`；原子 `compare_exchange` 的获胜者执行 `prepareMig`，登记 `metaPath()`、重置提交消息并先写一份 `Finished=false` 的空元数据。
3. 每个 `BackupFileSet` 先经过 `verifyCompatibilityFor`。验证通过后，每个 SST 和每条 `TableIDRemapHint` 各启动一个线程；函数随后返回等待闭包。
4. SST 线程在 `putSST` 中取得并发 permit，检查两端存储，调用 `Copier::CopyFrom` 从原文件名复制到 `v1/ext_backups/<name>/sst_files/<name>`，成功后才把改写过路径的文件克隆加入共享消息。改写线程在 `putRewriteRule` 中插入映射，同一旧 ID 指向不同新 ID 时失败。
5. importer 调用等待闭包后，闭包 join 所有线程；panic 转成 `PiTR upload worker panicked`，普通错误保留第一个。仅当全部成功时才调用 `persistExtraBackupMeta` 写批次快照。
6. importer 关闭时调用 `close`。恢复未成功则持久化当前半完成元数据；恢复成功则 `commit` 先标记 `Finished=true`，取得 TSO 写入 `AsOfTs`，再持久化最终快照。
7. 供测试使用的 `onBatch` 保留相同验证、复制、改写和持久化步骤，但复制在线程外、等待闭包内顺序进行，不能用它推断生产回调的并发启动时序。

## 数据与状态

路径不变量由 `outputPath`、`metaPath`、`sstPath` 集中定义：根目录固定为 `v1/ext_backups/<collector-name>`，元数据文件为 `extbackupmeta`，SST 位于 `sst_files/`。`resetCommitting` 同时清空文件和改写映射，设置 `FilesPrefixHint`、`Finished=false` 及 `RestoreUuid`；它在初始化和 migration 准备时调用。

`ingestedSSTMeta` 的文件列表只记录复制成功的 SST；失败复制不会提前污染元数据。表 ID 映射允许重复写入相同值，但拒绝同一源 ID 的冲突值。`toProtoMessage` 每次生成独立克隆，因此序列化不直接借用可变共享状态；`HashMap` 的迭代顺序不稳定，消费者不应依赖 `RewrittenTables` 的顺序。

`putMigOnce` 表示是否已经尝试过首次准备。当前实现使用 `AtomicBool::compare_exchange`：一旦线程赢得交换，即使随后 `prepareMig` 失败也不会重试，这与 Go `sync.Once` 的“调用一次”语义一致。`migration_paths` 只是当前 Rust 移植用的可观察记录，并非外部存储中的真实 migration 清单。

## 依赖与调用关系

上游主链由 RustCodeGraph/源码定位为 `client.rs::SnapClient::InstallPiTRSupport -> newPiTRColl -> pitrCollector::onBatchOwned`，随后 importer 的关闭回调调用 `pitrCollector::close`。`lib.rs` 将本模块公开符号重导出给 crate 使用。独立测试和 `parity_test.rs` 还直接调用 `onBatch`、`putRewriteRule`、`close` 等接口。

下游依赖主要来自 `crate::stubs`：`Context` 提供取消状态，`PdClient` 提供 store 列表，`ExternalStorage`/`Copier` 提供读写与跨存储复制，`backuppb` 提供文件和额外备份消息，`berrors` 提供错误类别，`summary::Succeed` 提供默认成功判定。标准库线程、`Mutex`、`Condvar`、`AtomicBool` 和 `Arc` 实现并发与共享所有权；`serde_json` 负责当前 Rust 消息编码。

Cargo 直接依赖还包括 `astersql-br-pkg-utils`、`astersql-errors`、`astersql-br-pkg-errors` 与 `astersql-br-pkg-restore`，但本文件的直接类型经过 `stubs.rs` 聚合。修改接口时应同时检查 `client.rs` 的回调签名、`stubs.rs` 的 trait/消息定义和 `lib.rs` 的重导出边界。

## 错误处理与边界

禁用 collector 时 `onBatchOwned`/`onBatch` 返回 `Ok(None)`，`close`、复制、改写和持久化为无副作用成功。启用但缺少目标或源存储时，`putSST` 分别返回 `task storage missing` 或 `restore storage missing`；缺少 PD client 时并发度加载失败。

兼容性错误使用 `ErrUnsupportedOperation` 包装，并带规则序号；改写冲突使用 `ErrInvalidArgument`。复制错误补充源/目标路径提示，批次等待阶段保留第一个 worker 错误，线程 panic 被显式转换为错误。只有所有 worker 成功才写批次元数据。

`commit` 在取 TSO 之前已把内存中的 `Finished` 置为 true；若 TSO 或最终写入失败，内存态不会回滚。`close` 使用 `Context::Background()`，不像 Go 版使用 30 秒超时，所以当前 Rust 关闭可能被存储实现长期阻塞。所有标准库锁均直接 `unwrap`：持锁线程 panic 会毒化锁，后续访问也会 panic，这是当前实现边界。

另一个重要差异是当前持久化使用 JSON 编码，而 Go 使用 protobuf；在真实跨语言 PiTR 消费链验证前，不能宣称两者的落盘格式兼容。

## 并发与资源生命周期

`onBatchOwned` 要求 `Arc<Self>`，每个工作线程持有 collector、上下文和输入对象的拥有副本；返回闭包拥有全部 `JoinHandle`，调用者必须执行该闭包才能回收线程、观察错误并完成批次元数据持久化。测试 `test_on_batch_owned_starts_uploads_before_wait_callback` 明确验证复制在返回等待闭包前已经启动。

`CopyConcurrency::acquire` 在复制期间持有 `CopyPermit`，成功、错误或 panic 展开时都会由 `Drop` 释放容量。`test_concurrency` 把上限设为 2 并验证十个并发批次的活跃复制峰值恰为 2。等待容量时会轮询上下文取消；一旦进入底层 `CopyFrom`，是否及时响应取消取决于存储实现。

`ingestedSSTMeta` 锁保护文件与映射，`persistLock` 覆盖“取得元数据锁、克隆/序列化、写文件”的整个持久化过程，确保较旧快照不能在较新快照之后落盘。两把锁的固定获取顺序是先 `persistLock` 后元数据锁；普通更新只取元数据锁，没有反向嵌套。

Rust 版没有 Go 的后台 persister channel，也没有显式线程关闭句柄；持久化由调用线程串行完成。collector 的生命周期由 `Arc` 和 importer 回调所有权决定，最终 `close` 不会阻止之后再次调用，测试 `test_reopen` 实际是构造新 collector 并复用存储/UUID，而不是重新打开同一个实例。

## 与 Go 版本的对应关系

Rust 与 `pitr_collector.go` 保留的核心语义包括：默认并发 1024、路径布局、复制成功后登记文件、冲突改写拒绝、受限改写规则的前置校验、首次 migration 准备、失败恢复写半完成元数据、成功恢复写 `Finished` 和 TSO，以及按 TiKV store 数计算总并发度。Rust 测试对应 Go 的 `TestCollAFile`、`TestCollManyFileAndRewriteRules`、`TestReopen`、`TestConflict` 和 `TestConcurrency`。

不能忽略的实现差异如下：

- Go `newPiTRColl` 通过 etcd 查询日志备份任务，拒绝多任务，零任务时禁用，并创建真实 task/restore storage；Rust 入口仅使用调用者注入的本地 trait 对象。
- Go `prepareMig` 通过 `stream.MigrationExtension(...).AppendMigration` 写真实 migration，并携带 operation context；Rust 只追加 `migration_paths`。
- Go 使用单一后台 persister goroutine 合并排队请求；Rust 用 `persistLock` 同步串行写入，不合并请求。
- Go 对 `IngestedSSTs` 调用 protobuf `Marshal`；Rust 当前调用 `serde_json::to_vec`。
- Go `close` 设置 30 秒超时并关闭 persister handle；Rust 使用背景上下文且无对应后台资源。
- Go `onBatch` 本身并发调度复制；Rust 生产接线用 `onBatchOwned` 达成相同时序，另一个同名 `onBatch` 仅是顺序兼容入口。

因此该文件可用于当前 stub 环境和现有 Rust 调用链，但若目标是替换 Go BR 的真实 PiTR 收集器，上述差异都是必须补齐并做互操作验证的迁移缺口。

## 扩展指南

新增可落盘字段时，应修改 `ingestedSSTsMeta`/`resetCommitting`/`toProtoMessage`，并在独立的 `pitr_collector_test.rs` 增加序列化、重开和成功/失败关闭场景；不要把测试内嵌到生产文件。若字段进入跨语言格式，还必须同步核对 `pitr_collector.go` 和 kvproto 定义，优先替换当前 JSON 路径为协议兼容编码。

新增批次副作用时，应接入 `onBatchOwned` 的 worker 集合，使等待闭包能够 join、传播错误并确保失败时不持久化完成批次；同时判断顺序测试入口 `onBatch` 是否需要同等语义。所有新增复制类工作都应通过 `CopyConcurrency`，并增加取消、上限和 worker panic/失败测试。

扩展兼容规则应优先放在 `verifyCompatibilityFor`，保证 `prepareMig` 之后、SST 复制之前失败；若要求绝对零外部副作用，则还应重新评估当前“先准备 migration、后逐 fileset 验证”的顺序。改变目录名、UUID、`Finished` 或 `AsOfTs` 时属于持久格式兼容变更，必须检查 PiTR 消费端而不只是本 crate 测试。

若补齐生产能力，最可能修改 `newPiTRColl`、`prepareMig`、`persistExtraBackupMeta` 和 `close`，并同步 `PiTRCollDep`。应分别覆盖零/单/多日志任务、真实 migration 登记、protobuf 互操作、30 秒关闭边界、持久化请求合并以及失败后可观察状态；外部客户端依赖应按仓库规则在其独立上游仓库移植和打 tag，不得在本仓库建立本地覆盖。

## 验证依据

- 生产源码：`br/pkg/restore/snap_client/pitr_collector.rs`，核对了常量、内部并发类型、状态结构、全部 `pitrCollector`/`PiTRCollDep` 方法及两个构造器。
- crate 边界：`br/pkg/restore/snap_client/Cargo.toml` 与 `lib.rs`，确认包名、依赖、本地 stubs 说明、模块装配和公开重导出。
- Rust 上游：`br/pkg/restore/snap_client/client.rs::SnapClient::InstallPiTRSupport`，确认 `onBatchOwned` 与 `close` 是 importer 的实际回调。
- Rust 独立测试：`br/pkg/restore/snap_client/pitr_collector_test.rs`，确认单/多文件、改写冲突、重开、并发上限、提前启动 worker 和不支持时间戳改写等边界；`parity_test.rs` 还覆盖禁用路径及基础契约。
- Go 对照：`br/pkg/restore/snap_client/pitr_collector.go`、`client.go::SnapClient.InstallPiTRSupport` 与 `pitr_collector_test.go`，用于核对真实日志任务发现、migration、后台 persister、关闭超时和 protobuf 语义。
- RustCodeGraph：`status` 显示索引包含目标目录；`files --filter br/pkg/restore/snap_client` 确认源、Go 对照和测试均被索引；符号查询定位 `pitrCollector`、`newPiTRColl`，源码/调用搜索确认 `client.rs` 的 `onBatchOwned` 和关闭回调边。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前仅执行固定十一章节结构检查和文档差异自审。
