# `pkg/executor/brie.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 根 `pkg/executor/lib.rs` 以 `pub mod brie` 导出它，并在测试配置下用独立文件 `pkg/executor/brie_test.rs` 挂载单元测试。它是 Go `pkg/executor/brie.go` 的 Rust 侧 BRIE（Backup/Restore）SQL 执行模型：覆盖语句配置构建、进程内任务排队、查询/取消/元数据展示、错误映射，以及 BR 库访问 TiDB 会话能力所需的 glue 抽象。

当前接线边界必须特别说明：仓库搜索未发现测试以外的 `brieRuntime` 实现，也未发现 `pkg/executor/builder.rs` 调用本文件的 `executorBuilder::buildBRIE`。因此该模块已经包含可独立测试的业务逻辑并由 crate 导出，但不能据此声称 Rust SQL 主执行树已经实际调用 BRIE；Go 生产入口仍是 `pkg/executor/brie.go` 中的 `executorBuilder.buildBRIE` 与 `BRIEExec.Next`。

## 核心职责

1. `executorBuilder::try_build_brie` 把 `brieStmt` 转成 `brieExecutor`，规范化存储地址，执行 SEM/store 类型约束，解析公共、备份和恢复选项，并生成任务展示信息。
2. `brieQueue` 维护进程内全局任务表、FIFO 等待队列和单 worker 互斥，支持查询、取消以及完成任务的延迟清理。
3. `BRIEExec::Next` 负责完整生命周期：登记、等待执行权、监视 SQL KILL、调用运行时 backup/restore、记录终态并生成 SQL 结果行。
4. `showQueryExec`、`cancelJobExec`、`showMetaExec` 和 `ShowExec::fetchShowBRIE` 提供查询原 SQL、取消任务、读取 backupmeta 和展示任务进度的旁路操作。
5. `tidbGlue`/`tidbGlueSession` 把 BR 需要的 domain、storage、进度、会话 SQL、建库建表、placement policy 和元数据刷新能力收束到 `brieRuntime` trait。

## 主要符号

- 错误与结果：`BrieErrorKind` 区分普通、查询中断、上下文取消、备份失败和恢复失败；`BrieError` 保留稳定错误身份与显示文本；`BrieResult<T>` 是统一结果别名。`handleBRIEError` 添加操作前缀，`mapBRIEError` 优先保留 KILL QUERY 身份。
- SQL/结果模型：`brieKind` 表示五类语句；`brieStmt` 携带存储地址、job ID、选项和过滤范围；`datum`、`resultChunk`、`sqlTime` 是本模块自包含的结果单元、行缓冲和时间表示。
- 配置模型：`commonConfig` 汇总 PD/TLS/storage/限速/并发/校验/凭证/加密/过滤；`backupConfig` 与 `restoreConfig` 分别追加备份、恢复专属字段；`brieOptionType`/`brieOption` 表示 AST 选项。
- 队列模型：`brieTaskInfo` 是可展示任务状态；`brieTaskProgress` 维护阶段名、总量和原子 current；`brieQueueItem` 组合信息、进度和取消上下文；`brieQueue` 负责分配 ID 与串行调度。`globalBRIEQueue` 通过 `LazyLock<RwLock<Arc<_>>>` 保存可供测试替换的全局实例。
- 构建和分派：本文件自己的 `executorBuilder` 持有 `Arc<dyn brieRuntime>`、session ID 和构建错误；`brieExecutor` 是 OneShot/ShowQuery/Cancel/ShowMeta/Main 的枚举分派器；`execOnce` 保证展示类执行器只产出一次。
- 运行入口：`BRIEExec::Next` 是 BACKUP/RESTORE 主入口；`showMetaExec::Next`、`showQueryExec::Next`、`cancelJobExec::Next` 与 `ShowExec::fetchShowBRIE` 是管理/展示入口。
- 运行时边界：`brieRuntime` 定义所有外部副作用；`tidbGlue` 和 `tidbGlueSession` 将这些能力适配成 BR 所需操作。该 trait 目前只在 `pkg/executor/brie_test.rs::MockRuntime` 中有实现。

## 执行流程

构建阶段从 `executorBuilder::buildBRIE` 开始；它调用 `try_build_brie`，失败时保存到 `builder.error` 并返回 `None`。SHOW BACKUP META、SHOW BRIE QUERY 使用 `execOnce`，CANCEL 直接返回取消执行器。BACKUP/RESTORE 则先从 `brieRuntime::global_config` 取得 PD/TLS/store 类型，调用 `normalize_storage_url`，拒绝 SEM v1 下的 `hdfs`/`local`/`file`/空 scheme，并要求 store type 为 `tikv`。

随后构建器解析公共选项。加密算法只接受 `aes128-ctr`、`aes192-ctr`、`aes256-ctr`、`plaintext`；表过滤优先于 schema 过滤，无过滤时使用 `*.*`，且固定启用大小写不敏感。BACKUP 再解析增量/目标 TSO、time-ago、压缩、压缩级别和忽略统计；压缩只接受 `zstd`、`snappy`、`lz4`。RESTORE 再解析 online、等待 TiFlash、恢复系统表与加载统计。`restoreQuery` 保存可展示 SQL，恢复失败时退化为 `N/A`。

执行阶段由 `BRIEExec::Next` 清空输出并检查 `info`；`info == None` 表示该执行器已完成。它清理过期项、写入连接 ID/排队时间，经 `registerTask` 派生可取消子上下文并入 FIFO。独立线程每三秒调用 `brieRuntime::check_killed`，只在错误身份为 `QueryInterrupted` 时取消任务。`acquireTask` 等到本任务位于队首且 worker 空闲；成功后以 `releaseTaskGuard` 保证释放执行权，以 `tidbGlue` 调用 `run_backup` 或 `run_restore`。

结束时写入 finish time 和错误消息；成功则按语句类型输出 storage、size、TSO、queue time、exec time，并将 `self.info` 清空以实现一次性语义。`cancelTaskGuard` 在任何退出路径取消上下文并关闭进度；它不会删除任务，任务要等 `clearTask` 在保留期后清除。

## 数据与状态

任务状态分散但有明确锁域：`brieTaskInfo` 置于 `Arc<Mutex<_>>`，任务表和等待 ID 队列分别由互斥锁保护；`brieTaskProgress.current` 用 `AtomicI64`，阶段名和 total 用同一个 `Mutex<brieTaskProgressState>`，`snapshot` 在持锁时同时读取三者。任务 ID 由 `AtomicU64` 从 1 递增。

时间字段以 Unix 毫秒和 `valid` 标志表达。未完成任务的 `finishTime.valid == false` 是清理不变量；完成超过 `outdatedDuration`（30 分钟）的任务才可删除，而且 `clearTask` 最多每 `clearInterval`（10 分钟）真正扫描一次。进度初始为 `Wait/1/0`；`Close` 在未完成时把命令追加 ` Canceled`，再把 current 推至 total。

结果行约定直接由各入口构造：SHOW BACKUP META 每表六列；SHOW BRIE 每任务九列；BACKUP 成功五列，RESTORE 成功六列。`tidbGlue::Record` 只识别 `BackupTS`、`RestoreTS`、`Size`，其他名称被忽略。

## 依赖与调用关系

RustCodeGraph 对精确符号给出的主要内部边包括：`buildBRIE -> try_build_brie`；`try_build_brie -> normalize_storage_url/read_cipher_key_file/parseTSString/execOnce/buildShowMetadataConfigFrom/restoreQuery`；`oneshotExecutor::Next -> brieExecutor::execute`；`ShowExec::fetchShowBRIE -> current_queue/items/brieTaskProgress::snapshot/clearTask`。`BRIEExec::Next` 的源码边则是 `clearTask -> registerTask -> acquireTask -> run_backup|run_restore -> mapBRIEError`，并由两个 Drop guard 回收执行权与取消上下文。

`pkg/executor/Cargo.toml` 声明 crate 名为 `astersql-executor`、库入口为 `lib.rs`，且 `package.metadata.porting.go-package = "pkg/executor"`。本文件自身只直接使用 Rust 标准库；真实配置、存储、BR 调用、session/DDL 等依赖均经 `brieRuntime` 反转，因而 Cargo 中的大量 executor 依赖并未被本文件直接引用。

相邻 `pkg/executor/brie_utils.rs` 承担 BRIE DDL 渲染与选项工具，但本文件没有直接调用它。生产 Rust 搜索只找到 `pkg/executor/lib.rs` 的模块导出，没有找到本模块执行器接入 `pkg/executor/builder.rs` 的调用边；这是扩展或上线前首先需要补齐和验证的边界。

## 错误处理与边界

配置错误在构建期返回：非法目标 URL 会增加 `invalid destination URL` 上下文；SEM/store type、加密算法、压缩算法也会被拒绝。`buildBRIE` 不直接向调用者返回 `Result`，而是把错误存入 `executorBuilder.error`，调用者必须同时检查返回值和该字段。

执行错误经 `mapBRIEError` 分类：若运行时 SQL killer 报 `QueryInterrupted`，优先返回该身份；否则 backup/restore 错误分别包装成 `BackupFailed`/`RestoreFailed`。CANCEL BRIE JOB 只取消 task context，不设置 SQL killer，因此底层取消仍按备份或恢复失败分类。排队阶段取消不附加 backup/restore 分类，但会写 finish time 和 message。

不存在的 SHOW QUERY 静默返回空结果；不存在的 CANCEL JOB 通过 `append_job_not_found_warning` 添加警告而不报错。SHOW BACKUP META 对读取失败增加 `failed to read metadata from backupmeta` 上下文，startVersion 为 0 时输出 NULL；endVersion 总是尝试格式化。

锁中毒均以 `expect` 触发 panic，缺失与语句类型匹配的 backup/restore 配置也以 `expect` 失败；这些是内部不变量而非可恢复用户错误。`buildShowMetadataConfigFrom` 用 `assert_eq!` 约束只能传 SHOW BACKUP META。SHOW 进度百分比直接计算 `100 * current / total`，调用方必须维持 total 非零。

## 并发与资源生命周期

`brieQueue` 在单进程内严格串行执行任务：只有等待 ID 位于 FIFO 队首且 `workerBusy == false` 才能取得执行权。Condvar 等待使用 20ms 超时，取消检查因此不会无限依赖通知；取消同时移除等待 ID 并 `notify_all`。这与 Go 的容量为 1 的 `workerCh` 都实现单 worker，但 Rust 额外显式维护 FIFO，而 Go channel 版本没有任务 ID 队列。

KILL 监视线程每任务创建一个 OS thread，每三秒轮询；任务结束时 `cancelTaskGuard` 令子上下文取消，线程随后退出，但 `BRIEExec::Next` 不 join 它。`releaseTaskGuard` 确保获得 worker 后的所有返回路径都会释放 worker；`cancelTaskGuard` 确保任务上下文和进度最终关闭。

`tidbGlue::UseOneShotSession` 创建会话、调用回调、显式 `Close`，再记录关闭；即使回调返回错误也会关闭。但若回调 panic，则当前实现没有 RAII guard，不能保证关闭。`CreatePlacementPolicy`、`AlterTableMode`、`RefreshMeta` 都先保存 query string，调用后恢复；普通错误路径能恢复，panic 路径不能保证恢复。

## 与 Go 版本的对应关系

Rust 的类型与方法大体一一对应 `pkg/executor/brie.go`：队列和进度对应 `brieQueue`/`brieTaskProgress`，构建对应 `executorBuilder.buildBRIE`，执行对应 `BRIEExec.Next`，展示对应 `ShowExec.fetchShowBRIE`，glue 对应 `tidbGlue`/`tidbGlueSession`。`pkg/executor/brie_test.rs` 也复刻了 Go 测试对配置、取消终态、清理和错误身份的核心断言。

重要差异如下：Go 直接使用真实 `task.BackupConfig`/`task.RestoreConfig`、`task.RunBackup`/`task.RunRestore`、sessionctx、chunk 和 DDL/domain；Rust 用本地数据类型与 `brieRuntime` trait 隔离这些能力，当前无生产实现。Go 的 builder 已进入生产执行器构建流程，Rust 同名 builder 是本文件独立类型，未接到 `pkg/executor/builder.rs`。Go 含 `block-on-brie`、`beforeRunBRIETask` failpoint，Rust 没有对应注入点。Go 的 one-shot session 使用 defer，Rust 只在回调正常返回 `Result` 时显式关闭。

Rust 的 FIFO 队列也不是 Go `workerCh` 的机械复制；它强化了等待顺序，并在取消等待任务时从 `waitingTaskIDs` 移除。错误表示同样从 Go terror 错误码抽象为本地 `BrieErrorKind`；语义测试覆盖身份，但不能证明客户端错误码完全等价。

## 扩展指南

- 接入生产执行链时，优先为 `brieRuntime` 提供真实实现，并在 `pkg/executor/builder.rs` 的计划分派处桥接本文件的构建器；不能仅靠模块导出宣称功能可用。同步新增独立测试文件或扩展 `pkg/executor/brie_test.rs`，不要把测试写入 `brie.rs`。
- 新增 AST 选项时，要同时更新 `brieOptionType`、`try_build_brie` 的公共或专属分支、对应配置字段，以及 Go `pkg/executor/brie.go` 的语义对照测试。需明确默认值、数值截断风险（如 `u64 -> u32/usize/i32`）和合法值集合。
- 修改任务生命周期时，维持三个不变量：单 worker 必须在所有路径释放；排队取消必须能退出并记录终态；未完成任务不能被 GC。重点扩展 `queued_brie_cancellation_records_terminal_state` 和 `clear_task_keeps_unfinished_brie_tasks`。
- 修改错误映射时，分别覆盖普通底层错误、KILL QUERY 与 CANCEL JOB；参考 `map_brie_error_preserves_kill_and_cancel_causes`，并对照 Go `TestMapBRIEErrPreservesCancelCause`。
- 修改 glue 的 session/query-string 生命周期时，建议引入 RAII guard 处理 panic/提前退出，并补充独立测试验证关闭和原值恢复。并发或监视策略改变时，应评估每任务 OS thread 的成本、取消延迟和锁顺序。
- SHOW/结果列变更必须同步 planner schema/上层消费方和 Go 结果顺序；进度 total 要禁止为 0，避免无穷或 NaN 百分比。

## 验证依据

- 源码与模块：`pkg/executor/brie.rs`（1563 行）、`pkg/executor/lib.rs`（`pub mod brie` 和 `#[path = "brie_test.rs"] mod brie_test`）、`pkg/executor/Cargo.toml`。
- RustCodeGraph：`status` 显示本仓库索引含 11467 files/307296 nodes/1848419 edges；读取了 `brie.rs` 全部行，并查询 `buildBRIE`、`BRIEExec::Next`、`fetchShowBRIE`、`UseOneShotSession` 的 callers/callees。图对同名 `Next` 存在歧义，因此用精确仓库搜索补核上游接线。
- Rust 测试：`pkg/executor/brie_test.rs` 覆盖 SQL 表面类型/行缓冲、KILL 与取消错误身份、排队取消终态、未完成任务保留、Go 对齐的 backup/restore 配置和非法加密/压缩值。
- Go 对照：`pkg/executor/brie.go` 与 `pkg/executor/brie_test.go`；重点核对 builder、queue、kill monitor、主 Next、SHOW、glue、query string 恢复和配置断言。
- 仓库搜索：除测试外未找到 `brieRuntime` 实现，也未找到 `pkg/executor/builder.rs` 调用本文件的 `buildBRIE`/`BRIEExec`；因此生产接线状态在本文标为未完成，而不是根据 Go 实现推断 Rust 已接线。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构检查要求本文恰含上述十一个固定二级标题。
