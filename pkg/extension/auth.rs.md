# `pkg/extension/auth.rs`

## 文件定位

`pkg/extension/auth.rs` 位于 `astersql-extension` crate，定义扩展框架的自定义认证与附加权限检查契约。crate 入口 `pkg/extension/lib.rs` 以 `pub mod auth` 声明模块并通过 `pub use auth::*` 对外再导出这些 API；`pkg/extension/Cargo.toml` 表明该文件直接依赖 `rustls`，并经 crate 内再导出使用 `astersql-privilege-conn`、`astersql-parser-auth` 和 `astersql-parser-mysql`。

该文件处在两条链路的交界处：注册期由 `WithCustomAuthPlugins` 把插件放入 `Manifest`，`newManifestWithSetup` 调用 `validateAuthPlugin` 拒绝无效声明；运行期由 `Extensions::GetAuthPlugins` 或 `newSessionExtensions` 汇总插件。当前 Rust 代码还在 `pkg/executor/utils.rs::encodePasswordWithPlugin` 中使用 `GenerateAuthString` 和 `ValidateAuthString`。仓库搜索未发现非测试 Rust 代码构造 `AuthenticateRequest`、`VerifyStaticPrivRequest`、`VerifyDynamicPrivRequest`，也未发现调用另外三个运行时回调，因此这些部分目前是已定义、可注册和可查询的契约，不能据此宣称登录认证与附加权限检查主链已经完整接线。

## 核心职责

1. 以 `AuthPlugin` 聚合插件名称、客户端插件协商要求以及认证串生成、认证串校验、登录认证、静态权限检查和动态权限检查回调。
2. 以 `AuthenticateRequest`、`VerifyStaticPrivRequest`、`VerifyDynamicPrivRequest` 固定回调入参，使用户名、主机、对象范围、角色、TLS 状态和认证握手数据的传递方式保持一致。
3. 以 `AuthConn` 隔离扩展插件与 privilege 层原始连接接口，并由 `AuthConnAdapter<T, C>` 负责转发认证数据包操作和注入 `Flush` 所需上下文。
4. 以 `validate_auth_plugins` 实施注册不变量：名称非空、批内唯一、不占用 MySQL 默认插件名，且三个核心回调必须存在；`validateAuthPlugin` 将该校验接到 `Manifest` 构建链。

本文件不选择具体插件、不保存用户凭据、不实现密码算法，也不负责插件的全局或会话级索引；这些职责分别位于调用方、插件实现、`extensions.rs` 和 `session.rs`。

## 主要符号

- `TlsConnectionState = rustls::ServerConnection`：TLS 服务端连接状态别名。它由 `Arc` 包装后放入请求，插件可据此读取当前 TLS 会话信息；它不是 Go `tls.ConnectionState` 的逐字段同型结构。
- `AuthConn: Send`：插件和客户端继续认证协商时的最小可变连接接口。`WriteAuthMoreData(&mut self, &[u8])` 写认证扩展数据，`ReadPacket(&mut self)` 读下一包，`Flush(&mut self)` 冲刷缓冲；三者统一返回 `ExtensionError`。
- `AuthConnAdapter<T, C>`：持有 `inner: T` 与 `context: C`。当 `T: privilege_conn::RawAuthConn<Context = C> + Send`、`C: Send` 时实现 `AuthConn`；`new` 装配二者，`into_inner` 消费适配器并返回底层连接，主要便于恢复所有权和检查其最终状态。
- `AuthenticateUserFunc`：`Arc<dyn Fn(AuthenticateRequest) -> Result<(), ExtensionError> + Send + Sync>`；错误表示认证失败，成功表示插件认可身份。
- `GenerateAuthStringFunc`：把明文输入转换为待存储认证串并返回成功标志。`ValidateAuthStringFunc` 检查已有认证串格式。二者已被 `encodePasswordWithPlugin` 使用。
- `VerifyPrivilegeFunc` 与 `VerifyDynamicPrivilegeFunc`：分别接受静态/动态权限请求并返回布尔值，表达 SQL 层原有授权之外的附加判定契约。
- `AuthPlugin`：公开字段的插件描述对象，派生 `Default` 后字符串为空、所有回调为 `None`。必填字段是 `Name`、`AuthenticateUser`、`GenerateAuthString`、`ValidateAuthString`；`RequiredClientSidePlugin`、`VerifyPrivilege`、`VerifyDynamicPrivilege` 可为空。
- `AuthenticateRequest`：包含 `User`、`StoredAuthString`、客户端 `InputAuthString`、握手 `Salt`、可选 `ConnState` 和独占的 `Box<dyn AuthConn>`。该类型未派生 `Clone`，连接对象随请求转移给回调。
- `VerifyStaticPrivRequest`：包含用户、主机、库/表/列、`mysql::PrivilegeType`、TLS 状态和激活角色。手写 `Default` 使用空字符串、权限位 `PrivilegeType(0)`、无 TLS 和空角色集。
- `VerifyDynamicPrivRequest`：包含用户、主机、动态权限名、TLS 状态、激活角色和 `WithGrant`，派生 `Clone + Default`。
- `validate_auth_plugins`：可公开直接校验一组 `Arc<AuthPlugin>`。`validateAuthPlugin` 是 crate 内部桥接函数；Manifest 没有插件时直接成功，有插件时委托前者。

## 执行流程

注册期的真实路径如下：扩展通过 `pkg/extension/manifest.rs::WithCustomAuthPlugins` 克隆 `Vec<Arc<AuthPlugin>>` 到 `Manifest.authPlugins`；`pkg/extension/registry.rs::registry::doSetup` 调用 `newManifestWithSetup`；后者应用全部选项并完成其他资源注册后调用 `validateAuthPlugin`。若列表存在，`validate_auth_plugins` 按输入顺序逐项检查：先判空名，再向局部 `HashSet<&str>` 插入名称以发现重复，然后与 `mysql::DefaultAuthPlugins` 比较，最后依次检查登录认证、认证串生成、认证串校验回调。首个错误立即返回，`newManifestWithSetup` 随即运行已经收集的清理函数并终止该扩展的构建。

注册成功后，`Extensions::GetAuthPlugins` 可把所有 Manifest 的插件汇总成名称映射；`newSessionExtensions` 也把插件复制为会话级 `HashMap`，再由 `SessionExtensions::GetAuthPlugin` 按名读取。需要注意，当前 `newSessionExtensions` 每遇到一个 `Some(authPlugins)` 就先清空映射，所以其跨 Manifest 行为与 `Extensions::GetAuthPlugins` 的持续累积并不相同；这是相邻模块的现状，不由本文件决定。

密码写入/校验路径中，`pkg/executor/utils.rs::encodePasswordWithPlugin` 在传入自定义插件时：明文形式调用 `GenerateAuthString`；哈希形式调用 `ValidateAuthString`。该调用方使用 `expect` 取回调，安全性依赖注册链已经执行本文件的必填校验。

连接适配流程是同步委托：`WriteAuthMoreData` 与 `ReadPacket` 原样转发数据；`Flush` 额外传入构造时保存的 `context`；底层错误均先转为字符串，再构造 `ExtensionError`。适配器不重试、不缓存包，也不自行刷新。

## 数据与状态

插件及回调以 `Arc` 共享，因此 Manifest、全局集合和会话集合复制的是共享所有权，不复制闭包内部状态。回调要求 `Send + Sync`，可以在多线程共享；若插件需要可变状态，状态同步是插件实现者的责任。`AuthPlugin` 自身没有内部锁，也没有注册状态标志。

`validate_auth_plugins` 唯一的临时状态是函数内的 `HashSet`，只在一次批量校验期间追踪已出现名称；它既不检测其他 Manifest 之间的同名，也不写入全局注册表。`Extensions::GetAuthPlugins` 对跨 Manifest 同名采取后插入覆盖，进一步说明本文件的“唯一”只约束同一传入切片。

请求内字符串、字节数组和角色列表均拥有数据。TLS 状态用 `Option<Arc<ServerConnection>>` 共享，静态和动态权限请求可克隆而不复制 TLS 连接本体或角色对象。`AuthenticateRequest.AuthConn` 是独占 trait object，保证同一请求中的包读写通过可变借用串行发生。

## 依赖与调用关系

上游注册调用边为 `WithCustomAuthPlugins` → `Manifest.authPlugins` → `newManifestWithSetup` → `validateAuthPlugin` → `validate_auth_plugins`。RustCodeGraph 对目标文件识别出 18 个符号，并确认 `manifest.rs` 导入 `validateAuthPlugin`、在 `newManifestWithSetup` 末尾调用它；`registry.rs::doSetup` 则负责发起 Manifest 构建。

下游类型依赖包括：`auth_identity::RoleIdentity` 填充激活角色，`mysql::PrivilegeType` 表示静态权限位，`mysql::DefaultAuthPlugins` 提供保留名集合，`rustls::ServerConnection` 提供 TLS 状态，`privilege_conn::RawAuthConn` 提供底层认证连接，`util::ExtensionError` 统一边界错误。

运行时消费边包括 `pkg/executor/utils.rs::encodePasswordWithPlugin` 调用 `GenerateAuthString`/`ValidateAuthString`，`extensions.rs::GetAuthPlugins` 和 `session.rs::newSessionExtensions` 建立查询映射。针对其余请求类型和回调，RustCodeGraph 名称查询仅返回 Go/Rust 定义，随后对非测试 Rust 的符号搜索也没有找到构造或调用点；因此其 Rust 主链接入状态应记为“未发现”，而不是推断为已完成。

## 错误处理与边界

校验严格按固定顺序短路，只报告第一个问题。空名错误会带一个空的名称占位；重复名检查区分大小写；保留名比较也是精确字符串比较。`RequiredClientSidePlugin` 不参与校验，两个权限回调也允许缺失。空插件切片和 `Manifest.authPlugins == None` 都成功。

适配器把底层错误压缩为 `error.to_string()`，因此保留可读消息但丢失具体错误类型、错误链和可供下转的结构化信息。它不捕获 panic。`into_inner` 不执行隐式 `Flush`，调用者必须在拆出底层连接前显式完成需要的发送。

`encodePasswordWithPlugin` 假定只有经过校验的插件能到达调用点，对缺失生成/校验回调会 panic；绕过注册流程手工传入未校验 `AuthPlugin` 会破坏这一前置条件。另一方面，`AuthenticateUser` 虽被校验为必填，但当前非测试 Rust 搜索未发现其调用，因此注册成功并不等同于 Rust 登录链已实际执行它。

## 并发与资源生命周期

所有回调均为 `Arc<dyn Fn + Send + Sync>`，允许注册后在多个会话或线程共享。请求中的 `AuthConn` 只要求 `Send` 而不要求 `Sync`，并通过 `&mut self` 串行读写，符合单次认证会话对连接的独占使用。`AuthConnAdapter` 只有在底层连接和上下文均可发送时才实现该接口。

TLS 状态和角色身份使用 `Arc` 延长到回调结束之后也可安全共享；本文件不创建后台任务、通道、锁或事务。适配器拥有底层连接和上下文，生命周期从 `new` 开始，到整体丢弃或 `into_inner` 消费结束；没有 `Drop` 清理逻辑。插件闭包捕获的资源随最后一个 `Arc` 释放，扩展注册表的 Reset/清理过程由 `registry.rs` 和 Manifest 清理链管理，而非本文件管理。

## 与 Go 版本的对应关系

`pkg/extension/auth.go` 是主要语义基准。Rust 的 `AuthPlugin` 字段、三个请求对象和 `validateAuthPlugin` 的检查顺序、错误文案与 Go 基本对应；`pkg/extension/registry_test.go::TestAuthPluginValidation` 覆盖空名、三个必填回调、重复名、保留名和成功注册，Rust 的 `auth_test.rs` 与 `auth_1_aster_unit_test.rs` 对这些边界作了独立回归。

关键表示差异是：Go 使用可空函数值，Rust 用 `Option<Arc<dyn Fn...>>`；Go 的切片/指针在 Rust 中变为拥有的 `Vec`、`String` 和 `Arc`；Go `conn.AuthConn` 在 Rust 中由本文件的对象安全 trait 加 `AuthConnAdapter` 桥接到 `RawAuthConn`；Go `*tls.ConnectionState` 对应 Rust `Option<Arc<rustls::ServerConnection>>`。Rust 额外公开了 `validate_auth_plugins`，使独立测试无需构造完整 Manifest。

Go 注释规定 `RequiredClientSidePlugin` 为空时使用 `AuthPlugin.Name`，也说明静态/动态权限回调只在 SQL 层已授权后作为附加检查；本文件的数据结构保留这些字段，但当前目标文件自身不实现回退或调用时机，非测试 Rust 搜索也未找到相应消费点。因此这些是需要上游接线维持的协议语义，不能归功于本文件当前实现。

## 扩展指南

新增认证插件字段或回调时，应先修改 `AuthPlugin` 及对应请求类型，再决定它是必填还是可选：必填项必须加入 `validate_auth_plugins`，并同步 `pkg/extension/auth_test.rs` 与 `pkg/extension/auth_1_aster_unit_test.rs` 的成功、缺失和错误文案用例。若字段来自 Go 迁移，还应同步核对 `pkg/extension/auth.go` 及 `auth_test.go`/`registry_test.go`，保持检查顺序和边界一致。

新增握手能力时，优先扩展 `AuthConn` 和 `AuthConnAdapter` 的成对方法，并在独立测试 `auth_1_aster_unit_test.rs::auth_conn_adapter_uses_the_migrated_privilege_connection_contract` 中验证参数、返回值、错误映射和上下文传递；不要把测试嵌入生产文件。若改动 `RawAuthConn` 契约，还必须同步 `astersql-privilege-conn` 的接口和所有实现。

接通登录或权限主链时，应在实际调用点构造完整请求，明确 TLS 状态取得方式、角色快照、连接所有权以及“SQL 层先授权、插件后附加拒绝”的次序，并新增对应模块的独立回归测试。性能上应避免逐语句重复复制大角色集合或认证数据；兼容性上不得改变插件名大小写规则、默认插件保留集合或既有错误文案而不评估 Go 行为。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `pkg/extension/auth.rs` 被完整读取为 245 行、18 个符号。
- RustCodeGraph 文件/符号证据：读取 `auth.rs`、`manifest.rs`、`registry.rs`、`extensions.rs`、`session.rs`、`lib.rs`、`pkg/executor/utils.rs`；查询 `AuthPlugin`、三个请求类型、`AuthConnAdapter`、`validateAuthPlugin` 和 `validate_auth_plugins`。图的常见名称解析对 `Flush` 等方法产生歧义，因此仅采用能由精确文件源码复核的调用边。
- crate 与对照证据：`pkg/extension/Cargo.toml`、`pkg/extension/auth.go`、`pkg/extension/registry_test.go::TestAuthPluginValidation`。
- Rust 测试证据：`pkg/extension/auth_test.rs::canonical_auth_plugin_validation_rejects_missing_callbacks_and_duplicates`；`pkg/extension/auth_1_aster_unit_test.rs::auth_validation_matches_go_errors_and_reserved_names` 与 `auth_conn_adapter_uses_the_migrated_privilege_connection_contract`。
- 补充搜索：在 RustCodeGraph 未给出可靠使用边后，用 `rg` 检索请求类型、适配器、校验函数及五个回调字段；确认 Manifest 校验调用和 executor 的两项回调消费，并记录其他运行时接线未发现。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；验收以源码/图事实复核及固定十一章节结构检查为准。
