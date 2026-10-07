# `cmd/ddltest/stubs.rs`

源码：[stubs.rs](./stubs.rs)。本文描述当前仓库中的实际实现；该文件是 `ddltest` 的测试支撑代码，不是 TiDB DDL 子系统的生产实现。

## 文件定位

`cmd/ddltest/stubs.rs` 属于 `astersql-cmd-ddltest` crate。`cmd/ddltest/Cargo.toml` 将该 crate 标记为 `test-only`，关闭自动测试发现，并把同目录的六个 Rust 文件声明为独立测试目标；其依赖表为空。`cmd/ddltest/lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 挂载本文件，再用 `pub use stubs::*` 将公开符号提升到 crate 根。

它存在的直接原因写在源码模块注释和 Cargo 注释中：在当前 darwin arm64 迁移环境不引入 `kv`、`domain`、`kvproto`、`grpcio` 等边界，而以本地内存桩承接 Go `cmd/ddltest` 的测试控制流。这里模拟多 server 路由、随机重启、异步 DDL、租约等待、SQL/DML 和结果断言，但不提供真实 TiKV、Domain、Session、MySQL 协议或外部进程。

## 核心职责

本文件承担五组职责。

1. 以 `ETCD`、`LEASE`、`SERVER_NUM`、`DATA_NUM` 等公开静态量保留 Go 包级 flag 的默认环境形态，但 Rust 侧不解析这些命令行 flag。
2. 以 `Datum`、`Column`、`IndexMeta`、`Table`、`Database` 和 `Engine` 构造仅覆盖当前用例的内存 schema、行存储与 SQL/DDL 执行器。
3. 以 `DdlSuite`/`Suite` 包装共享执行器、伪 server 和后台重启线程，对测试暴露 `exec`、`query`、`run_ddl`、`teardown` 等 Go 风格入口。
4. 以 `check_add_column`、`check_drop_column`、`check_drop_index`、`match_rows` 等函数复刻 Go 用例的可观察断言。
5. 以随机 helper 和 `SuiteOps` 生成 DDL 窗口内的并发 DML 扰动。

职责边界是“支撑这组测试已使用的语句和状态”。`Engine::exec` 对未知 SQL 返回 `unsupported SQL`，索引只保存元数据，事务与 GC range 删除是占位操作，因此不能把该文件当作 SQL 引擎、在线 DDL 状态机或一致性协议的实现。

## 主要符号

- 配置常量：`ETCD`、`TIDB_IP`、`TIKV_PATH`、`LEASE`、`SERVER_NUM`、`START_PORT`、`STATUS_PORT`、`LOG_LEVEL`、`DDL_SERVER_LOG_LEVEL`、`DATA_NUM`、`ENABLE_RESTART`。其中地址和端口主要用于保持接口及伪地址形态；`LEASE` 被缩放为毫秒级测试节奏。
- 随机函数：私有 `next_u64` 用原子 xorshift 状态产生值；公开 `random_int`、`random_intn`、`random_float`、`random_string`、`random_num` 对齐 Go helper 的取值范围与非法参数行为，而不对齐随机序列。
- 数据模型：`Datum::{Int, Float, Str, Null}` 提供 `get_int64`、宽松 `deep_equal` 和文本化；`Column`、`IndexMeta`、`IndexHandle`、`Table` 保存最小 schema；`Table::col_index` 和 `find_col` 做大小写不敏感列查找。
- 执行器：私有 `Database`/`Engine` 负责预置表及 SQL 路由。`ExecResult` 只公开 `rows_affected`；`QueryRows` 一次性物化查询结果，并以 `next/current/close` 模拟 `sql.Rows` 游标。
- suite：`DdlSuite` 持有 `Arc<Mutex<Engine>>`、伪 server 列表、退出原子标志和后台线程句柄；`Suite = Arc<DdlSuite>` 是公开共享类型。`create_ddl_suite` 是构造入口。
- 流量与断言：`SuiteOps::{exec_column_operations, exec_index_operations}` 产生并发 DML；`check_add_column`、`check_drop_column`、`check_drop_index` 检查 DDL 后状态；`get_index`、`lease_tick`、`dump_rows`、`match_rows`、`match_row`、`setup_test_main` 为测试提供辅助入口。
- `HandleMap` 是只保留去重和计数能力的 `HashSet<i64>` 包装，用于迁移后的 ddl 测试断言。

## 执行流程

典型调用从独立测试中的 `create_ddl_suite()` 开始。`DdlSuite::create` 创建并预置 `Engine`，按 `SERVER_NUM` 建立伪 server，随后启动后台线程。后台线程等待一个按 `LEASE` 缩放的随机窗口；若 `ENABLE_RESTART` 为真，它将随机实例标记为失活并在同一地址槽位放入新实例。

普通 SQL 经 `DdlSuite::exec` 先调用 `get_server` 选择存活伪实例，再在 `engine` 互斥锁内进入 `Engine::exec`。后者按前缀分派 `CREATE/DROP TABLE`、`ALTER TABLE`、`CREATE/DROP INDEX`、`INSERT`、`UPDATE`、`DELETE` 和 `ADMIN CHECK TABLE`；查询则由 `Engine::query` 处理单表投影及可选 `LIMIT`。行保存在 `BTreeMap` 中，所以遍历按整数 handle 稳定排序。

在线 DDL 场景通过 `DdlSuite::run_ddl` 建立 channel 并派生线程。线程持锁执行 DDL，成功后再等待两个缩放租约窗口，然后发送 `Result<(), String>`。`column_test.rs` 和 `index_test.rs` 用 `recv_timeout(lease_tick())` 等待；超时时分别调用 `SuiteOps` 的列或索引操作制造并发负载，收到结果后检查最终 schema/数据。

表初始化由 `Database::bootstrap_tables` 完成：普通场景预置以 `c1` 为 handle 的两列表，`test_index` 预置四列，`*_common` 表记录联合主键名称。当前存储实际仍只取 `pk_cols[0]` 作为整数 handle；联合主键名称保留了用例外形，却不是完整 common handle 实现。

结束时测试必须调用 `DdlSuite::teardown`：它设置退出标志、`join` 后台线程，再清空伪 server。独立测试用 `TeardownGuard` 保证 panic 展开时也执行此动作；`DdlSuite` 本身没有 `Drop` 自动清理实现。

## 数据与状态

`Engine` 是所有 SQL 状态的唯一所有者，并由 `Arc<Mutex<_>>` 串行保护。`Database.tables` 以原始表名为键；`Table.columns` 的顺序同时决定查询 `*` 和行向量的位置，故 `exec_alter` 加列时必须为每行追加默认值/`NULL`，删列时必须同时从 schema 与每个行向量移除相同下标。`next_col_id` 只单调增加，删除列不会复用 ID。

`Table.rows` 的键来自第一主键列经 `Datum::get_int64` 的宽松转换。重复 handle 返回 MySQL 风格 `Duplicate entry ... for key 'PRIMARY'`，使 `DdlSuite::exec_insert` 在启用重启模拟时能将预期冲突折叠为零受影响行。该转换会把无法解析的字符串和 `NULL` 变成 `0`，这是桩的简化边界。

索引状态只存在于 `Table.indices`；创建和删除只改变 `IndexMeta`，没有二级索引键、回填任务或 GC range。`QueryRows.idx` 是本地游标位置，`close` 不释放外部资源。`RAND_STATE`、`quit`、server 的 `alive` 与测试传入的行号均使用原子值；其余共享复合状态使用互斥锁。

## 依赖与调用关系

crate 内的直接装配边为 `lib.rs -> stubs.rs`。RustCodeGraph 对 `create_ddl_suite` 的精确查询定位到 `stubs.rs:1288`；同目录直接引用进一步确认上游：`column_test.rs`、`index_test.rs`、`ddl_test.rs` 和 `parity_test.rs` 创建 suite，`main_test.rs` 调用 `setup_test_main`，`random_test.rs` 调用随机 helper。`column_test.rs` 调用 `run_ddl`、`exec_column_operations`、`check_add_column`、`check_drop_column`；`index_test.rs` 调用 `run_ddl`、`exec_index_operations`、`get_index`、`check_drop_index`；`ddl_test.rs` 还覆盖通用执行、查询、结果匹配和 `HandleMap`。

下游仅是 Rust 标准库：集合、原子值、`Arc`/`Mutex`、MPSC channel、线程和时间。`Cargo.toml` 的 `[dependencies]` 与 `[dev-dependencies]` 均为空，佐证该桩不会接入真实数据库 crate。

内部主链为 `create_ddl_suite -> DdlSuite::create -> Engine::new -> Database::bootstrap_tables`；执行链为 `DdlSuite::{exec,query} -> get_server -> Engine::{exec,query}`；异步链为 `run_ddl -> thread::spawn -> Engine::exec -> sleep -> channel::send`；断言链从 `get_table` 取得克隆快照，再由 `iter_records` 或匹配 helper 遍历。

## 错误处理与边界

可恢复错误统一为 `Result<_, String>`，包括表/列/索引不存在、重复表/索引、列数不匹配、缺少 `FROM`/`SET`、不支持的 SQL 或查询。`must_exec` 与多数测试断言入口将错误升级为 panic；`exec_insert` 仅在 `ENABLE_RESTART` 且错误文本同时包含 `Duplicate entry` 与 `for key` 时吞掉冲突。

随机 helper 明确拒绝非法范围：`random_intn(n <= 0)` 和负长度 `random_string` panic；`parity_test.rs` 固定了这些边界。游标 `current` 约定只能在成功 `next` 后调用，否则会因 `idx - 1` 越界而 panic。锁均使用 `unwrap`，因此持锁线程 panic 会导致 poison 后的调用继续 panic。

SQL 解析是基于字符串前缀、空格、逗号和括号的有限解析器：不处理转义引号、一般表达式、复杂约束、带引号标识符、任意 WHERE 条件、真正的复合主键语义或事务隔离。未知能力应显式扩展并增加测试，不能通过静默成功掩盖缺失语义。

## 并发与资源生命周期

suite 的后台重启线程从 `DdlSuite::create` 存活到 `teardown`。它周期检查 `quit`，等待阶段每 5ms 检查一次，因此退出不会被完整租约窗口长期阻塞。`restart_handle: Mutex<Option<JoinHandle<()>>>` 保证线程只被 join 一次；伪 server 列表加锁后原位替换实例。

每个 `run_ddl` 会额外派生一个未保存 `JoinHandle` 的短期线程，其生命周期由发送端和返回的 `Receiver` 间接观察。即使接收端已丢弃，线程仍会完成执行与等待，发送错误被忽略。`SuiteOps` 为每个 worker 派生线程并逐一 join；线程共享 suite 与 `AtomicI64`，但所有 SQL 最终在同一个 `Engine` 互斥锁内串行化。因此模拟的是调用与等待窗口的并发形状，不是数据库内部并发执行或分布式一致性。

测试负责显式 teardown。若调用方创建 suite 后既不 teardown 也不结束进程，后台线程会持续运行；扩展调用点应沿用独立测试里的 `TeardownGuard` 模式。

## 与 Go 版本的对应关系

Go 的基线分散在 `ddl_test.go`、`column_test.go`、`index_test.go`、`random_test.go` 和 `main_test.go`。Rust 将这些文件依赖的共用实现集中进 `stubs.rs`，而测试仍保持独立文件。

- Go `createDDLSuite` 打开 TiKV storage、启动 DDL owner、bootstrap Domain/Session、启动多个 `tidb-server` 进程并经 MySQL 连接路由；Rust `DdlSuite::create` 改为预置内存表和伪 server。
- Go `restartServerRegularly` 真正杀死并重启进程；Rust 只切换 `MockServer.alive` 并替换同地址对象。
- Go `runDDL` 通过选中 server 的数据库连接执行 SQL，并等待两倍真实 schema lease；Rust 在线程内调用 `Engine::exec`，使用缩放毫秒等待保留控制流。
- Go 列检查以 `sessiontxn.NewTxn`、`tables.IterRecords` 和真实 `types.Datum` 读取表；Rust `new_txn` 是成功占位，`iter_records` 遍历克隆的 `BTreeMap` 快照，分类断言与 Go 保持对应。
- Go 删除索引后调用 `MockGCWorker.DeleteRanges` 再 `ADMIN CHECK TABLE`；Rust `mock_gc_delete_ranges` 为空成功函数，`ADMIN CHECK TABLE` 只确认表存在。
- Go 随机 helper 使用 `math/rand`；Rust 使用进程内原子 xorshift，但保留半开区间、字符集和 panic 契约。
- Go `TestMain` 初始化公共测试环境、logger 并做 goroutine 泄漏检查；Rust `setup_test_main` 只验证日志级别非空，线程清理由各测试的 teardown 断言承担。

因此“对齐”限定为当前迁移测试观察到的流程、返回形态和断言意图，不表示基础设施、时序、事务、索引、GC 或故障恢复机制等价。

## 扩展指南

新增 SQL 形态时，先在对应的独立 Rust 测试文件增加与 Go 用例同意图的回归，再扩展 `Engine::exec`/`query` 的分派和具体 `exec_*` helper；不要让未知语句默认成功。涉及 schema 的修改必须保持 `columns` 与每个 `rows` 值向量下标一致，并审查 `next_col_id`、默认值回填和 handle 提取。

新增并发场景应通过 `SuiteOps` 或新的测试侧 helper 产生负载，同时明确 `Engine` 全局锁意味着内部仍串行。如果需求必须验证真实 owner 选举、事务隔离、索引回填、GC range、网络断开或进程恢复，本桩不是合适接入点，应改用相应真实子系统测试，而不是继续扩大字符串执行器。

修改公共入口需同步检查 `lib.rs` 的再导出以及 `column_test.rs`、`index_test.rs`、`ddl_test.rs`、`main_test.rs`、`random_test.rs`、`parity_test.rs`。测试逻辑应继续放在这些独立测试文件，不嵌入 `stubs.rs`。资源相关扩展必须提供确定性 teardown；若新增长期线程，应保存句柄、响应退出信号并在 teardown 中 join。

主要兼容风险是偏离 Go 错误文本或测试控制流；正确性风险是行向量与 schema 错位、锁中 panic 和未回收线程；性能风险主要来自全局 `Engine` 锁与每轮大量派生线程。由于这是小规模测试桩，任何性能优化都不应牺牲可复核的 Go 流程对应关系。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter cmd/ddltest` 列出本 crate 的 13 个 Go/Rust 文件；`node --file cmd/ddltest/stubs.rs` 完整核对 1,480 行源码；`query create_ddl_suite --kind function` 定位公开构造入口。宽泛 `explore` 的跨仓结果存在同名符号噪声，精确 `callers` 在本环境未及时返回，因此调用者结论以同目录直接引用复核。
- crate 边界：`cmd/ddltest/Cargo.toml`、`cmd/ddltest/lib.rs`。
- Rust 调用与测试：`cmd/ddltest/column_test.rs`、`index_test.rs`、`ddl_test.rs`、`main_test.rs`、`random_test.rs`、`parity_test.rs`。其中 parity 测试覆盖非法随机参数、删除后查询、未知投影、`IF NOT EXISTS` 保留和重复建表错误；column/index/ddl 测试覆盖 suite 生命周期、异步 DDL 与结果断言。
- Go 对照：`cmd/ddltest/ddl_test.go` 的 `ddlSuite`、`createDDLSuite`、server 路由/重启和 `runDDL`；`column_test.go` 的列检查与并发操作；`index_test.go` 的索引/GC 检查；`random_test.go` 的随机契约；`main_test.go` 的公共初始化与泄漏检查。
- 本任务为纯文档分析，按计划不运行 Cargo。交付检查只验证目标文件存在且恰有十一个规定的二级标题，并人工复核上述路径、符号、边界和扩展建议均来自直接证据。
