# `pkg/privilege/privileges/ldap/sasl.rs`

## 文件定位

本文件属于 Cargo crate `astersql-privilege-privileges-ldap`，由同目录 `lib.rs` 以 `pub mod sasl` 暴露。它承接 LDAP SASL 认证中的协议编排：先把账户名与认证字符串解析成可绑定的 DN，再在 LDAP 服务端和 TiDB 客户端之间转发多轮 challenge/response。LDAP 配置、DN 搜索和连接池实现位于 `ldap_common.rs`，SASL 方法名常量位于 `const.rs`。

当前 Rust 仓库中，生产代码可识别 `authentication_ldap_sasl`（`pkg/parser/mysql/const.rs`、`pkg/executor/utils.rs`），但除本文件自身外，`LdapSaslAuthImpl`/`LDAPSASLAuthImpl` 的直接 Rust 使用者只在 `migration_aster_unit_test.rs` 中找到。因此本文件已经实现并测试了认证循环，却尚无证据表明它已接入 Rust 权限认证主链。Go 版本的接线位于 `pkg/privilege/privileges/privileges.go`。

## 核心职责

- 用 `AuthConn` 抽象 TiDB 客户端连接侧的 `AuthMoreData` 写入、刷新及下一认证包读取。
- 用 `SaslSession` 抽象单次 LDAP `ServerBindStep`，使认证循环不依赖具体 LDAP 线协议对象，也便于独立测试。
- 由 `LdapSaslAuthImpl::AuthLDAPSASL` 选择 DN：认证字符串为空时调用 `LdapAuthImpl::search_user`，非空时调用 `canonicalize_dn`。
- 由 `auth_with_session` 重复执行 LDAP bind step，并保证即使本轮已经成功，也会先把最后一份服务端凭证发送给客户端。
- 以读写锁保存可运行期修改的 SASL 方法名，并提供进程级惰性默认实例。

本文件不负责建立 LDAP 连接、初始化连接池、选择具体 SASL 算法、解析客户端协议包格式，也不负责把该实现挂到上层账户权限校验流程。

## 主要符号

- `pub trait AuthConn`：客户端协议适配边界。`write_auth_more_data(&[u8])` 写挑战数据，`flush()` 确保数据实际发出，`read_packet()` 取得下一轮客户端凭证；三个操作均返回 `anyhow::Result`。
- `pub trait SaslSession`：LDAP SASL 单步边界。`server_bind_step(client_cred, dn, method)` 返回 `(result_code, server_cred)`；`result_code == 0` 在本模块中解释为 LDAP 成功。
- `pub struct LdapSaslAuthImpl`：认证编排器。公开字段 `ldap: Arc<LdapAuthImpl>` 共享 LDAP 配置/搜索实现；私有字段 `sasl_auth_method: RwLock<String>` 保存方法名。
- `LdapSaslAuthImpl::new`：注入共享 LDAP 实现和初始方法名，是测试及定制实例的构造入口。
- `LdapSaslAuthImpl::AuthLDAPSASL`：公开认证入口，负责 DN 决策，然后调用 `auth_with_session`。
- `LdapSaslAuthImpl::auth_with_session`：多轮挑战循环的核心实现。
- `SetSASLAuthMethod` / `GetSASLAuthMethod`：加锁写入与读取方法名；getter 返回克隆的 `String`，使循环不长期持锁。
- `impl Default`：构造默认 `LdapAuthImpl` 和空方法名。
- `pub static LDAPSASLAuthImpl`：通过 `LazyLock` 延迟初始化的进程级默认实例。

## 执行流程

1. 调用者向 `AuthLDAPSASL(user_name, dn, client_cred, session, auth_conn)` 提供账户名、账户记录中的 DN/后缀、首轮客户端凭证以及两侧协议适配器。
2. 若 `dn` 为空，调用 `ldap.search_user(user_name)`：公共实现从配置读取搜索基准、属性及 root 凭据，以 root DN bind 后在子树中搜索并取第一条结果的 DN。若 `dn` 非空，调用 `canonicalize_dn`；只有以 `+` 开头时才拼成 `<search_attr>=<user_name>,<suffix>`，否则原样使用。
3. `AuthLDAPSASL` 把最终 DN、初始凭证和两个适配器交给 `auth_with_session`。RustCodeGraph 显示的本文件内部主边为 `AuthLDAPSASL -> auth_with_session`。
4. `auth_with_session` 在进入循环前调用一次 `GetSASLAuthMethod`，得到本次认证固定使用的方法快照。
5. 每轮调用 `session.server_bind_step(client_cred, dn, method)`，得到 LDAP 结果码及服务端凭证。
6. 无论结果码是否成功，都依次调用 `write_auth_more_data(server_cred)` 和 `flush()`；这是客户端收到 SASL 最终消息所必需的顺序。
7. 若 `result_code == 0`，立即返回成功，且不会再读取客户端包；否则调用 `read_packet()`，以返回的数据替换 `client_cred` 并继续下一轮。

`migration_aster_unit_test.rs::sasl_loop_sends_final_credential_before_success` 用两轮序列 `(14, "challenge")`、`(0, "final")` 验证了初始凭证/响应的传递顺序、两次写入、两次刷新，以及成功轮仍发送 `final` 的不变量。

## 数据与状态

`LdapSaslAuthImpl` 只直接持有两类长期状态：一个引用计数的 `Arc<LdapAuthImpl>`，以及受 `RwLock` 保护的方法名。DN、客户端凭证、服务端凭证和 LDAP 结果码都是单次调用内的临时值。`client_cred: Vec<u8>` 在循环中被下一包整体替换；`server_cred` 每轮只借用给写接口，不在模块中缓存。

方法名在循环开始时克隆为局部 `String`，因此并发调用 `SetSASLAuthMethod` 只影响之后开始并读取配置的认证，不会改变已经运行中的会话。默认实例的方法名为空，实际使用默认单例前必须由外部配置路径设置为受支持的方法；本文件本身不验证空字符串或方法名是否合法。

## 依赖与调用关系

上游方面，RustCodeGraph 仅确认 `migration_aster_unit_test.rs::sasl_loop_sends_final_credential_before_success` 构造 `LdapSaslAuthImpl` 并调用 `AuthLDAPSASL`；未找到 Rust 生产调用者。Go 对照链路是 `UserPrivileges` 的认证校验分支调用 `ldap.LDAPSASLAuthImpl.AuthLDAPSASL`。`pkg/executor/utils.rs::encodedPassword` 只说明 LDAP SASL 账户的认证字符串按 DN 原样保存，并非本 Rust 文件的运行时调用者。

下游方面，`AuthLDAPSASL` 调用 `LdapAuthImpl::search_user` 或 `canonicalize_dn`，再调用 `auth_with_session`；后者调用 `GetSASLAuthMethod`、`SaslSession::server_bind_step` 以及 `AuthConn` 的三个方法。`search_user` 进一步依赖 `ldap3`、连接池和 root bind，而这些实现均在 `ldap_common.rs`，不属于本文件。

`Cargo.toml` 声明本 crate 直接依赖 `anyhow`、带 `sync`/`tls-native` feature 的 tag `v0.12.3` 上游 `astersql/ldap3`、`native-tls` 和 `r2d2`。本文件自身直接使用 `anyhow` 与标准库同步原语；LDAP/连接池依赖通过 `LdapAuthImpl` 间接进入。根 workspace 将该 crate 列为成员并定义 facade 依赖别名，但这只证明可构建边界，不等同于 SASL 已接入 Rust 主链。

## 错误处理与边界

所有可恢复失败通过 `anyhow::Result` 立即向上传播：DN 搜索失败、LDAP 单步失败、写 AuthMoreData 失败、刷新失败或读取下一包失败都会终止认证。已发生的网络写入不会回滚。例如服务端凭证写入成功但 `flush` 失败时，函数返回错误，不再读取客户端响应。

模块只把数值 `0` 当作成功；所有非零码均被视为“继续交换”，没有在本层区分继续处理中、认证拒绝或其他 LDAP 结果。如果底层对终止性非零错误仍返回 `Ok`，循环可能继续等待客户端包，因此适配器必须把不能继续的 LDAP 结果转换为 `Err`。代码也没有轮数、凭证大小或总时限限制，这些资源边界必须由具体连接/会话实现提供。

`RwLock` 中毒会通过 `expect("LDAP SASL lock poisoned")` panic，而不是返回认证错误。DN 非空时不会校验格式；`+` 后缀、搜索属性和用户名直接参与字符串拼接，LDAP 搜索分支的过滤器转义责任位于 `ldap_common.rs::search_filter`。本文件也不隐藏或记录敏感数据，因为它完全不产生日志。

## 并发与资源生命周期

`Arc<LdapAuthImpl>` 允许多个认证器或并发认证共享公共 LDAP 配置。方法名单独用 `RwLock<String>` 保护，setter 持写锁至赋值完成，getter 持读锁至克隆完成；网络交互期间不持有该锁，从而避免慢客户端阻塞配置更新。`LazyLock` 保证全局默认实例只初始化一次。

`session: &mut S` 和 `auth_conn: &mut C` 要求每次调用独占两个会话对象，循环本身不生成线程、任务或通道。其网络资源生命周期由调用者管理：Rust API 接收已经建立的 `SaslSession`，本文件既不从 `LdapAuthImpl` 连接池获取 SASL 连接，也不归还连接。相比之下，Go 实现会在入口中 `getConnection` 并通过 `defer putConnection` 回收；若未来把真实 `ldap3` 会话接入 Rust，必须在适配层或调用者明确实现等价的池化和回收语义。

## 与 Go 版本的对应关系

Rust 的 DN 两分支、多轮 `ServerBindStep`、每轮写入并刷新、成功轮仍发送最后凭证、非成功轮读取下一包，以及 SASL 方法的并发配置，均对应 `sasl.go`。独立 Rust 测试明确固定了最关键的“成功前仍发送最终 credential”行为；方法名常量另由 `sasl_method_constants_match_go` 校验。

主要差异是依赖倒置和资源所有权。Go 的 `AuthLDAPSASL` 自行锁住公共配置快照、搜索/规范化 DN、从池取出 `*ldap.Conn`、执行真实 `ServerBindStep` 并延迟归还；Rust 将真实 LDAP 单步和客户端 I/O 分别抽象为 trait，由调用者传入可变会话。Rust 的 DN 搜索会在 `search_user` 内自行短暂取连接，但认证循环使用的 `SaslSession` 与该连接不是同一对象。Rust 也只对方法名加锁，没有像 Go 那样在 DN 决策和取连接阶段持有 `ldapAuthImpl` 的配置读锁。因此可以认定循环语义已经对齐，但真实生产连接获取、同一配置快照和上层权限接线尚不能从当前 Rust 代码证实。

## 扩展指南

- 接入真实认证主链时，应在权限校验层创建或取得具体 `SaslSession` 与 `AuthConn` 适配器，然后调用 `AuthLDAPSASL`；同时明确连接池 checkout/checkin、超时和断线回收，不能仅调用当前全局默认实例便假设资源已管理。
- 新增 SASL 方法通常应先在 `const.rs` 增加方法常量，并由配置路径调用 `SetSASLAuthMethod`；若不同会话需不同方法，优先把方法作为会话输入，而不是在全局锁中频繁切换。
- 改动挑战循环时必须保持“先写并 flush 服务端凭证，再判断成功”的协议顺序；相关回归测试应继续放在独立的 `migration_aster_unit_test.rs` 或新增同目录独立测试文件，不应内嵌进 `sasl.rs`。
- 应补充错误分支测试：`server_bind_step`、write、flush、read 分别失败时停止后续操作；还应覆盖空 DN 的搜索失败、原样 DN、锁配置快照以及异常非零结果的终止策略。
- 若要达到 Go 生产等价，应重点验证配置读锁范围、认证所用连接与搜索连接的关系、连接归还、底层 LDAP result code 映射。这里涉及兼容性和资源泄漏风险；无限轮交换还具有慢客户端占用连接的性能/拒绝服务风险。

## 验证依据

- RustCodeGraph `status`：索引覆盖本仓库 7,032 个 Rust 文件；目标文件已索引。
- RustCodeGraph `node --file pkg/privilege/privileges/ldap/sasl.rs`：读取全部 131 行，确认两个 trait、实现结构、五个方法/实现入口和全局单例。
- RustCodeGraph 精确查询与 `explore`：确认 `AuthLDAPSASL`、`auth_with_session`、`search_user`、`canonicalize_dn` 的定义，以及 `AuthLDAPSASL -> auth_with_session`、`search_user -> get_connection/search_filter` 调用边；未发现 Rust 生产上游调用者。
- `pkg/privilege/privileges/ldap/Cargo.toml`、`lib.rs` 与根 `Cargo.toml`：核对 crate 名、模块公开边界、依赖 feature/tag、workspace 成员及 facade 别名。
- `pkg/privilege/privileges/ldap/ldap_common.rs`：核对 `LdapAuthImpl`、DN 规范化、用户搜索、连接池获取与锁语义。
- `pkg/privilege/privileges/ldap/sasl.go`、`pkg/privilege/privileges/privileges.go`：核对 Go 原始认证循环、配置锁、连接池生命周期及权限主链接线。
- `pkg/privilege/privileges/ldap/migration_aster_unit_test.rs`：核对方法常量、DN 规则与两轮 SASL 交换的真实断言；同目录未发现专门的 `sasl_test.rs`，SASL 回归集中在该独立迁移测试文件。
- `pkg/executor/utils.rs`、`pkg/parser/mysql/const.rs`：核对 Rust 对 LDAP SASL 插件名和认证字符串透传的支持边界。
- 本任务是纯文档分析，按计划不运行 Cargo；最终仅执行固定十一章节的结构校验。
