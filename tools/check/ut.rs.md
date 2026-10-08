# `tools/check/ut.rs`

## 文件定位

`tools/check/ut.rs` 是 `astersql-tools-check` crate 的单元测试驱动主体，复刻同目录 Go 工具 [`tools/check/ut.go`](ut.go) 的命令行语义。crate 由 [`tools/check/Cargo.toml`](Cargo.toml) 定义，库入口 [`tools/check/lib.rs`](lib.rs) 将本文件声明为 `pub mod ut`，二进制壳 [`tools/check/bin_main.rs`](bin_main.rs) 依次调用 `astersql_tools_check::main()`、`ut::main()` 和 `run()`。它驱动的仍是 Go 包和 Go 测试二进制，不是 Rust 测试框架。

Cargo 元数据将该 crate 标为 `kind = "binary"`，且仅直接依赖 `regex`。进程执行、覆盖率解析和正则边界通过 [`tools/check/stubs.rs`](stubs.rs) 适配；生产默认执行真实子进程，测试可注入进程处理器。

## 核心职责

本文件把一次 `ut` 调用组织为五段流水线：

1. `run` 消费 `--junitfile`、`--coverprofile`、`--except`、`--only`、`--race`、`--short`、`--long`，初始化 `UtState`，再分派 `list`、`build`、`run`、`run-multi`。
2. `list_packages` 通过 `go list ./...` 枚举包，`skip_dir` 排除 `br`、`lightning`、`pkg/lightning`、`cmd`、`dumpling`、`tests`、`tools`、`build` 前缀。
3. `build_test_binary` 或 `build_test_binary_multi` 生成 `.test.bin`；批量路径先由 `generate_build_cache` 预热，再使用 `tools/bin/xprog`。
4. `list_new_test_cases` 用 `-test.list Test` 枚举测试，`cmd_run` 再应用名字、正则、`--only`、`--except` 或长测清单过滤。
5. `run_test_cases` 打散并并发执行 `Task`，随后按需汇总 JUnit 和 Go cover profile，以 worker 的失败标记决定命令成功与否。

## 主要符号

- `usage() -> bool`：打印与 Go 版一致的帮助文本；未知子命令也走此成功分支。
- `module_path() -> String`：构造逻辑模块前缀 `github.com/pingcap/tidb`，用于包名裁剪、完整导入路径和 JUnit `classname`。
- `Task { pkg, test }`：最小执行单元；其 `Display` 结果为 `"<pkg> <test>"`，也是 only/except 文件的匹配键。
- `UtState`：一次调用的配置快照，包括运行/构建并发度、仓库工作目录、报告路径、模式开关和过滤文件。
- `cmd_list`、`cmd_build`、`cmd_run_multi`、`cmd_run`：四个子命令入口；`cmd_run` 还覆盖不带子命令时的默认全量运行。
- `run_test_cases`：执行调度和结果收口中心。
- `Numa`：每个 worker 私有的失败位和 `TestResult` 集合；`run_one` 保持失败后继续消费任务，`run_test_case` 负责实际命令、有限重试和计时，`test_command` 负责参数组装。
- `TestCommand`：把程序、参数、工作目录和输出捕获配置转换为 `stubs::CommandSpec`；真正执行集中在 `stubs::run_command`。
- `merge_profile`、`append_with_reduce`：排序并归并覆盖率 block；相同坐标使用按位或合并 `count`，`num_stmt` 不一致时 panic。
- `JUnitTestSuites`、`JUnitTestSuite`、`JUnitTestCase`、`JUnitFailure` 等：轻量 JUnit 模型；`write` 和各 `to_xml` 方法手工序列化并转义属性/文本。

## 执行流程

正常运行链为 `bin_main::main → astersql_tools_check::main → ut::main → run`。`run` 先从参数向量移除所有全局 flag；启用覆盖率时创建 `cov<纳秒时间戳>` 临时目录；并发度取 `available_parallelism()`，构建并发为其两倍；工作目录取当前目录。随后按位置参数调用相应 `cmd_*`，结束前递归删除覆盖率临时目录，并把布尔结果转换为退出码 0/1。

全量 `cmd_run` 先构建所有包，再由 `list_test_cases_for_pkgs` 并发枚举测试；单包或单测路径只构建指定包并确认二进制存在。`--long` 用 [`tools/check/longtests.rs`](longtests.rs) 的静态映射代替二进制枚举。最终任务还会依次应用 except 和 only 集合。

`run_test_cases` 选择普通模式 `state.p` 个 worker，长测模式固定 `LONG_TEST_WORKER_COUNT` 个；任务经容量 100 的 `sync_channel` 发送。每个线程维护自己的 `Numa`，但接收端因标准库 MPSC 的限制包在 `Arc<Mutex<Receiver>>` 中。发送端关闭后，主线程等待所有 worker，再生成可选的 JUnit/覆盖率文件，并检查是否有 worker 失败。

单测执行命令固定以 `-test.run ^<name>$` 精确选择函数；普通非 race 模式超时 2 分钟，race 或 long 模式为 30 分钟。长测会按机器并发度为每个测试分配 `-test.cpu`。只有段错误、断点陷阱或输出包含 `panic during panic` 时才最多重试三次，普通测试失败不重试。

## 数据与状态

`UtState` 在 `run` 中建立，进入 worker 前被克隆到 `Arc`，线程只读，避免共享可变配置。任务以 `Vec<Task>` 形成、过滤并洗牌，随后通过有界通道转移所有权。每个 `Numa` 独占其 `Vec<TestResult>`；线程结束时才把整个 worker 推入共享 `Vec<Numa>`，最终在主线程克隆并聚合。

覆盖率中间文件位于 `cover_file_temp_dir`，文件名由包路径（平台分隔符替换成 `_`）和测试名组成。`collect_cover_profile_file` 将其解析为 `HashMap<String, CoverProfile>`，按源文件聚合 block 后写入最终 `mode: set` 文件。JUnit 汇总使用两个以 `classname` 为键的 `HashMap`，分别收集案例和累计时长，因此 suite 和 case 的输出顺序没有稳定排序保证。

本文件还依赖两个进程级状态：当前工作目录，以及 `stubs.rs` 中测试可替换的全局进程处理器。`GOVERSION` 可覆盖报告里的 Go 版本；`NEXT_GEN=1` 会使 `go_test_cmd` 使用 `--tags=intest,nextgen`。

## 依赖与调用关系

上游直接入口是 `tools/check/bin_main.rs` 和 `tools/check/lib.rs`；独立回归测试 [`tools/check/parity_test.rs`](parity_test.rs) 直接调用 `run`、过滤/合并 helper、JUnit 类型和 `Numa`。RustCodeGraph 对文件节点显示 `ut.rs` 有 67 个符号；对 `run_test_cases` 的 callee 边确认其调用 `shuffle`、`Numa::run_one`、`collect_test_results`、`write` 和 `collect_cover_profile_file`。

主要下游如下：

- 标准库：文件与路径、线程、`sync_channel`、`Arc<Mutex<_>>`、时间和进程退出。
- `crate::longtests::{long_tests, LONG_TEST_WORKER_COUNT}`：长测任务和专用 worker 数。
- `crate::stubs`：`run_command` 子进程边界、`compile_regex`、Go cover profile 解析及相关数据结构。
- 外部程序/文件：`go list`、`go test`、包内 `.test.bin`、`tools/check/go-compile-without-link.sh`、`tools/bin/xprog`。

`Cargo.toml` 的唯一第三方依赖 `regex` 实际由 `stubs.rs` 封装，本文件只通过 `compile_regex` 使用它。没有发现其他 crate 对该模块的显式 Rust 调用；生产主链由本 crate 的二进制入口触发。

## 错误处理与边界

命令层主要使用 `bool` 或退出码：包枚举、构建、过滤、测试执行或报告写入失败会使命令返回失败，`main` 再退出。可恢复 helper 多返回 `Result<_, String>`，`with_trace` 为非空错误附加 Rust backtrace。`parse_case_list_from_file` 把文件不存在解释为空集合，但其他读取错误上抛；它保留每行原文，不 trim 或校验格式。

几个边界必须特别注意：`cmd_list` 在无参数打印完包列表后返回 `false`，这是当前代码事实；未知子命令打印 usage 并返回成功。`list_new_test_cases` 排除历史特例 `TestT` 和 `TestBenchDaily`，且命令失败但已有输出时仍返回解析结果。`test_binary_exist` 把包括权限错误在内的元数据错误均视为“不存在”。覆盖率目录读取、profile 打开/解析和写入失败直接 `process::exit(255)`，无法由 `run` 清理或转成普通错误；同坐标 block 的 `num_stmt` 不一致则 panic。

`handle_flags` 延续 Go 的宽松行为：flag 缺值时得到空串。测试名被直接插入 `^...$`，没有正则转义；这是与 Go 版本一致的现状，扩展时不能无证据改变。手写 XML 会转义 `&`、引号和尖括号，但并未声称覆盖完整 XML 规范。

## 并发与资源生命周期

任务执行线程数至少为 1；长测固定使用 `LONG_TEST_WORKER_COUNT`。有界通道提供背压，发送者在缓冲满时等待；所有 sender 被销毁后 worker 从 `recv` 得到断开并退出，主线程逐一 `join`。线程 panic 的 join 错误当前被忽略，但该 worker 可能来不及写入 `works`，这是修改调度代码时需要保留或明确修正的风险点。

`Receiver` 外层互斥锁只包住一次阻塞 `recv`，取到任务后即释放；测试进程可并行执行。每个 worker 私有保存结果，只有收尾时短暂锁住 `works`。`list_test_cases_for_pkgs` 使用无界通道和每包一个线程，并保留收到的第一份错误；它不保存线程句柄，而是按已启动线程数接收结果。

覆盖率临时目录由 `run` 创建并在普通返回路径结束前 `remove_dir_all`；JUnit 文件通过作用域释放句柄。`collect_cover_profile_file` 的 `exit(255)`、进程被终止或 panic 会绕过普通清理。`parity_test.rs::contract_resource_cleanup` 验证最终 JUnit 和 cover 文件可读，但明确只做临时目录清理的 best-effort 观察。

## 与 Go 版本的对应关系

Rust 文件几乎按符号对应 [`tools/check/ut.go`](ut.go)：`task → Task`，Go 全局变量 → `UtState`，`cmdList/cmdBuild/cmdRunMulti/cmdRun → cmd_*`，`numa → Numa`，以及构建、枚举、覆盖率归并、JUnit 模型等同名 snake_case 函数。`modulePath`、跳过目录、构建标签、超时、重试条件、长测清单和输出文案均以 Go 实现为语义基准。

刻意的 Rust 化差异包括：全局可变状态收口到可克隆的 `UtState`；`run` 返回退出码以便测试，`main` 才真正退出；goroutine/channel 改为线程和标准库通道；`errgroup` 枚举改为线程加结果通道；`exec.Cmd` 和 Go cover/regexp 包改由本地 `stubs.rs` 适配；XML 改为手工序列化。另一个已注释差异是覆盖率临时目录用 `remove_dir_all` 清理，而 Go 使用 `os.Remove`。

[`tools/check/parity_test.rs`](parity_test.rs) 是相关独立 Rust 测试文件，覆盖正常路径、缺失过滤文件/跳过目录/长测边界、构建或用例失败、非法正则、JUnit 与覆盖率产物及资源收尾。Go 侧没有同名 `ut_test.go`；语义证据主要来自 `ut.go` 本体和 `longtests.go`。

## 扩展指南

- 新增全局 flag：在 `run` 中先用 `handle_flag(s)` 消费，再加入 `UtState`；同步检查单包构建、批量构建和 `Numa::test_command` 是否都需要传播，并在独立的 `parity_test.rs` 增加参数消费与命令参数断言。
- 新增子命令：扩展 `run` 的分派和 `usage`，保持返回布尔值/退出码约定；若涉及包范围，应复用 `list_packages` 和 `skip_dir`，避免形成第二套过滤规则。
- 修改调度或重试：集中在 `run_test_cases`、`Numa::run_one/run_test_case/test_command`，必须覆盖普通、race、short、long、失败后继续和已知崩溃重试；注意通道背压、线程 panic 与临时文件名冲突。
- 修改覆盖率：同步检查 `collect_cover_profile_file`、`merge_profile`、`append_with_reduce` 和 `stubs::parse_profiles_from_reader`；保持 block 排序/合并不变量并添加非法 profile、重复 block 和 `num_stmt` 冲突测试。
- 修改 JUnit：在本文件的数据模型与 XML 转义函数中完成，并在 `parity_test.rs` 断言统计、特殊字符和失败内容。不要把测试嵌回 `ut.rs`，仓库规则要求 Rust 源码与测试逻辑分文件。
- 任何与 Go 行为的有意偏离都应先对照 `ut.go` 对应符号并记录兼容影响；构建参数变化可能影响缓存、race/coverage 兼容与 CI 性能。

## 验证依据

- 已读生产与装配文件：`tools/check/ut.rs`（1725 行、67 个索引符号）、`tools/check/Cargo.toml`、`tools/check/lib.rs`、`tools/check/bin_main.rs`、`tools/check/stubs.rs`、`tools/check/longtests.rs`。
- 已读对照与测试：`tools/check/ut.go`（对应主流程和 58 个 Go 索引符号）、`tools/check/longtests.go`、`tools/check/parity_test.rs`；同目录没有 `ut_test.rs` 或 `ut_test.go`。
- RustCodeGraph：`status` 显示项目索引覆盖 11,467 个文件；`files --filter tools/check` 列出目标、入口、Go 对照和 parity 测试；`node --file tools/check/ut.rs --offset 620 --limit 100` 核对 `main/run`；`callees run_test_cases --limit 30` 核对调度下游。组合 `explore` 和部分精确 query/callers 在限定时间内未返回结果，因此调用方向又由 `lib.rs`、`bin_main.rs`、`parity_test.rs` 与源码中的直接调用复核，未将超时输出当作证据。
- 静态搜索：`rg` 枚举了本文件全部模块级 `pub fn`、结构体和 `impl`，并确认测试导入来自 `parity_test.rs`；Cargo 声明确认 crate 边界和 `regex` 依赖。
- 本任务只新增说明文档，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰有 11 个固定二级章节，并人工检查未把桩适配描述成完整通用实现、未建议把测试写入生产源文件。
