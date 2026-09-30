# 任务 43: pkg/domain 第 1 组 Go 差异移植

批次：【批次 17】依赖：阶段 1 全部批次

状态：进行中（真实 TiKV/PD/etcd 生产 DDL 链已通过；Extract HTTP 生产入口已接通；TTL 通用 timer runtime 尚待接入）

本轮续做（2026-09-30）：`CanonicalServerDomain` 现在构造并持有 `CanonicalExtractRuntime`，通过真实 Domain、语句摘要、SQL 会话与 ExtStorage 生成和流式读取 Extract ZIP；后台任务沿用 Go 的空文件名返回语义。`go_merge_43_canonical_server_domain_serves_extract_archive` 使用非空真实语句摘要、真实 SQL 表和统计导出，解码 ZIP 并检查元数据、schema、stats、SQL、variables 条目，通过。非空回归先暴露 SQL runtime 不支持 `tidb_decode_binary_plan`，改为直接调用仓库已有 `plancodec::DecodeBinaryPlan`，并按 Go builtin 在无效计划载荷时返回空文本。Extract 元数据键和值与 Go `dumpExtractMeta` 对齐为 `SkipStats = "false"` 和 `taskType = "Plan"`；`config.toml` 已改为序列化完整全局配置并由回归解析。仍需核对统计 JSON 和其它文件内容与 Go replayer 的格式兼容性、视图及分区表完整归档。

TTL 本轮增加 `TIMER_EXT` 回归：在调度间隔变更前写入手动请求及事件信息，旧代码清空它们导致断言失败；修复 `sync_ttl_timers` 后仅更新 tags，保留 manual/event 字段，回归通过。已确认通用 `pkg/timer/{api,tablestore,runtime}` 具备 SQL store、watch notifier 与 Hook worker，但 Session 生产 TTL 仍使用独立 10 秒全范围 tick，缺少将真实 syssession Pool、Go 等价 TTL Hook/job adapter、etcd notifier 和命令/任务通知 watcher 接入该 runtime 的链路。不能据目前的计时器行状态和单机 SQL ownership 声称分布式 timer 行为完成；保留任务文件。`make lint`、`cargo fmt --all -- --check`、`git diff --check`、Server Extract 聚焦测试及 TTL timer 同步聚焦测试通过；测试编译临时创建的 mysqlcompat 清单已删除。

阻塞恢复（2026-09-30）：本机虽无 `tiup`/`tikv-server`/`etcd` 命令，但 Docker 已有 PD/TiKV 镜像及运行时。先发现共享 TiKV 为 API v1，目标 keyspace 请求 API v2 报 `ApiVersionNotMatched`；随后单独启动 PD/TiKV v8.5.1 容器，以 `[storage] api-version = 2` 和独立 `aster43_e2e` keyspace 运行 `go_merge_43_real_tikv_crossks_runtime_submits_and_consumes_table_mode`，17.26 秒通过。测试覆盖目标 keyspace bootstrap、真实 Store/etcd、虚拟 serverinfo、持久化 JobSubmit、owner 消费与表模式可见性。临时容器、网络、配置和 mysqlcompat 测试清单均已删除；共享集群中先前创建的 `aster43_20260930` 测试 keyspace 已按 ENABLED→DISABLED→ARCHIVED→TOMBSTONE 清理。以下旧「本机无运行时」阻塞段只保留为排错历史，不再作为当前阻塞理由。

剩余生产路径核对（2026-09-30）：`pkg/session/runtime/ttl_runtime.rs` 的生产入口每 10 秒调度，已调用 `sync_ttl_timers` 及触发/完成持久化，但尚非 Go timer runtime 的完整通知/watch/分布式调度链；`pkg/session/runtime/inference_test.rs` 已覆盖 IF 未选分支惰性求值、顶层/嵌套/表列 `EMBED_TEXT`，Go 本组 diff 仅要求 Domain EmbedFn 启停与 getter，已由 `Domain::init_inference_providers`、`close_inference_providers` 实现；`pkg/domain/extract.rs` 的 `new_with_domain` 会用真实 parser AST 遍历视图，生产 `CanonicalServerDomain` 的 `extract_runtime` 已在本轮接入。Extract 的完整 Go replayer 格式兼容性仍待验证。

阻塞证据（2026-09-30）：`command -v tiup`、`command -v tikv-server`、`command -v etcd` 均无结果；本地 `InMemoryBackend` 在 canonical snapshot 明确拒绝生产路径，不能替代 TiKV/PD/etcd 的 keyspace、租约及 owner 选举行为。需要一套可访问的目标 keyspace TiKV/PD/etcd 后，执行虚拟 serverinfo 登记→目标 Store/InfoSchema 加载→持久化 AlterTableMode JobSubmit→跨节点 owner 消费→history 完成和关闭清理验证；真实多 Region 边界也需该环境。当前不能把仅编译和 mock 链路当作端到端通过。仍存在需要在真实环境核对的 Go/Rust 兼容性和任务文件列出的 TTL timer、完整 extract、inference 惰性求值生产路径差异，故批次 18 不应将本任务视为已满足依赖。

本轮消除一项本地代码阻碍：`KvInfoSchemaLoader` 及 DDL 写路径现在可在私有 catalog 缺失或落后时从 Go meta 的 `SchemaVersionKey`、`NextGlobalID`、`DBs`、`DB:<id>` hash 读取库表，同时过滤 AutoID 等非表字段；ID 变更写回 Go `NextGlobalID`。独立回归先模拟旧 loader 得到 schema version 0 而失败，再通过；从仅有 Go meta 的快照加载并修改 TableMode 后版本变为 2。命令：`ASTERSQL_GO_MERGE_43_REPRO_OLD_LOADER=1 CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-domain CARGO_BUILD_JOBS=4 cargo test --manifest-path pkg/domain/Cargo.toml --lib go_merge_43_loader_reads_go_meta_without_private_catalog`（预期失败）；撤销临时复现开关后 `CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-domain CARGO_BUILD_JOBS=4 cargo test --manifest-path pkg/domain/Cargo.toml --lib go_merge_43`（11 通过）；`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-session CARGO_BUILD_JOBS=4 cargo check --manifest-path cmd/tidb-server/Cargo.toml --bin astersql-cmd-tidb-server --features nextgen`、`make lint`、`cargo fmt --all -- --check`、`git diff --check` 均通过。复现开关未保留在源码中。

最新验证（2026-09-30）：crossks 生产构造、真实 jobsubmit 与选举后的持久化 AlterTableMode consumer 已完成本地组装；Session `go_merge_43_crossks_` 10 项、Domain 10 项及 crossks 10 项聚焦回归通过，NextGen 服务入口和 Session 编译通过。Ready 的 `make lint`、`cargo fmt --all -- --check`、`git diff --check` 通过；临时 mysqlcompat 清单已清理。自审确认 SessionManager 逆序关闭 lifecycles，先停 owner，再停刷新循环/schema syncer，最后关目标 Domain 与 Store。尚未在真实目标 TiKV/etcd 上运行完整虚拟 serverinfo→JobSubmit→owner→history 链；本地 InMemoryBackend 不支持 canonical snapshot，且没有 tiup。另需验证跨语言 Go 元数据兼容性：Rust Domain 的 catalog 使用独立 `DDL_CATALOG_KEY`，与 Go meta 键空间不相同。故保留任务为进行中，不删任务文件，也不声称真实部署已验证。

续做进展（2026-09-30）：生产 `CrossKSProductionRuntimeFactory` 已接入 NextGen 真实 TiKV Session 工厂，并由 Domain 持有/关闭。每个目标 keyspace 从 PD 解析数值 ID 后建立独立 etcd client；虚拟 serverinfo 在目标运行时创建前登记，同一虚拟 ID 用于 schema 版本上报。目标 Store、5 个线程隔离 SQL 会话、系统表与 MinJobId 刷新循环、server state 读取、DDL JobSubmit、etcd DDL owner 选举、持久化 AlterTableMode 消费、schema 版本发布/等待、history 状态查询与关闭顺序已组装。新回归覆盖目标 etcd 隔离、选举交接、提交/消费/完成、失败 Job 历史、schema 等待屏障、构造失败回收。NextGen `cargo check` 通过。生产 factory 的完整目标 Store/Domain 运行尚不能用本机 `InMemoryBackend` 验证：它明确不支持 canonical snapshot（测试在该边界先失败）；本机无真实 TiKV/tiup。还需 Ready 全量验证与对照 Go owner/InfoSchema 生命周期审核，再决定完成状态。

crossks 最新实现（2026-09-30）：`pkg/session/runtime/crossks_session_pool.rs` 已建 5 个线程隔离的真实 SQL 系统会话；同一租约保留事务亲和性，支持 jobsubmit 的 BDR 角色读取、全局 ID 锁定与分配、系统表查询和关闭。`crossks_job_submit.rs` 将 AlterTableMode 转成版本 2 DDL Job，通过实际 `jobsubmit::submit_batch` 写入目标 `mysql.tidb_ddl_job`，独立测试按系统表解码并验证元数据；目标 TiKV Store 独立打开与 keyspace 隔离测试通过。Domain 已持有 crossks Manager 并在关闭时清理，但服务构造仍未安装生产 RuntimeFactory。剩余关键依赖是目标 keyspace 数值 ID 的 etcd 命名空间、schema/state syncer、真实 DDL owner 消费与历史 Job 完成路径，以及完整生产构造和失败清理回归。当前同步安装仅提交 Job、没有 consumer 会使 `wait_ddl_finished` 一直轮询，因此不能将不完整 factory 当作完成。

本次续做：`new_manager_with_server_info_provider` 允许每个目标 keyspace 在登记虚拟 serverinfo 前解析自己的 etcd client；原单 client API 保持兼容。双目标 client 隔离、未知 keyspace 提前失败和关闭清理回归通过。`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-crossks CARGO_BUILD_JOBS=4 cargo test --manifest-path pkg/domain/crossks/Cargo.toml --lib go_merge_43` 7 项通过；`make lint`、`cargo fmt --all -- --check`、`git diff --check` 通过。Session 聚焦 jobsubmit/系统会话/Store 回归此前通过；临时 mysqlcompat JSON 清单已删除。尚不能删除任务文件。

最新进展（2026-09-30）：生产 TiKV Session 现在从 PD 解析 keyspace 数值 ID，为 serverinfo 的 etcd 键设置 `/keyspaces/tidb/{id}` 命名空间；Domain 持有 Syncer、刷新心跳并在关闭时撤销租约。Domain DDL 创建/删除 TTL 表的外部管理器调用已接入，Starter 的 Session 工厂从实际配置构造管理器，并接到已有真实外部控制器 gRPC 客户端（含集群 TLS 文件）；Domain 关闭时释放管理器。相应的 Domain 9 项、extworkload 10 项、Session TTL 16 项回归、服务入口 `cargo check`、`make lint`、`cargo fmt --all -- --check` 和 `git diff --check` 均通过。Session 单测所需的临时 `tests/mysqlcompat/compatibility-cases.json` 已删除。仍需将 crossks manager 接入生产 Domain/Session 构造；本机无 tiup，真实多 Region TiKV 属获准的本地验证限制，模拟 Region 测试 17 项已通过。

生产 crossks 构造核查：Go `createSessionManager` 同时打开目标 keyspace Store、创建真实系统会话池、schema version/state syncer、InfoSchema syncer 与 DDL jobsubmit 客户端；虚拟 serverinfo 在这些组件启动前登记，失败和关闭时清理。Rust `astersql-domain-crossks` 当前仅有抽象 `RuntimeFactory`/`DdlBackend` 与测试实现，`pkg/domain/Cargo.toml` 将此依赖放在 Windows-only 区块，生产 Domain/Session 无任何引用。`Domain::CrossKeyspaceCoordinator` 只在 testkit 路径绑定，不能替代 Go 的 SessionManager。后续应建立目标 TiKV Store 与实际 session/DDL jobsubmit 的生产适配，再将 `new_manager_with_server_info` 所包装的 factory 安装到 Domain；仅注入一个会立即报错或直接同步改表的 DDL stub 会违反本任务的 Go/Rust 行为对齐要求。

继续实施：crossks 已进入 Domain 普通依赖；Domain 新增 manager 安装、获取及关闭所有权，10 项 `go_merge_43` Domain 回归通过（新增关闭回归）。尚未在 Session 构造 manager：`jobsubmit::SessionPool` 当前只有测试实现，`ConcreteSession` 持有 `RefCell` 而非 `Send`，不能直接满足 jobsubmit 的 `Session: Send`；需要线程隔离的系统会话适配及持久化 job 消费链。`schemaver`、`serverstate`、`issyncer` 都有各自的 Rust 抽象，但目标 Store/etcd 的生产适配尚未组装。以上是生产生命周期的实际依赖，不应以测试 factory 代替。

目标 Store 阶段：`pkg/session/runtime/crossks_store.rs` 用生产 TiKV Driver 对给定 PD 地址与目标 keyspace 单独开 Store，URL 编码 keyspace 名，并以 crossks `Store` 契约持有/关闭；隔离与关闭回归 1 项通过，Session `cargo check` 通过。`make lint`、`cargo fmt --all -- --check`、`git diff --check` 再通过。此适配尚未传入 RuntimeFactory，后续仍需真实系统会话、schema syncer 和 DDL jobsubmit/消费链。

底层元数据契约核查：Domain 目前通过 `KvInfoSchemaLoader::read_catalog` 读取单个 `DDL_CATALOG_KEY`，而 `infoschema::issyncer::SchemaStore` 所需的 `MaxDiffVersion/GetSchemaDiff/ListDatabases/ListTables` 对应 `meta::Reader` 的 Go 式分散元数据键；仓库里这个 SchemaStore 仅有测试实现。DDL jobsubmit 的 `Session: Send`/`SessionPool`、ID 锁定/生成与持久化 job 插入也只有测试实现，Domain 只提供同步 `ddl_set_table_mode_by_ids`，没有生产 job 消费器。要达到 Go crossks 的真实 DDL 提交/完成语义，需先统一这些元数据与 job 表契约，不能只将 Domain getter 接入测试 factory。服务入口 `cargo check` 已在 Domain crossks 依赖变更后通过。

进度（2026-09-30）：已保留并验证独立子路径：crossks 虚拟 server info 的关闭/初始化失败清理；Domain 外部工作负载角色与 TTL 系统变量转发；sysvar 缓存重复回调；plan replayer 动态保留时间；Domain 对 EmbedFn 的启动/关闭所有权；新建 Rust inference 核心的注册、批处理、共享请求、缓存与取消（含取消后新请求不复用已取消批次）、满批立即派发。已实现 OpenAI、Jina AI、Cohere、HuggingFace、NVIDIA NIM、Gemini、TiDB Cloud Hosted HTTP 提供商，Domain 从全局变量动态读取 API key/base URL 并在配置变更时失效缓存；本地 HTTP 回归通过。TTL 已加入真实 SQL session、InfoSchema 物理表发现、系统表持久化认领/心跳/超时接管/收尾、超时作业原水位接续、分区扫描删除、Domain 拥有的定时 worker 与服务启动接线；Session TTL 回归新增周期心跳与取消保留作业检查，TTL worker 57 项回归通过。`extract` 已加入 Domain InfoSchema 视图定义包装源和真实 parser `ast::Walk` 路径，嵌套视图 SQL 回归通过；还需在完整 extract 生产构造入口验证。嵌套 SQL 表达式中的 inference、TTL timer 触发/完成状态持久化与 serverinfo Syncer 选项已补齐并有聚焦回归。仍需核对 TTL 通知链、真实 TiKV Region 边界、Domain 到 infosync/DDL 的生产构造传递和全量覆盖，未达到完成标准。`plan.md` 保持只读。

逐文件覆盖核对（2026-09-30）：

| Go 差异 | Rust 当前路径 | 尚需完成 |
| --- | --- | --- |
| `crossks/cross_ks.go` | `cross_ks.rs` 的注册守卫、关闭清理；`new_manager_with_server_info` 在 runtime factory 创建前登记真实 syncer，失败自动清理、成功移交关闭所有权；真实 lease 撤销与先停 loop 后撤销顺序回归通过（共 6 项） | 当前 Domain 尚未使用 crossks manager 的真实构造入口；Go failpoint 跳过 refresher 属测试设施，Rust 尚无相应 refresher |
| `crossks/cross_ks_test.go` / `export_test.go` | `cross_ks_test.rs` 的清理与 ID 检查 | 对照 Go 的实际 etcd/session 生命周期再核对 |
| `domain.go` | `domain.rs` 的外部角色、保留期与 inference 生命周期；TTL worker 的 Domain 启停与 Session 生产 tick 已接线 | DDL 构造时传递外部管理器；server info options 传递；TTL timer/恢复/分片与 Go 全部调度行为 |
| `domain_sysvars.go` | `domain.rs` 的 master-only 转发与 `sysvar_cache_test.rs` | SQL `SET GLOBAL tidb_ttl_job_enable` 已接 Domain master-only 控制器回调，回归通过；其它入口仍需核对 |
| `domain_test.go` | `canonical_domain_test.rs` 与 `sysvar_cache_test.rs` | TTL 真正启动/不启动和多 Domain 入口的回归 |
| `extract.go` | `extract.rs` 的 `ExtractSource::view_dependencies`；`server/extract_runtime.rs` 生产数据源 | 已加入 `new_with_domain` 生产 AST 包装源和真实 `ast::Walk` 嵌套依赖回归；Server HTTP Extract 入口及非空摘要归档已接线并验证；仍需 Go replayer 格式兼容性核对 |
| `inference.go` | `domain.rs` 拥有 `EmbedFn` 启停；`pkg/inference` 有核心实现、OpenAI/Jina/Cohere/HuggingFace/NVIDIA/Gemini/TiDB Cloud Hosted HTTP 提供商及本地回归 | SQL 常量及表列 SELECT 的顶层和嵌套表达式已接入并经 HTTP 回归；其它 SQL 算子、短路表达式中的惰性求值及 HTTP 阻塞请求取消语义仍需对齐 Go |

TTL 依赖核对：Go `jobLoopWithSession` 在一个真实系统会话中使用 `tidb_ttl_table_status` / task / history 表、InfoSchema 缓存、timer store、命令及通知 watcher、扫描/删除 worker。Rust 新增 `pkg/session/runtime/ttl_worker_session.rs`，让 TTL worker 的 `%?` 参数 SQL 在真实 `ConcreteSession` 执行；`ttl_metadata.rs` 从单个 InfoSchema 快照取得 TTL 元数据并为分区生成独立物理表；`persistent.rs` 用事务持久化认领/心跳/超时接管/收尾，并阻止被接管的旧 owner 完成作业；`ttl_runtime.rs` 实际调度扫描、限速删除、重试及汇总，超时后重读原扫描水位并接续，`Domain` 启停 worker，服务启动时接线。回归验证真实表中过期 DATETIME 行及真实分区表由 `TtlScanTask` 扫描并由 `DeleteTask` 删除；修复扫描的 Unix 秒水位比较与 Session DELETE 的表类型感知日期谓词（该缺陷曾导致 DELETE 成功却影响 0 行）。由于通用 SQL runtime 尚不支持 `FROM_UNIXTIME`，适配器在 TTL UTC 边界将该谓词转换为等价 DATETIME 字面量。接续新回归已通过，验证超时旧 owner 作业以原始过期水位完成。系统表 timer 记录已支持创建/更新/禁用/恢复、触发/完成事件状态、摘要和作业水位，仍缺 Go timer runtime、命令/通知 watcher 与真实 TiKV Region 边界验证（本机未安装 tiup；超过 64 个 TiKV store 时的动态分片数也尚未接入）；Region 任务分片已写入系统表并经双范围接管回归，游标在每个成功删除批次后持久化并用于接管续扫；扫描期间独立 SQL 会话周期心跳已接线并经 ownership loss 回归；取消扫描不会再清理持久化作业，新增真实系统表回归先失败后通过。当前同步全范围 tick 尚不等价于完整 Go JobManager。

TTL 下一实施段：在已有真实 SQL/InfoSchema/系统表/Domain worker 接线上，继续整合现有 `BaseWorker`、timer syncer、命令/通知客户端，并在真实 TiKV 多 Region 环境验证分片边界；给运行中的任务加周期心跳与取消/恢复测试。随后完成本任务其余 Go 文件的生产路径核对。回归继续验证真实过期行删除、心跳/接管、关闭清理与外部工作负载角色门禁。

验证记录（最新）：`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-crossks cargo test --manifest-path pkg/domain/crossks/Cargo.toml --lib go_merge_43`（6 通过）；`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-serverinfo cargo test --manifest-path pkg/domain/serverinfo/Cargo.toml --lib`（12 通过）；`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-inference CARGO_BUILD_JOBS=4 cargo test --manifest-path pkg/inference/Cargo.toml --lib go_merge_43`（16 通过）；`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-vardef cargo test --manifest-path pkg/sessionctx/vardef/Cargo.toml --lib go_merge_43`（1 通过）；`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-domain CARGO_BUILD_JOBS=4 cargo test --manifest-path pkg/domain/Cargo.toml --lib go_merge_43`（7 通过）；`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-session CARGO_BUILD_JOBS=4 cargo test --manifest-path pkg/session/Cargo.toml --lib go_merge_43_ttl_`（16 通过，取消保留作业及 timer 事件摘要回归先失败后通过）；`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-session CARGO_BUILD_JOBS=4 cargo test --manifest-path pkg/session/Cargo.toml --lib --features nextgen go_merge_43_embed_text_sql_calls_domain_provider_and_returns_vector`（1 通过，真实 HTTP 与 SQL）；`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-ttlworker CARGO_BUILD_JOBS=4 cargo test --manifest-path pkg/ttl/ttlworker/Cargo.toml --lib`（55 通过）及 `physical_partition`（2 通过）；`CARGO_TARGET_DIR=/tmp/astersql-go-merge-43-session CARGO_BUILD_JOBS=4 cargo check --manifest-path cmd/tidb-server/Cargo.toml --bin astersql-cmd-tidb-server` 通过；`make lint` 与 `git diff --check` 通过；store driver `cargo check` 通过。Session 测试编译时临时创建缺失的 `tests/mysqlcompat/compatibility-cases.json` 空清单，测试后删除。`cargo fmt --all -- --check` 曾因并行任务文件格式差异失败；本任务文件逐个 rustfmt，最终需再检查。

目的：逐项同步本组 Go 文件在合并中引入的行为与测试意图，保持 Rust 实现和 Go 最新逻辑等价。

来源任务：`ad193e964b` 第一父差异；覆盖清单中的任务 43。

预计会话范围：8 个 Go 文件，合计 450 行差异。只处理本组函数及相邻 Rust 测试；若单文件内容较大，按函数/行为分段验证，并记录每段证据。

最近一次 Ready 检查（2026-09-30）：`make lint` 通过；`git diff --check` 通过；本任务改动的 Rust 文件 `rustfmt --edition 2024 --check` 通过；`cargo fmt --all -- --check` 因同一共享工作区中其他任务的 `pkg/domain/domain.rs`、`pkg/session/runtime/dispatch.rs` 等未格式化差异失败。`pkg/ttl/cache` 的 `split` 过滤 17 项通过，`pkg/store/driver` cargo check 通过，`pkg/domain` 的 `go_merge_43` 7 项通过，`pkg/domain/serverinfo` 全部 12 项通过，Session TTL 16 项、Starter 嵌套 EMBED_TEXT 及 NextGen HTTP 回归通过。`tests/mysqlcompat/compatibility-cases.json` 临时空清单已删除。尚未验证真实多 Region TiKV（本机无 tiup；用户允许先用模拟 Region），且 Domain 当前未构造 InfoSyncer/DDL 实例，相关 Go 选项无法在其生产初始化链上验证。

## 文件

- Go 来源：`pkg/domain/crossks/cross_ks.go`（+19/-4）
- Go 来源：`pkg/domain/crossks/cross_ks_test.go`（+147/-6）
- Go 来源：`pkg/domain/crossks/export_test.go`（+5/-0）
- Go 来源：`pkg/domain/domain.go`（+66/-4）
- Go 来源：`pkg/domain/domain_sysvars.go`（+9/-0）
- Go 来源：`pkg/domain/domain_test.go`（+155/-0）
- Go 来源：`pkg/domain/extract.go`（+1/-1）
- Go 来源：`pkg/domain/inference.go`（+33/-0）
- Rust 候选：`pkg/domain/crossks/cross_ks.rs`
- Rust 候选：`pkg/domain/crossks/cross_ks_test.rs`
- Rust 候选：`pkg/domain/crossks/export_test.rs`
- Rust 候选：`pkg/domain/domain.rs`
- Rust 候选：`pkg/domain/domain_sysvars.rs`
- Rust 候选：`pkg/domain/domain_test.rs`
- Rust 候选：`pkg/domain/extract.rs`
- Rust 候选：`pkg/domain/inference.rs（候选，先用索引确认）`
- Cargo 包线索：`pkg/domain/Cargo.toml`、`pkg/domain/crossks/Cargo.toml`

## 上下文

- 先运行 `~/.rustcodegraph/bin/rustcodegraph status`，若索引新鲜，再按本组 Go 符号用 `explore` / `node` 查 Rust 调用链；索引不覆盖时用 `rg` 和原始文件。目标包有 `doc.go` 时先读。
- 用 `git diff ad193e964b^1 ad193e964b -- pkg/domain/crossks/cross_ks.go pkg/domain/crossks/cross_ks_test.go pkg/domain/crossks/export_test.go pkg/domain/domain.go pkg/domain/domain_sysvars.go pkg/domain/domain_test.go pkg/domain/extract.go pkg/domain/inference.go` 阅读完整来源差异，连同 Go 测试、调用方和 Rust 独有适配层核对。上列 Rust 路径仅为文件名候选，不能据此省略真实调用链。
- Rust 单元测试与源文件分离；不删 PingCAP 注释；修复真正可用后在 Rust 源文件顶部增加 `// Copyright 2026 AsterSQL.`。


## 测试计划

- 行为：本组 Go 改动中的每个可观察函数分支、错误与边界，在 Rust 对应调用路径中产生相同结果。先列 Go→Rust 符号/测试对照；测试专用或生成文件也要追溯意图并记录判定。
- 失败验证测试：在对应 Rust 独立测试文件中新增或扩展 `go_merge_43` 前缀的聚焦回归测试；若本组只有生成物或测试设施变更，先记录为何无法构造先失败的行为测试并采用生成/编译或测试意图检查。
- 失败验证命令：`cargo test --manifest-path pkg/domain/Cargo.toml --lib go_merge_43`
- 预期失败原因：未移植的 Go 语义在 Rust 真实路径上产生不同结果；如果基线先因无关编译错误失败，记录准确错误并修复本任务涉及的依赖或标记待回归。
- 通过验证命令：`cargo test --manifest-path pkg/domain/Cargo.toml --lib go_merge_43`
- 模拟策略：优先使用现有真实 Rust 依赖与测试设施；仅对网络、外部服务或时间等明确边界使用现有 mock，核对输入与副作用。

## 步骤

1. 逐文件审查来源差异，建立本任务内部的 Go 文件/符号→Rust 文件/符号→测试对应清单；对没有 Rust 行为的测试辅助或生成产物，写明源码证据与原因。
2. 编写失败回归测试并运行失败验证命令；测试与源码分离。
3. 按 Go 控制流、状态更新、错误返回、并发与边界逐项移植，保留 Rust 特有实现所必需的适配；只扩展实际证据要求的范围。
4. 运行通过验证命令及相邻检查；对共享文件修改先与同批次任务协调，不能安全并行时等前一批完成。
5. 自审 `git diff --check` 和本任务涉及的 Rust 差异，并按 `AGENTS.md` 的 Ready profile 做交付验证；验证失败需记录原因和风险。

## 验证

- 格式：Rust 代码修改完成后，先运行 `cargo fmt --all` 自动格式化，再运行 `cargo fmt --all -- --check` 校验；自审格式化产生的差异。
- 运行：`cargo test --manifest-path pkg/domain/Cargo.toml --lib go_merge_43`
- 运行：`cargo fmt --all -- --check`
- 运行：`make lint`（代码交付的 Ready 门槛；若环境/基线阻塞，记录具体错误）
- 预期：本组回归测试经历预期失败后通过；每个 Go 文件均有 Rust 对应行为、测试意图或有源码证据的无可移植项说明。
- 所需证据：Go→Rust 逐文件覆盖清单、失败/通过命令和结果、修改文件、`git diff --check`、格式/lint 结果、未验证项及正确性/兼容性/性能风险。仅以编译通过不足以标记完成。

## 完成

获得上述证据后删除本任务文件，并在最终回复报告逐文件覆盖与准确命令。若阻塞，只更新此文件为 `已阻塞` 并记具体原因与检查；若仅因无关基线使验证无法运行，可设为 `已完成，待回归` 并记录可复现证据。不要修改 `plan.md`。
