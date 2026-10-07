# `br/pkg/task/backup.rs`

## 文件定位

`backup.rs` 是 `astersql-br-pkg-task` crate 的全量/表级备份配置与控制流模块。crate 根 [`br/pkg/task/lib.rs`](lib.rs) 通过 `pub mod backup` 挂载它，再以 `pub use backup::*` 平铺公开 API；命令层 [`br/cmd/br/backup.rs`](../../cmd/br/backup.rs) 使用 `DefineBackupFlags`、`BackupConfig` 和 `RunBackupWithDefaults` 完成 CLI 接线。该 crate 的边界和依赖由 [`br/pkg/task/Cargo.toml`](Cargo.toml) 定义，其中本文件直接用到同 crate 的 `common`/`stubs`，以及 `astersql-br-pkg-gc`、`serde_json`、`sha2`。

它是 Go [`br/pkg/task/backup.go`](backup.go) 的迁移实现，但不是 Go 主流程的逐语句完整复刻：当前 Rust 版本保留配置解析、不可变配置哈希、TS 解析、GC safepoint、调度器恢复、范围备份和元数据落盘的核心骨架；Go 中的 Domain/统计信息、schema/checksum、DDL、checkpoint runner、锁文件和 tracing 等完整生产编排尚未全部出现在本文件中。因此应把它理解为已接入 Rust BR 命令的可运行迁移路径，而非 Go 功能面的完全等价实现。

## 核心职责

- `DefineBackupFlags` 与 `BackupConfig::ParseFromFlags` 定义并解析备份专用参数，维护增量备份、checkpoint、压缩、并发、GC TTL、keyspace 和 EBS 扩展配置之间的约束。
- `BackupConfig::Adjust` 为 BR 二进制和嵌入式调用统一补默认值并限制危险组合：并发限制在 `1..=256` 的有效策略内，启用速率限制时退化为单并发，未知压缩回落到 ZSTD。
- `BackupConfig::Hash` 只哈希会影响 checkpoint 兼容性的不可变字段，允许运行时调优字段在重试间变化。
- `RunBackup`/`run_backup_body` 组织一次范围备份：确定 TS、注册 GC 保护、可选移除调度器、校验存储、构造请求、执行范围 RPC、写入 backup meta 并设置成功状态。
- `ParseTSString` 及其私有日期函数把空值、TSO 字面量和带/不带时区的 civil datetime 转为 TSO。
- `DefaultBackupConfig` 和 `RunBackupWithDefaults` 提供默认配置及内存 client 路径；后者目前由 Rust CLI 使用，也供测试/对齐场景使用。

## 主要符号

- 旗标与命令常量：`flagBackupTimeago`、`flagBackupTS`、`flagLastBackupTS`、`flagCompressionType`、`flagUseCheckpoint`、`flagKeyspaceName` 等字符串必须与 Go CLI 名称保持一致；`FullBackupCmd` 等名称参与 `isFullBackup` 判定与摘要展示。
- `CompressionConfig { CompressionType, CompressionLevel }`：封装 TiKV SST 压缩请求参数。
- `BackupConfig`：聚合公共 `Config` 与备份字段。`BackupTS == 0` 表示运行时取当前 TS；`LastBackupTS > 0` 表示增量起点；`UseCheckpoint` 控制失败时是否保留 GC 屏障。
- `DefineBackupFlags(&mut FlagSet)`：注册默认值；`remove-schedulers` 与 `use-checkpoint` 被标为隐藏旗标。
- `BackupConfig::{ParseFromFlags, Adjust, Hash}`：分别承担输入校验、默认值归一化和 checkpoint 配置指纹计算。
- `RunBackup`：公开、可注入 `Mgr`/`BackupClient` 的执行入口；`MgrCloseGuard` 保证任意返回路径关闭 manager。
- `BackupGCGuard`：RAII 清理器；取消 keeper，并根据成功状态和 checkpoint 策略决定是否删除 service safepoint。
- `run_backup_body`：私有主体，串联 `Mgr`、`Glue`、`BackupClient`、`MetaWriter`。
- `ParseTSString`、`parse_datetime_millis`、`valid_civil_datetime`、`days_from_civil`、`local_epoch_seconds`：时间解析链；Unix 使用 `mktime` 遵循本地时区，非 Unix 分支按无偏移 civil time 计算。
- `parseCompressionType`、`parseReplicaReadLabelFlag`：小型输入解析器；前者只接受 `lz4|snappy|zstd`，后者只接受恰好一个冒号分隔的 `key:value`。
- `RunBackupWithDefaults`：创建 `NewMgr` 和固定 `cluster_id=1/current_ts=100` 的 `MemBackupClient` 后委托 `RunBackup`。

## 执行流程

1. CLI 在 [`br/cmd/br/backup.rs`](../../cmd/br/backup.rs) 中调用 `DefineBackupFlags`，执行时填充 `BackupConfig`，最终调用 `RunBackupWithDefaults`。
2. `RunBackup` 首先创建 `MgrCloseGuard`，随后执行 `cfg.Adjust()`、初始化 `Summary`，最后进入 `run_backup_body`；无论成功或 `?` 提前返回，manager 都会在栈展开时关闭。
3. `run_backup_body` 读取集群/BR 版本与 region 估算值，启动进度通道。若 `BackupTS` 为零，通过 `BackupClient::GetCurrentTS` 获取当前点，并用 `Glue::Record` 记录。
4. 增量备份先验证 `backupTS > LastBackupTS`。随后从 `Mgr::GetGCManager` 获取 GC 管理器，注册 `BRServiceSafePoint`：全量保护 `backupTS`，增量保护 `LastBackupTS`；命名 keyspace 缺少 GC manager 时直接失败，禁止无保护快照。
5. 若配置 `RemoveSchedulers`，调用 `Mgr::RemoveSchedulers` 并持有局部 `SchedulerGuard`，退出时调用返回的恢复闭包。
6. client 提供存储后，`SetStorageAndCheckNotInUse` 携带凭据选项及 S3 Object Lock 检查。然后构造 `BackupRequest`，其版本区间是 `[LastBackupTS, backupTS]`，并带上限速、并发、存储、压缩与加密信息。
7. `BuildBackupRanges` 根据过滤器、TS 和是否全备生成范围；非空才调用 `BackupRanges`，空范围不会发备份 RPC。范围阶段结束后关闭进度通道。
8. `MetaWriter` 异步写入并刷新元数据，记录版本、cluster id、BR/集群版本及 API version；成功刷新后记录归档大小，将 GC guard 标记为完成并调用 `SetSuccessStatus(true)`。
9. 离开作用域时先后触发调度器恢复、GC keeper 清理和 manager 关闭。checkpoint 模式下若主体失败，`BackupGCGuard` 保留 safepoint 直至 TTL；成功时仍删除。

## 数据与状态

`BackupConfig` 是本模块的中心可变状态。`ParseFromFlags` 会在 `LastBackupTS > 0` 时强制关闭 `UseCheckpoint`；`Adjust` 会修改并发、TTL、压缩类型和云 API 并发，因此调用方若要计算与最终运行一致的值，应先明确是否需要归一化。`Hash` 使用 JSON 序列化后 SHA-256，输入包括 `LastBackupTS`、`IgnoreStats`、`UseCheckpoint`、对象存储 backend options、存储 URI、PD 列表、凭据开关、过滤器、cipher 和 keyspace；并发、限速、TLS、`BackupTS`、TTL 等调优/瞬时字段有意不进入哈希。

`run_backup_body` 的关键派生状态是 `backupTS`、`BackupRequest` 和 `BackupGCGuard.complete`。只有 `FlushBackupMeta` 成功后才把 `complete` 置为 `true`，这使“备份数据已写但元数据未持久化”仍被视为失败路径。`MetaWriter` 当前在函数内新建，更新内容通过闭包写入；归档大小通过 `Glue::Record(BackupDataSize, ...)` 进入摘要。成功状态由 `SetSuccessStatus(true)` 写入全局摘要状态。

## 依赖与调用关系

上游调用关系经 RustCodeGraph 符号查询和仓库搜索核对：[`br/cmd/br/backup.rs`](../../cmd/br/backup.rs) 调用 `DefineBackupFlags` 与 `RunBackupWithDefaults`；同 crate 的 [`restore.rs`](restore.rs) 和 [`stream.rs`](stream.rs) 复用 `ParseTSString`；[`common_test.rs`](common_test.rs) 使用 `DefaultBackupConfig`。crate 根 `lib.rs` 的通配重导出让命令层以 `astersql_br_pkg_task::*` 风格访问这些符号。

下游依赖主要分三层：`common::{Config, TLSConfig, NewMgr, GetKeepalive}` 提供公共配置与 manager 工厂；`stubs::{Mgr, BackupClient, Glue, MetaWriter, FlagSet}` 定义迁移期抽象和内存实现；`astersql_br_pkg_gc::{Manager, BRServiceSafePoint, StartServiceSafePointKeeper}` 提供真实 GC 屏障生命周期。`serde_json` 与 `sha2` 只服务于 `BackupConfig::Hash`。

RustCodeGraph 精确查询确认了 `BackupConfig`、`DefineBackupFlags`、`ParseTSString` 和 `RunBackupWithDefaults` 的定义位置；调用图命令在当前索引上未返回边，因此调用者证据以命令层和同 crate 的直接符号引用搜索补齐，未据此推断动态 trait 调用的具体实现。

## 错误处理与边界

- `ParseFromFlags` 通过 `Result` 传播 FlagSet 错误，并显式拒绝负 `timeago`、非正 `range-limit`、非法 full backup type、非法压缩名和非法 replica label。Rust `Duration` 本身不能表达负值，因此当前 `as_secs_f64() < 0.0` 分支事实上不可达；真正的负值是否在 `FlagSet` 层被拒绝取决于其实现。
- 增量备份要求 `backupTS` 严格大于 `LastBackupTS`，否则返回 `ErrInvalidArgument`；测试证明错误返回后 manager 仍关闭。
- 命名 keyspace 必须有 GC manager；GC 注册失败会阻止范围读取。注册过程中 guard 已建立，所以失败路径仍尝试删除 safepoint。
- 空范围是成功情况：跳过 `BackupRanges`，仍刷新 backup meta。该语义由 `test_run_backup_skips_rpc_for_empty_ranges` 固定。
- `ParseTSString` 空串返回零、纯数字原样返回；`tzCheck=true` 时 datetime 必须带 `Z` 或偏移。日期检查拒绝非法闰日、越界时间、超过六位的小数以及超过 `±14:00` 的偏移。Unix 无偏移时间依赖进程本地时区，跨环境比较时不能假定 UTC。
- `Drop` 中删除 safepoint或恢复调度器失败只写 `stderr`，不能覆盖主体返回值；因此调用者需依靠运维日志发现清理失败。

## 并发与资源生命周期

本文件不直接创建 Rust 线程或 Tokio task；并发度只作为 `BackupRequest.Concurrency` 和配置字段下传。`MetaWriter::StartWriteMetasAsync` 表明元数据写入由 writer 内部异步执行，调用方必须按顺序执行 `FinishWriteMetas` 再 `FlushBackupMeta`，本文件遵守这一生命周期。

资源清理由 RAII 保证：`MgrCloseGuard` 总是关闭 manager；`SchedulerGuard` 在作用域结束时恢复被移除的调度器；`BackupGCGuard` 总是先调用 cancel，成功或非 checkpoint 失败再删除 safepoint，checkpoint 失败则保留屏障。进度通道只在范围构建和 RPC 正常结束后显式 `Close`；在更早的错误路径上是否自动释放依赖 `Progress` 实现的析构语义，本文件没有额外 guard，这是扩展错误分支时需关注的资源边界。

`BackupClient` 与 `Mgr` 以 trait 引用/`Arc<dyn Mgr>` 注入，使测试可以观察调用顺序。`BackupGCGuard` 持有 GC manager 的 `Arc` 与 `Send + Sync` cancel 闭包，确保 guard 存活期间清理依赖不会先析构。

## 与 Go 版本的对应关系

配置层高度对应 [`backup.go`](backup.go)：旗标名称/默认值、`LastBackupTS` 关闭 checkpoint、并发上限、限速强制单并发、默认 GC TTL/ZSTD、不可变哈希字段、压缩与 replica label 解析均保持相同意图。Rust 独立测试 [`backup_test.rs`](backup_test.rs) 对照 Go [`backup_test.go`](backup_test.go) 的 TS、压缩和哈希断言，并额外覆盖 Rust RAII 与 GC 生命周期。

执行层只对应 Go `RunBackup` 的安全主干。Rust 已有：manager 关闭、TS 选择、增量单调性、GC safepoint、可选调度器移除、存储占用检查、请求构造、范围备份、meta flush 和成功摘要。当前未在本文件实现的 Go 步骤包括：全局 keyspace 配置、backend URI 解析/noop checkpoint 降级、Domain 与 stats handle、集群 checkpoint 能力检查、collation 查询、`TableBackupClient` 初始化、checkpoint 配置校验与 lock file、checkpoint runner/清理、schema/policy/DDL/checksum 备份、精确 range progress、failpoint、tracing 及 `Size` 双重摘要记录。新增功能不能仅凭 Go 已存在就宣称 Rust 已支持，应先补齐对应抽象和独立测试。

另一个显著接线差异是 Go `RunBackup` 内部创建真实 manager/client，而 Rust 将它们注入；Rust CLI 的 `RunBackupWithDefaults` 当前使用 `MemBackupClient`。这便于离线测试，但也意味着评估生产完备性时必须同时检查命令层和 `stubs` 的真实/内存实现边界。

## 扩展指南

- 新增备份旗标时，同步修改 `DefineBackupFlags`、`BackupConfig`、`ParseFromFlags`；若 BR 嵌入 TiDB 时也需要默认值，再更新 `Adjust`。同时核对 Go `backup.go` 及 CLI 注册位置，避免只在一端可见。
- 改动 checkpoint 相容字段时必须审查 `BackupConfig::Hash` 的序列化名称与字段集，并扩展 `backup_test.rs::test_backup_config_hash`；随意加入并发、TTL 等可变字段会破坏重试兼容。
- 扩展执行流程优先接入 `run_backup_body` 的既有阶段，并保持“GC 屏障先于任何快照读取、meta flush 后才算完成”的不变量。新增早退路径应验证 manager、调度器、GC guard 和进度通道的清理。
- 增加真实 schema/checksum/checkpoint 功能时，不应把测试写进 `backup.rs`；按仓库约定扩展独立 [`backup_test.rs`](backup_test.rs)，并以 Go `backup_test.go`/`RunBackup` 为行为基准。
- 修改时间解析需覆盖 Unix 本地时区、显式 offset、UTC `Z`、闰年、非法日期和小数精度；非 Unix fallback 与 Go `time.ParseInLocation` 的差异应显式记录。
- 性能敏感点包括范围数量、请求并发、限速与 meta 缓冲；配置调整不得绕过 `maxBackupConcurrency`，且有 rate limit 时要保持单并发语义。

## 验证依据

- 源码全貌：[`br/pkg/task/backup.rs`](backup.rs)，核对了全部常量、`CompressionConfig`、`BackupConfig`、公开函数、私有日期函数、两个 RAII guard 与条件编译的 `local_epoch_seconds`。
- crate/模块证据：[`br/pkg/task/Cargo.toml`](Cargo.toml) 与 [`br/pkg/task/lib.rs`](lib.rs)，确认 crate 名、依赖、模块挂载和重导出。
- 上游接线：[`br/cmd/br/backup.rs`](../../cmd/br/backup.rs)，确认 `DefineBackupFlags` 和 `RunBackupWithDefaults` 的直接调用。
- Go 对照：[`br/pkg/task/backup.go`](backup.go) 与 [`br/pkg/task/backup_test.go`](backup_test.go)，核对配置、哈希、TS 解析及完整 Go `RunBackup` 的迁移差异。
- Rust 测试：[`br/pkg/task/backup_test.rs`](backup_test.rs)、[`br/pkg/task/common_test.rs`](common_test.rs) 与 [`br/pkg/task/parity_test.rs`](parity_test.rs)，确认纯函数、默认配置、空范围、manager 关闭、增量 TS、GC 成功/失败/checkpoint 生命周期和注册失败行为。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter br/pkg/task` 确认模块与对照文件；`query BackupConfig/DefineBackupFlags/ParseTSString/RunBackupWithDefaults --json` 确认定义位置。`node --file`、`explore` 和调用边命令本次未返回内容，故未将其空结果作为调用关系证据，而以直接引用搜索补足。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令检查目标文件存在且恰有十一个固定二级标题，并人工检查所有本地链接与迁移边界陈述。
