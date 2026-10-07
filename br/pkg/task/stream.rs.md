# `br/pkg/task/stream.rs`

## 文件定位

`stream.rs` 是 `astersql-br-pkg-task` crate 的日志备份（Log Backup）与时间点恢复（PiTR）任务层实现，对照 Go 文件 [`br/pkg/task/stream.go`](./stream.go)。crate 根 [`br/pkg/task/lib.rs`](./lib.rs) 以 `pub mod stream` 挂载本文件并通过 `pub use stream::*` 平铺公开符号；直接的 CLI 上游是 [`br/cmd/br/stream.rs`](../../cmd/br/stream.rs) 中的 `streamCommand`，它解析各子命令旗标、把命令名映射为短名，再调用 `RunStreamCommand`。

本文件不是单纯的 stream 命令门面。它还承担三组工作：读取对象存储中的 backup meta、truncate safepoint 与 checkpoint；为 PiTR 构造重写规则和 scheduler 暂停 key range；在流式恢复前后调整 TiKV/SQL 配置、持久化 checkpoint 元数据并恢复全局配置。Cargo 包名和边界由 [`br/pkg/task/Cargo.toml`](./Cargo.toml) 的 `astersql-br-pkg-task`、`[lib] path = "lib.rs"` 以及 `package.metadata.porting.go-package = "br/pkg/task"` 确认。

当前实现处于分阶段移植状态。文件头和具体函数均明确指出 `StreamMgr` 的 etcd/streamhelper 管理、多个 stream 控制命令和 advancer 仍是可运行桩；`restoreStreamWithTiKVConfigControl` 及其配置恢复路径则已经接入 `conn`、`registry`、`checkpoint`、`restore/log_client` 和 `utils` 等独立 crate。不能把 Go 版本的完整能力等同于 Rust 当前能力。

## 核心职责

1. **命令配置与分发**：`DefineStream*Flags` 注册 start、pause、status、truncate 等旗标；`StreamConfig::ParseStream*FromFlags` 将旗标转成任务配置；`RunStreamCommand` 分发 `start/stop/pause/resume/status/truncate/metadata/advancer`。
2. **日志任务控制**：`NewStreamMgr`、`RunStreamStart` 等提供 stream 生命周期外壳，包括存储构造、起始 TS 校验、进度与成功状态。锁、任务注册、GC safepoint 和实际启停在多处仍未移植。
3. **日志目录解释**：`getLogInfoFromStorage` 根据 `backupmeta`、truncate 文件、resume state 与 per-store global checkpoint 推导 `[logMinTS, logMaxTS]`，`checkLogRange` 校验恢复或截断边界。
4. **PiTR 恢复辅助**：`buildRewriteRules`、`buildKeyRangesFromSchemasReplace`、`ShiftTS`、`isCurrentIdMapSaved` 等处理 ID 映射、scheduler 暂停范围和 TSO 安全偏移。
5. **恢复期配置生命周期**：`restoreStreamWithTiKVConfigControl` 调整 per-store 并发，创建并保证关闭 log client，通过 registry 串行修改 GC ratio 与 RocksDB background jobs，按 checkpoint 状态决定何时恢复配置。

## 主要符号

- 常量 `RESUME_STATE_FILE_NAME`、`LEGACY_RESUME_STATE_FILE_NAME` 指向当前与旧 Rust resume state 路径；`STREAM_SHIFT_DURATION_SECS` 固定为一小时。`FlagStream*` 常量是 CLI 旗标名，其中 `FlagStreamGCTTL` 的实际字符串为 `gc-ttl`。
- `AdvancerCommandConfig` 保存退避、tick、推进阈值、checkpoint 延迟上限和选主周期；`Default` 给出 5 秒、12 秒、4 分钟、48 小时与零周期。
- `StreamConfig` 组合公共 `Config` 与任务名、起止 TSO、safepoint TTL、truncate/status/advancer 参数。`DefaultStreamConfig` 会注册旗标后尝试解析默认值，但公共解析要求非空 `task-name`，该辅助函数忽略解析错误，因此返回值主要是默认结构而不是一个可直接执行的任务。
- `StreamMgr` 目前只持有 `StreamConfig`、`Arc<dyn Storage>` 和 `closed` 标记。`checkLock` 恒为 `Ok(true)`，`setLock` 为空操作，`buildObserveRanges` 返回一个默认 range；只有 `adjustAndCheckStartTS` 的非零校验是真实约束。
- `RunStreamCommand` 是普通 stream 子命令总入口；`RunStreamStart/Stop/Pause/Resume/Status/Truncate/Metadata/Advancer` 是叶子入口。`RunStreamRestore` 不经过该 match，而是由恢复命令链单独调用。
- `BackupLogInfo` 保存 `logMinTS`、`logMaxTS` 与源集群 `clusterID`。`getLogInfoFromStorage`、`getGlobalCheckpointFromStorage`、`getCheckpointFromResumeState` 和 `getMaxRecoverableCheckpointFromStorage` 共同解释存储状态。
- `LogRestoreConfig`、`PiTRTaskInfo`、`RestoreTiKVConfigControl` 是恢复期状态。后者注入 SQL session factory、registry、连接管理器、HTTP client、log client 工厂、checkpoint metadata manager 和 TiFlash 项，避免在任务层凭空构造这些跨 crate 资源。
- `RestoreSQLSessionFactory` 是 SQL/domain 传输边界；`DisableGC` 与 `KeepRocksDBMaxBackgroundJobsLow` 返回可恢复旧值的闭包。`RestoreLogClientGuard` 的 `Drop` 调用 `LogClient::Close`。
- `restoreStream` 先执行 `prepareStreamRestore`，再要求 `RestoreConfig::TiKVConfigControl` 已配置；`restoreStreamWithTiKVConfigControl` 管理环境，`restoreStreamBody` 通过 `Glue::GetRestoreLifecycle` 获取 compacted SST 生命周期并执行 `RestoreCompactedSST`。

本文件没有条件编译项；测试由 `lib.rs` 通过独立文件 `#[cfg(test)] #[path = "stream_test.rs"] mod stream_test` 挂载，符合生产逻辑与测试分文件的仓库约定。

## 执行流程

普通 stream 命令链如下：

1. `br/cmd/br/stream.rs::streamCommand` 先调用公共 `Config::ParseFromFlags`，再按命令调用 `ParseStreamStartFromFlags`、`ParseStreamPauseFromFlags`、`ParseStreamStatusFromFlags`、`ParseStreamTruncateFromFlags` 或公共解析。
2. CLI 将命令名映射后调用 `RunStreamCommand`；未知短名立即返回 `Error::Errorf`。
3. 各叶子入口先调用 `Summary(cmdName)`，执行自身的最低校验或存储操作，成功后调用 `SetSuccessStatus(true)`。
4. `RunStreamStart` 构造 `StreamMgr`、检查非零 `StartTS`、调用空实现的 `setLock`、生成但不提交安全配置，并完成一个单位的进度；因此它尚未等价于 Go 的任务注册与 safepoint 建立。
5. `RunStreamMetadata` 读取并向 `Glue` 记录 `log-min-ts`、`log-max-ts`。`RunStreamTruncate` 校验 until 范围，非 dry-run 时仅把 `UntilTS` 的 8 字节小端表示写入 `TruncateSafePointFileName`；它尚未删除旧日志或管理 Go 侧的 truncating lock。

日志范围计算流程是：解析 `MetaFile` 为 `BackupMeta`；若 `EndVersion > 0` 则拒绝全量备份目录；以 `max(StartVersion, truncateTS)` 为下界；若当前或兼容路径存在 resume state，则其 `LastCheckpoint` 优先，否则遍历 global checkpoint 前缀下所有 `.ts` 文件取最大值；最后令上界至少等于下界。

PiTR 主链从 `RunStreamRestore` 开始：调整 stream restore 配置并创建公共 manager，然后进入 `restoreStream`。`prepareStreamRestore` 当前使用空 `MemStorage`，读取失败时回退到虚拟范围 `1..=100`，并在未指定 `RestoreTS` 时取 100；随后 `restoreStreamWithTiKVConfigControl` 处理真实控制面。该函数先通过 `ProcessTiKVConfigs` 调整并发，再创建受 `RestoreLogClientGuard` 管理的 client；在 registry 的 ID 屏障后关闭 GC、降低 background jobs；启用 checkpoint 时加载或创建元数据并回填旧配置及快照大小；执行恢复闭包后，根据 `RestorePhase` 与 checkpoint 是否已持久化决定保留低配置供重试，还是通过 `GlobalOperationAfterSetResettingStatus` 恢复旧值。

## 数据与状态

- 时间戳使用 `u64` TSO。`ShiftTS` 只将物理毫秒部分回退一小时并保留 logical 部分；物理值不足时返回 0。
- truncate 与 global checkpoint 文件都是至少 8 字节的小端 `u64`。truncate 文件缺失或不足 8 字节在当前实现中被当作 0；global checkpoint 的短文件被忽略。resume state 使用 JSON `PersistentState { LastCheckpoint }`，存在但 JSON 非法会报错。
- `StreamConfig::SafePointTTL` 在 start 解析时非正值归一到 1800 秒，在 pause 解析时归一到 24 小时；`ParseStreamCommonFromFlags` 强制任务名非空。truncate 解析不调用公共解析，因此其任务名不是该路径的前置条件。
- `buildRewriteRules` 以 old table/partition ID 为键，跳过 filtered、系统库与临时系统库；同一 old ID 首次写入后不覆盖。
- `buildKeyRangesFromSchemasReplace` 接收 `[start, end)` 快照 ID 区间，把快照外的新表/分区 ID 排序后合并为 `[min, max+1)`，再用 `EncodeTablePrefix` 与 `EncodeBytes` 转成 TiKV 半开 key range。有效快照存在时虽然调用 `verifyContiguousIDs`，当前结果被丢弃，因此不连续 ID 仍会被更宽区间覆盖。
- `restoreStreamWithTiKVConfigControl` 的关键状态是 `persisted` 与 `restorable`。未启用 checkpoint 时视为已持久化；启用时只有 metadata 成功后才置位。`RestorePhase == 1` 表示当前尚不可恢复全局配置，失败或未完成阶段会保留降低后的参数以便续跑。

## 依赖与调用关系

上游调用边由 RustCodeGraph 文件查询与源码核对确认：`br/cmd/br/stream.rs::streamCommand -> RunStreamCommand`；crate 内 `RunStreamCommand -> RunStreamStart/Stop/Pause/Resume/Status/Truncate/Metadata/Advancer`；`RunStreamRestore -> restoreStream -> restoreStreamWithTiKVConfigControl -> restoreStreamBody`。RustCodeGraph 还报告目标文件被 `br/pkg/task/parity_test.rs`、`restore_lifecycle_test.rs`、`restore_test.rs` 引用，而专用行为测试在 `stream_test.rs` 中由 `lib.rs` 条件挂载。

下游依赖分为两层：

- 同 crate 的 `common` 提供 `Config`、旗标定义、`GetStorage`、`NewMgr`，`restore` 提供 `RestoreConfig` 与 stream restore 判定，`stubs` 提供存储、Glue、backup protobuf 替身、编码和错误类型。
- Cargo 显式依赖 `astersql-br-pkg-conn`、`registry`、`checkpoint`、`restore-log-client` 与 `utils`，分别用于 TiKV 配置、恢复任务协调、checkpoint 元数据、日志恢复 client 和 SQL 配置读写；此外还声明 `restore`、`gc`、`stream` 等 BR crate。本文件直接使用其中前述五个真实边界。

外部存储通过 `Arc<dyn Storage>` 共享；SQL session、registry、manager、HTTP client、metadata manager 和 client 工厂通过 `RestoreTiKVConfigControl` 注入。这个依赖方向让测试可以替换控制面，但也意味着 `restoreStream` 在没有 `TiKVConfigControl` 时会明确失败。

## 错误处理与边界

- 旗标读取、存储构造、文件读写、JSON 解析、SQL 配置与 registry 操作普遍使用 `Result` 和 `?` 向上传播；无任务名、无 start TS、无 PD、无 until TS、恢复范围越界分别返回带 `ErrInvalidArgument` 的错误。
- `checkLogRange` 接受边界相等和单点区间，拒绝 `restoreFrom < logMin`、`restoreFrom > restoreTo` 或 `restoreTo > logMax`。
- `getLogInfoFromStorage` 的 `checkRequirements` 当前未使用，不执行 Go 的 backup meta 兼容性检查；`getFullBackupTS` 同样只解析 JSON，未实现解密与版本兼容校验。这是兼容性缺口，不应被文档描述为已支持。
- `prepareStreamRestore` 使用 `unwrap_or` 吞掉空内存存储的所有元数据错误并代入 `1..=100`，会掩盖真实存储错误；这是当前桩行为，不是生产级错误策略。
- `cleanUpWithRetErr` 只保留首个错误，而 Go 用 `multierr.Combine` 合并清理错误；`restoreStreamWithTiKVConfigControl` 则特意复现 Go 命名返回值 defer 的效果：最终全局配置恢复结果可能覆盖先前 operation/metadata 错误。
- `PiTRTaskInfo::hasTiFlashItemsInCheckpoint` 恒为 false，`getRestoreStartTS` 直接返回 `RestoreTS`，`RegisterRestoreIfNeeded` 为空操作；相关 Go 逻辑尚未移植。
- `oldGC` 以 `-` 开头时恢复为默认 GC ratio；background jobs 原值为空时不写新值，也返回空操作恢复闭包，避免对不支持该配置的目标执行 SQL。

## 并发与资源生命周期

`StreamMgr` 的 `close` 仅设置布尔值，没有 RAII，也没有真实 etcd/streamhelper 资源；`RunStreamStart` 显式调用它，但中途报错不会触发 close。Go 版本的锁、连接和 safepoint 生命周期因此不能套用到当前 Rust 实现。

真实恢复控制面使用多层生命周期保护：`Arc` 共享存储与注入对象；`RestoreLogClientGuard::Drop` 保证在正常、恢复失败和配置建立失败路径上关闭已创建的 log client；进度对象在 `restoreStreamBody` 调用恢复后显式 `Close`。`Registry::OperationAfterWaitIDs` 先等待注册 ID 屏障再修改集群配置，`GlobalOperationAfterSetResettingStatus` 在恢复配置时建立全局 resetting 状态，避免多个恢复任务无协调地改写全局参数。

checkpoint 模式下，失败或 `RestorePhase == 1` 时保留 GC/background-jobs 调整是刻意的重试语义；其余路径恢复旧值。测试中的 mutex/原子计数验证了 SQL 状态变化与 client 恰好关闭一次，但本文件本身不创建线程、异步任务或 channel。`AdvancerCommandConfig` 虽描述周期，`runOwnershipCycle` 当前不启动循环。

## 与 Go 版本的对应关系

名称和基本数据流主要一一对应 `stream.go`：旗标与解析、`StreamConfig`、命令分发、日志区间、checkpoint 优先级、rewrite/key-range、`ShiftTS`、GC/background-jobs 配置和恢复清理都能找到同名 Go 符号。独立的 [`stream_test.rs`](./stream_test.rs) 对照 [`stream_test.go`](./stream_test.go)，覆盖旗标默认值、安全配置、TS 偏移、范围校验、checkpoint、日志目录、key range 和配置生命周期。

重要差异如下：

- Go `streamMgr` 创建 etcd、PD、streamhelper 客户端并管理锁、safepoint、schema 与 observe range；Rust `StreamMgr` 是存储加状态位的桩。
- Go 的 start/stop/pause/resume/status/truncate/advancer 会修改真实任务、PD 与对象存储；Rust 除元数据读取、truncate safepoint 写入和基础校验外，大部分只设置成功状态。
- Go `restoreStream` 包含完整的日志迭代、ID map、schema reload、scheduler pause、导入与统计；Rust 的 `restoreStreamBody` 目前只委托 `RestoreLifecycle::RestoreCompactedSST`，而准备阶段还使用虚拟 `MemStorage` 范围。
- Go 从 protobuf 解析并检查/解密 backup meta；Rust 当前使用 JSON stubs。Go 只读取当前 resume state 路径；Rust额外兼容早期端口的 legacy 路径。
- Go `isCurrentIdMapSaved` 判断精确阶段相等，Rust使用 `>=`；Go 的清理合并错误，Rust只保留首错。后续对齐时应先确认这些差异是否有意，不能机械复制。

## 扩展指南

- 补全普通 stream 控制命令时，应从 `StreamMgr::{checkLock,setLock,buildObserveRanges,close}` 和各 `RunStream*` 叶子入口接线；同步扩展独立的 `stream_test.rs`，若涉及 CLI 解析还要覆盖 `br/cmd/br/stream_test.rs`。不要把测试写回生产文件。
- 补全存储兼容性时，应修改 `getLogInfoFromStorage`、`getFullBackupTS` 和 `prepareStreamRestore`，替换 JSON/`MemStorage` 桩，并测试损坏 meta、加密 meta、短 TS 文件、兼容性开关与真实 storage 错误。需要维持 resume state 优先于 global checkpoint 的既有不变量。
- 扩展 PiTR 恢复时，应沿 `RunStreamRestore -> restoreStream -> restoreStreamWithTiKVConfigControl -> restoreStreamBody` 接入；配置调整必须在 checkpoint 元数据持久化、重试保留和最终恢复之间保持现有顺序，并继续用 guard 保证 client 关闭。
- 修改 scheduler pause 范围时，应同时审查 `buildKeyRangesFromSchemasReplace`、`isValidSnapshotRange`、`verifyContiguousIDs` 与 Go 的 `LogRestoreConfig.tableMappingManager/metadata` 取值逻辑。`maxID + 1` 还需防止 `i64::MAX` 溢出，当前代码未处理。
- 新增跨 crate 依赖前先确认 `br/pkg/task/Cargo.toml` 的 arm64 限制注释；外部 Rust 依赖必须按仓库规则在上游发布 tag，不得本地 patch/vendor。
- 所有 Rust 行为修复都应保留文件顶部两段版权，并在同目录独立测试文件增加回归；完成后按仓库要求先 `cargo fmt --all`，但本次纯文档任务不运行 Cargo。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/task` 确认 `stream.rs`、`stream_test.rs`、Go 对照与 crate 入口均已索引；`node --file br/pkg/task/stream.rs --offset 1 --limit 1200` 读取了目标 1043 行及其三处文件级使用关系；`query` 分别确认 Rust/Go 的 `RunStreamCommand`、`getLogInfoFromStorage` 和 Rust 的 `restoreStreamWithTiKVConfigControl` 定义。精确 `callers/callees` 查询在本地超时，因此调用边由已索引的 `node` 源码和精确引用搜索交叉核对，没有据此虚构图边。
- 读过的 Rust 证据：[`br/pkg/task/stream.rs`](./stream.rs)、[`br/pkg/task/stream_test.rs`](./stream_test.rs)、[`br/pkg/task/lib.rs`](./lib.rs)、[`br/cmd/br/stream.rs`](../../cmd/br/stream.rs)。
- 读过的边界与对照证据：[`br/pkg/task/Cargo.toml`](./Cargo.toml)、[`br/pkg/task/stream.go`](./stream.go)、[`br/pkg/task/stream_test.go`](./stream_test.go)。最近的目标包中没有 `doc.go`；包级 Rust 契约由 `lib.rs` 的模块文档提供。
- 现有 Rust 测试覆盖 `stream_flag_defaults_and_parsing_match_go`、`generate_security_config_requires_effective_non_empty_key_material`、`test_shift_ts`、`test_check_log_range`、global/resume checkpoint、全量/日志目录、key range，以及 `stream_tikv_config_*`、client close 和 restore target TS 初始化。按任务约束未运行 Cargo，这些测试仅作为行为证据读取。
- 交付结构验证使用任务指定命令，要求目标文档存在且恰有本文的 11 个固定二级标题；此外人工复核本文明确回答文件为何存在、当前如何运行、哪些路径是桩以及应从何处安全扩展。
