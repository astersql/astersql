# `pkg/lightning/config/global.rs`

## 文件定位

本文件属于 `astersql-lightning-config` crate（见 `pkg/lightning/config/Cargo.toml`），实现 Lightning 启动阶段的“全局配置”中间层：把进程级命令行参数和配置文件中的少量全局字段合并成 `GlobalConfig`。模块由 `pkg/lightning/config/lib.rs` 声明并公开再导出；完整任务配置随后可由 `Config::load_from_global`（`pkg/lightning/config/config.rs`）读取 `GlobalConfig` 中保存的原始 TOML，再覆盖 CLI/全局字段。

当前接线边界必须特别说明：仓库内排除测试与 `stubs.rs` 后，未发现 `load_global_config` 的 Rust 生产调用；实际 Rust 行为覆盖来自独立测试 `pkg/lightning/config/config_test.rs`。Go 生产入口 `lightning/cmd/tidb-lightning/main.go` 和 `lightning/cmd/tidb-lightning-ctl/main.go` 调用同包的 Go `LoadGlobalConfig`，而对应 Rust 命令目前使用各自 `stubs.rs` 中的配置加载器。因此本文件是可复用且已测试的 crate API，但不能据此声称 Rust Lightning 二进制已走入这条实现链。

## 核心职责

- 用 `new_global_config` 建立与 Go `NewGlobalConfig` 对齐的默认值，包括 TiDB 地址、检查点开关、默认表过滤器以及导入后的校验/统计级别。
- 用 `parse_flags` 解析 Lightning 全局 CLI 参数，支持 `-c`/`-config` 别名、`--key=value`、布尔开关、枚举白名单、重复 `-f` 过滤器和调用方提前登记的额外 flag。
- 用 `decode_global_toml` 只解码全局层关心的 TOML 字段，并把原始字节保存在 `GlobalConfig::config_file_content` 中，供 `Config::load_from_global` 继续解析任务级字段。
- 在 `load_global_config` 中落实覆盖顺序和跨字段约束：默认值先于 TOML，CLI 的非空/非零显式值最后覆盖；server mode 必须具备监听地址；日志配置在返回前规范化。
- 用 `must` 把库式错误转换为命令进程退出语义：帮助请求退出 0，其他错误打印后退出 2。

## 主要符号

- `LogConfig { level, file }` 与 `LogConfig::adjust`：日志级别为空时补成 `info`，并把兼容写法 `warning` 归一为 `warn`。
- `GlobalLightning`：进程日志、状态监听地址、服务模式、依赖检查开关及遗留 `pprof_port`。
- `GlobalTiDB`：TiDB SQL/status 端口、用户凭据、PD 地址及 TiDB 日志级别。
- `GlobalMydumper`：数据源目录、已废弃但仍兼容的 `no_schema`、表过滤规则和忽略列规则。
- `GlobalImporter`、`GlobalCheckpoint`、`GlobalPostRestore`：分别保存后端/本地排序目录、断点续传开关、导入后 checksum/analyze 级别。
- `GlobalConfig`：聚合上述各段、`Security` 以及原始 `config_file_content`；字段均为拥有所有权的值，因此每次加载结果互不共享可变状态。
- `new_global_config() -> GlobalConfig`：公开默认值构造器。关键默认值为 host `127.0.0.1`、user `root`、status port `10080`、checkpoint 开启、checksum `Required`、analyze `Optional`、`check_requirements` 开启。
- `load_global_config(args, extra_flags)`：公开主入口，返回 `Result<GlobalConfig, ConfigError>`。
- `FlagSet::register`：公开的扩展 flag 登记接口；这里只负责允许名称通过未知参数检查，不负责为扩展 flag 暴露解析值。
- `ParsedFlags`、`parse_flags`：内部 CLI 表示与解析器；普通键写入 `values`，`-f` 追加到 `filters`。
- `decode_global_toml` 及 `set_string`、`set_bool`、`set_i32`、`string_array`、`decode_ignore_columns`：内部 TOML 解码与类型检查辅助函数。
- `must` 与 `timestamp_log_file_name`：分别实现进程退出策略和缺省日志文件路径生成。

## 执行流程

1. `load_global_config` 创建 `FlagSet`，在解析前调用可选 `extra_flags` 回调；随后 `parse_flags` 逐项扫描 `args`。
2. `parse_flags` 接受单/双横线与内联等号形式，把 `c` 归一为 `config`，使重复的 `-c`/`-config` 通过 `HashMap::insert` 呈现“最后一次生效”。布尔 flag 未带值时视为 `true`；非布尔 flag 必须消耗下一参数；未知 flag、缺值和非法枚举立即返回 `ConfigError::Invalid`。
3. 若 `-V` 为真，入口打印 `AsterSQL Lightning` 并返回 `ConfigError::Help`。`-h`/`--help` 在解析阶段同样返回 `Help`。
4. 入口通过 `new_global_config` 建立默认配置。若设置 `config`，读取整个文件，用 `decode_global_toml` 合并全局段，并保存同一份原始字节到 `config_file_content`。
5. CLI 值在 TOML 之后应用。字符串仅在非空时覆盖，端口仅在非零时覆盖；false-default 开关只在真时置位，true-default 开关只在显式假时关闭；非空的 `-f` 列表整体替换默认 filter。
6. 未显式指定日志文件时，`timestamp_log_file_name` 在系统临时目录生成带 Unix 纳秒时间戳的文件名。若 `status_addr` 仍为空而 TOML 给出了非零 `pprof_port`，则生成 `:<port>` 兼容地址。
7. server mode 且最终 `status_addr` 为空时返回配置错误；否则调用 `LogConfig::adjust`，返回合并后的 `GlobalConfig`。
8. 调用方可将结果传给 `Config::load_from_global`：它先解析 `config_file_content` 中的完整任务配置，再用全局结构中的 TiDB、mydumper、importer、checkpoint、post-restore、security 等字段覆盖对应项。

## 数据与状态

覆盖优先级是不变量：`new_global_config` 默认值 < TOML 全局字段 < CLI 的有效非空/非零值。该规则避免 CLI flag 的占位默认值意外擦除配置文件值；对应实现集中在 `load_global_config` 的逐字段条件赋值。

`config_file_content` 保存文件原始字节而非重新编码的 TOML。这样 `GlobalConfig` 可以只理解启动期字段，同时让 `Config::load_from_global` 解析其余任务字段，并由 `toml_codec.rs` 联合判断未知键。未提供配置文件时该向量为空，完整配置解析等价于加载空 TOML。

`ParsedFlags::values` 对单值 flag 使用最后写入覆盖；`filters` 则保留每次 `-f` 的顺序。`new_global_config` 每次重新构造默认 filter 和所有容器；`config_test.rs::test_create_several_configs_with_different_filters` 验证不同配置实例不会互相污染。

安全字段中的密码、证书路径和私钥路径只是字符串数据，本文件不打开证书或建立连接。`config_file_content` 可能含敏感内容，调用方不应把整个结构直接输出到日志。

## 依赖与调用关系

crate 内部依赖来自 `crate::{ConfigError, IgnoreColumns, PostOpLevel, Security, get_default_filter, parse_bool, parse_i32}`：错误与标量解析由 `config.rs` 提供，TOML 由 Cargo 依赖 `toml = "0.8"` 解析。标准库负责集合、文件读取、临时目录、时间戳和进程退出。

已验证的下游边包括：`load_global_config -> parse_flags`、`load_global_config -> new_global_config`、`load_global_config -> decode_global_toml`、`load_global_config -> LogConfig::adjust`，以及 `decode_global_toml -> set_* / string_array / decode_ignore_columns / PostOpLevel::from_string_value`。完整配置消费边为 `Config::load_from_global(GlobalConfig)`（`pkg/lightning/config/config.rs`）。

RustCodeGraph 将 `global.rs` 标为被 12 个文件使用，但其宽泛文件级结果混入同名 `GlobalConfig` 等符号；精确 `callers`/`callees` 命令未输出边。因此入口接线以 `rg` 的精确符号引用复核：生产 Rust 中没有本文件 `load_global_config` 的直接调用，测试调用集中于 `pkg/lightning/config/config_test.rs`。`Cargo.toml` 还表明该 crate 被 executor importer、import SDK、global sort、ingest control、DXF import-into、DDL 等 crate 依赖，但这只能证明 crate 依赖，不能证明它们调用本文件入口。

## 错误处理与边界

`ConfigError::Help` 同时承载帮助和版本输出的非失败控制流；只有命令入口选择调用 `must` 时才会转为进程退出。库调用者应直接匹配该变体，避免在可复用代码中触发退出。

CLI 边界包括：不带横线的位置参数报 `unexpected argument`；未知 flag 报 `flag provided but not defined`；非布尔 flag 缺少后继值时报 `flag needs an argument`；整型由 `parse_i32` 校验；日志级别、backend、checksum、analyze 由白名单拒绝非法值。布尔参数当前未消费后继的 `false`，必须使用 `--flag=false` 才能显式关闭 true-default 开关，这是 `parse_flags` 的实际语义。

配置文件读取失败被包装为包含路径的 `ConfigError::Invalid`；UTF-8、TOML 语法、字段类型、整数超出 `i32`、字符串数组成员类型、ignore-columns 表结构和 post-restore 值错误转换为 `ConfigError::Parse`，再由入口包装为包含文件路径的 `Invalid`。未知 TOML 字段不会由 `decode_global_toml` 自身拒绝；其联合校验属于后续完整配置加载。

跨字段边界是 `server_mode => status_addr 非空`。遗留 `pprof_port` 会先尝试补出状态地址，所以只有两者都没有形成地址时才失败。`SystemTime` 早于 Unix epoch 时日志文件时间戳通过 `unwrap_or_default` 回退为 0，不会 panic；文件名的纳秒值只用于降低冲突概率，不提供严格唯一性保证。

## 并发与资源生命周期

本文件没有全局静态可变状态、锁、通道、异步任务或线程。加载过程完全在调用栈内操作拥有所有权的 `GlobalConfig`，因此并行调用之间除外部文件系统与进程标准输出外没有共享状态。

资源生命周期很短：配置文件由 `std::fs::read` 一次性读入 `Vec<u8>` 并随返回的配置继续存活；TOML 解析树只在 `decode_global_toml` 调用期间存在；临时日志路径仅被计算，本文件不创建日志文件。`must` 是唯一改变进程生命周期的函数，一旦匹配帮助或错误分支便不返回。

若多线程同时省略日志路径，纳秒时间戳通常产生不同名字，但实现没有原子预留或冲突检测；真正创建/轮转日志文件的组件仍需处理竞争。配置文件在一次 `read` 后解析同一字节快照，不会在解析中再次访问路径。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/lightning/config/global.go`，Rust 的结构分段、默认值、CLI 名称、TOML 段、覆盖顺序、`pprof-port` 兼容、server-mode 校验、日志调整和 `Must` 退出码均按 Go 流程移植。独立测试 `config_test.rs::test_load_config` 与 Go `config_test.go::TestLoadConfig` 覆盖非法端口、版本请求、文件不存在、server mode、主要 CLI 覆盖、默认日志路径及 `-` 日志目标；Rust 另有 `test_load_global_config_mydumper_collections` 验证 filter/ignore-columns TOML 集合。

已确认的差异与限制如下：

- Go `-V` 输出 `build.Info()`，Rust 当前只输出固定字符串 `AsterSQL Lightning`。
- Go 使用标准 `flag.FlagSet`，`extraFlags` 可注册并由回调持有具体 flag 值；Rust `FlagSet::register` 只放行额外名称，值留在私有 `ParsedFlags` 中，当前没有公开取值接口。
- Go 默认日志名是格式化的本地时间，Rust 使用 Unix 纳秒；两者都放在系统临时目录，但文件名不要求逐字一致。
- Go `toml.Unmarshal` 通过结构标签整体解码，Rust 手工枚举全局字段；Rust 不会自动接纳后来新增但尚未加入 `decode_global_toml` 的字段。
- Go 对 `PostOpLevel::FromStringValue` 的返回值显式忽略；Rust 使用 `?` 传播错误。现有 CLI 白名单使正常 CLI 路径等价，但 TOML 错误在 Rust 中明确失败。
- Rust `IgnoreColumns` 保存值对象，Go 使用指针切片；对配置语义无差异，但空值/共享身份不存在一一映射。
- Go Lightning 命令已直接调用 Go 实现；Rust 命令仍走各自 stub，当前不存在已验证的生产调用链。

## 扩展指南

新增全局字段时，应同步修改承载该字段的 `Global*` 结构、`new_global_config` 默认值、`decode_global_toml` 的段/键映射、必要的 CLI 注册与 `load_global_config` 覆盖逻辑，并检查 `Config::load_from_global` 和 `toml_codec.rs` 的全局专属键集合。若字段来自 Go 移植，还应同步核对 `global.go` 的标签、默认值与覆盖条件，避免 Rust 手工解码发生漂移。

新增 CLI flag 时，至少更新 `parse_flags` 的 `known` 集合；布尔项还需加入 `boolean`，枚举项需加入 `choices`。如果嵌入命令需要读取自定义 flag 值，当前 `FlagSet` API 不足，应先设计显式的结果访问接口，而不是仅调用 `register` 后假设值可见。

回归测试应放在独立文件 `pkg/lightning/config/config_test.rs`，不要嵌入生产源文件。建议分别覆盖默认值、TOML 类型错误、TOML/CLI 覆盖优先级、重复 flag、`--bool=false`、额外 flag、server-mode 组合与敏感字段；对应 Go 行为应在 `pkg/lightning/config/config_test.go` 核对。若未来把该入口接入 Rust 命令，还需在命令自身的独立 parity 测试中证明实际调用的是 crate 实现，而非保留的 stub。

兼容风险主要来自已有 flag/TOML 键的默认值和覆盖条件；安全风险来自错误或日志意外暴露密码及原始 TOML；性能风险较低，但配置文件当前整文件读入且手工构建 TOML 值树，新增大集合时需留意内存放大。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/lightning/config` 确认目标、Go 对照及独立测试均已索引；`node --file pkg/lightning/config/global.rs` 读取 599 行源码并确认 29 个符号；`query load_global_config`、`query decode_global_toml`、`query parse_flags` 确认目标符号位置。精确 `callers`/`callees` 查询未返回可见边，故没有把宽泛同名结果当作事实。
- 源码与模块边界：`pkg/lightning/config/global.rs`、`pkg/lightning/config/lib.rs`、`pkg/lightning/config/config.rs`、`pkg/lightning/config/toml_codec.rs`、`pkg/lightning/config/Cargo.toml`。
- Go 对照：`pkg/lightning/config/global.go`、`pkg/lightning/config/config.go`、`lightning/cmd/tidb-lightning/main.go`、`lightning/cmd/tidb-lightning-ctl/main.go`。
- 独立测试：`pkg/lightning/config/config_test.rs` 中 `test_load_config`、`test_load_from_invalid_config`、`test_create_several_configs_with_different_filters`、`test_load_global_config_mydumper_collections`；对应 Go 测试位于 `pkg/lightning/config/config_test.go` 的 `TestLoadConfig` 与 `TestCreateSeveralConfigsWithDifferentFilters`。
- 接线复核：对非测试、非 stub Rust 文件精确搜索 `load_global_config(` 与 `new_global_config(`，仅命中本文件定义/内部调用；因此文档将“当前无生产调用”列为已验证限制。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构验证要求目标文档存在且恰有 11 个固定二级标题。
