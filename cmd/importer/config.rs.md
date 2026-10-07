# `cmd/importer/config.rs`

## 文件定位

该文件属于 `astersql-cmd-importer` crate 的配置层。crate 根在 `cmd/importer/lib.rs` 中以 `pub mod config` 暴露本模块，二进制入口由 `cmd/importer/Cargo.toml` 的 `[[bin]]` 指向 `bin_main.rs`，实际业务入口 `cmd/importer/main.rs::run_with_args` 依次调用 `NewConfig` 和 `Config::Parse`。因此它位于“进程参数进入 importer”之后、“DDL 解析、数据库连接、统计加载和导入任务调度”之前，负责把默认值、可选 TOML 与命令行参数合并成其余模块消费的 `Config`。

Cargo 元数据把本 crate 标记为 Go 包 `cmd/importer` 的二进制移植（`package.metadata.porting`），本文件直接使用的外部解析依赖是 `toml = "0.8"`；错误抽象则来自 crate 内的 `crate::stubs::{Error, Result}`。它不是通用 TiDB 配置中心，也不负责建立连接、解析 SQL 或启动 worker。

## 核心职责

1. `NewConfig`/各 `Default` 实现建立与 Go `cmd/importer/config.go::NewConfig` 相同的基线值：数据库 `127.0.0.1:3306`、用户 `root`、库 `test`，系统参数为 `info/2/10000/1000`。
2. `Config::Parse` 实现“默认值 < TOML 文件 < 显式 CLI”的三层优先级。它先解析一次参数以找到 `-config`，读取文件，再解析同一参数列表以恢复 CLI 的最高优先级。
3. `parse_flags`、`split_flag`、`take_val`、`take_int_val` 和 `parse_go_int` 模拟 Go `flag.FlagSet` 与 `strconv.ParseInt(value, 0, strconv.IntSize)` 的关键输入语义，包括单双横线、`key=value`、帮助、尾随参数、进制前缀、下划线和本机 `isize` 范围。
4. `configFromFile` 只把 `[db]`、`[ddl]`、`[stats]`、`[sys]` 的已知键覆盖进现有配置，并对节与值的 TOML 类型做显式校验。
5. `DBConfig::String`、`Config::String` 以及 `DBConfig` 的 `Display` 实现保留 Go `fmt.Stringer` 的输出形状，供诊断和对齐测试使用。

## 主要符号

- `pub fn NewConfig() -> Config`：公开构造入口，返回 `Config::default_with_flags()`；调用方无需单独注册 flag。
- `pub struct DBConfig`：数据库连接五元组 `Host/User/Password/Name/Port`。`Default` 固化 Go 版默认值，`String(Option<&DBConfig>)` 额外表达 Go 的 nil 接收者为 `"<nil>"`。
- `pub struct DDLConfig`：保存 `TableSQL` 与 `IndexSQL`，分别对应 `-t`、`-i` 和 TOML `ddl.table-sql`、`ddl.index-sql`。
- `pub struct StatsConfig`：保存可选统计文件 `Path`，对应 `-s` 和 `stats.stats-file-path`。
- `pub struct SysConfig`：保存 `LogLevel/WorkerCount/JobCount/Batch`；后三项直接控制 importer 后续并发和批次规模。
- `pub struct Config`：聚合四类公开子配置，并保存内部解析状态 `configFile`、尾随 `args` 与私有 `help_requested`。公开 API 是 `Parse` 和 `String`；`default_with_flags`、`configFromFile`、`parse_flags` 均为模块内部实现。
- `Config::Parse(&mut self, arguments: &[String]) -> Result<()>`：配置合并的核心入口。
- `split_flag`/`take_val`/`take_int_val`：分别负责 flag 词法拆分、参数取值与带 Go 错误形状的整数转换。
- `IntParseError::{Syntax, Range}` 与 `parse_go_int`：区分格式错误和本机整数越界，支持十进制、传统前导零八进制以及 `0x`、`0b`、`0o` 前缀。
- `toml_section`/`apply_toml_string`/`apply_toml_int`/`toml_type_error`：TOML 的已知节读取、类型检查和字段覆盖辅助函数。

本文件没有 trait、异步函数或条件编译项；公开结构字段沿用 Go 风格命名，crate 根通过 `#![allow(non_snake_case)]` 接受这种对齐布局。

## 执行流程

入口流程为 `cmd/importer/main.rs::run_with_args → NewConfig → Config::Parse`：

1. `NewConfig` 创建全部默认值，并清空 `configFile`、`args` 和帮助状态。
2. `Parse` 第一次调用 `parse_flags(arguments)`。解析器从左到右处理参数；遇到 `--` 或首个非 flag 后停止，并把余项写入 `args`；遇到帮助立即返回 `Error::help()`；未知 flag、坏语法、缺值或整数错误立即失败。
3. 如果第一轮得到非空 `configFile`，`configFromFile` 读取 UTF-8 文本并解析为 `toml::Table`。存在的已知节逐字段覆盖当前值；缺失节和缺失键保持已有值。
4. `Parse` 用相同参数再次调用 `parse_flags`。第二轮先清空 `args` 和帮助状态，但不重置配置字段，因此显式 CLI 值覆盖文件值。
5. 第二轮完成后，只要 `args` 非空，就用第一项返回 `'<arg>' is an invalid flag`；否则解析成功。
6. 成功后的 `Config` 被 `run_with_args` 分流：`DDLCfg` 进入 `parseTableSQL/parseIndexSQL`，`DBCfg` 和 `WorkerCount` 进入 `createDBs`，`StatsCfg.Path` 决定是否 `loadStats`，`JobCount/WorkerCount/Batch` 进入 `doProcess`，`LogLevel` 当前在该 Rust 主流程中没有下游消费点。

## 数据与状态

`Config` 是可克隆的拥有型快照：字符串和 `Vec<String>` 均由实例持有，没有借用外部参数。默认构造把 Go `FlagSet` 注册时的默认值直接折叠到字段中；Rust 版没有保存真实 `FlagSet`，`Config::String` 以配置对象地址模拟 Go 输出中的 `FlagSet` 指针形状。

`Parse` 会原地修改接收者，并非事务式更新。第一轮 flag、TOML 覆盖或第二轮 flag 在较晚位置失败时，之前成功写入的字段可能保留；调用方当前在错误后直接结束入口流程，未依赖回滚。两轮之间只有配置字段与 `configFile` 延续，`parse_flags` 每轮都会清空 `args` 和 `help_requested`。

数值使用 `isize` 对齐 Go 的 `int` 本机位宽。`parse_go_int` 先规范符号、基数和下划线，再经 `u128 → i128 → isize` 检查范围；因此同一超大值在 64 位平台可能成功、在 32 位平台可能越界，这正是 `cmd/importer/config_test.rs::integer_flags_accept_go_base_zero_and_native_width` 验证的契约。

## 依赖与调用关系

上游关系：

- `cmd/importer/lib.rs` 声明 `pub mod config`，并把独立测试 `config_test.rs` 接入 crate。
- `cmd/importer/main.rs::run_with_args` 是生产主调用方：先 `NewConfig()`，再 `cfg.Parse(args)`，帮助错误映射为退出码 0，其他解析错误映射为退出码 2。
- `cmd/importer/parity_test.rs` 直接调用 `NewConfig`/`Parse` 验证公共契约、边界和错误路径。
- RustCodeGraph 对 `NewConfig` 的结果列出 `cmd/importer/parity_test.rs` 中五个契约函数调用方；对本文件内部还确认了 `Parse → configFromFile/parse_flags`、`parse_flags → split_flag/take_val/take_int_val`、`take_int_val → parse_go_int` 的调用链。精确 `callers/callees` 命令未为方法节点输出额外边，故其余关系以入口和测试源码为直接证据。

下游关系：

- 标准库 `std::fs::read_to_string` 负责读取配置文件。
- `toml::Table`/`toml::Value` 负责完整 TOML 语法解析和类型判定。
- `crate::stubs::Error` 统一承载普通错误、帮助标记和显示文本。
- 配置结果的业务消费者位于 `main.rs`、`db.rs`、`parser.rs`、`stats.rs`、`job.rs`；本文件只生产配置，不直接调用这些模块。

## 错误处理与边界

- 文件不存在、不可读或 TOML 语法错误：`configFromFile` 把底层错误文本包装为 `Error` 并向上返回。
- 已知节不是 table，或已知字符串/整数键类型错误：`toml_type_error` 返回包含完整字段路径、期望类型与实际 TOML 类型的错误；`apply_toml_int` 还拒绝超出 `isize` 的整数。
- 未知 TOML 节和未知键当前被忽略，因为代码只主动读取已知字段；扩展时不能误称为“严格拒绝所有未知配置”。
- `---h`、空 flag 名、以 `=` 开头的 flag 为坏语法；未知合法形状 flag 返回 `flag provided but not defined`；无值返回 `flag needs an argument`。
- `-help`、`--help` 以及拆分后键为 `help`（包括 `-help=anything`）返回带 `is_help` 标志的专用错误。
- `--` 或首个裸参数终止 flag 扫描，余项最终按无效 flag 报错；单独的 `-` 同样成为尾随参数。
- 整数下划线只能位于合法数字之间，带基数前缀时允许紧随前缀的首个下划线；非法八进制如 `08` 属于语法错误。
- 配置层不验证 worker/job/batch/port 的业务取值范围（例如零或负数），只验证能否解析进 `isize`；这些值的运行时有效性由后续消费者承担。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或数据库连接，所有解析均在调用线程同步完成。`Config` 没有内部共享状态；若跨线程使用，调用方应在完成可变解析后传递拥有值或自行使用同步原语。

唯一外部资源是配置文件：`fs::read_to_string` 在函数调用内打开、完整读取并关闭文件，不保留句柄。TOML 文本和解析树都是局部值，在 `configFromFile` 返回时释放；配置中只留下复制/转换后的拥有字段。`Config::String` 读取对象地址仅用于格式化，不延长生命周期也不解引用裸指针。

## 与 Go 版本的对应关系

Rust 的 `DBConfig`、`DDLConfig`、`StatsConfig`、`SysConfig` 和 `Config` 逐项对应 `cmd/importer/config.go` 的同名结构；CLI 短参数、TOML 节/键和默认值也逐项一致。`Config::Parse` 保留 Go 版两次 `FlagSet.Parse`、中间 `toml.DecodeFile`、最后拒绝 `FlagSet.Args()` 的顺序。

实现差异主要有三类：

- Go 在 `Config` 中嵌入 `*flag.FlagSet`；Rust 用手写 `parse_flags` 和内部状态代替，因此新增 flag 必须同步维护匹配分支、默认值、TOML 映射和测试。
- Go 的 BurntSushi TOML 直接按 struct tag 解码；Rust 用 `toml::Table` 后显式提取已知字段。当前已验证的已知字段类型和覆盖行为对齐，但未知字段不会由本文件报错。
- Go 的 nil 方法接收者可自然输出 `"<nil>"`；Rust 通过 `String(Option<&...>)` 显式表达。`Config::String` 的 `FlagSet` 指针只是输出形状兼容，不代表存在真实 flag 集合。

`cmd/importer/config_test.rs` 专门覆盖 Go 风格整数/TOML/flag/字符串契约；`cmd/importer/parity_test.rs` 进一步覆盖默认值、非法 TOML 类型、帮助、未知 flag、尾随参数及进程退出码。仓库中没有同路径 `config_test.go`，因此 Go 语义的直接权威来源是 `config.go`、`main.go` 与 Rust 对齐测试中的明确断言。

## 扩展指南

新增配置项时，应按来源完整接线，而不能只给结构体加字段：

1. 在所属子结构中增加字段，并决定默认值；若对应 Go 迁移，先核对 `cmd/importer/config.go` 的字段、struct tag 与 `NewConfig` 注册值。
2. 如支持 CLI，在 `parse_flags` 增加唯一键并选择 `take_val` 或 `take_int_val`；如 Go 支持长短名或布尔 flag，需要先补齐当前手写解析器缺少的语义。
3. 如支持 TOML，在 `configFromFile` 对应节中增加类型明确的覆盖逻辑，并使用完整路径生成错误。
4. 检查 `cmd/importer/main.rs::run_with_args` 及具体消费者模块是否需要使用新字段；配置存在但无人消费不等于功能已支持。
5. 在独立的 `cmd/importer/config_test.rs` 添加默认值、CLI、TOML、优先级、非法类型和边界回归；跨模块/退出码契约放在 `cmd/importer/parity_test.rs`。不要把 Rust 测试内嵌回 `config.rs`。

兼容风险集中在 flag 错误文本、双解析优先级、Go 基数整数与本机位宽；性能风险通常较低，因为配置文件只读一次，但不要在解析器中引入与参数数量无关的昂贵扫描。涉及密码的诊断扩展需特别注意：当前 `DBConfig::String` 和 `Config::String` 会输出 `Password`，新增日志调用前应评估泄露风险。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、7032 个 Rust 文件；`files --filter cmd/importer` 确认目标、入口、Go 对照与独立测试均已索引，`config.rs` 记录 26 个符号。
- RustCodeGraph 源码节点：`cmd/importer/config.rs` 全部 530 行；核心符号 `NewConfig`（31）、`Config::Parse`（174）、`configFromFile`（229）、`parse_flags`（268）、`parse_go_int`（418）及 TOML 辅助函数（491-530）。
- RustCodeGraph 查询：`query NewConfig` 同时定位 Go `config.go:26` 与 Rust `config.rs:31`；`query Parse` 定位 Rust `config.rs:174`；`query configFromFile` 和 `query parse_flags` 确认内部节点。`explore` 给出 `NewConfig` 的 parity 测试调用方和本文件内部调用链；精确方法图命令未返回额外边，未据此臆造覆盖关系。
- 读取的 crate/入口证据：`cmd/importer/Cargo.toml`、`cmd/importer/lib.rs`、`cmd/importer/main.rs`、`cmd/importer/main.go`。
- 读取的 Go 对照：`cmd/importer/config.go`；其字段、默认值、两次解析和 TOML 解码顺序与本文逐项核对。
- 读取的独立测试：`cmd/importer/config_test.rs`、`cmd/importer/parity_test.rs`；覆盖本机位宽整数、TOML 语法与类型、CLI 优先级、帮助、错误文本、默认值和入口退出码。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证恰有 11 个固定二级标题，并人工复查文档没有把未知键、业务范围验证或图中缺失边描述成已支持事实。
