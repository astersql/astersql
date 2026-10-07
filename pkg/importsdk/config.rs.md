# `pkg/importsdk/config.rs`

## 文件定位

对应源码：[config.rs](./config.rs)。`config.rs` 是 `astersql-importsdk` crate 的配置边界。crate 入口 `pkg/importsdk/lib.rs` 以私有模块 `mod config` 装配它，再通过 `pub use config::*` 将配置类型和 `WithXxx` 构造函数公开。实际入口 `NewImportSDK`（`pkg/importsdk/sdk.rs`）先调用 `defaultSDKConfig`，按调用方给定顺序执行全部 `SDKOption`，最后把完整 `SDKConfig` 按值交给 `NewFileScanner`。因此本文件不执行扫描、DDL 或导入作业，而是集中定义这些流程所需的默认策略和覆盖方式。

该文件属于 `pkg/importsdk/Cargo.toml` 定义的 `astersql-importsdk` library crate。就本文件直接使用的类型而言，配置边界依赖 `astersql-lightning-log`、`astersql-lightning-mydump` 和 `astersql-parser-mysql`；crate 的扫描实现还通过同一 Cargo manifest 依赖对象存储、Lightning、DDL、parser 和 importer 等组件。

## 核心职责

- `SDKConfig` 汇总文件发现、建库建表、schema 解析、体量估算和日志所需的内部配置。字段为 `pub(crate)`，对外修改入口是函数式选项，而不是任意字段写入。
- `defaultSDKConfig` 建立可直接工作的基线：4 个并发 worker、默认 SQL mode、用户库全选且排除六类系统 schema、自动字符集、Lightning 风格 CSV 默认值、二进制源数据字符集、无限制扫描文件数、遇坏文件即报错，以及默认估算压缩/Parquet 的真实大小。
- `SDKOption = Box<dyn FnOnce(&mut SDKConfig) + Send>` 将一次性配置变更包装为可跨线程边界传递的闭包；`NewImportSDK` 负责依次应用它们。
- `WithConcurrency`、`WithCharset`、`WithDataCharacterSet`、`WithMaxScanFiles` 对无效输入采用“保留现值”语义；其余 `WithXxx` 直接替换对应字段。
- `TableRouteRule`、`Routes`、`CSVConfig` 为 Go 配置形状提供 Rust 表达；`FileRouteRule` 则直接别名到 `mydump::FileRouteRule`，避免复制文件路由的数据结构。

## 主要符号

- `TableRouteRule`：四个公开字符串字段分别表示源 schema/table 匹配模式和目标 schema/table 名称；派生 `Clone`、`Debug`、`Default`。`Routes` 是 `Vec<TableRouteRule>`。
- `FileRouteRule`：`mydump::FileRouteRule` 的公开类型别名，供 `WithFileRouters` 和下游 loader 使用。
- `CSVConfig`：CSV 方言及容错开关的值对象。字段涵盖字段/行分隔、包围与转义字符、NULL 字面量、表头、空行和未转义引号等；派生 `Clone`、`Debug`、`Default`。
- `SDKOption`：一次性、可发送的 `SDKConfig` 原地变更函数。使用 `FnOnce` 允许闭包取得 `String`、`Vec`、logger 等参数的所有权。
- `SDKConfig`：仅派生 `Clone`；保存 `concurrency`、`sql_mode`、文件/表路由、过滤、字符集、`csv_config`、扫描上限、坏文件策略、真实大小估算开关和 logger。
- `defaultSDKConfig() -> SDKConfig`：生成基线配置。CSV 显式默认包括逗号分隔、双引号包围、`\\N` 表示 NULL、首行为表头、表头匹配 schema、反斜杠转义；其余 CSV 字段取 `Default::default()`。
- `WithConcurrency`：仅当 `n > 0` 时覆盖并发数。
- `WithLogger`、`WithSQLMode`、`WithFilter`、`WithFileRouters`、`WithRoutes`、`WithCSVConfig`、`WithEstimateRealSize`、`WithSkipInvalidFiles`：无条件替换各自字段。
- `WithCharset`、`WithDataCharacterSet`：仅接受非空字符串。
- `WithMaxScanFiles`：仅当 `limit > 0` 时写入 `Some(limit)`；零或负数不会清除已有上限。

## 执行流程

1. `NewImportSDK` 调用 `defaultSDKConfig`，获得拥有 logger、字符串和集合的独立配置值。
2. 它遍历 `Vec<SDKOption>`，以传入顺序对同一个 `&mut SDKConfig` 执行闭包；后执行的无条件选项覆盖先前值，有输入保护的选项可能保持先前值。
3. `NewFileScanner` 接收配置所有权。它把 `charset`、`file_route_rules` 和 `filter` 复制到 `mydump::LoaderConfig`；空文件路由会启用默认文件规则和 Aurora 自动映射。
4. `max_scan_files`、`concurrency`、`estimate_real_size` 被转换为 MyDump loader options；loader 创建后，logger 和完整配置被保存在 `fileScanner` 中。
5. 后续建库建表使用 `concurrency.max(1)`；schema 解析和大小采样使用 `sql_mode`；采样读取 `data_character_set` 及部分 `csv_config`；元数据列举和估算在非 Aurora 来源上按 `skip_invalid_files` 决定跳过并记录警告还是立即报错。

重要的当前实现边界是：Go `NewFileScanner` 会把 `cfg.routes` 写入 loader 配置，而 Rust `NewFileScanner` 当前构造的 `mydump::LoaderConfig` 没有使用 `SDKConfig.routes`。因此 `WithRoutes` 在本文件内确实保存值，但现有直接证据不能证明 Rust 扫描路径会应用表路由。

## 数据与状态

`SDKConfig` 是构造阶段的可变值、扫描阶段的只读持有值。函数式选项通过移动参数进入闭包，再将集合、字符串或 logger 直接移入配置，避免额外借用生命周期。配置被传给扫描器后，`fileScanner` 同时保存完整 `SDKConfig` 和一个克隆的 logger；扫描器关闭存储资源不会改变配置内容。

默认过滤列表顺序固定为 `*.*`，随后排除 `mysql`、`sys`、`INFORMATION_SCHEMA`、`PERFORMANCE_SCHEMA`、`METRICS_SCHEMA`、`INSPECTION_SCHEMA`。`max_scan_files: None` 表示没有显式上限；调用 `WithMaxScanFiles(0)` 或负数也不会把已设的 `Some` 恢复为 `None`。同理，空字符集参数不是“清空”，而是“不修改”。

`CSVConfig` 的全部字段都会被保存和克隆，但当前 `file_scanner.rs::buildEstimateSampleConfig` 只直接消费 NULL 定义、字段分隔/包围/转义、行前缀/终止符、表头、反斜杠转义和源数据字符集；`HeaderSchemaMatch`、`TrimLastEmptyField`、`NotNull`、`AllowEmptyLine`、`QuotedNullIsText`、`UnescapedQuote` 未在该采样配置构造函数中直接读取。扩展时不能仅凭字段存在就假定所有开关已经贯穿扫描/采样链路。

## 依赖与调用关系

上游主调用边为 `NewImportSDK -> defaultSDKConfig -> SDKOption::call -> NewFileScanner`，见 `pkg/importsdk/sdk.rs`。独立测试也直接调用 `defaultSDKConfig` 和每个 `WithXxx`，见 `pkg/importsdk/config_test.rs`；`file_scanner_test.rs` 则通过修改配置字段或传入默认配置验证下游扫描行为。

下游关系集中在 `pkg/importsdk/file_scanner.rs`：

- `mydump::LoaderConfig` 消费 `charset`、`file_route_rules`、`filter`；loader options 消费扫描上限、并发、真实大小估算开关。
- `schemaImporter` 消费并发数；表元数据和体量估算流程消费 `skip_invalid_files` 与 logger。
- `buildEstimateSampleConfig` 消费 SQL mode、数据字符集和 CSV 方言；`buildEstimateTableInfo` 也把 SQL mode 设置给 parser。
- `estimate_real_size` 同时影响 loader 是否采样真实大小，以及 `dataFileSize` 对压缩文件选择存储大小还是估算大小。

RustCodeGraph 的文件关系报告 `config.rs` 被 7 个文件使用，并明确列出 `sdk.rs`、`file_scanner.rs`、`config_test.rs`、`file_scanner_test.rs`、`sdk_test.rs` 等；精确 `callers/callees` 查询因 Go/Rust 同名符号未返回可区分边，所以以上具体边由这些已索引文件中的调用点复核。

## 错误处理与边界

本文件中的 option 闭包不返回 `Result`，也不会自行验证过滤表达式、路由模式、SQL mode、CSV 方言或 logger。它只在四处执行轻量输入保护：非正并发、空导入字符集、空数据字符集、非正扫描上限被忽略。真正的 URL、存储、loader、schema、文件和采样错误由 `NewFileScanner` 及后续方法产生和传播。

`skip_invalid_files` 不是全局吞错开关：下游仅在构建表元数据或估算单表大小失败、且来源不是 Aurora 自动映射时记录 warning 并跳过。Aurora 来源仍返回错误，保持自动映射必须完整且有效的约束。`WithMaxScanFiles` 的 Go 注释还区分自动映射需要完整列表、显式文件路由可保留部分扫描行为；该具体判定由 MyDump loader 实现，不在配置闭包中。

`WithRoutes` 的存储测试通过不等于运行时路由已经接线；当前 Rust `NewFileScanner` 未消费该字段，这是兼容性风险而非配置函数错误。类似地，未被当前采样构造函数读取的 CSV 开关也应视为尚未由本文件直接证实的端到端能力。

## 并发与资源生命周期

`SDKOption` 带 `Send`，可作为值跨线程传递，但没有 `Sync` 约束，也不并发修改配置；`NewImportSDK` 在构造期间顺序消费每个 `FnOnce`。`SDKConfig` 本身不包含锁、通道、任务或事务，配置阶段没有共享可变状态。

`concurrency` 同时用于 MyDump 文件扫描和 `SchemaImporter` 的 DDL worker 数。默认值为 4；option 拒绝非正值，而下游仍以 `max(1)` 防御。logger 内部共享由日志 crate 管理，本文件只移动或克隆句柄。对象存储的创建、loader 构造失败时的关闭以及 `FileScanner::Close` 生命周期属于 `file_scanner.rs`；配置只决定行为，不拥有该资源。

## 与 Go 版本的对应关系

主要结构和 option 语义逐项对应 `pkg/importsdk/config.go`：Go 的 `func(*SDKConfig)` 对应 Rust 的装箱 `FnOnce(&mut SDKConfig) + Send`；Go `*int` 扫描上限对应 `Option<i32>`；Go slice 对应 `Vec`；Go logger 对应 Rust logger 句柄。正数/非空保护、默认并发、过滤、字符集和真实大小估算均与 Go 测试意图一致。

Rust 没有直接复用 Go `config.NewConfig()` 返回的 CSV 默认对象，而是在 `defaultSDKConfig` 中显式构造目前所需默认值；`pkg/importsdk/config_test.rs` 比 Go 测试更细地断言 CSV 与数据字符集默认值，以及空值和非正上限不覆盖现值。Rust `FileRouteRule` 是值向量，而 Go 使用指针切片；所有权不同，但配置层都原样保存规则。

已确认的差异是 Go `file_scanner.go` 的 loader 配置包含 `Routes: cfg.routes`，Rust 对应构造未使用 `routes`。此外，Go 的 CSV 类型来自 Lightning config，Rust 使用本文件自有 `CSVConfig`，字段是否完整传递必须逐个以 `file_scanner.rs` 消费点为准。

## 扩展指南

新增配置项时，应同时完成四个层面，而不只添加 `WithXxx`：在 `SDKConfig` 定义存储形态并在 `defaultSDKConfig` 给出明确默认值；实现 option 的覆盖/校验语义；在 `NewFileScanner` 或实际消费者中接线；在独立的 `pkg/importsdk/config_test.rs` 验证默认值与 option 边界，并在 `file_scanner_test.rs` 或 `sdk_test.rs` 验证行为确实生效。Rust 源与测试必须继续分文件维护。

若补齐表路由，应优先检查 `mydump::LoaderConfig` 的 Rust API，令 `SDKConfig.routes` 真正进入 loader，并增加端到端扫描断言，而不是删除 `WithRoutes` 或只加强 setter 测试。若扩展 CSV 语义，应核对 Go `config.CSVConfig`、`buildEstimateSampleConfig` 和 parser 能力，特别注意 NULL、转义、表头以及非 UTF-8 数据对兼容性和采样准确度的影响。

修改并发或扫描上限会影响初始化延迟、内存/IO 压力和完整性判定；修改过滤、文件路由或表路由会改变可见数据集；修改 `estimate_real_size` 或 CSV 参数会影响容量估算而非只影响展示。新增闭包捕获类型还应保持 `SDKOption: Send`，避免破坏调用方在线程间组装选项的能力。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/importsdk` 确认模块文件集合；`node --file pkg/importsdk/config.rs` 读取 20 个符号及使用文件关系；并读取了已索引的 `sdk.rs`、`file_scanner.rs`、`config_test.rs`、`lib.rs`。对同名 `defaultSDKConfig`、`WithConcurrency`、`WithSkipInvalidFiles` 执行了查询；精确 `callers/callees` 无可区分输出，因此未把它当作调用边证据。
- Rust 源：`pkg/importsdk/config.rs`（类型、默认值、全部 option）；`pkg/importsdk/sdk.rs`（默认配置和顺序应用入口）；`pkg/importsdk/file_scanner.rs`（loader 接线、并发、跳过策略、采样与 parser 消费点）；`pkg/importsdk/lib.rs`（模块可见性与再导出）。
- crate 边界：`pkg/importsdk/Cargo.toml` 的 package、library path、porting metadata、直接依赖与 dev-dependencies。
- Go 对照：`pkg/importsdk/config.go`、`pkg/importsdk/file_scanner.go`；前者核对默认值和 options，后者核对配置实际消费以及 Rust 尚缺的 `Routes` 接线。
- 独立测试：`pkg/importsdk/config_test.rs` 与 `pkg/importsdk/config_test.go`；另以 `pkg/importsdk/file_scanner_test.rs` 的配置构造调用点确认配置会进入扫描器测试面。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另执行任务规定的 11 标题结构命令，并人工检查本文能够回答文件存在原因、构造与消费流程、错误/生命周期边界和安全扩展位置。
