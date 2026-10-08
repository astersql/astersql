# `pkg/server/http_handler.rs`

## 文件定位

本文件属于 `astersql-server` crate，由 [`pkg/server/lib.rs`](lib.rs) 以公开模块 `http_handler` 装配。它位于 `Server` 与 status HTTP 路由器之间：从运行中的 `Server` 提取少量只读信息，维护一份按职责分类的 TiKV/status 路径清单，并为尚未接入真实实现的路由提供分类依据。真正的请求解析、路由匹配、TCP 监听和多数 handler 实现在 [`pkg/server/http_status.rs`](http_status.rs)；TiKV 业务 handler 则位于 `pkg/server/handler/` 的独立 crate 中。

该文件当前更接近“路由契约与构造参数快照”，不是 Go `http_handler.go` 中完整依赖对象的等价实现。特别是 `TikvHandlerTool` 只保存驱动名称与 Domain 可用性，并不持有 Go 版本所需的 TiKV `Storage`/`helper.Helper`。

## 核心职责

1. `HandlerKind` 将 status 路径归入 status、optimizer、settings、schema、DDL、DXF、ingest、region、MVCC 和 test 十类，并通过 `as_str` 提供稳定的人类可读名称。
2. `TikvHandlerTool::from_server` 对 `Server` 做轻量快照，记录 `ServerDriver::name()` 和 `Server::domain()` 是否已初始化；`validate` 可显式检查这两个前置条件。
3. `TikvHandlerTool::routes` 返回 Go status 表面中 schema、DDL、DXF、ingest、region、MVCC 与测试接口的路径模式和类别。`http_status::build_status_router` 消费该清单，给尚未被前序真实 handler 命中的路径注册分类化 503 handler。
4. `new_optimize_trace_handler` 与 `new_plan_replayer_handler` 从 `ServerConfig` 和 Domain 状态构造两个仅含地址、端口、可用性标志的参数对象，保留与 Go 构造入口对应的边界。

## 主要符号

- `pub enum HandlerKind`：可复制、可比较的路由类别。`Status` 与 `Optimizer` 虽不出现在 `TikvHandlerTool::routes` 返回项中，但由 `http_status.rs` 注册其他占位路径时直接使用。
- `HandlerKind::as_str(self) -> &'static str`：把类别映射为 503 JSON 文案中的名称；其中 `Dxf` 特意映射为 `"distributed framework"`，其余为短小写名称。
- `pub struct TikvHandlerTool { driver_name, domain_available }`：构造时刻的值快照，不借用 `Server`，也不拥有存储连接、锁或异步任务。
- `TikvHandlerTool::from_server(&Server) -> Self`：读取 `server.driver().name()` 并复制为 `String`，读取 `server.domain().is_some()` 为布尔值。
- `TikvHandlerTool::validate(&self) -> Result<(), String>`：先拒绝空驱动名，再拒绝未初始化 Domain；成功时返回 `Ok(())`。当前仓库调用搜索未发现生产调用者。
- `TikvHandlerTool::routes(&self) -> Vec<(&'static str, HandlerKind)>`：每次调用创建路径清单；路径含 `{db}`、`{table}`、`{regionID}` 等模板段。该方法当前不根据 `driver_name` 或 `domain_available` 过滤路由。
- `OptimizeTraceHandler`、`PlanReplayerHandler`：字段相同的公开配置快照，均包含 `advertise_address: String`、`status_port: u16`、`domain_available: bool`；它们没有 `handle`/`ServeHTTP` 方法。
- `new_optimize_trace_handler(&Server)`、`new_plan_replayer_handler(&Server)`：分别构造上述快照。RustCodeGraph 精确查询未发现调用者，实际 plan-replayer 下载在 `http_status.rs` 中由另一条实现链处理。

文件没有 trait、模块级常量、全局可变状态或条件编译项；所有声明均为公开符号，只有各类型的字段与纯同步方法。

## 执行流程

status 服务的相关主流程如下：

1. `Server::start_status_http` 在启用 `StatusConfig::report_status` 后构建 status router 并启动监听线程，入口位于 `pkg/server/http_status.rs`。
2. `build_status_router` 先注册已有的真实 handler，包括健康状态、指标、部分 schema/DXF/ingest/plan-replayer 等端点。
3. 随后调用 `TikvHandlerTool::from_server(&server)`，再遍历 `tool.routes()`，对每个路径执行 `router.add(path, unavailable(kind))`。
4. `Router::handle_from` 按注册顺序选择第一条同时满足方法和路径的路由。因此同一路径若已有真实 handler，前序实现优先；清单中的后序条目只为未接线路径兜底。
5. 兜底 handler 调用 `HandlerKind::as_str`，返回 HTTP 503 和形如 `{"error":"schema handler is not configured"}` 的 JSON。

这条运行链没有调用 `TikvHandlerTool::validate`。因此空驱动名或未设置 Domain 不会在清单注册阶段报错；具体真实 handler 会自行检查所需的 `Domain`/runtime，未接线端点则落到 503。

两个优化器构造函数的流程更短：读取 `server.config()`，复制 `config.host` 和 `config.status.port`，再以 `server.domain().is_some()` 记录 Domain 状态。注意字段名虽为 `advertise_address`，当前 Rust 实现取的是 `ServerConfig::host`，不是 `ServerConfig::advertise_address`。

## 数据与状态

- `HandlerKind` 是无载荷枚举，类别本身不携带方法限制、handler 或运行时依赖。
- `routes` 的路径均为 `'static` 字符串，返回的 `Vec` 归调用者所有。清单是编译期写死的兼容表，每次调用重新分配向量；没有缓存或修改入口。
- `TikvHandlerTool`、`OptimizeTraceHandler`、`PlanReplayerHandler` 都是 `Clone + Debug` 的普通值对象。它们只在构造时读取 `Server`，后续 Domain 初始化或配置变化不会反映到已有对象中。
- `Server::domain()` 内部从 `RwLock<Option<Arc<dyn Domain>>>` 克隆当前值；本文件只把 `is_some()` 结果保留下来，不延长 Domain 生命周期。
- 路由模板中的变量名只表达注册契约；实际提取和校验由 `http_status.rs` 及下游 handler 完成，本文件不解析 URL、查询参数、请求体或 HTTP 方法。

## 依赖与调用关系

上游装配与调用：

- `pkg/server/lib.rs` 公开 `http_handler` 模块。
- `pkg/server/http_status.rs::build_status_router` 是已确认的生产调用者：调用 `TikvHandlerTool::from_server` 和 `routes`，并直接使用多个 `HandlerKind` 变体生成分类化 503。
- RustCodeGraph 对 `new_optimize_trace_handler`、`new_plan_replayer_handler` 的精确查询各只找到定义，未找到调用者；`validate` 的仓库搜索也未发现本文件对象上的生产调用。

下游依赖：

- 唯一源码导入是 `crate::server::Server`。本文件经 `Server::driver` 使用 `ServerDriver::name`，经 `Server::domain` 判断 Domain 是否存在，经 `Server::config` 读取 host/status port。
- `pkg/server/Cargo.toml` 声明本 crate 为 `astersql-server`、库入口为 `lib.rs`、`autotests = false`，并依赖 `astersql-server-handler`、`astersql-server-handler-optimizor`、`astersql-server-handler-tikvhandler` 等相邻 handler crate；但本文件本身没有直接导入这些外部 crate。
- 实际消费 `HandlerKind` 的 `unavailable` 和实际路由表 `Router` 都在 `pkg/server/http_status.rs`。该路由表由 `Arc<Mutex<Vec<Route>>>` 保存，并按注册顺序匹配。

## 错误处理与边界

- `validate` 只覆盖两个结构性错误，并保留固定错误文本：空驱动名为 `invalid key-value store driver`，Domain 缺失为 `server domain is not initialized`。若两者同时非法，驱动错误优先。
- `from_server` 自身不返回 `Result`：它假定 `Server` 总有驱动，只把 Domain 缺失编码为 `false`。调用 `server.domain()` 时若其内部锁中毒，`Server` 的访问器会 panic；本文件不捕获该异常。
- `routes` 不验证配置、存储类型、keyspace、kernel type 或请求方法。Go `startHTTPServer` 中部分路由受 `IsSystemKS`、`IsNextGen` 或 TiKV store 类型约束，Rust 清单本身没有这些条件，真实接线条件必须在 `http_status.rs` 或具体 handler 中维持。
- 兜底错误由 `http_status.rs::unavailable` 产生，而非本文件直接生成。`HandlerKind::as_str` 是错误文案兼容性的一部分，修改会改变外部可见 JSON。
- 当前清单不是 Go 路由的完整全集。例如 Go 代码还包含 `/dxf/nodes` 与 `/dxf/schedule/task_cleanup_batch_size`；不能把 `routes` 当作全部 status API 的权威列表，应与 `build_status_router` 的其他显式注册合并理解。

## 并发与资源生命周期

本文件不创建线程、future、channel、锁、事务或网络资源。所有方法都是短生命周期的同步读取与值构造。

`from_server` 和两个 handler 构造函数只通过 `Server` 的线程安全访问器读取共享状态。生成的字符串和布尔值与 `Server` 解耦，所以可跨线程移动/克隆，但也意味着它们是瞬时快照。路由闭包的并发共享、路由锁以及 status 监听线程的启动/关闭由 `http_status.rs` 管理；具体 Domain、TiKV runtime 和存储资源的生命周期不由这里拥有。

## 与 Go 版本的对应关系

- Go `pkg/server/http_handler.go::(*Server).NewTikvHandlerTool` 会断言 driver 是 `*TiDBDriver`、底层 store 实现 `helper.Storage`，失败即 panic，然后构造持有真实 helper/storage 的 `handler.TikvHandlerTool`。Rust 同名概念只是 `driver_name + domain_available` 快照，并提供尚未用于生产接线的 `validate`；两者能力不等价。
- Go `newOptimizeTraceHandler` 从全局配置读取 `AdvertiseAddress`/status port，并从 Domain 提取可选 `InfoSyncer`。Rust 版本只保留地址、端口、Domain 是否存在，且当前地址来源是 `config.host`。
- Go `newPlanReplayerHandler` 还传入 `InfoSchema`、statistics handle 与 `InfoSyncer`。Rust 版本没有这些对象，只记录 Domain 可用性；实际 Rust plan-replayer 下载链位于 `http_status.rs` 和全局 ext storage。
- `routes` 主要镜像 Go `pkg/server/http_status.go::startHTTPServer` 的路径表，但 Rust 将路径与职责类别分离，供占位注册使用；Go 则直接为每条路径绑定具体 handler，并含 system keyspace、NextGen、store type 等条件分支。
- 相关 Go 行为回归集中在 `pkg/server/handler/tests/http_handler_test.go`、`http_handler_serial_test.go` 和 `dxf_test.go`。对应 Rust 测试位于同目录的独立 `*_test.rs` 文件，符合测试与生产源文件分离要求。

## 扩展指南

- 新增 status 路径时，先判断它是否已有真实 Rust handler。真实 handler 应优先在 `pkg/server/http_status.rs::build_status_router` 注册；若仍需兼容占位，再把路径加入 `TikvHandlerTool::routes` 并选择准确的 `HandlerKind`。注意注册顺序决定同路径的实际处理者。
- 新增职责类别时，需要同步更新 `HandlerKind` 与 `as_str`，并检查所有 503 文案消费者。新增枚举变体会影响穷尽匹配，类别字符串会影响外部错误契约。
- 若要把 `TikvHandlerTool` 提升为 Go 等价的真实依赖载体，不应仅扩充布尔字段；应复用 `Domain::tikv_runtime` 和既有 `astersql-server-handler-*` 类型，并在 `http_status.rs` 完成最小接线，避免与其他同名 `TikvHandlerTool` 类型混淆。
- 若启用两个优化器构造器，需要先补齐其消费者与 Go 所需的 InfoSyncer/InfoSchema/statistics 能力，或明确它们仍只是可用性快照；同时审视 `host` 与 `advertise_address` 的选择。
- 测试应放在独立文件：本 crate 的路由/监听契约优先扩展 `pkg/server/http_status_test.rs`；具体 schema、DDL、DXF、region、MVCC 行为扩展 `pkg/server/handler/tests/http_handler_test.rs`、`http_handler_serial_test.rs` 或 `dxf_test.rs`。至少覆盖真实 handler 优先于占位、未接线路由返回分类化 503、条件路由与 Go 保持一致。
- 兼容风险主要是路径增删、模板拼写、注册顺序和错误文案；性能风险较低，但 `routes` 每次调用都会分配新向量，若未来从启动期调用改为逐请求调用应改用静态清单。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件显示完整 203 行和 11 个符号。
- 源码与模块边界：`pkg/server/http_handler.rs`、`pkg/server/lib.rs`、`pkg/server/server.rs`、`pkg/server/http_status.rs`、`pkg/server/Cargo.toml`。
- Go 对照：`pkg/server/http_handler.go`（三个构造入口）与 `pkg/server/http_status.go::startHTTPServer`（真实路由及条件分支）。
- 独立测试证据：`pkg/server/http_status_test.rs` 覆盖真实 TCP status 服务、schema 路由和 plan-replayer 下载；`pkg/server/handler/tests/http_handler_test.rs`、`http_handler_serial_test.rs`、`dxf_test.rs` 覆盖 schema/settings/DDL/DXF/region/MVCC 等 status 表面。未发现与 `http_handler.rs` 同名的独立 Rust 测试。
- 调用边：`http_status.rs::build_status_router -> TikvHandlerTool::from_server -> Server::{driver,domain}`，以及 `build_status_router -> TikvHandlerTool::routes -> unavailable -> HandlerKind::as_str`。精确图查询未发现两个 `new_*_handler` 的调用者。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；验收使用任务指定的 11 章节结构检查，并人工核对本文没有把占位清单或未接线构造器写成已实现业务能力。
