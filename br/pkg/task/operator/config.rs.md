# `br/pkg/task/operator/config.rs`

## 文件定位

`config.rs` 是 `astersql-br-pkg-task-operator` crate 的命令配置层，源码由 [`lib.rs`](./lib.rs) 以 `pub mod config` 纳入并通过 `pub use config::*` 平铺导出。它对应 Go 文件 [`config.go`](./config.go)，位于 BR 的 operator 运维子命令与具体业务执行器之间：上游 [`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs) 注册命令及 flag、构造配置并调用 `ParseFromFlags`，下游的 `prepare_snap.rs`、`base64ify.rs`、`list_migration.rs`、`migrate_to.rs`、`force_flush.rs`、`crr_checkpoint.rs` 和 `checksum_table.rs` 消费解析结果。

crate 边界由 [`Cargo.toml`](./Cargo.toml) 声明：包名为 `astersql-br-pkg-task-operator`、库入口是 `lib.rs`，porting 元数据指向 Go 包 `br/pkg/task/operator`。该 crate 直接依赖 `regex`，并通过本 crate 的 `stubs` 门面取得公共配置、flag、错误及 CRR 配置类型；配置文件本身不执行网络、存储或 checksum 操作。

## 核心职责

本文件承担三类职责。

1. 用 `flagTableConcurrency` 至 `flagDryRun` 这 19 个字符串常量固定 CLI 的公开 flag 名。名称与 Go 常量逐项一致，改名会破坏脚本兼容性及测试夹具。
2. 为九类 operator 用途定义配置数据：快照准备、存储 URI 编码、迁移列表、迁移执行、日志备份强制 flush、CRR checkpoint，以及三种 checksum 路径各自的配置。
3. 提供对应的 `DefineFlagsFor*` 和 `ParseFromFlags`，并在配置层执行仅依赖参数的校验：`MigrateToConfig::Verify` 检查模式互斥，`ForceFlushConfig::ParseFromFlags` 编译正则，`CRRCheckpointConfig::ParseFromFlags` 检查三个必填参数。

职责边界很明确：本文件只把 `FlagSet` 转换为有类型的状态并返回错误；打开外部存储、连接 PD/etcd、修改 safepoint、flush store、执行迁移或计算 checksum 都由相邻执行器完成。

## 主要符号

- `PauseGcConfig`：组合公共 `Config`，增加 `SafePoint`、`SafePointID`、`TTL` 及两个测试/同步回调。其 `Default` 将 TTL 设为 120 秒，回调为空；`DefineFlagsForPrepareSnapBackup` 注册 `--ttl/-i` 与 `--safepoint/-t`。
- `Base64ifyConfig`：保存 `BackendOptions`、`StorageURI` 与历史拼写字段 `LoadCerd`；`DefineFlagsForBase64ifyConfig` 先注册后端公共 flag，再注册 `--storage/-s` 和 `--load-creds`。
- `ListMigrationConfig`：保存后端选项、存储 URI 与 `JSONOutput`；JSON 开关只控制结果展示。
- `MigrateToConfig`：保存后端选项、存储 URI，以及 `Recent`、`MigrateTo`、`Base` 三种目标选择和 `Yes`、`DryRun` 两个执行策略。`DefineFlagsForMigrateToConfig` 将 `Recent` 默认设为 `true`；`Verify` 拒绝 `Recent` 与非零 `MigrateTo` 并用，也拒绝 `Base` 与前两者并用。
- `ForceFlushConfig`：组合公共 `Config` 和已编译的 `regex::Regex`。默认正则为 `.*`；解析时先读取并编译 `--stores`，成功后才解析公共配置。
- `CRRCheckpointConfig`：组合公共 `Config` 与 `CRRServiceConfig`，并保存上下游存储 URI 及 `CheckSyncedFromDownstreamStorage` 策略开关。解析结束前要求 task name、upstream storage、downstream storage 均非空。
- `ChecksumWithRewriteRulesConfig`：仅组合公共 `Config`，把 `--table-concurrency` 写入其中，再解析公共 flag。
- `ChecksumWithPitrIdMapConfig`：组合 `RestoreConfig` 并增加 `ChecksumTS`；解析 table concurrency、restore TS、upstream cluster ID 与 checksum TS。
- `ChecksumUpstreamConfig`：组合 `RestoreConfig`；解析 table concurrency 与 restore TS。
- `DefineFlagsForChecksumTableConfig`、`DefineFlagsForChecksumUpstreamTableConfig`、`DefineFlagsForChecksumPitrTableConfig`：保持三条 checksum 命令的 flag 集合彼此独立；PiTR 版本额外注册 upstream cluster ID 和 checksum TS。

文件没有 trait、枚举、条件编译项或私有辅助函数；所有结构体、flag 常量、注册函数及其解析方法均为公开 API。唯一手写校验方法是 `MigrateToConfig::Verify`。

## 执行流程

共同流程由 [`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs) 证明：命令构造器调用对应 `DefineFlagsFor*` 注册 flag；`RunE` 或 CRR 的 status-server 准备阶段创建 `Default` 配置；随后调用 `ParseFromFlags`；解析成功才把配置交给业务函数，错误则立即返回。

各分支的具体流向如下。

1. 两个快照准备命令名复用 `PauseGcConfig`，解析公共配置、safepoint 与 TTL 后调用 `AdaptEnvForSnapshotBackup`。
2. `base64ify`、`list-migrations`、隐藏的 `unsafe-migrate-to` 分别构造对应配置并调用 `Base64ify`、`RunListMigrations`、`RunMigrateTo`。`RunMigrateTo` 的第一步是 `cfg.Verify()`，因此互斥模式在打开存储之前被拒绝。
3. `checksum-as`、`checksum-pitr`、`checksum-upstream` 分别使用三个配置类型和三个独立注册函数，之后进入 `RunChecksumTable`、`RunPitrChecksumTable`、`RunUpstreamChecksumTable`。
4. `force-flush` 编译 store 地址正则并解析公共配置，然后以引用传给 `RunForceFlush`。
5. `crr-checkpoint` 在 status-server 准备阶段解析配置并调用 `NewCRRCheckpointService`；命令运行阶段复用已创建的服务，而不是重复解析和初始化。

解析顺序是可观察契约。例如 `PauseGcConfig` 与 CRR 先解析公共 `Config`；`ForceFlushConfig` 则先拒绝非法正则；checksum 的嵌套 `RestoreConfig` 路径先填专用字段，再调用公共解析。扩展时不应随意交换这些步骤，因为首个暴露给用户的错误可能随之改变。

## 数据与状态

所有状态都属于一次命令配置对象，没有全局可变状态。默认值来自各结构体的 `Default`、flag 注册默认值以及 `stubs` 中公共配置的默认实现。

- 数字零具有业务含义：`MigrateTo == 0` 表示没有指定目标序号；`ChecksumTS == 0` 表示后续运行时取 PD 当前 TSO；safepoint 默认也是零。
- `MigrateToConfig::Recent` 的 flag 默认值是 `true`，但派生的 Rust `Default` 会给它 `false`。正常 CLI 路径先注册 flag 再解析，因此得到 flag 默认值；直接构造 `MigrateToConfig::default()` 的调用者若跳过解析，语义不同。
- `SafePointID` 仅供测试注入；生产路径应保持空值。`OnAllReady` 与 `OnExit` 不参与 flag 解析，也不序列化，而是由测试或调用方直接装入。
- `StoresPattern` 始终保存一个已编译正则；`Default` 用 `Regex::new(".*").unwrap()` 构造匹配全部 store 的安全常量。
- PiTR/upstream checksum 由于 Rust 用显式组合而非 Go 匿名嵌入，解析时将 `TableConcurrency` 同时写入 `RestoreConfig.TableConcurrency` 与 `RestoreConfig.Config.TableConcurrency`，保持内外层消费者看到同一值。

这些类型大多按值交给下游。`ForceFlushConfig` 按引用传递；`PauseGcConfig` 包含不可克隆的 boxed 回调，因此没有派生 `Clone`，而纯数据配置按下游需要派生了 `Clone`/`Debug`/`Default`。

## 依赖与调用关系

上游调用关系以 [`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs) 为主：其中的命令构造器是全部九个 `DefineFlagsFor*` 的生产入口，也是所有配置 `ParseFromFlags` 的 CLI 入口。[`lib.rs`](./lib.rs) 将配置 API 平铺导出，命令 crate 的 [`Cargo.toml`](../../../cmd/br/Cargo.toml) 通过路径依赖 `astersql-br-pkg-task-operator` 接入它。

直接下游关系为：

- `PauseGcConfig` → `prepare_snap.rs::AdaptEnvForSnapshotBackup`；
- `Base64ifyConfig` → `base64ify.rs::Base64ify` / `runEncode`；
- `ListMigrationConfig` → `list_migration.rs::RunListMigrations`；
- `MigrateToConfig` → `migrate_to.rs::RunMigrateTo`，并由同文件的 `getTargetVersion` 解释 Recent/Base/显式序号；
- `ForceFlushConfig` → `force_flush.rs::RunForceFlush`；
- `CRRCheckpointConfig` → `crr_checkpoint.rs::NewCRRCheckpointService`；
- 三个 checksum 配置 → `checksum_table.rs` 的普通、PiTR、upstream 三个入口。

文件内的主要下游依赖来自 `crate::stubs`：`FlagSet` 提供注册/取值接口，`Config`/`RestoreConfig`/`BackendOptions`/`CRRServiceConfig` 承载公共字段，`Error`、`Result`、`berrors::ErrInvalidArgument` 统一错误语义。外部 `regex::Regex` 只用于 store 地址模式，`std::time::Duration` 用于 safepoint TTL。Cargo 中的其他依赖由同 crate 的执行器使用，不能仅凭 crate manifest 归因给本文件。

RustCodeGraph `files` 将本文件识别为含 31 个符号的已索引文件；`explore` 报告它被 22 个文件使用，并能定位 CLI、执行器及测试引用。对这些配置符号单独执行 `callers`/`impact` 时未返回可用静态边，因此具体边以模块导出、CLI 源码和全仓引用搜索交叉核对。

## 错误处理与边界

所有解析方法都返回 crate 的 `Result<()>`，flag 取值或公共配置解析失败会短路，不继续执行业务。

- `MigrateToConfig::Verify` 使用 `ErrInvalidArgument` 并在消息中点名冲突 flag。注意 `ParseFromFlags` 本身不调用 `Verify`；当前生产入口 `RunMigrateTo` 会在任何存储操作前调用它。新的调用方也必须遵守这一前置条件。
- `ForceFlushConfig::ParseFromFlags` 把正则编译错误包装为带 `--stores` 上下文的错误；非法模式不会退化成匹配全部。
- `CRRCheckpointConfig::ParseFromFlags` 依次检查 task name、upstream storage、downstream storage，空值返回 `ErrInvalidArgument`，消息包含完整 flag 名。它只检查字符串非空；URI 合法性、lock 文件和日志备份目录身份由服务构造阶段检查。
- Base64、list migration 与 checksum 配置不在本层拒绝空 URI、零时间戳或其他业务值；其边界由下游执行器或公共 `Config` 负责。文档不能把这些未实现的校验描述为“已支持”。
- checksum 的专用字段读取使用 `Error::Trace` 保留底层错误；普通 rewrite-rules 路径直接使用 `?`，最终都阻止进入执行器。

相关 Rust 回归证据位于独立文件 [`parity_test.rs`](./parity_test.rs) 和 [`crr_checkpoint_test.rs`](./crr_checkpoint_test.rs)：前者的总入口 `go_rust_public_contract_matches` 间接运行正常、边界、错误与资源清理子检查，覆盖迁移冲突和坏正则；后者直接断言缺少 downstream storage 的错误文本。测试逻辑没有内嵌在生产文件中。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、客户端或外部存储，因此自身没有并发协议，也不拥有需要关闭的运行时资源。`FlagSet` 在解析期间以共享引用读取；配置构造完成后通常按值移交，避免解析过程中共享可变状态。

生命周期相关字段只存在于 `PauseGcConfig`：`OnAllReady` 与 `OnExit` 是 `Box<dyn Fn() + Send + Sync>`，因此可安全移入可能跨线程使用的快照准备流程，但回调的调用时机由 `prepare_snap.rs` 管理。独立 Rust parity 测试通过 channel 等待 ready、取消上下文，并断言 exit 在取消之后触发；Go 的 `tests/realtikvtest/brietest/operator_test.go::TestOperator` 进一步在真实 TiKV 场景验证 ready 后 safepoint/调度器已暂停、取消后退出回调触发并恢复资源。

CRR 配置只携带资源创建参数；服务及 cleanup 的所有权由 CLI 的 `CrrCheckpointServiceState` 管理，`Run` 后取出并执行一次 cleanup。force-flush 的 PD/store manager 关闭、迁移存储生命周期和 checksum 执行资源同样属于各自执行器，而非本配置文件。

## 与 Go 版本的对应关系

[`config.go`](./config.go) 是逐项对照基准：19 个 flag 常量、9 个 flag 注册函数、9 个配置结构体、解析顺序、默认值、互斥规则和错误文案均保持同一意图。`LoadCerd` 的历史拼写也刻意保留，避免迁移时改变公开字段契约。

主要语言映射差异如下。

- Go 通过匿名嵌入 `task.Config`、`task.RestoreConfig`、`objstore.BackendOptions` 暴露字段；Rust 使用命名组合字段，因此访问路径更显式。checksum 解析中的 table concurrency 双写是维持 Go 提升字段语义所需的 Rust 接线。
- Go 的 `func()` 回调映射为带 `Send + Sync` 约束的 boxed trait object；Go 的 `*regexp.Regexp` 映射为值拥有的 `Regex`，默认配置始终持有有效模式。
- Go `int` 的 migration 序号映射为 Rust `i32`。当前 flag/stub 与迁移结构都采用 `i32`；若未来放宽序号范围，必须同时核对 Go CLI、迁移元数据和 Rust stub，而不能只改本字段。
- Go 的错误由 `pingcap/errors` 注解；Rust 通过本地 `Error::Annotatef`/`Error::Trace` 保留错误分类或上下文。错误文本关键片段由 parity 测试约束。
- Go 测试 `br/pkg/task/operator/crr_checkpoint_test.go` 与 Rust `crr_checkpoint_test.rs` 对齐 CRR 缺参及非日志备份目录行为；Go RealTiKV 测试覆盖 `PauseGcConfig` 的真实生命周期，Rust parity 测试使用内存替身覆盖相同回调顺序。Go `pitr_test.go` 还展示 `ForceFlushConfig` 在日志备份推进场景中的实际消费方式。

当前 Rust 文件不是门面、生成代码或空桩；它是已接入 CLI 和执行器的配置实现。不过公共 flag、公共配置和外部系统接口由 `stubs.rs` 提供，故对完整生产依赖能力的判断应继续追到对应 canonical crate，不能由本文件单独推出。

## 扩展指南

新增或修改 operator 参数时，建议按以下边界操作。

1. 在本文件增加稳定的 flag 常量、配置字段、对应 `DefineFlagsFor*` 注册和 `ParseFromFlags` 读取；随后在 [`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs) 的目标命令中确认注册与解析入口都已接线。
2. 业务无关的参数互斥/必填校验放在配置层，并保持在打开网络或存储资源之前调用；需要外部 IO 才能判断的合法性留给执行器。
3. 若修改公共/restore 配置组合，特别检查字段是否需要像 `TableConcurrency` 一样同步到内外层；否则不同下游可能读取到不一致的值。
4. 保持三类 checksum flag 注册函数独立，避免 PiTR 专用参数泄漏到普通/upstream 命令。改动 flag 名、默认值、短选项或错误文本前要评估 CLI 脚本兼容性。
5. 同步更新独立 Rust 测试：通用 flag、迁移与 force-flush 契约放在 [`parity_test.rs`](./parity_test.rs)，CRR 参数放在 [`crr_checkpoint_test.rs`](./crr_checkpoint_test.rs)，base64 行为放在 [`base64ify_test.rs`](./base64ify_test.rs)；不要把测试写回 `config.rs`。涉及真实 safepoint/scheduler 生命周期时，还需核对 Go RealTiKV 测试的意图。
6. 与 Go 对齐的改动应同步检查 [`config.go`](./config.go)；若有刻意差异，需在实现和测试中写明原因。性能风险主要来自新增正则编译或昂贵解析进入热路径，但当前配置只在命令启动时解析一次；兼容风险主要来自 flag 契约、默认值和错误顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/task/operator/config.rs` 确认目标文件已索引且含 31 个符号；`explore "br/pkg/task/operator/config.rs symbols responsibilities callers callees"` 报告目标被 22 个文件使用并定位配置、CLI 与测试引用；对九个注册函数执行 `callers`、对九个配置类型执行 `impact --depth 2` 未产生可用静态边，此限制已用下列源码证据补足。
- 目标与模块证据：[`config.rs`](./config.rs) 全部 450 行、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)、[`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs) 及命令 crate 的 [`Cargo.toml`](../../../cmd/br/Cargo.toml)。目录内没有 `doc.go`，因此最近的包级职责说明来自 Rust `lib.rs` 和 Go package/file 实现。
- 下游证据：`migrate_to.rs::RunMigrateTo` 在资源创建前调用 `Verify`；`checksum_table.rs` 消费三类 checksum 配置；其余直接引用由 `base64ify.rs`、`list_migration.rs`、`force_flush.rs`、`crr_checkpoint.rs`、`prepare_snap.rs` 核对。
- Go 对照：[`config.go`](./config.go)、[`crr_checkpoint_test.go`](./crr_checkpoint_test.go)、`tests/realtikvtest/brietest/operator_test.go` 和 `tests/realtikvtest/brietest/pitr_test.go`。
- Rust 测试：[`parity_test.rs`](./parity_test.rs)、[`crr_checkpoint_test.rs`](./crr_checkpoint_test.rs)、[`base64ify_test.rs`](./base64ify_test.rs)。本任务按计划为纯文档分析，未运行 Cargo 或代码测试。
- 结构校验使用任务指定命令，要求目标文件存在且固定二级标题恰好为 11 个；交付前另以 `git diff --check` 检查文档补丁格式，并人工复核没有把配置层未执行的 IO 或校验写成现状。
