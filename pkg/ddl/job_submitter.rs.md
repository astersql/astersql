# `pkg/ddl/job_submitter.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-ddl`，由 `pkg/ddl/lib.rs` 以 `pub mod job_submitter` 公开。它实现一个内存化的 DDL Job 提交模型：接收 [`JobSpec`](job_submitter.rs) 列表，按规则合并可批量执行的 `CREATE TABLE`，以 Job ID 去重后写入内存映射和待处理队列，并记录“有新任务”的通知次数。

它目前不是 Rust DDL 生产主链的持久化提交器。仓库内非测试 Rust 源码没有调用本文件的 `JobSubmitter`、`JobSpec` 或两个合并辅助函数；直接调用者是 `pkg/ddl/job_submitter_test.rs`、`pkg/ddl/executor_nokit_test.rs` 和 `pkg/ddl/tests/fastcreatetable/fastcreatetable_test.rs`。Rust 的真实系统表提交能力另位于 `pkg/ddl/jobsubmit/submit.rs`，会处理会话池、校验、全局 ID 分配和 `mysql.tidb_ddl_job` 插入。因此，阅读本文件时应把它视为已公开、由测试约束的局部语义模型，而不能等同于 Go `pkg/ddl/job_submitter.go` 的完整生产实现。

按照 DDL 包契约（`pkg/ddl/doc.go`），完整 DDL 需要维持全局 schema 版本同步不变量；本文件只覆盖“提交前合并”和“内存排队”这一小段，不实现 schema 状态转换、owner 调度、重组回填或版本同步。

## 核心职责

1. 用 `JobSpec` 为 `crate::ddl::Job` 增加“ID 是否已分配”“是否含外键”和“被合并的原始 Job”三组提交期信息。
2. `merge_create_table_jobs` 识别可合并的建表 SQL，按 `schema_id` 分组，将每组均匀拆成最大 8 个 Job 的批次，并把批次压缩成一个宿主 `JobSpec`。
3. `build_query_string_from_jobs` 规范化批次中的 SQL：去除每条语句首尾空白，缺少尾分号时补分号，再以单个空格连接。
4. `JobSubmitter::submit` 对合并后的宿主 Job ID 去重，成功项写入 `persisted` 与 `pending`，并在本次至少成功一项时将通知计数加一。
5. `take_pending` 以移动方式取走整个待处理队列；`notification_count` 暴露累计通知次数供测试或上层观测。

本文件不负责分配 Job/Table ID，不把数据写入系统表，不启动后台线程，不唤醒 DDL owner，也不等待 Job 完成。其 `persisted` 名称表示进程内去重状态，并非持久化存储。

## 主要符号

- `pub struct JobSpec`：提交单元。`job` 是主 Job；`id_allocated` 和 `has_foreign_keys` 控制快速建表合并资格；`merged_jobs` 在合并后保存该批全部原始 `Job`（包含宿主自身）。派生 `Clone/Debug/Eq/PartialEq`，便于队列复制和精确测试。
- `JobSpec::new(job, id_allocated) -> JobSpec`：便捷构造器，默认 `has_foreign_keys = false`、`merged_jobs = []`。调用方若有外键必须在构造后显式设置标志，否则会被视为可合并候选。
- `pub struct JobSubmitter`：包含 `pending: Vec<JobSpec>`、`persisted: BTreeMap<i64, Job>`、`notifications: usize`。字段均为私有，通过默认构造和方法维护。
- `JobSubmitter::submit(&mut self, Vec<JobSpec>) -> Vec<Result<i64, String>>`：先合并再逐宿主提交。返回向量对应“合并后的输出项”，并不保证与原始输入一一对应；两个可合并输入会只返回一个宿主 ID，这一点由 `submitter_merges_create_table_jobs_and_notifies_once` 固定。
- `JobSubmitter::take_pending(&mut self) -> Vec<JobSpec>`：使用 `std::mem::take` 返回旧队列并把内部队列替换为空向量；不删除 `persisted`，因此取走待处理项后再次提交同 ID 仍会重复。
- `JobSubmitter::notification_count(&self) -> usize`：只读返回累计计数。每次 `submit` 只要至少一个合并后项成功就加一，与成功 Job 数量无关；全重复批次不增加。
- `pub fn merge_create_table_jobs(Vec<JobSpec>) -> Vec<JobSpec>`：纯函数式合并入口。候选条件以 SQL 文本（忽略前导空白和 ASCII 大小写后以 `create table` 开头）、`!id_allocated`、`!has_foreign_keys` 三者共同决定；代码不检查 `JobState`。
- `pub fn build_query_string_from_jobs(&[JobSpec]) -> String`：纯格式化函数。空切片返回空串；已有一个或多个尾分号的 SQL 原样保留尾分号，只处理首尾空白。
- `MAX_BATCH_SIZE: usize = 8`：定义在 `merge_create_table_jobs` 函数内部，不是模块公开常量。

文件没有 trait、异步函数、条件编译项或自定义错误类型。

## 执行流程

`JobSubmitter::submit` 的主流程如下：

1. 把输入整体交给 `merge_create_table_jobs`。
2. 合并函数逐个检查资格。不可合并项进入 `passthrough`；可合并项以 `job.schema_id` 为键进入 `BTreeMap`。
3. 每个 schema 组先按 `job.id` 升序排序。组大小为 `n` 时，先计算 `ceil(n / 8)` 个批次，再用商和余数把元素尽量均匀地分到各批；例如 9 个拆成 5/4，22 个拆成 8/7/7（测试比较时会排序为 7/7/8）。
4. 单元素批次原样进入输出。多元素批次取最低 ID 的首项为宿主，把全批次的 `Job` 克隆进宿主 `merged_jobs`，并用 `build_query_string_from_jobs` 重写宿主 `job.query`。其余 `JobSpec` 不再单独出现在输出中。
5. 所有透传项和合并宿主按 `job.id` 全局升序排序后返回。
6. `submit` 遍历合并结果：若 `persisted` 已含宿主 ID，记录 `Err("DDL job … already exists")` 并跳过；否则先克隆主 Job 写入 `persisted`，再克隆整个规格写入 `pending`，返回 `Ok(id)`。
7. 若结果中存在任意 `Ok`，`notifications += 1`；随后返回逐合并项结果。

下游通过 `take_pending` 一次性取得当前队列。这个动作只转移内存所有权，不执行 Job，也不把执行结果反馈到原始被合并项；集成测试 `test_merged_job` 使用自己的 `run_pending_jobs` 最小执行器模拟这一后续阶段。

## 数据与状态

- `pending` 保存尚未由调用方取走的 `JobSpec`。提交成功后追加，顺序由合并函数最终的 Job ID 排序决定；多次 `submit` 之间仍保持调用批次的追加顺序。
- `persisted` 仅保存 `Job` 克隆，以 ID 为键，生命周期与 `JobSubmitter` 实例相同。它不会因 `take_pending` 清理，承担跨提交调用的重复 ID 防护；没有完成、取消或历史清理 API。
- `notifications` 是单调递增的进程内计数。空输入、全重复输入不增加；混合成功/重复输入增加一次。
- `merged_jobs` 为空表示未发生多 Job 合并；非空时含全批原始 Job，包括宿主原始 Job。宿主的 ID、schema_id 和其他除 `query` 外的字段来自排序后的首项。
- `BTreeMap` 同时用于 schema 分组和持久化映射，使 schema 遍历与 Job 输出稳定排序；但公开契约只由代码和测试确认最终按 Job ID 排序，不应依赖中间分组遍历细节。
- SQL 识别是文本启发式：`create tablex ...` 也满足当前 `starts_with("create table")`，而以注释、提示或其他前缀开头的合法建表 SQL 不满足。这里没有解析 AST 或检查 `ActionType`。

## 依赖与调用关系

本文件的直接依赖很小：

- `crate::ddl::Job`（`pkg/ddl/ddl.rs`）提供 `id`、`query`、`schema_id` 等数据；`Job::new` 生成 `JobState::None` 的初始 Job，但合并函数本身不依赖状态。
- 标准库 `BTreeMap` 提供稳定有序的 schema 分组和 ID 映射；`std::mem::take` 清空 pending；迭代器完成 Job 克隆和 SQL 拼接。
- `pkg/ddl/lib.rs` 将模块公开，并在 `#[cfg(test)]` 下装配 `executor_nokit_test` 与 `job_submitter_test`。
- `pkg/ddl/Cargo.toml` 声明 crate 名为 `astersql-ddl`、库入口为 `lib.rs`；本文件自身没有使用该 manifest 中的外部依赖或 feature gate。

已确认的 Rust 上游均为测试：

- `pkg/ddl/job_submitter_test.rs` 直接验证提交合并、重复 ID、通知次数、分号保持以及状态不影响合并。
- `pkg/ddl/executor_nokit_test.rs` 直接验证 SQL 拼接、资格规则、同 schema 查询串和最多 8 个的均衡批次。
- `pkg/ddl/tests/fastcreatetable/fastcreatetable_test.rs` 通过公开 crate API 使用 `JobSubmitter`，模拟合并批次的事务性共同成功/失败和自增起点。

RustCodeGraph 将目标文件标为被 `ddl.rs` 等多个文件“used by”，但精确 callers 查询没有产生函数调用边；仓库级非测试 Rust 搜索也未发现本文件符号的生产调用。因此不能据图的文件级依赖声称 `Ddl::submit_job` 会进入本提交器。当前 `pkg/ddl/ddl.rs::Ddl::submit_job` 是另一条独立的内存 Job 状态路径。

Go 生产链则明确为 `pkg/ddl/ddl.go::Start` 启动 `JobSubmitter.submitLoop`，后者批量读取 `limitJobCh`，调用 `addBatchDDLJobs`；成功写表后 `notifyNewJobSubmitted` 向本地 owner 通道或 etcd 发通知，owner 侧 `pkg/ddl/job_scheduler.go::ownerListener` 把同一通知通道交给 scheduler。

## 错误处理与边界

- 本文件唯一显式业务错误是重复宿主 Job ID，表现为单项 `Err(String)`；同一批其他项仍继续提交，没有整批回滚。
- 合并发生在去重前。若多个原始 Job 合并成一个宿主，只有宿主（最低）ID参与 `persisted` 去重；被吸收 Job 的 ID 不写入 `persisted`，之后可作为独立宿主再次提交。
- 先写 `persisted` 再写 `pending`，两步均为不可失败的内存操作；当前没有产生“已持久化但未排队”的错误分支。
- 空输入返回空结果、不通知。单个合并候选原样透传。分组计算仅发生在非空组上，因此 `div_ceil` 后的 `batch_count` 不会为零。
- SQL 拼接不验证空查询；空白查询会变成 `";"`。它不解析或转义 SQL 内部的分号，也不处理语句语义。
- Rust 合并函数当前不会像 Go `mergeCreateTableJobsOfSameSchema` 那样检查同批重复表名，也没有可返回的合并错误；重复表冲突只能由后续执行层发现。
- 源码第 83 行注释提到“状态为 None”，但实现和 `merge_eligibility_does_not_depend_on_job_state` 都证明状态不参与资格判断。扩展时应以实现和测试为准，并同步修正文档注释以避免漂移。
- `submit` 文档注释称返回值“与输入对应”，实际在合并后长度会缩短；调用方必须按合并结果而非原始输入索引解释返回值。

## 并发与资源生命周期

`JobSubmitter` 方法使用 `&mut self`，自身不含 `Arc`、锁、原子、通道或后台任务；Rust 借用规则保证单个可变引用调用期间的独占访问，但类型没有提供跨线程共享协议。若上层自行放入锁中，共享、阻塞与关闭语义由上层负责。

所有 Job、规格和 SQL 都在内存中拥有或克隆：提交时主 Job 至少克隆进 `persisted`，完整规格再克隆进 `pending`；合并还会把每个原始 Job 克隆进 `merged_jobs`，并新建拼接后的查询字符串。批量建表数量大时，这带来与 Job 元数据及 SQL 长度成正比的内存复制成本。

`take_pending` 将向量缓冲区一并移出，提交器随后持有新的空向量；取得的队列完全归调用方所有。提交器没有 stop/close/drop 清理逻辑，实例销毁时由 Rust 自动释放全部 Job 和字符串。`notifications` 只是计数，不代表真实通知已被消费，也没有丢通知或背压模型。

与此相对，Go 同名实现的 `submitLoop` 持有 context、输入通道、session pool、系统表 manager、完成通道映射和 owner 通知通道；它在 context 取消时退出，并在系统表事务提交前注册完成通道、失败时清理。本 Rust 文件尚未移植这些生命周期保证。

## 与 Go 版本的对应关系

一致或有意对齐的局部语义：

- Rust `merge_create_table_jobs` 对应 Go `mergeCreateTableJobs`：排除预分配 ID 和含外键的建表任务；同 schema 分组；最大批次为 8；使用均匀拆批策略。Go `executor_nokit_test.go::TestMergeCreateTableJobs` 与 Rust 两个 nokit 测试固定了 9→5/4、7→7、22→8/7/7 等结果。
- Rust `build_query_string_from_jobs` 对应 Go `buildQueryStringFromJobs`：`TrimSpace`/`trim`、仅在无尾分号时补分号、语句间加单个空格；两端的 nokit 测试使用相同示例。
- Rust 合并宿主采用组内最低 ID，是 Rust 为稳定结果增加的排序规则；Go 采用 map 分组后每组输入顺序的首项，未承诺跨组最终顺序。

尚未对齐或只是简化模型的部分：

- Go 按 `model.ActionCreateTable` 判断动作并按 `SchemaName` 分组；Rust 按查询字符串前缀判断并按数值 `schema_id` 分组。
- Go 合并为 `ActionCreateTables`，组装 `BatchCreateTableArgs`、`InvolvingSchemaInfo`，检查重复表名并合并所有结果通道；Rust 只改写宿主 query、记录 `merged_jobs`，没有动作类型、参数或结果通道等价物。
- Go 快速建表由 `vardef.EnableFastCreateTable` 控制，合并失败会记录告警并回退原 Job；Rust `submit` 总是尝试合并，没有 feature/运行时开关和失败回退。
- Go `addBatchDDLJobs2Table` 调用 `jobsubmit.SubmitBatch` 分配全局 ID并写 `mysql.tidb_ddl_job`，还注册完成通道；Rust 只写进程内 `BTreeMap`。
- Go `notifyNewJobSubmitted` 根据是否 owner 选择本地异步通知或 etcd；Rust 只增加整数计数。
- Go 会对每个原始 `JobWrapper` 传回提交结果和指标；Rust 合并后只产生宿主级结果，无法逐原始 Job 通知。

因此，本文件当前能用于验证快速建表的核心纯逻辑和最小排队行为，但不能作为 Go 提交器已完整移植的证据。

## 扩展指南

- 修改合并资格时，优先改 `merge_create_table_jobs`，并同步 `pkg/ddl/job_submitter_test.rs` 与 `pkg/ddl/executor_nokit_test.rs`；若目标是对齐 Go，应引入明确动作类型而不是继续扩展 SQL 字符串前缀判断，并覆盖注释/提示、大小写和相似前缀。
- 修改批次大小或分配算法时，调整函数内 `MAX_BATCH_SIZE`，同步 9/7/22 的均衡拆批断言，并评估查询字符串长度、Job 克隆内存和下游单批事务成本。
- 增加外键、重复表名或批量参数语义时，应在 `JobSpec` 中携带结构化元数据，在合并前验证；不要靠 SQL 文本反向解析。需要对齐 Go 的重复表名错误和“整组合并失败”行为。
- 修改提交结果契约前，先决定结果是按原始输入还是按合并宿主返回。目前实现是后者，源码注释却暗示前者；若改为逐输入结果，需要保留宿主到原始 Job/结果接收者的映射。
- 若把本模型接入生产 DDL，不能只添加一个调用点：必须复用 `pkg/ddl/jobsubmit/submit.rs` 的全局 ID、系统表事务和清理契约，并补 owner 通知、完成通道、故障恢复、幂等与停机生命周期。不要建立第二套内存“持久化”路径。
- 增加并发访问时，应由外层明确定义锁粒度、通知和队列消费协议；避免在持锁期间执行系统表 I/O。对并发重复 ID、部分成功、通知合并和关闭竞态增加独立测试。
- Rust 单元测试继续放在同目录独立 `*_test.rs` 中，不要内嵌回生产文件。生产链扩展还应同步或新增 `pkg/ddl/tests/fastcreatetable/fastcreatetable_test.rs` 一类集成行为测试。

主要兼容风险是改变合并资格或返回向量基数后破坏上层结果关联；正确性风险是仅记录宿主 ID导致被合并 ID可重复提交；性能风险来自大 SQL 和 `merged_jobs` 的多次克隆。任何生产化改动还需评估系统表格式、全局 ID、owner failover 和 schema 同步兼容性。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件、4,415 个 Go 文件；目标源码通过 `node --file pkg/ddl/job_submitter.rs` 完整读取为 154 行。
- RustCodeGraph `query`：确认 `job_submitter.rs::merge_create_table_jobs(Vec<JobSpec>) -> Vec<JobSpec>`、`job_submitter.rs::build_query_string_from_jobs(&[JobSpec]) -> String`，以及对应的三个 Rust nokit 测试符号。对精确符号执行 `callers`/`callees` 未返回调用边，故再以模块装配和仓库搜索核验真实调用范围。
- 已读 Rust 源码：`pkg/ddl/job_submitter.rs`、`pkg/ddl/ddl.rs`、`pkg/ddl/lib.rs`；其中 `lib.rs` 公开模块并装配独立测试，`ddl.rs` 定义 `Job` 且显示 `Ddl::submit_job` 是独立路径。
- 已读 Cargo：`pkg/ddl/Cargo.toml`，确认 crate 名、`lib.rs` 入口、Go 包映射以及本文件无条件编译 feature。
- 已读 Rust 测试：`pkg/ddl/job_submitter_test.rs`、`pkg/ddl/executor_nokit_test.rs`、`pkg/ddl/tests/fastcreatetable/fastcreatetable_test.rs`。关键断言覆盖合并后的单一返回、重复 ID、通知、分号、JobState 无关、资格规则、均衡批次及批次共同成败。
- 已读 Go 对照：`pkg/ddl/job_submitter.go`、`pkg/ddl/executor_nokit_test.go`、`pkg/ddl/job_submitter_test.go`、`pkg/ddl/ddl.go`、`pkg/ddl/job_scheduler.go`，确认完整 Go 主链、合并细节、持久化与 owner 通知差异。
- 已读包级契约和 DDL 导航：`pkg/ddl/doc.go`、`docs/agents/ddl/README.md`。后者只作为导航，所有本文行为结论均回查到源码或测试。
- 仓库搜索 `JobSubmitter|JobSpec|merge_create_table_jobs|build_query_string_from_jobs` 并排除测试与目标文件后，没有发现本模块的非测试生产调用；同名 Lightning trait 和 `pkg/ddl/jobsubmit::JobSpec` 属于不同类型，不混为本文件调用者。
- 本任务为纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰有固定的 11 个二级标题；内容已人工复核，能够回答文件为何存在、当前如何运行、与 Go 的差距及安全扩展位置。
