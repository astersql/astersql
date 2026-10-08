# `pkg/plugin/conn_ip_example/conn_ip_example.rs`

## 文件定位

本文件实现一个名为 `conn_ip_example`、版本为 `1` 的审计插件示例。它属于 Cargo 包 `astersql-plugin-conn_ip_example`：该包以同目录 `lib.rs` 为 crate 根并依赖父目录的 `astersql-plugin`；根 `Cargo.toml` 将其列为 workspace 成员，并以 `facade_plugin_conn_ip_example` 别名提供给 `pkg/lib.rs` 的门面模块。`lib.rs` 公开重导出本文件的符号，因此外部使用者可以从 crate 根访问 `plugin_manifest()` 等 API。

`plugin_manifest()` 是框架接线入口：它生成 `Kind::Audit` 的 `AuditManifest`，将基础生命周期回调与连接、通用语句事件回调交给插件框架。仓库内可见的完整加载路径由独立测试 `conn_ip_example_test.rs::test_load_plugin` 证明：测试钩子导出该清单，随后依次调用 `load`、`init`、`foreach_plugin` 和 `shutdown`。同目录 `manifest.toml` 则记录 Go 动态插件的同名、同版本和对应扩展点；它不是本 Rust 文件读取的运行时配置。

## 核心职责

- `plugin_manifest()` 描述插件身份并注册 `validate`、`on_init`、`on_shutdown`、`on_general_event` 和 `on_connection_event`；全局变量事件和解析事件显式留空。
- `on_init()` 创建示例系统变量 `conn_ip_example_key`，演示取值规范化及会话级、全局级设置回调，并重置连接事件计数。
- `on_general_event()` 输出一次语句事件的会话状态、SQL、摘要、表、用户、事件阶段和命令；清单中的适配闭包负责把框架数据转换成本文件的简化视图。
- `on_connection_event()` 输出拒绝原因和连接信息，每收到一次回调就原子递增计数；`on_shutdown()` 将计数归零。
- 本实现是教学/测试用途的可执行示例，而不是按 IP 拒绝连接的策略实现：它记录 `host` 和拒绝原因，但不作准入判断。

## 主要符号

- `SYSTEM_VARIABLE_NAME`、`SCOPE_GLOBAL`、`SCOPE_SESSION`：定义变量名以及两个可按位组合的作用域标志；`on_init()` 使用 `SCOPE_GLOBAL | SCOPE_SESSION`。
- `CONNECTIONS: AtomicI32`：进程内共享的连接事件累计数。它统计回调次数，并不表示当前活跃连接数，因为所有连接事件类型都会递增，且仅初始化/关闭时清零。
- `ValidationCallback`、`SetSessionCallback`、`SetGlobalCallback`：线程安全的动态回调类型，均以 `Arc<dyn Fn + Send + Sync>` 保存并以 `PluginError` 传播失败。
- `SystemVariable`：保存名称、作用域、默认值及三个可选钩子。`variables()` 返回由 `OnceLock<RwLock<HashMap<...>>>` 延迟建立的全局注册表；`register_system_variable()` 覆盖同名项，`get_system_variable()` 返回克隆快照。
- `StatementContext`、`SessionVars`、`ConnectionInfo`：本示例的本地数据视图，避免事件打印逻辑直接依赖框架结构的全部字段。
- `validate()`：只打印被调用信息并返回成功，不验证清单或环境。
- `on_init()`、`on_shutdown()`：插件生命周期回调，分别注册变量/清零计数和读取变量/清零计数。
- `on_general_event()`：处理 `GeneralEvent::{Starting, Completed, Error}`，无返回值，也不能拒绝语句。
- `on_connection_event()`：读取 `RejectReasonContextKey`，打印连接数据，原子递增后返回成功。
- `rejection_reason()`：包内可见的上下文读取辅助函数；键缺失时返回空字符串。
- `connection_count()`：以 `SeqCst` 读取连接事件累计数，供观察和测试断言。
- `plugin_manifest()`：公开的框架清单构造器及数据适配边界。
- `plugin_session()`、`plugin_connection()`：分别构造仅填入连接 ID、或用户和主机的框架对象；源码将它们标注为测试辅助，当前同目录测试未调用这两个函数。

## 执行流程

1. 加载方调用 `plugin_manifest()`。函数通过 `Manifest::new(Kind::Audit, "conn_ip_example", 1)` 建立基础清单，填入三个生命周期回调，再构建 `AuditManifest`。
2. 框架验证时调用 `validate()`；当前实现打印标记后无条件返回 `Ok(())`。
3. 框架初始化时调用 `on_init()`。它组装 `SystemVariable`，其中校验器把已经规范化的值转为 ASCII 小写，两个设置回调只打印日志；随后注册变量、读回并打印默认值 `v1`，最后用原子交换把计数设为零。
4. 通用事件到达清单闭包后，闭包把框架 `SessionVars` 映射成本地 `SessionVars`：复制状态和原始 SQL；当前把 `digest` 固定为空字符串；将每个表格式化为 `db.table`；用户为空时使用 `connection:<connection_id>` 作为展示值。若框架会话本身为 `None`，则继续以 `None` 调用打印函数。
5. `on_general_event()` 在会话存在时打印其字段，再按 Starting、Completed 或 Error 打印阶段，最后始终打印命令字符串。
6. 连接事件到达清单闭包后，闭包克隆框架连接信息的 `user`、`host`、`database` 和 `connection_type` 到本地结构，再调用 `on_connection_event()`。该函数读取可选拒绝原因、打印数据、执行一次 `fetch_add(1, SeqCst)` 并返回成功。
7. 关闭时 `on_shutdown()` 尝试读回系统变量并打印其值，然后以原子交换清零计数。它不从全局注册表删除变量。

## 数据与状态

状态分为两个进程级对象。`CONNECTIONS` 是静态原子整数，生命周期贯穿进程；`on_init()` 和 `on_shutdown()` 都会重置它。系统变量注册表由函数内的 `OnceLock` 惰性创建，内部 `RwLock<HashMap<String, SystemVariable>>` 以变量名为键；重复注册执行替换，而不是报错。注册表没有注销或清空入口，所以关闭后 `conn_ip_example_key` 仍可被读取。

`SystemVariable` 及其回调被克隆时共享 `Arc` 中的闭包。`get_system_variable()` 克隆整项后释放读锁，调用者不会持锁操作回调。事件数据则是每次回调构造的临时拥有值：框架对象中的字符串被克隆，表名被新建为 `db.table` 字符串，本地视图只活到事件函数返回。

重要不变量是：成功取得注册表写锁才会注册，成功取得读锁才会返回变量；锁中毒与变量缺失在当前 API 中都表现为 `None` 或静默跳过。连接计数每次进入 `on_connection_event()` 恰好增加一次，且该函数当前没有在递增前返回错误的分支。

## 依赖与调用关系

直接依赖全部来自 `astersql_plugin`：`Manifest`/`Kind` 表示基础插件清单，`AuditManifest` 承载审计扩展回调，`Context` 提供类型化上下文值，`GeneralEvent` 与 `ConnectionEvent` 表示事件种类，框架 `SessionVars` 和 `ConnectionInfo` 是适配输入，`PluginError` 是回调错误类型。标准库提供 `HashMap`、`Arc`、`OnceLock`、`RwLock` 和原子操作。

本文件内部调用边为：`plugin_manifest()` 直接引用三个生命周期函数，并由两个闭包下调 `on_general_event()` 和 `on_connection_event()`；后者继续调用 `rejection_reason()`；初始化和关闭都调用 `get_system_variable()`，初始化还调用 `register_system_variable()`；注册与查询最终访问 `variables()`。RustCodeGraph 对目标文件识别出 18 个符号；精确 `query plugin_manifest --kind function` 定位到第 218 行。图的 `callers`/`callees` 查询已执行但未返回可用边，因此上述边由索引的文件源码和独立测试调用点交叉核验。

框架侧 `AuditManifest::export_manifest()` 会把四类可选回调包装进 `Manifest.extension`，加载后再由审计声明/提取逻辑取回。当前仓库搜索到 `plugin_manifest()` 的直接外部调用者是 `conn_ip_example_test.rs::test_load_plugin`；该测试导出清单后通过框架遍历间接触发事件。不能据此声称生产服务器默认自动启用此示例，是否加载仍取决于插件配置或加载方。

## 错误处理与边界

所有声明为 `Result` 的本地回调当前都只返回 `Ok`；三个系统变量闭包也不产生错误。`rejection_reason()` 在键不存在时返回空字符串。与 Go 版本对上下文值直接作 `string` 类型断言不同，Rust 的类型化 `ContextKey` 把值类型固定为 `String`，避免运行时错误类型断言。

注册表的锁错误被有意降级：写锁失败时 `register_system_variable()` 静默不注册，读锁失败时 `get_system_variable()` 返回 `None`；初始化和关闭不会把这种失败传播为 `PluginError`。这意味着日志缺失可能同时表示变量不存在或锁已中毒。计数使用 `i32` 且无溢出处理；本示例也不区分 Connected、Disconnected 等连接事件，因此不应把数值解释为在线连接数。

通用事件的框架适配目前不保留真实 SQL 摘要，而是写入空字符串；用户为空时还会生成连接 ID 展示值。这两点均是 Rust 适配行为，不应在消费者中当作与 Go 会话字段完全等价。表名直接使用 `format!("{}.{}", db, table)`，数据库名为空时仍会产生前导点。`GeneralEvent` 当前被穷尽匹配为三种变体；若框架枚举将来新增变体，此处会在编译期要求更新，不存在 Go `default` 分支的运行时“unrecognized”输出。

## 并发与资源生命周期

全局注册表以 `OnceLock` 保证只初始化一次，以 `RwLock` 支持并发查询和互斥注册；读取返回克隆值，锁不会跨越日志输出或回调执行。回调由 `Arc` 管理并要求 `Send + Sync`，适合被框架跨线程共享。连接计数的读、加一和清零均采用最强的 `SeqCst` 顺序，提供单一全序；这些操作不与注册表形成复合事务。

生命周期上，`on_init()` 可以多次覆盖同名系统变量并清零计数，`on_shutdown()` 只清零计数而不销毁 `OnceLock`、注册表或已注册回调。若事件回调与 shutdown 并发，原子操作本身安全，但最终计数取决于交换与加一的全序位置：shutdown 返回前后可能仍观察到后来到达的事件。独立 Rust 测试使用 `serial_test::serial`，并在前后清理插件框架全局状态，说明测试必须隔离这些进程级资源。

## 与 Go 版本的对应关系

`Cargo.toml` 的 `package.metadata.porting.go-package` 明确指向 `pkg/plugin/conn_ip_example`。Rust 的 `validate`、`on_init`、`on_shutdown`、`on_general_event`、`on_connection_event` 与 Go 的 `Validate`、`OnInit`、`OnShutdown`、`OnGeneralEvent`、`OnConnectionEvent` 一一对应；变量名、默认值、双作用域、ASCII/字符串小写规范化、设置回调日志、连接信息日志及原子计数的基本意图保持一致。Rust 独立测试复刻 Go `TestLoadPlugin` 的加载、初始化、一次 Completed 事件、五次 Connected 事件、计数断言和 shutdown 清零断言，并额外验证拒绝原因存在/缺失两种情况。

已确认的差异包括：Rust 用进程内示例注册表代替 Go 服务器的 `variable.RegisterSysVar`；Rust `validate()` 不打印上下文本身；Rust 通过 `plugin_manifest()` 在代码中组装清单，而 Go 测试手工构造 `plugin.AuditManifest`，同目录 `manifest.toml` 也描述 Go 导出点；Rust 的框架会话适配丢弃真实 digest，并把表转换为字符串；Go 直接读取 `StmtCtx.SQLDigest()` 和原始表结构；Rust 用户回退为连接 ID，而 Go 直接打印 `sctx.User`；Rust 枚举匹配没有 Go 的未知事件兜底输出。此外，Rust 注册表锁失败会静默降级，这不是 Go 全局变量 API 在此示例中展示的行为。

## 扩展指南

- 新增审计事件时优先修改 `plugin_manifest()` 的对应可选字段，并在本地事件函数中实现行为；若新增全局变量或解析事件，需要同时补上目前为 `None` 的清单字段。
- 若需要真实 SQL 摘要、结构化表名或更多身份字段，应扩展 `StatementContext`/`SessionVars` 与清单适配闭包，先确认框架 `astersql_plugin::SessionVars` 已提供所需数据；不要只在打印函数中伪造值。
- 若把示例注册表用于真实配置，应让 `register_system_variable()` 和查询 API 区分锁中毒、缺失和成功，并设计 shutdown 注销语义；当前静默行为只适合示例。
- 若要按 IP 实施策略，应在 `on_connection_event()` 中明确限定事件类型、校验 `ConnectionInfo.host`，并返回可解释的 `PluginError`；同时确认框架如何处理该错误。不要复用 `CONNECTIONS` 充当在线连接数。
- 行为变更应同步独立测试 `pkg/plugin/conn_ip_example/conn_ip_example_test.rs`，保持 Rust 测试不嵌入生产源文件；对照语义还应检查 `conn_ip_example.go` 和 `conn_ip_example_test.go`。涉及全局状态的测试需继续串行并在前后清理状态。
- 兼容性风险集中在插件名称/版本、系统变量名、作用域和回调签名；性能风险主要来自每次事件的字符串克隆、表名格式化和同步标准输出。高频审计路径扩展时应避免持锁、减少分配，并评估日志量。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，其中 7,032 个 Rust 文件；`files --filter pkg/plugin/conn_ip_example` 列出本模块的 Go/Rust 实现、测试与 crate 入口。
- RustCodeGraph `node --file pkg/plugin/conn_ip_example/conn_ip_example.rs --offset 1 --limit 260` 及 `--offset 260 --limit 40`：核对本文件全部 278 行、类型、函数、闭包适配和原子操作；`query plugin_manifest --kind function` 唯一定位第 218 行。
- RustCodeGraph `node AuditManifest` 与 `node --file pkg/plugin/audit.rs --offset 180 --limit 90`：核对审计清单的四种可选回调、导出到 `Manifest.extension` 的方式以及拒绝原因键的值类型。
- RustCodeGraph 对 `plugin_manifest`、`on_general_event`、`on_connection_event` 执行了 `callers`/`callees` 查询，但未返回可用结果；因此用目标源码内部引用和仓库精确搜索确认调用边，并把“当前可见调用者”限定为测试证据。
- Cargo/入口证据：`pkg/plugin/conn_ip_example/Cargo.toml`、`pkg/plugin/conn_ip_example/lib.rs`、根 `Cargo.toml` 第 428/1376 行、`pkg/lib.rs` 第 1176-1177 行，以及同目录 `manifest.toml`。
- Go 对照与测试证据：`pkg/plugin/conn_ip_example/conn_ip_example.go`、`conn_ip_example_test.go`、`main_test.go`；Rust 独立测试证据：`conn_ip_example_test.rs::{test_reject_reason_from_context,test_load_plugin}`。
- 本任务为纯文档分析，按任务约束未运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核文档只描述有上述文件或符号支撑的当前行为。
