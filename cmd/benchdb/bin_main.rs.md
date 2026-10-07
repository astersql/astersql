# [`cmd/benchdb/bin_main.rs`](bin_main.rs)

## 文件定位

`bin_main.rs` 是 `astersql-cmd-benchdb` 包的可执行目标入口，而不是基准逻辑的实现文件。`cmd/benchdb/Cargo.toml` 的 `[[bin]]` 将二进制名 `astersql-cmd-benchdb` 映射到本文件，同时 `[lib]` 将同一包的库根映射到 `lib.rs`。Rust 会把包名中的连字符转换为 crate 路径中的下划线，因此本文件通过 `astersql_cmd_benchdb` 访问同包库目标。

该文件由操作系统启动生成的可执行程序时进入；源码中没有普通 Rust 调用者。RustCodeGraph 对文件给出的 `used by 0 files` 与这一入口属性一致。它不是门面式再导出模块，也没有条件编译项，而是一个只有 11 行的薄二进制包装层。

## 核心职责

本文件只有一个职责：在进程入口 `main` 中调用库入口 `astersql_cmd_benchdb::main()`。它刻意不持有命令行解析、日志初始化、存储注册、会话创建或作业调度逻辑，使二进制启动路径与库内测试观察到的是同一套实现。

真实装配链为 `bin_main.rs::main` → `lib.rs::main` → `main.rs::main` → `main.rs::run_with_flags`。因此，本文件是否正确的核心不变量是“无条件、恰好一次地把控制权交给库入口”，而不是在这里复制任何 `benchdb` 行为。

## 主要符号

- `fn main()`（`bin_main.rs:9`）：私有的 Rust 二进制入口，签名无参数、无显式返回值。函数体唯一语句是 `astersql_cmd_benchdb::main();`。
- 本文件没有模块级常量、类型、trait、`impl`、公开 API、条件编译属性或局部状态。
- `astersql_cmd_benchdb::main()` 的定义位于 `lib.rs:34`，是公开库函数；其函数体调用 `entry::main()`。`entry` 由 `lib.rs:24-25` 通过 `#[path = "main.rs"] pub mod entry` 指向 `main.rs`。

## 执行流程

1. Cargo 根据 `cmd/benchdb/Cargo.toml:16-18` 构建并启动 `bin_main.rs` 对应的二进制。
2. 运行时进入本文件的 `main`，不解析参数也不建立任何资源，直接调用 `astersql_cmd_benchdb::main()`。
3. `lib.rs::main` 调用 `entry::main()`；后者在 `main.rs:41-46` 获取进程参数、解析 flags、打印默认值，并调用 `run_with_flags`。
4. `run_with_flags` 在 `main.rs:53-85` 初始化日志、注册 TiKV store 类型、创建 `BenchDB`，再按 `|` 拆分作业并分派 `create`、`truncate`、`insert`、两类 `update`、`select` 与 `query`。这些都是下游库逻辑，不属于本文件自身的实现。
5. 库入口正常返回时，本文件的 `main` 随之返回，进程采用 Rust `main` 的默认成功退出行为；若下游 panic 或直接退出，本文件不拦截。

## 数据与状态

本文件不声明或保存数据：没有参数对象、全局变量、堆分配、共享状态或返回结果。进程参数由下游 `main.rs::main` 经 `stubs::args_from_env()` 读取，而不是由包装入口传递。

下游状态的所有权也不在本文件。`main.rs::run_with_flags` 创建 `BenchDB`，其中保存 `Storage`、`RecordingSession` 和 `Flags`；这些类型及其可观察副作用由 `main.rs` 与 `stubs.rs` 管理。本包当前 `Cargo.toml` 的 `[dependencies]` 为空，且注释明确说明使用本地 stubs 覆盖 SQL session、store、logging 和 flags 边界，所以当前 Rust 目标不应被描述为已经链接真实 TiKV/TiDB 运行时。

## 依赖与调用关系

- 上游：构建和进程启动边界来自 `cmd/benchdb/Cargo.toml` 的 `[[bin]]`；没有源码级调用者。RustCodeGraph 对 `bin_main.rs` 报告 `used by 0 files`。
- 直接下游：`astersql_cmd_benchdb::main`，定义于 `cmd/benchdb/lib.rs:34-36`。
- 间接下游：`lib.rs::main` 转发到 `cmd/benchdb/main.rs::main`；RustCodeGraph 记录后者调用 `run_with_flags`，并记录 `run_with_flags` 调用 `new_bench_db`、`must_parse_work`、各作业方法、`c_log` 以及 `stubs.rs` 中的日志配置与 store 注册函数。
- crate 边界：二进制目标与库目标属于同一 package；依赖使用 Cargo 自动提供的库 crate 名 `astersql_cmd_benchdb`，不是 `mod` 包含，也不是外部第三方依赖。
- 测试边界：`lib.rs:27-29` 只在 `cfg(test)` 下装入独立文件 `parity_test.rs`。该测试通过库/模块入口验证下游公开合同，不从测试直接调用二进制的私有 `main`。

RustCodeGraph 未解析出 `bin_main.rs::main` 到同包库入口的调用边（其 callees 结果为空），但该直接调用由 `bin_main.rs:9-10` 的源码与 Cargo 的库/二进制双目标声明共同确认；图结果不能覆盖这一 Cargo crate 连接边界。

## 错误处理与边界

包装入口没有 `Result` 返回值、错误类型、日志或恢复分支，也不捕获 panic。下游成功返回时它直接结束；下游的解析失败、初始化失败、SQL 错误或资源关闭错误会按库逻辑传播或终止，包装层不改变退出语义。

错误合同的直接测试位于 `cmd/benchdb/parity_test.rs:388-443`：非法范围、非法整数、未知 flag 和 SQL 执行失败必须进入致命路径。帮助参数的成功退出由 `parity_test.rs:54-75` 通过子进程验证。未知作业则由 `main.rs::run_with_flags` 打印后提前返回，并由 `parity_test.rs:282-301` 验证后续作业不再执行。这些行为属于被委托入口，不能误归因于本文件中的分支，因为本文件没有分支。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、会话或存储连接，也没有显式清理步骤。它对所有下游资源生命周期采用同步调用栈式委托：库入口未返回前，二进制 `main` 保持在调用中；库入口返回后，本文件没有额外收尾。

下游 `new_bench_db` 创建 store 和 session，写入作业在 `main.rs` 中以 `begin`/`commit` 成对执行。结果集正常路径的读空与关闭由 `BenchDB::must_exec` 完成；`parity_test.rs:445-479` 验证正常查询只关闭一次结果集、关闭失败进入致命路径。本文件不得增加另一套清理或并发逻辑，否则会破坏库入口统一拥有资源生命周期的分层。

## 与 Go 版本的对应关系

Go 对照文件 `cmd/benchdb/main.go` 没有独立的薄包装文件：其 `func main()`（`main.go:58-90`）直接完成 flag 解析、日志初始化、TiKV driver 注册、`benchDB` 创建和作业分派。Rust 为了让二进制与独立测试复用同一逻辑，将这一职责拆成三层：本文件的进程入口、`lib.rs` 的公共转发入口，以及 `main.rs` 中与 Go 主流程对齐的实现。

语义映射为：Go `main.go::main` 的真实行为对应 Rust `main.rs::main`/`run_with_flags`，而 `bin_main.rs::main` 只提供 Cargo 所需的进程入口。`Cargo.toml` 的 `package.metadata.porting.go-package = "cmd/benchdb"` 进一步确认 Go 包映射。`parity_test.rs` 覆盖默认 flags、作业顺序、SQL 模板、范围和批次边界、致命错误以及结果集清理；当前没有专门针对这一个两行包装函数的同名测试。

还需注意迁移状态差异：Go 版本直接依赖真实 TiDB/TiKV 包；当前 Rust package 没有 Cargo 依赖，并由 `stubs.rs` 提供记录型 session/store 等边界。文档只能确认入口转发和已测试的模拟合同，不能据此宣称 Rust 二进制已经具备 Go 工具的真实集群连接能力。

## 扩展指南

- 若只是新增或修改 benchmark 作业，应修改 `cmd/benchdb/main.rs::run_with_flags` 及对应 `BenchDB` 方法，并同步扩展独立测试 `cmd/benchdb/parity_test.rs`；不要把作业逻辑放进本文件。
- 若需要改变进程级行为（例如在库入口调用前安装仅属于二进制的运行时钩子），才考虑修改 `bin_main.rs::main`。修改前应确认该行为不需要被库调用者或 parity test 复用；否则应继续放在 `lib.rs`/`main.rs`。
- 若将 stubs 替换为真实依赖，应在 crate 边界和 `main.rs`/`stubs.rs` 接线层完成，并验证 Cargo 依赖与目标平台；本文件原则上仍只保留转发。
- 为包装层新增回归测试时应使用独立测试文件，不把 `#[cfg(test)] mod tests` 内嵌到本 Rust 源文件。可采用进程级集成测试验证二进制退出状态与参数转发，同时保留 `parity_test.rs` 对库逻辑的细粒度合同验证。
- 兼容风险主要是重复解析参数、重复初始化或改变退出传播；性能风险很低，因为现有包装只增加一次同步函数调用。真正的性能与事务风险位于 `main.rs` 的作业实现，不应通过修改包装层规避。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件；`files --filter cmd/benchdb` 找到 `bin_main.rs`、`lib.rs`、`main.rs`、`parity_test.rs`、`stubs.rs` 与 Go 对照文件。
- RustCodeGraph 源码/符号证据：`node --file cmd/benchdb/bin_main.rs` 确认文件共 11 行、仅含 `main`；`node cmd/benchdb/bin_main.rs::main` 确认定义在第 9 行；文件结果显示 `used by 0 files`。
- RustCodeGraph 调用证据：`node cmd/benchdb/lib.rs::main` 确认公共入口调用 `entry::main`；`callees cmd/benchdb/main.rs::main` 确认其调用 `run_with_flags`；对 `run_with_flags` 的 callees 查询确认作业分派及主要下游方法。图未识别二进制到库 crate 的直接边，已用源码和 Cargo 声明交叉核验。
- 已读源码/配置：`cmd/benchdb/bin_main.rs`、`cmd/benchdb/lib.rs`、`cmd/benchdb/main.rs`、`cmd/benchdb/Cargo.toml`。
- 已读 Go 对照：`cmd/benchdb/main.go`，重点核对 `func main`、`newBenchDB`、作业分派、错误处理与结果集关闭。
- 已读独立 Rust 测试：`cmd/benchdb/parity_test.rs`。仓库搜索未发现同名 `bin_main` 测试；该文件是 `lib.rs` 指定的本包合同测试入口。
- 本任务是纯文档分析，未运行 Cargo。交付结构验证要求目标文档存在，并且固定二级标题恰好为 11 个。
