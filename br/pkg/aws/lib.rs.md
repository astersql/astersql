# `br/pkg/aws/lib.rs`

## 文件定位

[`br/pkg/aws/lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-aws` 的 crate 根，而不是 AWS 业务实现本身。`br/pkg/aws/Cargo.toml` 的 `[lib] path = "lib.rs"` 把它指定为库入口，根 `Cargo.toml` 又把 `br/pkg/aws` 纳入 workspace。该入口通过 `#[path = "ebs.rs"] pub mod ebs` 装配唯一的生产模块，并通过 `pub use ebs::*` 给下游提供扁平的 crate-root API。

该 crate 位于 BR 云盘备份/恢复链路的 AWS 适配层。直接可见的 Rust 消费者是 `br/pkg/task`：其 `Cargo.toml` 以路径依赖引入本 crate，`backup_ebs.rs` 使用根级 `EBSBasedBRMeta`，`restore_ebs_meta.rs` 使用根级 `EBSBasedBRMeta`、`Progress` 和 `NewEC2Session`。因此本文件的实际职责是稳定公开路径 `astersql_br_pkg_aws::<符号>`，真实 AWS 调用、重试、轮询和并发逻辑均在 `br/pkg/aws/ebs.rs`。

## 核心职责

1. 用 `pub mod ebs` 声明并公开 EBS 实现模块，使使用者既可走 `astersql_br_pkg_aws::ebs::...`，也可走 crate 根。
2. 用 `pub use ebs::*` 重导出 `ebs.rs` 的全部公开项，形成与 Go 包级符号接近的平坦接口。该重导出包括 `EC2Session`、`NewEC2Session`、`Progress`、`EBSBasedBRMeta`、AWS 请求/响应视图、客户端 trait 和各类状态类型。
3. 仅在 `cfg(test)` 下用显式 `#[path]` 挂载 `parity_test.rs` 与 `ebs_test.rs`。测试逻辑保持在独立文件中，不进入普通库构建，也不与生产源文件混放。
4. 在 crate 级放宽迁移期命名和未使用告警。`dead_code`、`non_snake_case`、`non_camel_case_types`、`non_upper_case_globals`、`unused_imports`、`unused_variables` 与 Go 风格公开名及尚在迁移中的接口形状有关；它们会作用于整个 crate，新增代码不能据此忽略真实缺陷。

## 主要符号

- `pub mod ebs`：公开子模块，源码固定映射到 `br/pkg/aws/ebs.rs`。它是唯一生产模块声明。
- `mod parity_test`：仅测试构建可见的包级契约测试模块，源码为 `br/pkg/aws/parity_test.rs`，不对外公开。
- `mod ebs_test`：仅测试构建可见的 Go 对齐细节测试模块，源码为 `br/pkg/aws/ebs_test.rs`，不对外公开。
- `pub use ebs::*`：本文件最关键的公共接口决策。它把 `ebs.rs` 中的公开项提升到 crate 根；例如下游可直接导入 `NewEC2Session`、`EC2Session`、`Progress`、`EBSBasedBRMeta`，无需写 `ebs::`。

本文件自身不定义常量、结构体、枚举、trait、函数或 `impl`。公开业务符号的 canonical 定义都在 `br/pkg/aws/ebs.rs`，其中核心入口是 `NewEC2Session(concurrency, region) -> Result<EC2Session>`；`EC2Session` 再提供 `CreateSnapshots`、`WaitSnapshotsCreated`、`DeleteSnapshots`、`EnableDataFSR`、`DisableDataFSR`、`CreateVolumes`、`WaitVolumesCreated`、`DeleteVolumes` 和 `HandleDescribeVolumesResponse`。

## 执行流程

普通库构建时，编译器先以本文件建立 crate 根，再按 `#[path = "ebs.rs"]` 编译 `ebs` 模块，最后把其公开项重导出到根命名空间；两个测试模块因 `cfg(test)` 为假而不参与编译。本文件没有运行时语句、初始化钩子或全局构造过程。

测试构建时，流程额外包含 `parity_test.rs` 和 `ebs_test.rs`。两者可以通过 `crate::{...}` 使用根级重导出；尤其 `parity_test.rs` 明确从 crate 根导入 `CloudWatchClient`、`Ec2Client`、`EC2Session`、`NewEC2Session` 等符号，因此同时验证了实现行为和本文件的导出接线。

应用运行时的实际链路不经过本文件中的函数调用，而是经公开路径解析到 `ebs.rs`：例如 `br/pkg/task/restore_ebs_meta.rs` 调用 `astersql_br_pkg_aws::NewEC2Session`，构造带 EC2/CloudWatch 客户端的 `EC2Session`，再由任务层驱动恢复操作。备份侧 `br/pkg/task/backup_ebs.rs` 当前直接使用本 crate 重导出的元数据类型；Go 对照链路则由 `backup_ebs.go`/`restore_ebs_meta.go` 调用 Go 包 `aws.NewEC2Session`。

## 数据与状态

本文件不持有任何运行时数据、静态可变状态或配置值。它只决定模块树、可见性和测试编译边界。

通过 `pub use ebs::*` 暴露的数据与状态由 `ebs.rs` 管理：`EC2Session` 保存并发度、注入的 `Arc<dyn Ec2Client>`、`Arc<dyn CloudWatchClient>` 以及轮询/重试定时参数；`EBSBasedBRMeta`/`EBSStore`/`EBSVolume` 表示备份元数据；`HashMap` 映射卷、快照与可用区。这里的重导出不复制或包装这些值，类型身份与 `ebs` 模块内定义完全相同。

需要区分两类编译状态：普通构建只有 `ebs`；测试构建还包含两个私有测试模块。crate 级 `allow` 属性同样是编译期策略，不是运行时开关。

## 依赖与调用关系

上游装配关系如下：根 `Cargo.toml` 的 workspace members 包含 `br/pkg/aws`；`br/pkg/aws/Cargo.toml` 指定 `lib.rs` 为库入口，并声明 Go 对照包为 `br/pkg/aws`。`br/pkg/task/Cargo.toml` 通过 `astersql-br-pkg-aws = { package = "astersql-br-pkg-aws", path = "../aws" }` 引入它。

已核对的直接 Rust 使用包括：

- `br/pkg/task/backup_ebs.rs`：`use astersql_br_pkg_aws::EBSBasedBRMeta`。
- `br/pkg/task/restore_ebs_meta.rs`：导入 `EBSBasedBRMeta` 与别名 `Progress as AwsProgress`，并调用 `astersql_br_pkg_aws::NewEC2Session`。
- `br/pkg/aws/parity_test.rs`：从 `crate` 根导入公开契约，证明 `pub use ebs::*` 是测试与外部用户共同依赖的入口。

下游实现关系只有 `br/pkg/aws/ebs.rs`。其 Cargo 外部依赖为 `aws-config`、`aws-sdk-ec2`、`aws-sdk-cloudwatch`、`aws-smithy-http-client`、`aws-types`、`hyper-rustls`、`serde`、`serde_json` 和 `tokio`；本文件没有直接 `use` 这些依赖。RustCodeGraph 对 `lib.rs` 的文件节点显示实现通过模块装配关联到 `ebs.rs`，而对 `NewEC2Session` 的精确节点显示它调用 `EC2Session::NewEC2SessionWithClients` 并构造官方 AWS 客户端。

## 错误处理与边界

本文件没有可返回错误的函数，也不捕获、转换或记录错误。错误契约来自重导出的 `ebs::Result<T> = std::result::Result<T, String>` 以及 `Ec2ApiError` 和各公开方法。

门面边界仍有重要兼容风险：删除或收窄 `pub use ebs::*` 会使现有 `astersql_br_pkg_aws::NewEC2Session` 等根级路径失效；把测试模块移出 `cfg(test)` 会把测试依赖和桩带入普通构建；改变 `#[path]` 必须同步实际文件位置。由于 glob 重导出会自动扩大公开面，向 `ebs.rs` 新增 `pub` 项也会无审查地成为 crate-root API，扩展时应检查命名冲突和长期兼容性。

行为边界由独立测试覆盖：`ebs_test.rs` 验证空 AWS ID、服务错误码/消息、快照进度解析、卷状态拆分和快照等待；`parity_test.rs` 进一步覆盖目标快照筛选、FSR、创建/等待/删除快照与卷、限流重试、错误文案和根级构造器。测试均使用注入式内存客户端，不证明真实 AWS 凭证、网络权限或服务时序。

## 并发与资源生命周期

本文件不创建线程、Tokio runtime、锁、条件变量、通道或云资源，也没有 `Drop`/关闭逻辑。它仅让这些实现通过公共 API 可达。

重导出的 `EC2Session` 生命周期由 `ebs.rs` 定义：生产构造器创建 Tokio runtime 和 AWS EC2/CloudWatch 客户端并用 `Arc` 共享；并发任务由 `WorkerPool`、线程句柄、`Mutex`/`Condvar` 和首错汇聚的 `ErrorGroup` 管理；等待方法轮询资源状态，删除方法执行尽力清理。`parity_test.rs` 通过 `NewEC2SessionWithClients` 注入线程安全内存实现，并把计时器缩短到 1 ms，避免真实 AWS 资源和长轮询。门面自身不改变这些所有权或同步语义。

## 与 Go 版本的对应关系

Go 的 `br/pkg/aws` 天然以目录包为公开边界，`ebs.go` 中的导出符号可直接由 `aws.NewEC2Session`、`aws.EC2Session` 等路径访问。Rust 需要显式 crate 根，因此本文件用 `pub mod ebs` 加 `pub use ebs::*` 模拟 Go 包级的平坦公开面；`br/pkg/aws/Cargo.toml` 的 `package.metadata.porting.go-package = "br/pkg/aws"` 明确记录了该映射。

`ebs.rs` 对照 `ebs.go` 实现 EC2/CloudWatch 会话、快照、FSR 和卷生命周期。Go 使用 SDK 具体客户端、`errgroup` 与 `util.WorkerPool`；Rust 为可测试性增加 `Ec2Client`/`CloudWatchClient` trait、同步 SDK 适配器、`ErrorGroup`/`WorkerPool`，但这些差异都位于 `ebs.rs`，本门面只负责公开它们。

测试对应关系也由本文件显式表达：`ebs_test.rs` 对齐 `ebs_test.go` 的表驱动与边界场景；`parity_test.rs` 没有单一同名 Go 文件，而是聚合 `ebs.go` 与 `ebs_test.go` 的公开契约。Go 测试中的 pending 快照用超时表达持续等待，Rust 测试通过可注入计时器和客户端验证相同可观察行为。

## 扩展指南

若只是新增 AWS EBS 行为，应优先修改 canonical 实现 `br/pkg/aws/ebs.rs`，并在独立的 `br/pkg/aws/ebs_test.rs` 或 `br/pkg/aws/parity_test.rs` 增补回归；不要把业务逻辑或测试函数写进 `lib.rs`。新公开项若声明为 `pub`，会被 glob 自动提升到 crate 根，需要同时评估：是否确实要形成稳定外部 API、是否与既有根级名称冲突、`br/pkg/task` 是否应改用该入口、Go 侧是否存在相应语义。

只有新增独立生产子模块时才应修改本文件。建议沿用显式 `#[path = "<file>.rs"] pub mod <module>;`，再明确决定是保留模块路径还是根级重导出；相应测试必须放在独立 `*_test.rs`/`parity_test.rs` 文件并用 `cfg(test)` 挂载。若收紧 crate 级 `allow`，要先确认迁移期 Go 风格名称是否仍需保留，避免把风格清理误做成 API 破坏。

兼容性重点是 crate-root 路径；正确性重点在 `ebs.rs` 与 Go 对照；性能重点在真实实现的 SDK runtime、轮询间隔和并发上限，而不是本门面。涉及生产行为的修改应同步审查 `br/pkg/aws/ebs.go`、`br/pkg/aws/ebs_test.go`、两个 Rust 测试文件及 `br/pkg/task` 调用点。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`node --file br/pkg/aws/lib.rs` 确认目标共 28 行及三个装配语句；`query/node NewEC2Session` 确认 Go/Rust 两侧定义，并确认 Rust 构造器调用 `NewEC2SessionWithClients`；对 `ebs.rs` 的文件节点核对了公开类型、trait、方法、并发辅助结构和实现依赖。
- 源码与配置：`br/pkg/aws/lib.rs`、`br/pkg/aws/ebs.rs`、`br/pkg/aws/Cargo.toml`、根 `Cargo.toml`、`br/pkg/task/Cargo.toml`、`br/pkg/task/backup_ebs.rs`、`br/pkg/task/restore_ebs_meta.rs`。
- Go 对照：`br/pkg/aws/ebs.go`、`br/pkg/aws/ebs_test.go`、`br/pkg/task/backup_ebs.go`、`br/pkg/task/restore_ebs_meta.go`。
- Rust 独立测试：`br/pkg/aws/ebs_test.rs`、`br/pkg/aws/parity_test.rs`。前者包含 `test_create_snapshots_preserves_empty_aws_ids_like_go`、`test_aws_service_error_preserves_code_and_message`、`test_ec2_session_extract_snap_progress`、`test_handle_describe_volumes_response`、`test_wait_snapshots_created`；后者的 `go_rust_public_contract_matches` 从 crate 根导入并串联公开 API。
- 本任务是纯文档分析，按计划不运行 Cargo，也不访问真实 AWS。结构验证要求目标文件存在且恰好包含上述 11 个固定二级标题；内容人工复核重点为“门面而非实现”、根级重导出、测试编译边界和直接消费者四项。
