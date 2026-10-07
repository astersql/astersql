# `lightning/pkg/importinto/checkpoint.rs`

源文件：[checkpoint.rs](checkpoint.rs)

## 文件定位

本文件是 `astersql-lightning-pkg-importinto` crate 的表级导入检查点实现。crate 入口 `lightning/pkg/importinto/lib.rs` 以 `mod checkpoint` 装入并公开重导出本文件；`lightning/pkg/importinto/Cargo.toml` 将该 crate 标为 Go 包 `lightning/pkg/importinto` 的 Rust library 移植，未声明 feature 开关。

它位于 IMPORT INTO 后端的恢复链路中心：`importer.rs::NewImporter` 在没有注入自定义管理器时调用 `NewCheckpointManager`，随后立即调用 `Initialize`；`job_orchestrator.rs` 在提交表任务前读取旧检查点并在提交成功后记录 `Running`；`job_monitor.rs` 把任务最终结果写成 `Finished` 或 `Failed`；`precheck.rs::CheckpointCheckItem::Check` 和 `importer.rs::initGroupKey` 通过 `GetCheckpoints` 检查旧失败任务并恢复同一批任务的 group key。

本文件只保存“每张表对应哪个 IMPORT INTO job、处于什么状态”这一层信息。engine/chunk 检查点属于接口兼容面，三个实现的 `DumpEngines`、`DumpChunks` 在当前代码中都不产生内容。

## 核心职责

1. 用 `CheckpointStatus` 和 `TableCheckpoint` 定义稳定的状态/持久化格式。
2. 用 `CheckpointManager: Send + Sync` 统一禁用检查点、JSON 文件和 MySQL 三种后端，使 importer、orchestrator、monitor、precheck 不依赖具体存储。
3. 用 `NewCheckpointManager` 根据 `cfg.Checkpoint.Enable` 与 `cfg.Checkpoint.Driver` 选择实现，并拒绝未知 driver。
4. 为文件后端提供内存索引、JSON 加载/保存、错误检查点重置/销毁及 CSV 导出。
5. 为 MySQL 后端创建 schema/table，以参数化 SQL 完成同一组读写，并用事务原子地“读取待销毁记录 + 删除记录”。
6. 保持 Go 同名实现的可观察契约，包括状态整数值、not-found 错误、`all` 语义、CSV 转义和资源关闭行为。

## 主要符号

- `CheckpointStatus`：`#[repr(i32)]` 的公开枚举，数值固定为 `Pending=0`、`Running=1`、`Finished=2`、`Failed=3`。自定义 `Serialize` 输出整数；`Deserialize` 经 `from_i32` 映射，未知整数回退为 `Pending`。`Default` 也是 `Pending`；`String`/`string_i32` 提供 Go 风格文本，其中原始未知值显示为 `"unknown"`。
- `TableCheckpoint`：公开的单表快照，字段为 `TableName`、`JobID`、`Status`、`Message`、`GroupKey`。Serde 名称与 Go JSON tag 一致；空 `Message` 序列化时省略，缺失的 `Message`/`GroupKey` 反序列化为空串。
- `CheckpointManager`：公开对象安全 trait，要求实现 `Send + Sync`。它定义初始化、单表/全量读取、更新、删除、失败恢复、失败销毁、三类 CSV dump 和关闭共 11 个操作。
- `NewCheckpointManager`：公开工厂。禁用时返回 `NoopCheckpointManager`；file driver 使用 `Checkpoint.DSN`；mysql driver 使用 `MySQLParam.unwrap_or_default()` 与 `Checkpoint.Schema`；其他值返回 `unknown checkpoint driver`。
- `NoopCheckpointManager`：禁用检查点时的无副作用实现。读取返回空，写入、dump、关闭全部成功。
- `FileCheckpointManager` / `NewFileCheckpointManager`：持有 `filePath`、可延迟初始化的 `LocalStorage` 和以表名为键的内存 map。私有 `save` 把完整 map pretty-print 为 JSON 后覆盖文件。
- `write_csv_record`：私有 CSV 编码器，模拟 Go `encoding/csv.Writer` 对逗号、引号、换行、回车及前导空白的引号规则，并把带引号字段中的 CRLF 规范化为 LF。
- `MySQLCheckpointManager` / `NewMySQLCheckpointManager`：持有 `sql::DB`、schema 名和固定表名 `import_into_checkpoints`。私有 `ensureTableCheckpointExists` 区分“目标存在但当前不是 Failed”与“目标完全不存在”。

本文件没有条件编译项。它通过 `use crate::stubs::*` 获得 `context`、`config`、`objstore`、`sql`、`errors`、`common`、`log` 等已移植边界；直接外部依赖是 `serde`/`serde_json`。

## 执行流程

### 创建与初始化

`NewImporter` 允许 `WithCheckpointManager` 注入替身；没有注入时才调用 `NewCheckpointManager`。无论来源如何，`NewImporter` 都先 `Initialize`，再执行 `initGroupKey` 和构建 orchestrator，因此后续并发任务看到的是已初始化管理器。

- noop：`Initialize` 直接成功。
- file：从 `filePath` 拆出目录和 basename，建立 `LocalStorage`，开启删除不存在文件时忽略 ENOENT，并在读文件前把 storage 发布到锁内。文件不存在或内容为空视为无检查点；非空内容按 `HashMap<String, TableCheckpoint>` 解码。
- mysql：依次执行 `CREATE DATABASE IF NOT EXISTS` 和 `CREATE TABLE IF NOT EXISTS`。表以 `table_name` 为主键，另存 job/status/message/group_key，并由 MySQL 更新 `update_time`。

### 导入与恢复主链

1. `Importer::initGroupKey` 调用 `GetCheckpoints`，取首个非空 `GroupKey`；没有历史值才生成新的 `lightning-<uuid>`。
2. `CheckpointCheckItem::Check` 遍历全部检查点；发现 `Failed` 时阻止预检查通过，提示清理失败检查点。
3. `JobOrchestrator::submitAllJobs` 对每张表调用 `Get`：`Finished` 直接跳过；`Running` 且 `JobID > 0` 时恢复已有 job；其余情况继续提交。
4. `recordSubmission` 在任务成功提交后写入 `Running`、job ID 和 group key。
5. `JobMonitor` 或 orchestrator 的收尾路径写入 `Finished`/`Failed`，失败时同时保存结果消息。
6. 全部成功后，若 `KeepAfterSuccess == CheckpointRemove`，`Importer::Run` 以 `common::AllTables` 调用 `Remove`；`Importer::Close` 最后调用管理器 `Close`。

### 修改和清理

- `Update` 是 upsert。文件实现先更新内存 map 再保存整个 JSON；MySQL 使用 `INSERT ... ON DUPLICATE KEY UPDATE`。
- `Remove(all)` 清空所有记录：文件实现清空 map 并删除文件，MySQL 执行无 WHERE 的 DELETE。单表删除即使目标不存在也成功。
- `IgnoreError` 只把 `Failed` 改回 `Pending`，同时清空 message、把 job ID 归零；非 Failed 的已存在记录保持原样。单表目标不存在返回 `ErrCheckpointTableNotFound`，`all` 则只处理所有失败项。
- `DestroyError` 只删除 `Failed` 并返回被删除的快照；单表目标存在但非 Failed 时返回空列表，目标不存在时报 not-found。MySQL 将 SELECT 与 DELETE 放入 `SQLWithRetry::Transact` 的同一事务。
- `DumpTables` 输出列 `table_name,job_id,status,message,group_key`，status 使用整数。map/数据库查询没有显式排序，因此行顺序不是接口保证。

## 数据与状态

`TableCheckpoint` 的业务不变量由调用链共同形成：`TableName` 是 `common::UniqueTable(database, table)` 的结果；`Running` 通常配正 job ID；终态由 monitor 写成 `Finished` 或 `Failed`；失败消息只对 `Failed` 有意义；同一批提交共享 `GroupKey`。类型本身没有强制这些组合，写入者必须维护它们。

文件后端以内存 map 为当前进程的权威视图，每次变更都重写完整 JSON。`Get`、`GetCheckpoints` 和 `DestroyError` 返回 clone，调用者修改返回值不会绕过 `Update` 改写内部状态。JSON 顶层键和 `TableCheckpoint.TableName` 预期相同，但反序列化没有额外校验；更新时以结构体内的 `TableName` 为键。

MySQL 后端用 `table_name` 主键保证每表一行。读取时 SQL NULL 的 `message`/`group_key` 被折叠为空串；数据库中的未知 status 通过 `from_i32` 变成 `Pending`。这与反序列化未知整数的 Rust 行为一致，但会丢失原始未知值。

状态整数和 JSON/SQL 字段均是兼容边界，不能随意重排枚举或改名。`GroupKey` 用于进程重启后让恢复任务继续归入原批次，而不只是展示字段。

## 依赖与调用关系

上游直接调用关系（由 RustCodeGraph 的目标文件索引及局部调用点核对）：

- `importer.rs::NewImporter -> NewCheckpointManager -> CheckpointManager::Initialize`
- `importer.rs::initGroupKey -> CheckpointManager::GetCheckpoints`
- `precheck.rs::CheckpointCheckItem::Check -> CheckpointManager::GetCheckpoints`
- `job_orchestrator.rs::submitAllJobs -> CheckpointManager::Get`
- `job_orchestrator.rs::recordSubmission/updateJobCheckpoints -> CheckpointManager::Update`
- `job_monitor.rs` 的状态处理 -> `CheckpointManager::Update`
- `Importer::Run -> CheckpointManager::Remove(AllTables)`，`Importer::Close -> CheckpointManager::Close`

下游依赖：

- file 路径：`objstore::NewLocalStorage` 及 `ReadFile`/`WriteFile`/`DeleteFile`，`serde_json`，`std::sync::RwLock`。
- mysql 路径：`common::MySQLConnectParam::Connect`、`sql::DB` 查询/执行接口、`common::EscapeIdentifier`、`common::SQLWithRetry::Transact`。
- 通用路径：`context::Context` 传递取消/超时语义，`common::AllTables` 表示批量操作，`common::ErrCheckpointTableNotFound` 保持可分类的 not-found 错误。

Cargo manifest 只显式列出同仓库 precheck crate、`serde`、`serde_json`、`url`、`uuid`；本文件的许多 Go 风格 API 来自本 crate 的 `stubs.rs`，并不代表连接了完整生产版 Go 依赖。模块入口公开重导出本文件符号，测试替身也实现同一 trait。

## 错误处理与边界

- 工厂遇到未知 driver 立即失败；MySQL 连接失败由构造器透传。
- file 初始化忽略不存在和空文件，但 I/O 错误或 JSON 损坏会被 `errors::Trace` 返回。storage 在解码前已保存，因此调用者即使收到损坏 JSON 错误，仍可用 `Update` 覆盖修复文件；`checkpoint_test.rs::test_file_checkpoint_manager_can_recover_after_invalid_json` 专门保护此行为。
- file 的 `save` 在未初始化时返回 `file checkpoint storage not initialized`。更新 map 发生在持久化之前，因此写盘失败后内存状态已经改变；这是与 Go 相同的操作顺序，调用者不能把返回错误理解为“内存回滚”。
- 单表 `IgnoreError`/`DestroyError` 对缺失目标产生可被 `errors::IsNotFound` 识别的错误；错误文本来自 `ErrCheckpointTableNotFound`，测试还确认不会混入旧命令行 flag 提示。
- MySQL `IgnoreError` 更新 0 行时会二次读取：行不存在则报 not-found，行存在但非 Failed 则成功。`DestroyError` 也用同样检查区分这两种情况。
- MySQL `DestroyError` 在事务闭包内先完整读取并检查 `rows.Err()`，再删除；任一步失败都会使 `Transact` 返回错误。当前 stubs 中的事务/重试能力是移植边界，不能由本文件推断出真实驱动的退避策略。
- CSV writer 的任一次写失败立即返回包装错误。file dump 的 HashMap 顺序未定义；测试只应断言内容或单记录精确编码，不应依赖多记录顺序。
- `RwLock::read/write().unwrap()` 与事务结果共享的 `Mutex::lock().unwrap()` 在锁中毒时会 panic，而不是返回 `Result`；这是当前实现的明确边界。

## 并发与资源生命周期

trait 的 `Send + Sync` 允许同一个 `Arc<dyn CheckpointManager>` 被 orchestrator 的多个提交线程和 monitor 共享。

文件实现用两个 `RwLock`：`checkpoints` 保护 map，`storage` 保护延迟发布的本地存储句柄。写操作在持有 map 写锁期间完成 JSON 编码和文件 I/O，从而串行化同一实例的修改并避免旧快照覆盖新快照；代价是慢 I/O 会阻塞所有读写。`Initialize` 分别操作两个锁，预期只在并发工作开始前调用。`Remove(all)` 同时持有 map 写锁与 storage 读锁；其他路径保持相同的“map 后 storage”获取次序，当前没有反向嵌套。

MySQL 实现依赖 `sql::DB` 自身的共享能力，没有本地状态锁。`DestroyError` 用数据库事务维持选择结果与删除集合一致；闭包把结果写入 `Arc<Mutex<Vec<_>>>`，事务成功后再 clone 返回。其他单条 upsert/delete/update 由数据库语句原子性负责。

资源所有权方面，file/noop 的 `Close` 无操作；MySQL `Close` 调用 `db.Close()`。`Importer::Close` 只记录关闭错误而不向调用方返回。所有 I/O/SQL 方法都接收 context；但 file 的纯内存 `Get` 和 dump 不检查取消状态。

## 与 Go 版本的对应关系

直接对照文件是 `lightning/pkg/importinto/checkpoint.go`，契约测试分别是 `checkpoint_test.go` 与 `checkpoint_test.rs`。Rust 保留 Go 的公开命名和接口形状，并以 `Arc<dyn CheckpointManager>` 对应 Go interface，以 `RwLock<HashMap<...>>` 对应 Go 的 `sync.RWMutex + map`。

主要一致点：四个状态的整数值和字符串、JSON 字段、三种后端选择、file 全量重写、MySQL 表结构/upsert、`all` 批处理、错误恢复/销毁规则、engine/chunk dump no-op、MySQL close 均与 Go 同名实现对应。Rust 独立测试覆盖了 Go 测试的 file/noop/mysql/status 场景。

需要注意的表达差异：

- Go 可以构造 `CheckpointStatus(999)`；Rust 安全枚举不能，因此用 `string_i32(999)` 验证 `"unknown"`，而反序列化未知值回退 `Pending`。
- Go file map 存指针，Rust map 存值并在读出时 clone；两者都防止调用者直接修改内部对象。
- Go 使用 `encoding/csv.Writer`，Rust 私有 `write_csv_record` 手工复刻转义；Rust 的精确测试覆盖前导空格、逗号、双引号和 CRLF。
- Go 在初始化时同样先发布 storage 再 JSON decode；Rust 的损坏 JSON 恢复测试把这一隐含顺序变成显式回归契约。
- Rust 当前通过 `stubs.rs` 提供 SQL/对象存储等边界。文档只能确认本 crate 当前可观察实现，不能据此宣称已具备 Go 生产驱动的全部连接池、事务重试或远端存储特性。

## 扩展指南

- 新增 checkpoint driver：扩展 `NewCheckpointManager` 的分支，新增独立 manager 类型实现完整 trait，并在 `checkpoint_test.rs` 增加选择、初始化、读写、错误、关闭测试；同时对照 Go 工厂与配置常量，避免 Rust-only driver 漂移。
- 修改状态机：必须保持现有整数编码；若新增状态，要同步 `Serialize`/`Deserialize`、`String`/`string_i32`、SQL/CSV 展示、orchestrator/monitor 分支，以及 Rust/Go 状态测试。未知值兼容策略若改变，需要显式迁移旧 JSON/SQL 数据。
- 修改 `TableCheckpoint`：同步 Serde 名、MySQL DDL、全部 SELECT/INSERT/scan 顺序、CSV header/record、Go struct tag；评估旧 JSON 缺字段和旧数据库 schema 的向后兼容。
- 修改 file 写入：保留“storage 即使 decode 失败也可用于修复”的顺序；若引入原子 rename、fsync 或回滚，应为写失败后的内存/磁盘一致性新增独立测试，不能把测试嵌入生产源文件。
- 修改失败处理：分别覆盖 all、缺失表、存在但非 Failed、Failed 四种情况。MySQL `DestroyError` 必须继续保证返回集合与删除集合处于同一事务。
- 修改并发模型：关注持锁 I/O 的吞吐、锁中毒 panic 和 Initialize 与工作线程的时序；若缩短锁区间，需要版本号或其他机制防止并发更新丢失。
- 修改 CSV：以 Go `encoding/csv` 为兼容标准，扩展 `checkpoint_test.rs::test_file_checkpoint_manager_dump_tables_uses_go_csv_escaping`；不要依赖 HashMap 行序。

相关独立 Rust 测试应继续放在 `lightning/pkg/importinto/checkpoint_test.rs`；跨文件入口/工厂语义可同步检查 `parity_test.rs` 和 `importer_test.rs`。本任务不建议把测试放回 `checkpoint.rs`。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 文件、307,296 节点；`files --filter lightning/pkg/importinto` 确认目标源、Go 对照和独立测试均已索引；`query NewCheckpointManager/FileCheckpointManager/MySQLCheckpointManager` 定位 Rust 与 Go 同名符号；`node --file lightning/pkg/importinto/checkpoint.rs` 核对完整定义和文件关系。精确 `callers` 查询未在等待窗口内返回，调用边随后用局部源码搜索复核。
- 生产源码：`lightning/pkg/importinto/checkpoint.rs`、`lib.rs`、`importer.rs`、`job_orchestrator.rs`、`job_monitor.rs`、`precheck.rs`。
- crate 边界：`lightning/pkg/importinto/Cargo.toml`。
- Go 对照：`lightning/pkg/importinto/checkpoint.go`。
- 测试证据：`lightning/pkg/importinto/checkpoint_test.rs`、`checkpoint_test.go`；另以 `parity_test.rs` 和 `importer_test.rs` 的调用点确认工厂与 importer 接线。
- 人工复核结论：本文件存在是为了把 IMPORT INTO 的表任务状态从并发编排逻辑中抽象出来，并提供可恢复的 noop/file/MySQL 后端；安全扩展的关键是同时维护状态编码、存储 schema、错误分类、事务/锁生命周期和 Go 对照测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰好包含 11 个固定二级章节。
