# [`br/cmd/br/backup.rs`](./backup.rs)

## 文件定位

本文件属于 `astersql-br-cmd-br` crate 的 BR 命令装配层。`br/cmd/br/lib.rs` 以 `pub mod backup` 暴露该模块，`br/cmd/br/main.rs::main` 调用 `NewBackupCommand`，把它作为 `br` 根命令下的 `backup` 一级子命令。文件本身不实现扫描 Region、发送备份 RPC 或写备份元数据等核心算法；它负责定义命令树、汇总 flag、建立命令运行环境，再把请求分派到 `astersql-br-pkg-task`。

crate 边界由 `br/cmd/br/Cargo.toml` 确认：包名为 `astersql-br-cmd-br`，直接依赖路径 crate `astersql-br-pkg-task`、trace、operator 和 streamhelper-config。当前 crate 被标记为从 Go `br/cmd/br` 迁移而来的 binary lane；文件中大量 CLI、日志、配置和 glue 类型来自 `crate::stubs`，这是 slim BR Rust 移植的一部分，而非完整生产依赖的原样替换。

## 核心职责

1. `NewBackupCommand` 构造 `backup` 命令，注册持久化公共备份 flag、统一的 `PersistentPreRunE`，并挂接 `full`、`db`、`table`、`raw`、`txn` 五个无位置参数的子命令。
2. `runBackupCommand` 服务逻辑备份的 full/db/table 三条路径：解析 `BackupConfig`、注册指标、按配置启用 tracing，并在 EBS 与普通 KV 逻辑备份之间分流。
3. `runBackupRawCommand` 与 `runBackupTxnCommand` 分别解析 raw KV、transactional KV 配置，以轻量 `TikvGlue` 调用对应任务层入口。
4. 普通逻辑备份期间，本文件临时调整进程级 TiDB 配置、关闭全局内存限制 tuner，并安装系统库过滤器；两个 Drop 守卫保证后两项在正常返回或 unwind 时恢复。
5. 本文件只负责 CLI 到任务层的适配。实际任务边界分别是 `br/pkg/task/backup.rs::RunBackupWithDefaults`、`backup_ebs.rs::RunBackupEBS`、`backup_raw.rs::RunBackupRawWithDefaults` 和 `backup_txn.rs::RunBackupTxnWithDefaults`。

## 主要符号

- `pub fn NewBackupCommand() -> Command`：本模块唯一面向其他生产模块公开的命令构造入口。它设置 `Use = "backup"`、`SilenceUsage = true`，注册预运行初始化，并通过 `DefineBackupFlags` 提供所有子命令继承的公共参数。
- `fn newFullBackupCommand() -> Command`：绑定 `FullBackupCmd`，默认过滤器由 `DefineFilterFlags(..., acceptAllTables(), false)` 提供，并额外注册 EBS flag。
- `fn newDBBackupCommand() -> Command` / `fn newTableBackupCommand() -> Command`：分别绑定 `DBBackupCmd`、`TableBackupCmd`，通过 `DefineDatabaseFlags`、`DefineTableFlags` 收窄备份对象。
- `fn newRawBackupCommand() -> Command` / `fn newTxnBackupCommand() -> Command`：构造仍标为 experimental 的 KV 范围备份入口，分别注册 raw/txn 专用 flag。
- `fn runBackupCommand(command: &mut Command, cmdName: &str) -> Result<()>`：full/db/table 的共享执行适配器；包含配置解析、指标注册、tracing、EBS 分支、进程配置调整和任务调用。
- `fn runBackupRawCommand(...)` / `fn runBackupTxnCommand(...)`：raw/txn 的执行适配器；两者结构相同，但配置类型、命令常量和任务入口严格分离。
- `fn scopeguard_enable_gctuner() -> impl Drop`：析构时调用 `GlobalMemoryLimitTuner.EnableAdjustMemoryLimit`。
- `pub(super) fn scopeguard_restore_filter(restore: Box<dyn FnOnce() + Send>) -> impl Drop`：持有一次性恢复闭包，析构时至多执行一次；`pub(super)` 仅为 crate 父级及同 crate 测试提供可见性，不是外部 API。

本文件没有模块级常量、trait、条件编译项或业务数据结构；命令名常量和配置类型均来自任务层。

## 执行流程

进程主链为 `br/cmd/br/main.rs::main` 创建根命令并调用 `NewBackupCommand`。用户选定子命令后，命令框架先运行 backup 的 `PersistentPreRunE`：`Init` 初始化公共运行环境，随后记录构建信息和环境变量、输出脱敏后的参数、关闭统计 worker，并把 summary 单位设为 `BackupUnit`。之后具体子命令的 `RunE` 将固定的任务命令名传给相应执行函数。

full/db/table 进入 `runBackupCommand`：

1. 用 `HasLogFile()` 初始化 `BackupConfig.Config.LogProgress`，再由 `effective_task_flags` 合并命令的 persistent/local flag（包括已定义但未显式修改的已知 flag）。
2. 调用 `BackupConfig::ParseFromFlags(&flags, false)`。失败时把 `command.SilenceUsage` 改为 `false`，以便 CLI 同时显示用法，然后返回带 trace 包装的错误。
3. 使用解析后的 PD、TLS 和 keyspace 配置注册 BR 指标；之后读取默认上下文和 `EnableOpenTracing`。
4. 若 `FullBackupType == FullBackupTypeEBS`，在 `with_tracing` 中锁定全局 `tidbGlue`，创建 `MemStorage`，直接调用 `RunBackupEBS`，不执行普通备份的 coprocessor、GC tuner 和数据库过滤器调整。
5. 非 EBS 路径在 tracing 作用域内把全局 `AdvertiseAddress` 改为 `UnavailableIP`、把 coprocessor cache 容量设为 0；关闭 GC tuner 并建立自动重启守卫；通过 `setTiDBGlueDBFilter(FilterLoadSysDBs)` 安装过滤器并建立恢复守卫；锁定 `tidbGlue` 后调用 `RunBackupWithDefaults`。
6. 任务错误以 `failed to backup` 记录并再次包装返回；成功返回 `Ok(())`。

raw 与 txn 路径分别使用 `RawKvConfig`、`TxnKvConfig` 的 `ParseBackupConfigFromFlags`，随后在可选 tracing 作用域中以无状态 `TikvGlue` 调用对应 `RunBackup*WithDefaults`。它们不修改 TiDB glue 过滤器、全局 TiDB advertised address 或 GC tuner，因为这两类命令绕过 SQL/TiDB glue 层直连 KV 任务接口。

## 数据与状态

- 命令树状态存放于返回的 `Command`：父命令持有公共 persistent flags 和预运行回调，子命令持有本地 flags 与 `RunE` 闭包。所有子命令设置 `no_args = true`，拒绝 Cobra/pflag 语义下误把 flag 值写成位置参数的用法。
- 每次执行新建一个 `BackupConfig`、`RawKvConfig` 或 `TxnKvConfig`。配置从命令 flag 快照填充；该文件不缓存跨命令配置。
- `tidbGlue()` 是共享的全局 glue，并通过 mutex 锁定。普通逻辑备份和 EBS 分支持锁调用任务层，因此扩展任务调用时必须避免在同一线程重入并再次获取该锁。
- 普通逻辑备份永久写入当前进程的全局 config：`AdvertiseAddress = UnavailableIP`、`CoprCache.CapacityMB = 0.0`。代码没有为这两项建立恢复守卫；设计假设 BR 是单用途批处理进程。
- GC tuner 与 InfoSchema filter 是临时全局状态，由 Drop 守卫恢复。局部变量声明顺序使析构逆序发生：先恢复 filter，再重新启用 tuner，最后离开 tracing 包装。
- 当前 `WithDefaults` 任务入口会构造内存版 manager/client；EBS 分支也显式使用 `MemStorage`。这些是当前 Rust slim 移植的真实状态，不能据此宣称已连接完整 PD/TiKV/AWS 生产后端。

## 依赖与调用关系

上游生产调用是 `br/cmd/br/main.rs::main -> backup::NewBackupCommand`；`br/cmd/br/lib.rs` 提供模块声明和库/二进制共享入口。`br/cmd/br/parity_test.rs::contract_normal_command_tree_and_filters` 也直接构造该命令，固定子命令顺序与根命令组合契约。

CLI 内部调用边为：

- `NewBackupCommand -> newFullBackupCommand/newDBBackupCommand/newTableBackupCommand/newRawBackupCommand/newTxnBackupCommand`；
- `newFull/newDB/newTable -> runBackupCommand`；
- `newRaw -> runBackupRawCommand`；`newTxn -> runBackupTxnCommand`；
- 三个 run 函数均依赖 `cmd.rs::{HasLogFile, GetDefaultContext, with_tracing}` 和 `stubs.rs::effective_task_flags`；
- 普通逻辑备份还依赖 `metricsutil::RegisterMetricsForBR`、`config::UpdateGlobal`、`gctuner`、`setTiDBGlueDBFilter` 与共享 `tidbGlue`；
- 任务层下游为 `RunBackupWithDefaults -> RunBackup`、`RunBackupRawWithDefaults -> RunBackupRaw`、`RunBackupTxnWithDefaults -> RunBackupTxn`；EBS 直接进入 `RunBackupEBS`。

RustCodeGraph 对上述三个 run 函数确认了 `HasLogFile`、`GetDefaultContext`、`with_tracing` 与各 `RunBackup*` 调用边。由于 Rust 与 Go 文件存在相同 CamelCase 名称，图查询 `NewBackupCommand`/`runBackupCommand` 时会同时返回 `backup.go` 的同名节点；生产入口接线因此以 `main.rs` 的显式 import/call 为准。

## 错误处理与边界

- 三类配置解析失败都设置 `SilenceUsage = false`，与默认成功/运行期失败保持静默 usage 的行为区分。返回值通过 `Error::Trace` 保留错误链。
- 指标注册发生在普通/EBS 分流之前；注册失败直接返回，任务不会启动。该错误当前没有 `failed to backup` 日志包装，因为日志包装只覆盖 tracing/任务执行结果。
- EBS 类型由严格相等比较触发；任务层 `RunBackupEBS` 还会验证类型并拒绝非 `aws-ebs` 值。当前命令层传入内存存储，任务层保存的是精简元数据路径。
- full/db/table/raw/txn 均禁止额外位置参数。raw/txn 的 experimental 标签说明其 CLI 稳定性边界，也提醒调用方自行保证 key 范围编码与恢复兼容性。
- `tidbGlue().lock().unwrap()` 在 mutex poisoned 时会 panic；本文件没有把此类 panic 转换为 `Result`。不过 filter 恢复守卫已经在获取 glue 锁之前创建，unwind 时仍会尝试执行恢复闭包。
- `scopeguard_restore_filter` 用 `Option::take` 保证恢复闭包只运行一次；`br/cmd/br/backup_test.rs::backup_filter_cleanup_runs_during_unwind` 验证了 panic unwind 场景。
- `with_tracing` 负责 start/finish 配对并返回闭包结果。它把原命令 `ctx` 传给任务闭包，而 tracing crate 返回的 `next_ctx` 仅用于 finish；新增依赖 span context 的业务前应先核实这一当前实现，而不能假定任务已收到派生 tracing context。

## 并发与资源生命周期

本文件不创建线程、异步任务或通道。并发生命周期主要来自共享进程状态：全局 glue mutex 将一次逻辑/EBS 任务的 glue 使用串行化；全局 config、GC tuner、InfoSchema filter 与 summary 单位都对同进程其他命令可见，因此当前设计依赖 CLI 单命令执行模型，不适合在同一进程并行运行多个 backup 子命令。

普通逻辑备份中资源建立顺序为：进入 tracing → 更新全局 config → 禁用 tuner → 创建 tuner 恢复守卫 → 安装 filter → 创建 filter 恢复守卫 → 获取 glue 锁 → 调用任务层。作用域结束时 mutex guard 先释放，随后 filter 恢复，再启用 tuner，最后 `with_tracing` 完成 span。Rust 的 Drop/unwind 保证覆盖正常返回、`Err` 和可展开 panic；进程 abort 或强制退出不保证析构。

EBS、raw、txn 没有上述两个恢复守卫。EBS 在 tracing 闭包中持有 glue mutex 并拥有一个 `Arc<dyn Storage>`；raw/txn 使用栈上的无状态 `TikvGlue`。任务层 manager/client/storage 的完整生命周期由对应 `RunBackup*` 调用管理，不由本文件跨调用保存。

## 与 Go 版本的对应关系

直接对照文件为 `br/cmd/br/backup.go`。Rust 保留了 Go 的三类执行入口、五个子命令、命令名常量、flag 注册位置、预运行初始化顺序、解析失败显示 usage、指标先于任务运行、EBS 提前分流、普通逻辑备份关闭 tuner/安装过滤器，以及错误日志文案。

Go 的 `defer` 在 Rust 中分别由 `with_tracing` 和两个 Drop 守卫表达。`scopeguard_restore_filter` 的 unwind 测试补充验证了 Go defer 到 Rust RAII 的关键清理语义。`no_args = true` 对应 Go 的 `cobra.NoArgs`。

需要明确的当前差异：

- Go 调用 `task.RunBackup/RunBackupRaw/RunBackupTxn` 并连接真实 manager/client；Rust 命令层调用 `RunBackup*WithDefaults`，后者当前构造 `MemBackupClient` 等默认桩。
- Go EBS 路径由真实上下文、全局 `tidbGlue` 与任务层外部存储行为驱动；Rust 额外传入 `MemStorage`，且任务层源码明确包含固定 Region/精简元数据行为。
- Go tracing 把 `TracerStartSpan` 返回的新 context 继续传给任务；Rust `with_tracing` 当前把原 `ctx` 传给闭包。
- Go 的 TiDB glue 是直接共享值；Rust 以 mutex 保护全局 glue，增加了串行化和 poisoned-lock panic 边界。

同目录没有 `backup_test.go`；Go 侧最接近的命令包测试是 `br/cmd/br/main_test.go`，未直接覆盖这些函数。Rust 独立测试 `br/cmd/br/backup_test.rs` 覆盖 filter unwind 清理，`br/cmd/br/parity_test.rs` 覆盖命令树、默认过滤器、summary 单位、全局清理与 tracing 配对。端到端脚本如 `br/tests/br_full/run.sh`、`br/tests/br_rawkv/run.sh`、`br/tests/br_txn/run.sh` 展示 Go BR 的真实 CLI 使用面，但不等同于对当前 Rust slim 后端的执行验证。

## 扩展指南

- 新增 backup 子命令时，在独立 `new*Command` 中定义本地 flag 和 `RunE`，再在 `NewBackupCommand::AddCommand` 中按兼容顺序注册；同步扩展 `br/cmd/br/parity_test.rs` 的命令树断言。不要把 Rust 单元测试内嵌到 `backup.rs`，应放在同目录独立 `*_test.rs` 文件并由 `lib.rs` 的 `#[cfg(test)]` 模块声明接入。
- 新增 full/db/table 共享行为优先接入 `runBackupCommand`；仅某一任务类型需要的 flag 或分支应留在对应构造器/任务层，避免改变其他命令的解析契约。
- 引入新的临时全局状态时必须像 tuner/filter 一样建立 RAII 恢复，并新增正常、错误和 unwind 路径测试。若状态应永久影响单用途进程，应在文档和测试中明确这一不变量。
- 将 slim 路径替换为真实后端时，修改重点是四个任务调用边和 EBS storage 构造；同时核对 `br/pkg/task` 的独立测试，而不是在命令层复制任务算法。真实依赖应继续由 Cargo crate 边界引入，不能把外部 Rust 依赖复制进 vendor/third_party 或使用本地 patch。
- 修改 tracing 时应优先验证任务闭包是否应接收派生 context，并同步 `cmd.rs::with_tracing` 及 `parity_test.rs` 的资源清理测试。
- 兼容风险集中在命令名/顺序、flag 继承、usage 展示和日志文案；性能风险集中在 glue 锁持有范围、全局 cache 关闭、任务层并发参数；正确性风险集中在过滤器恢复、tuner 恢复、EBS 分流和 raw/txn key 范围语义。

## 验证依据

- 目标源码：`br/cmd/br/backup.rs`（298 行）；模块与生产入口：`br/cmd/br/lib.rs`、`br/cmd/br/main.rs`；crate 声明：`br/cmd/br/Cargo.toml`。
- Go 对照：`br/cmd/br/backup.go`；未发现同目录 `backup_test.go`，补读测试 `br/cmd/br/main_test.go` 的存在性并以 Rust 独立测试为直接命令契约证据。
- Rust 测试：`br/cmd/br/backup_test.rs::backup_filter_cleanup_runs_during_unwind`；`br/cmd/br/parity_test.rs::contract_normal_command_tree_and_filters`、`contract_resource_cleanup`。
- 任务层直接证据：`br/pkg/task/backup.rs::{BackupConfig, DefineBackupFlags, RunBackupWithDefaults}`、`backup_ebs.rs::RunBackupEBS`、`backup_raw.rs::{RawKvConfig, RunBackupRawWithDefaults}`、`backup_txn.rs::{TxnKvConfig, RunBackupTxnWithDefaults}`，以及 `br/cmd/br/stubs.rs::effective_task_flags`。
- RustCodeGraph：索引状态为 11,467 个文件、7,032 个 Rust 文件；目标文件包含 27 个符号。`node --file br/cmd/br/backup.rs` 核对完整源码；`query` 核对 `NewBackupCommand`、三个 run 函数和 `effective_task_flags`；`callers/callees --file` 核对五个子命令到三类 run 函数，以及 run 函数到 `with_tracing` 和四个任务层入口的调用边。图对 Rust/Go 同名符号存在混合，相关入口另由 `main.rs` 源码交叉验证。
- 文档只描述已读源码能证明的现状；没有运行 Cargo，也没有把 Go 集成脚本视为 Rust 后端通过证据。结构验证按任务规定检查文件存在且恰有十一个固定二级标题。
