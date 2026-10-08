# `pkg/session/runtime/ttl_worker_session.rs`

## 文件定位

本文件位于 `astersql-session` crate 的运行时层，由 `pkg/session/runtime.rs` 以公开模块 `ttl_worker_session` 装配。它把会话 crate 内真正可执行 SQL 的 `ConcreteSession` 适配为 `astersql_ttl_ttlworker::session::WorkerSession`，使 TTL worker 可以通过稳定的抽象执行扫描、删除、作业持久化及定时器元数据 SQL，而不直接依赖完整会话内部结构。

crate 边界由 `pkg/session/Cargo.toml` 证明：`astersql-session` 直接依赖本地 crate `astersql-ttl-ttlworker`、`astersql-parser-mysql`，并依赖 `chrono` 与 `chrono-tz`。本文件没有条件编译项、模块级常量或自定义 trait；生产类型只有 `TtlWorkerSqlSession`，测试位于独立文件 `pkg/session/runtime/ttl_worker_session_test.rs`。

上游运行入口主要位于 `pkg/session/runtime/ttl_runtime.rs` 和 `pkg/session/runtime/ttl_timer.rs`。例如 `run_ttl_tick_inner` 为协调、扫描、删除和检查点分别创建适配器，`JobHeartbeat::start` 在线程内创建独立适配器，`sync_ttl_timers` 与 `job_status` 则用它读写 TTL 系统表。

## 核心职责

1. `impl WorkerSession for TtlWorkerSqlSession` 将 TTL 层的 `Datum` 参数绑定为可执行 SQL，并把 `ConcreteSession` 的结果集逐行转换成 TTL 层的 `Vec<Row>`。
2. `refresh_state` 从真实 SQL 会话读取 TTL 准备/恢复所需的会话变量与事务状态，而不是只维护一份脱离运行时的内存镜像。
3. `expiration_predicate` 按 TTL 列类型和全局时区构造固定的过期谓词，并缓存由扫描阶段捕获的谓词，保证扫描分页、删除重试以及全局时区变化之间使用同一水位语义。
4. `execute_in_transaction` 为 `TableSession::execute_sql_with_check` 提供显式乐观事务边界，并在提交或回滚失败后禁止复用会话。
5. `bind_ttl_parameters` 负责 TTL 专用 `%?` 占位符的参数数目校验、字面量编码，以及当前通用 SQL 运行时尚不能计算 `FROM_UNIXTIME` 时的兼容转换。

本文件不负责生成扫描/删除 SQL、判断 TTL 元数据是否仍有效、安排重试或维护作业状态。这些职责分别位于 `pkg/ttl/ttlworker/scan.rs`、`del.rs`、`session.rs` 和 `persistent.rs`；本文件只提供它们落到真实会话的边界。

## 主要符号

- `pub struct TtlWorkerSqlSession`：持有一个 `ConcreteSession`、一份 TTL 可见的 `SessionState`、可复用标志，以及可选的 `(physical_id, unix, ExpirationPredicate)` 缓存。类型公开，但字段均私有。
- `TtlWorkerSqlSession::new(session)`：接管具体会话，初始化默认状态、允许复用并清空过期谓词缓存。
- `TtlWorkerSqlSession::use_expiration(table, unix, predicate)`：crate 内可见的扫描到删除交接入口，按物理表 ID 和 Unix 水位保存已捕获谓词。
- `TtlWorkerSqlSession::reusable()`：供池化生命周期或测试判断会话是否仍可安全复用。
- `WorkerSession::{state,state_mut}`：暴露 TTL 通用层需要维护的状态快照。
- `WorkerSession::execute(sql,args)`：调用 `bind_ttl_parameters`，再调用 `ConcreteSession::execute`，遍历全部结果集并将每个单元格转为 `Datum::Text`。
- `WorkerSession::refresh_state()`：读取七个会话变量，规范化 1PC/异步提交布尔值，解析扫描并发度，并从 `ConcreteSession` 的事务槽位刷新 `in_transaction`。
- `WorkerSession::expiration_predicate(table,unix)`：命中缓存时直接复用；否则读取全局时区、查询当前表模型与 TTL 列类型，并产生 TIMESTAMP 或墙上时间类型对应的谓词。
- `WorkerSession::execute_in_transaction(sql,args)`：执行 `BEGIN OPTIMISTIC`，运行目标语句后提交；语句失败时尝试回滚。
- `WorkerSession::avoid_reuse()`：把 `reusable` 永久置为 `false`。
- `fn bind_ttl_parameters(sql,args)`：文件私有的 `%?` 参数绑定器。

## 执行流程

典型 TTL tick 的直接流程如下：

1. `pkg/session/runtime/ttl_runtime.rs::run_ttl_tick_inner` 创建协调会话并通过 `PersistentJobStore` 认领或恢复作业；该层的系统表 SQL最终调用本文件的 `execute`。
2. 对每个扫描范围，运行时分别创建 `scan_session`、`delete_session` 和 `checkpoint_session`，调用 `prepare_session_checked`；扫描会话还调用 `prepare_scan_session_checked`，将分布式扫描并发设为 1 并关闭 paging。
3. 扫描会话调用 `expiration_predicate`。缓存未命中时，本文件执行 `SELECT @@global.time_zone`，解析命名或固定偏移时区，从 `Domain::stats_table` 取得当前表模型并定位 TTL 列。
4. TIMESTAMP 列得到 `FROM_UNIXTIME(%?)` 与 `Datum::Unsigned(unix)`；DATETIME/DATE 得到 `CAST(%? AS DATETIME)` 与按捕获时区格式化的墙上时间文本。DATE 且水位恰为午夜时使用日期文本，否则保留时分秒。
5. `run_ttl_tick_inner` 用 `use_expiration` 把同一谓词交给删除会话。`TtlScanTask::execute_with_checkpoint` 和 `DeleteTask::do_delete` 分别调用 `expiration_predicate`，替换各自 SQL 中的 `FROM_UNIXTIME(%?)`，从而共享同一捕获水位。
6. 每次 `execute` 先绑定 `%?`：NULL、整数、无符号整数、转义文本和十六进制字节分别编码；特殊的完整 `FROM_UNIXTIME(%?)` 调用先把 Unix 秒转换为 UTC 文本，再交给运行时执行。
7. `ConcreteSession::execute` 返回一个或多个结果集；适配器逐个调用 `next_row` 直至耗尽，合并所有行并把值包装为 `Datum::Text`。
8. TTL 通用层若要求表校验事务，则通过 `TableSession::execute_sql_with_check` 调用 `execute_in_transaction`：先开启乐观事务，执行语句，成功后提交，失败后回滚，然后通用层在语句后校验元数据。

另两条直接入口不经过扫描主循环：`JobHeartbeat::start` 在线程内用独立会话更新所有权租约；`pkg/session/runtime/ttl_timer.rs::{job_status,sync_ttl_timers}` 用适配器查询作业历史和同步 `mysql.tidb_timers`。

## 数据与状态

`session: ConcreteSession` 是实际 SQL 执行器。`ConcreteSession` 内部以 `Rc<ConcreteSessionInner>` 持有大量 `RefCell`/`Cell` 会话状态，因此适配器本身是线程受限对象；调用方在需要线程并行时于目标线程内重新构造，而不是在线程间共享同一个实例。

`state: SessionState` 是 TTL 通用层的状态镜像，包含变量映射、事务标志、内部用户表扫描标志、扫描并发、paging 开关和当前 TTL job ID。`refresh_state` 用真实 SQL同步变量，并直接观察底层 transaction 是否存在；`prepare_session_checked`、`restore_session_checked`、`prepare_scan_session_checked` 等通用函数通过 `state/state_mut` 配合本文件的 `execute` 完成真实会话与镜像的同步。

`reusable: bool` 初始为 `true`，一旦 `avoid_reuse` 被调用就只会变为 `false`。提交失败、回滚失败、会话准备/恢复失败或 panic 清理路径会触发这一标记，避免污染的会话重新进入池。

`expiration` 是单元素缓存，键为物理表 ID和 Unix 水位，值为完整 `ExpirationPredicate`。只在键完全相等时命中，新的谓词会覆盖旧值；缓存不按逻辑表 ID或表名匹配，避免分区之间误用。

结果行没有在本文件保留原始类型：`execute` 将底层返回的每个字符串值统一包装为 `Datum::Text`。输入侧则完整区分 `Null`、有符号/无符号整数、文本和字节，以便生成对应 SQL 字面量。

## 依赖与调用关系

上游直接调用者：

- `pkg/session/runtime/ttl_runtime.rs`：手动触发结果轮询、作业协调、独立心跳、扫描/删除/检查点会话创建和过期谓词交接。
- `pkg/session/runtime/ttl_timer.rs`：`job_status` 查询历史状态，`sync_ttl_timers` 同步定时器系统表。
- `pkg/ttl/ttlworker/scan.rs::TtlScanTask::execute_with_checkpoint`：获取过期谓词并通过 `execute_with_ttl_job` 执行分页扫描。
- `pkg/ttl/ttlworker/del.rs::DeleteTask::do_delete`：获取相同谓词，批量绑定主键与水位后执行删除。
- `pkg/ttl/ttlworker/persistent.rs::PersistentJobStore`：执行作业认领、心跳、结束和任务持久化 SQL，并在事务收尾失败时调用 `avoid_reuse`。
- `pkg/ttl/ttlworker/session.rs`：定义 `WorkerSession`、会话准备/恢复逻辑及 `TableSession` 事务校验包装。

下游依赖：

- `ConcreteSession::execute` 和结果集 `next_row` 提供真正的 SQL执行与取行。
- `super::quote_argument` 对文本中的反斜线和单引号转义并包裹 SQL 单引号。
- `Domain::stats_table` 与 `astersql-parser-mysql` 的 `TypeTimestamp`/`TypeDate` 用于确认 TTL 列类型。
- `RuntimeTimeZone::parse`、`chrono::DateTime::from_timestamp` 与 `chrono::Offset::fix` 把 Unix 水位转换为命名时区或固定偏移下的墙上时间。

RustCodeGraph 的文件级关系显示本文件被 8 个文件使用，包含 `ttl_runtime.rs`、`ttl_timer.rs`、`ttl_worker_session_test.rs` 及若干同模块测试/服务文件。impl 方法的精确 caller/callee 查询未产出有效边，因此上述方法级关系同时由这些直接源码调用位置核验。

## 错误处理与边界

`execute` 将参数绑定、SQL 执行和取行错误统一映射为 `SessionError::Execute`；底层错误仅保留字符串。它会汇总多个结果集而不是强制单结果集，也不会在此层恢复原始列类型。

参数绑定严格要求 `%?` 与参数一一对应：找不到下一个占位符时报“too many TTL SQL arguments”，参数耗尽后 SQL仍含 `%?` 报“too few TTL SQL arguments”。文本通过 `quote_argument` 转义，字节输出为大写十六进制 `X'…'`。特殊 `FROM_UNIXTIME(%?)` 仅接受整数秒；无符号值超出 `i64` 或时间戳不可表示均报范围错误。该绑定器基于字符串查找，不是通用 SQL 语法解析器，因此扩展占位符语法必须同时评估注释/字符串字面量等上下文风险。

`refresh_state` 要求变量查询至少返回一行一列文本；缺失时明确指出变量名。扫描并发必须可解析为 `usize`，否则返回“invalid scan concurrency”。布尔变量只把大小写无关的 `ON` 或精确的 `1` 视为真。

`expiration_predicate` 对全局时区查询空结果、非法时区、Unix 时间越界分别返回执行错误；表模型或 TTL 列不存在映射为 `TableChanged`。TIMESTAMP 按绝对时刻生成 Unix 参数；DATETIME/DATE 按捕获全局时区生成墙上时间，且比较仍采用严格小于边界。缓存使全局时区在扫描后变化时不改变删除语义。

`execute_in_transaction` 的 `BEGIN OPTIMISTIC` 失败会直接返回。目标语句成功但 COMMIT 失败时返回提交错误并禁止复用；目标语句失败时始终返回原始语句错误，只有 ROLLBACK 失败才额外禁止复用。换言之，回滚错误不会覆盖原始执行错误。

## 并发与资源生命周期

本文件不创建线程、锁、通道或异步任务，也不实现 `Drop`。资源生命周期由拥有它的运行时控制：适配器拥有一个 `ConcreteSession`，离开作用域时随之释放。由于底层使用 `Rc` 与内部可变性，本文件按“一名 worker 一个适配器”的模型工作。

`pkg/session/runtime/ttl_runtime.rs` 明确为心跳创建专用线程与专用 `TtlWorkerSqlSession`，使长扫描或删除限流不会饿死租约更新；每个扫描范围又分别创建扫描、删除和检查点会话，避免事务及会话变量互相干扰。线程停止、join、会话池关闭等生命周期不由本文件管理。

会话准备/恢复采用显式状态快照。通用层设置 retry limit、1PC、异步提交、UTC 与读引擎，并在退出时恢复；扫描层额外设置单并发和关闭 paging。任何部分设置、恢复、commit/rollback 失败都会通过 `avoid_reuse` 将实例标记为不可回池。这是本文件最重要的资源安全不变量。

过期谓词的生命周期与适配器实例相同。运行时先在扫描会话捕获，再显式克隆给删除会话；这样无需共享可变状态或锁，同时保证两条执行链看到相同边界。

## 与 Go 版本的对应关系

Go 对照逻辑集中在 `pkg/ttl/ttlworker/session.go`，并非与本文件同路径的一对一文件复刻：

- Go `withSession` 从 `syssession.Pool` 取会话、挂接统计收集器、调用 `prepareSession` 并 defer 恢复；Rust 将可复用的准备/恢复算法放在 `pkg/ttl/ttlworker/session.rs`，本文件提供真实 SQL执行与 `avoid_reuse` 适配，具体实例由 `ttl_runtime.rs` 创建。
- Go `prepareSession` 保存并设置 retry limit、1PC、异步提交、UTC 和读引擎，失败时清理且禁止复用；Rust 的 `prepare_session_checked`/`restore_session_checked` 保留相同语句顺序和失败清理策略，`refresh_state` 为其读取真实初始值。
- Go `NewScanSession` 保存内部扫描标志、设置扫描并发为 1、关闭 paging，并在恢复失败时 `AvoidReuse`；Rust 对应 `prepare_scan_session_checked`/`restore_scan_session_checked`，本文件提供所需状态与 SQL落地。
- Go `ttlTableSession.ExecuteSQLWithCheck` 在乐观事务中执行语句，并在语句之后验证 TTL 元数据；Rust 对应 `TableSession::execute_sql_with_check` 加本文件 `execute_in_transaction`。
- Go 会话接口原生接受参数化 `ExecuteSQL(ctx, sql, args...)` 和 `time.Time` 过期值；当前 Rust 适配器使用 TTL 专用 `%?` 字面量绑定，并通过 `ExpirationPredicate` 显式区分 TIMESTAMP 绝对时刻与 DATETIME/DATE 墙上时间。这是实现形态差异，不应误写成 Go 代码逐行翻译。

Go 测试 `pkg/ttl/ttlworker/session_test.go` 验证准备/恢复、部分失败不可回池、恢复继续清理、扫描设置和事务后元数据校验。Rust 独立测试 `pkg/session/runtime/ttl_worker_session_test.rs` 进一步覆盖真实 SQL 参数转义、分区隔离、系统表持久化、扫描删除、DST/固定偏移时区、TIMESTAMP 分页、全局时区变化后的严格删除边界，以及真实变量恢复。

## 扩展指南

- 新增 `Datum` 类型时，应修改 `bind_ttl_parameters` 的穷尽匹配，并在 `pkg/session/runtime/ttl_worker_session_test.rs` 增加真实 SQL往返测试；若结果需要保留类型，还需评估 `execute` 当前统一转为 `Datum::Text` 的契约以及 TTL SQL builder 的消费方式。
- 新增 TTL 会话变量时，应同步修改 `refresh_state`、`pkg/ttl/ttlworker/session.rs` 的准备/恢复和失败清理列表，并对照 Go `prepareSession`/`NewScanSession`；必须覆盖设置中途失败、恢复失败仍继续清理和 `reusable == false`。
- 修改过期时间规则时，应优先改 `expiration_predicate` 及 `use_expiration` 交接，不要让扫描与删除各自重新读取全局时区。至少同步覆盖 TIMESTAMP、DATETIME、DATE 午夜与非午夜、命名时区 DST 重叠/跳变、固定偏移、Unix 越界和 TTL 列变化。
- 修改事务策略时，应同时审查 `execute_in_transaction`、`TableSession::execute_sql_with_check` 与 `PersistentJobStore` 自己管理的悲观事务。需要维持“执行后再校验元数据”以及 commit/rollback 失败不可复用的 Go 语义。
- 修改参数绑定时，应注意 `%?` 计数、文本/二进制转义和 `FROM_UNIXTIME` 特判。更换为通用 prepared statement 之前，必须证明 UTC 会话下的 TTL 水位、观测 SQL与所有系统表语句仍等价。
- 性能上，`execute` 会把所有结果集完整物化到 `Vec<Row>`，且字符串绑定会重新分配 SQL；TTL 扫描依赖上层 batch limit 控制规模。若引入流式返回，需要连同 `WorkerSession` trait、扫描重试和 checkpoint 边界一起设计。
- Rust 单元测试必须继续放在独立的 `ttl_worker_session_test.rs`，不要嵌回生产文件；本文件已有 AsterSQL 与 PingCAP 版权头，不得删除。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被识别为 275 行、18 个符号，并显示 8 个文件级使用者。
- RustCodeGraph 读取的生产路径：`pkg/session/runtime/ttl_worker_session.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/ttl_runtime.rs`、`pkg/session/runtime/ttl_timer.rs`、`pkg/ttl/ttlworker/session.rs`、`pkg/ttl/ttlworker/scan.rs`、`pkg/ttl/ttlworker/del.rs`、`pkg/ttl/ttlworker/persistent.rs`。
- Cargo 依据：`pkg/session/Cargo.toml` 中 crate 名为 `astersql-session`，声明本地依赖 `astersql-ttl-ttlworker`、`astersql-parser-mysql` 及时间依赖 `chrono`/`chrono-tz`。
- Go 对照：`pkg/ttl/ttlworker/session.go` 的 `withSession`、`prepareSession`、`NewScanSession`、`ttlTableSession::ExecuteSQLWithCheck` 与 `validateTTLWork`。
- 独立测试依据：Rust `pkg/session/runtime/ttl_worker_session_test.rs`；Go `pkg/ttl/ttlworker/session_test.go`。两者共同覆盖参数、生命周期、事务、元数据变化及会话变量恢复；Rust 文件额外提供真实 SQL和跨时区过期边界证据。
- RustCodeGraph 的 impl 方法 caller/callee 命令未返回可用的精确边；因此调用关系以索引文件内容配合 `rg` 定位的直接调用点复核，并在本文中限定为这些已见证路径。
- 本任务是纯文档分析，按计划未运行 Cargo、单元测试或构建。交付前仅执行任务规定的 11 章节结构检查，并人工复核本文没有把未见代码写成已支持能力。
