# `pkg/server/handler/tikvhandler/tikv_handler.rs`

## 文件定位

本文件是 `astersql-server-handler-tikvhandler` crate 中通用 TiKV 状态 HTTP 接口的主体实现。crate 入口 [`lib.rs`](./lib.rs) 公开 `tikv_handler` 模块并再导出其符号；[`Cargo.toml`](./Cargo.toml) 通过 `package.metadata.porting.go-package` 明确其 Go 来源是 `pkg/server/handler/tikvhandler`。状态服务器的直接接线位于 `pkg/server/http_status.rs`：适配函数构造本文件的 handler、把外层请求转换成 `Request`，调用 `ServeHTTP`，再把 `ResponseWriter` 转成真实 HTTP 响应。

它覆盖 schema、表/Region、MVCC、TiFlash 副本、DDL 诊断、节点信息、profile、测试 GC、删除测试 key、server label、ingest 并发参数和事务 GC 状态等运维接口。文件不是底层 TiKV/PD/DDL 客户端本身：真实副作用经 `Storage`/`PdClient` 持有的 `Arc<dyn TikvRuntime>` 注入。当前源码还在文件内定义了 `TableInfo`、`Request`、`ResponseWriter`、`Data` 等适配/占位类型，因此应把它理解为“Go handler 行为的 Rust 端口与状态服务边界”，而不是所有依赖子系统的完整 Rust 数据模型。

## 核心职责

- 定义 HTTP handler 门面及构造函数，例如 `SettingsHandler`/`NewSettingsHandler`、`TableHandler`/`NewTableHandler`、`MvccTxnHandler`/`NewMvccTxnHandler`。各 `ServeHTTP` 方法只转发到对应的 `serve_*_http`，使状态服务能以统一方式调用。
- 解析和验证 HTTP 输入：方法、路径变量、表单、query、JSON/body，以及 Go `strconv`、`net/url`、`encoding/base64` 的若干边界语义。代表性入口为 `serve_value_http`、`parseQuery`、`parse_i64`、`parse_start_ts` 和 `base64_decode`。
- 按操作字符串分派表与 MVCC 请求。`serve_table_http` 处理 regions/ranges/disk-usage/scatter，`serve_mvcc_txn_http` 处理 hex/key/idx/txn。
- 构造稳定的响应形状和 TiKV key 范围。`createRangeDetail`、`createTableRanges`、`table_ranges_response`、`manualWriteJSONArray` 分别负责范围、分区展开和单一 JSON 数组响应语义。
- 将存储、PD、schema、DDL、MVCC、ingest 和日志副作用收束到 `TikvRuntime: Send + Sync`，便于真实状态服务适配和独立测试替身复用。
- 保留 Go 实现的重要保护条件：只允许特定 HTTP 方法、设置值范围检查、DDL 查询分页约束、GC 单实例执行、ingest 非负值以及不支持操作的显式错误。

## 主要符号

- 通用边界：`TikvHandlerTool { store, region_cache }` 聚合运行时和 PD client；`Storage`、`RegionCache`、`PdClient` 都可克隆并共享同一个运行时；`TikvRuntime` 是全部外部能力的 trait 边界。
- handler 组：`SettingsHandler`、`SchemaHandler`、`SchemaStorageHandler`、`DBTableHandler`、`FlashReplicaHandler`、`RegionHandler`、`TableHandler`、`DDLHistoryJobHandler`、`DDLResignOwnerHandler`、`DDLCheckHandler`、`ServerInfoHandler`、`AllServerInfoHandler`、`ProfileHandler`、`DDLHookHandler`、`ValueHandler`、`LabelHandler`、`MvccTxnHandler`、`TestHandler`、`DeleteKeyHandler`、`IngestConcurrencyHandler`、`TxnGCStatesHandler`。
- 操作常量：`OP_TABLE_*` 决定表接口分支；`OP_MVCC_GET_BY_*` 决定 MVCC 分支；四个 `INGEST_PARAM_*` 限定可读写的 ingest 参数；`REQUEST_DEFAULT_TIMEOUT` 表示默认十秒请求超时，但本文件内未直接消费它。
- 请求/响应适配：`Request` 保存 method、form、raw query、body、context 和 path；`ResponseWriter` 延迟收集 header、status、body、类型擦除数据及错误，最终由外层状态服务序列化。
- 数据结构：`TableRegions`、`RangeDetail`、`TableRanges`、`IndexRegions`、`RegionDetail`/`FrameItem`、`TableFlashReplicaInfo`、`SchemaTableStorage`、`ServerInfo`、`ClusterServerInfo`、`DBTableInfo`、`DDLCheckResult`。
- 编码辅助：`encode_table_prefix`、`gen_table_record_prefix`、`gen_table_index_prefix` 和 `encode_table_index_prefix` 使用可排序的大端有符号整数编码构造 TiDB 表 key；`index_ranges` 与 `encode_row_key_with_max_handle` 生成半开区间端点。
- 公开性需谨慎理解：很多结构和函数为 `pub` 是为了 crate 外状态服务适配和测试访问；`validate_setting_value`、`write_error`、key 编码函数等仍是文件内部实现。

## 执行流程

1. `pkg/server/http_status.rs` 从 URL 组装路径参数，通过 `tikv_tool(server)` 或 `domain.tikv_runtime()` 获取运行时，调用 `New*Handler`（或直接构造无工具 handler）。
2. handler 的 `ServeHTTP` 转发到对应 `serve_*_http`。入口先解析 method/path/form/query/body；失败立即把 `Error` 放进 `ResponseWriter.errors`，部分方法错误还设置 405。
3. schema/Region/DDL/节点/profile 等入口完成少量路由和约束处理后调用 `TikvRuntime`。例如 `serve_schema_http -> schema -> resolve_schema_request`，`serve_region_http -> region_route/region_detail`，`serve_ddl_check_http -> execute_admin_check`。
4. `serve_table_http` 先用 `ExtractTableAndPartitionName` 标准化 `table(partition)`，再通过 `resolve_table_route` 取逻辑表；regions、ranges、disk usage 对逻辑表工作，scatter/stop-scatter 选择一个 `PhysicalTable` 后调用运行时。
5. `serve_mvcc_txn_http` 根据 `op` 分派。idx/key 路径先 `parseQuery(..., true)`，再解析表与 handle；txn 路径按 Go base-0 规则解析 `startTS`，以表前缀和最大 row key 作为扫描范围；hex 路径直接交给运行时。
6. `serve_settings_http` 的非 POST 返回配置；POST 逐项忽略空值、先执行本文件拥有的格式/范围校验，再调用 `apply_setting`。`ddl_slow_threshold <= 0` 与 Go 一样不更新。
7. `serve_ingest_concurrency_http` 仅接受四种参数：GET 读取，POST 解码 `value`、拒绝负数并写入，其他方法置 405。`serve_txn_gc_states_http` 则只接受 GET。
8. 外层 `tikv_response` 消费 `ResponseWriter`，把延迟数据或错误转成最终 HTTP 响应；因此本文件多数成功路径不直接写字节，`serve_profile_http` 是直接写二进制 body 的例外。

## 数据与状态

- `TikvHandlerTool`、`Storage`、`PdClient` 通过 `Arc<dyn TikvRuntime>` 共享外部状态；handler 通常按请求构造并持有工具值。`TikvRuntime: Send + Sync` 是跨线程共享的静态约束。
- `ResponseWriter.data` 使用 `Box<dyn Any + Send>` 保存待序列化响应，`Map<T>` 用 `Vec<(String, T)>` 保持插入顺序；这支持 Go 稳定 JSON 断言，但类型正确性由状态服务适配层负责。
- `UrlValues` 用有序键值列表保留重复 query 参数，并区分 `?a=`（一个空值）和 `?a`（键存在但无值）；`count`、`contains`、`get` 服务于 MVCC 参数语义。
- `TestHandler.gc_is_running: AtomicU32` 是本文件唯一显式的可变并发状态。`handleGC` 以 `compare_exchange(0, 1, AcqRel/Acquire)` 抢占，结束后用 `Release` 清零。
- TiFlash POST 把 `region_count == flash_region_count` 视为可用，再调用 `update_table_replica`；GET 合并当前 schema 与历史删除/截断表信息。
- 表范围是 `[start, end)`：整表终点使用 `table_id.wrapping_add(1)`，索引终点使用 `index_id.wrapping_add(1)`，明确复现 Go `int64` 溢出行为。分区表由 `table_ranges_response` 为每个分区生成独立对象。
- 本文件中的许多业务类型标注为“占位”；它们只携带当前 HTTP 边界需要的字段。新增字段不能假设会自动出现在 wire JSON，必须同步检查 `pkg/server/http_status.rs` 的显式序列化分支。

## 依赖与调用关系

上游直接调用者是 `pkg/server/http_status.rs` 中的 `schema_response`、`table_response`、`region_*_response`、`mvcc_*_response`、`ddl_*_response`、`ingest_response`、`test_gc_response` 等适配函数。该文件把真实请求转换为本文件的 `Request`，也承担 `ResponseWriter` 的 JSON/错误转换。`pkg/server/server.rs` 暴露 `tikv_runtime()`，为这些适配函数提供 `Arc<dyn TikvRuntime>`。

下游主要通过 `TikvRuntime` 调用：schema/表解析、PD stats/address/region scan、MVCC 查询和解码、DDL owner/history/admin check、TiFlash 状态、profile、锁解析、标签、ingest 与 GC 状态均在 trait 中声明。唯一明确的跨 crate 直接调用是 `astersql_server_handler::util::ExtractTableAndPartitionName`；`Cargo.toml` 同时声明 config、DDL、domain、infoschema、kv、meta、session、store helper、tablecodec 等迁移依赖，反映 Go 包的目标子系统边界，但本文件当前并未直接导入其中大多数真实类型。

RustCodeGraph 能定位 `serve_table_http`、`serve_mvcc_txn_http` 和 `TikvRuntime`，但对这些经 `ServeHTTP` 门面、trait 和状态服务适配发生的调用未给出 caller/callee 边；调用关系因此由 `pkg/server/http_status.rs` 的直接构造与调用证据补齐。crate 内 `lib.rs` 通过 `pub use tikv_handler::*` 暴露这些入口。

## 错误处理与边界

- handler 使用提前返回：解析或运行时错误进入 `write_error`；`http_error` 同时设置状态码。最终状态码与错误体由外层适配器决定，不能仅从 `ResponseWriter.status` 推断所有错误一定是同一 HTTP code。
- 明确的方法限制包括 DDL resign/check、delete key、DDL hook、label 仅 POST，txn GC 仅 GET；ingest 仅 GET/POST。其余不支持的 `op` 返回清晰错误。
- `validate_setting_value` 复现布尔值集合、deadlock capacity `0..=10000`、transaction summary capacity `0..=5000`、digest duration `0..=i32::MAX`；`parse_go_bool` 接受 Go `strconv.ParseBool` 的大小写拼写。
- `parse_i64`/`base_zero_digits` 支持 `0x`、`0b`、`0o`、旧式前导零八进制和受限下划线；`parse_start_ts` 再按 Go 有符号解析后转 `u64`。无效 digit、下划线和溢出均返回错误。
- `base64_decode` 只忽略 CR/LF，严格拒绝其他空白、错误 padding、padding 后数据和非四字节结尾；`maybe_unescape` 支持 `+` 与 `%XX` 并拒绝不完整/非法转义。
- DDL history 的 `start_job_id` 必须大于零，`limit` 必须在 1 到 10；缺省值仍为 0，交由运行时采用默认行为。
- `collectRecordSetRows` 遇到行错误会标记当前 `RecordSet.closed` 并返回，但正常耗尽路径没有在此占位模型中显式 close；真实资源关闭责任需结合运行时适配器实现审查。

## 并发与资源生命周期

handler 自身大多是每请求短生命周期值；长生命周期能力由 `Arc<dyn TikvRuntime>` 管理。本文件不创建线程、异步任务、通道或显式锁，也不持有事务对象。`Send + Sync` 保证运行时可被状态服务并发共享，但具体锁、连接池、PD client 和事务生命周期均属于 trait 实现，而非本文件。

`TestHandler` 的 GC 互斥只保护同一个实例。当前 `pkg/server/http_status.rs::test_gc_response` 每次请求都以初始值 0 构造新 handler，因此若要依赖跨请求互斥，必须确认或调整上层实例生命周期；本文件的原子逻辑本身只保证共享实例内互斥。`handleGC` 在正常分派路径后恢复标志，但若未来分支引入 panic 或提前返回，显式清零不具备 RAII/defer 保证，应使用守卫保持不变量。

Go 版 scatter helper 会关闭 PD HTTP response body，session 分支会 `defer Close`，ingest 会开启事务；Rust 文件把这些动作移入 `TikvRuntime`，因此实现者必须在那里保证 response、session、事务和客户端资源释放。`serve_profile_http` 的 body 写失败只记录日志，不再向响应错误集合追加错误，这是与流式写出生命周期相关的特殊分支。

## 与 Go 版本的对应关系

直接对照文件是 [`tikv_handler.go`](./tikv_handler.go)。Rust 保留了 Go 的 handler 名称、构造器、操作常量、分派顺序和主要响应结构，并刻意复现以下可观察语义：空设置值不触发修改、非正 DDL slow threshold 不更新、query 中无 `=` 的键与空值不同、startTS 使用 base-0 有符号解析、表/索引 ID 后继使用 `int64` 回绕、TiFlash 可用性判断、GC 原子标志和 ingest 方法/范围检查。

二者实现层次并不相同。Go 文件直接使用 `kv.Storage`、InfoSchema、PD HTTP/client、session、DDL owner、meta transaction、tablecodec 和日志设施；Rust 文件将这些行为压缩到 `TikvRuntime`，并以内置轻量结构承接 wire 所需字段。比如 Go `TableHandler.ServeHTTP` 直接查 schema 和分区，Rust 先规范化分区名再调用 `resolve_table_route`；Go ingest handler 直接开启 KV 事务并更新进程全局原子值，Rust 通过 `ingest_get/ingest_set` 委托；Go txn GC 直接取得 PD GC client，Rust 委托 `gc_state`。因此“接口路径与外部行为已移植”不等于“Go 下游实现已全部位于此文件”。

另一个差异是 Go 类型 `FlashReplicaDeprecatedHandler` 在 Rust 中表现为 `FlashReplicaHandler`，且当前 Rust 状态服务也使用后者。扩展或比对时应按路由和响应行为对应，而不能只按类型名机械匹配。

## 扩展指南

- 新增 HTTP endpoint：先决定是否属于现有 handler 的新 `op`，或新增 handler；在本文件添加构造器、`ServeHTTP` 门面和最小解析逻辑，再在 `pkg/server/http_status.rs` 注册/适配真实路由与序列化。
- 新增外部副作用：优先扩展 `TikvRuntime`，并同步真实运行时实现及 `pkg/server/handler/tests/http_handler_serial_test.rs` 的 `SerialHandlerRuntime`，不要在 handler 中绕过抽象直接建立存储/PD 连接。
- 修改 wire 数据结构：同时检查 `ResponseWriter` 类型擦除值的 downcast/JSON 分支。字段只加入本文件结构但未加入 `tikv_response` 并不能保证客户端可见。
- 修改表/MVCC 编码：保持 TiDB comparable integer、半开区间、分区展开、base-0 数值和 query 重复值语义；同步独立测试 [`tikv_handler_test.rs`](./tikv_handler_test.rs)，不要把测试嵌回生产文件。
- 修改并发 GC：确保互斥状态跨实际请求共享，并用作用域守卫保证所有返回/异常路径复位。修改 ingest 时要在运行时保持元数据持久化与进程内限流值更新的一致性。
- 对齐 Go 新行为时，应逐个核验 `tikv_handler.go` 的输入校验、错误文本/状态、资源关闭和 response shape；不要仅添加返回成功的桩。兼容风险主要在 URL/数字编码、JSON 形状和错误码，性能风险主要在大 schema/region 列表、MVCC 解码及 `manualWriteJSONArray` 是否仍保持单数组且避免不必要复制。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `tikv_handler.rs`；`files --filter pkg/server/handler/tikvhandler` 确认源、Go 对照与独立测试均已索引；`query` 定位 `tikv_handler.rs::serve_table_http`（611 行）、`serve_mvcc_txn_http`（876 行）和 `TikvRuntime`（1519 行）；`node --file` 用于通读 2284 行源码。对关键函数执行 `callers/callees` 返回空边，因此未以图中不存在的动态/门面调用边作结论。
- Rust 源与 crate：[`tikv_handler.rs`](./tikv_handler.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)。上游接线补充读取 `pkg/server/http_status.rs` 与 `pkg/server/server.rs`。
- Go 对照：[`tikv_handler.go`](./tikv_handler.go)，重点核验 Settings、Table、MVCC、Test/GC、Ingest 和 TxnGCStates 的 `ServeHTTP` 及 `parseQuery`。
- 独立测试：[`tikv_handler_test.rs`](./tikv_handler_test.rs) 验证单数组响应、分区 ID 查找、非分区 range 形状、Go base-0 startTS、严格 base64 和 `int64` 回绕；`pkg/server/handler/tests/http_handler_serial_test.rs` 提供 `TikvRuntime` 替身并覆盖真实状态 TCP 路由及 txn GC 的 GET/405 行为。
- 本任务是纯文档分析，未运行 Cargo。结构验收由任务指定的 11 个固定二级标题检查完成；人工复核重点是区分本文件已实现的路由/校验逻辑与 `TikvRuntime` 下游责任。
