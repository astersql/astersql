# [`lightning/pkg/importinto/precheck.rs`](./precheck.rs)

## 文件定位

该文件属于 `astersql-lightning-pkg-importinto` library crate，是 IMPORT INTO 后端在真正提交导入任务前执行检查的实现。crate 入口 `lightning/pkg/importinto/lib.rs` 通过 `mod precheck` 加载并 `pub use precheck::*` 导出这里的公开类型和构造函数；`lightning/pkg/importinto/Cargo.toml` 则声明它直接依赖相邻的 `astersql-lightning-pkg-precheck` crate，以复用 `Checker`、`CheckResult` 和检查项 ID。

生产调用链位于 `lightning/pkg/importinto/importer.rs`：`Importer::runOnce` 在创建表并取得表元数据后，仅当 `cfg.App.CheckRequirements` 为真时调用 `Importer::runPrechecks`；后者创建 `PrecheckRunner`、注册 `CheckpointCheckItem` 并运行检查。检查通过后才进入 `JobOrchestrator::SubmitAndWait`，因此本文件是导入任务提交前的阻断/告警边界，而不是 checkpoint 的持久化实现。

## 核心职责

本文件有两层职责：

1. `PrecheckRunner` 提供通用的顺序检查器容器，负责注册检查器、传递取消状态、记录每项检查日志，并在第一个执行错误或不通过结果处短路。
2. `CheckpointCheckItem` 实现 `precheck::Checker`，根据配置和现存 checkpoint 判定导入能否继续：关闭 checkpoint 或记录为空时直接通过；发现失败记录时阻断；只有非失败记录时允许继续但返回性能级告警，提醒必须沿用配置或先清理 checkpoint。

它只读取配置和 checkpoint 快照，不创建、更新、删除 checkpoint，也不负责展示 `CheckItemID` 的人类可读名称。

## 主要符号

- `pub struct PrecheckRunner { checkers: Vec<Box<dyn precheck::Checker>> }`：按注册顺序持有异构检查器。字段私有，调用方只能通过 `Register` 追加。
- `pub fn NewPrecheckRunner() -> PrecheckRunner`：构造空 runner；沿用 Go API 命名风格。
- `PrecheckRunner::Register(&mut self, checker)`：把检查器追加到 `Vec` 尾部，因而运行顺序与注册顺序一致。
- `PrecheckRunner::Run(&mut self, ctx) -> Result<()>`：逐项执行并将检查 crate 的错误映射为 importinto crate 的错误；遇到 `Ok(None)` 视为跳过，遇到失败结果或错误立即返回。
- `pub struct CheckpointCheckItem`：持有 `Arc<config::Config>` 和 `Arc<dyn CheckpointManager>`。两个字段均为私有，只在检查期间读取。
- `pub fn NewCheckpointCheckItem(...) -> Box<dyn precheck::Checker>`：封装具体类型并直接返回 trait object，供 runner 注册。
- `CheckpointCheckItem::GetCheckItemID()`：固定返回 `precheck::CheckCheckpoints`，其值在 `lightning/pkg/precheck/precheck.rs` 中定义为 `"CHECK_CHECKPOINTS"`。
- `CheckpointCheckItem::Check(...)`：实现 checkpoint 状态判定，是本文件唯一读取外部运行状态的函数。

文件没有模块级常量、条件编译项或自定义 enum；严重级别、检查 ID 和结果结构均来自 precheck crate。

## 执行流程

`PrecheckRunner::Run` 的流程如下：

1. 从 importinto 的 `context::Context` 创建 precheck crate 的背景 context，并把调用者在进入函数时的 `is_cancelled()` 状态复制到 `pctx.cancelled`。
2. 按 `checkers` 的注册顺序取出检查器，先调用 `GetCheckItemID` 并记录 `running precheck` Debug 日志。
3. 用克隆的 `pctx` 调用 `Checker::Check`。检查器返回错误时，转换错误文本，记录 `precheck error`，并附加 `precheck <itemID> failed` 上下文后立即返回。
4. `Ok(None)` 表示检查项主动跳过，runner 不记录通过日志并继续下一项。
5. `Some(result)` 且 `Passed == false` 时记录失败消息，返回 `precheck <itemID> failed: <message>`。
6. 通过时记录 Info 日志；只有非空 `Message` 才附带 message 字段。所有项目完成后返回 `Ok(())`。

`CheckpointCheckItem::Check` 的分支顺序为：

1. `cfg.Checkpoint.Enable == false`：不访问 manager，返回 `Passed: true`。
2. 将 precheck context 的已取消状态映射到 importinto context；随后调用 `CheckpointManager::GetCheckpoints`。
3. manager 报错：转换为 precheck error 并向上传播，不生成 `CheckResult`。
4. checkpoint 列表为空：返回通过。
5. 按 manager 返回顺序扫描；首个 `CheckpointStatus::Failed` 立即返回不通过，并在消息中带出示例表名和两种清理命令。
6. 列表非空且没有失败项：返回通过、`Severity = precheck::Warn`（其兼容值为 `"performance"`）以及恢复/重新导入提示。

## 数据与状态

`PrecheckRunner` 的唯一可变状态是检查器向量，以及每个 `Checker::Check(&mut self, ...)` 可能改变的检查器内部状态。runner 不缓存历史结果，因此同一实例重复运行会再次执行全部已注册检查器。

`CheckpointCheckItem` 通过 `Arc` 共享配置和 manager 所有权，不复制配置内容，也不持有 checkpoint 快照。每次 `Check` 都调用 `GetCheckpoints` 获取当时的完整列表。返回的 `CheckResult` 只显式设置必要字段，其他字段取 `Default`：普通通过时 severity/message 为空；失败和告警消息为新建 `String`；当前实现没有设置 `Item` 字段，runner 使用检查器单独提供的 ID 做日志与报错标识。

关键不变量是：任何 `Failed` checkpoint 都阻止提交新导入任务；非空但无失败的 checkpoint 不阻断，因为它可能表示可恢复任务，但必须给出配置一致性警告。

## 依赖与调用关系

- 上游生产调用：`Importer::runOnce` → `Importer::runPrechecks` → `NewPrecheckRunner` / `Register(NewCheckpointCheckItem(...))` → `PrecheckRunner::Run`。对应代码在 `lightning/pkg/importinto/importer.rs`。
- 模块装配：`lightning/pkg/importinto/lib.rs` 导出本文件符号；`Cargo.toml` 将 crate 定义为 library，并以 path 依赖连接 `../precheck`。
- 下游接口：`astersql_lightning_pkg_precheck::Checker`、`CheckResult`、`CheckCheckpoints`、`Warn` 和该 crate 的 context/error 类型。
- 本 crate 依赖：`crate::checkpoint::{CheckpointManager, CheckpointStatus}` 提供 checkpoint 查询抽象及失败状态；`crate::stubs::*` 提供配置、context、日志、zap、错误适配等当前移植层类型。
- `CheckpointManager` 要求 `Send + Sync`；实际 manager 可为 noop、文件或 MySQL 实现，本文件只依赖 `GetCheckpoints`，不关心后端类型。

RustCodeGraph 对目标文件给出的直接使用文件包括 `precheck_test.rs` 和 `parity_test.rs`；生产连接由 `importer.rs` 中对公开符号的导入和调用形成。由于符号以 `lib.rs` 重导出且存在 Go/Rust 同名符号，检索调用边时应携带文件路径消歧。

## 错误处理与边界

- runner 区分三种结果：`Err` 是检查执行失败；`Some(Passed: false)` 是检查成功执行但前置条件不满足；`None` 是跳过。前两者都会短路，后续检查器不再运行。
- `CheckpointCheckItem` 关闭 checkpoint 时不会触碰 manager；空列表与关闭配置均为无消息通过。
- manager 错误仅保留其字符串进入 precheck error，再由 runner 加上检查 ID，最后 `Importer::runPrechecks` 再附加一层 `precheck failed`。扩展时不要丢失这条上下文链。
- 扫描 checkpoint 时只报告遇到的第一个失败表，消息明确把它称为示例；它不汇总所有失败记录。
- 非 `Failed` 的所有状态都走“可继续但警告”分支，包括 Running、Finished 以及将来新增而未专门处理的状态。新增状态时必须重新判断这个默认分支是否安全。
- Rust runner 对 `Ok(None)` 有显式跳过处理；同路径 Go `PrecheckRunner.Run` 直接读取 `res.Passed`，并未显式处理 nil 结果。该 Rust 分支来自共享 trait 的契约，不能笼统描述成 Go 当前实现已有的 nil 安全行为。
- 取消状态是调用 `Run` 时的一次布尔快照，不是持续联动的取消通道；`CheckpointCheckItem::Check` 也据此构造本 crate context。若检查开始后调用方才取消，当前桥接不会自动观察到新状态。

## 并发与资源生命周期

runner 在当前线程串行执行检查器，没有创建线程、异步任务、通道、锁或事务。`&mut self` 保证单次调用独占 runner 及其中的 checker；相同 `pctx` 的克隆按项传入，但检查器之间不共享结果。

`Arc<Config>` 和 `Arc<dyn CheckpointManager>` 让检查项与 importer 共享对象生命周期。`CheckpointManager: Send + Sync` 允许其具体实现内部使用同步机制，但本文件不加锁；例如文件 manager 自己用 `RwLock` 保护存储句柄和 checkpoint map。由 `GetCheckpoints` 返回的 `Vec<TableCheckpoint>` 归本次检查所有，遍历期间不持有 manager 的借用或锁。

本文件不打开或关闭 manager，不负责清理 checkpoint，也不持有文件、网络连接等资源。manager 的初始化、关闭和 checkpoint 清理由 importer/manager 生命周期负责。

## 与 Go 版本的对应关系

直接对照文件是 `lightning/pkg/importinto/precheck.go`，测试对照为 `precheck_test.go` 与 `precheck_test.rs`。Rust 保留了 Go 的公开命名、检查器注册顺序、首错短路、错误文本、日志分支、checkpoint 判定顺序、失败/告警消息和 `CHECK_CHECKPOINTS` ID。

主要表达差异如下：

- Go 使用 `[]precheck.Checker` 与接口值；Rust 使用 `Vec<Box<dyn Checker>>`。
- Go 使用普通指针保存 config 与 manager 接口；Rust 使用 `Arc`，且 manager trait 明确要求 `Send + Sync`。
- Go 将同一个 `context.Context` 直接交给检查器和 manager；Rust 的两个 crate 使用不同 context stand-in，因此复制进入函数时的取消布尔值。Rust 独立测试额外验证了“调用前已取消”能够传到 checker。
- Go 返回 `*CheckResult`；Rust 用 `Option<CheckResult>` 编码 nil/跳过，并在 runner 中安全忽略 `None`。这比当前 Go runner 的直接解引用更明确。
- Go 的构造函数返回 `*PrecheckRunner` / `*CheckpointCheckItem`；Rust 分别按值返回 runner、以 boxed trait object 返回 checkpoint checker。

测试场景逐项对齐：checkpoint 关闭、查询报错、空 checkpoint、存在失败项、仅有运行中/已完成项，以及 runner 的全通过、checker 错误和结果不通过。Rust 测试另覆盖取消快照传递。

## 扩展指南

- 新增通用检查项：实现 `astersql_lightning_pkg_precheck::Checker`，给出稳定 `CheckItemID`，然后在 `Importer::runPrechecks` 中按期望顺序 `Register`。同步扩展独立测试 `lightning/pkg/importinto/precheck_test.rs`，并核对 Go 对应接线与测试。
- 修改 checkpoint 策略：优先修改 `CheckpointCheckItem::Check`，明确每个新增 `CheckpointStatus` 是阻断、告警还是通过；同步覆盖关闭、空集合、查询错误、失败项位置和混合状态。不要把测试写回生产源文件。
- 改变错误或消息：注意测试和运维命令可能依赖稳定子串；同时检查 runner、`Importer::runPrechecks` 的多层错误包装和同路径 Go 文本。
- 引入真正的动态取消：需要同时审视两个 context stand-in 的契约，不能只修改当前布尔复制，否则 runner 与 checkpoint manager 的取消语义会分裂。
- 并行执行检查器前：必须定义错误选择顺序、日志顺序、checker 的 `Send` 约束和取消策略；当前可观察契约是注册顺序与首错短路。
- 性能方面，本检查会读取完整 checkpoint 列表并线性扫描，时间复杂度为 O(n)，且失败项越靠后读取越多；若数据量成为问题，应先在 `CheckpointManager` 抽象上设计可移植的查询能力，并保持文件/MySQL/Go 实现一致。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标 `precheck.rs` 共 205 行、11 个符号。
- RustCodeGraph 读取与查询：`node --file lightning/pkg/importinto/precheck.rs`、`query NewPrecheckRunner`、`callers/callees` 同名符号检查、`node` 查看 `importer.rs`、`checkpoint.rs`、`precheck/precheck.rs`。同名 `Run` 查询确认目标 runner 调用 `GetCheckItemID`、`Check` 和错误转换；`importer.rs` 确认生产接线与运行顺序。
- crate/模块证据：`lightning/pkg/importinto/Cargo.toml`、`lightning/pkg/importinto/lib.rs`。
- Go 对照：`lightning/pkg/importinto/precheck.go`、`lightning/pkg/importinto/precheck_test.go`、`lightning/pkg/importinto/importer.go`。
- Rust 行为测试：`lightning/pkg/importinto/precheck_test.rs`；相关生产入口测试位于 `lightning/pkg/importinto/importer_test.rs`。本任务按计划是纯文档分析，未运行 Cargo 或代码测试。
- 人工复核重点：本文明确回答了文件为何存在、在提交导入任务前如何运行、状态/错误/取消的边界，以及安全扩展时需修改的符号和独立测试位置；未把支撑用 stubs 当成真实外部基础设施。
