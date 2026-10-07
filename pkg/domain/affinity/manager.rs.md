# `pkg/domain/affinity/manager.rs`

## 文件定位

`manager.rs` 是 `astersql-domain-affinity` crate 的 Affinity Group 生命周期管理核心。crate 入口 `pkg/domain/affinity/lib.rs` 公开重导出本文件和 `interface.rs`；`pkg/domain/affinity/Cargo.toml` 又以 `go-package = "pkg/domain/affinity"` 明确它对应同目录 Go 包。文件本身不负责生成表/分区 key range，也不直接拼装 HTTP 请求，而是在上层业务与 `PdClient` 之间定义稳定的创建、删除、查询语义以及旧版 PD 的兼容回退。

运行时有两种接入方式。包级 API `pkg/domain/affinity/interface.rs` 把 `new_pd_manager` 或 `new_mock_manager` 存入进程级 `PackageState`，随后由 `create_groups_if_not_exists`、`delete_groups`、`get_groups` 分派；当前会话建表资源路径 `pkg/session/runtime/create_table_resources.rs` 也会在非 mock storage 下直接构造 `PdManager`，为建表创建 affinity groups，并在删除资源时清理它们。真实网络落点是 `pkg/domain/affinity/http_client.rs` 的 `HttpClient: PdClient` 实现。

## 核心职责

- 用 `Manager` trait 固定创建（若不存在）、强制删除、按 ID 查询三项上层能力，并用 `PdClient` trait 隔离实际 PD HTTP 客户端。
- 让创建具有重试友好的幂等语义：先尝试支持 `skip_exist_check` 的新接口；遇到可识别的旧版/兼容性错误时，先查询已有组，只创建缺失项。
- 保持按 ID 查询的结果语义稳定：即使旧 PD 忽略 `ids` 参数或不支持该接口，也只向调用者返回请求的 ID；请求过大时主动改为“拉取全部再过滤”。
- 提供线程安全的 `MockManager`，供测试和未配置 PD 的包级状态使用；其重复创建不会覆盖已有状态。
- 把错误兼容判定、URL 编码长度预算和结果过滤拆成可独立验证的纯函数。

这些职责由 `PdManager::{create_affinity_groups_if_not_exists, delete_affinity_groups, get_affinity_groups}`、辅助函数 `should_*` / `filter_affinity_groups`，以及 `MockManager` 的同名 trait 实现共同承担。

## 主要符号

- `MAX_AFFINITY_GROUP_IDS_QUERY_LEN = 4096`、`MAX_AFFINITY_GROUP_IDS_COUNT = 100`：按 ID 查询的 URI 长度和 ID 数量上限；判断使用严格大于，因此恰好达到上限仍可直查。
- `AffinityGroupKeyRange { start_key, end_key }`：一个半开区间 `[start_key, end_key)`；`new` 复制传入字节，所有权与调用者解耦。
- `AffinityGroup` 与 `AffinityGroupState`：分别保存 group ID，以及 ID 加 `range_count` 的运行时查询结果。
- `AffinityError`：保存可显示的错误消息和 `http_service_error` 标记。`http_status` 生成可解析的 PD HTTP 状态消息，`http_service` 标记无状态码服务错误，`status_code` 委托给 `extract_pd_http_status_code`。
- `Context` / `BackgroundContext`：暴露取消状态与可选绝对截止时间，作为 Go `context.Context` 的最小边界；manager 只向下传递，真正的中断行为由 `PdClient` 决定。
- `PdClient`：PD 边界，包含创建、强制批量删除、按 ID 查询、全量查询。创建方法显式接收 `skip_exist_check`。
- `Manager`：上层生命周期接口；`Send + Sync` 使 trait object 可在线程间共享。
- `PdManager { client: Arc<dyn PdClient> }`：真实实现；`new` 保留共享 client，`new_pd_manager` 将其擦除为 `Arc<dyn Manager>`。
- `affinity_group_ids_escaped_query_len`、`should_use_get_all_affinity_groups`：用 `url::form_urlencoded::Serializer` 按真实 `ids=<value>` 形式计算查询串，并选择直查或全量扫描。
- `extract_pd_http_status_code`、`is_pd_http_status_error`、`is_pd_http_service_error_without_status`、`should_fallback_*`：从错误中识别兼容性回退条件。
- `filter_affinity_groups`：只保留请求 ID、忽略不存在项，并借助 `HashSet` 去重。
- `MockManager { groups: RwLock<HashMap<...>> }`：内存实现；`new_mock_manager` 返回共享 trait object。

本文件没有条件编译项；测试通过 `pkg/domain/affinity/lib.rs` 中的 `#[cfg(test)]` 独立模块接入，而不是内嵌在生产文件中。

## 执行流程

创建路径 `PdManager::create_affinity_groups_if_not_exists`：

1. 空 map 直接成功，不访问 client。
2. 首次调用 `PdClient::create_affinity_groups(ctx, groups, true)`，请求 PD 跳过已存在检查。
3. 成功即结束；400、409 或标记为无状态码 HTTP 服务错误时，进入 `create_affinity_groups_if_not_exists_by_filtering`；其他错误原样返回。
4. 回退路径先排序 group ID，调用 manager 自身的 `get_affinity_groups`。这一步仍可触发查询侧的全量扫描兼容逻辑。
5. 从输入中移除已存在 ID；若没有缺失项则成功，否则以 `skip_exist_check = false` 创建缺失集合。

删除路径 `PdManager::delete_affinity_groups` 对空列表直接成功，否则调用 `batch_delete_affinity_groups(ctx, ids, true)`；重试不在本文件内，而在 `interface.rs::delete_groups_with_retry`。

查询路径 `PdManager::get_affinity_groups`：

1. 空 ID 列表返回空 map。
2. ID 数量超过 100，或 form-urlencoded 查询串超过 4096 字节时，直接调用 `get_affinity_groups_by_scanning_all`。
3. 否则调用 `PdClient::get_affinity_groups`。成功结果仍经 `filter_affinity_groups` 过滤，防止旧 PD 忽略查询参数后返回额外 group。
4. 400、404、414 或无状态码 HTTP 服务错误改走全量查询再过滤；其他错误原样返回。

`MockManager` 的创建取得写锁并使用 `entry(...).or_insert_with(...)`，因此第一次创建确定 `range_count`，后续同 ID 输入不覆盖；删除持写锁逐项移除；查询持读锁并复用统一过滤函数。

## 数据与状态

`PdManager` 自身只持有 `Arc<dyn PdClient>`，不缓存 PD 返回值，也不维护事务状态。每次方法调用的输入集合、兼容回退中间集合和结果 map 都是调用栈局部值。创建回退会克隆缺失 group 的 ID 和 key ranges；查询过滤会克隆命中的 `AffinityGroupState`，因此返回值不借用 client 响应。

`AffinityGroupKeyRange` 的字节向量表示已经由上游编码好的存储 key。当前会话路径在 `create_table_resources.rs::create_affinity` 中根据 table/partition 物理 ID 生成表前缀，再经 storage 的 `EncodeDDLRegionRange` 转换后交给本文件；manager 不校验区间是否有序、非空或互不重叠。

`MockManager` 是唯一持久化进程内 group 状态的实现，状态位于 `RwLock<HashMap<String, AffinityGroupState>>` 中。它只保存 ID 和首次创建时的 range 数量，不保存实际 ranges，所以不能模拟 PD 的完整 placement 行为。

## 依赖与调用关系

上游关系：

- `pkg/domain/affinity/interface.rs::{init_manager, create_groups_if_not_exists, delete_groups, get_groups}` 构造并调用 `Arc<dyn Manager>`；默认状态使用 `MockManager`，注入 PD client 后使用 `PdManager`。
- `pkg/session/runtime/create_table_resources.rs::{create_affinity, delete_affinity}` 在 mock storage 下走包级接口，在真实 storage 下由 PD endpoints 构造 `HttpClient`，再直接调用 `new_pd_manager`。
- `pkg/domain/affinity/lib.rs` 公开重导出本文件全部 public API，并把独立测试模块接入 crate。

下游关系：

- `PdManager` 只依赖 `PdClient` trait。真实实现 `pkg/domain/affinity/http_client.rs::HttpClient` 将 ranges 做 Base64 编码，通过 `reqwest` 调用 PD affinity-groups API；删除请求携带 `force`，按 ID 查询用 `url::form_urlencoded` 生成相同格式的查询串。
- 标准库 `HashMap`/`HashSet` 用于输入、过滤与去重，`Arc` 用于共享 trait object，`RwLock` 用于 mock 并发访问，`Instant` 构成 context 截止期类型。
- `pkg/domain/affinity/Cargo.toml` 的直接依赖为 `serde_json`、`base64`、`reqwest`、`log`、`url`；本文件直接使用其中的 `url`，其余主要服务于相邻 `http_client.rs` 和 `interface.rs`。

RustCodeGraph 将 `manager.rs` 标记为被 `http_client.rs`、`interface_test.rs`、`manager_test.rs`、`migration_aster_unit_test.rs` 等文件使用。精确 `query` 能定位 `PdManager`、`new_pd_manager`、回退函数与过滤函数；本次索引的 `callers/callees` 对这些 Rust trait 方法未返回边，因此上述生产调用边另由相邻源码和限定范围 `rg` 核验，而没有把空图结果误写为“无调用者”。

## 错误处理与边界

- manager 不包装下游错误：不符合兼容回退条件时直接返回原 `AffinityError`；回退后则返回查询、全量查询或第二次创建产生的错误。
- HTTP 状态识别依赖消息中第一个 `status: 'NNN` 片段。缺少数字或数字解析失败返回 `None`；它不是结构化 HTTP 响应类型。扩展错误格式时必须同步 `extract_pd_http_status_code` 及测试。
- 创建仅对 400、409、无状态码服务错误回退；查询仅对 400、404、414、无状态码服务错误回退。500 等非兼容性错误不会被全量扫描或二次请求掩盖。
- `http_service_error` 是 Rust 边界的显式布尔标记；只有“没有可解析状态码且该标记为真”才属于无状态码服务错误。
- 空创建、删除、查询都是无网络副作用的成功操作。过滤时缺失 ID 被静默忽略，重复 ID 只产生一个结果。
- `MockManager` 遇到 poisoned lock 会 `expect` 并 panic，而不是返回 `AffinityError`。真实 `PdManager` 不持有本地锁。
- `Context` 的取消/截止期不在 manager 中预检查；是否停止请求取决于具体 `PdClient`。因此自定义 client 必须明确实现 context 语义。

## 并发与资源生命周期

`Manager`、`PdClient`、`Context` 都要求 `Send + Sync`，构造器返回 `Arc<dyn Manager>`，允许包级状态和并发请求共享实现。`PdManager` 不持可变本地状态；并发安全责任落在 `Arc<dyn PdClient>` 的实现上。

`MockManager` 以一次操作为锁粒度：创建/删除独占写锁，查询持共享读锁。锁守卫在方法返回前释放；不存在后台任务、通道或显式 close。创建回退在首次创建、查询已有项、创建缺失项之间没有跨请求事务或锁，因此并发创建者可能同时判定同一 ID 缺失；最终一致性依赖 PD 创建接口的冲突/幂等行为。这与 Go 实现同样是多次远程调用，而不是原子事务。

包级生命周期属于 `interface.rs::PackageState`：`OnceLock<RwLock<_>>` 常驻进程，`init_manager` 整体替换 manager/client。直接从会话路径构造的 `PdManager` 则随返回的 `Arc` 引用计数释放。`BackgroundContext` 永不取消且无 deadline；需要可取消请求时应由调用者提供另一个 `Context` 实现。

## 与 Go 版本的对应关系

`pkg/domain/affinity/manager.go` 是逐项语义基准：Go `Manager`/`pdManager`/`mockManager` 分别对应 Rust `Manager`/`PdManager`/`MockManager`；两个阈值、空输入短路、首次 `skip_exist_check`、创建回退、查询全量回退、强制删除、客户端侧过滤和 mock 幂等逻辑均保持一致。`pkg/domain/affinity/Cargo.toml` 的 porting metadata 也把该 crate 指向这个 Go 包。

有意的表示差异包括：Go 直接嵌入 `pdhttp.Client`，Rust 通过 `Arc<dyn PdClient>` 解耦；Go 状态值是指针，Rust map 保存可克隆值；Go 使用 `context.Context`，Rust 仅移植取消与 deadline 子集；Go 用 `errors.Cause` 和 TiDB errno 识别无状态码服务错误，Rust 在 `AffinityError` 中保存显式标记；Go mock 将锁与 map 分开并容忍 nil map，Rust `Default` 保证 map 初始化并把它整体置于 `RwLock` 中。

URL 长度算法通过相同的重复 `ids` 参数和百分号编码保持一致。独立 Rust 测试与 Go 测试都验证 `"/".repeat(1364)` 恰为 4096 字节、1365 个斜杠超出 3 字节，以及 101 个 ID 触发全量扫描。

## 扩展指南

- 新增生命周期操作时，先扩展 `Manager`；若涉及 PD，再同步扩展 `PdClient`、`PdManager`、`MockManager` 和 `http_client.rs::HttpClient`。包级调用还需接入 `interface.rs`，业务接线通常位于 `create_table_resources.rs`。不要只在 mock 或 HTTP 层单边增加方法。
- 调整兼容状态码或 PD 错误格式时，修改 `should_fallback_create_affinity_groups` / `should_fallback_get_affinity_groups` / `extract_pd_http_status_code`，并在独立的 `manager_test.rs` 与 Go 对照测试中覆盖“应回退”和“不应回退”两类路径，避免把真实服务故障误判为版本兼容。
- 调整查询阈值时，保持 `affinity_group_ids_escaped_query_len` 与 `http_client.rs` 的实际序列化方式一致；同时评估全量查询的 PD 负载、代理 URI 限制和大 map 克隆成本。
- 扩充状态模型时，需同步 `AffinityGroupState`、HTTP JSON 转换、过滤克隆、mock 存储和测试构造器。若要模拟 ranges，本文件的 mock 当前信息不足，不能仅修改 `range_count`。
- 改变创建回退时须保留确定性排序和“只创建缺失项”不变量，并认识到查询与第二次创建之间不是原子事务。
- 测试逻辑应继续放在 `pkg/domain/affinity/manager_test.rs` 或迁移期独立测试中，不要放回生产源文件；Go 对齐变化还应同步检查 `manager_test.go`。

兼容性风险主要是混合版本 PD 的错误识别和返回过滤；性能风险主要是超过阈值后的全量扫描与结果克隆；并发正确性风险主要是回退的查后建竞态和 mock 锁中毒。任何改动都应优先复用现有 trait 边界，而不是绕过 manager 直接依赖具体 HTTP client。

## 验证依据

- 生产源码：`pkg/domain/affinity/manager.rs`（全部 421 行）；关键依据为 `Manager`/`PdClient`、`PdManager` 三条主路径、回退判定纯函数、`filter_affinity_groups` 和 `MockManager`。
- crate 与模块：`pkg/domain/affinity/Cargo.toml`、`pkg/domain/affinity/lib.rs`、`pkg/domain/affinity/interface.rs`、`pkg/domain/affinity/http_client.rs`。
- 应用接线：`pkg/session/runtime/create_table_resources.rs::create_affinity` 与 `delete_affinity`。
- Go 对照：`pkg/domain/affinity/manager.go`；Go 独立测试：`pkg/domain/affinity/manager_test.go`。
- Rust 独立测试：`pkg/domain/affinity/manager_test.rs` 验证首次 skip、409/服务错误回退、500 透传、响应过滤、长度/数量阈值和状态码匹配；`pkg/domain/affinity/migration_aster_unit_test.rs` 另验证过滤去重、mock 首次写入不覆盖、全量扫描与包级强制删除重试。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/domain/affinity` 覆盖目标、Go 对照及独立测试；`node --file` 读取目标源码、模块入口、相邻实现和测试；`query` 精确定位 `PdManager`、`new_pd_manager`、`should_fallback_create_affinity_groups`、`filter_affinity_groups`、`new_mock_manager`。该索引对本文件 Rust trait 方法的精确 `callers/callees` 没有输出，调用边已用上述直接源码与限定范围搜索交叉确认。
- 本任务是纯文档分析，未运行 Cargo；验收使用任务文件规定的 11 个固定二级标题结构检查，并人工复核所有“已支持”陈述均可回指到上述符号或文件。
