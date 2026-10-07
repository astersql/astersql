# `dumpling/export/config.rs`

源文件：[`config.rs`](./config.rs)

## 文件定位

`config.rs` 是 `astersql-dumpling-export` crate 的配置装配中心。该 crate 在 [`Cargo.toml`](./Cargo.toml) 中声明为库，入口是 [`lib.rs`](./lib.rs)；`lib.rs` 不是用独立子模块隔离本文件，而是在 crate 根作用域执行 `include!("config.rs")`，因此这里的公开类型和函数直接成为 crate 级 API，并与同样被 `include!` 的 `prepare.rs`、`column_filter.rs`、`dump.rs` 等文件共享符号。

在完整导出链中，本文件处于“用户输入/程序化配置”与“导出运行时”之间：`DefaultConfig` 建立可运行的基线，`Config::DefineFlags` 与 `Config::ParseFromFlags` 把命令行值写回配置，`dump.rs::NewDumper` 再依次调用 `buildTLSConfig`、`validateSpecifiedSQL`、`adjustFileFormat` 和 `validateIncludeGeneratedColumns`，之后才创建上下文、指标、存储、数据库连接等资源。它不是实际写文件或查询数据库的实现；这些职责分别在 `writer.rs`、`dump.rs`、`conn.rs` 等文件中。

当前 Rust 文件是迁移期实现而非 Go 版本的无差别复刻。源码明确保留了 arm64-safe 的轻量替身路径；例如 `buildTLSConfig` 目前是空操作，`createExternalStorage` 使用 `MemStorage`，内置 `FlagSet` 也只覆盖配置测试需要的 pflag 子集。阅读或扩展时必须以这些当前事实为准，不能把 Go 版完整能力视为 Rust 已支持。

## 核心职责

1. 定义输出兼容模型：`CSVDialect`、`BinaryFormat` 与 `DialectBinaryFormatMap` 决定不同 CSV 目标如何表示二进制列。
2. 定义总配置状态：`Security` 保存 TLS 路径和证书字节，`Config` 聚合连接、筛选、输出、模板、监控、存储、PD/GC、Parquet 等选项。
3. 提供默认值与可变副本：`DefaultConfig` 生成基线；`Config::clone_for_mutate` 复制静态/共享状态，同时刻意清空 `StatusAddr`、重置 `Labels`，供 `Arc<Config>` 场景下安全地产生新配置。
4. 解析并规范化 CLI 输入：`DefineFlags` 注册当前 Rust 子集，`FlagSet::Parse` 接受 `--name value` 和 `--name=value`，`ParseFromFlags` 完成类型转换、表/过滤器/模板/压缩/方言/Parquet 参数装配。
5. 在资源打开前拒绝冲突配置：`validateSpecifiedSQL`、`adjustFileFormat`、`validateIncludeGeneratedColumns` 约束 SQL、where、分区、生成列、压缩与输出格式的组合；输出分片时还校验文件名模板包含条件块外的独立 `{{.Index}}`。
6. 提供配置辅助操作：文件大小、表清单、方言、压缩、分区和 MySQL 缺陷版本解析，以及外部存储缓存和驱动配置投影。

## 主要符号

- `CSVDialect` / `BinaryFormat` / `DialectBinaryFormatMap`：公开枚举与映射函数。默认方言对应 UTF-8，Snowflake/Redshift 对应 HEX，BigQuery 对应 Base64；枚举使用 `#[repr(i32)]` 保持稳定的整数判别值。
- `Security`：公开 TLS 材料容器；`Default` 把路径和字节全部置空。与 Go 不同，它没有保存已构造的 `tls.Config`。
- `Config`：核心公开结构体。重要字段组包括连接认证（`Host`、`Port`、`User`、`Password`、`Security`）、范围选择（`Databases`、`Tables`、`TableFilter`、`Where`、`SQL`、`Partitions`、`columnFilter`）、输出（`FileType`、CSV 参数、`OutputFileTemplate`、`Rows`、`FileSize`、压缩和 Parquet 参数）以及运行时共享对象（`Logger`、Prometheus factory/registry、`ExtStorage`、`IOTotalBytes`）。`columnFilter`、`columnProjection` 保持 crate 内可见语义，由同一 crate 的配置/导出流程消费。
- `DefaultConfig() -> Config`：以 `127.0.0.1:3306`、`root`、4 线程、自动一致性、SQL 文本默认推导、默认 CSV 字符、1 MiB Parquet 页和默认 row-group 限制等值建立基线；同时创建全表过滤器、默认输出模板和 Prometheus 对象。
- `Config::String`：只输出 Host、Port、User、Consistency、FileType 的 JSON 形摘要，不是完整序列化；密码等字段不会进入结果。
- `Config::GetDriverConfig`：把连接相关字段投影到 `MysqlConfig`，空 `Net` 回落为 `tcp`，但不尝试连接。
- `Config::createExternalStorage`：若 `ExtStorage` 已存在则克隆 `Arc` 复用，否则用 `OutputDirPath` 创建并缓存 `MemStorage`。
- `Config::clone_for_mutate`：手工复制绝大多数字段，共享 `Arc` 型依赖和原子计数器；刻意把 `BackendOptions`、`StatusAddr`、`Labels` 恢复为默认/空状态。
- `ParseFileSize`、`ParseTableFilter`、`GetConfTables`、`ParseOutputDialect`、`parseParquetCompressType`：将 CLI 文本转换为内部尺寸、过滤器、按库聚合的表集合、CSV 方言和压缩枚举。
- `adjustConfig`：按切片顺序执行 `fn(&mut Config) -> Result<()>`；首个错误立即终止后续修正。
- `buildTLSConfig`：保留给构造 TLS 状态的扩展挂点，当前直接返回成功，不读取路径、不验证证书，也不产生 TLS 对象。
- `validateSpecifiedSQL` / `adjustFileFormat`：前者拒绝 `--sql` 与 `--where`/`--partitions` 并用；后者小写化文件类型、推导默认格式，并约束 SQL/Parquet/通用压缩组合。
- `matchMysqlBugversion`：仅对 MySQL 且有版本号时判断开区间 `(8.0.2, 8.0.23)`；TiDB、未知服务或边界版本不命中。
- `normalizePartitions`：对分区名 trim、小写化、去空并去重，同时保留首次出现顺序。
- `outputTemplateUsesIndex`：读取指定模板定义，移除简单的 `{{if ...}}{{end}}` 块后检查是否仍有 `{{.Index}}`；这是针对当前模板风险模型的启发式检查，而非完整 Go template 语法分析器。
- `FlagSet`：公开但定位为测试所需的最小 pflag 替身；保存字符串值、默认值、显式变更集合和可重复数组参数。
- `Config::DefineFlags` / `Config::ParseFromFlags`：定义并消费 Rust 当前支持的 CLI 子集。后者还串联列过滤、表过滤、文件大小、模板、压缩、CSV 方言和 Parquet 尺寸解析。
- `GeneratedColumnsMode` 及 `GeneratedColumnsNone/Stored/Virtual/All`：以字符串保留程序化配置输入；当前只接受 `none` 和 `stored`，`virtual`/`all` 是保留值但明确报“不支持”。
- `validateIncludeGeneratedColumns` / `Config::includeStoredGeneratedColumns`：规范化生成列模式、检查冲突，并为下游列投影提供布尔判断。

## 执行流程

典型 CLI/测试路径如下：

1. 调用方用 `DefaultConfig` 建立初始配置，再由 `Config::DefineFlags` 用这些默认值注册 flag。
2. `FlagSet::Parse` 记录用户值和 `Changed` 状态；数组参数可重复追加，普通参数覆盖旧值。无 `--` 前缀、非布尔参数缺值会立即报错。
3. `Config::ParseFromFlags` 先复制标量字段并解析整数/无符号整数；接着验证线程数大于 0、CSV 分隔符非空。
4. 它读取 tables-list、filter 与列过滤选项：`parseColumnFilterOptions` 处理 inline/file 冲突，`GetConfTables` 生成 `DatabaseTables`，`ParseTableFilter` 生成实际过滤器。
5. `ParseFileSize` 解析 split 阈值；若自定义 SQL 没有显式模板，则选择匿名模板。`ParseOutputFileTemplate` 生成模板对象。
6. 当 `Rows != 0` 或 `FileSize != 0` 且用户显式改变模板时，`outputTemplateUsesIndex` 必须证明 data 模板在条件块外包含独立索引，否则在写文件之前拒绝，避免 chunk 覆盖。
7. 最后解析通用压缩、CSV 方言、Parquet 压缩/页大小/row-group 大小，以及 PD 和集群 TLS 路径。CSV 方言只能与显式 CSV 文件类型组合。
8. 运行入口 `dump.rs::NewDumper` 收到 `Config` 后，在创建任何运行时资源前执行 `buildTLSConfig -> validateSpecifiedSQL -> adjustFileFormat -> validateIncludeGeneratedColumns`。这保证空文件类型已推导（有 SQL 为 CSV，否则为 SQL text），生成列校验看到的是最终格式，并且错误优先于存储/数据库初始化。
9. 初始化步骤中的 `dump.rs::createExternalStore` 从 `Arc<Config>` 调用 `clone_for_mutate`，再通过 `createExternalStorage` 创建或复用存储并回写新 `Arc<Config>`。导出列投影在 `dump.rs::buildColumnProjection` 中调用 `includeStoredGeneratedColumns`，决定是否把 stored generated columns 纳入可写列集合。

程序化调用若绕开 `NewDumper`，必须自行保持同样的校验顺序；特别是 `validateIncludeGeneratedColumns` 的文档契约要求在 `adjustFileFormat` 之后运行。

## 数据与状态

`Config` 同时包含静态选项和少量懒初始化运行时状态。`ExtStorage: Option<Arc<dyn Storage>>` 是缓存：第一次创建后，后续调用共享同一存储对象。`PromFactory`、`PromRegistry`、`IOTotalBytes`、`TableFilter` 也用 `Arc` 共享；`clone_for_mutate` 不深拷贝这些对象，所以副本间可观察同一原子计数或同一 trait object。字符串、向量、map、`Security` 字节则被复制。

`UnspecifiedSize == 0` 同时表示 rows/filesize 未指定并关闭相应 split 条件。`DefaultStatementSize == 1_000_000` 是 INSERT 目标大小；`defaultTaskChannelCapacity == 128`、`defaultDumpGCSafePointTTL == 300` 秒和 `dumplingServiceSafePointPrefix == "dumpling"` 供导出执行/GC 协调路径使用。`LooseCollationCompatible` 与 `StrictCollationCompatible` 是排序规则兼容模式的字符串契约。

配置中的密码和证书字节属于敏感状态，但 Rust 的 `Config::String` 只输出五个摘要字段。另一方面，`Config` 未实现自动脱敏序列化；新增日志代码不应直接调试打印整个对象。

`FlagSet.changed` 的意义不同于最终值：它区分“用户显式设置了某个值”和“沿用默认值”。模板的 split 安全校验正是只在用户改变 `output-filename-template` 时触发。`normalizePartitions` 返回新向量，不修改输入；去重用临时 `HashMap`，输出顺序由首次出现顺序决定。

## 依赖与调用关系

- crate 边界：[`Cargo.toml`](./Cargo.toml) 将本目录定义为 `astersql-dumpling-export`，直接依赖 dumpling 的 `cli/context/log`、CSV/SQL/Parquet dumpformat、objstore store API、table-filter、parser/format/mysql、`toml` 和 `regex`。本文件通过 `lib.rs` 根级导入与 `stubs.rs` 的再导出取得这些类型，而不是在文件自身重复 `use`。
- 上游入口：RustCodeGraph 将 `adjustFileFormat` 和 `validateIncludeGeneratedColumns` 的生产调用者定位为 `dump.rs::NewDumper`；`createExternalStorage` 的直接生产桥接是 `dump.rs::createExternalStore`。`DefaultConfig` 也被 `main_test.rs`、`dump_test.rs`、`prepare_test.rs`、`parity_test.rs` 等测试夹具消费。
- 下游依赖：`DefaultConfig` 调用 `filter_parse`、`DefaultOutputFileTemplate`、`NewDefaultFactory`、`NewDefaultRegistry` 和 `ServerInfoUnknown`；`ParseFromFlags` 调用列过滤实现、表过滤、模板解析、容量解析和错误构造辅助；这些符号来自同 crate 的 `prepare.rs`、`column_filter.rs`、`stubs.rs` 等 include 文件。
- 运行期消费：`Dumper` 持有 `Arc<Config>`。writer/metadata/SQL 相关逻辑读取格式、模板、压缩、表集合和筛选字段；`buildColumnProjection` 通过 `includeStoredGeneratedColumns` 把生成列模式落实为查询字段选择。
- 图查询限制：索引对 `config.rs` 的 basename/同名 Go 符号存在少量跨语言误匹配，因而调用结论以带 `--file dumpling/export/config.rs` 的查询、文件级 “used by” 结果及真实 `dump.rs` 源码交叉确认，未采用明显属于 `br/`、`pkg/ddl` 等路径的噪声边。

## 错误处理与边界

所有可能失败的解析/校验使用 crate 的 `Result`/`Error`，一般在发现首个错误时返回：

- `ParseFileSize("")` 返回未指定；纯数字按 MiB 解释并使用 `wrapping_mul`，这与 Go 的普通无符号乘法结果在溢出时同为模运算，但 Rust 当前不打印 Go 版“无单位不推荐”的警告；其他文本交给 `RAMInBytes`，失败时给出包含原值的错误。
- `GetConfTables` 要求每项至少有一个点，并只按第一个点拆成 database/table；缺点时错误精确指向该元素。
- `ParseTableFilter` 在 tables-list 非空时只允许过滤器严格等于 `["*.*", DefaultTableFilter]`，顺序或大小写变化也会冲突；无 tables-list 且 filters 为空时 Rust 会主动补默认系统库排除规则和全表通配。
- `ParseOutputDialect` 大小写不敏感，只接受空/default、snowflake、redshift、bigquery；`mysql` 等未声明别名会失败。
- `parseCompressType` 与 `parseParquetCompressType` 只接受 no-compression、gzip、snappy、zstd（及空值）；Parquet 文件类型禁止同时使用通用 `--compress`。
- `validateSpecifiedSQL` 禁止 SQL 与 where、partitions 并用；`adjustFileFormat` 禁止 SQL + sql filetype，并拒绝未知文件类型。
- `ParseFromFlags` 拒绝非正线程数、空 CSV separator、非法数字/容量、错误列过滤、冲突表过滤、不安全分片模板、非 CSV 下的 CSV dialect 等。当前 `FlagSet::GetInt/GetUint64` 会静默回落 0，但 `ParseFromFlags` 对关键数字使用 `GetIntResult/GetUint64Result`，保留可见错误。
- `parseGeneratedColumnsMode` 只支持 `none`/`stored`；`validateIncludeGeneratedColumns` 对 stored 模式按 SQL、where、column filter、no-data、SQL text 的固定优先级返回冲突错误。none/空模式会先规范化为 none 然后跳过其他冲突。
- `outputTemplateUsesIndex` 只剥离简单 `if...end` 块；嵌套或更复杂模板结构不应假定已被完整语法分析。任何扩展都应同步增加覆盖覆盖风险的独立测试。
- `timestampDirName` 使用 `SystemTime::duration_since(UNIX_EPOCH).unwrap_or_default()`；系统时间早于 epoch 时回落为 `0`，并且仅有秒级粒度，不能保证同秒并发调用唯一。

## 并发与资源生命周期

本文件不启动线程、任务或 channel；`defaultTaskChannelCapacity` 只是下游容量常量。并发语义主要来自共享所有权：`Config` 被 `Dumper` 包在 `Arc` 中，修改前通过 `clone_for_mutate` 建立新值，再整体替换 `Arc`；这避免直接可变借用已共享配置。`IOTotalBytes: Arc<AtomicU64>` 在副本间共享累计状态，原子操作的具体顺序由消费方决定。

`createExternalStorage` 是惰性初始化，但它要求 `&mut Config`，因此在单个可变配置访问内检查并写入缓存；函数自身没有跨线程锁，不能据此推断多个独立副本会只创建一次资源。`ExtStorage` 的 `Arc` 负责共享所有权，真正的关闭/释放由具体 `Storage` 实现与最后一个引用的生命周期决定。

`clone_for_mutate` 清空 `StatusAddr`，避免派生配置再次监听同一状态端口；重置 `Labels` 防止上一轮运行的标签状态泄漏。它保留 `Logger`、Prometheus 对象、存储和原子计数器，因此这些资源不会因配置派生而自动重建。`NewDumper` 的资源顺序保证配置校验在上下文、HTTP、SQL DB 等资源打开之前完成，相关回归由 `prepare_test.rs::generated_columns_dumper_validates_before_opening_resources` 锁定。

## 与 Go 版本的对应关系

主要对齐点来自 [`config.go`](./config.go) 与 [`config_test.go`](./config_test.go)：

- 枚举、核心字段、默认主机/端口/用户/线程、默认过滤器、statement size、CSV 字符、输出模板、一致性、Parquet 默认值和生成列默认模式基本一一对应。Go 的 `DialectBinaryFormatMap` 是静态 map，Rust 用纯函数表达相同映射。
- Rust 的 `ParseFromFlags` 保留了 Go 主流程的字段复制、列过滤冲突、表/过滤器构造、split 模板保护、压缩/方言/Parquet 解析；Rust 与 Go 测试都覆盖 MySQL 版本开区间、表名错误、Parquet 默认与尺寸、大小写方言、模板独立 Index 和生成列模式。
- Go `NewDumper` 与 Rust `NewDumper` 都在资源初始化前运行 TLS、SQL/格式/生成列校验；生成列 stored 模式的错误顺序与允许格式也保持一致。

已确认的差异/未完成迁移点：

- Go `buildTLSConfig` 用路径、内存证书和最低 TLS 版本构造 `tls.Config` 并保存到 `Security.TLS`；Rust `Security` 无 TLS 对象字段，`buildTLSConfig` 当前无条件成功。
- Go `createExternalStorage` 解析本地/对象存储 backend 并调用 objstore；Rust 总是构造 `MemStorage(OutputDirPath)`，虽测试 URI 为 `file:`，但不能据此宣称已支持 Go 的远端 backend。
- Go 使用完整 `pflag.FlagSet`；Rust `FlagSet` 是测试子集。Rust 源码明确未在 `ParseFromFlags` 展开 `read-timeout`、`params`、TLS bytes 和 backend options；Go 会解析 duration、把 session param key 小写化，并调用 `BackendOptions.ParseFromFlags`。
- Go 在非 case-sensitive 模式下给 table filter 包 `filter.CaseInsensitive`；Rust 当前把 case-sensitive 传给列过滤解析，但没有在本函数中为 `TableFilter` 增加等价包装。
- Go 无单位 filesize 会打印迁移警告，Rust 不打印。Go `timestampDirName` 使用 RFC3339 格式，Rust 返回 epoch 秒文本。
- Go 的 `Config.String` 走 JSON marshal（并依赖字段标签隐藏敏感项）；Rust 只生成五字段摘要。两者都不应被理解为完整配置往返序列化。

因此，扩展 Rust 时应优先保持已有对齐行为，但不能为了表面一致而跳过仍缺失的真实依赖或用无验证桩声称支持。

## 扩展指南

- 新增配置字段时，至少同步检查 `Config`、`DefaultConfig`、`clone_for_mutate`、`DefineFlags`、`ParseFromFlags` 和 `Config::String` 是否需要变更；若字段影响运行前合法性，还应插入 `NewDumper` 的校验序列。遗漏 `clone_for_mutate` 会导致从 `Arc<Config>` 派生配置时静默丢值。
- 扩展 CLI 时，不要把当前 `FlagSet` 当作完整 pflag。应明确需要继续测试替身还是接入真实 CLI 层，并为 `--name value`、`--name=value`、默认值、显式 false、重复参数和错误值分别定义契约。尤其当前布尔裸 flag 与 `--flag=false` 的处理路径不同。
- 实现 TLS 时，最可能修改 `Security` 与 `buildTLSConfig`，并需要在独立的 `config_test.rs` 或连接层测试文件验证路径/内存材料、证书键配对、最低版本和错误传播；不得把测试写进生产 `config.rs`。
- 实现对象存储时，修改 `createExternalStorage` 与 `BackendOptions` 接线，并核对 `Cargo.toml` 的正式依赖；必须保留“已有 `ExtStorage` 直接复用”的生命周期契约，并增加本地与远端 backend 的独立测试。
- 新增格式/方言/压缩类型时，同时更新解析函数、`adjustFileFormat` 的组合约束、writer 的真实消费点、Go 对照和 `config_test.rs`。只让解析成功而 writer 不认识该值不算完成。
- 修改 split 模板规则时，从“是否可能覆盖已有 chunk”出发，更新 `outputTemplateUsesIndex` 和 rows/filesize 两组测试；复杂 Go template 语法可能需要替换当前字符串启发式实现，而非继续叠加脆弱替换。
- 扩展生成列能力时，依次检查 `parseGeneratedColumnsMode`、`validateIncludeGeneratedColumns`、`includeStoredGeneratedColumns`、`dump.rs::buildColumnProjection` 及 schema/data 一致性。`config_test.rs` 负责 flag/模式，`prepare_test.rs` 负责校验次序，`dump_test.rs` 负责最终数据列变化；三类证据不可互相替代。
- 性能风险集中在配置派生时的大 map/vector/证书字节克隆和模板/过滤器重复解析；兼容风险集中在默认值、错误优先级、大小写、flag 显式性及 Go/Rust 差异。修改前应先复用同路径 Go 测试意图，不要删减边界来让 Rust 测试通过。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、7,032 个 Rust 文件；目标 `dumpling/export/config.rs` 已索引，共 990 行/72 个符号，文件级反向引用包括 `dump.rs`、`dump_test.rs`、`prepare_test.rs`。
- 已执行的图查询：目标文件 `node --file` 全文；`query DefaultConfig/ParseFromFlags/adjustFileFormat/validateIncludeGeneratedColumns/createExternalStorage`；对 `DefaultConfig`、`adjustFileFormat`、`validateIncludeGeneratedColumns` 执行 `callers`/`callees` 并用 `--file` 消歧。关键可信边为 `dump.rs::NewDumper -> adjustFileFormat/validateIncludeGeneratedColumns`、`DefaultConfig -> filter_parse/DefaultOutputFileTemplate/NewDefaultFactory/NewDefaultRegistry`。
- 已读 Rust 生产路径：[`config.rs`](./config.rs)、[`lib.rs`](./lib.rs)、[`dump.rs`](./dump.rs) 的 `NewDumper`、`createExternalStore` 与 `buildColumnProjection`，以及 [`Cargo.toml`](./Cargo.toml)。本目录没有 `doc.go`，故无额外 package contract 可读。
- 已读 Rust 独立测试：[`config_test.rs`](./config_test.rs) 全文；[`prepare_test.rs`](./prepare_test.rs) 的生成列校验与“资源打开前失败”用例；[`dump_test.rs`](./dump_test.rs) 的生成列 CSV 数据/模式用例。测试位于独立文件，由 `lib.rs` 的 `#[cfg(test)]` 模块声明挂载，符合生产逻辑与测试分离要求。
- 已读 Go 对照：[`config.go`](./config.go) 的枚举/结构体、`DefaultConfig`、`ParseFromFlags`、`ParseFileSize`、`ParseTableFilter`、`createExternalStorage`、`buildTLSConfig`、`adjustFileFormat`、`validateIncludeGeneratedColumns`；[`config_test.go`](./config_test.go) 的存储、版本、表清单、session params、列过滤、Parquet、CSV 方言、模板和生成列用例。
- 人工复核结论：本文区分了已接线行为与当前桩/简化边界，说明了文件存在原因、从 flag 到 `NewDumper` 的运行顺序、共享状态与资源生命周期，以及安全扩展时应修改的符号和独立测试位置；未运行 Cargo，符合纯文档任务约束。
