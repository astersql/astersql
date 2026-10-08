# `pkg/statistics/handle/autoanalyze/priorityqueue/queue_ddl_handler.rs`

## 文件定位

对应生产源码：[queue_ddl_handler.rs](queue_ddl_handler.rs)。

本文件是 `astersql-statistics-handle-autoanalyze-priorityqueue` crate 中“DDL 事件到分析队列变更”的适配层。crate 入口 `pkg/statistics/handle/autoanalyze/priorityqueue/lib.rs` 以私有模块 `mod queue_ddl_handler` 装配它，再通过 `pub use queue_ddl_handler::*` 导出其中的常量、事件类型和 `AnalysisPriorityQueue` 的公开方法。`Cargo.toml` 将该 crate 的库入口设为 `lib.rs`，并以 `package.metadata.porting.go-package` 指向同路径 Go 包。

它不解析真实 DDL notifier 事件，也不自行构造分析作业；它接收本文件定义的扁平化 `SchemaChangeEvent`，决定哪些物理 ID 要从队列删除、哪些表要交给队列的数据源重建。实际堆操作与状态锁位于 `queue.rs`。全仓 Rust 引用检索只找到本文件及 `queue_ddl_handler_test.rs` 对这些 API 的使用，未找到从 `aster_sql_ddl_notifier::SchemaChangeEvent` 到本类型的生产适配或生产态 `HandleDDLEvent` 调用者，因此当前不能把 Go 版本的 notifier/session 集成视为已经在 Rust 中接通。

## 核心职责

1. 通过 `HandleDDLEvent` 实施初始化门禁：队列未初始化且 auto-analyze 开启时返回唯一可重试错误；auto-analyze 关闭时允许忽略事件。
2. 把十类关心的 schema 动作分派给专用处理函数；`None` 和 `Other` 都是不修改队列的成功操作。
3. 删除已经失效的表、分区或旧物理 ID 对应的队列作业，并在需要时按当前数据源重建仍存在的全局表或新表作业。
4. 保持 Go 入口的确认语义：初始化完成后，具体处理函数的错误不会由 `HandleDDLEvent` 返回，以免 notifier 在没有重试上限时无限重放同一事件。

本文件不负责队列初始化、优先级计算、后台刷新、作业执行或 DDL 事务提交；这些分别由 `queue.rs`、`calculator.rs`、各类 job 实现及上层 DDL/notifier 组件承担。

## 主要符号

- `ERR_NOT_READY_RETRY_LATER: &str`：队列未就绪且需要上层重试时的稳定错误文本，对应 Go 的 `notifier.ErrNotReadyRetryLater` 语义。
- `SchemaChangeAction`：本地动作枚举，覆盖 `AddIndex`、表截断/删除、分区截断/删除/交换/重组、增加或移除分区化、删库，以及兜底 `Other`。它是 Go `model.ActionType` 的局部投影，并非 notifier 的完整动作集合。
- `SchemaChangeEvent`：供本处理层使用的扁平事件。`table_id` 表示主表或新表，`old_table_id` 表示被替换的旧对象，`affected_table_ids` 携带其它受影响物理 ID，`added_index_analyzed` 表示新增索引是否已在 DDL 阶段完成分析。`Default` 会产生 `action: None`、ID 为零且集合为空的事件。
- `AnalysisPriorityQueue::HandleDDLEvent(run_auto_analyze, event)`：公开总入口，先检查 `IsInitialized`，再按动作分派；只有就绪门禁错误能向调用者传播。
- `GetAndDeleteJob(table_id)`：公开薄封装，调用 `queue.rs::DeleteByTableID`；删除不存在的作业也是成功。
- `RecreateAndPushJobForTable(table_id)`：公开薄封装，调用 `queue.rs::RecreateAndPushJob`，后者先删除旧作业，再调用 `QueueSource::recreate_job`，若返回 `Some(job)` 才入堆。
- `HandleAddIndexEvent`：若 `added_index_analyzed` 为真则跳过，否则重建 `table_id`。
- `HandleTruncateTableEvent`、`HandleDropTableEvent`、`HandleDropSchemaEvent`：删除事件指出的所有失效物理 ID，不重建新作业。
- `HandleTruncateTablePartitionEvent`、`HandleDropTablePartitionEvent`、`HandleReorganizePartitionEvent`：共同调用私有函数 `delete_partitions_and_recreate_global`。
- `HandleExchangeTablePartitionEvent`：清理分区、旧非分区表和全局表作业，然后重建全局表及 `affected_table_ids` 的首个交换分区 ID。
- `HandleAlterTablePartitioningEvent`、`HandleRemovePartitioningEvent`：清理旧布局中的作业，并为新布局的 `table_id` 重建作业。
- `delete_partitions_and_recreate_global`：本文件唯一私有函数；依次删除 `affected_table_ids`、删除 `table_id`，最后重建 `table_id`。

文件没有 trait、宏、模块级可变状态或条件编译项；所有方法都实现于 `AnalysisPriorityQueue` 的 inherent `impl`。

## 执行流程

`HandleDDLEvent` 的主流程如下：

1. 调用 `IsInitialized()`。若尚未初始化，`run_auto_analyze == true` 时立即返回 `ERR_NOT_READY_RETRY_LATER`；为假时直接返回成功，因为以后重新启用 auto-analyze 会通过初始化全量建队。
2. 将缺失动作 `None` 归为 `Other`，再对 `SchemaChangeAction` 做穷尽分派。
3. 处理函数通过 `GetAndDeleteJob` 清理旧 ID；需要重建时通过 `RecreateAndPushJobForTable` 进入 `QueueSource::recreate_job -> Push` 链路。
4. 分派结果保存在 `result` 后被显式丢弃，入口返回 `Ok(())`。因此初始化后的删除、数据源重建或入堆失败不会触发入口级重试。

各动作的 ID 变化规则是：

| 动作 | 删除 | 重建 |
| --- | --- | --- |
| `AddIndex` | `RecreateAndPushJob` 内先删除 `table_id` | `table_id`；若索引已分析则均不做 |
| `TruncateTable` | `old_table_id`，缺失时退回 `table_id`；再删除全部 `affected_table_ids` | 无 |
| `DropTable` / `DropSchema` | `table_id` 和全部 `affected_table_ids` | 无 |
| `TruncateTablePartition` / `DropTablePartition` / `ReorganizePartition` | 全部 `affected_table_ids` 和全局 `table_id` | 全局 `table_id` |
| `ExchangeTablePartition` | 全部 `affected_table_ids`、可选 `old_table_id`、全局 `table_id` | 全局 `table_id`，以及首个 `affected_table_ids` |
| `AlterTablePartitioning` | 可选旧单表 ID 和新全局 `table_id` | 新全局 `table_id` |
| `RemovePartitioning` | 全部旧分区 ID 和可选旧全局表 ID | 新单表 `table_id` |
| `Other` / `None` | 无 | 无 |

删除循环使用 `?`，所以专用处理函数遇到第一个错误便停止后续步骤；只是该错误随后会被总入口吞掉。

## 数据与状态

事件本身只保存复制或拥有的数据：动作是 `Copy` 枚举，ID 使用 `i64`，附加 ID 使用 `Vec<i64>`。本文件不缓存事件，也不改变事件内容。

真正的队列状态在 `queue.rs::QueueState` 中，包括最大堆、`initialized`、`running_jobs` 与 `must_retry_jobs`，由 `AnalysisPriorityQueue.state: Arc<Mutex<QueueState>>` 保护。`DeleteByTableID` 在持锁状态下按 key 删除堆项，并无条件清理相同 ID 的 `must_retry_jobs`；它不会移除 `running_jobs` 中已被 `Pop` 的作业。`RecreateAndPushJob` 先完成一次删除，再在不持有该状态锁时调用外部 `QueueSource::recreate_job`，随后由 `Push` 重新加锁、计算权重并入堆。

`affected_table_ids` 的含义依动作而变：通常是旧静态分区 ID；删表/删库时也可表示同一对象下的其它物理 ID；交换分区时首元素还被当作交换后需要重建的新非分区表 ID。调用者必须按动作提供正确、完整且顺序符合约定的 ID，本类型本身不验证这些关系。

## 依赖与调用关系

- crate 内部上游：`lib.rs` 导出本文件的公开类型和方法。已索引图显示本文件包含 30 个符号；RustCodeGraph 对 `HandleAddIndexEvent` 的结果定位到本文件第 121 行。
- 仓库内已验证调用者：`queue_ddl_handler_test.rs` 直接构造 `SchemaChangeEvent` 并调用 `HandleDDLEvent`；全仓 `.rs` 检索未发现其它调用，因此生产 DDL 入口目前未验证接线。
- 直接下游：`HandleDDLEvent -> IsInitialized`；各处理函数经 `GetAndDeleteJob -> DeleteByTableID`，或经 `RecreateAndPushJobForTable -> RecreateAndPushJob -> QueueSource::recreate_job / Push`。这些实现位于 `queue.rs`。
- 数据结构下游：`DeleteByTableID` 操作 `PqHeapImpl`，`Push` 使用 `PriorityCalculator` 并为作业注册成功/失败钩子。
- crate 边界：本 crate 的 `Cargo.toml` 仅声明 `astersql-statistics-handle-logutil` 路径依赖；本文件自身只通过 `crate::queue::AnalysisPriorityQueue` 使用 crate 内类型，没有直接外部 crate import。
- 消费该 priorityqueue crate 的 Cargo manifest 包括 `pkg/statistics/handle/autoanalyze/refresher/Cargo.toml` 及两个分析/时区辅助 crate，但这只能证明 crate 依赖，不能证明它们调用了本文件的 DDL API。

## 错误处理与边界

- 未初始化是唯一由总入口传播的错误边界：auto-analyze 开启时要求重试，关闭时确认并忽略。
- 初始化之后，专用处理函数仍返回 `Result<(), String>`，底层堆、数据源或入队错误可沿 `?` 传播到分派局部结果；`HandleDDLEvent` 最终丢弃该结果并成功返回。Rust 实现当前没有 Go 版本的断言和错误日志，只保留“避免无限重试”的确认行为。
- `action: None` 被静默当作 `Other`。零或负 ID、重复 ID、`old_table_id` 缺失、空 `affected_table_ids` 均无显式校验；其效果由底层按 key 删除和数据源重建决定。
- 删除不存在的 ID 是幂等成功。重复 ID 可能导致多次删除/重建请求，但通常不会因“不存在”本身报错。
- `TruncateTable` 在没有 `old_table_id` 时删除 `table_id`，这是容错回退而非事件完整性验证。
- `ExchangeTablePartition` 只重建 `affected_table_ids.first()`，其余受影响 ID 仅删除；空列表时不会重建交换后的非分区表。
- `DropSchema` 使用 `?`，与 Go 版本“逐项记录错误并尽力继续删除”不完全一致；即使专用函数提前停止，总入口仍返回成功。

## 并发与资源生命周期

本文件不创建线程、任务、通道、事务或长期资源。每次调用只借用 `&AnalysisPriorityQueue` 和 `&SchemaChangeEvent`，方法结束后不保留事件引用。

并发安全来自 `queue.rs` 的 `Arc<Mutex<QueueState>>`：初始化检查、每次删除和每次推入分别获取状态锁，毒化锁会通过 `into_inner()` 恢复。与 Go 实现不同，Rust 的 `HandleDDLEvent` 没有在整个事件处理期间持有一把总锁；多 ID 事件由多次独立加锁组成，重建期间还会释放锁调用 `QueueSource`。因此整个 DDL 事件不是原子事务，可能与后台刷新或其它队列操作交错，但单次堆变更受锁保护。

`DeleteByTableID` 不取消已经运行的作业；运行中作业由注册在 job 上的成功/失败钩子维护。`RecreateAndPushJob` 的“删除—查询数据源—推入”也不是单个临界区。扩展代码不得在已持有 `QueueState` 锁时回调未知数据源，以免形成锁重入或长时间阻塞。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/autoanalyze/priorityqueue/queue_ddl_handler.go`，相关集成测试是同目录 `queue_ddl_handler_test.go`。

保持的核心语义包括：未就绪时是否重试取决于 auto-analyze 开关；只处理选定动作；新增索引若已由 DDL 分析则跳过；表/分区布局变化会删除失效物理 ID并重建仍存在的目标；初始化后 handler 错误被确认而不是无限重试。Rust 独立测试覆盖了这些关键分支中的就绪门禁、截断表不重建、错误吞并、已分析索引跳过、三类分区事件重建全局表，以及破坏性动作清理失效 ID。

当前 Rust 是参数压缩后的本地模型，不是 Go 实现的等量集成：

- Go 接收 `context.Context`、`sessionctx.Context` 和 notifier 事件，并从事件携带的 `TableInfo`/分区定义获取 ID；Rust 接收布尔开关和扁平 ID 事件。
- Go 重建路径读取会话参数、时间戳、InfoSchema、锁表信息、分区裁剪模式和统计元数据；Rust 把这些全部抽象到 `QueueSource::recreate_job(table_id)`。
- Go 在整个入口持有 `syncFields.mu`；Rust 只在底层单次状态操作时持锁。
- Go 记录具体 handler 错误并在测试模式断言允许的错误类型；Rust 以 `let _ = result` 静默丢弃。
- Go 对静态分区可能逐分区构造作业；Rust 只把事件提供的 ID 交给 `QueueSource`，结果取决于数据源实现。
- Go `DropSchema` 对每张表和分区尽力清理；Rust 在首个底层错误处停止专用函数。
- Go 测试覆盖真实 TestKit、DDL notifier、统计元数据和静态/动态分区模式；Rust 文件前半保留了大量 Go 测试注释，实际可执行测试从 `use crate::{...}` 后开始，覆盖的是本地队列模型。

所以扩展或修复时应以 Go 文件校验行为目标，同时以 Rust 当前公开类型、`QueueSource` 边界和可执行测试判断已经落地的能力，不能仅凭注释认定生产链路已完成。

## 扩展指南

- 新增 DDL 动作时，同时扩展 `SchemaChangeAction`、`HandleDDLEvent` 的穷尽匹配、`SchemaChangeEvent` 所需载荷和独立测试；若真实 notifier 需要接线，应在适配层明确转换，避免继续让一个 `Vec<i64>` 承担多种未校验语义。
- 修改删除/重建顺序时，优先修改对应的 `Handle*Event` 或共享的 `delete_partitions_and_recreate_global`，并在 `queue_ddl_handler_test.rs` 断言删除后的 `Snapshot` 和 `RecordingSource` 的重建调用顺序。
- 若要改变错误策略，应同时决定 notifier 重试上限、日志位置和部分变更后的恢复方式。不能简单让所有底层错误向上返回，否则可能重新引入 Go 注释所述的无限重试；也不应继续静默吞错而不提供可观测性。
- 若要加强事件一致性，应校验动作所需的 `old_table_id`、`affected_table_ids` 数量及 ID 合法性，并为畸形事件新增回归测试。尤其要覆盖交换分区空列表/多元素，以及删库途中失败的继续处理策略。
- 若要保证单事件原子性，需要评估后台 worker、`QueueSource` 回调和锁顺序；不要直接把当前整段流程包入状态锁，因为重建会调用外部实现并再次通过 `Push` 获取同一锁。
- 需要同步维护的独立测试是 `pkg/statistics/handle/autoanalyze/priorityqueue/queue_ddl_handler_test.rs`；Go 语义变更还应对照 `queue_ddl_handler_test.go`。按仓库约定不要把 Rust 测试内嵌回生产源文件。
- 兼容风险主要是事件字段含义和确认/重试合同；正确性风险是遗漏物理 ID 或错误重建对象；性能风险来自大规模删库时逐 ID 加锁和逐项堆操作，以及重建过程的数据源访问。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件；目标文件已索引为 228 行、30 个符号。`node --file ... --offset 1 --limit 500` 用于读取全貌；`query` 定位了 `SchemaChangeAction`、其全部枚举项、`ERR_NOT_READY_RETRY_LATER` 和 `HandleAddIndexEvent`。对本文件 inherent `impl` 的 `callers/callees` 查询无输出，因此调用边用下述源码检索补齐。
- 生产源码：`pkg/statistics/handle/autoanalyze/priorityqueue/queue_ddl_handler.rs`（事件模型、分派、各动作处理）；`queue.rs`（`AnalysisPriorityQueue`、锁、`DeleteByTableID`、`RecreateAndPushJob`、`QueueSource`）；`lib.rs`（模块装配与公开重导出）。目标包及其逐级父目录未发现 `doc.go`。
- crate 配置：`pkg/statistics/handle/autoanalyze/priorityqueue/Cargo.toml`，核对 crate 名、`lib.rs` 入口、直接依赖及 Go 包映射；根 `Cargo.toml` 和 `refresher/Cargo.toml` 仅用于确认 workspace/消费者边界。
- Go 对照：`pkg/statistics/handle/autoanalyze/priorityqueue/queue_ddl_handler.go`，核对就绪门禁、动作映射、删除/重建策略、日志/确认语义与会话依赖。
- 测试证据：`pkg/statistics/handle/autoanalyze/priorityqueue/queue_ddl_handler_test.rs` 的六个可执行测试；`queue_ddl_handler_test.go` 的 TestKit/DDL 集成用例列表。按任务约束未运行 Cargo，也未把注释中的 Go 测试当作 Rust 可执行覆盖。
- 全仓 `rg` 检索 `HandleDDLEvent`、`SchemaChangeEvent`、`SchemaChangeAction` 和错误常量，未发现本地事件模型的生产调用者；这是“尚未验证生产接线”结论的依据，而不是对未来接线能力的推断。
