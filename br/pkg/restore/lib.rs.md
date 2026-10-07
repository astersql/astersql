# `br/pkg/restore/lib.rs`

## 文件定位

`br/pkg/restore/lib.rs` 是 Cargo 包 `astersql-br-pkg-restore` 的 crate root。`br/pkg/restore/Cargo.toml` 通过 `[lib] path = "lib.rs"` 把它指定为库入口，`[package.metadata.porting]` 则将该 crate 对应到 Go 包 `br/pkg/restore`。根 workspace `Cargo.toml` 同时把它和 `restore/data`、`restore/log_client`、`restore/snap_client` 等子 crate 分别列为 workspace 成员；因此本文件是核心 restore crate 的门面，不是整个 `br/pkg/restore/**` 目录树的递归模块入口。

本文件本身只负责组装和再导出，没有定义恢复算法、运行时状态或可调用入口函数。实际实现位于它挂载的 `stubs.rs`、`import_mode_switcher.rs`、`misc.rs` 和 `restorer.rs`。

## 核心职责

1. 以 `#[path = "..."] pub mod ...` 显式挂载四个生产模块，确定该 crate 的编译边界。
2. 在 `#[cfg(test)]` 下挂载五个独立测试文件，保持生产源码与测试逻辑分离。
3. 通过四条 `pub use ...::*` 把子模块公开项折叠到 crate 根，使上层可以使用 `astersql_br_pkg_restore::SstRestorer` 或 `astersql_br_pkg_restore::NewImportModeSwitcher`，也可以通过 `astersql_br_pkg_restore::import_mode_switcher::ImportModeSwitcher` 显式定位模块。
4. 在迁移期间容忍 Go 风格命名、未使用项和未完成的本地边界抽象；文件级 `#![allow(...)]` 包含 `dead_code`、三类命名 lint、`unused_*` 与 `clippy::all`。这是迁移兼容策略，不代表所有再导出 API 都已由真实集群路径覆盖。

## 主要符号

- `pub mod stubs`：导出 restore 实现所需的 `Context`、`Error`/`Result`、`PdClient`、`ConnMgr`、`Storage`、`WorkerPool`、`ErrorGroup`、checkpoint 等本地抽象与内存替身。`stubs.rs` 的模块文档明确说明它尚不是 PD/TiKV/domain/storage 的完整生产 RPC 实现。
- `pub mod import_mode_switcher`：提供 `ImportModeSwitcher`、`NewImportModeSwitcher`、`RestorePreWork`、`FineGrainedRestorePreWork`、`RestorePostWork` 和 `GrpcImportSstSwitcher`，处理 TiKV Import/Normal mode 与 PD scheduler 的恢复前后生命周期。
- `pub mod misc`：提供日志恢复 blocklist 编解码、TS/schema 查询、用户库检查、region 扫描与 `GroupOverlappedBackupFileSetsIter` 等辅助逻辑。
- `pub mod restorer`：定义 `BackupFileSet`/`BatchBackupFileSet`、`FileImporter`、`BalancedFileImporter`、`SstRestorer` 及 Simple/Batch/MultiTables/Pipeline 恢复编排实现。
- `parity_test`、`export_test`、`import_mode_switcher_test`、`misc_test`、`restorer_test`：只在测试构建中可见，且都是 crate-private `mod`，不进入库的公开 API。
- `pub use import_mode_switcher::*`、`pub use misc::*`、`pub use restorer::*`、`pub use stubs::*`：建立扁平公开表面；这些 glob 再导出的精确项集由四个子模块中的 `pub` 符号决定。

## 执行流程

本文件没有自主运行流程。它在编译阶段先建立模块树，再建立公开名称解析表；运行时调用直接落到子模块符号。典型主链由源码引用可核验为：

1. `br/pkg/task/Cargo.toml` 以路径依赖引入 `astersql-br-pkg-restore`，`br/pkg/task/restore_lifecycle.rs` 将其别名为 `restore`。
2. `RestoreLifecycle::Start` 通过根再导出的 `NewImportModeSwitcher` 构造 mode switcher，并根据 key ranges 选择 `FineGrainedRestorePreWork` 或 `RestorePreWork`。
3. `RestoreLifecycle::RestoreFiles` 根据 `RestoreKind` 构造 `NewMultiTablesRestorer` 或 `NewSimpleSstRestorer`，提交 `BatchBackupFileSet`，等待完成并关闭 importer。
4. `RestoreSession::drop` 在正常清理路径调用 `RestorePostWork`；checkpoint 需保留暂停状态时则只停止刷新线程。
5. `br/pkg/restore/log_client/client.rs` 也通过 crate 根使用 `SstRestorer`、`FileImporter`、`NewSimpleSstRestorer` 等；`br/pkg/restore/snap_client/import.rs` 为根门面的 `FileImporter`/`BalancedFileImporter` 提供具体实现。

因此 `lib.rs` 只是编译时接线点，实际控制流、I/O 与错误路径分布在上述调用者和子模块中。

## 数据与状态

`lib.rs` 不声明 static、const、struct、enum、trait 或可变状态。通过根再导出暴露的主要数据分为三类：

- 恢复工作单元：`BackupFileSet` 携带 table ID、SST 文件列表和可选 rewrite rules，`BatchBackupFileSet` 是其向量。
- 跨边界契约：`FileImporter`、`BalancedFileImporter`、`SstRestorer`、`PdClient`、`ConnMgr`、`Storage` 和 `RestoreCheckpoint` 等 trait 隔离编排与实际客户端。
- 生命周期状态：`ImportModeSwitcher` 持有 PD client、mode transport、刷新间隔、mutex、cancel/wake handle 与 wait group；各 restorer 持有 error group、context、worker pool、importer 和可选 checkpoint runner。

上述类型都不在本文件中实现；修改 glob 再导出会改变它们的外部路径，但不改变类型自身状态语义。

## 依赖与调用关系

crate 边界由 `br/pkg/restore/Cargo.toml` 确定。直接依赖是 `grpcio`、带固定 tag `v0.0.2-aster.20260929` 的 `kvproto`、本地 `astersql-br-pkg-restore-utils`、本地 `astersql-br-pkg-utils-iter`、`base64` 和 `sha2`。注释声明 arm64 Darwin 路径使用本地 traits/stubs 和精简 rewrite helpers；这与 `stubs.rs` 自述的“未完整接入真实边界”一致。

上游直接依赖至少包括 `br/pkg/task/Cargo.toml`、`br/pkg/restore/log_client/Cargo.toml` 和 `br/pkg/restore/snap_client/Cargo.toml`。文本引用证据显示：

- `br/pkg/task/restore_lifecycle.rs` 使用门面中的预处理/收尾、file-set、importer 和 restorer API；
- `br/pkg/restore/log_client/client.rs` 持有 `Arc<dyn astersql_br_pkg_restore::SstRestorer>` 并构造 Simple restorer；
- `br/pkg/restore/snap_client/import.rs` 实现该门面导出的 importer traits。

RustCodeGraph 对 `restorer.rs` 报告有 34 个使用文件，对 `misc.rs` 报告有 15 个使用文件；但对 `lib.rs` 的文件级“used by”只显示一个非业务检查文件，而精确 Rust 构造器的 callers 未跨 crate 解析。因此跨 crate 关系以 Cargo 路径依赖和上述源码引用为直接证据，不将图索引的空 callers 解读为无调用者。

## 错误处理与边界

`lib.rs` 本身不创建、包装或传播运行时错误。它将 `stubs::Error`/`Result` 再导出，但错误语义由子模块决定：mode switcher 通过 `ErrorGroup` 汇聚 store 切换失败，restorer 在 importer/checkpoint 失败时通过 error group 向 `WaitUntilFinish` 收束，`misc` 对非法 blocklist、checksum 不匹配、schema/TS 查询与不合法重叠分组返回错误。

门面层有三个重要边界：

- `#![allow(...)]` 会在整个 crate 作用域内容忍本可由 lint 提示的问题，所以新 API 不能仅以“编译无警告”作为可用性证据。
- glob 再导出可能在子模块新增同名公开项时引入歧义或意外扩大 API；新符号需同时检查模块路径与 crate-root 路径。
- `pub use stubs::*` 使迁移期替身成为公开表面的一部分；将其换成真实客户端不只是内部重构，还需要评估上游实现的 trait/type 兼容性。

## 并发与资源生命周期

`lib.rs` 不启动线程、不创建锁、通道、worker pool、RPC 连接或存储句柄。但它把相关生命周期 API 变成根级公开契约：

- `ImportModeSwitcher` 使用 `Mutex`、cancel function、wake channel 和 `WaitGroup` 管理周期刷新线程。`SwitchToNormalMode` 先取消并等待线程退出，再切回 Normal；`RestorePostWork` 还要执行 scheduler undo。
- Simple/Batch/MultiTables restorer 通过 `WorkerPool` 和 `ErrorGroup` 提交并发导入；`GoRestore` 是提交边界，调用者必须再调用 `WaitUntilFinish` 收集异步失败，最后调用 `Close` 释放 importer 资源。
- `br/pkg/task/restore_lifecycle.rs` 的 `RestoreSession::drop` 是直接上游 RAII 收尾点，确保正常路径执行 `RestorePostWork`，或在 checkpoint 保留状态时执行 `StopRefreshing`。

这些约束来自再导出符号的实现与直接上游，而不是 `lib.rs` 自身的执行行为。

## 与 Go 版本的对应关系

Go 不需要与 Rust `lib.rs` 一对一的包入口文件：同目录下声明 `package restore` 的 `import_mode_switcher.go`、`misc.go`、`restorer.go` 会自动合并为同一 Go 包公开面。Rust 需要由本文件显式挂载对应 `.rs` 文件并再导出公开符号，才能近似 Go 的包级名称空间。

`stubs.rs` 没有单一 Go 同名生产文件；它把 Go 实现中来自 `pd.Client`、`conn.Mgr`、storage/domain/checkpoint/util 等多个包的边界集中成 Rust trait 和测试替身。这是当前迁移状态，不应被描述为 Go 生产实现的完整等价物。

测试映射也是显式的：`import_mode_switcher_test.rs`、`misc_test.rs`、`restorer_test.rs` 对应同名 Go 测试，`parity_test.rs` 聚合三个 Go 生产文件的关键契约，`export_test.rs` 对应 Go 同包测试为外部测试包暴露私有 blocklist 符号的惯用法。

## 扩展指南

- 新增 restore 生产模块时，先确定它属于当前 crate 还是已独立的子 crate。只有前者应在本文件新增 `#[path] pub mod`；不要为了便利把 `restore/log_client` 或 `restore/snap_client` 等独立 Cargo 包重复挂载进来。
- 需要根级 API 时，检查新符号是否会与四个 glob 来源的现有名称冲突。对需要长期稳定的外部契约，可考虑显式 `pub use module::Symbol`，但需同步上游导入与兼容性评估。
- 新增测试应继续放在独立 `*_test.rs` 文件，并只在 `#[cfg(test)]` 下挂载，不要内嵌到生产 `.rs` 中。模块行为回归应优先扩展对应的 `import_mode_switcher_test.rs`、`misc_test.rs` 或 `restorer_test.rs`；Go/Rust 契约变更还应扩展 `parity_test.rs`。
- 修改 `stubs` 公开项时，必须搜索 `br/pkg/task`、`restore/log_client` 和 `restore/snap_client` 中对 crate-root 与 `stubs::` 路径的实现，防止破坏 trait object 可替换性。
- 移除或收紧 `#![allow(...)]` 应分步进行并检查整个 crate；其影响范围不限于 57 行门面。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件，其中 7032 个 Rust 文件；索引中 `br/pkg/restore/lib.rs` 为 57 行，只含 4 个生产模块、5 个测试模块和 4 条再导出。
- RustCodeGraph 查询：`node --file br/pkg/restore/lib.rs --offset 1 --limit 240`；`query NewImportModeSwitcher --kind function --json`；`query NewSimpleSstRestorer --kind function --json`；`query RestorePreWork --kind function --json`；`query GroupOverlappedBackupFileSetsIter --kind function --json`；以及对 `import_mode_switcher.rs`、`restorer.rs`、`misc.rs` 的定位读取。
- 读过的 crate/上游边界：根 `Cargo.toml`、`br/pkg/restore/Cargo.toml`、`br/pkg/task/Cargo.toml`、`br/pkg/task/restore_lifecycle.rs`、`br/pkg/restore/log_client/Cargo.toml`、`br/pkg/restore/log_client/client.rs`、`br/pkg/restore/snap_client/Cargo.toml`、`br/pkg/restore/snap_client/import.rs`。
- 读过的 Rust 生产与测试证据：`br/pkg/restore/lib.rs`、`stubs.rs`、`import_mode_switcher.rs`、`misc.rs`、`restorer.rs`、`parity_test.rs`、`export_test.rs`、`import_mode_switcher_test.rs`、`misc_test.rs`、`restorer_test.rs`；直接上游回归还包括 `br/pkg/task/restore_lifecycle_test.rs` 和 `br/pkg/restore/log_client/flow_control_test.rs`。
- 读过的 Go 对照：`br/pkg/restore/import_mode_switcher.go`、`misc.go`、`restorer.go`、`export_test.go`，以及对应 `*_test.go`。Go 目录中没有对应 `lib.rs` 的必需单文件，包级聚合由 Go 编译器完成。
- 相关测试意图：mode switcher 测试覆盖 import/normal 顺序、TiFlash 过滤和 scheduler 收尾；misc 测试覆盖 blocklist、TS/schema 边界与重叠文件分组；restorer 测试覆盖 Simple/Batch/MultiTables、checkpoint、进度与错误传播；上游 lifecycle 测试覆盖门面的跨 crate 接线。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证为 11 个固定二级标题的结构检查、所引路径存在性、`git diff --check` 与人工事实复核。
