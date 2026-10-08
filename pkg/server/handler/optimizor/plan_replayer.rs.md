# `pkg/server/handler/optimizor/plan_replayer.rs` 逻辑说明

## 文件定位

本文件属于 `astersql-server-handler-optimizor` crate；crate 根 `pkg/server/handler/optimizor/lib.rs` 以 `pub mod plan_replayer` 暴露它，`pkg/server/handler/optimizor/Cargo.toml` 则把该 crate 对应到 Go 包 `pkg/server/handler/optimizor`。它是 Plan Replayer 压缩包下载逻辑的 Rust 移植：接收路由中的文件名，优先读取本机 dump，必要时向其他 TiDB 状态服务转发请求，并对特定 capture 包补入历史统计。

需要区分“模块能力”和“当前主服务接线”。本文件的入口是 `PlanReplayerHandler::ServeHTTP`，但当前 Rust status server 在 `pkg/server/http_status.rs::build_status_router` 中把 `/plan_replayer/dump/{filename}` 注册到同文件的 `plan_replayer_download_response`，代码搜索没有发现生产代码构造或调用本文件的 `PlanReplayerHandler`；后者目前由 `pkg/server/handler/optimizor/plan_replayer_test.rs` 直接测试。当前实际 status 路由只从全局 ExtStorage 读取并返回文件，没有使用本文件的跨节点转发和 capture 重写流程。

## 核心职责

- `PlanReplayerHandler::ServeHTTP` 把请求侧信息转换为 `downloadFileHandler`，统一约定 dump 目录、远程 URL、附件文件名和内部 HTTP scheme。
- `handleDownloadFile` 执行“本地读取 → 已转发请求停止 → 遍历远端拓扑 → 最终 404”的下载决策，并统一写 zip 响应头。
- `handlePlanReplayerCaptureFile` 只为文件名以 `capture_replayer` 开头且下载类型为 `plan_replayer` 的本地文件补入快照时刻的历史统计。
- `loadSQLMetaFile`、`loadSchemaMeta` 和 `dumpJSONStatsIntoZipInMemory` 分别处理归档内 SQL 元数据、schema 元数据和 `stats/<db>.<table>.json` 条目。
- `PlanReplayerRuntime` 将文件存储、拓扑发现、HTTP、zip 编解码、表解析、历史统计和 HTTP 响应全部抽象出去，使本文件本身不绑定具体服务器或 IO 实现。

以上职责来自 `plan_replayer.rs` 的同名符号。文件没有条件编译项、模块级常量或独立 `impl PlanReplayerRuntime`；所有真实外部副作用都由调用者提供 runtime。

## 主要符号

- `Archive`：用 `BTreeMap<String, Vec<u8>>` 表示完整内存 zip；有序 map 让既有条目的遍历顺序稳定，但最终编码细节由 runtime 决定。
- `TableMeta` 与 `tblInfo`：前者保存表 ID、库名和表名；后者在此基础上携带可选的历史统计 JSON。`tblInfo` 是内部风格名称，但当前声明为 `pub`。
- `Topology` 与 `RemoteResponse`：分别描述远端状态服务的 IP/端口和一次 HTTP GET 的状态码/完整响应体。
- `downloadFileHandler`：一次下载所需的内部上下文，包括本地路径、远端路径、地址、端口、scheme 和下载附件名；类型本身未公开。
- `PlanReplayerRuntime`：本文件的关键扩展边界。其关联错误类型 `Error` 贯穿读文件、拓扑、HTTP、归档、统计和响应写入；`write_error` 只由顶层 `ServeHTTP` 用于最终错误映射。
- `PlanReplayerHandler` / `NewPlanReplayerHandler`：公开 handler 及 Go 风格构造函数；结构体只保存本机地址和 status 端口，其余依赖均来自 runtime。
- `handleDownloadFile`：公开的共享下载算法，也是 `ServeHTTP` 的直接下游。
- `handlePlanReplayerCaptureFile`、`loadSQLMetaFile`、`loadSchemaMeta`、`dumpJSONStatsIntoZipInMemory`：公开的 capture 包处理流水线。
- `write_zip_response`、`filepath_join`、`join_host_port`：私有辅助函数，分别负责附件响应、Go `filepath.Join` 风格路径合并和 Go `net.JoinHostPort` 风格地址格式化。

## 执行流程

1. `PlanReplayerHandler::ServeHTTP` 从 `runtime.route_file_name()` 取得路由文件名，以 `runtime.plan_replayer_directory()` 和 `filepath_join` 生成本地路径，并构造 `plan_replayer/dump/<file>` 远端路径。它调用 `handleDownloadFile`；出现错误时不再返回 `Result`，而是交给 `runtime.write_error`。
2. `handleDownloadFile` 先缓存 `runtime.forwarded()`，并用 `join_host_port` 形成日志地址，然后调用 `read_local_file`。
3. 本地命中时，只有附件类型为 `plan_replayer` 且文件名以 `capture_replayer` 开头才调用 `handlePlanReplayerCaptureFile`。之后 `write_zip_response` 设置 `Content-Type: application/zip`、固定的 `<downloaded_filename>.zip` 附件名并写出内容，记录成功日志后返回。
4. 本地未命中且请求已经带有转发语义时，立即写 404 并返回，不再继续广播；这是防止节点间循环转发的终止条件。
5. 原始请求本地未命中时，遍历 `runtime.topology()`：跳过与 handler 地址和端口都相同的本机节点；对其他节点构造 `<scheme>://<host:port>/<url_path>?forward=true`。第一个状态码为 200 的响应会被写回并终止遍历；非 200 和请求错误只记录日志，继续尝试下一节点。
6. 所有远端均未命中时写 404，再写 `can't find dump file ... in any remote server` 正文。正文写失败会作为 `Err` 返回。
7. capture 流程先 `decode_zip`，再由 `loadSQLMetaFile` 读取 `sql_meta.toml` 的 `startTS`。条目不存在时返回 0，调用者原样返回输入字节；存在且快照非零时，`loadSchemaMeta` 读取 `schema/schema_meta.txt`，逐表 `resolve_table` 和 `dump_historical_stats`，最后把统计条目放入 archive 并 `encode_zip`。

## 数据与状态

本文件不维护跨请求全局状态。`PlanReplayerHandler` 只有不可变的本机地址和端口；一次请求的可变状态位于传入的 `&mut R` 以及局部的 `Archive`、`HashMap<i64, tblInfo>` 和字节缓冲区中。

`Archive`、`RemoteResponse.body`、本地文件内容以及 capture 的重新编码结果都是完整 `Vec<u8>`。因此本文件的 Rust 路径会把下载对象整体放入内存；capture 还会同时持有原始内容、解码条目、每表统计及重新编码结果。普通本地文件和远端 200 响应也通过完整字节写出，不具备 Go 版本 `io.Copy` 的流式内存上界。

`loadSchemaMeta` 以表 ID 为 `HashMap` 键；多个 schema 行若解析到同一个 ID，后者覆盖前者。`dumpJSONStatsIntoZipInMemory` 又按 `HashMap::values()` 遍历，新增 stats 条目的编码顺序不保证稳定。每个 `tblInfo.json_stats` 在写归档前必须为 `Some`。

## 依赖与调用关系

直接编译依赖只有 `astersql_domain::plan_replayer_dump::{PLAN_REPLAYER_SCHEMA_META_FILE, PLAN_REPLAYER_SQL_META_FILE}`；它们确定归档元数据条目名称。尽管 `pkg/server/handler/optimizor/Cargo.toml` 声明了 infoschema、statistics、server-handler、replayer 等更多 crate，本文件通过 `PlanReplayerRuntime` 隔离这些具体类型，并未直接引用它们。

已确认的文件内调用链为：`PlanReplayerHandler::ServeHTTP → handleDownloadFile → {handlePlanReplayerCaptureFile, write_zip_response}`；capture 分支为 `handlePlanReplayerCaptureFile → loadSQLMetaFile → loadSchemaMeta → PlanReplayerRuntime::dump_historical_stats → dumpJSONStatsIntoZipInMemory → PlanReplayerRuntime::encode_zip`。转发分支依赖 runtime 的 `topology` 和 `http_get`，响应与日志同样由 runtime 完成。

RustCodeGraph 对目标文件报告 40 个符号，并能定位 Rust/Go 两组同名核心函数；对这些泛型且跨语言重名的符号执行 `callers/callees` 未返回静态边。因此生产接线另以仓库搜索核验：`pkg/server/http_status.rs::build_status_router` 当前调用的是 `plan_replayer_download_response`，而非本文件入口。crate 通过 `pkg/server/Cargo.toml` 被 server 依赖、通过根 `pkg/lib.rs` facade 再导出，但再导出不等于本 handler 已进入主请求链。

## 错误处理与边界

- 除顶层 `ServeHTTP` 外，核心函数都用 `Result<_, R::Error>` 传播 runtime 错误；顶层捕获一次并调用 `write_error`。日志接口的失败不建模为错误。
- 本地读、拓扑发现、capture 解码/编码、表解析、历史统计和响应正文写入失败会终止当前流程。远端 HTTP 请求失败或远端非 200 则是可恢复事件，只记录后继续遍历。
- 已转发请求本地未命中时只写 404，不写说明正文；原始请求遍历完远端后既写 404 也写说明正文。
- `loadSQLMetaFile` 在 `sql_meta.toml` 条目缺失时返回 0；条目存在但不是 UTF-8、`startTS` 非无符号整数或完全缺少 `startTS` 时返回 `invalid_data`。解析器只扫描按行 `key=value`，并非完整 TOML 解析；只去掉值两侧的单/双引号。
- `loadSchemaMeta` 在 schema meta 条目缺失时返回空 map；非 UTF-8 报错；缺少前两个分号字段的行被忽略，多余字段被忽略。数据库名和表名不做 trim 或转义处理，逐行同步调用 `resolve_table`。
- `dumpJSONStatsIntoZipInMemory` 遇到任何表缺少 `json_stats` 就失败；已插入的局部 archive 修改不会被编码返回。相同 stats 路径会覆盖归档中已有条目。
- `filepath_join` 明确保留 Go `filepath.Join` 的清理语义，包括处理 `..` 时弹出目录组件；例如测试中的 `../outside.zip` 会从 `/tmp/replayer` 解析为 `/tmp/outside.zip`。本文件没有自行限制路由文件名必须留在 replayer 根目录，安全边界必须由路由/runtime 或后续接线显式保证。
- `join_host_port` 为含冒号的主机加方括号，覆盖 IPv6 URL 格式；它不校验 IP 或主机名合法性。

## 并发与资源生命周期

函数签名要求一次调用独占借用 `&mut R`，因此单个 runtime 实例的操作在本文件内是顺序执行的；本文件没有线程、异步任务、锁、channel 或共享缓存。多个请求是否能并发取决于上层是否为它们提供独立 runtime，或在外部同步共享状态。

拓扑节点严格串行尝试，没有并发广播、超时或取消逻辑；这些策略若存在只能由 `http_get` 的具体实现提供。远端成功以“首个遍历到的 200”为准，因此拓扑顺序影响响应延迟和命中节点。

文件、HTTP body 和 zip reader/writer 的打开与关闭不在本文件管理：runtime 只交换拥有所有权的 `Vec<u8>` 和 `Archive`。这避免本模块泄露句柄，但代价是全量内存物化。`pkg/server/handler/optimizor/plan_replayer_test.rs` 的真实 HTTP fixture 使用全局 ExtStorage，因而用 `PLAN_REPLAYER_E2E_SERIAL` mutex 串行化并在结束时清空 storage、关闭 server/storage、删除临时目录；这是相邻测试环境的生命周期要求，不是本文件生产代码内部的锁。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/server/handler/optimizor/plan_replayer.go`。Rust 保留了 `PlanReplayerHandler`、`NewPlanReplayerHandler`、`ServeHTTP`、`handleDownloadFile`、`handlePlanReplayerCaptureFile`、`loadSQLMetaFile`、`loadSchemaMeta`、`dumpJSONStatsIntoZipInMemory` 和 `tblInfo` 等主要结构及总体分支顺序，也保留了 `forward=true` 防循环、跳过本机、固定附件名、IPv6 host:port 和 capture 历史统计注入语义。

已验证的差异如下：

- Go handler 直接持有 `InfoSchema`、统计句柄和 `InfoSyncer`；Rust handler 只持有地址/端口，其余通过 `PlanReplayerRuntime` 注入。
- Go 从全局 ExtStorage 打开文件；普通本地文件和远端响应用 `io.Copy` 流式返回，只有 capture 包全量读取。Rust runtime 接口对所有路径统一使用完整字节。
- Go 使用 BurntSushi TOML 解码 `sql_meta.toml`；Rust 是按行的有限解析器。Go 在 meta 文件存在但没有 key 时对空字符串做 `ParseUint` 并报错，Rust也报 `missing startTS`，但错误文本和可接受语法不完全相同。
- Go 的 `loadSchemaMeta` 直接索引分号拆分结果，格式不完整可能 panic；Rust 忽略字段不足的行。Go 使用 `context.Background()` 查表，Rust把上下文策略交给 `resolve_table`。
- Go `dumpJSONStatsIntoZipInMemory` 复制原 zip 条目并对结构化 `JSONTable` 再做 `json.Marshal`；Rust假定 runtime 返回的 `Vec<u8>` 已经是最终 JSON，并直接插入抽象 archive。
- Go 在 `infoGetter == nil` 或请求已转发时直接 404；Rust trait 没有“拓扑提供者缺失”状态，只有 `forwarded` 会短路，拓扑缺失应由 `topology()` 的返回值或错误表达。
- Go 生产路由通过 `Server.newPlanReplayerHandler()` 使用该 Go handler；Rust 当前生产路由尚未接入本文件，而使用 `pkg/server/http_status.rs::plan_replayer_download_response` 的较窄实现。

Go 测试 `pkg/server/handler/optimizor/plan_replayer_test.go` 覆盖真实 dump/download/load、capture 文件清单、缺失文件、递归外键和历史统计行为。Rust 同目录测试前半部分仍有保留 Go 语义的迁移说明，但末尾已有针对本模块的可执行测试，覆盖元数据名与转发 URL、最终正文写错误、缺失 `startTS`、IPv6、只重写 plan-replayer capture、路径 join，以及相邻 status server 的真实下载链路。

## 扩展指南

- 若要让本文件成为真实 Rust status 路由实现，首先应在 `pkg/server/http_status.rs::build_status_router` 建立具体 `PlanReplayerRuntime` 适配器并替换或复用 `plan_replayer_download_response`；必须同步核验 capture 历史统计、远端拓扑、错误到 HTTP 状态的映射和请求取消。仅在 facade 中再导出类型不足以完成接线。
- 若增加存储后端或 HTTP client，应实现/扩展 `PlanReplayerRuntime::{read_local_file, topology, http_get}`，并在 `pkg/server/handler/optimizor/plan_replayer_test.rs` 增加本地错误、拓扑错误、远端 200/非 200/传输错误及 forwarded 短路测试。
- 若扩展归档格式，应优先修改 `loadSQLMetaFile`、`loadSchemaMeta` 或 `dumpJSONStatsIntoZipInMemory`，同时对照 `pkg/server/handler/optimizor/plan_replayer.go` 和 `astersql_domain::plan_replayer_dump` 的常量/编码契约；需要测试缺失条目、无效 UTF-8、重复表 ID、已有 stats 条目和多表输出。
- 若处理超大 dump，应重新设计 runtime 为 reader/stream 接口，至少让非 capture 本地/远端路径流式传输；当前 `Vec<u8>` API 无法只靠内部改写消除峰值内存。capture 重写还需评估 zip bomb、条目大小与条目数量限制。
- 若收紧安全边界，应在进入 `filepath_join` 前验证文件名是单一正常路径组件，并为绝对路径、`..`、平台前缀和 URL 编码变体添加回归测试。改变现有 Go 兼容语义前需明确兼容策略。
- 若并行请求远端，必须定义首个成功响应、取消其余请求、日志顺序和 runtime 的 `Send`/`Sync` 约束；当前串行、可变独占 runtime 合约不能直接并发复用。
- 行为修改应同时更新 `pkg/server/handler/optimizor/plan_replayer_test.rs`；涉及与 Go 一致性的变化还应核对 `pkg/server/handler/optimizor/plan_replayer_test.go`，但测试逻辑应继续留在独立测试文件，不嵌入生产 `.rs`。

## 验证依据

- RustCodeGraph：`status` 显示项目已索引 11,467 个文件、目标目录包含 Rust/Go 源及独立测试；`node --file pkg/server/handler/optimizor/plan_replayer.rs --offset 1 --limit 500` 读取到目标文件 349 行和 40 个符号；`query` 分别定位了 Rust/Go 的 `handleDownloadFile`、`handlePlanReplayerCaptureFile`、`loadSQLMetaFile`、`loadSchemaMeta`、`dumpJSONStatsIntoZipInMemory`。对精确 Rust 名称执行 `callers/callees` 无静态边输出，因此没有据此声称生产调用边。
- Rust 源与装配：`pkg/server/handler/optimizor/plan_replayer.rs`、`pkg/server/handler/optimizor/lib.rs`、`pkg/server/handler/optimizor/Cargo.toml`、`pkg/server/Cargo.toml`、`pkg/lib.rs`。
- 当前 Rust 路由证据：`pkg/server/http_status.rs::plan_replayer_download_response` 与 `build_status_router`，以及 `pkg/server/http_status_test.rs::status_listener_downloads_plan_replayer_from_ext_storage`。
- Go 对照与生产接线：`pkg/server/handler/optimizor/plan_replayer.go`、`pkg/server/http_handler.go::newPlanReplayerHandler`、`pkg/server/http_status.go::startHTTPServer`。
- 测试证据：`pkg/server/handler/optimizor/plan_replayer_test.rs` 中 `HandlerRuntime` 及 `plan_replayer_uses_go_archive_metadata_names_and_forward_url`、`plan_replayer_propagates_final_body_write_errors`、`plan_replayer_rejects_sql_meta_without_start_ts`、`plan_replayer_formats_ipv6_forward_addresses_like_go`、`shared_download_handler_only_rewrites_plan_replayer_capture_files`、`plan_replayer_uses_filepath_join_semantics_for_route_names`；Go 文件 `pkg/server/handler/optimizor/plan_replayer_test.go` 中 `TestDumpPlanReplayerAPI` 与 `TestDumpPlanReplayerAPIWithHistoryStats`。
- 本任务只新增说明文档，未运行 Cargo。交付前按任务命令验证固定十一个二级章节，并用 `git diff --check` 检查文档补丁格式。
