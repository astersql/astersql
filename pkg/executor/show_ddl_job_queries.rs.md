# [`pkg/executor/show_ddl_job_queries.rs`](show_ddl_job_queries.rs)

## 文件定位

本文件位于 `astersql-executor` crate，并由 `pkg/executor/lib.rs` 以 `pub mod show_ddl_job_queries` 公开。它保存 `ADMIN SHOW DDL JOB QUERIES` 两种形式的 Rust 移植核心：按 Job ID 查询，以及按 `offset`/`limit` 查询。`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "pkg/executor"` 共同确认其 crate 与 Go 包对应关系。

当前接线需作区分：`pkg/executor/builder.rs::build` 会把 `Plan::ShowDdlJobQueries` 和 `Plan::ShowDdlJobQueriesWithRange` 分发到同名 builder 方法，但这两个方法仅调用 `build_leaf` 生成对应 `ExecutorKind`；仓库中没有本文件的 `DDLJobSource`、`StringChunk` 实现，也没有构造 `ShowDDLJobQueriesExec` 或 `ShowDDLJobQueriesWithRangeExec` 的 Rust 代码。因此，本文件已经表达核心算法与资源边界，却尚无证据表明它已接入 Rust 实际执行链。Go 的完整接线位于 `pkg/executor/builder.go::buildShowDDLJobQueries*`。

## 核心职责

- 用 `DDLJob` 抽象作业 ID 与原始 SQL，用 `DDLJobSource` 抽象基础执行器打开、内部 DDL 事务、运行中/历史作业读取与 chunk 大小配置。
- 在一次内部 DDL 事务中取得运行中作业和历史作业，然后由 `append_distinct_jobs` 按 ID 合并；运行中集合排在历史集合之前，同一 ID 只保留首次出现者。这用于处理读取期间作业从运行态进入历史态的重叠窗口。
- `ShowDDLJobQueriesExec` 只输出命中 `jobIDs` 的 query 字符串；历史读取固定限制为 `DEFAULT_HISTORY_JOB_COUNT`（10）。
- `ShowDDLJobQueriesWithRangeExec` 读取至多 `offset + limit` 条历史作业，将游标前移到 offset，并输出作业 ID 和 query 两列。
- 两个 `Next` 都以输出 chunk 容量为批次上限并推进 `cursor`，支持多轮拉取。

## 主要符号

- `DEFAULT_HISTORY_JOB_COUNT: usize = 10`：按 ID 形式的历史搜索窗口，对应 Go 的 `ddl.DefNumHistoryJobs` 当前语义。
- `DDLJob`：只读 trait；`id() -> i64` 是去重和匹配键，`query() -> &str` 提供输出文本。
- `DDLJobSource`：依赖倒置边界。关联类型 `Error`、`Job`、`Context` 让算法不绑定具体 session、事务和作业类型。`with_internal_ddl_transaction` 接收一次性闭包，并约定闭包返回后释放系统会话/回滚内部事务。
- `StringChunk`：结果缓冲区的最小接口；`grow_and_reset` 开始新批次，`capacity` 决定扫描窗口，`append_string` 写指定列。
- `append_distinct_jobs<J: DDLJob>`：私有合并函数。它以局部 `HashSet<i64>` 记录已追加 ID，遍历顺序为 `running.into_iter().chain(history)`。
- `ShowDDLJobQueriesExec<S>`：公开泛型结构，保存 `BaseExecutor`、`cursor`、`jobs` 和 `jobIDs`；`Open` 装载快照，`Next` 按 ID 输出单列 query。
- `ShowDDLJobQueriesWithRangeExec<S>`：公开泛型结构，额外保存 `offset`、`limit`；`Open` 建立范围窗口，`Next` 输出十进制 ID 与 query 两列。

字段与方法沿用 Go 风格命名（如 `BaseExecutor`、`jobIDs`、`Open`、`Next`），源码未使用条件编译项，也未为两种结构实现统一的 Rust executor trait。

## 执行流程

按 ID 形式的 `ShowDDLJobQueriesExec::Open`：

1. 调用 `BaseExecutor.open_base(context)`；错误立即返回，后续读取不发生。
2. 进入 `with_internal_ddl_transaction`，先调用 `running_jobs(context)`，再用限制 10 调用 `history_jobs`。任一步失败都经 `?` 退出闭包和 `Open`。
3. `append_distinct_jobs` 先追加运行中作业，再追加历史作业，过滤重复 ID；结果追加到已有 `self.jobs`，方法本身不会清空旧数据。

其 `Next` 先重置结果 chunk。游标到达末尾时返回空批；`jobIDs.len() >= jobs.len()` 时也直接返回，这是与 Go 源码一致的现有分支。否则扫描至多 `min(request.capacity(), jobs.len() - cursor)` 个作业：外层按请求 ID 顺序、内层按当前作业窗口顺序匹配，命中时向第 0 列写 query，最后将游标推进整个扫描窗口而非命中数。

范围形式的 `ShowDDLJobQueriesWithRangeExec::Open` 采用相同资源流程，但以 `offset.wrapping_add(limit) as usize` 作为历史读取上限。合并去重后，若当前游标小于 `offset as usize`，将其直接移至 offset。

其 `Next` 重置 chunk，随后检查游标越界和 `offset > jobs.len()`。当前批次仍按 chunk 容量切分；遍历时遇到绝对位置 `>= offset.wrapping_add(limit) as usize` 即停止，未停止的行向第 0 列写十进制 Job ID、第 1 列写 query。无论因 limit 提前停止与否，结尾都将游标增加 `current_batch`。

## 数据与状态

两个执行器都是有状态的分页器：`jobs` 保存 `Open` 获取的合并结果，`cursor` 保存下一批扫描起点；`Next` 不再访问作业源。调用者必须在开始迭代前完成 `Open`，并保持同一实例直至消费结束。

合并结果的稳定次序来自输入：全部运行中作业在前，随后是未重复的历史作业。`HashSet` 仅用于成员判断，不参与输出遍历，因此不会引入哈希迭代乱序。相同 Job ID 在两组中出现时保留运行中对象。

按 ID 形式输出只有 query 一列，且输出顺序首先受 `jobIDs` 顺序影响；重复请求 ID 可产生重复 query。范围形式输出两列，并以合并后 `jobs` 的顺序分页。文件不缓存字符串副本：`query()` 借用作业内文本，仅在 `append_string` 调用期间使用。

`Open` 向已有 `jobs` 追加，且不重置 `cursor`；重复打开同一实例不是幂等操作。范围形式会把游标至少提升到 offset，但不会把已超过 offset 的游标后退。

## 依赖与调用关系

上游静态关系为：`pkg/executor/lib.rs` 导出模块；`pkg/executor/builder.rs::build` 识别两种 `Plan`，并通过 `buildShowDDLJobQueries*` 生成 `ExecutorKind::ShowDdlJobQueries*` 叶节点。`pkg/planner/core/common_plans.rs` 定义对应计划数据结构，`pkg/planner/core/planbuilder.rs` 提供相关输出 schema 构造函数。不过仓库搜索没有找到从这些叶节点到本文件结构体的 Rust 构造边，也没有找到本文件三个 trait 的具体实现，这是当前移植接线缺口。

本文件内部调用链为：两种 `Open` -> `DDLJobSource::{open_base, with_internal_ddl_transaction, running_jobs, history_jobs}` -> `append_distinct_jobs` -> `DDLJob::id`；两种 `Next` -> `StringChunk::{grow_and_reset, capacity, append_string}`，并调用 `DDLJob::{id, query}`。唯一直接标准库依赖是 `std::collections::HashSet`；具体 DDL、meta、sessiontxn、chunk 依赖被隔离在 trait 实现之外。

Go 运行链由 `pkg/executor/builder.go` 构造具体执行器；`pkg/executor/show_ddl_job_queries.go` 再直接依赖 `ddl`、`meta`、`sessiontxn`、`kv` 与 `chunk`。Rust 文件把这些具体依赖收拢为 `DDLJobSource` 和 `StringChunk`，但尚无对应适配器。

## 错误处理与边界

所有可失败的操作都使用 `S::Error` 原样传播；本文件不包装、不记录也不降级错误。`open_base`、进入/退出内部事务边界、运行中作业读取、历史作业读取的失败都会使 `Open` 返回 `Err`。`Next` 自身没有新增失败点，但为统一执行器接口仍返回 `Result<(), S::Error>`。

边界行为包括：空作业集合、游标到末尾和超出范围的 offset 都产生成功的空批；范围 `limit = 0` 时循环在首行即停止；chunk 容量为 0 时游标不推进，调用者若持续拉取会持续得到空批。按 ID 路径中的 `jobIDs.len() >= jobs.len()` 会直接返回，即使集合中可能存在匹配 ID；这是 Go/Rust 当前共同逻辑，扩展时不能默默“纠正”而不补回归依据。

范围加法显式使用 `wrapping_add`，随后转换为 `usize`。这复刻 Go `uint64` 加法的回绕倾向，但在 `usize` 小于 64 位的平台还可能截断；当前 Rust issue 测试只覆盖 268,430,000 级别的大值，并未覆盖 `u64` 溢出。`offset as usize` 同样具有平台相关截断风险。

## 并发与资源生命周期

本文件不创建线程、任务、锁或通道。并发一致性由 `DDLJobSource::with_internal_ddl_transaction` 的实现负责：契约要求创建系统会话事务、标记 in-transaction，并在闭包结束（成功或失败）后按内部 DDL source type 释放资源。Rust 闭包与可变借用使运行中和历史读取发生在同一 source 生命周期内；但真正的 rollback/release 保证只有具体实现后才能验证。

读取结果可能跨越运行态到历史态的转换，因此 `append_distinct_jobs` 是并发场景的关键不变量：同一 ID 最多加入一次，且运行中版本优先。之后 `jobs` 属于执行器实例，`Next(&mut self, ...)` 串行推进游标；该 API 不声明实例可在多个消费者间并发共享。

Go `Open` 通过 `defer ReleaseSysSession(...)` 保证系统会话释放及事务自动回滚；Rust 用 trait 文档把相同责任转交给 `with_internal_ddl_transaction`。在实现该 trait 时，必须保证错误路径和 panic/提前返回路径也能完成清理，不能只在闭包成功时释放。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/show_ddl_job_queries.go`。两边均先打开 base executor，再在内部事务读取运行中与历史作业，按 Job ID 去重并保持“运行中优先”，随后按 chunk 容量分页。默认历史条数 10、按 ID 形式的一列 query、范围形式的 ID/query 两列，以及游标推进方式均逐句对应。

主要结构差异是 Rust 通过泛型 trait 隔离具体系统会话、meta mutator、DDL API 与 chunk；Go 直接调用 `GetSysSession`、`sessiontxn.NewTxn`、`session.Txn(true)`、`ddl.GetAllDDLJobs`、`ddl.GetLastNHistoryDDLJobs`。Rust 的 `with_internal_ddl_transaction` 契约意在承载 Go 的 `defer ReleaseSysSession` 自动回滚语义。

测试证据以 Go 为主：`pkg/executor/test/executor/executor_test.go` 覆盖未知/重复 Job ID、最近历史作业、两种 limit/offset 写法和 DDL 并发期间结果不重复；`pkg/executor/test/issuetest/executor_issue_test.go::TestIssue42298` 覆盖大 limit 与大 offset。`pkg/executor/test/issuetest/executor_issue_test.rs::test_issue42298` 是可执行 Rust 风格用例，但走 testkit SQL 层，不能单独证明本文件已被接线；`pkg/executor/test/executor/executor_test.rs` 明确把大段 Go 原文保存在 `GO_EXECUTOR_TEST_REFERENCE` 字符串中，其中的对应场景只是迁移参考，不是本算法的可执行单元测试。当前没有 `show_ddl_job_queries_test.rs`。

## 扩展指南

- 接入真实 Rust 执行链时，应实现 `DDLJob`、`DDLJobSource` 和 `StringChunk` 适配器，并在 executor factory 中把两个 `ExecutorKind` 实例化为本文件结构；同时验证计划字段 `JobIDs`、`Limit`、`Offset` 与输出 schema 列数。不要只改 `builder.rs::build_leaf` 的枚举接线。
- 修改作业来源、事务语义或错误模型时，优先改 `DDLJobSource` 契约与实现；必须保持系统会话在所有返回路径上释放，并为打开失败、运行作业读取失败、历史读取失败分别添加测试。
- 修改合并顺序或去重键时，应聚焦 `append_distinct_jobs`，增加独立的 `pkg/executor/show_ddl_job_queries_test.rs`，至少覆盖组内重复、跨运行/历史重复、空输入及顺序稳定性。测试逻辑不要内嵌回生产文件。
- 修改分页逻辑时，应分别覆盖多 chunk、零容量、空 jobIDs、重复 jobIDs、`jobIDs.len() >= jobs.len()`、offset 等于/大于长度、limit 为 0，以及 `offset + limit` 溢出。需先确认与 Go 的兼容意图，再决定是否同步修复 Go；当前分支不能按直觉删减。
- 性能上，按 ID 形式当前复杂度约为“请求 ID 数 × 当前批作业数”；若改为索引查找，必须保留请求 ID 顺序、重复 ID 行为和批次游标语义。历史读取量随 `offset + limit` 增长，大范围请求还会放大内存占用。
- 新增 Rust 回归测试应放在独立测试文件，并从 `pkg/executor/lib.rs` 通过 `#[cfg(test)] mod show_ddl_job_queries_test;` 接入；不能把测试放入本生产源文件。

## 验证依据

- RustCodeGraph 索引状态：项目含 11,467 个已索引文件；`query ShowDDLJobQueriesExec --kind struct` 和 `query ShowDDLJobQueriesWithRangeExec --kind struct` 同时定位 Rust/Go 定义；`explore pkg/executor/show_ddl_job_queries.rs ShowDDLJobQueriesExecutor` 给出目标文件全文、内部调用以及未发现覆盖测试；针对 builder 的 explore 确认 `build` 是两个 Rust builder 方法的调用者。
- 已读源码：`pkg/executor/show_ddl_job_queries.rs`、`pkg/executor/lib.rs`、`pkg/executor/builder.rs`、`pkg/planner/core/common_plans.rs`、`pkg/planner/core/planbuilder.rs`。
- 已读 crate/Go 对照：`pkg/executor/Cargo.toml`、`pkg/executor/show_ddl_job_queries.go`、`pkg/executor/builder.go`。
- 已读测试证据：`pkg/executor/test/executor/executor_test.go`、`pkg/executor/test/executor/executor_test.rs`、`pkg/executor/test/issuetest/executor_issue_test.go`、`pkg/executor/test/issuetest/executor_issue_test.rs`。
- 仓库搜索确认：Rust 侧只有目标文件自身实现 `ShowDDLJobQueriesExec`/`ShowDDLJobQueriesWithRangeExec`，没有 `DDLJobSource for`、`StringChunk for` 或两结构体构造表达式；相关 SQL 回归位置如上。未运行 Cargo，符合本纯文档任务约束。
