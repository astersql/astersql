# [`pkg/privilege/privileges/privileges.rs`](./privileges.rs)

## 文件定位

本文件属于 `astersql-privilege-privileges` crate。crate 入口 `pkg/privilege/privileges/lib.rs` 将本模块的公开项全部再导出；`Cargo.toml` 则把 crate 的 Go 对照包声明为 `pkg/privilege/privileges`。它位于权限缓存实现 `cache.rs` 之上：`Handle` 管理可替换的 `MySQLPrivilege` 快照，本文件的 `UserPrivileges` 为单个登录身份保存 `user`、`host` 与 `Handle`，并提供连接鉴权、静态/动态权限检查、角色查询、SSL 约束和若干账户属性辅助函数。

Rust 会话运行时已经依赖该 crate，并通过 `pkg/session/runtime.rs::runtime_privilege_handle` 为每个 `Domain` 保存一个 `Handle`；当前生产路径中可见的很多语句权限判断直接调用 `Handle::Get()` 得到的缓存（例如 `pkg/session/runtime/dispatch.rs`、`query.rs`、`control.rs`），而完整 `UserPrivileges::ConnectionVerification` 的跨 crate 调用在现有 Rust 代码中由 `pkg/session/test/privileges/privileges_test.rs::session_auth` 明确验证。因而，本文件既是已使用权限缓存 API 的上层门面，也是仍在持续对齐 Go 会话接线的迁移边界，不能假定 Go `Manager` 的所有入口都已经通过该 Rust 类型接入生产登录主链。

## 核心职责

1. `UserPrivileges` 把会话身份和 `Handle` 绑定起来，把授权判断委托给当前 `MySQLPrivilege` 快照，并在委托前执行 `skip-grant-tables`、SEM、系统 schema 等硬规则。
2. `ConnectionVerification` 组织本地登录检查：定位用户记录、检查账户锁定和 SSL 要求、验证存储哈希形状、校验主/副密码，再判断密码是否过期，最后返回规范化的认证身份、插件、沙箱标记和资源组。
3. `checkSSL`、`checkCertSAN` 与 URI 辅助函数实现 `REQUIRE SSL/X509/CIPHER/ISSUER/SUBJECT/SAN` 的证书约束，其中 URI 的 `*` 仅匹配一个非空路径段。
4. `RequestVerification*`、`RequestDynamicVerification*`、`DBIsVisible`、角色与 `SHOW GRANTS` 方法构成会话执行阶段的授权查询门面。
5. `RegisterDynamicPrivilege`、`GetDynamicPrivileges`、`RemoveDynamicPrivilege` 维护进程级动态权限名注册表；`PasswordLocking::ParseJSON` 与 JSON 辅助函数解析账户锁定属性。
6. `checkAuthTokenClaims` 和 `VerificationInfoWithSessionToken` 提供令牌校验后的局部语义，但本文件本身不获取 JWKS 或验证 JWT 签名；这些职责位于 `tidb_auth_token.rs` 或调用方。

## 主要符号

- `SKIP_WITH_GRANT: AtomicBool`、`set_skip_with_grant`、`SkipWithGrant`：进程级免鉴权开关。多数检查在它为真时立即放行；角色查询有意返回空集合，`FindEdge` 返回 `false`。
- `SANDBOX_MODE: AtomicBool`、`set_sandbox_mode`：控制过期密码是返回 `MustChangePassword`，还是让 `CheckPasswordExpired` 返回 `Ok(true)`。
- `defaultTokenLife`：Auth Token 的默认 15 分钟生命周期常量；`checkAuthTokenClaims` 接收显式 `token_life`，便于调用方或测试覆盖。
- `UserIdentity`、`SessionVars`、`VerificationInfo`：本文件自有的精简边界类型，分别描述 MySQL 用户/主机、鉴权所需 TLS 与默认密码寿命、鉴权结果。它们不是 `pkg/privilege/privilege.rs` 中同名/相近 trait 类型的自动实现。
- `Certificate`、`TlsConnectionState`：供 SSL 约束使用的证书和连接状态快照；包含密码套件、证书链验证结果及 DNS/IP/URI SAN。
- `UserPrivileges`、`NewUserPrivileges`：面向当前会话身份的门面。构造时身份为空；成功匹配后由 `AuthSuccess` 或部分辅助路径写入。
- `RequestVerification`：静态权限主入口；先执行 SEM 和虚拟 schema 硬规则，再调用 `MySQLPrivilege::RequestVerification`。
- `RequestDynamicVerification`、`HasExplicitlyGrantedDynamicPrivilege`：动态权限入口。前者允许缓存层按其语义处理回退，后者只检查显式授予。
- `ConnectionVerification`、`authenticateWithPlugin`、`checkPasswordForPlugin`：连接鉴权主流程及密码插件检查。后两者当前只在本 crate 内工作，未承载 Go 扩展认证插件的完整回调协议。
- `checkSSL`、`checkCertSAN`、`uri_parts`、`match_uri_with_wildcard`：TLS 与 SAN 约束实现；`UriParts` 为私有的 Go `url.URL` 比较形状。
- `VerificationInfoWithSessionToken`、`checkAuthTokenClaims`：已验证会话令牌的结果构造，以及 `sub/email/iat/exp/iss` claims 检查。
- `BuildPasswordLockingJSON`、`PasswordLocking::ParseJSON`、`extract*FromJSON`：账户自动锁定 JSON 的序列化与解析。
- `RegisterDynamicPrivilege`、`GetDynamicPrivileges`、`RemoveDynamicPrivilege`、`UserPrivileges::IsDynamicPrivilege`：动态权限注册表操作。

## 执行流程

静态权限检查从 `UserPrivileges::RequestVerification` 开始：先读取 `SKIP_WITH_GRANT`，空身份也按内部/未绑定上下文放行；随后将库表名转为小写。SEM 开启且用户没有 `RESTRICTED_TABLES_ADMIN` 时，不可见表直接拒绝，`mysql`、`sys`、`workload_schema` 及内存 schema 的八类 DDL/DML 权限被硬拒绝。之后，所有内存 schema 禁止 `is_write_privilege` 列出的写类权限；`information_schema` 的非写访问直接允许；`metrics_schema` 的 `SELECT` 被转化为同时要求 `SelectPriv | ProcessPriv`；`performance_schema.tidb_*` 禁止写。其余请求连同激活角色、当前身份和对象层级交给缓存。

连接鉴权由 `ConnectionVerification` 串联。免鉴权模式直接填充结果；正常模式依次通过 `connectionVerification` 选择主机匹配记录，拒绝显式锁定账户，读取全局权限记录并调用 `checkSSL`，再验证哈希格式。只有存储主密码或客户端响应至少一个非空时才执行密码插件比对，因此“空密码账户 + 空认证响应”按 Go 语义成功；主密码和附加密码任一匹配即可。最后 `CheckPasswordExpired` 根据用户级寿命或 `SessionVars::default_password_lifetime` 计算过期状态，并构造 `VerificationInfo`。注意该函数只返回结果，不自动调用 `AuthSuccess` 写入 `self.user/self.host`。

SSL 检查按 `SSLType` 分支：未指定/None 不要求 TLS，Any 要求存在连接状态，X509 要求证书链已验证，Specified 还要求对等证书、密码套件、issuer、subject 和所有已知 SAN 类型满足配置。一个 SAN 类型内的多个要求值是“任一命中”，不同 SAN 类型之间是“全部满足”；未知 SAN 类型按 Go 当前行为跳过。URI 有通配符时先解析并逐字段比较，只有路径中整个 `*` 段可匹配一个非空段。

动态权限注册先拒绝空名称，将名称转为大写，限制为最多 32 字节，再在互斥保护的向量中查重并追加。查询返回克隆快照；删除按大小写不敏感查找并移除首个命中项。

## 数据与状态

`UserPrivileges` 的 `user`、`host` 是会话局部可变状态，`Handle` 是可克隆的权限缓存句柄。每次权限检查通常调用 `Handle::Get()` 取得当前快照，因此角色撤销等缓存更新可在后续请求中生效；`privileges_test.rs::TestIssue29823` 覆盖了撤销角色边后不再授权的行为。

`SKIP_WITH_GRANT` 与 `SANDBOX_MODE` 是全进程原子状态，分别使用 Release 写入和 Acquire 读取。动态权限表由 `OnceLock<Mutex<Vec<String>>>` 延迟初始化，初始值包含备份恢复、SEM、资源组、流量捕获/回放等内置名称。注册、删除、枚举与 `IsDynamicPrivilege` 都在同一互斥量下操作；枚举会克隆数据后释放锁，不向调用方暴露内部容器。

时间统一使用 Unix 秒。密码寿命以天换算为 `86400` 秒；Auth Token 的 `iat` 和 `exp` 从 JSON 数字读取。`BuildSuccessPasswordLockingJSON` 写入当前 Unix 秒的字符串，而解析器兼容整数或数字字符串。JSON 缺失的整数/布尔字段分别默认为 `0`/`false`，非法时间字符串才返回 `InvalidJson`。

## 依赖与调用关系

上游方面，`lib.rs` 公开再导出本文件。RustCodeGraph 将该文件标记为被 8 个 Rust 文件使用；文本核验可见会话执行器的权限调用主要分布在 `pkg/session/runtime/{dispatch,query,control,ddl,statistics,system_query,mlog_purge}.rs`，用户属性过滤器 `user_attributes_filter.rs` 也调用静态/动态校验。`pkg/session/test/privileges/privileges_test.rs` 直接构造 `UserPrivileges` 并把登录语义落到 `ConnectionVerification`。`pkg/executor/infoschema_reader.rs` 使用 `UserPrivileges` 构造用户属性过滤和 `USER_PRIVILEGES` 数据。

下游方面，绝大多数授权决定委托给同 crate 的 `Handle`/`MySQLPrivilege`：`ensureActiveUser` 按需加载用户，`connectionVerification`/`matchIdentity`/`matchUser` 选择记录，`RequestVerification` 与 `RequestDynamicVerification` 计算权限，`FindAllUserEffectiveRoles`、`FindRole` 和角色 getter 处理角色图，`showGrants`/`UserPrivilegesTable` 负责展示。本文件还依赖 `sem` crate 提供增强安全模式和不可见对象判定，依赖 `sha1`/`hex` 实现 native password 验证，依赖 `serde_json` 表示 claims 与账户属性。

RustCodeGraph 对自由函数给出了具体被调用边，例如 `checkAuthTokenClaims -> now_unix`、`RegisterDynamicPrivilege -> dynamic_privileges`、`checkPasswordForPlugin -> hex::decode`；但其方法名索引优先命中了同路径 Go 定义，未能可靠列出 Rust `UserPrivileges` 方法调用者，所以跨 crate 方法调用以上述精确文本检索为补充证据。

## 错误处理与边界

授权查询多数返回 `bool`，把“不存在用户/未授权/硬规则拒绝”折叠为 `false`；连接和解析路径使用 `PrivilegeError` 区分 `AccessDenied`、`AccountLocked`、`Authentication`、`MustChangePassword`、`PasswordLock`、`InvalidPrivilegeType` 与 `InvalidJson`。若 `ensureActiveUser` 失败，当前多个入口有意忽略其错误并继续读取缓存，这与 Go 的日志后继续路径接近，但调用方不能从返回值区分加载失败和普通未授权。

`ConnectionVerification` 对错误消息使用客户端提交的 `user.Username/user.Hostname`，成功结果则使用匹配记录的规范 host。无记录、SSL 不满足、哈希格式非法或密码不匹配都拒绝访问。`checkPasswordForPlugin` 的空哈希总返回 `false`；空密码账户的成功特例只存在于主流程的“完全不调用校验函数”分支，直接调用辅助函数时不能获得该特例。

`caching_sha2_password` 与 `tidb_sm3_password` 当前仅按字节比较存储字符串和认证输入；其他未识别插件返回 `false`。Go 版本支持扩展认证插件、LDAP、auth socket、TiDB Auth Token/JWKS 等更丰富的连接回调，本文件的 `isValidHash` 虽认可部分插件名，却没有在 `ConnectionVerification` 中复刻全部 Go 插件分支，属于必须明确保留的迁移限制。

URI 解析拒绝 ASCII 控制字符、坏的 `%xx` 编码和非法 scheme；含通配符但任一 URI 无法解析时直接不匹配。`dynamic_privileges().lock().unwrap()` 在互斥量中毒时会 panic，这是当前实现边界。名称长度按 UTF-8 字节数计算，而不是字符数。

## 并发与资源生命周期

两个全局开关使用原子量，不需要外部锁；Acquire/Release 保证开关读写的基本跨线程可见性。动态权限注册表的 `OnceLock` 保证只初始化一次，`Mutex` 串行化读改写。测试使用 `serial_test` 和 RAII 清理守卫恢复这些全局状态，新增触及全局开关或注册表的测试也必须避免并发污染。

`Handle` 的快照和更新生命周期由 `cache.rs` 管理；本文件不持有数据库事务、网络连接或异步任务。每次方法调用只短暂取得缓存快照。证书、claims 和用户记录均以借用方式读取，返回的 `VerificationInfo` 和展示行拥有自己的字符串。动态权限枚举克隆整个向量，注册表锁不会跨调用方处理持续持有。

本文件读取系统时间，因此密码过期和令牌边界在秒级时钟跳变处敏感；现有测试在天级边界附近通过短暂等待消除整数秒歧义。它没有注入时钟，也不负责定时刷新权限缓存。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/privilege/privileges/privileges.go`，主要名称和控制流保持同构：静态/动态权限检查、系统 schema/SEM 硬规则、连接记录选择、密码过期、SSL/SAN、角色查询、动态权限注册以及 Password Locking JSON 均能找到对应符号。Rust 测试 `privileges_test.rs` 也按 Go 测试名覆盖数据库/表/视图权限、角色、系统 schema、SEM、动态权限、SSL/SAN、密码过期、skip-grant 和会话令牌结果。

仍有重要差异。Go `UserPrivileges` 保存 extension auth plugins，并可把扩展插件的静态/动态权限回调安装到会话门面；Rust 类型只有 `user/host/Handle`。Go `ConnectionVerification` 还编排 auth socket、LDAP、TiDB Auth Token、连接回调和登录失败计数持久化，Rust 当前主流程只做本地缓存、TLS、哈希/密码和过期检查。Go `MatchUserResourceGroupName` 通过受限 SQL 读取 `mysql.user`，Rust 在内存缓存中遍历。Go 的 `AuthSuccess` 会配合失败计数逻辑，Rust 当前只写会话身份。Go Password Locking 使用格式化时间及更详细的剩余锁定时间，Rust 使用 Unix 秒字符串并简化了自动解锁返回信息。

此外，`pkg/privilege/privilege.rs` 定义了另一套通用 `Manager` trait 和 `VerificationInfo`；代码搜索没有发现本文件的 `UserPrivileges` 对该 trait 的实现。扩展生产接线时应先确认是继续直接使用该 crate API，还是提供明确适配层，避免把相似类型误认为同一接口。

## 扩展指南

新增静态权限硬规则时，最可能修改 `RequestVerification`、`is_write_privilege` 或 `is_sem_write_privilege`；必须确认规则应覆盖普通 schema、三个内存 schema、SEM 系统库中的哪一层，并在独立 `privileges_test.rs` 中加入允许与拒绝两侧用例。不要只改门面而绕过缓存层的角色、层级和 GRANT OPTION 语义。

新增认证插件时，应同时审查 `isValidHash`、`GetAuthPluginForConnection`、`ConnectionVerification` 和 `checkPasswordForPlugin`，以及 `tidb_auth_token.rs`/`ldap` 子 crate 的职责。若目标是对齐 Go 扩展插件，不能用简单字节比较替代 Go 的 `AuthConn`、TLS 状态、插件验证和权限回调；还要补 `pkg/session/test/privileges/privileges_test.rs` 的跨 crate 登录测试。

扩展证书约束应修改 `checkSSL`/`checkCertSAN`，URI 行为则集中在 `uri_parts` 与 `match_uri_with_wildcard`。同步更新 `privileges_test.rs::TestCheckCertBasedAuth` 和 `uri_san_wildcards_match_only_whole_non_empty_path_segments`，并与 Go `TestCheckCertBasedAuthWithURIWildcard` 核对整段通配、空段、authority、query、fragment、坏转义以及 DNS/IP 精确匹配。

新增动态权限默认项修改 `initial_dynamic_privileges`；运行时注册规则修改 `RegisterDynamicPrivilege`。测试必须清理注册项并串行运行。账户属性字段变化应同步 `BuildPasswordLockingJSON`、`ParseJSON` 和三个 `extract*` 函数，以及 `cache.rs` 中调用 `ParseJSON` 的加载路径。Rust 单元测试继续放在同目录独立 `privileges_test.rs`，不要嵌入生产文件。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/privilege/privileges` 确认目标、Go 对照和独立测试均在索引内；`node --file pkg/privilege/privileges/privileges.rs --offset 1 --limit 500` 与 `--offset 500 --limit 700` 读取了 1,187 行完整源码，并报告该文件被 8 个文件使用。
- RustCodeGraph 调用查询：对 `checkPasswordForPlugin`、`checkAuthTokenClaims`、`RegisterDynamicPrivilege`、`UserPrivileges` 执行 `callers`/`callees`；确认前述自由函数的下游边，同时确认方法调用者存在名称消歧缺口，随后用限定 `.rs` 且排除目标/测试文件的 `rg` 补查。
- 源与边界：完整阅读 `pkg/privilege/privileges/privileges.rs`；读取 `pkg/privilege/privileges/Cargo.toml` 和 `lib.rs` 核对 crate 名、再导出、`sem`/`serde_json`/`sha1`/`hex` 依赖及 Go 包元数据；读取 `pkg/privilege/privilege.rs` 核对通用 trait 边界。
- Go 对照：读取 `pkg/privilege/privileges/privileges.go` 的对应符号，并通过函数索引核对 `RequestVerification`、认证、TLS/SAN、角色、动态权限和 Password Locking 实现；差异项以现有代码为准，未把 Go 独有插件路径写成 Rust 已支持。
- 测试证据：读取 `pkg/privilege/privileges/privileges_test.rs` 的权限层级、TLS/SAN、系统 schema、SEM、动态权限、skip-grant、角色撤销、密码过期和 session token 用例；对照 `privileges_test.go` 的对应测试名；读取 `pkg/session/test/privileges/privileges_test.rs::session_auth` 核对跨 crate 登录门面测试。
- 运行时接线：文本检索并抽读 `pkg/session/runtime.rs::runtime_privilege_handle`、会话 bootstrap 及 `pkg/session/runtime/*.rs` 的权限调用，确认 `Handle` 的生产使用与 `UserPrivileges` 完整登录门面的当前接线范围。
- 本任务只新增说明文档，不修改 Rust/Go/Cargo，不运行 Cargo。最终结构验证必须确认文件存在且恰有十一个规定二级标题；文档中的路径和符号还需通过 `rg` 做存在性复核。
