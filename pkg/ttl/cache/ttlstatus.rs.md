# `pkg/ttl/cache/ttlstatus.rs`

## 文件定位

对应 Go 文件 `pkg/ttl/cache/ttlstatus.go`，负责描述并读取系统表 `mysql.tidb_ttl_table_status` 中每个物理表的 TTL 作业状态。

## 核心职责

- 以字符串常量定义 TTL 作业的六种已知生命周期状态，同时用 `String` 保存状态，允许保留系统表中的未知值。
- 用 `selectFromTTLTableStatus` 固定查询 17 列；`SelectFromTTLTableStatusWithID` 在此基础上生成按物理表 ID 查询的 SQL 和绑定参数。
- 用 `TableStatus` 承载一张物理表的历史作业与当前作业信息。
- 通过最小会话抽象 `StatusSession` 执行全表查询，并由 `TableStatusCache::Update` 建立 `table_id -> TableStatus` 的内存快照。
- 由 `RowToTableStatus` 按固定列序解码查询结果，并实现 `current_job_status` 的 Go 兼容空值语义。
- 复用 `baseCache` 的刷新间隔和最近成功刷新时刻，提供 `ShouldUpdate`、`SetInterval` 与成功更新后的 `MarkUpdated`。

## 主要符号

- `pub type JobStatus = String`：不是封闭枚举。已知常量为 `JobStatusWaiting`、`JobStatusRunning`、`JobStatusCancelling`、`JobStatusCancelled`、`JobStatusTimeout`、`JobStatusFinished`；未知数据库值仍可原样保存。
- `selectFromTTLTableStatus: &str`：按确定顺序选择 17 列，并使用 `LOW_PRIORITY` 降低内部读取与在线业务争用。它是行解码列序的隐式契约。
- `SelectFromTTLTableStatusWithID(i64) -> (String, Vec<Datum>)`：追加 `WHERE table_id = %?`，并把 ID 包装为 `Datum::Int`。它只构造 SQL，不执行查询。
- `TableStatus`：公开数据结构。`TableID`/`ParentTableID` 标识物理表及父表；`LastJob*` 保存最近完成作业；`CurrentJob*` 保存当前作业 ID、所有者、心跳、起始/过期时间、内部状态、生命周期状态和状态更新时间。
- `StatusSession::execute_sql(&self, &str) -> Result<Vec<Row>, String>`：Rust 移植所需的最小查询接口。它没有上下文、绑定参数或会话时区参数。
- `TableStatusCache { cache, Tables }`：私有 `baseCache` 管理刷新节拍，公开 `Tables: HashMap<i64, TableStatus>` 保存当前快照。
- `NewTableStatusCache(Duration) -> TableStatusCache`：调用 `newBaseCache` 并创建空表映射；新缓存尚未 `MarkUpdated`，所以 `ShouldUpdate` 为真。
- `TableStatusCache::{ShouldUpdate, SetInterval, Update}`：分别代理刷新判断、调整间隔和执行全量刷新。
- `RowToTableStatus(&Row) -> Result<TableStatus, String>`：校验至少 17 列，然后按索引把 `Datum` 转为字段值。

## 执行流程

全量缓存刷新由 `TableStatusCache::Update` 驱动：

1. 使用固定 SQL `selectFromTTLTableStatus` 调用 `StatusSession::execute_sql`。
2. 查询失败时通过 `?` 立即返回错误，不修改现有 `Tables`，也不更新时间。
3. 按返回行数预分配新的 `HashMap`，逐行调用 `RowToTableStatus`。
4. 任一行解码失败时立即返回；因为所有结果先写入局部 map，旧缓存仍完整保留。
5. 以 `TableID` 为键插入。若查询结果包含重复 ID，后出现的行覆盖先出现的行。
6. 所有行成功后一次性把 `self.Tables` 替换为新 map；空结果会清空旧缓存。
7. 最后调用 `baseCache::MarkUpdated`，只有成功完成的刷新才影响下一次 `ShouldUpdate` 判断。

单行解码由 `RowToTableStatus` 完成。它先拒绝少于 17 列的行；随后特别处理索引 15：`NULL` 产生空字符串，非 `NULL` 的空字符串规范化为 `"waiting"`，其他字符串（包括未知状态）原样保留。其余字段通过 `pkg/ttl/cache/task.rs` 的 `int`、`string` 辅助函数按索引 0 到 16 提取。

单表查询构造器 `SelectFromTTLTableStatusWithID` 不属于全量刷新路径；在 Go 生产实现中，它用于事务内 `FOR UPDATE NOWAIT` 查询和 timer 状态同步。Rust 当前没有对应生产调用者。

## 数据与状态

`TableStatusCache::Tables` 是一次成功查询形成的完整快照，而非增量缓存。其关键不变量是 map 键应等于值内的 `TableStatus.TableID`；该关系由 `Update` 插入时直接建立。缓存不保留数据库中已删除的行，因为每次更新都会全量替换。

`TableStatus` 派生 `Default`，因此所有数值/时间字段的零值是 `0`，字符串为空。`RowToTableStatus` 也沿用这种零值语义：`task.rs::int` 对 `Null`、缺失或类型不匹配返回 `0`；`task.rs::string` 对这些情况返回空字符串，并会把 `Bytes` 以有损 UTF-8 转成字符串。只有“整行不足 17 列”会被显式拒绝，单个列类型不符不会报错。

Rust 中所有时间字段均为 `i64`，由 `Datum::Time`、`Datum::Int` 或 `Datum::UInt` 转入；这只是当前移植层的数据表示。它不同于 Go 的 `time.Time`，没有在本文件中保存时区或执行本地时区转换。`Datum::UInt` 转 `i64` 使用 Rust 的 `as` 转换，超出 `i64::MAX` 时会发生二进制截断语义，而不是返回错误。

`JobStatus` 使用字符串而非 Rust enum 是兼容性选择：数据库可能出现新状态，读取端不能因枚举未覆盖而拒绝整个快照。尤其要区分状态列为 `NULL`（空字符串）和非 `NULL` 空串（`waiting`）。

## 依赖与调用关系

直接内部依赖如下：

- `crate::base::{baseCache, newBaseCache}`：提供刷新间隔、`ShouldUpdate`、`SetInterval` 和 `MarkUpdated`。
- `crate::task::{Datum, Row, int, string}`：提供简化的系统表单元值、行类型和容错提取函数。
- 标准库 `HashMap`、`Duration`：分别保存快照和配置刷新间隔。

RustCodeGraph 对 `NewTableStatusCache` 的被调边显示其调用 `newBaseCache` 并实例化 `TableStatusCache`；对 `RowToTableStatus` 的有效文件内边显示其引用 `JobStatusWaiting` 并实例化 `TableStatus`。图查询没有返回这三个公开构造/转换函数的 Rust 生产 callers；`rg` 进一步确认直接调用均在 `pkg/ttl/cache/ttlstatus_test.rs`。对通用名 `Update` 的全仓图查询存在大量同名歧义，不能据此推导本方法的调用者。

Go 对照链提供应用位置证据：`NewJobManager` 创建 `tableStatusCache`；job loop 定时调用 `updateTableStatusCache`；`checkNotOwnJob`、`localJobs` 和 `readyForLockHBTimeoutJobTables` 读取快照；`getTableStatusForUpdateNotWait` 与 `getTTLTableStatus` 使用单表查询及行解码。上述是 Go 生产接线，不是 Rust 当前已接线的证据。

`Cargo.toml` 将 crate 的 `lib.rs` 设为入口，并以 `package.metadata.porting.go-package = "pkg/ttl/cache"` 声明 Go 对照目录。其跨 crate 依赖和 dev-dependencies 当前都位于 `cfg(target_os = "windows")` 表内；本文件自身只依赖同 crate 模块和标准库。

## 错误处理与边界

- `StatusSession::execute_sql` 的字符串错误由 `Update` 原样传播；没有包装 SQL 上下文，也没有重试、超时或取消机制。
- `RowToTableStatus` 唯一显式结构错误是列数少于 17，错误包含实际列数和期望列数。多于 17 列会忽略尾部列，便于查询结果向后追加，但固定 SELECT 本身仍必须与前 17 列序一致。
- 列类型不符合预期时，`int`/`string` 静默返回零值；这会隐藏 schema 或适配层错误。扩展时若要加强校验，应评估是否会改变 Go 的 NULL/零值兼容行为。
- `current_job_status` 的非字符串非 NULL 值会经 `string` 变为空串，随后转成 `waiting`；这与 NULL 分支不同，是现有逻辑的重要边界。
- `HashMap::insert` 不检查重复 `table_id`。系统表应保证唯一性；若上游破坏该约束，最后一行胜出且不报错。
- `SelectFromTTLTableStatusWithID` 返回 TiDB 风格 `%?` 占位符；只有理解该占位符和 `Datum` 参数的执行适配层才能安全使用，不能直接当作通用 SQL 客户端语句执行。

## 并发与资源生命周期

本文件没有锁、原子、异步任务、通道或后台线程。`Update` 需要 `&mut self`，Rust 借用规则保证同一个缓存实例在该调用期间不能被其他安全 Rust 代码同时读写；跨线程共享策略应由未来调用方用 `Mutex`、`RwLock` 或消息传递等外层机制决定。

刷新采用“局部构建、成功后交换”的事务式内存更新：数据库读取或任一行解码失败都保留旧快照，成功后一次赋值替换。赋值后旧 `HashMap` 及其字符串随所有权释放。该过程不是数据库事务快照的保证；一致性仍取决于 `StatusSession::execute_sql` 的实现。

`baseCache` 用 `Instant` 记录进程内单调时间。新缓存的更新时间为 `None`；成功 `Update` 后置为当前时刻。`SetInterval` 只修改阈值，不触发刷新。文件本身不管理 session 的创建、关闭或连接池归还，也不提供定时器；这些生命周期必须由上层管理。

## 与 Go 版本的对应关系

仍存在明确的移植差异：

- Go `TableStatusCache` 嵌入 `baseCache` 并保存 `map[int64]*TableStatus`；Rust 组合私有 `baseCache`，保存拥有值的 `HashMap<i64, TableStatus>`。
- Go `Update(context.Context, session.Session)` 支持取消/截止时间并由真实 session 提供时区；Rust `Update(&dyn StatusSession)` 没有 context、参数绑定或时区入口。
- Go 对每列先判断 `IsNull`；Rust 辅助函数对 NULL、缺失和错误类型统一给零值，但先用总列数校验避免缺列被完全吞掉。
- Go 缓存已接入 `ttlworker.JobManager`，Rust 缓存在本仓库中尚无生产接线与 `StatusSession` 生产实现。
- Go 的缓存更新时间使用 `time.Now()`；Rust 通过 `baseCache::MarkUpdated()` 保存 `Instant::now()`，都只在成功刷新后更新，但具体时间类型不同。

因此新增 Rust 行为时应以 Go 的可观察语义为基线，但不能把 Rust 现有简化接口误认为已经完整复刻了 Go 的会话、时间和调度集成。

## 扩展指南

新增系统表列时，应同步修改 `selectFromTTLTableStatus` 的列序、`TableStatus` 字段、`RowToTableStatus` 的最小列数与索引映射，并在独立文件 `pkg/ttl/cache/ttlstatus_test.rs` 增加映射、NULL、类型异常和短行用例；同时核对 Go `pkg/ttl/cache/ttlstatus.go` 与 `ttlstatus_test.go`。列插入到中间会改变所有后续索引，风险高于尾部追加。

新增 job 状态时，通常只需增加已知字符串常量，不应把 `JobStatus` 改成拒绝未知值的封闭枚举，否则会破坏滚动升级和新旧版本共存。必须保留 NULL、空串和未知字符串三者的现有区别，并同步状态语义测试。

若接入 Rust 生产调度器，最可能的接入点是：为真实 TTL session 实现 `StatusSession`；在调度器生命周期中创建 `TableStatusCache`；按 `ShouldUpdate`/间隔触发 `Update`；明确缓存的同步原语；在单表锁定路径使用 `SelectFromTTLTableStatusWithID` 和参数，而不是字符串拼接。接入前还需决定如何补齐 context/超时、参数绑定、数据库事务以及时区到 `i64` 的规范。

若要强化解码错误，应优先在 `RowToTableStatus` 或类型化的 `Datum` 提取器处实现，并增加独立回归测试。兼容风险是原先静默零值的数据可能开始使整次刷新失败；性能风险主要来自全表读取和每次全量分配，优化为增量更新前必须保留“失败不暴露半更新”和“删除行能从缓存消失”两项不变量。

测试逻辑必须继续放在独立的 `ttlstatus_test.rs`，不要内嵌到生产源文件。
