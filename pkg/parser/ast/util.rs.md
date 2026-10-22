# `pkg/parser/ast/util.rs`

源文件：[`util.rs`](./util.rs)

## 文件定位

位于解析器产出的 AST 与后续执行/规划代码之间，提供不依赖执行上下文的语句只读性分类，以及 AST 文本处理所需的转义工具再导出。

## 核心职责

1. 通过 `is_read_only(node, check_global_vars)` 对一棵语句 AST 分类。已明确支持 `SelectStmt`、`ExplainStmt`、`DoStmt`、`ShowStmt`、`RecommendIndexStmt`、`SetOprStmt`、`SetOprSelectList`、`AdminStmt` 和 `TraceStmt`；未列出的节点默认返回 `false`，因此对未知语句采取保守策略。
2. 用 `ReadOnlyChecker` 遍历 SELECT 子树，识别 `VariableExpr { is_system: true, value: Some(_) }` 这一系统变量赋值形态。
3. 提供 Go 迁移兼容名称：`IsReadOnly` 委托给 Rust 风格的 `is_read_only`，`UnspecifiedSize` 与 `UNSPECIFIED_SIZE` 等值。
4. 通过 `#[path = "../util/escape.rs"]` 引入并公开 `UnescapeChar`。直接调用点位于 [`base.rs`](./base.rs) 的 AST 原始文本恢复逻辑，用它展开 MySQL 反斜杠转义。

## 主要符号

- `pub const UNSPECIFIED_SIZE: u64 = u64::MAX`：未指定长度或大小的哨兵值，对应 Go 的 `math.MaxUint64`。
- `pub const UnspecifiedSize: u64`：保留 Go 命名的兼容别名；使用 `#[allow(non_upper_case_globals)]` 避免命名告警。
- `pub fn is_read_only(node: &dyn Node, check_global_vars: bool) -> bool`：核心分类入口。`Node::as_any` 提供运行时向具体 AST 类型的降型；返回值只表达本文件定义的只读分类，不返回诊断信息。
- `pub fn IsReadOnly(...) -> bool`：Go 风格兼容入口，不添加额外逻辑。
- `pub struct ReadOnlyChecker { pub read_only: bool }`：SELECT 子树扫描状态。它实现 `Visitor`；`enter` 遇到系统变量赋值后把状态置为 `false` 并跳过该节点子树，`leave` 返回当前状态，使遍历在发现写入后停止继续推进。
- `pub use self::parser_escape::UnescapeChar`：公开 MySQL 单字节转义展开函数；实际实现位于 [`../util/escape.rs`](../util/escape.rs)，不在本文件内重复实现。

## 执行流程

冻结增量在 DO/SHOW 与集合语句分派之间识别 crate 根部 `RecommendIndexStmt`：只有 `Action == RecommendIndexActionRun` 或 `Action == RecommendIndexActionReturn` 返回 true；SET、SHOW、APPLY、IGNORE、空值和未知 action 均为 false。该分支不受 `check_global_vars` 参数影响，也不解析 `SQL` 字符串或遍历其中的语句。此结果是 AST 分类契约，不能推导推荐算法的运行时副作用或权限。

`is_read_only` 按以下顺序分类，顺序也构成扩展时需要维护的分派规则：

1. 若节点是 `SelectStmt`，先检查 `lock_info`。`ForUpdate`、`ForUpdateNoWait`、`ForUpdateWaitN`、`ForShare`、`ForShareNoWait` 均立即返回 `false`；源码没有把 `ForUpdateSkipLocked`、`ForShareSkipLocked` 或 `None` 列入写意图集合。
2. 无上述锁且 `check_global_vars == false` 时，SELECT 立即返回 `true`，不会遍历变量表达式。
3. 需要检查变量时，创建 `ReadOnlyChecker { read_only: true }` 并调用 `SelectStmt::accept`。访问器仅在同时满足 `VariableExpr.is_system` 和 `VariableExpr.value.is_some()` 时翻转状态；用户变量赋值或只有变量读取不会触发。
4. `ExplainStmt` 在 `analyze == false` 时直接为只读；`EXPLAIN ANALYZE` 只有在存在内部语句且内部语句递归判定为只读时才为只读。空的 ANALYZE 包装因 `Option::is_some_and` 返回 `false`。
5. `DoStmt` 和 `ShowStmt` 直接返回 `true`。
6. `SetOprStmt` 与 `SetOprSelectList` 对所有 `selects` 递归调用本函数，并以 `Iterator::all` 汇总。空列表按 Rust `all` 的恒真语义返回 `true`。
7. `AdminStmt` 只允许七个查询型枚举值：`ShowDdl`、`ShowDdlJobs`、`ShowSlow`、`CaptureBindings`、`ShowNextRowId`、`ShowDdlJobQueries`、`ShowDdlJobQueriesWithRange`；其他 ADMIN 类型返回 `false`。
8. `TraceStmt` 递归继承内部 `Stmt` 的结果。其余节点最终落入默认 `false`。

`UnescapeChar` 不参与上述分类链。`base.rs` 在还原不可打印的字符串字面量时逐字节扫描，遇到可处理的反斜杠后调用它，并把返回的一个或两个字节追加到缓冲区。

## 数据与状态

本文件没有全局可变状态。两个大小常量在编译期确定；只读判定只借用 `&dyn Node`，不修改 AST。

`ReadOnlyChecker.read_only` 是一次遍历内的单调状态：初始为 `true`，唯一写操作是改为 `false`，不会恢复。`VariableExpr.value: Option<()>` 在当前 Rust AST 中只表示是否携带赋值，不保存赋值内容；因此本判定关心“是否赋值”，不解析右值。

集合运算节点以 `Vec<Box<dyn Node>>` 保存分支，函数只顺序借用并递归检查。`TraceStmt.Stmt` 是必有的 boxed 节点，而 `ExplainStmt.stmt` 是可选节点，这造成二者对空内部语句的边界不同。

## 依赖与调用关系

- crate 边界：[`Cargo.toml`](./Cargo.toml) 将本目录定义为 `astersql-parser-ast`，crate 根为 `lib.rs`；本文件所用的 AST 类型、`Node` 和 `Visitor` 都从 crate 根导入。本文件本身没有直接调用 Cargo 声明的外部 crate。
- 上游公开路径：`lib.rs::pub mod util` 使调用者可通过 `astersql_parser_ast::util` 使用常量、只读判定和 `UnescapeChar`。
- 已验证的 Rust 使用处：RustCodeGraph 将 [`util_test.rs`](./util_test.rs) 和 `pkg/session/runtime/scan_adapter_runtime_test.rs` 标为本文件使用者；仓库文本搜索还确认测试聚合入口 [`util_tests.rs`](./util_tests.rs) 重新导出 `crate::util::*`。当前索引没有给出生产主链调用 `is_read_only` 的直接边，因此不能据此声称它已接入所有执行入口。
- 下游 AST 依赖：`SelectStmt::accept` 通过 `walk::Children::visit_children` 驱动 `ReadOnlyChecker`；集合运算、EXPLAIN 和 TRACE 则在本函数内显式递归。
- 转义调用边：[`base.rs`](./base.rs) 调用 `crate::util::UnescapeChar`；实现来自 [`../util/escape.rs`](../util/escape.rs)。这是本模块除只读判定外的另一条独立使用链。

## 错误处理与边界

该 API 返回 `bool`，没有 `Result`、错误码或日志。不能识别的节点、非白名单 ADMIN、带指定锁的 SELECT，以及缺少内部语句的 `EXPLAIN ANALYZE` 都以 `false` 保守处理。

重要边界包括：关闭 `check_global_vars` 会在无锁 SELECT 上跳过整棵变量检查；仅 `is_system && value.is_some()` 算变量写入；空集合运算列表返回 `true`；普通 `EXPLAIN` 即使包装写语句也返回 `true`，因为它不执行内部语句，而 `EXPLAIN ANALYZE` 与 `TRACE` 会递归继承内部语义。

访问器依赖每种 AST 节点正确实现 `Node::accept` 及子节点枚举。若新增表达式字段却未纳入 `walk::Children`，这里可能漏检系统变量赋值；这属于 AST 遍历完整性风险，而不是本函数可报告的运行时错误。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源。`is_read_only` 的所有状态都在调用栈和局部 `ReadOnlyChecker` 中，每次调用相互独立；共享不可变 AST 时可否跨线程使用，取决于具体 `dyn Node` 类型的线程安全约束，本 API 没有声明 `Send` 或 `Sync`。

时间复杂度通常为已检查 AST 节点数 `O(n)`，空间主要来自包装语句和集合分支的递归深度。发现系统变量赋值或集合中的首个非只读分支后会短路；`check_global_vars == false` 的无锁 SELECT 为常数路径。`UnescapeChar` 每次分配一个很小的 `Vec<u8>`，其生命周期由 `base.rs` 调用方立即消费。

## 与 Go 版本的对应关系

实现机制有所不同：Go 用 type switch 与 `Walk(node, &checker)`，Rust 用 `Any` 降型、`Node::accept` 和 trait 对象；Go 的 checker 字段私有，而 Rust 当前结构体及字段均公开。

Rust 额外提供蛇形命名主入口和大写常量，同时保留 Go 风格别名以兼容机械迁移代码。Rust 的 `ExplainStmt.stmt` 是 `Option<Box<dyn Node>>`，因此显式定义了 Go 正常构造路径较少出现的“ANALYZE 但无内部节点”边界。转义函数的 Go 原始对应位于 `pkg/parser/util/escape.go`，本文件仅因 crate 模块路径需要将 Rust 实现再导出。

Rust 测试额外固定了所有五种写意图锁、空集合列表、变量三种组合、ADMIN 白名单和两个常量名。

## 扩展指南

- 新增可只读语句类型时，在 `is_read_only` 的具体类型分派中增加显式分支；不要改变末尾默认 `false`，除非能证明所有未知节点都安全。
- 新增 `SelectLockType` 时，应先判定它是否表达加锁/写意图，再同步锁匹配列表。尤其不要从名称类推 `SkipLocked` 行为，当前测试明确只锁定已有约定。
- 新增 ADMIN 子命令时，只有确认它纯查询且与 Go 行为一致后才加入白名单；同步更新 [`util.go`](./util.go) 对照分析及独立测试。
- 修改变量赋值 AST 形态时，应同步修改 `ReadOnlyChecker::enter`，并确认新字段已进入 `walk::Children`。测试应放在独立的 [`util_test.rs`](./util_test.rs) 或 [`util_8_aster_unit_test.rs`](./util_8_aster_unit_test.rs)，不要把测试写回生产文件。
- 调整 `UnescapeChar` 应修改真实实现 [`../util/escape.rs`](../util/escape.rs)，并检查 [`base.rs`](./base.rs) 的调用语境；本文件只负责模块装配与公开路径。
- 兼容性风险集中在误把写语句判为只读；性能风险集中在对大型 SELECT AST 的全树扫描和集合递归。扩展时应覆盖“应只读”和“必须非只读”两侧用例，并保持短路行为。
