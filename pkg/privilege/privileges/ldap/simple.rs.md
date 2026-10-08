# `pkg/privilege/privileges/ldap/simple.rs`

源文件：[simple.rs](./simple.rs)

## 文件定位

本文件属于独立 crate `astersql-privilege-privileges-ldap`，由同目录 `Cargo.toml` 的 `[lib] path = "lib.rs"` 纳入，模块入口 `lib.rs` 通过 `pub mod simple` 将其公开。根 workspace 将该 crate 列为成员，并在 `pkg/lib.rs` 的 `privilege::privileges::ldap` 门面中再导出；`pkg/server/Cargo.toml` 也声明了该依赖。

它对应 Go 文件 `pkg/privilege/privileges/ldap/simple.go`，职责是把 MySQL `authentication_ldap_simple` 插件收到的 NUL 结尾认证数据转换为 LDAP Simple Bind 请求。需要注意当前接线状态：仓库搜索与 RustCodeGraph 只找到 Rust 测试对本模块的直接调用，没有找到 Rust 生产代码调用 `LdapSimpleAuthImpl::AuthLDAPSimple`；当前完整应用认证入口仍可在 Go `pkg/privilege/privileges/privileges.go` 中看到。因此本文件是已实现且已导出的 Rust 认证组件，但不能仅凭本文件断言 Rust 服务主链已经使用它。

## 核心职责

- `LdapSimpleAuthImpl` 把共享的 `LdapAuthImpl` 包装成 Simple Bind 专用认证器。
- `password_bytes` 校验客户端密码必须以 `0x00` 结尾，并剥离且只剥离最后一个终止字节；返回借用切片，保留包括非 UTF-8 在内的原始密码字节。
- `password_string` 为确实要求文本的兼容调用方增加 UTF-8 转换，但实际认证入口不使用它。
- `AuthLDAPSimple` 完成轻量编排：先解析密码，再把用户名、数据库记录中的 DN 与原始密码字节交给 `LdapAuthImpl::auth_simple`。
- `LDAPSimpleAuthImpl` 提供进程级惰性默认实例，与 Go 包级变量的使用形式对应。

## 主要符号

- `pub struct LdapSimpleAuthImpl { pub ldap: Arc<LdapAuthImpl> }`：认证器状态只有一个共享公共 LDAP 实现。字段公开，允许上层读取或配置同一份 `LdapAuthImpl`。
- `impl Default for LdapSimpleAuthImpl`：创建默认 `LdapAuthImpl` 并置于 `Arc` 中；默认公共实现的连接池是否可用仍取决于后续配置。
- `pub fn new(ldap: Arc<LdapAuthImpl>) -> Self`：注入现有公共实现，便于多个认证器共享配置、连接池与锁，也便于测试替换实例。
- `pub fn password_bytes(password: &[u8]) -> Result<&[u8]>`：若最后一字节不是 NUL（空切片也包含在内），返回 `invalid password`；否则返回去掉末字节的原切片。
- `pub fn password_string(password: &[u8]) -> Result<String>`：复用 `password_bytes`，再以严格 UTF-8 解码；解码失败附加 `password is not valid UTF-8` 上下文。
- `pub fn AuthLDAPSimple(&self, user_name: &str, dn: &str, password: &[u8]) -> Result<()>`：公开的 Go 风格入口，成功只返回 `()`，错误原样向上传播。
- `pub static LDAPSimpleAuthImpl: LazyLock<LdapSimpleAuthImpl>`：首次访问时构造默认认证器，之后全进程复用。

文件没有 trait、枚举、条件编译项或模块级业务常量；`#![allow(non_snake_case, non_upper_case_globals)]` 专门容纳 Go 对齐名称 `AuthLDAPSimple` 与 `LDAPSimpleAuthImpl`。

## 执行流程

1. 调用方把登录用户名、`mysql.user.authentication_string` 中保存的 DN，以及 MySQL clear-password 认证响应交给 `AuthLDAPSimple`。
2. `password_bytes` 检查认证响应末尾必须存在 NUL。合法时返回 `&password[..len-1]`；例如 `b"\0"` 产生空密码，而空输入或 `b"secret"` 被拒绝。
3. `AuthLDAPSimple` 调用 `self.ldap.auth_simple(user_name, dn, password)`。后续流程实际定义在 `ldap_common.rs`：
   - `dn` 为空时，以配置中的 root DN/root password 绑定，从 `bind_base_dn` 子树按 `search_attr=user_name` 搜索并取第一个条目的 DN；
   - `dn` 以 `+` 开头时，将其规范化为 `search_attr=user_name,<suffix>`；
   - 其他非空 DN 原样使用。
4. 公共实现从连接池取得连接，调用 `simple_bind_bytes(&bind_dn, password)`，并要求 LDAP 响应 `success()`；池化连接由 `PooledConnection` 离开作用域时归还。
5. 任一步骤失败都立即返回 `anyhow::Error`；本层不记录日志，也不把认证失败转换为 SQL 层的 access-denied。Go 生产调用方 `privileges.go` 才负责记录告警并转换登录结果。

## 数据与状态

本文件不复制 LDAP 配置，只通过 `Arc<LdapAuthImpl>` 引用公共状态。`Arc` 的克隆共享同一配置、连接池和同步原语，不生成独立连接池。默认静态实例由 `LazyLock` 保证只初始化一次。

密码路径刻意分成字节与字符串两种语义：认证入口使用 `&[u8]`，避免 Go 的 `string([]byte)` 可容纳任意字节而 Rust `String` 必须为 UTF-8 所造成的迁移偏差；`password_string` 仅是显式文本兼容辅助函数。返回的 `password_bytes` 切片借用调用者输入，不分配、不拷贝，并且不会修改或清零原缓冲区。

DN、连接池、TLS、搜索条件和 root 凭据的实际状态位于 `ldap_common.rs` 的 `LdapAuthImpl` 内。本文件不缓存某次认证的用户名、DN 或密码。

## 依赖与调用关系

直接依赖如下：

- 标准库 `Arc` 用于共享公共实现，`LazyLock` 用于全局默认实例。
- `anyhow::{Result, anyhow, Context}` 构造密码格式错误并为 UTF-8 错误添加上下文。
- `crate::ldap_common::LdapAuthImpl` 承担配置、DN 搜索/规范化、连接池及 LDAP bind。
- `ldap_common.rs::auth_simple` 下游调用 `search_filter`、连接池获取以及 `ldap3` 的 `simple_bind`/`simple_bind_bytes`；这些能力来自同 crate `Cargo.toml` 中带 `sync`、`tls-native` feature 的带 tag Git 依赖 `ldap3 v0.12.3`，并使用 `r2d2` 与 `native-tls`。

RustCodeGraph 将 `simple.rs` 标为被 `simple_test.rs`、`migration_aster_unit_test.rs` 和 `pkg/executor/utils_test.rs` 使用；后者实际只涉及同名认证插件常量，精确仓库搜索未显示它调用本文件符号。精确搜索还表明 `pkg/lib.rs` 再导出该 crate、`pkg/server/Cargo.toml` 声明依赖，但未发现 Rust 生产调用 `AuthLDAPSimple`。Go 对照主链为 `pkg/privilege/privileges/privileges.go` 的插件分支调用 `ldap.LDAPSimpleAuthImpl.AuthLDAPSimple(...)`。

## 错误处理与边界

- 空密码缓冲区与缺失终止 NUL 使用同一稳定消息 `invalid password`；只有一个 NUL 的输入合法并代表空密码。
- 末尾 NUL 之前的嵌入 NUL 不会被本层拒绝或截断；整个前缀会作为 LDAP OCTET STRING 交给 `simple_bind_bytes`。是否接受由 LDAP 服务端决定。
- 非 UTF-8 密码对真实认证路径合法；只有显式调用 `password_string` 才会因 UTF-8 失败。
- 空 `dn` 不是错误，而是触发用户搜索；`+suffix` 触发规范化；其他 DN 不做语法验证。
- 公共实现可能返回连接池未初始化、连接/绑定/搜索失败或 `LDAP user not found`。最终用户 bind 的连接获取错误附带 `create LDAP connection`，bind 请求或 LDAP 非成功响应附带 `bind LDAP`。
- `LdapAuthImpl` 的内部 `RwLock` 若中毒会通过 `expect("LDAP state lock poisoned")` panic，而不是返回普通认证错误；本文件没有捕获该 panic。
- 本层没有超时、重试或日志策略；这些由 `ldap_common.rs` 的连接管理和更上层调用者负责。

## 并发与资源生命周期

`LdapSimpleAuthImpl` 自身没有可变字段；并发安全依赖 `Arc<LdapAuthImpl>` 内部状态。`auth_simple` 在整个 DN 选择、可选搜索及最终用户 bind 期间持有公共状态的读锁，以对齐 Go `AuthLDAPSimple` 外层 `RLock`，因此配置写入不会在一次认证中途改变。代价是 LDAP 网络 I/O 期间写配置会等待，Go 源码也明确提示持锁重试/休眠的风险。

连接来自 `r2d2` 池。搜索用户时取得的连接在该分支作用域结束后归还，随后最终 bind 再取得连接；最终连接也由 RAII 在函数退出（包括错误退出）时归还。`LazyLock` 默认实例存活到进程结束，`Arc` 确保注入实例在所有使用者释放前不会销毁。文件没有自行创建线程、异步任务或通道。

## 与 Go 版本的对应关系

核心流程与 `simple.go::ldapSimplAuthImpl.AuthLDAPSimple` 一致：验证末尾 NUL、按空 DN 搜索或规范化非空 DN、从池取连接、执行用户 bind，并在函数范围内保持配置读锁。Rust 将 Go 的嵌入指针 `*ldapAuthImpl` 表达为 `Arc<LdapAuthImpl>`，将包级指针变量表达为 `LazyLock<LdapSimpleAuthImpl>`。

有两点实现层差异需要明确：

- Go 用 `string(password[:len-1])`，该转换保留任意字节；Rust 真实认证路径因此使用 `password_bytes` 与 ldap3 的 `simple_bind_bytes`，而不是要求 UTF-8 的 `password_string`。`simple_test.rs` 的非 UTF-8 用例专门固定这一语义。
- Go 的 DN 搜索、连接获取和 bind 分散在 `simple.go` 与 `ldap_common.go`；Rust 将完整持锁流程集中到 `ldap_common.rs::auth_simple`，本文件只做密码协议适配和委派。

Go `privileges.go` 已把该入口接入登录认证并转换为 access-denied；当前证据没有显示同等的 Rust 生产调用边，所以迁移状态应描述为“组件实现和 crate 导出已存在，生产接线未由本次证据验证”，不能写成端到端 Rust 登录已经完成。

## 扩展指南

- 修改 MySQL clear-password 封包规则时，优先修改 `password_bytes`，并在独立文件 `simple_test.rs` 增加原始字节边界用例；不要把测试内嵌进 `simple.rs`。
- 修改文本兼容行为时只调整 `password_string`，同步 `migration_aster_unit_test.rs::simple_password_validation_matches_go_nul_termination`；不要让 UTF-8 限制泄漏到 `AuthLDAPSimple`。
- 修改 DN 搜索、规范化、连接池、TLS 或 bind 行为应落在 `ldap_common.rs::auth_simple` 或其公共设施，并同步 `ldap_common_test.rs`/`migration_aster_unit_test.rs`；本文件应保持薄协议适配层。
- 若将 Rust 实现接入生产登录主链，应在实际权限认证分支调用 `AuthLDAPSimple`，同时补独立的上层回归测试，覆盖插件选择、错误到 access-denied 的映射以及明文密码协议；仅添加 Cargo 依赖或门面再导出不构成接线证据。
- 变更全局默认实例前评估共享配置和连接池的进程级影响；需要隔离时使用 `new(Arc<LdapAuthImpl>)` 注入，而不是增加另一套隐式全局状态。
- 安全审查需关注密码/绑定凭据不得进入日志、LDAP 搜索过滤器当前保持 Go 的字面拼接行为，以及持读锁执行网络 I/O 对配置更新延迟的影响。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/privilege/privileges/ldap` 列出本模块 13 个 Go/Rust 文件；`node --file pkg/privilege/privileges/ldap/simple.rs --offset 1 --limit 260` 读取完整 81 行源码，并报告三个“used by”文件；`query AuthLDAPSimple`、`query password_bytes`、`query auth_simple` 分别定位本文件入口/辅助函数和 `ldap_common.rs` 下游实现。`callers`/`callees` 未返回可用静态调用边，因此调用现状又以精确仓库搜索核验。
- 源码与装配：`pkg/privilege/privileges/ldap/simple.rs`、`ldap_common.rs`、`lib.rs`、同目录 `Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`、`pkg/server/Cargo.toml`。
- Go 对照与应用入口：`pkg/privilege/privileges/ldap/simple.go`、`ldap_common.go`、`pkg/privilege/privileges/privileges.go`；`pkg/server/conn_test.go::TestLDAPAuthSwitch` 只验证 clear-password 插件切换，不等同于 LDAP bind 成功测试。
- 独立 Rust 测试：`simple_test.rs::simple_password_preserves_non_utf8_bytes_like_go`；`migration_aster_unit_test.rs::simple_password_validation_matches_go_nul_termination`；DN 和连接公共行为另由 `ldap_common_test.rs::TestCanonicalizeDN` 等测试覆盖。
- 精确 `rg` 结果未找到独立 Go `simple_test.go`，也未找到 Rust 生产代码直接调用 `AuthLDAPSimple`；这些缺口在正文中按“未验证接线”处理，没有用预期架构代替代码事实。
- 本任务是只读代码分析加文档新增，按计划不运行 Cargo；交付前使用任务给定命令验证恰有十一个固定二级标题，并人工复核所有关键结论均可回溯到上述符号或文件。
