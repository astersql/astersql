# `pkg/parser/ast/dml.rs`

## 文件定位

`pkg/parser/ast/dml.rs` 是 `astersql-parser-ast` crate 中公开的轻量 DML AST 与 SQL 文本还原模块。crate 入口 `pkg/parser/ast/lib.rs` 通过 `pub mod dml` 暴露它，crate 边界由 `pkg/parser/ast/Cargo.toml` 定义；该文件自身只依赖 Rust 标准库，没有直接使用 Cargo 清单里的其他 parser crate 或第三方库。

本文件不是完整的 DML 语句层：它只覆盖表名、索引 Hint、LIMIT、通配符、排序/分组项、窗口 Frame、结果集与 Join 等基础结构。`lib.rs` 中仍存在另一套字段名保持 Go 风格、实现 `Node`/`Visitor` 协议的完整 AST 类型。仓库搜索到的 `crate::dml::*`、`dml::TableName`、`dml::FrameClause`、`dml::Join` 等直接使用目前集中在 `pkg/parser/ast/dml_3_aster_unit_test.rs`、`pkg/parser/ast/dml_test.rs` 和 `pkg/parser/ast/go_merge_13_test.rs`；没有检出生产解析主链直接调用本模块的证据。因此，它当前应被理解为公开但尚未替代 crate 根完整 AST 的值语义移植面，而不是 SQL parser 的唯一 DML AST。

## 核心职责

1. 以拥有所有权的 `String`、`Vec`、`Option`、`Box` 表达一组常用 DML AST 片段，避免 Go 指针 AST 对应的共享可变状态（文件注释及各类型字段）。
2. 将这些值还原为规范化 SQL 文本：`quote_name` 负责 MySQL 反引号转义，各类型的 `restore`/`try_restore` 负责固定关键字、空格、逗号和子节点顺序。
3. 保留关键的 MySQL Join 结合优先级：`NewCrossJoin` 在右侧为未显式括号约束的 Join 树时，把新交叉连接插入最左叶，而不是简单把整棵右树挂到新根下。
4. 显式保留 Go 行为中的边界：`ByItem::restore` 不使用 `null_order`；`FrameClause::try_restore` 拒绝 `FrameType::Groups`；`Join::restore` 在左右子节点也是 Join 时补括号。

该文件不负责词法/语法解析、名称解析、优化或执行，也不实现 crate 根 `Node`/`Visitor`。输入表达式和查询多以 `String` 保存，其合法性由调用方保证。

## 主要符号

### 辅助函数

- `quote_name(name: &str) -> String`：公开的标识符引用函数，用反引号包围名称，并把内部反引号替换成两个反引号。
- `restore_expression(expression: &str) -> String`：私有的简化表达式还原器。`NULL`（大小写不敏感）、`?`、可解析为 `i128` 的整数和以单引号开头的文本原样保留；其他文本按 `.` 分段并逐段调用 `quote_name`。它只由 `ByItem::restore` 调用。

### 表、Hint 与简单子句

- `TableName { schema, name, partition_names, index_hints }`：`new` 构造无分区、无 Hint 的表名；`restore` 依次输出可选 schema、表名、`PARTITION(...)` 和每条 `IndexHint`。
- `IndexHintType::{Use, Ignore, Force, Order, NoOrder}` 与 `IndexHintScope::{Scan, Join, OrderBy, GroupBy}`：有限枚举保证 `IndexHint::restore` 的类型和作用域总能映射为关键字，不存在 Go 版本无效整数枚举的默认错误分支。
- `IndexHint { hint_type, scope, index_names }`：还原为 `USE/IGNORE/FORCE/ORDER/NO ORDER INDEX`，可附加 `FOR JOIN/ORDER BY/GROUP BY`；索引名以逗号加空格连接。
- `Limit { count, offset }`：以字符串保存计数和偏移，允许参数占位符；输出 `LIMIT count` 或 `LIMIT offset,count`。
- `WildCardField { schema, table }`：覆盖 `*`、`table.*`、`schema.table.*`。若只有 schema 而没有 table，当前匹配分支会退化为 `*`。
- `ByItem { expression, desc, null_order }`：输出简化表达式和可选 ` DESC`；`null_order` 是保留字段，当前不参与输出。
- `item_clause!`：私有宏，为 `GroupByClause`、`OrderByClause`、`PartitionByClause` 生成相同的 `items`、`new` 与 `restore` 骨架。前两者项目分隔符为 `,`，后者为 `, `。

### 窗口 Frame

- `FrameType::{Rows, Range, Groups}`：窗口帧种类；只有前两种可成功还原。
- `BoundDirection::{Preceding, Following}`：由私有 `restore` 映射方向关键字。
- `FrameBound::{CurrentRow, Unbounded(direction), Expr { expression, direction }}`：提供 `current_row`、`preceding`、`following` 构造器；`restore` 负责单侧边界文本。
- `FrameClause { frame_type, start, end }`：`try_restore` 返回 `Result<String, &'static str>`，输出 `ROWS/RANGE BETWEEN start AND end`；`Groups` 返回固定错误。`restore` 是便捷接口，会对该错误执行 `expect` 并 panic。

### Join 树

- `JoinType::{CrossJoin, LeftJoin, RightJoin, FullJoin}`：默认值为 `CrossJoin`。
- `ResultSet::{Table(TableName), Join(Box<Join>), Query(String)}`：统一表、嵌套 Join 与查询文本；`Query` 还原时总在外层加括号。
- `Join { left, right, join_type, on, using, natural_join, straight_join, explicit_parens }`：`new` 要求初始左右节点均存在；链式构造器 `natural`、`using`、`on`、`straight`、`explicit_parens` 设置可选属性；`restore` 按树形和属性生成 SQL。
- `NewCrossJoin(left, right) -> Join`：名称刻意沿用 Go API。普通右节点、右 Join 缺少右子节点或右 Join 有显式括号时直接建新根；否则沿右树左脊下沉，并把新交叉连接插入最左可插入位置。

本文件没有 trait 定义、trait impl、条件编译项或模块级常量；公开面由上述函数、结构体、枚举及其公开字段/方法组成。

## 执行流程

### 一般 SQL 还原

1. 调用方构造拥有数据的 AST 值，例如 `TableName::new`、`ByItem::new`、`FrameClause::new` 或 `Join::new`。
2. 调用对应 `restore`。表名和索引名进入 `quote_name`；`ByItem` 的表达式进入 `restore_expression`；复合对象递归调用子对象的 `restore`。
3. `TableName::restore` 的顺序不变：`schema.name` → 分区列表 → 索引 Hint 列表。`Join::restore` 的顺序不变：左节点 → Join 修饰和类型 → 右节点 → `ON` → `USING`。
4. `FrameClause` 若需要可恢复错误，应调用 `try_restore`；仅在类型已知为 `Rows`/`Range` 时使用会 panic 的 `restore`。

### `Join::restore`

1. 若左节点是 `ResultSet::Join`，先在其输出外加括号；否则直接输出。
2. `right == None` 时立即返回左节点文本。这允许表示只有左侧的包装 Join。
3. 依次追加 `NATURAL`、`LEFT`/`RIGHT`/`FULL OUTER`；`CrossJoin` 不输出类型修饰。随后按 `straight_join` 选择 `STRAIGHT_JOIN` 或普通 `JOIN`。
4. 右节点为 Join 时也加括号；`Query` 自身还会由 `ResultSet::restore` 加括号。
5. 最后输出原样保存的 `ON` 表达式，以及以反引号引用的 `USING` 列表。

### `NewCrossJoin`

1. 若 `right` 不是 Join，或右 Join 的 `right` 为空，或根节点带 `explicit_parens`，直接返回 `Join::new(left, right, CrossJoin)`，保持显式作用域。
2. 否则从右 Join 根开始沿 `left` 子树向下，只要左子节点仍是具有右孩子的 Join 就继续。
3. 遍历到的节点若是带显式括号的 `RightJoin`，交换其左右子节点并改为 `LeftJoin`，与 Go `NewCrossJoin` 的等价改写一致。
4. 到达最左可插入节点后，暂时用空 `ResultSet::Query` 占位取走旧左孩子，立即用 `Join(left, old_left, CrossJoin)` 替换；占位值不会逸出函数。
5. 返回经原地重写后的右 Join 根。`pkg/parser/ast/dml_3_aster_unit_test.rs::join_restore_and_cross_join_rewrite_match_go` 覆盖普通右连接、根显式括号与嵌套最左叶插入。

## 数据与状态

所有节点都按值拥有状态：文本使用 `String`，重复项使用 `Vec`，可缺省状态使用 `Option`，递归 Join 使用 `Box`。派生的 `Clone`、`Debug`、`Eq`、`PartialEq` 便于复制、诊断与精确比较；`JoinType` 另外实现 `Default`，默认交叉连接。

关键状态约束如下：

- `TableName.name`、`Limit.count` 等字段类型上允许空字符串，本文件不做语义校验。
- `Join.right` 可以为空，但 `Join::new` 总会写入 `Some(right)`；空值只能由公开字段直接构造/修改，并使 `restore` 只返回左侧。
- `Join.on` 与非空 `Join.using` 可以同时存在，`natural_join` 也可与二者同时存在；本文件按固定顺序全部输出，不检查 SQL 语义互斥性。
- `explicit_parens` 只影响 `NewCrossJoin` 的树重写决策，本身不会让 `Join::restore` 给当前节点主动加括号；括号由父 `ResultSet::Join` 的恢复逻辑决定。
- `ByItem.null_order` 当前不影响结果，这与 Go `ByItem.Restore` 忽略 `NullOrder` 一致。
- `FrameBound::Expr.expression`、`Join.on` 和 `ResultSet::Query` 是已格式化文本，不拥有表达式 AST，也不进行转义或合法性检查。

## 依赖与调用关系

### crate 与模块边界

- `pkg/parser/ast/Cargo.toml` 声明 crate 名 `astersql-parser-ast`、入口 `lib.rs`，并记录 Go 包映射 `pkg/parser/ast`。本文件没有引用清单中的 `parser-auth`、`parser-charset`、`parser-mysql`、`parser-types`、`serde`、`serde_json` 或 `url`。
- `pkg/parser/ast/lib.rs` 用 `#[path = "dml.rs"] pub mod dml;` 导出本模块，但没有 `pub use dml::*`，因此调用方通常通过 `parser_ast::dml::...` 访问，以免与 crate 根同名类型混淆。

### 已核对的调用边

- RustCodeGraph 将 `restore_expression -> quote_name`、`ByItem::restore -> restore_expression`、`TableName::restore -> quote_name/IndexHint::restore`、`FrameClause::restore -> try_restore`、`ResultSet::restore <-> Join::restore`、`Join::restore -> quote_name` 标为文件内调用关系。
- RustCodeGraph 的 `NewCrossJoin` 节点显示 Rust 定义位于 `pkg/parser/ast/dml.rs:525`，并同时定位到 Go 对照 `pkg/parser/ast/dml.go:132`；Rust 图对通用名 `Join`/`Query` 存在歧义，因此下游关系以源文件控制流为准。
- 仓库文本搜索未发现生产 Rust 文件直接调用 `dml::NewCrossJoin` 或这些轻量类型；直接消费者是 `pkg/parser/ast/dml_3_aster_unit_test.rs`、`pkg/parser/ast/dml_test.rs`、`pkg/parser/ast/go_merge_13_test.rs`。作为 `pub mod`，仓库外部 crate 仍可调用，但本地索引不能证明外部使用情况。

这意味着当前应用主链仍以 `lib.rs` 的完整 AST、parser actions 和相关 `Node`/`Visitor` 实现为主；本文件提供的是一块可独立使用和逐步迁移的还原/树操作 API。

## 错误处理与边界

- `FrameClause::try_restore` 是唯一显式返回错误的接口；`Groups` 返回 `Err("Unsupported window function frame type")`，与 Go `FrameClause.Restore` 的错误文本一致。
- `FrameClause::restore` 对同一情况 panic，错误信息是小写的 `unsupported window function frame type`。可接收不可信或未校验枚举状态的路径应使用 `try_restore`。
- `NewCrossJoin` 中的 `unwrap` 和两个 `unreachable!` 依赖前置模式匹配及循环不变量：进入改写路径时根右孩子存在；需要交换时当前节点右孩子存在；函数末尾的 `right` 仍是 `ResultSet::Join`。未来修改遍历条件时必须同步维护这些不变量。
- `restore_expression` 不是通用 SQL 表达式格式器：负整数可由 `i128` 识别，但小数、函数调用、运算式、双引号字符串等会被当作点分标识符引用；以单引号开头但不闭合的文本也原样通过。
- `quote_name` 只处理标识符反引号，不负责字符集、排序规则或 SQL 字符串转义。
- 枚举消除了 Go 版本 `IndexHintType`/`IndexHintScope` 非法整数的错误分支，但公开字段组合仍可能生成语义无效 SQL。
- 与完整 Go `TableName.Restore` 相比，本文件没有默认数据库、CTE、别名、`AS OF` 或 `TABLESAMPLE` 状态；与完整 Go `Join.Restore` 相比，没有基于派生表包装节点选择逗号连接的特殊分支。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、I/O 或外部资源句柄。所有 `restore(&self)` 都只读借用节点并创建新的 `String`；`NewCrossJoin` 消费左右 `ResultSet` 的所有权并在局部可变右树上完成重写，返回后没有共享引用或后台生命周期。

复杂度主要由输出和树深决定：名称/列表还原会分配临时 `Vec<String>` 后 `join`；`Join::restore` 递归遍历树并反复构造字符串；`NewCrossJoin` 沿右树左脊线性遍历，时间为 `O(h)`、额外树栈空间为常数。极深 Join 树的 `restore` 仍受递归调用栈限制。

类型没有显式 `Send`/`Sync` 实现，但其成员均为标准拥有型数据，自动 trait 由编译器推导；并发使用时调用方可安全共享不可变引用，若要修改必须由 Rust 独占借用或外部同步保证。

## 与 Go 版本的对应关系

Go 权威对照是 `pkg/parser/ast/dml.go`，相关回归是 `pkg/parser/ast/dml_test.go`；Rust 独立测试位于 `pkg/parser/ast/dml_test.rs` 与 `pkg/parser/ast/dml_3_aster_unit_test.rs`。

### 已保持的语义

- `NewCrossJoin` 对应 Go 同名函数：保留显式括号作用域、沿右树最左叶插入，并在特定右连接节点上交换孩子改写为左连接。
- 表名、分区与 Hint 的输出顺序一致；标识符内部反引号均成对转义。
- `Limit` 保持 `LIMIT offset,count` 格式；通配符、Group/Order/Partition By 分隔符、Join 关键字及 `USING` 列分隔符与相关 Go 恢复测试一致。
- `ByItem` 与 Go 一样忽略 `NullOrder`；`FrameClause` 与 Go 一样只接受 ROWS/RANGE 并拒绝 GROUPS。
- `pkg/parser/ast/dml_test.rs` 的 `TestTableNameRestore`、`TestLimitRestore`、`TestWildCardFieldRestore`、`TestJoinRestore`、`TestByItemRestore`、`TestGroupByClauseRestore`、`TestOrderByClauseRestore`、`TestFrameBoundRestore`、`TestFrameClauseRestore`、`TestPartitionByClauseRestore` 镜像了 Go 测试意图。

### 有意或当前存在的表示差异

- Go 节点嵌入 `node`、实现 `Node.Restore/Accept` 并持有真实 `ExprNode`、`ResultSetNode`、`OnCondition`、`ColumnName`；本文件使用字符串和小型枚举，不实现访问者协议。
- Go `TableName` 还包括 `TableSample`、`AsOf`、`IsAlias`，并受 `RestoreCtx` 的默认库、CTE 与 flags 影响；轻量 `TableName` 只保留 schema/name/partition/hints。
- Go `GroupByClause` 有 `Rollup`，`OrderByClause` 有 `ForUnion`；轻量子句不保存这些字段。
- Go `FrameBound` 分开保存边界类型、`UnBounded`、表达式和时间单位；轻量枚举把已格式化表达式（包括 `INTERVAL ...`）作为字符串保存。
- Go `Restore` 把子节点错误加上下文后传播；轻量接口大多返回 `String`，只为不支持的 Frame 类型保留 `Result`。
- crate 根的同名完整 AST 与 `dml` 子模块类型是不同 Rust 类型，不能直接互换。新增功能前必须先决定改的是生产完整 AST、轻量迁移 API，还是两者都要同步。

## 扩展指南

1. 新增或修改 SQL 还原行为时，先在 `pkg/parser/ast/dml.go` 和 `pkg/parser/ast/dml_test.go` 确认 Go 契约，再定位本文件对应 `restore`/`try_restore`；不要用字符串捷径删减 Go 的分支。
2. 若功能属于完整 parser AST（需要 `Node`、visitor、真实表达式或 parser actions），主实现位置很可能是 `pkg/parser/ast/lib.rs` 及相关 parser action，而不是只改本文件。若轻量 API 也承诺该能力，应显式同步两套类型并记录转换边界。
3. 修改标识符格式时集中调整 `quote_name`；扩大表达式能力时不要继续堆叠 `restore_expression` 的启发式分支，宜引入明确的表达式表示或复用完整 AST 的恢复能力，同时防止对原样 SQL 的注入/错误引用。
4. 扩展 `IndexHintType`/`IndexHintScope` 时同步枚举与 `IndexHint::restore`，并在 `pkg/parser/ast/dml_3_aster_unit_test.rs` 添加每种类型、作用域、空列表和含反引号名称的用例。
5. 扩展 Frame 时优先修改 `FrameType`、`FrameBound` 和 `FrameClause::try_restore`，保持 `restore` 仅作为已验证输入的便捷层；同步 `frame_clause_rejects_groups_like_go` 和 Go 对照错误契约。
6. 修改 Join 树时保持 `right == None`、显式括号、嵌套 Join、Right-to-Left 改写和最左叶插入不变量；回归测试应放在独立文件 `pkg/parser/ast/dml_3_aster_unit_test.rs` 或 `pkg/parser/ast/dml_test.rs`，不要内嵌进 `dml.rs`。
7. 性能敏感扩展应关注各 `restore` 中 `collect::<Vec<_>>().join(...)` 与递归字符串拼接带来的分配；优化时必须保持输出字节级兼容。
8. 本任务仅分析文档，不改变 Rust 行为；若后续修改 Rust 源码，按仓库规则同步独立测试、运行 `cargo fmt --all`，并使用适用验证流程。

## 验证依据

### RustCodeGraph

- `rustcodegraph status`：索引可用，包含 11,467 个文件、307,296 个节点、1,848,419 条边。
- `rustcodegraph files --filter pkg/parser/ast/dml.rs`：目标文件已索引，报告 68 个符号。
- `rustcodegraph node --file pkg/parser/ast/dml.rs --offset 1 --limit 420` 与 `--offset 421 --limit 220`：覆盖源文件全部 559 行，并列出 29 个引用该文件的索引文件。
- `rustcodegraph node NewCrossJoin`：核对 Rust `pkg/parser/ast/dml.rs:525` 与 Go `pkg/parser/ast/dml.go:132` 两个定义及源码。
- `rustcodegraph query TableName --kind struct`、`query Join --kind struct`、`query FrameClause --kind struct`：确认同名符号分布；`FrameClause` 查询同时定位完整 AST、轻量 AST、Go 实现和 Rust 测试。通用符号名的 callers/callees 查询产生歧义，因此未把模糊边当作生产调用证据。

### 已阅读文件

- 目标源码：`pkg/parser/ast/dml.rs`。
- crate 边界与模块入口：`pkg/parser/ast/Cargo.toml`、`pkg/parser/ast/lib.rs`。目标目录无 `doc.go`，因此没有可读取的包级 Go 契约文件。
- Go 对照：`pkg/parser/ast/dml.go`，重点核对 `Join`/`NewCrossJoin`、`TableName`、`IndexHint`、`WildCardField`、`ByItem`、Group/Order/Partition By、`Limit`、`FrameClause` 与 `FrameBound`。
- Rust 独立测试：`pkg/parser/ast/dml_3_aster_unit_test.rs`、`pkg/parser/ast/dml_test.rs`，并通过仓库搜索确认 `pkg/parser/ast/go_merge_13_test.rs` 的 Full Join 直接用例。
- Go 测试：`pkg/parser/ast/dml_test.go`，重点核对同名 Restore 测试和 Join 优先级用例。

### 验证结论与限制

人工复核确认本文说明了文件存在目的、构造与恢复流程、关键树改写、不变量、错误边界、迁移差异和安全扩展位置。按任务约束未运行 Cargo 或代码测试；本文记录的是源码、调用图和已有测试意图，不宣称本轮重新执行过测试。结构验证命令及退出码在任务交付时记录。
