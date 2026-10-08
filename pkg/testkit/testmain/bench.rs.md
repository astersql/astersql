# [`pkg/testkit/testmain/bench.rs`](./bench.rs)

## 文件定位

该文件属于 `astersql-testkit-testmain` crate；crate 根由 `pkg/testkit/testmain/Cargo.toml` 的 `[lib] path = "lib.rs"` 指向 `pkg/testkit/testmain/lib.rs`。`lib.rs` 将 `bench` 声明为公开模块并通过 `pub use bench::*` 再导出其公开函数，因此调用方可从 `astersql_testkit_testmain` crate 根直接使用 `benchmark_exit_code` 和 `ShortCircuitForBench`。

它位于测试进程入口层，而不是数据库 SQL、事务或存储执行链中。其职责是复刻 Go 测试框架在 `TestMain` 开头的 benchmark 短路：检测非空 `test.bench` 过滤条件时，先运行测试运行器，再把运行器的退出码作为进程退出码；没有有效 benchmark 条件时，把控制权留给调用方继续执行普通测试初始化。Go 侧直接入口是同目录的 `pkg/testkit/testmain/bench.go::ShortCircuitForBench`。

## 核心职责

- `test_flag_takes_value` 维护会影响命令行扫描位置的 Go testing 非布尔标志集合。扫描器遇到这些标志的分离值写法时会同时消费下一参数，避免把该值误判为“首个非 flag 参数”而过早停止。
- `benchmark_exit_code` 将命令行解析与进程退出拆开，接受可注入参数序列和 `&dyn TestingM`，从而可以在不终止测试进程的情况下验证短路判定和运行次数。
- `ShortCircuitForBench` 是进程侧包装器：读取 `std::env::args_os()`，仅在 `benchmark_exit_code` 返回 `Some(exit_code)` 时调用 `std::process::exit(exit_code)`。

本文件只判断 benchmark 是否应短路并传递运行结果，不负责构造测试运行器、普通测试环境初始化、failpoint、全局配置或泄漏检查。这些后续阶段由各测试 harness 负责；例如 `pkg/session/test/main_test.go::TestMain` 把 Go 版 `ShortCircuitForBench` 放在 `SetupForCommonTest` 和 `flag.Parse` 之前。

## 主要符号

- `fn test_flag_takes_value(argument: &str) -> bool`：私有纯判断函数。它匹配 `-test.*` 与 `--test.*` 两种前缀下需要独立值的 testing 标志，包括 `test.run`、`test.timeout`、profile、fuzz、parallel 等；`test.bench` 由主扫描逻辑专门处理，不在此表中。
- `pub fn benchmark_exit_code<I, S>(testing_m: &dyn TestingM, args: I) -> Option<i32>`：可测试的核心入口。`I: IntoIterator<Item = S>`、`S: AsRef<OsStr>` 允许数组、向量或 OS 字符串序列作为输入；`None` 表示不短路，`Some(code)` 表示已恰好调用一次 `TestingM::run` 并获得退出码。
- `pub fn ShortCircuitForBench(testing_m: &dyn TestingM)`：公开的 Go 风格命名入口，以真实进程参数调用核心函数；存在有效 benchmark 时不返回。`#[allow(non_snake_case)]` 明确保留与 Go API 一致的名称。
- `super::TestingM`：定义在 `pkg/testkit/testmain/wrapper.rs` 的最小运行器 trait，唯一方法为 `fn run(&self) -> i32`。本文件只依赖该抽象，不持有具体测试运行器。

## 执行流程

1. `ShortCircuitForBench` 取得 `std::env::args_os()` 并调用 `benchmark_exit_code`。
2. `benchmark_exit_code` 跳过第一个参数（可执行文件名），创建可向前消费的迭代器，并以 `benchmark_enabled: Option<bool>` 保存最近一次 `test.bench` 设置。
3. 扫描遇到 `--`、单独的 `-` 或首个不以 `-` 开头的参数时立即结束，对齐 Go `flag.Parse` 的停止位置；这些位置之后的 `test.bench` 不参与判定。
4. 对 `-test.bench=<value>` 或 `--test.bench=<value>`，记录 `<value>` 是否非空；对分离写法 `-test.bench <value>` 或 `--test.bench <value>`，消费下一参数并按其是否非空记录，缺值等同于禁用。
5. 重复的 benchmark 标志不会立即运行，后出现的值覆盖前值。`pkg/testkit/testmain/bench_test.rs::benchmark_uses_the_last_repeated_flag_value` 分别验证“空后非空”会运行、“非空后空”不运行。
6. 对 `test_flag_takes_value` 识别的其他 testing 标志，额外消费其分离值后继续扫描。`benchmark_after_another_test_flag_value_is_parsed` 证明 `-test.run TestDDL` 不会挡住后续 benchmark。
7. 扫描完成后，只有最终状态为 `true` 才调用 `testing_m.run()` 并返回 `Some(exit_code)`；未出现、空值或最终被空值覆盖都返回 `None`。
8. `ShortCircuitForBench` 收到 `Some` 后立即 `std::process::exit`；收到 `None` 时正常返回，让 harness 继续后续阶段。

## 数据与状态

核心函数的持久状态只有局部变量 `benchmark_enabled: Option<bool>`：`None` 表示尚未观察到 benchmark 设置，`Some(false)` 表示最近设置为空或分离值缺失，`Some(true)` 表示最近设置非空。最终通过 `unwrap_or(false)` 把“未设置”与“禁用”统一为不运行。

参数按迭代器单向消费，不复制整个命令行。每个参数先经 `AsRef<OsStr>` 借用，再用 `to_string_lossy()` 临时转换为可匹配文本；非 UTF-8 字节会被替换字符表示，因此只有能够匹配 ASCII `-test.*` 前缀的参数具有控制意义。返回值保留 `TestingM::run` 的原始 `i32`，不解释成功或失败，也不改写退出码。

文件没有全局可变状态、缓存或数据库状态。唯一进程级输入是 `ShortCircuitForBench` 读取的当前命令行，唯一不可逆副作用是有效 benchmark 路径上的进程退出。

## 依赖与调用关系

RustCodeGraph 给出的文件内调用链为 `ShortCircuitForBench → benchmark_exit_code → test_flag_takes_value`，有效 benchmark 分支还调用 `TestingM::run`。`ShortCircuitForBench` 依赖标准库的 `std::env::args_os` 与 `std::process::exit`；`benchmark_exit_code` 依赖 `std::ffi::OsStr`、迭代器和 `wrapper.rs::TestingM`，Cargo manifest 没有为此 crate 声明额外第三方依赖。

上游分两类：

- 生产式测试入口通过 `lib.rs` 的公开再导出取得 API。当前 Rust harness 主要直接调用可测试核心函数，例如 `pkg/session/test/main_test.rs::session_harness_preserves_testmain_control_flow`、`pkg/session/test/bootstraptest/main_test.rs::bootstrap_harness_preserves_benchmark_and_global_config_side_effects`、`pkg/session/test/variable/main_test.rs::variable_harness_preserves_testmain_control_flow`。
- Go 的多个 `TestMain` 直接调用同目录 Go API；`pkg/session/test/main_test.go::TestMain` 是完整顺序证据，短路位于公共测试设置、显式 `flag.Parse`、配置修改、failpoint 和 goleak 包装之前。

下游只有测试运行器抽象：本文件不感知 `WrapTestingM` 的内部回调，但传入包装器时，调用 `run` 会自然触发 `pkg/testkit/testmain/wrapper.rs::TestingM for WrapTestingM` 的“底层运行器后处理退出码”逻辑。

## 错误处理与边界

该 API 不返回 `Result`，也不产生自定义错误。解析失败情形被折叠为“不短路”：没有参数、没有 benchmark、空 benchmark、分离写法缺值均返回 `None`。有效路径上的运行失败由 `TestingM::run` 的非零退出码表达，并原样返回或用于退出进程。

关键边界如下：

- 参数 0 始终视为程序名并跳过；调用测试辅助函数时必须包含这一占位项。
- `--`、`-`、首个位置参数是解析终点；其后的 benchmark 标志被忽略，见 `benchmark_flag_after_argument_or_double_dash_is_not_parsed`。
- 重复 benchmark 采用最后值，不是“任意一次非空即运行”。
- 只有 `test_flag_takes_value` 白名单中的其他标志会消费下一项。若 Go testing 新增需要值的标志而此表未同步，后续扫描可能过早停止，这是明确的兼容维护点。
- `ShortCircuitForBench` 的 `std::process::exit` 不执行 Rust 栈展开与局部析构；需要清理的测试资源必须由 `TestingM::run` 自身完成，或避免在调用短路入口前创建。
- `to_string_lossy` 意味着本实现不是通用的无损 OS 参数解析器；它仅针对 ASCII testing 标志协议。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或文件句柄。`benchmark_exit_code` 在调用线程同步扫描参数，并在满足条件时同步调用一次 `TestingM::run`；测试中的 `Cell<usize>` 计数器也证明当前契约只要求单线程可观察调用次数，而不声明 `TestingM: Send + Sync`。

借用生命周期局限于函数调用：参数项只在当前循环迭代中借用，`&dyn TestingM` 不被保存。正常返回路径不拥有需回收资源。进程包装器的有效 benchmark 路径以 `std::process::exit` 结束整个进程，因此不会运行当前线程栈上的析构器；这正是它应在 harness 其他资源初始化之前调用的原因。

## 与 Go 版本的对应关系

Go `pkg/testkit/testmain/bench.go::ShortCircuitForBench` 在 flag 尚未解析时调用 `flag.Parse()`，随后通过 `flag.Lookup("test.bench")` 读取最终值；值非空时执行 `os.Exit(m.Run())`。Rust 版保持三个核心行为：只对非空 benchmark 过滤串短路、调用运行器并透传退出码、进程入口立即退出。

Rust 为可测试性额外拆出 `benchmark_exit_code`，并手工扫描参数来近似 Go `flag` 的顺序语义。它显式支持单横线/双横线、等号/分离值、重复设置、解析终止符，以及已列出的 testing 值标志。与 Go 全局 flag 注册表不同，Rust 不查询动态注册的 flag，也没有“已解析”状态；兼容性取决于本文件的标志白名单与 Go testing 标志保持同步。Go 入口接收具体 `*testing.M`，Rust 则通过 `TestingM` trait 支持真实或伪运行器。

独立 Rust 测试 `pkg/testkit/testmain/bench_test.rs` 覆盖重复值、解析终止和跨其他值标志继续扫描；`pkg/testkit/testmain/migration_aster_unit_test.rs` 覆盖无标志、空过滤串、分离非空过滤串及退出码透传。当前目录没有 `bench_test.go`，Go 行为的直接依据是 `bench.go` 和调用它的各 `main_test.go`。

## 扩展指南

- Go testing 增加新的“值与标志分离”选项时，在 `test_flag_takes_value` 同时加入单横线和双横线形式，并在独立的 `pkg/testkit/testmain/bench_test.rs` 增加“该值不会截断后续 benchmark 扫描”的回归用例。
- 修改 benchmark 语法、重复值规则或停止位置时，集中改 `benchmark_exit_code`，不要把解析逻辑放回 `ShortCircuitForBench`；后者应继续只负责真实参数与进程退出，以免测试必须启动子进程。
- 扩展运行器行为时优先修改 `pkg/testkit/testmain/wrapper.rs::TestingM` 或包装器，并同步其独立测试 `wrapper_test.rs`；不要让本文件耦合具体 session、配置或 failpoint 类型。
- 若要验证 `std::process::exit` 本身，只能使用隔离子进程测试，避免在普通单元测试进程中调用 `ShortCircuitForBench` 的命中路径。
- 兼容性风险主要来自 Go flag 语义漂移和未知值标志；性能风险很低，扫描为参数数量的线性复杂度且不保留集合。安全修改应同时复查 Go `bench.go`、Rust 两个 bench 测试文件以及至少一个真实 harness 的调用顺序。

## 验证依据

- RustCodeGraph 索引状态：项目含 `pkg/testkit/testmain/bench.rs` 的 6 个索引节点；文件查询显示 136 行源码，并列出 `pkg/session/test/bootstraptest/main_test.rs`、`pkg/session/test/main_test.rs`、`pkg/session/test/variable/main_test.rs`、`pkg/testkit/testmain/bench_test.rs` 等使用方。
- RustCodeGraph 精确符号：`bench.rs::test_flag_takes_value`（第 25 行）、`bench.rs::benchmark_exit_code`（第 84 行）、`bench.rs::ShortCircuitForBench`（第 132 行）。节点调用轨迹确认 `ShortCircuitForBench` 调用 `benchmark_exit_code`，后者调用 `test_flag_takes_value` 和运行器 `run`。
- 源码与边界：[`bench.rs`](./bench.rs)；crate 声明：[`Cargo.toml`](./Cargo.toml)；公开模块与测试装配：[`lib.rs`](./lib.rs)；运行器 trait：[`wrapper.rs`](./wrapper.rs)。
- Go 对照与真实入口：[`bench.go::ShortCircuitForBench`](./bench.go)、[`pkg/session/test/main_test.go::TestMain`](../../session/test/main_test.go)。
- 独立测试：[`bench_test.rs`](./bench_test.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)；补充 harness 证据：[`pkg/session/test/main_test.rs`](../../session/test/main_test.rs)、[`pkg/session/test/bootstraptest/main_test.rs`](../../session/test/bootstraptest/main_test.rs)、[`pkg/session/test/variable/main_test.rs`](../../session/test/variable/main_test.rs)。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务文件指定的结构命令验证目标文档存在且固定二级标题恰好为 11 个，并人工复核唯一输出、源码链接、调用边、边界条件和扩展测试位置。
