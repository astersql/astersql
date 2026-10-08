# `pkg/statistics/handle/util/util.rs`

## 文件定位

该文件位于 `astersql-statistics-handle-util` crate 中，由同目录 `lib.rs` 的 `pub mod util` 装配并通过 `pub use util::*` 对外导出。它不是统计估算算法本身，而是统计 Handle 访问内部会话、事务和受限 SQL 的公共适配层，同时承载统计会话需要刷新的系统变量、统计历史来源常量、时间戳换算和特殊全局索引判定等小型公共能力。

`pkg/statistics/handle/util/Cargo.toml` 表明当前可编译依赖只有 `astersql-sessionctx-vardef` 和 `astersql-testkit-testfailpoint`；大量 Go 侧真实会话、元数据和 SQL 执行依赖仍列在 `cfg(any())` 下。因此，本文件用本地 trait 和轻量 DTO 表达这些边界。当前 Rust 生产接线可见于 `pkg/statistics/handle/cache/statscache.rs`，DDL 事务路径的直接使用位于 `pkg/statistics/handle/ddl/testutil/util.rs`；`pkg/statistics/handle/types/interfaces.rs` 还会再导出其中的执行接口和类型。

## 核心职责

1. 用 `SessionContext`、`SessionPool`、`SqlExecutor`、`RestrictedSqlExecutor`、`Transaction` 和 `GlobalVariableAccessor` 抽象 Go 版本依赖的真实会话设施。
2. `update_sctx_vars_for_stats` 在每次借用池化会话时刷新会影响统计行为的全局变量，避免自动分析等长生命周期内部会话保留旧配置。
3. `call_with_sctx` 统一“借会话—刷新变量—可选事务—执行回调—归还会话”的控制流，并按 Go `util.Recover` 的现有语义吞掉普通 panic。
4. `exec`/`exec_with_ctx` 和 `exec_rows`/`exec_rows_with_ctx`/`exec_with_opts` 分别封装流式内部 SQL 与物化受限 SQL；`ExecRowsTimeout`、`EXEC_ROWS_TIMEOUT`、`IN_TEST` 提供与 Go failpoint/测试 mock 对齐的注入点。
5. 提供统计元历史来源常量、`duration_to_ts`、`is_special_global_index` 等被统计存储、自动分析和 DDL 逻辑消费的通用判断或编码规则。

## 主要符号

- `StatsError`：统一表示全局变量读取、变量解析、SQL、事务和会话池关闭错误；`Display` 保留错误类别及关键变量值。
- `ExecutionContext`、`InternalSourceType` 与静态 `STATS_CONTEXT`：标记统计前台优先的内部请求。`ExecOption::UseCurrentSession` 和 `USE_CURRENT_SESSION_OPTIONS` 强制受限 SQL 复用当前会话。
- `SqlValue`、`Row`、`ResultField`、`RecordSet`：内部 SQL 参数、物化结果和流式结果的轻量边界类型。
- `SessionVariables`：保存全局变量访问器和刷新后的统计会话快照；布尔/整数使用原子量，字符串及列表使用 `RwLock`。公开 getter 返回标量或克隆值，不向调用方暴露锁守卫。
- `SessionContext` 与 `SessionPool`：前者提供变量、事务、SQL 执行器、系统变量设置和时区位置；后者通过 `with_session` 将归还策略留给具体池实现。
- `update_sctx_vars_for_stats`、`call_with_sctx`、`get_current_prune_mode`：池化会话刷新和使用的主入口。
- `wrap_txn`、私有 `finish_transaction`、`get_start_ts`：悲观事务包装、提交/回滚和事务版本读取。
- `exec`、`exec_with_ctx`、`exec_rows`、`exec_rows_with_ctx`、`exec_with_opts`：内部 SQL 执行入口；其中 `exec_with_opts` 只接受选项，执行上下文固定为 `STATS_CONTEXT`。
- `duration_to_ts`：把毫秒物理时间编码到 TiDB TSO 的高位（左移 18 位）。
- `IndexInfo`、`IndexColumn`、`TableInfo`、`ColumnInfo` 与 `is_special_global_index`：以最小元数据形状识别含虚拟生成列或前缀列的全局索引。
- `STATS_META_HISTORY_SOURCE_*`：记录 ANALYZE、加载、刷盘、schema 变更和扩展统计这五类统计历史来源。

## 执行流程

### 借用统计会话

`call_with_sctx` 先把 `FnOnce` 回调放入 `Option`，再交给 `SessionPool::with_session`。池调用内部闭包后，流程依次为：

1. `update_sctx_vars_for_stats` 取得 `SessionVariables` 并逐项读取全局变量。
2. 布尔值经私有 `option_on` 识别 `"1"` 或不区分大小写的 `"ON"`；整数经 `parse_i64` 转换。
3. `tidb_analyze_store_batch_size` 和 `time_zone` 通过 `SessionContext::set_system_variable` 写入真实会话；随后把 `location()` 同步到 `statement_time_zone`。
4. 若 `flags` 含 `FLAG_WRAP_TXN`，转入 `wrap_txn`；否则直接运行调用方回调。
5. 外层 `catch_unwind` 将池或回调产生的 panic 转为 `Ok(())`；普通 `Result::Err` 保持传播。`FnOnce` 只允许取出一次，若池重复调用闭包会触发明确 panic，该 panic 同样被外层恢复逻辑吞掉。

`get_current_prune_mode` 是该流程的窄封装：在回调中读取刷新后的 `partition_prune_mode`，通过共享 `RwLock<String>` 带出结果。

### 事务包装

`wrap_txn` 先经 `exec_rows` 执行 `BEGIN PESSIMISTIC`；开始失败时不会执行回调。开始成功后执行回调，并把结果交给 `finish_transaction`：成功则执行大写 `COMMIT`，提交失败会成为最终错误；回调失败则尽力执行小写 `rollback`，忽略回滚错误并返回原始回调错误。这个顺序保证主要业务错误不会被清理错误覆盖。

### SQL 执行

`exec` 把 `STATS_CONTEXT` 传给 `exec_with_ctx`，后者调用 `SqlExecutor::execute_internal` 并返回流式 `RecordSet`。`exec_rows` 则先执行同名 Go failpoint 对应的 `ExecRowsTimeout`，再检查进程内原子测试开关，任一命中都会在访问执行器之前返回 `StatsError::Sql("inject timeout error")`；正常情况下转入 `exec_rows_with_ctx`。

`exec_rows_with_ctx` 在 `IN_TEST` 开启且会话提供 mock 执行器时，刻意忽略调用方传入的执行上下文并使用 `STATS_CONTEXT`，与 Go 测试钩子保持一致；其他情况使用真实受限 SQL 执行器及调用方上下文。两条分支都固定传入 `USE_CURRENT_SESSION_OPTIONS`。`exec_with_opts` 则让调用方选择选项，但固定使用 `STATS_CONTEXT`。

### 特殊全局索引判断

`is_special_global_index` 对非全局索引立即返回 `false`；全局索引逐列读取 `table.columns[index_column.offset]`，只要列为虚拟生成列，或索引长度不等于 `UNSPECIFIED_LENGTH`（即前缀索引），就返回 `true`。函数依赖上游保证列偏移有效，不自行做边界校验。

## 数据与状态

`SessionVariables` 是本文件最主要的可变状态。布尔值和整数通过 Acquire/Release 原子读写；分区剪枝模式、跳过列类型、会话时区和语句时区通过 `RwLock` 更新。刷新并非事务式批量提交：`update_sctx_vars_for_stats` 按固定顺序逐项写入，若中途失败，先前字段已经更新，后续字段保持旧值。调用方只能在整个刷新成功后进入业务回调，因此不会通过 `call_with_sctx` 消费这次半完成刷新，但共享同一 `SessionVariables` 的其他并发读者可能观察到逐项更新过程。

`EXEC_ROWS_TIMEOUT` 与 `IN_TEST` 是进程级 `AtomicBool` 测试开关；`set_exec_rows_timeout` 和 `set_in_test` 会影响所有使用该 crate 实例的并发调用。`STATS_CONTEXT`、历史来源字符串、系统变量名、`FLAG_WRAP_TXN` 和 `UNSPECIFIED_LENGTH` 均为只读静态数据。

`duration_to_ts` 先把 `Duration::as_millis()` 截为不超过 `u64::MAX` 的值，再左移 18 位。它表达 TSO 的“物理毫秒 + 18 位逻辑部分”布局；调用方应使用能在左移后仍落入 `u64` 物理位范围的时长。当前 Rust 统计 GC 文件有自己的私有同名实现，而本文件这个公开函数未在非测试 Rust 搜索结果中发现直接调用。

## 依赖与调用关系

- crate 装配：`pkg/statistics/handle/util/lib.rs` 公开本模块全部符号；根 workspace 还通过 facade 再导出该 crate。
- 上游 Rust：`pkg/statistics/handle/cache/statscache.rs` 用 `call_with_sctx` 和 `exec_rows` 刷新 `mysql.stats_meta` 缓存；`pkg/statistics/handle/ddl/testutil/util.rs` 用 `call_with_sctx(..., &[FLAG_WRAP_TXN])` 在事务内处理 DDL 事件；`pkg/statistics/handle/types/interfaces.rs` 将会话、执行器、DTO 和 `exec_rows` 重新导出为统计 Handle 接口。
- 共享 failpoint：`pkg/statistics/handle/restricted_sql.rs` 也调用 `ExecRowsTimeout`，因此 Go 同名 failpoint 能覆盖该文件和独立 restricted-SQL 实现。
- 下游 Rust：系统变量常量部分来自 `astersql-sessionctx-vardef` 的再导出；failpoint 求值来自 `astersql-testkit-testfailpoint`。其余会话/SQL/事务依赖均经本文件 trait 反转注入。
- Go 主链：同路径 `util.go` 被统计存储、缓存、历史、全局统计、锁统计、usage、DDL 和自动分析代码广泛调用。Rust 当前不能据此宣称所有 Go 调用点已接线；这些 Go 文件只证明目标语义和未来接入位置。
- RustCodeGraph 对精确符号确认了 `call_with_sctx -> update_sctx_vars_for_stats`、`call_with_sctx -> wrap_txn`、`exec_rows -> exec_rows_with_ctx`、`get_current_prune_mode -> call_with_sctx` 等文件内调用边。索引对跨 crate 泛型/再导出调用覆盖不完整，实际 Rust 上游另以精确文本搜索核验。

## 错误处理与边界

- 全局变量访问器返回的错误直接传播；整数格式错误转换为带变量名和值的 `StatsError::ParseVariable`。布尔值除 `1`/`ON` 外一律视为关闭，不返回解析错误。
- `tidb_analyze_skip_column_types` 先整体转小写，再按英文逗号切分，只保留固定白名单；不会去除空白，也不会为未知类型报错。这与 Go `ParseAnalyzeSkipColumnTypes` 的防御性过滤意图一致。
- 锁等待秒数使用 `saturating_mul(1000)` 转为毫秒，避免 Rust 整数溢出 panic；这是相对 Go 直接乘法更明确的饱和边界。
- `call_with_sctx` 只区分“包含 `FLAG_WRAP_TXN`”与否；因为该标志值为 `0`，任何包含零的 flags 都开启事务。panic 被恢复为成功会丢失 panic 内容，调用方不能把 panic 当作可观察错误通道。
- `finish_transaction` 保留回调原始错误并丢弃 rollback 错误；成功路径上的 COMMIT 错误则传播。没有检测会话是否已经处于事务中，Go 源码也保留了同一 TODO。
- `is_special_global_index` 直接按 offset 索引列数组；越界会 panic。安全扩展时应维持“元数据已验证”的前置条件，或同时调整 Go/Rust 行为与独立测试，不能只在一侧静默容错。
- `RwLock::read/write().unwrap()` 在锁中毒时会 panic，随后是否被吞掉取决于调用是否处于 `call_with_sctx` 的恢复边界内。

## 并发与资源生命周期

`SessionVariables` 可被 `Arc` 共享：标量用原子操作避免互斥，复合值用 `RwLock` 保证内存安全；getter 克隆复合值，使锁只在复制期间持有。Acquire/Release 提供单字段发布与读取顺序，但没有跨多个字段的一致快照保证。

会话资源生命周期由 `SessionPool::with_session` 实现负责。本文件不直接取出或归还对象，而是让池围绕闭包保证归还；`util_test.rs::test_call_sctx_failed` 验证回调返回错误时 fake 池仍完成释放。事务生命周期由 `wrap_txn` 管理：BEGIN 成功后必定选择 COMMIT 或 rollback；若回调 panic，则 panic 会越过 `wrap_txn`，被外层 `call_with_sctx` 捕获，此路径不会执行 `finish_transaction`，因此具体会话池/数据库连接需要承担异常清理。

两个全局测试开关没有作用域守卫，测试若并行修改必须自行串行化并恢复原值；同名 failpoint 的 `enable` 守卫则由测试工具在离开作用域时清理。文件本身不创建线程、异步任务或通道。

## 与 Go 版本的对应关系

Rust 文件逐项复刻同目录 `util.go` 的核心语义：历史来源常量、`StatsCtx`/当前会话选项、`CallWithSCtx`、变量刷新、事务包装、start TS、内部/受限 SQL、时长转 TSO 以及特殊全局索引判定均有对应符号。

主要表示差异如下：

- Go 直接依赖 `sessionctx.Context`、`syssession.Pool`、`sqlexec`、`model.TableInfo`；Rust 用本地 trait 和 DTO 解耦。这与 Cargo 中真实依赖仍被放在 `cfg(any())` 下的迁移状态一致。
- Go 的可变 `SessionVars` 字段在 Rust 中拆为原子量与锁；Rust 返回克隆的字符串/列表，而非共享可变引用。
- Go `ExecRowsWithCtx` 内部执行 failpoint；Rust `exec_rows` 执行 failpoint，而 `exec_rows_with_ctx` 本身不执行。因此直接调用 Rust `exec_rows_with_ctx` 会绕过超时注入，调用约定与 Go 的同名函数存在这一可观察差异。
- Go 还提供 `ExecWithOptsWithCtx(ctx, ...)`；当前 Rust 只有固定 `STATS_CONTEXT` 的 `exec_with_opts`，没有“自定义 context + 自定义 options”组合入口。
- Go `DurationToTS` 使用 `oracle.ComposeTS`；Rust用毫秒左移 18 位表达相同布局，但其超大时长截断边界与 `ComposeTS` 并未由当前独立 Rust 测试逐项证明。
- Go 测试通过真实 mock store/domain 建表和检查内部会话释放；Rust `util_test.rs` 用 fake trait 对象复现目标分支。`test_is_special_global_index`、`test_call_sctx_failed`、`test_call_with_sctx_recovers_panic`、时区同步、failpoint 和 skip-column 白名单测试覆盖了本文件关键契约，但没有完整真实 TiKV/会话集成证明。

## 扩展指南

- 新增影响统计行为的系统变量时，应在 `update_sctx_vars_for_stats` 中沿 Go 顺序读取和转换，在 `SessionVariables` 增加并发安全存储及 getter，并同步 `util_test.rs::default_global_vars` 和错误/边界测试。若变量应调用真实 session 校验，使用 `set_system_variable`，不要只写缓存。
- 新增 SQL 执行入口时，应明确选择流式还是物化结果、是否允许自定义 `ExecutionContext`、是否必须复用当前会话，以及测试 mock/failpoint 是否应覆盖该入口。尤其不要无意绕过 `ExecRowsTimeout`。
- 修改事务语义时，应分别覆盖 BEGIN 失败、回调失败且 rollback 失败、COMMIT 失败和 panic；生产测试逻辑继续放在独立 `util_test.rs`，不要嵌入源文件。
- 扩展 `StatsError` 时保持 `Display` 带足上下文，并让 trait 实现能保留原始错误类别；不要让清理错误覆盖主要业务错误，除非 Go 对照行为也改变。
- 修改特殊全局索引规则时，同步 `IndexInfo`/`TableInfo` 的最小元数据表示、Go `IsSpecialGlobalIndex` 和 `util_test.rs::test_is_special_global_index` 的普通全局列、非全局表达式、虚拟生成列及前缀列矩阵。
- 若把 `cfg(any())` 中的真实依赖接回生产 crate，应逐步以真实 AsterSQL 类型替换本地 DTO/trait，并核对 `pkg/statistics/handle/types/interfaces.rs` 的再导出和所有依赖 crate；这属于跨文件迁移，不应在本文件文档任务中假定已完成。
- 性能风险主要在每次借会话逐项访问全局变量和字符串/列表克隆；兼容风险集中在 SQL 执行 context、panic 恢复和事务错误优先级，调整前应以 Go 行为及调用方需求共同验证。

## 验证依据

- 目标源码：`pkg/statistics/handle/util/util.rs`，完整核对 606 行源码及公开/私有符号。
- crate 与装配：`pkg/statistics/handle/util/Cargo.toml`、`pkg/statistics/handle/util/lib.rs`、`pkg/statistics/handle/types/interfaces.rs`。
- Rust 直接调用证据：`pkg/statistics/handle/cache/statscache.rs`、`pkg/statistics/handle/ddl/testutil/util.rs`、`pkg/statistics/handle/restricted_sql.rs`。
- Go 对照：`pkg/statistics/handle/util/util.go`；Go 独立测试：`pkg/statistics/handle/util/util_test.go`。调用面通过统计目录中 `CallWithSCtx`、`ExecRowsWithCtx`、`GetStartTS`、`DurationToTS`、`IsSpecialGlobalIndex` 等精确搜索确认。
- Rust 独立测试：`pkg/statistics/handle/util/util_test.rs`，覆盖特殊索引判定、回调错误时释放会话、panic 恢复、每次调用刷新语句时区、超时 failpoint、skip-column 白名单和废弃 merge concurrency 不再读取。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/statistics/handle/util` 确认 Rust/Go/测试文件；`node --file ... --offset ...` 阅读目标源码；精确 `node` 查询确认 `update_sctx_vars_for_stats`、`call_with_sctx`、`exec_rows_with_ctx`、`is_special_global_index`、`duration_to_ts` 的定义及文件内调用边。
- 本任务为纯文档分析，按计划不运行 Cargo；最终以任务指定的 11 个固定二级标题检查和人工事实复核为验收依据。
