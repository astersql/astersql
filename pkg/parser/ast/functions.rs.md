# `pkg/parser/ast/functions.rs`

## 文件定位

`functions.rs` 是 `astersql-parser-ast` crate 中的函数相关辅助模块，由 [`lib.rs`](lib.rs) 通过 `#[path = "functions.rs"] pub mod functions` 挂载，并把一部分函数名常量重导出到 crate 根。所属 crate 的边界由 [`Cargo.toml`](Cargo.toml) 声明：library 入口是 `lib.rs`，crate 名为 `astersql-parser-ast`，其包级迁移元数据对应 Go 的 `pkg/parser/ast`。

该文件对照 [`functions.go`](functions.go) 保留函数名常量、普通/聚合/窗口/类型转换函数的 SQL 还原规则，以及 TRIM 方向、时间单位和 `GET_FORMAT` 选择器等辅助表示。但它当前是一套基于 SQL 字符串的轻量模型：`Expr` 只保存 `sql: String`，`Visitor` 也不是 crate 根的 `Node`/`Visitor` 协议。不应把它与 [`lib.rs`](lib.rs) 内完整主 AST 的同名概念视为同一类型。

## 核心职责

- `function_names!` 产生与 Go 常量拼写对齐的 `&str` 公开常量，供 parser/planner 分派和还原时共用；包括普通、聚合、窗口、JSON、向量、FTS 和 TiDB 内部函数名。
- `FuncCallExpr` 把关键字函数或带 schema 的通用函数还原成 SQL，并为日期字面量、`JSON_MEMBER_OF`、`CONVERT`、日期加减、`EXTRACT`、`POSITION`、`TRIM` 和 `WEIGHT_STRING` 保留特殊语法。
- `JSONSumCrc32Expr` 和 `FuncCastExpr` 生成 `JSON_SUM_CRC32(expr AS type)` 及 `CAST`/`CONVERT`/`BINARY` 三种类型转换形态。
- `AggregateFuncExpr` 和 `WindowFuncExpr` 处理聚合/窗口函数的参数、`DISTINCT`、`GROUP_CONCAT` 的排序与分隔符，以及窗口的 `FROM LAST`、`IGNORE NULLS` 和 `OVER` 规格。
- `TimeUnitType::duration` 只把固定长度的单位转为 `std::time::Duration`，拒绝日历单位和组合单位；其他小型枚举/表达式负责稳定的 SQL 关键字文本。

## 主要符号

- `AstResult<T>` / `AstError`：本模块的错误通道。`AstError::new` 为私有构造器，错误只携带字符串并实现 `Display`/`Error`。
- `CiString { original, lower }`：`new` 保留原始拼写，同时通过 Unicode `to_lowercase` 生成分派用小写值。
- `Expr`、`expr`、`Expr::restore`/`format`：把已有 SQL 片段包装为参数节点；当前还原和格式化都只克隆内部字符串。
- `Visitor` / `visit_expr`：对单个 `Expr` 先调用 `enter_expr`、再调用 `leave_expr`，用 leave 返回的布尔值决定是否继续。`enter_expr` 返回的布尔值当前未被使用。
- `FuncCallExprType::{Keyword, Generic}` 与 `FuncCallExpr::{keyword,generic,arg,custom_restore,restore,format,accept}`：核心普通函数模型。`Keyword` 使用大写函数名，`Generic` 用反引号引用 schema/函数标识符。
- `quote_name` / `restore_args`：前者按 MySQL 标识符规则用反引号包围并将内部反引号翻倍；后者顺序还原参数并用 `", "` 连接。
- `FieldType` / `restore_as_cast_type`：当前只是目标类型文本的包装，`explicit_charset` 参数尚未改变输出。
- `JSONSumCrc32Expr`、`CastFunctionType`、`FuncCastExpr`：类型转换类节点；两个结构都只访问其单个 `expression` 子节点。
- `TrimDirectionType`、`DateArithType`、`TimeUnitType`、`GetFormatSelectorType`：对应 Go 枚举。`Invalid` 在需要字符串时输出空串。
- `AggregateFuncExpr`：`args` 的末项在 `GROUP_CONCAT` 中被当作 separator，`order_by` 是已还原的可选 SQL 片段。
- `WindowFuncExpr`：`spec` 是已还原的窗口规格文本，不是可递归访问的 AST 节点。
- `TrimDirectionExpr`、`TimeUnitExpr`、`GetFormatSelectorExpr`：无子节点的轻量关键字包装，`restore` 与 `format` 输出一致。

## 执行流程

1. 构造方在拿到函数名和已还原参数后，通过 `FuncCallExpr::keyword` 或 `generic` 建立轻量节点。目前可见的生产调用位于 [`sql_restore.rs`](sql_restore.rs)：它把主 AST 的时间间隔函数参数先还原为字符串，再调用 `FuncCallExpr::keyword(...).restore()` 复用 Go 兼容排列。
2. `FuncCallExpr::restore` 先进入 `custom_restore`。`DateLiteral`/`TimeLiteral`/`TimestampLiteral` 直接生成类型化字面量；`JSONMemberOf` 强制恰好两个参数并生成中缀 `MEMBER OF` 语法。
3. 如果没有特殊提前返回，`restore` 先输出可选 schema，再根据 `function_type` 决定大写关键字还是引用标识符。然后对特殊函数调整参数间关键字，其他函数走 `restore_args`。
4. `format` 是简化的调试文本路径，只特别处理日期加减、`JSON_MEMBER_OF` 和 `EXTRACT`；它不是 `restore` 的通用等价替代。
5. 聚合还原先输出大写名和可选 `DISTINCT`。对 `GROUP_CONCAT`，`split_last` 分离 separator，值参数之后插入 `order_by` 和 `SEPARATOR`；其他聚合直接还原参数。
6. 窗口还原在参数非空时才输出 `DISTINCT`，随后依次附加 `FROM LAST`、`IGNORE NULLS` 和 `OVER spec`。
7. 所有具有 `accept` 的复合结构按参数顺序调用 `visit_expr`，首个返回 `false` 的 leave 结果会立即终止后续访问。

## 数据与状态

本文件没有全局可变状态。函数名是编译期 `&'static str`；节点数据均由 `String`、`Vec<Expr>`、值枚举和 `Option<String>` 拥有。还原时在局部 `String` 中累积输出，不会修改节点；`accept(&mut self, ...)` 才会用 visitor 返回的 `Expr` 替换参数。

需要维持的结构不变量包括：类型化日期/时间字面量至少有参数 0；`CONVERT`、`EXTRACT`、`POSITION` 以及日期加减分支具有它们所索引的参数；`WEIGHT_STRING` 至少有参数 0；`GROUP_CONCAT` 的 `args` 末项是 separator，因此不得为空。这些中只有 `JSON_MEMBER_OF` 的数量被显式检查，其余多数依赖上游构造正确的 AST。

`TimeUnitType::duration` 的值域分为三类：微秒到周是固定 `Duration`；月/季/年因长度依赖日历而报错；`Invalid` 和全部组合单位走“尚不支持”错误。

## 依赖与调用关系

- 标准库依赖只有 `std::fmt` 和 `std::time::Duration`；文件内通过 `#[path = "flag.rs"] pub mod flag` 另挂载表达式标志传播子模块。目标文件本身不直接使用 `Cargo.toml` 中的 serde、URL 或其他 parser crates。
- crate 根 [`lib.rs`](lib.rs) 公开 `functions` 模块，并重导出规划器/表达式边界常用的部分常量；其余类型和常量可通过 `crate::functions::*` 访问。
- 直接生产调用证据是 [`sql_restore.rs`](sql_restore.rs) 的 `restore_expr` 时间间隔函数分支：主 AST `ExprKind::Function` → 递归还原参数 → `functions::expr` → `FuncCallExpr::keyword` → `restore`。
- [`functions_test.rs`](functions_test.rs) 是主要独立 Rust 测试，构造本文件的真实类型并调用生产 `restore()`；[`flag_5_aster_unit_test.rs`](flag_5_aster_unit_test.rs) 另覆盖特殊函数、聚合、窗口、CAST 和时间单位。
- RustCodeGraph `files --filter pkg/parser/ast` 将本文件识别为 103 个符号，`node --file` 报告它被 39 个已索引文件使用；然而精确 `callers`/`callees` 命令本次出现超时/符号 ID 误解析，因此上述具体边由直接源码搜索确认，不使用未验证的图推断。

## 错误处理与边界

`Expr::restore` 本身当前不会失败，但复合 API 仍统一返回 `AstResult<String>`，为参数缺失和后续扩展保留错误通道。`FuncCallExpr::arg` 在索引越界时返回 `missing function argument {index}`；`JSON_MEMBER_OF` 参数不为 2 时返回与 Go 对齐的 native-function 参数数量错误；空 `GROUP_CONCAT.args` 返回 `GROUP_CONCAT requires a separator argument`。

一些边界是有意的 Go 兼容行为：`TRIM` 参数数量不是 1、2、3 时不报错，而是还原为 `TRIM()`，由 `trim_unsupported_argument_counts_match_go_restore` 锁定。`WindowFuncExpr::restore` 只负责序列化 `distinct`/`ignore_null`/`from_last`，不检查具体窗口函数是否允许这些选项。

其他需由上游保证的边界包括：多个特殊还原分支直接索引固定参数，手工构造的非法节点可能在 `format` 中越界 panic；`AggregateFuncExpr::format` 和 `WindowFuncExpr::format` 明确 `panic!("Not implemented")`；`JSONSumCrc32Expr::format` 与 `FuncCastExpr::format` 用 `expect` 将当前的“内存字符串还原不会失败”假设固化为 panic 边界。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、文件句柄、网络连接或事务。所有节点都是普通拥有值，资源生命周期遵循 Rust 的作用域与 `Vec`/`String` 自动释放。

`restore(&self)` 和各种值枚举转换只读自身，不触及共享状态；多线程能否共享具体节点由它们的字段自动 trait 决定，文件没有额外并发协议。`accept` 需要对节点和 visitor 的独占可变借用，访问过程为同步、顺序并且可提前终止。

## 与 Go 版本的对应关系

Rust 的函数名常量、`FuncCallExpr.Restore/customRestore/Format/Accept`、`JSONSumCrc32Expr`、`FuncCastExpr`、`TrimDirectionType`、`DateArithType`、`AggregateFuncExpr`、`WindowFuncExpr`、`TimeUnitType.Duration`、`TimeUnitExpr` 和 `GetFormatSelectorExpr` 都有 [`functions.go`](functions.go) 中的直接对照。Rust 测试 [`functions_test.rs`](functions_test.rs) 按 Go [`functions_test.go`](functions_test.go) 用例驱动真实 Rust `restore()` API，覆盖特殊函数、CAST、聚合、窗口、generic schema 和错误路径。

目前不完全对等的地方必须在扩展时考虑：

- Go 节点实现统一 `Node`/`ExprNode` 和 `format.RestoreCtx`，并保留子节点类型；Rust 模块将参数、`order_by` 和窗口 `spec` 简化为 SQL 文本，因此不能在这些文本内继续结构化访问。
- Go `Accept` 包含当前节点的 enter/leave、可跳过子节点，且聚合会访问 `OrderByClause`、窗口会访问 `WindowSpec`；Rust `accept` 只访问 `Expr` 参数，忽略 enter 返回布尔值，也没有可访问的 order/spec 结构。
- Go `FieldType.RestoreAsCastType` 真正处理目标类型与显式字符集；Rust `FieldType` 只原样返回字符串，忽略 `explicit_charset`。
- Go 的 binary cast 使用一元运算符父子优先级还原，`JSON_MEMBER_OF` 使用二元运算符父子优先级还原；Rust 对已还原文本直接插值，不会自动增加优先级所需括号。
- Go `GROUP_CONCAT` 还原会使用 `RestoreStringWithoutCharset` 处理 separator；Rust 只还原传入的 `Expr` 文本。Go 的 TRIM 还原还会根据 nil `ValueExpr` 决定空格，Rust 的二/三参数版使用固定文本排列。
- Go 和 Rust 均将聚合/窗口 `Format` 保留为未实现 panic，这是当前事实，不是可用功能。

## 扩展指南

- 新增或修改函数名时，先核对 `functions.go` 的实际常量和大小写，更新 `function_names!` 列表；若 crate 根消费者需要无模块前缀访问，还要评估 [`lib.rs`](lib.rs) 的选择性 `pub use`。
- 新增非 `name(args)` 语法时，在 `FuncCallExpr::custom_restore` 或 `restore` 的分派中处理，并同步评估 `format`、参数数量验证、优先级括号和 `sql_restore.rs` 的主 AST 桥接。
- 扩展 CAST/JSON CRC32 类型文本时，应先补足 `FieldType::restore_as_cast_type` 与 Go field type 还原规则，特别是 charset/collation 和 `explicit_charset`，不要只增加字符串特例。
- 扩展聚合或窗口节点时，要明确 `order_by`/`spec` 是否仍然可以作为不透明文本；如需 visitor 重写它们，应与 crate 根主 AST 统一结构，而不是在文本上做二次解析。
- 每个行为变更应优先扩展独立的 [`functions_test.rs`](functions_test.rs)；涉及 flag 或 `sql_restore.rs` 桥接时再同步对应独立测试，不要把测试内嵌进 `functions.rs`。
- 兼容性风险主要来自标识符引用、关键字大小写、参数空格、运算符优先级和 Go 的有意容错语义；性能风险集中在多次 `String` 克隆、`format!` 和中间 `Vec<String>` 分配，但任何优化都必须先保留精确还原结果。

## 验证依据

- RustCodeGraph 索引状态：`rustcodegraph status` 报告 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/parser/ast` 列出 `functions.rs`/`functions_test.rs`/`functions.go` 等直接证据。
- RustCodeGraph 文件/符号证据：`node --file pkg/parser/ast/functions.rs --offset 1 --limit 500` 与 `--offset 500 --limit 500` 覆盖全部 934 行；`query FuncCallExpr/AggregateFuncExpr/WindowFuncExpr/TimeUnitType` 同时定位 Rust 与 Go 对照符号。精确 callers/callees 查询本次超时或错解析，因此不把其作为具体调用边证据。
- 已读 Rust 路径：[`functions.rs`](functions.rs)、[`lib.rs`](lib.rs)、[`sql_restore.rs`](sql_restore.rs)、[`functions_test.rs`](functions_test.rs)、[`flag_5_aster_unit_test.rs`](flag_5_aster_unit_test.rs)。当前目录不存在 `doc.go`，因此没有额外包约定可读。
- 已读 crate/对照路径：[`Cargo.toml`](Cargo.toml)、[`functions.go`](functions.go)、[`functions_test.go`](functions_test.go)。Rust 与 Go 的关键还原分支、visitor 范围、时间单位错误和测试意图均以这些文件交叉核对。
- 文档不宣称 Cargo 验证：任务明确为纯文档分析且禁止运行 Cargo。交付结构通过任务指定的 `test -f` 与 11 个固定二级标题计数命令验证。
