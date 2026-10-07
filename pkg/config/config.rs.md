# `pkg/config/config.rs`

## 文件定位

`pkg/config/config.rs` 是 `astersql-config` crate 的核心配置实现。crate 根模块 `pkg/config/lib.rs` 以 `pub mod config; pub use config::*;` 将本文件的公开类型和函数提升为 `astersql_config::*` API；`pkg/config/Cargo.toml` 则声明该 crate 依赖 Serde/TOML、正则、URL、用户身份、原子初始化，以及资源组和配置子 crate。本文件不是仅用于反序列化的 DTO：它同时定义服务端配置模型、默认值、文件加载、语义校验、全局发布、配置导出和若干运行时查询入口。

在完整应用中，本文件位于“启动配置输入”和“运行时子系统消费”之间。`cmd/tidb-server/main.rs:1203` 调用 `load_postgres_port` 从配置文件投影独立 PostgreSQL 监听端口；服务运行后，session、DDL、executor、distsql、store、domain、server、planner 等模块通过 `get_global_config` 或更窄的查询函数读取已发布快照，例如 `pkg/session/runtime/planning.rs:434`、`pkg/ddl/job_worker.rs:573`、`pkg/executor/statement_ru_result.rs:103`、`pkg/store/store.rs:531` 和 `pkg/server/http_status.rs:1764`。

该文件已有 `// Copyright 2026 AsterSQL.`，并明确说明它由同目录 `pkg/config/config.go` 迁移。Rust 实现当前覆盖可被 Rust 主链使用的核心配置子集，并非 Go 文件全部启动编排逻辑的一比一替代；具体差异见“与 Go 版本的对应关系”。

## 核心职责

1. 定义配置协议及默认值：`Config` 聚合网络、部署模式、日志、安全、性能、实例、TiKV 客户端、悲观事务、RU v2、外部负载、keyspace、列存和事务摘要等分区；各子结构通过 `Default` 与 Serde 属性固定 TOML/JSON 名称和缺省语义。
2. 加载并解释配置文件：`Config::load` 读取 TOML，区分“键是否显式出现”，应用 token 上限及 starter/premium_reserved 模式默认值和限制，并拒绝 `extra` 捕获到的未知顶层配置。
3. 做运行前语义校验和少量规范化：`Config::valid` 检查枚举、数值范围、模式互斥、TLS/权限条件、URI、正则和跨字段约束；同时归一旧/新日志开关以及落盘加密算法大小写。
4. 发布进程级只读快照：`GLOBAL_CONFIG` 保存 `Arc<Config>`，`get_global_config` 提供低成本快照读取，`store_global_config` 整体替换配置并同步错误消息扩展缓存，`update_global` 采用拷贝—修改—发布方式更新。
5. 提供兼容和导出工具：`section_moved_to_instance`、`removed_config`、`contain_hidden_config` 描述旧配置迁移/移除信息；`get_json_config` 在导出前按路径剔除隐藏项和已移除项。
6. 提供窄接口：临时落盘目录编码、全局 keyspace、表锁参数、starter 模式的 `max_allowed_packet`、TiKV 客户端配置投影、部署模式解析及 PostgreSQL 端口投影。

## 主要符号

- 顶层协议：`Config` 是 TOML 根对象，`#[serde(default, rename_all = "kebab-case")]` 使缺失字段继承默认值，并由 `extra: toml::Table` 捕获未识别的顶层键。少数字段用显式标签保持 Go 公共契约，例如 `tmp-storage-path`、`use-autoscaler`、`ru-v2`、`transaction-summary`、`keyspace-activate` 和 `instance.tidb_slow_log_threshold`。
- 配置分区：`Log`/`FileLogConfig`、`Security`、`Status`、`Performance`、`Instance`、`TiKVClient`/`AsyncCommit`/`CoprCache`/`TiKVRUV2Config`、`PessimisticTxn`、`Experimental`、`StarterParams`、`Standby`、`IsolationRead`、`Cse`、`TrxSummary`、`RUV2Config`/`DDLWeights`、`HostedEmbedding`。`Config` 还组合相邻模块的 `ExternalWorkload`、`KeyspaceObservability` 和 `KeyspaceObservabilityValues`。
- 兼容值类型：`AtomicBool` 用 `SeqCst` 原子读写并手写 Clone/Serde，克隆时复制当前值而不共享原子存储；`NullableBool` 用 `UNSET`/`FALSE`/`TRUE` 区分未配置和显式 false；`DeployMode` 表示 `premium`、`premium_reserved`、`starter`；`TxnLocalLatches` 保留 Go mockstore 字段但通过 `#[serde(skip)]` 排除持久化协议。
- 载入与校验：`Config::load`、`Config::adjust_starter_config`、`Config::valid`、私有 `apply_security_env`、`valid_keyspace_name`、`valid_ru_weight`。`ConfigError` 将错误分成可读语义错误、带路径的 I/O 错误和带路径的 TOML 解码错误。
- 默认和派生值：`Config::default`/`new_config`、`Config::update_temp_storage_path`、`encode_def_temp_storage_dir`、`default_temp_storage_dir_name`、`get_tikv_config`、`valid_max_allowed_packet`/`get_max_allowed_packet`。
- 错误扩展：`ErrorMessageExtension::new`、`matches`、`prepare_error_message_extensions`；序列化配置只保存 pattern/suffix，编译后的 `Regex` 位于跳过序列化的 `regexp` 字段。
- 全局状态：`GLOBAL_CONFIG`、`PREPARED_EXTENSIONS`、`get_global_config`、`store_global_config`、`update_global`、`restore_func`、`get_error_message_extensions`，以及只读便捷函数 `table_lock_enabled`、`table_lock_delay_clean`、`get_global_keyspace_name`。
- 兼容和可见性：`section_moved_to_instance`、`removed_config`、`hide_config`、`contain_hidden_config`、`is_all_removed_config_items`、`get_json_config` 和私有 `remove_json_path`。
- 启动适配：`load_postgres_port` 只反序列化 `postgres-port: Option<u16>`，避免 Rust 入口必须接管整套 Go 风格配置加载器；`init_by_ld_flags` 更新 `CHECK_TABLE_BEFORE_DROP` 并关闭遥测。

## 执行流程

默认构造流程从 `Config::default` 开始：先填充所有标量和子结构默认值，再调用 `encode_def_temp_storage_dir(std::env::temp_dir(), host, status_host, port, status_port)` 生成实例特定的落盘目录。目录中包含当前 UID 和对 `host:port/status_host:status_port` 做 URL-safe Base64 后的片段，避免同机默认端口组合互相覆盖。`PessimisticTxn::default` 还通过 `astersql_config_kerneltype::IsNextGen()` 决定悲观自动提交默认值。

完整文件载入由 `Config::load` 完成：读取文本后先解码为 `toml::Table` 保存“用户显式配置了哪个键”的元信息，再解码为带默认值的 `Config`。随后它把 `token-limit=0` 回退到 1000，并将过大值裁到 `MAX_TOKEN_LIMIT`；仅允许 starter 显式配置错误扩展、hosted embedding、非空 bootstrap file 和 external workload；starter 未显式给出 `max-import-data-size` 时补 25 GiB，未显式给出 `standby.enable-zero-backend` 时补 true；仅 `premium_reserved` 可显式配置 `dxf-resource-limit`。最后若 `extra` 非空则以第一个未知键报错，全部成功才用 `*self = loaded` 整体提交，因此前置失败不会把半成品写回调用者。

载入后的语义阶段由调用方显式执行 `Config::valid`。它先校验 RU v2 模式与所有权重为有限非负数，再归一冲突的新旧日志布尔项；随后检查 root 权限、starter/premium_reserved 专属项、external workload/keyspace observability 子校验、正则编译、存储类型、standby 与 keyspace activate 互斥，以及索引/列数/审计日志/统计加载/事务大小等范围。末段校验 keyspace、metering URI、隔离读引擎、落盘加密算法、列存类型和日志级别。该方法不是纯谓词：它会清空被新版日志开关覆盖的废弃字段、规范化 external workload（由其 `Valid` 实现负责）并把加密算法改为小写。

starter 安全覆盖是独立流程：`adjust_starter_config(false)` 直接返回；为 true 时，`apply_security_env` 分别处理 CLUSTER 和 SQL 两套 CA/cert/key。证书和私钥必须成对；设置 CA 时证书与私钥必须齐全；非空环境变量覆盖当前配置，空环境变量保留文件值。

发布流程以 `store_global_config` 为中心：先用 `prepare_error_message_extensions(..., true)` 克隆并编译合法正则，忽略无效规则但保留合法规则，再分别取得写锁替换 `PREPARED_EXTENSIONS` 和 `GLOBAL_CONFIG`。读取方通过 `get_global_config` 克隆 `Arc`，不持有全局读锁开展业务。`update_global` 先克隆当前 `Config`，在局部可变副本上执行闭包，再发布整个副本；`restore_func` 捕获旧快照内容，适合测试结束时恢复。

导出流程由 `get_json_config` 将当前快照转换为 `serde_json::Value`，依次对 `removed_config` 和 `hide_config` 的点分路径调用 `remove_json_path`，再用制表符缩进序列化。删除路径不存在或中途不是对象时安全停止，不影响其他字段。

## 数据与状态

`Config` 是值语义的配置快照。普通字符串、数字、集合和子结构在快照克隆时深拷贝；`Instance` 中 `tidb_enable_ddl` 与 `enable_collect_execution_info`、`PessimisticTxn` 中 `pessimistic_auto_commit` 使用 `AtomicBool`，但其 `Clone` 读取旧原子值并创建新原子，因此不同 `Config` 快照不会共享可变原子单元。`extra` 只承载反序列化阶段的未知顶层键；成功的 `Config::load` 要求它为空。

`GLOBAL_CONFIG: Lazy<RwLock<Arc<Config>>>` 是规范全局快照。锁只保护 `Arc` 指针的获取或替换，业务读取在克隆 `Arc` 后脱锁运行。`PREPARED_EXTENSIONS` 以相同形式存储已编译错误正则，但 `get_error_message_extensions` 返回元素深拷贝，调用方不能通过修改结果污染缓存。两个锁是独立的，发布顺序为先扩展缓存、后配置；因此这不是把两者封装为单一原子事务，要求新增跨缓存不变量时谨慎处理可观察窗口。

`CHECK_TABLE_BEFORE_DROP` 是独立的 `Lazy<StdAtomicBool>`，由 `init_by_ld_flags` 以 `SeqCst` 写入。`AtomicBool` 本身也统一使用 `SeqCst`，优先保证清晰的跨线程可见顺序，而非采用更弱内存序优化。

若干字段刻意不参与序列化：编译后的 `ErrorMessageExtension.regexp`、运行时解析值 `keyspace_observability_values`、`TxnLocalLatches`、命令行专用 `StarterParams.enable_rg_fallback` 和 `Experimental.enable_new_charset` 的输出。`StarterParams.max_import_data_size` 通过 `starter_byte_size` 调用 `ByteSize_MarshalText`/`ByteSize_UnmarshalText`，以诸如 `1KiB` 的文本形式往返，而不是裸整数。

## 依赖与调用关系

上游入口和消费者可分为四类：

- 启动适配：`cmd/tidb-server/main.rs:1203` 调用 `load_postgres_port`；`cmd/tidb-server/main.rs:1794` 使用 `update_global` 把入口配置合并进规范快照。
- 运行时读取：`pkg/session/runtime/*` 读取规划、DDL、事务作用域、权限和表锁相关字段；`pkg/executor/statement_ru_result.rs` 读取 RU v2 权重和报告模式；`pkg/distsql/distsql.rs` 读取执行信息采集开关；`pkg/ddl/*`、`pkg/store/*`、`pkg/domain/*`、`pkg/infoschema/*`、`pkg/server/*` 分别读取 DDL、存储、实例通告、storage class 和状态输出配置。
- 运行时发布：`pkg/sessionctx/variable/sysvar_builtins.rs:1654-1658` 克隆当前配置并调用 `store_global_config` 应用系统变量变更；`br/pkg/gluetikv/glue.rs:240-244` 也以克隆后整体发布的方式临时调整全局配置。
- 窄接口桥接：`pkg/keyspace/keyspace.rs` 使用 `get_global_keyspace_name`；`pkg/session/runtime/dispatch.rs:2816` 通过 `get_json_config` 对外返回过滤后的配置；`pkg/config/lib.rs` 的 `tikvcfg::GetTxnScopeFromConfig` 从全局 labels 的 `zone` 推导事务 scope。

本文件的直接下游依赖包括：Serde/TOML 负责协议；`regex` 编译错误扩展；`base64` 和 `users::get_current_uid` 构造临时路径；`users::get_effective_uid` 检查 `skip-grant-table` 权限；`url` 验证 metering URI；`once_cell`、`Arc`、`RwLock` 和原子类型实现全局状态；`astersql-resourcegroup` 提供共享 `StmtWeights` 及默认权重；`astersql-config-kerneltype` 提供内核类型默认分支；`astersql-config-configtypes` 提供 Go 兼容字节大小文本协议。相邻模块提供 external workload 与 keyspace observability 的数据和校验逻辑。

RustCodeGraph 对 `config.rs::get_global_config` 的调用边直接列出 `update_global`、`restore_func`、表锁/keyspace/max-packet/JSON 便捷函数，以及 session runtime 消费者；对 `prepare_error_message_extensions` 的调用边为 `Config::valid` 和 `store_global_config`；对 `store_global_config` 的文件内调用边为 `update_global` 和 `restore_func`。由于常见函数名存在跨包同名，检索调用关系时必须按 `pkg/config/config.rs` 定义消歧，不能把 `pkg/domain/globalconfigsync` 或 TiKV driver 的同名函数当成本文件 API。

## 错误处理与边界

文件 I/O 和 TOML 错误通过 `ConfigError::Io`/`Toml` 保留源路径与底层错误；语义错误统一由 `ConfigError::Message` 返回。`Config::load` 负责语法、未知顶层键和“是否显式配置”的部署模式限制，但不自动调用 `valid`，调用方必须在发布前完成语义校验和需要的 starter 环境覆盖。`load_postgres_port` 也只验证文件可读、TOML 可解码以及端口能放入 `u16`，不执行完整配置校验。

关键边界包括：RU 权重必须有限且非负；store 仅接受 `tikv`/`unistore`/`mocktikv`；starter 专属项不能出现在其他模式；DXF 限制在 10..=100 且非默认值只允许 premium_reserved；starter 的 `max_allowed_packet` 必须在 1 KiB..=1 GiB 且按 1 KiB 对齐；索引长度、索引数、列数、统计加载、审计日志和事务大小均有显式边界；隔离读列表不能为空且只能含 tidb/tikv/tiflash；metering URI 必须是带 host 的 s3 或 azure URL；keyspace 最多 20 字节且字符限 ASCII 字母数字、下划线、连字符；日志级别限 debug/info/warn/warning/error/fatal。

错误扩展在严格校验路径 `Config::valid` 中遇到首个空 pattern 或非法正则即失败，并返回空准备结果；发布路径 `store_global_config` 使用 `ignore_invalid=true`，保留所有可编译规则并记录首个错误返回值，但当前调用点有意丢弃该错误。因此未经 `valid` 直接发布会静默跳过坏规则。规则按 pattern 长度降序，再按 pattern 和 suffix 字典序排序，确保更具体规则优先且同长度稳定。

全局锁中毒以 `expect` 触发 panic，而不是转成 `ConfigError`；这是内部不变量边界。`store_global_config` 自身不调用 `Config::valid`，`update_global` 也不验证闭包结果，因此所有发布调用方必须保证新快照已满足业务约束。`get_max_allowed_packet` 在非 starter 或全局值非法时安全回退 `DEF_MAX_ALLOWED_PACKET`。

## 并发与资源生命周期

正常读取路径不暴露可变引用：`get_global_config` 在读锁内只克隆 `Arc`，随后旧快照至少存活到所有持有者释放；新配置发布不会使进行中的请求看到对象内部被逐字段修改。`update_global` 也不会原地修改当前快照，而是在独立克隆上运行闭包后一次替换。这个 copy-on-publish 模型应当延续到新增配置更新入口。

错误正则的生命周期与配置发布绑定：配置文本中的 pattern 在 `prepare_error_message_extensions` 中被编译进克隆对象，发布到 `PREPARED_EXTENSIONS`；原始 `Config.error_msg_extension` 仍保存可序列化文本。消费者取得的是 `Vec` 副本，因此无额外锁生命周期泄露。需要注意两个全局写锁并非同时持有，读者理论上可在短暂窗口内看到新扩展与旧配置；当前消费者主要独立读取准备后的规则，源码未提供跨两者原子一致性保证。

环境变量与文件句柄都是短生命周期资源：`fs::read_to_string` 完成后释放文件，`adjust_starter_config` 每次调用即时读取六个环境变量，不缓存环境视图。临时目录函数只生成字符串，不创建目录，也不负责清理落盘文件。`restore_func` 捕获一份 `Arc` 指向的旧值，但恢复时会克隆其 `Config` 并重新编译扩展；测试用 `pkg/config/config_test.rs::GLOBAL_TEST_LOCK` 串行化会修改全局状态的用例，避免相互污染。

## 与 Go 版本的对应关系

主要语义直接对应 `pkg/config/config.go`：Rust `Config::default` 对应 Go `defaultConf`/`NewConfig`；`get_global_config`、`store_global_config`、`update_global`、`restore_func` 对应 `GetGlobalConfig`、`StoreGlobalConfig`、`UpdateGlobal`、`RestoreFunc`；`Config::load`/`valid`/`adjust_starter_config` 对应 `Load`/`Valid`/`AdjustStarterConfig`；临时目录、JSON 过滤、隐藏项判断、错误扩展准备和 max packet 也保留相同规则。`AtomicBool` 对应 Go `atomicutil.Bool`，`NullableBool` 对应 Go `nullableBool`；Rust 的 Serde 显式标签用于保持 Go TOML/JSON 公共键名。

Rust 独立测试把“保持 Go 语义”作为显式合同：`pkg/config/config_test.rs::test_go_default_config_parity` 检查 unistore、socket 和 cross join 默认值；`test_go_config_field_names_parity` 检查公开键名；`test_go_validation_parity` 覆盖权限、store、mocktikv DDL、DXF 和事务摘要边界；`go_merge_2_*` 覆盖新配置字段、RU 权重与序列化；`pkg/config/config_2_aster_unit_test.rs` 进一步检查原子布尔、max packet、临时路径、错误扩展和服务器连接默认值。

当前 Rust 迁移边界必须明确：Go `InitializeConfig` 会协调配置检查模式、命令行覆盖、旧项警告、`Valid`、starter 环境覆盖和最终发布；本文件没有等价总编排函数，Rust 调用方需自行组合步骤。Go `Load` 使用 TOML metadata 生成完整 undecoded 列表并诊断迁移到 `[instance]` 的冲突/废弃项，Rust `load` 目前只从 `extra` 报第一个未知顶层键，`section_moved_to_instance` 仅提供映射数据。Go `StoreGlobalConfig` 还在 `TikvConfigLock` 下同步 TiKV 客户端全局配置；Rust `store_global_config` 只替换本 crate 快照与错误扩展缓存，`get_tikv_config` 只是值投影。Go keyspace 名称验证委托 `naming.CheckKeyspaceName`，Rust 当前本地规则是最多 20 字节及限定 ASCII 字符；独立 Rust 测试中的 `a18446744073709551615` 预期还提示该部分迁移语义应持续核验，不能假设两端所有边界已完全等价。

## 扩展指南

新增持久化配置字段时，应同时修改 `Config` 或对应子结构、`Default`、必要的显式 Serde 键名、`Config::load` 的“显式出现”处理以及 `Config::valid` 的语义约束；若字段是 starter/premium_reserved 专属，必须同时覆盖文件加载和程序化构造后校验两条路径。新增字段还应对照 `pkg/config/config.go` 的字段、标签、默认值和边界，并扩展独立的 `pkg/config/config_test.rs`，不要把测试嵌入本生产文件。

新增运行时可更新项时，优先让消费者从一次 `get_global_config()` 返回的同一快照读取相关字段，发布方走 `update_global`/`store_global_config`，不要在旧快照上增加共享内部可变状态。若需要派生缓存，应像错误扩展一样定义构建和更新时机，但若派生值必须与配置强一致，应改成单一锁/单一快照对象，而不能照搬当前两个锁的非原子发布顺序。

新增错误类型或校验时，优先保留字段路径和非法值，使错误可行动；文件/TOML 错误继续用带 path/source 的变体。新增隐私或废弃项时，同步更新 `removed_config`/`hide_config`、`get_json_config` 过滤测试和 `contain_hidden_config` 测试，确认嵌套点分路径正确。新增自定义序列化字段需同时验证 TOML 与 JSON 往返，并明确运行时字段是否应 `skip`。

若扩展启动加载链，必须先决定该逻辑属于完整 `Config::load`/`valid`，还是像 `load_postgres_port` 一样属于入口适配投影；后者不应悄悄承担完整验证。需要同步检查 `cmd/tidb-server/main.rs` 入口、`pkg/config/lib.rs` 导出、`pkg/config/Cargo.toml` 依赖，以及 Go `InitializeConfig` 尚未迁移的职责。任何修改 Rust 生产代码时应按仓库规则同步独立测试、运行 `cargo fmt --all`，并在真正可用后保留/添加 AsterSQL 2026 版权行。

兼容风险主要来自公开 TOML/JSON 键名和默认值变化；正确性风险集中在“load 不自动 valid”“store 不自动 valid”和跨字段模式约束；性能风险集中在高频 `update_global` 的整份深克隆与错误正则重编译。读取只克隆 `Arc`，不应改成每次深拷贝。

## 验证依据

- 源码与 crate 边界：完整阅读 `pkg/config/config.rs`（1889 行）、`pkg/config/lib.rs` 和 `pkg/config/Cargo.toml`；确认 crate 通过根模块重导出本文件，依赖与本文描述一致。
- RustCodeGraph：`status` 显示本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/config` 确认目标、Go 对照和独立测试均已索引；`node --file pkg/config/config.rs` 分段核对全部源码。`query`/`node` 检查了 `Config`、`get_global_config`、`store_global_config`、`prepare_error_message_extensions`、`load_postgres_port`、`valid_max_allowed_packet`；调用边确认 `prepare_error_message_extensions <- Config::valid/store_global_config`、`store_global_config -> prepare_error_message_extensions/GLOBAL_CONFIG/PREPARED_EXTENSIONS`、`get_global_config` 的文件内包装函数与 session runtime 消费者。
- 直接调用证据：检索 Rust 生产文件确认 `cmd/tidb-server/main.rs:1203,1794`、`pkg/sessionctx/variable/sysvar_builtins.rs:1654-1658`、`br/pkg/gluetikv/glue.rs:240-244` 以及 session/DDL/executor/distsql/store/domain/server 等读取点。RustCodeGraph 对常见同名符号存在多定义，因此调用结论以目标文件定义和路径消歧。
- Go 对照：阅读 `pkg/config/config.go` 的 `Config`、默认/全局快照、`InitializeConfig`、`AdjustStarterConfig`、`Load`、`Valid`、`UpdateGlobal`、`RestoreFunc`、`GetJSONConfig` 和 `GetMaxAllowedPacket` 邻近实现；对照确认相同主语义与上述迁移边界。参考 Go 回归入口 `pkg/config/config_test.go`，包括 `TestLogConfig`、`TestErrorMessageExtensionConfig`、`TestConfig`、`TestDeployModeConfig`、`TestConflictInstanceConfig`、`TestDeprecatedConfig`、`TestGetJSONConfig` 等。
- Rust 独立测试：阅读 `pkg/config/config_test.rs` 与 `pkg/config/config_2_aster_unit_test.rs`，重点核对 TOML 载入、Serde 键名/默认值、日志三态布尔、部署模式、未知项、数值边界、TLS/root 权限、全局恢复、JSON 过滤、错误正则、RU、字节大小及 Go parity。测试由 `pkg/config/lib.rs` 的 `#[cfg(test)] #[path = ...]` 独立挂载，符合生产源与测试分离要求。
- 本任务是纯文档分析，按计划不运行 Cargo，也未修改 Rust、Go、Cargo 或 `plan.md`。交付前执行任务指定的 11 章节结构命令，并人工复核本文能回答文件为何存在、如何加载/校验/发布、由谁消费以及如何安全扩展。
