# `pkg/importsdk/model.rs`

## 文件定位

`model.rs` 是 `astersql-importsdk` crate 的纯数据模型层。私有模块 `model` 在 [`pkg/importsdk/lib.rs`](lib.rs) 中声明，并通过 `pub use model::*` 把本文件的公开类型重新导出为 crate API。它位于“扫描导入源文件 → 生成 `IMPORT INTO` SQL → 提交并观察导入作业”的共享边界：`file_scanner.rs` 产生表与大小元数据，`sql_generator.rs` 读取表元数据和选项，`job_manager.rs` 从 SQL 查询结果构造作业/组状态。

本文件只有六个公开结构体和 `JobStatus` 的四个状态判断方法；没有 I/O、SQL 执行、全局变量、条件编译项、trait 或异步任务。crate 边界由 [`pkg/importsdk/Cargo.toml`](Cargo.toml) 定义；本文件直接使用 crate 内的 `CSVConfig`、`astersql-lightning-mydump` 的文件格式枚举和 `chrono` 的无时区日期时间。

## 核心职责

1. 用 `TableMeta`/`DataFileMeta` 表达文件扫描器发现的一张逻辑表及其物理数据文件，保留生成 SQL 所需的库表名和通配路径。
2. 用 `TableDataSizeEstimate`/`ImportDataSizeEstimate` 承载逐表及整次导入的源文件大小、预计 TiKV 编码数据量。
3. 用 `ImportOptions` 作为 SQL 生成器的强类型输入，集中表达通用、CSV、云存储、作业组和执行控制选项。
4. 用 `GroupStatus`/`JobStatus` 镜像 `SHOW IMPORT GROUP` 与 `SHOW IMPORT JOB` 的结果形状，并用严格字符串匹配派生作业终结状态。

这些结构只定义数据契约，不自行校验字段间一致性，也不负责把数据库行或 mydump 元数据转换成模型；转换和错误处理分别在 `file_scanner.rs`、`job_manager.rs`、`sql_generator.rs` 中完成。

## 主要符号

- `TableMeta`：一张待导入表的聚合描述。`Database`、`Table` 标识目标；`DataFiles` 保存每个文件；`TotalSize` 是文件大小合计；`WildcardPath` 是提交给 `IMPORT INTO ... FROM` 的路径；`SchemaFile` 指向建表 DDL。派生 `Clone + Debug + Default`，便于扫描器逐步补字段、提交器保存快照。
- `DataFileMeta`：单个文件的 `Path`、字节数 `Size`、`mydump::SourceType` 格式和 `mydump::Compression` 压缩类型。格式被 Lightning `DefaultJobSubmitter::buildImportOptions` 转成 `ImportOptions.Format`。
- `TableDataSizeEstimate`：以 `Database`/`Table` 标识逐表估算，`SourceSize` 是源数据量，`TiKVSize` 对应 Go 模型中单副本编码 KV 大小的估计值。
- `ImportDataSizeEstimate`：`Tables` 保存逐表结果，`TotalSourceSize` 与 `TotalTiKVSize` 保存其汇总；`Default` 提供空列表和零累计值。
- `ImportOptions`：SQL 生成输入。`Format` 决定源格式；`CSVConfig: Option<Box<CSVConfig>>` 表示可缺省的 CSV 专属配置；`Thread`、`DiskQuota`、`MaxWriteSpeed`、`SplitFile`、`RecordErrors` 控制执行；`Detached`、`GroupKey` 控制作业提交与归组；其余字段覆盖云存储、跳行、字符集、校验和、TiKV import mode、预检查及资源查询参数。
- `GroupStatus`：组键、作业总数、五类状态计数和首个创建/最后更新时间。它只可 `Clone + Debug`，没有在本文件提供默认值或聚合算法。
- `JobStatus`：`SHOW IMPORT JOB` 的 21 列模型，包含身份、来源与目标、阶段/状态、行数与结果消息、五个时间点/主体字段，以及处理量、百分比、速度和 ETA 等进度文本。
- `JobStatus::{IsFinished, IsFailed, IsCancelled}`：分别将 `Status` 与小写字面量 `"finished"`、`"failed"`、`"cancelled"` 做区分大小写的精确比较。
- `JobStatus::IsCompleted`：调用前三个方法并取逻辑或；仅上述三种状态被视为终结态。

所有字段沿用 Go 导出字段的首字母大写命名；`lib.rs` 在 crate 级允许相应的非 Rust 惯用命名，以保持移植 API 形状。

## 执行流程

1. `fileScanner::buildTableMeta` 遍历 mydump 表的 `data_files`，通过 `createDataFileMeta` 建立 `DataFileMeta`，按扫描配置修正 `Size` 并累计 `TotalSize`；存在数据文件时再生成 `WildcardPath`，没有文件时返回路径为空的模型。
2. `fileScanner::EstimateImportDataSize` 从空的 `ImportDataSizeEstimate::default()` 开始，为每张有效表创建 `TableDataSizeEstimate`，随后同时累加两个总量并追加到 `Tables`。是否跳过估算失败的表由扫描配置和 Aurora 来源分支决定，不由模型决定。
3. Lightning 的 `DefaultJobSubmitter::buildImportOptions` 从配置和 `TableMeta.DataFiles.first()` 构造 `ImportOptions`；`sqlGenerator::GenerateImportSQL` 读取 `TableMeta.Database/Table/WildcardPath` 与这些选项，生成最终 SQL。只有 `Format == "csv"` 时才展开 `CSVConfig`。
4. `JobManagerImpl` 执行 `SHOW IMPORT JOB`/`SHOW IMPORT GROUP`；`scanJobStatus` 严格要求 21 列并构造 `JobStatus`，`scanGroupStatus` 严格要求 9 列并构造 `GroupStatus`。`ImportSDK` 门面原样返回这些模型。
5. Lightning 监控器遍历 `JobStatus`：按三个具体状态方法计数，使用 `IsCompleted` 去重完成事件、记录 checkpoint 并产生失败/取消错误。取消编排器也用 `IsCompleted` 跳过已终结作业，从而避免重复取消。

## 数据与状态

- 所有数据的所有权都在值内：字符串和向量为拥有型，`CSVConfig` 由 `Box` 独占；结构体的 `Clone` 会深拷贝这些拥有型字段。模型不借用扫描器、数据库游标或配置对象。
- 大小字段使用 `i64`，与 Go 的 `int64` 对齐。`SourceFileSize`、`ProcessedSize`、`TotalSize`、`Percent`、`Speed` 和 `ETA` 刻意保留为展示字符串，模型不解析单位或百分比。
- `ImportOptions.Thread`/`SkipRows` 使用 `isize` 对应 Go 的 `int`；其宽度随编译目标变化。生成器仅在数值大于零时输出这些选项，负值和零在模型层仍可表示。
- `JobStatus` 与 `GroupStatus` 的时间使用 `NaiveDateTime`，不携带时区。`job_manager.rs::parseTime` 以固定布局解析，空值或解析失败映射到公元 1 年的 Go 零时间等价值；模型本身无法区分“数据库 NULL”“空串”与“解析失败”。
- `Default` 只提供机械零值，不代表业务上可提交：例如空 `WildcardPath`、空 `Format`、空 `GroupKey` 都可能是合法的中间状态或需由调用层拒绝的输入。
- 终结态是不变量式分类而非可变状态机：本文件不修改 `Status`，只是根据调用者提供的字符串计算布尔值。`"FINISHED"`、前后带空白或未知状态均不是终结态。

## 依赖与调用关系

- 上游构造者：`pkg/importsdk/file_scanner.rs::{buildTableMeta, createDataFileMeta, EstimateImportDataSize}` 构造四类扫描/估算模型；`pkg/importsdk/job_manager.rs::{scanJobStatus, scanGroupStatus}` 构造两类状态模型；`lightning/pkg/importinto/job_submitter.rs::buildImportOptions` 构造 `ImportOptions`。
- crate 内消费者：`pkg/importsdk/sql_generator.rs::GenerateImportSQL/buildOptions` 读取 `TableMeta` 与 `ImportOptions`；`pkg/importsdk/sdk.rs` 的门面 trait 实现转交并返回 `ImportDataSizeEstimate`、`JobStatus`、`GroupStatus` 和 `Vec<JobStatus>`；`pkg/importsdk/mock/sdk_mock.rs` 用这些类型定义模拟调用契约。
- crate 外主要消费者：`lightning/pkg/importinto/job_submitter.rs` 保存 `TableMeta` 并生成选项；`job_monitor.rs::processJobStatuses` 消费 `JobStatus` 的进度字段和终结态方法；`job_orchestrator.rs::cancelJobsInGroup/updateCheckpointsAfterCancel` 用状态方法决定取消与 checkpoint 更新。
- 直接外部类型依赖：`CSVConfig` 经 `crate` 根重导出，实际来自本 crate 的 `config.rs`；`SourceType`/`Compression` 来自 Cargo 路径依赖 `astersql-lightning-mydump`；`NaiveDateTime` 来自 `chrono = "0.4"`。
- RustCodeGraph 对 `model.rs` 报告 12 个符号并显示该文件被 8 个文件使用；精确查询确认 `scanJobStatus` 实例化 `JobStatus`，`IsCompleted` 调用三个具体状态判断，而 Lightning 的监控与取消路径调用这些方法。

## 错误处理与边界

本文件的公开方法均返回 `bool`，不会产生 `Result`、panic 或日志。真正的输入边界在相邻模块：文件扫描可能因 schema、路径或大小估算失败而返回错误；SQL 生成可能因 CSV 空值配置不合法而失败；作业管理会拒绝列数/类型不匹配的查询结果。模型的公开字段允许调用者绕过这些构造路径，因此新增消费者不能假设任意手工构造值已经验证。

状态判断严格且封闭：只有三个约定的小写字面量属于终结态；`running`、`pending`、`unknown` 以及未来新增状态默认返回 false。这一默认行为既避免把未知状态误判为完成，也意味着服务端若新增终结态，Rust 模型、Go 模型、监控/取消逻辑与测试必须同步更新。

`JobStatus` 的可空数据库列在 `job_manager.rs` 中被折叠为空字符串、零或零时间，因此消费层不能借助这些字段区分 NULL。大小汇总使用普通 `i64` 加法，没有在模型或扫描代码中显式做溢出检查；异常巨大输入是需在上游约束的兼容与正确性风险。

## 并发与资源生命周期

模型本身没有锁、原子量、通道、线程、future、文件句柄或数据库连接，也没有 `Drop` 实现。拥有型字段让各结构可独立移动；派生的 `Clone` 支持提交器、监控器和 mock 保存独立快照。它们没有派生 `Copy`，克隆大型 `DataFiles` 或字符串会分配并复制，热路径新增调用应优先借用。

本文件也未显式声明 `Send`/`Sync`；其字段均由标准拥有型数据、`chrono::NaiveDateTime`、`CSVConfig` 和 mydump 枚举组成，能否跨线程由这些成员的自动 trait 共同决定。数据库游标始终由 `job_manager.rs` 管理并关闭，对象存储由 `file_scanner.rs::Close` 管理，二者生命周期均不进入这些模型。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/importsdk/model.go`](model.go)。六个结构体、字段顺序/语义和四个 `JobStatus` 方法逐项移植；Rust 额外派生 `Clone`、`Debug`，并为前四种可自然零值化的结构派生 `Default`。

主要类型映射如下：Go `string` → Rust `String`，`[]DataFileMeta`/`[]TableDataSizeEstimate` → `Vec<_>`，`int64` → `i64`，`int` → `isize`，`*config.CSVConfig` → `Option<Box<CSVConfig>>`，`time.Time` → `chrono::NaiveDateTime`。Go 方法使用 `*JobStatus` 接收者，Rust 使用不可变 `&self`；两者均不改变对象。Rust 的 `IsCompleted` 与 Go 一样复用 `IsFinished`、`IsFailed`、`IsCancelled`，不是重新复制字符串条件。

语义差异集中在类型表达：`NaiveDateTime` 不含 Go `time.Time` 可携带的位置/单调时间信息；可空 CSV 配置显式表示为 `Option`；Rust 的默认构造和克隆能力是移植层便利接口。`model_test.rs::job_completion_states_match_go_table` 完整镜像 Go `model_test.go::TestJobStatus` 的 finished、failed、cancelled、running、pending、unknown 六行真值表。

## 扩展指南

- 新增或重命名模型字段时，先确认对应 `model.go` 差异；同步更新实际构造者和消费者，而不是只改结构体。`JobStatus` 字段变化必须同步 `job_manager.rs::scanJobStatus` 的列数、索引和类型读取，`GroupStatus` 同理更新 `scanGroupStatus`。
- 新增 `ImportOptions` 字段时，在 `sql_generator.rs::buildOptions`（必要时 `buildCSVOptions`）定义输出条件和转义规则，并检查 `lightning/pkg/importinto/job_submitter.rs::buildImportOptions`、mock 参数签名及 `sql_generator_test.rs`。不要在模型层隐式纠正非法配置。
- 扩展文件格式或压缩类型时保持 `DataFileMeta` 使用 mydump 的权威枚举，并同步 `file_scanner.rs::createDataFileMeta`、格式映射、提交选项和独立测试。
- 新增终结状态时必须同时修改 Go/Rust 的状态判断、`model_test.go` 与 `model_test.rs` 真值表，并审查 `job_monitor.rs` 的分类/报错和 `job_orchestrator.rs` 的取消/checkpoint 分支；否则新状态会被当作 pending 或再次取消。
- 新增大量字段或在高频路径传递模型时评估 `Clone` 成本；优先传 `&TableMeta`/`&JobStatus`，只有需要独立生命周期时才克隆。
- 测试逻辑继续放在独立的 `pkg/importsdk/model_test.rs`，不要内嵌回生产源文件；Go 对照测试保持在 `model_test.go`。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/importsdk` 确认 Rust/Go 源和独立测试；`node --file pkg/importsdk/model.rs` 核对本文件 176 行、12 个符号及直接使用关系。
- RustCodeGraph 精确查询/调用证据：`query TableMeta/DataFileMeta/ImportOptions/JobStatus/GroupStatus/ImportDataSizeEstimate/TableDataSizeEstimate`；`query IsCompleted`；`callees JobStatus::IsCompleted` 显示 Rust `IsCompleted` 调用三个具体判断；聚焦 `explore` 显示 `scanJobStatus` 构造状态以及 Lightning 监控、提交、取消路径的消费者。
- 已读 Rust 路径：`pkg/importsdk/model.rs`、`lib.rs`、`file_scanner.rs`、`job_manager.rs`、`sql_generator.rs`、`sdk.rs`、`model_test.rs`，以及直接业务消费者 `lightning/pkg/importinto/job_submitter.rs`、`job_monitor.rs`、`job_orchestrator.rs`。
- 已读配置/Go 路径：`pkg/importsdk/Cargo.toml`、`pkg/importsdk/model.go`、`pkg/importsdk/model_test.go`。目标包不存在 `doc.go`；crate 契约由 `lib.rs` 与 Cargo metadata 的 `go-package = "pkg/importsdk"` 共同给出。
- 结构验证使用任务指定命令，要求本文档存在且恰好出现十一个固定二级标题；本任务是纯文档分析，按计划不运行 Cargo。
