# `cmd/benchdb/stubs.rs`

## 文件定位

[`stubs.rs`](stubs.rs) 是 `astersql-cmd-benchdb` crate 的本地依赖适配层。`Cargo.toml` 将该 crate 同时声明为库（`lib.rs`）和二进制（`bin_main.rs`），且 `[dependencies]` 为空；`lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 暴露本模块，再由 `bin_main.rs -> lib.rs::main -> main.rs::main` 进入命令主流程。它不是 TiDB session/store 的真实 Rust 实现，而是在不引入 `kv/domain/kvproto/grpcio` 等重依赖时，为 benchdb 保留参数、初始化、SQL 调用和资源清理的可观察形状（依据：`Cargo.toml`、`lib.rs:19-25`、`bin_main.rs:9-10`、`stubs.rs:2-10`）。

该文件由 `cmd/benchdb/main.rs` 直接消费，也由独立测试 `cmd/benchdb/parity_test.rs` 作为可注入、可观测的替身使用。RustCodeGraph 将目标文件识别为 71 个符号，并给出了 `main.rs`、`parity_test.rs` 等使用者；crate 内真正相关的直接入口以这两个文件为准，不能因图中同名符号的跨仓库候选而推断额外耦合。

## 核心职责

本模块把 Go `cmd/benchdb/main.go` 所依赖的六组外部边界压缩为本地实现：

1. `Error`、`must_nil`、`fatal` 模拟 `terror.MustNil` 与 `log.Fatal` 的快速失败合同。
2. `Flags`、`default_run_jobs`、`parse_flags`、`print_defaults` 模拟 benchdb 使用到的 Go `flag` 子集。
3. `LogConfig`、`new_log_config`、`init_logger` 保留日志初始化调用面，但不建立日志后端。
4. `StoreType`、`GlobalConfig`、`store_register`、`store_new`、`start_owner_manager`、`bootstrap_session` 保留 store/DDL/session bootstrap 的顺序与可观察配置，但不连接 TiKV。
5. `SqlArg`、`Session`、`ResultSet`、`RecordingSession` 记录 SQL、类型化参数、结果集排空和关闭行为，为主流程及 parity test 提供同一实现。
6. `rand_read`、`rand_intn` 提供非密码学伪随机载荷和区间随机数；`SessionFactory` 则允许测试注入预配置 session。

因此它的正确性目标是“边界合同与 Go 版可对照”，不是“具备真实分布式数据库能力”。`store_new` 只解析连接串中的 `disableGC=true`，owner manager 与 bootstrap 是无副作用空实现，`init_logger` 固定成功（依据：`stubs.rs:271-277,327-375`）。

## 主要符号

- `Error { msg }` 与 `Result<T>`：字符串错误载体；实现 `Display`/`std::error::Error`，并保留 Go 风格 `Error()`。`must_nil(Option<Error>)` 只在有错时调用发散函数 `fatal`。
- `Flags`：六个公开字段分别承载 PD 地址、表名、批大小、blob 大小、日志级别和作业串；`Default` 与 Go 旗标默认值一致。`parse_flags(&[String]) -> Flags` 支持单/双短横线以及 `-k=v`、`-k v`，在 `-`、`--` 或首个位置参数处停止。
- `FileLogConfig`、`LogConfig`：仅保留 benchdb 传入的配置形状；`EMPTY_FILE_LOG_CONFIG` 和 `DEFAULT_LOG_FORMAT` 对应 Go 常量/零值。
- `StoreType::TiKV`、`GlobalConfig`、`TiKVDriver`、`Storage`：最小 store 模型。`Storage` 只保存原始 `path` 和派生的 `disable_gc`。
- `SqlArg::{Null, Int, Str, Bytes, Ident}`：区分普通 SQL 值与 `%n` 标识符，避免测试把表名参数误当普通字符串；提供 `i32/i64/&str/String/Vec<u8>` 转换。
- `ExecRecord { sql, args }`：记录一次 `ExecuteInternal` 调用，是 parity 断言 SQL 顺序、模板和参数的依据。
- `Chunk` 与 `ResultSet`：最小结果集协议为 `NewChunk -> Next* -> Close`；`EmptyResultSet` 表示立即耗尽且关闭成功的普通实现。
- `Session`：仅声明 `ExecuteInternal(&mut self, sql, args)`；这是 benchdb 唯一需要的 session 能力。
- `RecordingSession`：`Arc<Mutex<RecordingSessionInner>>` 包装的可克隆记录型 session，公开读取执行记录/关闭次数及三个故障注入入口。内部 `RecordingResultSet` 与 session 共享状态。
- `rand_read`、`rand_intn`：基于全局 `AtomicU64` 状态的 xorshift 字节生成，以及要求 `n > 0` 的 `[0,n)` 映射。
- `SessionFactory { create: Rc<dyn Fn(...)> }`：把 session 创建从 `new_bench_db` 解耦；默认闭包调用 `create_session`。

## 执行流程

主链从 `entry::main` 读取进程参数并调用 `parse_flags`，随后 `run_with_flags` 依次执行 `new_log_config -> init_logger -> must_nil`、`store_register -> must_nil`，再进入 `new_bench_db`（依据：`main.rs:41-66`）。

`new_bench_db` 构造 `tikv://<addr>?disableGC=true`，由 `store_new` 生成不联网的 `Storage`；之后调用 `set_global_store`、`start_owner_manager`、`bootstrap_session`，再通过 `SessionFactory.create` 获得 `RecordingSession`，最后执行一次 `use test`。所有返回的 `Option<Error>` 都立即交给 `must_nil`，所以首个错误会中断初始化（依据：`main.rs:103-125`）。

具体作业调用 `BenchDB::must_exec` 时，`RecordingSession::ExecuteInternal` 先追加 `ExecRecord`。若 SQL 命中 `fail_on_sql`，返回错误且不创建结果集；否则取走一次性的 `rows_before_empty`，创建共享同一内部状态的 `RecordingResultSet`。调用方反复 `Next`：剩余值大于零时每次报告一行，耗尽后报告零行，然后正常路径调用 `Close`。`Close` 将 `close_count` 加一，并可按 `fail_on_close` 返回错误（依据：`stubs.rs:602-633`、`main.rs:139-157`）。

插入作业调用 `rand_read` 填充 `blob_size / 2` 字节；随机更新调用 `rand_intn(end-start)+start` 选取主键。`rand_read` 首次从系统时间取种子，逐字节推进 xorshift 状态并写回全局原子值（依据：`main.rs:291-348`、`stubs.rs:644-684`）。

## 数据与状态

- `GLOBAL_CONFIG` 是线程本地 `RefCell<GlobalConfig>`。`get_global_config` 返回快照，`set_global_store` 只修改当前线程的 store 字段；这有意避免并行测试通过进程全局配置互相污染，但也不等同于 Go 的进程级全局配置。
- store 注册表由 `OnceLock<Mutex<HashMap<String,bool>>>` 延迟初始化。当前仅会写入 `"tikv" -> true`；没有公开清理/读取 API，重复注册会覆盖为 `true` 而不报错。
- `RecordingSessionInner` 保存有序 `execs`、`close_count`、`fail_on_sql`、`fail_on_close` 和一次性 `rows_before_empty`。`execs()` 返回克隆快照，避免把锁借用泄露给测试。
- `RecordingResultSet` 自己持有 `closed` 与 `remaining_rows`，并通过共享 `Arc<Mutex<_>>` 在关闭时更新 session 统计。`EmptyResultSet` 则独立保存 `closed` 和内部 `next_calls`。
- `RNG_STATE` 是进程级 `AtomicU64`，用 `Relaxed` 读写；它不承诺可复现序列、密码学安全或跨线程严格串行的随机状态演进。
- `SessionFactory` 使用单线程引用计数 `Rc`，适合当前同步命令/测试注入；它没有 `Send`/`Sync` 合同。

## 依赖与调用关系

上游装配关系为 `bin_main.rs::main -> astersql_cmd_benchdb::main -> entry::main/run_with_flags -> stubs`。直接调用边包括：

- `entry::main -> args_from_env, parse_flags, print_defaults`；
- `run_with_flags -> new_log_config, init_logger, must_nil, store_register`；
- `new_bench_db -> store_new, set_global_store, start_owner_manager, bootstrap_session, SessionFactory.create, Session::ExecuteInternal`；
- `BenchDB::must_exec -> Session::ExecuteInternal -> ResultSet::{NewChunk,Next,Close}`；
- `insert_rows -> rand_read`，`update_random_rows -> rand_intn -> rand_read`。

下游只有 Rust 标准库：环境参数、stderr 输出、格式化、时间、`Rc`/`Arc`、互斥锁、线程本地状态和原子数。`cmd/benchdb/Cargo.toml` 的空 `[dependencies]` 是“本模块不拉入真实 TiDB/TiKV crate”的直接证据。

测试调用者集中在 `cmd/benchdb/parity_test.rs`：它直接构造 `Flags`、`RecordingSession` 和 `SessionFactory`，读取 `ExecRecord`，并配置 SQL/Close/多 chunk 故障场景。该目录没有 `doc.go`，也没有 Go `*_test.go`；Go 语义证据来自同目录 `main.go`，Rust 回归证据来自独立的 `parity_test.rs`，测试逻辑未内嵌在生产源文件中。

## 错误处理与边界

`fatal` 使用 `panic!`，使 parity test 能用 `catch_unwind` 观察失败；Go `log.Fatal` 则退出进程。两者都保证正常控制流不会越过首个致命错误，但 panic 会运行栈展开清理，而 `os.Exit` 不会，因此不能把它们视为完全相同的进程生命周期语义。帮助参数是例外：`parse_flags("-h"/"--help")` 打印默认值后直接 `process::exit(0)`，测试通过子进程验证它不会继续执行作业。

未知旗标、缺少旗标值、非法 `batch/blob` 数值都会 fatal；解析在 `-`、`--` 或首个位置参数停止。数值使用 `i64`，覆盖受支持 64 位目标上的 Go `int` 范围。`print_defaults` 忽略 stderr 写入错误，`init_logger`、`store_register`、`store_new`、owner/bootstrap/create session 的默认路径均不会产生错误；这些是桩的限制，不是对真实后端可靠性的证明。

`rand_intn` 用断言要求 `n > 0`；上游若给随机更新传入空区间（`end == start`），会 panic。锁获取统一 `unwrap()`，互斥锁中毒也会 panic。`RecordingSession::ExecuteInternal` 在故障判断前已经记录 SQL；命中执行失败时返回 `None` 结果集。正常结果集只在成功排空后关闭，执行或 `Next` 的 fatal 路径不会由 `must_exec` 主动调用 `Close`；这与 parity test 锁定的 Go fatal 路径意图一致。

## 并发与资源生命周期

主流程是同步的，没有异步任务、通道或后台线程。session 状态使用 `Arc<Mutex<_>>`，使结果集可在关闭时安全回写计数；锁只在短临界区持有，`ExecuteInternal` 在构造结果集前显式 `drop(g)`，避免同一线程携锁后再次访问共享状态。

每次成功执行生成一个 `RecordingResultSet`。`rows_before_empty` 在创建时被复制到结果集并立即清零，因此只影响下一次执行；`Next` 可被重复调用直到返回零行；`Close` 每次调用都会增加计数，本实现没有防止重复关闭，但 `BenchDB::must_exec` 正常只调用一次。`Storage` 没有 `Close` 或 Drop 资源，因为它只是字符串/布尔值句柄；owner manager、bootstrap 与 logger 也没有真实资源生命周期。

`RNG_STATE` 的原子访问避免数据竞争造成未定义行为，但 load/计算/store 不是一个原子更新事务：多个线程可从相同旧状态生成重叠序列并互相覆盖最终状态。当前 benchdb 同步使用方式不依赖并行随机质量；若未来并行化作业，必须重新评估 RNG 和 `Rc<SessionFactory>`。

## 与 Go 版本的对应关系

`cmd/benchdb/main.go` 直接调用真实的 `logutil`、`config`、`ddl`、`session`、`store/driver`、TiKV storage 与 `math/rand`；本文件逐一提供最小同形边界。旗标默认值和默认作业串逐项一致，`disableGC=true`、全局 store 类型、owner/bootstrap/session 创建顺序以及启动后的 `use test` 也由 Rust 主流程保留。

SQL 路径通过 `SqlArg::Ident` 对应 Go 的 `%n` 标识符参数，通过 `Int/Bytes/Str/Null` 保留 `...any` 的相关值类别。Rust `RecordingSession` 不执行 SQL，而是让 `parity_test.rs` 验证建表/清表模板、批事务数、随机更新范围、区间参数顺序、查询次数和未知作业停止行为。

重要差异必须保留在认知中：真实 store/DDL/bootstrap/logging 均被简化；全局配置变成线程本地；fatal 通常由 panic 模拟；伪随机算法与 Go `math/rand` 不保证同序列；结果集仅模拟“若干单行 chunk 后耗尽”，没有列数据、上下文取消或后端错误类别。这里承诺的是 benchdb 当前消费面和测试观察面，不是这些 Go 包的通用替代品。

## 扩展指南

新增旗标时，应同步修改 `Flags`、`Default`、`parse_flags`、`print_defaults`，以及 Go `main.go` 对应声明和 `parity_test.rs` 的默认值/解析边界断言。不要引入静默接受的未知参数，也要明确位置参数和 `--` 的停止规则。

新增 SQL 参数类别或 session 行为时，优先扩展 `SqlArg`、`Session`/`ResultSet` 合同与 `RecordingSessionInner`，并在独立 `parity_test.rs` 中验证 SQL 模板、参数类型/顺序、错误发生点及 Close 次数；不要把测试模块放回 `stubs.rs`。如果需要真实查询结果，应增加明确的行/列模型，而不是让 `Chunk::num_rows` 假装承载数据。

将空实现替换为真实依赖前，要同时审查 `Cargo.toml`、`run_with_flags` 和 `new_bench_db` 的初始化顺序、错误类型转换、store 关闭、owner/bootstrap 清理以及目标平台兼容性。外部 Rust 依赖必须遵守仓库规则：在独立上游仓库移植、提交并打 tag，当前仓库只引用统一 tag，不能复制到 `vendor/third_party` 或用本地 `[patch]`。

若并行化 benchdb，必须把 `Rc<SessionFactory>` 改为具备所需线程合同的工厂，确认 `RecordingSession` 锁粒度，并用原子 RMW 或线程局部 RNG 避免随机状态丢失更新。任何此类修改都应继续对照 Go 行为；若是 Rust 独有能力，应明确标注差异，而不是悄悄改变 parity 合同。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter cmd/benchdb` 列出 `bin_main.rs`、`lib.rs`、`main.rs`、`parity_test.rs`、`stubs.rs`；`node --file cmd/benchdb/stubs.rs` 覆盖 1-708 行；`explore "cmd/benchdb/stubs.rs public symbols callers callees role in benchdb"` 给出 71 个符号、主流程/测试调用者及关键调用边。
- 源码与装配：`cmd/benchdb/stubs.rs`、`cmd/benchdb/main.rs`、`cmd/benchdb/lib.rs`、`cmd/benchdb/bin_main.rs`、`cmd/benchdb/Cargo.toml`。
- Go 对照：`cmd/benchdb/main.go`，重点为旗标块、`main/newBenchDB/mustExec`、各作业 SQL 与随机调用。
- 独立测试：`cmd/benchdb/parity_test.rs` 的四类合同覆盖正常 SQL 副作用、解析/批边界、致命错误和结果集资源清理；额外测试覆盖负次数、单独 `-` 与帮助参数子进程退出。目录内未发现 Go `*_test.go`。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；交付检查以任务指定的 11 章节结构命令、Markdown 引用核对、源码事实抽查和 git diff 自审为准。
