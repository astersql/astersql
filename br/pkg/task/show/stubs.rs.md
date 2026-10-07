# `br/pkg/task/show/stubs.rs`

## 文件定位

`stubs.rs` 是 workspace 成员 crate `astersql-br-pkg-task-show` 的本地适配层。`br/pkg/task/show/Cargo.toml` 把 `lib.rs` 定义为 library 入口且没有声明外部依赖；`lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 装入本文件，再导出 `Context`、`MetaReader`、`MemStorage`、`NewMetaReader`、`set_read_backup_meta_hook` 等符号。直接业务消费者是同 crate 的 `cmd.rs`：`CreateExec` 调用 `ReadBackupMeta`/`NewMetaReader`，`CmdExecutor::Read` 调用 `MetaReader::{GetBasic, ReadSchemasFiles}` 并使用 `Context`、`berrors`、`oracle` 等辅助能力。

这不是 Go BR 真实依赖的 Rust 实现，而是为 darwin arm64 等轻量构建与独立测试提供的桩层。文件顶部明确排除真实 kvproto 解码、对象存储 IO、metautil V2 schema 遍历与 PD/oracle 时钟源。因此它当前只保证 show 包的类型形状、控制流和测试注入契约，不能作为已接通生产 BR 后端的证据。

## 核心职责

本文件承担五类桩适配职责：

1. 以 `Error`/`Result` 和 `Context`/`CancelFunc` 模拟 Go errors 与 `context.Context` 的最小错误包装、取消传播语义。
2. 在 `backuppb`、`encryptionpb`、`objstore`、`task`、`berrors` 子模块中定义 show 主流所需的最小类型和错误工厂，保留 Go 字段/符号命名以便 parity 对照。
3. 以 `CIStr`、`DBInfo`、`TableInfo`、`MetaTable`、`ReadSchemaConfig` 表达 metautil schema 遍历所需的数据投影和选项。
4. 以 `Storage` trait、`MemStorage` 和 `MetaReader` 构成纯内存的 backup meta/schema 读取边界，支持测试预置表或读取错误。
5. 以 thread-local `READ_BACKUP_META_HOOK` 替代真实 storage 打开和 protobuf 解码；无 hook 时显式失败，防止将桩默认行为误认为生产能力。

## 主要符号

- `pub type Result<T> = std::result::Result<T, Error>` 与 `Error { msg }`：本地错误栈。`Error::new` 构造消息，`Error::Annotate` 前缀上下文，`Error::Trace` 是不采集栈的恒等操作；实现 `Display` 和 `std::error::Error`。
- `Context { cancelled, parent }` 与 `CancelFunc`：`Background` 创建根上下文，`WithCancel` 返回动态观察父级的子上下文和取消句柄，`Err`/`Done` 查询本地或父级取消，`CancelFunc::cancel` 写入固定错误 `context canceled`。`Context::cancel` 也可写入自定义错误，当前 show 主流未调用它。
- `berrors::ErrInvalidMetaFile`：产生 `invalid metafile: ...` 消息，对应 Go `ErrInvalidMetaFile.GenWithStackByArgs`，但不携带 RFC code 或 stack。
- `encryptionpb::EncryptionMethod`、`backuppb::{CipherInfo, RawRange, BackupMeta, StorageBackend}`：仅保留 show 需要的 protobuf 字段。`RawRangeIndex: Option<Vec<u8>>` 只用于识别并拒绝 Raw V2；没有 protobuf codec 或加解密实现。
- `HexBytes(Vec<u8>)`：`from_slice` 复制字节，`Display` 生成小写、无分隔符的两位十六进制序列，对应 Go `logutil.HexBytes`。
- `oracle::{GetTimeFromTS, format_show_ts}`：用 `ts >> 18` 取物理毫秒。Unix 上通过 `localtime_r`/`strftime` 生成 Go show 布局；其他平台以 `civil_from_days` 按 UTC 回退格式化。
- `objstore::BackendOptions` 和 `task::Config { Storage, BackendOptions, CipherInfo }`：为 `cmd::Config::lameTaskConfig` 与 `ReadBackupMeta` 签名提供最小配置形状。
- `CIStr`、`DBInfo`、`TableInfo`、`MetaTable`：保留库/表名、KV 数量/字节数和 TiFlash 副本数。`CIStr::String` 返回原始名 `O` 的副本。
- `ReadSchemaOption`、`ReadSchemaConfig`、`SkipFiles`、`SkipStats`：模拟 Go metautil 函数选项。标志会被设置，但桩不根据它们过滤数据。
- `MetaFile`：值为 `"backupmeta"`，与 Go `metautil.MetaFile` 一致。
- `Storage: Send + Sync`：要求 `name` 并可选返回 `schema_tables`，默认为空。`MemStorage::{new, with_tables}` 返回 `Arc<Self>`，后者预装 schema 表。
- `MetaReader`：拥有 `BackupMeta`、预装表、可选读错误、storage 名和 cipher。`with_tables`/`with_read_err` 是测试构造器，`GetBasic` 返回 meta 副本，`ReadSchemasFiles` 向同步通道重放表。
- `NewMetaReader`：从 meta、`Arc<dyn Storage>` 和 cipher 建立 reader，初始表取自 `storage.schema_tables()`。
- `ReadBackupMetaHook`、`set_read_backup_meta_hook`、`ReadBackupMeta`：hook 接收 context、文件名和 task config，返回 backend、storage trait object 和 meta；公开 setter 在当前线程安装/清除 hook。

## 执行流程

`cmd.rs` 的配置驱动路径如下：

1. `CreateExec` 把 show `Config` 裁剪为 `task::Config`，以 `MetaFile` 作为文件名调用 `ReadBackupMeta`。
2. `ReadBackupMeta` 在当前线程查找 `READ_BACKUP_META_HOOK`。已安装时，闭包完全接管 URI/文件校验、存储句柄与已解码 meta 的产生；未安装时返回包含 storage 和 file 的 `not configured` 错误。
3. `CreateExec` 把返回的 meta/storage 交给 `NewMetaReader`。后者立即调用 `storage.schema_tables()` 克隆预装表，并保存 storage 名和 cipher 副本。
4. `CmdExecutor::Read` 通过 `GetBasic` 获得 meta 副本。在非 RawKV 分支，它启动生产者线程调用 `ReadSchemasFiles(ctx, out, &[SkipFiles, SkipStats])`。
5. `ReadSchemasFiles` 先顺序应用选项，再检查预置 `read_err`；若无错误，则按 `tables` 顺序遍历，每次发送前检查 context 取消，并把 receiver 已关闭映射为 `schema output channel closed`。
6. `cmd.rs::collectResult` 消费表并转成展示结果。RawKV 分支不使用 `MetaReader::ReadSchemasFiles`，而是直接读 `BackupMeta.RawRanges`。

测试路径可以跳过配置读取：用 `NewMetaReader(...).with_tables(...)` 或 `.with_read_err(...)` 构造 reader，再经 `CmdExecutor::from_reader` 执行。`cmd_test.rs::install_local_hook` 则使用完整路径，在 hook 内核对真实落盘 fixture 字节和 cipher，但 meta 解码/schema 遍历仍由测试替身完成。

## 数据与状态

`Error`、protobuf 投影、schema 投影和 `HexBytes` 都是拥有型普通数据。`BackupMeta` 仅建模 show 读取的字段；未出现的真实 protobuf 字段不会被保留。`MetaReader::GetBasic` 和 `Storage::schema_tables` 都返回克隆，故 reader 读取不暴露内部可变引用，代价是 meta 和全部表的复制。

`Context.cancelled` 是 `Arc<Mutex<Option<Error>>>`，克隆的同一 context 共享取消单元。子 context 把父 context 的克隆放入 `Arc<Context>`；`Err` 每次先克隆本地错误，再递归查询父级，所以子级在创建后仍能看到父级的晚取消。它没有 deadline、value 或可等待的 Done channel。

`READ_BACKUP_META_HOOK` 是 `thread_local RefCell<Option<Box<dyn Fn + Send + Sync>>>`。其生命周期与设置它的线程绑定，并非进程全局注册表；同线程后续用例会继承未清理 hook，其他线程则看不到。`ReadSchemaConfig.skipFiles/skipStats` 是私有状态，当前只记录选项已应用，不改变重放的表。

## 依赖与调用关系

上游与模块装配：

- `br/pkg/task/show/lib.rs` 是直接模块入口，公开再导出本文件的 show 所需符号。
- `br/pkg/task/show/cmd.rs` 是唯一直接业务消费者：`CreateExec -> ReadBackupMeta -> NewMetaReader`，`CmdExecutor::Read -> MetaReader::GetBasic/ReadSchemasFiles`，`TimeStamp::fmt -> oracle::format_show_ts`，版本/Raw V2 错误经 `berrors::ErrInvalidMetaFile` 构造。
- `br/pkg/task/show/cmd_test.rs` 使用 `set_read_backup_meta_hook`、`MemStorage::with_tables`、`Context::Background` 跑全量、V2 多表和加密 fixture 意图。
- `br/pkg/task/show/parity_test.rs` 直接使用 `NewMetaReader`、`with_tables`、`with_read_err`、`WithCancel` 和 hook，覆盖转换、错误、取消与资源释放契约。

下游只使用 Rust 标准库：`Arc`/`Mutex`、`RefCell`、`mpsc::SyncSender`、时间类型、格式化，以及 Unix 上的 libc ABI 函数。`Cargo.toml` 的 `[dependencies]` 为空，与文件中的“local stand-ins”定位一致。RustCodeGraph 对 `ReadBackupMeta` 的全库同名查询还返回 `br/pkg/task/common.rs` 及 Go 定义；那些是独立实现，不是本 thread-local hook 函数的静态调用边。

## 错误处理与边界

- `Error::Annotate` 仅拼接 `<context>: <original>`；`Trace` 不采集调用栈，`ErrInvalidMetaFile` 也没有 Go berrors 的错误码、stack 或 metrics 标签。依赖类型化错误分支的代码不能直接使用此桩替代。
- `Context` 的 mutex 通过 `unwrap()` 取锁；若共享 mutex 因 panic 中毒，后续查询/取消会 panic，而不是返回 `Result`。`MetaReader` 本身不含锁，读取时通过克隆数据避免共享可变状态。
- `ReadSchemasFiles` 在发送每张表之前检查取消；取消与发送之间仍有竞态窗口，且容量已满时 `SyncSender::send` 可阻塞到消费者读取或关闭。无独立取消选择分支。
- 预置 `read_err` 会在发送任何表前立即返回；receiver 关闭则统一映射为 `schema output channel closed`，不保留标准库 `SendError` 的数据。
- `ReadBackupMeta` 未安装 hook 的错误只表示“测试/集成边界未配置”，不是 local/S3/GCS 读取失败。hook 的 panic 也不会被捕获或转换为 `Error`。
- `oracle::format_show_ts` 在 Unix 上使用进程本地时区；`localtime_r`/`strftime` 失败时退回 UTC 算法，因而极端平台失败时的时区语义可能与 Go `time.Local` 不同。`GetTimeFromTS` 只使用物理部分，不验证 TSO 来源。
- `SkipFiles`/`SkipStats`、`CipherInfo`、`BackendOptions`、`StorageBackend` 在桩层中都只传递或记录，不产生真实过滤、解密、URI 解析或 IO 行为。

## 并发与资源生命周期

`Context` 可跨线程克隆：取消单元使用 `Arc<Mutex<_>>`，父链也由 `Arc` 保持存活。`Storage` 被约束为 `Send + Sync`，`ReadBackupMetaHook` 闭包也要求 `Send + Sync`，因此 hook 可返回能安全共享给 `cmd.rs` 生产者线程的 `Arc<dyn Storage>`。`MemStorage` 本身不含内部可变状态；`schema_tables` 通过克隆完整 `Vec<MetaTable>` 将数据转移到 reader。

hook 槽使用 `thread_local!` 而非 `Mutex` 全局变量，因此不同测试线程可以安装各自 hook，但 hook 不会自动跨线程传播。`set_read_backup_meta_hook(None)` 是显式清理契约；`cmd_test.rs::clear_hook` 与 `parity_test.rs` 的正常/错误路径都体现了该要求。若同线程用例在清理前 panic，hook 会留到该线程后续执行；当前没有 RAII guard 自动恢复旧值。

`MetaReader::ReadSchemasFiles` 不创建线程，由 `CmdExecutor::Read` 将克隆的 reader/context 移入背景线程。它借用 `SyncSender`，不拥有 receiver；方法返回后 sender 由调用方 drop 以关闭数据通道。表、meta、cipher、storage 名和错误都依靠 Rust RAII 释放，无文件句柄、网络连接、runtime task 或事务需要在本文件中显式关闭。

## 与 Go 版本的对应关系

`stubs.rs` 没有单一的 Go 同路径文件；它把 `br/pkg/task/show/cmd.go` 依赖的多个 Go 包压缩到一个本地 Rust 文件中。对应关系为：Go `context` → `Context`/`CancelFunc`，`pingcap/errors` 与 `br/pkg/errors` → `Error`/`berrors`，kvproto `brpb`/`encryptionpb` → `backuppb`/`encryptionpb`，`br/pkg/logutil.HexBytes` → `HexBytes`，`br/pkg/metautil.MetaReader`/`Table`/options → `MetaReader`/`MetaTable`/`SkipFiles`/`SkipStats`，`br/pkg/task.Config`/`ReadBackupMeta` → `task::Config`/hook 函数，`pkg/objstore` → `BackendOptions`/`Storage`，client-go `oracle.GetTimeFromTS` → `oracle` 子模块。

语义上保留的部分包括：`MetaFile == "backupmeta"`，TSO 物理毫秒为 `ts >> 18`，HexBytes 的小写十六进制展示，`ReadSchemaOption` 函数选项形状，meta/schema 的主要字段，以及取消、schema 顺序发送与错误传播意图。

语义上未实现的部分包括：Go context deadline/value/Done channel，errors stack/code，protobuf 编解码，cipher/KMS，objstore backend 解析与 IO，metautil V1/V2 schema 遍历和 options 过滤，以及真实 PD 时钟源。`cmd_test.go` 在 Go 中通过真实 `show.CreateExec` 读取 full/V2/encrypted fixture；Rust `cmd_test.rs` 保留相同主要断言，但通过 `install_local_hook` 校验落盘字节后注入已解码结果，所以不能等价证明 Go 依赖已在 Rust 中移植。

## 扩展指南

- 接入真实 BR 生产能力时，应在对应 canonical crate 移植 storage/protobuf/metautil/context/error 能力，再把 show crate 依赖切换到真实类型；不应继续扩充单个 `stubs.rs` 成为另一套完整子系统。外部 Rust 依赖须按仓库规则在上游仓库移植并以 tag 引用，不可用本地 patch/vendor 绕过。
- 若新增 show 所需 meta/schema 字段，同步修改 `backuppb::BackupMeta` 或 `MetaTable`、`cmd.rs` 转换层和独立 `cmd_test.rs`/`parity_test.rs`，并核对 Go 类型的 nil/默认值、数值宽度和序列化意义。Rust 单元测试不应嵌入本生产文件。
- 若扩展 `ReadSchemasFiles`，需保持输入顺序、取消可见性和 receiver 关闭错误，并为 `SkipFiles`/`SkipStats`、中途取消、背压与发送失败增加独立测试。引入真实 IO 后还需明确 reader/stream 关闭与 async runtime 生命周期。
- 若保留 hook 测试边界，建议封装 RAII guard 以在 panic/unwind 时恢复旧 hook，并补充同线程串行污染与多线程隔离测试。不应把 thread-local hook 暴露为生产依赖注入机制。
- 修改 `oracle::format_show_ts` 时，同步核对 `cmd_test.rs::timestamp_display_uses_local_timezone_like_go`、`test_show_via_sql` 和 Go `TimeStamp.String`；区分进程 local timezone 诊断字符串与 SQL session timezone 展示。
- 更换 `Error` 或 `Context` 时，先检查 `cmd.rs::collectResult` 的取消/错误优先级，并保留 `CreateExec` 的 `failed to create execution` 上下文以及子 context 观察晚父取消的回归契约。

## 验证依据

- Rust 目标源：`br/pkg/task/show/stubs.rs` 全部 574 行，核对了模块、类型别名、常量、struct、enum、trait、impl、函数、`thread_local!` 与 `#[cfg(unix)]` 分支。
- crate 边界：`br/pkg/task/show/Cargo.toml` 确认 library 名、`lib.rs` 入口、Go 包映射、lane 2 与空依赖；根 `Cargo.toml` 将该路径列为 workspace 成员。`br/pkg/task/show/lib.rs` 确认模块装配和公开再导出。
- 直接 Rust 调用者：`br/pkg/task/show/cmd.rs` 中 `CreateExec`、`CmdExecutor::Read`、`TimeStamp::fmt` 及对应 import；由此确认 `ReadBackupMeta`/`NewMetaReader`/`ReadSchemasFiles`/context/error/oracle 的实际数据流。
- Rust 独立测试：`br/pkg/task/show/cmd_test.rs` 覆盖 full-schema、500 张 V2 小表、AES256-CTR fixture 形状、hook 清理和时区展示；`br/pkg/task/show/parity_test.rs` 覆盖预置 schema、读取错误、无 hook 错误、context 取消、子 context 晚父取消与资源 drop。
- Go 对照：`br/pkg/task/show/cmd.go` 用于核对实际依赖、`Config`/`CreateExec`/`Read`/转换函数契约；`br/pkg/task/show/cmd_test.go` 用于核对 full/V2/encrypted/SQL timezone 的原始测试意图。同目录无 `doc.go`。
- RustCodeGraph：`status` 报告 7032 个 Rust 文件；`files --filter br/pkg/task/show` 列出 `stubs.rs`、`cmd.rs`、`lib.rs` 与两个独立 Rust 测试；`node --file br/pkg/task/show/stubs.rs --offset 1 --limit 400` 及 `--offset 400 --limit 220` 覆盖完整 574 行；`explore "br/pkg/task/show/stubs.rs ReadBackupMeta MetaReader MemStorage SetReadBackupMetaHook"` 交叉核对了同名符号及广度影响。因同名 `ReadBackupMeta` 跨 Go/Rust 多实现存在，精确调用关系另以 `cmd.rs` 源码与定向仓库搜索复核，未将宽泛 blast-radius 条目当作本桩的直接调用者。
- 本任务只生成文档，按计划不运行 Cargo。交付结构检查使用任务指定的 `test -f` 与十一个固定二级标题计数命令。
