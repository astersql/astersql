# `pkg/server/handler/tikvhandler/flash_replica.rs`

## 文件定位

本文件属于 Cargo crate `astersql-server-handler-tikvhandler`，由同目录 `lib.rs` 以 `pub mod flash_replica` 声明并通过 `pub use flash_replica::*` 再导出。它只定义 `GET /tiflash/replica` 所需的响应数据契约和 `reload` 查询参数解析器，不直接注册路由，也不持有 server、domain、InfoSchema 或存储对象。

运行时接线位于 `pkg/server/http_status.rs`：`build_status_router` 将 `/tiflash/replica` 注册到 `tiflash_replica_summary_response`；后者调用本文件的 `parse_flash_replica_reload_query`，读取 domain 状态并构造 `FlashReplicaSummary`，最后调用 `to_json` 生成响应。RustCodeGraph 给出的直接调用边是 `tiflash_replica_summary_response -> parse_flash_replica_reload_query`，并显示该函数会实例化 `FlashReplicaSummary`。

## 核心职责

本文件有两个聚焦职责：

1. 用 `FlashReplicaSummary` 固定运维接口的七个公开字段，包括 keyspace 身份、列存开关、列存类型、当前含 TiFlash replica 元数据的表数、能否禁用列存以及是否执行过 reload。
2. 用 `parse_flash_replica_reload_query` 复刻 Go `strconv.ParseBool` 在此端点采用的合法拼写，并为缺失或空参数提供 `false` 默认值。

它刻意不返回逐表身份。`can_disable` 只是上游根据 `table_count == 0` 填入的即时、尽力而为判断；本文件既不计算表数，也不提供跨节点锁或原子检查，因此调用方不能把该值解释为分布式协调保证。该限制来自同路径 Go 文件 `flash_replica.go` 的类型注释和 handler 实现。

## 主要符号

- `pub struct FlashReplicaSummary`：公开响应 DTO，派生 `Clone`、`Debug`、`Eq`、`PartialEq`，便于复制、诊断和精确比较。字符串字段为 `keyspace`、`tidb_columnar_storage_enabled`、`columnar_store_type`；`keyspace_id` 为 `u32`；`table_count` 为 `usize`；两个布尔字段为 `can_disable` 和 `reloaded`。
- `FlashReplicaSummary::to_json(&self) -> serde_json::Value`：将 DTO 显式投影为七个 snake_case JSON 字段。显式映射保持 Go 的 JSON 字段名，同时不会意外泄露表名或表 ID；返回 `Value`，序列化成 HTTP body 是上游的职责。
- `parse_flash_replica_reload_query(raw: Option<&str>) -> Result<bool, String>`：公开的无状态解析函数。`None` 和空串返回 `Ok(false)`；`1/t/T/true/TRUE/True` 返回 `Ok(true)`；`0/f/F/false/FALSE/False` 返回 `Ok(false)`；其他输入返回带原始调试表示的错误字符串。

文件中没有常量、trait、条件编译项或私有辅助函数。测试通过同 crate 的 `flash_replica_test.rs` 独立编译，未嵌入生产源文件。

## 执行流程

完整请求链以 `pkg/server/http_status.rs::tiflash_replica_summary_response` 为入口：

1. 路由层先拒绝非 GET 请求并返回 405；此判断不在本文件内。
2. 上游从查询映射取出 `reload`，以 `Option<&str>` 调用 `parse_flash_replica_reload_query`。缺失或空值走便宜的非 reload 路径；非法值由上游映射为 400。
3. `reload=true` 时，上游要求 domain 执行 `reload_schema`；失败映射为 500。随后取得 schema snapshot，统计带 `TiFlashReplica` 元数据的当前表。
4. 上游读取 `tidb_columnar_storage_enabled`、全局配置中的列存类型以及 keyspace 身份，并设置 `can_disable = table_count == 0`、`reloaded = reload`。
5. 构造好的 `FlashReplicaSummary` 调用 `to_json`；该方法一次性生成七字段 JSON object，上游再转成字符串并返回 200。

因此，本文件的内部流程均为同步、确定性的值转换；所有 I/O、schema 刷新和错误状态码决策都发生在调用方。

## 数据与状态

`FlashReplicaSummary` 是拥有数据的快照值：三个字符串由调用方传入并归结构体所有，其余字段是可复制的标量。类型自身没有默认值构造器，也不验证字段间关系；例如 `can_disable` 与 `table_count` 的一致性依赖上游构造者维护。

`to_json` 只读取 `&self`，不会消费或改变 summary。当前 JSON 契约恰有 `keyspace`、`keyspace_id`、`tidb_columnar_storage_enabled`、`columnar_store_type`、`can_disable`、`table_count`、`reloaded` 七项；`flash_replica_test.rs::summary_json_has_only_operator_fields` 特别断言 `table_count` 可读且不存在 `tables` 字段。

解析器没有缓存和全局状态。它不裁剪空白、不做 Unicode 大小写归一化，也不接受任意大小写组合；合法集合是源码 `match` 明列的 Go `strconv.ParseBool` 拼写。

## 依赖与调用关系

直接下游依赖只有 `serde_json`：`to_json` 使用 `serde_json::json!` 构建动态 JSON 值。`pkg/server/handler/tikvhandler/Cargo.toml` 将其声明为普通依赖；本文件不直接使用该 crate 中其余大量 AsterSQL 子系统依赖。

直接上游包括：

- `pkg/server/http_status.rs::tiflash_replica_summary_response`：生产调用者，解析 reload、实例化 summary 并序列化响应。
- `pkg/server/handler/tikvhandler/flash_replica_test.rs`：直接覆盖解析器和 JSON 投影的单元测试。
- `pkg/server/http_status_test.rs`：通过真实 status listener 覆盖路由接线、空/非空计数、reload、非法参数和错误状态码。

职责分界很重要：InfoSchema 遍历、domain reload、全局变量读取、keyspace 获取和 HTTP 状态码映射均属于 `pkg/server/http_status.rs`；本文件只是稳定的数据/解析边界。RustCodeGraph 索引状态显示目标文件已纳入索引，并给出上述生产调用边。

## 错误处理与边界

`parse_flash_replica_reload_query` 唯一的失败是输入不在白名单中，返回 `Err(String)`，文本形如 `invalid reload query value "maybe", expect true/false/1/0`。它不 panic，也不包装错误类型；HTTP 层负责把该错误变为 400。`None` 与 `Some("")` 均被有意视为未请求 reload，而不是错误。

`to_json` 使用只含 Rust 基础标量和字符串的 `json!` 投影，接口本身不返回 `Result`。它不负责校验表计数、列存开关字符串或 keyspace ID，也不捕获上游 schema/domain/config 错误。

上游边界由 `pkg/server/http_status.rs` 和对应测试确认：非 GET 为 405，domain/schema/reload/全局变量失败为 500，非法 reload 为 400，成功为 200。Go 测试还证明已 drop/truncate 的残留不会计入新 summary，逐表遗留信息仍由 `/tiflash/replica-deprecated` 提供；这些是端点语义，不是本文件自行实施的过滤逻辑。

## 并发与资源生命周期

本文件没有锁、原子变量、channel、异步任务、事务、文件句柄或网络资源。解析器只借用请求参数字符串直到函数返回；summary 拥有其字符串；`to_json` 创建独立的 `serde_json::Value`，不存在跨调用共享的可变状态，因此可由并发请求独立调用。

真正的资源生命周期在上游：可选的 schema reload 可能与其他 schema 同步/reload 调用串行化，取得的 schema 是一次快照。即使先 reload，检查结果也不是线性一致的集群锁，检查与后续操作之间仍可能出现新 replica；Go 文件注释明确要求运维方协调数据库用户并在操作后复查。

## 与 Go 版本的对应关系

同路径 `flash_replica.go` 是语义基准。Rust `FlashReplicaSummary` 的七个字段及 JSON 名称与 Go 结构体一致；Rust `usize table_count` 对应 Go `int TableCount`，Rust 显式 `to_json` 对应 Go `encoding/json` 根据 struct tag 自动编码。

Rust `parse_flash_replica_reload_query(Option<&str>)` 对应 Go `parseFlashReplicaReloadQuery(*http.Request)` 的值解析部分：两者都把缺失/空值视为 false，并接受 Go `strconv.ParseBool` 的全部真/假拼写。差异是 Rust 函数不认识 HTTP request，只接收已经提取的值，并以 `String` 返回错误。

Go 将 `FlashReplicaSummaryHandler.ServeHTTP`、domain 获取/Reload、InfoSchema 特殊属性遍历、全局变量读取和写响应全部放在同文件。Rust 将这些编排迁至 `pkg/server/http_status.rs::tiflash_replica_summary_response`，本文件保留可复用的契约和纯函数。Go 的 `ListTablesWithSpecialAttribute(TiFlashAttribute)` 与 Rust 上游遍历所有 schema/table 后检查 `metadata.TiFlashReplica.is_some()` 是对应计数意图；具体实现位置不同。

`pkg/server/handler/tests/http_handler_test.go::TestTiFlashReplicaSummary` 覆盖 Go 端点的完整语义，包括分区表按表计数、启停 replica、drop 残留排除、列存开关、列存类型、keyspace、reload 失败及方法限制。Rust 的近邻单元测试覆盖本文件纯逻辑，`pkg/server/http_status_test.rs` 再覆盖 Rust 运行时接线。

## 扩展指南

- 新增或重命名响应字段时，应同步修改 `FlashReplicaSummary` 与 `to_json`，并扩展 `flash_replica_test.rs::summary_json_has_only_operator_fields`；同时更新 `pkg/server/http_status.rs` 的构造点、Rust HTTP 测试、Go 结构体与 Go HTTP 测试，防止跨语言响应漂移。新增字段要评估旧运维客户端的兼容性和是否会泄露表身份。
- 改变 `reload` 语法时，应修改 `parse_flash_replica_reload_query` 并扩展 `reload_query_matches_go_parse_bool_contract`。除非 Go 端契约同步改变，不应采用 `trim` 或通用大小写归一化扩大合法输入集合。
- 改变 `can_disable` 或计数规则时，主要接入点不是本文件，而是 `tiflash_replica_summary_response`；应同步 Go `ServeHTTP` 以及两侧端到端测试。需特别评估分区计数、drop/truncate 残留、schema lease 延迟和 reload 成本。
- 若需要强一致的“可禁用”判断，不能只在 DTO 中增加判断；需要在 domain/集群协调层设计锁或版本校验，并明确检查—操作窗口。该类变更有并发和可用性风险。
- 测试继续放在独立文件：本 crate 的纯逻辑测试位于 `flash_replica_test.rs`，路由集成测试位于 `pkg/server/http_status_test.rs`，不要把 `#[cfg(test)]` 测试模块嵌回本生产文件。

## 验证依据

- RustCodeGraph：`status` 显示目标仓库索引包含 Rust 文件；`files --filter pkg/server/handler/tikvhandler` 列出 `flash_replica.rs`、`flash_replica_test.rs` 与 `lib.rs`；`query/node/explore` 确认 `FlashReplicaSummary`、`parse_flash_replica_reload_query`，以及 `tiflash_replica_summary_response -> parse_flash_replica_reload_query` 调用边和 summary 实例化关系。
- 生产源码：`pkg/server/handler/tikvhandler/flash_replica.rs`（DTO、JSON 投影、解析白名单）；`pkg/server/http_status.rs`（生产调用、错误映射、数据收集与路由注册）；`pkg/server/handler/tikvhandler/lib.rs`（模块声明、再导出、独立测试模块）。
- crate 边界：`pkg/server/handler/tikvhandler/Cargo.toml`（crate 名、`lib.rs` 入口、Go package 移植元数据、`serde_json` 及 AsterSQL 子系统依赖）。
- Go 对照：`pkg/server/handler/tikvhandler/flash_replica.go`（完整 handler、响应字段、reload 语义与一致性限制）；`pkg/server/http_status.go`（GET 路由注册）。
- 测试证据：`pkg/server/handler/tikvhandler/flash_replica_test.rs`（解析合法集合与不泄露 tables）；`pkg/server/http_status_test.rs`（Rust listener 的 200/400/405、计数、reload 和列存开关）；`pkg/server/handler/tests/http_handler_test.go::TestTiFlashReplicaSummary`（Go 完整行为与边界）。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构检查要求目标文档存在且恰有十一个规定的二级标题。
