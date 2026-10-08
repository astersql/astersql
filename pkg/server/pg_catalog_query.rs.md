# `pkg/server/pg_catalog_query.rs`

## 文件定位

`pg_catalog_query.rs` 是 `astersql-server` crate 内部的 PostgreSQL catalog 查询前端。它由根模块 [`pkg/server/lib.rs`](lib.rs) 以私有模块 `pg_catalog_query` 装配，不对 crate 外公开；[`pkg/server/Cargo.toml`](Cargo.toml) 将该 crate 定义为 `astersql-server`，入口为 `lib.rs`，并通过 `package.metadata.porting.go-package = "pkg/server"` 说明包级移植归属。

该文件处在 PostgreSQL 协议接入与 catalog 执行器之间：它只负责识别 catalog SQL、词法分析并构造一个有意受限的 AST，不读取 catalog 行，也不执行查询。AST 随后由 [`pkg/server/pg_catalog.rs`](pg_catalog.rs) 中的 `CatalogQuery` 完成参数、名称和类型绑定并执行。文件级注释“Parsing builds expressions, never freezes catalog rows at Parse time”对应这一边界，因此 Parse 阶段不会把当时的模式快照固化进语句。

## 核心职责

- `lex` 把 SQL 转为 `Token`，处理大小写折叠、引号转义、嵌套块注释、行注释、`::`、整数和 PostgreSQL `$n` 参数，并尽早产生 SQLSTATE 风格错误。
- `parse` / `parse_shadowed` 判断语句是否归本模块处理。只有显式 `pg_catalog.<relation>`、未被用户表遮蔽的隐式 catalog 关系，或两个无 `FROM` 的原生 PG 探针函数会进入 catalog 解析；普通引擎 SQL返回 `Ok(None)`。
- `Parser` 递归下降解析受限的只读 `SELECT`：投影、关系、有限 JOIN、WHERE、ORDER BY、LIMIT、非递归 CTE、UNION、子查询、cast、函数调用和一组表达式操作，并构造 `Select` / `Expr` 树。
- 解析器显式限制复杂度和功能面，避免客户端 introspection SQL把任意 PostgreSQL 语法带入这个专用执行器。它区分语法错误 `42601`、不支持能力 `0A000`、参数错误 `42P02`、关系不存在 `42P01` 和重复 CTE `42712`。
- `implicit_relations` 为 session 的 `search_path`/遮蔽判断提供轻量扫描结果；真正解析仍由 `parse_shadowed` 完成。

## 主要符号

- `ParseResult<T> = Result<T, (&'static str, String)>`：模块统一的 SQLSTATE 与消息载体；`syntax` 固定生成 `42601`，`unsupported` 固定生成 `0A000`。
- `Token`：词法单元，包括未加引号/加引号标识符、字符串、整数、零基参数下标、单字符符号与 `::` cast。未加引号单词在 `lex` 中转为小写，加引号名称保留原值。
- `Expr`：catalog 表达式 IR，涵盖列、常量、参数、调用、cast、算术/拼接、数组下标、比较、`IN`/`ANY`、子查询、`array_agg`、三值逻辑相关节点、`CASE` 及数组 `unnest` 投影。
- `CompareOp` 与 `CastType`：把支持的比较运算和 cast 目标闭合为枚举。对象名 cast（如 `regtype`）必须原子地继续转成 `varchar`/`text`，避免暴露文件未建模的独立 wire type。
- `Projection`、`Relation`、`TableFunction`、`Join`、`Ordering`、`Cte`、`Select`：组成查询 AST。`Relation::cte_id` 用稳定编号区分同名嵌套 CTE，`Select` 保存 CTE、UNION、投影、FROM/JOIN、过滤、排序和限制。
- `Parser`：单次解析的可变状态，`pos` 是 token 游标；`depth`、`casts`、`predicates`、`query_depth` 是复杂度预算；`next_cte` 与 `cte_scope` 管理词法 CTE 作用域；`shadowed` 记录优先于隐式 catalog 名称的用户关系。
- `lex`：公开到 crate 的词法入口，也被 [`pkg/server/pg_session.rs`](pg_session.rs) 复用来解析 session 语句。
- `parse`：无遮蔽上下文的便捷入口；委托 `parse_shadowed(sql, &[])`。
- `parse_shadowed`：所有权判断与完整解析入口；返回 `None` 表示应继续走原生 SQL 引擎，返回 `Some(Select)` 表示 catalog 查询，返回 `Err` 表示已归属 catalog 但语法或能力不被接受。
- `is_catalog_relation`：识别 `pg_user`、`pg_opclass`、`pg_locks` 及 [`pkg/server/pg_oid.rs`](pg_oid.rs) 的 `SYSTEM_RELATIONS`。
- `implicit_positions` / `implicit_relations`：按嵌套括号维护 FROM 上下文，只提取未限定 catalog 关系，避免把投影逗号、字符串和注释误当作关系引用。

## 执行流程

1. 上游 `CatalogQuery::parse` 或 `CatalogQuery::parse_session_with_types` 传入原始 SQL；session 版本先调用 `implicit_relations`，结合当前数据库模式与 `search_path` 计算 `shadowed` 名称。
2. `parse_shadowed` 调用 `lex`。词法失败时，仅当原 SQL包含 `pg_catalog.` 才把错误归给 catalog；否则返回 `None`，保留普通引擎对非 catalog SQL 的所有权。
3. 它在 token 上识别三类 catalog 所有权：显式限定关系/已知对象、未被 `shadowed` 排除的隐式关系、`pg_catalog.pg_is_in_recovery()` 或 `pg_catalog.txid_current()` 标量探针。均不命中时返回 `None`；命中后只允许首词为 `SELECT` 或 `WITH`。
4. `Parser::query` 建立查询深度与 CTE 作用域边界，`query_inner` 先解析非递归 CTE，再解析主 `SELECT` 和最多 16 个 UNION arm；最终把 CTE定义挂到主 `Select`。
5. `Parser::select` 按投影、FROM、JOIN、WHERE、ORDER BY、LIMIT 顺序构造 AST。没有 FROM 时，仅允许上述原生 PG 探针，并以内部关系 `__pg_scalar` 表示单行标量来源。
6. 表达式按 `atom/cast`、余数、减法、拼接、比较、`NOT`、`AND`、`OR` 的优先级递归构造。二字段行相等被展开为两个 `Equal` 的 `And`；`NOT IN` 被表示为 `Not(In(...))`。
7. `relation` 解析 catalog 关系、CTE 或受支持的表函数。未限定名称先查最近的 CTE，再检查 `shadowed`，最后由 `is_catalog_relation` 验证；显式 `pg_catalog` 永远指向 provider 而不是同名 CTE。
8. 主查询完成后只容许一个可选分号；剩余 token 被视为不支持的 clause 或多语句。成功 AST回到 `CatalogQuery`，后者负责绑定、求值及协议结果生成，而不是本文件。

## 数据与状态

所有 AST 节点都拥有自己的 `String`、`Vec` 和 `Box` 数据，并实现适合测试与后续阶段使用的 `Clone`、`Debug`、`PartialEq`、`Eq`。本文件没有全局可变状态；唯一跨函数的解析状态位于栈上的单个 `Parser`。

关键不变量与预算如下：表达式嵌套最大 64 层；单条 cast 链最大 64、整条查询累计 cast 最大 128；谓词/部分运算累计最大 128；查询嵌套最大 8 层；CTE 总数最大 16；UNION arm 最大 16；每个 SELECT最多 8 个 JOIN；`CASE` 最多 32 个 WHEN；常量 `IN` 列表最多 128 项。CTE ID由 `next_cte` 单调分配，离开子查询时 `cte_scope` 截断到进入前长度，但 ID不回退，从而避免嵌套同名定义混淆。

参数 token 把 PostgreSQL 的一基 `$1..$32767` 转成零基 `Parameter(0..32766)`。`Select` 中保存的是表达式与关系结构，不保存 schema snapshot 或结果行；这一点让 prepared statement 在执行时仍读取当前权威元数据。

## 依赖与调用关系

直接下游很窄：除 Rust 标准库集合/转换能力外，仅通过 `crate::pg_oid::SYSTEM_RELATIONS` 查询已知系统关系。文件本身不依赖外部 crate；[`pkg/server/Cargo.toml`](Cargo.toml) 的依赖属于整个 server crate，而非此解析器的直接调用面。

主要上游调用边为：

- [`pkg/server/pg_catalog.rs`](pg_catalog.rs) 的 `CatalogQuery::parse` → `pg_catalog_query::parse` → `parse_shadowed`。
- `CatalogQuery::parse_session_with_types` → `implicit_relations`，查询当前 schema 中的遮蔽表后 → `parse_shadowed`。
- [`pkg/server/pg_conn.rs`](pg_conn.rs) 的简单查询循环 → `CatalogQuery::parse_session`；识别成功后直接走 catalog 执行路径，否则继续名称适配/原生引擎路径。
- [`pkg/server/pg_extended.rs`](pg_extended.rs) 的 Parse 消息处理 → `CatalogQuery::parse_session_with_types`；该文件还消费 `Expr` 来保存文本/二进制绑定参数。
- [`pkg/server/pg_session.rs`](pg_session.rs) → `lex`，复用一致的 token 规则识别 PostgreSQL session 语句。

RustCodeGraph 对 `pg_catalog_query.rs::parse_shadowed` 的节点记录确认其调用 `lex`、`implicit_positions`、`Parser::query`，调用方为本文件 `parse` 和 `pkg/server/pg_catalog.rs:147`；对 `parse` 的节点记录还显示独立测试 `catalog_select_structure`、`catalog_projection_syntax_and_boundaries`、`catalog_datagrip_scalar_and_lateral_bounds` 等调用者。

## 错误处理与边界

错误是协议可映射的 `(SQLSTATE, message)`，不 panic 作为用户输入处理策略。`42601` 用于未闭合注释/引号、缺失 token、空 IN、LIMIT 格式等语法问题；`0A000` 用于已识别 catalog 查询中的越界复杂度、不支持运算/cast/clause、混合 public/catalog 关系、递归 CTE、多语句等能力边界；`42P02` 用于非法或越界 `$n`；未知 catalog 关系返回 `42P01`；重复 CTE 名返回 `42712`。

归属判断是重要边界：字符串或注释里的 `pg_catalog` 不会劫持普通 SQL；普通 SQL 的词法错误通常交还原生引擎；但显式包含 `pg_catalog.` 的词法错误由本模块报告。隐式名称如果被当前 public schema 的同名表遮蔽，则返回 `None` 让原生引擎处理；显式限定名称不受该遮蔽影响。

该语法不是通用 PostgreSQL parser。关系函数仅允许 `unnest` 与 `pg_indexam_has_property` 的有限形式；标量运算、cast、JOIN、CTE、UNION 也都是白名单。解析器可接受 `SELECT *` 为 AST，但完整 `CatalogQuery::bind` 仍可拒绝它，说明“可解析”不等于“可执行”。扩展时必须同时核对绑定/求值层，不能只放宽语法。

## 并发与资源生命周期

解析完全同步、无锁、无通道、无任务、无网络或磁盘 I/O。每次调用新建 token `Vec` 和 `Parser`，因此多个连接可并发解析而不共享解析器状态。递归深度和各类计数预算同时限制调用栈与 AST/集合增长；所有资源随返回的 AST 或局部变量按 Rust 所有权释放。

解析结果可能存入 [`pkg/server/pg_extended.rs`](pg_extended.rs) 的 `Statement`/`Portal`，但其 prepared 生命周期由扩展协议状态机管理；本文件不持有 portal、连接或 schema snapshot。实际 catalog 数据在执行阶段读取，所以 parse 与 execute 之间的 DDL/重命名可以被后续执行观察到，相关实时协议测试也验证了重命名、跨连接和服务重启后的元数据行为。

## 与 Go 版本的对应关系

[`pkg/server/Cargo.toml`](Cargo.toml) 只提供包级映射 `go-package = "pkg/server"`。当前仓库的 `pkg/server` 下没有 `doc.go` 或与 `pg_catalog_query.rs` 同路径/同名的 Go 文件；对全仓库 Go 文件搜索 `pg_catalog`、`CatalogQuery`、`implicit_relations`、`parse_shadowed` 也未找到直接实现。因此，本文件是 AsterSQL Rust PostgreSQL 兼容层的专用实现，没有可逐函数对照的 TiDB Go 版本。

可对齐的是外部语义而非源码结构：普通 TiDB SQL必须继续由原生引擎拥有，catalog introspection 查询才进入该白名单解析/执行链；SQLSTATE、search path 遮蔽、prepared 参数和协议可见类型由 Rust 侧独立测试约束。后续修改不得凭 Go 包级映射推断存在同名逻辑，应以本文件、`pg_catalog.rs` 及独立 Rust 测试为事实来源。

## 扩展指南

- 新增 token 或词法形式时修改 `Token`/`lex`，并在独立的 [`pkg/server/pg_catalog_query_test.rs`](pg_catalog_query_test.rs) 增加引号、注释、非法输入与“普通 SQL不被劫持”回归；不要把测试写进生产源文件。
- 新增表达式/运算/cast 时同步修改 `Expr` 或 `CastType`、`Parser::expr_inner`/优先级函数，并检查 `pg_catalog.rs` 中的绑定、类型推导、求值、排序和协议 OID处理。只新增 AST 节点而没有下游语义会把错误延迟到执行阶段。
- 新增 catalog 关系时优先更新 `pg_oid::SYSTEM_RELATIONS` 的权威集合，或有明确理由再扩展 `is_catalog_relation`；同时验证显式限定、隐式 search path、用户表遮蔽、CTE同名和未知关系 SQLSTATE。
- 新增查询 clause、JOIN 或表函数时修改 `Parser::select`/`relation`，保持复杂度预算并评估笛卡尔积、AST内存和 catalog 执行成本。必要时在 `pg_catalog.rs` 增加相应执行支持后才宣称功能可用。
- 修改所有权启发式时同时审查 `parse_shadowed` 与 `implicit_positions`，确保字符串/注释、嵌套 FROM、逗号 JOIN、显式 `pg_catalog` 和 `shadowed` 的优先级不回归。
- 回归至少扩展 `pg_catalog_query_test.rs` 的结构/边界测试；若影响绑定与执行，再同步 `pg_catalog_test.rs`；若影响简单或扩展协议路由，再同步 `pg_conn_test.rs`、`pg_extended_test.rs` 或 `pg_client_integration_test.rs`。兼容风险主要是误接管普通 SQL、改变 SQLSTATE/列类型；性能风险主要是放宽预算后带来的深递归、AST膨胀或高代价 catalog JOIN。

## 验证依据

- 生产源码：[`pkg/server/pg_catalog_query.rs`](pg_catalog_query.rs)；重点符号为 `Token`、`Expr`、`Select`、`Parser`、`lex`、`parse`、`parse_shadowed`、`implicit_relations` 与 `is_catalog_relation`。
- crate 与装配：[`pkg/server/Cargo.toml`](Cargo.toml)、[`pkg/server/lib.rs`](lib.rs)；确认 crate 名、入口、Go 包级元数据、私有模块及独立测试模块。
- 直接调用链：[`pkg/server/pg_catalog.rs`](pg_catalog.rs)、[`pkg/server/pg_conn.rs`](pg_conn.rs)、[`pkg/server/pg_extended.rs`](pg_extended.rs)、[`pkg/server/pg_session.rs`](pg_session.rs)、[`pkg/server/pg_oid.rs`](pg_oid.rs)。
- 独立测试：[`pkg/server/pg_catalog_query_test.rs`](pg_catalog_query_test.rs) 验证 AST形状、注释/引号归属、SQLSTATE、表达式/cast/查询/CTE预算、参数节点、标量探针、表函数及 live OID行为；更完整执行语义由 `pg_catalog_test.rs`、协议路由由 `pg_conn_test.rs`/`pg_extended_test.rs` 覆盖。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点；`query parse_shadowed`、`query implicit_relations`、`node pg_catalog_query.rs::parse`、`node parse_shadowed` 确认符号位置与上述关键调用边。初始 `files --filter pkg/server/pg_catalog_query` 和 `explore` 无输出，随后精确符号查询成功，因此没有用自然语言探索结果替代源码事实。
- Go 对照检查：当前 `pkg/server` 无 `doc.go` 和同路径 Go 文件；全仓库 Go 文件中未检出上述 Rust 专用符号或 `pg_catalog` 实现，故文档明确标记无直接 Go 对照。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定的 11 章节结构命令，并人工检查所有链接、边界数字、调用关系与“解析不等于可执行”的分层说明。
