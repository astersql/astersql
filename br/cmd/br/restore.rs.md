# `br/cmd/br/restore.rs`

## 文件定位

`restore.rs` 是 Rust 版 BR 可执行 crate `astersql-br-cmd-br` 中的恢复命令装配层。`br/cmd/br/lib.rs` 以 `pub mod restore` 纳入本模块，`br/cmd/br/main.rs::main` 再通过 `NewRestoreCommand()` 把它挂到顶层 `br` 命令。因此它位于“命令行解析/进程环境准备”和 `astersql_br_pkg_task` 恢复任务实现之间，而不是数据恢复算法所在处。

crate 边界由 `br/cmd/br/Cargo.toml` 确认：该包既有 `lib.rs`，也以 `bin_main.rs` 生成 `astersql-br-cmd-br`，并直接依赖路径 crate `../../pkg/task`（包名 `astersql-br-pkg-task`）。Cargo 元数据把它标为对齐 Go 包 `br/cmd/br` 的 binary 迁移单元。当前实现使用本 crate 的 `Command`、glue、日志和配置桩；不能仅凭函数名推断它已经具有 Go 版全部外部 I/O 能力。

直接对照文件是 `br/cmd/br/restore.go`。Rust 文件保留了 Go 文件的三类运行入口、一个用户提示函数和六个子命令构造器，但以 Rust 的 `Result`、`Arc` 回调和 Drop 守卫表达错误传播与清理。

## 核心职责

本文件承担四类职责：

1. `NewRestoreCommand` 建立 `restore` 命令及 `full`、`db`、`table`、`raw`、`txn`、`point` 六个子命令，并注册公共或子命令专属 flag。
2. `runRestoreCommand` 负责 full/db/table/point 的共享前置编排：解析配置、注册指标、设置进程级恢复环境、选择 EBS 分支、管理 schema 过滤器和 tracing，最后调用任务层 `RunRestore`。
3. `runRestoreRawCommand` 与 `runRestoreTxnCommand` 分别解析 RawKV/TxnKV 所需配置，并以 `TikvGlue` 调用任务层入口。
4. `printWorkaroundOnFullRestoreError` 只为“目标集群非空”和“系统表不兼容”两类可识别错误输出操作建议。

该文件不读取备份元数据、不拆分 region、不导入 SST，也不实现 PITR 回放。上述工作由 `br/pkg/task/restore.rs`、`restore_raw.rs`、`restore_txn.rs`、`restore_data.rs`、`restore_ebs_meta.rs` 及其更下游模块完成。

## 主要符号

- `fn runRestoreCommand(command: &mut Command, cmdName: &str) -> Result<()>`：full、db、table、point 共用的执行入口。它根据 `cmdName` 判断是否追加解析 stream/PITR flag，并根据 `cfg.FullBackupType` 和 `cfg.Prepare` 选择普通恢复、EBS 元信息恢复或 EBS KV 数据解析路径。
- `pub fn printWorkaroundOnFullRestoreError(err: &Error)`：模块中除顶层构造器外唯一公开函数。它用 `ErrorEqual` 比较 `ErrRestoreNotFreshCluster` 与 `ErrRestoreIncompatibleSys`，其他错误不输出提示。
- `fn runRestoreRawCommand(...)`：构造 `RestoreRawConfig`，调用 `ParseFromFlags` 后以 `TikvGlue` 执行 `RunRestoreRaw`。
- `fn runRestoreTxnCommand(...)`：构造通用 `Config`，调用 `ParseFromFlags` 后以 `TikvGlue` 执行 `RunRestoreTxn`。
- `pub fn NewRestoreCommand() -> Command`：模块的主要公开入口。其 `PersistentPreRunE` 依次执行 `Init`、构建信息与环境日志、参数审计、统计 worker 禁用、事务大小上限放宽和恢复摘要单位设置。
- `newFullRestoreCommand`：注册默认系统库过滤器及 snapshot restore flag，执行时传入 `FullRestoreCmd`。
- `newDBRestoreCommand` / `newTableRestoreCommand`：分别注册数据库/表选择 flag，传入 `DBRestoreCmd` / `TableRestoreCmd`。
- `newRawRestoreCommand` / `newTxnRestoreCommand`：均使用 raw restore flag 集；分别进入 RawKV 与 TxnKV 执行器。
- `newStreamRestoreCommand`：用户可见命令名为 `point`，注册启用 stream 语义的过滤器和 stream restore flag，并以 `PointRestoreCmd` 复用 `runRestoreCommand`。

本文件没有自定义结构体、枚举、trait、模块常量或条件编译项。回调类型、`Command` 和 flag 容器来自 `br/cmd/br/stubs.rs`；任务命令常量来自 `br/pkg/task/restore.rs`。

## 执行流程

进程主链如下：

1. `br/cmd/br/main.rs::main` 创建根 `Command`，调用 `NewRestoreCommand` 挂载恢复命令。
2. 命令执行器先运行 `restore` 的 `PersistentPreRunE`。它初始化日志/状态环境，记录构建、环境和参数信息，调整统计与事务大小全局开关，并把 summary 单位设为 restore。
3. 叶子命令拒绝位置参数（`no_args: true`），随后其 `RunE` 按命令种类进入三个执行入口之一。

full/db/table/point 的共享流程是：

1. 以 `HasLogFile()` 初始化 `RestoreConfig.Config.LogProgress`，用 `effective_task_flags` 合并持久 flag 与叶子本地 flag。
2. `RestoreConfig::ParseFromFlags(..., false)` 失败时重新显示 usage（`SilenceUsage = false`）并返回追踪后的错误。
3. 使用最终的 PD、TLS、Keyspace 配置注册 BR metrics；若 `IsStreamRestore(cmdName)` 为真，再解析 PITR 专属参数。
4. 打开全局 `SkipGrantTable`，取得默认上下文与 tracing 开关。
5. 若备份类型是 `FullBackupTypeEBS`：`Prepare` 为真时用 `TikvGlue` 调用 `RunRestoreEBSMetaWithDefaults`；否则锁定 `tidbGlue`，构造当前 Rust 任务边界使用的 `MemStorage`，调用 `RunResolveKvData`。此分支完成后直接返回，不执行后续普通 snapshot/point 恢复环境调整。
6. 非 EBS 路径把 BR 的 advertised address 设为不可用地址、关闭 coprocessor cache，随后禁用全局内存限制 tuner，并建立 Drop 守卫保证函数退出时重新启用。
7. 配置中指定 schema 时，先去除名称反引号，再用 `FilterLoadSpecifiedDBAndSysDBs` 临时替换 `tidbGlue` 的 InfoSchema 过滤器。
8. 在 `with_tracing` 中锁定 `tidbGlue` 并调用 `RunRestore`；调用返回后恢复临时过滤器。失败时记录日志，按错误类型打印 workaround，然后返回错误；成功则返回 `Ok(())`。

RawKV 和 TxnKV 路径较短：各自构造配置、合并并解析 flag、读取 tracing 开关，以 `TikvGlue` 调用 `RunRestoreRaw` 或 `RunRestoreTxn`；任务失败会记录带路径区分的错误消息再向上传播。

## 数据与状态

- `RestoreConfig` 是普通、PITR 和 EBS 路径的主要可变状态；`RestoreRawConfig` 用于 RawKV；`Config` 用于 TxnKV。三者只在一次命令执行期间存在，并以 `&mut` 传给任务层。
- `effective_task_flags(command)` 合并父命令 persistent flag 与当前叶子命令 local flag。文档或扩展不能只检查 `command.Flags()`，否则会漏掉 `DefineRestoreFlags` 注册在父命令上的配置。
- `cmdName` 不是用户输入的 `Use` 文本，而是任务层常量：`Full Restore`、`DataBase Restore`、`Table Restore`、`Raw Restore`、`Txn Restore`、`Point Restore`。任务层用它生成 summary，并用 `PointRestoreCmd` 判定 stream restore。
- `cfg.Config.Schemas` 驱动临时 InfoSchema 过滤；每个 schema 在传给 `FilterLoadSpecifiedDBAndSysDBs` 前经 `utils::UnquoteName` 规范化。
- 进程级可变状态包括 `Security.SkipGrantTable`、`AdvertiseAddress`、coprocessor cache 容量、内存 tuner、`TxnTotalSizeLimit`、summary unit 及 `tidbGlue` 的过滤器。其中本文件只显式恢复内存 tuner 和临时数据库过滤器；其他设置按 BR 单次进程模型保留。
- 当前 EBS 非 prepare 分支显式创建 `Arc<dyn Storage>` 包装的 `MemStorage`。这是 Rust slim 任务边界的当前事实，不等价于 Go 路径直接使用完整外部存储栈。

## 依赖与调用关系

上游关系：

- `br/cmd/br/lib.rs` 声明 `pub mod restore`。
- `br/cmd/br/main.rs::main` 调用 `NewRestoreCommand`，把返回值与 debug、backup、stream、operator、abort 并列加入根命令。
- `br/cmd/br/parity_test.rs::contract_normal_command_tree_and_filters` 直接构造该命令并验证六个子命令的顺序；`contract_error_paths` 直接调用 workaround 函数。

文件内调用边由 RustCodeGraph `explore` 核对：`NewRestoreCommand -> newFullRestoreCommand -> runRestoreCommand`、`NewRestoreCommand -> newRawRestoreCommand -> runRestoreRawCommand`、`NewRestoreCommand -> newTxnRestoreCommand -> runRestoreTxnCommand`；其 blast radius 还显示 db/table/point 三个构造器调用 `runRestoreCommand`。

下游依赖分三层：

- CLI 公共层：`crate::cmd::{Init, GetDefaultContext, HasLogFile, with_tracing, setTiDBGlueDBFilter, tidbGlue}` 与 `crate::stubs::{Command, Error, Result, TikvGlue, config, metricsutil, gctuner, ...}`。
- 任务配置层：`DefineRestoreFlags`、`DefineFilterFlags`、`DefineDatabaseFlags`、`DefineTableFlags`、`DefineRawRestoreFlags`、`DefineRestoreSnapshotFlags`、`DefineStreamRestoreFlags` 及三个配置类型。
- 任务执行层：普通恢复调用 `RunRestore`；EBS prepare 调用 `RunRestoreEBSMetaWithDefaults`；EBS 数据解析调用 `RunResolveKvData`；RawKV/TxnKV 分别调用 `RunRestoreRaw`/`RunRestoreTxn`。真正的 manager、storage、meta 校验和进度生命周期位于 `br/pkg/task`。

## 错误处理与边界

- 三个配置解析入口失败时均把 `command.SilenceUsage` 设为 `false`，让参数错误重新显示用法；业务执行错误则保持默认静默 usage，避免把运行失败误导成语法错误。
- metrics 注册和 point restore 的额外 flag 解析使用 `?` 或显式 `Error::Trace` 向上传播。metrics 注册发生在 stream flag 追加解析之前，与当前源码顺序一致。
- EBS 分支按 prepare/resolve 分别记录 `failed to restore EBS meta` 或 `failed to restore data`；普通、raw、txn 路径各有独立错误日志。
- workaround 严格依赖 `ErrorEqual`：仅 `ErrRestoreNotFreshCluster` 提示清理已有库表，仅 `ErrRestoreIncompatibleSys` 提示 `--with-sys-table=false`。它不修改返回错误，也不吞掉错误。
- `tidbGlue().lock().unwrap()` 会在 mutex 中毒时 panic；本文件没有把锁中毒转换为 `Result`。过滤器恢复闭包在正常返回和 `Err` 返回后执行，但若任务调用 panic，则不能像 RAII 守卫一样保证恢复。
- EBS 分支在普通路径的全局配置调整和内存 tuner 守卫之前提前返回，这是控制流边界，扩展 EBS 行为时不能假设这些普通路径设置已经生效。
- raw/txn 标记为 experimental；调用者仍需承担 key 编码、备份模式和目标集群兼容性。更具体的模式拒绝由任务层验证，而非本文件验证。

## 并发与资源生命周期

- `RunE` 与 `PersistentPreRunE` 被存放在 `Arc<dyn Fn + Send + Sync>` 中，满足命令对象可克隆和共享的要求；本文件自身不创建线程或异步任务。
- `tidbGlue` 是 mutex 保护的全局单例。普通恢复在调用 `RunRestore` 期间持锁；临时替换和恢复 InfoSchema 过滤器时也分别取锁。因此并行执行多个 restore 命令会在 glue 上串行，并共享进程级配置，当前设计更适合单命令进程而非同进程并发恢复。
- 内存 tuner 通过局部 `G: Drop` 守卫恢复，正常返回、`?` 提前返回和栈展开都会调用 `EnableAdjustMemoryLimit`（进程 abort 除外）。
- schema 过滤器由 `setTiDBGlueDBFilter` 返回 `FnOnce` 恢复闭包；本文件在 `RunRestore` 返回后显式执行，因此覆盖成功和普通错误，但不覆盖 panic。
- `with_tracing` 在启用时开始 span，在业务闭包返回后结束 span，再返回原结果；关闭时直接调用闭包。RawKV、TxnKV 和普通/EBS 分支均通过这一同步包装器运行。
- EBS storage 用 `Arc<dyn Storage>` 传递所有权；更深层 manager、progress 和 EBS controller 的关闭由任务层实现及其独立测试验证。

## 与 Go 版本的对应关系

`br/cmd/br/restore.go` 是逐函数对照基线。命令名、子命令顺序、flag 组合、配置解析错误显示 usage、PITR 追加解析、SkipGrantTable、metrics、普通恢复环境调整、schema 过滤、三种 KV 路径以及 workaround 文案均有一一对应的 Rust 符号。

主要表达差异如下：

- Go 的 `*cobra.Command` 对应本地 `Command` 替身；Go 闭包对应 `Arc` 回调；Go `errors.Trace` 对应 `Error::Trace`/`map_err`。
- Go 的 `defer GlobalMemoryLimitTuner.EnableAdjustMemoryLimit()` 在 Rust 中由 Drop 守卫实现；Go 的 `defer restore()` 在 Rust 中是任务返回后的显式 `FnOnce` 调用。
- Go 直接使用 `gluetidb`/`gluetikv`、appdash 和真实任务依赖；Rust 通过 `tidbGlue`、`TikvGlue`、`with_tracing` 及 `stubs.rs` 的平台适配运行。
- Go 的 EBS prepare 调用 `RunRestoreEBSMeta`，Rust CLI 调用 `RunRestoreEBSMetaWithDefaults`；Go resolve 路径由任务配置取得存储，当前 Rust CLI 显式传入 `MemStorage`。因此文档只能确认控制流和契约对齐，不能声称 EBS 外部存储集成完全等价。
- Rust 在普通 restore 完成后手动恢复 schema filter；语义覆盖普通成功/错误返回，但 panic 清理保证弱于 Go `defer`。

Go 同目录没有直接针对 `restore.go` 的独立测试；Rust 使用同目录独立文件 `parity_test.rs` 固定公开命令树和错误分类。更深的执行语义由 `br/pkg/task/*_test.rs` 覆盖。

## 扩展指南

- 新增 restore 子命令时，在 `NewRestoreCommand` 注册构造器，为构造器选择正确的任务命令常量和 flag 定义；同步更新 `br/cmd/br/parity_test.rs` 的 restore 子命令顺序断言，并与 `restore.go` 的增量保持一致。
- 新增普通/PITR 配置时，先确认 flag 应属于父级 `DefineRestoreFlags` 还是叶子专属定义，再确认 `effective_task_flags` 能收集它，并在任务 crate 的配置解析独立测试中覆盖默认值、非法值和边界值。
- 扩展 EBS 路径时应分别审查 prepare 与 resolve 两条分支，特别是 storage 来源、glue 类型、提前返回和清理语义；不能把当前 `MemStorage` 当成真实远端存储实现。
- 修改全局设置或临时过滤器时优先使用 Drop 守卫，保证 `Err` 与 panic 路径都恢复；同时评估同进程并发命令对全局单例和锁持有时间的影响。
- 新增可操作错误提示时，在 `printWorkaroundOnFullRestoreError` 使用稳定错误身份比较，不以字符串匹配；在 `parity_test.rs` 增加独立断言，并核对 Go 文案和触发条件。
- 本仓库要求 Rust 测试与生产源分离。该文件的 CLI 契约测试应继续放在 `br/cmd/br/parity_test.rs` 或新建独立 `*_test.rs`；任务执行细节应在 `br/pkg/task/restore_test.rs`、`restore_nokit_test.rs`、`restore_lifecycle_test.rs`、`restore_raw_test.rs`、`restore_txn_test.rs`、`restore_data_test.rs`、`restore_ebs_meta_test.rs` 等对应测试文件扩展。
- 兼容性风险集中在 CLI 名称/flag 默认值、错误展示、全局状态恢复和 Go/Rust 分支顺序；性能风险主要来自扩大 `tidbGlue` 锁临界区、增加全局 cache/metrics 开销或改变任务层并发配置。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/cmd/br` 确认 `restore.rs` 为 14-symbol 生产文件，且同目录存在 `restore.go` 与独立 `parity_test.rs`；`explore "br/cmd/br/restore.rs symbols callers callees restore command flow"` 给出构造器到三个执行入口的调用路径及各入口 caller 数。精确 `callers/callees` 命令本次未输出文本，因此没有据此推断不存在其他调用。
- 已读 Rust 源：`br/cmd/br/restore.rs`、`br/cmd/br/main.rs`、`br/cmd/br/lib.rs`、`br/cmd/br/cmd.rs`、`br/cmd/br/stubs.rs`。
- 已读 crate 配置与 Go 对照：`br/cmd/br/Cargo.toml`、`br/cmd/br/restore.go`；同目录未发现 `doc.go`。
- 已读直接 Rust 测试：`br/cmd/br/parity_test.rs`，其中 `contract_normal_command_tree_and_filters` 验证恢复子命令恰为 `full/db/table/raw/txn/point`，`contract_error_paths` 覆盖普通错误和两类特定 workaround 错误。
- 已核对任务层入口及测试位置：`br/pkg/task/restore.rs`、`restore_raw.rs`、`restore_txn.rs`、`restore_data.rs`、`restore_ebs_meta.rs`；相关独立测试包括 `restore_test.rs`、`restore_nokit_test.rs`、`restore_lifecycle_test.rs`、`restore_raw_test.rs`、`restore_txn_test.rs`、`restore_data_test.rs`、`restore_ebs_meta_test.rs` 与 `parity_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务给定的 shell 命令验证目标文件存在且恰有 11 个固定二级标题，并人工检查所有重要陈述均能回溯到上述符号或文件。
