# `pkg/parser/ast/misc.rs` 逻辑说明

## 文件定位

[`misc.rs`](./misc.rs) 是 `astersql-parser-ast` crate 中公开的 `misc` 子模块，由 [`lib.rs`](./lib.rs) 的 `pub mod misc` 暴露。它把 Go [`misc.go`](./misc.go) 中一批相对独立的杂项 AST 行为移植成 Rust 值类型：保存语句字段、把节点还原为 SQL、生成不泄露凭据的安全文本，并提供一个轻量访问者协议。

这个模块不是完整解析器的唯一 AST 类型面。`lib.rs` 仍定义主 `Node`/`Visitor` 体系以及若干同名规范节点；语法动作主要构造那一套类型。当前可确认的生产侧直接使用以辅助能力为主：`pkg/sessionctx/variable/sysvar_builtins.rs`、`pkg/server/handler/tikvhandler/global_variables.rs`、`pkg/ddl/backfilling_clean_s3.rs`、`pkg/objstore/parse.rs` 和 `pkg/dxf/importinto/scheduler.rs` 调用 `misc::redact_url`，`pkg/ddl/bdr/lib.rs` 再导出 `BDRRole`/`BdrRole`。因此扩展前必须先判断目标属于 `misc.rs` 的轻量兼容面，还是 `lib.rs` 的主 AST 面，必要时两处同步。

crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义：包名为 `astersql-parser-ast`，入口是 `lib.rs`，本文件直接使用标准库 `BTreeMap` 和外部 `url` crate；crate 还依赖 parser-auth、parser-charset、parser-mysql、parser-types、serde 与 serde_json，但本文件没有直接调用这些依赖。

## 核心职责

1. 表达杂项 SQL 的轻量 AST 数据：事务与预处理语句、用户/角色/权限、绑定与扩展统计、ADMIN/FLUSH、TRACE/EXPLAIN、BRIE、流量捕获回放、资源校准和 QUERY WATCH 等。
2. 通过各类型的 `restore` 方法执行确定性的 SQL 文本还原；标识符经 `quote_name` 反引号转义，字符串经 `quote_string` 单引号和反斜杠转义。
3. 通过 `SensitiveStatement` 统一安全文本入口，保护密码、对象存储 URL 凭据、流量回放密码和 embedding API key。
4. 表达优化器提示：`HintTable`、递归的 `LeadingList`/`LeadingItem`、`HintData` 与 `TableOptimizerHint` 负责查询块、表、分区、索引和各类提示载荷的格式化。
5. 用 `MiscVisitor`/`MiscVisitable` 提供轻量节点遍历契约，并通过宏为叶节点批量实现；它与 `lib.rs` 的完整、可遍历子节点的 `Visitor`/`Node` 契约不同。

## 主要符号

- 基础常量与简单值：`READ_COMMITTED`、`READ_UNCOMMITTED`、`SERIALIZABLE`、`REPEATABLE_READ`、`OPTIMISTIC`、`PESSIMISTIC`，以及 `TypeOpt`、`FloatOpt`、`TextString`、`Ident`、`SelectStmtOpts`。其中 `BinaryLiteral` 只声明 `to_string_value`，本文件未提供实现。
- SQL 引用辅助：私有 `quote_name` 与 `quote_string`。前者把反引号翻倍，后者转义反斜杠和单引号；多数 `restore` 依赖这两个函数保持输出可重放。
- PLAN REPLAYER：`PlanReplayerStmt::{load,capture,remove,dump_statements,dump_slow_query,restore}`。`restore` 的分支优先级为 load、capture、remove、dump；dump 再区分单语句、慢查询/文件和语句列表。
- 提示：`HintTable::restore`、`LeadingList::{flatten,restore_with_qb}`、`HintData`、`TableOptimizerHint::restore`。`LeadingList::flatten` 深度优先保留表顺序；还原时查询块名只附着一次，嵌套列表保留括号。
- QUERY WATCH：`QueryWatchOptionType`、`QueryWatchOption`、`check_query_watch_append`、`AddQueryWatchStmt`、`DropQueryWatchStmt`、`QueryWatchResourceGroupOption`、`QueryWatchTextOption`。重复检查以 option type 为唯一性维度，不比较文本值。
- 预处理与事务：`PrepareStmt`、`DeallocateStmt`、`Prepared`、`ExecuteStmt`、`BeginStmt`、`BinlogStmt`、`CompletionType`、`CommitStmt`、`RollbackStmt`、`SavepointStmt`、`ReleaseSavepointStmt`、`UseStmt`。
- SET 与敏感值：`VariableAssignment`、`SetStmt`、`SetConfigStmt`、`SetSessionStatesStmt`、`SetCharsetStmt`、`SetPwdStmt`。`VariableAssignment::restore` 仅对明确列举的六个 system embedding API key 打码；同名用户变量和未来未知变量不会被误打码。
- 账户与权限：`Identity`、`AuthOption`、`UserSpec`、TLS/token、资源、密码/锁和元数据选项，`CreateUserStmt`、`AlterUserStmt`、`DropUserStmt`、角色设置/授予/撤销类型，以及 `PrivElem`、`RoleOrPriv`、`GrantLevel`、`GrantStmt`、`RevokeStmt`、`GrantProxyStmt`。私有 `restore_account_options` 固定按用户、REQUIRE、WITH resource、密码/锁、metadata、resource group 的顺序拼接。
- 运维和诊断：`AdminStmt` 及其 `AdminStmtType`，`TrafficStmt`，`CompactTableStmt`，`FlushStmt`，`KillStmt`，`TraceStmt`，`ExplainForStmt`，`ExplainStmt`，`ShutdownStmt`、`RestartStmt`、`HelpStmt`。
- 备份与资源：`BrieKind`/`BRIEKind`、`BRIEOption`、`BRIEStmt`、`ImportIntoActionStmt`、`CancelDistributionJobStmt`、`SetResourceGroupStmt`、`CalibrateResourceStmt`。`AuthTokenOrTLSOption`、`BDRRole` 等别名保持 Go 风格名称兼容。
- 横切契约：`SensitiveStatement::secure_sql` 由 `sensitive_statement!` 为八类敏感语句实现；`MiscVisitor`、`MiscVisitable` 与 `leaf_visitable!` 为列出的叶节点提供 enter/leave 调用。

## 执行流程

典型流程是“构造字段 → 选择语句分支 → 格式化子值 → 拼接 SQL”。例如 `AdminStmt::restore` 先预计算表名和 job id 列表，再按 `AdminStmtType` 选择语法；需要附属状态的分支读取 `ShowSlow`、`StatementScope`、`BdrRole`、范围或 ALTER JOB 选项，最后统一添加 `ADMIN ` 前缀。`BRIEStmt::restore` 先由 `BrieKind::as_str` 决定命令，再按备份/恢复、job、流备份和 metadata/purge 分支选择 `TO`、`FROM` 或 job id，末尾追加所有 `BRIEOption`。

敏感输出不直接修改原节点。`TrafficStmt::secure_text` 和 `BRIEStmt::secure_text` 克隆节点，调用 `redact_url` 处理存储地址；Traffic 还把 Password 选项替换成 `xxxxxx`，之后复用正常 `restore`。`UserSpec::security_string` 只暴露身份和双密码选项，将真实认证串写成 `password = ***`；`GrantStmt`/`GrantRoleStmt` 从原始文本中截断首个大小写不敏感的 `identified` 后缀。

`redact_url` 的流程是：用 `url::Url::parse` 解析；无法解析或 scheme 不在 s3/ks3/oss/azure/azblob 白名单时原样返回；把查询参数收集进 `BTreeMap<String, Vec<String>>`，以大小写不敏感且下划线等价连字符的键名匹配敏感项；敏感键的全部值折叠为单个 `xxxxxx`；最后按键排序重新序列化。这解释了输出参数顺序可能变化，以及重复敏感键只保留一个遮罩值。

`TableOptimizerHint::restore` 先标准化提示名和查询块，再按提示名/`HintData` 组合解释载荷。索引提示要求至少一张表；LEADING 递归还原；内存字节数换算为 MB；布尔、时间范围、SET_VAR、存储引擎和普通表列表分别走专用格式；不支持的数据组合返回错误。

访问流程由 `MiscVisitable::accept` 驱动。`leaf_visitable!` 的实现总是调用一次 `enter` 和一次 `leave`，记录 `enter` 的返回值但因为这些实现被视为叶节点，没有可跳过的子节点。这不是 `lib.rs` 主访问器对嵌套表达式/表节点的递归替代。

## 数据与状态

所有节点都只持有拥有所有权的 `String`、数值、布尔、`Option`、`Vec` 或其他值节点；没有全局可变状态。大多数类型派生 `Clone`、`Debug`、`Eq`、`PartialEq`，许多还派生 `Default`，因此调用者可以先用默认值构造再覆盖相关字段。多个 enum 的默认值代表“未指定”或常用分支，例如 `CompletionType::Default`、`StatementScope::None`、`BdrRole::None`、`HintData::None`。

字段之间存在语义不变量但主要由构造者/语法动作保证：`PrepareStmt` 的 `sql_text` 与 `sql_var` 至少有一个；索引提示至少有一张表；`AdminStmt::ShowSlow` 需要 `show_slow`；Flush plan cache 需要非 None scope；设置 BDR role 只接受 Primary/Secondary；`RoleOrPriv` 不能同时被当成角色和权限。模块在还原阶段对其中一部分不变量返回 `Err(String)`，其余空集合会生成最小或不完整文本，因此直接构造节点时应同步验证。

`BTreeMap` 只在一次 `redact_url` 调用内存在，用于稳定排序查询参数；克隆式安全输出产生与节点大小成正比的临时副本。`LeadingList` 是递归拥有的树，`flatten` 输出 `HintTable` 克隆列表。

## 依赖与调用关系

- 模块入口：`pkg/parser/ast/lib.rs -> pub mod misc`；Cargo 入口为 `pkg/parser/ast/Cargo.toml -> [lib] path = "lib.rs"`。
- 内部公共辅助链：`SetStmt::restore -> VariableAssignment::restore -> redact_url`（仅 `TIDB_CLOUD_STORAGE_URI` 分支）；`TrafficStmt::secure_text -> redact_url -> TrafficStmt::restore`；`BRIEStmt::secure_text -> redact_url -> BRIEStmt::restore`。
- 账户链：`CreateUserStmt::restore` 与 `AlterUserStmt::restore -> restore_account_options -> UserSpec/AuthTokenOrTlsOption/ResourceOption/PasswordOrLockOption/...::restore`。
- 权限链：`GrantStmt::restore` 与 `RevokeStmt::restore -> restore_privileges -> PrivElem::restore`，并复用 `GrantLevel::restore`、`UserSpec::restore`。
- 提示链：`TableOptimizerHint::restore -> HintTable::restore`，LEADING 分支继续进入 `LeadingList::restore_with_qb -> restore_inner`；`flatten -> flatten_into` 深度优先递归。
- 生产调用者：上述五个模块直接调用 `redact_url` 来清理系统变量、global variables、DDL S3 日志、对象存储 URL 和 import-into 调度信息；DDL BDR 模块直接复用角色枚举。未从代码搜索确认其他轻量 AST 节点被生产路径直接构造，当前主要证据来自独立测试和 Go 对照，不能据此声称它们已接入完整 parser 执行链。
- RustCodeGraph 对目标文件报告“used by 79 files”，但对常见 `restore`/`is_empty` 等名称的边存在跨文件同名污染；因此本文只把精确路径搜索确认的边列为生产调用证据，不把模糊同名边当作事实。

## 错误处理与边界

返回 `Result<String, String>` 的主要失败点包括：`PrepareStmt::restore` 没有 SQL 文本或变量；`DualPasswordOptionType::None::restore` 不可单独还原；账户还原向上传播用户规格错误；`AdminStmt` 缺 `ShowSlow`、plan cache scope 或使用 None/Unknown BDR role；`FlushStmtType::None`；空权限类型；`RoleOrPriv` 的角色/权限形态冲突；索引提示缺表或 hint data 组合不支持。上层必须传播或转换这些字符串错误，不能用 `unwrap` 处理不可信构造输入。

大量 `restore` 返回普通 `String`，意味着它们假设字段已由语法层校验。例如 QUERY WATCH 选项的互斥只由 `check_query_watch_append` 提供，`AddQueryWatchStmt::restore` 本身不会拒绝重复类型；`ImportIntoActionTp` 目前只有 Cancel；空 identity、table 或表达式也可能被原样格式化。

安全边界是 scheme 和字段白名单而非任意字符串扫描：未知 scheme、无效 URL 原样返回；S3 的 `endpoint` 不打码，而 Azure 的 `endpoint` 打码；查询键会做大小写和 `_`/`-` 归一化。新增凭据参数时若不更新 `redact_url` 白名单，会造成日志泄露风险。`GrantStmt` 的文本截断依赖单词 `identified` 的首次出现，若原文本结构发生变化应重新评估。

`quote_string` 是本模块的轻量 SQL 转义逻辑，不携带 charset、collation 或 `NO_BACKSLASH_ESCAPES` 上下文；Go 主 AST 使用 `format.RestoreCtx`，两者在复杂字面量上不可默认视为完全等价。

## 并发与资源生命周期

本文件没有锁、原子变量、channel、异步任务、线程、事务句柄、网络连接或文件句柄。节点为普通拥有值，`restore` 只读借用 `&self`；可否跨线程共享由字段的自动 `Send`/`Sync` 属性决定，但模块没有并发调度语义。

资源生命周期局限于函数栈和返回值：SQL 拼接创建临时 `String`/`Vec`；`redact_url` 临时拥有解析后的 URL、参数 map 和 serializer；安全文本方法的克隆在返回后释放；visitor 接受调用者借用的可变节点和 visitor，不保存引用。性能风险主要来自长列表反复 `collect/join`、递归 LEADING 深度以及安全输出整节点克隆，不存在后台清理要求。

## 与 Go 版本的对应关系

直接对照文件为 [`misc.go`](./misc.go)，行为测试对照为 [`misc_test.go`](./misc_test.go)。Rust 保留了许多 Go 名称和分支顺序，并通过 `BRIEKind`、`BDRRole`、`AuthTokenOrTLSOption` 等别名兼容 Go 缩写风格。`misc_test.rs` 复核提示、BRIE、PLAN REPLAYER、URL、QUERY WATCH、Traffic 和 SET PASSWORD 的代表性输出；`misc_6_aster_unit_test.rs` 进一步覆盖账户/权限、事务、ADMIN、visitor 与错误敏感契约；`go_merge_16_test.rs` 覆盖 embedding key 和 Azure endpoint 的后续合并语义。

Rust 版本不是 Go 文件的逐类型完整镜像。Go `misc.go` 还包含 materialized view 等节点，主 Rust `lib.rs` 也存在若干同名、实现完整 `Node` 的类型；本文件则用字符串字段替代许多 Go 的 `ExprNode`、`TableName`、`StmtNode` 和 `RestoreCtx`，并以 `MiscVisitor` 把列出的节点当作叶节点。Go visitor 能递归进入子表达式并替换节点，当前轻量 visitor 不能。

还原细节也可能因抽象层不同而不同：Go 的 `RestoreCtx` 控制关键字、名称、字符串和 charset 前缀；Rust 本文件用本地字符串函数。现有测试证明的是列出的代表性兼容行为，而不是所有 Go AST 行为已经迁移。尤其应避免把 `misc.rs` 的同名 `AdminStmt`、`DoStmt`、`ExplainStmt` 等与 `lib.rs` 主 AST 类型混用。

## 扩展指南

新增杂项语句前先检查 Go `misc.go`、Rust `lib.rs` 和 parser actions 是否已有规范节点。若功能必须由语法解析产生，应优先接入主 `Node`/`Visitor` 类型面；若同时维护轻量兼容 API，再在 `misc.rs` 增加对应数据和 `restore`，不要只新增一个无法从 parser 到达的孤立类型。

新增/修改 `restore` 时：复用 `quote_name`/`quote_string`；把字段组合不变量转换为明确 `Result` 错误；按 Go 分支优先级和空值行为逐项对照；为新增类型决定是否加入 `leaf_visitable!`。若节点实际包含可遍历子节点，则不应继续套用叶节点宏，而应编写显式 `MiscVisitable` 或使用主 AST visitor。

涉及密码、token、URI 或原始 SQL 的节点必须实现安全文本，并评估是否加入 `SensitiveStatement` 宏。扩展对象存储 scheme/参数时同步更新 `redact_url` 和至少三类用例：应打码、应保留、无效 URL；同时检查重复键、大小写、下划线/连字符与参数排序。

测试逻辑保持在独立文件中：主要同步 [`misc_test.rs`](./misc_test.rs)，Go 语义变化对照 [`misc_test.go`](./misc_test.go)；迁移差异/补充分支可放 `misc_6_aster_unit_test.rs` 或相关 merge 测试。重点覆盖成功输出、所有错误分支、敏感信息不泄露、visitor enter/leave 次数以及 Go/Rust 差异。修改 Rust 源码时还应按仓库规则更新顶部 `// Copyright 2026 AsterSQL.` 并运行 `cargo fmt --all`；本次纯文档任务未修改源码。

兼容风险集中在外部可观察 SQL 文本、参数排序和错误文本；安全风险集中在漏打码；性能风险集中在长列表分配、递归列表和 clone。任何新分支都应确认这些三类风险，并检查已确认的 `redact_url` 生产调用者。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 files、307,296 nodes、1,848,419 edges；`node --file pkg/parser/ast/misc.rs` 分段读取了 1–3139 行；`query` 精确确认了 `redact_url`、`check_query_watch_append`、`TableOptimizerHint`、`SensitiveStatement`、`MiscVisitable`、`PlanReplayerStmt`、`VariableAssignment`、`BRIEStmt`、`TrafficStmt` 和 `AdminStmt`。调用图对常见同名方法有歧义，故生产调用关系又用精确路径搜索复核。
- 源与边界：读过 `pkg/parser/ast/misc.rs`、`pkg/parser/ast/Cargo.toml`、`pkg/parser/ast/lib.rs`；该包没有 `doc.go`，因此最近的 crate 契约来自 `lib.rs` 模块入口与 trait 定义。
- Go 对照：读过 `pkg/parser/ast/misc.go` 的符号与相关实现，以及 `pkg/parser/ast/misc_test.go` 的 visitor、敏感语句、hint、BRIE、PLAN REPLAYER、URL、QUERY WATCH、Traffic、embedding key 和 SET PASSWORD 用例。
- Rust 测试：读过 `pkg/parser/ast/misc_test.rs`、`pkg/parser/ast/misc_6_aster_unit_test.rs`、`pkg/parser/ast/go_merge_16_test.rs` 的相关断言。它们验证了深度优先 LEADING、分支优先级、账户选项顺序、错误敏感文本、URL scheme/key 规则及 leaf visitor 的 enter/leave 行为。
- 结构验收使用任务指定命令，要求目标文件存在且恰有本文这 11 个固定二级标题。人工复核重点是：文件存在原因、主要还原/脱敏流程、接线边界、安全扩展位置和独立测试路径均有直接代码依据；未运行 Cargo，符合本任务约束。
