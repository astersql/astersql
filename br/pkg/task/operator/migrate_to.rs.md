# `br/pkg/task/operator/migrate_to.rs`

## 文件定位

本文件属于 `astersql-br-pkg-task-operator` library crate；crate 根 `br/pkg/task/operator/lib.rs` 以 `pub mod migrate_to` 挂载它，并通过 `pub use migrate_to::*` 导出其公开入口。上游命令注册位于 `br/cmd/br/operator.rs::newMigrateToCommand`：隐藏的危险命令 `unsafe-migrate-to` 解析 `MigrateToConfig` 后调用 `RunMigrateTo`。

它是 Go 文件 `br/pkg/task/operator/migrate_to.go` 的 Rust 对照实现，负责把日志备份 migration 栈合并到最近层、BASE 或显式序号。需要特别区分目标语义和当前实现能力：`br/pkg/task/operator/Cargo.toml` 注明该 crate 在 arm64 Darwin 使用“Local traits/stubs only”，而本文件实际依赖同 crate 的 `stubs.rs`；因此当前 Rust 路径是可测试的迁移期实现，不等同于 Go 版已经接入真实 `objstore`、BR stream migration 和进度设施。

## 核心职责

- `MigrateToConfig::getTargetVersion` 把 `Recent`、`Base`、`MigrateTo` 三种配置转换为合并目标序号，并对“Recent 但没有 layer”给出可跳过信号。
- `RunMigrateTo` 编排参数校验、操作上下文创建、存储打开、migration 元数据加载、交互确认以及 dry-run/实际路径选择。
- `migrateToCtx::dryRun` 在副作用记录模式下估算合并结果，展示新 BASE，把 effects 写入临时 JSON，并打印估算警告。
- `migrateToCtx::printErr` 和 `askForContinue` 封装控制台展示。当前 `RunMigrateTo` 为满足 `InteractiveCheck` 的 `'static`/共享闭包要求，内联实现了与 `askForContinue` 相同的表格确认逻辑；因此 Rust 中的 `askForContinue` 当前没有生产调用者。

## 主要符号

- `MigrateToConfig::getTargetVersion(&self, migs: &Migrations) -> (i32, bool)`：公开方法。优先级为 `Recent`、`Base`、显式 `MigrateTo`；`Recent` 取 `Layers[0].SeqNum`，空层返回 `(0, false)`，`Base` 返回 `(0, true)`。
- `migrateToCtx { cfg, console, est }`：文件私有运行时聚合。`cfg` 保存原配置，`console` 负责输出/询问，`est: MigrationExt` 承载存储、操作上下文及 dry-run/合并状态。当前 `cfg` 字段构造后未被方法读取。
- `migrateToCtx::printErr(&self, errs, msg)`：仅在警告非空时打印标题，并逐条以 `color_hi_red` 格式化。
- `migrateToCtx::askForContinue(&self, targetMig) -> bool`：把目标 migration 加入表格并调用 `PromptBool`；当前生产流程使用等价内联闭包而未调用此方法。
- `migrateToCtx::dryRun(&self, f) -> Result<()>`：克隆 `MigrationExt`，用 `MigrationExt::DryRun` 执行一次合并回调，延后传播回调错误；成功后展示 `NewBase`、保存 effects，并报告 `Warnings`。
- `RunMigrateTo(cfg: MigrateToConfig) -> Result<()>`：文件唯一公开顶层入口。源码没有条件编译项、模块级常量或 trait 定义。

## 执行流程

1. `RunMigrateTo` 首先调用 `MigrateToConfig::Verify`，拒绝 `Recent` 与显式 `--to` 同用，以及 `Base` 与 `Recent/--to` 同用。
2. 通过 `NewOperationContext("operator migrate-to")` 创建操作标识；随后 `ParseBackend` 解析 `StorageURI`，`CreateStorage(..., false)` 创建存储。
3. 构造标准控制台，以 `MigrationExtension(st).WithOperationContext(...)` 创建 `MigrationExt`，调用 `NewProgressBarHooks`，再以 `Load(false)` 加载 migration 元数据。`false` 令缺少 `migrations.json` 时得到默认空集合而非错误。
4. `getTargetVersion` 解析目标。若选择 `Recent` 但没有 layer，则打印跳过消息并成功返回，不执行确认或合并。
5. 构造共享 `InteractiveCheck`。`cfg.Yes` 为真时直接允许；否则展示目标 migration 表格并询问 `Continue?`。
6. `cfg.DryRun` 为真时，`migrateToCtx::dryRun` 调用 `MergeAndMigrateTo` 的临时副本路径，输出估算的新 BASE、临时 effects 文件和警告；否则直接调用 `est.MergeAndMigrateTo`。
7. 非 dry-run 合并返回的 `Warnings` 不改变成功结果，只提示可重新执行；真正的 `Err` 通过 `?` 返回。

在当前 `stubs.rs` 中，`MergeAndMigrateTo` 会再次 `Load(false)`，按目标序号选择 BASE/layer（找不到显式层时生成名为 `seq-{target}` 的占位 migration），执行确认回调，然后把 `merge target=...` 记入共享 effects 并返回注入结果或默认结果。它没有把合并后的 migration 元数据写回存储；这是当前 Rust 桩的边界，不应解读为真实迁移已完成。

## 数据与状态

- 输入配置 `MigrateToConfig` 来自 `config.rs`，包含 `StorageURI`、`BackendOptions`、`Recent`、`MigrateTo: i32`、`Base`、`Yes`、`DryRun`。配置会克隆进 `migrateToCtx`，闭包另捕获 `Yes`、控制台和 `MigrationExt` 克隆。
- `Migrations` 由 `Base: Migration` 和有序的 `Layers: Vec<MigrationLayer>` 组成。本文件假定 `Layers[0]` 是“最近”层；排序责任不在本文件。
- `MergeAndMigratedTo` 包含 `NewBase` 与 `Warnings`。警告属于可报告、可重试的部分失败，不自动升级为函数错误。
- dry-run 的局部状态包括 `runErr: Option<Error>`、默认 `estBase` 和 `effects`。回调不能直接返回错误，因此先把错误存入 `runErr`，`DryRun` 结束后再用 `Error::Trace` 返回。
- `MigrationExt` 的当前桩用 `Arc<Mutex<...>>` 共享 effects、注入错误和注入结果；clone 共享这些内部状态而非复制快照。

## 依赖与调用关系

上游主链为 `br/cmd/br/operator.rs::newMigrateToCommand` → crate 根再导出的 `RunMigrateTo`。测试上游为 `br/pkg/task/operator/parity_test.rs`，直接导入 `crate::migrate_to::RunMigrateTo`，并调用 `getTargetVersion`。

下游均由 `crate::config` 与 `crate::stubs` 提供：配置校验来自 `MigrateToConfig::Verify`；存储链为 `ParseBackend` → `CreateStorage`；migration 链为 `MigrationExtension` → `WithOperationContext` → `Load` / `DryRun` / `MergeAndMigrateTo`；展示链为 `ConsoleOperations`、`AddMigrationToTable`、颜色函数与 `SaveJSONEffectsToTmp`。标准库 `Arc` 只用于构造可共享、可跨边界持有的 `InteractiveCheck`。

Cargo 清单没有直接引入 Go 版所用的 kvproto、真实 BR stream 或云对象存储 SDK；本文件的相关类型全部来自本地桩。`SaveJSONEffectsToTmp` 间接使用清单中的 `serde_json` 和 `uuid`，而存储桩位于同一 crate。

## 错误处理与边界

- 配置冲突、空存储 URI、后端解析/创建失败、migration 读取或 JSON 解析失败、合并失败、effects 序列化/临时文件写入失败都会返回 `Err`。
- dry-run 回调错误在 `DryRun` 返回后包装为 `Error::Trace`；发生该错误时不展示默认新 BASE，也不保存 effects。
- 用户拒绝确认时，当前桩返回 `Error("migration cancelled by user")`；`--yes` 仅跳过确认，不跳过配置校验、加载或合并。
- `Recent` 空层是显式成功跳过；但 `Base` 总能得到目标 `0`。显式目标是否存在不由本文件验证，当前桩甚至会生成 `seq-{target}` 占位对象，真实 Go 下游的约束需要由 stream 实现决定。
- `Warnings` 与致命错误分离：dry-run 和实际路径都会打印警告并返回成功。调用方若需要将特定 warning 视为失败，必须修改契约而不能只改展示文本。
- 结构上存在两个维护风险：`migrateToCtx::askForContinue` 与入口内联确认重复，且 `migrateToCtx.cfg` 未使用；修改确认行为时必须同步检查两处，或先在独立变更中收敛实现。

## 并发与资源生命周期

本文件不创建线程、异步任务或通道，整个入口是同步执行。`Arc<dyn Fn + Send + Sync>` 让确认回调满足共享接口；闭包拥有 `ConsoleOperations` 与 `MigrationExt` 的 clone，生命周期覆盖 `MergeAndMigrateTo` 调用。

`MigrationExt` clone 共享 `Arc<Mutex<...>>` 状态，锁只在桩方法内短暂持有；本文件没有跨控制台 I/O 或用户确认持锁。dry-run 先清空共享 effects，再运行克隆 extension，最后复制 effects；若未来允许同一 extension 并发执行多个 dry-run/merge，这个“清空—执行—读取”复合操作没有整体锁隔离，可能互相污染，当前同步单次调用则没有该竞争。

存储由 `Arc<dyn ExternalStorage>` 持有，随 `MigrationExt` 及闭包 clone 自动释放；没有显式 close。操作上下文被存入 extension，但当前本地桩只保存 UUID，没有 Go 版 context 的取消/截止时间传播。临时 effects JSON 由 `SaveJSONEffectsToTmp` 创建后留给操作者检查，本文件不删除它。

## 与 Go 版本的对应关系

Rust 的目标选择、参数校验后的编排顺序、Recent 空层成功跳过、表格确认、dry-run 展示、warning 非致命处理与 `br/pkg/task/operator/migrate_to.go` 基本同形。Go 的 `RunMigrateTo(ctx, cfg)` 显式接收 `context.Context`；Rust 入口不接收上下文，只创建本地 `OperationContext`。Go 把 `cx.askForContinue(ctx, m)` 直接放入 `MMOptInteractiveCheck`，Rust 因接口形态改用等价内联闭包。

能力层面尚未等价。Go 使用 `objstore.ParseBackend/Create`、`stream.MigrationExtension`、`stream.NewProgressBarHooks` 和真实 `MergeAndMigrateTo`；Rust 使用 `stubs.rs`，其中 `CreateStorage` 返回 `MemStorage`，`NewProgressBarHooks` 为空实现，`OperationContext` 只有 UUID，`MergeAndMigrateTo` 只记录 effect/返回模拟结果而不持久化。Go 的调用上下文也参与 `Load`、表格生成和合并，Rust 桩接口没有这些 context 参数。扩展或宣称生产可用前，必须逐项消除这些差异，而不能只比较本文件控制流。

## 扩展指南

- 新增目标选择模式时，先修改 `config.rs::MigrateToConfig` 的解析与 `Verify`，再修改 `getTargetVersion`；同步 Go 对照语义，并在独立文件 `br/pkg/task/operator/parity_test.rs` 增加优先级、空集合、非法组合和目标边界测试。
- 改确认流程时，保持 `--yes` 短路语义以及“先展示目标、后询问”；同时处理入口闭包与当前未使用的 `askForContinue`，避免两套逻辑漂移。拒绝确认的错误契约也应有独立回归断言。
- 改 dry-run 时，保证不写真实外部存储、回调错误优先于展示结果、effects 文件写入失败可见，并覆盖 warnings 与临时文件内容。测试仍应放在独立 `*_test.rs`，不要嵌入生产源文件。
- 接入真实生产实现时，修改点主要在 `stubs` 抽象及 Cargo 依赖，而不是删除本文件中的校验/交互/警告分支。需要验证存储写入原子性、取消传播、layer 排序与目标不存在行为，并与 Go 的真实 stream tests 对齐。
- 性能上本文件只做一次元数据加载（当前桩合并内部会再次加载）和线性目标层查找；layer 数量很大或远端元数据昂贵时，应在下游实现评估缓存/索引，不能牺牲一致性校验。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7032 个 Rust 文件；`files --filter br/pkg/task/operator/migrate_to.rs` 确认目标文件含 7 个符号；`node --file ... --offset 1 --limit 400` 读取了完整 176 行；`query` 分别定位 Rust/Go 的 `getTargetVersion`、`RunMigrateTo` 和 `dryRun`。`explore` 识别到 Rust `RunMigrateTo`/`getTargetVersion`/`dryRun` 与 `parity_test.rs` 的关系。精确 `callers RunMigrateTo` 在本地索引持续无输出后被终止，调用者改由限定范围 `rg` 复核。
- 源码与接线：`br/pkg/task/operator/migrate_to.rs`、`config.rs`、`stubs.rs`、`lib.rs`、`Cargo.toml`，以及 CLI 入口 `br/cmd/br/operator.rs`。
- Go 对照：`br/pkg/task/operator/migrate_to.go`、`config.go` 与 `br/cmd/br/operator.go`。
- 独立 Rust 测试：`br/pkg/task/operator/parity_test.rs::contract_normal_config_and_helpers` 覆盖 Recent/BASE 目标；`contract_boundary_migrate_and_crr` 覆盖 Recent 空层跳过和 BASE dry-run。测试注释也明确当前 fresh `MemStorage` 限制，因此没有证明真实持久化迁移。该目录没有同名 `migrate_to_test.rs` 或 Go `migrate_to_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前只执行任务指定的 11 章节结构检查，并人工复核上述符号、调用边、桩边界与 Go 差异。
