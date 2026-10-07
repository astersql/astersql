# `pkg/parser/parser_actions/security.rs`

## 文件定位

`security.rs` 是 `astersql-parser` crate 内主 SQL 文法的安全与账户类语义动作实现。它不是鉴权执行器，也不访问权限表；它处在 LR 解析器的“归约后构造语义值”阶段，把 `pkg/parser/generated/main_tables.rs` 给出的稳定 `RuleId` 和归约栈右部值转换为 `parser_ast`、`parser_auth`、`parser_mysql::privs` 中的 AST/身份/权限对象。模块由 `pkg/parser/parser_actions/mod.rs` 私有声明并分类派发，实际入口来自 `pkg/parser/parser_runtime.rs` 的归约循环。

crate 边界由 `pkg/parser/Cargo.toml` 确认：包名为 `astersql-parser`，本文件直接使用的 AST、身份与 MySQL 权限类型分别来自工作区路径依赖 `astersql-parser-ast`、`astersql-parser-auth`、`astersql-parser-mysql`。文件本身无 feature 或条件编译分支。

## 核心职责

- 用私有枚举 `SecurityRule` 为账户、角色、授权、撤权、认证、TLS、资源限制和密码锁定等文法候选分配可读分支名；`identify` 将生成器产生的稳定字符串 `RuleId` 映射到这些分支。
- 用 `owns` 告诉总派发器某条归约是否属于本模块。`pkg/parser/parser_actions/remaining_aster_unit_test.rs::all_action_rules_have_one_owner` 固定当前安全规则清单为 182 条，并检查每条需要动作的规则恰有一个所有者。
- 用 `apply`/`apply_rule` 从动态类型的 `Rhs` 读取前序语义值，构造语句节点或中间值，写入 `Context.output`；必要时读取 `Context.parser_state` 的兼容模式，并通过 `Context.lexer` 记录错误或警告。
- 保持 Go 文法动作的具体语义，包括缺省主机 `%`、主机名小写化、列表累积、权限常量映射、`REVOKE ALL ... GRANT OPTION` 的特殊 AST、MariaDB 专用 `BINLOG MONITOR` 限制，以及 `ENCRYPTION` 的警告/错误行为。

## 主要符号

- `enum SecurityRule`：私有且可比较/复制的动作标签。变体按文法非终结符及候选序号命名，例如 `CreateUserStmtAlt01`、`PrivTypeAlt01..Alt37`、`AlterPasswordOrLockOptionAlt01..Alt14`。它只用于本文件内的分派，不是公开 AST。
- `fn identify(rule_id: RuleId) -> Option<SecurityRule>`：穷举稳定规则字符串；已知规则返回标签，未知规则返回 `None`。字符串与 `pkg/parser/generated/main_tables.rs::RULE_IDS_BY_REDUCTION` 同源，避免依赖会随生成变化的数字规则号。
- `pub(super) fn owns(rule_id: RuleId) -> bool`：以 `identify(...).is_some()` 实现模块归属判断，供 `parser_actions::apply` 与 `has_semantic_action` 使用。
- `fn role_from_semantic(value: RoleOrPrivSemantic) -> Option<auth::RoleIdentity>`：把显式角色原样返回，把动态权限名包装为主机 `%` 的角色；普通 `Priv` 返回 `None`，供 GRANT/REVOKE ROLE 拒绝权限项。
- `pub(super) fn apply(...) -> Option<Result<bool, isize>>`：先识别规则；非本模块规则返回 `None`，已识别规则调用 `apply_rule`。
- `fn apply_rule(...) -> Result<bool, isize>`：核心动作表。成功处理返回 `Ok(true)`；必要输入类型缺失时部分分支返回 `Ok(false)`；语义错误追加 lexer 错误并返回 `Err(1)`。

## 执行流程

1. `pkg/parser/parser_runtime.rs` 在负 action 上取得 `RULE_IDS_BY_REDUCTION[generated_reduction]`，建立当前右部的 `Rhs` 和包含输出值、解析器状态、lexer 的 `Context`。
2. `pkg/parser/parser_actions/mod.rs::apply` 按模块顺序调用 `owns`；安全规则命中后调用 `security::apply(...).expect("owned security rule has an action")`。
3. `security::apply` 通过 `identify` 再次取得 `SecurityRule`，进入 `apply_rule`。后者记录 `rhs_len`，再按“距右端多少项”读取 `rhs[rhs_len - back]`；索引语义由 `parser_actions/mod.rs` 的 `Index/IndexMut for Rhs` 提供。
4. 叶子规则先产生中间值：例如 `PrivTypeAlt*` 产生 `PrivilegeType`，`UsernameAlt*`/`Rolename*` 产生身份，`ObjectTypeAlt*` 与 `PrivLevelAlt*` 产生授权范围，认证/TLS/连接/密码规则产生相应 option。
5. 列表规则把单项包装为 `Vec`，或克隆已有向量后追加新项；空候选写入空向量、`None` 或缺省布尔值。上层语句规则再组合这些中间值，写入 `out.statement`，典型结果包括 `CreateUserStmt`、`AlterUserStmt`、`DropUserStmt`、`SetRoleStmt`、`GrantProxyStmt`、`GrantRoleStmt`、`RevokeStmt` 和 `RevokeRoleStmt`。
6. `apply_rule` 返回后，runtime 将归约值压回解析栈；若动作返回 `Err(status)`，解析立即以该状态结束。若返回未处理，runtime 才使用 goyacc 风格的 `$$ = $1` 回退，但按唯一归属测试，正常安全规则应命中其动作。

若按领域观察动作表，重要分支如下：

- 身份与角色：裸用户名/角色名使用 `%`；带主机形式将主机小写，并对单 `@identifier` 去掉前导 `@`；`CURRENT_USER` 设置 `current_user` 标志。
- 用户生命周期：CREATE USER/ROLE 和 ALTER USER 共享规格、TLS、资源、密码/锁定、注释/属性和资源组的组装逻辑；DROP ROLE 复用 `DropUserStmt`，以 `IsDropRole` 区分。
- 权限：37 个 `PrivType` 候选映射为 `parser_mysql::privs` 常量；对象类型和 global/database/table 范围分别形成 `ObjectTypeType` 与 `GrantLevel`。
- GRANT/REVOKE：`RoleOrPrivSemantic` 暂存角色、静态权限或动态权限名。角色语句必须能经 `role_from_semantic` 转换；REVOKE 的 `[AllPriv, GrantPriv]` 特例生成全局 `RevokeStmt`，而非 `RevokeRoleStmt`。
- TLS/密码：REQUIRE 子句构建 `AuthTokenOrTLSOption`；密码历史、复用、过期、失败次数、锁定时长等构建 `PasswordOrLockOption`，只有带数值的候选读取计数。

## 数据与状态

`apply_rule` 不持有跨调用状态。输入状态全部借用于一次归约：`Rhs<'_>` 是解析栈右部的可变视图，`Context.output` 是本次归约的 `yySymType`，`Context.parser_state` 是当前 `Parser`，`Context.lexer` 是当前 `yyLexer`。

语义值通过 `yySymType.item: Option<Box<dyn Any...>>` 一类动态槽位传递，因此代码用 `downcast_ref` 检查具体类型；多数读取会克隆拥有值，少数透传分支用 `take()` 转移所有权，例如 `SetRoleStmtAlt01` 和若干 option 包装规则。`out.statement` 保存最终语句节点，`out.item` 保存中间结构，`out.ident`/`out.expr` 保存标识符或表达式语义。

唯一被读取的持久解析配置是 `parser_state.enableMariaDB`：`BINLOG MONITOR` 仅在该模式开启时映射为 `ReplicationClientPriv`。其他状态变化是 lexer 的错误/警告列表；本文件不修改数据库、会话权限或外部资源。

## 依赖与调用关系

上游调用链为：

`generated/main_tables.rs::RULE_IDS_BY_REDUCTION` → `parser_runtime.rs` 归约循环 → `parser_actions::apply` → `security::owns` → `security::apply` → `apply_rule`。

RustCodeGraph 对目标文件的直接关系显示：`apply` 调用 `identify` 和 `apply_rule`，`owns` 调用 `identify`，`apply_rule` 调用 `role_from_semantic`；索引还识别到 `pkg/parser/parser_actions/remaining_aster_unit_test.rs` 对本文件的直接使用。总派发器源码进一步证明 `security` 位于 query 与 admin 动作之间，未知规则最终由其他模块处理或返回 `Ok(false)`。

下游类型依赖主要为：

- `parser_ast`：语句、授权级别、用户规格、认证/TLS、资源、密码锁定以及辅助列表节点。
- `parser_auth::...::auth`（并经父模块导入为 `auth`）：`UserIdentity`、`RoleIdentity`。
- `parser_mysql::privs`：静态权限常量。
- 父模块的 `RuleId`、`Rhs`、`Context`、`Parser`、`yyLexer` 与 `yySymType`。

这些都是内存内同步调用；本文件没有网络、磁盘、事务、channel 或异步任务依赖。

## 错误处理与边界

- 未识别 `RuleId`：`identify` 返回 `None`，`apply` 返回 `None`；总派发器只会在 `owns` 已确认后解包，因此“归属与动作表不一致”会触发 `expect`，唯一归属测试用于提前发现这种漂移。
- 动态类型不符：关键必需值（如 `UserToUser` 两端、`PrivElem` 的权限、SET DEFAULT ROLE 的中间节点）会返回 `Ok(false)`；许多可选或列表值则采用 `unwrap_or_default()`，保持 Go 空切片/零值语义。扩展时必须精确核对 RHS 偏移，否则错误类型可能被静默降为缺省值。
- 角色/权限混用：GRANT ROLE 或普通 REVOKE ROLE 遇到 `Priv` 时，通过 `AppendError(Errorf("expected role"))` 并返回 `Err(1)`；无法判定 `RoleOrPrivElem` 类型时报告 `invalid role or privilege`。
- MariaDB 边界：未启用 `enableMariaDB` 时 `BINLOG MONITOR` 追加 `syntax error` 并返回 `Err(1)`。
- ENCRYPTION：`Y/y` 被接受但追加“所有存储引擎忽略”错误后调用 `LastErrorAsWarn` 降为警告；`N/n` 静默接受；其他值追加参数错误并返回 `Err(1)`。这与 `pkg/parser/parser.y::EncryptionOpt` 的 Go 行为一致，但 Rust 当前错误文本不是 Go 的 `ErrWrongValue.GenWithStackByArgs` 类型化构造。
- `Rhs` 索引越界会在父模块的 `Index` 实现中以 `expect("semantic RHS position")` panic；稳定规则与动作偏移必须成对更新。

## 并发与资源生命周期

本文件没有内部并发。每次动作只在所属解析调用的当前线程上，短暂借用解析栈、输出槽、`Parser` 与 lexer；Rust 生命周期阻止这些引用逃逸。构造出的 AST、字符串和向量转为拥有值后随 parser 结果生命周期管理，临时借用在 `apply_rule` 返回时结束。

解析器实例本身的并行安全不由此文件保证；调用方若并发解析，应使用相互独立的 `Parser`/lexer/栈。错误和警告追加到本次 lexer，不存在全局锁、共享缓存或需显式释放的句柄。

## 与 Go 版本的对应关系

直接 Go 对照不是同名 `.go` 文件，而是 `pkg/parser/parser.y` 中相同非终结符的内嵌语义动作；`pkg/parser/parser.go` 是 goyacc 生成结果。Rust 把这些动作从文法文件拆到本文件，并用稳定哈希式 `RuleId` 替代数字 case。

已核对的等价点包括：CREATE/ALTER/DROP USER/ROLE 的字段组合；角色与用户缺省主机 `%`；权限、对象类型和授权级别映射；`BINLOG MONITOR` 的 MariaDB 开关；GRANT ROLE 的角色转换；`REVOKE ALL [PRIVILEGES], GRANT OPTION` 特例；以及 ENCRYPTION 的 Y/N 分支。

实现形态有两项值得注意：Go 使用具体接口断言和指针切片，Rust 使用动态 `Box` 下转型及拥有值向量；Go 的 `ast.RoleOrPriv` 同时承载 node/symbols，Rust 用私有 `RoleOrPrivSemantic::{Role, Dynamic, Priv}` 表达同一中间态。二者不应仅因类型布局不同被误判为行为差异。

相关独立 Rust 测试为：

- `pkg/parser/parser_actions/remaining_aster_unit_test.rs`：规则唯一归属、182 条 security inventory、禁止数字规则回退。
- `pkg/parser/grant_role_aster_unit_test.rs`：GRANT/REVOKE 裸角色与用户均保留 `%` 缺省主机。
- `pkg/parser/parser_3_aster_unit_test.rs::parser_3_builds_go_account_option_nodes`：CREATE USER 的 IF NOT EXISTS、认证、REQUIRE SSL、资源限制、密码过期与账户锁定 AST。
- `pkg/parser/parser_test.rs` / `pkg/parser/parser_test.go`：更广泛的 SQL parser 合同与 Go 回归语料；本任务只将其作为相关测试面，不声称逐案执行。

## 扩展指南

新增或修改安全文法动作时，应按以下接点同步：

1. 先在 `pkg/parser/parser.y` 明确 Go 基准语义，再更新生成表来源，使新 production 获得稳定 `RuleId`；不要引入数字规则号回退。
2. 在 `SecurityRule` 增加对应标签，并在 `identify` 增加精确字符串映射；确认该规则不会被其他 action 模块认领。
3. 在 `apply_rule` 按真实 production 右部核对每个 `rhs_len - back`，优先使用与邻近规则一致的中间类型。必需值应明确失败，可选值才使用缺省值。
4. 若增加权限、认证或用户选项，同步检查 `parser_mysql::privs`、`parser_ast` 和 `parser_auth` 的类型是否已表达该语义；不要在动作层复制执行期权限逻辑。
5. 测试必须放在独立 Rust 测试文件，至少扩展 `parser_actions/remaining_aster_unit_test.rs` 的归属清单保障，并在 `pkg/parser/*_test.rs` 添加端到端解析断言；同时对照 `parser.y`/Go 测试覆盖缺省值、大小写、非法混用和错误/警告分支。

主要兼容风险是 stable `RuleId` 漂移、RHS 偏移错位、动态下转型类型不匹配、Go 空值与 Rust default 的差异，以及错误类别/文本偏离。性能上，该文件在每次相关归约中做字符串 RuleId 匹配并可能克隆向量；增加大列表动作时应关注重复克隆，但没有证据表明当前实现是性能瓶颈。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、目标 `security.rs` 已索引为 190 个符号；`files --filter pkg/parser/parser_actions` 确认模块及独立测试；精确文件节点读取覆盖 `SecurityRule`、`identify`、`owns`、`role_from_semantic`、`apply`、`apply_rule` 全部 1,840 行。`explore` 给出 `apply → identify/apply_rule`、`owns → identify`、`apply_rule → role_from_semantic` 及归属测试调用边。
- Rust 源：`pkg/parser/parser_runtime.rs`（RuleId 取得、动作调用、错误状态传播与默认归约）；`pkg/parser/parser_actions/mod.rs`（模块声明、Context、Rhs 索引、总派发）；`pkg/parser/generated/main_tables.rs`（稳定 RuleId 表）；`pkg/parser/lib.rs`（parser runtime/生成表的 crate 装配）。
- crate 配置：`pkg/parser/Cargo.toml`（包名、lib 路径及 AST/auth/mysql 路径依赖）。
- Go 对照：`pkg/parser/parser.y` 的 RenameUserStmt、用户/角色、认证、TLS、密码锁定、GRANT/REVOKE、权限与 EncryptionOpt 动作；`pkg/parser/parser.go` 的相应生成 case。
- 测试：`pkg/parser/parser_actions/remaining_aster_unit_test.rs`、`pkg/parser/grant_role_aster_unit_test.rs`、`pkg/parser/parser_3_aster_unit_test.rs`，以及相关 parser 合同测试路径。
- 本任务为纯文档分析，按计划不运行 Cargo。交付结构检查要求文档存在且恰有十一个固定二级标题；人工复核范围为文件存在原因、运行路径、安全扩展点及当前行为边界。
