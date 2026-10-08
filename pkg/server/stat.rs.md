# `pkg/server/stat.rs`

## 文件定位

`pkg/server/stat.rs` 属于 `astersql-server` crate；crate 根 `pkg/server/lib.rs` 以 `pub mod stat` 公开该模块。它为 `server::Server` 增加服务器级状态查询能力，当前只覆盖 TLS 服务端证书有效期与进程 `Uptime`。这里的“统计”是 MySQL `SHOW STATUS` 风格的状态变量，不是查询优化器统计信息，也不是 `COM_STATISTICS` 的连接摘要；后者由 `pkg/server/conn.rs::ClientConn::writeStats` 独立生成。

当前 Rust 生产代码中没有 `Server::statistics` 或 `Server::status_scope` 的调用点，也没有将 `Server` 实现并注册到 `pkg/sessionctx/variable/statusvar.rs::Statistics` 聚合链。已确认的调用者是 `pkg/server/stat_test.rs` 和 `pkg/server/tests/tls/tls_test.rs`。因此本文件现在是公开、可直接调用且有测试覆盖的状态快照 API，但尚不能据此断言 SQL 状态变量聚合路径已经接线。

## 核心职责

1. 用 `SSL_SERVER_NOT_AFTER`、`SSL_SERVER_NOT_BEFORE`、`UPTIME` 固定对外状态名，避免调用者重复拼写 MySQL 兼容名称。
2. 用 `StatusScope` 表达状态变量的可见范围；`Server::status_scope` 当前对任意名称都返回 `GlobalAndSession`。
3. 用 `StatusValue` 将字符串证书时间与整数运行秒数放入同一个 `HashMap<String, StatusValue>`。
4. `Server::statistics` 始终先建立三个默认项，再尽力从 `Server` 的 TLS 与 Domain 快照覆盖它们；某一路信息缺失不会影响其他项返回。

## 主要符号

- `SSL_SERVER_NOT_AFTER: &str`：键名 `Ssl_server_not_after`，值为证书“不晚于”时间；无元数据时为空字符串。
- `SSL_SERVER_NOT_BEFORE: &str`：键名 `Ssl_server_not_before`，值为证书“不早于”时间；无元数据时为空字符串。
- `UPTIME: &str`：键名 `Uptime`，通常表示服务器运行秒数；无 Domain 时为 `0`。若 Domain 提供未来启动时间，当前实现会返回负值。
- `StatusScope::{Global, Session, GlobalAndSession}`：状态作用域枚举。三个变体都属于公开 API，但当前 `status_scope` 只产生 `GlobalAndSession`。
- `StatusValue::{String(String), Integer(i64)}`：本模块自己的状态值枚举。它不同于 `pkg/sessionctx/variable/statusvar.rs::StatusValue` 的类型擦除包装器，不能直接代入后者的 `Statistics` trait。
- `Server::status_scope(&self, _name: &str) -> StatusScope`：忽略名称并返回 `GlobalAndSession`。`pkg/server/stat_test.rs::statistics_include_tls_dates_and_non_negative_uptime` 还验证未知名称也得到相同结果。
- `Server::statistics(&self) -> HashMap<String, StatusValue>`：生成包含三个固定键的即时快照；没有 `Result`，所有可用性失败均以默认值表达。

## 执行流程

调用 `Server::statistics` 后依次发生：

1. 建立三个默认条目：两个 TLS 时间为空字符串，`Uptime` 为 `0`。
2. 通过 `Server::tls_config` 取得当前 TLS 配置的 `Arc` 快照。若存在，`not_after` 和 `not_before` 字符串字段分别优先；相应字符串缺失时，才把兼容字段 `not_after_unix`、`not_before_unix` 转成十进制字符串。两个方向独立选择，因此可只覆盖其中一个默认值。
3. 通过 `Server::domain` 取得当前 Domain 的 `Arc` 快照。若存在，读取系统当前 Unix 秒，并减去 `Domain::start_timestamp()`；使用 `saturating_sub` 防止整数下溢/上溢。
4. 返回整个 `HashMap`。键集合固定为三个，但 `HashMap` 不保证迭代顺序。

真实证书加载路径在 `pkg/server/server.rs::load_tls_config`：它把解析出的证书日期格式化为字符串写入 `TlsConfig::not_before/not_after`；`Server::reload_tls_config` 再通过 `update_tls_config` 原子式替换配置快照。Unix 整数字段主要保留给兼容和测试路径。

## 数据与状态

本文件不持有全局变量或缓存，每次调用都新建并返回独立 `HashMap`。输入状态来自 `Server`：

- `Server::tls_config` 位于 `RwLock<Option<Arc<TlsConfig>>>` 中。读方法克隆 `Arc`，所以后续热加载不会改变本次统计读取到的快照。
- `Server::domain` 位于 `RwLock<Option<Arc<dyn Domain>>>` 中。`Domain::start_timestamp()` 的单位必须是 Unix 秒，才能与 `SystemTime` 计算一致。
- 当前时间早于 Unix epoch 时，`duration_since(UNIX_EPOCH)` 的错误被 `map_or(0, ...)` 转为 `0`；随后仍按饱和减法计算。
- `StatusValue` 派生 `Clone/Debug/PartialEq`，适合快照传递与断言；`StatusScope` 还派生 `Copy/Eq`。

不变量是：正常返回时三个键始终存在；TLS 或 Domain 缺失只改变值，不改变键集合。TLS 文本字段优先于 Unix 兼容字段也是明确的选择顺序。

## 依赖与调用关系

上游边界：

- `pkg/server/lib.rs` 公开 `stat` 模块，并在测试构建中把 `pkg/server/stat_test.rs` 作为独立测试模块挂载。
- `pkg/server/stat_test.rs` 直接调用 `Server::statistics` 和 `Server::status_scope`。
- `pkg/server/tests/tls/tls_test.rs::test_tls_basic` 通过真实证书加载与 TLS 握手后调用 `statistics`，验证公开证书日期。
- RustCodeGraph 将 `stat.rs` 标为被 5 个文件引用，但逐文件文本核验表明 `http_status.rs`、`pg_catalog.rs`、`pg_extended.rs`、`sysvar_builtins.rs` 没有调用这些符号；这是文件级依赖推断噪声，不能作为调用边。

下游边界：

- `Server::tls_config`、`TlsConfig::{not_before, not_after, not_before_unix, not_after_unix}` 来自 `pkg/server/server.rs`。
- `Server::domain` 与 `Domain::start_timestamp` 来自同一文件。
- 标准库 `SystemTime/UNIX_EPOCH` 提供当前时间，`HashMap` 承载返回值。
- `pkg/server/Cargo.toml` 声明 crate 名为 `astersql-server`，库入口为 `lib.rs`；本文件自身只直接使用标准库和 crate 内 `Server`，没有新增外部依赖或 feature 条件。

## 错误处理与边界

API 不返回错误。缺少 TLS、缺少单个证书日期或缺少 Domain 时保留对应默认值；这保证一个来源不可用不会阻止其他状态项返回。它不会在查询时解析证书，因此证书解析错误应在 `server.rs` 的加载/重载阶段处理。

锁中毒不是这里的可恢复分支：`Server::tls_config` 与 `Server::domain` 都以 `expect(...)` 读取 `RwLock`，中毒时会 panic。时间源早于 epoch 被折叠为 `0`，而饱和减法只避免算术溢出；若启动时间位于未来但差值仍可表示，结果是负数。未知状态名对 `status_scope` 没有报错或特殊分支。

与 Go 相比，Rust 返回类型不含 `Result`，也没有动态证书回调、解析和日志告警路径；这些并非本函数内的“成功支持”，而是当前接口和接线差异。

## 并发与资源生命周期

`statistics(&self)` 可在共享 `Server` 上并发调用；它只短暂读取 TLS 和 Domain 的 `RwLock`，克隆各自的 `Arc` 后即释放锁，随后在局部快照上工作。函数不启动线程、任务或通道，不持有锁跨越证书字段读取或时间计算，也不修改 Server。

TLS 热加载由 `Server::update_tls_config/reload_tls_config` 替换整个 `Arc<TlsConfig>`。并发查询要么看到旧快照，要么看到新快照，不会看到字段逐个更新的中间态；`pkg/server/tests/tls/tls_test.rs::test_update_tls_config_runtime_state` 验证旧 `Arc` 快照在替换后仍保持旧内容，并验证传入 `None` 可清空配置。Domain 也以克隆 `Arc` 的方式读取。返回的字符串和映射拥有自己的数据，不借用锁内对象。

## 与 Go 版本的对应关系

Go 对照为 `pkg/server/stat.go`：`GetScope`、`Stats`、三个同名状态键和“先填默认值，再尽力覆盖”的失败隔离策略均被保留。`pkg/server/stat_test.go::TestUptime` 验证 Go 版本从 `infosync.GetServerInfo` 计算 Uptime；Rust 独立测试改为注入实现 `Domain::start_timestamp` 的 `TestDomain`。

存在以下已核实差异：

- Go `defaultStatus` 将 TLS 两项声明为全局加会话、将 `Uptime` 声明为仅全局，但 `GetScope` 实际对任意名称返回 `variable.DefaultStatusVarScopeFlag`。Rust 没有复制 `defaultStatus` 中逐项 Scope，`status_scope` 始终返回 `GlobalAndSession`；测试固定了该当前行为。
- Go `Stats` 实现 `variable.Statistics` 约定，返回 `map[string]any, error`，并在 `pkg/server/server.go::NewServer` 中执行 `variable.RegisterStatistics(s)`。Rust 使用本地 `StatusValue` 和无错误返回，目前既未实现 `pkg/sessionctx/variable/statusvar.rs::Statistics`，也未注册。
- Go 可调用 `tls.Config.GetCertificate` 并在查询时解析叶子证书，也可解析单个静态证书；失败时记录日志。Rust 查询预先写入 `TlsConfig` 的日期字符串，字符串缺失才回退 Unix 数字，没有查询时解析或日志。
- Go 从全局 infosync server info 获取启动时间；Rust 从挂到当前 `Server` 的 `Domain` 获取，使用饱和减法并在缺失时返回 `0`。

因此，Rust 文件对核心值语义做了局部移植，但状态聚合注册、作用域类型和动态证书获取仍不是 Go 路径的完整同构实现。

## 扩展指南

新增状态项时，至少同步修改键常量、`statistics` 的默认映射和覆盖逻辑，并在 `pkg/server/stat_test.rs` 增加缺省值、正常值和来源缺失测试。若数据来自 `Server` 共享状态，应沿用“锁内克隆快照、锁外计算”的模式，避免扩大锁持有时间；若值可能失败，应先决定继续用默认值表达，还是扩展 API 的错误模型。

若要完成 `SHOW STATUS` 接线，不应仅在本文件增加调用：需要在不混淆两个 `StatusValue` 类型的前提下，让 `Server` 适配 `pkg/sessionctx/variable/statusvar.rs::Statistics`，并在 Server 构造/销毁生命周期中成对注册、注销。必须补独立测试验证注册聚合、重复实例生命周期、作用域和错误传播，避免只测试直接方法。

证书格式变更要同时核对 `pkg/server/server.rs::load_tls_config` 和 `pkg/server/tests/tls/tls_test.rs::test_tls_basic`，并注意外部兼容性：当前真实加载路径输出 OpenSSL 风格时间字符串，而 Unix 回退输出十进制秒。修改 Uptime 时需保持秒单位，并明确未来启动时间、epoch 前系统时钟和整数边界的兼容行为。测试继续放在独立 `stat_test.rs` 或集成测试目录，不要内嵌进生产源文件。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件及 `pkg/server/stat.rs`；`files --filter pkg/server/stat.rs` 报告该文件含 16 个符号；`node --file pkg/server/stat.rs --offset 1 --limit 500` 展示完整 110 行源码及文件级使用者。精确方法查询 `node/callers/callees stat.rs::Server::statistics` 未命中，故没有把缺失的方法级图边当成事实。
- 源码：`pkg/server/stat.rs`；状态来源与同步机制：`pkg/server/server.rs::{TlsConfig, Domain, Server, load_tls_config, update_tls_config, reload_tls_config, tls_config, domain}`；模块入口：`pkg/server/lib.rs`；crate 声明：`pkg/server/Cargo.toml`。
- Rust 测试：`pkg/server/stat_test.rs::statistics_include_tls_dates_and_non_negative_uptime`；真实 TLS 集成：`pkg/server/tests/tls/tls_test.rs::{test_tls_basic, test_update_tls_config_runtime_state}`。
- Go 对照：`pkg/server/stat.go::{defaultStatus, Server.GetScope, Server.Stats}`、`pkg/server/server.go::NewServer` 的注册调用，以及 `pkg/server/stat_test.go::TestUptime`。
- 聚合接口对照：`pkg/sessionctx/variable/statusvar.rs::{StatusValue, Statistics, RegisterStatistics, UnregisterStatistics, GetStatusVars}`。仓库文本搜索确认 Rust 生产代码中不存在对本文件两个方法的调用，也不存在 Server 的对应 trait 实现/注册。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定命令校验 11 个固定二级标题，并人工复核上述符号、调用边、当前未接线事实和扩展风险。
