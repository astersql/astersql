# `pkg/parser/auth/auth.rs`

## 文件定位

[`auth.rs`](./auth.rs) 是 `astersql-parser-auth` crate 中的身份数据模型文件。crate 根在 [`lib.rs`](./lib.rs) 中以 `pub mod auth` 暴露本模块，并通过 `parser::auth::auth` 兼容迁移后的导入路径；[`Cargo.toml`](./Cargo.toml) 指定 crate 名为 `astersql-parser-auth`、库入口为 `lib.rs`，且以 `astersql-parser-format` 和 `serde` 分别提供 SQL 恢复与序列化能力。

本文件位于“SQL 语法身份表示”边界：解析器的安全相关动作会构造 `UserIdentity`/`RoleIdentity`，AST 节点和会话变量会保存它们，后续权限、会话、表达式与执行模块再读取这些值。它不实现密码散列、认证插件协议、权限匹配或网络握手；同 crate 的 `mysql_native_password.rs`、`caching_sha2.rs`、`tidb_sm3.rs` 承担散列/校验逻辑。

## 核心职责

- 用 `UserNameMaxLength = 32` 与 `HostNameMaxLength = 255` 保留 Go 版本的身份长度常量。当前文件只声明上限，不在构造、恢复或显示时主动校验。
- 用 `UserIdentity` 同时保存客户端提交的登录身份（`username`、`hostname`）和权限系统最终匹配的认证身份（`auth_username`、`auth_hostname`），另存 `current_user` 语法标记和握手选定的 `auth_plugin` 名称。
- 用 `RoleIdentity` 保存角色名及可选主机名。
- 为两类身份提供 SQL 恢复规则和面向日志/显示的字符串规则；两者用途不同，不能把 `identity_string` 或 `role_string` 当成可安全拼接回 SQL 的转义函数。
- 通过 `optional_identity_string`、`optional_login_string` 显式映射 Go 的 nil receiver 行为：`None` 返回空串。

## 主要符号

- `pub const UserNameMaxLength: usize = 32`、`pub const HostNameMaxLength: usize = 255`：与 [`auth.go`](./auth.go) 同名常量对齐；本文件没有使用它们实施约束。
- `pub struct UserIdentity`：公开、可克隆、可比较、可默认构造，并派生 `serde::Serialize`/`Deserialize`。各字段用 `Username`、`Hostname`、`CurrentUser`、`AuthUsername`、`AuthHostname`、`AuthPlugin` 作为 serde 名称，以保持 Go 风格的数据形状。
- `UserIdentity::restore(&self, &mut RestoreCtx) -> io::Result<()>`：把身份恢复成 `CURRENT_USER`，或把登录身份恢复成带名称转义的 `user@host` SQL 文本。
- `UserIdentity::identity_string(&self) -> String`：若 `auth_username` 非空，返回权限表匹配身份 `auth_username@auth_hostname`；否则返回登录身份 `username@hostname`。
- `UserIdentity::login_string(&self) -> String`：始终返回客户端登录身份。`Display for UserIdentity` 委托给 `identity_string`，因此格式化默认展示认证后身份。
- `optional_identity_string(Option<&UserIdentity>)`、`optional_login_string(Option<&UserIdentity>)`：分别委托上述显示方法；`None` 通过 `unwrap_or_default` 变为空串。
- `pub struct RoleIdentity`：含 `username`、`hostname` 两个公开字符串字段，同样支持克隆、比较、默认值及使用 Go 字段名的 serde 序列化。
- `RoleIdentity::restore`：总是恢复角色名，仅当主机非空时恢复 `@host`。
- `RoleIdentity::role_string`：始终返回反引号包围的 `` `role`@`host` `` 形态，包括空主机；`Display for RoleIdentity` 委托此方法。

文件没有 trait、自定义错误类型、条件编译项或内部私有辅助函数；所有业务符号均为公开 API。

## 执行流程

1. 解析安全语法时，[`parser_actions/security.rs`](../parser_actions/security.rs) 的 `UsernameAlt01..04` 根据语法分支构造 `UserIdentity`：省略主机时默认 `%`，显式主机转为小写，`CURRENT_USER` 分支只设置 `current_user = true`。`Rolename*` 分支以相同方式构造 `RoleIdentity`。
2. 构造出的身份进入 AST。例如 [`ast/lib.rs`](../ast/lib.rs) 的 `ShowStmt.User/Roles`、`SetRoleStmt.RoleList`、`SetDefaultRoleStmt.RoleList/UserList` 和 `UserSpec.User` 直接持有本文件的类型。
3. AST 需要还原 SQL 时，调用 `restore`。`UserIdentity` 先判断 `current_user`：真时只调用 `RestoreCtx::WriteKeyWord("CURRENT_USER")`；假时依次 `WriteName(username)`、`WritePlain("@")`、`WriteName(hostname)`。`RoleIdentity` 先写角色名，再按非空主机条件写 `@host`。
4. 需要诊断、显示或身份键文本时，调用 `identity_string`、`login_string`、`role_string` 或 `Display`。这些路径直接拼接字段，不经过 `RestoreCtx`，所以不会进行 SQL 名称转义。
5. 权限系统完成匹配后可填写 `auth_username`/`auth_hostname`；此后 `identity_string` 和 `Display` 展示匹配项，而 `login_string` 仍保留原始登录来源，二者共同区分“登录者”与“授权时命中的账户记录”。`auth_plugin` 仅随身份保存，不触发插件调用。

## 数据与状态

`UserIdentity` 是纯拥有型值对象，六个字段均为 `String` 或 `bool`，没有隐藏状态。其关键不变量来自方法分支而非类型系统：

- `current_user = true` 时，`restore` 忽略其余身份字段；这允许解析阶段用默认空字符串表示 `CURRENT_USER`。
- `auth_username` 是否为空是“是否展示认证匹配身份”的唯一判据；即使 `auth_hostname` 已填写，只要 `auth_username` 为空仍回退到登录身份。
- `auth_plugin` 是握手选择结果的记录字段，恢复和三个字符串方法都不会读取它。
- `RoleIdentity.hostname` 为空时，SQL 恢复省略整个 `@host`，但 `role_string` 仍输出空主机位置。这是恢复语法与诊断显示之间有意保留的差异。
- `Default` 会产生所有字符串为空、布尔值为假的对象；文件本身不拒绝空用户名、超长名称或不规范插件名。

派生的 serde 实现使用显式字段重命名，因而持久化/交换形状依赖这些 Go 风格键名。修改字段名、类型或 serde 属性属于兼容性变更，不能只看 Rust 调用点。

## 依赖与调用关系

直接下游依赖只有 `crate::parser::format`：`restore` 使用其中的 `RestoreCtx`、`WriteKeyWord`、`WriteName` 和 `WritePlain`。[`format/format.rs`](../format/format.rs) 证明 `WriteName` 根据恢复标志选择引号、转换大小写并成对转义内部定界符，`WriteKeyWord` 处理关键字大小写，三种写入都可能返回 `io::Error`。

`serde` 来自本 crate 的 [`Cargo.toml`](./Cargo.toml)，只参与两个结构体的派生序列化。标准库提供 `fmt::Display`、`String` 和 `io::Result`。Cargo 中其余散列、随机数、错误等依赖由同 crate 的其他认证模块使用，不是本文件的直接依赖。

RustCodeGraph 对 `pkg/parser/auth/auth.rs` 识别到 12 个符号，并报告文件被 `pkg/executor/infoschema_reader.rs`、`pkg/executor/infoschema_reader_internal_test.rs`、`pkg/parser/auth/migration_aster_unit_test.rs`、`pkg/parser/hintparserimpl.rs`、`pkg/parser/lexer_5_aster_unit_test.rs` 等文件使用。源码搜索进一步确认：

- [`parser_actions/security.rs`](../parser_actions/security.rs) 是身份值的直接构造入口；[`ast/lib.rs`](../ast/lib.rs) 是主要 AST 承载边界。
- `pkg/sessionctx/variable/variable.rs` 的会话变量保存 `Option<UserIdentity>` 与 `Vec<RoleIdentity>`。
- `pkg/session/sessionapi/lib.rs` 再导出 `UserIdentity`；会话认证与多处会话/执行集成测试直接构造该类型。
- `pkg/expression/sessionexpr/lib.rs`、`pkg/expression/expropt/lib.rs` 再导出两类身份，供当前用户和活动角色表达式读取。

RustCodeGraph 对 `identity_string` 的精确 callers 查询未返回静态调用边；这不代表 API 未使用，因为派生/trait 调用、跨 crate 再导出和字段承载不一定形成可解析的方法调用边。上述源码引用是这一图覆盖限制下的直接补证。

## 错误处理与边界

`restore` 的唯一显式失败来源是底层 `RestoreWriter` 的 I/O 写入。每次写入均用 `?` 立即传播；若中途失败，调用者可能观察到已经写出的前缀，本文件不回滚输出，也不包装错误。字符串显示方法只分配并格式化 `String`，没有可恢复错误返回。

名称长度常量不是验证器：调用者必须在协议、DDL 或权限边界实施长度检查。`identity_string`、`login_string` 和 `role_string` 仍保留 Go 中“尚未实现显示转义”的语义，包含 `@`、反引号或其他特殊字符时只能作为显示文本，不能代替 `restore`。`RoleIdentity::role_string` 甚至直接把字段放入反引号而不转义内部反引号。

Go 可在 nil `*UserIdentity` 上调用 `String`/`LoginString`；Rust 的实例方法不能在空引用上调用，因此只有两个 `optional_*` 函数提供等价边界。`RoleIdentity` 没有 optional 辅助函数，因为 Go 对应方法也未处理 nil receiver。

## 并发与资源生命周期

本文件没有锁、原子变量、全局可变状态、任务、通道、事务或外部资源句柄。两类身份值拥有自己的字符串；克隆会复制状态，方法除向调用者提供的 `RestoreCtx` 写入外不修改自身。

并发安全由值的所有权和调用者决定：不可变借用的显示方法可安全并行读取同一身份，但 `restore` 需要对单个 `RestoreCtx` 的独占可变借用，写入顺序因此在一次调用内是串行的。文件不负责认证身份何时从登录值更新为权限匹配值，也不保证跨线程的会话一致性；生命周期由保存它的 AST、会话变量或上层 `Arc`/会话对象管理。

## 与 Go 版本的对应关系

直接对照文件为 [`auth.go`](./auth.go)。常量值、两个结构体的数据含义、`CURRENT_USER` 分支、用户 `user@host` 恢复、角色空主机省略、认证身份优先显示以及登录身份显示均保持一致。

主要语言映射与差异如下：

- Go 导出字段使用 PascalCase；Rust 使用 snake_case，并用 serde `rename` 保留 Go 字段键名。
- Go `Restore` 写入接口没有返回每一步的错误；Rust 的格式化接口返回 `io::Result`，因此本文件逐步传播写入错误，最终成功时显式返回 `Ok(())`。
- Go `String`/`LoginString` 的 nil receiver 返回空串；Rust 用 `Option<&UserIdentity>` 辅助函数表达，普通实例方法只接受有效引用。
- Go 的 `String()` 方法对应 Rust 的显式字符串方法以及 `Display`；`Display<UserIdentity>` 选择认证身份优先规则，`Display<RoleIdentity>` 选择反引号格式。
- Go 与 Rust 都不在本文件执行长度校验、显示字符串转义或认证插件逻辑；不应把常量或 `AuthPlugin` 字段误解为已经执行了这些行为。

## 扩展指南

- 新增身份字段时，先明确它属于登录输入、权限匹配结果还是握手元数据，再同步 `UserIdentity`、serde 键、Go `auth.go`、解析动作构造点、AST/会话消费方与独立测试。兼容风险主要是序列化形状和下游结构体字面量编译失败。
- 修改 SQL 表示时应改 `restore`，并通过 `RestoreCtx` 写入，不能复用未转义的显示方法。新增分支需覆盖关键字大小写、名称引号、内部反引号、空主机和写入失败后的错误传播。
- 修改用户显示优先级时应同时审查 `identity_string`、`Display` 与 `login_string`，避免把权限表匹配身份和客户端登录身份混淆；权限审计、日志和会话函数可能依赖这种区别。
- 修改角色格式时应分别决定 SQL 恢复与诊断字符串的行为，因为当前空主机规则有意不同。
- 长度验证若要落地，应放在明确的输入/协议或语义验证层，并复用这里的常量；直接在数据结构构造中截断会改变 Go 行为并可能掩盖错误。
- 测试必须放在独立 Rust 测试文件。最直接的同步位置是 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)；若改变解析构造规则，还应扩展 parser 的独立 lexer/parser 测试。不要把 `#[cfg(test)]` 测试内嵌到 `auth.rs`。

性能上，这些方法都会为显示结果分配新 `String`，`restore` 则流式写入。若新增高频格式化路径，应避免无必要的重复克隆，但不能以缓存换取会话身份陈旧或引入共享可变状态。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/parser/auth` 确认目标源码、Go 对照和独立测试均已索引；`node --file pkg/parser/auth/auth.rs --offset 1 --limit 260` 读取了完整 137 行源码并报告直接使用文件；`query UserIdentity`、`query RoleIdentity`、`query identity_string --json` 核对了主要符号；精确 `callers` 查询未返回方法级调用边，已用直接源码引用补证。
- 源文件：[`auth.rs`](./auth.rs)；crate 边界：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)；格式化语义：[`format/format.rs`](../format/format.rs)。
- 上游构造与承载：[`parser_actions/security.rs`](../parser_actions/security.rs)、[`ast/lib.rs`](../ast/lib.rs)；跨模块消费通过 `pkg/sessionctx/variable/variable.rs`、`pkg/session/sessionapi/lib.rs`、`pkg/expression/sessionexpr/lib.rs` 等引用核对。
- Go 对照：[`auth.go`](./auth.go)。独立 Rust 测试：[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 的 `identities_match_go_display_and_restore_rules` 与 `current_user_restore_matches_go`，覆盖认证身份优先、登录身份、None/nil 兼容、名称转义、角色空主机和 `CURRENT_USER`。同目录 Go 测试主要覆盖各密码插件，没有 `auth.go` 身份结构的同名独立测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工核对只新增本说明、未修改 Rust/Go/Cargo/`plan.md`。
