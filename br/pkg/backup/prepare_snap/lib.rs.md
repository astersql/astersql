# `br/pkg/backup/prepare_snap/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-backup-prepare-snap` 的 crate 根。`br/pkg/backup/prepare_snap/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，并用 `package.metadata.porting.go-package = "br/pkg/backup/prepare_snap"` 声明对应的 Go 包；根 `Cargo.toml` 又把该目录列为 workspace member。

这个文件本身不实现快照准备算法，而是把实现拆成 `env.rs`、`errors.rs`、`prepare.rs`、`stream.rs` 四个生产模块，并在测试构建中挂载四个独立测试文件。它因此是该 Rust crate 的命名空间和公开 API 边界，而不是运行时入口。

当前接线状态需要和 Go 侧区分：Go 生产代码 `br/pkg/task/operator/prepare_snap.go` 与 `br/pkg/task/backup_ebs.go` 会调用 Go 包的 `preparesnap.New`；仓库内其他 Rust `Cargo.toml` 没有依赖本 crate，Rust 版 `br/pkg/task/operator/prepare_snap.rs` 目前使用的是其本地 `stubs::NewPreparer`。因此本 crate 的实现和测试已经存在，但不能据此声称 Rust BR 主应用已通过该门面走入真实生产链。

## 核心职责

1. 用 `#[path = "..."]` 固定四个生产子模块的源文件位置：`env` 定义外部环境边界，`errors` 定义本地错误模型，`prepare` 实现状态机，`stream` 管理每个 store 的 PrepareSnapshot 流。
2. 用 `#[cfg(test)]` 只在测试构建中编译 `parity_test.rs`、`prepare_test.rs`、`env_test.rs`、`errors_test.rs`，保持测试逻辑与生产源文件分离。
3. 通过 `pub mod` 允许调用者选择显式模块路径，例如 `env::Env` 或 `stream::prepareStream`；再通过根级 `pub use` 提供 Go 风格的扁平入口。
4. 在 crate 根集中放宽迁移代码的命名与未使用项 lint。该 `#![allow(...)]` 覆盖整个 crate，目的是容纳 Go 风格符号（如 `New`、`convertErr`）和阶段性未接线 API；它不会改变运行时行为。

## 主要符号

- `pub mod env`：公开 `Context`、`Env`、`PrepareClient`、`Region`、`CliEnv`、`RetryAndSplitRequestEnv`、协议替身类型及请求拆分/重试辅助。`pub use env::*` 还把这些公开项全部提升到 crate 根。
- `pub mod errors`：公开本地 `Error`、`Result<T>` 及错误辅助。根级只显式重导出 `Error`、`Result`、`convertErr`、`leaseExpired`、`retryLimitExceeded`、`unsupported`；`errors::eof` 仍可通过模块路径访问，但不在根级显式列表中。
- `pub mod prepare`：承载状态机。根级只重导出构造函数 `New(Arc<dyn Env>) -> Preparer` 和类型 `Preparer`。
- `pub mod stream`：公开流层符号，但 `lib.rs` 没有执行 `pub use stream::*`；调用者需要使用 `stream::...` 路径。其主要职责是把每个 store 的响应转换成内部事件并维护 lease。
- `mod parity_test`、`mod prepare_test`、`mod env_test`、`mod errors_test`：均为私有且受 `cfg(test)` 控制，只作为 crate 内测试容器存在，不形成发布 API。

根级公开面存在一个需要注意的非对称性：`env` 被通配重导出，`errors` 和 `prepare` 只重导出选定项，`stream` 完全不重导出。新增 API 时应先判断它是否确实需要进入 crate 根，不能仅因为子模块中的项是 `pub` 就假设根级可直接访问。

## 执行流程

`lib.rs` 没有可执行语句；它在编译阶段完成模块装配。运行时主流程由它暴露的 `New` 与 `Preparer` 间接启动：

1. 调用者实现或组装 `env::Env`，再调用根级重导出的 `New`。`prepare.rs::New` 创建容量为 128 的同步事件通道，初始化 inflight/失败区间/成功区间/客户端集合，并设置默认重试与 lease 参数。
2. 调用者执行 `Preparer::DriveLoopAndWaitPrepare`。RustCodeGraph 显示该方法依次调用 `PrepareConnections`、`AdvanceState`、`WaitAndHandleNextEvent`，直到 `waitApplyFinished` 为真。
3. `PrepareConnections` 经 `Env` 获取 live stores 和流客户端；`stream::prepareStream` 先发送 `UpdateLease` 完成握手，再启动接收与续约线程。状态机加载 region、按 leader store 发送 `WaitApply`，消费 `WaitApplyDone`，并对失败区间或覆盖空洞进行退避重试。
4. 快照窗口结束后，调用者执行 `Preparer::Finalize`。它并行结束各 store 流，同时继续处理/排空事件；每条流停止后台循环、发送 `Finish` 并等待剩余响应，从而撤销 lease、恢复正常模式。

以上步骤是 `prepare.rs`、`env.rs`、`stream.rs` 的组合行为；`lib.rs` 的作用仅是让这些符号在一个 crate 边界内可达。Go 生产调用证据位于 `br/pkg/task/operator/prepare_snap.go::pauseAdminAndWaitApply` 和 `br/pkg/task/backup_ebs.go`，Rust 侧目前只有本 crate 的测试直接调用这套入口。

## 数据与状态

`lib.rs` 不声明常量、结构体或可变全局状态。它影响的是符号可见性和条件编译集合。

实际状态集中在 `prepare::Preparer`：环境对象 `Arc<dyn Env>`、按 region id 记录的 `inflightReqs`、失败区间 `failed`、按起始 key 排序的 `waitApplyDoneRegions`、重试计数/时刻、容量为 128 的事件通道、按 store id 保存的 `clients`，以及 `waitApplyFinished`。公开配置 `RetryBackoff`、`RetryLimit`、`LeaseDuration` 和连接完成钩子必须在驱动开始前设置；Go 版本也明确这些配置不是线程安全的。

`stream::prepareStream` 保存单 store 客户端、lease 时长、事件发送端、共享响应接收端、后台线程句柄和停止标志。`env.rs` 中的 `Context`、本地 `brpb`/`metapb` 类型和 trait 是为无外部 Cargo 依赖的当前移植边界提供的本地实现，并非真实 kvproto/gRPC 类型。

## 依赖与调用关系

- crate 边界：`br/pkg/backup/prepare_snap/Cargo.toml` 的 `[dependencies]` 为空；源文件仅依赖标准库和 crate 内模块。本地协议、context、重试和连接 trait 均定义在 `env.rs`。
- 内部依赖：`prepare.rs` 使用 `env::{Context, Env, PrepareClient, Region, brpb, metapb}`、`errors::{Error, Result, ...}` 与 `stream::{event, prepareStream, ...}`；`stream.rs` 使用 `env` 的客户端/协议抽象和 `errors` 的转换函数。
- RustCodeGraph 调用边：`prepare.rs::New -> Preparer`；`Preparer::DriveLoopAndWaitPrepare -> PrepareConnections -> AdvanceState/WaitAndHandleNextEvent`；`Preparer::Finalize -> prepareStream::Finalize -> stopClientLoop`。
- crate 内调用者：RustCodeGraph 将 `parity_test.rs` 和 `prepare_test.rs` 标为 `Preparer`/`New` 的调用者；`New` 的具体调用覆盖正常 prepare、错误、lease 超时、重试等测试。
- Go 生产调用者：`br/pkg/task/operator/prepare_snap.go::pauseAdminAndWaitApply` 用 `CliEnv` 外包 `RetryAndSplitRequestEnv` 后构造 `Preparer`；`br/pkg/task/backup_ebs.go` 为 EBS 备份构造 `Preparer` 并配对调用 `DriveLoopAndWaitPrepare`/`Finalize`。
- Rust 主应用边界：检索全部 Cargo manifest 只找到 workspace membership 与本 crate 自身声明，没有消费者依赖；`br/pkg/task/operator/prepare_snap.rs` 调用的是 operator crate 的 `NewPreparer` 桩。因此真实生产接线仍需单独完成，不能通过扩大本文件的 re-export 自动获得。

## 错误处理与边界

`lib.rs` 自身不产生运行时错误。它公开的统一错误面是 `errors::Error` 和 `errors::Result<T>`：错误可用 `annotate`/`annotatef` 保留原因链，`convertErr` 把可选 kvproto 错误转换为本地错误，固定辅助函数表示 lease 过期、不支持操作和重试耗尽。`errors::eof` 是流结束控制信号，只能经 `errors::eof` 访问。

下游边界包括：连接失败、初始 lease 握手返回意外类型、发送/接收失败、未知响应、lease 失效、重试次数耗尽、上下文取消及 Finalize 线程 panic。`DriveLoopAndWaitPrepare` 会给建连、首次推进和循环步进错误增加上下文；`Finalize` 返回首个流错误，并在成功结束前排空事件。

当前实现有明确的移植边界：`env.rs` 注释指出它使用本地 trait 与协议替身，避免在 arm64 拉入真实 logutil/utils/engine/kvproto 原生依赖；`Cargo.toml` 也注明 “Local stubs only”。因此文档和调用者都不应把本 crate 的测试通过解释为真实 PD/TiKV/gRPC 集成已经完成。

## 并发与资源生命周期

crate 根只决定哪些并发实现被编译，真正的生命周期由公开的 `Preparer` API 约束：

1. `New` 创建有界同步事件通道；每个 `prepareStream` 的后台接收/续约线程通过该通道向单一状态机汇报事件。
2. `DriveLoopAndWaitPrepare` 要求调用者独占可变的 `Preparer`；Go 对照还明确调用后不应跨 goroutine 共享。Rust 的 `&mut self` 在类型层面强化了这一要求。
3. 进入安全快照窗口后，lease 会持续阻止 split、ingest 和配置变更。即使 `DriveLoopAndWaitPrepare` 返回，该状态仍持续到调用 `Finalize`。
4. `Finalize` 取走 client map，并行为每个 store 结束流；主线程同时排空事件，防止后台生产者因有界通道背压而无法退出。流层先通知后台循环停止并 join，再发送 `Finish`、排空响应。
5. 上下文取消会使驱动或收尾返回错误；调用方必须把成功进入准备态与最终清理成对管理。Go 生产调用使用 `defer`/`sync.Once` 保证配对，未来 Rust 生产接线也应维持同等清理保证。

测试资源与生产资源严格分离：四个测试模块仅在 `cfg(test)` 下存在，发布构建不会包含 mock store、虚拟时钟和故障注入逻辑。

## 与 Go 版本的对应关系

Rust 文件布局逐一对应 Go 包：`env.rs` 对 `env.go`，`errors.rs` 对 `errors.go`，`prepare.rs` 对 `prepare.go`，`stream.rs` 对 `stream.go`；`lib.rs` 则承担 Go 不需要的 crate 根装配职责。

主要语义保持一致：`Env`/`PrepareClient`/`Region` 抽象、按大小拆分 `WaitApply`、连接重试、默认 60 次重试与 5 秒退避、120 秒 lease、按 store 建流、覆盖空洞检测、事件驱动推进和 Finalize 撤销 lease。Rust 用 `Arc`、`Mutex`、`std::sync::mpsc` 与线程对应 Go 的 interface、mutex、channel 与 goroutine/errgroup。

已知差异不能隐藏：Rust `New` 接收 `Arc<dyn Env>` 并返回值类型 `Preparer`，Go 返回 `*Preparer`；Rust 错误模型是本地最小实现；协议与连接也是本地替身；Rust `prepare.rs` 不实现 Go 的 zap `MarshalLogObject`；Rust operator 尚未依赖本 crate。测试注释同样强调内存 mock 只验证契约，不代表真实集群吞吐或生产接线。

Go `prepare_test.go` 覆盖 basic、失败、错误、lease 超时、重试/拆包、连接延迟、钩子和 Finalize 大量消息。Rust `prepare_test.rs` 复刻这些核心场景并补充 context 取消、TiFlash compute store 过滤、多错误保留等回归；`parity_test.rs` 聚焦公开契约及重叠 region；`env_test.rs` 与 `errors_test.rs` 分别验证环境适配和 EOF identity。

## 扩展指南

- 新增生产子模块时，在 `lib.rs` 添加明确的 `#[path] pub mod`，并评估是否需要根级重导出；不要默认扩大 `env::*` 式的公开面。
- 新增根级 API 时，同步增加独立测试文件中的可见性/行为断言，并核对 Go 包是否有同名或等价契约。若只供内部状态机使用，应保留在子模块路径下。
- 修改 `Preparer` 主流程应落在 `prepare.rs`，流协议/lease 生命周期落在 `stream.rs`，PD/TiKV/连接边界落在 `env.rs`，错误 identity/包装落在 `errors.rs`；不要把业务算法塞进门面文件。
- 接入真实 Rust BR 应新增明确的 Cargo 依赖，并把 operator/EBS 调用从本地桩切到本 crate；同时替换或适配 `env.rs` 的本地协议与连接 trait。该工作涉及真实网络、取消和清理语义，不能仅通过 re-export 完成。
- 保持测试源文件独立：主状态机变更同步更新 `prepare_test.rs` 和必要的 `parity_test.rs`；环境或流边界变更同步更新 `env_test.rs`；错误 identity 变更同步更新 `errors_test.rs`。需要真实 TiKV 的接线测试应另建集成测试，而不是把测试嵌入 `lib.rs`。
- 兼容风险主要来自根级公开面变化和 Go/Rust 错误文案/事件顺序差异；性能风险主要来自有界通道容量、请求拆包阈值、重试退避及每 store 线程数量。所有这些参数变化都应保留 Go 对照证据并添加边界测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/backup/prepare_snap` 列出目标模块及四个独立 Rust 测试；`node --file br/pkg/backup/prepare_snap/lib.rs` 核对全部 58 行门面定义。
- RustCodeGraph 符号/调用边：`node Preparer`、`node New`、`node DriveLoopAndWaitPrepare`、`node Finalize`；确认 `New` 实例化 `Preparer`，驱动方法调用 `PrepareConnections`/`AdvanceState`/`WaitAndHandleNextEvent`，收尾方法调用流的 `Finalize`，以及 Rust 调用者目前来自 crate 内测试。
- Rust 源：`br/pkg/backup/prepare_snap/{lib.rs,env.rs,errors.rs,prepare.rs,stream.rs}`；重点核对模块公开面、本地替身边界、状态字段、默认值、事件循环、lease 与 Finalize 生命周期。
- Cargo：根 `Cargo.toml` 和 `br/pkg/backup/prepare_snap/Cargo.toml`；确认 workspace membership、crate 名、`lib.rs` 入口、Go 包映射、空依赖集合及 local-stubs 注释。全仓 Cargo manifest 检索未发现其他 crate 依赖本包。
- Go 对照与生产入口：`br/pkg/backup/prepare_snap/{env.go,errors.go,prepare.go,stream.go,prepare_test.go}`、`br/pkg/task/operator/prepare_snap.go`、`br/pkg/task/backup_ebs.go`。
- Rust 独立测试：`br/pkg/backup/prepare_snap/{parity_test.rs,prepare_test.rs,env_test.rs,errors_test.rs}`；Rust operator 现状另由 `br/pkg/task/operator/prepare_snap.rs` 的 `stubs::NewPreparer` 调用验证。
- 本任务是纯文档分析，按计划不运行 Cargo。结构检查要求目标文件存在且恰好含有本页这 11 个固定二级标题。
