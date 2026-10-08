# [`pkg/resourcegroup/runaway/record.rs`](record.rs)

## 文件定位

`record.rs` 属于 `astersql-resourcegroup-runaway` crate；crate 入口 `pkg/resourcegroup/runaway/lib.rs` 以 `pub mod record` 导出它。它位于 runaway 判定与系统表之间的数据/持久化边界：用 Rust 类型表示失控查询日志和 quarantine watch（隔离监视）记录，并生成交给 `RestrictedSqlExecutor` 执行的参数化 SQL。本文件自身不负责判定查询是否失控、不维护 watch 内存索引，也不创建后台刷盘循环。

该 crate 的 `Cargo.toml` 以 `lib.rs` 为库入口，并通过 `[package.metadata.porting] go-package = "pkg/resourcegroup/runaway"` 声明 Go 对照包。当前依赖只列在 `cfg(windows)` 目标段；本文件直接使用的执行器、动作/匹配枚举、时间戳和统一错误类型均由 crate 根提供，而不是直接引用外部 crate。

## 核心职责

1. `Record`、`RecordKey` 与 `genRunawayQueriesStmt` 描述并批量编码 `mysql.tidb_runaway_queries` 的写入数据。`RecordKey` 只包含资源组、SQL digest、计划 digest 和匹配类型，供上层合并重复记录；真正的重复计数存放在 `Record::Repeats`。
2. `QuarantineRecord` 描述 `mysql.tidb_runaway_watch` 的行，统一生成单条/批量插入、按 ID 删除，以及迁入 `mysql.tidb_runaway_watch_done` 所需的参数。
3. `handleRunawayWatchDone` 用同一个 `ExecutorRef` 依次执行 `begin`、插入 done 表、删除 watch 表和 `commit`，使正常写入路径中的“归档后删除”处于一个事务中。
4. `SqlValue` 是该子系统的受限 SQL 参数值类型，也被 `syncer.rs` 用作系统表结果行的列值表示，因此它同时连接写入和读取两侧。

当前 Rust 接线并不等同于 Go：`QuarantineRecord::genInsertionStmt` 和 `handleRunawayWatchDone` 已由 `manager.rs` 的公开管理操作调用；`genRunawayQueriesStmt`、`genBatchInsertWatchStmt`、`genBatchDeleteWatchByIDStmt` 在生产 Rust 文件中尚无调用者，只在 `record_test.rs` 中得到结构验证。Rust `Manager` 目前只暴露三个 drain 方法交出队列，尚未移植 Go `RunawayRecordFlushLoop` 对三个批量构造器的完整接线。

## 主要符号

- `MAX_ID_RETRIES: usize = 3`：供 `manager.rs::AddRunawayWatch` 获取最近插入 ID 时限定尝试次数；重试策略本身不在本文件。
- `RUNAWAY_WATCH_FULL_TABLE_NAME`、`RUNAWAY_WATCH_DONE_FULL_TABLE_NAME`：watch 与 done 系统表的全名，同时由 `syncer.rs::Syncer::new` 构造读取器。
- `SqlValue::{Null, Int, UInt, Text, Time}`：执行器参数/结果的最小值集合；`Time` 内含 crate 的微秒 Unix 时间戳别名 `Timestamp = i64`。
- `Record`：失控查询日志的十列数据模型。`Default` 会令字符串为空、数值为 0；常规生产者 `manager.rs::markRunaway` 显式将 `Repeats` 初始化为 1。
- `RecordKey` 及 `From<&Record>`：克隆四个参与去重的字段；`Action`、样例 SQL、来源、超限原因、起始时间和重复次数不参与键相等性。
- `genRunawayQueriesStmt(&HashMap<RecordKey, Record>)`：每条记录生成 10 个 `%?` 占位符，并按系统表列顺序追加参数。
- `QuarantineRecord`：包含原 watch 行 ID、资源组、起止时间、匹配类型/文本、来源、原因、动作及 SwitchGroup 目标。`EndTime == 0` 表示永不过期。
- `QuarantineRecord::getRecordKey`：返回 `资源组/匹配文本`，由 `manager.rs` 的内存 watch 表增删与查重使用；匹配类型没有进入该键。
- `getSwitchGroupName` / `GetActionString` / `GetExceedCause`：分别规范化持久化目标组、生成面向记录的动作文本、暴露超限原因。只有 `SwitchGroup` 会返回目标组或在动作文本中附加 `(目标组)`。
- `genInsertionStmt`、`genInsertionDoneStmt`、`genDeletionStmt`：生成单条 watch 插入、done 插入和 watch 删除。done 参数在九个公共 watch 参数之前增加原 ID，之后增加 `nowMicros()` 产生的完成时间。
- `watchParams`：私有的九列编码中心；将零 `EndTime` 写成 `SqlValue::Null`，并把 `RunawayWatchType`、`RunawayAction` 按当前枚举判别值转为 `i64`。
- `genBatchInsertWatchStmt` / `genBatchDeleteWatchByIDStmt`：分别按记录值批量插入、按 ID 集合批量删除。
- `handleRunawayWatchDone`：迁移 watch 到 done 表的事务编排函数，返回 crate 统一的 `Result<()>`。

文件没有条件编译项、trait 或异步函数；公开 API 保留了与 Go 相近的大小写命名，crate 根通过 `allow(non_snake_case)` 接受这些名称。

## 执行流程

失控查询日志路径的当前数据流是：`checker.rs` 触发管理器记录逻辑，`manager.rs::markRunaway` 组装 `Record` 并放入 `runaway_records` 有界队列，`drainRunawayRecords` 将队列整体取出。若上层要落表，应以 `RecordKey::from(&record)` 合并同键记录、累计 `Repeats`，再调用 `genRunawayQueriesStmt` 并把 SQL/参数交给执行器。最后两步正是 Go `RunawayRecordFlushLoop` 的行为，但在当前 Rust 生产代码中尚未接线，不能把队列 drain 等同于已经写入系统表。

新增 watch 的路径是：`manager.rs::markQuarantine` 建立 `QuarantineRecord`，先加入内存 watch 列表，再放入 `quarantine_records`；手动添加则由 `Manager::AddRunawayWatch` 直接调用 `genInsertionStmt` 和 `ExecutorRef::Execute`，随后最多读取三次 `LastInsertId`。批量 drain 后调用 `genBatchInsertWatchStmt` 的路径目前同样只具备构造器，没有 Rust 生产调用点。

移除 watch 时，`Manager::RemoveRunawayWatch` 或 `RemoveRunawayResourceGroupWatch` 先经 `Syncer` 从 watch 表读出完整记录，再调用 `handleRunawayWatchDone`：

1. 由 `genInsertionDoneStmt` 捕获当前 `nowMicros()`，准备原 ID、watch 九列和完成时间。
2. 执行 `begin`；失败则立即通过 `?` 返回。
3. 插入 done 表；失败时尽力执行 `rollback`，忽略 rollback 结果并返回原插入错误。
4. 删除 watch 表原 ID；失败时同样回滚并返回原删除错误。
5. 尝试 `commit`，忽略 commit 的成功或失败，并返回 `Ok(())`。这是为复现 Go 未命名返回值配合 defer 的既有可观察行为，测试明确锁定了这一点。

系统表同步是反向路径：`syncer.rs` 使用同两个表名扫描行，以 `SqlValue` 解码回 `QuarantineRecord`，再由管理器添加或移除本地 watch。

## 数据与状态

本文件的数据结构均为拥有所有权的值类型，SQL 构造时会克隆字符串到 `Vec<SqlValue>`；生成结果不借用输入，可在调用结束后独立交给执行器。批量函数接受 `HashMap`，所以 SQL 中记录顺序和参数顺序彼此一致，但不同运行之间没有稳定排序保证；调用方和测试不应依赖批次内行序。

必须保持的列不变量包括：runaway query 每行恰好 10 个参数；watch 每行恰好 9 个公共参数；done 每行是原 ID + 9 个公共参数 + done time，共 11 个参数。`SwitchGroupName` 对非 `SwitchGroup` 动作强制写空串。`EndTime == 0` 与数据库 `NULL` 双向对应：本文件负责写侧，`syncer.rs::SqlRow::nullable_time` 负责读侧。

`RecordKey` 的四字段定义决定重复合并语义。改变它会影响 `Repeats` 的聚合粒度，必须与 Go `recordKey`、刷盘 merge 逻辑和测试同步。类似地，`getRecordKey` 当前没有包括 `Watch` 类型；相同资源组和匹配文本会竞争同一内存键，这是 `manager.rs::addWatchList` 的覆盖/冲突判断基础。

空 `HashMap` 是重要调用边界：三个批量函数不会主动拒绝空输入，分别会形成只有 `VALUES ` 的插入 SQL或 `id in ()` 的删除 SQL。当前设计依赖上层 flusher 只在非空缓冲区调用它们；若引入新的调用者，应在上层维持这一前置条件或为构造器增加显式空批处理策略。

## 依赖与调用关系

上游生产调用关系如下：

- `manager.rs::markRunaway`、`markQuarantine` 生产 `Record` / `QuarantineRecord`；`addWatchList`、`removeWatch` 和 `examineWatchList` 使用 quarantine 记录键、目标组和原因。
- `Manager::AddRunawayWatch` 调用 `QuarantineRecord::genInsertionStmt`；`RemoveRunawayWatch` 与 `RemoveRunawayResourceGroupWatch` 调用 `handleRunawayWatchDone`。
- `syncer.rs` 引用两个表名和 `SqlValue`，并把查询行解码为 `QuarantineRecord`。
- `lib.rs::RestrictedSqlExecutor` 的 `Execute(&str, &[SqlValue])` 是本文件所有 SQL 最终执行的抽象边界；`ExecutorRef` 是其共享引用。

下游依赖包括标准库 `HashMap`，以及 crate 根的 `ExecutorRef`、`Result`、`RunawayAction`、`RunawayWatchType`、`Timestamp`、`nowMicros`。本文件不直接持有 session、连接或事务对象。

RustCodeGraph 将 `record.rs` 标记为被 `record_test.rs`、`syncer_test.rs` 直接使用；精确 `callers` 对自由函数未生成跨文件边，因此又以符号搜索核验生产引用。搜索结果确认三个批量构造器没有 Rust 生产调用点，而 Go `manager.go::RunawayRecordFlushLoop` 分别把它们绑定到 runaway、quarantine 和 stale 三个 `batchFlusher`。

## 错误处理与边界

纯 SQL 构造函数不返回错误，也不验证字段长度、枚举合法性或空批次；数据库类型/约束错误由执行器层报告。枚举通过 `as i64` 持久化，因而枚举判别值与 Go/protobuf 及 `syncer.rs::decodeQuarantineRecord` 的数值映射构成兼容约束，不能随意重排。

`handleRunawayWatchDone` 对 begin、insert、delete 的错误采取不同阶段处理：begin 失败直接返回；insert/delete 失败尝试回滚但保留原错误；commit 与 rollback 的错误均被忽略。由此存在一个刻意保留的边界：commit 实际失败时调用方仍收到成功。修改这一行为会改变 Go 对齐语义和现有回归测试，应作为显式兼容决策处理。

Go `GetActionString` 接受 nil receiver 并返回 `NoneAction`；Rust 方法要求有效的 `&self`，类型系统排除了 nil receiver。这是语言层面的差异，不是遗漏。时间上，Go 使用 UTC `time.Time`，Rust 使用微秒整数；`nowMicros` 基于 Unix epoch，系统时钟早于 epoch 时通过 `unwrap_or_default` 得到 0。

SQL 占位符采用 TiDB 内部执行器约定的 `%?`，不能未经执行器契约核验改成普通 `?`。表插入使用无列名的 `VALUES`，因此参数顺序与系统表物理列顺序强耦合；表结构增加/重排列时必须同步修改这些构造器和解码列索引。

## 并发与资源生命周期

本文件没有锁、线程、通道或后台任务；所有函数都同步执行。并发控制位于调用方：`Manager` 用 `Mutex` 保护队列、watch 列表和 `Syncer`，并以 `Arc` 共享执行器。`RestrictedSqlExecutor: Send + Sync` 允许 `ExecutorRef` 跨线程共享，但本文件不规定执行器内部的连接复用方式。

事务生命周期完全包在一次 `handleRunawayWatchDone` 调用中，不跨函数返回：成功路径 begin → insert → delete → commit；写失败路径以 rollback 收尾。函数没有 RAII transaction guard，且回滚/提交错误被忽略，所以调用方不能仅凭 `Ok(())` 证明数据库已经提交。

`HashMap` 的迭代只在借用期间发生，构造结果随后拥有全部参数。`genInsertionDoneStmt` 在构造阶段读取一次时钟，因而重试整个函数会产生新的 done time；同一函数内部插入与删除共享同一条 `QuarantineRecord` 的 ID。

## 与 Go 版本的对应关系

主体逐项对应 `pkg/resourcegroup/runaway/record.go`：Rust `Record` / `RecordKey`、十列 runaway insert、`QuarantineRecord`、九列 watch 参数、批量 insert/delete，以及 done 事务的 SQL 顺序均保留 Go 设计。`record_test.rs` 还内嵌了一段 Go 测试参考文本，但可执行的 Rust 测试位于文件后半部分，独立于生产源码。

已确认的语言适配包括：Go `time.Time{}` / `nil` 结束时间映射为 Rust `Timestamp == 0` / `SqlValue::Null`；Go `[]any` 映射为强类型 `Vec<SqlValue>`；Go 指针 map 映射为拥有值的 `HashMap`；Go protobuf 枚举映射为 crate 本地枚举。Rust 用 `From<&Record>` 集中构造复合键，而 Go 在刷盘循环中写结构体字面量。

迁移差异必须保留在文档视野内：

- Go `RunawayRecordFlushLoop` 已把三个批量函数接入周期/阈值刷盘，Rust 目前没有对应生产循环，只提供队列 drain 和独立 `BatchFlusher` 类型。
- Go `AddRunawayWatch` 在事务中插入、带退避执行 `SELECT LAST_INSERT_ID()` 并提交/回滚；Rust 管理器直接执行 insert 后调用抽象执行器的 `LastInsertId`，不属于本文件的事务逻辑。
- Go 的 done 处理从 session pool 获取/归还 session 并设置内部事务来源；Rust 接收已经构造好的 `ExecutorRef`，这些资源管理职责在抽象边界之外。
- Go 对移除资源组 watch 的错误增加上下文；Rust 当前直接传播统一错误。

## 扩展指南

新增或调整系统表字段时，应同时修改 `Record`/`QuarantineRecord`、对应 SQL 的列/占位符与参数顺序、`syncer.rs` 的 `QuarantineColumns` 和解码逻辑，并更新独立的 `record_test.rs`、`syncer_test.rs`；由于 insert 未列出列名，还必须核对实际系统表 DDL。新增 `SqlValue` 变体则要同步所有执行器适配和 `syncer.rs::SqlRow` 访问器。

调整去重策略时，修改点是 `RecordKey`、`From<&Record>` 以及未来/现有 flusher 的 merge 函数；应增加覆盖“哪些字段相同会累计 `Repeats`”的测试。调整 watch 冲突规则时，应同时检查 `getRecordKey` 和 `manager.rs::addWatchList`，尤其要决定匹配类型是否属于键。

若补齐 Rust 刷盘接线，应复用 `flusher.rs::BatchFlusher`，将 `drainRunawayRecords`、`drainQuarantineRecords`、`drainStaleRecords` 分别接入三个批量构造器，并保持 Go 的合并规则：runaway 同键累计次数、quarantine 同键保留首条、stale 按 ID 覆盖。还需明确空批次不调用 SQL 构造器、失败批次是否丢弃，以及 stop 时的最终 flush；测试仍应放在独立 `*_test.rs`，不要内嵌进生产文件。

改变 done 事务语义前，应扩展 `record_test.rs` 的捕获执行器用例，覆盖 begin、insert、delete、rollback、commit 各失败点，并明确是否继续吞掉 commit 错误。若要增强原子性，优先在执行器接口或事务 guard 层提供可验证的提交结果，而不是只改变 SQL 字符串。

性能风险集中在每批对所有字符串的克隆、无稳定顺序的 `HashMap` 展开以及过大批次生成的 SQL/参数体积；兼容风险集中在枚举数值、系统表列序、`%?` 占位符和 commit 错误行为。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter pkg/resourcegroup/runaway` 确认模块文件集合；`node --file pkg/resourcegroup/runaway/record.rs` 读取全部 265 行及 28 个符号；对 `genRunawayQueriesStmt`、`genBatchInsertWatchStmt`、`genBatchDeleteWatchByIDStmt`、`handleRunawayWatchDone` 执行了 callers/callees 查询。图能解析本文件内部调用，但部分自由函数 callers 为空，因此用精确符号搜索补证。
- 生产源码：`pkg/resourcegroup/runaway/record.rs`（目标实现）、`lib.rs`（模块导出、公共类型与执行器契约）、`manager.rs`（记录生产、队列、单条插入与移除接线）、`flusher.rs`（可复用批量缓冲器）、`syncer.rs`（表名消费、参数值读侧和记录解码）。该目录没有 `doc.go`。
- crate 边界：`pkg/resourcegroup/runaway/Cargo.toml`，确认库入口、Go 包映射和条件依赖声明。
- Go 对照：`pkg/resourcegroup/runaway/record.go`（数据模型、SQL、done 事务、手工增删），`manager.go::RunawayRecordFlushLoop`（三个批量函数的真实生产接线），`record_test.go`（复合键语义）。
- Rust 测试：`pkg/resourcegroup/runaway/record_test.rs` 验证 `RecordKey`/10 参数 runaway insert、watch 的 9 参数批量插入、按 ID 删除、done 四步顺序、commit 错误吞掉，以及 insert/delete 错误触发 rollback；`syncer_test.rs` 为 `SqlValue` 和 watch/done 解码提供相邻读侧证据。
- 生产引用精确搜索确认：`genRunawayQueriesStmt`、`genBatchInsertWatchStmt`、`genBatchDeleteWatchByIDStmt` 目前只出现在目标文件和 Rust 测试；`manager.rs` 生产调用 `getRecordKey`、`genInsertionStmt` 和 `handleRunawayWatchDone`。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验收使用任务文件规定的 11 个固定二级标题检查，并人工复核源码链接、事实边界与扩展建议。
