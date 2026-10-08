# [`pkg/util/printer.rs`](printer.rs)

## 文件定位

`pkg/util/printer.rs` 是根工具 crate `astersql-util` 中的轻量构建信息模块。`pkg/util/lib.rs` 通过 `pub mod printer;` 将它暴露为 `astersql_util::printer`；对应 crate 的清单是 `pkg/util/Cargo.toml`，其中 `[lib] path = "lib.rs"`，并直接声明了本文件使用的 `log` 与 `rustc_version_runtime` 依赖。

本文件只管理四项通用构建元信息并提供字符串/日志两种输出，不负责 TiDB 服务端的完整版本、内核类型、部署模式或配置打印。仓库另有独立子 crate `pkg/util/printer/`，其 `printer.rs` 提供 `PrintTiDBInfo`、`GetTiDBInfo` 和表格渲染；两者名称相近但不是同一模块，扩展时不能混用。

## 核心职责

- 用 `Version`、`BuildTS`、`GitHash`、`GitBranch` 保存进程级构建元信息，初始值均为 `"None"`。
- 通过 `SetBuildInfo` 一次接收四个借用字符串并复制进全局状态，为不能像 Go 那样直接覆写包级字符串的 Rust 调用方提供显式写入口。
- 通过 `GetRawInfo` 生成适合命令输出或诊断文本的六行字符串，最后一行也带换行符。
- 通过 `PrintInfo` 生成一条 info 级欢迎日志；它输出同一组构建字段，但不复用 `GetRawInfo` 的多行文本。

这些职责可由 `pkg/util/printer.rs` 的四个静态量和三个公开函数直接复核。仓库内 Rust 生产源码当前没有发现这三个函数的调用点，因此该模块目前是已实现、已测试但尚未接入生产启动链的工具 API，不能把 Go 侧历史用途推断成 Rust 侧已经接线。

## 主要符号

- `pub static Version: LazyLock<RwLock<String>>`：发布版本，首次访问时构造值为 `"None"` 的读写锁。
- `pub static BuildTS: LazyLock<RwLock<String>>`：UTC 构建时间戳，存储和格式不在本模块内校验。
- `pub static GitHash: LazyLock<RwLock<String>>`：Git commit hash，不限制长度或字符集。
- `pub static GitBranch: LazyLock<RwLock<String>>`：Git 分支名，不做转义或规范化。
- `pub fn SetBuildInfo(version: &str, build_ts: &str, git_hash: &str, git_branch: &str)`：按版本、时间、hash、分支的固定顺序依次取得四把写锁，并把参数复制为拥有所有权的 `String`。
- `pub fn GetRawInfo(app: &str) -> String`：读取四项状态，调用 `rustc_version_runtime::version()`，返回包含应用名和编译器版本的多行文本。
- `pub fn PrintInfo(app: &str)`：读取四项状态和运行时编译器版本，通过 `log::info!` 发出单条欢迎日志。

文件级 `#![allow(non_snake_case, non_upper_case_globals)]` 是为了保留 Go 移植 API 的导出命名；本文件没有类型、trait、`impl`、条件编译分支或私有辅助函数。

## 执行流程

1. 模块加载后，各 `LazyLock` 尚未必分配内部 `String`；首次读写相应字段时才以 `"None"` 初始化。
2. 调用方可先调用 `SetBuildInfo`。函数依次写入 `Version`、`BuildTS`、`GitHash`、`GitBranch`，每一项都从输入 `&str` 创建独立 `String`。
3. 调用 `GetRawInfo(app)` 时，函数用一次 `format!` 依次读取版本、hash、分支、时间，并即时取得 `rustc_version_runtime::version()`；结果顺序固定为 App Name、Release Version、Git Commit Hash、Git Branch、UTC Build Time、Rust Version。
4. 调用 `PrintInfo(app)` 时，函数用 `log::info!` 构造一条无嵌入换行的消息，字段以 `key=value` 形式排列；实际是否输出及输出目的地由进程安装的 `log` facade 实现和级别过滤决定。

`pkg/util/cpu_posix_1_aster_unit_test.rs::printer_and_rlimit_are_operational` 证明先写后读能在原始文本中观察到应用名、版本及 Rust 版本字段；`pkg/util/printer_test.rs::print_info_logs_welcome_and_build_fields_without_raw_info_duplication` 证明日志路径只产生一条欢迎消息，且不夹带 `GetRawInfo` 的多行标签。

## 数据与状态

四项元信息都是进程级共享可变状态，生命周期持续到进程退出。`LazyLock` 负责线程安全的一次初始化，`RwLock<String>` 允许多个并发读者或一个写者。`SetBuildInfo` 复制输入，因此全局值不借用调用方内存；读路径只把锁保护的字符串用于格式化，不向外暴露 guard 或可变引用。

`SetBuildInfo` 并非四字段的原子事务：它依次释放/获取各字段的写锁；`GetRawInfo` 和 `PrintInfo` 也分别读取每个锁。若运行中有并发更新，输出可能由新旧两组字段混合组成。当前代码适合启动阶段先设置、之后只读的使用方式，但没有用类型或同步屏障强制这个阶段约束。

## 依赖与调用关系

- 上游装配：`pkg/util/lib.rs` 声明 `pub mod printer`，因此依赖 `astersql-util` 的 crate 可以访问三个函数和四个静态量；`pkg/util/Cargo.toml` 将该 crate 定义为 `astersql-util`。
- 已确认调用者：RustCodeGraph 为 `SetBuildInfo`、`GetRawInfo`、`PrintInfo` 建立了 `pkg/util/printer.rs` 中的函数节点。由于图的 callers/callees 查询在本次环境中未返回结果，进一步用 Rust 全仓文本检索确认直接调用只出现于 `pkg/util/cpu_posix_1_aster_unit_test.rs` 和 `pkg/util/printer_test.rs`，未发现生产 Rust 调用点。
- 下游标准库：`std::sync::LazyLock` 提供延迟初始化，`RwLock` 提供共享状态同步，`String`/`format!` 完成拥有型存储与文本组装。
- 下游外部依赖：`rustc_version_runtime::version()` 提供构建本二进制所用 Rust 编译器版本；`log::info!` 将消息提交给日志 facade。两项依赖均由 `pkg/util/Cargo.toml` 直接声明。
- Go 对照：`pkg/util/printer.go` 是相同四字段和两个输出函数的直接语义来源。`pkg/util/printer/printer.rs` 与 `pkg/util/printer/printer.go` 属于另一个 printer 子 crate/Go 子包，不是本文件的调用下游。

## 错误处理与边界

公开函数没有 `Result` 返回值。四处读锁和四处写锁都用 `expect(...)` 处理中毒；若曾有持锁线程 panic 导致锁中毒，后续相关读写会再次 panic，错误消息会指出 version、build timestamp、git hash 或 git branch 锁。`SetBuildInfo` 中途 panic 时，已经写完的较早字段不会回滚。

输入字符串没有长度、编码内容或格式校验；Rust `&str` 保证 UTF-8，但其中可包含换行或日志分隔字符，所以调用者负责确保构建元信息适合面向用户输出。空字符串会被原样保存和打印。未调用 `SetBuildInfo` 时，两种输出都合法地显示 `"None"`。`PrintInfo` 不报告日志被过滤或未安装后端，行为遵循 `log` facade；`GetRawInfo` 则总是返回已分配的 `String`。

## 并发与资源生命周期

模块不创建线程、异步任务、通道、文件、网络连接或事务。全部资源是四个静态 `LazyLock<RwLock<String>>`，首次使用后一直存活；每次更新会分配新的 `String` 并在替换时释放旧字符串，每次原始信息输出会分配结果字符串。

锁的粒度是“每字段一把锁”，降低了无关字段之间的锁竞争，但牺牲了整组快照一致性。所有函数都按固定字段顺序访问锁，当前实现没有嵌套持有多把 guard 的显式代码，因而没有形成锁顺序环；不过未来若要提供一致快照，宜把四字段合入单个结构并由一把锁保护，而不是在调用端同时取得多把锁。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/printer.go`。Rust 保留了 `Version`、`BuildTS`、`GitHash`、`GitBranch` 以及 `GetRawInfo`、`PrintInfo` 的名称和字段顺序；两边默认值都是 `"None"`。主要差异如下：

- Go 的包级 `string` 可直接由包内或链接参数赋值；Rust 使用 `LazyLock<RwLock<String>>`，并新增 `SetBuildInfo` 作为安全写入口。
- Go 原始文本末行是 `Go Version`，来源为 `runtime.Version()`；Rust 对应为 `Rust Version`，来源为 `rustc_version_runtime::version()`。
- Go `PrintInfo` 使用 PingCAP 日志和结构化 `zap.String` 字段；Rust使用 `log::info!` 生成单条插值消息。字段语义相同，但 Rust 日志不是结构化字段，查询和编码行为取决于后端。
- Go 没有锁中毒概念；Rust 在锁中毒时 panic。Rust 逐字段锁也使并发更新期间可能观察到混合快照，而 Go 代码本身没有同步保护并会产生数据竞争；两者都隐含“启动时写入、运行时读取”的调用约束。
- Go 文件没有 `SetBuildInfo`；这是 Rust 为受控修改全局值增加的迁移接口。仓库现有 Rust 测试覆盖该接口，Go 同路径没有独立 `printer_test.go`。

## 扩展指南

- 新增构建字段时，应同步更新静态状态（或统一的状态结构）、`SetBuildInfo`、`GetRawInfo`、`PrintInfo` 以及直接 Go 对照语义；同时扩展独立测试 `pkg/util/printer_test.rs`，不要把测试嵌入生产文件。
- 若字段必须作为一致版本快照更新，优先将四项及新增项封装为单个结构并用一把 `RwLock` 保护，同时保留现有公开函数或提供兼容迁移；需要重点评估公开静态量的兼容性及启动路径接线。
- 若要把 `PrintInfo` 改成结构化日志，应先确认仓库统一日志后端的 API，并验证字段名、日志级别和单条消息约束；当前测试明确要求没有原始多行文本重复。
- 若要接入生产启动流程，应在真实入口显式调用 `SetBuildInfo`/`PrintInfo`，并增加对应入口的独立测试。当前代码搜索没有生产调用证据，因此不能只修改本模块就宣称启动日志已经生效。
- 不要将这里的轻量 API 扩展成 `pkg/util/printer/` 子 crate 的完整 TiDB 信息打印器；完整 TiDB 版本、配置和表格输出应在后者维护，避免两个同名模块继续漂移。
- 性能风险主要来自高频调用时的锁和字符串格式化；这些 API 预期用于低频启动/诊断场景。若改变为热路径调用，应增加并发与分配基准，而不仅是功能断言。

## 验证依据

- 源码：`pkg/util/printer.rs`，核对四个公开静态量、`SetBuildInfo`、`GetRawInfo`、`PrintInfo`、锁策略、格式字符串和 panic 路径。
- crate 边界：`pkg/util/lib.rs` 的 `pub mod printer` 与 `pkg/util/Cargo.toml` 的 `astersql-util` 包定义、`log`/`rustc_version_runtime` 依赖。
- Rust 测试：`pkg/util/printer_test.rs` 验证 info 日志的单消息、无换行和字段集合；`pkg/util/cpu_posix_1_aster_unit_test.rs::printer_and_rlimit_are_operational` 验证 `SetBuildInfo` 后 `GetRawInfo` 可观察应用名、版本和 Rust 版本。
- Go 对照：`pkg/util/printer.go`，核对默认字段、原始文本顺序和欢迎日志字段；同目录未发现根 Go 包对应的 `pkg/util/printer_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/util/printer` 定位了根文件与同名子目录；`query` 精确定位本文件三个函数节点。`callers/callees` 本次没有返回可用结果，故调用点结论由 `rg` 对全部 Rust 源文件的直接调用检索补充，并明确限制为当前仓库静态可见调用。
- 结构验收使用任务规定的命令，要求目标文档存在且恰好包含本页 11 个固定二级标题；本任务是纯文档分析，按计划不运行 Cargo。
