# `br/pkg/task/restore_txn.rs`

## 文件定位

本文件是 `astersql-br-pkg-task` crate 中 TxnKV 备份恢复任务的 Rust 编排层，对应 Go 文件 `br/pkg/task/restore_txn.go`。`br/pkg/task/lib.rs` 以 `pub mod restore_txn` 挂载模块并通过 `pub use restore_txn::*` 导出公开入口；`br/cmd/br/restore.rs::runRestoreTxnCommand` 解析公共恢复参数后，以 `TikvGlue` 调用 `RunRestoreTxn`，因此它位于 `br restore txn` CLI 与恢复运行时之间。

该文件只负责配置归一化、备份元数据读取与模式校验、范围/进度计算，以及把选中的文件交给 `restore_lifecycle.rs`。PD 调度器管理、导入模式切换和 SST 导入并不在本文件中实现。当前 Rust 生产入口仍有明显迁移边界：`RunRestoreTxn` 会创建 `MemStorage` 并写入一份固定的 TxnKV 元数据，而不是像 Go 版本那样从 `cfg.Storage` 打开真实外部存储；真正的 PD/import 运行时还必须由 `Glue::GetRestoreLifecycle` 注入。

## 核心职责

- `merge_file_ranges` 按 `StartKey` 排序并合并重叠或相接的文件范围，为区域分裂进度提供范围数量；它同时累加 `Size` 和文件 `Count`，但不产生 rewrite rule。
- `RunRestoreTxn` 提供 CLI 可见的简化入口，构造一份名为 `txn-1`、大小为 20 的内存备份元数据，再转交可注入存储的核心入口。
- `RunRestoreTxnWithStorage` 完成 TxnKV 恢复编排：调整配置、创建连接管理器、读取 `backupmeta`、拒绝 RawKV 元数据、记录归档大小、建立进度、取得并校验恢复生命周期、执行文件恢复和设置成功状态。
- 文件把具体导入委托给 `RestoreLifecycle::RestoreFiles`；后者负责 pre-work、导入模式、调度器暂停/恢复、worker pool、`GoRestore`/`WaitUntilFinish` 以及 importer 关闭。

## 主要符号

- `fn merge_file_ranges(files: &[backuppb::File]) -> Vec<RangeStats>`：私有纯函数。先复制并按起始键排序；若上一范围无界（`EndKey` 为空）或当前起始键不大于上一终止键，则合并。任一终止键为空时结果保持无界，否则取较大的终止键。大小使用 `saturating_add`，避免 `u64` 溢出回绕。
- `pub fn RunRestoreTxn(g: &dyn Glue, cmdName: &str, cfg: &mut Config) -> Result<()>`：公开便捷入口。它不读取 `cfg.Storage`，只把内置 `MemStorage` 交给下层，因此当前更接近可运行的迁移/对齐入口，而非 Go 版本的完整外部备份恢复入口。
- `pub fn RunRestoreTxnWithStorage(..., storage: &dyn Storage) -> Result<()>`：核心公开入口，也是 `restore_txn_test.rs` 与 `restore_lifecycle_test.rs` 的直接测试面。调用者负责提供可读取 `MetaFile` 的存储和能够生成 `RestoreKind::Txn` 生命周期的 `Glue`。
- `defaultRestoreConcurrency`：当 `Config::Concurrency == 0` 时采用的默认并发值。
- `RestoreKind::Txn`、`RestoreLifecycle::ValidateFiles`、`RestoreLifecycle::RestoreFiles`：跨文件委托点，分别标识恢复类型、保证准备后的 SST 集合未漏掉元数据选中文件、执行真实恢复生命周期。

## 执行流程

1. `br/cmd/br/restore.rs::runRestoreTxnCommand` 创建 `Config`、解析 flags、建立 tracing，并调用 `RunRestoreTxn`。
2. `RunRestoreTxn` 创建 `MemStorage`，序列化固定 `BackupMeta { IsTxnKv: true, Files: [txn-1] }` 到 `MetaFile`，随后调用 `RunRestoreTxnWithStorage`。序列化错误被转换为本 crate 的 `Error`。
3. 核心入口先执行 `cfg.adjust()`；若并发仍为零，再写入 `defaultRestoreConcurrency`。随后调用 `Summary(cmdName)`，取得 keepalive 配置并设置 `PermitWithoutStream = true`。
4. `NewMgr` 使用 keyspace、PD、TLS、需求检查开关和 `NormalVersionChecker` 创建管理器。创建失败会立即返回；创建成功后，后续主体无论成功或失败，函数末尾都会调用 `mgr.Close()`。
5. `ReadBackupMeta(MetaFile, cfg, storage)` 读取元数据。若 `backup_meta.IsRawKv` 为真，返回带 `berrors::ErrRestoreModeMismatch` 的错误，且不会启动进度或请求恢复生命周期。
6. 对 `backup_meta.Files` 调用 `ArchiveSize` 并以 `RestoreDataSize` 记录。空文件集合直接成功返回主体，但不会进入进度、生命周期或 `SetSuccessStatus(true)`。
7. 非空时记录文件数量，用 `merge_file_ranges` 计算合并范围，以 `ranges.len() + files.len()` 作为总进度：前一部分代表 split/scatter，后一部分代表逐文件导入。
8. `getEndKeys` 过滤掉空终止键，本文件直接按非空终止键数量推进 split 进度；这里没有像 Go 版本那样实际调用 `client.SplitPoints`。
9. 通过 `g.GetRestoreLifecycle(RestoreKind::Txn, &files)` 获取运行时，先用 `ValidateFiles` 对比文件名、起止键、CF 和大小，再调用 `RestoreFiles(..., concurrency, online=false, checkpoint=false, progress_callback)`。
10. `RestoreFiles` 返回后关闭进度；错误继续向上传播，成功时设置全局摘要成功状态。最后关闭 manager 并返回主体结果。

## 数据与状态

- 输入配置 `Config` 会被原地修改：`adjust` 补齐 keepalive、checksum 并发和元数据批量大小等默认值，本文件还补齐恢复并发。调用者应把 `cfg` 视为执行后的有效配置，而非不可变请求。
- 备份元数据的关键判别位是 `BackupMeta::IsRawKv`；当前代码并未显式要求 `IsTxnKv == true`，只要不是 RawKV 就会继续。文件列表随后被移动出元数据。
- 合并范围仅用于进度估算和终止键计数，不改变传给生命周期的原始 `files`。因此两个重叠文件会得到一个范围单位，但仍得到两个导入单位。
- `RangeStats::EndKey` 为空表示无界终点；无界范围吸收后续范围，且 `getEndKeys` 不为它生成 split 进度。
- 可观察状态包括 `Glue::Record(RestoreDataSize, ...)`、摘要的文件计数与成功标志，以及 `Progress` 的当前值/关闭状态。测试中的 `RecordingGlue` 对记录和进度使用 `Mutex`，`MemProgress` 用原子量计数。
- `RestoreLifecycle` 持有 PD 客户端、调度器管理器、导入模式传输层、importer、准备好的 file sets 和可选 checkpoint 状态；本文件只借助其公开方法，不拥有这些对象的构造细节。

## 依赖与调用关系

上游接线为 `NewRestoreCommand` 注册 txn 子命令，命令执行函数 `runRestoreTxnCommand` 调用 crate 根重导出的 `RunRestoreTxn`。测试上游包括 `restore_txn_test.rs` 对 `RunRestoreTxnWithStorage` 的直接调用、`restore_lifecycle_test.rs::verify_entry(RestoreKind::Txn)` 的生命周期验证，以及 `parity_test.rs` 对公开便捷入口的烟雾调用。

本文件的直接下游可分为四组：`common::{Config, GetKeepalive, NewMgr, ReadBackupMeta}` 处理公共配置、连接管理和元数据；`stubs` 提供跨平台的 `Glue`、`Storage`、错误、摘要和 protobuf 形状；`restore_raw::getEndKeys` 复用范围终点提取；`restore_lifecycle` 提供实际导入运行时。`Cargo.toml` 将该目录声明为 `astersql-br-pkg-task` library（入口 `lib.rs`），并直接依赖 `astersql-br-pkg-restore`、`astersql-br-pkg-conn` 等瘦 BR crate；`serde_json` 用于当前内存元数据序列化。

RustCodeGraph 的文件级关系显示 `restore_lifecycle_test.rs` 使用本文件；函数级 `callers`/`callees` 查询当前未返回边，因此 CLI 与测试调用关系由索引源码及模块重导出交叉核验，而不是把缺失图边解释为“无调用者”。

## 错误处理与边界

- `serde_json::to_vec`、`NewMgr`、`ReadBackupMeta`、`GetRestoreLifecycle`、`ValidateFiles` 和 `RestoreFiles` 的错误都经 `Result` 返回；主体用闭包保留统一的 manager 关闭路径。
- RawKV 元数据返回 `ErrRestoreModeMismatch` 注解，错误消息为 `cannot do transactional restore from raw data`。`restore_txn_test.rs` 验证该错误发生在启动进度之前。
- 默认 `Glue::GetRestoreLifecycle` 返回 `restore PD/import lifecycle is not configured`。因此普通 `MemGlue` 无法完成非空文件恢复；调用方必须提供 `RestoreGlue` 或等价覆盖实现。
- `ValidateFiles` 防止生命周期 factory 静默丢弃选中的备份文件；比较字段为文件名、起止键、CF 和大小，顺序不敏感。它并不校验所有 protobuf 字段。
- `RestoreFiles` 出错时，`update_ch.Close()` 已执行，然后错误传播；manager 也会关闭。若获取生命周期或文件校验在 `RestoreFiles` 之前失败，则当前代码会提前离开主体而没有显式关闭进度，这一点与调用成功后的路径不同，扩展时应谨慎处理。
- 空文件集合会在记录归档大小后返回 `Ok(())`，但不设置成功状态。Go 版本同样在文件为空时提前返回，不过它的 `summary.Summary` 是 defer；Rust 当前是在开始阶段直接调用 `Summary`，语义并非完全相同。
- `merge_file_ranges` 将相接边界（`StartKey == EndKey`）视为可合并，并用饱和加法处理大小；空终止键是无界哨兵，不能改成普通最小值处理。

## 并发与资源生命周期

本文件本身不创建线程或 async task。并发度以 `cfg.Concurrency` 传给 `RestoreLifecycle::RestoreFiles`，后者以 `concurrency.max(1)` 创建 worker pool；因此即使上层默认值异常为零，导入层仍至少使用一个 worker。

`Mgr` 在创建成功后覆盖主体所有返回路径，并在函数末尾显式 `Close`。进度对象是 `Arc<dyn Progress>`：闭包捕获其 clone，由 restorer 在文件完成时并发调用 `IncBy`，原对象在 `RestoreFiles` 返回后关闭。`RestoreLifecycle::RestoreFiles` 内部以 `RestoreSession::Drop` 恢复调度器/normal mode，并无论结果如何调用 importer 的 `Close`；Txn 路径固定 `online=false`、`checkpoint=false`，测试确认会切换 Import → Normal 并恢复调度器。

keepalive 的 `PermitWithoutStream` 在创建 manager 前置为真，以允许连接池空闲时发送心跳。与 Go 版本不同，Rust 文件没有显式 cancellation context，也没有在本文件中创建/关闭独立 restore client；这些资源被收敛到注入的生命周期及其拥有者。

## 与 Go 版本的对应关系

对齐部分包括：配置调整与默认并发、连接 manager 创建、非 RawKV 模式检查、归档大小和文件数统计、范围数加文件数的进度模型、离线 import-mode 恢复、逐文件进度回调、成功摘要，以及错误后的资源清理意图。`merge_file_ranges` 对应无 rewrite rules 时的 `restoreutils.MergeAndRewriteFileRanges` 核心区间合并语义。

尚未等价的部分必须视为当前迁移状态：Go 入口从真实存储读取 backupmeta，并创建/配置 `snapclient.RestoreClient`（速率、加密、单 store 并发、连接、schema 初始化）；Rust `RunRestoreTxn` 使用固定 `MemStorage`。Go 调用真实 `SplitPoints`，Rust 只按 `getEndKeys` 数量推进进度。Go 在本函数中完成 `RestorePreWork`、`GoRestore` 与 `WaitUntilFinish`，Rust 把这些行为委托给外部注入的 `RestoreLifecycle`。Go 的 summary 与 context cancel 使用 defer，Rust 没有等价 cancellation context，且 `Summary` 在开头调用。当前 Rust 代码也没有使用 `cfg.RateLimit`、`CipherInfo`、`ExplicitFilter` 或 schema reader 完成 Go 中的初始化。

因此不能仅凭 CLI 已接线断言真实备份介质上的 TxnKV 恢复已经完整可用；现有证据证明的是编排契约和可注入生命周期能够执行真实 restorer 路径。

## 扩展指南

- 若要接通真实存储，优先修改 `RunRestoreTxn` 的存储获取方式，并保持 `RunRestoreTxnWithStorage` 作为可测试核心；同时补充独立测试，证明 `cfg.Storage`、加密元数据与读取错误被正确处理。
- 若要补齐 split/scatter，不应只增加进度；应在生命周期或明确的下游客户端接口中执行真实 split，并为失败时进度关闭和 manager/importer 清理增加回归测试。
- 若修改范围合并，必须覆盖乱序输入、相接/重叠范围、完全包含、空 `EndKey`、大小饱和与空文件。测试逻辑继续放在同目录独立 `restore_txn_test.rs`，不要内嵌进生产文件。
- 若增加 Txn 模式校验，需明确是否同时要求 `IsTxnKv`，并与 Go 的 `client.IsRawKvMode()` 语义及旧元数据兼容性对齐。
- 若改变 lifecycle factory 或 prepared file sets，保留 `ValidateFiles` 的“不丢文件”不变量；字段扩展应同步 `restore_lifecycle_test.rs::prepared_files_cannot_omit_selected_data`。
- 若改错误路径，建议用作用域守卫确保进度在 `GetRestoreLifecycle`、`ValidateFiles` 和恢复错误时均关闭，并验证摘要成功位只在完整完成后设置。
- 性能风险集中在 `files.to_vec()` 与排序的 `O(n log n)` 成本、全部文件元数据常驻内存、进度回调同步开销和导入并发；兼容风险集中在范围边界、Raw/Txn 判别、file-set 完整性与 import-mode 清理。

## 验证依据

- 目标实现：`br/pkg/task/restore_txn.rs`，重点符号为 `merge_file_ranges`、`RunRestoreTxn`、`RunRestoreTxnWithStorage`。
- 模块与 crate：`br/pkg/task/lib.rs` 的模块挂载/重导出，`br/pkg/task/Cargo.toml` 的 library 边界和依赖。
- 上游入口：`br/cmd/br/restore.rs::runRestoreTxnCommand`。
- 下游实现：`br/pkg/task/common.rs::{Config::adjust, NewMgr, ReadBackupMeta}`、`br/pkg/task/restore_raw.rs::getEndKeys`、`br/pkg/task/stubs.rs::Glue`、`br/pkg/task/restore_lifecycle.rs::{RestoreGlue, ValidateFiles, RestoreFiles}`。
- Go 对照：`br/pkg/task/restore_txn.go::RunRestoreTxn`。
- 独立 Rust 测试：`br/pkg/task/restore_txn_test.rs::{txn_restore_reads_meta_merges_ranges_and_tracks_go_progress, txn_restore_rejects_raw_meta_before_starting_progress}`、`br/pkg/task/restore_lifecycle_test.rs::{txn_restore_consumes_real_restorer_and_offline_mode_lifecycle, prepared_files_cannot_omit_selected_data}`，以及 `br/pkg/task/parity_test.rs` 的公开入口烟雾调用。
- RustCodeGraph：`status` 确认索引含 7032 个 Rust 文件；`files --filter br/pkg/task/restore_txn.rs` 确认目标已索引；`node --file` 读取目标、CLI、Go 对照、测试和生命周期实现；`query RunRestoreTxn`、`query getEndKeys`、`query NewMgr`、`query ReadBackupMeta` 定位定义与接线；函数级 `callers`/`callees` 无输出，已由上述源码证据补足。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验证要求文档存在且恰好包含这里的十一个固定二级标题。
