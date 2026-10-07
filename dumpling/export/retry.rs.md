# `dumpling/export/retry.rs`

源码：[retry.rs](./retry.rs)；Go 对照：[retry.go](./retry.go)。

## 文件定位

`retry.rs` 属于 `astersql-dumpling-export` library crate。crate 入口 [lib.rs](./lib.rs) 通过 `include!("retry.rs")` 将它并入与 Go `dumpling/export` 包相近的单包作用域；因此文件本身没有 `use` 或 `mod` 声明，`Duration`、`HashMap`、`tcontext`、`Field` 和桩类型均来自 `lib.rs` 的共享导入或 [stubs.rs](./stubs.rs)。[Cargo.toml](./Cargo.toml) 将该 crate 标记为 `kind = "library"`、Go 包为 `dumpling/export`，并直接依赖 dumpling 的 `cli/context/log` 等本地 crate。

本文件不是通用重试执行器，而是为导出流程提供两组领域策略：`BaseConn` 的连接重建重试，以及 `LOCK TABLES` 遇到表消失时的收敛重试。真正驱动循环的是 [stubs.rs](./stubs.rs) 中的 `WithRetry`，或 [consistency.rs](./consistency.rs) 中的显式循环。

## 核心职责

- 用 `newRebuildConnBackOffer` 在“允许重建连接”和“禁止重试”之间选择统一的 `backOfferResettable` 接口，避免 `BaseConn` 的查询与执行路径分叉。
- 用 `dumpChunkBackoffer` 维护三次重试预算和有上限的指数退避状态；已知的不可重试 MySQL 错误会立即耗尽预算。
- 用 `lockTablesBackoffer` 只识别 MySQL 1146（`ErrNoSuchTable`），从错误文本提取库表名并写入 `block_list`，让下一轮重建 `LOCK TABLES` SQL 时排除已经消失的表。
- 用 `getTableFromMySQLError` 实现与 Go 版本一致的、刻意受限的 `db.table` 文本解析；复杂限定名不被猜测性拆分，而是返回错误并终止锁表重试。

## 主要符号

- 常量 `dumpChunkRetryTime = 3`、`lockTablesRetryTime = 5`：分别定义连接/导出块与锁表路径的初始预算。`dumpChunkWaitInterval = 50ms`、`dumpChunkMaxWaitInterval = 200ms` 定义连接策略的延迟状态；`ErrNoSuchTable = 1146` 是锁表策略唯一特殊处理的 MySQL 错误码。
- trait `backOfferResettable: BackoffStrategy`：在 `NextBackoff`、`RemainingAttempts` 之外增加 `Reset`。它是 `BaseConn.backOffer` 的动态分派接口。
- `newRebuildConnBackOffer(should_retry) -> Box<dyn backOfferResettable>`：`true` 返回 `dumpChunkBackoffer`，`false` 返回只保留一次执行机会的 `noopBackoffer`。
- `dumpChunkBackoffer { attempt, delay_time, max_delay_time }`：`NextBackoff` 先将延迟乘二、预算减一，再将返回延迟封顶；若根因是不可重试 MySQL 错误，则直接设置 `attempt = 0` 并返回零。
- `noopBackoffer { attempt }`：第一次失败时把唯一预算减为零，始终返回零延迟；`Reset` 恢复为 1。
- `newLockTablesBackoffer(tctx, block_list, conf)`：显式指定表（`conf.SpecifiedTables`）时预算为 1，否则为 5；返回具体的 `lockTablesBackoffer`，使调用方能读取逐步扩展的 `block_list`。
- `lockTablesBackoffer { tctx, attempt, block_list }`：1146 且表名可解析时扣减一次预算、记录库表并返回零；解析失败会记录日志并清零预算，其他错误直接清零预算。
- `getTableFromMySQLError(msg) -> Result<(String, String)>`：去掉可选的 `Table '` 前缀与 `' doesn't exist` 后缀，再要求按 `.` 分割后恰有两段。

这些符号都处于 `include!` 后的 crate 根作用域；文件内没有条件编译项。虽然多数符号声明为 `pub`，当前直接生产调用集中在同一 crate 的 `conn.rs` 和 `consistency.rs`。

## 执行流程

连接重建路径如下：

1. [conn.rs](./conn.rs) 的 `newBaseConn` 调用 `newRebuildConnBackOffer(should_retry)`，把结果保存到 `BaseConn.backOffer`；只有 `should_retry = true` 时才同时保留 `rebuildConnFn`。
2. `BaseConn::queryRows` 或 `BaseConn::ExecSQL` 把业务闭包和该策略交给 `WithRetry`。第二次及后续执行前，若存在 `rebuildConnFn`，会先替换 `DBConn`。
3. 业务闭包失败后，`WithRetry` 检查预算、调用 `NextBackoff`，再检查预算：不可重试 MySQL 错误或 noop 策略会在这里立即返回最后错误；其他错误可进入下一轮。
4. 整次 API 调用退出后，`queryRows`/`ExecSQL` 都调用 `Reset`，因此同一个 `BaseConn` 的下一条 SQL 不继承上一条 SQL 消耗的预算和延迟状态。

锁表路径如下：

1. [consistency.rs](./consistency.rs) 的 `consistency_lock_setup` 用空 `HashMap` 构造 `lockTablesBackoffer`，每轮依据其 `block_list` 生成 `LOCK TABLES` SQL。
2. 执行成功时，`block_list` 中的表会通过 `filterTablesFunc` 从后续导出集合剔除；若所有基表均被排除，调用方关闭连接并将 `empty_lock_sql` 置为真。
3. 执行失败时，调用方保留 `Error.mysql` 根因并附加 SQL 文本，然后检查预算并调用 `NextBackoff`。
4. 仅 1146 进入解析分支。解析成功后把 `(db, table)` 写入 `block_list`，下一轮不再锁该表；解析失败、非 1146 或预算耗尽时返回当前错误。

## 数据与状态

`attempt` 是可变的剩余预算，`RemainingAttempts() <= 0` 是两个驱动器的共同终止条件。`dumpChunkBackoffer.delay_time` 表示下一次计算的退避基数：初值 50ms，第一次失败先翻倍后返回 100ms，之后返回值最多 200ms；`Reset` 同时恢复预算 3 和基数 50ms。`noopBackoffer` 只有预算，没有延迟状态。

`lockTablesBackoffer.block_list` 的形状是 `HashMap<数据库名, HashMap<表名, ()>>`。它既是策略的累积状态，也是 `consistency_lock_setup` 下一轮 SQL 构造及成功后过滤配置的输入。`tctx` 仅用于解析失败时记录错误日志；此策略没有 `Reset`，生命周期限定在一次 lock setup。

`errors_cause` 保留/提取 `Error` 的底层 MySQL 信息。是否重试连接由 `IsRetryableError` 分类；锁表路径不复用该通用分类，而是严格限定为 1146。

## 依赖与调用关系

上游直接关系经 RustCodeGraph 的文件关系及精确搜索核对：

- [conn.rs](./conn.rs)：`newBaseConn -> newRebuildConnBackOffer`；`BaseConn::queryRows` 和 `BaseConn::ExecSQL` 消费 `backOfferResettable`，并在调用结束时执行 `Reset`。
- [consistency.rs](./consistency.rs)：`consistency_lock_setup -> newLockTablesBackoffer`，循环读取 `block_list` 并调用 `RemainingAttempts`/`NextBackoff`。
- [conn_test.rs](./conn_test.rs)、[sql_test.rs](./sql_test.rs)、[consistency_test.rs](./consistency_test.rs) 和 [parity_test.rs](./parity_test.rs)：分别从连接重建、元数据清理、锁表收敛和解析契约侧覆盖本文件行为。

下游依赖包括 [stubs.rs](./stubs.rs) 的 `BackoffStrategy`、`WithRetry`、`Error`、`MySQLError`、`IsRetryableError`、`errors_cause` 与 `errors_errorf`，[config.rs](./config.rs) 的 `Config.SpecifiedTables`，以及 `astersql-dumpling-context` 提供的日志上下文。标准库依赖为 `Duration` 与嵌套 `HashMap`。

RustCodeGraph 对 `retry.rs` 报告的使用文件为 `conn.rs`、`consistency.rs`、`conn_test.rs`、`sql_test.rs` 和 `parity_test.rs`。精确 `callers` 子命令在本地索引上无输出并挂起，已中止；上述调用边因此进一步由这些文件中的真实符号引用核验，而不是把挂起解释成“无调用者”。

## 错误处理与边界

- `dumpChunkBackoffer` 只对“具有 MySQL 根因且分类为不可重试”的错误立即清零；非 MySQL 错误及可重试 MySQL 错误都会消耗一份预算后继续。它不改变原错误，最终由驱动器向上传播。
- `lockTablesBackoffer` 对 1146 以外的任何错误均立即清零，避免在已进入锁表流程时扩大副作用；1146 的文本无法解析时，会通过 `tctx.L().Error` 记录原因并终止。
- `getTableFromMySQLError` 使用 `strip_prefix`/`strip_suffix`，因此前后固定文本不是强制条件；真正的结构约束是最终恰好一个点号。`a.b.c`、包含点号的引用名等会返回 `doesn't support retry lock table ...`。这与 Go `strings.Trim*` 加 `strings.Split` 的行为一致。
- `SpecifiedTables = true` 时锁表预算为 1。按 `consistency_lock_setup` 的“失败前后各检查一次预算”流程，首次 1146 会把预算减至零并直接返回，不会重新构造 SQL；这是当前实现的明确语义。
- 当前 Rust [stubs.rs](./stubs.rs) 的 `WithRetry` 会调用 `NextBackoff`，但丢弃返回的 `Duration`，没有 sleep/timer；因此连接策略当前只实现预算与分类语义，并未实现 Go `br/pkg/utils.WithRetryV2` 使用 `time.After(backoff)` 的真实等待。`consistency_lock_setup` 同样忽略返回值，不过锁表策略本来始终返回零。
- 文件不聚合多次错误；Rust stub 返回最后一次失败，而 Go `WithRetry` 会经 `WithRetryV2` 聚合错误。这是执行器层差异，不应在本策略文件中假装已解决。

## 并发与资源生命周期

三个 backoffer 都要求 `&mut self` 推进状态，本身没有锁、原子量、通道或后台任务，也未声明 `Send`/`Sync` 边界。`BaseConn` 独占 `Box<dyn backOfferResettable>` 并在同步重试循环中使用；不要在多个线程间并发共享同一策略实例。

连接资源由 [conn.rs](./conn.rs) 管理：重试时 `rebuildConnFn` 产生新 `Conn` 并替换 `DBConn`，查询路径无论业务处理成功与否都会尝试 `Rows.Close()`。锁表资源由 [consistency.rs](./consistency.rs) 管理：若过滤后没有可锁基表，会取出并关闭连接；本文件仅维护决策状态，不拥有或关闭连接。

取消信号也由驱动器处理：`BaseConn` 把 `tctx.Done()` 的快照传给 `WithRetry`，锁表循环则每轮检查 `tctx.Done()`。本文件不创建取消令牌，也不负责超时。

## 与 Go 版本的对应关系

Rust [retry.rs](./retry.rs) 按符号一一对应 Go [retry.go](./retry.go)：四个常量、`backOfferResettable`、两个连接策略、锁表策略及 `getTableFromMySQLError` 的预算、翻倍顺序、封顶值、1146 分类和字符串拆分逻辑均保持一致。Rust 用 `Box<dyn ...>` 对应 Go interface，用 `HashMap<String, HashMap<String, ()>>` 对应 `map[string]map[string]any`，用 `Result<(String, String)>` 对应 Go 的命名多返回值。

主要接线差异是 Go 的 `newLockTablesBackoffer` 接收调用方共享的 `blockList` map，而 Rust 将 map 移入具体策略，并由 `consistency_lock_setup` 直接从策略读取；结果仍是每次 1146 后用累积排除集重建 SQL。Go 的连接路径在 `conn.go` 使用相同构造器，`writer.go` 负责决定是否允许重建；Rust 当前由 crate 内 `newBaseConn` 的调用点传入 `should_retry`。

Go 的 `br/pkg/utils.WithRetryV2` 会等待 `NextBackoff` 返回的时长并收集历次错误；当前 Rust 本地 stub 不等待且返回最后错误。该差异属于尚未完全移植的通用执行层，不改变本文件计算出的时长和预算，但会改变真实时间行为与最终错误形态。

## 扩展指南

- 新增连接可重试错误时，优先修改 `stubs.rs` 中 `IsRetryableError` 的分类及其独立测试；只有策略的预算/退避规则改变时才修改 `dumpChunkBackoffer::NextBackoff`。同步覆盖不可重试、可重试、非 MySQL、封顶和 `Reset` 后状态。
- 修改退避时序时，同时审查 `stubs.rs::WithRetry`：仅改变本文件返回的 `Duration` 不会让当前 Rust 路径真正等待。若补齐等待，需评估取消响应、测试耗时和连接故障下的总体延迟，并与 Go `br/pkg/utils/retry.go` 对齐。
- 扩展锁表恢复范围时，在 `lockTablesBackoffer::NextBackoff` 集中分类，并保持 `consistency_lock_setup` 的 SQL 重建及成功后 `filterTablesFunc` 一致；不要仅吞掉错误而不更新排除集。
- 若要支持带点号/转义符的标识符，需替换 `getTableFromMySQLError` 的简单 split 契约，并同步 Go 实现或明确迁移差异。至少更新 [parity_test.rs](./parity_test.rs) 的正常/拒绝用例及 [consistency_test.rs](./consistency_test.rs) 的端到端锁表重试。
- 连接路径回归应放在独立的 [conn_test.rs](./conn_test.rs)；锁表路径放在 [consistency_test.rs](./consistency_test.rs)；跨语言契约放在 [parity_test.rs](./parity_test.rs)。不要把 Rust 测试内嵌进 `retry.rs`。
- 变更预算时要注意 `SpecifiedTables` 的单次语义、`RemainingAttempts` 的前后双检查，以及延迟“先翻倍再返回”的既有顺序；这些细节会影响尝试次数与首轮等待。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter dumpling/export` 确认目标及对照/测试文件；`node --file dumpling/export/retry.rs` 读取 175 行全貌并报告 5 个使用文件；`query` 分辨 Rust/Go 同名符号；对 `conn.rs`、`consistency.rs`、`stubs.rs`、`lib.rs` 和相关测试执行按文件 `node`。精确 `callers newRebuildConnBackOffer` 在本地挂起后被中止，调用边改由索引使用关系和 `rg` 双重核验。
- Rust 源与接线：[retry.rs](./retry.rs)、[lib.rs](./lib.rs)、[conn.rs](./conn.rs)、[consistency.rs](./consistency.rs)、[stubs.rs](./stubs.rs)、[config.rs](./config.rs)；crate 边界：[Cargo.toml](./Cargo.toml)。
- Go 对照：[retry.go](./retry.go)、[conn.go](./conn.go)、[consistency.go](./consistency.go)、[writer.go](./writer.go) 及 `br/pkg/utils/retry.go`。
- 独立 Rust 测试：[conn_test.rs](./conn_test.rs) 验证失败后重建、清理部分结果和策略复位；[sql_test.rs](./sql_test.rs) 验证失败元数据被丢弃且预算复位；[consistency_test.rs](./consistency_test.rs) 验证 1146 触发恰好一次重试、缺失表从第二条 SQL 和最终配置消失，以及唯一基表消失后的空锁路径；[parity_test.rs](./parity_test.rs) 验证 `pingcap.t1` 解析成功和 `a.b.c` 被拒绝。
- Go 测试：[consistency_test.go](./consistency_test.go) 覆盖 1146 后去除缺失表及唯一表消失；未发现同名 `retry_test.go`/`retry_test.rs`，策略通过上述调用方独立测试间接覆盖。
- 结构验证按任务命令执行；人工复核重点为：文件存在原因、两条运行路径、可变状态、错误终止条件、Go 差异和安全扩展位置均有直接源码或测试依据。
