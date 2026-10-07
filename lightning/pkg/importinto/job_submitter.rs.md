# `lightning/pkg/importinto/job_submitter.rs`

## 文件定位

本文件是 `astersql-lightning-pkg-importinto` crate 的“单表 IMPORT INTO 作业提交”边界。模块由 [`lib.rs`](lib.rs) 通过 `mod job_submitter` 装配并整体重导出；上游 [`importer.rs`](importer.rs) 的 `Importer::buildOrchestrator` 创建提交器，再把它作为 `OrchestratorConfig::Submitter` 交给 [`job_orchestrator.rs`](job_orchestrator.rs)。因此它位于“Lightning 配置与表元数据”到“import SDK 生成 SQL 并向 TiDB 提交异步作业”的转换点，而不负责轮询、取消或 checkpoint 持久化。

crate 元数据在 [`Cargo.toml`](Cargo.toml) 中声明 Go 包对应关系为 `lightning/pkg/importinto`、库入口为 `lib.rs`。当前 Rust crate 仅直接声明 `url` 等少量依赖，`config`、`importsdk`、`objstore`、`ast`、日志和错误类型来自同 crate 的 [`stubs.rs`](stubs.rs)，所以这里的主流程与 Go 对齐，但 SDK、URL 脱敏和 S3-like 判断的完备性受这些本地适配层限制。

## 核心职责

1. 用 `JobSubmitter` trait 隔离编排器与具体提交实现，使编排器和测试替身只依赖 `SubmitTable`、`GetGroupKey` 两个契约。
2. `DefaultJobSubmitter::SubmitTable` 从 `TableMeta` 与 Lightning 配置构造 `ImportOptions`，调用 SDK 生成真实 SQL，再提交并返回带表元数据及组键的 `ImportJob`。
3. 单独生成一份用于日志的脱敏 SQL；日志生成失败不会阻断真实 SQL 的提交，真实 SQL 本身不会被脱敏版本替换。
4. 在兼容开关开启时，仅从 S3-like 源的 SQL 资源参数中删除 `external-id`，同时保持 `Config.Mydumper.SourceDir` 原值不变，以免改变 Lightning 自己访问对象存储时的凭据语义。

## 主要符号

- `ImportJob { JobID, TableMeta, GroupKey }`：成功提交后的轻量句柄。`TableMeta` 克隆进 `Option`，供编排器记录 checkpoint、监控及恢复；`GroupKey` 用于按组查询或取消。
- `JobSubmitter: Send + Sync`：线程安全抽象。`SubmitTable(&Context, &TableMeta) -> Result<ImportJob>` 执行一次提交，`GetGroupKey() -> String` 返回所属组键。
- `DefaultJobSubmitter`：保存共享 SDK、只读配置、组键、结构化 logger 和 external-id 兼容开关。`sdk`、`config` 使用 `Arc`，类型本身没有可变运行时作业状态。
- `JobSubmitterOption = Box<dyn FnOnce(&mut DefaultJobSubmitter) + Send>`：构造期一次性 option；`WithJobSubmitterStripS3ExternalIDForImportSQL` 设置兼容开关。
- `NewJobSubmitter(...) -> Arc<dyn JobSubmitter>`：先从 `cfg.TikvImporter.StripS3ExternalIDForImportSQL` 应用默认值，再依次应用调用方 options；后传 option 因此能覆盖配置值。
- `DefaultJobSubmitter::buildImportOptions`：把配置和首个数据文件的格式映射为 SDK 选项。
- `buildResourceParametersForImportSQL`：提取 URL query，并在满足开关与 S3-like 两个条件时过滤 external-id；发生过滤后按键排序，以对齐 Go `url.Values.Encode`。
- `stripS3ExternalIDResourceParameters`：按 `NormalizeQueryParameterKey` 归一化大小写及下划线/连字符后删除 external-id，返回是否实际删除过。
- `generateImportSQLForLog`：克隆元数据与选项，合并资源参数、脱敏 URL，再用独立 SQL generator 生成仅供日志使用的 SQL。
- `strip_s3_external_id_for_test`：公开测试辅助入口，只封装 URL 解析和过滤；生产调用链不使用它。

本文件没有模块级业务常量、条件编译项或内嵌测试模块。

## 执行流程

构造阶段：`Importer::buildOrchestrator` 收集调用方兼容 option，传入 SDK、配置、组键和带 `component=submitter` 字段的 logger。`NewJobSubmitter` 创建默认实现，先应用配置开关，再按顺序执行 `opts`，最后擦除为 `Arc<dyn JobSubmitter>`。

单表提交阶段：

1. `SubmitTable` 为日志附加数据库名和表名。
2. `buildImportOptions` 固定 `Detached=true`、复制 `GroupKey`，并把 `DisablePrecheck` 设为 `!CheckRequirements`。
3. 若存在数据文件，只以 `DataFiles[0]` 决定格式；CSV 时复制 CSV 配置，并在有 header 时设置 `SkipRows=1`。随后映射 `StrictFormat → SplitFile`、正数 `MaxError.Type → RecordErrors`、非空且非 `binary` 字符集，以及源 URL 查询参数。
4. SDK `GenerateImportSQL(tableMeta, options)` 生成将被实际提交的 SQL；失败立即返回带 `generate import SQL` 上下文的错误。
5. `generateImportSQLForLog` 尽力生成脱敏副本。成功则记录 SQL；失败只记录不带 SQL 的“submitting import job”，继续主流程。
6. SDK `SubmitJob(ctx, sql)` 使用原始 SQL 提交；失败返回带 `submit job` 上下文的错误。成功记录 job ID，并返回 `ImportJob`。

在编排器中，`DefaultJobOrchestrator` 会先检查 checkpoint：已完成表跳过；运行中作业以 checkpoint 的 job ID 和 `submitter.GetGroupKey()` 重建 `ImportJob`；否则调用 `SubmitTable`，把成功结果加入 `activeJobs` 并记录 submission。取消路径在没有 active job 时也以 `GetGroupKey` 作为回退组键。

## 数据与状态

`DefaultJobSubmitter` 的字段在构造完成后只读；唯一可配置字段 `stripS3ExternalIDForImportSQL` 只在返回 trait object 前由 options 修改。每次提交都创建新的 `ImportOptions`、日志上下文和返回对象，没有跨调用累积计数、缓存或作业列表。

关键选项不变量包括：提交为 detached；组键与返回 `ImportJob.GroupKey` 一致；关闭 Lightning requirement check 时同时关闭 IMPORT INTO precheck；只使用首个文件判断格式；`RecordErrors` 仅在配置值大于零时显式设置；`binary` 字符集不传给 SDK。源目录解析失败时资源参数保持默认空字符串，函数不会因此报错。

external-id 过滤只操作由 query 解码得到的新 `Vec<(String, String)>`，不修改配置中的源字符串。若未启用、scheme 非 S3-like、或没有匹配键，直接返回原始 query，从而保留原编码与顺序；只有确实删除键后才重新编码和按键排序。

## 依赖与调用关系

上游生产调用链为 `Importer::buildOrchestrator → NewJobSubmitter → NewJobOrchestrator`。直接消费方是 `DefaultJobOrchestrator::SubmitAndWait` 内部的并发提交闭包（调用 `SubmitTable`），以及其恢复/取消辅助逻辑（调用 `GetGroupKey`）。`lib.rs` 的 `ScriptJobSubmitter` 和 `mock/import_mock.rs` 则是独立测试替身。

下游调用集中在 `stubs.rs` 暴露的契约：`importsdk::SDK::{GenerateImportSQL, SubmitJob}`、`importsdk::NewSQLGenerator`、`objstore::{IsS3Like, NormalizeQueryParameterKey}`、`s3like::S3ExternalID`、`ast::RedactURL`，以及 `config`、`log`、`zap`、`errors`、`context`。外部 crate `url` 负责 URL 解析、query 解码和重新编码。

RustCodeGraph 的索引确认 `job_submitter.rs` 含 15 个符号，并将 `NewJobSubmitter`、`buildImportOptions`、`buildResourceParametersForImportSQL`、`generateImportSQLForLog` 分别解析为该文件的函数。图的 callers/callees 查询在本次会话中超时，所以上述直接调用边另由 `rg` 定位并读取 `importer.rs`、`job_orchestrator.rs` 和测试源码确认，而非根据未返回的图结果推断。

## 错误处理与边界

真实 SQL 生成错误和提交错误是硬失败，分别经 `errors::Annotate` 增加阶段上下文并停止后续步骤；生成 SQL 失败时不会调用 `SubmitJob`。日志 SQL 生成是 best effort：失败被有意忽略，只省略 SQL 字段，避免可观测性辅助逻辑阻塞导入。

`buildImportOptions` 对非法 `SourceDir` URL 静默跳过资源参数，这是与 Go `url.Parse` 出错后不设置该字段一致的宽容边界。`generateImportSQLForLog` 中 wildcard URL 解析失败时仍清空克隆选项的 `ResourceParameters` 并继续生成日志 SQL；这意味着其首要保证是“不把未脱敏参数重复拼入日志”，而不是保证日志 SQL 与真实 SQL 字节一致。

脱敏只影响日志副本：`WildcardPath` 与 `CloudStorageURI` 经 `ast::RedactURL`，真实 `tableMeta`、`options` 和提交 SQL不变。当前 Rust `ast::RedactURL` 是 `stubs.rs` 中面向 URL query 的局部实现，并非 Go parser/ast 的完整能力；扩展敏感键或 URI 语法时必须同时核对该边界。

`strip_s3_external_id_for_test` 对非法 URL 使用 `expect("url")`，可能 panic，但它是测试辅助函数，不在生产路径。生产路径的 URL 解析使用 `if let Ok`，不会 panic。

## 并发与资源生命周期

`JobSubmitter` 要求 `Send + Sync`，并通过 `Arc<dyn JobSubmitter>` 在编排器的并发闭包间共享。SDK 与配置也由 `Arc` 持有；提交器不持有锁，是否允许并发调用由 `SDK: Send + Sync` 契约和 SDK 实现负责。`SubmitTable` 只使用局部数据或克隆，不修改共享配置与表元数据。

传入的 `context::Context` 只向 `SubmitJob` 透传；SQL 生成和选项构建不接收 context。编排器负责创建 started-submit context、并发额度、active job 列表、checkpoint 记录、监控和取消；这些资源生命周期不属于本文件。

logger 被克隆并添加字段，不需要显式关闭。构造 option 是 `FnOnce`，在构造阶段消费后释放；返回对象内的 SDK、配置和 logger 随最后一个 `Arc` 引用释放。

## 与 Go 版本的对应关系

Rust 文件逐段对应 [`job_submitter.go`](job_submitter.go)：`ImportJob`、接口/trait、默认实现、functional option、构造器、提交主流程、选项映射、S3 external-id 过滤和日志 SQL 生成的顺序一致。Rust 用 `Arc<dyn Trait>` 表达 Go interface 共享，用 `Option<TableMeta>` 表达 Go 指针可空性，用 owned clone 代替 Go 指针字段。

重要语义对齐点：配置开关先应用、显式 option 后覆盖；S3-like 判断后才过滤；归一化键可匹配 `external-id`、`External_ID` 和编码后下划线形式；实际过滤后按键字典序编码；原始 source dir 不变；日志脱敏失败不影响提交；错误上下文分别是 `generate import SQL` 与 `submit job`。

[`job_submitter_test.rs`](job_submitter_test.rs) 是独立 Rust 测试文件，对齐 [`job_submitter_test.go`](job_submitter_test.go)，覆盖成功返回 job ID/组键、生成 SQL 失败、提交失败、默认保留 S3 external-id、option 与配置两种开启方式、OSS 作为 S3-like、GCS 参数保留、`GetGroupKey` 和日志凭据脱敏。Rust 另由 [`parity_test.rs`](parity_test.rs) 通过 `strip_s3_external_id_for_test` 做 crate 级兼容抽查。

当前差异主要来自移植边界：Go 连接真实 `pkg/importsdk`、对象存储与 parser AST；Rust 在本 crate 内使用精简 stubs，`ImportOptions` 只保留当前消费字段，mock SQL generator 也不等价于完整生产 generator。因此可以确认本文件控制流和已测试契约对齐，不能据此宣称所有真实 SDK/URI 行为已完整移植。

## 扩展指南

- 新增 IMPORT INTO option 时，优先修改 `buildImportOptions`，核对 Go 同名函数、`stubs.rs::ImportOptions` 和真实上游 SDK 契约；在 `job_submitter_test.rs` 增加独立测试，不要把测试内嵌到生产文件。
- 改变提交协议或返回元数据时，同步检查 `JobSubmitter`、`ImportJob`、`DefaultJobOrchestrator` 的恢复/记录/取消路径，以及 `lib.rs::ScriptJobSubmitter` 与 `mock/import_mock.rs` 的实现。
- 新增凭据字段或 URI scheme 时，同时更新 `generateImportSQLForLog` 下游的 `ast::RedactURL`、S3-like 判断和 Go 对照测试；任何日志改动都必须保证真实提交 SQL 不被脱敏副本替代。
- 调整 external-id 兼容逻辑时保留三个不变量：不改源配置、只处理 S3-like、未发生删除时返回原始 query。重新编码会改变顺序和转义，必须有 Go `url.Values.Encode` 对齐证据。
- 若需要把当前 crate 接到真实 Rust importsdk，应在独立上游仓库移植、提交并打 tag，再让本仓库 Cargo manifest 统一引用该 tag；不得把依赖复制进 vendor/third_party 或用本地 `[patch]`。
- 性能风险主要是每次提交都会克隆 `TableMeta`/`ImportOptions` 并额外生成一次日志 SQL；正确性风险主要是选项默认值、URL 重编码和脱敏覆盖不足；兼容性风险主要是 Go/Rust SDK 字段和 S3-like scheme 集合漂移。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7,032 个 Rust 文件；`files --filter lightning/pkg/importinto` 包含目标文件；`query` 确认目标 Rust 符号；`node --file ... --offset 1 --limit 400` 读取了目标文件完整 287 行。callers/callees 查询超时，未把空输出作为调用关系证据。
- 生产源码：[`job_submitter.rs`](job_submitter.rs)、[`importer.rs`](importer.rs) 的 `buildOrchestrator`、[`job_orchestrator.rs`](job_orchestrator.rs) 的 `getGroupKey` 与提交闭包、[`lib.rs`](lib.rs) 的模块装配、[`stubs.rs`](stubs.rs) 的 SDK/选项/对象存储/脱敏边界。
- crate 配置：[`Cargo.toml`](Cargo.toml) 的包名、库入口、Go 包映射和直接依赖。
- Go 对照：[`job_submitter.go`](job_submitter.go) 的全部同名生产逻辑。
- 测试证据：[`job_submitter_test.rs`](job_submitter_test.rs)、[`job_submitter_test.go`](job_submitter_test.go) 和 [`parity_test.rs`](parity_test.rs) 的 external-id 抽查；测试未运行，因为本计划明确是纯文档分析且禁止运行 Cargo。
- 人工复核结论：本文件存在于导入编排器与 SDK 之间，负责从单表配置生成/提交 SQL，并以独立脱敏 SQL提供安全日志；安全扩展应从 `buildImportOptions`、trait/返回对象或脱敏辅助函数切入，同时同步独立 Rust/Go 测试与 stubs/真实 SDK 契约。
