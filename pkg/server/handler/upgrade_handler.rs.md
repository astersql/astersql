# `pkg/server/handler/upgrade_handler.rs`

## 文件定位

本文件是 `astersql-server-handler` crate 中的集群升级状态 HTTP 处理器，源码入口为 [`upgrade_handler.rs`](upgrade_handler.rs)，由 [`lib.rs`](lib.rs) 通过 `pub mod upgrade_handler` 导出。它保留 Go `pkg/server/handler/upgrade_handler.go` 的公开命名和三类操作：`start` 进入升级态、`finish` 恢复普通运行态、`show` 查询升级进度。

在当前 Rust 应用中的直接接线位于 `pkg/server/http_status.rs`：`build_status_router` 创建一个本地 `Storage`，构造 `ClusterUpgradeHandler`，并把它注册到 `/upgrade/{op}`；`upgrade_response` 再把 status server 的 `Request`/`Response` 适配成本文件的 `Request` 与 `crate::util::ResponseWriter`。因此本文件已经进入 Rust status HTTP 请求链，但当前接线使用的是独立内存状态，并不是 `Server` 的真实 KV storage。

`pkg/server/handler/Cargo.toml` 将本目录声明为 `astersql-server-handler`，并列出了 `astersql-kv`、`astersql-session`、`astersql-domain-infosync`、`astersql-domain-serverinfo` 等迁移目标依赖；本文件当前实际只导入 `std::sync::{Arc, Mutex}` 和同 crate 的 `util`，尚未使用这些真实依赖。文件没有条件编译项；测试模块由 `lib.rs` 的 `#[cfg(test)]` 和独立文件 `upgrade_handler_test.rs` 接入。

## 核心职责

- `serve_http`/`ClusterUpgradeHandler::ServeHTTP` 实现 POST-only 的操作分派，保持 Go API 的错误文本、幂等提示和成功响应形状。
- `StartUpgrade` 与 `FinishUpgrade` 在共享 `ClusterState.upgrading` 上实现两态转换，并通过返回的 `bool` 区分“已经完成过的重复操作”和“本次完成了状态切换”。
- `showUpgrade` 在升级态下收集内存中的 server 信息和 DDL owner，按最高版本字符串对应的完整 `VersionInfo` 计数计算进度，并在版本信息不一致时返回全部节点摘要。
- `JsonValue` 实现保持 Go JSON 字段名及 `omitempty` 行为：零 `servers_num`、`false` 的一致性标志和空差异列表不会写出。
- `Storage`、`Session`、`Domain`、`Request`、`Context`、`Duration` 及若干辅助函数是本文件内的最小可测试替身。它们使状态机和 HTTP 形状可运行，但不等同于 Go 中真实的 `kv.Storage`、session、infosync、DDL owner manager、context timeout 或日志系统。

## 主要符号

- `pub struct ClusterUpgradeHandler { pub store: Storage }`：处理器本体；公开字段允许调用方提供共享状态。
- `pub fn NewClusterUpgradeHandler(Storage) -> ClusterUpgradeHandler`：Go 风格构造入口。RustCodeGraph 显示生产调用者为 `pkg/server/http_status.rs::build_status_router`，测试调用者为 `upgrade_handler_test.rs::show_treats_different_git_hashes_as_different_version_info`。
- `pub fn serve_http(&ClusterUpgradeHandler, &mut ResponseWriter, &Request)`：实际分派函数；`ServeHTTP` 只是方法式转发。非 POST 和未知操作分别经 `WriteError` 返回 400。
- `ClusterUpgradeHandler::{StartUpgrade, FinishUpgrade}`：公开状态转换 API，返回 `Result<bool, Error>`；`true` 表示调用前已经处于目标态。
- `ClusterUpgradeHandler::showUpgrade`：私有查询实现。普通态直接写提示；升级态生成 `ClusterUpgradeInfo`。
- `SimpleServerInfo`：对 `ServerInfo` 的响应投影，JSON 字段为 `version`、`git_hash`、`ddl_id`、`ip`、`listening_port`、`server_id`。
- `ClusterUpgradeInfo`：聚合响应，包含节点数、owner、升级百分比、一致性标记及差异节点。
- `Storage` 与私有 `ClusterState`：`Arc<Mutex<ClusterState>>` 包装的进程内共享状态；公开 setter 仅有 `set_servers` 和 `set_owner_id`。
- `Session`/`Domain`：共享同一状态的轻量句柄；`Session` 记录 `closed`，`Drop` 再次调用 `close`，`Domain::ddl_owner_id` 读取 owner。
- `VersionInfo`、`ServerInfo`、`VersionCounter`：版本、节点及计数模型。`VersionInfo` 的相等性同时比较 `version` 与 `git_hash`，但最大版本只按 `version` 字符串选择。
- `METHOD_POST`、`OPERATION`：分别固定为 `"POST"` 与 `"op"`。`Duration(i64)`、`Context` 和 `log_info` 当前仅保留接口形状，值不会影响行为。

## 执行流程

1. `pkg/server/http_status.rs::serve_stream` 解析 HTTP 请求，经 `Router` 命中 `build_status_router` 注册的 `/upgrade/{op}`，调用 `upgrade_response`。
2. `upgrade_response` 从路径第二段提取操作，把 status server 的方法映射为字符串，构造本文件的 `Request`，调用 `ClusterUpgradeHandler::ServeHTTP`，最后把 `ResponseWriter` 状态码和 body 转回 status `Response`。
3. `ServeHTTP` 转发到 `serve_http`。后者先校验方法必须精确等于 `METHOD_POST`；不满足时写入 `"This API only support POST method"` 并结束。
4. `serve_http` 读取 `OPERATION` 对应的路径变量：`start` 调 `StartUpgrade`，`finish` 调 `FinishUpgrade`，`show` 调 `showUpgrade`；其他值写入 `wrong operation:<op>`。
5. `StartUpgrade` 创建 `Session` 并读取 `upgrading`。若已为 `true`，关闭 session 并返回 `Ok(true)`；否则调用 `sync_upgrade_state` 写为 `true`，关闭后返回 `Ok(false)`。传入的十秒 `Duration` 当前未实施等待或超时。
6. `FinishUpgrade` 对称地读取状态；已为普通态时返回 `Ok(true)`，否则调用 `sync_normal_running` 写为 `false`，随后返回 `Ok(false)`。
7. `showUpgrade` 在普通态写入 `"The cluster state is normal."` 并返回；在升级态则获取 `Domain`、复制 server 列表、读取 owner，将所有节点版本加入 `VersionCounter`。
8. `VersionCounter::max_version` 以 `VersionInfo.version` 的 Rust 字符串字典序选最大项；`count` 再以完整 `VersionInfo`（含 `git_hash`）计数。百分比是 `count * 100 / server_count` 的整数除法；空列表用分母 `max(1)`，得到 0 而不会除零。
9. 若百分比不是 100，`showUpgrade` 将全部节点映射成 `SimpleServerInfo`；否则保持差异列表为空。响应经 `WriteData` 序列化。
10. 分派成功后，`serve_http` 仍会统一写一次响应：重复 start/finish 写幂等提示，其余写 `"success!"`。所以 `show` 会先写查询结果再追加 JSON 字符串 `"success!"`；这与 Go 当前流程以及 Go 测试对普通态 show 的拼接断言一致，但对结构化 JSON 消费者是值得注意的协议特征。

## 数据与状态

唯一可变共享状态是 `Storage.state: Arc<Mutex<ClusterState>>`。克隆 `Storage`、创建 `Session` 或创建 `Domain` 都只克隆 `Arc`，因而观察和修改同一个 `ClusterState`。该状态包含：`upgrading: bool`（默认 `false`）、`owner_id: String`（默认空）、`servers: Vec<ServerInfo>`（默认空）。当前不存在持久化、跨进程同步或真实集群发现，status router 每次构建时还会创建全新的 `Storage`。

升级进度不是“完成升级的任务数”，而是“版本字符串最大的那一项，其完整 `VersionInfo` 出现次数占节点总数的百分比”。这产生两个重要不变量：一是同版本号但不同 `git_hash` 的节点属于不同计数桶；二是最大桶的候选版本先仅按 `version` 选择，若多个不同 hash 具有相同最大版本，`max_by` 的并列选择会影响采用哪个桶，但单元测试确认两个不同 hash 的两节点场景结果为 50%。

JSON 省略规则由 `ClusterUpgradeInfo::to_json` 手写：`owner_id` 和 `upgraded_percent` 始终存在；`servers_num != 0`、`is_all_upgraded == true`、差异列表非空时才写对应字段。`SimpleServerInfo::to_json` 始终写全六个字段，并通过 `json_string` 转义字符串。

## 依赖与调用关系

上游主链为 `Server::start_status_http` → `build_status_router` → `/upgrade/{op}` → `upgrade_response` → `ClusterUpgradeHandler::ServeHTTP` → `serve_http`。RustCodeGraph 还把测试 `show_treats_different_git_hashes_as_different_version_info` 标为构造器、`ServeHTTP` 和 `StartUpgrade` 的调用者。

文件内下游关系为：`serve_http` 调用 `StartUpgrade`/`FinishUpgrade`/`showUpgrade` 以及 `util::{WriteData, WriteError}`；状态转换调用 `create_session`、`is_upgrading_cluster_state`、`sync_upgrade_state` 或 `sync_normal_running`；查询调用 `get_domain`、`get_all_server_info`、`Domain::ddl_owner_id`、`VersionCounter` 和 `SimpleServerInfo::from_server_info`。

真实编译依赖只有标准库同步原语和本 crate 的 `util::{Error, JsonValue, ResponseWriter, WriteData, WriteError, json_string}`。`Cargo.toml` 中的 real KV/session/domain crates 是 crate 级依赖，并非当前文件的实际下游。后续接入真实集群时，应替换本文件的同名替身而不是误认为它们已经委托给这些 crates。

## 错误处理与边界

- `WriteError` 的实现默认写 HTTP 400；因此非 POST、未知操作以及从分支传播到 `serve_http` 的 `Error` 都是 400，不是 500。`upgrade_response` 保留 `ResponseWriter` 的状态码。
- 当前 `create_session`、状态读写、`get_domain`、`get_all_server_info` 和 `ddl_owner_id` 除互斥锁中毒导致 panic 外都恒定返回 `Ok`，所以 `Result` 的错误分支主要是为未来真实依赖保留的接口。
- 所有锁都用 `lock().unwrap()`；任何持锁 panic 造成的 poison 会在后续请求中再次 panic，没有转换成 `Error` 或 HTTP 响应。
- 空 server 列表不会 panic：`max_version` 回退为空版本，除数用 `max(1)`，最终报告 `upgraded_percent: 0`、`is_all_upgraded: false`、无差异列表。Go 版本直接读取 `allVersions[0]`，隐含 infosync 至少返回一个节点；这是明确的 Rust 防御性差异。
- 版本“最大值”使用字典序而非语义版本比较，与 Go 的 `strings.Compare` 对齐；例如非规范版本字符串不会按 semver 排序。
- `show` 的结果之后会追加统一成功字符串；修改这一行为会改变 Go 对齐协议和现有测试预期，必须作为显式兼容性变更处理。
- `Duration::seconds(10)`、三秒 owner timeout、`background_context` 和 `timeout_context` 当前都不执行超时/取消语义；`log_info` 也是空函数，失败和成功不会产生真实日志。

## 并发与资源生命周期

`Arc<Mutex<ClusterState>>` 允许多个 handler/session/domain 句柄跨线程共享状态；每个读写方法只在一次短临界区内持锁，不会在写 HTTP 响应时持锁。`showUpgrade` 分别复制 server 列表、读取 owner，二者不是同一锁快照：并发 setter 可使响应看到不同时间点的数据。状态检查与状态切换也分成两次加锁，两个并发 `start` 或 `finish` 都可能先看到旧状态并各自报告“本次完成”，所以幂等状态最终正确，但 `has_done` 的响应语义不是原子的。

`Session` 在显式分支中调用 `close`，离开作用域时 `Drop` 又调用一次；由于 `close` 只是把布尔值设为 `true`，双重关闭是幂等的。值得注意的是，`showUpgrade` 在成功与普通态路径显式关闭，但在创建 session 之后的 `?` 错误路径依靠 `Drop` 收尾，等价于 Go 的 `defer se.Close()`。`Domain` 和复制出的 server 列表没有外部资源。

status server 自身为每个连接启动线程，但本文件不创建线程或异步任务；同步互斥锁就是其并发边界。真实 session、集群 RPC、context deadline 与日志资源生命周期尚未接入。

## 与 Go 版本的对应关系

结构和主要分支直接对应 `pkg/server/handler/upgrade_handler.go`：构造器、POST 校验、`start`/`finish`/`show` 分派、重复操作文案、10 秒升级同步参数、3 秒 owner 查询参数、版本字典序、整数百分比、差异节点返回条件及 JSON 字段名均被保留。Go `TestUpgrade` 还验证了 GET 返回 bad request、重复 start/finish 的幂等文案、状态切换，以及普通态 show 产生“状态提示 + success”拼接；`testUpgradeShow` 验证 1 节点 100%、3 个不同版本 33% 且返回三条详情、3 个相同版本 100% 且不返回详情。

当前迁移差异同样关键：Go handler 持有 `kv.Storage`，用 `session.CreateSession`、`session.IsUpgradingClusterState`、`session.SyncUpgradeState`、`session.SyncNormalRunning`、`session.GetDomain`、`infosync.GetAllServerInfo` 和 DDL owner manager 操作真实集群；Rust 文件把这些全部实现为本地替身。Go context 的 deadline/cancel 和 zap 日志在 Rust 中为空壳。Go 假设至少一个 server，Rust为空列表提供了安全回退。Rust 的独立测试只覆盖“相同 version、不同 git hash 必须分桶”的 50% 场景，没有完整承接 Go `TestUpgrade` 的 HTTP、错误、幂等和多版本矩阵。

`pkg/server/http_status.rs` 的生产接线进一步体现迁移未完成：它创建本地空 server 列表并硬编码 owner 为 `handler-test-owner`，没有从 `Server`/`Domain` 注入真实 storage。因此本文件当前可用于本进程状态机和协议形状验证，不能据此宣称已经协调多 TiDB 节点升级或暂停真实 DDL。

## 扩展指南

接入真实集群能力时，最可能修改 `ClusterUpgradeHandler.store`、`create_session`、四个状态/domain/infosync 辅助函数以及 `Context`/`Duration`；同时必须修改 `pkg/server/http_status.rs::build_status_router`，从 `Server` 注入真实共享 storage，移除固定 owner。应保持 `StartUpgrade`/`FinishUpgrade` 的幂等返回约定、Go 错误文案、JSON 字段与省略规则，除非上层 API 明确批准不兼容变更。

修改进度算法时需同时审查 `VersionCounter::{max_version, count}` 和 `VersionInfo` 的相等性：版本选择仍须说明是字典序还是 semver，git hash 是否属于一致性身份也必须由测试固定。若要求一致快照或严格的重复操作响应，应把“检查 + 切换”收敛到一次同步操作，而不是只在现有两个锁区间之间加代码。

测试必须继续放在独立文件，优先扩展 `pkg/server/handler/upgrade_handler_test.rs`，不要内嵌进生产源文件。至少应补齐非 POST、未知 op、start/finish 首次与重复调用、普通态 show、空节点、单节点、多版本/同版本不同 hash、JSON 转义和并发状态转换；真实 status 接线则同步扩展 `pkg/server/http_status_test.rs`。若完成真实 Go 语义移植，还应以 `pkg/server/handler/tests/http_handler_test.go::TestUpgrade` 和 `testUpgradeShow` 的矩阵为逐项依据。

兼容风险主要在响应 body 的双写、400/200 状态码、字段省略和重复操作文案；正确性风险主要在非原子状态转换、非一致快照、字符串版本排序和空集语义；性能风险主要是复制完整 server 列表、版本线性计数以及进程内互斥锁，在替换为真实 RPC 后还需评估超时与阻塞。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件、307,296 个节点和 1,848,419 条边；索引包含目标 Rust/Go 文件及测试。
- RustCodeGraph `explore "pkg/server/handler/upgrade_handler.rs UpgradeHandler handle_upgrade is_upgrade_requested"`：读取目标文件全貌与 Go 对照，并确认 `NewClusterUpgradeHandler` 的调用者包含 `build_status_router` 和独立 Rust 测试。
- RustCodeGraph `explore "build_status_router NewClusterUpgradeHandler ServeHTTP StartUpgrade FinishUpgrade showUpgrade pkg/server/http_status.rs"`：确认构造、路由、适配、分派和测试调用边；通用符号存在重名，结论仅采用结果中明确标注目标路径的边。
- RustCodeGraph `node --file`：分段核对 `upgrade_handler.rs`、`upgrade_handler_test.rs`、`http_status.rs` 与 `util.rs` 的实际源码，包含状态模型、JSON、路由挂载、`upgrade_response`、线程入口和 `WriteData`/`WriteError` 状态码语义。
- 直接读取 `pkg/server/handler/Cargo.toml`、`pkg/server/handler/lib.rs`：核对 crate 边界、crate 级依赖、模块导出和独立测试接线；本目录不存在 `doc.go`。
- 直接读取 `pkg/server/handler/upgrade_handler.go` 与 `pkg/server/handler/tests/http_handler_test.go` 的 `TestUpgrade`/`testUpgradeShow`：核对 Go 状态机、HTTP 文案、owner/版本进度和测试边界。
- 独立 Rust 测试 `pkg/server/handler/upgrade_handler_test.rs::show_treats_different_git_hashes_as_different_version_info`：确认同版本字符串但不同 git hash 的两个节点得到 50%，返回差异列表且不标记全一致。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前只执行任务指定的 11 章节结构校验并人工复查链接、事实限定和扩展建议。
