# `pkg/util/benchdaily/bench_daily.rs`

## 文件定位

本文件是 workspace crate `astersql-util-benchdaily` 的核心实现，负责把一组 Rust 微基准函数运行后写成与 Go `pkg/util/benchdaily` 兼容的 JSON。crate 边界由 `pkg/util/benchdaily/Cargo.toml` 定义，`pkg/util/benchdaily/lib.rs` 通过 `pub mod bench_daily` 和 `pub use bench_daily::*` 导出本文件 API；根门面还在 `pkg/lib.rs` 的 `util::benchdaily` 中重导出该 crate。

它属于测试/CI 性能数据采集设施，而不是 SQL 请求、规划、执行或存储运行时主链。实际使用者是各模块的 benchmark 测试入口，例如 `pkg/session/bench_test.rs::benchdaily_run`、`pkg/statistics/handle/cache/bench_test.rs::TestBenchDaily` 和 `pkg/planner/core/casetest/tpch/tpch_test.rs::test_bench_daily_registration_matches_go`。这些入口把 `fn(&mut Benchmark)` 函数列表交给 `Run`；只有命令行提供非空 `outfile` 时才真正执行并落盘。

## 核心职责

1. 用 `BenchmarkFn`、`BenchmarkCase` 和 `Benchmark` 表示可运行的基准函数、带静态名称的用例及迭代状态。
2. 用 `execute_benchmark` 固定执行 100 个逻辑迭代，按总耗时除以 100 计算 `ns/op`。
3. 用 `BenchResult`/`BenchOutput` 定义与 Go 导出字段一致的 PascalCase JSON 协议，并提供结果转换和文件读写。
4. 用 `Run` 解析 `--outfile`、`-outfile` 及其 `=value` 形式，从函数指针恢复短名称后运行所有用例。
5. 保留 `benchmarkResultToJSON`、`callerName`、`readBenchResultFromFile`、`writeBenchResultToFile` 等 Go 风格兼容拼写，便于机械迁移调用点逐步改用 Rust 风格 API。

本实现不是完整的 Rust benchmark 框架：它没有 Go `testing.Benchmark` 的自适应迭代和分配统计能力，当前只保留日常结果采集所需的最小行为。

## 主要符号

- `BenchDailyResult<T> = Result<T, Box<dyn Error + Send + Sync>>`：文件 IO 与 JSON API 的统一可传播错误类型。
- `BenchmarkFn = fn(&mut Benchmark)`：基准入口 ABI；只接受无捕获函数指针，不接受携带环境的闭包。
- `BenchmarkCase { name, function }` 与 `BenchmarkCase::new`：把静态名称绑定到函数指针。字段私有，外部调用者通过构造器或宏创建。
- `benchmark_case!`：以 `stringify!($function)` 在编译期保存函数路径文本，避免运行时符号解析；这是 `run_to_file` 的推荐具名入口。
- `BenchOutput { date, commit, result }`：每日汇总文件的数据模型。本文件只定义并序列化该类型，扫描和合并流程位于独立测试 `pkg/util/benchdaily/bench_daily_test.rs`。
- `BenchResult { name, ns_per_op, allocs_per_op, bytes_per_op }`：单个用例的公开、可序列化结果。`#[serde(rename_all = "PascalCase")]` 生成 `Name`、`NsPerOp`、`AllocsPerOp`、`BytesPerOp`。
- `BenchmarkResult`：原生 runner 的内部指标值；字段私有，通过 `new`、`ns_per_op`、`allocs_per_op`、`allocated_bytes_per_op` 访问。
- `Benchmark { iterations }`：传给基准函数的最小状态。`iterations()` 暴露计划次数，`iter()` 每次调用操作并用 `std::hint::black_box` 消费返回值。
- `benchmark_result_to_json`：把内部指标和名称映射成 `BenchResult`，不做单位或范围转换。
- `caller_name<F>`：通过 `std::any::type_name::<F>()` 取得函数项类型名的最后一段，适用于调用点仍保留具体函数项类型时。
- `caller_name_from_pointer`：供兼容入口 `Run` 使用；通过 `backtrace::resolve` 解析函数地址、去除 Rust hash 后缀并提取最后一个名称片段。
- `execute_benchmark`：私有计时核心，固定 `ITERATIONS = 100`，调用基准函数一次，由基准函数使用 `Benchmark::iter` 完成逻辑迭代。
- `run_to_file`：显式名称 API；空输出路径直接成功返回，否则依次执行全部 `BenchmarkCase` 并写文件。
- `Run`：Go 兼容 API；解析进程参数、解析函数指针名称、构造用例并调用 `run_to_file`，错误时 panic。
- `read_bench_result_from_file` / `write_bench_result_to_file`：返回 `Result` 的 Rust 风格 JSON 文件 API；后者创建或截断文件并确保尾部换行和 flush。

## 执行流程

推荐流程从 `benchmark_case!(sample_benchmark)` 开始：宏生成带静态名称的 `BenchmarkCase`，`run_to_file` 首先检查输出路径。路径为空时不运行任何基准、不创建文件；路径非空时按输入切片顺序处理每个用例。

对每个用例，`execute_benchmark` 创建 `Benchmark::new(100)`，记录 `Instant::now()`，再调用一次用例函数。用例应通过 `Benchmark::iter` 执行操作 100 次；结束后总纳秒数除以 100，超出 `i64` 时饱和为 `i64::MAX`，分配次数和分配字节数均写 0。`benchmark_result_to_json` 随后附加用例名称，最终由 `write_bench_result_to_file` 一次性写成 JSON 数组和换行。

兼容流程从 `Run(Vec<BenchmarkFn>)` 开始。它扫描 `std::env::args().skip(1)`，后出现的受支持 outfile 参数会覆盖先前值；分离形式缺少值时使用空字符串。每个函数指针经 `caller_name_from_pointer` 解析名称，再由 `Box::leak` 提升为 `'static` 字符串以构造 `BenchmarkCase`。随后复用 `run_to_file`；任何返回错误都转换为 panic。

读路径 `read_bench_result_from_file` 只接受 `Vec<BenchResult>` JSON，而不是 `BenchOutput` 汇总对象。每日汇总测试先读取多个这样的数组，再构造 `BenchOutput`。

## 数据与状态

所有结果模型都按值持有数据，没有全局可变状态。`BenchOutput` 和 `BenchResult` 派生 `Serialize`/`Deserialize`，其 JSON 字段大小写由 serde 属性稳定约束；`BenchOutput` 的 `date` 与 `commit` 不在本文件内生成或校验。

`Benchmark` 只保存不可从外部修改的 `u64` 迭代数。runner 固定为 100，因此调用者不能请求自适应采样；基准函数也必须遵守“每个逻辑迭代执行一次待测操作”的约定，否则 `ns/op` 的分母与真实操作数不一致。

`Run` 为每个动态解析出的名称执行一次 `Box::leak`。这些字符串在进程生命周期内不会释放；源码将其限定为短生命周期命令的兼容成本。直接使用 `run_to_file` 与 `benchmark_case!` 不需要泄漏。

输出通过 `File::create` 创建或截断，`BufWriter` 缓冲，写完显式 `flush`。文件不是追加模式，也没有原子临时文件替换或跨进程锁；同一路径的并发写者会互相覆盖或产生竞争结果。

## 依赖与调用关系

`pkg/util/benchdaily/Cargo.toml` 声明三个运行依赖：`backtrace` 用于函数指针符号解析，`serde` 用于结果模型派生，`serde_json` 用于流式读写；`tempfile` 仅供测试。crate 的 porting 元数据把 Go 来源标记为 `pkg/util/benchdaily`。

上游接线有两层：`pkg/util/benchdaily/lib.rs` 导出本文件全部公开符号，workspace 根 `Cargo.toml` 将其注册为成员并用 `facade_util_benchdaily` 引用，`pkg/lib.rs::util::benchdaily` 再提供统一门面。直接依赖 crate 的测试包在各自 `Cargo.toml` 中以路径依赖声明，例如 `pkg/session/Cargo.toml`、`pkg/distsql/Cargo.toml` 和 `pkg/util/codec/Cargo.toml`。

真实调用边包括：

- `pkg/session/bench_test.rs::benchdaily_run -> Run`，其中 `daily_adapter!` 把既有 `usize` 迭代接口适配为 `&mut Benchmark`。
- `pkg/statistics/handle/cache/bench_test.rs::TestBenchDaily -> Run`，注册六个缓存基准。
- `pkg/planner/core/casetest/tpch/tpch_test.rs::test_bench_daily_registration_matches_go -> Run`，注册 TPCH 基准列表。
- `pkg/util/benchdaily/migration_aster_unit_test.rs::run_executes_benchmarks_and_writes_results -> run_to_file -> execute_benchmark / write_bench_result_to_file`。
- `Run -> caller_name_from_pointer -> backtrace::resolve`；`run_to_file -> benchmark_result_to_json`；读写函数分别下沉到 `File`、`BufReader`/`BufWriter` 和 `serde_json`。

RustCodeGraph 已索引目标文件及 20 个符号，但精确 callers/callees 查询未返回可用边；以上调用边因此由模块入口、Cargo 路径依赖和调用点源码直接核验，而不是根据架构概览推断。

## 错误处理与边界

Rust 风格的 `run_to_file`、`read_bench_result_from_file` 和 `write_bench_result_to_file` 用 `BenchDailyResult` 传播打开、创建、解析、序列化、写入和 flush 错误。读取非法或结构不匹配的 JSON 会返回错误，不返回部分结果；`File::create` 会截断已有文件。

Go 风格兼容函数保持更强硬的失败语义：`Run`、`readBenchResultFromFile`、`writeBenchResultToFile` 把错误转换为 panic。它们适合测试/CI 命令入口，不应直接用于需要恢复或向用户返回结构化错误的服务路径。

空路径是明确的快速返回边界。`Run` 遇到未知参数会忽略；分离式 outfile 缺值会退化为空路径并静默跳过。名称解析失败时不会报错，而是使用格式化的函数地址；符号裁剪依赖编译器/平台符号格式，因此 `benchmark_case!` 的静态名称更稳定。

计时只检查 `elapsed / 100` 到 `i64` 的转换溢出并饱和；它不校验基准函数是否实际调用 `iter()`、是否 panic、是否产生副作用，也不隔离单个用例失败。任一基准 panic 会中止当前批次，且文件要到全部用例完成后才创建写入。

## 并发与资源生命周期

runner 自身完全串行：按输入顺序逐个基准计时，`Benchmark::iter` 也在当前线程循环。基准函数可以自行创建线程，例如 `pkg/statistics/handle/cache/bench_test.rs` 使用 `std::thread::scope` 并发执行缓存操作；线程的 join 和同步责任属于基准函数，不属于本文件。

计时范围覆盖整个基准函数调用，因此包括基准函数在 `iter()` 外执行的准备、线程创建、同步和清理成本。扩展 runner 时若要区分 setup 与测量阶段，必须同时调整 API、计时边界和 Go 对照说明。

文件句柄由 RAII 在函数返回时关闭；写端在返回前显式 flush，读端由 `BufReader` 持有。没有后台任务、通道、锁或事务。`BenchmarkFn` 是普通函数指针，可在线程间传递，但当前入口不并行调度它们。

## 与 Go 版本的对应关系

数据协议直接对应 `pkg/util/benchdaily/bench_daily.go`：`BenchOutput`/`BenchResult` 字段及 PascalCase JSON 名称一致，`benchmark_result_to_json` 对应 `benchmarkResultToJSON`，文件 API 也保留了同名兼容包装。两端都在输出为空时跳过昂贵基准，都从函数入口恢复短名称，并以 JSON 数组加尾部换行写单批结果。

关键差异必须保留在使用预期中：

- Go `testing.Benchmark` 自适应选择 `B.N` 并提供真实 `NsPerOp`、`AllocsPerOp`、`AllocedBytesPerOp`；Rust `execute_benchmark` 固定 100 次，仅计算墙钟平均值，两个分配指标固定为 0。
- Go 使用全局 `flag` 包并在必要时 `flag.Parse()`；Rust 直接扫描进程参数，只识别四种 outfile 写法并忽略其他参数。
- Go `runtime.FuncForPC`/反射解析名称；Rust `Run` 用 `backtrace::resolve`，另提供更稳定的 `benchmark_case!` 和类型名版 `caller_name`。
- Go 文件错误调用 `log.Panic`/`log.Fatal`；Rust 核心 API 返回 `Result`，只有兼容包装 panic。
- Go 通过 defer 关闭文件；Rust 依赖 RAII，并在写端显式 flush。

`pkg/util/benchdaily/bench_daily_test.go` 的目录扫描和汇总行为没有进入本生产文件；Rust 对应逻辑当前位于独立测试 `pkg/util/benchdaily/bench_daily_test.rs`，只用于验证汇总语义。不能把该测试辅助实现描述为本 crate 的公开生产 API。

## 扩展指南

新增 runner 行为应优先修改 `run_to_file`/`execute_benchmark`，并在独立的 `pkg/util/benchdaily/migration_aster_unit_test.rs` 增加回归测试；不要把测试内嵌到生产文件。新增结果字段时需同步 `BenchResult`、`BenchmarkResult`、`benchmark_result_to_json`、Go 对照字段和精确 JSON 断言，同时评估历史结果消费者的兼容性。

若增加命令行选项，应把解析逻辑集中在 `Run`，覆盖分离形式、等号形式、重复参数和缺值边界。若需要可靠名称，优先让调用点使用 `BenchmarkCase`/`benchmark_case!`，避免进一步依赖平台符号；若改变现有 `Run(Vec<BenchmarkFn>)` 签名，要同步所有直接调用者及其 Cargo 依赖。

若实现自适应迭代或真实分配统计，必须与 Go `testing.Benchmark` 的预热、计时和指标定义核对，不能仅调整常量使测试通过。还需关注长基准耗时、整数溢出、计时噪声和不同平台 allocator 的性能风险。

若把每日扫描/合并提升为生产 API，应从 `pkg/util/benchdaily/bench_daily_test.rs` 提取到新的生产源文件，并保留独立测试；同时定义输入排序、`.git` 跳过、部分文件失败、输出原子性及并发写入策略。当前文件不应悄然承担这些未接线职责。

## 验证依据

- RustCodeGraph：`status` 显示项目索引包含 `pkg/util/benchdaily/bench_daily.rs`；`files --filter pkg/util/benchdaily` 列出 Rust/Go 源与独立测试；`node --file pkg/util/benchdaily/bench_daily.rs --offset 1 --limit 400` 读取到本文件完整 287 行及 20 个符号；精确 `query` 定位了 `run_to_file`、`execute_benchmark`、`caller_name_from_pointer` 和 `write_bench_result_to_file`。callers/callees 未产出可用边，调用关系改由调用点源码核验。
- 源与模块边界：`pkg/util/benchdaily/bench_daily.rs`、`pkg/util/benchdaily/lib.rs`、`pkg/util/benchdaily/Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`。
- Go 对照：`pkg/util/benchdaily/bench_daily.go`、`pkg/util/benchdaily/bench_daily_test.go`、`pkg/util/benchdaily/main_test.go`。
- Rust 独立测试：`pkg/util/benchdaily/migration_aster_unit_test.rs` 验证指标映射、JSON 精确格式与截断、非法 JSON、短名称和实际执行落盘；`pkg/util/benchdaily/bench_daily_test.rs` 验证汇总扫描、排序、`.git` 跳过及 `BenchOutput`。
- 直接调用证据：`pkg/session/bench_test.rs`、`pkg/statistics/handle/cache/bench_test.rs`、`pkg/planner/core/casetest/tpch/tpch_test.rs`，以及 `rg` 对 `Run`、`run_to_file`、兼容读写函数和 Cargo 路径依赖的检索结果。
- 本任务只新增说明文档，按计划不运行 Cargo；结构验证要求目标文件存在且恰有上述 11 个固定二级标题。
