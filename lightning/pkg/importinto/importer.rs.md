# `lightning/pkg/importinto/importer.rs`

## 文件定位

本文件是 Lightning `IMPORT INTO` 后端的生命周期总控。它不负责逐表 SQL 的拼装、作业轮询或 checkpoint 的具体持久化，而是把 `importsdk::SDK`、`CheckpointManager`、`JobOrchestrator` 和进度回调组装成一个可由 Lightning server 驱动的 `Importer`。crate 入口 `lightning/pkg/importinto/lib.rs` 通过 `mod importer; pub use importer::*;` 暴露这里的 API；crate 清单 `lightning/pkg/importinto/Cargo.toml` 将其声明为 `astersql-lightning-pkg-importinto` library，并用 `package.metadata.porting.go-package` 标明对应 Go 包为 `lightning/pkg/importinto`。

真实上游入口位于 `lightning/pkg/server/lightning.rs` 的 `ImportIntoImporter::Run`：它把 Lightning 配置转换为本 crate 的配置，用 `WithProgressUpdater` 接入 `LightningStatus`，调用 `NewImporter`、`Importer::Run`，最后显式调用 `Importer::Close`。因此，本文件位于“Lightning 后端选择”之后、“IMPORT INTO 作业提交/监控”之前。

该 Rust crate 当前还通过 `stubs.rs` 提供本地的配置、SQL、日志、context 和 SDK 边界类型；这反映当前移植形态，不能据此推断它已经直接链接 Go 生产依赖。`Cargo.toml` 的注释也明确说明，本地 stubs 用于覆盖 importsdk/SQL/objstore/config/common/log 边界。

## 核心职责

1. `NewImporter` 应用函数式选项，并为未注入的 SDK、checkpoint manager、logger 和 orchestrator 创建默认实现。
2. 构造阶段初始化 checkpoint，并从已有 checkpoint 恢复稳定的 `groupKey`；没有可恢复值时生成 `lightning-<UUID>`。
3. `Run`/`runOnce` 按固定顺序执行建表、读取表元数据、可选预检、提交并等待作业，以及成功后的可选 checkpoint 清理。
4. `Run` 区分普通 context 取消和 failover 取消：普通取消尝试在独立的 60 秒背景 context 内取消远端作业；failover 取消保留作业，供接管者继续处理。
5. `Close` 尽最大努力依次关闭 checkpoint manager、SDK 和数据库连接；单项关闭失败只记录告警，不阻止后续资源释放。
6. `Pause` 和 `Resume` 明确是兼容接口，目前仅记录“不支持”并返回成功，不改变任何作业状态。

## 主要符号

- `cancelTimeout: Duration`：普通取消之后等待 `JobOrchestrator::Cancel` 的上限，固定为 60 秒。
- `ErrFailoverCancel() -> Error`：创建用于比较 context cause 的 failover 取消错误。`Run` 通过 `errors::Cause` 与 `errors::ErrorEqual` 判断该特殊原因。
- `ProgressUpdater`：`Send + Sync` 的进度更新边界，含 `UpdateTotalSize(i64)` 和 `UpdateFinishedSize(i64)`。本文件只保存并向 orchestrator 传递它；实际调用发生在相邻的作业监控/进度路径。
- `ImporterOption = Box<dyn FnOnce(&mut Importer) + Send>`：一次性构造选项。`WithProgressUpdater`、`WithCheckpointManager`、`WithBackendSDK`、`WithOrchestrator` 分别注入可替换组件；`WithStripS3ExternalIDForImportSQL` 打开提交 SQL 时去除显式 S3 external ID 的开关。
- `Importer`：生命周期状态容器。公开字段包括不可变共享配置 `cfg`、数据库句柄 `db`、可注入的 SDK/checkpoint/orchestrator、带组件字段的 logger、跨重启作业组键 `groupKey`、进度回调及 S3 external ID 开关。
- `NewImporter(...) -> Result<Importer>`：公开构造入口。它在返回前保证 `sdk`、`cpMgr`、`orchestrator` 均已就绪，checkpoint 已初始化且 `groupKey` 已确定。
- `Importer::buildOrchestrator`：用当前 SDK、配置、group key、checkpoint manager、并发数、轮询/日志周期和进度回调构造默认 submitter 与 orchestrator。
- `Importer::Run`：公开执行入口，在 `runOnce` 失败且属于 context 取消时追加远端取消策略，最终始终返回原始执行结果。
- `Importer::runOnce`：单次导入的主干流程。
- `Importer::runPrechecks`：当前只注册 `NewCheckpointCheckItem`，执行失败时增加 `precheck failed` 上下文。
- `Importer::initGroupKey`：优先采用首个非空 checkpoint group key，否则生成新 UUID 前缀键。
- `Importer::{Pause, Resume, Close}`：兼容生命周期接口和资源收尾入口。

文件没有条件编译分支；测试模块由 `lib.rs` 统一在 `#[cfg(test)]` 下装配，生产实现与测试逻辑保持在独立文件中。

## 执行流程

构造流程如下：

1. `NewImporter` 先保存 `cfg` 与 `db`，其余可注入组件为空，默认 `stripS3ExternalIDForImportSQL = false`。
2. 按传入顺序消费 `Vec<ImporterOption>`。同一字段若被多个选项设置，后一个选项覆盖前一个。
3. 若 logger 未初始化，则建立带 `backend=import-into` 字段的默认 logger。
4. 若没有注入 SDK，则从配置提取 SQL mode、过滤器、文件路由、表路由、字符集、CSV 配置和 logger，调用 `importsdk::NewImportSDK`。创建失败立即返回。
5. 若没有注入 checkpoint manager，则调用 `NewCheckpointManager(&cfg)`；不支持的 driver 等错误直接终止构造。
6. 调用 `CheckpointManager::Initialize`，随后由 `initGroupKey` 读取全部 checkpoint。首个非空 `GroupKey` 被恢复；否则生成新键。
7. 若没有注入 orchestrator，则由 `buildOrchestrator` 创建默认实现。此时依赖已齐备，内部的 `unwrap` 建立在前述构造不变量上。

运行流程如下：

1. `Run` 调用 `runOnce`。
2. `runOnce` 先调用 `SDK::CreateSchemasAndTables`，再调用 `SDK::GetTableMetas`；任一步失败都以 `?` 原样提前返回，后续预检、提交和清理不执行。
3. 当 `cfg.App.CheckRequirements` 为真时调用 `runPrechecks`；否则只记录跳过日志。
4. 调用 `JobOrchestrator::SubmitAndWait(ctx, &tables)`。默认 orchestrator 内部负责并发提交、监控和进度更新，本文件不自行创建线程或轮询循环。
5. 只有上述步骤全部成功，并且 checkpoint 已启用且 `KeepAfterSuccess == CheckpointRemove` 时，才调用 `CheckpointManager::Remove(ctx, AllTables)`。删除失败只告警，不把成功导入改为失败。
6. 若 `runOnce` 返回 context-canceled 类错误，`Run` 检查 context cause。cause 等于 `ErrFailoverCancel()` 时跳过远端取消；否则以 `context::Background()` 派生 60 秒超时 context 调用 orchestrator `Cancel`。取消成功或失败都不替换 `runOnce` 的原始错误。

## 数据与状态

- `cfg: Arc<config::Config>` 在构造后由 submitter、orchestrator 和 importer 共享；本文件不在运行期修改配置。
- `sdk`、`cpMgr`、`orchestrator` 使用 `Option<Arc<dyn ...>>` 支持构造期注入。成功构造后逻辑上均为 `Some`；方法中的 `unwrap` 依赖这一不变量，绕过 `NewImporter` 直接手工构造会破坏它。
- `groupKey` 是同一次逻辑导入跨进程/故障恢复关联作业的标识。恢复策略是扫描 `GetCheckpoints` 的返回顺序并取第一个非空值；代码没有验证多个 checkpoint 的非空键是否一致。
- `progressUpdater` 是可选共享回调。`buildOrchestrator` 将其克隆进 `OrchestratorConfig`，本文件不缓存进度数字。
- `stripS3ExternalIDForImportSQL` 只影响默认 submitter 的选项；如果调用者同时注入自定义 orchestrator，该开关不会重建或修改该 orchestrator。
- `db` 按值保存在 importer 中，并在 `Close` 最后关闭。`Close` 返回 `()`，不向调用方暴露关闭错误。

关键状态不变量是：`NewImporter` 返回 `Ok` 后，checkpoint 初始化已完成、group key 已确定、运行依赖均存在；而导入成功并不意味着 checkpoint 一定被删除，因为删除由两个配置条件控制且删除错误被降级为告警。

## 依赖与调用关系

上游调用关系：

- `lightning/pkg/server/lightning.rs::ImportIntoImporter::Run` 是生产侧直接入口：创建进度适配器，调用 `NewImporter -> Run -> Close`。
- `lightning/pkg/importinto/importer_test.rs` 直接构造 importer，覆盖成功、各阶段错误、取消、group key 恢复、默认 orchestrator 和关闭路径。
- `lightning/pkg/importinto/parity_test.rs` 组合真实相邻组件边界，验证恢复 group key、运行、成功清 checkpoint 和关闭数据库等跨文件语义。

下游调用关系：

- `checkpoint.rs`：`NewCheckpointManager`、`CheckpointManager::{Initialize, GetCheckpoints, Remove, Close}`。
- `job_submitter.rs`：`NewJobSubmitter` 与 `WithJobSubmitterStripS3ExternalIDForImportSQL`。
- `job_orchestrator.rs`：`NewJobOrchestrator`、`JobOrchestrator::{SubmitAndWait, Cancel}`；配置中的提交并发来自 `cfg.App.TableConcurrency`，轮询周期来自 `DefaultPollInterval`，日志周期来自 `cfg.Cron.LogProgress.Duration`。
- `precheck.rs`：`NewPrecheckRunner` 和 `NewCheckpointCheckItem`。
- `stubs.rs` 中的 `importsdk::SDK`：默认 SDK 构造、schema/table 创建、表元数据获取与关闭；同文件还提供 context、error、logger、SQL DB 和 UUID 等当前 Rust 移植边界。

RustCodeGraph 对目标文件的文件节点报告其被 `importer_test.rs`、`job_monitor.rs`、`job_orchestrator.rs`、`lib.rs`、`mock/parity_test.rs` 等 7 个文件使用；对精确符号的调用边查询没有返回可用边，因此生产入口由上述直接引用搜索和源文件读取补证，未把宽泛同名搜索结果当成调用事实。

## 错误处理与边界

- 构造阶段的 SDK 创建、checkpoint manager 创建、checkpoint 初始化和 checkpoint 读取均是硬错误，使用 `?` 直接阻止 importer 返回。
- 主流程的建表、取表元数据、预检与 `SubmitAndWait` 都是硬错误。`runPrechecks` 会记录原错误，并用 `errors::Annotate(err, "precheck failed")` 增加上下文。
- 成功后的 checkpoint 全量删除是 best effort：失败只写 warning，`runOnce` 仍返回 `Ok(())`。扩展这里时不能误把维护性清理变成导入结果错误，除非同步改变 Go 契约和测试。
- 普通取消的 `Cancel` 也是 best effort；失败只告警。`Run` 返回的是原始 context/执行错误，而不是 cancel 错误。
- failover cause 是特殊边界：必须跳过 `Cancel`，否则接管流程可能失去仍需恢复的远端作业。
- `Pause`/`Resume` 的成功返回不表示真正暂停或恢复，只表示该后端接受了接口调用但不执行操作。
- `Close` 对三个资源分别处理错误并继续关闭后续资源。它没有幂等性声明；测试验证一次调用后的状态，但未证明任意实现均支持重复关闭。
- 内部 `unwrap` 不处理缺失依赖，因为成功构造已经建立存在性不变量。若未来公开结构字段被收紧或增加替代构造器，应同时维护这一约束。

## 并发与资源生命周期

本文件自身不启动线程、task 或 channel。并发由下游 `JobOrchestrator` 管理，`buildOrchestrator` 把 `cfg.App.TableConcurrency` 作为 `SubmitConcurrency` 传入。共享组件采用 `Arc<dyn Trait>`，相关 trait（包括 `ProgressUpdater`、`CheckpointManager`、`JobOrchestrator`、SDK）要求或用于线程安全共享；`ImporterOption` 也要求 `Send`，但选项只在构造过程中顺序执行。

生命周期顺序为“构造并初始化 checkpoint → 确定 group key → 构造 orchestrator → Run → Close”。生产入口显式保证 `Run` 后调用 `Close`，即使 `Run` 返回错误也会执行该语句；本类型没有 `Drop` 实现，因此其他调用者若遗漏 `Close`，本文件不会自动调用各资源的业务关闭方法。

取消时不用已取消的父 context，而是从 `context::Background()` 创建独立超时 context，确保仍有最多 60 秒执行远端作业取消。failover 则刻意保留远端资源。关闭时顺序固定为 checkpoint manager、SDK、DB，任何一步错误不会短路后续步骤。

## 与 Go 版本的对应关系

直接对照文件是 `lightning/pkg/importinto/importer.go`，主要结构和顺序保持一致：同样的 option 模式、默认 SDK/checkpoint/orchestrator、checkpoint 初始化、group key 恢复、运行阶段顺序、取消分支、无操作的 Pause/Resume，以及 best-effort Close。

语义映射包括：Go 接口值对应 Rust 的 `Arc<dyn Trait>`；Go `ImporterOption func(*Importer)` 对应 `Box<dyn FnOnce(&mut Importer) + Send>`；Go 指针配置对应 Rust `Arc<Config>`；Go 的 `time.Minute` 对应 Rust `Duration::from_secs(60)`。Rust 的 `Vec<ImporterOption>` 替代 Go variadic options，但调用顺序和覆盖语义相同。

可观察行为由两套独立测试互证：`importer_test.go::TestImporterRun/TestImporterNewImporter/TestImporterClose` 与 `importer_test.rs::test_importer_run/test_importer_new_importer/test_importer_close` 都覆盖建表错误、取元数据错误、预检错误、orchestrator 错误、普通取消、failover 取消、恢复 group key、非法 checkpoint driver 和关闭错误继续清理。

需要注意当前 Rust 生产接线的环境差异：`lightning/pkg/server/lightning.rs::ImportIntoImporter::Run` 通过 bridge 转换配置，并构造本 crate 的 background context 与内存 DB；Go `lightning/pkg/server/lightning.go::newImporter` 则把调用者 context、原配置和 `param.DB` 直接传给 Go importer。文档只确认本文件内部与 Go 的控制流对齐，不把这些 bridge/stub 差异描述为完全等价的外部运行环境。

## 扩展指南

- 新增构造依赖时，优先增加新的 `With...` option，并在 `NewImporter` 中明确默认创建时机；同时检查 `buildOrchestrator` 是否需要传递该依赖。不要允许 `Ok(Importer)` 留下后续会被 `unwrap` 的空字段。
- 修改运行阶段顺序时，应先核对 Go `Importer.runOnce`，并同步扩展 `lightning/pkg/importinto/importer_test.rs`，至少覆盖成功顺序、目标阶段错误后的短路以及 checkpoint 清理是否仍只发生在成功路径。
- 扩展预检应接入 `runPrechecks` 的 runner 注册点，并在独立的 `precheck_test.rs` 覆盖检查项自身，在 `importer_test.rs` 覆盖总控传播与错误注解。
- 修改取消语义必须保留“普通取消尝试 Cancel、failover 不 Cancel、Cancel 错误不覆盖原错误”三项契约，并同步 Rust/Go 对照测试。超时调整应评估远端清理耗时和 Lightning 退出延迟。
- 修改 group key 策略应同时检查 checkpoint 数据兼容性、多个非空键冲突策略和恢复作业的关联行为；对应测试应放在 `importer_test.rs`，checkpoint 存储细节放在 `checkpoint_test.rs`。
- 修改 S3 external ID 行为时，需要注意该 option 只影响本文件创建的默认 submitter；自定义 orchestrator 的行为由注入者负责。应同时覆盖 `job_submitter_test.rs` 和端到端组合路径。
- 增加资源时应把关闭逻辑加入 `Close`，保持“失败记录后继续”的清理风格，并在独立测试文件中验证前一资源关闭失败不阻断后一资源。
- 性能风险主要集中在 `TableConcurrency`、轮询周期和日志周期的传递；正确性风险集中在 group key 稳定性、checkpoint 成功后清理条件和取消原因判断；兼容风险集中在 Go/Rust option 默认值和错误传播差异。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter lightning/pkg/importinto` 确认目标、相邻实现和测试均被索引；`node --file lightning/pkg/importinto/importer.rs --offset 1 --limit 400` 读取了完整 365 行目标源；`query` 确认 `NewImporter`、`buildOrchestrator`、`runOnce`、`runPrechecks`、`initGroupKey` 的 Rust/Go 对照符号。精确 `callers/callees` 未返回可用结果，故调用边由直接引用补证。
- 生产与装配：`lightning/pkg/importinto/importer.rs`、`lightning/pkg/importinto/lib.rs`、`lightning/pkg/importinto/Cargo.toml`、`lightning/pkg/server/lightning.rs`。
- 直接下游定义：`lightning/pkg/importinto/checkpoint.rs`、`job_submitter.rs`、`job_orchestrator.rs`、`precheck.rs`、`stubs.rs`。
- Go 对照：`lightning/pkg/importinto/importer.go`、`lightning/pkg/server/lightning.go`。
- 独立测试：`lightning/pkg/importinto/importer_test.rs`、`lightning/pkg/importinto/importer_test.go`；组合语义另由 `lightning/pkg/importinto/parity_test.rs` 提供证据。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；验收使用任务指定的文件存在与固定 11 章节结构命令，并人工复核上述路径和符号可回溯。
