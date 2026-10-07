# `br/cmd/br/operator.rs`

## 文件定位

本文件属于 `astersql-br-cmd-br` crate，是隐藏一级命令 `br operator` 的命令树装配层。模块由 `br/cmd/br/lib.rs` 以 `pub mod operator` 纳入 crate，`br/cmd/br/main.rs::main` 将 `newOperatorCommand()` 加入 `br` 根命令；`br/cmd/br/Cargo.toml` 则声明该 crate 直接依赖 `astersql-br-pkg-task` 与 `astersql-br-pkg-task-operator`。因此本文件负责 CLI 边界，不实现迁移、checksum、外部存储探测或集群控制算法，实际业务由 `br/pkg/task/operator/*.rs` 承担。

源文件对应 Go 实现 `br/cmd/br/operator.go`。当前 Rust 文件已接入命令树，但仍依赖 `br/cmd/br/stubs.rs` 提供的 Cobra、context、status mux 与 glue 替身；尤其 CRR 服务运行和 HTTP 注册仍包含明确的 slim-stub 行为，不能据此认定生产能力已与 Go 完全等价。

## 核心职责

`newOperatorCommand` 构造隐藏的运维命令组，统一执行 `Init`、构建信息记录、环境变量记录与参数审计，然后注册 11 个子命令实例（其中快照准备同时保留新旧两个命令名）。每个叶子命令遵循同一边界：声明无位置参数，注册 operator 配置 flag，在 `RunE` 中从 flag 解析配置，再把配置及必要的默认 context/TiDB glue 转交给 task-operator crate。

该文件还承担 CRR checkpoint 的特殊两阶段接线：status server 准备阶段创建服务、保存服务与一次性清理函数并返回路由注册器；真正的 `RunE` 阶段从命令 context 取回同一个状态，运行后无论成功失败均尝试清理。相关符号为 `newCRRCheckpointCommand`、`prepareCRRCheckpointStatusServer`、`getCRRCheckpointServiceState` 与 `CrrCheckpointServiceState`。

## 主要符号

- `CRR_STATE_KEY: u64`：`Context::WithValue/Value` 使用的本地键，用来跨 status 准备与执行阶段传递 CRR 状态。
- `CrrCheckpointServiceState`：持有 task-operator 返回的 `CRRService`、`Mutex<Option<cleanupFunc>>` 和原子 `ran` 标记。`Run()` 当前只设置标记并校验任务名非空；`Register()` 当前只在 `ServeMux` 挂载 `/crr/status`，两者都是 Go 长运行服务的简化替身。
- `newOperatorCommand() -> Command`：本文件唯一公开函数，构造隐藏顶级命令、公共 `PersistentPreRunE` 和全部子命令。
- `newPrepareForSnapshotBackupCommand`：同时构造兼容名 `pause-gc-and-schedulers` 与新名 `prepare-for-snapshot-backup`，解析 `PauseGcConfig` 后调用 `AdaptEnvForSnapshotBackup`。
- `newBase64ifyCommand`、`newListMigrationsCommand`、`newMigrateToCommand`、`newForceFlushCommand`、`newTestStorageCommand`：分别转发外部存储配置编码、迁移清单查询、危险的指定版本迁移、日志备份强制 flush 与外部存储能力探测。`unsafe-migrate-to` 自身也标为隐藏。
- `newChecksumCommand`、`newPitrChecksumCommand`、`newUpstreamChecksumCommand`：分别按备份 rewrite rules、PITR id map 或当前上游视角计算 checksum；三者都以 `!*.*` 作为默认过滤规则，并通过 `tidbGlue()` 取得 glue。
- `newCRRCheckpointCommand`：注册 CRR flag、status preparer 与运行回调。
- `prepareCRRCheckpointStatusServer`：解析 `CRRCheckpointConfig`，调用 `NewCRRCheckpointService`，把 `Arc<CrrCheckpointServiceState>` 写入继承原 context 的新 context，并返回捕获同一状态的 `StatusServerRegistrar`。
- `getCRRCheckpointServiceState`：校验 context、键和值类型，使用 `Arc::downcast` 返回强类型状态。

## 执行流程

1. `br/cmd/br/main.rs::main` 调用 `newOperatorCommand`，把结果和 backup、restore、log 等命令一起挂入根命令。
2. 构造阶段为每个叶子命令注册对应的 `DefineFlagsFor*`；三个 checksum 命令额外调用 `DefineFilterFlags`。`newOperatorCommand` 按固定顺序加入两个快照准备入口及 base64、迁移、flush、CRR、checksum、存储探测命令。
3. 用户选择叶子命令时，父命令的 `PersistentPreRunE` 先执行 `Init`、`build::LogInfo(build::BR)`、`logutil::LogEnvVariables()` 和 `log_arguments_for`。任何初始化错误通过 `?` 终止执行。
4. 普通叶子命令在 `RunE` 中创建默认配置，调用 `ParseFromFlags(cmd.OpFlags())`，解析失败立即返回；成功后直接调用 task-operator 的 `Run*`/`Base64ify`/`AdaptEnvForSnapshotBackup`。快照准备和 base64 路径把默认 context 的取消标记转成 operator context；checksum 路径锁定全局 glue 后传入 `g.as_op()`。
5. CRR 路径先由公共 status-server 流程调用 `prepareCRRCheckpointStatusServer`：创建服务及 cleanup，状态写入命令 context，注册器捕获同一 `Arc`。之后 `RunE` 经 `getCRRCheckpointServiceState` 取回状态并调用 `Run`，最后通过 `Option::take` 至多执行一次 cleanup。

RustCodeGraph 的精确 flow 显示 `newOperatorCommand -> newCRRCheckpointCommand -> prepareCRRCheckpointStatusServer/getCRRCheckpointServiceState`；其 blast-radius 还显示所有叶子构造器只由 `newOperatorCommand` 调用，而 `newOperatorCommand` 的现有 Rust 测试调用者是 `br/cmd/br/parity_test.rs::contract_normal_command_tree_and_filters`。

## 数据与状态

普通子命令的配置均为回调内局部值，解析完成后按值或引用交给 task-operator；本文件不缓存迁移、checksum 或存储探测结果。三个 checksum 命令短暂持有 `tidbGlue()` 的 `MutexGuard`，直至下游同步调用返回。

CRR 是唯一跨阶段状态：`Arc` 允许运行回调与 status registrar 共享服务；`Mutex<Option<cleanupFunc>>` 把 `FnOnce` 清理动作变成可共享且至多取得一次的资源；`AtomicBool` 以 `SeqCst` 记录 `Run` 是否调用，但当前文件没有读取该标记，主要供迁移期观测。写入新 context 前会复用 `cmd.Context()`，没有上游 context 时才使用 `Context::Background()`，因此原有取消标记和值链不应被准备阶段丢弃。

## 依赖与调用关系

上游入口是 `br/cmd/br/main.rs::main`；测试入口是 `br/cmd/br/parity_test.rs`。CRR status 接线调用 `br/cmd/br/cmd.rs::registerStatusServerPreparer`，后续由同文件的 `prepareStatusServer` 按命令 id 找到准备器。CLI 类型、flag 集、context、mux、错误和 glue 来自 `crate::stubs::*`，公共上下文与初始化来自 `crate::cmd`。

业务下游集中在 `astersql_br_pkg_task_operator`：配置类型来自 `br/pkg/task/operator/config.rs`，执行函数分别位于 `prepare_snap.rs`、`base64ify.rs`、`list_migration.rs`、`migrate_to.rs`、`force_flush.rs`、`checksum_table.rs`、`crr_checkpoint.rs` 与 `test_storage.rs`。`br/pkg/task/operator/lib.rs` 对这些模块进行平铺导出。过滤 flag 则来自 `astersql_br_pkg_task::DefineFilterFlags`。

crate 边界由 `br/cmd/br/Cargo.toml` 证明：它是带 `lib.rs` 的二进制 crate，bin 入口为 `bin_main.rs`，对上述两个本地 crate 使用 workspace path 依赖；该 manifest 注释说明 arm64 Darwin 路径刻意使用 slim BR crates 与本地替身，进一步限定了本文件当前能力的解释范围。

## 错误处理与边界

所有配置解析错误与 task-operator 错误都转换为本 crate 的 `Error` 并从 `RunE` 返回，没有在 CLI 层吞掉或重试。所有叶子命令设置 `no_args: true`；对象选择类 checksum 默认 `!*.*`，要求调用者显式选择范围，避免默认扫描全库。`unsafe-migrate-to` 名称、警告文案和隐藏属性共同表达其高风险边界。

CRR 准备阶段的 flag 解析或服务创建失败会阻止状态写入和 registrar 返回；运行阶段分别区分“没有 context”“没有准备状态/类型不匹配”。cleanup 在 `state.Run()` 返回后执行，覆盖成功和普通错误结果，但 `cleanup.lock().unwrap()` 或 `tidbGlue().lock().unwrap()` 遇到 poisoned mutex 会 panic，当前没有恢复策略；若 `Run` 自身 panic，显式 cleanup 也不会像 RAII guard 那样保证执行。

当前 Rust 与 Go 的 context 传播并不完全一致：`list-migrations`、`migrate-to`、三个 checksum、`force-flush` 和 `test-storage` 虽调用 `GetDefaultContext()`，但局部变量是 `_ctx` 且下游 Rust 签名不接收它；因此这些路径当前不能据此声称支持 Go 等价的取消传播。CRR 的 `CrrCheckpointServiceState::Run` 也只验证空任务名而非阻塞运行真实服务。

## 并发与资源生命周期

命令树构造与普通 `RunE` 均是同步调用。快照准备业务的等待、取消和清理由 `AdaptEnvForSnapshotBackup` 管理；其独立测试在 `br/pkg/task/operator/parity_test.rs::contract_resource_cleanup` 验证取消发生前 `OnAllReady`、取消后 `OnExit`。force-flush 的 PD 与 store-manager 关闭也在该测试中验证，生命周期不由本 CLI 文件自行实现。

CRR 服务由 `Arc` 在运行端和 registrar 闭包之间共享；cleanup 被 `Mutex<Option<_>>` 保护并由 `take()` 保证运行端最多调用一次。task-operator 的 `NewCRRCheckpointService` 在成功时返回一次性 cleanup，在失败路径按已打开资源关闭 storage、manager 与 etcd；相关实现位于 `br/pkg/task/operator/crr_checkpoint.rs`。不过 registrar 持有的 `Arc` 可能延长服务状态对象寿命，而资源清理取决于 `RunE` 被执行到 cleanup 段；当前文件没有 Drop 后备清理。

## 与 Go 版本的对应关系

Rust 命令名、帮助文本、子命令顺序、flag 定义入口、`!*.*` checksum 默认过滤、顶层隐藏属性以及新旧快照准备别名均逐项对应 `br/cmd/br/operator.go`。Go 的 `crrCheckpointServiceState` 保存 service 与 cleanup，准备器继承/创建 context 并写入状态，运行时 `defer state.cleanup()`；Rust 用 `Arc<CrrCheckpointServiceState>`、数值 context key 和 `Mutex<Option<FnOnce>>` 表达相同两阶段与单次清理意图。

仍有明确差异：Go `service.Run(GetDefaultContext())` 是真实长运行服务，registrar 直接使用 `svc.Register`；Rust `Run/Register` 是本文件内的 slim stub。Go 的多数 operator 执行函数显式接收 context，Rust 只有快照准备和 base64 路径真正向下传递取消标记。Go 的 context key 是私有零大小类型，Rust 替身 API 只支持 `u64`，故使用 `CRR_STATE_KEY`。这些差异是当前迁移状态，不应在文档或后续测试中被掩盖。

## 扩展指南

新增普通 operator 子命令时，应在本文件增加独立构造器，在其中完成配置默认值、`ParseFromFlags`、业务函数转发与 `DefineFlagsFor*`，再只在 `newOperatorCommand` 注册一次；业务逻辑应进入 `br/pkg/task/operator` 的独立源文件，测试也放在该目录的独立 `*_test.rs`，不要嵌入生产文件。同步更新 Go 对照时应核对命令名、隐藏性、无位置参数约束、默认过滤和错误传播，而非只让命令出现在树中。

扩展 checksum 路径需确认 glue 锁持有范围、过滤默认值和潜在全库扫描成本。扩展 CRR 状态时必须保持 context 继承、单实例复用、cleanup 恰好一次及失败资源回收；若替换 slim service，应优先消除本地 `Run/Register` 替身并补充 status 准备失败、缺 context、错误后清理和并发访问的独立测试。若为现有无 context 的 Rust 下游补取消能力，应同时修改 task-operator API 与相应 parity/资源生命周期测试，不能只保留未使用的 `_ctx`。

现有最近测试包括：`br/cmd/br/parity_test.rs::contract_normal_command_tree_and_filters`（operator 顶级隐藏性与根命令位置）、`operator_context_observes_command_cancellation`（取消标记桥接）；下游 `br/pkg/task/operator/parity_test.rs` 覆盖配置、错误与清理契约；`crr_checkpoint_test.rs` 覆盖必需 flag、非日志备份目录、resume 与 lock 文件；`base64ify_test.rs`、`test_storage_test.rs` 覆盖相应业务。新增 CLI 结构契约宜继续放在 `br/cmd/br/parity_test.rs` 或新建同目录独立测试文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter br/cmd/br/operator.rs` 报告 17 个符号；`node --file ... --offset 1 --limit 500` 读取完整 420 行源码；精确 `explore` 给出 `newOperatorCommand -> newCRRCheckpointCommand -> prepareCRRCheckpointStatusServer/getCRRCheckpointServiceState` 及各叶子构造器调用关系。直接 `callers/callees` 在本地后端超时且无输出，因此未将其作为额外证据。
- 源与入口：`br/cmd/br/operator.rs`、`br/cmd/br/lib.rs`、`br/cmd/br/main.rs`、`br/cmd/br/cmd.rs`、`br/cmd/br/stubs.rs`。
- crate 与业务边界：`br/cmd/br/Cargo.toml`、`br/pkg/task/operator/lib.rs`、`config.rs`、`crr_checkpoint.rs` 及各 `Run*` 所在模块。
- Go 对照：`br/cmd/br/operator.go`。
- 测试证据：`br/cmd/br/parity_test.rs`、`br/pkg/task/operator/parity_test.rs`、`br/pkg/task/operator/crr_checkpoint_test.rs`；搜索确认 `br/cmd/br` 下没有同名 `operator_test.rs`，当前直接 CLI 契约集中在 `parity_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务规定的 11 章节结构检查，并人工复核以上符号、路径和已声明迁移限制。
