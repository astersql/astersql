# `pkg/ddl/ddl_running_jobs.rs`

## 文件定位

本文件属于 `astersql-ddl` crate；crate 根在 [`pkg/ddl/lib.rs`](lib.rs) 以 `pub mod ddl_running_jobs` 导出模块，依赖边界由 [`pkg/ddl/Cargo.toml`](Cargo.toml) 定义。它位于 DDL owner 的作业调度路径中，不执行具体 schema 变更，而是在调度器把持久化作业交给 general/reorg worker 前，判断作业声明的对象范围能否与其他作业并行。

直接生产调用者是 [`pkg/ddl/job_scheduler.rs`](job_scheduler.rs) 的 `JobScheduler`。内存队列路径 `JobScheduler::schedule` 和持久化系统表路径 `JobScheduler::schedule_persisted` 都持有一个 `RunningJobs`，并围绕每次 worker step 调用 `check_runnable`、`add_pending`、`add_running`、`finish_or_pend_job` 和 `reset_all_pending`。因此，本文件是调度层的对象级冲突登记表，而不是数据库锁、事务锁或持久化队列本身。

## 核心职责

1. 用 `InvolvingSchemaInfo` 表示一个作业涉及的三类对象之一：库表、placement policy 或 resource group；用 `InvolvingMode::{Shared, Exclusive}` 表示共享/独占语义。
2. 用 `RunningJobs` 同时追踪运行作业 ID、运行中的独占/共享对象，以及本轮调度中已等待的对象。
3. 在 `RunningJobs::check_runnable` 中实现类似读写锁的兼容矩阵：共享之间可并行；独占与共享、独占冲突；任何新请求还要避让 `pending`，以防后到的共享请求持续越过先到的独占请求。
4. 用引用计数处理多个作业对同一对象的重复占用，并在作业完成时精确释放；用 `*` 表达表级或全局范围。
5. 提供独立的 `has_conflict`，用于比较两组对象描述是否重叠且至少一方为独占模式。当前仓库精确引用搜索只找到其定义，没有生产调用者；它是公开的纯判断 API，不参与 `JobScheduler` 当前主链。

## 主要符号

- `INVOLVING_ALL = "*"` 与 `INVOLVING_NONE = ""`：分别表示某层级的全部对象和该类别未被选择。库表对象要求 `database` 与 `table` 同时有值；例如 `db.*` 表示库内所有表，`*.*` 表示全部库表。
- `InvolvingMode`：`Shared` 类似读占用，`Exclusive` 类似写占用；`Default` 是 `Exclusive`，使未显式指定模式的对象采用更保守的冲突策略。
- `InvolvingSchemaInfo`：公开对象描述。`schema`、`policy`、`resource_group` 三个构造器分别选择一种对象类别，`shared` 将默认独占改为共享。合法性由内部 `assert_valid_info` 在登记或非空冲突检查时断言。
- `Objects`：私有的分类计数器。`schemas` 是 `database -> table -> count` 两层 `BTreeMap`，另两个 map 分别计数 policy 和 resource group。`add`、`remove`、`conflicts`、`is_empty`、`counts_are_positive` 封装引用计数和冲突查询。
- `RunningJobs`：核心状态。`running` 保存 `job_id -> involves`；`exclusive`、`shared`、`pending` 保存三类占用视图。`BTreeMap`/`BTreeSet` 使 ID 输出与集合迭代稳定有序。
- `RunningJobs::check_runnable`：只读准入检查；拒绝重复 job ID、已有全局独占和对象冲突，并处理空状态快速路径。
- `add_running`、`remove_running`、`finish_or_pend_job`：管理运行作业及占用计数。后者把“释放运行占用”和“转入 pending”放在一次 `&mut self` 调用内完成。
- `add_pending`、`reset_all_pending`：维护一轮扫描内的公平性屏障；pending 不是持久作业队列，也不保存 job ID。
- `all_ids`、`running_ids`：分别返回有序的逗号分隔 ID 和 `BTreeSet<i64>`；`invariants_hold` 暴露给测试检查计数表中无零计数或空的表级 map。
- `has_schema_conflict`、`object_overlap`、`decrement`：私有基础操作；`has_conflict` 则是公开的两组对象比较函数。

## 执行流程

内存调度路径可从 `JobScheduler::schedule` 反向验证：

1. 调度器从队首取出 `ScheduledJob`，把 `job.id` 和 `involves` 传给 `check_runnable`。
2. `check_runnable` 先拒绝已在 `running` 中的 ID；若 `exclusive.schemas` 已登记 `*`，也立即拒绝任何新作业。三份对象表都为空时直接放行。
3. 非空状态下逐个执行 `assert_valid_info`。独占 `*.*` 只有在系统完全空闲时才能通过；其他独占请求依次检查 `exclusive`、`shared`、`pending`，共享请求只检查 `exclusive` 和 `pending`。任意一个涉及对象冲突即拒绝整个作业。
4. 被拒绝的作业由调度器调用 `add_pending` 后重新排到队尾；被接受的作业先由 `add_running` 登记，再交给 general 或 reorg worker 推进一步。
5. worker step 后，调度器调用 `finish_or_pend_job`。该函数先依据模式释放 `exclusive`/`shared` 引用并从 `running` 删除 ID；未结束的作业（持久化路径还包括 step 出错）同时被加入 `pending`，从而在本轮剩余扫描中保留先后依赖。
6. 一轮扫描完成后 `reset_all_pending`。下一轮会根据仍在运行的真实占用重新判断，而不会永久保留上一轮的等待标记。

`schedule_persisted` 的对象输入来自持久化 `Job::get_involving_schema_info()`，随后把元数据层的 shared/exclusive 枚举转换为本文件的 `InvolvingMode`。这说明本文件消费由具体 DDL 动作生成的对象声明，自身不推断 SQL 或 job 类型。

## 数据与状态

`running` 是作业身份到原始对象列表的权威内存映射；释放时 `remove_running` 读取这里保存的列表，避免调用者另传一份可能不一致的描述。`exclusive` 和 `shared` 是从该映射派生的加速索引，`pending` 则是调度轮次级状态，不对应持久化记录。

所有对象索引使用引用计数而不是集合：两个共享作业可同时登记相同对象，移除其中一个时计数仍大于零，冲突关系继续存在。`Objects::remove`/`decrement` 在计数归零时删除键，并在数据库下无表条目时删除数据库键；`counts_are_positive` 对应这一不变量。

库表冲突按层级处理。`has_schema_conflict` 在同一 database map 内，将请求表为 `*`、已登记表为 `*` 或精确表名相等视为冲突。全库 `*.*` 的运行中独占由 `check_runnable` 的全局快速拒绝处理。policy/resource group 不支持通配符，按字符串精确匹配。公开 `has_conflict` 的 `object_overlap` 则对左右两侧 database/table 的 `*` 都做对称重叠判断。

## 依赖与调用关系

- 上游模块：`pkg/ddl/lib.rs` 声明公开模块；`pkg/ddl/job_scheduler.rs` 构造并驱动 `RunningJobs`。RustCodeGraph 对目标文件识别出 30 个符号，但方法级 `callers/callees` 查询未返回边，因此上述生产调用边由全仓精确引用和调度器源码补证。
- 输入来源：`JobScheduler::schedule_persisted` 从 `astersql-meta` 解码 Go 兼容 job，并从 `astersql-meta-model` 的 `InvolvingSchemaInfoMode` 转换模式；`schedule` 则接收已构造的 `ScheduledJob.involves`。
- 下游依赖：本文件只有标准库 `BTreeMap`、`BTreeSet`，不直接依赖 `Cargo.toml` 中的外部 crate，不访问 `mysql.tidb_ddl_job`，也不调用 worker、schema sync 或事务 API。
- 运行主链：持久化 job 行 → `Job::get_involving_schema_info` → `JobScheduler::schedule_persisted` → `RunningJobs::check_runnable/add_running` → worker step → `finish_or_pend_job` → 轮末 `reset_all_pending`。
- 测试：独立 Rust 测试是 [`pkg/ddl/ddl_running_jobs_test.rs`](ddl_running_jobs_test.rs)，由 [`pkg/ddl/lib.rs`](lib.rs) 以测试模块装配；Go 对照测试是 [`pkg/ddl/ddl_running_jobs_test.go`](ddl_running_jobs_test.go)。

## 错误处理与边界

本模块没有 `Result` 返回值或可恢复 I/O 错误；无效对象描述通过 `assert_valid_info` 触发 panic。该断言要求 database/table 同时为空或同时非空，并要求库表、policy、resource group 恰好选择一种。调用者必须在进入调度前产生合法描述。

空状态快速路径会在逐项断言前直接返回 `true`。因此，若调用者在系统完全空闲时传入非法 `involves`，`check_runnable` 本身不会验证它；随后的 `add_running` 会执行断言。空 `involves` 也会通过 `iter().all`，当前文件没有“作业至少涉及一个对象”的约束。

`add_running` 假定此前已成功调用 `check_runnable`。若绕过协议、用重复 job ID 调用它，`running.insert` 会替换列表，但此前增加的对象计数不会自动释放；正确性依赖“检查后、下一次检查前立即登记”的调用契约。相反，`remove_running` 对未知 ID 是无操作，`Objects::remove` 对不存在的键也是无操作。

`has_schema_conflict` 的 database 查找本身不是对称通配符算法；运行中独占 `*.*` 由 `check_runnable` 的专门分支兜底。新增 shared `*.*` 或改变通配符语义时不能只复用现状，必须增加对称场景测试。`has_conflict` 已具备对称通配符比较，但当前未接入主调度路径。

## 并发与资源生命周期

Rust 实现没有内部 mutex、原子量或后台任务；所有状态修改都要求 `&mut RunningJobs`。当前 `JobScheduler` 自身独占该实例并同步调用 worker step，所以借用规则将“检查/登记/释放”串行化。`finish_or_pend_job` 在同一个可变借用中完成释放和 pending 登记，不会向同一调度器暴露两步之间的空窗。

生命周期以调度轮为界：`exclusive/shared` 随 `add_running` 建立，随 `remove_running` 或 `finish_or_pend_job` 释放；`pending` 只活到 `reset_all_pending`。持久化、owner failover 与进程恢复不由这里负责；持久化路径每轮从 `mysql.tidb_ddl_job` 重载 job，而 `RunningJobs` 是 owner 进程内的派生状态。

如果未来 worker 真正并行并从其他线程直接完成作业，当前类型不能像 Go 版那样在共享引用下并发删除。应优先维持“完成事件回到调度器、由调度器独占更新”的所有权模型；若确需共享修改，再在更高层设计同步边界，不能仅给局部 map 随意加锁。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/ddl/ddl_running_jobs.go`](ddl_running_jobs.go)。Rust 的 `RunningJobs`/`Objects`、`check_runnable`、`has_schema_conflict`、add/remove/pending/reset 流程与 Go 的 `runningJobs`/`objects` 保持同一核心语义，Rust 测试也复刻了 Go 测试中的精确表、`db.*`、`*.*`、policy、resource group、共享/独占和公平性案例。

存在以下实现层差异：

- Go 版含 `sync.RWMutex`，因为注释明确 worker goroutine 可并发调用 `removeRunning`；Rust 版由 `&mut self` 和当前同步 `JobScheduler` 串行驱动，没有内部锁。
- Go 版只保存 ID 集合，删除时由调用者再次传入 `involves`；Rust 版 `running` 保存完整列表，`remove_running(job_id)` 从内部取回列表，降低错配风险。
- Go 的 ID 来自 map，`allIDs` 顺序不稳定且测试需排序；Rust 使用 `BTreeMap`，`all_ids` 天然按数值升序。Go 还用 `sync.Once` 缓存 ID 字符串，Rust 每次现算。
- Go 的内部检查受 `intest.EnableInternalCheck` 影响，并显式处理未知模式；Rust 枚举消除了未知模式分支，但 `assert_valid_info` 是无条件断言且要求恰好一个类别，约束更集中。
- Rust 额外公开 `running_ids`、`invariants_hold` 和 `has_conflict`；其中后两者主要服务可观测性/测试或未来复用，`has_conflict` 当前没有仓库内调用者。
- Rust 的 `finish_or_pend_job` 在 [`pkg/ddl/ddl_running_jobs_test.rs`](ddl_running_jobs_test.rs) 有独立原子转换回归用例；Go 对照测试主要覆盖相同公平性规则，没有同名独立测试函数。

## 扩展指南

新增一种 DDL 对象类别时，需要成组修改 `InvolvingSchemaInfo`、`assert_valid_info`、`Objects` 的存储与 `is_empty/add/remove/conflicts/counts_are_positive`、`object_overlap`，并同步元数据层对象描述及 `JobScheduler::schedule_persisted` 的转换。遗漏任何一处都可能造成“能登记但不能释放”或“检查路径看不到占用”。相应测试必须放在独立的 `pkg/ddl/ddl_running_jobs_test.rs`，不要嵌入生产源文件。

修改兼容矩阵或公平性时，以 `check_runnable` 为主要入口，同时审查 `JobScheduler` 是否仍遵守：成功后立即 `add_running`，失败后在下一次检查前 `add_pending`，轮末总会 `reset_all_pending`。需要补测共享对共享、独占对共享、pending 阻断后到共享，以及 step 失败仍保留 pending 的调度器路径。

扩展通配符时必须同时审查 `has_schema_conflict` 与 `object_overlap`。尤其应明确 database 级 `*` 是否允许 shared 模式，并补左右两侧通配符的对称用例；不能假定主路径的“全局独占快速拒绝”覆盖所有新模式。

若引入并行 worker，不应破坏 `RunningJobs` 的单写者不变量；推荐由 worker 发送完成结果、调度器统一调用 `finish_or_pend_job`。性能上，当前检查复杂度约为“作业涉及对象数 × 至多三份 map 查询”，BTree 查询还带对数因子；只有实测成为瓶颈后才值得替换结构，同时必须保留稳定 ID 输出与引用计数语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/ddl/ddl_running_jobs.rs` 确认目标文件已索引并有 30 个符号；`node --file ...` 阅读了目标文件 1–414 行、`job_scheduler.rs` 100–330 行、Rust 测试 1–277 行、Go 实现 1–366 行和 Go 测试 1–360 行。对 Rust 方法执行 `callers/callees` 未产生边，故使用精确引用搜索验证生产接线。
- 源与模块边界：`pkg/ddl/ddl_running_jobs.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`。
- 直接调用证据：`pkg/ddl/job_scheduler.rs` 中的 `JobScheduler::schedule` 与 `JobScheduler::schedule_persisted`。
- Rust 行为测试：`pkg/ddl/ddl_running_jobs_test.rs` 的 `running_schema_jobs_follow_go_conflict_rules`、`schema_policy_and_resource_group_conflicts_match_go`、`exclusive_shared_and_pending_jobs_are_fair`、`finish_or_pend_updates_counts_atomically`。
- Go 语义对照：`pkg/ddl/ddl_running_jobs.go` 与 `pkg/ddl/ddl_running_jobs_test.go` 的 `TestRunningJobs`、`TestSchemaPolicyAndResourceGroup`、`TestExclusiveShared`。
- 本任务仅新增说明文档，按计划不运行 Cargo 或代码测试；交付验证以固定章节结构、链接/引用存在性、范围检查和人工事实复核为准。
