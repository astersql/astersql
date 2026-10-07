# `pkg/executor/workloadrepo.rs`

## 文件定位

本文件位于 `astersql-executor` crate，crate 根 `pkg/executor/lib.rs` 通过 `pub mod workloadrepo` 公开该模块。它把“创建负载仓库时触发一次手工快照”抽象成一个很薄的执行器边界：文件只定义运行时协议和条件调用逻辑，不实现负载仓库的采样、落表、快照编号或后台 worker。

`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "pkg/executor"` 说明它属于执行器主 crate，并以同目录 Go 包为移植对照。当前文件没有直接使用 crate 的任何外部依赖，也没有条件编译项、模块级常量或自由函数。

## 核心职责

文件承担两项职责：

1. `WorkloadRepoRuntime` 定义执行器与实际快照设施之间的最小协议，包括判断 hook 是否安装以及执行快照。
2. `WorkloadRepoCreateExec::Next` 在拉取执行结果时检查 hook；已安装则同步调用一次 `take_snapshot`，未安装则返回成功。

它刻意不负责构建执行器和注册 hook。Rust 的计划分发位于 `pkg/executor/builder.rs`，而实际负载仓库算法位于 `pkg/util/workloadrepo/`。仓库搜索未发现 `WorkloadRepoRuntime` 的实现或 `WorkloadRepoCreateExec` 的构造点，因此本文件当前提供了可复用边界，但尚无证据证明该具体泛型执行器已经接入 Rust 执行主链。

## 主要符号

- `pub trait WorkloadRepoRuntime`：公开运行时协议。关联类型 `Context` 隔离具体会话/事务上下文，关联类型 `Error` 隔离错误体系。
- `WorkloadRepoRuntime::snapshot_hook_installed(&self) -> bool`：只读探测 hook 是否存在。返回 `false` 表示本次执行应为空操作。
- `WorkloadRepoRuntime::take_snapshot(&mut self, context: &mut Self::Context) -> Result<(), Self::Error>`：取得运行时和上下文的独占可变借用，同步执行一次快照，并原样返回运行时错误。
- `pub struct WorkloadRepoCreateExec<R: WorkloadRepoRuntime>`：公开泛型执行器，仅持有公开字段 `runtime: R`，没有额外状态。
- `WorkloadRepoCreateExec::Next<T>(&mut self, context: &mut R::Context, _request: &mut T) -> Result<(), R::Error>`：公开执行入口。泛型请求参数仅用于兼容拉取式执行器形状，函数体不读取或写入 `_request`。大写命名由文件级 `#![allow(non_snake_case)]` 显式允许，以保持 Go `Next` 命名。

## 执行流程

调用 `Next` 后的流程只有一个分支：

1. 通过 `self.runtime.snapshot_hook_installed()` 查询当前运行时。
2. 若返回 `true`，立即调用 `self.runtime.take_snapshot(context)`，并把其 `Result` 直接作为 `Next` 的结果返回。
3. 若返回 `false`，不访问上下文、不触碰请求对象，直接返回 `Ok(())`。

一次 `Next` 调用至多触发一次快照；文件没有“一生只运行一次”的标志，因此调用方若重复调用 `Next`，每次都会重新探测并可能重新快照。是否限制调用次数属于外层执行器生命周期的责任，不能从本文件推出。

在更外层，`pkg/executor/builder.rs` 将 `Plan::WorkloadRepoCreate` 分派给 `buildWorkloadRepoCreate`，后者调用 `build_leaf(ExecutorKind::WorkloadRepoCreate, plan)`。该通用构建路径没有直接引用本文件的 `WorkloadRepoCreateExec`，所以这里只能确认计划种类存在，不能确认二者已实际绑定。

## 数据与状态

唯一持久字段是 `WorkloadRepoCreateExec::runtime`。上下文由调用者以 `&mut R::Context` 临时传入，请求对象由 `&mut T` 传入但未使用；本文件不分配集合、不缓存快照结果，也不维护 snap ID、时间戳或启用状态。

hook 的存在性以及快照所需的全部状态都由 `R` 管理。先检查后执行是两个独立 trait 调用；协议没有承诺两次调用之间 hook 状态不可变化，实现者必须自行保证这组操作的语义一致性。

## 依赖与调用关系

直接下游调用边是 `WorkloadRepoCreateExec::Next → WorkloadRepoRuntime::snapshot_hook_installed`，以及安装 hook 时的 `WorkloadRepoCreateExec::Next → WorkloadRepoRuntime::take_snapshot`。RustCodeGraph 对目标文件识别出 6 个符号，并确认上述两个 trait 方法各只有该 `Next` 调用。

模块上游是 `pkg/executor/lib.rs` 的公开模块声明。仓库级精确搜索没有找到目标 trait 的实现、目标 struct 的实例化或其 `Next` 的直接调用；`pkg/executor/builder.rs` 的相邻计划接线只到 `ExecutorKind::WorkloadRepoCreate` 的通用 leaf 工厂。

Go 侧的真实下游是 `pkg/util/workloadrepo/worker.go::takeSnapshot`：其 `init` 把该函数赋给 `executor.TakeSnapshot`。该函数持有 `workerCtx` 的互斥锁，检查仓库是否启用，再调用 worker 的快照实现。Rust 文件没有复制这些 worker 细节，而是通过 trait 留给运行时实现。

## 错误处理与边界

未安装 hook 被定义为成功的空操作，不是错误。已安装时，`take_snapshot` 的 `Err(R::Error)` 不做包装、日志记录或重试，直接传播给调用者；错误类型完全由运行时决定。

本文件不验证上下文、不捕获 panic，也没有超时或取消逻辑。请求参数被忽略，因而不会向结果 chunk 写行。trait 只表达同步 `Result`；异步快照、后台排队和部分成功语义均不在当前接口内。

Go 的 `pkg/util/workloadrepo/worker.go::takeSnapshot` 进一步把“仓库未启动”和“无法启动快照”映射为专用错误并记录日志，但这是 Go hook 实现的行为，不是当前 Rust trait 的既有保证。

## 并发与资源生命周期

`Next` 需要 `&mut self`，`take_snapshot` 还需要 `&mut R::Context`，因此安全 Rust 调用期间不能并发借用同一执行器、运行时或同一上下文。文件自身没有锁、线程、异步任务、通道或显式资源清理；资源生命周期跟随 `runtime` 字段和调用者提供的上下文。

trait 没有要求 `Send`、`Sync` 或 `'static`，所以不能据此认定执行器可跨线程共享。若运行时内部存在全局 hook 或 worker，原子性、锁顺序、取消和清理都必须由具体 `WorkloadRepoRuntime` 实现负责。Go 对照实现会在 `takeSnapshot` 整段持有 `workerCtx` 锁，这一点尚未在 Rust 文件中形成等价的并发保证。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/workloadrepo.go`：

- Go 的全局变量 `TakeSnapshot func(context.Context) error` 对应 Rust trait 中“是否安装”与“执行快照”两项能力；Rust 用注入的 `runtime` 取代可空全局函数。
- Go `WorkloadRepoCreateExec` 嵌入 `exec.BaseExecutor`；Rust struct 只保存运行时，没有 base executor 字段。
- 两侧 `Next` 都忽略输出请求/chunk，在 hook 存在时同步调用并原样传播错误，不存在时成功返回。
- Go hook 由 `pkg/util/workloadrepo/worker.go::init` 自动注册；Rust 仓库当前未发现 `WorkloadRepoRuntime` 实现或等价注册接线。因此文件内分支语义与 Go 对齐，但完整应用集成仍未验证，不能宣称 Rust 已具备 Go 的端到端手工快照能力。

相关 Go 测试 `pkg/util/workloadrepo/worker_test.go` 覆盖 worker 的建表、采样和直接快照行为，但仓库搜索未发现专门调用 `WorkloadRepoCreateExec.Next` 或 SQL `CREATE WORKLOAD REPOSITORY` 的同名独立测试。Rust 的 `pkg/util/workloadrepo/worker_test.rs` 测试自身 worker 快照状态，也不构成本文件 trait/执行器的直接覆盖。

## 扩展指南

若要完成 Rust 主链接线，最可能需要：实现 `WorkloadRepoRuntime`，在执行器构建依赖中把 `ExecutorKind::WorkloadRepoCreate` 映射到 `WorkloadRepoCreateExec`，并明确上下文与统一错误类型的转换。接线时应保留 Go 的关键契约：无 hook 时成功、每次执行最多调用一次 hook、hook 错误原样使语句失败、请求 chunk 不产生结果行。

测试逻辑不要内嵌到 `workloadrepo.rs`。应在同目录独立 `workloadrepo_test.rs`（并由 `lib.rs` 在 `#[cfg(test)]` 下声明）覆盖至少四类情况：未安装时不调用快照、安装时恰好调用一次、错误传播、重复 `Next` 的调用次数语义；端到端接线还应增加 builder 测试，必要时补充 SQL 集成测试。

扩展并发或异步能力前，应先决定 hook 探测与调用是否需要原子化，以及取消、超时、重入和锁持有边界。性能风险主要来自真实快照 I/O 同步阻塞 `Next`；兼容风险主要来自改变“未安装即成功”或错误透传语义。

## 验证依据

- Rust 源码：`pkg/executor/workloadrepo.rs`，核对 trait、关联类型、泛型 struct、`Next` 分支和文件级命名许可。
- crate/模块：`pkg/executor/Cargo.toml` 与 `pkg/executor/lib.rs`，核对 crate 归属、Go 包映射和公开模块声明。
- Rust 上游接线：`pkg/executor/builder.rs` 中 `Plan::WorkloadRepoCreate`、`ExecutorKind::WorkloadRepoCreate`、`buildWorkloadRepoCreate` 与 `build_leaf` 调用。
- Go 对照：`pkg/executor/workloadrepo.go` 和 `pkg/executor/builder.go::buildWorkloadRepoCreate`。
- hook 实现及测试边界：`pkg/util/workloadrepo/worker.go::takeSnapshot`、其 `init` 注册，以及 `pkg/util/workloadrepo/worker_test.go`、`pkg/util/workloadrepo/worker_test.rs`。
- RustCodeGraph：`status` 显示索引含目标文件；`files --filter pkg/executor/workloadrepo.rs` 显示该文件有 6 个符号；文件节点和调用流确认 `Next` 对两个 trait 方法的直接调用。由于 `Next` 是高频同名符号，仓库上游结论采用文件限定节点并以精确 `rg` 搜索交叉核验。
- 测试覆盖检索：精确搜索未找到 `WorkloadRepoRuntime` 实现、`WorkloadRepoCreateExec` 构造/直接测试或 `CREATE WORKLOAD REPOSITORY` 测试；因此这些项目明确记录为当前未接线或未验证，而非推断为已支持。
