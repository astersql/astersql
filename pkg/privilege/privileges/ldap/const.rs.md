# `pkg/privilege/privileges/ldap/const.rs`

## 文件定位

该文件属于 Cargo crate `astersql-privilege-privileges-ldap`，由同目录 `lib.rs` 通过 `#[path = "const.rs"] pub mod constants;` 公开为 `astersql_privilege_ldap::constants`。它位于 LDAP 权限认证子系统的协议词汇层，只定义 LDAP SASL 机制名称，不建立连接、不执行认证，也不保存运行时配置。

crate 边界由 `pkg/privilege/privileges/ldap/Cargo.toml` 确认；该清单声明 LDAP、TLS 和连接池依赖，但本文件自身只使用 Rust 内建的 `&str`，不直接调用这些依赖。文件顶部保留 PingCAP Apache License 声明，并已有 AsterSQL 处理标记。

## 核心职责

本文件集中定义三种允许被 LDAP SASL 配置或认证适配层使用的标准机制标识：SCRAM-SHA-1、SCRAM-SHA-256 和 GSSAPI。集中命名的价值是让配置枚举、测试和认证会话传入完全相同的区分大小写字符串，避免各处散落字面量。

当前 Rust 代码中的直接事实是：三个常量由 `constants` 公共模块导出，迁移测试会验证其精确取值，并使用 `SASLAuthMethodSCRAMSHA256` 构造 `LdapSaslAuthImpl`。仓库搜索未发现 Rust 生产代码直接引用这些常量；因此它们目前是公开的协议常量和迁移兼容面，而不是已经接入 Rust 系统变量注册链的证据。

## 主要符号

- `SASLAuthMethodSCRAMSHA1: &str = "SCRAM-SHA-1"`：SCRAM 使用 SHA-1 的 SASL 机制名。
- `SASLAuthMethodSCRAMSHA256: &str = "SCRAM-SHA-256"`：SCRAM 使用 SHA-256 的 SASL 机制名；`migration_aster_unit_test.rs` 还用它初始化 `LdapSaslAuthImpl::new` 并断言模拟 LDAP 会话收到相同方法名。
- `SASLAuthMethodGSSAPI: &str = "GSSAPI"`：基于 GSSAPI（通常对应 Kerberos 集成）的 SASL 机制名。
- `#![allow(non_upper_case_globals)]`：允许保留与 Go 导出常量一致的驼峰命名。这里是迁移兼容选择，不改变常量的可见性或生命周期。

三个符号均为 `pub const`，类型均为 `&'static str`（源码写作 `&str`，常量字符串字面量具有静态生命周期），没有类型、trait、函数、`impl` 或条件编译项。

## 执行流程

本文件没有可执行流程。使用方在编译期引用常量，运行时取得对应的静态字符串切片，不触发初始化、分配或 I/O。

从已验证的 Rust 测试路径看，流程为：`migration_aster_unit_test.rs::sasl_loop_sends_final_credential_before_success` 读取 `SASLAuthMethodSCRAMSHA256` → 将其传给 `LdapSaslAuthImpl::new` → `LdapSaslAuthImpl::auth_with_session` 通过 `GetSASLAuthMethod` 取得已保存的 `String` → 调用 `SaslSession::server_bind_step(..., method)`。常量只为第一步提供规范字符串；挑战响应、最终凭证发送和成功码判断均在 `sasl.rs` 中完成。

Go 当前完整应用接线则是：`pkg/sessionctx/variable/sysvar.go` 把三个 Go 同名常量放入 `authentication_ldap_sasl_auth_method_name` 的 `PossibleValues` → 全局变量 setter 调用 `ldap.LDAPSASLAuthImpl.SetSASLAuthMethod` → `sasl.go` 在 `ServerBindStep` 中传递所选方法名。不能由这条 Go 调用链推断 Rust 系统变量链已经接通。

## 数据与状态

三个值都是不可变的编译期常量，指向程序静态数据段中的 UTF-8 字符串；本文件没有堆分配、全局可变状态或缓存。值的大小写和连字符属于外部协议标识的一部分，调用方应原样传递。

方法的可变运行时状态不在这里，而在 `sasl.rs::LdapSaslAuthImpl::sasl_auth_method: RwLock<String>` 中。常量与该字段没有自动绑定：构造或 setter 必须显式传入某个常量（也可以传入任意其他字符串）。因此，这些定义本身既不提供合法值校验，也不设置默认值；Rust `Default` 当前使用空字符串，而默认配置字符串另见 `pkg/sessionctx/vardef/tidb_vars.rs::DefAuthenticationLDAPSASLAuthMethodName`。

## 依赖与调用关系

上游直接引用证据如下：

- `pkg/privilege/privileges/ldap/lib.rs` 将文件公开为 `constants` 模块。
- `pkg/privilege/privileges/ldap/migration_aster_unit_test.rs` 导入全部三个常量；测试 `sasl_method_constants_match_go` 校验字面值，SASL 循环测试使用 SHA-256 常量。
- RustCodeGraph 能识别测试文件中的 `astersql_privilege_ldap::constants` 导入，但没有为本文件的常量定义建立可用的 `callers`/`callees` 节点；用仓库级 `rg` 补充后，未发现其他 Rust 引用。

下游方面，本文件没有函数调用，也不依赖其他 crate。间接语义消费者是 `sasl.rs::LdapSaslAuthImpl` 和 `SaslSession::server_bind_step`，但只有在调用者把常量值显式传入时才形成数据关系。

Go 对照的生产调用方是 `pkg/sessionctx/variable/sysvar.go`：它以三个 Go 常量限制系统变量枚举，并把选值写入 `LDAPSASLAuthImpl`。这是理解文件在完整 TiDB/AsterSQL LDAP 配置链中设计位置的直接证据。

## 错误处理与边界

常量访问不会失败，本文件没有 `Result`、错误类型或 panic 路径。边界风险来自使用处：

- 字符串必须与 LDAP 服务端及客户端支持的 SASL 机制精确匹配；大小写、连字符或拼写变化都可能导致协商失败。
- 常量集合不是 Rust 类型级封闭枚举，`LdapSaslAuthImpl::new` 和 `SetSASLAuthMethod` 接受任意可转成 `String` 的值；合法值约束必须由配置层或调用者承担。
- 定义 GSSAPI 常量只表示可表达该机制名，不单独证明当前 Rust LDAP 依赖、凭据环境或部署已具备 Kerberos/GSSAPI 的端到端能力。
- 更改现有值具有配置和协议兼容风险，应视为外部行为变更，而非普通重命名。

## 并发与资源生命周期

本文件不创建锁、线程、异步任务、通道、连接或文件句柄。字符串字面量具有进程全生命周期，无需释放；并发读取天然安全且无竞争。

认证方法的并发读写由 `sasl.rs::LdapSaslAuthImpl` 内的 `RwLock<String>` 管理，LDAP 连接与认证交换的生命周期也由相邻实现负责。常量不参与锁顺序或连接归还规则，新增常量不会改变这些资源语义。

## 与 Go 版本的对应关系

直接对照文件 `pkg/privilege/privileges/ldap/const.go` 定义了同名、同值的三个包级常量。Rust 保留 Go 风格名称并通过 `allow(non_upper_case_globals)` 消除命名 lint，属于一对一移植；`migration_aster_unit_test.rs::sasl_method_constants_match_go` 为这种对应提供回归断言。

差异主要在接线状态和类型表达：Go 的 `pkg/sessionctx/variable/sysvar.go` 已直接用这些常量构建系统变量 `PossibleValues`，Rust 仓库搜索只发现测试引用；Rust 常量显式声明为 `&str`，Go 使用未显式标注类型的字符串常量。两边都没有在常量文件中实现协议协商或能力探测。

## 扩展指南

新增 SASL 机制时，应首先确认 LDAP 客户端依赖与实际服务端链路支持该机制，再在本文件增加公开常量，并同步 `const.go` 或明确记录迁移差异。若该机制应成为用户可配置值，还必须更新真正负责枚举校验的配置/系统变量注册处；只增加常量不会自动生效。

测试应放在独立文件而非 `const.rs` 中。至少同步扩展 `pkg/privilege/privileges/ldap/migration_aster_unit_test.rs::sasl_method_constants_match_go`；若机制进入认证流程，还应在该测试文件或同目录独立测试中验证方法名确实传给 `SaslSession::server_bind_step`。Go 接线改变时，应同步对应的 Go 系统变量和 LDAP 测试。兼容审查应覆盖默认值、旧配置可读性、客户端/服务端支持矩阵和凭据要求；常量读取本身没有性能风险。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件且数据库可用；`files --filter pkg/privilege/privileges/ldap` 列出本 crate 的 Rust/Go 实现和测试；`node --file pkg/privilege/privileges/ldap/const.rs --offset 1 --limit 200` 核对了完整 25 行源码。
- RustCodeGraph：对 `SASLAuthMethodSCRAMSHA256` 的 `node` 定位到 `migration_aster_unit_test.rs` 的导入；`callers`/`callees` 未找到常量定义节点，因此没有把缺失图边解释为“无人使用”，而是用文本搜索补证。
- 源码与 crate：读取 `pkg/privilege/privileges/ldap/const.rs`、`lib.rs`、`Cargo.toml` 和 `sasl.rs`，确认模块公开方式、crate 依赖边界、常量内容及认证方法的数据流。
- Go 对照：读取 `pkg/privilege/privileges/ldap/const.go`、`sasl.go` 和 `pkg/sessionctx/variable/sysvar.go` 的 LDAP SASL 系统变量注册段，确认名称、取值与 Go 生产接线。
- 测试：读取 `pkg/privilege/privileges/ldap/migration_aster_unit_test.rs`；`sasl_method_constants_match_go` 覆盖三个精确值，`sasl_loop_sends_final_credential_before_success` 覆盖 SHA-256 方法向模拟会话的传递。仓库搜索未发现其他 Rust 直接引用。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另运行任务指定的 11 章节结构检查。
