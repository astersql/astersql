# `pkg/executor/importer/job.rs`

## 文件定位

`job.rs` 属于 `astersql-executor-importer` crate；crate 根 `pkg/executor/importer/lib.rs` 以 `mod job; pub use job::*;` 将本文件的公开项暴露给导入执行器和 DXF 导入调度器。它是 `mysql.tidb_import_jobs` 系统表的 Rust 持久化边界：将导入任务的创建、查询、状态迁移、权限过滤和 JSON 字段编解码转换为参数化 SQL，而不负责文件扫描、KV 编码或子任务执行。

crate 的 `[lib]` 入口是 `lib.rs`，`Cargo.toml` 的 `package.metadata.porting.go-package` 指向 Go 包 `pkg/executor/importer`。本文件直接使用同 crate 的 `ImportParameters`、`Summary` 和 `astersql-types` 的 `Time`；实际 SQL 会话及 JSON codec 由上层注入，因此核心逻辑不绑定某一种 session 实现。

## 核心职责

1. 定义导入任务的状态与步骤字符串，以及查询完整任务行的 `baseQuerySQL`。状态主线是 `pending -> running -> finished`，并允许 `pending/running -> failed`、`pending/running -> cancelled`；准备型任务的步骤可经过 `preparing`，全局排序路径还会经过 `global-sorting` 和 `resolving-conflicts`。
2. 以 `JobInfo` 表示系统表的一行，并提供 `CanCancel`、`IsCancelled`、`IsSuccess`、`IsSourceFileSizeUnknown` 四个只读判断。
3. 通过 `ImportJobExecutor`、`ImportJobRow`、`ImportJobCodec` 三个 trait 隔离 SQL 后端、行读取和 Go 兼容 JSON 格式；`JobValue` 保留绑定参数的 NULL、整数、字符串和字节类型。
4. 实现任务创建、单项/列表查询、活跃任务计数、准备信息回写、步骤切换以及终态更新；SQL 的 `WHERE status ...` 条件承担并发状态守卫。
5. 为 `ImportParameters` 实现确定性的 JSON 文本输出，处理 JSON 转义、`omitempty` 字段和排序后的 options 键。

以上职责可由 `JobInfo`、`CreateJob`、`GetJob`、`StartJob`、`Job2Step`、`UpdateJobPreparedInfo`、`FinishJob`、`FailJob`、`CancelJob` 与 `impl fmt::Display for ImportParameters` 直接复核。

## 主要符号

- `jobStatusPending`、`JobStatusRunning`、`jogStatusCancelled`、`jobStatusFailed`、`JobStatusFinished`：持久化状态字面量。`jogStatusCancelled` 的历史拼写只影响 Rust 标识符，落库值仍是 `cancelled`。
- `jobStepNone`、`JobStepPreparing`、`JobStepGlobalSorting`、`JobStepImporting`、`JobStepResolvingConflicts`、`JobStepValidating`：调度阶段到系统表 `step` 列的稳定映射。
- `baseQuerySQL`：固定选择 16 列；其列顺序与 `convert2JobInfo` 的索引读取严格耦合。
- `JobValue`：SQL 参数联合类型。`From<i64>`、`From<&str>`、`From<String>`、`From<Vec<u8>>` 让调用处保留原始绑定类型；`Null`、`UInt` 供适配器和扩展调用使用。
- `ImportJobRow`：按列读取 `i64`、字符串、`Time` 及 NULL 状态；读取失败统一为 `String` 错误。
- `ImportJobExecutor`：`ExecuteInternal` 执行写操作，`QueryInternal` 返回抽象行并接收预期列数。生产实现位于 `pkg/dxf/importinto/scheduler.rs` 的 `ImportJobSqlSession` 和 `ImportJobStorageSession`。
- `ImportJobCodec`：参数与摘要的双向编码接口。生产 codec 是 `pkg/dxf/importinto/scheduler.rs` 的 `ImportJobJsonCodec`。
- `JobInfo`：完整任务快照；`Summary` 使用 `Option<Box<Summary>>` 表示 NULL/空摘要，三个可空时间映射为 `Time::default()`。
- `GetJob`、`GetActiveJobCnt`、`GetJobsByGroupKey`、`GetAllViewableJobs`：读取 API。`GetJob` 还实施 owner/SUPER 可见性检查。
- `CreateJob`：编码参数、插入 pending 任务、读取同连接的 `LAST_INSERT_ID()`，并以 `SeqCst` 写入测试观测量 `TestLastImportJobID`。
- `StartJob`、`Job2Step`、`UpdateJobPreparedInfo`、`FinishJob`、`FailJob`、`CancelJob`、`CancelPendingJob`：带状态条件的生命周期写 API；`cancelJobInState` 是两种取消入口共享的内部实现。
- `convert2JobInfo`、`getJobInfoFromSQL`：分别负责单行解码和批量映射。
- `write_json_string` 与 `Display<ImportParameters>`：输出兼容 Go JSON 字段名与省略规则的文本表示。

## 执行流程

典型生产链如下：

1. 上层先创建任务；`CreateJob` 调用 `EncodeParameters`，将库、表、表 ID、分组、创建者、参数快照、源大小和初始 `pending/""` 插入系统表，再查询 `LAST_INSERT_ID()`。
2. DXF 调度器 `prepareImportTask` 先调用 `checkImportJobNotCancelled`，后者借助 `GetJob(..., has_super_privilege=true)` 排除已取消任务；准备模式随后以 `StartJob(..., preparing)` 原子地把 pending 任务置为 running。
3. 文件发现和资源计算成功后，`UpdateJobPreparedInfo` 只读取 running 行。无匹配行时保持历史 no-op；有行时解码原参数，只在非空 `format` 输入下改写格式，再回写源文件大小和完整参数 JSON。
4. `nextImportSubtasksBatch` 根据 DXF 下一步调用 `StartJob` 或 `Job2Step`：本地/全局排序进入 `importing` 或 `global-sorting`，写入阶段进入 `importing`，冲突路径进入 `resolving-conflicts`，后处理进入 `validating`。
5. `doneImportTask` 根据最终任务状态调用 `CancelJob`、`FailJob` 或 `FinishJob`。完成会清空 step 并写摘要；失败保留当前 step、写错误与摘要；取消写固定错误 `cancelled by user`，但不写 start/end time。
6. 若取消时 DXF task 尚不存在，`pkg/executor/import_into_storage.rs::cancelDanglingImportJob` 使用 `CancelPendingJob`，仅允许 pending 行被取消，并用 affected rows 检测并发状态变化。

列表查询走 `getJobInfoFromSQL -> QueryInternal -> convert2JobInfo`。`GetJobsByGroupKey` 同时组合创建者权限和 group key 条件；空 group key 参数表示查询所有非空分组。`GetAllViewableJobs` 对 SUPER 返回全表，否则增加 `created_by` 条件。

## 数据与状态

`JobInfo` 中持久化数据可分为四组：身份字段（`ID`、表 schema/name/ID、`CreatedBy`、`GroupKey`）、时间字段（创建/开始/更新/结束）、执行配置与规模（`Parameters`、`SourceFileSize`）、生命周期结果（`Status`、`Step`、`Summary`、`ErrorMessage`）。`baseQuerySQL` 的 16 列顺序就是 `convert2JobInfo` 的反序列化协议；新增或调整列时必须同步两处及行适配器测试。

状态更新依赖 SQL 谓词而非先读后写：`StartJob` 只匹配 pending，`Job2Step`、`UpdateJobPreparedInfo`、`FinishJob` 只匹配 running，`FailJob` 匹配 pending/running，`CancelJob` 匹配 pending/running，而 `CancelPendingJob` 只匹配 pending。这使重复完成、重复取消或过期调度回调成为零行更新，而不是覆盖终态。

`IsSourceFileSizeUnknown` 只有在 `SourceFileSize <= 0` 且任务为 pending，或任务为 running/preparing 时返回 true；一旦大小为正，或进入其他步骤，即视为已知。`Summary` 的数据库 NULL 或空字符串映射为 `None`，而终态写入在调用者未提供摘要时使用 `{}`；生产 codec 会把 `{}` 解成零值 `Summary`，因此“从未写摘要”和“写入空摘要”在读取模型中可能分别表现为 `None` 与 `Some(零值)`。

## 依赖与调用关系

上游生产调用集中在 `pkg/dxf/importinto/scheduler.rs`：

- `checkImportJobNotCancelled -> GetJob`；
- `prepareImportTask -> StartJob -> UpdateJobPreparedInfo`；
- `nextImportSubtasksBatch -> StartJob/Job2Step`；
- `doneImportTask -> CancelJob/FailJob/FinishJob`。

`pkg/executor/import_into_storage.rs::cancelDanglingImportJob -> CancelPendingJob` 是另一条生产边界。Rust 全仓精确引用检查未发现 `GetJobsByGroupKey` 和 `GetAllViewableJobs` 的生产调用者；目前它们由 `job_test.rs` 验证，属于已导出的查询能力，不能把 Go 侧使用场景误写成 Rust 已接线事实。

下游依赖是 trait 注入的：所有数据库交互进入 `ImportJobExecutor::{ExecuteInternal, QueryInternal}`，所有行字段进入 `ImportJobRow`，JSON 字段进入 `ImportJobCodec`。DXF 中 `ImportJobSqlSession`/`ImportJobStorageSession` 将 `JobValue` 转换到各自 SQL 层值类型、检查查询列数并包装行；`ImportJobJsonCodec` 用 `serde_json` 编解码参数和各步骤摘要。

`Cargo.toml` 证明本 crate 直接依赖 `astersql-types`；大量其余依赖属于整个 importer crate，而不能据此推断本文件直接调用它们。`lib.rs` 的公开再导出使调度器以 `astersql_executor_importer`/`importer` 路径访问本文件 API。

## 错误处理与边界

- 所有 executor、行访问和 codec 错误都以 `Result<_, String>` 原样或经 `to_string()` 向上传播；本文件不重试。DXF 的 `withImportJobSessionRetry` 在外层实现 3/6/12/24/30 秒退避。
- `GetJob` 要求结果恰好一行；零行或多行统一报 `import job <id> not found`。非 SUPER 且 owner 不匹配时返回固定 SUPER 权限错误。
- `CreateJob` 在参数编码、INSERT、`LAST_INSERT_ID()` 查询、ID 读取任一阶段失败即终止；ID 查询结果不是一行时显式报长度异常。INSERT 与取 ID 依赖同一可变 executor/连接语义，但本函数本身不创建事务。
- `GetActiveJobCnt` 对空结果返回明确错误；第一列类型错误继续由 `ImportJobRow::Int64` 传播。
- `UpdateJobPreparedInfo` 对“不存在 running 行”静默成功；参数内容非空但不可解码时失败，且不执行后续 UPDATE。
- 生命周期 UPDATE 不检查 affected rows，所以状态不匹配按设计是成功 no-op。需要确认写入确实发生的调用者必须像 `cancelDanglingImportJob` 一样在 session 层检查 affected rows。
- `cancelJobInState` 由固定非空状态切片调用；若将来直接以空切片调用会生成空 `IN ()`，因此新增入口应维持“至少一个允许状态”的不变量。
- `write_json_string` 转义引号、反斜线、常用控制符和 U+0000..U+001F；其余 Unicode 直接输出。`Display` 无法报告 codec 错误，因此只适合诊断/展示，不替代持久化所用 `ImportJobCodec`。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或后台资源。数据库并发正确性主要来自单条 UPDATE 的状态谓词：竞争者只能更新仍处于允许前态的行，后到调用得到零行成功。它不提供 compare-and-return、显式事务或行锁，调用者若依赖“必须更新一行”需额外检查 affected rows。

`CreateJob` 的 `TestLastImportJobID: AtomicI64` 使用 `Ordering::SeqCst`，只记录最近一次成功读取的 ID；并发创建时它是最后写入者获胜的测试观测点，不是业务 ID 分配器，也不能关联特定请求。Rust 当前每次成功创建都会更新它，而 Go 版本只在 `setLastImportJobID` failpoint 启用时更新。

资源所有权由上层适配器管理：`ImportJobSqlSession` 借用 SQL executor，`ImportJobStorageSession` 持有 storage executor；`withImportJobSession`/`withImportJobSessionRetry` 负责获取和归还 session。本文件只在调用栈内持有返回行的 `Box<dyn ImportJobRow>`，没有需要显式关闭的 record set；这与 Go 函数中的 `defer rs.Close` 是接口形态差异。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/importer/job.go`，回归测试是 `pkg/executor/importer/job_test.go`；Rust 独立测试位于 `pkg/executor/importer/job_test.rs`，没有把测试内嵌进生产文件。两版的状态/步骤字面量、16 列查询、生命周期 SQL 谓词、准备信息 no-op、owner/SUPER 过滤、取消固定错误、空摘要 `{}` 以及空 group key 表示非空分组等核心语义一致。

主要实现差异如下：

- Go API接收 `context.Context` 和 `sqlexec.SQLExecutor`，并标记 `kv.InternalImportInto`、关闭 record set；Rust 将这些职责移到 `ImportJobExecutor` 的生产适配器和 DXF session 生命周期中。
- Go 使用 TiDB 结构化错误（job not found、权限拒绝），Rust 当前公开 `String` 错误；上层若按文本分类，修改文案会构成兼容风险。
- Go 的 `ImportParameters.Options` 是 `map[string]any`；当前 Rust 类型由 `ImportParameters` 定义为字符串值映射，生产 codec 会把非字符串 JSON option 转成 JSON 文本。因此复杂 option 的精确类型往返能力不能仅凭本文件宣称完全等价。
- Go 的测试 ID 仅由 failpoint 写入；Rust `CreateJob` 无条件写 `TestLastImportJobID`。该差异只应作为测试观测差异使用，不能成为业务逻辑依赖。
- Go 真实 session 测试覆盖 affected rows、时间列和系统表行为；Rust `job_test.rs` 以 mock executor 精确验证 SQL 参数、状态守卫、NULL 解码、错误传播与 JSON 输出，另有 DXF/executor 集成测试覆盖生产接线。

## 扩展指南

- 新增状态或迁移时，先修改状态常量和对应 UPDATE 谓词，再同步 `JobInfo` 判断、DXF 调度映射、Go 对照实现及独立 `job_test.rs`。不要只添加字符串而绕开数据库前态守卫。
- 新增步骤时，修改步骤常量及 `pkg/dxf/importinto/scheduler.rs::nextImportSubtasksBatch` 的映射，并测试本地排序、全局排序、冲突处理和准备模式的不同路径。
- 新增系统表列时，必须同步 `baseQuerySQL`、`convert2JobInfo` 的列索引、`JobInfo`、两个生产 `ImportJobRow` 适配器的列数预期、Rust mock 行和 Go 版本；列顺序漂移会产生静默错读风险。
- 调整参数/摘要 JSON 时，优先扩展 `ImportJobCodec` 的生产实现与 codec 测试；`Display<ImportParameters>` 只承担文本格式化，也要同步 Go 字段名、`omitempty` 和转义规则。
- 新增查询入口要明确 owner/SUPER 过滤策略，避免绕过 `GetJob`/列表函数已有权限边界。新增写入口若必须知道是否成功，应扩展 executor 返回 affected rows，而不是把“SQL 执行成功”误当成“状态已迁移”。
- 所有回归测试继续放在独立文件 `pkg/executor/importer/job_test.rs`；涉及真实 DXF 生命周期时同步 `pkg/dxf/importinto/scheduler_test.rs` 或 `scheduler_testkit_test.rs`，涉及取消 fallback 时同步 `pkg/executor/import_into_test.rs`。
- 兼容风险集中在持久化字面量、错误文本、JSON 模式和列顺序；性能风险主要是列表查询无分页以及每行独立 JSON 解码。扩展列表 API 时应考虑排序、分页和过滤索引，但不能在没有实际需求与基准证据时擅自改变语义。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/executor/importer/job.rs` 确认目标文件已索引；`node --file pkg/executor/importer/job.rs --offset 1/501` 读取完整 617 行；`query` 核对 `GetJob`、`GetActiveJobCnt`、`CreateJob`、`StartJob`、`Job2Step`、`UpdateJobPreparedInfo`、`FinishJob`、`FailJob`、两种列表查询、两种取消入口及 `convert2JobInfo`。
- 调用边：RustCodeGraph `explore` 定位 `Job2Step` 到 `pkg/dxf/importinto/scheduler.rs` 与 `job_test.rs`；精确 `callers/callees` 对目标 Rust symbol 无输出/超时后，按技能回退用精确 `rg` 引用检查，确认 scheduler 的生产调用和 `import_into_storage.rs` 的 pending 取消调用，并确认两个列表 API 暂无 Rust 生产调用者。
- 已读 Rust 路径：`pkg/executor/importer/job.rs`、`lib.rs`、`job_test.rs`，以及直接生产证据 `pkg/dxf/importinto/scheduler.rs`、`pkg/executor/import_into_storage.rs`。
- 已读边界与对照：`pkg/executor/importer/Cargo.toml`、`pkg/executor/importer/job.go`、`pkg/executor/importer/job_test.go`。
- `job_test.rs` 覆盖状态判断、创建参数顺序和 ID、生命周期守卫、NULL/权限/未找到处理、准备信息 no-op、列表过滤、executor 错误、JSON 格式与 pending-only 取消；Go 测试补充真实系统表的时间、affected rows、幂等终态和活跃计数证据。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查本文档存在且恰含规定的 11 个二级标题，并人工复核没有把未接线或接口差异描述为已支持事实。
