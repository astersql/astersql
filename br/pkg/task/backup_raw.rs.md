# `br/pkg/task/backup_raw.rs`

## 文件定位

`backup_raw.rs` 属于 Cargo crate `astersql-br-pkg-task`，由 [`br/pkg/task/lib.rs`](./lib.rs) 以 `pub mod backup_raw` 挂载并通配再导出。它是 `br backup raw` 的任务层实现：命令层 [`br/cmd/br/backup.rs`](../../cmd/br/backup.rs) 负责构造命令、解析公共运行环境和 tracing，随后由 `runBackupRawCommand` 调用本文件的 `RunBackupRawWithDefaults`。

本文件对照 [`br/pkg/task/backup_raw.go`](./backup_raw.go)，面向 RawKV（非事务 KV）备份，而不是 SQL 逻辑备份或 TxnKV 备份。其直接职责止于配置解析、单个键范围的备份编排和 RawKV 元数据落盘；连接管理、备份 RPC、元数据存储等能力通过 `common`、`backup` 与 `stubs` 中的抽象获得。

## 核心职责

1. 用 `DefineRawBackupFlags` 注册 RawKV 专用参数：键格式、起止键、TiKV 列族、keyspace、压缩算法及隐藏的调度器开关。
2. 用 `RawKvConfig::ParseFromFlags` 和 `ParseBackupConfigFromFlags` 将 flag 转换成强类型配置，并拒绝非空且 `start >= end` 的非法范围。
3. 用 `RunBackupRaw` 调整公共配置、校验目标存储未被占用、可选摘除 PD 调度器、构造 `BackupRequest`、执行范围备份并写入 `BackupMeta.RawRanges`。
4. 用 `SchedulerRestoreGuard` 保证摘除调度器后，即使版本查询、region 计数或备份阶段返回错误，也会在作用域退出时尝试恢复。
5. 用 `RunBackupRawWithDefaults` 为命令入口组装当前 Rust 移植层的默认 `Mgr` 和 `MemBackupClient`。

这里的关键协议是 RawKV 请求必须设置 `IsRawKv = true`、`StartVersion = EndVersion = 0` 并携带 `Cf`；恢复端依靠写入元数据的 `RawRanges` 和列族识别非事务数据范围。

## 主要符号

- `flagKeyFormat`、`flagTiKVColumnFamily`、`flagStartKey`、`flagEndKey`：四个公开 flag 名称常量。默认格式为 `hex`，默认列族为 `default`。
- `RawKvConfig`：聚合公共 `Config`、解码后的 `StartKey`/`EndKey`、列族 `CF`、`CompressionConfig` 和 `RemoveSchedulers`。空起止键分别表示无下界和无上界。
- `DefineRawBackupFlags(&mut FlagSet)`：注册专用参数并通过 `MarkHidden` 隐藏 `remove-schedulers`，因为该参数会改变在线集群的调度行为。
- `RawKvConfig::ParseFromFlags`：先用 `ParseKey` 按指定格式解码起止键，再做字节序范围校验，最后解析列族和公共 `Config`。
- `RawKvConfig::ParseBackupConfigFromFlags`：在基础解析之上读取 keyspace、压缩类型/级别和调度器开关。
- `RawKvConfig::Adjust`：委托 `Config::adjust` 填充公共默认值，例如 keepalive 和并发相关默认配置。
- `SchedulerRestoreGuard`：私有 RAII 守卫；`Drop` 时取出并调用一次 `RestoreSchedulers` 闭包，恢复失败按 Go 的 best-effort 策略被忽略，不覆盖主任务结果。
- `RunBackupRaw`：可注入 `Glue`、`Mgr` 和 `BackupClient` 的核心编排函数，也是生命周期测试的直接测试点。
- `RunBackupRawWithDefaults`：公开便利入口；调用 `NewMgr`，明确传入 `needDomain = false` 和 `NormalVersionChecker`，再以 `MemBackupClient` 调用核心函数。

## 执行流程

命令主链为 `newRawBackupCommand` → `runBackupRawCommand` → `RawKvConfig::ParseBackupConfigFromFlags` → `RunBackupRawWithDefaults` → `RunBackupRaw`。

`RunBackupRaw` 的顺序具有可观察语义：

1. `cfg.Adjust()` 补齐公共默认值，`Summary(cmdName)` 初始化/登记任务摘要。
2. 从配置构造 `StorageOptions`，其中强制 `CheckS3ObjectLockOptions = true`，随后调用 `BackupClient::SetStorageAndCheckNotInUse`；失败时尚未修改调度器。
3. 把配置中的起止键复制到唯一的 `KeyRange`。
4. 当 `RemoveSchedulers` 为真时调用 `Mgr::RemoveSchedulers`，并立即把恢复闭包装入 `SchedulerRestoreGuard`。后续任意 `?` 提前返回都会触发其 `Drop`。
5. 从 `Glue` 和 `Mgr` 获取 BR 版本、集群版本与范围内的近似 region 数；region 数写入摘要，并用于 `Glue::StartProgress` 的总量。
6. 构造 `BackupRequest`：使用客户端 cluster ID/存储 backend，复制范围、限速、并发、压缩和加密配置，固定两个版本为 0，并标记 RawKV 与列族。
7. `BackupClient::BackupRanges` 仅接收该单一范围；成功后关闭进度对象。`UnitRange` 在当前实现中只保留符号引用，未实现 Go 版逐响应的进度回调。
8. 创建 `MetaWriter`，启动异步元数据写入，把版本、RawKV 标记、`RawRange`、集群/BR/API 版本更新到备份元数据，再依次完成写入与刷新。
9. 将归档大小记录到 `BackupDataSize`，调用 `SetSuccessStatus(true)`，返回成功；函数离开时调度器守卫最后执行恢复。

## 数据与状态

`RawKvConfig` 是整个流程的可变输入。解析阶段会覆盖键范围、列族、keyspace、压缩设置和调度器开关；执行开始时 `Adjust` 还会修改其内嵌公共配置。`RunBackupRaw` 不修改起止键和列族，而是克隆它们形成请求和元数据，确保请求范围与恢复描述一致。

`BackupRequest` 是下游备份协议状态：`ClusterId` 与 backend 来自客户端，`RateLimit`、`Concurrency`、`CipherInfo` 来自公共配置，压缩字段来自 `CompressionConfig`。`BackupMeta` 保存请求的版本和 RawKV 标志，并额外保存 `RawRanges`、cluster/BR/API 版本；只有 `FinishWriteMetas`、`FlushBackupMeta` 都成功后，任务才设置成功状态。

进度和摘要是进程内副作用：region 近似数写入 `CollectInt`，`StartProgress` 的 `redirect_log` 参数是 `!LogProgress`，成功结束后关闭进度对象并记录归档大小。PD 调度器状态是外部集群副作用，由 `SchedulerRestoreGuard` 管理。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 显示为：

- [`br/cmd/br/backup.rs`](../../cmd/br/backup.rs) 的 `runBackupRawCommand` 调用 `RunBackupRawWithDefaults`；`newRawBackupCommand` 注册 `DefineRawBackupFlags` 并把子命令接入 `br backup`。
- [`br/pkg/task/backup_raw_test.rs`](./backup_raw_test.rs) 直接调用可注入版本 `RunBackupRaw`，验证失败路径的资源恢复。
- [`br/pkg/task/parity_test.rs`](./parity_test.rs) 引用 `RunBackupRawWithDefaults` 等公开契约；`lib.rs` 将本模块公开符号再导出给命令 crate。

主要下游依赖如下：

- `crate::common::{Config, GetKeepalive, NewMgr}`：公共配置调整和连接管理器构造。
- `crate::backup::{CompressionConfig, parseCompressionFlags, ...}`：共享压缩及 flag 定义。
- `Glue`：版本、进度和摘要数据记录的宿主接口。
- `Mgr`：集群版本、region 计数及 PD 调度器操作。
- `BackupClient`：存储占用检查、集群/API 信息和 `BackupRanges` 执行。
- `MetaWriter`：组装、完成并刷新备份元数据。
- `backuppb::{BackupRequest, RawRange}`：与备份/恢复协议对应的数据结构。

[`br/pkg/task/Cargo.toml`](./Cargo.toml) 声明该目录是 library crate，`go-package = "br/pkg/task"`；本文件使用的多数 BR 能力当前经 crate 内本地 traits/stubs 暴露，Cargo 注释也明确 arm64 Darwin 路径避免直接引入 kv/domain/kvproto/grpcio。

## 错误处理与边界

解析阶段所有 flag 读取和 `ParseKey` 错误均通过 `Result` 原样传播。只有起止键同时非空时才检查顺序，因此开放边界合法；两端相等或反向时返回带有 `berrors::ErrBackupInvalidRange` 的 `Error::Annotate("endKey must be greater than startKey")`。调用方必须先注册公共及 RawKV flags，否则缺失 flag 的读取错误会直接返回。

执行阶段使用 `?` 在存储检查、调度器摘除、集群信息查询、范围备份和元数据完成/刷新失败时立即终止。调度器恢复错误在 `Drop` 中被有意忽略，因此无法替换原任务错误，也不会让原本成功的备份转为失败；这与 Go 代码记录警告但保留主结果的意图相同，但 Rust 当前没有输出恢复失败警告，运维侧可观测性较弱。

当前实现还有几项必须明确的迁移边界：`GetStorageBackend().unwrap_or_default()` 会把 backend 缺失降为默认值，而 Go 版先用 `objstore.ParseBackend` 校验 URI；`RunBackupRawWithDefaults` 使用 `MemBackupClient`，不是 Go 版真实 `backup.NewBackupClient`；Rust 核心函数没有 Go 版的 context 取消和子 span；`Mgr` 由参数持有且未显式调用 `Close`。这些事实意味着默认入口保留了调用形状和核心数据契约，但不能据此宣称已完整接通真实集群 I/O。

## 并发与资源生命周期

本文件本身不创建线程或 Tokio 任务。`MetaWriter::StartWriteMetasAsync` 表达异步元数据阶段，但其具体并发和持久化行为由注入实现负责；本函数严格在 `BackupRanges` 成功后启动它，并顺序等待 `FinishWriteMetas`、`FlushBackupMeta`。

`SchedulerRestoreGuard` 是最重要的生命周期保证：守卫定义在版本查询和备份之前，按 Rust 栈展开规则在正常返回、`?` 提前返回和 panic unwind（未配置 abort 时）触发一次恢复。独立测试 `test_run_backup_raw_restores_schedulers_on_error` 让 `GetClusterVersion` 返回错误，并用 `AtomicBool` 证明恢复闭包已运行。

进度对象只在 `BackupRanges` 成功后显式 `Close`；若备份调用返回错误，本文件没有 RAII 进度守卫。管理器同样没有显式 `Close` 配对，实际释放依赖 `Arc` 下具体对象的析构；扩展到真实客户端时应先补足或验证这两个资源契约，不能仅依赖当前内存桩行为。

## 与 Go 版本的对应关系

Rust 的常量、`RawKvConfig` 字段、flag 默认值、字节序范围检查、请求的 RawKV/版本/CF/压缩/加密字段，以及写入 `RawRanges` 的元数据语义均直接对应 [`backup_raw.go`](./backup_raw.go)。`needDomain = false`、`NormalVersionChecker`、S3 Object Lock 检查和可选摘除调度器也保留了 Go 主流程决策。

Rust 用 RAII 的 `SchedulerRestoreGuard` 替代 Go `defer restore(ctx)`，并由测试覆盖错误路径。Go 在 context 已取消时改用 `context.Background()` 恢复，并会记录恢复失败警告；Rust 的无 context 闭包无法表达这一分支，且静默忽略恢复错误。

尚未等价的部分包括：Go 创建/取消 context 和 tracing span、解析真实存储 URI、构造并关闭真实 manager/client、通过 callback 按非 `UnitRange` 响应递增进度，以及使用真实外部存储的 `MetaWriter`。Rust 的 `UnitRange`、`GetKeepalive`/`NewMgr` 注释及内存实现体现了迁移中的接口占位，不能当作完整生产接线。

Go 文件在 `RemoveSchedulers` 的错误分支写成 `return errors.Trace(err)` 而不是 `e`；Rust 使用 `mgr.RemoveSchedulers()?` 返回实际错误，避免传播旧变量。后续若追求逐行行为对齐，应先确认 Go 此处是否为缺陷，而不应把错误变量行为机械复制到 Rust。

## 扩展指南

- 新增 RawKV flag 时，应同时修改 `DefineRawBackupFlags`、对应解析方法和 `RawKvConfig`，并核对命令层是否已注册依赖的公共 flag；解析边界测试应放在独立的 `backup_raw_test.rs`，不要嵌入生产文件。
- 改动键范围语义时，必须保持请求 `KeyRange`、元数据 `RawRange` 与恢复端 [`restore_raw.rs`](./restore_raw.rs) 一致，并补充空边界、相等、反向及不同编码格式测试。
- 接入真实 backend/client 时，优先替换 `RunBackupRawWithDefaults` 的内存组装，保留可注入的 `RunBackupRaw` 以便单测；同时实现 URI 校验、manager close、context 取消、tracing 和进度失败清理。
- 调整备份协议字段时，应同时核对 Go `BackupRequest`、`backuppb` 兼容性和 `BackupMeta` 恢复消费方；特别注意 `IsRawKv`、版本 0、CF 与 cipher 不得丢失。
- 修改调度器生命周期时，继续保证“成功、任意中途错误、取消”都恢复，并增加恢复闭包自身失败的可观测性测试；不要让清理错误遮蔽主要备份错误。
- 性能风险主要在 region 估算、请求并发/限速、范围拆分和元数据写入；当前仅发出一个逻辑范围，扩展多范围时需定义进度总量、顺序和部分失败后的元数据原子性。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，其中 Rust 7,032 个；目标文件在索引中且列出 19 个符号。
- RustCodeGraph `node --file br/pkg/task/backup_raw.rs --offset 1 --limit 260`：读取目标文件完整 234 行，核对常量、配置解析、RAII 守卫、请求构造、元数据和默认入口。
- RustCodeGraph `explore "br/pkg/task/backup_raw.rs backup_raw RunBackupRaw ..."` 及针对 `RunBackupRaw`/flags 的查询：确认 `runBackupRawCommand → RunBackupRawWithDefaults → RunBackupRaw`，并确认测试与 parity 引用。
- 读取 [`br/pkg/task/Cargo.toml`](./Cargo.toml) 与 [`br/pkg/task/lib.rs`](./lib.rs)：确认 crate 边界、Go package 映射、模块挂载和公开再导出；该目录不存在 `doc.go`。
- RustCodeGraph 读取 [`br/pkg/task/backup_raw.go`](./backup_raw.go)：核对 Go 的 flag、配置字段、context/tracing、manager/client、请求、进度回调、元数据和清理流程。
- RustCodeGraph 读取 [`br/pkg/task/backup_raw_test.rs`](./backup_raw_test.rs)、[`br/pkg/task/parity_test.rs`](./parity_test.rs) 与 [`br/pkg/task/common_test.rs`](./common_test.rs)：确认独立测试位置、错误后调度器恢复证据及公开配置契约覆盖范围。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另以任务指定命令验证恰有 11 个固定二级标题，并人工复核没有把 stubs/占位接线描述成真实生产 I/O。
