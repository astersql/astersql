# `pkg/privilege/privileges/cache.rs`

## 文件定位

`cache.rs` 是 `astersql-privilege-privileges` crate 的权限数据层与判定内核。crate 入口 `pkg/privilege/privileges/lib.rs` 将它作为私有模块声明并把其公开项重新导出；相邻的 `privileges.rs` 在此基础上叠加会话身份、SEM 和系统 schema 特例。Cargo 元数据把本 crate 对应到 Go 包 `pkg/privilege/privileges`，直接依赖 `astersql-util`、`sem` 与 `serde_json` 等组件（`pkg/privilege/privileges/Cargo.toml`）。

文件把 `mysql.user`、`mysql.global_priv`、`mysql.global_grants`、`mysql.db`、`mysql.tables_priv`、`mysql.columns_priv`、`mysql.default_roles` 和角色边转换为内存快照，并提供连接身份匹配、静态/动态权限判断、数据库可见性、`SHOW GRANTS`/`USER_PRIVILEGES` 输出及增量刷新能力（`MySQLPrivilege`、`Handle`）。它不是 SQL 查询执行器：系统表访问由 `PrivilegeDataSource` 抽象注入。

## 核心职责

- 用 `PrivilegeType = u64` 及 `CreatePriv`、`SelectPriv` 等位常量表示静态权限；`ALL_GLOBAL_PRIVS`、`ALL_DB_PRIVS`、`ALL_TABLE_PRIVS` 定义各作用域可枚举集合，刻意排除只作为后缀呈现的 `GrantPriv`。
- 用不同记录类型保持系统表语义：`UserRecord` 保存认证与全局权限，`globalPrivRecord` 保存 TLS/X509/SAN 要求，`dynamicPrivRecord` 保存字符串动态权限，其余记录分别覆盖库、表、列、默认角色和角色图。
- 从 `PrivilegeDataSource` 全量加载并排序记录，或通过 `Grant*`、`RevokePrivilegeMask`、`DropAccount` 合并已提交的权限变化；列权限变化后同步重建 `ColumnsPrivMap`。
- 按 Host 模式的特异性选择账户，并沿“用户/有效角色 × 全局/库/表/列”层级判断权限；角色闭包用带去重集合的 BFS，能终止于有环角色图。
- 生成稳定的 `SHOW GRANTS`、信息模式权限行和 MySQL SET 列编码；处理标识符引用、ANSI_QUOTES、`ALL PRIVILEGES`、`USAGE` 与 `WITH GRANT OPTION`。
- 由 `Handle` 以共享锁和原子标志发布不可变快照，支持全量缓存与只缓存活跃用户两种模式。

## 主要符号

- `PrivilegeType` 与权限常量：静态权限位模型。`computePrivMask`、`DecodePrivilegeColumns`、`decodeSetToPrivilege`/`EncodePrivilegeSet`、`privilege_name`/`privilege_column_name` 完成集合、系统表列和显示名称之间的转换。
- `baseRecord { Host, User }`：所有权限记录的身份基类。`hostMatch` 依次处理 loopback/`localhost`、IPv4 `ip/mask`、`%`/`_` 通配；`match` 是用户名精确加 Host 模式匹配，`fullyMatch` 则要求 Host 也逐字一致。
- `UserRecord`、`GlobalPrivValue`、`globalPrivRecord`、`dynamicPrivRecord`、`dbRecord`、`tablesPrivRecord`、`columnsPrivRecord`、`defaultRoleRecord`：对应系统表行。`GlobalPrivValue::RequireStr` 输出 `NONE`、`SSL`、`X509` 或 CIPHER/ISSUER/SUBJECT/SAN 子句。
- `RoleIdentity` 与 `roleGraphEdgesTable`：角色图节点和邻接集合；`FindAllRole` 做 BFS，`FindAllUserEffectiveRoles` 先丢弃未直接授予给当前账户的活动角色，再展开传递闭包。
- `PrivilegeDataSource`：加载边界，各方法默认返回空集合，具体数据源负责提供已解码记录或错误。`LoadAll` 按 user、global_priv、global_grants、db、tables、columns、default_roles、role_graph 顺序调用八个加载器并以 `?` 立即传播错误。
- `MySQLPrivilege`：权限快照主体。核心入口包括 `RequestVerification`、`RequestDynamicVerification`、`DBIsVisible`、`showGrants`、`UserPrivilegesTable`；变更入口包括 `GrantDatabasePrivilegeColumns`、`GrantGlobalPrivilegeMask`、`GrantTablePrivilegeMask`、`GrantColumnPrivilegeMask`、`RevokePrivilegeMask` 和 `DropAccount`。
- `Handle`：`Arc<RwLock<MySQLPrivilege>>` 包装；`UpdateAll`、`UpdateAllActive`、`Update`、`ensureActiveUser` 更新快照，`Get` 返回克隆，避免把锁守卫泄漏给调用方。
- 行解码入口：`decodeUserTableRow`、`decodeGlobalPrivTableRow`、`decodeGlobalGrantsTableRow`、`decodeDBTableRow`、`decodeTablesPrivTableRow`、`decodeColumnsPrivTableRow`、`decodeDefaultRoleTableRow`、`decodeRoleEdgesTable`。输入统一为 `PrivilegeRow = HashMap<String, serde_json::Value>`。

## 执行流程

1. 全量刷新从 `Handle::UpdateAll` 开始：创建空 `MySQLPrivilege`，调用 `LoadAll`，成功后依次替换 `source_data` 与对外 `data`，最后以 Release 写入 `full_data=true`。任何加载失败都发生在发布前，旧 `data` 保持可读。
2. `LoadAll` 让数据源提供每张权限表的记录；各加载器覆盖相应向量并排序。`LoadColumnsPrivTable` 还调用 `buildColumnsPrivMap`，角色图则整体替换。
3. 会话层 `privileges.rs` 调用 `Handle::ensureActiveUser`，随后通过 `Get` 获取快照。按需模式下，未见过的用户名从 `source_data` 找出其全部间接角色，借助 `MySQLPrivilege::merge` 按用户名替换相关记录，再登记到 `active_users`。
4. `RequestVerification` 对 `UsagePriv` 直接成功；否则将账户本身与经过重新校验的有效角色组成身份序列，对每个身份依次检查全局、库、表和列记录，任一作用域含目标位即成功。`column == "*"` 时，任意列级 `SELECT` 可匹配。
5. `RequestDynamicVerification` 先查询账户及有效角色的显式动态权限，并在 `with_grant=true` 时同时要求 grant option；失败后，SEM 的受限权限禁止回退，否则兼容性地用 `SUPER` 代替动态权限，且可授予场景还要求 `GrantPriv`。
6. `DBIsVisible` 对 `information_schema` 无条件可见；全局可见权限、`metrics_schema` 的 `PROCESS`、或库/表/列任一级非零授权都可使数据库可见。角色可见性由上层 `privileges.rs::DBIsVisible` 遍历有效角色补充。
7. `showGrants` 聚合账户与有效角色的各级权限，依次产生全局、库、表、列、角色和动态权限语句，并在每个作用域内排序。标识符由 `escape_identifier` 按 SQL mode 引用；只有 `GrantPriv` 时输出 `USAGE ... WITH GRANT OPTION`。

## 数据与状态

`MySQLPrivilege` 的多个 `Vec` 是有序快照。排序首先由 `compareHost` 保证精确 Host 优先于尾 `%` 模式、空 Host 与全 `%`，随后按用户及作用域键稳定区分记录；因此 `matchUser` 等线性查找会命中更具体的候选。`ColumnsPrivMap` 是按用户名分组的列权限派生索引，必须与 `columns_priv` 同步重建。

静态权限在 `u64` 中按位合并；动态权限保留为大写字符串和独立 `GrantOption`。`UserRecord` 还包含认证插件、主/附加认证串、账户锁定、密码寿命、失败登录策略、token issuer、连接上限与资源组。`decodeUserTableRow` 在插件列为空时使用 `default_auth_plugin`，再回退到 `mysql_native_password`；缺失 `password_lifetime` 用 `-1` 表示。

`Handle` 同时维护当前对外数据 `data`、完整来源快照 `source_data`、活跃用户名集合 `active_users` 和 `full_data` 模式位。`UpdateAllActive` 只将活跃用户及其角色从新来源合入现有缓存；`Update` 若目标列表不含任何活跃用户则只更新来源并直接返回。

## 依赖与调用关系

上游主要是 `pkg/privilege/privileges/privileges.rs`：它在鉴权前调用 `ensureActiveUser`，再委托 `RequestVerification`、`RequestDynamicVerification`、`DBIsVisible` 与 `showGrants`。更外层的会话执行路径在 `pkg/session/runtime/dispatch.rs`、`control.rs`、`system_query.rs`、`query.rs`、`ddl.rs`、`statistics.rs` 和 `mlog_purge.rs` 中调用这些权限接口；RustCodeGraph 的文件关系也标记了 `pkg/session/runtime.rs`、`control.rs`、`dispatch.rs`、`session.rs` 等使用者。

下游调用集中在本文件：RustCodeGraph 显示 `LoadAll` 调用八个 `Load*` 方法；`RequestVerification` 调用 `FindAllUserEffectiveRoles`、`matchUser`、`matchDB`、`matchTables`、`MatchColumns`；`showGrants` 调用角色展开、记录匹配、三个作用域格式化函数、`collectColumnGrant` 与标识符转义。外部功能依赖仅有 `sem::IsEnabled`/`IsRestrictedPrivilege` 和 `astersql_util::misc::ParseAndCheckSAN`。

`Cargo.toml` 的 `[lib] path = "lib.rs"` 说明文件属于独立 crate，而非根 crate 的内嵌源码；`lib.rs` 通过 `pub use cache::*` 暴露公开 API，并把 `cache_test.rs` 作为独立测试模块，符合源文件与测试分离约束。

## 错误处理与边界

所有数据源与解码入口返回 `Result<_, PrivilegeError>`；加载链用 `?` 保留首个错误，`loadTable` 也在任一行解码失败时停止。`noSuchTable` 仅识别 `PrivilegeError::NoSuchTable`，但当前 Rust `LoadAll` 不像 Go 实现那样对旧版本缺表降级，而是统一传播错误。

JSON 标量缺失通常按兼容默认值处理：字符串为空、整数为 0、布尔仅识别 JSON `true` 或字符串 `"Y"`。`global_priv.priv` 非对象或 SAN 解析失败不会返回错误，而将 `globalPrivRecord::Broken` 置为 true。未知 SET 权限名映射为 0。`RevokePrivilegeMask` 对不完整的 `(db, table, column)` 组合静默不操作。

Host CIDR 只接受 IPv4 `network/dotted-mask`，并要求网络地址未设置掩码外的位；无法解析时回退通配匹配。`localhost` 模式额外匹配任意 loopback IP。通配算法按字节递归，`%` 可匹配任意长度、`_` 匹配一个字节、反斜杠转义下一字节；极长且含大量 `%` 的模式具有递归回溯成本。

锁获取使用 `unwrap()`，因此某次持写锁期间 panic 会毒化锁并使后续访问 panic；API 没有把该情况转换成 `PrivilegeError`。`ensureActiveUser` 当前签名虽返回 `Result`，正常路径没有可产生的业务错误。

## 并发与资源生命周期

`Handle` 可廉价克隆，因为四个共享状态均在 `Arc` 中。读者通过 `Get` 在读锁内克隆整个 `MySQLPrivilege`，释放锁后在独立快照上工作；写者先离锁构造新快照，再短暂获取写锁替换数据，避免读者观察半加载状态。

`full_data` 使用 Release/Acquire 表达模式切换的可见性。`UpdateAll` 在数据发布后置 true；`UpdateAllActive`/`Update` 在按需更新前置 false。各 `RwLock` 并非一个事务锁：`source_data`、`data`、`active_users` 与原子位是分步更新的，调用者只能依赖最终快照语义，不能假定跨字段瞬时原子一致。

角色 BFS 的 `HashSet` 同时负责去重和环检测，队列耗尽即释放；文件不创建线程、异步任务、通道、数据库事务或长期外部资源。数据源生命周期由调用者负责，缓存只在加载期间借用 `&dyn PrivilegeDataSource`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/privilege/privileges/cache.go`。Rust 保留了 Go 的主要类型和方法命名、权限层级、角色闭包、Host 排序、动态权限的 `SUPER` 回退、数据库可见性与 `SHOW GRANTS` 顺序。Rust 独立测试 `cache_test.rs` 与 Go `cache_test.go` 都覆盖 user/global_priv/db/table/column/default-role 加载、通配 Host、角色 BFS、排序、TLS REQUIRE 和 DB 可见性。

重要边界差异如下：

- Go `Handle` 通过 session pool 实时执行 SQL，以原子指针 CAS 合并权限；Rust 用注入的 `PrivilegeDataSource` 和 `Arc<RwLock<_>>`，并保留 `source_data` 支持本地按需合并。
- Go `LoadAll` 对部分旧系统表的 `NoSuchTable` 可记录告警后继续，并把错误包装成表名相关错误；Rust 当前八步全部严格传播。
- Go 的按需路径按用户名构造过滤 SQL；Rust 的 `addUserFilterCondition` 保留字符串构造辅助，但当前 `Update` 先让数据源全量加载到 `next` 再内存筛选，性能边界不同。
- Go 可在 `skipNameResolve=false` 时走反向 DNS；Rust `matchIdentity` 明确没有 resolver 边界，只做提供 Host 的直接模式匹配。
- Go 发布完整缓存使用 atomic pointer，Rust `Get` 会深克隆快照；两者都避免暴露可变共享记录，但读放大和内存成本不同。

## 扩展指南

新增静态权限时，应同步更新位常量、正确作用域的 `ALL_*_PRIVS`、`privilege_name`、`privilege_column_name`、SET 映射和扫描集合；尤其不要把 `GrantPriv` 加入展示列表，否则 `SHOW GRANTS` 会把它既作为普通权限又作为 `WITH GRANT OPTION` 重复输出。同步扩展 `cache_test.rs` 的解码、判定、字符串输出与 Go 对照用例，并核对 `pkg/parser/mysql/privs.go`。

新增系统表字段或记录类型时，接入点是相应记录结构、`PrivilegeDataSource`、`Load*`、`decode*TableRow`、排序/合并逻辑和独立测试数据源。若字段参与按需刷新，必须加入 `MySQLPrivilege::merge`；若影响列权限，任何增删路径都必须维护 `ColumnsPrivMap`。

修改身份或 Host 规则时集中评估 `baseRecord::{hostMatch,match,fullyMatch}`、`compareHost`、`parseHostIPNet` 和 `wildcard_match`，并保留精确 Host 优先、大小写规则、loopback、CIDR、转义和恶意长模式测试。修改角色规则时同时验证撤销后的陈旧活动角色不能继续生效以及有环图能终止。

改变并发更新协议前，应明确是否仍保证“失败不发布半成品”和读者只见完整快照；若要减少 `Get` 的深克隆成本，可评估发布 `Arc<MySQLPrivilege>`，但需同时重做 `data/source_data/active_users/full_data` 的一致性设计。相应测试仍放在 `pkg/privilege/privileges/cache_test.rs`，不要嵌入生产文件。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；目标文件被索引为 2,270 行、218 个符号。文件级使用者包括 `pkg/privilege/privileges/privileges_test.rs` 与多个 `pkg/session/runtime*.rs` 文件。
- RustCodeGraph 调用边：`LoadAll -> LoadUserTable/LoadGlobalPrivTable/LoadGlobalGrantsTable/LoadDBTable/LoadTablesPrivTable/LoadColumnsPrivTable/LoadDefaultRoles/LoadRoleGraph`；`RequestVerification -> FindAllUserEffectiveRoles/matchUser/matchDB/matchTables/MatchColumns`；`showGrants -> FindAllUserEffectiveRoles/escape_identifier/*PrivToString/collectColumnGrant`；`UpdateAll -> LoadAll`。
- 已读 Rust 路径：`pkg/privilege/privileges/cache.rs`、`lib.rs`、`privileges.rs`、`cache_test.rs` 和 `Cargo.toml`。测试证实默认认证插件、密码寿命、SSL/SAN、Host/DB 通配、角色环与撤销、ANSI_QUOTES、DB 可见性及增量 grant/revoke/drop 行为。
- 已读 Go 对照：`pkg/privilege/privileges/cache.go` 与 `cache_test.go`，重点核对 `MySQLPrivilege::LoadAll`、静态/动态验证、`DBIsVisible`、`Handle` 全量/活跃用户刷新及对应测试名。
- 精确上游搜索确认 `privileges.rs` 是主要适配层，并确认会话 runtime 的授权、SHOW GRANTS 和数据库可见性路径直接使用本缓存 API。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；交付验证仅检查文档存在且恰含规定的十一个二级章节。
