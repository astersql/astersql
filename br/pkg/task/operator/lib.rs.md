# `br/pkg/task/operator/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-task-operator` 的 crate 根文件。`br/pkg/task/operator/Cargo.toml` 通过 `[lib] path = "lib.rs"` 明确了这一边界，并用 `package.metadata.porting.go-package = "br/pkg/task/operator"` 记录其 Go 对照包。它位于 BR 运维工具链的业务层：上游 `br/cmd/br/operator.rs` 负责构造隐藏的 `br operator` 命令树、解析命令行并传入 Glue/Context；本 crate 的各子模块负责配置、存储探测、校验、迁移、强制刷新和快照准备等实际操作。

该文件是模块聚合与兼容门面，不实现具体算法，也没有自己的函数、类型、常量或运行时分支。RustCodeGraph 对该文件只识别到一个文件级节点；真正的实现位于它声明的同目录子模块中。

## 核心职责

1. 用 `#[path = "..."] pub mod ...` 把 `stubs`、`config`、`base64ify`、`checksum_table`、`crr_checkpoint`、`force_flush`、`list_migration`、`migrate_to`、`prepare_snap` 和 `test_storage` 组成一个 crate。
2. 先声明 `stubs` 和 `config`，使后续模块能共享本地抽象、错误类型、配置结构和 flag 定义。这里的先后顺序主要体现依赖阅读顺序；Rust 名称解析并不把声明顺序当作运行顺序。
3. 通过九组 `pub use <module>::*` 将除 `stubs` 以外的子模块公开项平铺到 crate 根，让 CLI 能直接导入 `RunForceFlush`、`RunChecksumTable`、`Base64ifyConfig` 等名称；`stubs` 则保留在公开命名空间 `astersql_br_pkg_task_operator::stubs` 下。
4. 仅在测试构建中挂载 `parity_test.rs`、`crr_checkpoint_test.rs`、`base64ify_test.rs`、`test_storage_test.rs`，保持测试逻辑与生产源文件分离。

需要特别注意：`test_storage` 虽然名称含 `test`，但它是生产 CLI 的外部存储能力探测命令，不能当作测试模块删除；真正的测试文件由 `#[cfg(test)]` 控制。

## 主要符号

本文件没有自定义 Rust 符号，公开契约来自模块声明和通配再导出：

- `pub mod stubs`：公开本 crate 的适配层，包括统一 `Result<T>`/`Error`、可取消 `Context`、`Glue`、`ExternalStorage`、`PDClient`、内存替身及 CRR/checksum 所需模型。它没有被 `pub use stubs::*` 平铺，因此调用者必须显式使用 `...::stubs::Context` 等路径。
- `pub mod config` 与 `pub use config::*`：导出 `PauseGcConfig`、`Base64ifyConfig`、`ListMigrationConfig`、`MigrateToConfig`、`ForceFlushConfig`、`CRRCheckpointConfig`、三类 checksum 配置，以及对应 `DefineFlagsFor...` 函数。
- `pub mod base64ify`：平铺导出 `Base64ify(Context, Base64ifyConfig) -> Result<()>`，负责将存储后端配置编码成兼容 Go `backuppb.StorageBackend` 线格式的 base64。
- `pub mod checksum_table`：平铺导出 `RunChecksumTable`、`RunUpstreamChecksumTable`、`RunPitrChecksumTable` 和结果/测试辅助类型，承接 rewrite rule、上游和 PITR id-map 三种 checksum 路径。
- `pub mod crr_checkpoint`：平铺导出 `NewCRRCheckpointService`、`cleanupFunc`、外部存储校验、对象同步检查器及 etcd 配置辅助。
- `pub mod force_flush`、`list_migration`、`migrate_to`：分别平铺导出 `RunForceFlush`、`RunListMigrations`、`RunMigrateTo` 等运维入口。
- `pub mod prepare_snap`：平铺导出 `AdaptEnvForSnapshotBackup` 及 PD/store-manager 连接辅助。
- `pub mod test_storage`：平铺导出 `TestStorageConfig`、`DefineFlagsForTestStorageConfig`、`RunTestStorage` 和报告类型；这是用户可调用的生产探测功能。
- 四个私有测试模块只在 `cfg(test)` 生效，不进入普通库构建的公开 API。

## 执行流程

该门面文件本身没有运行时执行流程；它在编译期确定命令业务层的可见结构。完整应用中的典型路径如下：

1. `br/cmd/br/main.rs` 调用 `newOperatorCommand`，后者在 `br/cmd/br/operator.rs` 注册 `prepare-for-snapshot-backup`、`base64ify`、`list-migrations`、`unsafe-migrate-to`、`force-flush`、`crr-checkpoint`、三类 checksum 与 `test-storage` 子命令。
2. `br/cmd/br/operator.rs` 从 crate 根导入本文件平铺出的配置类型、flag 定义与 `Run...`/`Base64ify`/`AdaptEnv...` 入口；CLI 回调解析 flag、构造 Context 或 Glue，再调用相应子模块。
3. 子模块使用 `stubs` 中的统一抽象访问外部存储、PD、元数据或 Glue，并将 `stubs::Result` 返回 CLI；CLI 通过 `br/cmd/br/stubs.rs` 的 `From<operator::stubs::Error>` 转换成命令层错误。
4. CRR checkpoint 是较特殊的两阶段路径：CLI 的 status-server 准备回调调用 `NewCRRCheckpointService` 并保存 `cleanupFunc`，运行回调执行服务后只调用一次 cleanup。门面仅负责使这两个符号可从 crate 根导入。
5. 测试构建时，四个 `#[cfg(test)]` 模块被加入 crate，可直接以 `crate::config::*`、`crate::force_flush::*` 等路径验证子模块和根级契约；生产构建不包含这些测试模块。

## 数据与状态

`lib.rs` 不拥有可变状态、缓存、全局句柄或持久化数据。它影响的是编译期命名空间：

- `pub mod` 同时暴露模块命名空间，因此调用者既可走 `crate::force_flush::RunForceFlush`，也可通过通配再导出走 `crate::RunForceFlush`。
- 平铺导出会把子模块的所有公开项纳入 crate 根 API；新增公开名称可能造成重名或扩大兼容面，修改时必须检查整个 crate 的导出集合。
- `stubs` 有意不平铺。其 `DIAL_HOOKS`、内存存储和各类 trait 的状态生命周期属于 `stubs.rs`，不是本文件创建或管理的状态。
- `test_storage` 的报告、随机数据和存储句柄由 `test_storage.rs` 管理；名称中的 `test` 不改变其生产模块身份。

## 依赖与调用关系

直接上游证据是 `br/cmd/br/Cargo.toml` 对 `astersql-br-pkg-task-operator` 的路径依赖，以及 `br/cmd/br/operator.rs` 的根级导入。后者调用所有主要业务入口；`br/cmd/br/stubs.rs` 还直接依赖公开的 `stubs` 模块来桥接 Context、Glue 和错误。RustCodeGraph 的调用结果显示 `newOperatorCommand` 组装各 `new...Command`，这些命令再调用 `Base64ify`、`RunListMigrations`、`RunMigrateTo`、`RunForceFlush`、checksum 入口和 `NewCRRCheckpointService`。

crate 的直接外部依赖由 `br/pkg/task/operator/Cargo.toml` 限定为：本仓库 `astersql-metaservice`、`astersql-objstore`，以及 `base64`、`regex`、`serde`、`serde_json`、`uuid`。注释明确当前 arm64 Darwin 版本不直接引入 kv/domain/kvproto/grpcio，而以本地 trait/stub 建模边界。因此，不能把这些抽象描述成已连接全部 Go 后端的完整实现。

下游依赖被各子模块消费：例如 base64 编码依赖 `base64`/对象存储后端解析，CRR 使用 metaservice、外部存储和同步检查器，checksum 依赖 Glue/KV 抽象，测试与报告序列化使用 `serde_json`。本文件只汇总模块，不直接调用这些依赖。

## 错误处理与边界

本文件没有 `Result` 返回点，也不捕获或改写错误。错误边界由子模块统一落在 `stubs::Error`/`stubs::Result<T>`，再由 `br/cmd/br/stubs.rs` 转换为 CLI 错误。通配再导出会把各入口的原始错误契约暴露给上游，因此门面层不应加入吞错、自动回退或错误文案改写。

现有独立测试给出了真实边界：`base64ify_test.rs` 验证未知存储 scheme 必须失败并核对 protobuf 线格式；`crr_checkpoint_test.rs` 验证缺少 downstream flag、非日志备份目录、上下游 `LockFile`、对象同步能力与 etcd 配置；`test_storage_test.rs` 验证随机数据生成的 I/O 错误保留上下文；`parity_test.rs` 汇总正常配置、迁移/CRR 边界、错误路径和资源清理。需要真实 etcd 的 `crr_dial_writes_to_keyspace_metadata_group` 被显式 `#[ignore]`，不能将其视作默认离线测试已经覆盖。

门面的另一边界是 API 冲突：任何子模块新增 `pub` 项都会因 `pub use ...::*` 自动进入 crate 根。新增前应搜索同名导出，避免编译期歧义或无意形成稳定公共 API。

## 并发与资源生命周期

`lib.rs` 不创建线程、异步任务、锁、通道或网络连接，模块声明和再导出也不改变资源生命周期。生命周期约束来自被导出的实现：

- `stubs` 中的 `ExternalStorage`、`PDClient`、`Glue` 等 trait 带 `Send + Sync`，使运维入口能够跨共享所有权边界使用实现对象。
- CRR 的 `cleanupFunc = Box<dyn FnOnce() + Send>` 表达一次性清理语义；`br/cmd/br/operator.rs` 将其保存在互斥保护的 `Option` 中并以 `take()` 保证至多调用一次。
- `parity_test.rs` 明确要求使用后清理全局 `DIAL_HOOKS`，否则会污染并行测试；这一约束属于被门面暴露的测试注入设施。
- `test_storage` 和 CRR 中的存储、reader/writer、PD/etcd 客户端由对应子模块创建和关闭。扩展门面时不能假设 `pub use` 会替调用者执行清理。

因此，这个文件的安全原则是保持“零运行时副作用”：只组织 API，不在 crate 初始化阶段启动任务或持有资源。

## 与 Go 版本的对应关系

Go 目录 `br/pkg/task/operator` 以同一个 `package operator` 跨文件自然合并命名空间，不需要 `lib.go`。Rust 无对应的 Go 单文件，因此 `lib.rs` 的作用是模拟 Go 包边界：每个 Go 文件对应同名 Rust 模块，再通过 `pub use` 恢复调用方熟悉的扁平包 API。`Cargo.toml` 的 `go-package` 元数据和文件头注释都明确记录了该关系。

主要文件对照为 `base64ify.go`/`.rs`、`checksum_table.go`/`.rs`、`config.go`/`.rs`、`crr_checkpoint.go`/`.rs`、`force_flush.go`/`.rs`、`list_migration.go`/`.rs`、`migrate_to.go`/`.rs`、`prepare_snap.go`/`.rs`、`test_storage.go`/`.rs`。Go 侧没有 `stubs.go` 的一一对应物；Rust 的 `stubs.rs` 是当前移植为了隔离缺失或平台受限依赖而建立的本地契约层。

Rust 与 Go 的重要差异必须保留在认知中：当前 Cargo 注释指出不接入 kv/domain/kvproto/grpcio；CRR 测试也说明 Rust 配置不持有 Go 式 context、用计数表示部分 dial options，并用 `MemStorage`/`MemGlue` 隔离网络边界。故“模块和公共契约对齐”不等于所有生产后端均已完整移植。`parity_test.rs` 和各专门测试是判断已对齐行为的直接证据，不能仅凭同名文件断言等价。

## 扩展指南

- 新增 Go operator 功能时，先在独立的 `<feature>.rs` 中实现，并在独立 `<feature>_test.rs` 中测试；不要把测试逻辑写入 `lib.rs` 或生产实现文件。
- 在本文件加入 `#[path = "<feature>.rs"] pub mod <feature>;`。若 CLI 需要根级 API，再决定是否加入 `pub use <feature>::*`；加入前用代码搜索检查与现有通配导出的名称冲突。
- 在 `br/cmd/br/Cargo.toml` 已有路径依赖的前提下，CLI 接线通常位于 `br/cmd/br/operator.rs`：增加配置解析、flag 定义和明确的 Context/Glue 传递，同时在 `br/cmd/br/parity_test.rs` 验证命令树。不要把 CLI 状态或资源清理塞进门面。
- 若增加外部依赖，必须更新 `br/pkg/task/operator/Cargo.toml` 并重新评估 arm64 Darwin 的本地 trait/stub 边界；不能仅靠导出一个桩就宣称生产能力已接线。
- 若修改 `stubs` 公开抽象，需同步检查 `br/cmd/br/stubs.rs` 的桥接实现以及 operator 的 parity/专门测试。涉及清理钩子、全局 hook 或资源句柄时，必须保持一次清理和测试隔离。
- Go 语义扩展应对照同路径 `.go` 与 `*_test.go`，并把 Rust 回归测试放在独立文件；当前相关 Rust 测试入口是 `parity_test.rs`、`crr_checkpoint_test.rs`、`base64ify_test.rs`、`test_storage_test.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/task/operator` 确认该目录的 25 个 Go/Rust 文件；`node --file br/pkg/task/operator/lib.rs --offset 1 --limit 240` 确认本文件 77 行模块/再导出结构；`explore "astersql_br_pkg_task_operator public exports ..."` 给出 CLI 构造函数、业务入口和测试调用关系。
- crate 与上游：读取 `br/pkg/task/operator/Cargo.toml`、`br/cmd/br/Cargo.toml`、`br/cmd/br/operator.rs`、`br/cmd/br/stubs.rs` 的引用结果，确认 crate 边界、直接依赖、CLI 根级导入与错误/抽象桥接。
- Go 对照：检索并核对 `br/pkg/task/operator/{base64ify,checksum_table,config,crr_checkpoint,force_flush,list_migration,migrate_to,prepare_snap,test_storage}.go`；该目录不存在与 `lib.rs` 一一对应的 Go 文件，Go 通过同包文件天然共享命名空间。
- 测试证据：读取 `br/pkg/task/operator/parity_test.rs`、`crr_checkpoint_test.rs`、`base64ify_test.rs`、`test_storage_test.rs`，并定位 Go 的 `crr_checkpoint_test.go`；确认测试独立挂载、边界条件、资源清理及真实 etcd 用例的忽略条件。
- 本任务只新增说明文档，不修改运行时代码，按计划不运行 Cargo。交付前使用任务给定命令验证文档存在且恰好具有 11 个固定二级标题，并人工复核本文件只承担编译期聚合、未把预期能力写成当前事实。
