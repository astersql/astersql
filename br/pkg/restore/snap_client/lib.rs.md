# `br/pkg/restore/snap_client/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-restore-snap-client` 的 crate root。`br/pkg/restore/snap_client/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，并在 `[package.metadata.porting]` 中把 Go 对照包标为 `br/pkg/restore/snap_client`、类型标为 `library`。根 `Cargo.toml` 将 `br/pkg/restore/snap_client` 列为 workspace member。

该文件是模块装配与公开 API 门面，不是一次恢复操作的运行入口。它没有 `main`、函数、类型或可变全局状态；真正的控制面、下载与 ingest、流水线、PiTR、placement、系统表处理和 SST 发送逻辑分别位于同目录的生产模块中。仓库内对 crate 名 `astersql-br-pkg-restore-snap-client` 及 Rust 路径形式的搜索只命中它自己的 manifest，因而目前只能确认它作为 workspace 库可独立构建和测试，不能据此声称某个 Rust BR 可执行程序已经依赖或调用它。

## 核心职责

该文件承担四项边界职责：

1. 用 `#[path = "..."] pub mod ...` 将 9 个生产文件纳入同一个 crate：`stubs`、`systable_schema_update`、`systable_restore`、`placement_rule_manager`、`pipeline_items`、`import`、`pitr_collector`、`tikv_sender`、`client`。
2. 用 8 条 `pub use ...::*` 把除 `stubs` 外各生产模块的公开符号提升到 crate 根，使使用者可以写 `snap_client::SnapClient` 一类路径，而不必依赖内部文件布局。
3. 公开 `stubs` 模块本身，但不把其内容通配重导出。该模块是当前移植环境中的本地协议、客户端 trait 和内存实现适配层；`lib.rs:38-40` 的注释明确它不代表真实 PD/TiKV 生产依赖。
4. 仅在测试构建中挂载 11 个独立的 `*_test.rs`/`parity_test.rs` 模块，保持生产代码和 Rust 测试代码分文件。

文件顶部的 crate 级 `#![allow(...)]` 放宽死代码、Go 风格命名、未使用项和 Clippy 告警。这是整个 crate 的编译策略，反映当前 Go-to-Rust 语义移植阶段；它也意味着“无告警”不能作为某个导出已被真实调用的证据。

## 主要符号

本文件没有自行定义业务符号；它定义的是模块和导出边界。

- `pub mod stubs`：公开适配层命名空间，但不通配导出。其他生产模块通过 `crate::stubs::{...}` 使用 `Context`、`Result`、`PdClient`、`SplitClient`、`ImporterClient`、元数据结构和内存实现等。
- `pub mod client` 与 `pub use client::*`：公开快照恢复控制器 `SnapClient`、构造函数 `NewRestoreClient`、配置常量以及建库建表、checkpoint、限速和连接初始化等 API（证据：`client.rs` 的 `SnapClient`、`NewRestoreClient`、`impl SnapClient`）。
- `pub mod import` 与 `pub use import::*`：公开 `SnapFileImporter`、`SnapFileImporterOptions`、`KvMode`、`RewriteMode` 及下载/ingest 能力（证据：`import.rs`）。
- `pub mod pipeline_items` 与 `pub use pipeline_items::*`：公开 `PipelineTask`、`PipelineContext`、`PipelineConcurrentBuilder`、统计元数据缓冲及流水线处理函数（证据：`pipeline_items.rs`）。
- `pub mod pitr_collector` 与 `pub use pitr_collector::*`：公开 PiTR 收集依赖 `PiTRCollDep` 和收集器相关构造/生命周期能力（证据：`pitr_collector.rs`）。
- `pub mod placement_rule_manager` 与 `pub use placement_rule_manager::*`：公开 `PlacementRuleManager`、在线/离线实现和 `NewPlacementRuleManager`（证据：`placement_rule_manager.rs`）。
- `pub mod systable_restore`、`systable_schema_update`：公开临时系统表识别/迁移、兼容性检查以及统计表 schema 版本升降级逻辑。
- `pub mod tikv_sender` 与 `pub use tikv_sender::*`：公开文件范围排序校验、split point 和恢复 SST 文件的发送流程。
- `export_test`、`main_test`、`parity_test` 及 8 个按生产模块命名的测试模块：均为私有且受 `#[cfg(test)]` 保护，不进入正常库构建的 API。

通配重导出会把子模块未来新增的任意 `pub` 项自动暴露到 crate 根。这能保持 Go 包式的扁平调用体验，但也扩大了 API 漂移风险；同名公开项还可能在 crate root 产生冲突。

## 执行流程

`lib.rs` 没有运行期执行流程。它影响的是编译期名称解析和测试装配：

1. Cargo 读取 `Cargo.toml`，以本文件作为库入口。
2. 编译器先应用 crate 级 lint 允许列表，再按显式 `#[path]` 找到 9 个生产模块；这些声明决定模块属于该 crate，并不按声明顺序执行模块业务。
3. `pub use` 将 8 个模块的公开项加入 crate 根导出面。外部使用者若存在，可以从根路径构造 `SnapClient`、`SnapFileImporter` 或调用系统表/placement/发送辅助函数。
4. 业务流程随后发生在被调用的实际符号中。例如 `NewRestoreClient` 建立 `SnapClient` 状态，`SnapClient::LoadSchemaIfNeededAndInitClient`/`InitConnections` 装配客户端，`SnapFileImporter::Import` 执行 SST 下载与 ingest，`PipelineConcurrentBuilder::StartPipelineTask` 运行表级后处理；这些都不是 `lib.rs` 主动串联的调用。
5. 使用 `cargo test` 编译此 crate 时，11 个 `#[cfg(test)]` 模块额外加入 crate，测试可以通过 `crate::client` 等内部路径检查私有协作边界；普通库构建完全排除这些模块。

因此，“加载这个 crate”不会自动连接 PD/TiKV、创建线程、执行 SQL 或恢复数据。调用次序和资源清理由实际入口及各实现模块决定。

## 数据与状态

本文件不创建或保存业务数据。唯一的 crate 级配置是静态编译属性：lint 允许列表、模块可见性、路径映射和测试条件编译。

经门面公开的主要状态所有者包括：

- `client::SnapClient`：持有 restorer/importer、PD/存储适配器、TLS、备份元数据、数据库会话池、checkpoint、策略模式、并发与限速等恢复控制状态。
- `import::SnapFileImporter`：持有每 store 下载/ingest token、PD 请求 token、条件变量、回调、KV/rewrite 模式和原始键范围。
- `pipeline_items::PipelineConcurrentBuilder`：持有表处理阶段及各阶段并发度；`statsMetaItemBuffer` 用互斥保护待刷新的统计元数据。
- `pitr_collector::pitrCollector`：管理 SST/重写规则收集、并发复制和迁移元数据提交生命周期。
- `placement_rule_manager::onlinePlacementRuleManager`：记录 restore store 与需要设置/清理规则的表 ID；离线实现不保存这些状态。

这些状态均定义在子模块，`pub use` 只改变访问路径，不复制对象，也不增加缓存或全局单例。

## 依赖与调用关系

向下依赖分两层：

- 模块层：本文件直接纳入同目录 9 个 `.rs` 文件。`client` 依赖 `import`、`pitr_collector`、`pipeline_items`、`placement_rule_manager`、`systable_restore` 和 `stubs`；其余模块也主要通过 `crate::stubs` 使用共同的协议与 trait。
- Cargo 层：manifest 的正常依赖只有 `astersql-br-pkg-utils`、`astersql-errors`、`astersql-br-pkg-errors`、`astersql-br-pkg-restore`、`serde`、`serde_json`、`sha2`，测试依赖为 `astersql-session`。manifest 注释说明 arm64 Darwin 版本没有真实 kv/domain/kvproto/grpcio，当前使用本地 trait/stub。

向上关系需要谨慎表述：RustCodeGraph 对 `lib.rs` 只索引到文件和模块装配，没有可供 `callers`/`callees` 查询的函数节点；仓库 Cargo 搜索没有发现其他 crate 对该 package 的依赖声明。因此可验证的直接上游是 Cargo workspace/test harness，而不是某个已接线的 Rust 恢复命令。Go 侧则有真实包使用者，例如 `br/pkg/restore/log_client/client.go` 导入 `br/pkg/restore/snap_client`；这只能证明 Go 应用链，不能反推 Rust crate 已接线。

测试关系是明确的：`lib.rs` 直接挂载 `client_test.rs`、`import_test.rs`、`pipeline_items_test.rs`、`pitr_collector_test.rs`、`placement_rule_manager_test.rs`、`systable_restore_test.rs`、`systable_schema_update_test.rs`、`tikv_sender_test.rs`，以及跨模块的 `export_test.rs`、`main_test.rs`、`parity_test.rs`。

## 错误处理与边界

本文件没有 `Result` 返回值或错误分支。模块文件缺失、重复导出名称、依赖不满足和测试模块编译失败都表现为编译期错误，而不是运行期恢复错误。

运行期错误边界在重导出的实现中：例如在线 placement 缺少 `SplitClient` 时 `NewPlacementRuleManager` 返回错误；规则清理会汇总失败表 ID； importer 的配置、region 扫描、下载、解密和 ingest 错误通过 `Result` 传播；系统表与文件范围函数会拒绝不兼容 schema 或非法范围。`lib.rs` 不捕获、包装或降级这些错误，因此从 crate 根调用与从原模块调用具有相同错误语义。

需要特别注意三个边界：

- `stubs` 是公开模块却不是通配根导出，调用者必须显式写 `snap_client::stubs::...`。
- `#[cfg(test)]` 辅助模块不是生产 API；不能让生产实现依赖 `export_test` 中的测试包装器。
- `#![allow(clippy::all)]` 等属性会隐藏风格/未使用告警，但不会把桩能力变成真实传输实现，也不会证明导出已接入应用。

## 并发与资源生命周期

`lib.rs` 自身不生成线程、任务、通道、锁、网络连接或事务，也没有 `Drop`/关闭逻辑。模块声明和重导出在编译期生效，不存在运行期先后顺序或释放顺序。

门面暴露的实现包含重要生命周期约束：`SnapClient` 管理 importer、会话池、checkpoint runner 和限速回调；`SnapFileImporter` 用 token 池和条件变量做每 store 背压，并在 `Close` 中运行关闭回调；流水线按阶段并发处理表并在阶段结束后收束错误；PiTR collector 限制并发复制并持久化提交元数据；在线 placement manager 在恢复前设置规则、结束时删除规则。相关改动应进入对应模块并由独立测试验证，不能在 `lib.rs` 中新增隐藏的全局资源。

现有 Rust 测试覆盖了这些边界，包括 importer 的 PD token 流控、每 peer 批量下载并行、取消与重试，pipeline 并发及统计元数据重试，PiTR 并发/冲突/重开，以及 placement 等待 region 就绪。`main_test.rs::test_main_initializes_shared_cluster` 只属于测试 fixture 生命周期，不是该库的生产启动函数。

## 与 Go 版本的对应关系

Go 没有对应的单一 `lib.go`：同目录所有 `package snapclient` 生产文件天然组成一个扁平包。Rust 需要显式 crate root，所以本文件用 `mod + pub use` 模拟 Go 包级命名空间。对应关系如下：

- `client.rs` ↔ `client.go`
- `import.rs` ↔ `import.go`
- `pipeline_items.rs` ↔ `pipeline_items.go`
- `pitr_collector.rs` ↔ `pitr_collector.go`
- `placement_rule_manager.rs` ↔ `placement_rule_manager.go`
- `systable_restore.rs` ↔ `systable_restore.go`
- `systable_schema_update.rs` ↔ `systable_schema_update.go`
- `tikv_sender.rs` ↔ `tikv_sender.go`

Go 的 `export_test.go` 在包内为测试开放私有能力；Rust 以 `#[cfg(test)] #[path = "export_test.rs"] mod export_test` 实现相同目的。Go 的多个 `*_test.go` 多使用外部测试包 `snapclient_test`，Rust 独立测试文件则被编译为 crate 内私有模块，因而可见性模型并不完全相同。`parity_test.rs::go_rust_public_contract_matches` 综合校验公开常量、placement、系统表、导入范围、文件范围、client helper 和 PiTR 元数据等契约。

当前 Rust manifest 明确采用本地 trait/stub，依赖集合也远小于 Go 包的真实 TiDB/PD/TiKV 依赖。因此 Rust 实现是在对齐可观察语义的移植库，不能把 Go 生产链路的网络与存储能力视为 Rust 侧已经具备。具体行为差异应以对应 `.rs`/`.go` 和同名测试为准，而不是由本门面的通配导出推断。

## 扩展指南

新增或修改功能时按所有权选择接入点：控制器配置、schema/DDL/checkpoint 进 `client.rs`；下载、region 扫描、SST 构造和 ingest 进 `import.rs`；表级并发后处理进 `pipeline_items.rs`；PiTR 复制/提交进 `pitr_collector.rs`；placement 进 `placement_rule_manager.rs`；系统表进两个 `systable_*` 模块；文件范围和 TiKV 发送进 `tikv_sender.rs`。不要把业务逻辑塞进 crate root。

新增生产模块时，应：

1. 创建独立 `.rs` 实现文件及独立 `*_test.rs`，保留许可证约定；测试逻辑不要内嵌到生产文件。
2. 在本文件增加显式 `#[path] pub mod`；只有确实需要 Go 包式根路径时才增加 `pub use`，并先检查重名和 API 扩张。
3. 若使用了新 crate，更新 `Cargo.toml`；若要进入真实应用，还必须在上游 crate manifest 中声明依赖并提供实际入口接线，不能以 workspace member 身份替代。
4. 同步 Go 对照语义和相应 Rust 独立测试；公开契约变化还应更新 `parity_test.rs`。涉及跨模块测试辅助时更新 `export_test.rs`，但保持其 `cfg(test)` 隔离。
5. 若替换 `stubs` 为真实客户端，逐项核对 trait、错误身份、取消、背压、重试与关闭顺序；这是兼容性和性能风险最高的边界。

仅重排模块声明通常不改变行为，但删除 `pub use` 会破坏根路径 API，新增通配重导出可能造成名字冲突。并发度、通道容量、批量阈值或重试策略不属于本文件，应在实际所有者中修改并运行对应测试。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`files --filter br/pkg/restore/snap_client` 列出本目录 39 个 Go/Rust 生产与测试文件。
- RustCodeGraph `node --file br/pkg/restore/snap_client/lib.rs --offset 1 --limit 240`：确认文件共 118 行、9 个生产模块、8 条通配重导出、11 个 `cfg(test)` 测试模块，且无业务函数/类型。
- RustCodeGraph 对 `client.rs`、`import.rs`、`pipeline_items.rs`、`placement_rule_manager.rs` 的文件节点：确认 `SnapClient`、`SnapFileImporter`、pipeline 和 placement 的真实状态与主入口位于子模块。
- `br/pkg/restore/snap_client/Cargo.toml` 与根 `Cargo.toml`：确认 crate 名、lib 路径、Go 包元数据、依赖集合和 workspace 成员身份。
- Cargo/Rust 全仓搜索 `astersql-br-pkg-restore-snap-client|astersql_br_pkg_restore_snap_client`：除自身 manifest 外无命中，故未把 Rust 可执行程序接线写成既成事实。
- Go 同目录 `client.go`、`import.go`、`pipeline_items.go`、`pitr_collector.go`、`placement_rule_manager.go`、`systable_restore.go`、`systable_schema_update.go`、`tikv_sender.go` 及其独立测试：确认 Rust 模块与 Go 包文件的对应面。
- Rust 独立测试 `client_test.rs`、`import_test.rs`、`pipeline_items_test.rs`、`pitr_collector_test.rs`、`placement_rule_manager_test.rs`、`systable_restore_test.rs`、`systable_schema_update_test.rs`、`tikv_sender_test.rs`、`parity_test.rs`、`export_test.rs`、`main_test.rs`：确认根模块挂载的测试面及关键边界。

本任务为纯文档分析，按计划不运行 Cargo。结构校验只验证目标文档存在且恰有 11 个规定的二级标题；运行期网络、真实 PD/TiKV 行为和完整应用接线未在本任务本地验证。
