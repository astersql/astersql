# `pkg/dxf/framework/storage/history.rs`

## 文件定位

本文件是 `astersql-dxf-framework-storage` crate 的历史任务存储实现。crate 根 `pkg/dxf/framework/storage/lib.rs` 通过 `include!("history.rs")` 将它直接展开到 crate 根，因此文件中的常量、结构体、自由函数以及 `TaskManager` 方法均属于 `astersql_dxf_framework_storage` 的公开或内部 API，而不是独立子模块。crate 边界、`nextgen` feature 和直接依赖见 `pkg/dxf/framework/storage/Cargo.toml`；本文件本身没有条件编译项，也没有独立 feature 分支。

它位于 DXF（Distributed eXecution Framework）存储层：上游调度器完成任务清理后把任务及其子任务从活动表迁到历史表，状态查询接口分页读取历史任务，后台维护逻辑清除过期子任务历史。真实表为 `mysql.tidb_global_task[_history]` 和 `mysql.tidb_background_subtask[_history]`。

## 核心职责

1. `TaskManager::TransferTasks2History` 在一个新事务内先刷新可能已脱敏的任务 `meta`，再依次迁移并删除活动任务和子任务；`TransferSubtasks2HistoryWithSession` 提供复用既有 session 的子任务迁移版本。
2. `TaskManager::GetTaskCleanupInfoByIDs` 同时查询活动表与历史表，向外部制品清理逻辑提供任务类型、状态和结束时间。
3. `TaskManager::ListHistoryTasks` 提供按 ID 降序的 keyset 分页、可选 keyspace 过滤和独立的近似总数查询，并将数据库行转换为不会泄露原始错误消息的摘要。
4. `TaskManager::GCSubtasks` 按保留期清理子任务历史。
5. `ValidateHistoryTaskPageSize`、`row2HistoryTaskSummary`、`ClassifyTaskError*`、`isDataError` 和 `taskErrorCode` 共同承担输入校验、行解码以及安全错误元数据提取。

## 主要符号

- `DefaultHistoryTaskPageSize = 20`、`MinHistoryTaskPageSize = 1`、`MaxHistoryTaskPageSize = 200`：历史任务 API 的默认值和允许区间。当前文件只使用上下界；默认值供外部调用者使用。
- `historyTaskSummaryColumns`：固定 16 列的查询投影。前 12 列交给 `row2TaskBasic`，索引 12 是序列化错误，13 至 15 依次是开始、状态更新和结束时间。列序是 `row2HistoryTaskSummary` 的硬约束。
- `HistoryTaskSummary`：包含 `proto::TaskBase`、安全错误码 `ErrorCode`、粗粒度 `ErrorCategory` 和三个时间字段。与 Go 的嵌入指针不同，Rust 直接持有 `TaskBase` 值。
- `HistoryTaskPage`：包含当前页 `Items`、`HasMore`、下一页 ID token 和 `ApproxTotalCount`。
- `TaskCleanupInfo`：外部清理决策所需的最小任务元数据；`EndTime` 为 `Option<SystemTime>`。
- `GetTaskCleanupInfoByIDs`：空 ID 列表直接返回空 map；否则对活动表和历史表执行 `UNION ALL`，两组占位符各绑定一次 ID，并以 ID 为键覆盖到结果 map。
- `TransferSubtasks2HistoryWithSession`：在传入 session 上先 `INSERT ... SELECT` 到子任务历史表，再删除活动行；两次都用 `TaskIDToKey` 将 `i64` 变为十进制字符串。
- `TransferTasks2History`：空批次是无操作；非空批次构造一次 `CASE id` 元数据更新和四条迁移 SQL，并由 `WithNewTxn` 保证整体提交或回滚。
- `ListHistoryTasks`：校验页大小后查询 `pageSize + 1` 行，以额外一行判定 `HasMore`；若有下一页，token 为截断后最后一项的 ID。
- `GCSubtasks`：默认使用 `defaultSubtaskKeepDays`（定义于 `task_table.rs`，当前为 14 天）换算秒数，并允许 `subtaskHistoryKeepSeconds` failpoint 修改。
- `ClassifyTaskError` / `ClassifyTaskErrorMessage`：前者从存储 `Error` 组合出含错误码的格式化消息，后者按任务状态和消息返回空串、`failed`、`cancelled` 或 `data-error`。

## 执行流程

批量归档从 `pkg/dxf/framework/scheduler/scheduler_manager.rs` 的清理流程开始，经 `scheduler::TaskManager` trait 与 `storage_adapter.rs::transfer_tasks_to_history` 到达 `TransferTasks2History`。方法先收集数值任务 ID 和带单引号的字符串 task key，生成 `UPDATE ... CASE` 参数；通过随机错误注入点后开启新事务；事务内严格执行“更新活动任务 meta → 插入任务历史 → 删除活动任务 → 插入子任务历史 → 删除活动子任务”。任一步失败都会短路，事务后端负责回滚，见 `sql_backend_test.rs::batch_history_transfer_rolls_back_at_each_sql_failure`。

历史列表从 `pkg/dxf/framework/handle/status.rs::ListHistoryTasks` 经运行时 trait 的 `list_history_tasks` 进入本文件。查询条件先加入可选 `t.keyspace = %?`，再在正 token 时加入 `t.id < %?`，按 `t.id desc` 取 `pageSize + 1` 行。随后另开 session/查询执行 `count(1)`；结果行逐项经 `row2HistoryTaskSummary` 解码。两个查询不是同一快照，所以总数是观测性近似值，分页正确性只依赖数据查询、`HasMore` 和 token。

清理信息查询由 `storage_adapter.rs::task_cleanup_info_by_ids` 调用。结束时间先由 SQL 的 `unix_timestamp(end_time)` 变为秒数，仅非空且大于零时才通过 `UNIX_EPOCH.checked_add` 形成 `SystemTime`；零日期、空值或溢出均表现为 `None`。

子任务历史 GC 由调度管理器的 `gc_subtasks` 路径触发。它删除 `state_update_time < UNIX_TIMESTAMP() - 保留秒数` 的行；这里只执行单条删除，不涉及任务历史表。

## 数据与状态

归档操作保持活动表与历史表的记录布局一致，使用 `insert ... select *`，因此表结构列序必须兼容。任务 ID 用数值 `IN (...)` 比较；子任务的 `task_key` 是 `VARCHAR`，故 `TransferTasks2History` 显式生成带引号的十进制 ID。这样避免大于 `2^53` 的相邻 ID 在 SQL 数值比较中被转换为 `DOUBLE` 后碰撞；Go 回归 `TestTransferTasks2HistoryWithAdjacentLargeTaskIDs` 和 Rust `subtask_sql_binds_large_task_ids_as_decimal_strings` 固化了该不变量。

分页 token 是上一页最后一项的 ID，不是偏移量；ID 降序且使用严格小于条件，避免插入新历史记录时造成基于 offset 的重复或漂移。只有实际多取到一行时才设置 `HasMore` 和非零 `NextPageToken`。

错误摘要不保存或返回原始消息。`ErrorCode` 优先采用非空且非 `"0"` 的 RFC code；RFC code 为空而数字 code 非零时回退为数字字符串。分类只在有错误时产生：`failed` 状态固定为 `failed`；`reverted` 依次检查取消错误、已知导入/唯一键数据错误，其他回退为 `failed`；其他状态为空。时间列为 NULL 时保持 `UNIX_EPOCH` 零值。

## 依赖与调用关系

上游直接证据包括：

- `pkg/dxf/framework/scheduler/storage_adapter.rs` 将调度器的 `task_cleanup_info_by_ids`、`transfer_tasks_to_history` 和 `gc_subtasks` 分别接到本文件的三个 `TaskManager` 方法。
- `pkg/dxf/framework/scheduler/scheduler_manager.rs` 在完成批量清理后调用 `transfer_tasks_to_history`，并暴露 GC 转发。
- `pkg/dxf/framework/handle/status.rs::ListHistoryTasks` 将状态/观测 API 转发给运行时历史列表接口。
- `pkg/dxf/importinto/conflictrows.rs` 消费 `TaskCleanupInfo`，据任务状态和结束时间决定冲突文件是否可删。
- `pkg/dxf/framework/scheduler/scheduler.rs` 调用 `ClassifyTaskErrorMessage`，复用同一安全分类规则。

下游依赖均来自同 crate 装配上下文：`TaskManager::{ExecuteSQLWithNewSession,WithNewTxn}` 和 session SQL executor 负责数据库访问及事务；`TaskIDToKey` 保证子任务键类型；`row2TaskBasic`、`row2TaskError` 负责共享行解码；`IsCancelledErr` 识别取消；`proto` 定义任务类型与状态；`injectfailpoint`/`failpoint` 提供故障和保留期注入。Cargo 声明表明 crate 直接依赖本地 `proto`、配置与调度状态 crate，并依赖 `serde`/`serde_json`；本文件使用的 session、SQL 和时间兼容层由 `lib.rs` 内部实现提供，而非额外 Cargo 依赖。

## 错误处理与边界

所有数据库和注入错误都通过 `Result<_, Error>` 原样短路。`TransferTasks2History` 的事务边界是最重要的失败语义：任何更新、插入或删除失败都不能提交部分迁移。相对地，`TransferSubtasks2HistoryWithSession` 不自行开事务；调用者是否提供原子性取决于所传 session，且插入成功、删除失败时会返回删除错误。

分页大小必须位于闭区间 `[1, 200]`，否则在访问数据库前返回 `page size should be within [1, 200]`。实现假定 count 查询成功时至少返回一行，直接读取 `countRows[0]`；这是 SQL `count(1)` 的数据库契约。行转换依赖固定列数和类型，列布局变化必须同步修改常量、转换器和测试。

`GetTaskCleanupInfoByIDs` 对空输入甚至允许空/无效 manager 引用的 Go 语义对应为不触碰 `self` 的早返回；重复 ID 或活动/历史同时出现时，后遍历的同 ID 行覆盖 map 中旧值。结束秒数只有正值才转换，`checked_add` 避免时间溢出。

错误分类刻意采用字符串匹配，以避免依赖 Lightning 的错误类型；这意味着规范错误消息变化会影响 `data-error` 分类。匹配条件要求成对关键片段，降低普通消息误判风险。分类结果不得改为透传错误文本，否则会破坏历史 API 的脱敏边界。

## 并发与资源生命周期

文件自身不创建线程、异步任务、锁或通道。数据库 session 是核心资源：普通查询和 GC 通过 `ExecuteSQLWithNewSession` 获取短生命周期 session；批量归档通过 `WithNewTxn` 独占一个事务 session，并在闭包成功后提交、失败后回滚；单独子任务迁移明确借用调用者提供的 session。

并发迁移期间，历史列表的数据查询和计数查询可能观察到不同快照，代码和 Go 实现均接受该偏差，因此 `ApproxTotalCount` 不能用于驱动分页终止。keyset token 仍由单次数据查询确定。归档期间任务及其子任务的跨表一致性依赖 `WithNewTxn`，而读取侧其他 `*WithHistory` 方法通过同时查询活动和历史表消除迁移窗口中的漏读。

GC 与归档可以并发执行；删除判定只基于历史子任务的 `state_update_time` 和保留秒数。本文件没有显式互斥，隔离和原子性由 SQL 后端承担。failpoint 在 SQL 前返回时不会占用事务或执行删除。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/storage/history.go`。Rust 保留了 Go 的核心顺序、SQL 条件、分页协议、错误分类和 GC 规则：空批次早返回、归档前刷新 meta、任务先迁移再迁子任务、字符串 task key 加引号、`pageSize + 1` 判定更多页、计数分离读取，以及 14 天默认子任务保留期。

可见差异包括：Rust 将 Go 的 `*HistoryTaskSummary`/`*TaskCleanupInfo` 改为值对象，将 `*time.Time` 改为 `Option<SystemTime>`；空时间在摘要中仍用 `UNIX_EPOCH`。Rust 的 `taskErrorCode` 在 RFC code 为空且数字 code 非零时回退为数字字符串，以兼容历史序列化数据；Go 当前只从可归一化的 PingCAP error 取非零 RFC code，仓库 Rust 测试明确覆盖了命名 RFC code、`kv:1062` 和纯数字 code。Rust 另提供 `ClassifyTaskErrorMessage`，供调度器无需构造存储错误即可复用分类。

Go 测试 `history_test.go::TestClassifyTaskError`、`table_test.go::TestTransferTasks2HistoryWithAdjacentLargeTaskIDs`、`TestGetTaskCleanupInfoByIDs` 以及历史分页/GC 子测试提供迁移语义基线；Rust 的 `converter_1_aster_unit_test.rs`、`table_test.rs`、`sql_backend_test.rs` 提供独立可执行对应证据。

## 扩展指南

- 增加历史摘要列时，应同步修改 `historyTaskSummaryColumns`、`HistoryTaskSummary`、`row2HistoryTaskSummary` 的索引，并更新 `table_test.rs` 的列序、NULL/畸形值测试和 Go 对应实现；不要在 Rust 源文件内嵌测试。
- 调整分页协议时优先修改 `ValidateHistoryTaskPageSize` 和 `ListHistoryTasks`，保持 token 为稳定排序键，并同步 `converter_1_aster_unit_test.rs::history_keyset_pagination_and_count_follow_go_queries`、`table_test.rs` 及 Go 分页回归。若希望总数强一致，必须明确评估把两个查询纳入同一事务的 session 成本。
- 修改归档步骤时必须保持元数据脱敏先于复制、活动/历史写入处于同一事务，并扩展 `sql_backend_test.rs::batch_history_transfer_rolls_back_at_each_sql_failure` 覆盖每个新 SQL 失败点。子任务 key 仍须按字符串比较，不能去掉引号或改为浮点可损的数值比较。
- 增加错误类别时在 `ClassifyTaskErrorMessage`/`isDataError` 接入，返回固定枚举式字符串，禁止暴露原始错误；同步 Rust 分类矩阵与 `history_test.go`。消息匹配变化要评估旧历史错误的向后兼容。
- 调整保留期或 GC 条件时修改 `GCSubtasks` 及 `defaultSubtaskKeepDays` 的定义/测试，并保留 failpoint 以验证边界；评估大表删除的锁、批量化和索引性能。
- 新增清理字段时扩展 `TaskCleanupInfo` 和活动/历史联合查询，保证两侧列布局一致，并同步 `pkg/dxf/importinto/conflictrows.rs` 消费逻辑及独立测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/dxf/framework/storage/history.rs` 定位目标；`node --file ... --offset 1 --limit 500` 返回完整 365 行及 20 个符号；`query` 核对了 `GetTaskCleanupInfoByIDs`、两种迁移、分页、GC、转换和分类符号。对精确 Rust `ListHistoryTasks` 节点执行 callers/callees 未返回边，因此调用关系使用下列源码搜索补证。
- 生产源码：`pkg/dxf/framework/storage/history.rs`、`lib.rs`、`task_table.rs`、`converter.rs`、`task_state.rs`；上游 `pkg/dxf/framework/scheduler/storage_adapter.rs`、`scheduler_manager.rs`、`scheduler.rs`、`pkg/dxf/framework/handle/status.rs`、`pkg/dxf/importinto/conflictrows.rs`。
- crate/Go 对照：`pkg/dxf/framework/storage/Cargo.toml`、`pkg/dxf/framework/storage/history.go`。
- Rust 测试：`pkg/dxf/framework/storage/converter_1_aster_unit_test.rs`（校验上下界、分页参数和归档 SQL 顺序）、`table_test.rs`（清理信息、列序、NULL/错误码分类和大 ID 绑定）、`sql_backend_test.rs`（各 SQL 失败点回滚）；相关调用层测试位于 `pkg/dxf/framework/handle/status_testkit_test.rs` 和调度器测试目录。
- Go 测试：`pkg/dxf/framework/storage/history_test.go` 与 `table_test.go`，覆盖分类、真实表迁移、GC、大 ID 隔离、清理时间和分页/keyspace 行为。
- 本任务是纯文档分析，按计划不运行 Cargo。最终结构检查验证目标文件存在且恰有十一个规定的二级标题；人工复核重点为 SQL 顺序、列索引、错误脱敏、事务边界和 Go/Rust 差异均有上述源码或测试依据。
