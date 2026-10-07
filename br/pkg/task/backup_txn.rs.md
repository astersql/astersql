# `br/pkg/task/backup_txn.rs`

## 文件定位

`backup_txn.rs` 是 `astersql-br-pkg-task` crate 中的 TxnKV 备份编排模块。crate 根 `br/pkg/task/lib.rs` 通过 `#[path = "backup_txn.rs"] pub mod backup_txn` 挂载它，并用 `pub use backup_txn::*` 向命令层平铺导出公开 API；`br/pkg/task/Cargo.toml` 则把该目录定义为 library crate，并说明当前 arm64 Darwin 迁移路径使用本地 trait/stub 与精简 BR crate，而不是完整的 Go 运行时依赖。

命令侧入口是 `br/cmd/br/backup.rs::runBackupTxnCommand`：它构造 `TxnKvConfig`、调用 `ParseBackupConfigFromFlags`，再经 `RunBackupTxnWithDefaults` 进入本文件的主流程。该文件对应 Go 实现 `br/pkg/task/backup_txn.go`，但当前 Rust 默认入口使用 `MemBackupClient`，因此应把它理解为迁移期可测试的瘦编排，而不能等同于已经接入真实 PD/TiKV RPC 和对象存储的完整生产实现。

## 核心职责

本文件承担四类职责：

1. `DefineTxnBackupFlags` 注册 TxnKV 备份专用 CLI 参数，包括起止键、历史 `start-version`、keyspace、压缩类型以及隐藏的调度器移除开关。
2. `TxnKvConfig::{ParseFromFlags, ParseBackupConfigFromFlags, Adjust}` 完成范围合法性校验、公共配置解析、压缩/调度器参数装载和默认并发补齐。
3. `RunBackupTxn` 以注入的 `Mgr` 和 `BackupClient` 编排存储占用检查、可选调度器摘除、备份时间戳获取、全 keyspace 范围备份、`BackupMeta` 标记与摘要成功状态。
4. `RunBackupTxnWithDefaults` 为 CLI 提供便捷装配：创建无需 domain 的 manager，并构造固定 `cluster_id=1`、`current_ts=42` 的 `MemBackupClient` 后调用核心函数。

它不负责真正实现范围 RPC、PD 管理或元数据持久化协议；这些能力分别通过 `BackupClient`、`Mgr`、`Glue` 和 `MetaWriter` 抽象取得，当前具体定义来自 `br/pkg/task/stubs.rs`。

## 主要符号

- `flagStartVersion: &str`：公开的历史 CLI 参数名 `start-version`。`TxnKvConfig::StartVersion` 会保存这一配置形状，但当前请求固定写 `StartVersion: 0`，运行流程没有消费该字段。
- `SchedulerRestoreGuard(Option<RestoreSchedulers>)`：文件私有 RAII guard。`Drop::drop` 取出并执行一次恢复闭包，保证调度器成功摘除后，无论主流程正常返回还是经 `?` 提前返回都尝试恢复；恢复错误被忽略。
- `TxnKvConfig`：公开配置结构，组合公共 `Config`、`StartKey`/`EndKey`、`StartVersion`、`CompressionConfig` 与 `RemoveSchedulers`。字段采用与迁移代码一致的 Go 风格命名。
- `DefineTxnBackupFlags(&mut FlagSet)`：公开 flag 注册函数。它复用 `backup_raw` 的 `flagStartKey`/`flagEndKey`，并将 `flagRemoveSchedulers` 标为隐藏；压缩级别不是在此处注册，而是由公共备份 flag 集合提供后在解析阶段读取。
- `TxnKvConfig::ParseFromFlags`：仅校验已经存在于配置字段中的非空起止键满足 `StartKey < EndKey`，随后调用 `Config::ParseFromFlags` 并读取 keyspace。它不会把 start/end flag 解码回这两个字段。
- `TxnKvConfig::ParseBackupConfigFromFlags`：在上述公共解析之后调用 `parseCompressionFlags`，再读取调度器开关和压缩级别；任一步失败即返回 `Result::Err`。
- `TxnKvConfig::Adjust`：调用公共 `Config::adjust`，并在并发为零时写入 `defaultBackupConcurrency`。
- `RunBackupTxn`：可注入依赖的核心公开函数，是本文件真正的行为边界，也是独立单元测试直接调用的入口。
- `RunBackupTxnWithDefaults`：CLI 使用的公开便利入口。RustCodeGraph 显示其调用者包括 `br/cmd/br/backup.rs::runBackupTxnCommand` 和 `br/pkg/task/parity_test.rs::go_rust_public_contract_matches`。

## 执行流程

`runBackupTxnCommand` 到主流程的调用链为：`runBackupTxnCommand` → `RunBackupTxnWithDefaults` → `RunBackupTxn`。核心函数按以下顺序执行：

1. 调用 `cfg.Adjust()` 补公共默认值和默认并发，并调用 `Summary(cmdName)` 初始化/输出当前摘要语义。
2. 从配置构造 `StorageOptions`，其中强制 `CheckS3ObjectLockOptions=true`；从 client 取得 backend，缺失时使用默认 backend，然后执行 `SetStorageAndCheckNotInUse`，防止同一路径被并发任务占用。
3. 构造唯一的 `KeyRange { StartKey: [], EndKey: [] }`。空边界在当前约定中表示整个 TxnKV keyspace；配置中的 `StartKey`、`EndKey` 不参与实际请求。
4. 若 `RemoveSchedulers=true`，先调用 `mgr.RemoveSchedulers()` 获得恢复闭包并放入 `SchedulerRestoreGuard`。摘除失败会立即返回；摘除成功后的所有退出路径都会触发 guard。
5. 读取 BR 版本、集群版本和空边界覆盖的 region 估算数；记录 `backup total regions`，并以该数值启动进度对象。
6. 通过 `client.GetCurrentTS()` 取得备份点，向 glue 记录 `BackupTS`；构造 `BackupRequest`，固定 `StartVersion=0`、`EndVersion=backupTS`、`IsRawKv=false`，并带入限速、并发、存储 backend、压缩和加密配置。
7. 调用 `client.BackupRanges(&backupRanges, &req)`。成功后关闭进度对象；失败则通过 `?` 返回，当前代码没有显式关闭进度对象。
8. 新建 `MetaWriter`，启动异步元数据阶段，写入版本区间、`IsRawKv=false`、`IsTxnKv=true`、集群 ID、集群/BR 版本和 API 版本；随后 finish、flush，记录归档大小并调用 `SetSuccessStatus(true)`。
9. 函数离开作用域时 `SchedulerRestoreGuard` 尝试恢复调度器，最终返回 `Ok(())` 或此前遇到的首个错误。

## 数据与状态

输入状态集中在可变的 `TxnKvConfig`。`RunBackupTxn` 首先就地调整它，因此调用结束后 `Config` 的默认值（尤其 `Concurrency`）可能已经变化。备份范围不是从配置字段派生，而是始终创建一个空起止键范围；`StartVersion` 字段同样不会影响请求，实际版本窗口恒为 `[0, current_ts]`。

跨组件传递的核心数据是 `BackupRequest`：`ClusterId` 和 `StorageBackend` 来自 client，`EndVersion` 来自 `GetCurrentTS`，限速/并发/压缩/加密来自配置。备份完成后，`MetaWriter::Update` 把请求中的版本和集群标识复制到 `BackupMeta`，额外设置 `IsTxnKv=true`，供恢复侧辨别事务 KV 备份。

可观察状态还包括 `Glue::Record` 中的 `BackupTS` 与 `BackupDataSize`、全局摘要中的 region 数和成功标志，以及可选的 PD 调度器临时状态。`RunBackupTxnWithDefaults` 的 `MemBackupClient` 固定时间戳为 42，这一值是测试/迁移桩数据，不是从真实 PD 获取的时间戳。

## 依赖与调用关系

上游关系：

- `br/pkg/task/lib.rs` 声明并重新导出本模块。
- `br/cmd/br/backup.rs::runBackupTxnCommand` 解析命令配置后调用 `RunBackupTxnWithDefaults`。
- `br/pkg/task/backup_txn_test.rs::test_run_backup_txn_restores_schedulers_on_error` 直接调用 `RunBackupTxn` 注入失败 manager。
- `br/pkg/task/parity_test.rs::go_rust_public_contract_matches` 验证非法范围和默认入口的公共契约。

下游关系：

- `crate::common::{Config, GetKeepalive, NewMgr}` 提供公共配置与 manager 装配。
- `crate::backup::{parseCompressionFlags, defaultBackupConcurrency, CompressionConfig}` 提供压缩解析和并发默认值。
- `crate::stubs::Mgr` 提供集群版本、region 计数和调度器控制；`BackupClient` 提供存储占用检查、当前 TS、范围备份和集群/API 信息；`Glue` 提供版本、进度与记录接口。
- `crate::stubs::backuppb::BackupRequest` 是下发给 client 的请求结构；`MetaWriter` 保存并 flush `BackupMeta`。

`br/pkg/task/Cargo.toml` 没有为本文件声明独立 feature；该模块随 library crate 一起编译。其直接代码依赖主要通过 crate 内模块和 path 依赖暴露，符合 Cargo metadata 中“local traits/stubs + slim BR crates”的迁移定位。

## 错误处理与边界

所有可失败步骤统一返回 `crate::stubs::Result<()>`，并用 `?` 保留首个错误：非法键范围返回带 `berrors::ErrBackupInvalidRange` 分类的注释错误；flag 读取、公共配置、压缩解析、manager/client 操作以及 meta finish/flush 错误均直接向上游传播。成功状态只在备份和元数据 flush 全部完成后设置。

关键边界如下：

- 只有起止键均非空时才比较；任一为空都允许通过。比较采用字节字典序，要求严格 `StartKey < EndKey`。
- 当前 `ParseFromFlags` 不读取 start/end flag，`RunBackupTxn` 也忽略配置中的键界，所以范围字段目前只参与对预填值的校验，不能提供局部 TxnKV 备份。
- `StartVersion` 同样未被实际使用，请求总是从版本 0 开始。
- storage backend 缺失时 `unwrap_or_default`，随后由 `SetStorageAndCheckNotInUse` 决定默认值是否合法。
- 调度器恢复是 best effort：恢复失败被丢弃，不覆盖原任务结果，也没有像 Go 版本那样记录警告。
- `BackupRanges` 失败后进度对象不会显式 `Close`；是否由对象析构完成清理由 `Glue` 的具体实现决定，当前文件没有提供更强保证。
- 当前 `MetaWriter` 在 `BackupRanges` 成功之后才创建，且没有作为参数传给 `BackupRanges`；与 Go 版本边备份边收集 data-file meta 的路径不同，不能据此声称 Rust 已持久化完整文件清单。

## 并发与资源生命周期

本文件没有显式创建线程或 async task。并发度只是作为 `BackupRequest::Concurrency` 传给 `BackupClient`；当配置值为零时，`Adjust` 使用默认备份并发。真实并行策略、速率限制落实和范围调度属于 client 实现，本文件不控制。

资源生命周期由作用域和调用顺序管理：存储先通过占用检查，进度对象在范围备份成功后关闭，`MetaWriter` 按 start → update → finish → flush 顺序完成元数据阶段。`SchedulerRestoreGuard` 是唯一明确的 RAII 清理机制，在调度器成功摘除后覆盖所有后续返回路径；`backup_txn_test.rs` 通过让 `GetClusterVersion` 失败，验证恢复闭包仍在错误返回前执行。

与 Go 版本相比，Rust 核心函数接收 `Arc<dyn Mgr>`，但不会调用 `Mgr::Close`；默认入口也没有建立 `context.WithCancel`、trace span 或显式 manager close guard。`RunBackupTxnWithDefaults` 创建的 manager 在 `Arc` 离开作用域时释放，但 trait 的 `Close` 语义并未由本文件触发。这是当前迁移实现的生命周期差异，扩展真实连接时必须处理。

## 与 Go 版本的对应关系

Rust 的配置形状、flag 名称、键界校验、默认并发、全范围空键、`StartVersion=0`/`EndVersion=current_ts`、压缩/加密字段、Txn meta 标记和调度器退出恢复意图均对应 `br/pkg/task/backup_txn.go`。

当前差异需要明确保留在评审视野中：

- Go `RunBackupTxn` 自己解析对象存储 backend、创建/关闭真实 manager 和 backup client；Rust 将 manager/client 注入核心函数，而默认入口使用 `MemBackupClient`。
- Go 使用 context 取消与 tracing span，Rust 当前接口不接收 context，也没有 tracing。
- Go 在备份前创建真实 `metautil.MetaWriter`，把它和进度回调传入 `BackupRanges`，随后 finish/flush；Rust 的 `BackupClient::BackupRanges` 只接收 ranges/request，`MetaWriter` 在备份后独立更新顶层字段。
- Go 的进度回调会忽略 `UnitRange`、对其他单位递增；Rust 只创建进度对象，并未向 client 传递回调，`let _ = UnitRange` 也不产生行为。
- Go defer 调度器恢复时处理 context 取消并记录恢复失败；Rust guard 无 context 且静默忽略恢复错误。
- Go defer 执行 `summary.Summary`，Rust在流程开始时直接调用 `Summary`；其时序并非严格等价。

因此本文档只确认当前 Rust 源码具备可测试的编排契约，不将 Go 完整 I/O、进度、上下文和元数据流水线能力误报为已迁移。

## 扩展指南

- 若要支持局部 TxnKV 范围，优先修改 `TxnKvConfig::ParseFromFlags` 以采用明确的 key 格式解析 start/end，再修改 `RunBackupTxn` 的 `backupRanges` 和 `GetRegionCount` 边界；同步扩展 `br/pkg/task/backup_txn_test.rs`，覆盖单边界、相等/逆序边界及半开区间语义，并与 Go 行为重新核对。
- 若要启用增量版本，应明确 `StartVersion` 的兼容契约，再让请求使用该字段并为 `start < end`、GC safe point 和 meta 版本窗口增加独立测试；不能仅把字段抄进请求。
- 若接入真实 client，应替换 `RunBackupTxnWithDefaults` 中的 `MemBackupClient`，并为 manager close、context 取消、真实 TS、backend 解析和 storage 释放建立显式生命周期；不得依赖 `Arc` drop 等同于 `Mgr::Close`。
- 若补齐 Go 的元数据流水线，应扩展 `BackupClient::BackupRanges` 或引入等价协作接口，使 data-file meta 和进度回调在范围备份期间产生；随后验证 finish/flush 错误、部分备份和归档大小。
- 修改调度器逻辑时，应保持“成功摘除后所有退出路径都恢复”的不变量，并决定恢复失败是只告警还是参与最终错误；现有 `test_run_backup_txn_restores_schedulers_on_error` 必须同步更新。
- 新增测试应继续放在独立的 `br/pkg/task/backup_txn_test.rs`（公共契约可补到 `parity_test.rs`），不要把测试嵌入生产源文件。

兼容风险主要在 backupmeta 字段和恢复侧判型；正确性风险集中于版本窗口、空范围语义、调度器恢复和部分失败后的资源清理；性能风险则集中于默认/用户并发、region 估算和未来范围拆分策略。

## 验证依据

本说明使用以下直接证据：

- Rust 源码：`br/pkg/task/backup_txn.rs`（17 个索引符号）、`br/pkg/task/lib.rs`、`br/cmd/br/backup.rs::runBackupTxnCommand`。
- crate 边界：`br/pkg/task/Cargo.toml` 的 `[package]`、`[lib]`、`package.metadata.porting` 和 path dependencies。
- Go 对照：`br/pkg/task/backup_txn.go::{TxnKvConfig, DefineTxnBackupFlags, ParseFromFlags, ParseBackupConfigFromFlags, Adjust, RunBackupTxn}`。
- Rust 测试：`br/pkg/task/backup_txn_test.rs::test_run_backup_txn_restores_schedulers_on_error`；`br/pkg/task/parity_test.rs::go_rust_public_contract_matches` 中的非法范围与默认入口断言。仓库内没有同名 `backup_txn_test.go`，因此 Go 行为依据来自生产对照文件及上述 Rust 对齐测试。
- RustCodeGraph：`status` 显示索引包含目标文件；`node --file br/pkg/task/backup_txn.rs` 列出源码和三个使用文件；`explore` 给出 `runBackupTxnCommand → RunBackupTxnWithDefaults → RunBackupTxn` 调用链；`node RunBackupTxn`、`node RunBackupTxnWithDefaults`、`node TxnKvConfig`、`node SchedulerRestoreGuard`、`node BackupClient`、`node MetaWriter`、`node Mgr` 核对了上下游符号与 trait 边界。
- 结构验证要求：目标文件必须存在，且上述十一个固定二级标题各出现一次。该任务为纯文档分析，按计划不运行 Cargo 或代码测试。
