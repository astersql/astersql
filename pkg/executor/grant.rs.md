# `pkg/executor/grant.rs`

## 文件定位

`grant.rs` 属于 `astersql-executor` crate；`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 与 `pkg/executor/lib.rs` 的 `pub mod grant` 共同把它公开为 `astersql_executor::grant`。文件承载 GRANT 权限写入模型、事务流程以及可复用的账户 TLS 选项序列化逻辑。

当前接线并不完全相同：仓库内 `GrantExec` 的 Rust 生产调用者尚未找到，除 `pkg/executor/grant_test.rs::executor` 外没有实例化点；实际 Go 执行链仍由 `pkg/executor/builder.go` 构造 `pkg/executor/grant.go::GrantExec`。另一方面，公开函数 `account_tls_options_to_global_priv` 已由 `pkg/session/runtime/control.rs::execute_alter_user` 和 `execute_create_user` 调用。因此本文件不是纯桩，但应区分“已接线的 TLS 转换”与“尚未接入 Rust SQL 执行主链的 GRANT 主执行器”。

## 核心职责

- `GrantExec::Next` 实现一次性执行语义：解析并规范化授权对象，切换到新事务，通过内部系统会话写权限表，提交后通知权限缓存更新。
- `GrantExec::apply_grant` 处理 `CURRENT_USER`、用户存在性、兼容 MySQL 5.7 的隐式建用户、各级权限行初始化、`WITH GRANT OPTION` 与逐项授权。
- `grantDynamicPriv`、`grantGlobalLevel`、`grantDBLevel`、`grantTableLevel` 和 `grantColumnLevel` 将动态权限及全局/库/表/列静态权限转为 `SystemMutation`。
- `tlsOption2GlobalPriv` 和 `account_tls_options_to_global_priv` 共用 `tls_options_to_global_priv_with_validation`，校验 REQUIRE 选项并生成 `mysql.global_priv` 所用 JSON。
- `compose*UpdateForGrant`、`recordExists`、`getTablePriv`、`getColumnPriv` 等辅助函数维护 SET 类型权限的“读取—合并—写回”语义，并通过 `RecordSet` 抽象读取系统表。

## 主要符号

- `GrantErrorKind`、`GrantError`、`GrantResult<T>`：本地错误协议。可区分通用错误、表不存在、非法权限级别、动态权限未注册；`Display` 直接输出 `message`。
- `GrantLevelKind` / `GrantLevel`、`ObjectType`、`PrivilegeType` / `PrivilegeElement`：表示授权层级、对象类型与权限项。`PrivilegeType::Extended` 是动态权限；`StaticPrivilege` 保存权限系统表列名、SET 名以及各级适用标志。
- `UserIdentity`、`UserSpec`、`AuthOption`：目标账号及认证选项。`UserIdentity.current_user` 指示执行时替换为已认证用户。
- `AuthTokenOrTlsOptionType`、`AuthTokenOrTlsOption`、`SslType`、`GlobalPrivValue`：REQUIRE TLS/X509/SAN/token issuer 的输入与持久化模型。
- `SystemMutation`、`SystemQuery`：把具体 SQL 隔离为类型化读写操作，覆盖 `mysql.user`、`mysql.global_priv`、`mysql.db`、`mysql.tables_priv`、`mysql.columns_priv` 与动态授权表。
- `RecordSet`、`SystemSession`：同步内部会话边界；前者分块读取和关闭结果集，后者负责事务、查询、写入、grantor 与 chunk 大小。
- `GrantDependencies: Send + Sync`：执行器的环境适配接口，提供当前库/用户、系统会话池、元数据、用户名和认证编码校验、动态权限注册、TLS 校验以及权限更新通知。
- `GrantExec`：持有权限、层级、用户、TLS 选项、catalog、一次性 `done` 状态和 `Arc<dyn GrantDependencies>` 的主状态机。
- `account_tls_options_to_global_priv`：本文件除测试外已确认的 Rust 生产入口，把 parser AST 选项映射为本地类型后复用 GRANT 的 TLS 序列化规则。

## 执行流程

1. `GrantExec::Next` 首次调用即把 `done` 设为 `true`；后续调用直接成功返回，独立测试 `grant_global_dynamic_and_tls_matches_go_mutations` 验证只开始和提交一次事务。
2. 它从 `GrantLevel` 或 `GrantDependencies::current_database` 取得库名。库级调用 `getTargetSchemaName` 规范化名称；表级先由 `validate_table_grant` 校验权限层级，再加载表元数据并把库表名改为规范大小写。不存在的表仅在包含 `ALL` 或 CREATE 类权限时被允许。
3. `new_transaction_in_statement` 结束/替换外部旧事务语境，随后 `run_internal_transaction` 获取 `SystemSession`，设置 grantor 用户并 `begin`。
4. `apply_grant` 逐个处理用户：解析 `CURRENT_USER`，检查用户是否存在；若不存在且未启用 `NO_AUTO_CREATE_USER`，校验用户名、选择认证插件、编码密码并发送 `CreateUser` mutation。
5. 每个用户先按需初始化 `global_priv`，再按 Database/Table 层级初始化空权限行。`WITH GRANT OPTION` 且存在非动态权限时，额外追加静态 `Grant` 权限；纯动态权限则把 grant option 随 `ReplaceDynamicGrant` 单独保存。
6. `grantGlobalPriv` 写 TLS JSON；`grantLevelPriv` 忽略 `USAGE`，动态权限交给 `grantDynamicPriv`，其他权限按层级分发。表和列权限先读取已有 SET，去重合并后再写回；`ALL` 则从 `PrivilegeCatalog` 展开。
7. 全部 mutation 成功后提交，再调用 `notify_update_privilege`。提交前失败会尝试回滚；无论结果如何都释放系统会话。`Next` 最后清除外部 `in_transaction` 标志，并优先保留原授权错误。
8. 独立的 `account_tls_options_to_global_priv` 路径不经过 `GrantExec`：它映射 parser AST，校验重复项、cipher、X509 name、SAN，调用 `serialize_global_priv`，由 session 的 CREATE/ALTER USER 流程消费结果。

## 数据与状态

- `GrantExec.done` 是一次性闩锁，在实际事务开始前就置位；首次失败后同一实例不会重试。
- `PrivilegeCatalog` 是 `ALL` 的展开来源，分别维护 global/database/table/column 静态权限列表；它决定 mutation 中的赋值列或 SET 元素，而非在本文件硬编码完整权限全集。
- 表级数据同时维护 `Table_priv` 和 `Column_priv`；`composeTablePrivUpdateForGrant` 对普通权限保留已有值并去重追加，对 `ALL` 使用 catalog 全量重建。列级逻辑同样读取并合并 `Column_priv`。
- 名称比较使用 ASCII 不区分大小写，最终写入元数据给出的规范库、表、列名。`grant_table_all_canonicalizes_names_and_expands_table_and_column_sets` 与 `grant_column_uses_canonical_column_and_rejects_non_column_privilege` 覆盖该不变量。
- `GlobalPrivValue::default` 使用 `SslType::NotSpecified`。序列化映射为 Go `SslType` 数值 `-1/0/1/2/3`，并模拟 Go JSON `omitempty`：`TlsNone` 最终是 `{}`，仅 token issuer 不产生 TLS 值而返回 `None`。
- `CellValue`、`ResultField` 和 `FieldType::Set` 控制系统表查询结果解析；`set_cell` 仅在字段声明为 SET 且单元格确为 `CellValue::Set` 时返回内容，否则以空串处理。

## 依赖与调用关系

上游关系：

- `pkg/executor/lib.rs` 公开 `grant` 模块，并仅在 `#[cfg(test)]` 下装配 `grant_test`。
- `pkg/session/runtime/control.rs::execute_alter_user` 和 `execute_create_user` 调用 `account_tls_options_to_global_priv`，分别把错误包装为 `ALTER USER REQUIRE` / `CREATE USER REQUIRE` 会话错误。
- `GrantExec` 的 Rust 上游目前只有 `pkg/executor/grant_test.rs::executor`；Go 生产对照入口为 `pkg/executor/builder.go`，它构造 `pkg/executor/grant.go::GrantExec`。

下游关系：

- `GrantExec::Next -> validate_table_grant -> table_by_name/canonical_schema_name` 完成对象与权限合法性校验。
- `Next -> run_internal_transaction -> apply_grant -> grantGlobalPriv/grantLevelPriv` 是主调用链；`grantLevelPriv` 再分派到动态及四级静态权限写入。
- 系统表 I/O 只通过 `SystemSession::{execute,query}` 与 `SystemMutation` / `SystemQuery` 表达，环境行为由 `GrantDependencies` 注入。
- TLS 的公共转换直接依赖 `astersql-parser-ast`、`astersql-util-tls::SupportCipher`、`astersql-util::misc::{CheckSupportX509NameOneline, ParseAndCheckSAN}`；这些 crate 均在 `pkg/executor/Cargo.toml` 声明。
- RustCodeGraph 对 `GrantExec::Next` 给出的主要下游包括 `getTargetSchemaName`、`validate_table_grant`、`run_internal_transaction`；对后两者继续追踪可达 `apply_grant`、各类 `grant*`、`compose*` 与系统会话 trait 方法。图查询对部分常见名称产生了跨语言歧义，调用者结论因此又用仓库级引用搜索核验。

## 错误处理与边界

- `validate_table_grant` 先报权限层级错误，再检查表是否存在，与 Go 为 issue #29302 保留的错误优先级一致。列清单只能搭配列级权限、`ALL` 或 `USAGE`。
- 动态权限必须用于 global 层级且必须已注册，错误分别使用 `IllegalPrivilegeLevel` 和 `DynamicPrivilegeNotRegistered` 分类；名字在写入前转为大写。
- DB 级拒绝 `global_only` 权限；各级 compose 函数也拒绝无对应系统表列或 SET 表示的权限类型。
- 表不存在时，表级 `ALL`/CREATE 可以继续使用输入表名写权限行；列级始终必须取得真实表与列。空库名进入 `getTargetSchemaAndTable` 会报 `missing database name`。
- REQUIRE 同类选项不可重复；cipher 必须在支持列表中，Issuer/Subject/SAN 分别经过对应校验。当前 enum 是封闭集合；与 Go `tlsOption2GlobalPriv` 的 `default: Unknown ssl type` 不同，Rust 不存在运行时未知枚举分支。
- `run_internal_transaction` 在提交前错误时回滚，但刻意忽略 rollback 错误；授权错误优先于 session release 错误。提交已成功而通知失败时不会回滚已提交内容。
- `getRowsAndFields` 在成功读完后关闭 `RecordSet`；如果读取过程先返回错误，`?` 会使函数提前返回而不会显式调用 `close`。读取表/列权限要求至少一行，否则附加 user/host/db/table 上下文报错。
- `serialize_global_priv` 手写 JSON 转义，覆盖引号、反斜杠、常见控制符和其他控制字符；新增字段时必须同步 Go JSON 的字段名、数值与省略规则。

## 并发与资源生命周期

- 本文件没有异步任务、线程、通道或内部锁。执行流程是同步、串行的：用户顺序执行，单个用户的权限项也顺序写入。
- `GrantDependencies` 要求 `Send + Sync` 并由 `Arc` 共享；`SystemSession` 与 `RecordSet` 要求 `Send`，但 `GrantExec` 本身依赖可变 `&mut self` 驱动，不在文件内并发执行。
- 系统会话生命周期为 acquire → set user → begin → apply → commit/rollback → release。release 在事务结果之后始终执行；rollback 仅在失败且尚未提交时尝试。
- 查询游标按 `maximum_chunk_size` 循环拉取，空 chunk 表示 EOF；正常路径随后关闭游标并读取字段描述。
- 权限缓存通知发生在 commit 后，保证消费者看到已持久化数据；其失败只向上返回错误，不能撤销已经提交的权限变更。

## 与 Go 版本的对应关系

- Rust `GrantExec` 的字段和 `Next` 主阶段对应 `pkg/executor/grant.go::GrantExec` 与 `Next`：一次性执行、库表规范化、旧事务切换、内部事务、隐式建用户、权限行初始化、逐级授权、提交后通知均被保留。
- Rust 用 `GrantDependencies`、`SystemSession`、`SystemMutation`、`SystemQuery` 隔离 TiDB 具体 session/infoschema/SQL 字符串；Go 版本直接调用 `sessionctx`、`InfoSchema` 和 `ExecuteInternal`。这是依赖倒置差异，不是授权语义删减。
- `PrivilegeCatalog` 对应 Go 的 `mysql.AllGlobalPrivs`、`AllDBPrivs`、`AllTablePrivs`、`AllColumnPrivs`；`StaticPrivilege` 的级别标记与列名/SET 名对应 Go `mysql.PrivilegeType` 的集合成员及 `ColumnString`/`SetString`。
- `account_tls_options_to_global_priv` 复用与 Go `tlsOption2GlobalPriv` 相同的空输入、重复项、cipher/X509/SAN 校验及 JSON 省略语义。Rust 测试明确验证 `{}`、SSL/X509 数值、token issuer-only 的 `None` 和 Go 错误文案。
- 已确认的迁移差异是生产接线：Go `GrantExec` 由 `builder.go` 构造并实现执行器接口；Rust `GrantExec` 尚无生产构造点。Rust 仅 TLS helper 已被 session runtime 的 CREATE/ALTER USER 路径使用，所以不能宣称整个 GRANT 执行器已经替代 Go。
- Go 回归测试 `pkg/executor/grant_test.go` 覆盖 global、DB、table、column scope、大小写与错误；Rust 独立测试 `pkg/executor/grant_test.rs` 聚焦 mutation、事务一次性、规范名称、ALL 展开、列权限与 TLS 行为。两者是互补证据，Rust 测试不是 Go 全部集成覆盖的等价替代。

## 扩展指南

- 新增权限类型或适用级别时，先更新上游构造的 `StaticPrivilege`/`PrivilegeCatalog`，再检查 `PrivilegeType::{column_name,set_name}`、四个 `is_*_privilege`、`composeGlobalPrivUpdate`、`composeDBPrivUpdate` 以及 table/column SET 合并；同步扩展 `pkg/executor/grant_test.rs`，并用 `pkg/executor/grant_test.go` 的对应 scope 行为核对。
- 新增系统表读写时，优先扩展 `SystemMutation`/`SystemQuery` 及 `SystemSession` 适配实现，不要在核心流程中拼 SQL；测试 mock 也应在独立的 `grant_test.rs` 同步处理，避免把测试嵌入源文件。
- 修改事务顺序时必须保持：旧事务先切换、内部会话必释放、提交前错误回滚、提交后再通知、原业务错误优先。应新增失败注入式独立测试，覆盖 begin/apply/commit/release/notify 各阶段，而不仅验证成功 mutation。
- 扩展 REQUIRE 类型时，要同步 parser AST 映射、`AuthTokenOrTlsOptionType`、重复项文案、校验函数、`GlobalPrivValue` 与 `serialize_global_priv`，并逐项对齐 Go `tlsOption2GlobalPriv` 和 `privileges.GlobalPrivValue` 的 JSON 兼容格式。
- 若把 Rust `GrantExec` 接入生产执行链，需要先提供 `GrantDependencies`/`SystemSession` 的真实适配器和 builder 构造点，并验证权限缓存通知及系统会话池生命周期；当前文件本身不能证明这些接线已存在。
- 性能上应避免在新增逻辑中为每个权限重复查询同一 SET；现实现按权限项执行读取—写回，批量优化必须保证顺序合并和去重语义不变，并评估多用户、多列授权的系统表往返次数。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；读取了 `pkg/executor/grant.rs` 全部 1576 行，并查询 `GrantExec`、`GrantExec::Next`、`account_tls_options_to_global_priv` 的节点与 callers/callees。图确认 `Next` 到 `validate_table_grant`、`run_internal_transaction`、`getTargetSchemaName`，以及后续到 `apply_grant` 和各类授权函数的调用边。
- 源与装配：`pkg/executor/grant.rs`、`pkg/executor/lib.rs`、`pkg/executor/Cargo.toml`。
- Rust 上游生产证据：`pkg/session/runtime/control.rs::execute_alter_user`、`execute_create_user`；仓库级 Rust 引用搜索未发现 `GrantExec` 的生产实例化点。
- Go 对照：`pkg/executor/grant.go` 的 `GrantExec::Next` 对应流程、`tlsOption2GlobalPriv`、各级 `grant*` 与 `compose*`；`pkg/executor/builder.go` 是 Go 构造入口。
- 独立 Rust 测试：`pkg/executor/grant_test.rs` 的六个测试覆盖错误/default、动态权限与 TLS mutation、表名规范化和 ALL 展开、列名规范化和非法列权限、账户 TLS JSON、校验错误。
- Go 回归证据：`pkg/executor/grant_test.go` 的 `TestGrantGlobal`、`TestGrantDBScope`、两个大小写测试、`TestGrantTableScope`、`TestGrantColumnScope`。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前仅执行任务规定的 11 章节结构检查，并人工复核没有把未接线的 Rust `GrantExec` 描述为生产主链。
