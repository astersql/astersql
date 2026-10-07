# `br/pkg/task/restore.rs`

## 文件定位

`restore.rs` 是 `astersql-br-pkg-task` crate 的恢复任务总控模块。crate 根 `br/pkg/task/lib.rs` 以 `#[path = "restore.rs"] pub mod restore` 挂载它，并通过 `pub use restore::*` 将公开符号平铺给 `br/cmd` 等调用方；`br/pkg/task/Cargo.toml` 将该 crate 标为对应 Go 包 `br/pkg/task` 的 library port，并依赖 `astersql-br-pkg-stream`、`astersql-br-pkg-restore`、`astersql-br-pkg-checkpoint`、`astersql-br-pkg-conn` 等瘦 BR crate。

文件对应 Go 实现 `br/pkg/task/restore.go`，但不是完整等价替换：配置、flag、校验、DDL 过滤、PiTR 表追踪和一个可执行的快照恢复最小主链已经存在；Go 版本中的 etcd/PD 任务注册、冲突检测、Domain、真实 TiKV/TiFlash 查询、完整 checkpoint/PiTR 编排及 abort 清理仍未全部移植。源码模块注释也明确说明“实际拉数/导入在其它模块”，因此本文件应理解为恢复配置与公共策略的归属点，加上当前瘦运行环境中的任务编排入口，而不是整个 BR 恢复子系统的唯一实现。

直接上游是 `br/cmd/br/restore.rs::runRestoreCommand`：解析 `RestoreConfig`，point restore 再解析 stream flags，最后调用 `RunRestore`；中止入口是 `br/cmd/br/abort.rs::runAbortRestoreCommand`，最终调用 `RunRestoreAbort`。独立 Rust 测试由 `br/pkg/task/lib.rs` 在 `cfg(test)` 下挂载 `restore_test.rs`、`restore_nokit_test.rs`、`config_test.rs` 和 `restore_lifecycle_test.rs`。

## 核心职责

1. 定义恢复命令名、CLI flag 名及默认值，并由 `DefineRestoreCommonFlags`、`DefineRestoreFlags`、`DefineStreamRestoreFlags` 注册到本地 `FlagSet`。
2. 用 `RestoreCommonConfig` 与 `RestoreConfig` 承载公共恢复配置、快照/PiTR 参数、EBS 参数、checkpoint 管理器、恢复存储和表追踪状态；通过 `ParseFromFlags`、`ParseStreamRestoreFlags`、`Adjust` 形成可执行配置。
3. 用 `Hash` 对影响恢复兼容性的不可变配置生成 SHA-256 checkpoint 身份；存储 URL 先经 `redact_storage_url` 脱敏，凭据轮换不会改变哈希。
4. 提供恢复前校验与变换：备份库表存在性、collation 一致性、TiFlash 副本、聚簇索引、磁盘空间、DDL blocklist、预分配 key range。
5. 根据日志备份表历史调整 PiTR 的恢复表集合，建立 `PiTRIdTracker`，并拒绝只恢复 partition exchange 一侧的歧义操作。
6. `RunRestore` 执行当前 Rust 已接线的快照恢复主链：调整配置、创建 manager、读取 backup meta、拒绝 raw/txn 元数据、记录归档大小、校验 lifecycle、恢复文件、关闭资源并设置成功状态。

## 主要符号

- `RestoreCommonConfig`：在线恢复、粒度、每 store 并发、小 Region 合并阈值、系统表与系统用户配置。`adjust` 只在对应 `ModifiedU64.Modified` 为假时回填阈值/并发，避免覆盖显式输入。
- `RestoreConfig`：组合 `common::Config` 与恢复专用字段。除 CLI 配置外，还保存 `PiTRTableTracker`、`RestoreStorage`、`CheckpointMetaManagers`、`snapshotRestoreDataSize`、`RestoreStartTS`、`RestoreID` 和 `TiKVConfigControl` 等运行态数据。
- `RestoreConfig::Hash`：序列化命令名、上游集群 ID、脱敏后的 storage、filter、系统表、快速系统表和统计信息开关，再计算 SHA-256。当前 Rust 与 Go 一样未把 `ExplicitFilter` 放入序列化值。
- `RestoreConfig::ParseFromFlags`：解析普通恢复 flag，并强制 `SplitRegionIndexStep > 0`、`RestorePhase` 只能为 1 或 2、分阶段恢复必须启用 checkpoint。
- `RestoreConfig::ParseStreamRestoreFlags`：解析起止 TS、全量备份地址和 PiTR batch 参数；`start-ts` 与 `full-backup-storage` 互斥。
- `CheckpointMetaManager` / `CheckpointMetaManagers`：最小关闭接口与 manager 集合。`CloseCheckpointMetaManager` 通过 `drain(..)` 实现一次性关闭，重复调用不会再次关闭旧对象。
- `RestoreClientConfig` / `configureRestoreClient`：把 region scan、split step、coarse scatter、DDL batch size 投影到 snapshot client 的最小 setter 面；它不包含 Go `configureRestoreClient` 的全部 setter。
- `VerifyDBAndTableInBackup`：大小写不敏感地检查显式库表选择，并把 `__TiDB_BR_Temporary_` 系统库备份名还原成逻辑库名。
- `EstimateTikvUsage`、`EstimateTiflashUsage`、`CheckStoreSpace`：每 store 空间估算与可用空间校验；store 数为零时估算返回零，TiKV 副本数会钳制到 store 数。
- `Job`、`FilterDDLJobs`、`DDLJobFilterRule`、`CheckDDLJobByRules`、`FilterDDLJobByRules`：本地最小 DDL job 模型及两类规则语义；`Check*` 命中即报错，`Filter*` 命中即丢弃。
- `PiTRIdTracker`：分别追踪 DB ID、表 ID 到 DB ID 的多值关系、partition ID、库表名；`AdjustTablesToRestoreAndCreateTableTracker` 是其主要构建者。
- `PreCheckTableTiFlashReplica`：NextGen 场景无条件移除 TiFlash 副本；普通场景清空 available 状态，存在 recorder 时记录后移除，副本数超过 TiFlash store 数时也移除。
- `PreCheckTableClusterIndex`：对快照表和 create-table DDL 中的表检查 `IsCommonHandle`，与已存在表不一致时报 restore mode mismatch。
- `RunRestore` / `RunRestoreAbort`：当前 Rust 的执行入口。前者包含最小快照文件恢复链，后者目前只做配置调整、summary、checkpoint manager 关闭和成功标记，远小于 Go abort 的任务定位与清理流程。

## 执行流程

普通 `full/db/table` 和 point restore 的配置路径如下：

1. `br/cmd/br/restore.rs::runRestoreCommand` 创建默认 `RestoreConfig`，调用 `ParseFromFlags`；point restore 由 `IsStreamRestore` 判定后追加 `ParseStreamRestoreFlags`。
2. `ParseFromFlags` 先读恢复专用值和 `RestoreCommonConfig`，按 `skipCommonConfig` 决定是否解析嵌入的通用 `Config`，再验证 split step、phase 与 checkpoint 组合。
3. CLI 完成 EBS 分流、全局配置和 schema filter 准备后调用 `RunRestore`。
4. `RunRestore` 调用 `Adjust` 回填未配置默认值，登记 `Summary(cmdName)`，并通过 `NewMgr` 建立管理连接。存储优先使用测试/注入的 `RestoreStorage`，否则调用 `common::GetStorage`。
5. `ReadBackupMeta` 读取元数据；`IsRawKv` 或 `IsTxnKv` 会立即返回 restore mode mismatch。随后用 `ArchiveSize` 计算并记录 `snapshotRestoreDataSize` 与 glue 指标。
6. 当前空间检查使用 `EstimateTikvUsage(archive, 3, 3)` 和合成的“需要量 + 1”可用空间，它验证 helper 调用链，但并未像 Go 版本那样查询真实 PD store 与副本配置。
7. 元数据含文件时，从 glue 获取 snapshot `RestoreLifecycle`，先 `ValidateFiles`。全量且无显式 filter，或增量备份时清空预分配 key ranges；部分快照若没有 key ranges 则报错。
8. 创建进度对象，把 progress callback 交给 `RestoreFiles`；无论恢复结果如何先 `progress.Close()`，再传播恢复错误。
9. 成功后 `SetSuccessStatus(true)`。闭包退出后总是先 `mgr.Close()`、再 drain 并关闭 checkpoint managers，最后返回原始结果。

PiTR 表集合调整由 `AdjustTablesToRestoreAndCreateTableTracker` 独立完成：先匹配新建库；对每个表的 start/end location 判断最终位置是否命中过滤器；对跨库 rename 按 end 命中情况补入或移除快照表；把最终 `table_map` 全量写入 tracker；最后检查 partition exchange 两侧的恢复决策必须一致。该 helper 在本文件中没有被 `RunRestore` 调用，属于供更完整 PiTR 编排接线的公开策略函数。

## 数据与状态

`RestoreConfig` 同时包含持久配置和单次任务状态。配置类字段包括并发、batch、过滤器、storage、checkpoint、phase、TiFlash 等；状态类字段包括 `RestoreStorage`、`CheckpointMetaManagers`、`PiTRTableTracker`、`snapshotRestoreDataSize` 和若干恢复 ID/时间戳。扩展时应避免把仅在进程内有效的对象加入 `Hash`，也不要漏掉会改变 checkpoint 可复用语义的不可变配置。

`PiTRIdTracker.table_ids` 的方向是 `table_id -> {db_id}`，允许同一物理表 ID 在历史中关联多个 DB；`ContainsDBAndTableId` 因此同时验证两级身份。`partition_ids` 独立存放物理 partition ID。`table_names` 当前被写入但没有公开查询方法，属于后续映射流程的预留状态。

`FilterDDLJobs` 会原地按 `SchemaVersion` 降序排列输入 slice，然后分别沿数据库和表的历史 ID/名字扩展闭包。一个 job 可同时被数据库循环和表循环加入结果，因此函数不承诺去重；调用者若需要集合语义必须额外处理。`Hash` 对 filter 字符串顺序敏感；URL 解析失败时 `redact_storage_url` 原样返回输入，意味着非标准 URL 中的敏感信息不会被该 helper 脱敏。

## 依赖与调用关系

上游调用关系以源码为准：

- `br/cmd/br/restore.rs::runRestoreCommand -> RestoreConfig::ParseFromFlags -> ParseStreamRestoreFlags（仅 point） -> RunRestore`。
- `br/cmd/br/abort.rs::runAbortRestoreCommand -> RestoreConfig::ParseFromFlags -> ParseStreamRestoreFlags（仅 point） -> RunRestoreAbort`。
- `br/pkg/task/stream.rs` 引用 `DefineStreamRestoreFlags`、`IsStreamRestore`、`RestoreConfig`，用于 stream 任务配置/parity 接线。
- `br/pkg/task/config_test.rs` 直接验证 `configureRestoreClient`、`VerifyDBAndTableInBackup` 和默认配置；`restore_test.rs`、`restore_nokit_test.rs`、`restore_lifecycle_test.rs` 直接驱动其余生产 helper 与 `RunRestore`。

主要下游依赖是 `common::{Config, NewMgr, ReadBackupMeta, GetKeepalive}`、`stubs::{Glue, Storage, FlagSet, Error, Summary}`、`restore_lifecycle::RestoreKind` 和 `astersql_br_pkg_stream::table_history`。标准库 `HashMap/HashSet` 承载追踪集合，`Arc` 共享 storage、manager 与进度回调；`serde_json + sha2` 生成配置哈希，`url` 负责按 scheme 脱敏查询参数。

RustCodeGraph 对该文件给出的文件级 used-by 是 `br/pkg/task/restore_test.rs` 与一个测试文件；对 `RunRestore` 的符号查询同时定位到 Rust/Go 定义和 CLI 恢复入口。由于图数据库的 `callers/callees` 命令在本次查询中超时，直接调用边进一步由已索引的 `br/cmd/br/restore.rs`、`br/cmd/br/abort.rs` node 输出及仓库精确符号搜索核验。

## 错误处理与边界

- flag getter、TS 解析、manager/storage/meta/lifecycle 操作均使用 `Result` 和 `?` 传播；配置约束使用 `Error::Annotatef(berrors::ErrInvalidArgument, ...)`，恢复模式冲突使用 `ErrRestoreModeMismatch`。
- `ParseStreamRestoreFlags` 禁止同时指定正数 `StartTS` 与非空 `FullBackupStorage`；`ParseFromFlags` 禁止 split step 为零、非法 phase、以及不启用 checkpoint 的分阶段恢复。
- `VerifyDBAndTableInBackup` 在未显式指定库表时直接成功；指定项不存在时返回带原始选择名的错误。它只验证名字存在，不验证表元数据内容。
- `EstimateTikvUsage` 与 `EstimateTiflashUsage` 在 store 数为零时返回零，以避免除零；乘法未做溢出保护，输入来自不可信或极大元数据时应评估 `u64` 溢出风险。
- `CheckStoreSpace` 把零或负可用量视为 PD invalid response，把不足视为 KV disk full。Rust 参数已是解析后的 `i64` 字节数，不覆盖 Go 版本对 PD 字符串容量的解析错误。
- `AdjustTablesToRestoreAndCreateTableTracker` 直接索引 `locations[0]`、`locations[1]`，隐含历史记录必须恰有可用 start/end 项；上游构造器必须维护该不变量，否则会 panic。
- `RunRestore` 在 raw/txn meta、文件验证、缺少 partial key ranges 或恢复文件失败时返回错误；成功状态仅在闭包末尾设置。manager 与 checkpoint manager 的关闭发生在返回前，但 `Close` 接口不返回错误，关闭失败无法向上传播。
- `RunRestoreAbort` 当前没有读取 registry、解析 restored TS 或删除暂停任务；不能把“返回成功”解释为已完成 Go 版本的远端 abort 语义。

## 并发与资源生命周期

本文件自身不创建线程或 Tokio task。并发量作为配置传给下游：`Config.Concurrency` 控制 `RestoreFiles`，另有每 store、PD、region scan、stats、PiTR 和 cloud API 并发字段。`adjustRestoreConfigForStreamRestore` 在默认 PiTR 并发上再加 1，与 Go 的额外管线槽位约定一致。

共享资源使用 `Arc`：`RestoreStorage` 的内存实现被提升为 `Arc<dyn Storage>`，checkpoint manager 保存为 `Arc<dyn CheckpointMetaManager>`，进度 reporter clone 后被 move 进回调。`RunRestore` 同步等待 `RestoreFiles` 返回，再关闭 progress；随后无论闭包成功失败都关闭 manager 并 drain checkpoint managers。此顺序保证恢复回调不再使用 progress 后才关闭它，也保证同一 checkpoint manager 不会被 `CloseCheckpointMetaManager` 重复处理。

`PiTRIdTracker` 和表/库 map 由 `&mut` 独占修改，没有内部锁；并行接入时必须在外层串行构建或提供同步。`tweakLocalConfForRestore` 当前仅返回捕获常量的 no-op 闭包，不具备 Go 版本修改并恢复全局 TiDB 配置的真实生命周期语义。

## 与 Go 版本的对应关系

对应文件为 `br/pkg/task/restore.go`。以下部分基本保持同名和主要控制流：恢复命令名及大部分默认 flag、`RestoreCommonConfig.adjust`、配置解析与默认回填、不可变配置哈希、库表存在性检查、空间估算、DDL job 血缘过滤与 blocklist、collation 检查、TiFlash/聚簇索引预检、PiTR rename/partition exchange 追踪。

重要差异必须在维护时显式考虑：

- Go `RestoreConfig` 嵌入真实 TiDB、PD、checkpoint、registry、table mapping 等对象；Rust 使用本地 stubs、简化模型和部分可选字段，字段集合并非一一等价。
- Go `configureRestoreClient` 设置更多 client 行为（no-schema、事务大小、placement、系统表、rewrite mode、collation 等）；Rust trait 目前只有四个 setter。
- Go `CheckNewCollationEnable` 创建 session、读取下游全局变量并更新 collate 全局状态；Rust 接收两个字符串，只保留比较/兼容决策。
- Go 空间预检从 PD 获取 store 与副本数据；Rust `RunRestore` 使用合成参数，只有独立估算函数保持公式语义。
- Go `RunRestore` 包含 etcd 连接、任务冲突/注册、全局配置、snapshot/PiTR 分支、blocklist、checkpoint、真实 client 和后处理；Rust 当前只接线最小 snapshot lifecycle。
- Go `RunRestoreAbort` 会定位并清理 registry 中的暂停任务；Rust 当前实现是占位式收尾。
- Rust `tweakLocalConfForRestore` 是 no-op；Go 会临时放宽最大索引长度、索引数和列数，并返回恢复函数。

因此新增行为应先判断是在补齐 Go 语义，还是在扩展 Rust 特有瘦运行环境；不能以现有 stub 的“可返回成功”代替 Go 行为已经移植完成。

## 扩展指南

- 新增 CLI 恢复参数时，同步修改 flag 常量、`DefineRestore*Flags`、`RestoreConfig`、解析函数、`Adjust`（如有默认回填）以及 `br/cmd/br/restore.rs`/`abort.rs` 的适用入口；若影响 checkpoint 兼容性，再审查 `Hash` 输入。
- 扩展 snapshot client 配置时，先增加 `RestoreClientConfig` setter 和 `configureRestoreClient` 调用，再在 `br/pkg/task/config_test.rs` 的 mock 中记录并断言；应逐项对照 Go `configureRestoreClient`，不要一次性引入无直接依赖的完整子系统。
- 调整 DDL 过滤时，同时维护 action 常量、blocklist、`CheckDDLJobByRules`/`FilterDDLJobByRules` 语义和 `br/pkg/task/restore_test.rs`；规则应保持“检查报错”与“过滤丢弃”的区别。
- 修改 PiTR 表历史算法时，重点保护跨库 rename、truncate 后 ID 变化、partition exchange 双侧一致性，并在独立 `restore_test.rs` 增加回归；不要把测试内嵌进生产文件。
- 完善 `RunRestore` 时优先在已有 `restore_lifecycle`、`common` 和专用 restore crate 上接线。资源获取后立即明确关闭顺序，并为每个错误分支验证 manager、progress、checkpoint manager 的回收。
- 补齐 abort 时不能复用当前成功桩作为完成标准；至少要对照 Go 的 operation context、storage/log info、registry 查询和暂停任务删除，并新增独立测试证明远端状态发生预期变化。
- 所有 Rust 行为变更应保持文件顶部 AsterSQL/PingCAP 版权注释；完成后运行 `cargo fmt --all`。本说明任务本身不修改 Rust，也按计划不运行 Cargo。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`node --file br/pkg/task/restore.rs --offset 1 --limit 500` 与 `--offset 500 --limit 850` 读取目标文件全部 1282 行；`query RunRestore --kind function --json` 定位 Rust `restore.rs::RunRestore`、Go 对照及 CLI 候选；`node` 读取 `br/cmd/br/restore.rs`、`br/cmd/br/abort.rs`、`br/pkg/task/restore_test.rs`、`config_test.rs`、`restore_nokit_test.rs`。`callers/callees` 查询尝试超时，未把缺失图输出当作证据。
- crate 与模块边界：`br/pkg/task/Cargo.toml`、`br/pkg/task/lib.rs`；目标包无 `doc.go`。
- Go 对照：`br/pkg/task/restore.go`，重点核对 `RestoreCommonConfig`、`RestoreConfig`、flag/parse/adjust、`RunRestore`、空间估算、PiTR tracker、TiFlash/clustered-index 预检、DDL 过滤与 `RunRestoreAbort`。
- Rust 独立测试：`br/pkg/task/restore_test.rs` 覆盖默认值、stream flags、checkpoint 关闭、TiFlash、clustered index、collation、DDL、空间、PiTR tracker、Hash 和快照归档大小；`br/pkg/task/config_test.rs` 覆盖 client 配置与库表校验；`br/pkg/task/restore_nokit_test.rs` 覆盖 raw meta 拒绝和 key range；`br/pkg/task/restore_lifecycle_test.rs` 覆盖 lifecycle 调用链。
- 直接调用证据：`br/cmd/br/restore.rs:31-132` 与 `br/cmd/br/abort.rs:135-170`；仓库精确符号搜索还确认 `br/pkg/task/stream.rs` 和上述独立测试的引用。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验证命令及退出码在任务交付时记录。
