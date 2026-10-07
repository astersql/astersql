# `cmd/benchdb/main.rs`

## 文件定位

`cmd/benchdb/main.rs` 是 workspace 成员 `astersql-cmd-benchdb` 的可测试命令实现模块。它不是 Rust 进程最外层的 `fn main`：真实装配链是 `cmd/benchdb/bin_main.rs::main` 调用 `astersql_cmd_benchdb::main`，后者由 `cmd/benchdb/lib.rs::main` 转发到本文件的 `entry::main`。`cmd/benchdb/Cargo.toml` 通过 `[lib] path = "lib.rs"`、`[[bin]] path = "bin_main.rs"` 明确了这层边界，并用 `package.metadata.porting.go-package = "cmd/benchdb"` 标记其 Go 对照目录。

该 crate 的 `[dependencies]` 为空。当前 Rust 命令并未连接真实 TiKV、DDL owner 或 TiDB session；本文件调用的 `Storage`、`SessionFactory`、`RecordingSession` 等都来自同目录 `stubs.rs`。因此它目前是用于保存和验证 Go `cmd/benchdb/main.go` 控制流与 SQL 契约的可执行迁移桩，而不是可对真实集群做性能测量的完整 benchdb。

## 核心职责

本文件集中承担四类职责：

1. `main`/`run_with_flags` 初始化日志和 TiKV store 注册，解析 `|` 分隔的作业串并顺序分派。
2. `new_bench_db` 按 Go 顺序构造 store、设置全局 store 类型、启动 owner manager、bootstrap session、创建会话并切换到 `test` 数据库。
3. `BenchDB` 的解析方法把 `name:spec`、`start_end[:count]` 转成作业参数，并对格式或整数错误走致命失败路径。
4. `create_table`、`truncate_table`、`insert_rows`、`update_random_rows`、`update_range_rows`、`select_rows`、`query` 生成与 Go 版本一致的内部 SQL；`must_exec` 负责执行、读空和关闭结果集，`run_count_times` 负责聚合耗时。

职责边界也很明确：命令行解析、伪随机数、存储/session/result-set 实现位于 `cmd/benchdb/stubs.rs`；二进制和库装配分别位于 `bin_main.rs`、`lib.rs`；行为契约测试独立放在 `parity_test.rs`，没有内嵌在生产源文件中。

## 主要符号

- `default_flags() -> Flags`：返回 `Flags::default()`，让二进制和 parity test 共享同一组 Go 默认旗标。
- `main()`：读取进程参数，经 `stubs::parse_flags` 解析并打印默认值，然后以默认 `SessionFactory` 调用 `run_with_flags`。
- `run_with_flags(flags, factory)`：可注入依赖的主调度入口。它初始化日志、注册 TiKV driver、创建 `BenchDB`，再按顺序分派作业；接受 `update-random`/`update_random` 和 `update-range`/`update_range` 两组别名，未知作业记录日志后立即返回。
- `BenchDB { store, session, flags }`：保存 store 句柄、记录型 session 和旗标快照。`store` 在本文件后续不直接读取，但保留 Go 版资源布局和初始化结果。
- `new_bench_db(&Flags, &SessionFactory) -> BenchDB`：使用 `tikv://<addr>?disableGC=true` 创建存储并完成 session bootstrap，最后执行 `use test`。
- `must_exec(&mut self, sql, args)`：调用 `Session::ExecuteInternal`；若有结果集，则循环 `Next` 到 `NumRows() == 0`，成功路径最后 `Close`。执行、读取或关闭错误都会 `fatal`。
- `must_parse_work`、`must_parse_int`、`must_parse_range`、`must_parse_spec`：作业与规格解析器。范围是半开区间语义的 `start_end`，要求 `start >= 0` 且 `end >= start`；省略 count 时默认为 1。
- `create_table`、`truncate_table`：发出带 `%n` 标识符参数的 DDL。
- `insert_rows`：按 `batch_size` 向上取整拆事务，在 `[start, end)` 插入行；每行生成长度为 `blob_size / 2` 的随机字节。
- `update_random_rows`：在 `[start, end)` 随机选择主键，共执行 `total_count` 次更新，并按 batch 分事务。
- `update_range_rows`：重复 count 次事务，每次更新固定半开区间 `[start, end)`。
- `select_rows`、`query`：前者重复执行固定范围查询；后者把 `sql:count` 的 SQL 原样重复执行。
- `run_count_times`：记录 sum、first、last、min、max 并计算平均耗时；负 count 对齐 Go 整数 range 的零次迭代，零 count 仍会在求平均时失败。
- `c_log_f`、`c_log`：用 ANSI 绿色向标准输出打印阶段日志，不参与控制流。

## 执行流程

1. `bin_main.rs::main` 进入 `lib.rs::main`，再进入本文件 `main`。
2. `main` 从环境读取 argv；`stubs::parse_flags` 生成 `Flags`，`print_defaults` 输出帮助默认值。
3. `run_with_flags` 先调用 `init_logger(new_log_config(...))`，再以 `store_register(StoreType::TiKV, &TiKVDriver)` 注册 store；任一步返回错误都由 `must_nil` 中止。
4. `new_bench_db` 拼接禁用 GC 的 TiKV URL，依次调用 `store_new`、`set_global_store`、`start_owner_manager`、`bootstrap_session`、注入的 `SessionFactory::create`，并执行 `use test`。
5. 调度器以 `|` 拆分 `flags.run_jobs`，逐项 trim 并转为小写；`must_parse_work` 只取第一个 `:` 前的名称，其余内容重新连接为 spec。随后 `query` 会再次按 `:` 拆分并只使用前两段，所以其中的 SQL 实际不能安全包含冒号。
6. `create`/`truncate` 直接执行一次 SQL；其余标准作业先解析范围和 count，再通过 `run_count_times` 执行相应次数。
7. 写作业显式发出 `begin` 和 `commit`。`insert_rows`、`update_random_rows` 每批一个事务；`update_range_rows` 每次一个事务。`select_rows` 和 `query` 不显式开启事务。
8. 每条 SQL 都经 `must_exec` 执行并读空结果集，确保懒读取成本落入当前操作；正常结束后关闭结果集。
9. 遇到未知作业（包括当前默认作业串中的 `gc`）时，记录 `Unknown job` 并返回，因此未知作业之后的阶段不会执行。

## 数据与状态

`Flags` 是本文件的主要配置快照，字段含 PD 地址、表名、事务 batch、blob 大小、日志级别和作业串；默认值定义在 `stubs.rs::Flags::default/default_run_jobs`。`BenchDB` 拥有 `Storage`、`RecordingSession` 和克隆后的 `Flags`，作业闭包通过 `&mut BenchDB` 串行修改 session 记录及局部计数。

作业范围统一按 `[start, end)` 使用：insert 在 `id == end` 时停止，随机更新调用 `rand_intn(end - start) + start`，范围更新与查询 SQL 使用 `id >= start and id < end`。insert 的 `id`、随机更新的 `run_count` 被闭包捕获并跨 batch 保留，保证尾批只执行剩余数量。SQL 表名使用 `SqlArg::Ident` 绑定，整数和字节使用类型化 `SqlArg`；这也是 `parity_test.rs` 可以核对模板和参数顺序的观测面。

计时状态只存在于单次 `run_count_times` 调用中：`sum/first/last` 初始为零，`minv` 为一分钟，`maxv` 为一纳秒。该函数打印聚合值但不返回测量结果，也不持久化指标。

## 依赖与调用关系

上游入口为 `cmd/benchdb/bin_main.rs::main → astersql_cmd_benchdb::main（lib.rs）→ entry::main（本文件）`。测试上游是 `cmd/benchdb/parity_test.rs`，它直接调用 `run_with_flags`、`new_bench_db` 和各 `BenchDB` 方法；RustCodeGraph 的 `query` 将 `run_with_flags` 精确定位为 `cmd/benchdb/main.rs:53`、将 `new_bench_db` 定位为第 103 行。

本文件的直接下游全部由 `crate::stubs` 提供：参数侧为 `args_from_env/parse_flags/print_defaults`，初始化侧为日志配置、`store_register/store_new/set_global_store/start_owner_manager/bootstrap_session`，执行侧为 `SessionFactory`、`Session::ExecuteInternal`、`ResultSet` 和 `SqlArg`，数据生成侧为 `rand_read/rand_intn`。`Cargo.toml` 的空依赖表证明当前没有真实 TiDB/TiKV crate 下游；`stubs.rs` 中 `start_owner_manager` 与 `bootstrap_session` 还是无副作用空实现，`store_new` 也只记录 URL 和 `disable_gc`。

RustCodeGraph 的 `callers/callees` 精确查询在当前本地索引上未返回结果并持续运行，已中断；因此这里不把图工具显示的跨仓库“used by”摘要解释成调用边。上述入口和下游关系由已索引的 `main.rs` 源码、`lib.rs`、`bin_main.rs` 以及 `parity_test.rs` 的直接调用交叉确认。

## 错误处理与边界

初始化错误统一交给 `stubs::must_nil`，SQL 执行、`Next` 或 `Close` 错误交给 `stubs::fatal`；桩实现以 panic 模拟 Go `log.Fatal`/进程终止，以便测试捕获。`must_exec` 只在正常执行和读空之后关闭结果集；这与 Go 的 `log.Fatal` 会跳过 defer 的进程退出效果保持一致，而不是 Rust 常见的可恢复 `Result` API。

解析边界包括：范围必须恰有两个 `_` 分段、整数必须可解析、起点不得为负、终点不得小于起点。`must_parse_spec` 与 `query` 都直接索引分段，因此空 spec 或缺少 query 计数不是友好诊断路径，可能先发生越界 panic；多余的 spec 分段也未显式拒绝。`update_random_rows` 要求 `end > start`，否则 `rand_intn(0)` 会触发断言。`batch_size <= 0`、负 `blob_size` 或过大整数可能在除法、整数转换或内存分配处失败，这些输入没有在本文件单独校验。

`run_count_times` 对负 count 执行零轮并返回零平均值，这是为对齐 Go 当前整数 range 语义而显式保留；count 为零时求平均会除零失败。未知作业不是 fatal，而是停止整个剩余流水线。尤其默认 `run_jobs` 包含未实现的 `gc`，因此默认流程会在 `gc` 处停止，这一事实由 `parity_test.rs` 明确锁定，不能描述为已经支持 GC。

## 并发与资源生命周期

本文件自身不创建线程、异步任务或通道；作业和 batch 完全串行执行，每个阶段结束后才进入下一阶段。写路径以 SQL 文本显式控制事务：`begin` 后逐条执行，最后 `commit`。若事务中间 `fatal`，本文件没有 rollback 或恢复逻辑，符合基准工具首错即停的设计，但替换为真实 session 时需要考虑未提交事务的连接清理。

`Storage` 和 `RecordingSession` 随 `BenchDB` 生命周期持有，但本文件没有显式关闭 store/session；Go 源码也注明该测试工具没有关闭这些组件。每个 `ExecuteInternal` 返回的结果集则由 `must_exec` 在成功路径读空并关闭，`parity_test.rs::contract_resource_cleanup` 验证多批读取后只关闭一次，并验证关闭错误会进入 fatal。

并发状态来自下游桩而非本文件：`RecordingSession` 使用 `Arc<Mutex<_>>` 共享执行记录，store 注册表使用全局 `Mutex<HashMap<...>>`，全局配置是 thread-local，伪随机状态是 `AtomicU64` 且使用 relaxed 顺序。本文件的 `SessionFactory` 内部使用 `Rc`，也进一步表明当前执行模型限定在单线程。

## 与 Go 版本的对应关系

Rust 符号基本逐一映射 `cmd/benchdb/main.go`：`BenchDB ↔ benchDB`、`new_bench_db ↔ newBenchDB`、`must_exec ↔ mustExec`、四个解析方法、七个作业方法、`run_count_times ↔ runCountTimes` 以及两种彩色日志函数。作业分派名称、SQL 模板、参数顺序、`disableGC=true`、bootstrap 后的 `use test`、batch 取整公式、半开区间和 blob 减半均保持一致。

主要迁移差异是依赖真实性：Go 版直接导入 `pkg/ddl`、`pkg/session`、`pkg/store`、TiKV client 和日志组件；Rust crate 没有依赖这些实现，只调用 `stubs.rs`。Go `main` 在解析 flag 后直接包含全部初始化和分派，Rust 则额外抽出 `run_with_flags(flags, factory)` 供测试注入。Go `benchDB` 只存 store/session，Rust 还保存 flags，以替代 Go 的包级 flag 指针。

Rust 的 `must_exec` 显式在成功路径末尾关闭结果集；Go 用 defer，但 `log.Fatal` 会退出进程而不执行 defer，因此错误路径均不保证 Close。Rust 使用 i64 对齐受支持平台上的 Go int 宽度。计时函数还显式处理负 count，使其行为与 Go 整数 range 保持一致。默认 `gc` 在 Go 和当前 Rust switch 中都没有 case，故两者都会把它作为未知作业并停止，而不是实际触发 GC。

## 扩展指南

- 增加新作业时，应同时修改 `run_with_flags` 的 match、实现相应 `BenchDB` 方法，并在 `cmd/benchdb/parity_test.rs` 扩展分派顺序、SQL 文本和参数断言；若是从 Go 移植，还需同步核对 `cmd/benchdb/main.go` 的增量。
- 若要实现默认串中的 `gc`，最小接入点是 `run_with_flags` 的 match，但真实行为还需要在 `stubs.rs` 增加可观测依赖边界及 parity test；不能只增加空 case 后宣称支持。
- 若替换为真实 TiKV/TiDB 依赖，应优先保持 `Session`/`SessionFactory` 注入边界，明确 store/session 的 Close、事务失败 rollback、owner manager 生命周期和线程安全；同时更新 `Cargo.toml`，并按仓库规定在独立上游仓库处理任何外部 Rust 依赖及 tag。
- 扩充规格语法时，修改 `must_parse_work/must_parse_spec/query`，补充缺段、多段、零/负 count、空区间、`end == start` 随机更新、非正 batch 和非法 blob 大小测试。不要把测试写回 `main.rs`；继续放在同目录独立 `parity_test.rs` 或新增独立测试文件。
- 调整 SQL 时必须保留 `%n` 与 `%?` 的类型化参数边界，核对范围上下界及事务包围顺序；否则会产生兼容或注入风险。高频循环中的 blob 克隆、随机生成和逐条 `ExecuteInternal` 是主要性能敏感点，优化时需证明没有改变 Go 工作负载形状。

## 验证依据

- RustCodeGraph：`status` 显示索引含 `cmd/benchdb/main.rs`；`files --filter cmd/benchdb` 列出 `main.rs`、`lib.rs`、`bin_main.rs`、`stubs.rs`、`parity_test.rs` 和 Go 对照；`node --file cmd/benchdb/main.rs` 读取了全文件；`query run_with_flags --kind function` 和 `query new_bench_db --kind function` 精确定位入口。`callers/callees` 查询无输出且长时间未完成，已中断，未据此推断调用关系。
- 生产与装配文件：`cmd/benchdb/main.rs`、`cmd/benchdb/lib.rs`、`cmd/benchdb/bin_main.rs`、`cmd/benchdb/stubs.rs`。
- crate 配置：`cmd/benchdb/Cargo.toml` 及根 `Cargo.toml` 的 workspace 成员项，确认该包是独立 binary、Go 映射为 `cmd/benchdb` 且当前依赖为空。
- Go 对照：`cmd/benchdb/main.go`，逐项核对 flags、初始化顺序、作业分派、SQL 模板、事务、计时与错误路径。
- 独立测试：`cmd/benchdb/parity_test.rs`；覆盖默认 flag、入口副作用、SQL/参数、batch 上取整、随机范围、未知作业停止、解析失败、执行/关闭失败、结果集读空和关闭、负 count 以及帮助参数。仓库搜索未发现与本模块相关的其他 Rust/Go 测试；同名 `insert_rows` 的其他命中属于无关模块。
- 本任务是纯文档分析，依计划不运行 Cargo。交付前使用任务规定的标题计数命令验证本文恰有十一个固定二级章节，并人工复核没有把 stub 能力表述成真实集群能力。
