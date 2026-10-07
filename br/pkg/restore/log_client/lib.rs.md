# `br/pkg/restore/log_client/lib.rs`

## 文件定位

`br/pkg/restore/log_client/lib.rs` 是 Cargo 包 `astersql-br-pkg-restore-log-client` 的 crate root。`br/pkg/restore/log_client/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定该入口，并用 `[package.metadata.porting] go-package = "br/pkg/restore/log_client"` 声明其 Go 对照包。它不是日志恢复算法的单一实现文件，而是把同目录的 13 个生产模块组装成一个 Rust 库，并提供接近 Go 包级名称空间的公开门面。

直接生产上游是 `br/pkg/task`：`br/pkg/task/Cargo.toml` 依赖本 crate，`br/pkg/task/stream.rs` 使用根级 `LogClient` 驱动流式恢复，`br/pkg/task/restore_lifecycle.rs` 调用其压缩 SST 恢复接口。文件本身没有 `main`、函数、类型或运行时初始化逻辑。

## 核心职责

1. 用 `#[path = "..."] pub mod ...` 显式挂载 `stubs`、日志文件/迁移模型、分裂策略、导入重试、导入器、客户端、ID map、批量元数据和流控等生产模块。
2. 用 `pub use ...::*` 将主要子模块公开项扁平再导出，让上游可从 `astersql_br_pkg_restore_log_client::LogClient`、`::NewLogFileImporter` 等根路径访问；`id_map` 是例外，只显式再导出 `PITRIdMapBlockSize` 和 `PitrIDMapsFilename`。
3. 在 `#[cfg(test)]` 下用独立 `*_test.rs` 文件挂载 13 个测试模块，保证测试逻辑不内嵌到生产源文件，也不进入非测试构建。
4. 以 crate 级 `#![allow(...)]` 容忍 Go 风格命名、迁移期未使用项和 Clippy 告警。该设置覆盖整个 crate，是移植兼容措施，不是功能完备或代码质量已经验证的证据。

## 主要符号

本文件只声明模块和再导出，不自行定义业务符号。公开模块及其主要职责如下：

- `stubs`：本地 `Context`、`Error`/`Result`、PD、存储、importer、checkpoint、stream 等边界抽象与内存替身；该模块本身也是 `pub`，上游目前会直接引用 `stubs::Context` 等类型。
- `log_file_map`：`LogFilesSkipMap` / `LogFilesSkipMapExt`，按 meta、group、file offset 记录 checkpoint 跳过状态。
- `ssts`：`SSTs`、`RewrittenSSTs`、`CompactedSSTs`、`CopiedSST`，统一表示压缩或复制得到的 SST 集合。
- `migration`：`WithMigrationsBuilder`、`WithMigrations`、`MetaWithMigrations`、`PhysicalWithMigrations`，将 migration 删除、compaction 和 ingested-SST 信息叠加到日志遍历。
- `log_file_manager`：`LogFileManager`、`CreateLogFileManager`、`LogDataFileInfo` 以及 meta/KV 读取和 TS 过滤辅助函数。
- `compacted_file_strategy` 与 `log_split_strategy`：分别决定压缩 SST 和日志文件的跳过、累计及 region 分裂时机。
- `import_retry`：`RangeController`、`RPCResult`、`RetryStrategy`，负责 region 扫描与导入 RPC 错误分类/重试决策。
- `import`：`LogFileImporter`、`NewLogFileImporter` 和 region/file 过滤，承担日志 KV 文件下载与 apply。
- `client`：`LogClient`、`LogRestoreManager`、`SstRestoreManager` 及批量/单文件应用函数，是 crate 的主要编排 API。
- `id_map`：PITR ID map 文件名、块大小和 `LogClient` 的映射持久化实现；根门面刻意只暴露两个常量/函数，其余仍可经公开模块路径访问。
- `batch_meta_processor`：`BatchMetaKVProcessor`、`RestoreMetaKVProcessor`、`MetaKVInfoProcessor`，抽象 meta KV 分批读取后的处理动作。
- `flow_control`：压缩 SST 流控估算、TiKV 配置读写和 `LogClient` 流控接线。它在测试模块声明之后挂载，但仍是无条件生产模块，声明位置不改变可见性。

`lib.rs` 未再导出 `stubs::*`，因此 stub 内符号一般通过 `crate::stubs::...` 或外部 `...::stubs::...` 访问；其余列入 glob 的模块会把所有 `pub` 项加入 crate 根公开面。

## 执行流程

本文件只有编译期组装流程，没有运行时控制流。实际使用链可由 Cargo 和直接源码引用复核：

1. Cargo 编译 `lib.rs`，依次装入 13 个生产模块；非测试构建跳过所有 `#[cfg(test)] mod ...`。
2. `pub use` 建立根级 API。`br/pkg/task/stream.rs` 的 `RestoreTiKVConfigControl::createLogClient` 产生 `LogClient`，`RestoreLogClientGuard` 在作用域结束时调用 `Close`。
3. `restoreStreamBody` 把根级 `LogClient` 交给 `RestoreLifecycle::RestoreCompactedSST`。
4. `br/pkg/task/restore_lifecycle.rs` 先调用 `LogClient::InitSSTFileRestorer` 安装 importer/checkpoint restorer，再调用 `RestoreSSTFileSets` 执行压缩 SST 恢复；错误被转换为 task crate 的错误类型返回。
5. 在 crate 内部，`client` 组合 `log_file_manager`、`migration`、分裂策略、`import`/`import_retry`、批量 meta processor 和 `flow_control`。这些步骤的具体分支与 I/O 均在对应子模块，不在 `lib.rs` 中。

测试构建还会装入 `export_test` 的测试可见包装、`main_test` 的共享夹具、`parity_test` 的综合 Go/Rust 契约检查，以及每个功能模块的独立回归测试。

## 数据与状态

`lib.rs` 自身没有常量、static、struct、enum、trait、锁或可变状态。它暴露的核心状态属于子模块：

- `LogClient` 聚合 PD、storage、日志文件管理、SST restorer、checkpoint、schema/ID 映射及运行统计，是上游持有的主状态对象。
- `LogFileManager` 保存读取时间窗、存储/metadata helper、migration 视图等日志遍历状态。
- `LogFilesSkipMap*` 和 `WithMigrations*` 保存 checkpoint 与 migration 过滤状态。
- `RangeController`、`LogFileImporter`、`LogRestoreManager` 维护 region 扫描、RPC/importer 与 worker/checkpoint 生命周期。

门面层的可观察状态只有“哪些名称在根路径可见”。改变 `pub mod` 或 `pub use` 会改变外部编译契约，即使子模块实现没有变化。

## 依赖与调用关系

`br/pkg/restore/log_client/Cargo.toml` 显示该 crate 直接依赖本地 `stream`、`utils`、`restore`、`checkpoint`、`restore/split`、`restore/utils`、`utils/iter`、`pkg/errors`，以及 `hex`、`sha2`、`serde`、`serde_json`。Cargo 注释明确当前 arm64 Darwin 路径采用精简依赖与本地 stubs，未直接接入完整 KV/domain/kvproto/grpcio 边界。

上游方面，`br/pkg/task/Cargo.toml` 是检索到的生产路径依赖者。精确源码引用包括：

- `br/pkg/task/stream.rs` 使用 `LogClient`、`stubs::Context`，并在 guard 的 `Drop` 中调用 `Close`；
- `br/pkg/task/restore_lifecycle.rs` 使用 `LogClient`、`SstRestoreManager`，调用 `InitSSTFileRestorer` 和 `RestoreSSTFileSets`；
- `br/pkg/task/stream_test.rs` 与 `restore_lifecycle_test.rs` 使用 `TEST_NewLogClient` 及 stub 类型验证跨 crate 接线。

RustCodeGraph 对文件级引用只报告 `tools/tazel/parity_test.rs`，而精确符号查询能定位 `LogClient`/`NewLogClient` 的 Rust 与 Go 定义，却未完整解析跨 crate method callers。因此本说明以 Cargo 依赖和源码中的完全限定路径作为跨 crate 主链证据，不把文件级图结果误解为“生产代码未使用”。

## 错误处理与边界

`lib.rs` 不创建或传播运行时错误。错误语义由再导出的实现承担：日志 metadata/编码损坏、TS 或 region 边界不合法、RPC 可重试分类、storage/checkpoint 失败以及资源关闭失败分别在 `log_file_manager`、`import_retry`、`import`、`client` 等模块处理。

门面本身有以下边界风险：

- glob 再导出的公开集合会随子模块新增 `pub` 项自动扩大；同名项可能产生歧义或破坏上游导入。
- `id_map` 的选择性根再导出是有意的不对称契约；不能假定 `id_map` 中所有公开方法都能从 crate 根访问。
- `stubs` 是公开模块且被生产上游用于 `Context`，但 Cargo 注释说明其边界是精简/本地实现；测试通过不能证明真实 PD/TiKV/对象存储链路已覆盖。
- `#![allow(clippy::all)]` 以及 `dead_code`、`unused_*` 会屏蔽迁移期告警；新增功能仍需逐项检查错误返回和实际接线，不能依赖 lint 沉默判断可用。
- `#[cfg(test)]` 包装只在 crate 自身测试编译时生效；外部 crate 不能依赖这些测试模块或 `export_test` 中的包装。

## 并发与资源生命周期

本文件不启动任务、线程或通道，也不持有资源。它公开的子模块形成以下生命周期契约：

- `LogRestoreManager`/`SstRestoreManager` 管理 importer、worker/restorer 与 checkpoint runner；上游先初始化，再恢复文件，最后关闭。
- `LogClient` 是主资源所有者；`br/pkg/task/stream.rs` 的 `RestoreLogClientGuard::drop` 无论操作成功或提前返回都会调用 `Close`，对应 Go 中 deferred close 的意图。
- `LogFileImporter` 持有 importer client，`Close` 负责关闭客户端；region 扫描和 apply 的并发/重试由 `RangeController`、client 和 restorer 协作。
- `flow_control` 在 SST 恢复前读取并可能调整 TiKV 配置，相关测试验证写入顺序、错误短路和恢复模式清理。

测试侧 `main_test.rs` 使用共享的内存 cluster，并在 `Drop` 中停止；Go 的 `main_test.go` 则启动 mock cluster、运行测试、停止 cluster 并执行 goroutine leak 检查。两者说明资源清理是包级测试契约，但不属于 `lib.rs` 的生产运行逻辑。

## 与 Go 版本的对应关系

Go 目录没有与 `lib.rs` 一一对应的生产入口文件；所有声明为 `package logclient` 的 `.go` 文件天然合并为包级名称空间。Rust 必须在本文件逐个 `mod` 挂载，并通过 `pub use` 模拟 Go 导出符号可直接由包路径访问的体验。

模块映射基本按同名文件保持：`client.rs`/`.go`、`import.rs`/`.go`、`import_retry.rs`/`.go`、`log_file_manager.rs`/`.go`、`log_file_map.rs`/`.go`、`migration.rs`/`.go`、`ssts.rs`/`.go`、两个分裂策略、`id_map`、`batch_meta_processor` 和 `flow_control` 均有直接对照。Rust 独有的 `stubs.rs` 汇集 Go 版从 PD、TiKV、domain、checkpoint、storage 等真实包取得的边界，反映当前精简移植状态，不应描述成 Go 生产依赖的完整等价实现。

测试也按文件分离：Rust 的 `client_test.rs`、`import_test.rs`、`import_retry_test.rs`、`log_file_manager_test.rs`、`log_file_map_test.rs`、`migration_test.rs` 等对照 Go 同名测试；Rust 还为 `compacted_file_strategy`、`log_split_strategy`、`ssts`、`flow_control`、`id_map` 提供独立测试，并以 `parity_test.rs` 聚合公开契约。`export_test.rs` 对应 Go `export_test.go` 为同包测试暴露内部入口的惯用目的，但 Rust 通过 `cfg(test)` 模块显式接入。

## 扩展指南

- 新增生产子模块时，应在本文件增加明确的 `#[path] pub mod`，并判断它是否真的需要根级再导出；长期稳定 API 优先考虑显式 `pub use module::Symbol`，避免无意扩大门面。
- 修改或移除根再导出前，搜索 `br/pkg/task` 中 `astersql_br_pkg_restore_log_client::...` 的完全限定引用，并检查 crate 内 `crate::...` 根路径用法。
- 新功能应落在职责对应的实现文件：编排进 `client.rs`，文件/TS 遍历进 `log_file_manager.rs`，下载 apply 进 `import.rs`，region/RPC 重试进 `import_retry.rs`，migration 进 `migration.rs`，流控进 `flow_control.rs`；`lib.rs` 只做接线。
- 测试必须继续放在独立 `*_test.rs` 中，并在这里以 `#[cfg(test)] #[path = "..."] mod ...` 挂载。优先扩展同名模块测试；改变跨模块公开契约时同步 `parity_test.rs`，改变上游生命周期时同步 `br/pkg/task/restore_lifecycle_test.rs` 或 `stream_test.rs`。
- 若把 stub 边界替换为真实外部依赖，应在独立上游仓库移植并以已发布 tag 的 Git 依赖接入，不能 vendor 或用本地 `[patch]`；同时评估 `stubs::Context` 等已被上游引用的公开路径兼容性。
- 收紧 crate 级 lint allow 时要检查全部子模块；其作用域不是这 140 行门面自身。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter br/pkg/restore/log_client` 列出本 crate 的 48 个 Go/Rust 文件。
- RustCodeGraph 查询：`node --file br/pkg/restore/log_client/lib.rs --offset 1 --limit 260`；`query LogClient --kind struct --limit 20`；`query NewLogClient --kind function --limit 20`；`callers LogClient --limit 40`；`callees NewLogClient --limit 40`；以及面向该 crate 模块、再导出和调用链的 `explore`。图对跨 crate callers 覆盖有限，已用 Cargo 与源码引用补证。
- 读过的边界与生产源码：`br/pkg/restore/log_client/lib.rs`、`Cargo.toml`、`BUILD.bazel`，以及 12 个非 stub 生产子模块的符号清单；直接上游读取了 `br/pkg/task/Cargo.toml`、`stream.rs`、`restore_lifecycle.rs`。
- 读过的 Go 对照：`client.go`、`import.go`、`export_test.go`、`main_test.go`，并用符号检索核对同目录其他 Go 生产文件和测试入口；`BUILD.bazel` 佐证 Go 包的生产源集合与测试集合。
- 读过的 Rust 测试证据：`main_test.rs`、`export_test.rs`、`parity_test.rs`，并检索全部 13 个 `*_test.rs` 的 `#[test]` 入口。测试覆盖 skip map、SST 契约、migration、meta/KV 过滤、region/import 重试、client 批处理、checkpoint、流控、资源清理和 Go/Rust 公开契约。
- 本任务只新增文档，按计划不运行 Cargo。交付检查限定为固定 11 章节结构、文档链接/路径人工复核、`git diff --check` 和只暂存目标文档的提交检查。
