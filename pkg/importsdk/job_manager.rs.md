# `pkg/importsdk/job_manager.rs`

## 文件定位

本文件属于 `astersql-importsdk` crate（见 `pkg/importsdk/Cargo.toml`），实现异步导入作业的提交、状态查询、取消和按组查询。crate 入口 `pkg/importsdk/lib.rs` 将 `job_manager` 的公开项重新导出；统一门面 `ImportSDK` 在 `NewImportSDK` 中创建 `JobManagerImpl`，并在其 `JobManager` 实现中逐项委托。因此，本文件位于“生成 `IMPORT INTO` SQL”之后、数据库执行 `IMPORT`/`SHOW IMPORT`/`CANCEL IMPORT` 语句之前，是 SDK 与 SQL 数据库连接之间的作业生命周期适配层。

生产调用链的直接证据包括：`lightning/pkg/importinto/job_submitter.rs` 调用 `SDK::SubmitJob`；`job_monitor.rs` 与 `job_orchestrator.rs` 调用 `GetJobsByGroup`；`job_orchestrator.rs` 还调用 `CancelJob`。这些调用通常先经过 `pkg/importsdk/sdk.rs` 中 `ImportSDK` 的委托实现，再进入本文件。

## 核心职责

- 用 `JobManager` trait 定义提交、单作业查询、取消、分组摘要和组内作业列表五类操作，并提供保留 Go 双返回值形状的 `*Parts` 默认适配方法。
- 用 `JobDatabase`、`JobRows` 和 `SQLValue` 隔离具体数据库驱动，使生产门面和独立测试都能提供自己的查询/游标实现。
- 由 `JobManagerImpl` 生成固定格式的 `SHOW IMPORT ...`、`CANCEL IMPORT ...` SQL，校验空组键并转义组键中的单引号。
- 将固定列布局的查询行解码为 `model.rs` 中的 `JobStatus`（21 列）或 `GroupStatus`（9 列），统一处理 NULL、类型错误和时间解析。
- 保证已成功取得的游标在成功、空结果、行读取失败或解码失败后都执行 `Close`；关闭错误不覆盖原查询语义。

## 主要符号

- `TIME_LAYOUT`：`SHOW IMPORT` 时间字符串格式 `%Y-%m-%d %H:%M:%S`，对应 Go 的 `timeLayout`。
- `SQLValue::{Null, Int64, String}`：数据库结果单元格的最小值域。解码器只接受与目标列约定一致的变体。
- `JobRows: Send`：游标抽象，`Next` 每次返回一整行或结束，`Close` 释放底层资源。方法名沿用 Go 风格。
- `JobDatabase: Send + Sync`：数据库抽象；`QueryContext` 产生游标，`ExecContext` 执行取消语句。上下文以 `&(dyn Any + Send + Sync)` 透传，具体实现负责解释其类型和取消语义。
- `JobManager: Send + Sync`：公开作业管理契约。核心方法返回 Rust `Result`；`SubmitJobParts`、`GetJobStatusParts`、`GetGroupSummaryParts`、`GetJobsByGroupParts` 把结果转换为 Go 风格的值与可选错误，其中错误时值为 `0` 或 `None`。
- `JobManagerImpl { db: Arc<dyn JobDatabase> }` 与 `NewJobManager`：默认实现及构造器。`Arc` 允许门面和调用方共享线程安全的数据库实现。
- `queryOne`：单行查询模板。它发起查询、只读取第一行、区分“缺失”和读取错误、调用传入的解码函数，并在返回前关闭游标。
- `scanJobStatus`、`scanGroupStatus`：分别按 21 列和 9 列协议构造 `JobStatus`、`GroupStatus`。
- `requireColumns`、`requiredString`、`nullableString`、`requiredInt`、`nullableInt`：行形状和单元格类型校验。可空字符串映射为空串，可空整数映射为 `0`。
- `parseTime`、`zeroTime`：解析失败或空时间回退到公历 `0001-01-01 00:00:00`，而不是 chrono 自身更早的最小时间，以匹配 Go `time.Time{}`。

## 执行流程

1. `NewImportSDK`（`pkg/importsdk/sdk.rs`）把共享的 `Arc<dyn JobDatabase>` 传给 `NewJobManager`；也可以单独构造 `JobManagerImpl`。
2. `SubmitJob` 原样执行调用方提供的导入 SQL，通过 `queryOne` 和 `scanJobStatus` 解码首行，最终只返回其中的 `JobID`；没有首行时返回 `ErrNoJobIDReturned`。
3. `GetJobStatus` 生成 `SHOW IMPORT JOB {job_id}`，以相同的 21 列布局解码；没有首行时返回 `ErrJobNotFound`。
4. `CancelJob` 生成 `CANCEL IMPORT JOB {job_id}` 并直接调用 `ExecContext`，不读取结果集。
5. `GetGroupSummary` 先拒绝空 `group_key`，再将单引号替换为两个单引号并生成 `SHOW IMPORT GROUP '<key>'`；首行按 9 列解码，无行返回 `ErrJobNotFound`。
6. `GetJobsByGroup` 做相同的空值校验和转义，执行 `SHOW IMPORT JOBS WHERE GROUP_KEY = '<key>'`，循环读取并按顺序解码所有 21 列行。空结果是合法的空 `Vec`，不是“未找到”错误。
7. 单行和多行路径在获得游标后都无条件调用 `Close`。多行路径遇到读取或解码错误立即停止，不返回此前积累的部分结果。

## 数据与状态

`JobManagerImpl` 的唯一持久字段是不可变的 `Arc<dyn JobDatabase>`；文件本身不缓存作业状态，也不启动后台轮询。真实作业状态由数据库中的导入子系统维护，本文件每次调用即时查询。

行协议是这里最重要的不变量。`scanJobStatus` 要求恰好 21 列，顺序与 `JobStatus` 字段一一对应；`scanGroupStatus` 要求恰好 9 列。必填列不接受 NULL 或错误类型；SQL NULL 在可空字段上折叠为 Rust 模型的默认表示（空字符串、`0` 或零时间），因为 `JobStatus`/`GroupStatus` 当前字段不是 `Option`。时间解析不返回错误，非法文本也折叠为零时间。

上下文没有被本文件读取或保存，只按借用传给数据库层。`*Parts` 适配方法同样不增加状态；它们仅改变返回值形状。

## 依赖与调用关系

上游直接关系：

- `pkg/importsdk/lib.rs` 公开重导出本模块。
- `pkg/importsdk/sdk.rs::NewImportSDK` 构造 `JobManagerImpl`；`impl JobManager for ImportSDK` 将全部方法委托给内部管理器。
- `lightning/pkg/importinto/job_submitter.rs` 经 SDK 提交导入 SQL；`job_monitor.rs` 和 `job_orchestrator.rs` 查询同组作业，编排器还能取消作业。
- `pkg/importsdk/mock/sdk_mock.rs` 实现同一 trait，供不需要真实数据库的上层测试替换。

下游直接关系：

- `JobDatabase::QueryContext`/`ExecContext` 是所有 SQL I/O 的边界，`JobRows` 是游标边界。
- `crate::{ErrInvalidOptions, ErrJobNotFound, ErrNoJobIDReturned}` 来自 `pkg/importsdk/error.rs`。
- `JobStatus` 与 `GroupStatus` 来自 `pkg/importsdk/model.rs`。
- `astersql-errors` 提供共享错误类型和动态错误构造；`chrono` 提供无时区的 `NaiveDateTime`。二者均在 `pkg/importsdk/Cargo.toml` 中声明。

RustCodeGraph 将本文件标为被 22 个文件使用，并能定位上述公开符号；由于当前索引的 `callers` 命令超时，具体生产调用点另以精确 `rg` 结果核验。

## 错误处理与边界

- `QueryContext`/`ExecContext`、`Next` 和解码错误均原样以 `errors::SharedError` 传播；本层不重试、不包装额外上下文。
- 单行查询无结果时使用调用点传入的哨兵错误：提交为 `ErrNoJobIDReturned`，作业/分组查询为 `ErrJobNotFound`。
- 两个分组 API 在访问数据库前拒绝空键，返回 `ErrInvalidOptions`。非空键只做 SQL 单引号加倍转义；数据库仍负责其余 SQL 语义。
- `requireColumns` 在索引取值前检查精确列数，避免辅助函数越界，并在错误中报告来源、实际列数和期望列数。
- 类型辅助函数严格区分字符串与整数；可空只意味着接受 `SQLValue::Null`，并不接受另一种非空类型。
- 时间格式错误被有意吞掉并变为 Go 零时间。这是兼容行为，也意味着调用方无法区分“数据库返回 NULL”和“数据库返回非法时间文本”。
- `Close` 错误被有意忽略，以匹配 Go 中 `defer rows.Close()` 未消费关闭错误的行为。若未来数据库实现要求暴露关闭失败，必须先评估 Go 兼容性。

## 并发与资源生命周期

`JobManager` 和 `JobDatabase` 都要求 `Send + Sync`，`JobManagerImpl` 通过 `Arc` 共享数据库对象，因此管理器可由多线程共享。本文件没有内部锁、可变缓存、任务或通道；并发控制、连接池容量、查询取消及超时由 `JobDatabase` 实现和传入上下文承担。

每次查询的资源生命周期为：`QueryContext` 创建 `Box<dyn JobRows>`，读取零到多行，然后在离开方法前显式 `Close`。若创建查询本身失败，则没有游标可关闭；一旦创建成功，即使 `Next` 或解码失败也会关闭。`GetJobsByGroup` 将解码后的状态保存在局部 `Vec` 中，发生中途错误时整个局部结果被丢弃。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/importsdk/job_manager.go`，五个公开操作、SQL 文本、空组键校验、单引号加倍、21/9 列扫描顺序和时间布局保持一致。Rust 用 `JobDatabase`/`JobRows` 替代 Go 的 `*sql.DB`/`*sql.Rows`，用 `SQLValue` 显式表达扫描后的值类型；`Arc<dyn JobDatabase>` 对应可共享的数据库句柄。

Rust `Result<T, SharedError>` 对应 Go 的 `(T, error)`；trait 上的 `*Parts` 方法专门保留双返回值观察方式。Go 返回 `*JobStatus`、`*GroupStatus` 和 `[]*JobStatus`，Rust 返回拥有所有权的值和 `Vec<JobStatus>`。

Rust 额外显式检查结果列数和类型，避免依赖驱动的 `rows.Scan` 报错；时间回退通过 `zeroTime` 精确采用公历一年，弥合 chrono 最小值与 Go 零值的差异。一个实现层面的差异是 Rust 的 `JobRows::Next` 直接返回行或读取错误，因此没有 Go 独立的 `rows.Err()` 收尾步骤；错误由每次 `Next` 直接传播。

对应测试为 `pkg/importsdk/job_manager_test.go` 与独立 Rust 文件 `pkg/importsdk/job_manager_test.rs`。Rust 测试用 `CanonicalDatabase`/`CanonicalRows` 替代 `sqlmock`，逐项保留成功、空结果和错误分支，并增加 SQL 文本、组键引号转义和零时间断言。

## 扩展指南

- 新增作业操作时，先扩展 `JobManager` 契约，再在 `JobManagerImpl` 和 `pkg/importsdk/sdk.rs` 的委托实现中接线；同步更新 `pkg/importsdk/mock/sdk_mock.rs`，并将测试放在独立的 `pkg/importsdk/job_manager_test.rs`，不要内嵌进生产文件。
- 若服务器改变 `SHOW IMPORT` 列布局，必须同步修改 `scanJobStatus`/`scanGroupStatus`、`model.rs` 的状态模型以及 Go/Rust 两侧测试行。列顺序、NULL 约定和类型都属于兼容协议，不能只调整期望列数。
- 新增单行查询可复用 `queryOne`，但要明确“无行”所对应的哨兵错误；多行查询应保持全有或全无的返回语义，并保证所有退出分支关闭游标。
- 拼接新的字符串条件时至少采用与组键相同的 SQL 字面量转义；若条件变复杂，优先在 `JobDatabase` 边界引入参数绑定能力，而不是继续手写不完整的转义。
- 若希望区分 NULL、非法时间和真实零值，应先把模型字段改为可表达这些状态的类型，并同步 Go 兼容契约和所有上层消费者；这不是局部解码器改动。
- 性能风险主要在高频轮询和大组结果：当前每次调用都访问数据库，`GetJobsByGroup` 会一次性收集全部行。引入分页、流式处理或缓存时需同时考虑调用者的顺序/完整性假设和并发失效策略。

## 验证依据

- RustCodeGraph：`status` 显示索引含 `pkg/importsdk/job_manager.rs`；`files --filter pkg/importsdk` 确认同目录 Rust/Go/测试文件；`node --file pkg/importsdk/job_manager.rs` 阅读全部 376 行；`query JobManager`、`query NewJobManager`、`query SubmitJob` 核对主要符号。`callers` 查询发生超时，调用边以源码和 `rg` 交叉验证。
- 生产源码：`pkg/importsdk/job_manager.rs`、`pkg/importsdk/lib.rs`、`pkg/importsdk/sdk.rs`、`pkg/importsdk/model.rs`、`pkg/importsdk/error.rs`。
- crate 边界：`pkg/importsdk/Cargo.toml`，确认 crate 名、入口、移植元数据及 `astersql-errors`、`chrono` 等依赖。
- Go 对照：`pkg/importsdk/job_manager.go`；Go 独立测试：`pkg/importsdk/job_manager_test.go`。
- Rust 独立测试：`pkg/importsdk/job_manager_test.rs`，覆盖五类操作、底层错误、空结果、SQL 生成、转义和零时间。
- 上游调用搜索：`pkg/importsdk/sdk.rs`、`lightning/pkg/importinto/job_submitter.rs`、`lightning/pkg/importinto/job_monitor.rs`、`lightning/pkg/importinto/job_orchestrator.rs`。
- 本任务是纯文档分析，按任务约束不运行 Cargo；交付只执行标题数量和文件存在性的结构验证，并人工复核本页能回答文件定位、运行流程和安全扩展方式。
