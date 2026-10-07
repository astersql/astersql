# `br/pkg/checkpoint/lib.rs`

源文件：[`lib.rs`](./lib.rs)

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-checkpoint` 的 crate 根。`br/pkg/checkpoint/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，根 `Cargo.toml` 又把 `br/pkg/checkpoint` 列为 workspace member。它不实现 checkpoint 算法，而是把同目录九个生产模块装配成一个 Rust 库，并用根级再导出提供接近 Go 包级符号的调用体验。

对应的 Go 实现没有单独的 `lib.go`：`br/pkg/checkpoint/*.go` 通过共同的 `package checkpoint` 自然共享包级命名空间。Rust 必须显式声明模块并再导出，所以该文件是迁移后的语言级装配层，而不是另一套 checkpoint 实现。

## 核心职责

1. 以 `#[path = "..."] pub mod ...` 声明 `stubs`、`ticker`、`checkpoint`、`external_storage`、`storage`、`backup`、`restore`、`log_restore`、`manager` 九个生产模块。
2. 以 `#[cfg(test)]` 挂载七个独立测试模块，保证测试逻辑不内嵌在生产源文件中。
3. 通过九条 `pub use <module>::*` 将各子模块公开项提升到 crate 根；调用方因此可以写 `astersql_br_pkg_checkpoint::LogMetaManager`，无需写 `astersql_br_pkg_checkpoint::manager::LogMetaManager`。
4. 在 crate 级集中允许迁移代码仍存在的 Go 风格命名、暂未使用项和 Clippy 告警。该 `#![allow(...)]` 只改变静态检查策略，不改变运行时行为。

## 主要符号

本文件没有常量、结构体、枚举、trait、函数或 `impl`；其符号都是模块声明与再导出。

- 生产模块：`stubs` 提供当前 Rust 移植使用的上下文、存储、计时器、加密及 SQL 边界；`ticker` 提供 `TimeTicker` 与 `dispatcherTicker`；`checkpoint` 提供泛型 `CheckpointRunner`、数据/校验和格式及读写辅助；`external_storage` 与 `storage` 分别承载对象存储和表存储实现；`backup`、`restore`、`log_restore` 提供三个业务场景适配；`manager` 提供 `SnapshotMetaManager`、`LogMetaManager` 及表/对象存储管理器。
- 测试模块：`parity_test`、`checkpoint_test`、`external_storage_test`、`storage_test`、`ticker_test`、`log_restore_test`、`restore_test` 仅在测试构建中可见，且均为私有模块。
- 根级公开门面：`pub use backup::*` 至 `pub use ticker::*` 把子模块的公开类型、函数和常量汇集到 crate 根。代表性公开项包括 `StartCheckpointRunnerForBackup`、`CheckpointRunner`、`NewLogStorageMetaManager`、`LogMetaManager`、`CheckpointMetadataForLogRestore`、`Context` 和 `TiFlashReplicaInfo`。

## 执行流程

`lib.rs` 自身没有可调用的执行路径；它在编译期建立下列接线：

1. Cargo 读取 `br/pkg/checkpoint/Cargo.toml`，选择本文件作为 library target。
2. 编译器按九个 `#[path]` 声明纳入生产模块。声明顺序用于阅读分层，不应理解为运行时初始化顺序；Rust 模块声明本身不会启动线程、打开存储或创建 runner。
3. 九组通配再导出形成 crate 根 API。外部 crate 编译时通过此门面解析符号，真正执行的仍是符号所属子模块代码。
4. 测试构建额外纳入七个 `*_test.rs`/`parity_test.rs` 文件；普通构建完全排除这些模块。
5. 真实应用示例是 `br/pkg/task/stream.rs`：它通过根级路径持有 `Arc<dyn astersql_br_pkg_checkpoint::LogMetaManager>`，并使用 `Context`、`TiFlashReplicaInfo` 与日志恢复元数据。`br/pkg/restore/log_client/client.rs` 同样通过根级路径读取或创建 `CheckpointMetadataForLogRestore`。这些调用证明本文件是 BR 日志恢复链路的公开入口，但业务分支位于调用方和子模块中。

## 数据与状态

本文件不拥有运行时数据、全局可变状态或持久化格式。数据模型由被装配模块定义，例如 `checkpoint.rs` 的 `RangeGroup`/`CheckpointRunner`、`manager.rs` 的元数据管理 trait、`log_restore.rs` 的 `CheckpointMetadataForLogRestore`，以及 `stubs.rs` 的边界类型。

根级通配再导出不复制数据，也不创建第二种类型；`astersql_br_pkg_checkpoint::LogMetaManager` 与 `manager::LogMetaManager` 指向同一项。序列化字段、checkpoint 路径、锁内容、进度枚举和校验和状态均由相应子模块维护，不能从本文件的声明顺序推断其值或生命周期。

## 依赖与调用关系

- crate 边界由 `br/pkg/checkpoint/Cargo.toml` 定义，直接第三方依赖为 `aes`、`ctr`、`crossbeam-channel`、`rand`、`serde`、`serde_json`、`sha2`、`uuid`；这些依赖由子模块使用，本文件没有 `use` 它们。
- 上游 Cargo 依赖可在 `br/pkg/task/Cargo.toml`（`path = "../checkpoint"`）和 `br/pkg/restore/log_client/Cargo.toml`（`path = "../../checkpoint"`）看到。Bazel 侧 `br/pkg/task/BUILD.bazel`、`br/pkg/restore/log_client/BUILD.bazel` 等也声明 `//br/pkg/checkpoint`。
- RustCodeGraph 对日志恢复主链的代表性结果为：`br/pkg/task/stream.rs::restoreStreamWithTiKVConfigControl` 调用 `br/pkg/restore/log_client/client.rs::LoadOrCreateCheckpointMetadataForLogRestore`；后者以根级再导出的 `LogMetaManager` 和 `CheckpointMetadataForLogRestore` 作为接口与数据类型。`NewLogStorageMetaManager` 还被 `br/pkg/task/stream_test.rs` 和本 crate 的 `checkpoint_test.rs` 调用。
- 下游实现均在本文件声明的九个模块内。本文件没有函数，因此不存在属于 `lib.rs` 自身的 callers/callees；图中的边连接实际实现符号，而 crate 根负责让这些符号可从稳定的根路径被解析。

## 错误处理与边界

模块声明、条件编译和再导出不产生运行时 `Result`，因此本文件没有错误转换、重试或降级逻辑。存储 I/O、序列化、锁冲突、通道关闭和上下文取消等错误由实际实现模块返回；例如 runner 和存储行为应分别到 `checkpoint.rs`、`external_storage.rs`、`storage.rs`、`manager.rs` 查看。

需要注意的编译期边界有三项：指定路径的文件缺失会使 crate 无法编译；通配再导出新增同名公开项可能造成根命名空间冲突或让调用方解析不再明确；`#![allow(clippy::all)]` 等宽泛豁免会隐藏静态告警，所以不能把“无告警”当作实现完整性的证据。`stubs` 是当前 crate 的真实依赖边界，但其存在不等于已经接入完整 PD/TiKV 生产设施。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、会话、计时器或存储句柄，也没有初始化/析构钩子。并发生命周期只通过再导出的 API 暴露：`CheckpointRunner` 的队列与刷盘循环位于 `checkpoint.rs`，ticker 位于 `ticker.rs`，元数据管理器资源位于 `manager.rs`，外部存储锁位于 `external_storage.rs`。

独立测试给出了这些生命周期的直接边界证据：`checkpoint_test.rs` 覆盖 `WaitForFinish(true)` 排空、重试/不重试、锁冲突与接管，并通过串行环境锁和 failpoint 清理避免跨用例污染；`ticker_test.rs` 覆盖慢消费者时丢弃 tick；`external_storage_test.rs` 覆盖锁更新时间与写后失败点。维护 crate 根时应保持这些测试为独立文件并继续由 `#[cfg(test)]` 挂载。

## 与 Go 版本的对应关系

Go 的 `br/pkg/checkpoint` 由 `checkpoint.go`、`backup.go`、`restore.go`、`log_restore.go`、`manager.go`、`storage.go`、`external_storage.go`、`ticker.go` 等同包文件共同组成；Rust 将相同职责拆成同名模块，并由本文件恢复 Go 包级的单一公开入口。因而 `pub use ...::*` 的主要兼容目标是路径与可见性，而不是重新实现 Go 泛型或运行时。

Rust `checkpoint_test.rs` 明确标注对齐 Go `checkpoint_test.go`，覆盖 backup/restore/log restore 元数据、storage/table 两类 manager、runner 刷盘/重试/锁；`parity_test.rs::go_rust_public_contract_matches` 从 crate 内同时导入各模块公开项，检查序列化、进度、压缩、加解密、checksum、ticker 和存储分片等跨模块契约。差异是 Rust 当前通过 `stubs.rs` 隔离部分 PD/TiKV、Domain 和 SQL 边界，而 Go 文件直接依赖对应生产包；这属于子模块移植边界，不应在 `lib.rs` 中伪装成完整生产接线。

## 扩展指南

- 新增 checkpoint 子模块时，在本文件增加明确的 `#[path] pub mod`；若它属于公开 API，再决定是否根级再导出。不要默认使用通配导出来掩盖命名冲突，先检查现有根级公开名和外部调用路径。
- 新增行为应实现于职责对应的子模块，不应写进 `lib.rs`。同步测试应放到独立 `*_test.rs`，再用 `#[cfg(test)]` 在此挂载；若是 Go 移植，还应对照同路径 Go 实现和 Go 测试保持行为、错误和序列化语义。
- 改动公开路径前搜索 `astersql_br_pkg_checkpoint::` 调用者，重点检查 `br/pkg/task`、`br/pkg/restore/log_client` 及其测试；移除根级再导出属于 API 兼容性变化，即使子模块路径仍可访问也可能破坏调用方。
- 修改并发、存储或错误语义时不要只补 crate 根测试挂载；应更新相应独立测试文件，并覆盖 runner 收尾、失败注入清理、锁与 ticker 等资源边界。
- 本文件当前已有 `// Copyright 2026 AsterSQL.`；维护时保留版权注释。它没有 PingCAP Apache License 块，文档任务不对源文件版权作改写。

## 验证依据

- 源码与声明：`br/pkg/checkpoint/lib.rs`（九个生产模块、七个 `cfg(test)` 模块、九组根级再导出、crate 级 lint allow）；`br/pkg/checkpoint/Cargo.toml`；根 `Cargo.toml` workspace members。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file br/pkg/checkpoint/lib.rs --offset 1 --limit 220` 返回完整 94 行并确认该文件主要由模块装配组成；`query CheckpointRunner`、`query NewLogStorageMetaManager`、`query LogMetaManager` 定位 Rust/Go 对应符号；`explore "NewLogStorageMetaManager LogMetaManager LoadOrCreateCheckpointMetadataForLogRestore restoreStreamWithTiKVConfigControl"` 给出 `task -> restore/log_client -> checkpoint` 的消费链和测试调用者。
- 上游入口：`br/pkg/task/Cargo.toml`、`br/pkg/task/stream.rs`、`br/pkg/restore/log_client/Cargo.toml`、`br/pkg/restore/log_client/client.rs`。
- Go 对照：`br/pkg/checkpoint/checkpoint.go`、`manager.go`、`backup.go`、`restore.go`、`log_restore.go`、`external_storage.go`、`storage.go`、`ticker.go`；Go 包没有与 Rust crate 根一一对应的单独文件。
- 独立测试：`br/pkg/checkpoint/checkpoint_test.rs`、`parity_test.rs`、`external_storage_test.rs`、`storage_test.rs`、`ticker_test.rs`、`log_restore_test.rs`、`restore_test.rs`；Go 对照测试为 `br/pkg/checkpoint/checkpoint_test.go`。
- 本任务是只读行为分析与文档新增，按计划不运行 Cargo；验收使用固定十一章节的结构检查，并人工确认本文没有把模块声明顺序描述为运行时顺序，也没有把 stub 边界描述为完整生产接线。
