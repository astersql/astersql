# `pkg/executor/load_data.rs`

## 文件定位

本文件属于 `astersql-executor` crate；`pkg/executor/Cargo.toml` 以 `lib.rs` 为库入口，而 `pkg/executor/lib.rs:136` 通过 `pub mod load_data` 对外公开本模块，`lib.rs:138` 将独立测试文件 `load_data_test.rs` 仅在测试构建中装入。文件提供一套以 `LoadDataBackend` 为边界的 LOAD DATA 执行模型：`LoadDataExec` 负责物理算子的打开、执行和关闭，`LoadDataWorker` 负责数据源分派与流水线编排，`encodeWorker` 和 `commitWorker` 分别负责行转换和事务写入。

这套类型当前是公开的 executor crate API，也被 `pkg/executor/test/writetest/write_test.rs` 直接用于行为测试。但代码搜索没有找到生产 Rust 调用者构造 `LoadDataExec` 或调用 `NewLoadDataWorker`；当前 SQL 会话的实际 LOAD DATA 入口是 `pkg/session/runtime/load_data.rs::execute_load_data`，它自行读取文件、解析 CSV、构造 `InsertStmt` 并执行插入。因此，不应把本文件描述成已接入当前 Rust SQL 主链的唯一实现；它更准确地是 Go executor 路径的可复用移植边界和测试覆盖对象。

## 核心职责

- 抽象输入和存储：`LoadDataReader`、`DataParser` 与 `LoadDataBackend` 把字节读取、格式解析、表达式求值、行规范化、事务和写表能力从流水线控制逻辑中分离。
- 管理两类输入：`LoadDataExec::Next` 根据 `FileLocRef` 选择 `LoadDataWorker::loadRemote` 或 `LoadDataWorker::LoadLocal`；本地流由 `LoadDataReaderBuilder::Build` 创建，并在关闭时调用 `Wait` 等待传输侧结束。
- 将解析与提交解耦：`LoadDataWorker::load` 使用容量为 1 的 reader 队列和容量为 `TASK_QUEUE_SIZE`（16）的提交队列，分别驱动编码线程与提交线程。
- 实现行语义：`encodeWorker::parserData2TableData` 处理字段数不匹配、用户变量、生成列占位、缺失非空时间列、SET 赋值、严格/非严格错误以及 backend 行规范化。
- 实现批写语义：`commitWorker::checkAndInsertOneBatch` 按 `Replace`、`Ignore`、`Error` 三种冲突策略调用 backend，并累计 Records/Copied/Deleted、任务数和写入耗时。
- 形成 MySQL 风格结果：`LoadDataWorker::setResult` 计算 Records、Deleted、Skipped、Warnings，且将告警总数限制到 `u16::MAX`。
- 提供局部适配设施：`SimpleSeekerOnReadCloser` 为只读流补充“查询当前位置”的受限 seek；三个 `loadDataVarKeyType` 常量保留 Go 会话键的形状。

## 主要符号

- `Datum`：流水线内部的标量枚举，覆盖 NULL、有/无符号整数、浮点、字节、文本和时间戳。
- `LoadDataError`：统一错误面，包括 backend 错误、取消、EOF、缺 reader、字段数、非法 auto-random、重复键、不支持 seek 和工作线程 panic。`Display` 当前输出枚举的 Debug 形式。
- `LoadDataReader` / `DataParser`：分别定义可关闭输入流，以及 `read_row`、`recycle_row`、`close` 的逐行解析协议。
- `LoadDataBackend`：本文件最重要的集成 trait。它负责数据文件初始化与 parser 构造、赋值表达式、行规范化、事务生命周期、批量或逐行插入、写 `_tidb_rowid` 权限和 killed 检查。
- `LoadDataController`：不可变执行配置，包含路径、严格模式、忽略行数、字段映射、插入列、赋值数量、冲突策略、批大小、低优先级和 shard 分配步长。
- `StatementStats`：由 `Arc<Mutex<_>>` 共享的语句统计与最终消息。
- `LoadDataExec<B>`：面向执行器生命周期的入口；`Open`、`Next`、`Close` 分别创建本地 reader、执行一次完整加载、释放 worker/reader。
- `LoadDataWorker<B>`：持有 backend、controller、`planInfo`、表名和统计；关键方法为 `loadRemote`、`LoadLocal`、`load`、`setResult`、`Close` 和测试辅助 `TestLoadLocal`。
- `encodeWorker<B>`：保存用户变量、行缓冲、全局行号和当前批计数；关键方法为 `processStream`、`processOneStream`、`readOneBatchRows`、`parserData2TableData`。
- `commitTask` / `commitWorker<B>`：提交任务只携带有效行数和行缓冲；提交 worker 通过 `commitWork`、`commitOneTask`、`checkAndInsertOneBatch` 顺序处理批次。
- `InsertValues<B>`、`createInsertValues`、`initEncodeCommitWorkers`：组装编码/提交双方共享的 backend、controller、计划与统计，并在组装前校验额外句柄写权限。
- `SimpleSeekerOnReadCloser`：跟踪累计读取字节数，只支持 `SeekFrom::Current(0)`，其 `GetFileSize` 明确不受支持。

## 执行流程

1. 构造阶段由 `NewLoadDataWorker` 创建共享 `StatementStats`；若 controller 非严格，`setNonRestrictiveFlags` 将统计标为 `non_restrictive`。`createInsertValues` 同时检查插入列是否包含 extra handle，以及 backend 是否允许写 row id。
2. `LoadDataExec::Open` 仅在同时存在 `readerBuilder` 和 worker 时调用 `Build(GetInfilePath())`。`Next` 对远端调用 `loadRemote`，对客户端输入取走 `infileReader` 后调用 `LoadLocal`；本地失败会立即进入 `closeLocalReader`。
3. `loadRemote` 先调用 `init_data_files`，再取得全部 `remote_reader_infos`；`LoadLocal` 则通过 `local_reader_info` 将已打开的 reader 登记成单一 reader info。二者最终进入 `LoadDataWorker::load`。
4. `load` 通过 `initEncodeCommitWorkers` 创建两个 worker，再启动 scoped 编码线程和提交线程。主线程把 reader info 送入容量 1 的同步通道；编码线程把 `commitTask` 送入容量 16 的同步通道；任一线程出错都会设置共享取消标志并通过 `store_first_error` 保留首错。
5. `processStream` 针对每个 reader 调用 `open_parser`，跳过 `ignore_lines`，再进入 `processOneStream`；无论处理结果如何都会尝试 `parser.close()`，但关闭失败被有意忽略，以匹配 Go 的日志式清理行为。
6. `readOneBatchRows` 逐行读取，先递增 `rowCount`，再由 `parserData2TableData` 映射字段并调用 `normalize_row`。到 EOF 或达到 `maxRowsInBatch` 返回；`processOneStream` 将缓冲所有权移入任务，发送成功后 `resetBatch`。
7. `commitWork` 顺序消费任务。`commitOneTask` 开启事务，依次执行批次插入、statement commit 和 transaction commit；任一步失败都会尽力 rollback，成功后递增 `commit_tasks`。
8. 编码与提交线程 join 后，`load` 先用编码侧表达式告警调用 `setResult`，再传播编码结果、提交结果或共享首错。线程 panic 由 `panic_error` 转成 `LoadDataError::Panic`。
9. `Close` 幂等关闭 controller；`LoadDataExec::Close` 随后关闭本地 reader、调用 builder 的 `Wait`，并保留 worker/reader 两类错误中的第一个。

`TestLoadLocal` 是测试专用的单线程捷径：它跳过指定行、读取一个批次、直接写入、statement commit、transaction commit，再形成结果消息；它不经过双线程和同步通道。

## 数据与状态

- `LoadDataController` 在 worker 内由 `Arc` 持有，执行期间视为只读配置。`max_rows_in_batch.max(1)` 保证编码循环一定有正批界限。
- `encodeWorker::rowCount` 跨所有 reader 单调递增，用于错误行号和 backend 规范化；`curBatchCnt` 仅表示当前缓冲有效行数，`resetBatch` 将其归零并按最大批量重新预留空间。
- 用户变量保存在编码 worker 的 `BTreeMap<String, Datum>` 中，键统一转小写；输入 NULL 会删除对应键，符合“不保留旧值”的实现选择。赋值表达式由索引和当前变量表交给 backend 求值。
- 字段映射对输入不足使用 `Datum::Null`；但“缺失字段”且目标为非空时间列时填 `current_timestamp`。显式传入的 NULL 不满足“缺失”条件，因此保持 NULL；`pkg/executor/load_data_test.rs::explicit_null_time_value_is_not_treated_as_a_missing_field` 固化了该差异。
- 生成列输入位置会被消费，但写入行中放置 NULL 占位。`normalize_row` 若在非严格模式返回普通错误，当前实现记录告警并返回空行；提交侧 `addRecordLD` 跳过空行。`InvalidAutoRandom` 是例外，始终作为真正错误传播。
- 统计通过 `Arc<Mutex<StatementStats>>` 在两个 worker 间共享。`record_rows` 在提交批次时增加而不是解析时增加；`copied_rows`、`deleted_rows` 由冲突策略路径更新；`runtime_nanos` 只计批插/逐行插入代码段。
- `setResult` 将静态 `expression_warnings` 按 record 数复制，和动态告警合并后截断到 65535 条，并以 `records - copied_rows` 的饱和减法计算 skipped。

## 依赖与调用关系

模块本身只直接使用 Rust 标准库的集合、I/O trait、原子量、`Arc<Mutex<_>>`、同步通道和 scoped thread；具体数据库、解析器和事务依赖全部倒置到 `LoadDataBackend`。crate 边界由 `pkg/executor/Cargo.toml` 的 `astersql-executor` 包和 `lib.rs` 入口确认，本模块没有 feature 条件或条件编译项。

RustCodeGraph 记录的关键下游边包括：

- `LoadDataWorker::load` → `initEncodeCommitWorkers`、`setResult`、`panic_error`。
- `encodeWorker::processStream` → `LoadDataBackend::open_parser`、`DataParser::{read_row,recycle_row,close}`、`processOneStream`。
- `commitWorker::commitOneTask` → `begin_transaction`、`checkAndInsertOneBatch`、`statement_commit`、`commit_transaction`、`rollback_transaction`。
- `checkAndInsertOneBatch` → `set_transaction_low_priority`、`batch_check_and_insert` 或 `addRecordLD`。

可确认的 Rust 上游是测试代码：`pkg/executor/load_data_test.rs` 在模块内部调用 worker/encoder，`pkg/executor/test/writetest/write_test.rs` 作为外部 crate 用户导入公开 API 并调用 `NewLoadDataWorker`、`TestLoadLocal`。`pkg/executor/builder.rs::buildLoadData` 只定义通用 `ExecutorBuildDependencies::build_load_data_executor` 接口并分派计划，搜索不到它对本文件具体类型的引用。生产 SQL 路径 `pkg/session/runtime/load_data.rs::execute_load_data` 走另一套直接实现，这是当前接线限制。

## 错误处理与边界

- 严格模式下，字段数不符、SET 求值失败和普通 `normalize_row` 失败立即终止；非严格模式将这些错误记为告警并继续。`InvalidAutoRandom` 无论模式都终止。
- parser 的非 EOF 读取错误会包装成 `Backend("cannot read LOAD DATA input: ...")`；EOF 是正常流结束。parser 关闭错误不会覆盖处理结果，独立测试 `parser_close_error_is_non_fatal_like_go_deferred_cleanup` 明确验证这一点。
- 通道断开或共享取消会转成 `Cancelled`；等待满队列时循环检查 `cancelled`，编码侧还调用 `backend.killed()`，因此查询终止可打断背压等待。reader 发送侧只检查取消标志，不调用 killed。
- `commitTask.cnt` 必须不大于 `rows.len()`，否则返回 backend 错误；`cnt == 0` 是无操作。未知冲突策略在 Rust 枚举类型下无法构造，因此不存在 Go `default` 分支。
- `Error` 冲突策略逐行写入并开启 duplicate check；`Replace`/`Ignore` 都走 `batch_check_and_insert`，以布尔值区分是否替换。具体重复键错误和值转换语义由 backend 保证。
- 低优先级仅在非空批次插入前调用 `set_transaction_low_priority`；失败会触发事务回滚。
- Mutex poison 通过 `expect` 触发 panic；工作线程 panic 可被 join 捕获并转成 `LoadDataError::Panic`，但主线程自身的统计锁 panic 不在该转换范围内。
- 本地 `Next` 会 `take()` reader，因此同一 exec 再次调用客户端 `Next` 会得到 `ReaderMissing`。`Close` 对 worker 幂等，但 reader 已取走并成功加载后不会重新放回 exec。
- `SimpleSeekerOnReadCloser` 不是通用 seek：仅查询当前位置合法，绝对/末尾/非零相对 seek 均失败，文件大小也不可查询。

## 并发与资源生命周期

`LoadDataWorker::load` 使用 `thread::scope`，两个工作线程不能逃逸调用栈，返回前必定 join。reader 通道容量 1 限制尚未解析的数据源数量，task 通道容量 16 为编码和存储之间提供有限背压；这两个容量是内存上界和吞吐权衡点。

`AtomicBool` 使用 Release 写、Acquire 读传播取消，`first_error: Mutex<Option<LoadDataError>>` 保证只保存第一个观察到的工作线程错误。线程仍各自返回原始 `Result`，join 后的传播次序是编码结果优先、提交结果其次、共享首错最后；这意味着两侧近同时失败时，最终错误不完全等同于最早写入 `first_error`，扩展时需要谨慎保持或明确修正该契约。

每个 parser 在单个 reader 处理结束后关闭。controller 由 `LoadDataWorker::Close` 关闭且 `closed` 保证幂等。本地 reader 通常由 exec 持有：失败路径和 exec 关闭路径调用 `closeLocalReader`；该函数先关闭 reader，再无条件调用 builder `Wait`。backend 事务则以提交任务为单位：成功 commit，失败尽力 rollback；rollback 错误不会覆盖原错误。

共享统计的每次修改都在短临界区内完成，但 backend 自身必须满足 `Send + Sync + 'static`，其事务方法是否允许提交线程独占或跨线程调用由具体实现负责。`DataParser` 和 `LoadDataReader` 只要求 `Send`，会随工作所有权移动，不在多线程间共享。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/load_data.go`。Rust 保留了 Go 的主体分层和命名：`LoadDataExec`/`LoadDataWorker`、编码与提交双 worker、容量 1/16 的通道、字段映射、用户变量大小写不敏感、生成列占位、缺失非空时间列默认值、三种冲突策略、告警上限、受限 seek 包装和三个会话键。

主要语义对应如下：

- Go `errgroup.WithContext` 的相互取消在 Rust 中由 `AtomicBool`、首错槽和 scoped threads 表达；Rust 用 `try_send + yield_now` 实现可检查取消的背压。
- Go `terror.Log(dataParser.Close())` 对应 Rust 丢弃 `parser.close()` 错误，且 Rust 独立测试明确锁定该行为。
- Go `parserData2TableData` 的严格/非严格分支、用户变量、生成列、时间默认值和 `ErrInvalidAutoRandom` 特例均有 Rust 对应。Rust backend 将 Go 的 session、expression 和 `InsertValues::getRow` 细节抽象成 `evaluate_assignment` 与 `normalize_row`。
- Go Replace/Ignore 使用 `batchCheckAndInsert`，Error 逐行 `addRecord` 并应用 shard size hint；Rust保持同一分支结构。
- Go `setResult` 按记录数复制初始化表达式告警并限制到 `math.MaxUint16`；Rust 使用 `MAX_WARNINGS` 和饱和算术实现同一结果形状。

已确认的差异与迁移边界：

- Go worker直接持有 `sessionctx.Context`、真实 table、planner 表达式和 importer controller；Rust用简化 `planInfo`、表名和 `LoadDataBackend` trait 替代，许多数据库语义取决于尚未在本文件中出现的具体 backend。
- Go 本地路径根据扩展名建立解压 reader；Rust `LoadLocal` 只委托 `local_reader_info/open_parser`，解压能力是否存在取决于 backend。
- Go `commitTask` 还有 `rowCount`，供提交前生成列填充使用；Rust 在编码阶段调用 `normalize_row`，任务不携带结束行号。
- Rust `commitOneTask` 显式以每批 begin/statement commit/transaction commit/rollback 管理事务；Go 文件中的生产提交依赖 session/insert 基础设施，没有同形的显式 begin/commit 序列。
- Go 的 killed 检查以 30 秒 ticker 执行；Rust在 task 通道满时每轮调用 `backend.killed()`，时机和调用频率不同。
- Go 当前 SQL executor 路径与 session/context 深度集成；Rust 当前 SQL 入口 `pkg/session/runtime/load_data.rs` 独立完成 LOAD DATA，尚无证据表明本文件的 executor worker 被它调用。

Go 回归意图还由 `pkg/executor/test/writetest/write_test.go` 的 `TestLoadDataMissingColumn`、`TestIssue18681`、`TestIssue34358` 对照到同目录 Rust `write_test.rs` 中的同名测试：分别覆盖缺失时间列、BIT 值/列顺序和 NULL 用户变量赋值告警。

## 扩展指南

- 新增文件格式或远端存储时，优先扩展具体 `LoadDataBackend::{init_data_files,remote_reader_infos,open_parser}`，不要把存储 SDK 或 parser 细节塞入线程编排；同时覆盖多 reader、`ignore_lines`、parser close 和取消路径。
- 新增列映射或 SET 语义时，修改 `FieldMapping`、`LoadDataController` 与 `parserData2TableData`，并同步独立测试 `pkg/executor/load_data_test.rs`；涉及 Go 移植语义时还应同步 `pkg/executor/test/writetest/write_test.rs` 中对应回归。必须区分显式 NULL 与缺失字段，避免破坏时间默认值测试。
- 新增冲突策略时，需要同时扩展 `OnDuplicateKeyHandling` 和 `checkAndInsertOneBatch`，定义 copied/deleted/skipped 统计、duplicate check、回滚和低优先级行为，并与 Go `load_data.go` 对照。
- 调整批量或通道容量时，关注峰值内存、生产/消费背压、取消延迟与吞吐；`TASK_QUEUE_SIZE` 和 `max_rows_in_batch` 的乘积近似决定待提交行缓冲的上界。
- 修改错误优先级或取消机制时，应增加双侧同时失败、发送端断开、线程 panic、rollback 失败的独立测试；当前 join 后的错误传播顺序与首错槽并不完全一致。
- 若要把本模块接入实际 Rust SQL 主链，必须在 executor builder/session runtime 中提供真实 `LoadDataBackend` 并明确替换还是复用 `execute_load_data`；在完成接线前，不应把仅通过测试 backend 的能力声明为生产已支持。
- 扩展 `SimpleSeekerOnReadCloser` 前先确认 parser/解压器真实 seek 契约；当前对象只承诺位置查询，不承诺随机访问或文件大小。
- Rust 单元测试继续放在独立 `pkg/executor/load_data_test.rs`，跨 crate 的 Go 对齐回归放在现有 `pkg/executor/test/writetest/write_test.rs`，不要内嵌到生产源文件。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标 `pkg/executor/load_data.rs` 可按行读取，共 956 行。
- 已读取源码与边界：`pkg/executor/load_data.rs`、`pkg/executor/lib.rs` 的模块声明、`pkg/executor/Cargo.toml`、`pkg/executor/builder.rs::buildLoadData`、`pkg/session/runtime/load_data.rs::execute_load_data`。
- 已读取测试：`pkg/executor/load_data_test.rs`；`pkg/executor/test/writetest/write_test.rs` 中 LOAD DATA backend 与 `TestLoadDataMissingColumn`、`TestIssue18681`、`TestIssue34358`；对应 Go 测试位于 `pkg/executor/test/writetest/write_test.go`。
- 已读取 Go 对照：`pkg/executor/load_data.go` 的执行器生命周期、worker 构造、双 worker 流水线、字段转换、提交策略、测试辅助、受限 seeker 和会话键。
- 已执行 RustCodeGraph `query`：`LoadDataExec`、`LoadDataWorker`、`encodeWorker`、`commitWorker`、`NewLoadDataWorker`、`initEncodeCommitWorkers`、`createInsertValues`、`SimpleSeekerOnReadCloser`、`LoadDataVarKey`。
- 已执行 RustCodeGraph `callees`：`NewLoadDataWorker`、`load`、`processStream`、`commitOneTask`、`checkAndInsertOneBatch`、`execute_load_data`。图查询的 `callers` 对本文件关键方法未返回生产调用边，随后用模块引用搜索确认公开模块和测试调用者，并核对 session 当前独立入口。
- 人工复核结论：本文区分了本文件的可复用 worker 能力和当前 SQL 主链接线事实；所有“已支持”描述均限定在本文件及 backend 契约内，没有把测试 backend 或 Go 行为推断成未验证的生产实现。
