# `pkg/privilege/privileges/ldap/ldap_common.rs`

## 文件定位

该文件是 `astersql-privilege-privileges-ldap` crate 的 LDAP 公共实现层，由 [`lib.rs`](lib.rs) 声明为 `ldap_common` 模块。它不直接处理 MySQL 客户端协议，而是为 [`simple.rs`](simple.rs) 的 Simple Bind 认证和 [`sasl.rs`](sasl.rs) 的 SASL 认证提供共享的配置、TLS 建连、r2d2 连接池、用户 DN 搜索与 DN 规范化能力。crate 边界和依赖由 [`Cargo.toml`](Cargo.toml) 确认：错误上下文使用 `anyhow`，LDAP 同步客户端使用带 `sync`/`tls-native` feature 的已发布 `ldap3` Git tag `v0.12.3`，TLS 使用 `native-tls`，连接池使用 `r2d2`。

## 核心职责

- 用 `LdapConfig` 表达搜索账户、LDAP 端点、TLS/CA 与连接池容量配置，并由 `LdapAuthImpl::state: RwLock<LdapState>` 将配置和对应连接池放在同一同步边界中。
- 由 `LdapConnectionManager` 为 r2d2 创建、校验 LDAP 连接；未启用 TLS 时使用 `ldap://`，启用时先尝试 StartTLS，失败再回退到 `ldaps://`。
- 从池中取出连接时经 root DN bind 检验，失败最多尝试 10 次，每次之间等待 500 ms。
- 支持将 `+suffix` 形式的数据库 DN 扩展为 `search_attr=user_name,suffix`；当没有提供 DN 时，以 root 账户在 Base DN 子树中搜索首个用户条目。
- 向上层提供 Go 命名风格的 `Set*`/`Get*` 配置 API，并在主机、端口、TLS 开关或容量变更时按条件重建池。

## 主要符号

- `LDAP_TIMEOUT` 同时用作 TCP/TLS 建连、LDAP 操作与 r2d2 等待超时，值为 10 秒。`GET_CONNECTION_MAX_RETRY` 为 10，`GET_CONNECTION_RETRY_INTERVAL` 为 500 ms。
- `search_filter(search_attr, user_name) -> String` 直接拼接 `({attr}={user})`；它刻意复刻 Go 的字面格式，不在此层做 LDAP filter escaping。
- `LdapConfig` 是可克隆配置快照，包含 `bind_base_dn`/`bind_root_dn`/`bind_root_pwd`/`search_attr`、服务器 host/port、`enable_tls`、`ca_path`/`ca_pem` 以及池的 `init_capacity`/`max_capacity`。
- 私有 `LdapState` 将 `config`、可选 `Pool<LdapConnectionManager>` 和仅用于观测重建次数的 `pool_generation` 绑定在一起。
- `LdapConnectionManager::{tls_connector,address,connect_url,connect_ldap}` 分别负责 TLS 1.2 下限与自定义 CA、IPv4/IPv6 `host:port` 格式、单个 URL 连接、按配置选择明文或 TLS 回退链。其 `ManageConnection` 实现通过 `connect_ldap` 新建连接，并以 root DN `simple_bind` 校验 checkout 的连接。
- `LdapAuthImpl` 是上层持有的公共门面。`search_user`/`canonicalize_dn` 为 SASL 路径提供 DN；`auth_simple` 将 Simple Bind 的搜索或规范化与最终 bind 包在一次配置读锁中；`get_connection` 是公开的取连接入口。
- `SetBindBaseDN`/`SetBindRootDN`/`SetBindRootPW`/`SetSearchAttr` 只更新搜索配置；`SetLDAPServerHost`/`SetLDAPServerPort`/`SetEnableTLS`/`SetInitCapacity`/`SetMaxCapacity` 仅在值变化时尝试重建池；`SetCAPath` 读取并预解析 PEM，但不重建已有池。同名 `Get*` 返回当前值的拷贝或标量。

## 执行流程

1. 配置接线时，调用方通过 `Set*` 更新 `LdapState::config`。当初始容量大于 0 且最大容量不小于初始容量时，`rebuild_pool` 用当前配置快照构造 `LdapConnectionManager`，设置 `max_size`、`min_idle`、10 秒取连接超时和 `test_on_check_out(true)`，然后替换池并递增世代号。
2. r2d2 需要创建连接时进入 `LdapConnectionManager::connect_ldap`。`address` 为含冒号的主机添加 IPv6 方括号。明文模式直接连接 `ldap://`；TLS 模式先以 `ldap://` + StartTLS 连接，失败后以 `ldaps://` 直连，两者皆使用最低 TLS 1.2 和可选自定义 CA。
3. checkout 时 `ManageConnection::is_valid` 先用 root DN/密码 bind，用于验证可用性并把可能留在用户身份上的复用连接重置回 root 身份。`get_connection_from_pool` 对 `pool.get()` 失败执行最多 10 次的定长重试。
4. Simple Bind 上游 `LdapSimpleAuthImpl::AuthLDAPSimple` 先去掉 MySQL 密码末尾 NUL，然后调用 `auth_simple`。该方法持有一次 `state` 读锁：DN 为空时取连接、root bind，在 `bind_base_dn` 子树内以 `search_attr` 搜索首个 DN；DN 以 `+` 开头时就地拼接；否则原样使用。最后再取一条连接，以密码原始字节执行 `simple_bind_bytes`。
5. SASL 上游 `LdapSaslAuthImpl::AuthLDAPSASL` 在 DN 为空时调用 `search_user`，否则调用 `canonicalize_dn`，再把得到的 DN 交给 SASL 多轮挑战循环。`search_user` 在短读锁内克隆搜索配置，释锁后连接、root bind 并搜索；因此它不保证“配置快照与当前池”像 `auth_simple` 那样在整个认证期间不变。

## 数据与状态

`LdapAuthImpl::default` 从全空/全零的 `LdapConfig` 开始，池为 `None`，世代号为 0；因此必须先给出合法的池容量才会有可取的连接池。`rebuild_pool` 克隆整个配置给 manager，所以池中新建连接使用的是重建时快照，而不是后续可变 `state.config` 的实时引用。这也解释了为何 host/port/TLS/容量 setter 要重建池。

`SetCAPath` 的状态更新顺序是先写入 `ca_path` 并清空 `ca_pem`，再读文件与校验证书；因此 I/O 或 PEM 解析失败时，方法会返回错误，但 `ca_path` 已保留失败路径且 `ca_pem` 为 `None`。成功加载的 PEM 只会被下一次池重建所创建的 manager 快照采用；当前 `SetCAPath` 本身不替换池，这一语义由 `set_ca_path_does_not_rebuild_connection_pool` 锁定。

LDAP 搜索只取结果中的第一个 DN，没有唯一性检查。用户密码不存入 `LdapState`，只以字节切片传入最终 bind；root 密码则作为 `String` 存在可克隆的配置中。

## 依赖与调用关系

- 上游：[`simple.rs`](simple.rs) 中 `LdapSimpleAuthImpl::AuthLDAPSimple -> LdapAuthImpl::auth_simple`；[`sasl.rs`](sasl.rs) 中 `LdapSaslAuthImpl::AuthLDAPSASL -> search_user/canonicalize_dn`。RustCodeGraph 查询确认 `auth_simple` 有 `AuthLDAPSimple` 调用者，`canonicalize_dn` 还被两个 Rust 对照测试调用。
- 下游：`ldap3::LdapConn` 提供建连、StartTLS、simple bind 和子树搜索；`native_tls` 构造 TLS 1.2+连接器并加载 PEM root certificate；`r2d2` 通过 `ManageConnection` 接管连接创建、checkout 校验和 RAII 归还；`anyhow::Context` 为每个外部失败增加阶段语义。
- 装配：[`lib.rs`](lib.rs) 公开 `ldap_common`、`simple`、`sasl` 三个模块，并仅在 `cfg(test)` 下挂载 `ldap_common_test.rs`、`simple_test.rs` 和 `migration_aster_unit_test.rs`；生产文件与测试逻辑分离。

## 错误处理与边界

- `connect_url` 分别标注 TLS connector 构建和目标 URL 建连失败；TLS 模式两条路径都失败时，`connect_ldap` 把最终 LDAPS 错误与首先发生的 StartTLS 错误一并保留。
- `get_connection`/`auth_simple` 在池尚未初始化时明确返回 `LDAP connection pool is not initialized`。取池重试用尽后返回带最后一个 r2d2 错误的 `fail to bind to anonymous user`；该文字沿用 Go，实际校验用的是配置的 root DN，不应将其解读为空 DN bind。
- root bind、搜索请求、搜索响应与最终用户 bind 都通过 `success()` 检查 LDAP result code；搜索零结果返回 `LDAP user not found`。
- `canonicalize_dn` 对空 DN 是安全的（`strip_prefix` 返回 `None`），但正常调用约定是上层仅对非空 DN 使用它。`search_filter` 不转义 `user_name` 中的 LDAP filter 元字符；`search_filter_matches_go_literal_formatting` 明确把这一 Go 对齐行为当作现状测试，扩展时不能在无兼容性评估下单方改变。
- `SetCAPath` 先更新路径再执行可失败 I/O，不具备事务回滚；相同路径会立即成功返回，不会重读已被外部替换的文件。
- 所有 `RwLock` 获取都对 poison 使用 `expect`，因此持锁代码 panic 后的后续调用会再次 panic，而不是返回 `Result`。`ManageConnection::has_broken` 始终返回 `false`，连接有效性依赖 checkout 时的 root bind，而非主动 broken-state 探测。

## 并发与资源生命周期

`LdapAuthImpl` 的配置与池替换由一把 `std::sync::RwLock` 保护。getter 持短读锁并克隆字符串，setter 持写锁；`rebuild_pool` 在 setter 已持有写锁时建立新池。废弃的旧 `Pool` 在最后一个克隆引用/借出连接释放后由 RAII 回收，代码没有手动 close 路径。`PooledConnection` 在 `search_user` 或 `auth_simple` 返回/报错时自动归还池。

`auth_simple` 故意在搜索、DN 生成、取第二条连接和用户 bind 期间一直持有配置读锁，对齐 Go 上层使用 `RLock` 包住整个 Simple Bind 流程的语义。代价是 pool checkout 失败时的最长约 4.5 秒定长 sleep（不含每次 pool 自身等待和 LDAP 网络超时）会阻止所有 setter 获取写锁。`search_user` 则先复制所需配置再释放读锁，网络 I/O 期间不持锁。

r2d2 可在创建 `min_idle` 连接时异步执行 manager 逻辑；本文件本身不创建 async runtime，也不管理通道或显式任务。TLS 与超时测试在独立 OS 线程中运行本地侦听器，生产连接 API 则是同步阻塞式。

## 与 Go 版本的对应关系

Rust 文件直接对应 [`ldap_common.go`](ldap_common.go)。`LdapConfig` + `LdapState` + `RwLock` 拆分了 Go `ldapAuthImpl` 中内嵌 `sync.RWMutex` 和所有字段；`LdapConnectionManager`/r2d2 取代 `pools.ResourcePool` 与 `connectionFactory`；`connect_ldap` 对应 Go 的明文/StartTLS/直连 TLS 选择；`search_user`、`canonicalize_dn`、容量校验、setter/getter 与 10 次、500 ms 重试均保留了 Go 意图。

已确认的差异与实现选择如下：

- Go `getConnection` 自己从池中取出对象后 root bind，失败则丢弃对象再重试；Rust 把 root bind 放到 r2d2 `test_on_check_out`/`is_valid` 中，外层对 `pool.get()` 重试。
- Go 重建池时显式 `Close` 旧池；Rust 通过 `Pool` 替换和引用计数回收。
- Go `initializeCAPool` 保存 `x509.CertPool`；Rust `SetCAPath` 保存已验证的 PEM 字节，由每个 manager 构造 `native_tls::Certificate`。两者的 CA setter 都不自动重建已有连接池。
- Go 有 `skipTLSForTest` 可在测试中禁用 LDAPS 回退，且测试可临时缩短可变 `ldapTimeout`；Rust 常量不可变且没有该测试开关，所以 Rust 超时测试等待真实 10 秒，TLS 测试分开验证 LDAPS 目标与 StartTLS 失败。
- Go `canonicalizeDN` 依赖非空 DN 前置条件并直接访问 `dn[0]`；Rust `strip_prefix` 对空字符串也安全。Rust `auth_simple` 另外使用 `simple_bind_bytes`保留 LDAP OCTET STRING 的非 UTF-8 密码能力。

[`ldap_common_test.go`](ldap_common_test.go) 与 [`ldap_common_test.rs`](ldap_common_test.rs) 共同锁定 DN 规范化、StartTLS 失败后直连 TLS、拒绝 TLS 1.1 以及 StartTLS 超时。Rust 的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 进一步覆盖 getter/setter、池重建世代、CA I/O/PEM 错误与清空路径。

## 扩展指南

- 新增会影响建连的配置时，应同时扩展 `LdapConfig`、对应 `Set*`/`Get*`、`LdapConnectionManager` 的快照使用点以及 `rebuild_pool` 触发条件；不能只更改 `state.config`，否则现有 manager 仍使用旧快照。在独立 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 中增加重建/不重建断言。
- 改动 TLS 策略时从 `tls_connector`、`connect_url`、`connect_ldap` 接入，同步检查 CA 加载和 IPv6 `address`。保持与 Go 的 StartTLS 优先/LDAPS 回退及 TLS 1.2 下限，或明确记录兼容性偏离；在独立 [`ldap_common_test.rs`](ldap_common_test.rs) 中添加本地服务器边界测试，不要把测试嵌入生产文件。
- 改动用户查找时从 `search_filter`、`search_user` 和 `auth_simple` 中的对应内联流程同步接入，避免 Simple 与 SASL 路径分叉。尤其是 filter escaping、多结果、DN 唯一性或搜索属性集都会改变当前 Go 兼容行为，需同时扩展 [`ldap_common_test.rs`](ldap_common_test.rs) 与相应 Go 对照测试。
- 修改锁粒度或重试策略时，必须保留 `auth_simple` 的“一次认证使用一致配置/池”不变量和 checkout 前 root bind 的身份重置意图。应在独立测试文件中补充并发 setter/认证、池耗尽和失效连接场景，并评估持读锁 sleep 对配置更新延迟的影响。
- 任何网络或密码处理更改都要关注凭据泄漏、LDAP filter injection、TLS 降级、超时乘数和池资源占用。修改 Rust 行为时必须同步相关独立 Rust 测试，并与 [`ldap_common.go`](ldap_common.go) 对照，不以简化实现代替完整语义。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件，目标 [`ldap_common.rs`](ldap_common.rs) 被识别为 527 行、57 个符号。通过 `files --filter pkg/privilege/privileges/ldap`、`node --file .../ldap_common.rs`、`query` 及 `explore` 核对了 `LdapAuthImpl`、`auth_simple`、`search_user`、`connect_ldap` 与上游调用边。
- 生产源码：[`ldap_common.rs`](ldap_common.rs) 的常量、`LdapConfig`、`LdapState`、`LdapConnectionManager`、`ManageConnection` 实现、`LdapAuthImpl` 及全部 setter/getter；[`simple.rs`](simple.rs) 的 `AuthLDAPSimple`；[`sasl.rs`](sasl.rs) 的 `AuthLDAPSASL`；[`lib.rs`](lib.rs) 的模块与测试装配。
- crate 与对照：[`Cargo.toml`](Cargo.toml) 的 package/lib/依赖/porting metadata；[`ldap_common.go`](ldap_common.go) 的 `ldapAuthImpl`、`searchUser`、`canonicalizeDN`、`connectionFactory`、`getConnection`、`initializePool` 与 setter/getter。
- 测试：[`ldap_common_test.rs`](ldap_common_test.rs) 的 `TestCanonicalizeDN`、`search_filter_matches_go_literal_formatting`、`set_ca_path_does_not_rebuild_connection_pool`、`TestConnectThrough636`、`TestConnectWithTLS11`、`TestLDAPStartTLSTimeout`；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `canonicalize_dn_matches_go_cases`、`setters_getters_and_pool_rebuild_rules_match_go`、`ca_path_reports_io_and_pem_errors_and_can_be_cleared`；[`ldap_common_test.go`](ldap_common_test.go) 的四个对应边界测试。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付时使用任务指定的结构命令检查文档存在且上述 11 个二级标题各出现一次，并人工复核本文未将门面、测试或期望设计误写为已接线的生产实现。
