# `dumpling/cmd/dumpling/config_flags.rs`

## 文件定位

该文件是 `astersql-dumpling-cmd-dumpling` crate 的命令行配置适配层。`dumpling/cmd/dumpling/lib.rs` 以 `config_flags` 模块公开它；真正的进程控制流位于 `dumpling/cmd/dumpling/main.rs`。后者在 `run_with_factory` 中创建本地 `FlagSet`，依次调用本文件的 `DefineFlags`、`FlagSet::Parse` 和 `ParseFromFlags`，再把得到的 `astersql_dumpling_export::Config` 交给 `export::NewDumper`。

`dumpling/cmd/dumpling/Cargo.toml` 将该目录声明为同时具有 library 和 binary 的 crate，并直接依赖 `astersql-dumpling-cli`、`astersql-dumpling-export` 和 `astersql-dumpling-log`。本文件只使用 export crate 和本 crate 的 `stubs::FlagSet`；它不是导出器实现，也不建立数据库连接。

## 核心职责

本文件承担两项相互配套的职责：

1. `DefineFlags(&mut FlagSet)` 注册 dumpling 的 CLI 参数、短参数、默认值、帮助文本与隐藏状态，使本地 flag stub 的可见契约尽量贴近 Go `(*Config).DefineFlags`。
2. `ParseFromFlags(&mut Config, &FlagSet)` 从已解析的 flag 集合构造可供导出器消费的配置：先逐字段回填，再执行过滤器、文件尺寸、输出模板、压缩、CSV 方言和 Parquet 尺寸等派生解析及组合约束。

它存在于 cmd crate 而非 export crate 的原因写在模块注释中：当前 arm64-safe export 适配面没有暴露完整 pflag 能力。因而这里是 CLI 胶水层，不能把它误认为 `dumpling/export/config.rs` 中简化 `Config::DefineFlags`/`Config::ParseFromFlags` 的简单转发。

## 主要符号

- `FLAG_*` 常量（第 41—105 行）集中保存 CLI 参数名。它们是模块私有常量，避免注册阶段和读取阶段使用不同拼写；`column-filter` 与 `column-filter-file` 目前仍直接使用字符串字面量。
- `timestamp_dir_name() -> String` 生成 `./export-YYYY-MM-DDTHH:MM:SSZ` 默认目录名。它读取 `SystemTime::now()`，并在系统时间早于 Unix epoch 或取时失败时回落到 epoch 秒数 `0`。
- `civil_date_from_unix_days(i64) -> (i64, u32, u32)` 使用 proleptic Gregorian 的 civil-from-days 算法，将 Unix 天数转换为年月日；只服务于默认输出目录生成。
- `human_bytes(i64) -> String` 调用 `export::HumanSize`，使 Parquet page/row-group 默认值沿用 export 层的人类可读格式。
- `pub fn DefineFlags(flags: &mut FlagSet)` 是第一个公开入口，注册连接、选择范围、并发与拆分、输出格式、TLS、session 参数、PD/集群 TLS、Parquet 等参数，并隐藏 `sql`、`read-timeout`、`transactional-consistency`。
- `parse_compress_type(&str) -> Result<CompressType, String>` 是大小写敏感的模块私有解析器：空字符串和 `no-compression`、`gzip|gz`、`snappy`、`zstd|zst` 有效，其他值返回 `unknown compress type ...`。
- `parse_size_flag(&FlagSet, &str) -> Result<i64, String>` 先取字符串 flag，再交给 `RAMInBytes`，由同一解析路径处理两个 Parquet 大小参数。
- `pub fn ParseFromFlags(conf: &mut Config, flags: &FlagSet) -> Result<(), String>` 是第二个公开入口；它会原地、分阶段修改 `conf`，成功时返回 `Ok(())`。

文件没有自定义类型、trait、`impl` 或条件编译项。公开 API 只有 `DefineFlags` 与 `ParseFromFlags`，其余符号均为内部实现。

## 执行流程

完整应用中的顺序由 `main.rs::run_with_factory` 固定：

1. `export::DefaultConfig()` 创建基础配置，`DefineFlags` 则把 CLI 默认值注册进新 `FlagSet`。这两个默认来源并不总相同，例如 parity test 证明 `DefaultConfig` 的端口是 3306，而 flag 默认端口是 4000；最终应以后续解析回填值为准。
2. `FlagSet::Parse` 先处理 argv。语法错误在 `main.rs` 中映射为退出码 2；帮助和版本路径会在调用 `ParseFromFlags` 前提前返回。
3. `ParseFromFlags` 第一阶段逐项读取连接、日志、一致性、输出、TLS、CSV 等简单字段，并对 partitions 调用 `normalizePartitions`。
4. 它立即拒绝 `Threads <= 0` 和空 CSV separator，然后读取 tables-list、filesize、filter、case-sensitive、column-filter、输出模板与 params 等需要联合推导的原始值。
5. `Config::parseColumnFilterOptions` 处理 inline/file 二选一、与 `--sql` 冲突、TOML 解析和匹配大小写；错误被转换为字符串返回。
6. `GetConfTables` 构造显式表集合；`ParseTableFilter` 构造表过滤器。默认大小写不敏感时，Rust 会规范化相关输入并以 `CaseInsensitive` 包装过滤器；当 tables-list 非空时保留原始 filters，以维持注册默认值和用户显式 `--filter` 的区分。
7. `ParseFileSize` 解析文件大小。SQL-only 且未给模板时补 `DefaultAnonymousOutputFileTemplateText`；随后解析模板，并在用户显式设置模板且 rows/filesize 开启 split 时，要求 data 模板在条件块外包含独立 `.Index`，以避免 chunk 覆盖。
8. 压缩别名收敛为 `CompressType`；CSV dialect 仅允许用于 CSV 文件类型；Parquet 压缩和两个尺寸参数分别解析并回填。
9. PD 地址、集群 TLS 字段直接写入配置；session 参数键统一转为小写后插入 `SessionParams`。当前 BackendOptions 解析明确是空操作，函数随后返回成功。

## 数据与状态

`DefineFlags` 修改调用者传入的 `FlagSet`，注册值的类型包括 bool、整数、字符串、字符串切片、字符串数组、字符串 map 和 duration。默认输出目录是唯一依赖当前时间的默认值；同一个 `FlagSet` 注册完成后该值保持不变。

`ParseFromFlags` 原地修改 `Config`，不是事务式构造：读取或校验在中途失败时，函数已经写入的早期字段不会回滚。因此调用者应像 `main.rs` 一样从新建的 `DefaultConfig` 开始，并在 `Err` 后丢弃该配置，而不应继续使用部分更新状态。

重要派生状态包括：`SpecifiedTables` 由 tables-list 是否为空决定；`Tables` 来自 `GetConfTables`；`TableFilter` 来自 tables-list/filter/case-sensitive 的组合；`FileSize`、`OutputFileTemplate`、压缩枚举、CSV dialect 和 Parquet 尺寸都存储解析后的规范形式；`SessionParams` 使用小写键，重复键遵循 map 的后写覆盖语义。

模块自身没有可变全局变量。唯一外部可观察的非确定状态是 `SystemTime::now()` 产生的默认目录时间戳。

## 依赖与调用关系

上游主链是 `lib.rs::main → entry::main → main.rs::run → run_with_factory → DefineFlags/ParseFromFlags`。测试上游包括独立的 `config_flags_test.rs` 和 `parity_test.rs`，它们直接构造 `FlagSet` 调用两个公开入口。

下游分为两层：

- `crate::stubs::{FlagSet, FlagHelp}` 提供 pflag 风格的注册、解析、`Changed`、typed getter、隐藏 flag 和 usage 行为。
- `astersql_dumpling_export` 提供 `Config` 及领域解析器：`GetConfTables`、`ParseTableFilter`、`ParseFileSize`、`ParseOutputFileTemplate`、`outputTemplateUsesIndex`、`ParseOutputDialect`、`parseParquetCompressType`、`RAMInBytes`、`normalizePartitions`，以及各类默认常量和枚举。

RustCodeGraph 将目标文件识别为含 66 个符号的已索引 Rust 文件，并精确定位 `config_flags.rs::DefineFlags`、`config_flags.rs::ParseFromFlags`、`timestamp_dir_name`、`parse_compress_type`、`parse_size_flag`。图数据库的精确 callers/callees 命令在本次环境中超时，所以上述调用边以模块导入和实际调用点复核：`main.rs` 第 27、88、119 行，以及两个独立测试文件中的直接调用。

## 错误处理与边界

所有 flag getter 错误都通过 `?` 原样转换为 `String`；领域解析器的错误通常提取其 `msg`。特殊上下文错误包括 `failed to parse filter: ...` 和隐藏原始模板解析细节、但回显用户模板文本的 `failed to parse output filename template (...)`。

本文件主动维护的关键边界是：线程数必须大于零；CSV separator 不得为空；inline column filters 与配置文件互斥，且两者均不能与 SQL-only 模式并用；split 模式下用户显式模板必须安全包含 `.Index`；CSV dialect 不能用于非 CSV；压缩名严格区分大小写；Parquet 大小必须被 `RAMInBytes` 接受。`config_flags_test.rs` 还固定了 `zst` 有效而 `none`、`uncompressed`、`GZIP` 无效的兼容集合。

该层只做部分组合校验。例如 consistency/snapshot 的完整业务合法性、TLS 文件可读性、PD 可达性和实际导出资源检查由后续导出流程负责。`BackendOptions.ParseFromFlags` 在这里是注释标明的 no-op，因此不能据此宣称对象存储 CLI 已接线。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、数据库连接或文件 writer。`--threads`、`--rows`、`--filesize` 和 Parquet row-group/page size 只是配置将来并发度、拆分粒度和内存阈值；实际资源由 `NewDumper` 及 export 层管理。

`ParseFromFlags` 要求调用期间独占 `&mut Config`，而只共享借用 `&FlagSet`，因此单次解析不存在内部并发写竞争。默认目录时间戳只在 `DefineFlags` 时读取一次；session map 的合并也在当前线程顺序执行。若未来让多个解析任务共享配置，应由调用方提供同步与独立实例，本模块没有并发安全协议。

资源生命周期方面，column-filter-file 的读取发生在 export 层 `parseColumnFilterOptions` 内；本文件不保留文件句柄。Dumper 的 `Dump/Close` 生命周期位于 `main.rs`，不属于这里。

## 与 Go 版本的对应关系

主要 Go 对照是 `dumpling/export/config.go` 的 `timestampDirName`、`(*Config).DefineFlags` 和 `(*Config).ParseFromFlags`，应用入口对照是 `dumpling/cmd/dumpling/main.go`。Rust 保留了 Go 的总体顺序、绝大多数 flag 默认值、隐藏参数、字段回填、过滤器/模板/压缩/方言/Parquet 解析以及 params 键小写化语义。

已确认的差异与迁移状态如下：

- Go 默认目录使用本地时区的 RFC3339；Rust 固定输出 UTC `Z`。二者形状兼容，但具体时区文本不保证相同。
- Go 首先调用 `objstore.DefineFlags`，最后调用 `BackendOptions.ParseFromFlags`；Rust 两处均明确为空操作，原因是当前 arm64-safe surface 未提供该能力。
- 当前 Go `DefineFlags`/`ParseFromFlags` 已包含 `include-generated-columns` 及其校验；本文件没有注册或解析它。因此本文件不是当前 Go 配置面的完全覆盖，扩展时应显式决定是否移植这一差异。
- Rust 在默认大小写不敏感路径中显式规范化 tables/filter 输入，以适配当前 Rust `PatternFilter` 行为；目标仍是复现 Go `filter.CaseInsensitive` 的外部效果。
- Go 使用结构化 `error` 与 trace；Rust 公共入口返回 `Result<(), String>`，保留关键文案但不保留 Go 错误链类型。

`dumpling/export/config.rs` 另有简化版 `Config` 方法，供 export crate 自身测试和能力使用；cmd 主流程明确导入本文件的自由函数，阅读和修改时不要混淆两套 flag surface。

## 扩展指南

新增 CLI 参数时，应同时完成四处对齐：在本文件增加稳定 flag 名常量（避免继续扩散字面量）、在 `DefineFlags` 注册类型/默认值/帮助与隐藏状态、在 `ParseFromFlags` 读取并做必要组合校验、在 `config_flags_test.rs` 增加独立回归测试。若参数影响入口退出码或全流程顺序，再同步扩展 `parity_test.rs`；不要把 Rust 测试写进生产源文件。

涉及 Go 对齐时，应以 `dumpling/export/config.go` 的实际增量及 `dumpling/export/config_test.go` 为依据，同时核对 export crate 的 `Config` 是否已经具备字段和领域解析器。对象存储参数不能只在此处注册：需要先补齐可复用的 BackendOptions surface，再恢复 `DefineFlags`/`ParseFromFlags` 两端接线。移植 `include-generated-columns` 时也应复用 export 层已有的模式解析和冲突校验，而不是在 CLI 层另造简化规则。

修改表过滤时需特别保护 pflag `StringSlice` 的“首次显式赋值替换默认值”语义及 tables-list 的大小写路径；修改输出模板时需保留 split 防覆盖不变量；新增 map 参数时需明确键规范化与覆盖规则。性能风险主要来自引入文件读取或昂贵解析到 CLI 热路径；兼容风险则集中在默认值、错误文案、短参数、隐藏状态和接受别名集合变化。

## 验证依据

- 源文件：`dumpling/cmd/dumpling/config_flags.rs`，完整检查了 628 行、54 个 `FLAG_*` 常量、两个公开函数和五个内部函数（含日期转换辅助函数）。
- crate 与入口：`dumpling/cmd/dumpling/Cargo.toml`、`lib.rs`、`main.rs`、`main.go`；确认 cmd crate 边界和 `run_with_factory` 的实际调用顺序。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter dumpling/cmd/dumpling` 确认目标及相邻入口/测试已入图；`node --file` 与 `query` 确认主要符号签名。精确 callers/callees 查询超时，调用边改由源码直接调用点复核。
- Rust 独立测试：`dumpling/cmd/dumpling/config_flags_test.rs` 验证压缩别名、重复 column-filter 优先级、inline/file/SQL 冲突和 session 参数键规范化；`parity_test.rs` 验证 pflag slice/短参数/duration/bool 契约、RFC3339 目录形状、默认值、tables-list 过滤和错误路径。
- Go 对照与测试：`dumpling/export/config.go`、`dumpling/export/config_test.go`，以及 Rust export 侧 `dumpling/export/config.rs`、`dumpling/export/config_test.rs`；它们支持上述共同语义与迁移差异结论。
- 本任务是只新增说明文档的静态分析，按计划不运行 Cargo。交付验证使用任务文件规定的 11 章节结构检查，并人工检查文档只描述有源码或测试依据的现状。
