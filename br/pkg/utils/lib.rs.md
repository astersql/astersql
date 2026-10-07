# `br/pkg/utils/lib.rs`

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-utils` 的 crate 根；`br/pkg/utils/Cargo.toml` 通过 `[lib] path = "lib.rs"` 明确指定它。它对应 Go 的 `br/pkg/utils` 包级命名空间，但 Rust 侧需要在这里显式声明各实现文件并决定哪些符号暴露在 crate 根。文件本身不执行备份或恢复流程，而是把通用工具组织成可供 BR、Lightning 等调用方依赖的公共门面。

入口当前公开声明 22 个模块：`stubs`、`backoff`、`common`、`db`、`dyn_pprof_other`、`dyn_pprof_unix`、`encryption`、`error_handling`、`filter`、`json`、`key`、`memory_monitor`、`misc`、`pointer`、`pprof`、`progress`、`register`、`retry`、`schema`、`store_manager`、`wait`、`worker`、`metadata_register`。除最后一个采用默认文件发现规则外，其余模块都用 `#[path = "..."]` 绑定到同目录实现文件。

## 核心职责

1. 以 `pub mod` 建立 BR 通用工具的公开模块树，让调用方既可使用 `astersql_br_pkg_utils::backoff::...` 这类模块路径，也可使用根级便捷导出。
2. 用 `pub use` 在 crate 根聚合高频 API：重试/退避、备份错误分类、PiTR 过滤、schema 名称处理、等待、worker 并发工具，以及 `KeyRange`、`KvKey`、`kvproto` 桩类型。
3. 通过 `pub use metadata_register::*` 暴露真实元数据服务适配器 `MetadataRegisterClient`，把 `register` 中的注册状态机接到 `astersql-metaservice`。
4. 仅在 `cfg(test)` 下挂载独立测试文件，满足实现与测试分文件的仓库约束；生产构建不会编入这些测试模块。
5. 在 crate 级允许 Go 风格命名和迁移期未使用项。`dead_code`、`non_snake_case` 等六项 `allow` 是兼容移植代码的宽松策略，也意味着编译器不会替本门面发现所有闲置导出。

## 主要符号

- 模块声明：本文件没有自定义函数、结构体、枚举、trait、常量或 `impl`；主要符号就是 22 个公开模块及测试期私有模块。
- `pub use stubs::kvproto`、`pub use stubs::{KeyRange, KvKey}`：把尚未完全接入上游 Proto/KV crate 的本地兼容类型提升到根路径。`Cargo.toml` 的注释也明确说明当前 Proto/KV/SQL 边界使用本地 stubs。
- `BackoffStrategy`：从 `backoff` 再导出的退避 trait，是 `WithRetry` 等循环与调用方自定义策略之间的根级契约。
- `ErrorContext`、`ErrorHandlingResult`、`ErrorHandlingStrategy` 及 `HandleBackupError` 等：从 `error_handling` 再导出的错误分类和重试决策接口。
- `PiTRIdTracker`、`NewPiTRIdTracker`、`MatchSchema`、`MatchTable`：从 `filter` 再导出的 PiTR 对象跟踪和过滤入口。
- `WithRetry`、`WithRetryV2`、`WithRetryReturnLastErr`、`VerboseRetry`、`GiveUpRetryOn`：从 `retry` 再导出的重试编排入口；它们消费 `BackoffStrategy`，具体循环不在本文件内。
- `EncloseName`、`TemporaryDBName`、`StripTempDBPrefix*`、`IsSysDB` 等：从 `schema` 再导出的标识符和系统库处理 API。
- `WaitUntil`：从 `wait` 再导出的可取消、带超时轮询入口。
- `AsyncStreamBy`、`WorkerTokenChannel`、`BuildWorkerTokenChannel`、`PanicToErr` 等：从 `worker` 再导出的线程、背压、并发令牌和 panic 隔离工具；其中子模块的 `Result<T>` 在根级改名为 `WorkerResult`，避免与标准 `Result` 混淆。
- `MetadataRegisterClient`：经 `pub use metadata_register::*` 通配再导出；它实现 `register::EtcdRegisterClient`，供 Lightning 导入流程复用注册状态机。

## 执行流程

作为 crate 根，本文件的“执行”发生在编译期名称解析阶段，而不是运行期：Cargo 读取 `lib.rs`，入口用 `#[path]` 纳入各实现模块，再把选定符号映射到 crate 根。下游代码随后有两条使用路径：需要完整能力时沿公开模块进入具体实现；需要稳定高频 API 时直接从根导入再导出符号。

典型运行链由子模块完成。例如调用方取得根级 `BackoffStrategy` 并调用根级 `WithRetry`，实际逻辑进入 `retry.rs::WithRetry`，后者反复调用策略的 `NextBackoff`/`RemainingAttempts`；`parity_test.rs::go_rust_public_contract_matches` 覆盖了持续错误最终停止的组合行为。另一个跨模块链是 Lightning 在 `lightning/pkg/importer/import.rs` 使用根级 `MetadataRegisterClient`，同时从 `register` 模块构造任务注册器，适配器再把注册状态机的 put/grant/keep-alive/get/revoke 调用转给 `astersql-metaservice::NamespacedEtcdClient`。

测试编译时，入口额外挂载 19 个独立测试模块：`parity_test`、`main_test`、`wait_test`、`worker_test`、`backoff_test`、`common_test`、`db_test`、`error_handling_test`、`filter_test`、`json_test`、`key_test`、`memory_monitor_test`、`misc_test`、`progress_test`、`pprof_test`、`register_test`、`retry_test`、`schema_test`、`store_manager_test`，并挂载 `metadata_register_test`；Unix 测试构建还挂载 `dyn_pprof_unix_test`。也就是说一般测试构建共 20 个测试模块，Unix 下为 21 个。

## 数据与状态

本文件没有运行期字段、全局可变状态、缓存或持久化数据。它只定义命名空间与可见性；真实状态归各子模块所有。例如 `PiTRIdTracker` 保存库表集合，`WorkerTokenChannel` 通过 `Arc<(Mutex<_>, Condvar)>` 共享令牌状态，`MetadataRegisterClient` 持有 `NamespacedEtcdClient`，这些对象都只是由本入口导出而非在此创建。

入口所做的一个重要类型映射是 `worker::Result` 到 `WorkerResult` 的别名再导出。另一个迁移期边界是 `stubs`：`KeyRange`、`KvKey` 和 `kvproto` 当前来自本地替身，因此根级 API 的类型身份取决于 `stubs.rs`，未来切换真实依赖时必须评估所有下游签名兼容性。

## 依赖与调用关系

`br/pkg/utils/Cargo.toml` 将本 crate 定义为普通 library，未声明 feature；直接依赖包括同仓库的 metaservice、BR errors/logutil/summary、parser/meta/util 系列 crate，以及 `crossbeam-channel`、`signal-hook`、`prometheus`、`pprof`、`prost` 等外部库。入口本身不直接调用这些库，依赖由所声明的子模块使用。

RustCodeGraph 的文件节点将 `br/pkg/utils/lib.rs` 关联到 `br/pkg/restore/snap_client/import.rs`、`br/pkg/restore/split/client.rs` 和 `br/pkg/task/parity_test.rs`。源码与 Cargo 反向搜索进一步确认生产依赖者包括：

- `br/pkg/conn/util`：根级 `BackoffStrategy` 与 `backoff::NewAggressivePDBackoffStrategy`。
- `br/pkg/metautil`：`encryption::{Decrypt, IsEffectiveEncryptionMethod}` 以及 stubs 中的 Proto 类型。
- `br/pkg/restore/snap_client`、`br/pkg/restore/log_client`：退避策略。
- `br/pkg/task`：`db` 配置辅助及 `stubs` SQL 执行接口。
- `lightning/pkg/importer`：根级 `MetadataRegisterClient`、`register` 状态机与上下文桩。

下游不应把同目录的 `iter`、`consts`、`storewatch` 误认为本入口子模块；它们各自有独立 `Cargo.toml` 和 crate 根，包名分别为 `astersql-br-pkg-utils-iter`、`astersql-br-pkg-utils-consts`、`astersql-br-pkg-utils-storewatch`。

## 错误处理与边界

本入口不产生或转换错误，错误语义由被导出的实现决定。根级接口覆盖的主要边界包括：`error_handling` 将备份错误分为重试、放弃等策略；`retry`/`backoff` 控制尝试次数与延迟；`WaitUntil` 区分上下文取消和超时；`PanicToErr` 把 panic 转为共享错误。`parity_test.rs` 对空备份错误、未知加密方法、用户取消消息、可重试存储消息和退避耗尽做了跨模块冒烟验证。

入口的兼容边界主要来自公开 API。删除模块、收窄可见性、移动根级再导出、改变 `WorkerResult` 别名或替换 stubs 类型，都可能让下游在编译期破坏。`pub use metadata_register::*` 还会自动扩大未来新增公开符号的根级表面积；新增适配器符号时应检查是否确实希望成为根 API。

两个动态 pprof 模块在本文件中均无平台 `cfg`，实际平台条件位于各自实现文件：`dyn_pprof_other.rs::StartDynamicPProfListener` 仅在非 Unix 类目标存在，`dyn_pprof_unix.rs` 的实现和再导出仅在 Unix 类目标存在。扩展平台集合时应修改实现条件并验证互斥性，不能只依赖入口注释。

## 并发与资源生命周期

本文件不启动线程、任务、定时器、通道、锁、网络连接或事务；声明模块和再导出符号也不产生运行期资源。并发与资源所有权由具体 API 管理：`worker::AsyncStreamBy` 启动后台线程并以零容量同步通道提供背压，接收端丢弃后生产线程退出；`WorkerTokenChannel` 用互斥锁和条件变量协调令牌；`wait::WaitUntil` 按上下文取消或截止时间结束轮询；`metadata_register::MetadataRegisterClient::keep_alive` 启动租约保活线程，在租约消失、接收端关闭、上下文取消或保活失败时退出。

测试生命周期也由条件编译隔离。`main_test.rs::test_main_setup` 调用公共测试初始化；Go 的 `main_test.go::TestMain` 还使用 goleak 检查 goroutine 泄漏，而 Rust 测试只记录“没有等价钩子时不强行模拟”。因此不能从入口测试挂载推断 Rust 已具备 Go goleak 的同等资源泄漏检测。

## 与 Go 版本的对应关系

Go 没有与 `lib.rs` 一一对应的入口文件；同目录所有声明 `package utils` 的 `.go` 文件天然合并为一个包。Rust 的 `lib.rs` 显式重建这一包边界，并把 Go 包级导出拆为模块路径和根级精选再导出。`Cargo.toml` 的 `[package.metadata.porting] go-package = "br/pkg/utils"` 是这一归属的直接证据。

根级抽样符号与 Go 同名契约对应，例如 Go `BackoffStrategy`/`WithRetry`/`WaitUntil`/`GetOrZero`/`EncloseName` 对应 Rust 的同名再导出。`parity_test.rs::go_rust_public_contract_matches` 专门从 crate 根导入这些符号，验证默认值、名称引用、错误分类、等待、退避和过滤等组合行为。

Rust 侧并非完全等价封装：本地 `stubs` 替代了部分真实 Proto/KV/SQL 依赖；`MetadataRegisterClient` 是 Rust 为接入 metaservice 新增的适配层，没有同名 Go 文件；Go `TestMain` 的 goleak 校验也未在 Rust 完整复刻。这些差异均应视为当前迁移状态，而不是 Go 行为已全部覆盖的证据。

## 扩展指南

- 新增普通工具实现时，在独立 `.rs` 文件中实现并测试，再在此添加 `#[path] pub mod`；只有跨模块高频且需要稳定根路径的 API 才加入 `pub use`。
- 新增或修改根级导出前，搜索 `astersql_br_pkg_utils::` 的生产调用点并评估破坏性；特别留意 `br/pkg/conn/util`、`br/pkg/metautil`、恢复链、`br/pkg/task` 和 Lightning。
- 修改 stubs 或将其替换为真实依赖时，必须检查 `KeyRange`、`KvKey`、`kvproto` 及 SQL executor 类型在下游签名中的身份变化，不应只验证本 crate 内部。
- 扩展 `metadata_register` 时，避免无意通过通配再导出扩大根 API；若新增资源线程，需在独立 `metadata_register_test.rs` 覆盖取消和清理。
- 平台相关模块应把 `cfg` 保持在实现文件中并确保目标集合互斥；新增平台测试应继续放在独立测试文件，而不是内嵌到 `lib.rs`。
- 测试应优先同步对应的 `*_test.rs`；根级导出和跨模块契约可补充到 `parity_test.rs`，公共测试初始化改动对应 `main_test.rs`。如果增加新的顶层测试模块，再在此用 `#[cfg(test)]` 和 `#[path]` 挂载。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/utils` 确认本区域文件集合；`node --file br/pkg/utils/lib.rs --offset 1 --limit 260` 给出完整入口及文件级使用关系；`query BackoffStrategy` 将 Rust trait 定位到 `backoff.rs:131`，并同时找到 Go 接口 `backoff.go:64`。
- crate 边界：`br/pkg/utils/Cargo.toml` 的包名、`[lib] path`、porting 元数据、依赖和 dev-dependencies。
- Rust 源码：`br/pkg/utils/lib.rs`、`backoff.rs`、`retry.rs`、`wait.rs`、`worker.rs`、`metadata_register.rs`、`dyn_pprof_other.rs`、`dyn_pprof_unix.rs`。
- Rust 测试：`br/pkg/utils/parity_test.rs`、`main_test.rs`、`metadata_register_test.rs`，以及入口列出的各子模块独立 `*_test.rs`。
- Go 对照：`br/pkg/utils/backoff.go`、`retry.go`、`wait.go`、`worker.go`、`pointer.go`、`schema.go`、`error_handling.go`、`main_test.go`；同目录 `package utils` 声明证明 Go 包由多文件共同组成。
- 调用方证据：Cargo manifest 与源码搜索定位到 `br/pkg/conn/util`、`br/pkg/metautil`、`br/pkg/restore/snap_client`、`br/pkg/restore/log_client`、`br/pkg/task`、`lightning/pkg/importer`。
- 本任务只做文档分析，按计划不运行 Cargo。交付前以固定章节结构命令验证文档恰含 11 个二级标题，并人工核对本文件为何存在、编译期如何接线及安全扩展位置。
