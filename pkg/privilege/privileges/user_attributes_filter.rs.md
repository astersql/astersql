# `pkg/privilege/privileges/user_attributes_filter.rs`

## 文件定位

本文件属于 `astersql-privilege-privileges` crate；crate 入口 `pkg/privilege/privileges/lib.rs` 以私有模块 `mod user_attributes_filter` 装配它，再通过 `pub use user_attributes_filter::*` 导出 `UserAttrFilter` 与 `NewUserAttrFilter`。它位于权限缓存与 INFORMATION_SCHEMA 行生成之间，专门实现 MySQL 8.0.22+ 的 `INFORMATION_SCHEMA.USER_ATTRIBUTES` 行可见性规则，不负责读取 `mysql.user.user_attributes`、解析属性 JSON 或构造结果行。

当前 Rust 生产入口是 `pkg/executor/infoschema_reader.rs::setDataForUserAttributes`：执行器先取得候选行、当前登录身份、活动角色和 `UserPrivileges`，构造本文件的过滤器，再对每个 `(user, host)` 调用 `Visible`。因此，本文件回答的是“某一账号行能否被当前查看者看到”，而不是“属性内容如何展示”。

## 核心职责

- `NewUserAttrFilter` 在一次 USER_ATTRIBUTES 扫描开始时，根据查看者的权限快照选定 `All`、`NonSystem` 或 `SelfOnly` 模式。
- `UserAttrFilter::Visible` 对候选账号执行模式对应的逐行判定，并保证受限模式下先识别查看者自身，再排除其他账号。
- 规则优先级与 Go 文件 `pkg/privilege/privileges/user_attributes_filter.go` 保持一致：`mysql.user` 的 SELECT/UPDATE 可看全部；CREATE USER 加 SYSTEM_USER 可看全部；只有 CREATE USER 可看自己和非系统账号；其余查看者只能看自己。
- `SkipWithGrant()`、缺少权限管理器或空查看者身份采用兼容性宽松回退，直接生成 `All` 过滤器。

## 主要符号

- `enum AttrVisMode { All, NonSystem, SelfOnly }`：文件私有的决策结果。`All` 不再访问权限缓存；`NonSystem` 允许本人和非 SYSTEM_USER 账号；`SelfOnly` 只允许本人。
- `pub struct UserAttrFilter`：一次扫描使用的不可变过滤快照。`privilege: Option<MySQLPrivilege>` 仅在受限路径中为 `Some`；`viewer_user`、`viewer_host` 保存查看者身份；`mode` 保存构造阶段的授权结论。
- `pub fn NewUserAttrFilter(active_roles, viewer_user, viewer_host, manager) -> UserAttrFilter`：公开构造入口。它从 `UserPrivileges.Handle.Get()` 获取 `MySQLPrivilege` 克隆快照，并调用 `RequestVerification`、`RequestDynamicVerification` 选择模式。
- `pub fn UserAttrFilter::Visible(&self, user, host) -> bool`：公开逐行判定入口。参数是候选账号的用户名和主机模式，而 `self.viewer_*` 是查看者的连接身份，两者不可互换。

命名沿用 Go API，故 Rust 公开函数和方法使用 `NewUserAttrFilter`、`Visible` 的非 snake_case 形式；crate 根的 `#![allow(non_snake_case, ...)]` 明确允许这种移植命名。

## 执行流程

1. `NewUserAttrFilter` 先创建默认 `All` 过滤器，且暂不保存权限缓存。
2. 若 `SkipWithGrant()` 为真、`manager` 为 `None`，或查看者 user/host 同时为空，则立即返回默认过滤器。生产执行器在取不到 `user_attributes_privileges()` 时会传入 `None`；测试也固定了该宽松行为。
3. 正常路径调用 `manager.Handle.Get()`，得到本次构造使用的 `MySQLPrivilege` 值快照。
4. 先用活动角色检查查看者是否对 `mysql.user` 具有 `SelectPriv` 或 `UpdatePriv`。任一成立即选择 `All`。
5. 否则检查全局 `CreateUserPriv`。没有该权限时选择 `SelfOnly`；有该权限时再检查查看者的动态 `SYSTEM_USER` 权限：有则 `All`，无则 `NonSystem`。
6. 正常路径无论最终模式为何，都会把权限快照存入 `filter.privilege`；随后返回过滤器。
7. `Visible` 在 `All` 模式立即返回 `true`。受限模式从 `privilege` 取快照，调用 `matchUser(user, host)` 找到候选账号记录，再用该记录的 `base.match(viewer_user, viewer_host)` 判断候选记录能否匹配查看者身份；匹配则优先返回 `true`。
8. 未命中本人时，`SelfOnly` 返回 `false`；`NonSystem` 则以空活动角色检查候选账号自身是否具有 `SYSTEM_USER`，取反后作为结果。

最后一步故意不把查看者的活动角色传给候选账号检查：它判定的是目标账号本身（及 `RequestDynamicVerification` 对该账号适用的兼容规则）是否属于系统用户，而不是重新授权查看者。

## 数据与状态

过滤器只拥有三个小型状态类别：两个查看者身份 `String`、一个 `AttrVisMode`，以及可选的完整 `MySQLPrivilege` 克隆。它不修改权限缓存、不维护跨调用计数，也不保存候选行引用。

`Handle::Get`（`pkg/privilege/privileges/cache.rs`）在读锁下克隆 `MySQLPrivilege`。因此同一个 `UserAttrFilter` 的所有 `Visible` 调用观察同一快照；后台刷新 `Handle` 不会让扫描中途切换权限视图。代价是构造受限过滤器时克隆缓存，之后逐行匹配 `MySQLPrivilege.user` 和动态权限记录。

账号自识别依赖 `cache.rs::baseRecord::match`：用户名精确、区分大小写，host 按缓存记录的模式执行 `hostMatch`（支持 `%`/`_`、CIDR 和 localhost/loopback 特例）。这意味着“同用户名但不同 host 模式”不必然属于本人；`user_attributes_host_matching_and_fallbacks` 明确覆盖了该边界。

## 依赖与调用关系

上游生产调用链为：

`InfoSchemaReader::retrieve` 的 USER_ATTRIBUTES 分支 → `setDataForUserAttributes`（`pkg/executor/infoschema_reader.rs`）→ `NewUserAttrFilter` → 对每个合法三列候选行调用 `UserAttrFilter::Visible`。

本文件的直接下游均由 crate 根重导出：

- `UserPrivileges.Handle.Get` 提供 `MySQLPrivilege` 快照。
- `MySQLPrivilege::RequestVerification` 汇总查看者及有效角色的全局、库、表和列级静态权限；这里分别查询 `mysql.user` 的 SELECT/UPDATE 与全局 CREATE USER。
- `MySQLPrivilege::RequestDynamicVerification` 检查 `SYSTEM_USER`，并包含 cache 层定义的 SEM/SUPER 兼容行为。
- `MySQLPrivilege::matchUser` 与 `baseRecord::match` 完成候选记录和查看者身份匹配。
- `SkipWithGrant` 是权限子系统的全局跳过开关。

`pkg/privilege/privileges/Cargo.toml` 将本文件编入 `astersql-privilege-privileges` 库，没有针对此模块的 feature 条件；文件本身也没有条件编译项。它直接使用 crate 内类型，未直接引入新的外部 crate。

## 错误处理与边界

公开 API 返回布尔值或过滤器，不返回 `Result`。权限不足属于正常决策，不是错误；缺少管理器、空身份和 `SkipWithGrant` 都按 Go 兼容语义放行，而不是报错。

受限模式的 `Visible` 使用 `expect("restricted filter has a cache")`。构造器保证只有正常路径才产生 `NonSystem`/`SelfOnly`，并在返回前写入 `Some(privilege)`，所以该 panic 表示结构内部不变量被破坏；外部调用者不能直接构造私有字段来制造此状态。

找不到候选账号缓存记录时不能认定为本人：`SelfOnly` 拒绝，`NonSystem` 继续把候选身份交给动态权限检查。空活动角色、host 模式差异和缺失账号的行为由独立 Rust 测试覆盖。执行器层还会在调用本文件前跳过非三列或 user/host 非文本的行，这不是本过滤器的职责。

需要特别注意：`RequestDynamicVerification` 可在非 SEM 受限模式下由 SUPER 兼容动态权限，因此文档中的“系统账号”应理解为该方法的真实判定结果，而不能简化成只查 `mysql.global_grants`。

## 并发与资源生命周期

本文件不创建线程、任务、通道、事务、文件或网络资源。`NewUserAttrFilter` 短暂读取 `Handle` 内的 `RwLock`，锁在 `Get()` 克隆结束时释放；返回的过滤器持有自有 `MySQLPrivilege` 值，不持有锁守卫或对 `Handle` 的借用。

过滤器方法只需 `&self`，逐行判定期间不改变自身状态。扫描生命周期由 `setDataForUserAttributes` 的局部变量控制：候选行过滤完成后过滤器及其缓存克隆一并释放。若未来把过滤器跨线程共享，应先依据其所有字段的线程安全性质和上游生命周期验证，而不应从当前 `&self` 签名直接推断官方并发契约。

## 与 Go 版本的对应关系

Rust 文件逐分支移植 `pkg/privilege/privileges/user_attributes_filter.go`：三种模式、权限检查顺序、本人优先、SYSTEM_USER 排除及宽松回退均一致。Go 集成测试 `pkg/privilege/privileges/privileges_test.go::TestInfoSchemaUserAttributes` 证明了最终 SQL 行集合：普通用户和仅 SUPER 用户只见本人，具有 `mysql.user` SELECT 的用户见全部，只有 CREATE USER 的用户看不到 SYSTEM_USER 账号。

实现形态有以下差异：

- Go 公开 `UserAttrFilter` interface，并以私有 `userAttrFilter` 指针实现；Rust 直接公开具体 struct。
- Go 接收通用 `privilege.Manager` 并在类型断言不是 `*UserPrivileges` 时放行；Rust 参数已收窄为 `Option<&UserPrivileges>`，注释约定用 `None` 表示缺失或外来 manager。
- Go `priv` 是缓存指针；Rust `Handle::Get` 返回克隆值，因此过滤器是稳定快照。
- Go 的整数 mode 有 default 拒绝分支；Rust enum 不能构造未知合法枚举值，不需要对应分支。
- Go 的 `All` 返回对象通常不保存查看者字符串；Rust 先统一复制 viewer 字符串再早退。这不改变可见性，只带来少量固定分配。

Rust 单元测试位于独立文件 `pkg/privilege/privileges/user_attributes_filter_test.rs`，没有把测试内嵌进生产文件；执行器级覆盖位于 `pkg/executor/infoschema_reader_internal_test.rs::user_attributes_visibility_follows_mysql_privileges` 和 `user_attributes_retriever_preserves_shape_errors_and_memory`。

## 扩展指南

- 修改授权矩阵时，优先调整 `NewUserAttrFilter` 的模式选择；新增逐行目标属性判断时，调整 `Visible`。不要把结果行形状、JSON/NULL 转换或内存记账塞入本文件，那些属于 `setDataForUserAttributes`。
- 新增模式时必须同时定义构造条件、本人优先级、缺失候选记录行为和 SYSTEM_USER/SEM/SUPER 兼容语义，并同步 Go 对照实现或明确记录迁移差异。
- 若改变 `mysql.user` 所需权限粒度，应检查 `RequestVerification` 的数据库/表参数是否仍准确；尤其不能把 `mysql.user` 权限误写成任意 `mysql.*` 规则，尽管库级授权可能由 cache 方法自然覆盖表请求。
- 若改变快照策略，应评估扫描一致性与克隆成本，且需同步检查 `Handle::Get` 的锁生命周期及执行器一次扫描只构造一个过滤器的假设。
- 回归测试优先扩展 `user_attributes_filter_test.rs` 的直接模式/host/回退用例，并扩展 `infoschema_reader_internal_test.rs` 验证最终行集合。用户可见 SQL 兼容性还应对齐 Go 的 `TestInfoSchemaUserAttributes`；Rust 测试仍应保持在独立测试文件中。

## 验证依据

- 生产源：`pkg/privilege/privileges/user_attributes_filter.rs`，核对 `AttrVisMode`、`UserAttrFilter`、`NewUserAttrFilter`、`Visible` 的全部分支。
- crate 边界：`pkg/privilege/privileges/lib.rs` 与 `Cargo.toml`，确认模块重导出、库入口、依赖及无 feature 门控。
- RustCodeGraph：`query UserAttrFilter`、`query NewUserAttrFilter`、`node NewUserAttrFilter`、`node RequestVerification`、`node RequestDynamicVerification`、`node matchUser`；索引识别出 Rust/Go 对照符号和 cache 下游，但 `files --filter pkg/privilege/privileges/user_attributes_filter` 未返回路径，故生产跨 crate 调用再以 `rg` 核验。
- Rust 上游与下游：`pkg/executor/infoschema_reader.rs::setDataForUserAttributes`、`pkg/privilege/privileges/cache.rs::{Handle::Get, MySQLPrivilege::matchUser, MySQLPrivilege::RequestVerification, MySQLPrivilege::RequestDynamicVerification, baseRecord::match}`。
- Rust 测试：`pkg/privilege/privileges/user_attributes_filter_test.rs`；执行器集成边界：`pkg/executor/infoschema_reader_internal_test.rs`。
- Go 对照与测试：`pkg/privilege/privileges/user_attributes_filter.go`、`pkg/executor/infoschema_reader.go::setDataForUserAttributes`、`pkg/privilege/privileges/privileges_test.go::TestInfoSchemaUserAttributes`。
- 本任务按计划为纯文档分析，未运行 Cargo；交付前使用任务规定的命令验证本文恰有十一个固定二级章节，并人工复核本文件只陈述上述代码与测试可支持的行为。
