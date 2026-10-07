# `pkg/lightning/mydump/router.rs`

## 文件定位

本文件属于 `astersql-lightning-mydump` crate 的文件发现与分类层。crate 根在 `pkg/lightning/mydump/Cargo.toml`，由 `lib.rs` 以 `mod router; pub use router::*;` 纳入并重导出；该 crate 的直接依赖中，路由实现使用 `regex` 编译/展开规则、使用 `percent-encoding` 解码库表名，并以 crate 根定义的 `MydumpError::Routing` 统一返回路由错误。

它位于“存储枚举出路径”与“loader 组装库表元数据”之间：`loader.rs::NewLoaderWithStore` 从 `LoaderConfig.file_routes` 调用 `NewFileRouter`，随后对每个路径调用 `FileRouter::Route`；得到的 `RouteResult` 决定文件是库级 schema、表/视图 schema、SQL/CSV/Parquet 数据还是应忽略，并把 schema、表名、分片 key 与整体压缩格式交给后续元数据构造。该文件只解释路径，不读取文件内容，也不负责表路由、过滤或解压。

## 核心职责

1. 定义路由输入、输出和枚举契约：`FileRouteRule`、`RouteResult`、`SourceType`、`Compression`、`CompressType` 与 `FileRouter`。
2. 用 `default_file_route_rules` 表达 mydumper 默认命名：先屏蔽 trigger/post 与备份文件，再识别库 schema、表 schema、视图 schema、带内部 codec 标记的 Parquet 和普通 SQL/CSV/Parquet 数据文件。
3. 由 `new_file_router`/`RegexRouterParser::parse` 在扫描前把所有配置规则编译为 `RegexRouter`，并提前校验正则、必需字段和模板捕获引用。
4. 由 `ChainRouters::route` 保持规则顺序，采用“首个命中即返回”的优先级语义；由 `RegexRouter::route` 展开捕获模板并生成 `RouteResult`。
5. 解析和约束类型/压缩：`parse_source_type`、`parse_compression_type`、`parse_compression_on_file_extension` 以及 `to_storage_compress_type`；特别拒绝“整个 Parquet 文件再次压缩”的路由结果。
6. 可选解码 schema/table 的 URL path 转义；解码失败时通过 `Logger` 记录告警并保留原捕获值，而不是令路由失败。

## 主要符号

- `SourceType`：稳定的文件用途枚举，判别值依次为 `Ignore`、`SchemaSchema`、`TableSchema`、`Sql`、`Csv`、`Parquet`、`ViewSchema`。`as_str`/`Display` 将其映射到 `schema-schema`、`table-schema`、`sql` 等配置字符串；未知值没有入口，未知字符串由 `parse_source_type` 报错。
- `Compression`：文件整体压缩后缀枚举，支持 `None/Gz/Lz4/Zstd/Xz/Lzo/Snappy`。它和 Parquet 文件内部 writer codec 是两层概念；默认规则对 `0001.snappy.parquet` 只记录分片 key，整体压缩仍为 `None`。
- `CompressType` 与 `to_storage_compress_type`：面向当前 Rust 存储读取层的较窄映射，只接受 gzip、snappy、zstd 和未压缩；LZ4、XZ、LZO 虽可被路由识别，但在转换到该存储类型时返回 `Routing` 错误。
- `FileRouteRule`：一条配置规则。`path` 是常量路径，`pattern` 是正则，两者互斥；`schema/table/type_name/key/compression` 是捕获展开模板；`unescape` 控制 schema/table 是否做 percent decode。
- `FileRouter`：统一接口 `route(&self, path) -> Result<Option<RouteResult>, Error>`；`Ok(None)` 表示不匹配，`Err` 表示路径已经匹配但展开值非法。`Route` 是 Go 风格别名。
- `ChainRouters`：保存有序 `Vec<RegexRouter>`，逐条尝试并返回第一个 `Some` 或第一个错误。
- `RegexRouter`：一条已编译规则，持有 `Regex` 和按字段顺序排列的 `PatternExpander`。
- `RegexRouterParser`：把 `FileRouteRule` 编译为 `RegexRouter`。`parse_field_extractor` 验证非空模板并调用 `check_sub_patterns`；后者接受 `$$`、`$1`、`${1}`、`$name` 和 `${name}`，拒绝越界数字捕获和不存在的命名捕获。
- `Setter`/`PatternExpander`：替代 Go 实现中的字段赋值闭包。模板展开后，`Setter` 分别负责类型解析、schema/table 解码、key 赋值和压缩校验。
- `RouteResult`：纯值结果，包含 `schema`、`name`、`key`、`compression`、`source_type`；默认值是空字符串、`Compression::None`、`SourceType::Ignore`。
- `Logger`：`Arc<Mutex<Vec<String>>>` 包装的轻量消息收集器；克隆后共享同一消息列表，供解码失败告警和外部错误记录使用。
- PascalCase/辅助导出：`NewFileRouter`、`NewDefaultFileRouter`、`ParseCompressionOnFileExtension`、`ToStorageCompressType` 等保留 Go 对齐命名；`parseSourceType`、`parseCompressionType`、`parseFieldExtractor`、`checkSubPatterns` 主要为兼容与独立测试暴露内部能力。

## 执行流程

规则构造阶段如下：

1. `LoaderConfig::default` 取得 `default_file_route_rules`，或调用方提供自定义 `Vec<FileRouteRule>`；`loader.rs::NewLoaderWithStore` 调用 `NewFileRouter`。
2. `new_file_router` 按配置顺序遍历规则。任一规则编译失败会立即返回错误，因此不会暴露部分可用的 `ChainRouters`。
3. `RegexRouterParser::parse` 先要求 `path` 与 `pattern` 恰有一个非空。常量 `path` 经 `regex_escape` 转成字面正则，并由 `quote_template` 把模板中的 `$` 变为 `$$`，防止常量 schema/table/key 被当作捕获引用。
4. 编译 `Regex` 后，解析器总是先加入 type extractor。若规则字面类型是 `ignore`，立即完成，schema/table/key/compression 都不再要求；否则依次加入 schema、table（库级 schema 文件除外）、可选 key、可选 compression。这个顺序保证压缩校验时 `source_type` 已写入。
5. `check_sub_patterns` 在运行前检查所有显式模板变量；数字捕获不得超过 `captures_len - 1`，命名捕获必须存在。由此把大部分配置错误前移到 loader 构造期。

单路径路由阶段如下：

1. `ChainRouters::route` 按原规则顺序调用各 `RegexRouter::route`。不匹配继续；第一个 `Some` 或错误立即结束；全部不匹配返回 `Ok(None)`。
2. `RegexRouter::route` 用 `Regex::captures` 匹配完整传入路径；命中后创建默认 `RouteResult`，并按 extractor 顺序调用 `PatternExpander::expand`。
3. `Captures::expand` 生成字段字符串。type/compression 分别经严格解析；schema/table 可由 `set_routed_value` 解码；key 原样保存。
4. 若结果类型是 `Parquet` 且整体压缩不是 `None`，compression setter 返回错误。正常完成后，结果交给 `loader.rs::NewLoaderWithStore`：`Ignore` 和未命中被跳过，其余结果参与 filter、`FileInfo` 构造与 `insert_meta` 分组。

默认规则顺序本身是行为：忽略规则置前，库/表/视图 schema 规则位于通用数据规则之前；内部压缩 Parquet 专用规则又位于通用数据规则之前，避免把 `snappy` 等内部 codec 错当成表名或整体压缩后缀。

## 数据与状态

- 已编译路由器在构造后不再修改：`ChainRouters.routers` 和每个 `RegexRouter` 的 `pattern/extractors` 都是私有拥有数据，单次 `route` 只创建局部 `Captures` 与 `RouteResult`。
- `FileRouteRule` 在 parser 内先被克隆；常量路径转换只修改克隆，Rust 调用方传入的配置不会像 Go 的指针版本那样被原地改写。
- `RouteResult.key` 保存分片序号等排序键但不解释其数值；loader 后续将它转入文件元数据排序流程。缺失可选捕获时展开为空字符串。
- `SourceType::Ignore` 同时是默认结果和明确忽略规则的类型。调用方必须区分 `Ok(None)`（没有规则命中）与 `Some(RouteResult { source_type: Ignore, .. })`（规则明确命中并忽略）；loader 当前对两者都跳过。
- `Logger` 是文件内唯一共享可变状态。其 `messages` 由 `Arc<Mutex<_>>` 保护，parser 克隆 logger 到 schema/table setter 后仍写入同一列表。
- `find_expand_variables` 每次校验模板时动态编译一个固定正则；规则数量很大时这属于构造期成本，不影响每个文件的匹配热路径。

## 依赖与调用关系

上游直接证据：

- `pkg/lightning/mydump/lib.rs` 声明并重导出本模块，同时把 `router_test.rs` 作为独立测试模块挂载。
- `pkg/lightning/mydump/loader.rs::LoaderConfig::default` 调用 `default_file_route_rules`；`NewLoaderWithStore` 调用 `NewFileRouter`，再在扫描循环调用 `router.Route(&path)`。
- `loader.rs::newAuroraFileRouter` 先以 fallback router 检查异常数据路径，再生成更高优先级的 `FileRouteRule`，最终仍回到本文件的 `NewFileRouter`；说明该路由抽象同时承载默认、自定义和 Aurora 自动规则。
- `loader.rs::insert_meta` 消费 `RouteResult.source_type/schema/name`：schema/view 文件写 `schema_file`，SQL/CSV/Parquet 进入 `data_files` 并参与大小汇总和排序。

下游直接依赖：

- `regex::Regex/Captures`：正则编译、路径捕获、模板展开及字面转义。
- `percent_encoding::percent_decode_str`：仅用于 schema/table 的 UTF-8 percent decode。
- `std::sync::{Arc, Mutex}`：共享路由告警日志，不参与路由规则本身同步。
- `crate::MydumpError::Routing`：承载规则配置、模板展开、来源类型和压缩类型错误。

RustCodeGraph 的 `files --filter pkg/lightning/mydump` 确认 `router.rs`、`loader.rs`、`lib.rs` 和独立测试均在索引中；`query` 能定位 `new_file_router` 与 `default_file_route_rules`。本次索引对这些 Rust 函数的 `callers/callees` 没有返回边，因此上述调用关系又以模块内直接引用搜索和对应源码位置复核，没有把索引缺边推断成“无调用者”。

## 错误处理与边界

- 构造期硬错误包括：`path`/`pattern` 皆空或同时设置、正则语法错误、type/schema/table 必需模板为空、数字捕获越界、命名捕获不存在。`new_file_router` 原样向上传播并整体失败。
- 运行期不匹配不是错误，而是 `Ok(None)`；匹配后若 type 字符串未知或 compression 字符串无效，则为 `MydumpError::Routing`。
- `parse_source_type` 和 `parse_compression_type` 都先 trim 并做 ASCII 小写，故大小写与周边空白不影响合法值。`parse_compression_on_file_extension` 只看最后一个点后的后缀，未知后缀有意降级为 `Compression::None`。
- schema/table percent decode 失败不是硬错误：`set_routed_value` 记录固定告警并返回原字符串。当前 logger 不记录失败值和底层错误细节，这是与 Go 结构化日志相比的信息差异。
- 全文件 Parquet 压缩明确不支持：只有 `source_type == Parquet && compression != None` 才拒绝；`table.0001.snappy.parquet` 的 `snappy` 是内部 codec 位置，由专用默认规则匹配且不会填充整体 `compression`。
- `to_storage_compress_type` 的支持集合窄于 `Compression` 枚举。新增或使用 LZ4/XZ/LZO 路由并不意味着下游存储读取已支持它们。
- 规则不是隐式全路径锚定的；是否匹配子串由配置正则自身的 `^/$` 决定。常量 `path` 经 `regex::escape`，但没有额外加锚，现有测试只证明目标常量匹配且明显替换后的路径不匹配；扩展时若需要严格全串语义，应先与 Go `regexp.QuoteMeta` 行为和配置兼容性对齐。
- `Logger` 的 `Mutex::lock().unwrap()` 在 mutex 中毒时会 panic；当前临界区只做 `Vec` push/clone，没有用户回调，正常路径不产生中毒。

## 并发与资源生命周期

路由本身是同步、CPU 内存内逻辑，没有异步任务、通道、文件句柄、事务或显式释放协议。`RegexRouter` 在构造期编译一次，随后每次调用只借用规则并创建局部结果；因此没有跨调用的匹配状态。

`Logger` 通过 `Arc<Mutex<Vec<String>>>` 允许多个 parser/setter 克隆共享消息。锁只在追加或复制消息期间持有，不跨正则匹配、模板展开或外部调用；`messages()` 返回快照而不是暴露内部引用。`FileRouter` trait 本身没有声明 `Send + Sync`，但当前 `RegexRouter`/`ChainRouters` 的字段没有可变匹配状态；若未来要把 trait object 跨线程共享，应在接口和调用处明确约束并增加并发测试，而不能只依赖当前具体类型的自动 trait 推导。

规则和捕获字符串均由路由器或单次调用拥有；`Captures` 不逃逸，`RouteResult` 返回拥有的 `String`，所以调用结束后不保留输入路径借用。构造失败时局部已编译 routers 随栈展开释放，不存在部分注册状态。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/mydump/router.go`，对应测试是 `router_test.go`。核心语义保持一致：

- `SourceType`/`Compression` 判别顺序、字符串映射和 `ToStorageCompressType` 支持集合一致。
- `default_file_route_rules` 的七条规则及顺序一致，包括备份忽略、schema/view、内部压缩 Parquet 和通用数据文件。
- Go `FileRouter.Route` 的 `nil, nil` 对应 Rust `Ok(None)`；Go `chainRouters` 与 Rust `ChainRouters` 都是首个命中优先。
- Go `regexRouterParser.Parse` 与 Rust `RegexRouterParser::parse` 的字段解析顺序、库 schema 不要求 table、ignore 提前返回、模板捕获校验和全文件压缩 Parquet 拒绝一致。
- Go 的 `patExpander` 保存 `applyFn` 闭包；Rust 用闭合枚举 `Setter` 表达同一组赋值行为。Go 的嵌入 `filter.Table` 在 Rust 中拆为 `RouteResult.schema/name`。

已确认的实现差异：

- Go 规则来自 `config.FileRouteRule` 指针且常量 path 分支会修改该对象；Rust 使用本 crate 的值类型并先 clone，不修改输入。
- Go URL 解码使用 `url.PathUnescape` 并记录 value/error 结构化字段；Rust 使用 `percent_decode_str(...).decode_utf8()`，失败时只保存固定文本消息。
- Go 模板变量正则接受 Unicode 字母/数字（`\pL`、`\p{Nd}`）；Rust `find_expand_variables` 使用 POSIX `[:alnum:]`。本次未通过测试证明所有非 ASCII 命名捕获完全等价，扩展 Unicode 模板前应补对照用例。
- Go `ToStorageCompressType` 的错误分支同时返回 `NoCompression` 和 error；Rust 用 `Result` 只返回 `Err`，调用者不能读取伴随的默认值。
- Go 文件还包含 Aurora router；Rust 将对应逻辑放在 `loader.rs::newAuroraFileRouter`，但生成的规则仍由本文件编译执行。

Rust `router_test.rs` 的 `TestRouteParser`、`TestDefaultRouter`、`TestInvalidRouteRule`、`TestSingleRouteRule`、`TestMultiRouteRule`、`TestRouteExpanding`、`parsing_contract_matches_go`、`TestRouteWithPath` 和 `TestRouteWithCompressedParquet` 与 Go 测试覆盖的主要契约相呼应。

## 扩展指南

- 新增来源类型：同步修改 `SourceType`、字符串常量、`parse_source_type`、`SourceType::as_str`，确认 `RouteResult` 消费端（特别是 `loader.rs` 的跳过逻辑与 `insert_meta`）如何处理，并在独立 `router_test.rs` 与必要的 loader 测试中增加 Go 对照用例。
- 新增整体压缩格式：先扩展 `Compression` 与 `parse_compression_type`；若读取层可用，再同步 `CompressType`/`to_storage_compress_type` 和实际 reader/parser。不要仅让文件名识别通过而遗漏解压能力；还要明确 Parquet 整体压缩政策。
- 修改默认命名：在 `default_file_route_rules` 调整时审查顺序和规则交叠，尤其是 ignore、schema/view 与 Parquet 专用规则；同步修改 Go `defaultFileRouteRules` 及两侧独立测试，避免同一路径在两种实现中落到不同首匹配规则。
- 扩展模板语法：修改 `find_expand_variables`、`check_sub_patterns` 和实际 `Captures::expand` 的兼容假设，重点测试 `$` 字面量、数字/命名捕获、相邻标识符、可选未命中捕获和 Unicode 名称。
- 改变 URL 解码或日志：修改 `set_routed_value`/`Logger`，明确失败是保留原值还是终止；若 logger 需要跨线程/高吞吐，评估无限增长的消息 Vec 与全局 mutex 争用。
- 新增测试必须继续放在独立 `pkg/lightning/mydump/router_test.rs`，不要嵌入生产源文件；Go 迁移语义变化同时更新 `router_test.go` 或记录有意差异。
- 性能敏感扩展应把固定正则（如模板变量扫描）改为一次初始化或构造器级复用，并用大规则集/大文件清单基准验证；当前文档没有性能基准证据，不能宣称路由成本可忽略。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/lightning/mydump` 列出目标、模块入口、loader、Go/Rust 对照和测试；`node --file pkg/lightning/mydump/router.rs --offset 1 --limit 500` 与 `--offset 501 --limit 200` 覆盖目标 641 行；`query new_file_router --kind function`、`query default_file_route_rules --kind function` 均定位到本文件。对相同符号执行的 `callers/callees` 无返回，已用直接引用搜索补证。
- 生产源码：`pkg/lightning/mydump/router.rs`（全部类型、规则编译、模板展开、错误边界）；`pkg/lightning/mydump/lib.rs`（模块装配和重导出）；`pkg/lightning/mydump/loader.rs`（`LoaderConfig::default`、`NewLoaderWithStore`、`newAuroraFileRouter`、`insert_meta`、自由函数 `route`）。该目录不存在 `doc.go`，因此最近的包级入口证据取自 `lib.rs` 和 `Cargo.toml`。
- crate 边界：`pkg/lightning/mydump/Cargo.toml` 声明包名 `astersql-lightning-mydump`、入口 `lib.rs`、`percent-encoding`/`regex`/`thiserror` 等依赖，以及 `go-package = "pkg/lightning/mydump"` 移植对应关系。
- Go 对照：`pkg/lightning/mydump/router.go`（同名枚举、默认规则、router/parser/expander/result）；`pkg/lightning/mydump/router_test.go`（规则编译、默认路由、非法规则、展开、常量 path、压缩 Parquet）。
- Rust 独立测试：`pkg/lightning/mydump/router_test.rs` 覆盖九个上述契约测试；本任务按计划是纯文档分析，没有运行 Cargo，也没有把测试写入生产文件。
- 调用引用搜索：在 `pkg/lightning` 与 `lightning` 的 Rust/Go 文件中检索 `NewFileRouter`、`FileRouter`、`RouteResult`、`default_file_route_rules` 和 `ToStorageCompressType`，确认 Rust 生产调用集中于 `loader.rs`，并确认 Go importer/loader/reader/parser 的对应消费链。
- 结构验收使用任务指定命令，要求目标存在且固定二级标题恰好为 11 个；交付前还人工复核唯一新增生产物、未修改 `plan.md`、没有把未验证的 Unicode 等价或性能结论写成已支持事实。
