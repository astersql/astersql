# `pkg/session/dml_runtime.rs` 逻辑说明

## 文件定位

`pkg/session/dml_runtime.rs` 属于 `astersql-session` crate；模块由 `pkg/session/lib.rs` 以 `pub mod dml_runtime` 公开。它位于解析器 AST 与具体会话写入运行时之间，负责把 `InsertStmt`、`UpdateStmt`、`DeleteStmt` 整理为会话层计划，并提供关系表写入所需的一组轻量表达式求值工具。真正的事务开启、锁、KV 编解码、外键/索引维护与提交不在本文件，而在 `pkg/session/runtime/control.rs`、`pkg/session/runtime/dml.rs` 等运行时模块。

文件同时支持两条路径：固定演示表 `aster_session_kv` 使用严格的 `(k, v)` 快路径；其它表生成携带完整 AST、排序与限制信息的关系表计划，再交给完整关系表运行时。因此它不是 Go TiDB 完整 planner/executor 的替代品，也不是独立执行器。

`pkg/session/Cargo.toml` 声明 crate 名为 `astersql-session`，`lib.rs` 为入口，并以 `package.metadata.porting.go-package = "pkg/session"` 标记 Go 移植归属。此文件直接使用工作区 parser/AST/MySQL 常量、meta model、types JSON，以及外部 `chrono`、`regex`、`rust_decimal`；`nextgen` feature 不改变本文件的条件编译，本文件本身没有 `cfg` 项。

## 核心职责

1. 将单表 INSERT/REPLACE、UPDATE、DELETE AST 规范化为 `InsertPlan`、`UpdatePlan`、`DeletePlan`，保留关系表执行所需的 schema、谓词 AST、赋值、ORDER BY、LIMIT 和冲突策略（`PlanInsert`、`PlanUpdate`、`PlanDelete`）。
2. 为 `aster_session_kv` 强制更窄的契约：INSERT 只接受两列 `(k,v)` 与字面量 VALUES；UPDATE/DELETE 只接受单个比较谓词，且执行层进一步要求 `WHERE k = literal`（`SESSION_KV_TABLE`、`comparison_keys`，以及 `runtime/control.rs::execute_update/execute_delete`）。
3. 求值关系表写入中的有限 AST 子集，包括字面量、列引用、算术/比较/三值逻辑、CASE、VALUES、部分标量/JSON/时间函数与 DECIMAL CAST（`Literal`、`EvalExpr`、`EvalExprWithBitColumns`、`CaseResult`）。
4. 将元数据中的生成列表达式文本重新解析为 AST（`ParseGeneratedExpr`），供生成列、条件索引、行编解码和系统会话写入复用。
5. 提供计划报告和辅助状态的数据结构：`DmlExecutionReport` 承载 EXPLAIN ANALYZE 所需统计；`RelationalTableState` 实现一个内存自增水位模型，但当前代码搜索仅发现基准测试调用，未接入生产关系表执行。

## 主要符号

- `SESSION_KV_TABLE`：私有常量，值为 `aster_session_kv`，决定规划时采用固定 KV 快路径还是关系表路径。
- `RelationalTableState { Info, NextAutoID }`：公开、可克隆的表元信息和下一 ID 容器。`New` 从 `max(AutoIncID, AutoIncIDExtra, 1)` 建立水位；`AllocateAutoID` 对显式值执行 rebase，对隐式值执行 checked increment，并同步 `Info.AutoIncID`。生产调用未检出；`pkg/session/bench_test.rs` 验证 1、显式 20、随后 21 的序列。
- `InsertPlan`：保存表名、仅供 KV 快路径使用的字符串行、`Replace`、`Ignore` 和 `OnDuplicate` AST 赋值。关系表行不会在 `PlanInsert` 中预求值，`Rows` 为空并由完整执行器处理。
- `UpdatePlan` / `DeletePlan`：保存显式 schema、表名、兼容 KV 快路径的首个谓词三元组、全部简单谓词、规范 WHERE AST、以及 ORDER/LIMIT；UPDATE 另含赋值 AST。
- `DmlExecutionReport`：记录算子/表、影响行数、insert ID、写键与 prewrite 键数、提交状态/等待时间、自增 alloc/rebase 次数和 INSERT 分阶段耗时。`runtime/session.rs` 把它作为 `last_dml_report`，`runtime/control.rs::explain_dml_record_set` 格式化输出。
- `Literal`：把 AST 值转换为字符串；NULL、未绑定参数和非字面量失败；字节/bit/hex 字面量必须是 UTF-8；支持括号和一元正负号。
- `PlanInsert` / `PlanUpdate` / `PlanDelete`：本文件三个公开规划入口。它们拒绝本模块无法表达的多表或 KV 特有形态，但关系表复杂 WHERE 会保留在 `Predicate` 中，不因简单谓词抽取失败而丢失。
- `EvaluateAssignment`：以 `k`、`v` 组成当前行调用 `EvalExpr`，并拒绝赋值结果为 NULL；只用于会话 KV UPDATE。
- `EvalExpr`：不带 BIT 元数据的公共包装；转发到 `EvalExprWithBitColumns(..., &[])`。
- `EvalExprWithBitColumns`：主要递归解释器。只有被表元数据标记为 BIT 的列在算术上下文中才把 `0x...` 按无符号大端整数解释，普通字符串不会被误判。
- `CaseResult`：先选择 CASE 分支但保留结果 AST 的原始类型，关系表 UPDATE 借此避免把 BLOB/hex 字面量过早转成 UTF-8 文本；未命中且无 ELSE 返回 `None`。
- `ParseGeneratedExpr`：用 `SELECT <expr>` 包装文本，以 `ModeNoBackslashEscapes` 解析并取第一个字段 AST。
- `MatchesPredicate`：用于退化的谓词三元组匹配；双方均可解析为 `i128` 时数值比较，否则字典序比较，`*` 仅在行缺少目标列时充当无过滤标记。

## 执行流程

INSERT 主链如下：会话解析得到 `InsertStmt` 后，`runtime/control.rs::execute_insert`（LOAD DATA 还会由 `runtime/load_data.rs` 进入）调用 `PlanInsert`。若有 SELECT，控制层先分流到 INSERT SELECT；若目标为 `aster_session_kv`，本文件验证列形状并把每行字面量变成 `(String, String)`，控制层再处理重复键的 REPLACE、IGNORE 或 ON DUPLICATE 分支，最后由 `apply_session_kv_mutations` 写入显式事务缓冲区或新建 KV 事务提交。其它表只在计划中保留冲突选项，实际行构造、类型转换、自增、生成列、索引、外键、锁和提交均由 `runtime/dml.rs::execute_relational_insert*` 完成。

UPDATE 主链为 `runtime/control.rs::execute_update -> PlanUpdate`。多物理源先由控制层分流到 join update；单表计划对 KV 表拒绝 ORDER/LIMIT、未知列与多个谓词，控制层又要求 `k = literal`，读取旧值、顺序求值赋值、检查键移动冲突后提交 mutation。关系表路径进入 `runtime/dml.rs::execute_relational_update`：完整 `Predicate` 优先通过关系表达式运行时判断，简单三元组只是后备；候选行再按 ORDER/LIMIT 选择、加锁、求值赋值与生成列、校验类型/索引/外键并编码写入。

DELETE 主链与 UPDATE 对称：`runtime/control.rs::execute_delete -> PlanDelete`；KV 路径只删除 `k = literal`，关系表路径由 `runtime/dml.rs::execute_relational_delete` 计算谓词、排序/限制候选、锁行、处理外键/物化日志并生成删除 mutation。

生成列表达式链从 `ParseGeneratedExpr` 开始。`runtime/dml.rs`、`runtime/relational_scan.rs`、`runtime/row_codec.rs`、`runtime/ddl.rs` 和 `runtime/system_session.rs` 将元数据文本转为 AST，再调用 `EvalExpr`/`EvalExprWithBitColumns`。递归求值遵循 NULL 传播；CASE 只求值被选择的结果；`VALUES(col)` 必须有 INSERT 入边行；无法覆盖的函数/节点返回 `unsupported relational DML expression`，部分含子查询或函数的关系表达式由 `runtime/dml.rs` 主动转给完整查询表达式路径。

## 数据与状态

计划结构拥有 AST 的 clone，不借用解析器对象，因此可安全跨越规划函数返回边界。行上下文统一为 `HashMap<String, Option<String>>`：键是规范化列名（限定列使用 `table.column`），`None` 表示 SQL NULL，`Some(String)` 是运行时文本表示。`incoming` 是 ON DUPLICATE 的待插入行，只供 `VALUES(col)` 使用；`bit_columns` 来自 `TableInfo`，防止按文本形状猜测类型。

数值运算优先保留整数精度：非除法先尝试 `i128` checked 运算，失败后使用 `rust_decimal::Decimal`；除零报错，而 `% 0`/`MOD(...,0)` 返回 NULL。比较在双方均能解析为 Decimal 时使用数值序，否则使用字符串序。逻辑 AND/OR 实现 SQL 三值逻辑。时间函数使用调用瞬间的 `Utc::now()`，并按 0 至 6 位精度格式化；它没有语句级固定时间缓存。

`DmlExecutionReport` 只是值对象，生命周期由 `ConcreteSession` 的 `last_dml_report` 管理。`RelationalTableState` 自身没有锁或原子性；它要求调用者持有可变引用。当前生产自增路径在 `runtime/dml.rs` 使用运行时 allocator，而不是该结构。

## 依赖与调用关系

上游直接调用边由 RustCodeGraph 与源码交叉确认：

- `PlanInsert <- runtime/control.rs::execute_insert`，以及 `runtime/load_data.rs::execute_load_data`；查询构造 VALUES 的路径也在 `runtime/query.rs` 使用它。
- `PlanUpdate <- runtime/control.rs::execute_update`；`PlanDelete <- runtime/control.rs::execute_delete`。
- `EvalExpr*` 被 `runtime/control.rs` 的 ON DUPLICATE/KV UPDATE、`runtime/dml.rs` 的关系表赋值与生成列、`runtime/relational_value.rs`、`runtime/row_codec.rs`、导入相关模块及系统会话使用。
- `ParseGeneratedExpr` 被 `runtime.rs::partition_expression_value`、`runtime/dml.rs`、`runtime/relational_scan.rs`、`runtime/row_codec.rs`、`runtime/ddl.rs`、`runtime/system_session.rs` 与 explain 辅助路径使用。
- `MatchesPredicate` 的生产调用位于 `runtime/dml.rs::execute_relational_update` 和 `execute_relational_delete` 的简单谓词后备路径。

主要下游依赖是 `astersql_parser`/`astersql_parser_ast`（解析和 AST）、`astersql_parser_mysql`（SQL mode/type 常量）、`astersql_meta_model::TableInfo`（元数据）、`astersql_types::json_functions`（二进制 JSON 解析、路径与 merge patch）、`chrono`（日期时间）、`rust_decimal`（精确十进制）与 `regex`。本文件通过 crate 根的 `SessionError`/`SessionResult` 统一返回错误，并在 `NULLIF` 中调用 `crate::runtime::relational_compare`，因此不是完全独立的表达式 crate。

## 错误处理与边界

所有可预期失败都转成 `SessionError`：缺表、JOIN/多表写、KV 表列形状错误、空 VALUES/赋值、未知列、未绑定参数、非 UTF-8 字面量、非法数字/日期/正则/JSON、参数个数错误、算术或时间溢出及未支持 AST。调用者普遍使用 `?` 保留错误上下文，事务回滚与可见性由运行时层负责。

需要特别注意的边界：

- 本文件不是通用 SQL 表达式引擎。未列出的函数、子查询和复杂节点可能由关系运行时接管，也可能明确失败；新增调用者不能假设任意 AST 可求值。
- KV 表 INSERT 不支持 INSERT SELECT；KV UPDATE/DELETE 不支持 ORDER/LIMIT、多谓词或非 `k = literal` 的执行。关系表则保留这些 AST 并在完整运行时实现。
- `Literal(NULL)` 报错，而 `EvalExpr(NULL)` 返回 `None`；二者适用场景不同。
- 限定列必须命中 `table.column` 键，不会退回同名裸列，以避免多表歧义。
- `ABS(i64::MIN)` 使用 `saturating_abs`，`REPEAT`/`LPAD`/substring 会检查负数和平台 `usize` 转换；`SLEEP` 会真实阻塞当前线程并拒绝负数或非有限值。
- `REGEXP_REPLACE` 当前只按前三个参数执行全局替换，虽然接受 3 至 6 个参数；额外参数未参与位置、次数或 match type 语义，扩展前必须对照 Go/MySQL 测试。
- `MatchesPredicate("*")` 的实现依赖缺列分支；对存在列传入 `*` 会落入“不支持运算符”错误，不能把它当一般恒真谓词使用。

## 并发与资源生命周期

本文件不创建异步任务、通道、锁或事务。计划和报告是拥有数据的普通值；表达式求值除 `Utc::now()`、`SLEEP` 与 JSON/regex 解析外没有持久副作用。`Regex::new`、生成列表达式 parser 和 JSON path 都是每次调用创建，热点路径扩展时需评估缓存，但缓存必须考虑 SQL mode、表达式文本与线程安全。

资源和原子性边界在调用层：`runtime/control.rs::apply_session_kv_mutations` 对显式事务写入 mem-buffer，自动提交则开始真实 `kv::Transaction`，任一 Set/Delete 失败回滚，提交失败不发布写入；关系表路径负责行锁、事务 overlay、外键级联与 mutation 提交。独立测试 `autocommit_failure_is_propagated_and_does_not_publish_kv_writes` 与 `explain_analyze_commit_failure_closes_terminal_and_preserves_rows` 验证失败写不可见以及会话仍可继续使用。

`RelationalTableState::AllocateAutoID` 只依赖 `&mut self`，没有跨会话同步保证；若未来接入共享表状态，必须由外层提供互斥/租约与持久化语义，不能直接把当前内存水位当作分布式 allocator。

## 与 Go 版本的对应关系

Go `pkg/session` 没有与本文件一一对应的 `dml_runtime.go`。Rust 将 Go 主链中分散在 planner、executor、expression、table/autoid 和事务上下文的部分语义聚合为具体会话运行时辅助层。因此应按行为而非文件名对照：

- INSERT 列初始化、默认值/生成列与行构造对应 `pkg/executor/insert_common.go`；重复键更新对应 `pkg/executor/insert.go` 和 `pkg/executor/write.go::updateRecord`。
- UPDATE 的“先求值普通列、检测变化、处理 on-update/generated、再做约束和写入”顺序以 `pkg/executor/write.go::updateRecord` 为 Go 依据；Rust 的完整实现落在 `runtime/dml.rs`，本文件只提供部分表达式求值与计划载体。
- DELETE 的逐行 handle 构造和删除/批处理语义可对照 `pkg/executor/delete.go`；Rust 的锁、选择和 mutation 仍由 `runtime/dml.rs` 负责。
- Go 使用完整 expression/type/collation 框架；本文件以 `String`/`Option<String>` 和有限 AST 子集实现兼容路径，因而在隐式转换、collation、warning 与函数覆盖上不能宣称完整等价。

`pkg/session/dml_runtime_test.rs` 以真实 SQL/KV 路径验证若干 Go 对齐语义：显式事务、INSERT IGNORE/ON DUPLICATE、整数溢出告警转换、NULL 时间主键零值、嵌套 JSON 生成列、自增 alloc/rebase 报告、外键阶段、BIT 算术以及 CASE 精确大整数/惰性分支。测试名中的 `follow_go_*` 是行为证据，但不代表未覆盖表达式自动等价于 Go。

## 扩展指南

新增 DML 形态时，先决定边界属于规划、轻量求值还是完整运行时。仅 AST 归一化或计划字段应修改 `Plan*` 与对应结构；涉及扫描、锁、事务、索引、外键或类型系统的行为必须接入 `runtime/dml.rs`/`runtime/control.rs`，不要堆进本文件。新增计划字段后同步全部构造点与 `pkg/session/dml_runtime_test.rs` 的真实 SQL 用例。

新增表达式节点或函数应扩展 `EvalExprWithBitColumns`，同时明确参数数目、NULL 传播、类型/字符边界、溢出和 Go/MySQL 差异。需要保留字节或精确整数类型的 CASE 场景应继续经 `CaseResult`，不要先转字符串。BIT 行为必须由 `TableInfo` 提供列名单；生成列表达式解析行为则应同步 `ParseGeneratedExpr` 及 `runtime/row_codec.rs`、`runtime/dml.rs` 的使用点。

最小测试面是独立文件 `pkg/session/dml_runtime_test.rs`，而不是把测试嵌入生产源文件。规划边界应覆盖 KV 与关系表各一例；表达式扩展应覆盖 NULL、非法参数、极值与字符/字节差异；事务行为应通过真实会话验证失败前后可见性。涉及 Go 行为对齐时还应定位相应的 `pkg/executor/*_test.go` 或 expression 测试，而不是从单个 Go 实现注释推导兼容性。

性能风险集中在递归求值和每行重复解析/编译：生成表达式 parser、Regex、JSON 文档/path 以及 HashMap 字符串复制都可能进入逐行热点。若引入缓存，需要保持语句时间函数、SQL mode、元数据版本和事务隔离边界正确。兼容风险集中在字符串化类型模型、collation、warning/strict mode 与错误文本；修改前应先增加回归测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；使用 `node --file pkg/session/dml_runtime.rs` 阅读全文件，并对 `PlanInsert`、`PlanUpdate`、`PlanDelete`、`EvalExprWithBitColumns`、`MatchesPredicate`、`ParseGeneratedExpr` 查询定义、callees 与 called-by trail。图确认 `PlanInsert <- execute_insert/execute_load_data`、`PlanUpdate <- execute_update`、`PlanDelete <- execute_delete`，以及 `MatchesPredicate` 的两个 `runtime/dml.rs` 调用点。批量 `callers` 命令未在超时窗口内返回文本，因此又以 node trail 和限定 `rg` 核对调用边。
- 源与 crate 边界：`pkg/session/dml_runtime.rs`、`pkg/session/lib.rs`、`pkg/session/Cargo.toml`。
- 直接运行时证据：`pkg/session/runtime/control.rs`、`pkg/session/runtime/dml.rs`、`pkg/session/runtime/relational_value.rs`、`pkg/session/runtime/row_codec.rs`、`pkg/session/runtime/load_data.rs`。
- 独立 Rust 测试：`pkg/session/dml_runtime_test.rs`；另以 `pkg/session/bench_test.rs` 核对 `RelationalTableState::AllocateAutoID`，以 `pkg/session/test/temporarytabletest/temporary_table_test.rs` 核对公开计划/谓词辅助函数的外部使用。
- Go 对照：`pkg/executor/insert_common.go`、`pkg/executor/insert.go`、`pkg/executor/write.go`、`pkg/executor/update.go`、`pkg/executor/delete.go`，以及相邻 Go executor 测试。仓库中不存在同名 Go 文件，因此未声称逐函数一一移植。
- 人工复核结论：该文件存在的原因是给具体 Rust 会话运行时提供 AST DML 计划和有限的写入表达式语义；事务与完整关系执行明确位于调用层；安全扩展必须同时维护独立测试、完整运行时边界以及 Go/MySQL 兼容证据。
