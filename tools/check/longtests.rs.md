# `tools/check/longtests.rs`

## 文件定位

`tools/check/longtests.rs` 是 `astersql-tools-check` crate 的长耗时测试注册表。模块由 [`tools/check/lib.rs`](lib.rs) 以公开模块 `longtests` 装配，二进制入口 [`tools/check/bin_main.rs`](bin_main.rs) 经 crate 的 `main()` 进入 [`tools/check/ut.rs`](ut.rs) 的命令行驱动。它不执行测试，也不解析参数；它只向调度层提供“哪些测试属于长测”和“长测最多使用多少个 worker”两项静态策略。

所属 crate 由 [`tools/check/Cargo.toml`](Cargo.toml) 定义，库入口为 `lib.rs`，二进制入口为 `bin_main.rs`。本文件只使用标准库 `std::collections::HashMap`，没有直接使用该 crate 声明的 `regex` 外部依赖。

## 核心职责

- `long_tests` 保存需要在 `ut run --long` 模式运行的包名与测试名白名单。目前包含 `pkg/ttl/ttlworker` 的三个用例和 `pkg/ttl/cache` 的一个用例。
- `LONG_TEST_WORKER_COUNT` 把长测执行并发固定为 `2`。调度代码据此减少同时运行的高耗时任务，并在机器并行度更高时把更多 `-test.cpu` 配额分给单个长测。
- 清单和并发值是 Rust 版对 Go [`tools/check/longtests.go`](longtests.go) 的直接移植契约；两端不一致会改变 CI 实际选择的测试或资源策略。

该文件不是通用测试发现机制。普通模式仍由 `ut.rs` 枚举测试二进制中的用例；只有长测模式显式查阅这里的白名单。

## 主要符号

### `pub fn long_tests() -> HashMap<&'static str, Vec<&'static str>>`

每次调用构造并返回一个新的哈希表。键和值都是静态字符串切片：键为仓库相对 Go 包路径，值为该包内的 Go 测试函数名列表。当前映射为：

- `pkg/ttl/ttlworker` → `TestParallelLockNewJob`、`TestParallelLockNewTask`、`TestJobManagerWithFault`；
- `pkg/ttl/cache` → `TestRegionDisappearDuringSplitRange`。

函数是公开 API，直接调用点位于 `ut.rs` 的 `cmd_run` 与 `list_long_tasks`，测试调用点位于 [`tools/check/parity_test.rs`](parity_test.rs) 的 `contract_normal_paths`。

### `pub const LONG_TEST_WORKER_COUNT: usize = 2`

公开编译期常量，表达长测专用 worker 数。`ut.rs` 的 `run_test_cases` 用它选择 worker 数，`Numa::test_command` 用它计算单用例的 `-test.cpu` 值；`parity_test.rs` 断言其值为 `2`。

文件中没有自定义类型、trait、`impl`、宏或条件编译项。

## 执行流程

1. `bin_main.rs::main` 调用 `astersql_tools_check::main`，再由 `lib.rs::main` 进入 `ut::main`；命令行解析将 `--long` 写入 `UtState.long`。
2. `ut.rs::cmd_run` 在长测模式调用 `long_tests()`，以映射的键替换普通包发现结果，所以无显式包参数时只构建两个登记包。
3. 对每个目标包，`ut.rs::list_long_tasks` 再调用 `long_tests()` 查表，将登记的测试名转换为 `Task { pkg, test }`。未知包查不到条目时保持原任务向量不变。
4. `ut.rs::run_test_cases` 在长测模式忽略普通 `state.p` worker 数，使用 `LONG_TEST_WORKER_COUNT` 启动两个 worker，并通过容量为 100 的同步通道分发任务。
5. `ut.rs::Numa::test_command` 在 `state.long && state.p > LONG_TEST_WORKER_COUNT` 时，将每个测试进程的 `-test.cpu` 设为 `state.p / LONG_TEST_WORKER_COUNT`；长测超时走 30 分钟分支。
6. 每个任务最终以精确的 `-test.run ^<测试名>$` 参数运行。因此本文件中的名字必须与对应 Go 测试函数完全相同。

映射采用 `HashMap`，所以遍历包键时不承诺固定顺序；任务在执行前还会被 `shuffle`。当前契约关心集合与资源上限，不依赖稳定执行顺序。

## 数据与状态

注册数据全部由字符串字面量和一个 `usize` 常量组成，不读取环境变量、文件或网络，也没有可变全局状态。`long_tests()` 不返回共享静态引用容器，而是每次新建 `HashMap` 与内部 `Vec`；调用者取得所有权并可局部修改，但不会改变后续调用结果。

所有字符串的生命周期均为 `'static`，因为它们来自编译期字面量。容器本身仍会在每次调用时分配。现有调用规模只有两个键、四个值，分配开销相对构建和运行测试进程可忽略；若清单显著增大，可评估惰性静态表或只读切片，但这会改变公开返回类型及调用方式，不能只在本文件内无验证地替换。

`LONG_TEST_WORKER_COUNT` 的有效不变量是正数。若设为 `0`，`run_test_cases` 不会创建消费者，发送任务时可能阻塞；`Numa::test_command` 的除法分支也可能除零。因此扩展时不能把它当作“禁用长测”的开关。

## 依赖与调用关系

上游装配和调用关系如下：

- `tools/check/lib.rs`：用 `#[path = "longtests.rs"] pub mod longtests` 将本文件纳入 crate。
- `tools/check/ut.rs`：导入两个公开符号；`cmd_run` 选择包，`list_long_tasks` 展开测试，`run_test_cases` 限制 worker，`Numa::test_command` 分配 CPU。
- `tools/check/parity_test.rs`：在测试编译目标中导入两个符号，验证清单、并发值和 `--long` 调度结果。

下游只有标准库 `HashMap::from` 与 `Vec` 构造。本文件不直接依赖 `ut.rs` 的 `Task`、进程桩或线程类型，因而注册数据与执行机制保持单向依赖。

RustCodeGraph 的文件节点报告本文件被 `tools/check/ut.rs` 和 `tools/check/parity_test.rs` 两个文件使用；本地引用搜索进一步确认上述具体调用位置。精确 `callers`/`callees` 命令本次没有产生可用文本，因此调用细节以索引文件关系和源码引用共同核对。

## 错误处理与边界

`long_tests()` 是纯构造函数，没有 `Result`/`Option` 返回和显式失败分支。内存分配失败遵循 Rust 标准分配器的进程级失败行为，本模块不单独恢复。

真正需要防守的是数据边界：

- 包名不在表中时，`list_long_tasks` 静默返回原列表，与 Go 对空 map 查询后遍历零次的行为一致。
- 测试名拼错、测试被重命名或删除时，本模块不会提前发现；执行阶段会用精确正则启动目标测试，因此必须同步核对测试定义。
- 重复测试名会生成重复任务；重复或错误包键会改变构建和执行范围。
- 哈希表顺序不稳定，调用者不得以返回顺序构建确定性断言。
- 并发常量必须大于零，并应结合 CI 资源和单测试 CPU 分配一起评估，而不能只观察线程数。

当前四个测试的 Go 定义分别位于 `pkg/ttl/ttlworker/job_manager_integration_test.go`、`pkg/ttl/ttlworker/task_manager_integration_test.go` 和 `pkg/ttl/cache/split_test.go`。

## 并发与资源生命周期

本文件自身不创建线程、通道、锁或子进程。它提供的 `LONG_TEST_WORKER_COUNT` 由 `run_test_cases` 消费：后者创建两个 worker 线程，使用容量 100 的同步通道传递 `Task`，关闭发送端后等待所有 worker 完成，再汇总 JUnit 与覆盖率结果。

同一常量还影响子进程资源：当用户并行度 `p` 大于 2 时，每个长测获得整数除法计算的 `p / 2` 个 `-test.cpu` 配额。长测模式也在 `Numa::test_command` 中使用 30 分钟超时。由此，修改并发值会同时改变并发进程数和单进程 CPU 配额，存在 CI 耗时、资源竞争与稳定性风险。

`long_tests()` 返回的映射只活到各调用者局部作用域结束，不跨线程共享。调度阶段会把条目复制成拥有 `String` 的 `Task`，因此 worker 生命周期不依赖本函数创建的映射。

## 与 Go 版本的对应关系

Rust `long_tests()` 对应 `tools/check/longtests.go` 的包级变量 `longTests`，Rust `LONG_TEST_WORKER_COUNT` 对应 Go 变量 `longTestWorkerCount`。本次逐项核对，两端均为两个包、四个测试、worker 数 `2`。

语义上的主要实现差异是：Go map 在进程生命周期内作为可变包级变量存在；Rust 函数每次返回新容器，避免可变全局状态。对当前只读调用行为，两者等价，并且两端 map/`HashMap` 都不保证遍历顺序。Go `listLongTasks` 对缺失键遍历 `nil` 切片，Rust 用 `if let Some` 跳过，外部结果相同。

Go `cmdRun`、`runTestCases`、`listLongTasks` 和 `numa.testCommand` 分别对应 Rust `cmd_run`、`run_test_cases`、`list_long_tasks` 和 `Numa::test_command`，且在包选择、任务展开、worker 限制、CPU 配额和超时策略上保持相同意图。

## 扩展指南

新增或移除长测时，应同时完成以下局部变更：

1. 在 `long_tests()` 中按真实仓库相对包路径维护测试名，保持与 `tools/check/longtests.go::longTests` 完全一致。
2. 确认目标 Go 测试函数真实存在且名称大小写精确匹配；不要把 Rust 独立测试名误填入该 Go 测试驱动清单。
3. 更新 `tools/check/parity_test.rs::contract_normal_paths` 的注册表断言；若总任务数变化，还要更新 `contract_boundary` 中对 `--long` 全链路执行次数的断言及进程桩所需包/二进制。
4. 测试逻辑应继续放在独立的 `parity_test.rs`，不要嵌入生产源文件。
5. 若调整 `LONG_TEST_WORKER_COUNT`，同步修改 Go 常量、parity 断言，并验证 `run_test_cases` 的 worker 行为与 `Numa::test_command` 的 `-test.cpu` 整数除法在最低和较高 `p` 值下仍合理。

兼容性风险主要是遗漏或误跑 CI 用例；性能风险主要是同时运行过多长测或给单个测试过少 CPU。注册表规模很小时无需为减少容器分配而增加全局同步或复杂初始化。

## 验证依据

- 源文件：`tools/check/longtests.rs`，确认唯一函数、唯一常量、完整清单与无条件编译结构。
- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`node --file tools/check/longtests.rs` 识别 41 行源码并报告使用文件为 `tools/check/ut.rs`、`tools/check/parity_test.rs`；`query long_tests --json` 定位公开函数签名。
- crate 边界：`tools/check/Cargo.toml`、`tools/check/lib.rs`、`tools/check/bin_main.rs`。
- Rust 调用与测试：`tools/check/ut.rs` 的 `cmd_run`、`run_test_cases`、`list_long_tasks`、`Numa::test_command`；`tools/check/parity_test.rs` 的 `contract_normal_paths`、`contract_boundary`。
- Go 对照：`tools/check/longtests.go`、`tools/check/ut.go` 的 `cmdRun`、`runTestCases`、`listLongTasks`、`numa.testCommand`。
- 登记测试定义：`pkg/ttl/ttlworker/job_manager_integration_test.go`、`pkg/ttl/ttlworker/task_manager_integration_test.go`、`pkg/ttl/cache/split_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用任务指定的十一章节结构检查，并人工复核 Rust/Go 清单、直接引用和独立测试证据。
