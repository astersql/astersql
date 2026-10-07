# `br/pkg/task/restore_data.rs`

## 文件定位

`restore_data.rs` 属于 `astersql-br-pkg-task` crate（见 `br/pkg/task/Cargo.toml`），由 crate 根 `br/pkg/task/lib.rs` 以 `pub mod restore_data` 挂载并通过 `pub use restore_data::*` 平铺导出。它对应 Go 文件 `br/pkg/task/restore_data.go`，承接 `br restore` 的 AWS EBS 快照恢复中“解析并恢复 KV 数据”这一任务入口。

Rust CLI 的直接入口是 `br/cmd/br/restore.rs::runRestoreCommand`：当 `RestoreConfig.FullBackupType` 为 `aws-ebs` 且 `Prepare == false` 时调用 `RunResolveKvData`。不过该 CLI 当前传入新建的空 `MemStorage`，并未像 Go 版本那样根据 `cfg.Config.Storage` 打开外部存储；因此真实命令会先在读取 `backupmeta.json` 时失败。当前文件应视为迁移期的任务编排桩，而不是完整可用的 EBS 数据恢复实现。

## 核心职责

本文件目前承担两项职责：

1. `ReadBackupMetaData` 从抽象 `Storage` 读取并反序列化 `backupmeta.json`，只接受显式标记为 `aws-ebs` 的备份，并返回恢复所需的 `resolved_ts` 与 TiKV 副本数。
2. `RunResolveKvData` 调整恢复配置、记录 resolve TS、构造并可靠关闭 `Mgr`、按 `副本数 * 3 + 2` 建立并完成进度条，最后标记 summary 成功。

它没有实现 Go 同名入口中的实际数据恢复链：没有获取恢复时间戳、注册 GC safe point、移除/恢复 PD scheduler、枚举当前 TiKV store、调用 `data.RecoverData`、执行 `UnmarkRecovering`，也没有调用 `resetTiFlashReplicas`。源码第 20 行和第 107—109 行也明确说明真实 TiKV resolve 仍为桩。

## 主要符号

- `ClusterInfo`：私有、仅反序列化使用的结构，读取嵌套 `cluster_info.full_backup_type` 和 `cluster_info.resolved_ts`；两个字段均有 `#[serde(default)]`。
- `TiKVMeta`：私有结构，只读取 `tikv.replicas: i32`，缺失时默认为 `0`。
- `BackupMeta`：私有 JSON 投影。优先承载嵌套 `cluster_info`，同时保留根级 `full_backup_type`、`resolved_ts` 以兼容较旧 operator 产物，并可选读取 `tikv`。
- `pub fn ReadBackupMetaData(storage: &dyn Storage) -> Result<(u64, i32)>`：公开元数据解析入口；成功值依次为 resolve TS 与副本数。
- `pub fn RunResolveKvData(g: &dyn Glue, cmdName: &str, cfg: &mut RestoreConfig, storage: Arc<dyn Storage>) -> Result<()>`：公开编排入口。
- `MgrCloseGuard(Arc<dyn Mgr>)`：定义在 `RunResolveKvData` 内的局部 RAII guard；其 `Drop` 无条件调用 `Mgr::Close`，保证构造成功后的正常返回和后续错误路径都会关闭管理器。

文件没有常量、trait、条件编译项或异步函数。公开 API 由 `lib.rs` 重新导出，测试则通过独立的 `restore_data_test.rs` 和 `parity_test.rs` 挂载。

## 执行流程

`ReadBackupMetaData` 的流程如下：

1. 调用 `Storage::ReadFile("backupmeta.json")`；存储错误直接通过 `?` 返回。
2. 用 `serde_json::from_slice` 解码为私有 `BackupMeta`；JSON 错误被转换为本 crate 的 `Error`，保留 `to_string()` 文本。
3. 若存在 `cluster_info`，使用其中的备份类型和 resolve TS；只有 `cluster_info == None` 时才回退到根级兼容字段。嵌套对象存在但字段缺失时不会再回退根级字段。
4. 检查备份类型是否严格等于 `common::FullBackupTypeEBS`（当前值为 `aws-ebs`）；否则返回 `invalid meta file, only support aws-ebs now`。
5. 从 `tikv.replicas` 取副本数；整个 `tikv` 缺失时返回 `0`，成功返回 `(resolved_ts, replicas)`。

`RunResolveKvData` 的流程如下：

1. `cfg.Adjust()` 回填通用恢复配置默认值，例如恢复并发度、切换模式间隔、PD/统计/云 API 并发度。
2. 调用 `Summary(cmdName)`。当前 Rust 桩中的 `Summary` 不输出内容，且这里不是 Go 版本的 `defer summary.Summary(cmdName)`，所以其时序语义尚未对齐。
3. 调用 `ReadBackupMetaData`；失败时立即返回，后续资源尚未创建。
4. 以键 `resolve-ts` 调用 `CollectInt`，将 `u64` 通过 `as i64` 转换后记录在线程局部 summary 中。
5. 用 keyspace、PD 地址、TLS、keepalive、需求检查开关和 `NormalVersionChecker` 调用 `common::NewMgr`。当前 `NewMgr` 校验 PD 非空和 TLS 配置后返回 `MemMgr` 桩。
6. 立即把 `Mgr` 放入 `MgrCloseGuard`，建立退出时关闭的不变量。
7. 以 `i64::from(numStores).saturating_mul(3).saturating_add(2)` 计算进度总量；调用 `Glue::StartProgress`，一次性 `IncBy` 到总量后 `Close`。
8. `SetSuccessStatus(true)` 后返回 `Ok(())`，离开作用域时 guard 调用 `Mgr::Close`。

## 数据与状态

输入状态由四部分组成：调用者提供的 `Glue`、命令名、可变 `RestoreConfig` 和共享所有权的 `Arc<dyn Storage>`。`cfg.Adjust()` 会原地修改配置零值；其余输入只借用。`Arc` 仅用于跨边界共享 storage，本函数自身不克隆或派生后台任务。

元数据只建模恢复入口当前使用的最小字段集，未知 JSON 字段会被 Serde 忽略。`#[serde(default)]` 使缺失的字符串、整数和可选对象分别落为 `""`、`0`、`None`。关键不变量是备份类型必须显式为 EBS；注释特别强调不能因为字段缺失而默认成 EBS。

进度公式沿用 Go 注释中的五类工作单元：每个 store 对应“读元数据、发送恢复、遍历 TiKV”三份工作，再加 prepare-flashback 与 flashback 两份固定工作。但 Rust 当前没有执行这些工作，只将元数据里的 `replicas` 当作 store 数并一次性完成进度。因此该计数是兼容性占位，不是实际恢复进度。负副本数也不会被拒绝；饱和运算只防止算术溢出，不保证结果非负。

summary 状态位于 `stubs.rs` 的线程局部 `SummaryCollector`：`CollectInt` 写入 `Mutex<HashMap<...>>`，`SetSuccessStatus` 写入 `AtomicBool`。它不会跨线程自动汇总。

## 依赖与调用关系

上游关系：

- `br/cmd/br/restore.rs::runRestoreCommand` 是生产代码中可见的直接调用者，在 EBS 非 prepare 分支调用 `RunResolveKvData`。
- RustCodeGraph 还识别到 `restore_data_test.rs::resolve_progress_uses_go_store_formula` 与 `parity_test.rs::go_rust_public_contract_matches` 的测试调用。
- `br/pkg/task/lib.rs` 负责模块挂载和公开重导出。

下游关系：

- `ReadBackupMetaData` 依赖 `Storage::ReadFile` 与 `serde_json::from_slice`。
- `RunResolveKvData` 调用 `RestoreConfig::Adjust`、`ReadBackupMetaData`、`CollectInt`、`GetKeepalive`、`NewMgr`、`Glue::StartProgress`、`Progress::IncBy/Close`、`SetSuccessStatus` 和 `Mgr::Close`。
- `NewMgr` 位于 `br/pkg/task/common.rs`；当前返回 `MemMgr`，并未连接真实 PD/TiKV 管理器。
- 所用 `Glue`、`Storage`、`Progress`、`Mgr`、`Error` 与 summary 函数来自 `br/pkg/task/stubs.rs`，说明该路径目前依赖本地抽象/桩，而非 Go 版本的真实 `br/pkg/conn`、`br/pkg/gc`、`br/pkg/restore/data` 链。

Cargo 边界方面，`Cargo.toml` 声明该目录为库 crate，并直接依赖 `serde`/`serde_json` 以及多个本地 BR crate；本文件实际显式使用 crate 内 `common`、`restore`、`stubs` 和 `serde`。

## 错误处理与边界

- `backupmeta.json` 不存在或存储读取失败：原样传播 `Storage::ReadFile` 的 `Error`；`MemStorage` 的典型消息是 `file not found: backupmeta.json`。
- JSON 非法或字段类型不符：转换为仅含 Serde 文本的本地 `Error`。
- 备份类型缺失、为空、为 `kv` 或其他值：统一返回固定的 EBS-only 错误。
- `cluster_info` 存在时拥有绝对优先级；即便根级字段有效，空的嵌套备份类型仍会导致拒绝。这是当前代码事实，扩展兼容格式时必须显式决定是否改变。
- `tikv` 或 `replicas` 缺失会得到 `0`，不会报错；`resolved_ts` 缺失也会得到 `0`，不会校验其业务有效性。
- `resolveTS as i64` 对大于 `i64::MAX` 的值会按 Rust 转换规则回绕为负数；当前没有范围检查。
- PD 列表为空时 `NewMgr` 返回 `pd address can not be empty`；TLS 无效也会在 manager 构造阶段失败。
- `Mgr` 构造之后当前没有可失败调用，因此 guard 主要保护未来扩展；一旦新增真实恢复步骤，应继续保持 guard 在第一个可能失败步骤之前建立。
- `Progress::IncBy` 与 `Close` 没有返回 `Result`，当前无法传播进度后端错误；成功标志只在所有现有步骤结束后设置。

## 并发与资源生命周期

本入口是同步函数，不创建线程、异步任务或 channel。`Arc<dyn Storage>` 和 `Arc<dyn Mgr>` 表达共享所有权，相关 trait 均要求 `Send + Sync`；当前执行仍完全发生在调用线程。

`MgrCloseGuard` 是本文件最重要的资源生命周期约束：只有 `NewMgr` 成功后才创建，创建后无论正常返回、`?` 提前返回还是 panic 展开，`Drop` 都会调用一次 `Close`（进程 abort 除外）。进度资源则是手工关闭，不具备 RAII guard；若未来在 `StartProgress` 与 `Close` 之间加入可失败步骤，必须增加关闭 guard 或等价 finally 结构，避免泄漏。

`MemProgress` 内部用 `AtomicI64` 和 `AtomicBool`，summary 用 thread-local、`Mutex` 与 `AtomicBool`；这些设施允许 trait 对象跨线程，但本函数没有并行恢复。Go 版本会启动 `progressFileWriterRoutine` goroutine，并通过可取消 context 管理其生命周期；Rust 当前未移植这些并发与取消语义。

## 与 Go 版本的对应关系

已对齐的部分：

- 两端都拒绝非 EBS 元数据，并返回 resolve TS 与 TiKV 副本数。
- 两端入口都先 `Adjust` 配置、记录 `resolve-ts`、创建 manager，并保证 manager 最终关闭。
- 进度总量的意图均为 `store 数 * 3 + 2`，成功结束时都会关闭进度并设置成功状态。

Rust 特有的兼容行为是直接解析 `backupmeta.json` 的简化 JSON，并兼容旧 operator 将 `full_backup_type`、`resolved_ts` 放在根级的格式；Go 通过 `config.NewMetaFromStorage` 获取完整元数据对象。

尚未对齐且影响真实功能的部分包括：Rust CLI 没有打开配置指定的外部存储；没有 context、取消和 tracing span；没有全局 TiDB 配置调整；没有获取 restore TS 或 GC safe point keeper；没有移除并恢复 PD scheduler；没有枚举实际 TiKV stores 和检测 store 数变化；没有 `data.RecoverData`、总 region 指标、`UnmarkRecovering`、TiFlash replica 重置及恢复耗时指标；没有进度文件 writer。Rust 还使用元数据副本数代替 Go 的实际 store 数，并立即完成整个进度条。这些差异意味着当前实现只验证局部契约，不能声称已经完成 EBS KV 数据恢复。

Go 同路径未发现专用的 `restore_data_test.go`；当前局部语义主要由 Rust 独立测试 `br/pkg/task/restore_data_test.rs` 和综合契约测试 `br/pkg/task/parity_test.rs` 覆盖。

## 扩展指南

若要补齐真实 EBS 恢复，建议保持 `RunResolveKvData` 为编排入口，并按 Go 顺序逐段接入能力，避免只让测试通过的缩减实现：

1. 先让 `br/cmd/br/restore.rs` 或任务层通过 `cfg.Config.Storage` 获得真实 `Storage`，保留可注入 storage 以便独立测试。
2. 将简化 JSON 投影与仓库 canonical backup metadata 模型对齐，同时为根级兼容格式、嵌套优先级、缺失/非法 TS 和副本数增加明确策略。
3. 为 `Mgr` 扩展 PD client、GC manager、scheduler、store enumeration、恢复和 unmark 所需接口；不要把生产能力继续塞入 `MemMgr`。
4. 按 Go 的资源获取逆序设计 RAII guards：context/cancellation、GC keeper、scheduler restore、manager close、progress close 都应在错误路径可恢复。
5. 把 `data.RecoverData` 和 TiFlash 重置接入真实 crate；进度必须由真实阶段逐步推进，而不是入口一次性填满。
6. 在 `br/pkg/task/restore_data_test.rs` 同步增加回归测试，至少覆盖存储读取/JSON 错误、嵌套与根级格式、非 EBS、缺失字段、负/极端副本数、空 PD、manager 关闭、恢复中途失败后的清理、进度关闭与成功标志时序。测试逻辑继续放在独立文件，不内嵌进生产源文件。

兼容风险集中在 backupmeta 格式优先级、Go/Rust 错误文本和资源清理顺序；正确性风险是误把副本数当 store 数或过早设置成功；性能风险主要来自未来 store 枚举、并发恢复和进度写盘，需以配置的恢复并发度及可取消任务控制。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`br/pkg/task/restore_data.rs`（RustCodeGraph `node --file` 显示完整 116 行）。
- crate 声明与模块入口：`br/pkg/task/Cargo.toml`、`br/pkg/task/lib.rs`。
- Rust 上游入口：`br/cmd/br/restore.rs::runRestoreCommand`。
- 直接依赖：`br/pkg/task/restore.rs::RestoreConfig::Adjust`、`br/pkg/task/common.rs::NewMgr`，以及 `br/pkg/task/stubs.rs` 中的 `Glue`、`Progress`、`Storage`、`Mgr`、`MemStorage`、summary 实现。
- Go 对照：`br/pkg/task/restore_data.go::ReadBackupMetaData`、`RunResolveKvData`、`resetTiFlashReplicas`。
- 独立 Rust 测试：`br/pkg/task/restore_data_test.rs` 验证嵌套 EBS 元数据、缺失备份类型错误和 `3 * 3 + 2 == 11` 的进度公式；`br/pkg/task/parity_test.rs::go_rust_public_contract_matches` 验证根级旧格式、非 EBS 错误与入口可完成。
- RustCodeGraph 查询：`query ReadBackupMetaData`、`query RunResolveKvData`、`callers/callees` 和 `node RunResolveKvData`。图确认 Rust 入口调用 `ReadBackupMetaData` 并实例化 `MgrCloseGuard`，测试调用者来自上述两个独立测试；图对 trait 方法调用的覆盖不完整，因此下游 trait 关系同时以源码核验。

本任务是纯文档分析，未运行 Cargo 或运行时恢复。结构验证应确认该文档存在且恰好包含任务要求的 11 个固定二级标题；人工复核重点是所有“当前支持”结论均限定在源码已有的桩边界内。
