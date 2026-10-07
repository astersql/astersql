# `pkg/parser/test_driver/test_driver.rs`

## 文件定位

本文件属于 `astersql-parser-test_driver` crate；crate 根 `pkg/parser/test_driver/lib.rs` 以私有模块 `test_driver` 装入本文件，再用 `pub use test_driver::*` 导出其公共符号。`pkg/parser/test_driver/Cargo.toml` 表明它直接依赖 parser 的 charset、format、mysql、types 子 crate 以及 `hex`，并通过 `[package.metadata.porting]` 对应 Go 包 `pkg/parser/test_driver`。

它提供 Go 测试驱动中 `ValueExpr`、`ParamMarkerExpr` 及访问者接口的 Rust 迁移实现。`pkg/parser/yy_parser.rs::New` 会调用 `init_test_driver`，因而形成主 parser crate 到本 crate 的显式链接；但该函数当前为空。Rust 主 AST 的值表达式路径目前由 `pkg/parser/ast/lib.rs::NewValueExpr` 和 `ExprNode::typed_value` 实现，并不通过本文件的 `newValueExpr` 动态注册。因此，本文件更准确的当前角色是轻量测试/迁移兼容实现，而不是 Rust 解析器所有字面量节点的唯一生产实现。

## 核心职责

- `ValueExpr` 把 `Datum`、`types::FieldType` 和投影偏移组合成可构造、还原、格式化和访问的叶子表达式。
- `ParamMarkerExpr` 在 `ValueExpr` 形状之上保存预处理语句参数的源码偏移、顺序和执行期标志，并能还原成 `?`。
- `Visitor` 支持拥有所有权的节点替换；`InPlaceVisitor` 支持不搬移节点的原地遍历。两个节点都是叶子，因此只有 Enter/Leave 两个阶段，没有子节点递归。
- `format_float` 与 `quote_go_string` 补足 Rust 标准格式化和 Go `strconv` 输出之间的差异；后者借助 `include!("../../util/plancodec/go_quote_printable.rs")` 的可打印 Unicode 区间复现 Go 引号规则。
- `init_test_driver` 保留 Go 包级 `init` 的调用边界，但当前没有注册或全局状态修改。

以上职责均来自本文件的真实符号；不应把 Go `init` 中对 `ast.NewValueExpr`、`ast.NewParamMarkerExpr`、十进制和二进制字面量构造器的赋值视为 Rust 已完成的行为。

## 主要符号

- `pub fn init_test_driver()`：无副作用的显式初始化入口，由 `pkg/parser/yy_parser.rs::New` 调用。
- `pub trait Visitor`：`Enter(Box<dyn Any>) -> (Box<dyn Any>, bool)` 可替换节点并决定是否跳过子节点；`Leave` 返回最终节点和继续标志。
- `pub trait InPlaceVisitor`：以 `&mut dyn Any` 接收节点；`Enter` 返回是否跳过子节点，`Leave` 返回是否继续遍历。
- `pub struct ValueExpr`：公开保存 `datum: Datum` 与 `Type: types::FieldType`，私有保存 `projection_offset: i32`。`Default` 来自字段默认值，而正常构造应使用 `ValueExpr::new` 以把偏移设为 `-1`。
- `ValueExpr::new` / `newValueExpr`：接收 `Box<dyn Any>`。若载荷本身是 `ValueExpr`，直接 downcast 并返回原对象；否则调用 `DefaultTypeForValue` 推断字段类型，再由 `Datum::SetValue` 保存值，最后将投影偏移设为 `-1`。
- `ValueExpr::Restore` / `RestoreToString`：按 `Datum::Kind` 输出 SQL 文本；字符串输出还受 `RestoreFlags`、字段字符集和默认字符集控制。
- `ValueExpr::Format`：输出展示/调试文本。字符串和字节使用 Go 兼容引号规则；写入错误被丢弃，未知 kind 会 panic。
- `ValueExpr::{SetProjectionOffset, GetProjectionOffset}`：维护逻辑计划使用的投影位置。
- `ValueExpr::{Accept, AcceptInPlace}`：分别实现可替换访问和原地访问。
- `pub struct ParamMarkerExpr`：包含 `value_expr: ValueExpr`、`Offset`、`Order`、`InExecute`；字段形状对应 Go 的嵌入式 `ValueExpr` 和参数元数据，但 Rust 的 `value_expr` 不是公开字段。
- `ParamMarkerExpr::new` / `newParamMarkerExpr`：仅设置源码 `Offset`，其他字段使用默认值。
- `ParamMarkerExpr::{Restore, RestoreToString}`：固定输出 `?`；`Format` 明确尚未实现并 panic；`SetOrder` 更新参数顺序。
- 私有 `format_float`：将有限浮点数格式化为带符号、至少两位指数数字的科学计数法，并专门返回 `NaN`、`+Inf`、`-Inf`。
- 私有 `quote_go_string`：处理 Go 的短控制字符转义、ASCII 十六进制转义以及 `\u`/`\U` Unicode 转义。

## 执行流程

构造普通字面量时，`ValueExpr::new` 首先检查动态载荷是否已是 `ValueExpr`。复用分支保留原 Datum、类型和投影偏移；普通分支先创建默认节点，再通过 `DefaultTypeForValue` 写入 FieldType，通过 `Datum::SetValue` 写入载荷，并建立 `projection_offset == -1` 的未映射约定。`newValueExpr` 只是该方法的 Go 风格别名。

还原 SQL 时，`RestoreToString` 创建 `format::RestoreCtx` 并委托 `Restore`。`Restore` 对 null、整数、无符号整数、32/64 位浮点、字符串、字节、十进制和二进制字面量分别处理。带 `mysql::IsBooleanFlag` 的有符号整数输出 `TRUE`/`FALSE`；字符串可加 `_CHARSET` 前缀；二进制字面量在 unsigned 标志下输出十六进制，否则输出 bit literal。明确列出的尚未支持 kind 返回 `io::Error("not implemented")`，其余未知 kind 返回 `io::Error("can't format to string")`。

展示格式化时，`ValueExpr::Format` 走相似但不完全相同的分派：字符串/字节采用 `quote_go_string`，不处理字符集前缀；不支持的 kind 直接 panic。最终 `writer.write_all` 的结果被忽略，所以调用者无法从本接口获知写错误。

访问节点时，`Accept` 先把节点所有权交给 `Visitor::Enter`。若要求跳过子节点，直接把 Enter 返回的节点交给 Leave；否则必须将替换节点 downcast 回相应具体类型，类型不匹配即 panic，然后调用 Leave。`AcceptInPlace` 同样始终按 Enter 后 Leave 的顺序运行；由于两类节点都是叶子，skip 标志只影响控制分支，不会减少 Leave 调用。

参数占位符由 `ParamMarkerExpr::new(offset)` 创建，解析位置存入 `Offset`；后续可用 `SetOrder` 指定绑定顺序。它的 Restore 永远生成 `?`，不会泄漏已绑定值或执行期状态。

## 数据与状态

`ValueExpr` 的核心状态是动态 `Datum`、静态字段元信息 `FieldType` 和私有投影偏移。正常新建节点的偏移为 `-1`，但 `Default::default()` 的偏移是 Rust 整数默认值 `0`；测试或扩展代码若依赖“未投影”等于 `-1`，必须走 `new`/`newValueExpr`。已有 `ValueExpr` 被再次传给构造器时不会重新推断字符集、collation 或字段类型。

`ParamMarkerExpr` 保存四类状态：内含的 `value_expr`、词法源码偏移 `Offset`、绑定顺序 `Order` 和执行期标志 `InExecute`。本文件只主动设置 Offset 和 Order；没有读取或切换 InExecute 的逻辑，也没有利用内含 ValueExpr 输出实际参数值。

文件没有全局可变状态。唯一文件级静态数据来自 `GO_PRINTABLE_RANGES` 的编译期 include，用于纯读取的字符分类。Restore 的临时缓冲区、visitor 的节点所有权和 writer 引用均局限于一次调用。

## 依赖与调用关系

上游边界如下：

- `pkg/parser/test_driver/lib.rs` 声明并再导出本模块，同时使 `Datum`、`DefaultTypeForValue`、`MyDecimal`、format、mysql、charset 和 types 可由 `use crate::*` 使用。
- `pkg/parser/yy_parser.rs::New -> parser_test_driver::init_test_driver` 是 RustCodeGraph 与源码共同确认的主 parser 调用边；当前调用没有运行时效果，只保证依赖被显式触达。
- `pkg/parser/test_driver/migration_aster_unit_test.rs`、`test_driver_test.rs` 和 `go_merge_35_test.rs` 直接构造并验证本文件的节点与访问者。
- RustCodeGraph 的文件级关系还列出 `pkg/parser/ast/lib.rs` 与本文件有关；源码核对表明该文件拥有独立的 `ValueExpr`/`NewValueExpr` 实现，不能据此推断它调用了本文件的同名构造器。

主要下游依赖如下：

- 构造链调用同 crate 的 `DefaultTypeForValue` 与 `Datum::SetValue`，实现位于 `test_driver_datum.rs`。
- Restore 使用 `parser_format::RestoreCtx`（经 crate 根别名 `format`）、`parser_mysql` 的 flag/字符集常量、`parser_types::FieldType` 以及 `hex::encode`。
- 二进制字面量输出委托 `BinaryLiteral::ToBitLiteralString`；十进制输出委托 `MyDecimal::String`。
- Format 的字符串分支调用本文件 `quote_go_string`，浮点分支调用 `format_float`。

RustCodeGraph 对 `Restore`、`Accept` 等常见重名方法产生了跨文件误配候选；本文只采用能由目标源码和具体相邻文件复核的调用边，不采用那些歧义边作为架构结论。

## 错误处理与边界

`Restore` 的可恢复错误通过 `io::Result` 返回：Duration、Enum、Bit、Set、Time、Interface、MinNotNull、MaxValue、Raw、JSON 当前统一返回 `not implemented`，未知 kind 返回 `can't format to string`。RestoreCtx 的写错误通过 `?` 传播，`RestoreToString` 继续向上传递该错误。

`RestoreToString` 最后用 `String::from_utf8(...).expect(...)`，其安全性依赖 RestoreCtx 和本文件只写入有效 UTF-8 文本这一不变量；如果未来允许写入任意字节，该 expect 会成为 panic 边界。

`Format` 的契约更弱：不支持的 kind panic，`ParamMarkerExpr::Format` 对所有调用都 panic，且 ValueExpr 的 writer 错误被忽略。需要可靠 I/O 错误传播的调用者应使用 Restore 路径，或在扩展时改变接口并同步所有调用者。

`Accept` 信任 Enter 返回同类具体节点；未 skip 时若 ValueExpr/ParamMarkerExpr 类型不匹配会 panic。skip 分支不做 downcast，允许 visitor 直接把任意替换节点传入 Leave，这与当前动态 `Any` 接口一致。动态 Datum 的具体值、FieldType flag 与 Kind 若不一致，格式化结果可能不符合预期，本文件没有额外一致性校验。

浮点格式化显式覆盖 NaN 和正负无穷；32 位路径先执行 f32 舍入再转回 f64。字符串转义基于生成的 Go 可打印字符表，修改或移动 include 文件会直接影响输出兼容性。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。所有方法只操作调用者独占的 `&mut self`、拥有所有权的 `Box`，或只读的 `&self`，因而内部没有共享可变状态。

拥有所有权的 `Accept` 会消费 `Box<Self>`，Enter 可以替换该 Box，最终所有权随 Leave 的返回值交回调用者；中途 downcast 失败时替换节点随 panic 展开被释放。`AcceptInPlace` 在同一借用期内把节点依次交给 Enter 和 Leave，不发生节点移动。`RestoreToString` 的 Vec 缓冲和 RestoreCtx 在函数内创建，ctx 先结束借用，再把缓冲转换为 String。

类型是否可跨线程由 `Datum` 内部的动态载荷及 trait object 约束决定；`Box<dyn Any>`、`Visitor` 和 `InPlaceVisitor` 均未声明 `Send`/`Sync`，所以本文件不承诺节点或 visitor 可在线程间传递。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/parser/test_driver/test_driver.go`。Rust 保留了 Go 的两个节点、Restore/Format、投影偏移、参数 Offset/Order 以及 visitor Enter/Leave 流程。`migration_aster_unit_test.rs` 验证了布尔、字符串、浮点、十六进制、不支持 kind、已有 ValueExpr 复用和替换 visitor；`test_driver_test.rs` 验证 Go `strconv.Quote` 的 `\a`/`\v`；`go_merge_35_test.rs` 验证两个叶子节点无论 skip 与否均调用 Enter 和 Leave。Go 的 `accept_in_place_test.go` 还以源码结构测试约束所有生产 AcceptInPlace 方法必须检查 skipChildren 并调用 Leave。

关键差异包括：

- Go `init` 会把构造函数注册到 `ast.NewValueExpr`、`ast.NewParamMarkerExpr` 等包级变量；Rust `init_test_driver` 当前为空，主 AST 在 `pkg/parser/ast/lib.rs` 有独立静态实现。
- Go 节点实现 AST 接口并嵌入 `ast.TexprNode`；本文件用 `Any` 和本地 Visitor trait 保持近似形状，未实现 `pkg/parser/ast/expressions.rs` 中的 AST trait。
- Go 构造器返回接口/指针；Rust 返回拥有所有权的具体值。Go 对已有 `*ValueExpr` 返回同一指针，Rust 则从 Box 中移出值，保留内容但不保留可观察的指针身份。
- Go ParamMarkerExpr 匿名嵌入 ValueExpr；Rust 使用命名字段 `value_expr`，没有 Go 的方法提升。
- Rust 额外提供 `RestoreToString` 便利方法以及显式的 Go 浮点/字符串兼容辅助函数。
- Rust 的 `ValueExpr::Default` 与正常构造在 projection offset 上不同；Go 通常由构造器显式设为 `-1`。

因此，迁移状态应描述为“测试驱动核心行为已有覆盖，但与完整 AST trait/注册机制仍分离”，而不是宣称 Go 驱动已被 Rust 等价替换。

## 扩展指南

新增 Datum kind 的 SQL 还原时，应同时修改 `ValueExpr::Restore` 和 `ValueExpr::Format`，检查 `DefaultTypeForValue`、Datum getter、字符集/flag 语义，并在独立的 `pkg/parser/test_driver/*_test.rs` 中添加 Restore 与 Format 边界用例；不要把测试内嵌回生产源文件。若输出需与 Go 一致，应先对照 `test_driver.go` 及 Go 的 `strconv`/format 行为，特别关注特殊浮点、Unicode 可打印性、二进制前缀和 RestoreFlags。

扩展 visitor 时必须保留叶子节点的 Enter/Leave 顺序、skip 语义和替换节点类型约束。若把本文件接入完整 Rust AST，应优先复用现有 `parser_ast` trait 和节点实现，明确解决与 `pkg/parser/ast/lib.rs::ValueExpr` 的重复类型，而不是新增第三套转换层；同时更新 `yy_parser.rs::New` 的初始化说明及 AST/driver 两侧的独立测试。

若实现 `ParamMarkerExpr::Format`、InExecute 行为或实际参数输出，需要明确是否仍应隐藏绑定值，并对预处理语句的 Offset/Order 生命周期增加测试。若改变 `Format` 的错误处理，需评估 API 兼容性；若让动态节点跨线程，则必须审计 Datum 载荷并显式引入 `Send`/`Sync` 约束。

新增依赖或改变 crate 边界时同步检查 `pkg/parser/test_driver/Cargo.toml` 和 `pkg/parser/Cargo.toml`。本任务只记录现状，不修改 Rust、Go 或 Cargo。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件和 307,296 个节点；`files --filter pkg/parser/test_driver` 列出本 crate 的 Rust/Go 源与测试。
- RustCodeGraph `node --file pkg/parser/test_driver/test_driver.rs --offset 1 --limit 500`：读取目标文件全部 331 行，确认 32 个符号以及文件级使用者 `pkg/parser/ast/lib.rs`、`pkg/parser/test_driver/go_merge_35_test.rs`、`pkg/parser/yy_parser.rs`。
- RustCodeGraph `query "test_driver.rs" --limit 50` 与 `query newValueExpr --json --limit 20`：确认公开/私有符号集合，并区分 Go、test_driver Rust、types/parser_driver Rust 和 ast Rust 的同名构造器。
- RustCodeGraph callers/callees 查询：确认图对常见同名方法存在消歧限制；有效的本地被调关系包括 `ValueExpr::new -> DefaultTypeForValue/SetValue`、Restore/Format -> `format_float`、Format -> `quote_go_string`，其余结论均再由源码核验。
- 已读生产与配置路径：`pkg/parser/test_driver/test_driver.rs`、`lib.rs`、`Cargo.toml`、`test_driver.go`、`pkg/parser/Cargo.toml`、`pkg/parser/yy_parser.rs`、`pkg/parser/ast/lib.rs`。目标目录没有 `doc.go`，因此以 crate 根 `lib.rs` 作为最近的包契约。
- 已读独立测试：`pkg/parser/test_driver/test_driver_test.rs`、`migration_aster_unit_test.rs`、`go_merge_35_test.rs`、`accept_in_place_test.go`。
- 人工复核重点：Rust 初始化为空、AST 存在独立实现、正常构造偏移为 `-1`、Restore/Format 的错误差异、visitor 的替换与 skip 分支、无并发资源，以及 Go/Rust 所有权和注册机制差异。
- 本任务为纯文档分析，按计划不运行 Cargo；结构验证要求本文恰含上述 11 个固定二级标题。
