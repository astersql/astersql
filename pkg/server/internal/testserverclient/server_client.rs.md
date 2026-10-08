# `pkg/server/internal/testserverclient/server_client.rs`

## 文件定位

本文件是 `astersql-server-internal-testserverclient` crate 的主体实现，crate 根模块在 [`lib.rs`](lib.rs) 中声明 `server_client` 并再导出主要类型与辅助函数。它服务于 server 层集成测试：一端描述如何连接已启动的 AsterSQL/TiDB 测试服务器（MySQL 协议端口及 status HTTP 端口），另一端把 Go `server_client.go` 中的回归场景移植成可注入、可由 Rust 测试替身或真实 session 执行的逻辑。

该文件不是线上 server 请求处理链的一部分，也不是单纯的 mock。实际调用者位于 server 测试设施和测试用例，例如 `pkg/server/tests/servertestkit/testkit.rs` 用它生成 DSN、等待测试服务器上线，`pkg/server/tests/commontest/tidb_part4_aster_unit_test.rs` 调用 `run_named_scenario`，`pkg/server/tests/standby/standby_test.rs` 与 `pkg/server/handler/tests/http_handler_test.rs` 调用就绪探测。

[`Cargo.toml`](Cargo.toml) 将 crate 的 Go 对照包标为 `pkg/server/internal/testserverclient`，入口为 `lib.rs`。manifest 列出的 `astersql-server`、`astersql-testkit` 等 workspace 依赖属于整个 crate；本文件本身只直接使用 Rust 标准库，SQL/server 实现通过本文件定义的 trait 注入。

## 核心职责

- 连接描述与探测：`TestServerClient` 保存 host、MySQL/status 端口、scheme 和超时配置；`get_dsn*` 生成 go-sql-driver 风格 DSN；`fetch_status`、`post_status`、`form_status` 发起最小 HTTP/1.1 请求；`wait_until_server_*` 轮询 TCP 与 `/status`。
- 场景驱动：`run_tests`、`run_tests_on_new_database`、`run_named_scenario` 提供通用编排；大量 `run_test_*` 方法逐项承载 Go 文件的回归、prepared statement、LOAD DATA、认证、错误码、指标、TLS、DDL schema-state 等测试语义。
- 驱动解耦：`SqlExecutor`、`ServerConnection`、`ServerConnector`、`SchemaStateController` 把 SQL、独立连接、协议能力及 DDL 特定状态抽象出来，使同一场景可在记录型替身和真实 session 上运行。
- 结果及协议辅助：`SqlValue`、`QueryResult`、`ExecuteResult` 统一结果模型；`rows`、`check_rows`、列名/指标/错误检查函数提供断言语义；私有 HTTP、chunked、Prometheus 与 URL 编码函数避免测试场景依赖额外客户端库。
- Go 覆盖跟踪：`GO_SCENARIO_NAMES` 固定记录 Go 侧 41 个 `RunTest*` 入口，`REGRESSION` 与访问函数提供进程级回归开关。

## 主要符号

- `MysqlConfig` / `MysqlConfig::format_dsn`：保存用户、口令、network/address、数据库、普通参数及 `allowAllFiles`、collation、multi-statements、TLS、packet size、cursor fetch size 等特殊项。数据库名使用 `percent_encode`，query 值使用 Go `url.QueryEscape` 语义的 `query_escape`；普通参数按键排序以获得稳定字符串。
- `HttpResponse`：保存 status、reason、小写 header map 与原始 body；`text` 严格按 UTF-8 解码，`is_success` 只接受 2xx。
- `TestServerClient`：核心客户端。`new` 默认 `http://localhost`、两个端口为 0、连接超时 1 秒、重试间隔 10 毫秒；端口必须由测试启动器填充。`addr` 的 scheme 来自 `status_scheme`，主要用于测试地址表达，并不表示本文件实现 MySQL TLS transport。
- `ConfigOverrider`：`Fn(&mut MysqlConfig) + Send + Sync` 回调别名；`get_dsn_optional` 通过 `flatten` 跳过 `None`，对应 Go 可传 nil overrider 的行为。
- `SqlExecutor`：最小 SQL 边界，要求 `execute`、`query`，可选 `fork` 默认返回“不支持独立 session”。事务并发场景必须由实现者覆盖 `fork`。
- `ServerConnection` / `ServerConnector`：在 `SqlExecutor` 上增加 ping、prepare、send-long-data、execute/close prepared，以及按 `MysqlConfig` 建新连接的能力，供认证、init-connect 和协议级类型场景使用。
- `SchemaStateController`：在 DDL 进入 write-reorganization 时执行回调，支撑 issue 53634/54254 的事务可见性场景。
- `TestDatabase<'a>`：对借用的 `SqlExecutor` 提供 `must_execute`、`must_query` 薄包装；错误仍以 `Result<_, String>` 返回。
- `Scenario` / `ScenarioStep`：11 个小型命名场景及静态 `Execute`、`Query { expected }` 步骤。它们是通用快速场景集合，不等于所有 `run_test_*` 的完整覆盖。
- `SqlValue` / `QueryResult` / `ExecuteResult`：跨驱动的值与结果载体。`SqlValue::display` 将 `NULL` 显示为 `<nil>`、布尔显示为 `0/1`、字节用有损 UTF-8 展示，与行断言接口衔接。
- `GO_SCENARIO_NAMES`、`regression_enabled`、`set_regression_enabled`：分别维护 Go 入口清单及使用 Acquire/Release 原子序的全局开关。

## 执行流程

1. 测试启动器构造 `TestServerClient::new`，填入实际 MySQL 与 status 端口。需要 SQL 驱动连接时，`mysql_config` 先生成默认 `root@tcp(127.0.0.1:<port>)/test` 配置，`get_dsn*` 顺序应用覆盖回调后由 `format_dsn` 输出字符串。
2. `wait_until_server_online` 先调用 `wait_until_server_can_connect` 验证 MySQL TCP 端口；仅此步骤成功后，才在同一个截止期限内重复 `GET /status`，收到 2xx 才返回成功。status 单独可用不能掩盖 SQL 端口未就绪。
3. HTTP 路径由 `fetch_status`/`post_status`/`form_status` 汇入 `http_request`。后者解析地址、按 `connect_timeout` 连接并设置读取超时，写出带 `Connection: close` 与 `Content-Length` 的 HTTP/1.1 请求，half-close 写端，读到 EOF，再由 `parse_http_response` 解析；chunked body 交给 `decode_chunked_body`。
4. 简单命名场景由 `Scenario::steps` 返回静态步骤，`run_named_scenario` 顺序调用 executor；查询步骤立即通过 `check_rows` 做顺序敏感的二维结果比较，任一步失败即停止。
5. 通用闭包场景由 `run_tests` 共享一个 executor；`run_tests_on_new_database` 先 drop/create/use 临时库，每个闭包成功后清理 `test` 表，并在正常或出错路径上都尝试 drop 临时库。最终 `result.and(cleanup)` 保持原始场景错误优先，只有主体成功时才暴露清理错误。
6. 专项 `run_test_*` 直接编排 SQL 和断言：基础类型与 prepared 场景检查元数据和值；LOAD DATA 系列覆盖 outfile 往返、slow log/statement summary、auto-random 校验和、分区 warning、column list、事务提交/回滚与锁冲突；认证/init-connect/SQL mode 场景通过 connector 创建独立会话；send-long-data 使用协议扩展；DDL issue 场景通过 schema-state controller 在指定阶段运行事务动作。
7. status/metrics 场景通过 `/status` 校验 version/git hash，通过 `/metrics` 解析 Prometheus samples，并比较 workload 前后的 statement counter；`run_test_db_stmt_count` 明确保持 Go 侧无条件 skip 的现状，当前只返回成功而不执行 workload。

## 数据与状态

- `TestServerClient` 是可克隆的纯配置值；网络连接不保存在结构体中，每次 HTTP/TCP 探测都创建临时 `TcpStream`。
- `MysqlConfig.parameters` 与 HTTP headers/metrics 使用 `HashMap`。DSN 与 form 输出前显式排序，避免哈希遍历顺序导致测试不稳定；解析后的 headers 统一为小写。
- `ScenarioStep` 引用 `'static` SQL 与期望行，运行时不分配场景定义；复杂场景则直接写在相应 `run_test_*` 方法中。
- `REGRESSION: AtomicBool` 默认开启，读取使用 `Ordering::Acquire`、写入使用 `Ordering::Release`。它只是本文件暴露的开关；调用者决定是否据此跳过回归场景。
- `TEMP_FILE_SEQUENCE: AtomicU64` 以 Relaxed 自增，与进程 id 一起组成临时文件名，目标是同进程并发测试下减少碰撞，不承担跨进程唯一性或安全临时文件保证。
- LOAD DATA 临时文件由 `unused_temp_path`、`create_temp_file`、`prepare_load_data_*` 管理。路径会先清除同名残留；`finish_file_cleanup` 在场景完成后删除，并与主体 `Result` 合并。
- 多会话事务测试通过 `SqlExecutor::fork` 获得指向同一 server/store 的独立 session。若 executor 没有实现该能力，默认错误是预期边界，而非自动降级为共享会话。

## 依赖与调用关系

上游调用关系（直接源码搜索证据）：

- `pkg/server/tests/servertestkit/testkit.rs` 持有 `TestServerClient`，代理 `get_dsn`，启动 server 后调用 `wait_until_server_online`。
- `pkg/server/tests/standby/standby_test.rs` 与 `pkg/server/handler/tests/http_handler_test.rs` 配置端口并等待 server 在线。
- `pkg/server/tests/commontest/tidb_part1_aster_unit_test.rs` 直接生成 DSN；`tidb_part4_aster_unit_test.rs` 将 executor 交给 `run_named_scenario(Scenario::ConnectionCount)`。
- `pkg/server/handler/optimizor/plan_replayer_test.rs` 构造该客户端作为 server 测试辅助。

本文件的主要下游边：

- DSN 链：`get_dsn` / `get_dsn_optional` / `get_dsn_with_cursor` → `mysql_config` → `MysqlConfig::format_dsn` → `percent_encode` / `query_escape`。
- status 链：`fetch_status` / `post_status` / `form_status` → `http_request` → `parse_http_response` →（按 header）`decode_chunked_body`；`get_metrics` 再调用 `HttpResponse::text` 与 `parse_prometheus`。
- 场景链：`run_named_scenario` → `Scenario::steps` → `SqlExecutor::{execute,query}` → `check_rows` → `rows` → `SqlValue::display`。
- 复杂场景还依赖注入的 `ServerConnector`、`ServerConnection`、`SchemaStateController`，而非直接依赖具体 driver 或 server 类型。

RustCodeGraph 将目标文件、Go 文件、测试文件和 `lib.rs` 均纳入索引，并能定位 `TestServerClient`、`run_named_scenario`、`wait_until_server_online`、`get_dsn` 等符号。图的 `callers/callees` 命令在本次查询中超时，因此上述调用边由已索引符号位置和仓库内直接调用点搜索共同核验；不能把该超时理解成没有调用关系。

## 错误处理与边界

- 对外操作统一返回 `Result<_, String>`，并在 DNS、connect、timeout 设置、请求写入、响应读取、临时文件及解析错误前添加操作上下文；没有自定义错误类型或错误源链。
- `http_request` 只实现明文 `http`。若 `status_scheme != "http"`，立即要求调用方注入外部 TLS transport；路径必须以 `/` 开头。解析器是测试用途的最小实现：要求 `\r\n\r\n`，支持普通 EOF body 和 chunked body，但不实现重定向、压缩、keep-alive 复用或完整 RFC trailer 处理。
- `wait_until_custom_server_can_connect` 与 online 检查都受调用方 timeout 限制，失败报告最后一次错误；地址解析失败直接返回，不进入重试。`wait_until_server_online` 在 SQL 端口阶段花费的时间计入同一 deadline，status 阶段不会重新获得完整 timeout。
- `check_rows` 比较完整行列值及顺序；`columns_as_expected` 也顺序敏感。字节经 `from_utf8_lossy` 展示，若测试必须精确验证非 UTF-8 字节，应使用协议/值级断言而不是字符串行辅助。
- `expect_mysql_error` 假设错误字符串以 `Error <number> ` 开头；连接错误场景要求完整字符串精确相等。这是对当前测试 adapter 格式的契约，改变 adapter 错误文本时需同步测试。
- metrics 查找找不到样本时返回 `0.0`，会把“指标缺失”和“实际为零”合并；调用者通常通过前后差值和已知 workload 约束它。`parse_prometheus` 仅接受末尾空白分隔的数值样本。
- `run_tests_on_new_database` 对数据库名只用反引号包裹，没有在内部转义反引号；调用方应传测试控制的合法名称。
- Go 中显式跳过的不稳定 DB statement-count 场景在 Rust 中仍是 no-op 成功；文档和测试不得把它描述为已验证指标行为。

## 并发与资源生命周期

- 客户端结构本身没有锁，且网络流均为方法内局部资源；`TcpStream` 成功探测后显式 shutdown，HTTP 请求写完后 half-close 并读至 EOF。
- `SqlExecutor: Send` 允许 executor 跨线程所有权移动，但方法接收 `&mut dyn SqlExecutor`，单个场景内仍是串行、独占访问。文件没有启动后台线程；唯一的 `thread::sleep` 用于轮询退避。
- `run_test_load_data_in_transaction` 用 `fork` 创建独立 session 来表达真实锁与可见性，而不是并行共享一个可变 executor。相关测试实现负责保证这些 session 指向相同 server/store。
- 临时文件路径含 pid 与原子序号，生成/写入后贯穿对应 LOAD DATA 场景。多数路径以 `finish_file_cleanup` 合并清理结果；若进程被强制终止则无法保证清理，后续同名路径会由 `unused_temp_path` 尝试删除残留。
- `run_tests_on_new_database` 在闭包失败时仍执行最终 `DROP DATABASE`；各复杂方法也通常先保存主体结果，再清理表、变量或文件。扩展场景时应保持这种“主体失败仍恢复环境”的结构。
- 独立 Rust 测试中的真实 SQL 场景使用 `REAL_SQL_TEST_LOCK: Mutex<()>` 串行化 canonical mock store，说明真实执行后端存在测试间共享状态，新增真实场景测试应复用该隔离策略。

## 与 Go 版本的对应关系

Go 对照文件是 [`server_client.go`](server_client.go)，Rust 独立测试是 [`server_client_test.rs`](server_client_test.rs)。`Cargo.toml` 的 `package.metadata.porting.go-package` 也明确指向同一 Go 包。

- `TestServerClient`、`NewTestServerClient`、`Addr`、`StatusURL`、HTTP 方法、`GetDSN*`、`RunTests*` 与 `WaitUntilServer*` 均有直接 Rust 对应；Rust 以 `Result` 传播错误，而 Go 多通过 `testing.T`/testkit 的 must 风格终止测试。
- Go 使用 `database/sql`、mysql driver、`http.Client` 和具体 server/testkit；Rust 将这些能力拆为 trait，使移植逻辑可被记录型 executor 与 canonical session 验证。Rust 的手写 HTTP 客户端只覆盖本测试所需的明文子集。
- `GO_SCENARIO_NAMES` 按 Go 文件顺序列出 41 个 `RunTest*`，独立测试 `go_run_test_inventory_is_complete_and_ordered` 检查清单。多数入口在 Rust 中是同名 snake_case 方法；部分能力需要额外注入 connector/schema controller。
- Rust 的 `Scenario` 只抽取 11 个可静态描述的小场景；Go 的完整行为主要对应各 `run_test_*` 方法，不能仅凭 `Scenario` 枚举判断移植覆盖率。
- Rust 测试覆盖 DSN 编码与 nil overrider、HTTP chunked/form、online 顺序、临时库清理、行列语义、LOAD DATA 全系列、错误码、认证、指标、send-long-data、DDL schema-state，并用真实解析/规划/执行链复核若干基础场景。
- 已知保留差异是 `run_test_db_stmt_count`：Go 源因不稳定而 `t.Skip`，Rust 明确 no-op 返回 `Ok(())`，且测试 `db_statement_count_remains_explicitly_skipped_like_go` 固化该状态。

## 扩展指南

- 新增普通 SQL 场景：若只需静态 execute/query 序列，可扩展 `Scenario` 与 `Scenario::steps`，并在独立 `server_client_test.rs` 添加顺序、期望结果和真实执行测试；若有资源、连接或阶段控制，应新增聚焦的 `run_test_*`，不要把动态生命周期塞进静态步骤。
- 新增 Go 对齐入口：同步更新 `GO_SCENARIO_NAMES`、Rust 方法、Go 对照说明与 inventory 测试，逐项保留 SQL 顺序、错误码、清理和 skip 语义。不能用简化 stub 代替 Go 中实际断言。
- 新增驱动能力：优先在最窄 trait 上扩展。普通 SQL 放在 `SqlExecutor`；协议 prepared/long-data 放在 `ServerConnection`；建连行为放在 `ServerConnector`；DDL 阶段挂钩放在 `SchemaStateController`。同步更新同目录独立测试替身，测试逻辑不要放回生产源文件。
- 扩展 HTTP：若需要 HTTPS、重定向、压缩或连接复用，应引入/注入清晰的 transport 边界，而不是让当前极简 parser 假装完整 HTTP client；保持 status path、timeout 与 body 上限的安全约束。
- 扩展 LOAD DATA：沿用唯一临时路径和 `finish_file_cleanup` 模式，确保 SQL/断言失败后仍恢复全局变量、表、事务与文件；涉及多 session 时要求 executor 明确支持 `fork`。
- 修改值展示或错误格式时，评估 `check_rows`、`mysql_error_number`、精确连接错误及所有 adapter；非 UTF-8 数据应使用 `SqlValue::Bytes` 的字节级测试，避免有损展示掩盖差异。
- 性能方面，本工具偏重确定性和可诊断性而非吞吐；避免在高频轮询中加入无界分配或无 timeout I/O，metrics/HTTP body 若用于非测试环境则必须增加大小限制。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/server/internal/testserverclient` 返回 `lib.rs`、`server_client.rs`、`server_client.go`、`server_client_test.rs`；`query` 定位了 Rust/Go `TestServerClient` 及 Rust `run_named_scenario`、`wait_until_server_online`、`get_dsn`；`node --file ... --offset 1 --limit 260` 核验文件开头的配置、DSN、HTTP 与客户端实现。`callers/callees` 查询超时并被中止，调用边改由直接调用点搜索验证。
- 已读实现与边界：`pkg/server/internal/testserverclient/server_client.rs`、同目录 `lib.rs` 与 `Cargo.toml`；该目录及上级没有 `doc.go`，因此无额外 package contract 可读。
- Go 对照：`pkg/server/internal/testserverclient/server_client.go`，核对构造、DSN/status、41 个 `RunTest*`、等待逻辑、metrics 与明确 skip 的场景。
- 独立 Rust 测试：`pkg/server/internal/testserverclient/server_client_test.rs`，核对 HTTP/DSN/online、失败清理、LOAD DATA、认证/错误码、指标、协议字节、DDL 状态及 canonical store 串行测试。
- 上游调用点：`pkg/server/tests/servertestkit/testkit.rs`、`pkg/server/tests/standby/standby_test.rs`、`pkg/server/tests/commontest/tidb_part1_aster_unit_test.rs`、`pkg/server/tests/commontest/tidb_part4_aster_unit_test.rs`、`pkg/server/handler/tests/http_handler_test.rs`、`pkg/server/handler/optimizor/plan_replayer_test.rs`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务文件规定的 11 章节结构命令，并检查 Markdown 只引用真实路径、没有把测试工具描述成线上 server 主链。
