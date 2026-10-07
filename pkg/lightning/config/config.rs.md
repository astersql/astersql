# `pkg/lightning/config/config.rs`

## 文件定位

本文件是 Rust `astersql-lightning-config` crate 的任务级配置核心，源文件为 [`config.rs`](./config.rs)。它定义一棵完整的 Lightning 导入配置树、默认值、文本枚举、规范化和交叉字段校验。crate 入口 [`lib.rs`](./lib.rs) 以 `pub use config::*` 再导出这些 API；[`Cargo.toml`](./Cargo.toml) 的 `package.metadata.porting.go-package = "pkg/lightning/config"` 明确其 Go 对照目录。

它位于“读取配置”和“启动导入逻辑”之间：`new_config` 先构造默认配置，`Config::load_from_toml` 或 `Config::load_from_global` 覆盖用户输入，`Config::adjust` / `adjust_with_settings` 再按依赖顺序补默认值并拒绝非法组合。TOML 字段映射的主体不在本文件，而在 [`toml_codec.rs`](./toml_codec.rs) 的 `load_config_from_toml`；本文件负责字段含义及调整后的运行时不变量。

RustCodeGraph 的文件节点将本文件识别为 1766 行、134 个符号，并报告被 11 个 Rust 文件使用，示例包括 `pkg/importsdk/file_scanner.rs`、`br/pkg/stream/crr/internal/checkpoint/storage.rs` 和若干测试。当前仓库还在根 `Cargo.toml` 通过 `facade_lightning_config` 引入本 crate，并由 `pkg/lib.rs` 再导出；多个导入、DDL、ingestor crate 的 Cargo manifest 也声明了该依赖。这里描述的是配置 crate 的真实边界，不等同于断言所有声明依赖都已形成运行时调用。

## 核心职责

- 用 `Config` 聚合 `[lightning]`、`[tidb]`、`[checkpoint]`、`[mydumper]`、`[tikv-importer]`、`[post-restore]`、`[cron]`、`[security]`、`[conflict]` 和路由规则。
- 用 `new_config` 提供独立的默认配置实例，包括 CPU 相关并发、CSV 方言、过滤规则、Region 参数、cron 周期及冲突阈值哨兵值。
- 用 `Config::adjust_with_settings` 以固定顺序完成后端归一化、默认值填充、路径处理、TLS/PD 发现、checkpoint 派生和冲突策略合并；后续步骤依赖前序步骤已经完成。
- 提供配置文本协议：`PostOpLevel`、`CheckpointKeepStrategy`、`DuplicateResolutionAlgorithm`、`CompressionType`、`Duration` 和 `Charset` 的解析或显示。
- 校验跨字段约束，例如 local 后端必须有 `sorted-kv-dir`、CSV 分隔符不能相互形成前缀、local 不能使用 `ignore` 冲突策略、tidb 后端不能启用导入前冲突预检。
- 承担少量边界 I/O：`fetch_tidb_settings` 同步读取 TiDB `/settings`，`MydumperRuntime::adjust_file_path` 和 `TikvImporter::adjust` 检查或规范化本地路径。

本文件不执行数据导入，也不拥有 checkpoint、PD 或存储客户端；它只把配置变成下游可消费的、经过约束检查的状态。

## 主要符号

### 顶层入口与错误

- `ConfigError::{Invalid, Parse, Io, Help}` 区分非法配置、文本解析、I/O 和 CLI help；`Display` 为 `Invalid` 添加 `[Lightning:Config:ErrInvalidConfig]` 前缀，`From<std::io::Error>` 保留底层 I/O 错误。
- `Config` 是任务配置根；`string` 只输出 task id、TiDB 地址、数据源和 backend 的精简 JSON，`redact` 临时替换 `source_dir` 后恢复原值，`load_from_global` / `load_from_toml` 负责加载，`adjust` / `adjust_with_settings` 负责最终调整。
- `new_config() -> Config` 每次新建拥有独立 `Vec`、`HashMap` 和原子字段的默认配置，避免多个任务共享可变默认过滤器。
- `SettingsProvider`、`HttpSettings`、`NoSettings` 和 `TiDbSettings` 把 TiDB `/settings` 发现抽象成可替换边界；测试用 provider 可在不联网时验证调整逻辑。

### 配置段

- `DBStore` 保存 SQL 连接、PD 地址、SQL mode、TLS、并发和会话变量；`DBStore::adjust` 解析 SQL mode，合并任务/全局安全配置，并在 local 后端缺少内部连接信息时调用 `SettingsProvider`。
- `Security` / `TlsConfig` 保存证书路径或字节、明文回退和 TLS 运行时选项；`Security::build_tls_config` 要求证书与私钥成对，并设置 TLS 1.2、HTTP/2/1.1 协议列表。
- `Lightning` 保存表、索引、Region 和 I/O 并发；`Lightning::adjust` 依据 `tidb`、`local`、`import-into` 后端派生默认值，并把 local 的 Region 并发限制到 `cpu_count()`。
- `MydumperRuntime`、`CSVConfig`、`FileRouteRule`、`IgnoreColumns` 描述输入数据、CSV 方言、文件路由和忽略列；`get_ignore_columns` 按精确库表或简单 `db.table` 通配规则选取首个匹配项。
- `TikvImporter` 描述逻辑/物理后端、Region 分裂、排序目录、内存缓存、冲突兼容字段和逻辑批大小；`adjust` 是 backend 级合法性检查的第一关。
- `Checkpoint` / `MySqlConnectParam` 派生 checkpoint schema、驱动、文件路径或 MySQL 连接参数；`format_dsn` 对 SQL mode 做 percent encoding。
- `PostRestore` 和 `PostOpLevel` 控制 checksum、analyze、compact；tidb 后端在 `PostRestore::adjust` 中关闭物理导入相关后处理。
- `Conflict` 和 `DuplicateResolutionAlgorithm` 统一新旧冲突字段并推导 `threshold` / `max_record_rows`。
- `Cron` / `Duration` 用有符号纳秒保存 Go 风格时长，支持复合单位的解析和 Go 风格字符串输出。

### 内部辅助

`adjust_routes`、`ensure_insecure_tls`、`parse_sql_mode`、`validate_pd_addrs`、`fetch_tidb_settings`、`parse_go_duration`、`table_filter_matches`、`redact_url`、`json_escape` 和 `cpu_count` 都是内部实现。`parse_toml_document`、`unquote`、`parse_bool`、`parse_i32` 以 `pub(crate)` 暴露给同 crate 的轻量解析辅助；当前完整 TOML 加载实际由 `toml_codec::load_config_from_toml` 完成。

## 执行流程

1. 调用者以 `new_config` 建立基线。默认 backend 和 `sorted_kv_dir` 故意留空，所以默认实例尚不能直接作为完整导入任务运行。
2. `Config::load_from_toml` 委托 `toml_codec::load_config_from_toml` 将 TOML 段写入配置，并检查未知字段；`load_from_global` 先加载全局配置文件内容，再用 CLI/全局对象中的关键字段覆盖结果。
3. `Config::adjust` 创建 `HttpSettings { host, status_port }`，随后进入可测试的 `adjust_with_settings`。若 local 后端已显式提供 TiDB port 与 PD 地址，就不会访问 `/settings`。
4. `TikvImporter::adjust` 先把 backend 和 PD scheduler scope 转小写，迁移 `incremental_import`，再按后端检查逻辑批大小或 local 的 Region 参数、缓存默认值、排序目录和互斥开关。
5. `Lightning::adjust` 依据已经归一化的 backend 补并发与元数据 schema；`MydumperRuntime::adjust` 校验 CSV、strict-format、路由路径、字符集和批次比例，并规范化数据源 URL；`PostRestore::adjust` 再依据 backend 关闭不适用后处理。
6. `DBStore::adjust` 解析 SQL mode 和 TLS 模式。local 后端缺 port 或 PD 地址时调用 provider；provider 返回的 PD path 由 `validate_pd_addrs` 检查 host 非空、port 非空且不为 `0`。
7. `Checkpoint::adjust` 使用已调整的 TiDB 信息派生 MySQL 参数，或生成 `/tmp/<schema>.pb`；已有 DSN 会剔除历史 `allowAllFiles=true` 片段。
8. `adjust_routes` 把相对 route source 拼到数据源目录并检查 schema/target schema；最后 `Conflict::adjust` 合并 `conflict.strategy`、`on_duplicate`、已废弃 `duplicate_resolution`，检查后端兼容性并推导记录上限。
9. 任一步返回 `ConfigError` 都会短路，后续段不会继续调整；因此不能把失败后的 `Config` 当作满足所有不变量的配置。

## 数据与状态

配置绝大部分是任务私有的普通值。关键状态变化如下：

- `new_config` 以 `-1` 表示尚待策略推导的 conflict 阈值，以 `0` 表示待补齐的若干并发/缓存，以空字符串表示必须由用户、全局配置或调整阶段提供的字段。
- `MaxError` 用四个 `AtomicI64` 保存运行时错误配额；`set_legacy_value` / `set_table` 以 `Ordering::Relaxed` 写入，因为这里只要求各计数本身的原子性，不建立跨字段同步顺序。`DBStore::io_total_bytes` 则是可选 `AtomicU64` 计数器。
- `load_from_global` 和 `load_from_toml` 都是覆盖式修改，不会自动调用 `adjust`。调用者必须在最终输入确定后显式调整。
- `Config::redact(&mut self)` 会暂时取走 `mydumper.source_dir`、写入脱敏 URL、生成摘要后恢复。正常返回时原值不变；该实现没有跨线程共享保护，调用期间需要独占 `&mut Config`。
- `MydumperRuntime::adjust` 会把忽略列名转为小写，不考虑 `case_sensitive`；`get_ignore_columns` 是否折叠输入库表名则由参数决定。
- `get_default_filter` 每次返回新 `Vec<String>`，默认排除系统 schema；`test_create_several_configs_with_different_filters` 验证不同配置实例的过滤器互不污染。

## 依赖与调用关系

上游入口关系为：`lib.rs` 再导出本文件符号；`toml_codec.rs` 接收 `&mut Config` 并写入本文件定义的各段；全局配置代码可通过 `Config::load_from_global` 将 `GlobalConfig` 收敛为任务配置。RustCodeGraph 文件节点还报告 `pkg/importsdk/file_scanner.rs`、checkpoint storage 和若干恢复测试引用本文件；这些使用方说明配置类型也被 Lightning 之外的导入/恢复路径复用。

关键内部调用边为：

```text
new_config -> get_default_filter + cpu_count
Config::load_from_toml -> toml_codec::load_config_from_toml
Config::adjust -> HttpSettings::settings -> fetch_tidb_settings
Config::adjust -> Config::adjust_with_settings
Config::adjust_with_settings
  -> TikvImporter::adjust
  -> Lightning::adjust
  -> MydumperRuntime::adjust -> CSVConfig::adjust + parse_charset + adjust_file_path
  -> PostRestore::adjust
  -> DBStore::adjust -> parse_sql_mode + Security::build_tls_config/ensure_insecure_tls
                     -> SettingsProvider::settings -> validate_pd_addrs
  -> Checkpoint::adjust
  -> adjust_routes
  -> Conflict::adjust
```

外部库使用可由 `Cargo.toml` 与源码交叉确认：`url` 处理数据源 URL 和敏感查询参数，`serde_json` 解析 `/settings` body，`toml` 由相邻 `toml_codec.rs` 使用，`astersql-util-cpu` 提供可用 CPU 数；`std::net::TcpStream` 和 `std::fs` 提供同步网络/文件系统边界。`serde` 是 crate 的配置编解码依赖，但本文件自身没有 derive `Serialize`/`Deserialize`。

## 错误处理与边界

- `Invalid` 表示语义非法，`Parse` 表示枚举、字符集、时长或 TOML 文本无法解析，`Io` 包装标准 I/O；绝大多数调整路径使用 `?` 原样向 `Config::adjust_with_settings` 传播。
- `fetch_tidb_settings` 使用最小 HTTP/1.0 请求，仅接受状态行包含 `200`、存在 header/body 分隔符且 body 是 JSON；port 缺失时得到 `0`，path 缺失时得到空串，再由后续校验拒绝。读取/写入超时设置失败被有意忽略，但连接和传输错误会转换成带手工填写提示的 `Invalid`。
- `validate_pd_addrs` 对齐 Go 当前行为，只检查每项 host、port 是否存在以及 port 不为 `0`；它不验证端口为数字或落在 `u16` 范围，也没有拒绝额外冒号后的部分，真正连接错误留给下游 PD 客户端。
- `adjust_file_path` 接受 `file/local/s3/oss/noop/gcs/gs/azure/azblob`；裸本地路径必须存在并可 canonicalize，未知 URL scheme 被拒绝。绝对文件路由必须位于 `source_dir` 下，包含父目录逃逸的相对结果被拒绝。
- CSV separator 不能为空；separator 与 delimiter 不能互为前缀；escape 必须至多一个 Unicode scalar，且不能与 separator、delimiter、terminator 相同。
- `parse_go_duration` 是 Go `time.ParseDuration` 的子集并用 `f64` 累计后转纳秒；扩展单位或精度敏感行为时必须与 Go 极值、舍入和复合格式测试对照，不能假定完全等价。
- `Config::string` 是手工拼出的精简摘要，不是完整配置序列化；它只对反斜杠和双引号做最小 JSON 字符串转义。checkpoint DSN/password 不在摘要中。

## 并发与资源生命周期

本文件不启动线程、异步任务或通道。`Config` 的调整方法都要求 `&mut self`，因此调整期间由 Rust 借用规则保证单写；普通字段完成调整后可由上层按自身同步策略只读共享。

并发相关状态限于 `MaxError` 的 `AtomicI64` 和 `DBStore::io_total_bytes` 的 `AtomicU64`。这里仅初始化或重设错误额度，不包含运行时扣减算法；使用 `Relaxed` 意味着调用方不能借这些原子读写推导其他字段的可见性顺序。

`fetch_tidb_settings` 的 `TcpStream` 为函数局部资源，设置 5 秒读写超时，返回或报错时由 RAII 关闭。路径检查只持有短生命周期 `Metadata`/`Path` 借用，不保持文件句柄。TLS 配置保存在 `Security` 中，没有在本文件建立连接。`redact` 的临时改写不是 panic guard：当前 `string` 不返回错误也不 panic 于正常格式化路径，但若未来在两次赋值之间加入可能 panic 的逻辑，应改成不会遗失原值的 guard 设计。

## 与 Go 版本的对应关系

主要依据是同目录 [`config.go`](./config.go) 和 [`config_test.go`](./config_test.go)。Rust 保留了 Go 的配置段、backend 常量、默认值和 `Adjust` 次序，尤其是 importer → app → mydumper → post-restore → TiDB → checkpoint → routes → conflict；Rust 测试也逐项复刻了 PD 发现、TLS、CSV、冲突、时长、checkpoint 和后端默认值场景。

已确认的实现差异与迁移边界：

- Go `Config.String` 用完整 JSON marshal；Rust `Config::string` 只产生关键字段摘要，不能作为完整 round-trip 格式。
- Go `Adjust(ctx)` 通过 `common.TLS.GetJSON` 使用现有 TLS/上下文访问 `/settings`；Rust `Config::adjust` 使用裸 `TcpStream` 的 HTTP/1.0 请求，不消费 `Security`、不支持 HTTPS、取消上下文或 chunked body。这是当前实现事实。
- Go routes 基于 `router.TableRule.Valid` 且在大小写不敏感时 `ToLower`；Rust `TableRouteRule` 是本地简化结构，`adjust_routes` 只处理 source 路径并要求源/目标 schema 非空。
- Go SQL mode 使用 TiDB parser 的 `mysql.GetSQLMode`；Rust `parse_sql_mode` 只规范化逗号列表并校验字符集合，不能证明所有 Go SQL mode 语义均已覆盖。
- Go TOML 依赖 BurntSushi decoder 的 metadata；Rust 使用 `toml_codec.rs` 和 `toml` crate 显式映射、追踪未知键。扩展字段必须同步 codec，而不是只给结构体加字段。
- Go `Security::BuildTLSConfig` 构造真实 `tls.Config` 并验证证书材料；Rust `TlsConfig` 是运行时选项数据结构，当前只检查 cert/key 成对，没有读取证书文件或解析证书。
- Go `time.Duration` 解析/格式化是标准库完整实现；Rust 是本文件自实现的兼容子集。Go checkpoint DSN 使用驱动解析再格式化；Rust 以 `&` 拆分移除 `allowAllFiles=true`，保留其余片段顺序。
- Rust 使用 snake_case 字段和 `Result<_, ConfigError>`；Go 使用导出 CamelCase 字段和 PingCAP error 包。两者的可观察错误文字大体对齐，但错误类型和堆栈能力不同。

因此，本文件是较完整的任务配置移植，但不应把“字段齐全”误读成所有 Go 辅助库行为均已等价；上述差异是后续扩展时需要优先保护或收敛的兼容面。

## 扩展指南

- 新增 TOML 字段时，同时修改对应配置结构、`new_config` 默认值（若需要）、`toml_codec.rs` 的 `apply_*`/编码逻辑，以及独立测试 [`config_test.rs`](./config_test.rs) 或 [`toml_codec_test.rs`](./toml_codec_test.rs)。只增加字段不会自动被 TOML 加载。
- 新增 backend 时至少同步 `TikvImporter::adjust`、`Lightning::adjust`、`PostRestore::adjust`、`DBStore::adjust` 和 `Conflict::adjust` 的分支，并与 Go `config.go` 的 backend 行为核对；未知 backend 当前必须报错。
- 改动调整顺序前先列出依赖。例如 checkpoint 依赖已调整的 TiDB，conflict 依赖已归一化的 importer；任意换序都可能改变派生值或错误优先级。
- 扩展 CSV、字符集、时长或枚举文本时，保留大小写、旧别名和错误信息兼容性，并将测试写在独立 `config_test.rs`，不要把测试嵌入生产文件。
- 增强 `/settings` 获取时，优先保持 `SettingsProvider` 注入边界；若加入 HTTPS、重定向、chunked encoding 或取消机制，应补真实 provider 测试，并明确资源超时和错误映射。
- 改动路径或 URL 逻辑时同时测试本地相对/绝对路径、对象存储 scheme、Windows volume path、父目录逃逸和凭据脱敏；不要在日志摘要中暴露 DSN、password、access key 或 token。
- 改动并发额度时区分“配置默认值”和“运行时原子扣减”。如果需要跨字段一致性，现有 `Relaxed` 原子不足，应在实际消费者中设计明确同步协议。
- 对照 Go 移植时不能删减看似多余的兼容分支；如果 Rust 选择不同实现，应在独立测试中固定可观察行为，并在本文的 Go 差异中更新说明。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/lightning/config/config.rs` 确认目标被索引；`node --file ...` 分段读取了完整 1766 行并报告 134 个符号及 11 个使用文件。`query` 准确定位了本文件的 `new_config`、`get_ignore_columns`、`parse_charset` 和各个 `adjust`。`callers/callees` 命令连续超时且未返回边列表，因此调用关系又以源文件和精确文本搜索复核，没有把缺失的图结果推测成事实。
- 已读生产文件：`pkg/lightning/config/config.rs`、`pkg/lightning/config/lib.rs`、`pkg/lightning/config/toml_codec.rs`、`pkg/lightning/config/Cargo.toml`、根 `Cargo.toml` 的 facade 声明及 `pkg/lib.rs` 的再导出位置。
- Go 对照：`pkg/lightning/config/config.go` 的 `DBStore.adjust`、`Config`、`CSVConfig.adjust`、`MydumperRuntime.adjust`、`TikvImporter.adjust`、`Checkpoint.adjust`、`Security.BuildTLSConfig`、`Duration`、`ParseCharset`、`Conflict.adjust`、`NewConfig`、`LoadFromGlobal`、`LoadFromTOML`、`Config.Adjust`；并检索了同目录 `config_test.go`。
- Rust 独立测试：[`config_test.rs`](./config_test.rs) 的 PD/port 发现与免联网分支、strict-format、backend、路径、TLS、CSV、Duration、默认并发、TOML、checkpoint、字符集、conflict、block size 和 URL 脱敏测试；[`toml_codec_test.rs`](./toml_codec_test.rs) 验证 TOML 委托入口。
- 人工复核结论：本文件存在是为了把 Lightning 的外部配置转换成后端可执行的不变量；安全扩展点集中在配置结构、`new_config`、TOML codec、固定 adjust 链和独立测试，且当前网络/TLS、route、SQL mode、duration 与 Go 仍有明确实现差异。
