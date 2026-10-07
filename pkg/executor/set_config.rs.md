# `pkg/executor/set_config.rs`

## 文件定位

本文件属于 `astersql-executor` crate（见 `pkg/executor/Cargo.toml` 的 `[package] name = "astersql-executor"`），由 `pkg/executor/lib.rs` 通过 `pub mod set_config;` 暴露。它承载 Rust 版 `SET CONFIG` 的具体行为：把一项配置变更规范化并编码为 JSON，再根据集群节点类型向 status HTTP 端点下发。

需要区分“模块可用”和“生产执行链已经直接采用本实现”。`pkg/executor/builder.rs::buildSetConfig` 当前只把计划交给注入的 `BuilderDependencies::build_executor(ExecutorKind::SetConfig, ...)`；全仓 Rust 搜索未发现该依赖分支直接构造 `set_config::SetConfigExec`，而 `SetConfigExec` 的明确实例化位于 `pkg/executor/set_test.rs`。因此，本文件已有可测试的完整局部实现，但从通用 builder 到该具体类型的生产接线在当前仓库证据中未验证。

## 核心职责

- `SetConfigExec::Open` 在发送请求前完成输入规范化和静态校验：节点类型、实例地址和配置名转为 ASCII 小写，拒绝未知或不支持在线改配的组件，并生成后续复用的 JSON 请求体。
- `SetConfigExec::Next` 获取集群服务列表，按可选的节点类型和实例精确过滤，选择组件专用 HTTP 路径，并按节点顺序逐一发送请求。
- `SetConfigExec::doRequest` 将后端 HTTP 响应压缩为成功或错误；`Next` 再把单节点错误降级为 warning，从而继续处理其他节点。
- `isValidInstance` 校验 `host:port` 或 `[IPv6]:port` 的结构，并要求主机可由 `ToSocketAddrs` 解析。
- `ConvertConfigItem2JSON` 将已求值的 `ConfigValue` 编码成仅含一个字段的 JSON 对象。

这些职责通过 `SetConfigBackend` 与集群发现、HTTP 客户端、会话 warning 和具体错误类型解耦；本文件自身不持有 TiDB 会话，也不直接依赖某个 HTTP 库。

## 主要符号

- `TestSetConfigServerInfoKey`、`TestSetConfigHTTPHandlerKey`：与 Go 版本同名的测试注入键。当前 Rust 实现的依赖注入实际通过 `SetConfigBackend` 完成，本文件内没有读取这两个常量的逻辑。
- `ConfigValue`：配置值的已求值表示，覆盖 `Null`、`String`、`Int`、`Boolean`、`Real`、`Decimal` 和 `Unsupported`。`Decimal` 保存原始十进制文本，避免在本层再次浮点化。
- `SetConfigPlan`：执行所需的最小计划数据，包括 `node_type`、`instance`、`name` 和 `value`。`Open` 会原地修改前三个字符串。
- `ConfigServerInfo`：后端返回的节点类型和 status 地址。
- `ConfigHttpResponse`：后端 HTTP 调用结果的状态码、状态文本和响应体摘要。
- `SetConfigBackend`：同步边界 trait。`cluster_servers` 和 `post_json` 可能失败；`append_warning` 记录可恢复的单节点失败；`error` 统一创建后端错误。
- `SetConfigExec<B>`：执行状态容器，持有泛型后端、计划和 `Open` 预生成的 `json_body`。
- `SetConfigExec::Open<C>`：初始化入口；泛型上下文参数当前未使用。
- `SetConfigExec::Next<C>`：产出零行的执行入口；先调用 `Chunk::Reset`，泛型上下文参数当前未使用。
- `SetConfigExec::doRequest`：单端点 POST 和状态码解释入口。
- `isValidInstance`、`ConvertConfigItem2JSON`：公开辅助函数，分别负责实例格式/解析检查和类型化 JSON 编码。

文件使用 `#![allow(non_snake_case)]` 保留 `Open`、`Next`、`ConvertConfigItem2JSON`、`isValidInstance` 等与 Go 移植来源一致的命名。

## 执行流程

1. 调用方构造 `SetConfigExec`，其中 `SetConfigPlan` 已包含表达式求值后的 `ConfigValue`。
2. `Open` 若收到非空 `node_type`，先转小写，只允许 `tikv`、`tidb`、`pd`、`tiflash`、`tso`、`scheduling`；随后立即拒绝 `tidb`、`tso` 和 `scheduling`，因为它们不支持此在线改配路径。
3. 若指定 `instance`，`Open` 将其转小写并交给 `isValidInstance`。该函数分别解析普通 `host:port` 和方括号 IPv6；裸 IPv6、空 host、空 port 或多余冒号均失败，然后用 `(host, 0).to_socket_addrs()` 验证主机至少能解析出一个地址。
4. `Open` 将配置名转小写。目标为 `tiflash` 时，配置名必须以 `raftstore-proxy.` 开头，验证后移除该前缀；其他类型不移除前缀。
5. `Open` 调用 `ConvertConfigItem2JSON`。字符串使用 Rust `Debug` 字符串形式提供引号和转义；整数、布尔、实数和十进制文本直接作为 JSON 标量；空值和不支持类型返回错误。成功结果写入 `json_body`。
6. `Next` 首先清空结果 `Chunk`，体现该管理语句不返回结果行；之后通过 `backend.cluster_servers()` 获取节点。
7. 非空 `node_type` 和 `instance` 分别对节点做精确字符串匹配。指定实例但没有匹配节点时，语句级失败，不发送请求。
8. 对每个剩余节点选择路径：PD 使用 `/pd/api/v1/config`，TiKV 与 TiFlash 使用 `/config`；TiDB 或未知服务类型在循环中产生语句级错误。URL 由 `internal_http_scheme`、status 地址和路径拼接。
9. `doRequest` 调用 `post_json(url, json_body)`。状态码 `200` 成功；`400..=599` 的错误包含按有损 UTF-8 解码的响应体；其他状态码使用响应状态文本。
10. `Next` 捕获每次 `doRequest` 错误并调用 `append_warning`，然后继续后续节点；只有节点发现、实例不存在、无法识别服务类型等循环外/路由错误会直接终止 `Next`。

## 数据与状态

`SetConfigExec` 的可变状态只有 `backend`、`plan` 和 `json_body`。`Open` 会原地规范化计划并缓存 JSON，因此后续 `Next` 不会再次编码值。调用约束是先成功执行 `Open` 再执行 `Next`；类型系统没有用状态类型强制该顺序，若绕过 `Open`，`Next` 会按调用方提供的原始计划和 `json_body` 工作。

过滤使用 `Vec::retain`，保持 `cluster_servers` 返回的相对顺序。下发过程不聚合成功计数，也不回滚已成功节点：在多节点场景中可能出现部分成功，失败节点仅形成 warning。配置改变位于远端组件，本文件没有本地事务状态或补偿记录。

`ConfigValue::Decimal(String)` 和字符串拼装意味着本层信任上游提供合法十进制字面量；同样，JSON 键和值没有经过通用序列化库。字符串分支使用 `Debug` 转义，但 `Decimal` 文本若来源不受信任，合法 JSON 仍依赖上游不变量。

## 依赖与调用关系

- 上游模块装配：`pkg/executor/lib.rs` 公开 `set_config`；测试条件下还装配 `set_config_test`。
- 上游构建入口：`pkg/executor/builder.rs::build` 对 `Plan::SetConfig` 调用 `buildSetConfig`，后者经 `build_leaf` 把 `ExecutorKind::SetConfig` 交给 `BuilderDependencies::build_executor`。当前 Rust 搜索只确认这种种类级接线，未确认它最终构造本文件的 `SetConfigExec`。
- 已确认调用者：`pkg/executor/set_test.rs::test_set_cluster_config` 和 `test_set_cluster_config_json_data` 直接构造 `SetConfigExec`；`pkg/executor/set_config_test.rs::unbracketed_ipv6_instance_is_invalid` 直接调用 `isValidInstance`。
- 下游 crate：本文件直接使用 `std::net::ToSocketAddrs` 和 `astersql_util_chunk::Chunk`；后者由 `pkg/executor/Cargo.toml` 的 `astersql-util-chunk` 路径依赖提供。
- 外部副作用均经 `SetConfigBackend`：集群节点发现、HTTP POST、warning 收集与错误构造由调用方实现，因此单元测试可使用纯内存替身。

RustCodeGraph 的 `node SetConfigExec` 给出了测试导入和两个测试实例化边；精确 `query` 也定位了 Rust 定义。不过 `files --filter pkg/executor/set_config` 没有列出目标文件，调用图覆盖并不完整，所以其余关系以全仓 `rg`、模块入口和相邻源码交叉核验。

## 错误处理与边界

- `Open` 的硬错误包括未知类型、不支持在线修改的组件、无效实例、TiFlash 配置名前缀不合法、NULL 值和不支持的值类型；这些错误阻止任何请求。
- `cluster_servers` 失败直接由 `Next` 传播。指定实例存在合法地址形式但不在集群列表中，也直接返回错误。
- 节点类型为空时会遍历后端提供的全部服务器；如果列表包含 TiDB 或未知类型，循环会在遇到它时直接失败。此前节点可能已完成变更，因此仍存在部分成功边界。
- `post_json` 传输错误与所有非 200 HTTP 响应由 `doRequest` 返回，但 `Next` 将其记录为 warning 并继续。`400..=599` 包含响应体，其他非 200 状态只包含 `status` 字段。
- 只有 `200` 被视为成功；其他 2xx 和 3xx 都失败，不跟随或解释重定向。
- `isValidInstance` 会执行名称解析，因此校验可能受 DNS/主机解析环境影响；它并不验证端口是否为数字，因为解析时实际传给 `ToSocketAddrs` 的端口固定为 `0`，原字符串 port 仅检查非空和冒号结构。这一点与 Go `net.SplitHostPort` 后再 `net.LookupIP(host)` 的意图接近，但不是严格等价的端口语法验证。
- 空 `node_type` 配合显式 `instance` 时，过滤仅按 status 地址进行，最终路由由匹配节点的真实 `server_type` 决定。

## 并发与资源生命周期

接口全部是同步的 `&mut self` 调用，没有异步任务、线程、锁、通道或共享所有权。Rust 借用规则保证一次执行期间不能并发可变访问同一执行器；是否可跨线程取决于具体 `B`，本文件没有声明额外的 `Send`/`Sync` 约束。

`Next` 顺序处理节点，因此后端请求不会由本文件并发发出；性能和总体延迟随目标节点数线性增长。每次响应资源的创建与释放由 `SetConfigBackend::post_json` 实现负责，本文件只接收已摘要化的 `ConfigHttpResponse`，没有响应流或连接句柄需要显式关闭。失败不触发重试、超时管理或补偿，这些策略若存在也必须由后端承担。

`Chunk` 在 `Next` 开头重置，之后不写入行。`json_body` 在执行器存活期间复用；连续多次调用 `Next` 会重复下发同一配置，代码没有一次性执行标记。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/set_config.go`，主要流程基本逐段对应：

- Rust `SetConfigExec` 对应 Go `SetConfigExec`；`Open` 同样规范化类型/实例/配置名、处理 TiFlash 前缀并预生成 JSON，`Next` 同样发现和过滤节点后按 PD/TiKV/TiFlash 路由。
- Rust `SetConfigBackend` 合并了 Go 版从 session context、`infoschema.GetClusterServerInfo`、`util.InternalHTTPClient` 和 statement context warning 获得的能力，使本文件无需依赖完整会话对象。
- Go 的计划字段是 `*core.SetConfig`，值仍是 `expression.Expression`，`ConvertConfigItem2JSON` 在 `Open` 中执行表达式求值；Rust 的 `SetConfigPlan` 和 `ConfigValue` 已是局部类型，因此表达式求值属于上游责任。
- Go 用 `http.NewRequest`、响应 body 读取和 `defer resp.Body.Close()` 管理 HTTP 生命周期；Rust 后端直接返回响应摘要，资源释放责任移到后端实现。
- Go `isValidInstance` 用 `net.SplitHostPort` 验证 host/port 结构，再只解析 host；Rust 手动拆分并解析 host，明确拒绝裸 IPv6，但没有验证端口数字格式。
- Go 的布尔值由带 `mysql.IsBooleanFlag` 的整数表达式识别；Rust 用独立的 `ConfigValue::Boolean` 表达这一语义。
- Go 对求值产生的 NULL 使用 `"can't set config to null"`，对空表达式使用 `"cannot set config to null"`；Rust 的单一 `ConfigValue::Null` 只返回后者，因此错误文案粒度略有差异。
- Go builder 的 `buildSetConfig` 明确构造 `&SetConfigExec{...}`；Rust builder 当前只分发 `ExecutorKind::SetConfig`，没有直接构造本文件类型的可见证据。这是当前移植接线差异，不能从局部测试推断已经等价接入生产链。

Go 测试 `pkg/executor/set_test.go::TestSetClusterConfig` 覆盖类型拒绝、DNS/实例查找、TiFlash 前缀、按类型/实例过滤、请求错误降级和 400 响应体；`TestSetClusterConfigJSONData` 覆盖字符串、布尔、整数、浮点、十进制、NULL 和不支持类型。Rust 对应测试位于独立文件 `pkg/executor/set_test.rs` 与 `pkg/executor/set_config_test.rs`，覆盖核心主路径，但 Go 测试的全部边界尚未在 Rust 中逐项复刻。

## 扩展指南

- 新增可在线改配的组件时，需要同步修改 `Open` 的类型白名单/拒绝规则和 `Next` 的 URL 路由，并在 `pkg/executor/set_test.rs` 增加成功路径、错误状态和多节点行为测试；还要核对 Go 同路径实现是否应保持一致。
- 改变配置值类型时，优先扩展 `ConfigValue` 与 `ConvertConfigItem2JSON`，同时验证合法 JSON、NULL 语义、字符串转义、特殊浮点值和十进制格式。若值来自不可信文本，宜考虑统一 JSON 序列化而不是继续手工拼接。
- 强化实例校验时应修改 `isValidInstance`，在独立的 `pkg/executor/set_config_test.rs` 中加入 IPv4、域名、方括号 IPv6、空/非数字/越界端口和解析失败用例；不要把测试嵌入生产源文件。
- 若要完成生产接线，应从 `pkg/executor/builder.rs` 的 `ExecutorKind::SetConfig` 处理边界追踪 `BuilderDependencies` 的具体实现，确认如何把 planner 的 `PlanData` 转成 `SetConfigPlan`、提供生产 `SetConfigBackend` 并适配通用 `Executor` trait。当前证据不足以建议仅在 `buildSetConfig` 中直接替换类型。
- 若希望并行下发、重试或原子化，必须先定义部分成功、warning 顺序、幂等性和超时契约；这会改变现有逐节点同步语义及 Go 兼容性，应增加多节点故障测试和性能评估。
- 修改后应保持测试文件独立，并优先扩充 `pkg/executor/set_test.rs` 和 `pkg/executor/set_config_test.rs`；涉及生产接线时还需增加从 builder 到实际执行器的集成证据。

## 验证依据

- 源实现：`pkg/executor/set_config.rs`，核对了两个测试键、六个数据/状态类型（含 trait）、`SetConfigExec` 的三个方法及两个辅助函数。
- crate 与模块边界：`pkg/executor/Cargo.toml`（crate 名、`astersql-util-chunk` 路径依赖）和 `pkg/executor/lib.rs`（`pub mod set_config`、独立测试模块）。目标包不存在 `doc.go`，没有可补充读取的包级 Go 契约。
- Rust 上游与调用证据：`pkg/executor/builder.rs::{build, buildSetConfig, build_leaf}`、`ExecutorKind::SetConfig`；全仓 Rust 搜索只发现 builder 的种类分发和测试对具体 `SetConfigExec` 的构造。
- Rust 测试：`pkg/executor/set_test.rs::{ConfigBackend, test_set_cluster_config, test_set_cluster_config_json_data}`；`pkg/executor/set_config_test.rs::unbracketed_ipv6_instance_is_invalid`。
- Go 对照：`pkg/executor/set_config.go::{SetConfigExec.Open, SetConfigExec.Next, SetConfigExec.doRequest, isValidInstance, ConvertConfigItem2JSON}`、`pkg/executor/builder.go::buildSetConfig`、`pkg/executor/set_test.go::{TestSetClusterConfig, TestSetClusterConfigJSONData}`。
- RustCodeGraph：`status` 显示索引可用；`query SetConfigExec --kind struct` 与 `node SetConfigExec` 定位 Rust 定义，并显示 `set_test.rs` 的导入及两个测试实例化边；`query ConvertConfigItem2JSON --kind function` 定位 Rust/Go 定义。`files --filter pkg/executor/set_config` 返回空，故未把缺失的 callers/callees 结果当作“无调用”证明，而以全仓 `rg` 交叉核验。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证文档存在且恰含 11 个固定二级章节，并人工复查每项关键行为均能回溯到上述源码、测试或模块证据。
