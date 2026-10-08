# `pkg/util/importer/config.rs`

## 文件定位

本文件属于 `astersql-util-importer` crate 的配置与公共错误边界。crate 入口 `pkg/util/importer/lib.rs` 以 `pub mod config` 声明模块，并通过 `pub use config::*` 将这里的 `DbConfig`、`Config` 和 `ImporterError` 暴露到 crate 根。`pkg/util/importer/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认该归属；其 `[package.metadata.porting]` 又把本 crate 对应到 Go 包 `pkg/util/importer`。

它不负责读取 TOML/JSON、建立连接或执行导入，而是保存这些阶段共享的输入数据、生成 MySQL 风格 DSN，并为 Rust 版各子模块定义统一的可传播错误。完整运行入口在 `pkg/util/importer/importer.rs::process`，该入口读取 `Config` 后调用解析、数据库和并发调度模块。

## 核心职责

- `DbConfig` 保存目标数据库的用户、口令、主机、端口和 schema，并由 `DbConfig::dsn` 生成连接器所需字符串。
- `Config` 聚合建表 SQL、可选建索引 SQL、日志级别、数据库配置、worker 数、job 数和事务批大小。它是 `importer.rs::process` 的只读输入。
- `Config::string` 和 `Display for Config` 复刻 Go `(*Config).String` 的可观察文本形式，包括空指针对应的 `"<nil>"` 分支。
- `ImporterError` 把解析、列类型、取值范围、数据库、配置、通道和线程失败收敛为一个错误类型，使 Rust 主流程可以用 `Result`/`?` 传播失败，而不是像部分 Go 路径那样直接 `log.Fatal`。

文件自身不执行配置合法性全检。例如 worker 数和 batch 的约束分别由 `importer.rs::process`、`job.rs::process_jobs` 在真正使用前检查；因此 `Default` 得到的全零/空配置只是可构造状态，不等于可运行配置。

## 主要符号

- `pub struct DbConfig`：五个公开字段分别是 `user: String`、`password: String`、`host: String`、`port: u16`、`schema: String`。派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`，方便配置复制、测试比较和默认构造。
- `pub fn DbConfig::dsn(&self) -> String`：输出 `user:password@tcp(host:port)/schema?charset=utf8`。它只做字符串拼装，不转义用户名、口令、主机或 schema，也不验证端口与字段是否为空。
- `pub struct Config`：公开字段 `table_sql`、`index_sql`、`log_level`、`db_config`、`worker_count`、`job_count`、`batch`。前三项和数据库配置描述导入目标，后三项控制工作量与并发批处理。
- `pub fn Config::string(config: Option<&Self>) -> String`：用 `Option<&Config>` 表达 Go 方法可接收 nil receiver 的语义；`None` 返回 `"<nil>"`，`Some` 按固定字段名和顺序输出。
- `impl Display for Config`：将非空 `&Config` 委托给 `Config::string(Some(self))`，所以 `config.to_string()` 与显式调用保持一致。
- `pub enum ImporterError`：`Parse(String)`、`UnsupportedColumn(String)`、`InvalidRange(String)`、`Database(String)`、`InvalidConfig(String)` 携带上下文；`ChannelClosed`、`WorkerPanic` 是无载荷状态。
- `impl Display for ImporterError`：为各变体提供稳定的人类可读前缀；`impl std::error::Error` 使其可作为标准错误使用。文件没有实现自动 `From` 转换，调用方需显式构造或 `map_err`。

本文件没有 trait、模块级常量、条件编译项或内部私有辅助函数。

## 执行流程

1. 上层构造 `Config`，然后把共享引用传给 `pkg/util/importer/importer.rs::process`。
2. `process` 先用 `table_sql` 和 `index_sql` 调用 `parse_table_sql`、`parse_index_sql`；解析失败以 `ImporterError` 返回。
3. `process` 拒绝 `worker_count == 0`，随后把 `db_config` 和 worker 数交给 `db.rs::create_databases`。
4. `create_databases` 为每个 worker 调用一次 `DbConfig::dsn`，再把结果传给 `DatabaseConnector::open`；中途失败时先关闭已经打开的连接，再返回原错误。
5. 建表与可选建索引完成后，`process` 把 `job_count`、`worker_count`、`batch` 传给 `job.rs::process_jobs`。后者再次检查 worker、连接数量与 batch，并启动生产者和 worker 线程。
6. parser、rand、db、job 任一层产生的 `ImporterError` 通过 `?` 返回到 `process`。连接关闭错误会被收集但不覆盖导入主体结果，这一资源清理策略位于 `importer.rs::process`，不是本文件实现。
7. 配置需要输出时，非空值走 `Display::fmt -> Config::string(Some(self))`；需要模拟 Go nil receiver 时直接调用 `Config::string(None)`。

`log_level` 在目标文件与 `process` 中均未被读取；当前代码只保存并在 `Config` 文本中输出它，不能据此声称 Rust 导入流程已配置日志级别。

## 数据与状态

三个主要类型都是拥有数据的值类型，没有借用字段、全局变量、静态缓存或内部可变性。`DbConfig` 与 `Config` 的字符串字段拥有各自内容；克隆会复制字符串。两个结构都派生 `Default`，默认端口为 `0`、计数为 `0`、字符串为空。

关键数据约束由消费方维护：`worker_count > 0` 是 `process` 的前置条件；`process_jobs` 还要求 `batch > 0` 且数据库连接数不少于 worker 数。`job_count` 可以为零，当前调度代码会产生零个 token 并返回报告。`index_sql` 可以为空，因为 `db.rs::execute_sql` 对空 SQL 直接成功。`DbConfig::dsn` 总是追加 `charset=utf8`。

`Config::string` 会原样输出数据库口令。该行为是为了匹配 Go `%+v` 形状并由 `config_test.rs` 固定，但意味着该字符串不适合未经脱敏写入不可信日志。

## 依赖与调用关系

下游依赖很小：本文件的运行时代码只使用标准库 `std::fmt::{Display, Formatter}`。`pkg/util/importer/Cargo.toml` 的普通 `[dependencies]` 为空；解析器和 `astersql-util-dbutil` 等声明仅位于 `target.'cfg(any())'.dependencies`，`cfg(any())` 恒为 false，不能视为当前构建依赖。

直接上游关系如下：

- `importer.rs::process` 读取 `Config` 全部运行字段，并构造 `ImporterError::InvalidConfig`。
- `db.rs::create_databases` 接收 `&DbConfig`、调用 `dsn`；同文件的数据库 trait 与数据生成函数均以 `ImporterError` 为错误类型。
- `job.rs::process_jobs` 使用 `ImporterError::InvalidConfig` 和 `WorkerPanic`，事务/生成错误也沿同一类型传播。
- `parser.rs` 使用 `Parse` 与 `UnsupportedColumn`；`rand.rs` 和 `db.rs` 使用 `InvalidRange`；数据库抽象和测试桩使用 `Database`。
- `lib.rs` 重导出本文件符号，并通过 `#[cfg(test)] mod config_test` 挂接独立测试文件。

RustCodeGraph 的文件节点报告 `config.rs` 被 11 个文件使用，并能精确定位 `DbConfig` 与 `ImporterError`；但本次对这些类型执行 callers/callees 没有返回字段访问和错误构造边，因此以上直接边由相邻 Rust 源码搜索核验，未把图的缺失解释为“无调用者”。

## 错误处理与边界

`ImporterError::Display` 的文本契约分别是 `parse failed: ...`、`unsupported column type - ...`、`invalid range: ...`、`database error: ...`、`invalid config: ...`、`job channel is closed` 和 `import worker panicked`。载荷只保存字符串，没有 source 链；标准 `Error` 实现也没有覆盖 `source()`。

本文件的两个格式化操作均返回已分配字符串，不会报告普通业务错误。`Display for Config::fmt` 通过 `write_str` 保留 formatter 的格式化错误。`DbConfig::dsn` 不做 URL/DSN 转义，因此特殊字符的解释交给连接器；若要支持这类输入，应先确认目标驱动契约并增加独立测试，而不能只在调用方局部替换字符。

边界校验分散在真实消费点：`process` 检查零 worker，`process_jobs` 检查零 batch 和连接不足，parser/rand/db 将各自领域错误映射到枚举。`ChannelClosed` 目前在本目录生产代码中没有构造点；通道接收关闭在 `job.rs::do_job` 中被当作正常结束，发送失败则终止生产者。文档因此只把该变体描述为公共错误能力，不声称当前主链会返回它。

## 并发与资源生命周期

本文件不创建线程、锁、通道、事务或数据库连接。`Config` 在 `process` 中以共享不可变引用读取；随后数值参数被复制、表与连接在 `job.rs` 中通过 `Arc` 分享。类型本身只含 `String`、整数和另一值结构，未实现自定义析构。

与配置直接相关的资源映射是“一名 worker 对应一个数据库连接”：`worker_count` 决定 `create_databases` 的循环次数和 `process_jobs` 的线程数；`batch` 决定每个 worker 单次事务包含的行数；`job_count` 决定生产者发送的 token 数。连接创建中途失败会关闭已有连接，完整导入结束或失败后 `process` 都调用 `close_databases`。这些生命周期保证由 `db.rs`、`importer.rs` 和 `job.rs` 实现，`config.rs` 只提供参数和错误表达。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/importer/config.go`。Rust `Config` 与 Go `Config` 的七个业务字段一一对应：`TableSQL`/`table_sql`、`IndexSQL`/`index_sql`、`LogLevel`/`log_level`、`DBCfg`/`db_config`、`WorkerCount`/`worker_count`、`JobCount`/`job_count`、`Batch`/`batch`。Rust `DbConfig` 对应 Go 的 `pkg/util/dbutil.DBConfig` 在该工具实际使用的 User、Password、Host、Port、Schema 子集；Go 格式化文本还展示 `Snapshot` 字段，而 Rust 为保持输出形状固定输出空 `Snapshot:`。

Go 字段带 TOML/JSON 标签，Rust 结构当前没有 `serde` 派生，crate 的正常依赖也为空，所以不能直接从同样的 TOML/JSON 反序列化。Go 计数类型是 `int`，Rust 使用 `usize`；Go 数据库端口的通用配置类型与 Rust 的 `u16` 也应在外部输入转换处检查范围。

Go `(*Config).String` 对 nil receiver 返回 `"<nil>"`，否则执行 `fmt.Sprintf("Config(%+v)", *c)`；Rust 以 `Config::string(Option<&Config>)` 显式保留 nil 分支，并用固定模板复刻当前字段名、顺序和嵌套 DB 形状。`config_test.rs::config_string_matches_go_fmt_shape_and_nil_contract` 覆盖这三条路径。

`ImporterError` 是 Rust 迁移层新增的统一错误模型，并非 `config.go` 中的对应声明。Go 主流程常用返回的 `error`，并在 `importer.go`/`job.go` 多处 `log.Fatal`；Rust `importer.rs::process` 返回 `Result<ProcessReport, ImporterError>`，让调用者决定如何处理错误。这是控制流表达差异，不应误写成 Go 枚举的机械翻译。

## 扩展指南

- 新增配置字段时，应同步修改 `Config`、`Config::string`、`config_test.rs` 的完整预期字符串、`importer.rs::process` 的消费接线，以及 Go `config.go`/真实输入边界；若字段影响序列化，还需先为 Rust crate 明确选择并声明序列化方案。
- 修改数据库连接字段或 DSN 规则时，应改 `DbConfig`/`DbConfig::dsn`，并在独立的 `config_test.rs` 增加特殊字符、空值和端口边界测试，同时核对 `db.rs::create_databases` 与实际 `DatabaseConnector` 的契约。不要把测试内嵌进 `config.rs`。
- 新增失败类别时，应为 `ImporterError` 增加变体与 `Display` 分支，并更新产生该错误的 parser/db/rand/job/importer 位置及相应独立测试。若需保留底层错误链，应设计带 source 的错误载荷，而不只是丢失类型信息的字符串。
- 调整 worker/job/batch 语义时，校验必须放在最早能掌握完整不变量的运行入口，并同步 `job_test.rs` 或 `tests.rs`；仅改变默认值不能替代运行时校验。
- 改动 `Config` 文本格式要考虑兼容性和泄露风险：现有测试把 Go 字段顺序和明文 password 固定为契约。若要脱敏，需明确这是有意的兼容性变化并同步 Go/Rust 期望。
- 性能上，本文件主要成本是格式化时的字符串分配；真正敏感参数是 `worker_count` 对连接/线程数量的线性放大，以及 `batch` 对单事务体积的影响，扩展配置时应在消费方评估资源上限。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引时间戳 `1791342965170`；`files --filter pkg/util/importer` 列出目标及同目录 Rust/Go 文件；`node --file pkg/util/importer/config.rs --offset 1 --limit 400` 返回目标 115 行源码和“used by 11 files”；`query DbConfig`、`query ImporterError` 精确定位本文件符号。精确 callers/callees 查询无可用输出，因此调用边另由源码核验。
- 目标与 crate 边界：`pkg/util/importer/config.rs`、`pkg/util/importer/Cargo.toml`、`pkg/util/importer/lib.rs`。
- Rust 直接消费方：`pkg/util/importer/importer.rs`、`pkg/util/importer/db.rs`、`pkg/util/importer/job.rs`；错误变体的其他直接使用由 `pkg/util/importer/parser.rs`、`pkg/util/importer/rand.rs` 核验。
- Go 对照：`pkg/util/importer/config.go`、`pkg/util/importer/importer.go`、`pkg/util/importer/db.go`、`pkg/util/importer/job.go`；同目录不存在 `config_test.go`。
- 独立 Rust 测试：`pkg/util/importer/config_test.rs` 验证字段化输出、`Display` 委托和 nil 契约；`job_test.rs` 与 `tests.rs` 提供配置错误和数据库错误在运行链中的补充证据。
- 人工复核结论：文件存在的原因是统一承载导入输入、DSN 格式与 Rust 错误边界；它通过 `process -> create_databases/process_jobs` 进入运行主链；安全扩展需要同步结构、格式化契约、消费方校验、Go 对照及独立测试。
