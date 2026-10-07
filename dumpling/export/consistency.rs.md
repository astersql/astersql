# `dumpling/export/consistency.rs`

## 文件定位

[`consistency.rs`](consistency.rs) 属于 Cargo 包 `astersql-dumpling-export`。该包由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 定义为库，`package.metadata.porting.go-package = "dumpling/export"` 指明它对照 Go 的 `dumpling/export` 包。crate 根 [`lib.rs`](lib.rs) 不是用独立子模块封装本文件，而是在 `sql.rs` 之后、`metadata.rs` 之前通过 `include!("consistency.rs")` 把它并入 crate 根，因此本文件中的公开符号直接成为 `astersql-dumpling-export` 的公开 API，并可直接使用 crate 根已经导入或定义的 `Config`、`DB`、`Conn`、`Error`、`ServerType`、`HashMap` 与 `tcontext`。

本文件位于 Dumpling 导出主链的“一致性保护”边界：[`dump.rs`](dump.rs) 的 `Dumper::Dump` 在生成元数据、表任务和文件之前创建控制器并执行 `Setup`，在导出闭包结束后执行 `TearDown`。它把配置字符串分派为无操作、全局读锁或逐表读锁策略；快照本身由其他查询配置实现，本文件对 TiDB 的 `snapshot` 只返回无操作控制器。`auto` 的选择也不在本文件完成，而由 `dump.rs::resolveAutoConsistency` 在更早阶段把它解析成具体模式。

## 核心职责

1. 用 `ConsistencyTypeAuto`、`ConsistencyTypeFlush`、`ConsistencyTypeLock`、`ConsistencyTypeSnapshot`、`ConsistencyTypeNone` 固定 CLI、配置与 Go 版本共享的一致性模式字符串。
2. 用 `ConsistencyController` 统一三段生命周期：导出前 `Setup`、导出后 `TearDown`、持锁连接健康检查 `PingContext`。
3. `NewConsistencyController` 为一次一致性会话取得独立数据库连接，并根据 `Config.Consistency` 和服务端类型选择具体控制器或尽早报错。
4. `ConsistencyFlushTableWithReadLock` 在非 TiDB 服务端执行 `FLUSH TABLES WITH READ LOCK`，结束时执行 `UNLOCK TABLES` 并关闭专用连接。
5. `ConsistencyLockDumpingTables` 为所有待导出基表生成 `LOCK TABLES ... READ`；遇到 MySQL 1146“表不存在”时把缺失表加入 block list、重建 SQL 并重试，成功后过滤控制器配置副本中的表清单。
6. `ConsistencyNone` 为 `none` 以及合法 TiDB `snapshot` 提供统一但无副作用的生命周期实现。

本文件不负责枚举表、生成快照时间戳、设置会话隔离级别、执行实际导出或解析 `auto`。锁 SQL、TiDB 开关查询、退避策略和表过滤骨架分别由 [`sql.rs`](sql.rs)、[`retry.rs`](retry.rs) 与 [`block_allow_list.rs`](block_allow_list.rs) 提供。

## 主要符号

- `ConsistencyTypeAuto/Flush/Lock/Snapshot/None: &str`：公开模式常量。`NewConsistencyController` 不接受尚未解析的 `auto`；生产主链必须先经过 `resolveAutoConsistency`。
- `errTiDBDisableTableLock() -> Error`：每次构造一份带明确配置提示的错误；用于 TiDB 关闭 table lock 时拒绝 `lock` 模式。Go 对应项是包级错误变量，Rust 改为函数。
- `ConsistencyController: Send`：公开 trait，包含 `Setup(&mut self, &tcontext::Context)`、`TearDown(&mut self)` 和 `PingContext(&self)`。`Send` 允许控制器所有权跨线程移动，但 trait 本身不承诺共享引用可并发访问。
- `NewConsistencyController(&Config, &DB) -> Result<Box<dyn ConsistencyController>>`：公开工厂。它先调用 `DB::Conn()`，再按配置构造 trait object；`flush` 和 `lock` 保存该连接，TiDB `snapshot` 与 `none` 返回 `ConsistencyNone`，非 TiDB `snapshot` 和未知字符串返回错误。
- `ConsistencyNone`：零字段控制器，三个生命周期方法均直接成功。
- `ConsistencyFlushTableWithReadLock { server_type, conn }`：全局读锁控制器。`conn: Option<Conn>` 同时表达连接所有权和是否已经清理。
- `ConsistencyLockDumpingTables { conn, empty_lock_sql, specified_tables, server_type, conf }`：逐表读锁控制器。当前真正的 setup 判断从 `conf` 读取 `ServerInfo.ServerType` 与 `SpecifiedTables`；单独保存的 `specified_tables`、`server_type` 主要保持构造形状，当前逻辑没有直接读取它们。
- `consistency_lock_setup(...)`：锁表核心循环。它是公开函数，但生产调用者是 `ConsistencyLockDumpingTables::Setup`；独立函数形态也方便测试和迁移接线。
- `snapshotFieldIndex: usize = 1`：公开常量，保留 Go 中“快照查询结果第二列”的索引约定。当前仓库 Rust 搜索没有发现本文件外使用者，不能据此声称它已参与快照解析。

## 执行流程

生产主流程如下：

1. `Dumper::Dump` 在 `lock` 模式下先用普通连接执行 `prepareTableListToDumpInner`，确保待锁表集合已经准备好。
2. `Dumper::Dump` 调用 `NewConsistencyController(&self.conf, &db)`。工厂取得一个专用 `Conn`，再按具体模式分派。
3. `Dumper::Dump` 调用 `Setup`。若这里失败，`?` 会立即返回，后续导出闭包与显式 `TearDown` 都不会运行。
4. setup 成功后，Dumper 才创建导出连接、记录全局元数据、生成任务并由 writer 输出结果；控制器持有的专用连接在整个导出区间保持锁或作为健康检查对象。
5. 导出闭包无论成功还是返回错误，随后都会尝试 `TearDown`。teardown 错误只写 warning，`Dumper::Dump` 最终返回原导出结果。

各模式的分支是：

- `none`：构造 `ConsistencyNone`，setup/teardown/ping 均无操作成功。
- `snapshot`：只允许 `ServerTypeTiDB`；构造 `ConsistencyNone`。一致性来自 TiDB 快照读的其他接线，不来自这里的锁。
- `flush`：构造时不拒绝 TiDB，直到 `Setup` 才检查并报错；非 TiDB 调用 `FlushTableWithReadLock`。teardown 先从 `Option` 取走连接，再尝试 `UnlockTables`，随后无论解锁结果如何都调用 `Close`，最后返回解锁结果。
- `lock`：`Setup` 克隆 `self.conf`，把连接临时移动到临时控制器，调用 `consistency_lock_setup`，再把连接、空 SQL 标志和变更后的配置搬回自身。

`consistency_lock_setup` 的详细循环是：

1. TiDB 先经 `CheckTiDBEnableTableLock` 查询开关；查询错误原样传播，结果为 false 则返回 `errTiDBDisableTableLock()`。
2. 用空 block list 和配置创建 `newLockTablesBackoffer`。显式指定表时预算为 1，否则为 `lockTablesRetryTime`。
3. 每轮先检查 `tctx.Done()`；取消时返回 `context canceled`。
4. `buildLockTablesSQL(&conf.Tables, &backoffer.block_list)` 只选择基表，并排除已记录的缺失表。空 SQL 表示没有可锁基表：设置 `empty_lock_sql`、取走并关闭连接，然后把本轮视为成功。
5. 非空 SQL 经专用连接 `ExecContext` 执行。成功且 block list 非空时，`filterTablesFunc` 从控制器配置副本中删除失败表，然后返回。
6. 执行错误会在展示文本前加 `sql: <LOCK SQL>`，同时保留 `Error.mysql` 根因。`lockTablesBackoffer::NextBackoff` 只把 MySQL 1146 当作可恢复错误，从消息解析 `db.table` 并加入 block list；其他错误或不可解析表名会耗尽预算。当前循环忽略返回的等待时长，因此 1146 重试立即发生。

## 数据与状态

三个实现中只有锁控制器具有复合状态：

- `conn: Option<Conn>`：`Some` 表示仍持有专用一致性连接；`TearDown` 或空锁 SQL 路径用 `take()` 把它变为 `None`，使重复 teardown 幂等。锁控制器在 setup 前把连接暂时移动到 `tmp`，结束后再移回。
- `empty_lock_sql: bool`：记录当前没有任何基表需要锁。该状态使 `PingContext` 在连接已经主动关闭的情况下仍返回成功。
- `conf: Config`：构造时由传入配置 `clone_for_mutate()` 获得；setup 又建立可变副本。1146 收敛后的 `filterTablesFunc` 更新这份控制器内部配置，再存回 `self.conf`。
- `backoffer.block_list: HashMap<String, HashMap<String, ()>>`：按数据库名和表名记录不应再锁的表；它同时驱动下一轮 SQL 重建和成功后的表清单过滤。
- `backoffer.attempt`：剩余预算。显式表清单为 1，否则采用锁表重试常量；非 1146 错误直接归零。

重要边界是：Rust 工厂克隆 `Config` 放进控制器，`Dumper::Dump` 没有在 setup 后把 `controller.conf` 写回 `Dumper.self.conf`。因此当前生产接线中，1146 后的过滤结果只存在于控制器内部；Go 控制器保存 `*Config`，会直接修改 Dumper 使用的同一份表清单。Rust 独立测试验证了 `ctrl.conf.Tables` 收敛，但没有证明 Dumper 后续任务生成能看到它。这是当前迁移差异，不应描述为完全对齐。

`snapshotFieldIndex` 是无可变状态的常量；模式字符串也都是静态只读值。本文件没有全局锁、缓存或计数器。

## 依赖与调用关系

- crate 归属：[`Cargo.toml`](Cargo.toml) 声明 `astersql-dumpling-export` 为库，并依赖 Dumpling CLI/context/log、dumpformat、objstore、parser 等组件；本文件直接依赖的数据库与错误类型主要来自 crate 内 [`stubs.rs`](stubs.rs)，上下文来自路径依赖 `astersql-dumpling-context`。
- 编译组织：`lib.rs -> include!("sql.rs") -> include!("consistency.rs") -> include!("dump.rs")`，所以锁 SQL helper 在本文件之前可见，Dumper 在之后调用本文件 API。
- 主要上游：RustCodeGraph 显示 Rust `NewConsistencyController` 的生产调用者是 `dump.rs::Dumper::Dump`，测试调用者包括 `consistency_test.rs` 与 `parity_test.rs`。`ConsistencyLockDumpingTables::Setup -> consistency_lock_setup`。
- 主要下游：`NewConsistencyController -> DB::Conn/Config::clone_for_mutate/errors_new/errors_errorf`；flush 控制器调用 `FlushTableWithReadLock`、`UnlockTables`、`Conn::Close/PingContext`；锁 setup 调用 `CheckTiDBEnableTableLock`、`newLockTablesBackoffer`、`buildLockTablesSQL`、`Conn::ExecContext/Close` 和 `filterTablesFunc`。
- SQL 依赖：`sql.rs::buildLockTablesSQL` 只为 `TableTypeBase` 生成带反引号转义的 `db.table READ` 片段；`FlushTableWithReadLock` 和 `UnlockTables` 分别执行固定 SQL；`CheckTiDBEnableTableLock` 当前通过 `SHOW CONFIG WHERE name='enable-table-lock'` 读取最后一列并接受 `true` 或 `1`。
- 重试依赖：`retry.rs::lockTablesBackoffer` 通过 `errors_cause` 读取 MySQL 错误码 1146，并用 `getTableFromMySQLError` 解析准确的两段式 `db.table`。
- 表过滤依赖：`block_allow_list.rs::filterTablesFunc` 重建 `Config.Tables`，同时保留 `DumpEmptyDatabase` 所要求的空库并记录被忽略项。

RustCodeGraph 的广义 `explore` 给出了上述关键边，但精确 `callers/callees` 命令在本次检查中超时且没有返回可用文本；因此具体边又用目标文件索引、`dump.rs` 和 helper 定义直接核验，没有补写未观察到的调用者。

## 错误处理与边界

- 工厂错误：`DB::Conn()` 失败直接传播；非 TiDB 请求 `snapshot` 返回“不支持此服务端”；未知模式返回包含原字符串的 `invalid consistency option`。工厂没有 `auto` 分支，因此调用者必须先解析它。
- flush 边界：TiDB 在 `Setup` 阶段显式失败。连接已由 teardown 取走时，`PingContext` 返回 `consistency connection has already been closed`；重复 teardown 成功。
- lock 边界：TiDB 开关查询失败直接传播，开关关闭返回带 `enable-table-lock=true` 建议的错误。上下文只在每轮循环开始检查，SQL 调用本身能否被取消取决于 `Conn::ExecContext` 的实现。
- 只有带 MySQL 根因且错误码为 1146 的错误可以收敛重试；其他 SQL 错误、重试预算耗尽或无法解析的三段式/异常表名都返回最后错误。错误消息被补充 SQL，但 `mysql` 字段仍保留，保证 backoffer 不会因注释丢失分类信息。
- 空锁 SQL 是正常状态而非错误：控制器关闭连接、令 ping 和 teardown 成功。视图不会进入 `LOCK TABLES` SQL，因此“只有视图”也会走该路径。
- teardown 保证在 `UNLOCK TABLES` 返回错误时仍尝试关闭连接，但关闭错误被忽略，最终只返回解锁错误。`Dumper::Dump` 又只记录 teardown warning，不覆盖原导出结果。
- setup 失败时 `Dumper::Dump` 因 `?` 立即返回，不会显式调用控制器 teardown；本文件也没有 `Drop` 清理实现。修改 setup 的部分成功路径时必须注意这一资源边界。
- `NewConsistencyController` 在检查具体模式之前已经取得连接；`none`、`snapshot` 和工厂错误分支不把该连接保存在返回控制器中，也没有在本函数显式 `Close`。实际释放语义取决于 `Conn` 实现，不能从本文件推断成已执行数据库关闭。

## 并发与资源生命周期

每个控制器面向一次导出、拥有一条独立于普通导出查询的连接。锁在 `Setup` 成功后保持到 `TearDown`，覆盖元数据读取、任务生成和 writer 消费整个导出区间。`Option::take` 建立单一清理所有权：首次 teardown 负责解锁与关闭，后续调用不重复发 SQL；空锁路径提前关闭后也不会再次处理连接。

`ConsistencyController: Send` 允许 boxed 控制器被移动到另一个线程，但方法仍要求 `&mut self` 才能 setup/teardown，且没有 `Sync` 约束；当前 `Dumper::Dump` 在同一同步调用栈中顺序使用它。本文件不创建线程、channel、异步任务或互斥锁。表清单和 block list 都是单线程可变值，重试也是串行循环。

flush 与非空 lock 的资源顺序是“取得专用连接 -> 加锁 -> 导出 -> 解锁 -> 关闭”。空 lock 是“取得连接 -> 发现无基表 -> 关闭 -> 以 no-op 健康状态继续”。snapshot/none 不持有控制器连接。`PingContext` 只检查专用连接当前是否可用，不自动重建连接，也不验证锁仍然存在；生产 `Dumper::Dump` 当前没有调用它，现有直接使用集中在测试契约。

## 与 Go 版本的对应关系

直接对照文件是 [`consistency.go`](consistency.go)，直接 Go 测试是 [`consistency_test.go`](consistency_test.go)。两版保留了相同模式字符串、控制器三段接口、flush/lock/none 三种实现、TiDB snapshot 限制、TiDB table-lock 开关检查、1146 跳过缺失表以及空锁 SQL 转 no-op 的整体意图。

主要差异如下：

- Go `NewConsistencyController(ctx, conf, session)` 用调用方 context 获取 `*sql.Conn`；Rust 工厂没有 context 参数，使用 crate 内 `DB::Conn()`。
- Go `TearDown` 与 `PingContext` 接收 `context.Context`；Rust 版本没有 context 参数。Rust `Setup` 仍接收 Dumpling context，并显式在重试轮次检查 `Done()`。
- Go 控制器保存 `*Config`，1146 后 `filterTablesFunc` 直接修改调用方配置；Rust 保存克隆配置，所以过滤结果当前不会自动回写 `Dumper.self.conf`。这是影响后续实际导出表集合的语义差异。
- Go 使用 `utils.WithRetry` 并让闭包与 backoffer 共享 `blockList`；Rust 用显式循环，block list 归 backoffer 所有。Rust 调用 `NextBackoff` 后忽略其 `Duration`，当前 1146 路径恰好返回零等待。
- Go `errTiDBDisableTableLock` 是稳定错误实例，并在所示版本中通过 `SELECT @@tidb_config` 解析配置；Rust 是错误工厂，当前 helper 查询 `SHOW CONFIG WHERE name='enable-table-lock'`。Rust 测试夹具使用 `SELECT @@tidb_enable_table_lock` seed，但实现查询与该 seed 并不一致，测试桩未匹配时的行为需要在未来迁移核验中关注。
- Go 的 `ConsistencyLockDumpingTables` 只有连接、配置指针和空 SQL 标志；Rust 还保存 `specified_tables`、`server_type`，但当前 setup 使用 `conf` 中的对应字段。
- Go `TearDown` 通过 defer 保证关闭并置空；Rust 用 `Option::take` 获得同样的重复调用幂等形状。两版都忽略关闭错误并优先返回解锁结果。
- Go `snapshotFieldIndex` 是包内常量；Rust 将其公开，但当前没有观察到 Rust 使用者。

Rust [`consistency_test.rs`](consistency_test.rs) 覆盖模式基本路径、1146 恰好一次重试、唯一基表消失后的空锁状态、非法模式/服务端组合和 TiDB 开关关闭。Go 测试还明确验证控制器具体动态类型、精确 SQL mock，以及 `auto` 在 unknown/MariaDB、无 SUPER 权限回退到 lock 等路径；这些 `auto` 逻辑属于 `dump.rs::resolveAutoConsistency`，不是本文件职责。Rust parity 测试补充了 none 生命周期、非 TiDB snapshot、TiDB flush 和关闭连接后 ping 失败等契约。

## 扩展指南

- 新增一致性模式时，在本文件增加模式常量与 `NewConsistencyController` 分支，并同步检查 `dump.rs::resolveAutoConsistency`、`needRepeatableRead`、CLI 默认值/校验和 `config.rs`。测试应扩展独立的 [`consistency_test.rs`](consistency_test.rs) 与 [`parity_test.rs`](parity_test.rs)，不要把测试写入生产文件。
- 修改锁表策略时，主要接入点是 `consistency_lock_setup`。同时核对 `sql.rs::buildLockTablesSQL`、`retry.rs::lockTablesBackoffer` 和 `block_allow_list.rs::filterTablesFunc`，覆盖基表/视图混合、零基表、1146 多次收敛、显式表预算、取消、不可解析错误和非 1146 错误。
- 若要真正对齐 Go 的缺失表过滤，需设计控制器如何把收敛后的 `Config.Tables` 反馈给 `Dumper`，不能只继续断言 `ctrl.conf`。应新增从 `Dumper::Dump` 到任务生成的集成级回归，证明消失表不会随后仍被导出；还要评估 `Arc<Config>` 的替换时机和并发读者。
- 修改资源管理时，应为工厂错误、none/snapshot 临时连接、setup 半途失败、unlock 失败以及重复 teardown 增加可观察关闭断言。若引入 `Drop`，必须避免与显式 teardown 双重解锁。
- 修改 TiDB 开关检测时，要统一 `CheckTiDBEnableTableLock` 实际 SQL、Rust 测试 seed 和 Go 对照语义，并分别覆盖“查询失败”“无行”“false/0”“true/1”。
- 修改 trait 签名或线程使用方式时，评估 `Send`/`Sync`、context 传播和锁连接的线程亲和性。不要仅为跨线程共享加 `Sync`，除非 `Conn` 和所有实现状态都满足真实并发安全要求。
- 正确性风险集中在锁覆盖区间、错误后资源释放和配置副本回写；兼容风险集中在公开 Go 风格名称、错误文本及 SQL；性能风险主要是锁持有时间和大表集合反复重建 SQL/配置，而不是本文件内的 CPU 并行度。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件、307,296 个节点和 1,848,419 条边；`files --filter dumpling/export/consistency.rs` 确认目标已索引且有 26 个符号。
- RustCodeGraph `node --file dumpling/export/consistency.rs --offset 1 --limit 500`：读取全部 223 行，核对五个模式常量、错误工厂、trait、工厂、三个控制器、锁 setup 循环和快照列索引。
- RustCodeGraph `query`：确认 `NewConsistencyController` 同时存在 Go/Rust 定义，`consistency_lock_setup` 只有 Rust 定义，并定位两个具体控制器；`explore` 确认 Rust 工厂由 `dump.rs::Dump` 与独立测试调用，`consistency_lock_setup` 由控制器 `Setup` 调用，下游包含 `newLockTablesBackoffer`、`buildLockTablesSQL`、`CheckTiDBEnableTableLock` 与 `filterTablesFunc`。
- RustCodeGraph 精确 `callers/callees` 命令已执行，但在 30 秒限制内没有产出；具体调用边随后由索引 `explore` 和直接入口/helper 源码交叉核验。
- 已读 crate 与生产接线：`dumpling/export/Cargo.toml`、`dumpling/export/lib.rs`、`dumpling/export/dump.rs`、`dumpling/export/sql.rs`、`dumpling/export/retry.rs`、`dumpling/export/block_allow_list.rs`。
- 已读 Go 对照与测试：`dumpling/export/consistency.go`、`dumpling/export/consistency_test.go`；已读独立 Rust 测试：`dumpling/export/consistency_test.rs`、`dumpling/export/parity_test.rs`。这些证据覆盖模式分派、锁 SQL、1146 收敛、空锁、错误分支、连接关闭与 Go/Rust 差异。
- 仓库搜索确认 `snapshotFieldIndex` 当前只在本文件定义；`ConsistencyTypeAuto` 的生产解析位于 `dump.rs::resolveAutoConsistency`。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令验证文档存在且恰有十一个固定二级章节，并人工复核本说明能回答文件为何存在、如何进入导出主链、各模式如何运行以及扩展时必须同步哪些独立测试。
