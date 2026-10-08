# `pkg/plugin/helper.rs`

## 文件定位

[`helper.rs`](helper.rs) 是 `astersql-plugin` crate 的清单适配、插件 ID 解析和测试装载辅助模块。crate 入口 [`lib.rs`](lib.rs) 通过 `pub mod helper` 声明该模块，并以 `pub use helper::*` 将全部公开符号重导出到 crate 根；[`Cargo.toml`](Cargo.toml) 指定库入口为 `lib.rs`，没有为本模块设置 feature 条件或生产外部依赖，唯一列出的 crate 级依赖是测试使用的 `serial_test`。

该文件位于 SPI 类型与插件运行时之间：[`audit.rs`](audit.rs) 和 [`spi.rs`](spi.rs) 定义通用/专用 `Manifest` 及扩展回调的存取方式，[`plugin.rs`](plugin.rs) 使用 `Id::decode` 驱动加载，并提供 `load_plugin_for_test` 所调用的全局注册、加载、初始化和关闭操作。它不负责动态库 I/O、版本校验细节、插件遍历或事件派发。

## 核心职责

- `declare_audit_manifest` 与 `declare_authentication_manifest` 将通用 `Manifest` 重新包装为专用清单，并从 `Manifest.extension` 恢复导出阶段保存的 `Arc` 回调。
- `declare_schema_manifest` 与 `declare_daemon_manifest` 为没有额外回调字段的清单提供对称的声明入口，只移动并包装基础 `Manifest`。
- `Id` 为 `name-version` 插件签名提供自有字符串包装；`Id::decode` 从最后一个连字符处分离名称和版本，供 `plugin.rs::load` 与 `load_one` 使用。
- `load_plugin_for_test` 构造内存中的静态审计插件，重置旧的进程级插件状态，依次执行 `load` 和 `init`，让集成测试能够注入 `GeneralEventCallback` 而无需真实 `.so`。

## 主要符号

### `declare_audit_manifest(manifest: Manifest) -> AuditManifest`

函数消费基础清单，先调用 `audit_callbacks(&manifest)` 对 `extension` 做 `AuditCallbacks` 类型的 downcast，再克隆四个可选 `Arc` 回调：`on_connection_event`、`on_general_event`、`on_global_variable_event`、`on_parse_event`。若扩展为空或类型不匹配，四项都成为 `None`，基础 `manifest` 仍原样进入返回值。它与 `audit.rs::ExportManifest for AuditManifest` 组成导出/恢复对。

### `declare_authentication_manifest(manifest: Manifest) -> AuthenticationManifest`

逻辑与审计清单相同，但通过 `authentication_callbacks` 恢复 `authenticate_user`、`generate_authentication_string`、`validate_authentication_string` 和 `set_salt`。克隆的是 `Arc` 句柄而非回调实现；`helper_test.rs::test_plugin_declare` 用 `Arc::ptr_eq` 证明恢复后仍指向同一回调对象。

### `declare_schema_manifest` 与 `declare_daemon_manifest`

两者分别返回 `SchemaManifest { manifest }` 和 `DaemonManifest { manifest }`。这两类清单在 `spi.rs` 中只有基础清单字段，故没有扩展提取、分支或失败路径。

### `pub struct Id(pub String)` 与 `Id::decode`

`Id` 是公开元组结构体，派生 `Clone + Debug + PartialEq + Eq + Hash`。`decode` 返回 `Result<(String, String), PluginError>`，使用 `rsplit_once('-')` 从最右侧拆分：例如 `audit-log-2` 得到 `("audit-log", "2")`。没有任何连字符时返回 `PluginErrorKind::InvalidPluginId`，消息为 `invalid plugin ID: <原值>`。函数只检查分隔符存在，不拒绝空名称、空版本，也不解析版本数字；数值和 manifest 一致性由 `plugin.rs::load_one` 的后续逻辑处理。

### `load_plugin_for_test(callback: GeneralEventCallback) -> Result<(), PluginError>`

该公开测试辅助函数接受线程安全的 `Arc<dyn Fn... + Send + Sync>` 回调。它注册名为 `audit_test`、版本为 `1` 的静态 `AuditManifest`：`validate`、`on_init`、`on_shutdown` 和连接事件回调均成功，参数回调放入 `on_general_event`，另外两种审计事件留空。随后使用签名 `audit_test-1`、空插件目录、`skip_when_fail = false` 和环境版本 `go=1112` 执行真实的 `load`/`init` 流程。

## 执行流程

清单恢复路径如下：

1. 插件或测试通过 `export_manifest` 把专用清单转成通用 `Manifest`；审计/认证实现将额外回调封装进类型擦除的 `extension`。
2. 运行时持有或克隆通用 `Manifest`。事件派发侧调用对应 `declare_*_manifest`。
3. 审计/认证声明函数尝试 downcast 扩展并克隆回调句柄；Schema/Daemon 声明函数直接包装基础清单。
4. 调用方检查返回清单中的 `Option` 并执行存在的回调。例如 `integration_test.rs::emit_query_events` 在 `foreach_plugin(Kind::Audit, ...)` 中恢复审计清单并触发通用事件。

插件 ID 的生产路径为 `Config.plugins` 字符串 → `plugin.rs::load` 构造 `Id` 并首次 `decode` 做重复名检查 → `load_one` 再次 `decode` 取得名称和版本文本 → 静态工厂、测试 hook 或 loader 提供 manifest → 后续名称/版本校验及 `Plugin` 构造。

`load_plugin_for_test` 的顺序具有语义：先 `clear_static_plugins`、`set_test_hook(None)`、`shutdown(default context)` 清理旧测试状态；再注册静态工厂；之后构造配置并调用 `load`，成功后调用 `init`。任一步返回错误即通过 `?` 停止，`init` 的返回值直接作为函数结果。

## 数据与状态

本文件自身只定义 `Id(String)`，不声明静态变量。四个声明函数的输入和返回值拥有 `Manifest`；回调通过 `Arc` 克隆共享，因此恢复清单不会复制闭包内部状态，也不会从原 `extension` 转移唯一所有权。

实际可变状态来自 `plugin.rs` 的进程级全局集合、静态插件注册表和测试 hook。`load_plugin_for_test` 会清空注册表、移除测试 hook、关闭并取走旧全局插件集合，随后写入新的 `audit_test` 插件集合；成功返回后该插件仍保持全局 `Ready` 状态，函数不会自动 shutdown。传入回调被 `move` 捕获到静态工厂，再克隆进导出的审计扩展，生命周期因此至少延续到注册表和已加载 manifest 被清理。

`Id::decode` 每次为名称和版本各分配一个新的 `String`。它不缓存结果，也不改变 `Id`。

## 依赖与调用关系

下游依赖均由 crate 根重导出：

- `audit_callbacks`、`AuditManifest`、`GeneralEventCallback` 来自 `audit.rs`；前者对 `Manifest.extension` 做 `AuditCallbacks` downcast。
- `authentication_callbacks`、`AuthenticationManifest`、`SchemaManifest`、`DaemonManifest`、`Manifest`、`Context` 来自 `spi.rs`。
- `Config`、`clear_static_plugins`、`register_static_plugin`、`set_test_hook`、`load`、`init`、`shutdown` 来自 `plugin.rs`。
- `Kind` 来自 `const.rs`，`PluginError`/`PluginErrorKind` 来自 `errors.rs`，`Arc` 来自标准库。

直接上游调用证据包括：`plugin.rs::load` 和 `load_one` 调用 `Id::decode`；`helper_test.rs` 覆盖全部四个声明函数及非法 ID；`spi_test.rs` 验证审计生命周期钩子与事件回调的导出/恢复；`integration_test.rs` 和 `conn_ip_example/conn_ip_example_test.rs` 在遍历已加载审计插件时调用 `declare_audit_manifest`；`integration_test.rs` 使用 `load_plugin_for_test` 注入事件记录回调。当前搜索未发现生产模块直接调用四个 `declare_*` 或 `load_plugin_for_test`，后者名义和行为均属于测试基础设施。

RustCodeGraph 精确列出了 `helper.rs` 的 11 个符号，并通过文件节点给出源码；`explore` 对全仓常见名称产生大量同名结果，精确 `callers declare_audit_manifest` 在 30 秒窗口内未返回。因此上述调用边使用目标目录内的精确 Rust 符号检索补证，没有把泛名 `Id`/`decode` 的跨仓结果归入本模块。

## 错误处理与边界

- 四个 `declare_*` 函数不返回 `Result`。审计或认证扩展缺失/类型错误会静默降级为全部回调 `None`，但基础 manifest 保留；调用方必须把缺失回调视为可选能力，不能据此假定导出一定正确。
- `Id::decode` 的唯一错误条件是没有 `-`。多连字符按最后一个分隔，`-1`、`name-` 和 `-` 当前都会成功；若上层需要非空名称、数字版本等约束，应在明确兼容 Go 行为后增加测试，而不能把当前函数描述为完整格式校验器。
- ID 错误使用 `PluginErrorKind::InvalidPluginId`。`load` 和 `load_one` 通过 `?` 原样传播；测试只断言错误存在，尚未核对 kind、消息或上述边界输入。
- `load_plugin_for_test` 会传播静态注册、加载、校验和初始化错误。清理函数 `clear_static_plugins`、`set_test_hook`、`shutdown` 的实现对锁中毒采用尽力而为/无返回值，因此清理失败不会在该函数的返回类型中显式报告，后续注册或加载可能成为首个可见失败点。
- 配置固定 `skip_when_fail = false`，所以测试辅助不会吞掉插件失败。成功注册但后续 `load`/`init` 失败时没有事务式回滚，注册表或全局集合可能保留部分状态，调用测试应在串行隔离和后续清理方面保持谨慎。

## 并发与资源生命周期

四个声明函数和 `Id::decode` 不创建线程、锁、通道、文件或网络资源。回调类型要求 `Send + Sync`，并由 `Arc` 共享；本文件只克隆引用计数句柄，线程安全边界由回调 trait bound 和回调自身实现保证。

`load_plugin_for_test` 操作的是 `plugin.rs` 中由 `OnceLock<RwLock<...>>` 保护的全局状态。单个读写操作有锁保护，但“清理 → 注册 → load → init”整体不是一个原子事务；并行测试可相互清空、覆盖或关闭插件。因此调用它的 `integration_test.rs` 测试以 `#[serial]` 串行运行，crate 的 `serial_test` 开发依赖是这一隔离策略的直接证据。

辅助函数创建的 `Context` 只用于当前 `load/init` 调用；注册工厂与回调则存活在全局注册表/插件集合中。它不启动 watcher，因为配置 `etcd: None` 且测试 manifest 没有 `on_flush`。旧插件在前置 `shutdown` 中进入关闭路径，新插件需要调用方或下一次辅助调用显式 shutdown/重置；函数没有 RAII guard 自动恢复之前状态。

## 与 Go 版本的对应关系

Go 对照文件 [`helper.go`](helper.go) 提供同名的 `DeclareAuditManifest`、`DeclareAuthenticationManifest`、`DeclareSchemaManifest`、`DeclareDaemonManifest`、`ID.Decode` 和 `LoadPluginForTest`。

Go 的四个声明函数依赖结构布局并通过 `unsafe.Pointer` 将 `*Manifest` 强制转换为专用清单；Rust 不能安全复刻这种别名/布局转换，因此 `export_manifest` 把额外回调存入类型擦除扩展，声明函数再 downcast 和克隆 `Arc`。Schema/Daemon 没有额外字段，Rust 直接进行拥有所有权的包装。结果目标相同，但 Rust 不保留 Go 指针身份；错误扩展类型也不会造成未定义行为，而会得到空回调。

两侧 ID 都按最后一个 `-` 拆分，并且都只在完全没有分隔符时报告非法 ID，因此名称可含连字符，空片段也未在此层拒绝。Go 返回命名字符串和 `errInvalidPluginID.GenWithStackByArgs`；Rust 返回拥有所有权的字符串元组及不带 Go errno/栈的 `PluginError`。

Go `LoadPluginForTest` 安装 `SetTestHook(loadOne)` 来绕过真实动态库，并用 `testing.T` 内部断言 `Load/Init` 成功；Rust 改为注册静态工厂，主动清除旧静态插件/test hook/全局集合，并用 `Result` 把错误交给调用方。两者构造相同的 `audit_test-1`、`go=1112` 环境和成功生命周期回调，但 Rust 版本的额外全局清理是为可重复测试服务的迁移差异，不应描述为普通生产加载 API。

## 扩展指南

- 增加新的专用 manifest 时，应在其所有者模块实现对称的 `ExportManifest` 扩展存储/提取，再在本文件添加声明适配；必须同时验证基础字段、每个额外回调的 `Arc` 身份，以及扩展缺失或类型错误时的策略。
- 修改 `Id::decode` 前需同步检查 `plugin.rs::load/load_one` 和 Go `helper.go::ID.Decode`。收紧空名称、空版本或版本数字规则会改变兼容性；应在独立 [`helper_test.rs`](helper_test.rs) 与 [`helper_test.go`](helper_test.go) 增加对照用例，不要把测试写回生产源文件。
- 扩展测试插件时，优先修改 `load_plugin_for_test` 中的 `Manifest`/`AuditManifest` 工厂及 [`integration_test.rs`](integration_test.rs) 的调用断言。若增加 flush、etcd 或后台线程，必须定义 watcher 的取消与 shutdown 顺序，避免测试退出后泄漏线程。
- 任何操作全局插件状态的新测试都应遵循现有 `#[serial]` 隔离，并在失败路径检查残留注册表和全局集合。不要假定多个锁保护就等同于完整装载事务。
- 声明函数位于事件派发热路径时只做 `Arc` 克隆和 downcast；增加深拷贝、日志或锁会改变成本。公共 `Id` 元组字段和函数签名由 crate 根重导出，变更会影响 `plugin.rs` 及外部 crate 的源码兼容性。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/plugin` 列出 31 个相关 Go/Rust 文件；`node --file pkg/plugin/helper.rs --offset 1 --limit 400` 完整读取目标文件 128 行；`query` 精确定位四个 `declare_*` 函数和 `load_plugin_for_test`。`explore` 因 `Id`/`helper` 等泛名返回大量跨仓同名项，精确 `callers declare_audit_manifest` 在 30 秒内无结果，故使用精确源码引用检索补足调用边。
- 目标与 crate 边界：`pkg/plugin/helper.rs`、`pkg/plugin/Cargo.toml`、`pkg/plugin/lib.rs`。目录中不存在 `doc.go`；最近的模块说明是 `pkg/plugin/README.md`，仅链接插件框架设计文档。
- 直接 Rust 证据：`pkg/plugin/audit.rs` 的 `AuditManifest`、`ExportManifest`、`audit_callbacks`；`pkg/plugin/spi.rs` 的通用/认证/Schema/Daemon manifest 及 `authentication_callbacks`；`pkg/plugin/plugin.rs` 的全局注册表、`load`、`load_one`、`init`、`shutdown`；`pkg/plugin/errors.rs` 的错误分类。
- 调用与测试：`pkg/plugin/helper_test.rs::test_plugin_declare/test_decode`、`pkg/plugin/spi_test.rs`、`pkg/plugin/integration_test.rs::emit_query_events` 及其串行集成测试、`pkg/plugin/conn_ip_example/conn_ip_example_test.rs`。Go 对照为 `pkg/plugin/helper.go`、`pkg/plugin/helper_test.go`、`pkg/plugin/plugin.go` 和 `pkg/plugin/integration_test.go`。
- 本任务仅新增静态说明，按计划未运行 Cargo。交付前运行任务指定的结构命令，确认文件存在且恰有 11 个固定二级标题；人工检查本文能回答文件为何存在、清单/ID/测试加载路径如何运行、全局状态风险及安全扩展位置。
