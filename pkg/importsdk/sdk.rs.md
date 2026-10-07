# `pkg/importsdk/sdk.rs`

## 文件定位

本文件是 `astersql-importsdk` crate 的统一门面实现。crate 入口 `pkg/importsdk/lib.rs` 通过私有 `mod sdk` 装入本模块，再以 `pub use sdk::*` 导出这里的 `SDK`、`ImportSDK` 和 `NewImportSDK`。它不自行实现文件发现、作业 SQL 或 `IMPORT INTO` SQL 拼接，而是把 `FileScanner`、`JobManager`、`SQLGenerator` 三个子系统组合成一个对象。

`pkg/importsdk/Cargo.toml` 指定 `lib.rs` 为库入口，并以 `package.metadata.porting.go-package = "pkg/importsdk"` 标明 Go 对照包。该 manifest 没有定义 feature；本文件也没有条件编译项。RustCodeGraph 的文件关系显示 `sdk.rs` 被 `pkg/importsdk/mock/sdk_mock.rs`、`pkg/importsdk/mock/sdk_mock_test.rs`、`pkg/importsdk/sdk_test.rs` 和 `pkg/importsdk/sql_generator_test.rs` 使用；索引没有给出生产调用点，因此本文不把尚未核实的应用接线描述成现状。

## 核心职责

1. 用组合 trait `SDK: FileScanner + JobManager + SQLGenerator` 定义导入 SDK 的统一能力面，并额外约定资源关闭方法。
2. 由 `ImportSDK` 分别保存扫描器、默认作业管理器和 SQL 生成器，使三个职责仍由各自实现承担。
3. 由 `NewImportSDK` 从默认配置开始顺序应用 `SDKOption`，优先构造可能失败的文件扫描器，再组装其余无失败构造步骤。
4. 为三个子接口逐方法转发，保持调用者只需持有一个 `ImportSDK`；`SDK::Close` 明确转发至 `FileScanner::Close`。

因此，本文件的价值在于 API 聚合、构造时序和所有权组织，而不是业务算法。扫描规则在 `file_scanner.rs`，作业状态解码在 `job_manager.rs`，SQL 生成规则在 `sql_generator.rs`。

## 主要符号

- `pub trait SDK: FileScanner + JobManager + SQLGenerator`：组合接口。父 trait 都是 `Send + Sync`，所以实现 `SDK` 的对象同时满足三组能力的线程间传递/共享约束。这里声明的 `Close(&mut self) -> Result<(), SharedError>` 与 `FileScanner::Close` 同名，调用时可用 trait 限定消除歧义。
- `pub struct ImportSDK`：具体门面，字段均为私有。
  - `file_scanner: Box<dyn FileScanner>`：有状态扫描器，负责外部存储、MyDump loader、建库建表、元数据与大小估算。
  - `job_manager: JobManagerImpl`：持有数据库抽象，负责提交、查询和取消 import job。
  - `sql_generator: Box<dyn SQLGenerator + Send + Sync>`：无状态 SQL 生成器。`SQLGenerator` 本身已要求 `Send + Sync`，字段上的重复界限把门面的线程安全要求写得更显式。
- `pub fn NewImportSDK(ctx, source_path, db, options) -> Result<ImportSDK, SharedError>`：唯一构造入口。`ctx` 用 `Any + Send + Sync` 抽象 Go 的 context 形状；`db` 是 `Arc<dyn JobDatabase>`；选项是一次性闭包 `Box<dyn FnOnce(&mut SDKConfig) + Send>`。
- `impl FileScanner for ImportSDK`：转发 9 个方法，包括三个 `...Parts` 兼容形状和 `Close`。
- `impl JobManager for ImportSDK`：转发提交、状态查询、取消、组汇总和按组列举及其 `...Parts` 变体。
- `impl SQLGenerator for ImportSDK`：转发 `GenerateImportSQL` 与 `GenerateImportSQLParts`。
- `impl SDK for ImportSDK`：用 `FileScanner::Close(self)` 实现组合接口的关闭操作。

## 执行流程

构造流程由 `NewImportSDK` 固定：

1. 调用 `defaultSDKConfig()` 得到默认配置（例如并发度 4、默认用户表过滤和 `utf8mb4`；完整字段见 `config.rs::SDKConfig`）。
2. 按调用方提供的 `Vec<SDKOption>` 顺序逐一消费选项。后执行的选项可以覆盖前一选项写入的同一配置字段。
3. 调用 `NewFileScanner(ctx, source_path, Arc::clone(&db), config)`。该下游会解析源 URL、创建对象存储、配置并创建 MyDump loader；任一步失败都直接由 `?` 返回，门面不会被部分构造。
4. 扫描器成功后，用原始 `db` 构造 `NewJobManager(db)`，再用 `NewSQLGenerator()` 创建无状态生成器，最后返回 `ImportSDK`。

运行阶段没有额外编排：每个公开方法立即委托给对应字段并原样返回结果。例如 `CreateSchemasAndTables` 进入扫描器，`SubmitJob` 进入作业管理器，`GenerateImportSQL` 进入 SQL 生成器。关闭时，`SDK::Close` 和 `FileScanner for ImportSDK::Close` 最终都到达内部扫描器的 `Close`。

## 数据与状态

门面自身不缓存表元数据、作业状态或生成的 SQL。可变状态集中在 `file_scanner`：其具体 `fileScanner` 保存脱敏后的源路径、外部存储 `Option<StorageRef>`、MyDump loader、配置和数据库引用。需要扫描或构建元数据的方法采用 `&mut self`；只读的总大小查询采用 `&self`。

数据库句柄以 `Arc<dyn JobDatabase>` 进入构造器。`NewImportSDK` 克隆一份给扫描器，原值移入 `JobManagerImpl`，因此建表路径与作业管理路径共享同一个数据库实现而不复制底层资源。`sqlGenerator` 是零字段类型，不保存调用间状态。

配置只在构造阶段可变。`SDKOption` 是 `FnOnce`，每个选项恰好应用一次；配置随后整体移入扫描器，门面不保留第二份配置。这也意味着构造后无法通过本文件动态重配扫描并发、路由或过滤器。

## 依赖与调用关系

上游边界是 `lib.rs` 的再导出以及调用 `NewImportSDK`/`ImportSDK` 方法的代码。RustCodeGraph 对本文件给出的直接使用文件均为 mock 或测试；`NewImportSDK` 精确查询同时找到 Go 对照、当前 Rust 实现、Lightning 桩和 RealTiKV 测试桩，但图查询未返回可确认的 Rust 生产调用者。Lightning 的同名桩不是本实现的调用者，不能据此推断运行时已接线。

下游关系如下：

- `config.rs::{defaultSDKConfig, SDKOption}`：建立并覆盖扫描配置。
- `file_scanner.rs::NewFileScanner`：构造最重且唯一可失败的组件；依赖对象存储和 MyDump loader。扫描器各方法继续负责 schema 导入、元数据发现、大小统计与估算。
- `job_manager.rs::{JobDatabase, NewJobManager, JobManagerImpl}`：复用数据库抽象执行 import-job SQL；`JobManagerImpl` 内部以 `Arc` 保存数据库。
- `sql_generator.rs::NewSQLGenerator`：返回隐藏具体类型的 trait 对象，按 `TableMeta` 和 `ImportOptions` 生成 SQL。
- `astersql_errors::SharedError`：三组接口统一的共享错误类型。

Cargo 层面，本文件直接体现的是 `astersql-errors` 与标准库；其所聚合组件把 crate 连接到对象存储、Lightning mydump/config/log、executor importer、parser、planner/table/meta 等依赖。不能把 manifest 中所有依赖都归因于本文件的直接调用。

## 错误处理与边界

`NewImportSDK` 不包装 `NewFileScanner` 的错误，而是用 `?` 保留其 `SharedError`。扫描器负责给存储 URL 解析、外部存储创建和 loader 创建错误增加上下文，并对凭据做脱敏；`sdk_test.rs::canonical_sdk_rejects_invalid_source_without_leaking_credentials` 验证非法 URL 的错误包含 `<redacted-invalid-source>` 且不包含 secret query value。

构造的原子性边界很清楚：只有文件扫描器可能失败；失败时不创建门面。`NewFileScanner` 在存储已创建但 loader 创建失败时主动关闭存储。扫描器成功后，`NewJobManager` 与 `NewSQLGenerator` 当前均为无失败构造函数。

门面的转发方法不捕获、不改写也不吞掉下游错误。`...Parts` 方法保持 Go 风格的“值加可选错误”形状，但在本文件中仍只是转发；默认转换语义定义在相应 trait 中。关闭当前总是返回 `Ok(())`，因为底层 `StorageRef::Close()` 没有可传播结果；调用方不应据此推断所有未来资源关闭都不会失败。

## 并发与资源生命周期

三个父 trait 都受 `Send + Sync` 约束，`JobDatabase` 也要求 `Send + Sync`，因此类型设计允许门面及共享数据库跨线程使用。不过需要扫描/建表/估算和关闭的 API 使用 `&mut self`，Rust 借用规则会阻止同一个普通 `ImportSDK` 实例在这些操作间无同步地并发可变访问；本文件没有启动线程、任务或通道，也没有内部锁。

扫描并发由下游配置控制：`NewFileScanner` 将正数 `config.concurrency` 传给 MyDump 扫描，并在 schema importer 中至少使用 1 个 worker。门面只传递配置，不管理 worker 生命周期。

外部存储是需要显式结束的主要资源。具体扫描器以 `Option<StorageRef>` 持有它，`Close` 通过 `take()` 取走并关闭，因此重复关闭是幂等的。`ImportSDK` 没有自定义 `Drop`，所以使用方应显式调用 `SDK::Close` 或 `FileScanner::Close`；Go 测试通过 `defer importSDK.Close()` 表达同一生命周期，而 Rust 测试在相关路径末尾显式调用 `sdk.Close()`。数据库 `Arc` 的生命周期由引用计数决定，本文件不关闭数据库。

## 与 Go 版本的对应关系

`pkg/importsdk/sdk.go` 与本文件保持相同骨架：Go 的 `SDK` 嵌入三个接口并增加 `Close`；`importSDK` 嵌入三个接口值；`NewImportSDK` 先应用默认配置与 options，再依次创建 scanner、job manager 和 generator；`Close` 仅关闭 scanner。

主要语言层差异是：

- Go 构造器返回 `SDK` 接口，Rust 返回具体 `ImportSDK`，但 `lib.rs` 同时导出组合 trait，调用方仍可自行装箱为 trait object。
- Go options 是可变参数 `...SDKOption`，Rust 用拥有所有权的 `Vec<SDKOption>`，并以 `FnOnce` 保证单次消费。
- Go 使用 `*sql.DB` 和 `context.Context`；Rust 以 `Arc<dyn JobDatabase>` 隔离数据库实现，以 `&(dyn Any + Send + Sync)` 保留上下文参数位置，但本文件不读取其内容。
- Go 通过接口嵌入自动转发方法；Rust 必须显式实现并逐项委托，因此新增父 trait 方法时必须同步更新 `ImportSDK` 的 impl。
- Go 的 `importSDK` 为包内类型，Rust 的 `ImportSDK` 是公开结构体但字段私有。

测试意图大体对齐。Go 的 `sdk_test.go` 使用 fake GCS server 与 `go-sqlmock`；Rust 的独立 `sdk_test.rs` 因当前 crate 未接入 fake GCS/`gs://` 测试后端，改用临时目录 `file://` 和内存 `JobDatabase`，覆盖 Dumpling SQL、CSV、只有数据文件、扫描上限、跳过无效文件、按名建表以及非法源脱敏。这个测试环境差异不应被表述为 GCS 行为已由 Rust 测试验证。

## 扩展指南

- 新增一项跨子系统的 SDK 能力时，先决定它属于 `FileScanner`、`JobManager` 还是 `SQLGenerator`。属于现有接口的，应在定义文件及 `ImportSDK` 对应 impl 增加委托，并同步更新 `pkg/importsdk/mock/sdk_mock.rs`；不要在门面重复业务逻辑。
- 修改构造参数或配置时，应保持“应用全部 options → 构造扫描器 → 构造其余组件”的错误原子性。若新增可能失败的后续组件，必须设计已创建扫描器/存储的回滚或 RAII 清理，避免部分构造泄漏资源。
- 新增需关闭的资源时，应明确所有权并扩展 `SDK::Close`；同时决定重复关闭语义和多资源关闭时的错误优先级。当前只关闭 scanner，不能直接套用为多资源策略。
- 修改任一父 trait 时，要同时检查 `sdk.rs` 的委托、`mock/sdk_mock.rs`、独立 `*_test.rs` 以及 Go 对照接口，避免 Rust 门面与 mock 漏方法。
- 构造与组合行为的回归测试放在 `pkg/importsdk/sdk_test.rs`，具体扫描、作业或 SQL 规则分别放在同目录对应的独立测试文件，不要把测试内嵌进生产源码。若补 GCS 行为，应先接入可用且可复现的 Rust 测试后端，再声明与 Go fake-GCS 覆盖等价。
- 兼容性风险主要是公开 trait/构造签名变化；正确性风险主要是遗漏委托或关闭路径；性能风险通常来自下游扫描并发与 `Arc` 后端，而本门面每次调用只增加一次动态分派。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`pkg/importsdk/sdk.rs` 被识别为 241 行、30 个符号。
- RustCodeGraph 源码与符号：读取 `sdk.rs` 全文件；查询 `NewImportSDK`、`ImportSDK`、`FileScanner`、`JobManager`、`SQLGenerator` 和 `SDKOption`；对 Rust `NewImportSDK` 执行 callers/callees 查询但未得到可用调用边，因此生产接线明确记为未验证。
- RustCodeGraph 下游源码：核对 `file_scanner.rs::FileScanner`、`NewFileScanner`、`fileScanner::Close`，`job_manager.rs::{JobDatabase, JobManager, JobManagerImpl, NewJobManager}`，`sql_generator.rs::{SQLGenerator, NewSQLGenerator}`，以及 `config.rs::{SDKOption, SDKConfig, defaultSDKConfig}`。
- 文件证据：读取 `pkg/importsdk/Cargo.toml`、`pkg/importsdk/lib.rs`、Go 对照 `pkg/importsdk/sdk.go`、Rust 独立测试 `pkg/importsdk/sdk_test.rs` 和 Go 测试 `pkg/importsdk/sdk_test.go`。
- 测试边界证据：Rust 测试包含非法源脱敏、SQL/CSV 源、文件路由、扫描限制和单表创建；Go 测试使用 fake GCS 与 sqlmock。本文未运行 Cargo，符合本纯文档任务约束。
- 结构校验使用任务指定命令，要求本文存在且恰有 11 个固定二级标题；最终退出码记录在任务交付报告中。
