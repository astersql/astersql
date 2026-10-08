# `pkg/table/tables/canonical_partition_expr.rs`

## 文件定位

本文件属于 `astersql-table-tables` crate，并且只在 `expression-runtime` feature 开启时由 `lib.rs` 公开为 `canonical_partition_expr` 模块。它把完整的 `model_dependency::TableInfo` 分区元数据编译成 `partition_expr.rs` 定义的运行时 `PartitionExpr`，供表元数据装载后的分区路由与裁剪使用。

直接生产入口位于 `tables.rs::table_from_meta_for_validation`：该函数为当前 `PartitionInfo` 调用一次 `build`，在 REORGANIZE/REMOVE/ALTER PARTITIONING 期间还会为 adding/dropping definitions 再构造一份重组表达式；同一文件也复用 `context` 解析部分索引条件。未启用 `expression-runtime` 时，本模块不参与编译，含分区表达式或条件索引的元数据装载会由 `tables.rs` 返回“runtime is required”错误。

## 核心职责

- `context` 建立非严格的表达式求值环境，使旧版本遗留的分区常量仍可折叠：允许无效日期，忽略截断、零日期、日期中零值和无效日期错误，并绑定全局的新排序规则开关。
- `build` 将 HASH、KEY、RANGE/RANGE COLUMNS、LIST/LIST COLUMNS 五类元数据归一为同一个 `PartitionExpr`，同时生成列偏移、定位表达式与各类裁剪辅助索引。
- `quote` 为 RANGE COLUMNS 的列名添加反引号并把内嵌反引号翻倍，避免拼装定位表达式时改变标识符含义。
- `reindex` 把 LIST 表达式里的列下标从整表 schema 改写到紧凑的分区列行布局；它递归处理 `ScalarFunction` 参数，并直接处理叶子 `Column`。

本文件只负责“从目录元数据构造表达式状态”。具体的 KEY 哈希、RANGE 比较、LIST 查找和位置集合运算由相邻的 `partition_expr.rs` 中各结构的方法执行。

## 主要符号

- `pub fn context() -> exprstatic_dependency::ExprContext`：创建构建期上下文。SQL mode 为 `ModeAllowInvalidDates`；`StrictFlags` 的四个忽略位被开启；截断错误组级别改为 `LevelIgnore`；排序规则模式取 `collate_dependency::NewCollationEnabled()`。
- `pub fn build(table, tp, text, names, defs) -> Result<Option<PartitionExpr>, String>`：唯一公开构建入口。`PartitionTypeNone` 返回 `Ok(None)`，支持的分区类型返回 `Ok(Some(...))`，错误统一转为字符串。
- `fn error(e: impl ToString) -> String`：把表达式、解析、求值或编码层错误压平为本模块 API 使用的 `String`。
- `fn quote(s: &str) -> String`：生成 SQL 反引号标识符。
- `fn reindex(expr: &mut ExprBox, cols: &[expression::Column]) -> Result<(), String>`：按 `UniqueID` 在紧凑列集合中定位列；未知列返回 `unknown LIST column ...`。

构建结果字段定义在 `partition_expr.rs::PartitionExpr`：`UpperBounds`、`OrigExpr`、`Expr`、四种 `For*Pruning` 状态以及 `ColumnOffset`。本文件不声明 trait、宏、常量或条件编译项；模块整体的条件编译由 `lib.rs` 控制。

## 执行流程

1. `build` 先处理 `PartitionTypeNone`，随后调用 `context`，再用 `ColumnInfos2ColumnsAndNamesWithCollate` 把目录列转换为表达式列和字段名。局部 `parse` 闭包固定使用这一 schema、字段名和 `TableInfo` 调用 `ParseSimpleExpr`。
2. 若 `names` 为空，`text` 被解析为分区表达式，`ExtractColumns` 提取引用列，并按 `UniqueID` 计算整表 `ColumnOffset`；表达式写入 `ret.Expr`。若 `names` 非空，则按不区分大小写的 `CIStr.L` 查找目录列、记录偏移，并按 `UniqueID` 去重得到分区列集合。
3. HASH：另用 `generatedexpr::ParseExpression` 保存原始 AST 到 `OrigExpr`；要求前一步已经生成 `Expr`，并调用 `HashCode` 触发表达式哈希码构造。
4. KEY：生成 `ForKeyPruning`，保存分区列以及上下文的排序规则模式。
5. RANGE：为每个 definition 拼装并解析定位条件。首个边界为 `MAXVALUE` 时条件恒为 `true`；多列边界中首次出现 `MAXVALUE` 时只比较它之前的前缀并使用 `<=`；否则做完整元组 `<` 比较。普通 RANGE 还构造整数 `LessThan`、`MaxValue`、`Unsigned`；RANGE COLUMNS 则要求每个有限边界解析为常量，并对 DATE/DATETIME 边界补 cast，`None` 表示后续 `MAXVALUE`。
6. LIST：普通表达式模式克隆定位/裁剪表达式，调用 `reindex` 改写裁剪表达式的列下标；逐项求整数值，把 NULL、DEFAULT 和有符号/无符号编码后的普通值分别记录。LIST COLUMNS 为每个分区列创建 `ForListColumnPruning`，将元数据值按目标类型和排序规则编码成字符串键，并记录键到分区及值组的映射。
7. 未识别的分区类型返回错误；成功时返回完整的 `PartitionExpr`。

## 数据与状态

函数本身没有全局可变状态。一次 `build` 的临时状态包括表达式 schema、字段名、解析闭包和逐步填充的 `PartitionExpr`。

关键不变量如下：

- `ColumnOffset` 总是相对于完整表列数组；LIST 裁剪表达式内部的 `Column.Index` 则相对于去重后的 `PruneExprCols`，两者不能混用。
- 普通 RANGE 的 `LessThan` 保持 definition 顺序；`MaxValue` 是独立标志，不用 `i64::MAX` 冒充无限上界；超出 `i64` 但属于 `u64` 的边界按位保存为 `i64` 并设置 `Unsigned`。
- RANGE COLUMNS 遇到 `MAXVALUE` 后向当前上界向量写入 `None` 并停止处理后续列，这与前缀已决定上界的语义一致。
- 普通 LIST 的值映射存入 `Arc<BTreeMap<u64, usize>>`；有符号整数用 `EncodeIntToCmpUint` 保序，无符号整数直接转 `u64`。重复键遵循 `BTreeMap::insert` 的后写覆盖行为。
- LIST COLUMNS 同时复制 `map` 给 `Sorted`、移动原表给 `ValueMap`。当前两者都是 `BTreeMap`，提供确定性键序；每个键可关联同一分区的多个 value-group 下标。
- NULL/DEFAULT 的分区下标初始化为 `-1`。LIST COLUMNS 的 `DefaultPartID` 保存 definition ID，`DefaultPartitionIdx` 保存 definition 在切片中的位置，两者用途不同。

## 依赖与调用关系

上游直接调用者均在 `pkg/table/tables/tables.rs::table_from_meta_for_validation`：

- `canonical_partition_expr::build(meta, partition.Type, ...)` 构造活动分区表达式；
- 同一入口针对 DDL 重组 definitions 构造 `reorganization_expression`；
- `canonical_partition_expr::context()` 为条件索引字符串提供一致的表达式解析上下文。

下游依赖包括：

- `model_dependency`：`TableInfo`、`PartitionDefinition`、`PartitionType` 和 `CIStr`；
- `expression`：目录列转换、schema、简单表达式解析、列提取、常量求值、类型/cast、比较编码及表达式树类型；
- `exprstatic_dependency` 与 `collate_dependency`：构建求值上下文和选择排序规则模式；
- `generatedexpr`：仅为 HASH 保存供 point-get 使用的原始 AST；
- `crate::partition_expr`：承载构建结果及运行期裁剪结构。

`Cargo.toml` 表明 `expression`、`exprstatic-dependency`、`stmtctx-dependency` 均由默认启用的 `expression-runtime` feature 拉入；`generatedexpr`、`model-dependency` 和 `collate-dependency` 是直接路径依赖。模块不直接使用 `stmtctx-dependency`，它是该 feature 的 crate 级依赖组成部分。

RustCodeGraph 能按文件读取本实现，但本次索引把该文件报告为 `used by 0 files`，未产生可靠 caller/callee 边；因此上述调用边以 `tables.rs` 的直接路径引用为准，而不是从缺失的图边推断。

## 错误处理与边界

所有失败通过 `Result<_, String>` 向 `table_from_meta_for_validation` 传播，不记录日志、不恢复，也不 panic。主要失败面包括目录列转换或表达式解析/求值/编码失败、未知列、缺失边界/值/类型、RANGE 列数不匹配、RANGE COLUMNS 非常量，以及未知分区类型。

显式边界行为：

- `PartitionTypeNone` 在访问表列前立即返回 `None`。
- `names` 中不存在的列返回带 `[table:1054]` 的错误；表达式提取出的列若无法按 `UniqueID` 回配，也返回未知分区列错误。
- HASH 若没有表达式返回 `HASH partition expression is missing`。
- RANGE 每个 definition 必须至少有一个 `LessThan`；非首列的 `MAXVALUE` 会截短元组比较；普通 RANGE 回退求值旧式常量表达式时拒绝 NULL。
- RANGE COLUMNS 的有限边界必须是 `expression::Constant`；DATE/DATETIME 会转换到目标列类型。代码以 `ret.ColumnOffset[i]` 索引类型，因此调用方元数据应保证每个边界的列数与分区列定义一致；定位表达式路径会先检查列数不匹配，但构造裁剪值的内层没有独立重复检查。
- 普通 LIST 每个 value group 必须至少含一个值；LIST COLUMNS 每组必须为每个列位置提供值。普通 LIST 对 `DEFAULT` 使用不区分大小写比较，LIST COLUMNS 当前只在单元素组中用精确字符串 `"DEFAULT"` 识别默认分区，这是扩展时必须保留或有意识修正的兼容点。
- `reindex` 只遍历 Column 和 ScalarFunction；若表达式树新增其他可含子表达式的节点种类，需要同步扩展遍历，否则列下标可能未被改写。

## 并发与资源生命周期

本模块不启动任务、不使用锁、通道、事务或 I/O。`context`、schema、解析闭包和中间向量都局限于一次同步 `build` 调用，并在返回后释放。

返回值中的共享映射使用 `Arc<BTreeMap<...>>`，允许克隆后的 `PartitionExpr` 只读共享裁剪索引；构建完成后本文件不再修改这些映射。表达式对象通过 `ExprBox` 持有；LIST 普通表达式显式克隆成 Locate/Prune 两份，避免 `reindex` 污染按整表布局求值的定位表达式。`context` 内的 eval context 也由 `Arc` 持有。

因此线程安全和实际执行期生命周期取决于 `ExprBox`、`ExprContext` 及 `partition_expr.rs` 消费者的约束；本文件自身没有跨线程协调逻辑，也没有资源清理回调。

## 与 Go 版本的对应关系

主要 Go 对照是 `pkg/table/tables/partition.go`：

- Rust `context` 对应 Go `NewPartitionExprBuildCtx` 的非严格日期/截断策略；Go 随后用表实例的 `UseNewCollate()` 覆盖排序规则，Rust canonical 路径当前直接取全局 `NewCollationEnabled()`。相邻模块提供的 `PartitionExpr::ForTable` 可以按具体 `TableCommon` 重绑 KEY/LIST COLUMNS 状态，但本次搜索只发现测试调用，未发现 canonical 生产装载链调用它。
- Rust `build` 合并了 Go `partitionedTable.newPartitionExpr`、`generateHashPartitionExpr`、`generateKeyPartitionExpr`、`generateRangePartitionExpr`、`generateListPartitionExpr` 及其若干辅助函数，同时改用完整 canonical `TableInfo` 作为入口。
- RANGE 的定位条件、旧式上界常量回退求值、`MAXVALUE`/无符号标志，以及 LIST 的 NULL/DEFAULT/比较编码均保持 Go 的核心数据语义。
- Go LIST 使用泛型 B-tree 保存值映射，Go LIST COLUMNS 同时维护 map 与有序 B-tree；Rust 使用 `BTreeMap` 承担查找与排序两种职责。
- Go 未知分区类型走不可达 panic；Rust 返回 `unknown partition type` 错误。Rust 还把多种下游错误统一降为字符串，因此不保留 Go `errors.Trace` 的类型和堆栈信息。
- Go 解析 HASH 时先建立 parser AST 再由 AST 构建表达式；Rust 的运行时表达式走 `ParseSimpleExpr`，原始 AST 单独走 `generatedexpr::ParseExpression`。两条结果仍分别填充 `Expr` 与 `OrigExpr`。

现有 Rust 直接回归证据是 `tables_test.rs::normal_ddl_plan_table_validation_canonical_key_and_reorganization`，验证 KEY 的列偏移、裁剪列和重组表达式均经 canonical 入口生成。`partition_expr_test.rs` 验证相邻模块 `NewPartitionExprBuildCtx` 的标志、按表重绑定排序规则以及下游裁剪结构；它不直接调用本文件的 `context`，也没有逐分支直接调用本文件 `build` 的完整 HASH/RANGE/LIST 边界矩阵。Go 同目录未发现以 `newPartitionExpr` 等构造函数命名的直接测试；相关行为主要由该包更广泛的分区测试间接覆盖。

## 扩展指南

- 新增分区类型或改变元数据解释时，修改 `build` 的类型分派，并同步检查 `PartitionExpr` 是否需要新状态；必须在独立测试文件中为成功路径、缺失元数据和错误传播增加用例。
- 新增表达式节点时，审计 `reindex` 是否能遍历节点的所有子表达式；建议在 `partition_expr_test.rs` 或 `tables_test.rs` 增加嵌套表达式回归，测试逻辑不要内嵌到生产文件。
- 调整 RANGE COLUMNS 时必须同时维护定位条件与裁剪边界两套表示，覆盖列数不匹配、前缀 `MAXVALUE`、DATE/DATETIME cast 和非常量错误。
- 调整 LIST/LIST COLUMNS 编码时必须保持 SQL 类型转换、statement error context、排序规则模式、有符号比较序及 DEFAULT/NULL 回退一致；相关消费逻辑位于 `partition_expr.rs`，两侧数据结构应一起审查。
- 若把 canonical 构建上下文改为表级排序规则，需核对 `tables.rs` 的活动/重组表达式装载及 `PartitionExpr::ForTable`，避免一份表达式使用全局模式、另一份使用表模式。
- 性能方面，当前 LIST COLUMNS 为每列、每 definition、每 value group 解析和编码，并克隆一份 `BTreeMap`；大分区元数据上的改动应关注构建耗时与内存，但不能用无序映射破坏有序范围查询语义。

## 验证依据

- 源码与类型：`pkg/table/tables/canonical_partition_expr.rs`、`pkg/table/tables/partition_expr.rs`、`pkg/table/tables/lib.rs`、`pkg/table/tables/tables.rs`。
- crate 边界：`pkg/table/tables/Cargo.toml` 的 `expression-runtime` feature、依赖列表和 `package.metadata.porting.go-package = "pkg/table/tables"`。
- Go 对照：`pkg/table/tables/partition.go` 中 `NewPartitionExprBuildCtx`、`newPartitionExpr`、`PartitionExpr`、`dataForRangePruning`、四个 `generate*PartitionExpr` 及 LIST 构建辅助函数。
- 测试：`pkg/table/tables/tables_test.rs::normal_ddl_plan_table_validation_canonical_key_and_reorganization`；`pkg/table/tables/partition_expr_test.rs` 中相邻构建上下文、排序规则及裁剪结构测试。未发现直接以本模块名或 `build` 为目标、覆盖全部构建分支的独立 Rust 测试。
- RustCodeGraph：`status` 显示项目已索引 11,467 个文件；`node --file pkg/table/tables/canonical_partition_expr.rs` 读取了全文件 355 行；`explore/query/callers/callees` 未给出可用调用边，文件节点显示 `used by 0 files`，故调用关系另由 `rg` 定位并读取 `tables.rs` 直接验证。
- 文档结构使用任务指定命令校验，要求目标存在且固定二级标题恰好 11 个；本任务是纯文档分析，按计划不运行 Cargo。
