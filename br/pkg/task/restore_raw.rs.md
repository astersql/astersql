# `br/pkg/task/restore_raw.rs`

## 文件定位

该文件属于 Cargo crate `astersql-br-pkg-task`（见 `br/pkg/task/Cargo.toml`），是 BR 任务层的 RawKV 恢复编排实现，与 Go 文件 `br/pkg/task/restore_raw.go` 对照。`br/pkg/task/lib.rs` 通过 `#[path = "restore_raw.rs"] pub mod restore_raw` 挂载模块，并以 `pub use restore_raw::*` 对外平铺导出其公开 API。

应用入口位于 `br/cmd/br/restore.rs::runRestoreRawCommand`：命令层构造 `RestoreRawConfig`、解析参数并在 tracing 包装中以 `TikvGlue` 调用 `RunRestoreRaw`。因此本文件位于“CLI 参数与环境准备”和“实际 PD/import 客户端生命周期”之间：它负责选择备份文件、计算进度与恢复参数，真实导入由 `Glue::GetRestoreLifecycle` 返回的 `restore_lifecycle::RestoreLifecycle` 完成。

当前 Rust 实现已经接入命令路径和真实 restorer 抽象，但并非逐语句复制 Go 的完整客户端初始化流程。尤其是默认 `RestoreStorage` 路径会构造一份内存 `BackupMeta`；生产级外部存储与 PD/import 客户端必须由嵌入方的 storage/lifecycle 绑定提供，不能仅凭本文件断言已完成真实集群端到端恢复。

## 核心职责

- `DefineRawRestoreFlags` 注册 RawKV 专有的 key 编码、列族、起止 key 参数，并追加通用恢复参数。
- `RestoreRawConfig::ParseFromFlags` 将 flag 分别解析到通用恢复配置和 RawKV 配置；`adjust` 归一化配置并为零并发设置默认值。
- `files_in_raw_range` 按列族和半开区间 `[start_key, end_key)` 验证请求范围是否被某个备份 RawRange 完整覆盖，再筛选相交的备份文件。
- `merge_file_ranges` 将按起始 key 排序后的相邻/重叠文件，在大小与文件数阈值内合并为 `RangeStats`；`getEndKeys` 提取非空结束 key。
- `RunRestoreRaw` 串联 manager、backupmeta、范围过滤、归档大小统计、进度、生命周期校验与 SST 导入，并确保已成功创建的 manager 在任务结束时关闭。

本文件不负责 SQL schema 恢复、key rewrite 或 checkpoint 恢复。调用 `RestoreLifecycle::RestoreFiles` 时使用 `RestoreKind::Raw`、`checkpoint = false`，也不传递 snapshot 专用 key ranges。

## 主要符号

- `pub struct RestoreRawConfig`：组合 `RawKvConfig` 与 `RestoreCommonConfig`，并增加 Rust 侧注入点 `RestoreStorage: Option<MemStorage>`。后者不在 Go 同名结构体中，用于测试或嵌入式调用传入 backupmeta 存储。
- `pub fn DefineRawRestoreFlags(flags: &mut FlagSet)`：公开 flag 注册函数。默认 key 格式为 `hex`、CF 为 `default`，空起止 key 表示不限制对应边界。
- `RestoreRawConfig::ParseFromFlags(&mut self, &FlagSet) -> Result<()>`：先读取 `flagOnline`，再解析 `RestoreCommonConfig` 和 `RawKvConfig`；任一阶段失败即返回错误。
- `RestoreRawConfig::adjust(&mut self)`：内部归一化函数。依次调用 Raw 内嵌 `Config::adjust`、通用恢复配置 `adjust`，最后将零并发回退到 `defaultRestoreConcurrency`。
- `pub fn getEndKeys(&[RangeStats]) -> Vec<Vec<u8>>`：被本文件和 `restore_txn.rs` 复用；跳过空 `EndKey`，因为空值代表无界终点，不应作为 split 边界。
- `fn files_in_raw_range(...) -> Result<Vec<File>>`：私有的元数据范围选择器。它要求请求区间完整包含于匹配 CF 的某个 `RawRange`，然后保留与请求范围相交的同 CF 文件。
- `fn merge_file_ranges(...) -> Vec<RangeStats>`：私有的文件范围合并器。使用 `saturating_add` 累加大小和文件计数，避免整数溢出；阈值为零时表示该维度不限制合并。
- `pub fn RunRestoreRaw(&dyn Glue, &str, &mut RestoreRawConfig) -> Result<()>`：RawKV 恢复公开入口，也是本文档的主流程。

文件没有 trait、enum、宏或条件编译项；类型和函数均为同步接口，异步导入细节封装在 lifecycle/restorer 内部。

## 执行流程

1. `RunRestoreRaw` 调用 `cfg.adjust()`，随后调用 `Summary(cmdName)` 初始化任务摘要。
2. 用 keyspace、PD、TLS、keepalive、需求检查标志和 `needDomain = false` 调用 `NewMgr`。RawKV 恢复不需要 TiDB domain。
3. 选择 backupmeta 存储：若 `RestoreStorage` 已注入则使用它；否则构造 `MemStorage`，写入一段与配置起止 key/CF 相同的 RawRange 和单个占位文件 `f1`。该默认分支是平台中立适配，不代表从配置 URL 读取真实归档。
4. `ReadBackupMeta(MetaFile, ...)` 解析元数据；若 `BackupMeta::IsRawKv` 为假，以 `ErrRestoreModeMismatch` 拒绝事务备份。
5. `files_in_raw_range` 验证请求范围覆盖关系并选择相交文件；`ArchiveSize` 的结果通过 `Glue::Record(RestoreDataSize, ...)` 记录。
6. 若没有文件，直接成功返回，不启动进度条和 lifecycle，也不调用 `SetSuccessStatus(true)`。这与 Go 路径在空文件处提前返回的控制流一致。
7. 对非空文件记录 `CollectInt("restore files", files.len())`，再根据 `MergeSmallRegionSizeBytes.Value` 和 `MergeSmallRegionKeyCount.Value` 合并范围。
8. 启动总量为 `ranges.len() + files.len()` 的 `Raw Restore` 进度。Rust 当前以 `getEndKeys(&ranges).len()` 直接推进 split 部分计数；本文件没有像 Go 版本那样显式调用 `client.SplitPoints`。
9. 从 glue 获取 `RestoreKind::Raw` lifecycle；`ValidateFiles` 比较被元数据选中的文件与 lifecycle 准备的 `file_sets`，防止工厂静默遗漏 SST。
10. `RestoreFiles` 使用配置的 mode 切换间隔、并发和 online 标志执行简单 SST restorer，并通过回调推进文件恢复进度；checkpoint 固定关闭。
11. 无论 `RestoreFiles` 返回成功还是失败，其返回之后都会关闭进度；只有成功才设置全局成功状态。闭包结束后 `mgr.Close()`，最后传播闭包结果。

## 数据与状态

配置状态分成三层：`RawKvConfig` 保存 `[StartKey, EndKey)`、`CF` 及底层连接/并发配置；`RestoreCommonConfig` 保存 online 和小 region 合并阈值；`RestoreStorage` 保存可选的内存元数据源。`RunRestoreRaw` 会原地调整配置，所以调用者在返回后可能观察到默认值补全（尤其是 `Concurrency`）。

范围判断采用字节字典序。备份 `RawRange.EndKey` 或请求 `end_key` 为空时表示无界终点。文件筛选条件保留 `file.EndKey >= start_key` 的文件，因此结束 key 恰等于请求起点的文件也会被计入；`restore_raw_test.rs::raw_restore_reads_meta_filters_requested_range_and_tracks_go_progress` 明确将此作为 Go 对齐契约验证。

`merge_file_ranges` 先复制并排序文件，不修改 `BackupMeta::Files`。合并后的 `RangeStats::Count` 是合并文件数而不是 KV 数；`Size` 是文件 `Size_` 的饱和和。任一范围或文件具有空终点时，合并结果终点保持为空。

任务还产生三个外部可见状态：`Record(RestoreDataSize, archive_size)` 指标、`CollectInt("restore files", ...)` 摘要计数和 `SetSuccessStatus(true)` 成功标志。进度对象由 `Glue` 持有，可跨恢复回调共享。

## 依赖与调用关系

上游主链为 `br/cmd/br/restore.rs::newRawRestoreCommand`（命令装配）→ `runRestoreRawCommand`（配置与 tracing）→ `RunRestoreRaw`。crate 根 `br/pkg/task/lib.rs` 负责公开模块和 re-export。RustCodeGraph 还识别到测试入口 `restore_lifecycle_test.rs::verify_entry` 直接调用 `RunRestoreRaw`。

本文件的直接内部依赖包括：

- `backup_raw::{RawKvConfig, flag*}`：RawKV 参数、key/CF 状态与基础连接配置。
- `restore::{RestoreCommonConfig, DefineRestoreCommonFlags, defaultRestoreConcurrency}`：通用恢复参数及默认并发。
- `common::{NewMgr, GetKeepalive, ReadBackupMeta}`：连接管理器、keepalive 和元数据读取。
- `stubs::{Glue, Storage, MemStorage, BackupMeta/File/RawRange, RangeStats, Summary, ArchiveSize, SetSuccessStatus}`：跨平台任务契约和内存实现。
- `restore_lifecycle::{RestoreKind, RestoreLifecycle}`：实际 PD、调度器、import mode、importer 和 restorer 生命周期。
- `serde_json` 与标准库 `Arc`：默认元数据序列化和共享 storage/progress 回调。

`br/pkg/task/Cargo.toml` 声明该 crate 为 library，Go 包映射是 `br/pkg/task`；本流程通过 `astersql-br-pkg-restore` 间接执行 SST restorer，并直接使用 workspace 内的 restore/conn/common 等瘦身 crate，而非把 Go 的依赖原样嵌入本文件。

## 错误处理与边界

所有可恢复失败统一通过 `crate::stubs::Result` 和 `?` 向上传播。主要错误边界如下：

- flag 不存在、类型不匹配或 RawKV key 解析失败，会在 `ParseFromFlags` 阶段终止。
- `NewMgr`、`ReadBackupMeta` 或默认元数据 JSON 序列化失败，会在任何导入动作前终止。
- 非 RawKV 元数据返回带 `ErrRestoreModeMismatch` 分类的错误，消息为 `cannot do raw restore from transactional data`。
- 请求范围只被备份范围部分覆盖时返回 `restore range mismatch: restore range is only partially covered`；没有匹配 CF/相交 RawRange 时返回 `restore range mismatch: no backup data in the range`。
- lifecycle 未配置、准备的文件集合与筛选结果不一致、pre-work/import/wait/close 相关执行失败，均由 `GetRestoreLifecycle`、`ValidateFiles` 或 `RestoreFiles` 传播。

资源清理存在明确的控制流边界：`mgr.Close()` 在 manager 创建成功后的所有闭包结果上执行；进度对象在 `RestoreFiles` 返回后关闭，包括导入失败，但若 `GetRestoreLifecycle` 或 `ValidateFiles` 在进度创建后提前报错，当前代码会因 `?` 提前离开闭包而没有显式 `Close()`。文档记录的是当前事实，不将其推断为已具备 RAII 保护。

## 并发与资源生命周期

本文件自身不创建线程、异步任务、锁或通道。共享所有权使用 `Arc<dyn Storage>` 和 `Arc` 进度对象；传给 `RestoreFiles` 的进度闭包带 `move`，持有克隆后的进度句柄，允许下游 worker 安全回调。

实际并发由 `RestoreLifecycle::RestoreFiles` 建立的 worker pool 控制，worker 数使用 `concurrency.max(1)`；本文件的 `adjust` 则尽早把零值替换为 `defaultRestoreConcurrency`。Raw 恢复的 lifecycle 会执行 import pre-work，并在非 online 模式切换 import/normal mode、暂停并恢复调度器；`RestoreSession::Drop` 承担 post-work 或停止 mode refresh。`restore_lifecycle_test.rs::raw_restore_consumes_real_restorer_and_online_mode_lifecycle` 验证了 Raw 路径确实消费准备好的 SST，并覆盖 online/offline 两种模式清理行为。

`RestoreLifecycle::RestoreFiles` 在结束时调用 importer `Close()`，包括 pre-work 或导入失败路径；`RunRestoreRaw` 随后关闭进度并最终关闭 manager。`checkpoint = false` 意味着 Raw 恢复失败时不会采用 snapshot checkpoint 保留调度器暂停状态的特殊语义。

## 与 Go 版本的对应关系

结构上，Rust 的 `RestoreRawConfig`、flag 默认值、解析顺序、配置归一化、Raw/事务模式检查、文件归档大小、范围合并、进度总量、导入完成后成功标记以及 `getEndKeys` 均对应 `br/pkg/task/restore_raw.go`。Rust 独立测试固定了范围过滤、CF 过滤、边界相交、模式错误和部分覆盖错误；lifecycle 测试固定了 real restorer 消费与 mode/scheduler 生命周期。

关键差异必须保留在架构理解中：

- Go `RunRestoreRaw` 接收 `context.Context`，创建可取消 context，并直接初始化 `snapclient.RestoreClient`、连接、crypter、限速和 per-store 并发；Rust 函数没有 context 参数，这些实时对象由 `RestoreLifecycle` 工厂提供。
- Go 会在合并阈值未显式修改时从 TiKV 拉取动态 region 配置；Rust 当前直接使用 `RestoreCommonConfig` 中的值。
- Go 通过配置的外部 storage 读取 backupmeta；Rust 当前仅能使用注入的 `MemStorage`，未注入时会合成内存元数据。
- Go 调用 `LoadSchemaIfNeededAndInitClient` 和 `GetFilesInRawRange`；Rust 在本地 `BackupMeta` 上用 `files_in_raw_range` 实现选择逻辑。
- Go 显式执行 `SplitPoints`，再调用 `RestorePreWork`、`GoRestore` 和 `WaitUntilFinish`；Rust 对 split 阶段只推进进度计数，恢复 pre/post-work 与 restorer 调用集中在 `RestoreLifecycle::RestoreFiles`。
- Go 以 `defer` 关闭 manager/client、取消 context 并执行 post-work；Rust 使用闭包后的显式 `mgr.Close()`、lifecycle 内部 RAII session 和 importer close。两者清理覆盖范围相近但并不完全同构。

因此，本文件应描述为“保留 Go 任务契约并接入 Rust lifecycle 的移植实现”，而不是声明其已单独复现 Go 的全部网络与集群行为。

## 扩展指南

若扩展 RawKV CLI 参数，应同步修改 `DefineRawRestoreFlags`、`RestoreRawConfig::ParseFromFlags`/`adjust`，并在独立文件 `br/pkg/task/restore_raw_test.rs` 增加默认值、正常值和解析错误覆盖；不要把测试内嵌进 `restore_raw.rs`。

若改变范围语义或文件选择，应优先修改 `files_in_raw_range`，明确半开区间、空终点和 `EndKey == StartKey` 的兼容要求，并同步 Go 对照与 `raw_restore_reads_meta_filters_requested_range_and_tracks_go_progress`、`raw_restore_rejects_transactional_meta_and_uncovered_range`。对多个 RawRange、无界范围和同 CF 重叠范围应补独立回归用例。

若改变合并策略，应修改 `merge_file_ranges` 并关注排序稳定性、空终点传播、阈值为零和 `u64` 溢出；同时检查进度总量与 split 行为是否仍一致。`getEndKeys` 同时被 `restore_txn.rs` 使用，改变其公开契约必须评估 txn 恢复影响。

若补齐真实外部 storage、动态 TiKV region 配置或 SplitPoints，应在 `RunRestoreRaw` 与 lifecycle/glue 的职责边界上接线，不应把网络客户端塞回测试 stub；需要同步 `br/cmd/br/restore.rs` 的生产 glue、`restore_lifecycle.rs` 及相应独立测试。特别要验证失败时 progress、manager、importer、mode switcher 与 scheduler 均能恢复。

兼容性风险主要是 Go/Rust 区间边界和错误分类漂移；正确性风险是 lifecycle 准备文件与元数据选择不一致；性能风险来自文件全量复制/排序、过度拆分及并发设置。任何生产行为变更都应继续保留 `ValidateFiles` 这道防丢文件检查。

## 验证依据

- 源文件：`br/pkg/task/restore_raw.rs`，已核对全部 266 行及 `RestoreRawConfig`、`DefineRawRestoreFlags`、`ParseFromFlags`、`adjust`、`getEndKeys`、`files_in_raw_range`、`merge_file_ranges`、`RunRestoreRaw`。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/task` 确认目标及对照文件已索引；`query/node/explore RunRestoreRaw` 确认 Rust/Go 定义、Rust 下游调用及测试调用者。图中 Rust `RunRestoreRaw` 的静态调用包括 `NewMgr`、`GetKeepalive`、`adjust`、`getEndKeys`、`files_in_raw_range`、`merge_file_ranges`、`CollectInt`、`SetSuccessStatus`，直接测试调用者包括 `restore_lifecycle_test.rs::verify_entry`。
- crate/模块证据：`br/pkg/task/Cargo.toml`、`br/pkg/task/lib.rs`。
- 应用入口证据：`br/cmd/br/restore.rs::runRestoreRawCommand`。
- 生命周期证据：`br/pkg/task/restore_lifecycle.rs::{RestoreGlue::GetRestoreLifecycle, RestoreLifecycle::ValidateFiles, RestoreLifecycle::RestoreFiles}` 与 `br/pkg/task/stubs.rs::Glue`。
- Go 对照：`br/pkg/task/restore_raw.go::{RestoreRawConfig, DefineRawRestoreFlags, ParseFromFlags, adjust, RunRestoreRaw, getEndKeys}` 及 `br/cmd/br/restore.go::runRestoreRawCommand`。
- 独立测试：`br/pkg/task/restore_raw_test.rs`、`br/pkg/task/restore_lifecycle_test.rs`，以及公开契约引用 `br/pkg/task/parity_test.rs`。本任务按计划不运行 Cargo；验证限于代码图、源码/配置/测试事实复核和文档结构检查。
