# [`pkg/privilege/privilege.rs`](./privilege.rs)

## 文件定位

本文件是 `astersql-privilege` crate 的权限门面定义，源码由 `pkg/privilege/lib.rs` 以 `privilege_api` 私有模块装入，再同时从 crate 根和 `privilege` 命名空间公开。它对应 Go 包入口 `pkg/privilege/privilege.go`，负责规定“上层会话如何持有并调用权限管理器”，而不负责读取 `mysql.*` 权限表或实现具体认证算法。

`pkg/privilege/Cargo.toml` 将该 crate 的库入口设为 `lib.rs`，依赖认证身份、MySQL 权限类型、认证连接、会话变量、`Datum` 和受限 SQL 执行器等多个细粒度本地 crate。当前 Rust 仓库中，`BindPrivilegeManager`、`GetPrivilegeManager` 和本文件的 `Manager` trait 仅在 `pkg/privilege/migration_aster_unit_test.rs` 找到直接使用；具体权限逻辑位于独立的 `astersql-privilege-privileges` crate，但尚未实现本文件的 `Manager` trait。因此，本文件目前是已定义、已单测的迁移边界，不应描述成已经接入 Rust 完整应用主链。

## 核心职责

- 定义对象安全的 `Manager` trait，把 SHOW GRANTS、静态/动态权限检查、连接认证、账户锁定、身份匹配、库可见性、角色关系和用户资源限制统一为一个会话可持有的接口（`Manager`，第 73 行）。
- 定义认证返回值 `VerificationInfo`，传递沙箱模式、错误密码归因和资源组名（第 62—69 行）。
- 用不可与其他键混淆的 `KeyType`/`KEY` 标识会话中的权限管理器槽位，并提供稳定字符串 `"privilege-key"`（第 39—57、193 行）。
- 用 `PrivilegeManagerKeyProvider` 与 `SessionContext` 隔离具体会话容器，只要求读取或写入 `Option<Arc<dyn Manager>>`（第 203—211 行）。
- 提供 `BindPrivilegeManager` 与 `GetPrivilegeManager` 两个薄包装，完成管理器的绑定、替换、解绑和读取（第 197—216 行）。

## 主要符号

- `KeyType(i32)`：字段私有，外部不能构造任意键；`String`、`string` 和 `Display::fmt` 都产生固定文本 `privilege-key`。公开的唯一预置值是 `KEY = KeyType(0)`。
- `VerificationInfo`：可克隆、可比较且有 `Default`。三个公开字段分别为 `InSandBoxMode: bool`、`FailedDueToWrongPassword: bool`、`ResourceGroupName: String`；默认值与 Go 零值一致，即两个标志为 `false`、字符串为空。
- `Manager`：包含 21 个必实现方法，没有默认实现。方法可按职责分为：授权展示（`ShowGrants`）、静态权限（`RequestVerification*`）、动态权限（`HasExplicitlyGrantedDynamicPrivilege`、`RequestDynamicVerification*`）、登录与账户状态（`VerifyAccountAutoLockInMemory` 至 `MatchIdentity`）、可见性与元数据行（`DBIsVisible`、`UserPrivilegesTable`）、角色图（`ActiveRoles`、`FindEdge`、`GetDefaultRoles`、`GetAllRoles`）以及认证插件/连接资源（`GetAuthPluginForConnection`、`GetUserResources`）。
- `PrivilegeManagerKeyProvider`：只读能力，`value(KEY)` 返回一个克隆后的共享 trait object 或 `None`。
- `SessionContext`：继承只读能力并增加 `set_value`；具体实现决定 `Some` 如何保存以及 `None` 如何清除。
- `BindPrivilegeManager`：把调用方给出的 `Option<Arc<dyn Manager>>` 原样交给 `SessionContext::set_value(KEY, ...)`。
- `GetPrivilegeManager`：调用 `PrivilegeManagerKeyProvider::value(KEY)`，不创建默认管理器，也不把缺失视为错误。

## 执行流程

绑定流程只有一跳：调用方把可共享的具体实现提升为 `Arc<dyn Manager>`，再调用 `BindPrivilegeManager(ctx, Some(manager))`；函数使用常量 `KEY` 调用上下文的 `set_value`。再次绑定由上下文覆盖旧值；传入 `None` 表示请求解绑。`pkg/privilege/migration_aster_unit_test.rs::manager_binding_matches_go_context_round_trip_and_replacement` 用 `HashMap` 上下文验证了首次绑定、替换与解绑。

读取流程同样只有一跳：`GetPrivilegeManager` 以相同 `KEY` 调用 `value` 并返回 `Option<Arc<dyn Manager>>`。调用方必须显式处理尚未绑定的 `None`；成功时获得的是同一管理器的共享所有权，测试通过 `Arc::ptr_eq` 验证并未复制管理器对象。

权限业务流程不在本文件内执行。获得管理器之后，调用方才会按场景调用 trait 方法；例如静态检查使用数据库、表、列和 `PrivilegeType`，动态检查使用权限名与 `grantable`，连接认证还会传入认证响应、盐、会话变量及可变 `AuthConn`。每个方法的实际分支、缓存查询和错误构造属于未来的 trait 实现，而不是此门面。

## 数据与状态

本文件自身没有全局可变状态。`KEY` 是零大小语义上的固定槽位标识；`KeyType` 的私有整数仅用于相等、哈希和调试，显示文本不依赖整数值。

权限管理器通过 `Arc<dyn Manager>` 共享。`BindPrivilegeManager` 转移一个 `Arc`（包装在 `Option` 中）到会话容器，`GetPrivilegeManager` 的契约要求容器返回另一个共享句柄。旧管理器何时释放取决于所有 `Arc` 克隆何时离开作用域，而不是绑定函数主动销毁。

`VerificationInfo` 是一次连接校验的值对象，不在本文件中缓存。`Manager` 的大多数输入使用借用切片或字符串引用，输出则按值返回 `Vec`、`String`、布尔值或 `Result`，所以接口没有把调用方输入的生命周期保存在门面层。

## 依赖与调用关系

直接下游依赖由 `pkg/privilege/lib.rs` 转接：`RoleIdentity`/`UserIdentity` 来自 parser auth，`PrivilegeType` 来自 parser mysql，`AuthConn` 来自 privilege conn，`Context`/`SessionVars` 来自 sessionctx，`Datum` 来自 types，`PrivilegeError`/`RestrictedSqlExecutor` 来自 util sqlexec。`std::fmt` 支持键显示，`std::sync::Arc` 提供共享所有权。

RustCodeGraph 对精确调用的有效结果为：测试 `manager_binding_matches_go_context_round_trip_and_replacement` 调用 `BindPrivilegeManager` 与 `GetPrivilegeManager`；`BindPrivilegeManager` 调用 `SessionContext::set_value`；`GetPrivilegeManager` 调用 `PrivilegeManagerKeyProvider::value`。普通名称（如 `Manager`、`value`、`String`）在全仓有大量同名符号，必须结合文件路径消歧，不能把那些边当作本接口的调用关系。

Cargo 依赖搜索显示 session、server、DDL、executor、planner、infoschema、gcworker 和 expression/sessionexpr 等 crate 声明了 `astersql-privilege` 依赖，但精确 Rust 源码搜索只找到本 crate 的迁移测试直接导入本文件 API。这说明“Cargo 可达”不等于“运行时已接线”。Go 对照则已在 `pkg/session/session.go` 绑定管理器，并由 planner、executor、server、infoschema 等路径频繁读取；这些 Go 调用只能作为预期架构证据，不能作为 Rust 已接线证据。

## 错误处理与边界

绑定和读取函数本身不返回错误：槽位缺失由 `None` 表示，存储失败也没有表达通道，因为 `SessionContext::set_value` 返回 `()`。新增上下文实现必须保证 `value(KEY)` 与 `set_value(KEY, ...)` 对同一槽位操作，并明确把 `None` 实现为删除而不是保存一个无法区分的空值。

`Manager` 将可能失败的操作分成两类。`ShowGrants`、自动解锁检查、连接认证、认证插件查询和资源限制查询返回 `Result<_, PrivilegeError>`；普通权限判断、角色边与可见性检查主要返回布尔值或集合。布尔返回值无法携带拒绝原因，调用层若需要用户可见错误，必须根据所调用方法的契约自行构造，不能假设 trait 会记录错误。

边界上，`GetPrivilegeManager` 允许未绑定；`ConnectionVerification` 接收可变 `AuthConn`，意味着实现可能通过连接执行认证插件交互；`MatchUserResourceGroupName` 接收可变受限 SQL 执行器，意味着实现可能执行内部查询。文件没有为 `Manager`、`SessionContext` 或 `PrivilegeManagerKeyProvider` 添加 `Send + Sync` 上界，故仅凭本接口不能断言 trait object 可跨线程移动或共享。

## 并发与资源生命周期

`Arc` 只保证引用计数是线程安全的，不会自动使 `dyn Manager` 的内部状态线程安全；本文件也没有 `Mutex`、`RwLock`、原子变量、任务、通道或事务。是否允许跨线程使用，应由后续 trait 上界和具体实现共同决定，当前 API 没有承诺。

绑定时，会话容器成为管理器的一个所有者；替换或解绑只释放容器持有的那一个引用，其他 `Arc` 仍可使旧管理器继续存活。`GetPrivilegeManager` 返回拥有所有权的 `Arc`，因此即使上下文随后解绑，已取出的句柄仍然有效。接口没有关闭/回收回调，具体管理器若持有连接、缓存或后台资源，必须在其自身 `Drop` 或显式生命周期 API 中管理。

测试上下文使用普通 `HashMap` 且只在单线程测试中访问，不能作为生产并发安全证据。文档未发现本文件生产上下文实现或运行时绑定点，因此并发行为仍取决于未来接线。

## 与 Go 版本的对应关系

类型和方法集合基本逐项映射 `pkg/privilege/privilege.go`：`keyType`/`key` 对应 `KeyType`/`KEY`，`VerificationInfo` 三个字段一致，Go `Manager` 的 21 个方法在 Rust trait 中均有对应项，Bind/Get 也保留 Go 风格命名。

关键语言适配包括：Go 的接口值改为 `Arc<dyn Manager>`；Go 的 `nil` 改为 `Option::None`；Go 的指针切片改为 Rust 值切片；Go `error` 改为 `PrivilegeError`；Go `context.Context` 映射为这里再导出的 `ExecutionContext`。Go 的上下文通过 `fmt.Stringer` 键和 `any` 做运行时类型断言，类型不匹配时返回 `nil`；Rust 把存储接口收窄为 `KeyType -> Option<Arc<dyn Manager>>`，因此类型匹配在编译期保证。

行为差异是 Rust 明确支持 `BindPrivilegeManager(..., None)` 解绑，而 Go 参数虽可传 `nil`，其通用上下文实际如何保存由 Go 会话实现决定。Rust 的 `KeyType` 为公开类型但字段私有，Go 的 `keyType` 整体包私有。Rust 另增小写 `string` 别名以兼容 Go 命名习惯。

迁移状态并不等价：Go 的 `pkg/privilege/privileges/privileges.go` 有 `var _ privilege.Manager = (*UserPrivileges)(nil)`，且完整服务路径已使用 Bind/Get；Rust 的 `pkg/privilege/privileges/privileges.rs` 定义了自己的 `VerificationInfo` 与 `UserPrivileges` 固有方法，其 crate 清单也未依赖 `astersql-privilege`，没有实现本 trait。因此扩展时必须先决定是连接两套 API，还是继续把本门面视为迁移契约，不能假设二者已经可互换。

## 扩展指南

新增权限能力时，先在 Go 对照接口和 Rust `Manager` 中确认是否属于跨子系统契约；若增加 trait 必选方法，所有实现（当前至少测试 `TestManager`，未来还包括生产适配器）都必须同步修改。不要为通过编译给测试或生产实现添加无语义桩：应复刻 Go 分支、返回值和错误语义，并把 Rust 测试放在独立的 `pkg/privilege/migration_aster_unit_test.rs` 或对应实现 crate 的独立测试文件中。

接通生产实现的最小合理位置是为真实权限对象提供本 trait 的适配实现，并让真实会话上下文实现 `PrivilegeManagerKeyProvider`/`SessionContext`，随后在 Rust 会话创建路径绑定、在 planner/executor/server 等消费者读取。接线时重点核对两个 Rust `VerificationInfo` 的字段差异、方法接收者可变性、`Datum` 与字符串表格返回类型，以及 `Context`/受限执行器/认证连接的适配，避免静默丢失 Go 语义。

若只改变键存储，必须扩充现有往返测试，覆盖空值、替换、解绑和 `Arc` 身份/生命周期；若改变认证结果，扩充 `verification_info_default_matches_go_zero_value` 并增加具体错误与沙箱/资源组用例；若改变 trait 方法，除门面契约测试外还要同步具体实现 crate 的 `pkg/privilege/privileges/*_test.rs`。兼容风险主要是破坏下游 trait 实现，正确性风险主要是未绑定时被调用方错误地 `unwrap`，性能风险则集中在频繁克隆 `Arc` 与未来实现中的缓存/SQL 调用，而非当前薄包装。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/privilege` 确认目标及相关 Rust/Go/测试文件已索引；`node --file pkg/privilege/privilege.rs --offset 1 --limit 400` 读取了完整 217 行源码。
- RustCodeGraph 精确查询：`query BindPrivilegeManager`、`query GetPrivilegeManager`、`query KeyType`、`query VerificationInfo`、`query Manager`；结合文件路径消歧后，`explore` 验证迁移测试到 Bind/Get、Bind 到 `set_value`、Get 到 `value` 的调用边。
- crate 边界：读取 `pkg/privilege/Cargo.toml` 与 `pkg/privilege/lib.rs`；另查阅 `pkg/privilege/privileges/Cargo.toml`、`lib.rs` 和 `privileges.rs`，确认具体逻辑位于独立 crate 且当前没有实现本文件 trait。
- Go 对照：读取 `pkg/privilege/privilege.go`；检索 `pkg/session/session.go`、`pkg/planner/**`、`pkg/executor/**`、`pkg/server/**` 等 Bind/Get 调用；`pkg/privilege/privileges/privileges.go` 的编译期断言证明 Go `UserPrivileges` 实现接口。
- 测试证据：读取独立 Rust 测试 `pkg/privilege/migration_aster_unit_test.rs`，其三个测试覆盖键文本、`VerificationInfo::default` 以及 Bind/Get/替换/解绑；检索 Go `pkg/privilege/privileges/privileges_test.go` 与相关调用，确认成熟 Go 路径中的权限检查与连接认证用法。
- 未运行 Cargo 或代码测试：任务明确为纯文档分析。交付仅执行任务指定的 11 章节结构检查，并人工复核本文只陈述可由上述源码、图查询、Cargo 和测试支持的事实。
