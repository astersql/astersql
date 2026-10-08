# `pkg/timer/tablestore/store.rs`

源文件：[store.rs](store.rs)。本文只描述当前 Rust 实现；同目录 Go 文件用于核对移植语义，不把 Go 已有能力自动视为 Rust 已支持。

## 文件定位

该文件实现 `astersql-timer-tablestore` crate 的表存储后端。`lib.rs` 将 `store` 声明为私有模块后用 `pub use store::*` 再导出其公开项；`Cargo.toml` 的 `[lib] path = "lib.rs"` 确认它属于该库，而不是独立二进制。它位于 Timer API 与系统会话池之间：上层通过 `api::TimerStore` 使用 CRUD/Watch，底层通过 `astersql-session-syssession` 执行由 `sql.rs` 生成的 SQL。

当前生产接线可见于 `pkg/session/runtime/ttl_runtime.rs`：TTL 后台运行时以 `mysql.tidb_timers`、系统会话池及可选通知传输调用 `NewTableTimerStore`，随后把返回的 store 交给 `NewTimerRuntimeBuilder`。因此本文件不是未接线门面；它是 TTL Timer 的持久化适配层。

## 核心职责

- `NewTableTimerStore` 组装 `TableTimerStoreCore`，有 etcd 客户端时使用 `NewEtcdNotifier`，否则使用进程内 notifier。
- `impl api::TimerStoreCore for TableTimerStoreCore` 实现 Create、List、Update、Delete、Watch 和 Close，将 Timer 领域对象映射到表 SQL 与变更事件。
- `with_session` 隔离池化会话状态：操作前清理事务并切换 UTC，操作后再次回滚并恢复原时区。
- `with_index_merge` 在 List 期间按需临时启用 index merge，以支持标签相关的多值索引查询，之后恢复原来的关闭状态。
- `checkUpdateConstraints` 在写入前执行事件 ID、版本、时区和调度策略校验；`runInTxn` 为 Update 提供悲观事务边界。
- `SqlCell`、`SqlRow`、`SqlResult` 和 `decode_timer` 构成系统会话结果与 `api::TimerRecord` 之间的类型化解码层。

## 主要符号

- `SqlCell`：查询单元格的封闭类型集合，覆盖 NULL、字符串、字节、整数、布尔、时间戳和 JSON。它避免 store 直接依赖 Go 版的 `chunk.Row`。
- `SqlRow(Vec<SqlCell>)`：提供 `string`、`bytes`、`i64`、`u64`、`timestamp`、`json` 等按列读取方法。越界或类型不符统一经 `cell_error` 返回 `TimerError`；`i64` 接受可表示的 `U64` 和 `Bool`，`u64` 拒绝负数。
- `SqlResult { rows }`：`Session::ExecuteInternal` 返回值期望 downcast 的具体结果载体。
- `TableTimerStoreCore`：保存 `Arc<dyn syssession::Pool + Send + Sync>`、数据库名、表名及 `Arc<dyn TimerWatchEventNotifier>`。类型为 `pub(crate)`，外部通常只持有 `api::TimerStore`。
- `NewTableTimerStore(...) -> api::TimerStore`：公开构造入口。名称保留 Go 风格；crate 根会再导出它。
- `with_session`：取得池会话并保证事务/时区清理，且用 `catch_unwind` 保证 panic 前完成恢复尝试后再继续展开 panic。
- `with_index_merge`：读取 `@@tidb_enable_index_merge`，仅在原值为关闭时临时置 ON；`index_merge_is_disabled` 识别 `off/0/false`（忽略大小写和首尾空白）。
- `checkUpdateConstraints`：合并现有和待更新的调度策略字段后调用 `CreateSchedEventPolicy` 验证组合。
- `executeSQL`：把 `Vec<SqlArg>` 装箱为系统会话参数，调用 `ExecuteInternal`，并要求结果能 downcast 为 `SqlResult`。
- `runInTxn`：执行 `BEGIN PESSIMISTIC`，业务成功后 COMMIT；业务或 COMMIT 失败时尽力 ROLLBACK。
- `decode_timer`：按 `buildSelectTimerSQL` 约定的固定列序把至少 19 列解码为 `TimerRecord`。

## 执行流程

1. 构造时，`NewTableTimerStore` 根据可选 etcd 选择跨节点或内存通知器，把 pool、库表名和 notifier 放入 core，再通过 `TimerStore::from_core` 返回领域门面。
2. 每次 CRUD 通过 `with_session` 租用会话。回调先执行 `ROLLBACK`，读取 `@@time_zone`，再设置为 UTC；业务完成或报错后再次回滚并恢复原时区。
3. Create 拒绝预设 ID、非零 Version 和预设 CreateTime，调用 `TimerRecord::Validate`，构造并执行 INSERT，再读取 `@@last_insert_id`。取得 ID 后在同一个会话回调内发出 Create 通知。
4. List 在会话内进入 `with_index_merge`，构造 SELECT 并执行，另读 `@@global.time_zone`。每行由 `decode_timer` 解码；空或 `TIDB` 时区使用全局时区解析，扩展 JSON 恢复 tags、manual request 和 event extra。
5. Update 在 `BEGIN PESSIMISTIC` 后先读取 `EVENT_ID, VERSION, SCHED_POLICY_TYPE, SCHED_POLICY_EXPR`。记录不存在返回 `ErrTimerNotExist`；约束通过后执行 UPDATE 并提交。只有事务整体成功后才发 Update 通知。
6. Delete 执行 DELETE，再以 `ROW_COUNT()>0` 判断是否实际删除；仅实际删除时发 Delete 通知。
7. Watch 直接委托 notifier，Close 关闭 notifier；`WatchSupported` 恒为 true。

## 数据与状态

持久状态在数据库表中，本结构自身只保存共享会话池、库表名和通知器。记录的 SQL 列序是 `decode_timer` 的关键不变量：它要求至少 19 列，并读取索引 0..16 与 18；列 17 当前不参与对象构造。若修改 `sql.rs::buildSelectTimerSQL` 的选择列，必须同步此处解码次序与测试。

`TIMER_EXT` 是 JSON 聚合状态，由 `decode_timer_ext` 拆为 tags、manual 和 event；NULL 扩展列使用默认值。Data、EventData、SummaryData 的 NULL 都映射为空字节数组，多个时间列的 NULL 映射为 `None`。数据库数字 ID 被格式化为字符串，Enable 由非零 `i64` 判定。

时间处理有两层：会话 SQL 运算强制 UTC；记录自身时区通过 `api::TimerLocation` 保存，并由 `timestamp_in_location` 转换 Watermark、EventStart 和 CreateTime 的显示偏移。`timestamp_from_unix` 以当前整秒为基准构造时间戳，避免直接暴露具体时间类型的构造细节。

## 依赖与调用关系

上游直接生产调用是 `pkg/session/runtime/ttl_runtime.rs` 的 TTL runtime 初始化；`pkg/session/runtime/ttl_timer_store_test.rs` 也直接构造 store 并走完整 CRUD。crate 根 `pkg/timer/tablestore/lib.rs` 再导出公开符号。

下游分为三类：

- `crate::sql`：`buildInsertTimerSQL`、`buildSelectTimerSQL`、`buildUpdateTimerSQL`、`buildDeleteTimerSQL`、`decode_timer_ext` 和标识符缩进函数。
- `crate::notifier` 与 `astersql-timer-api`：通知器选择、TimerStore trait、领域记录/更新/条件、校验函数及领域错误。
- `astersql-session-syssession`：池化会话、内部 SQL 执行、不可复用标记和错误桥接。

`Cargo.toml` 中本文件实际使用的必选 crate 是 `astersql-session-syssession` 与 `astersql-timer-api`；同 manifest 还保留多项 optional 移植依赖，但不能仅凭声明认定本文件运行时使用它们。

RustCodeGraph 的文件节点确认 `store.rs` 含 53 个符号，并能定位上述源文件；对关键符号执行 callers/callees 查询时未返回可用边。因此调用关系同时以已索引的 `ttl_runtime.rs` 源码和精确符号引用搜索核实，不能把“图无输出”解释成“没有调用者”。

## 错误处理与边界

Create 的输入前置条件、List 的缺行/列、类型不匹配、整数溢出、负数转无符号、无法 downcast 的结果集、无记录 Update，以及版本/事件 ID 冲突都有显式错误。数据库与会话错误被转换为 `TimerError`；`session_error` 在池回调边界再包装成 `SessionError`。

`with_session` 的初始化失败会阻止业务回调。清理阶段的 ROLLBACK 或时区恢复失败不会覆盖已经产生的业务结果，而是调用 `AvoidReuse` 防止污染会话回池；如果 pool 没有执行回调，则把 pool 错误或 `session callback did not run` 返回给调用方。panic 会在清理尝试后由 `resume_unwind` 原样继续传播。

`with_index_merge` 只在原配置为关闭时恢复 OFF；恢复失败同样标记会话不可复用，不覆盖 List 的业务结果或 panic。它只明确识别 `off/0/false` 为关闭，其他文本按“未关闭”处理。

`runInTxn` 保留首要错误：业务错误时忽略 ROLLBACK 错误；COMMIT 错误时尝试 ROLLBACK 后返回 COMMIT 错误。通知没有可传播的返回值，因此代码不能证明接收端已观察到事件，只能证明调用了 notifier。

## 并发与资源生命周期

core 内的 pool 和 notifier 都以 `Arc` 共享，trait object 具备相应线程安全边界；单次操作使用池租出的 `&Session`，文件内没有全局可变状态。悲观事务只包围 Update 的“读取约束列—校验—写入”序列，避免版本和事件条件检查与写入分离。

池化会话是最重要的资源边界。`with_session` 在借用前后清事务、保存/恢复时区，`with_index_merge` 保存/恢复优化器开关；恢复失败通过 `AvoidReuse` 隔离污染。闭包以 `FnOnce` 约束，`operation.take().expect(...)` 保证池回调至多真正执行业务一次；若池异常地不调用回调，会返回错误。

Watch 通道的生命周期由 notifier 管理，`TableTimerStoreCore::Close` 是本文件提供的关闭入口。生产 TTL runtime 同时保存 store 和 pool；真实 SQL 测试在末尾依次关闭 store、关闭 pool，并验证关闭后的 pool 拒绝新租约。

## 与 Go 版本的对应关系

`pkg/timer/tablestore/store.go` 是逐函数语义基线：构造器、CRUD、更新约束、悲观事务、会话 UTC 隔离、Watch/Close 和通知时机均有直接对应。Rust 将 Go 的多个 `*WithSession` 小函数内联到 trait 方法，并用 `SqlCell/SqlRow/SqlResult` 代替 `chunk.Row` 与 record set drain。

需要关注的实现差异：

- Go 的 `executeSQL` 给 context 标注 `kv.InternalTimer` 并负责关闭/排空 record set；Rust 当前把传入的 `api::Context` 命名为 `_ctx`，依赖 syssession 的 `ExecuteInternal` 和 `SqlResult` downcast，未在本文件表达内部来源标记或 record set 关闭逻辑。
- Go List 通过 session context 直接读取/修改 index-merge 与全局时区；Rust 通过 SQL 系统变量完成同一目的，并在恢复失败时显式 `AvoidReuse`。
- Go 对无法解析的记录时区回退到 `timeutil.SystemLocation()`；Rust 先解析记录/全局时区，失败后调用 `api::parse_location("")`。两者是否在所有部署环境完全等价取决于 Timer API 的空字符串语义，本文不扩大为已验证等价。
- Go 的 deferred 清理与 Rust 的 `catch_unwind` 都保留业务错误/panic；Rust 还显式处理“池未执行回调”的异常边界。
- Go Update 接收指针且在函数内没有显式 nil 检查；Rust Update 明确把 `None` 转为 `update should not be nil`。

## 扩展指南

- 新增或调整表字段：先改 `sql.rs` 的 SELECT/INSERT/UPDATE 构造与扩展 JSON，再同步 `decode_timer` 固定列序、`SqlCell` 类型转换和 `sql_test.rs`；避免只改解码端。
- 新增 Update 条件：接入 `checkUpdateConstraints`，并确保条件读取与 UPDATE 仍在同一 `runInTxn` 悲观事务内；同步 Go 语义及错误常量。
- 修改会话变量：在 `with_session` 或 `with_index_merge` 中成对保存/恢复，覆盖成功、业务错误、panic 和恢复失败；恢复失败必须阻止会话复用。
- 修改通知行为：保持 Create 在拿到新 ID 后、Update 在成功提交后、Delete 在 `ROW_COUNT()>0` 后通知，评估 etcd 与内存 notifier 的一致性和重复事件风险。
- 修改时间处理：同时检查 UTC 会话设置、`@@global.time_zone`、`TIDB` 兼容值、无效时区回退以及三个时间字段的 location 转换。
- 测试必须放在独立文件，不能内嵌到 `store.rs`。近邻入口是 `store_test.rs`（当前仅 index-merge 文本解析）和 `sql_test.rs`（会话清理、事务及 SQL mock）；真实 SQL CRUD/Watch 回归位于 `pkg/session/runtime/ttl_timer_store_test.rs`。还应参考 Go 的 `sql_test.go` 与 `pkg/timer/store_intergartion_test.go` 保持迁移意图。
- 性能风险集中在 List 的结果全量解码、额外系统变量查询和标签查询所需 index merge；兼容风险集中在列序、时区回退、可接受的单元格类型和错误文本。

## 验证依据

- RustCodeGraph：`status` 显示索引含 `pkg/timer/tablestore` 的 10 个 Go/Rust 文件；读取了 `store.rs` 全部 554 行、`lib.rs`、`store_test.rs`、`sql_test.rs` 相关区段、`ttl_runtime.rs` 生产调用区段和 `ttl_timer_store_test.rs`。查询了 `NewTableTimerStore`、`checkUpdateConstraints`、`executeSQL`、`runInTxn`、`TableTimerStoreCore`；关键 callers/callees 查询无可用输出，故未将其当作否定证据。
- crate/模块边界：读取 `pkg/timer/tablestore/Cargo.toml` 和 `pkg/timer/tablestore/lib.rs`。
- Go 对照：通过 RustCodeGraph 读取 `pkg/timer/tablestore/store.go` 全文及 `sql_test.go` 的会话/事务测试区段；精确搜索确认 `pkg/timer/store_intergartion_test.go` 存在构造器集成用例。
- Rust 测试：`pkg/timer/tablestore/store_test.rs::index_merge_state_matches_tidb_boolean_values` 验证关闭值识别；`pkg/timer/tablestore/sql_test.rs::test_run_in_txn` 及相邻 `with_session` 用例验证事务顺序、错误保留、panic 和不可复用标记；`pkg/session/runtime/ttl_timer_store_test.rs::go_merge_43_ttl_table_timer_store_real_sql_roundtrip` 验证真实 SQL CRUD、二进制/时间解码、三类通知及关闭生命周期。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前另运行任务指定的 11 章节结构命令，并人工复核路径、符号、调用边和未验证边界。
