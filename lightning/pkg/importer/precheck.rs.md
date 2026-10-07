# `lightning/pkg/importer/precheck.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-importer` crate；crate 根 `lightning/pkg/importer/lib.rs` 以 `#[path = "precheck.rs"] mod precheck` 装入并通过 `pub use precheck::*` 导出其接口。它对应 Go 文件 `lightning/pkg/importer/precheck.go`，处在 Lightning 导入控制器与具体预检实现 `precheck_impl.rs` 之间：上游把配置和运行时资源组装成 `PrecheckItemBuilder`，下游由 builder 根据 `CheckItemID` 创建实现了 `astersql_lightning_pkg_precheck::Checker` 的对象。本文件是接线和分发层，不实现每个检查项的判断算法。

`lightning/pkg/importer/Cargo.toml` 将该目录声明为 library crate，并直接依赖 checkpoints、importer-opts、precheck 三个相邻 crate；目标端 SQL、PD HTTP、mydump 等边界目前由 importer crate 内模块提供。源码没有 feature 或条件编译分支；测试模块由 `lib.rs` 中独立的 `precheck_test.rs` 引入，测试没有嵌入生产文件。

## 核心职责

1. `WithPrecheckKey` 为 importer 的 `Context` 注入预检所需的任务级对象；当前直接使用点是 `check_info.rs::Controller::clusterResource`，它用 `taskManagerKey` 传递任务管理器。
2. `NewPrecheckItemBuilderFromConfig` 从完整 `Config` 建立目标端 DB、目标信息 getter、mydump loader、源数据库元数据、统一的 `PreImportInfoGetter` 和 checkpoint DB，形成可供整条预检链复用的依赖集合。
3. `NewPrecheckItemBuilder` / `NewPrecheckItemBuilderWithKeyspaceName` 支持调用方用现成依赖直接构造 builder，并把 PD leader 查询封装成带静态地址回退的闭包。
4. `PrecheckItemBuilder::BuildPrecheckItem` 把检查 ID 映射到 `precheck_impl.rs` 的具体构造函数；`GetPreInfoGetter` 则把共享 getter 的 `Arc` 克隆给 builder 外的调用方。

上游实际执行骨架位于 `check_info.rs::Controller::doPreCheckOnItem`：取得 builder、同步 `keyspaceName`、调用 `BuildPrecheckItem`、执行 `Checker::Check`，最后把非空结果交给 `checkTemplate.Collect`。`import.rs::Controller::preCheckRequirements` 再通过 `DataCheck`、`ClusterIsAvailable`、`localResource`、`clusterResource`、`checkCDCPiTR` 等方法驱动这条链路。

## 主要符号

- `pub type precheckContextKey = String`：预检上下文键的别名；不是新的强类型。`taskManagerKey` 是当前定义的固定键，值为 `PRECHECK/TASK_MANAGER`。
- `pub fn WithPrecheckKey(ctx, key, val) -> Context`：把 `Arc<dyn Any + Send + Sync>` 写入上下文，值必须可在线程间安全共享。
- `pub struct PrecheckItemBuilder`：保存 `keyspaceName`、克隆后的 `Config`、`Vec<MDDatabaseMeta>`、共享的 `Arc<dyn PreImportInfoGetter>`、可选 checkpoint DB、PD 地址闭包及可选目标 DB。字段均公开，当前上游会直接更新 `keyspaceName`。
- `NewPrecheckItemBuilderFromConfig(...) -> Result<(PrecheckItemBuilder, Option<crate::Error>)>`：完整装配入口。外层 `Err` 表示无法建立 builder；元组中的可选错误表示 mydump loader 返回了可继续使用的部分结果。
- `NewPrecheckItemBuilder(...) -> PrecheckItemBuilder`：以 `cfg.TikvImporter.KeyspaceName` 为 keyspace，转交给带显式 keyspace 的构造函数。
- `NewPrecheckItemBuilderWithKeyspaceName(...) -> PrecheckItemBuilder`：建立 `pdAddrsGetter`。有 PD client 时查询 leader URL；查询失败或 URL 为空时回退到 `cfg.TiDB.PdAddr`，无 client 时直接返回该静态地址。
- `BuildPrecheckItem(checkID) -> Result<Box<dyn Checker>>`：支持 14 个 ID：大文件、源存储权限、目标表为空、源 schema、checkpoint、CSV header、目标集群容量、空 region、region 分布、集群版本、本地磁盘布局、本地临时 KV 目录、CDC/PiTR、PD 与 TiDB 同集群。未知 ID 返回 `unsupported check item`。
- `GetPreInfoGetter() -> Arc<dyn PreImportInfoGetter>`：只克隆引用计数句柄，不复制 getter 的内部状态。

## 执行流程

完整配置路径按以下顺序运行（`NewPrecheckItemBuilderFromConfig`）：

1. 顺序应用 `PrecheckItemBuilderOption`，得到 `PrecheckItemBuilderConfig`。
2. 折叠调用方的 mydump loader options，提取已设置的扫描并发；随后追加 Lightning 自身的扫描并发覆盖值 `max(RegionConcurrency, 1) * 2`。
3. `DBFromConfig` 建立目标 DB，再由 `NewTargetInfoGetterImpl` 组合配置、DB 与可选 PD HTTP client。
4. `mydump::NewLoader` 扫描源数据。成功则继续；若同时返回部分 loader 和错误，则保留 loader 并把错误放入最终元组；若没有 loader，则立即失败。
5. 从 loader 取得数据库元数据和源存储，调用 `NewPreImportInfoGetter` 建立源端/目标端统一查询入口。
6. 将 importer 配置中 checkpoint、源目录、backend、sorted KV、TiDB/PD 等实际被 checkpoint crate 使用的字段复制到它的精简配置，再调用 `OpenCheckpointsDB`。
7. 调用 `NewPrecheckItemBuilder`，建立 PD 地址闭包并返回 builder 与可能存在的 loader 告警。

执行检查时，`Controller::doPreCheckOnItem` 先按检查 ID 调用 `BuildPrecheckItem`。分发函数只把相应共享依赖传给具体构造器；返回对象的 `Check` 方法才执行 I/O 或业务判断。`Checker` trait 约定 `Ok(None)` 代表跳过，`Ok(Some(CheckResult { Passed: false, .. }))` 代表检查已执行但未通过，两者不能混同。

## 数据与状态

builder 是一次导入任务的预检依赖快照。`cfg` 与 `dbMetas` 在构造时拥有化，避免依赖调用方借用生命周期；`preInfoGetter`、checkpoint DB 和 PD 地址闭包通过 `Arc` 共享。`targetDB` 与 `checkpointsDB` 使用 `Option`，允许细粒度测试或精简接线路径不提供资源，但选择需要这些资源的检查项时，具体实现仍可能报告缺失或无法完成。

`pdAddrsGetter` 捕获构造时的静态 PD 地址和可选 client。leader 查询结果非空时优先使用动态 URL；任何查询错误及空 URL 都被降级为单元素静态地址列表。`keyspaceName` 初始来自配置或显式参数，执行前 `doPreCheckOnItem` 会以控制器当前 keyspace 再同步一次，因此新增使用该字段的检查必须考虑它可能在 builder 构造后更新。

`NewPrecheckItemBuilderFromConfig` 的双层错误通道是重要状态：`Result::Err` 没有可用 builder；`Ok((builder, Some(err)))` 有部分 loader 数据但带告警；`Ok((builder, None))` 为无告警完成。调用方不能只检查外层 `Result` 而丢弃第二项。

## 依赖与调用关系

上游调用链（RustCodeGraph）：

- `import.rs::Controller::Run -> preCheckRequirements`，后者调用各类预检包装方法。
- `check_info.rs::Controller::{ClusterIsAvailable, clusterResource, StoragePermission, localResource, ...} -> doPreCheckOnItem -> PrecheckItemBuilder::BuildPrecheckItem -> Checker::Check`。
- `check_info.rs::clusterResource -> WithPrecheckKey`，在检查上下文中加入任务管理器。
- `meta_service_group_test.rs` 直接构造 builder，验证 metadata service group 相关接线。

主要下游依赖：

- `config::Config` 决定 loader 并发、checkpoint、backend、TiDB/PD 与 keyspace 参数。
- `mydump::{NewLoader, MDDatabaseMeta}` 提供源文件扫描、数据库元数据和源存储。
- `get_pre_info::{NewTargetInfoGetterImpl, NewPreImportInfoGetter, PreImportInfoGetter}` 聚合目标端与源端查询。
- `checkpoints::OpenCheckpointsDB` 打开 checkpoint 后端；`ropts` 提供 builder options；`precheck` crate 提供 `CheckItemID` 与 `Checker` 契约。
- `precheck_impl.rs` 提供 14 个具体检查器构造函数。不同分支按最小需要传递配置、元数据、getter、checkpoint DB、PD 地址闭包或目标 DB。

## 错误处理与边界

`DBFromConfig`、目标 getter、pre-info getter 的错误经 `errors::Trace` 传播；checkpoint crate 的错误被转成 importer 的字符串错误。mydump loader 特别允许“部分 loader + 错误”继续，这一错误由返回元组保留。`BuildPrecheckItem` 对未列出的 ID 明确失败，不使用默认 checker，也不静默跳过。

PD leader 获取是刻意的容错边界：失败和空 URL 不向上传播，而回退静态配置。因此该闭包返回地址并不证明 leader 查询成功；具体检查仍需处理静态地址无效的情况。`WithPrecheckKey` 使用动态 `Any`，键名或具体值类型不一致只能在消费侧暴露，扩展上下文值时必须同时核对写入和读取类型。

当前完整配置路径为 checkpoint crate 使用 `checkpoints::context::Background()`，该精简 context 是 marker，不能携带传入 importer `Context` 的取消和值；源码注释已明确这一迁移边界。另一个边界是 importer options 到 mydump options 的转换目前只暴露扫描文件并发，不能把 Go 侧任意 loader option 都假定为已经等价移植。

## 并发与资源生命周期

本文件不创建线程、任务或通道。并发参数仅在 loader 构造前计算：最终追加的扫描并发为 `max(RegionConcurrency, 1) * 2`，避免零或负配置转换成无效的 `usize`。具体 loader 是否并行以及工作线程如何结束属于 mydump 实现，不由本文件管理。

共享对象通过 `Arc` 进入多个 checker；`pdAddrsGetter` 被约束为 `Send + Sync`，上下文值也必须 `Send + Sync`，使 checker 能跨线程安全持有这些句柄。builder 没有显式 `Drop` 或关闭方法；目标 DB、checkpoint DB、getter 和闭包的释放依赖所有 owner/`Arc` clone 离开作用域。`GetPreInfoGetter` 会延长 getter 生命周期，调用方应避免无界保留。PD client 被闭包捕获，其生命周期至少持续到 builder 及所有克隆闭包释放。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `precheck.go`：上下文键、builder 字段、完整配置构造、直接构造、PD leader 回退、检查 ID 分发和 getter 访问器均保留。14 个分发分支及各自依赖与 Go switch 对齐；Rust 测试和 Go 测试共同覆盖其中前 12 个基础检查项的 ID 回报一致性。

需要注意的语言与迁移差异：

- Go 的 `precheckContextKey` 是独立字符串类型，Rust 当前只是 `String` 别名；Rust 失去一层编译期键类型区分。
- Go builder 保存配置和元数据指针，Rust 保存克隆后的拥有值；接口对象与可选资源由 `Arc`/`Option` 表达。
- Go 完整构造器以 `(builder, error)` 同时表达部分 loader 结果，Rust 显式返回 `Result<(builder, Option<Error>)>`，把致命错误与可继续告警分层。
- Rust 在计算覆盖并发时使用 `RegionConcurrency.max(1)`；Go 直接乘以 2。Rust 还把 checkpoint 字段复制到相邻 crate 的精简配置，并使用 marker background context，这是当前 crate 拆分造成的接线差异。
- Go 的 builder 字段不导出，而 Rust 字段均为 `pub`；扩展时仍应优先通过构造函数维护不变量，不应把公开可写视为稳定的任意组装契约。
- Rust 独立回归测试额外验证未知 checkpoint driver 必须报错，防止悄悄替换为空 checkpoint DB。

## 扩展指南

新增检查项时，应先在 `lightning/pkg/precheck` 定义/核对 `CheckItemID` 与显示语义，在 `precheck_impl.rs` 新增实现 `Checker` 的类型和构造器，然后在 `BuildPrecheckItem` 增加唯一分支并只传入所需依赖。同步扩展独立的 `precheck_test.rs` ID 表，验证 `GetCheckItemID`，并同步 Go `precheck.go` / `precheck_test.go` 或明确记录有意差异；不要把测试写进本生产文件。

若新增依赖是所有检查共享的，应通过构造函数建立并存入 builder；若只属于单一检查，优先在对应构造器内部取得，避免扩大 builder 的资源生命周期。新增上下文值时要成对维护 key、写入值的动态类型和消费侧 downcast。新增 PD/DB/checkpoint I/O 时要明确取消传播、失败是否可回退以及资源关闭所有权，尤其不能误以为 checkpoint marker context 已继承 importer context。

修改完整配置构造路径时，重点回归三类风险：部分 loader 结果不能被误判为致命失败；checkpoint 配置字段不能漏映射或被空实现掩盖；调用方 options 与 Lightning 最终并发覆盖的顺序不能颠倒。性能上应关注源数据扫描重复、过大的扫描并发、以及为不需要某资源的检查过早建立网络连接。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter lightning/pkg/importer` 确认目标、Go 对照、独立测试和相邻实现均在图中。
- 源码与符号：`rustcodegraph node --file lightning/pkg/importer/precheck.rs` 完整读取 301 行；`node NewPrecheckItemBuilderFromConfig` 和 `node BuildPrecheckItem` 核对构造顺序、Go/Rust定义及下游构造器。
- 调用证据：`node preCheckRequirements`、`node doPreCheckOnItem`、`node clusterResource` 核对 `Run -> preCheckRequirements -> 检查包装方法 -> doPreCheckOnItem -> BuildPrecheckItem -> Checker::Check`，以及 `clusterResource -> WithPrecheckKey`。
- 契约证据：`lightning/pkg/precheck/precheck.rs::Checker` 明确 `Check` 的 `None`/失败结果语义；`lightning/pkg/importer/lib.rs` 明确模块导出和独立测试装配。
- crate 边界：`lightning/pkg/importer/Cargo.toml` 声明 library、Go package 元数据，以及 checkpoints/importer-opts/precheck 等直接依赖。
- 对照与测试：完整核对 `lightning/pkg/importer/precheck.go`、`precheck_test.go`、`precheck_test.rs`；Rust 测试覆盖 12 个基础分发分支和未知 checkpoint driver 错误，两个网络相关分支的构造语义由源码和 Go 对照确认，本任务按约束未运行 Cargo。
