# `pkg/parser/parser_value.rs`

## 文件定位

本文件是 `astersql-parser` crate 的主 SQL 解析器语义值基础层。它不单独声明模块，而是由 [`parser.rs`](parser.rs) 通过 `include!("parser_value.rs")` 嵌入 `parser_impl`，随后同一作用域继续嵌入语义辅助函数和 LR 运行时，并声明 `parser_actions`。因此这里的类型可以被词法器、移进/规约状态机和各类命名语义动作直接共享，但没有形成独立的公共 crate 模块。

文件头标明它属于 goyacc 生成解析器产物；当前 Rust 拆分把 Go `parser.go` 中的 `yySymType` 和 Rust 动作分发需要的 `Rhs` 集中到此处。crate 边界由 [`Cargo.toml`](Cargo.toml) 确认：包名是 `astersql-parser`，库入口为 `lib.rs`，AST 类型来自本地依赖 `astersql-parser-ast`。

## 核心职责

1. `yySymType` 为 LR 栈中的每个终结符、非终结符同时保存解析状态和语义载荷，承接词法器输出，并在规约时逐步形成 AST。
2. `Default` 为新栈槽、lookahead 值和错误恢复值提供确定的空状态；动态载荷为空，字符串为空，状态/偏移为零，而 `semantic_complete` 初始为真。
3. `Rhs<'a>` 把一次规约的右部栈切片包装为一基、从左到右的 `$1..$n` 访问接口，使 Rust 语义动作保持与 yacc 动作位置语义一致。
4. `take` 与 `take_default` 通过移动而非克隆取出动态 AST/`Any` 载荷；这既适配不可克隆的 trait object，也实现无显式动作时 goyacc 的默认 `$$ = $1` 语义。

## 主要符号

- `pub struct yySymType`：解析栈语义值。类型在 `parser_impl` 内可见，但字段全部私有；外部调用者不能直接构造或读取字段。字段含义如下：
  - `yys: isize`：该栈槽关联的 LR 状态。`parser_runtime.rs::yyParse` 在移进、goto 和错误恢复时写入并读取它。
  - `offset: i32`：词元/规约值在原 SQL 中的偏移。`lexer.rs::Lex` 写入词元偏移，规约时从首个右部值继承，之后 `yySetOffset` 可把它写入表达式节点的原文位置。
  - `item: Option<Box<dyn Any>>`：异构中间值通道，保存选项、集合、字面量和其他不能统一成单一静态类型的语义对象。语义动作按预期类型 `downcast`。
  - `ident: String`：标识符及部分词元文本；由词法器写入，也由动作组合或传递。
  - `expr: Option<parser_ast::ExprNode>`：表达式语义值。
  - `statement: Option<Box<dyn parser_ast::Node>>`：语句或其他以 `Node` trait object 表示的 AST 值。
  - `semantic_complete: bool`：Rust 语义动作覆盖完整性沿规约树传播的标记。`StatementList` 动作最终把它汇入 `Parser::allStatementsSemanticallyComplete`。
- `impl Default for yySymType`：初始化全部空载荷，并把 `semantic_complete` 设为 `true`。`item` 的默认空值由 `parser_3_empty_semantic_values_do_not_allocate_items` 固定。
- `pub(crate) struct Rhs<'a>`：独占借用 `&'a mut [yySymType]`，因此一次只能有一个包装者修改该规约右部。
- `Rhs::new`：从运行时计算出的右部切片建立包装。
- `Rhs::len`：返回右部符号数。
- `Rhs::borrow` / `borrow_mut`：把一基位置转换为 Rust 零基下标；位置为 `0` 时 `checked_sub` 失败，越过末尾时 `get`/`get_mut` 返回 `None`。
- `Rhs::take`：以 `std::mem::take` 移出指定槽的完整 `yySymType`，原槽恢复为 `Default`，避免复制动态对象。
- `Rhs::take_default`：消费包装并尝试取 `$1`；空右部或异常情况下返回默认语义值。
- `Rhs::as_mut_slice`：把内部可变切片交给需要批量访问的 crate 内逻辑；它不改变索引或所有权语义。当前精确引用搜索未发现本文件外调用者，属于已提供但尚未被现有动作采用的 crate 内接口。

## 执行流程

1. `lexer.rs::Lex` 接收一个默认 `yySymType`，填写 `offset`、`ident`，并对字面量等 token 填写 `item`；数值转换辅助函数也会把解析结果放入 `item`。
2. `parser_runtime.rs::yyParse` 移进 token 时，用 `mem::take` 把 `Parser::yylval` 移入复用的 `Vec<yySymType>` 栈，并在该值的 `yys` 中记录下一状态。
3. 发生规约时，运行时依据生成表计算 `pop_count` 和 `value_start`，对 `values[value_start..]` 建立 `Rhs`。它先合取所有右部值的 `semantic_complete`，并预置规约结果的首项偏移和标识符。
4. `parser_actions::apply` 根据稳定 `RuleId` 把 `Rhs` 和输出 `Context` 分派到 DDL、DML、表达式、查询、安全、管理等动作。`parser_actions/mod.rs` 还为 `Rhs` 实现 `Index`/`IndexMut`，有效位置可直接按 yacc 风格索引；无效位置会以 `expect("semantic RHS position")` 终止。
5. 若规则没有被命名动作处理且右部非空，运行时调用 `take_default`，把 `$1` 整体移动为 `$$`；若右部为空，则保留默认值。之后运行时截断旧右部、计算 goto 状态、写入偏移/状态，并把规约结果压回栈。
6. 语句列表动作从右部值取走 `statement`，同时把 `semantic_complete` 汇总到解析器级状态；最终 AST 因而携带了前序词法值与多轮规约构造的结果。

## 数据与状态

`yySymType` 是状态机控制数据与语义数据的共同容器。它位于 `Parser::cache: Vec<yySymType>` 中；解析开始时缓存被取出并清空，结束时再放回 `Parser`，所以同一个解析器可复用栈容量。`parser_3_reuses_stack_storage_for_long_insert` 以 1001 行 `INSERT` 验证第二次解析复用相同指针和容量。

`item` 使用运行时类型擦除，所有权属于当前栈槽；动作通常通过 `take` 或字段上的 `Option::take` 消费它，再用 `downcast::<T>()` 恢复具体类型。`expr` 与 `statement` 使用明确的 AST 类型，后者仍以 trait object 保留不同语句节点的多态性。`Rhs` 自身只保存临时借用，不复制切片、不缓存下标，也不会在规约之外存活。

`semantic_complete` 是 Rust 移植额外维护的布尔状态：默认值和词法值视为完整；规约先继承全部子值的合取结果，未执行一个被清单标为需要动作的规则时会把结果置为不完整。该值不替代语法错误，而是记录语义动作覆盖状态。

## 依赖与调用关系

上游组装与调用关系为：

- `lib.rs::parser_impl` 嵌入 `yy_parser.rs`、生成表和 `parser.rs`；`parser.rs` 再嵌入本文件。
- `lexer.rs::{Lex, handleIdent}` 以及 `yy_parser.rs` 的字面量转换辅助函数生产 `yySymType`。
- `parser_runtime.rs::{yylex1, yyParse}` 创建、移动、读取语义值，并在规约点构造 `Rhs::new`。
- `parser_actions/mod.rs::apply` 及其 `admin`、`ddl`、`dml`、`expression`、`misc`、`mview`、`query`、`security` 子模块消费 `Rhs`；例如 DDL 和物化视图动作使用 `borrow_mut` 取走嵌套语义对象，语句列表动作读取 `semantic_complete`。
- `parser_semantic_support.rs::take_subquery_statement` 展示了典型动态值路径：从 `yySymType.item` 取出值，向下转型为 `SubquerySemantic`，再取出查询节点。

直接下游依赖包括标准库的 `Any`、切片访问与 `std::mem::take`，以及 `parser_ast::{ExprNode, Node}`。`Any` 的导入位于同一 `include!` 作用域中的 `parser_semantic_support.rs`；这是文本包含式组装带来的跨文件作用域依赖。RustCodeGraph 能识别本文件及 11 个符号，但对 `Rhs::take` 等短方法名的调用边产生全仓库同名歧义，因此以上直接边由精确引用搜索和相邻源码复核补足。

## 错误处理与边界

- `borrow`、`borrow_mut`、`take` 对 `$0` 和超过 `len` 的位置返回 `None`，不会发生整数下溢或切片越界；`checked_sub(1)` 是 `$0` 的第一道边界。
- `take_default` 在空右部时返回 `yySymType::default()`，所以调用者无需为 epsilon 规约制造虚假 `$1`。
- 动作代码通过 `Index`/`IndexMut` 访问时把位置有效性视为生成文法的不变量，违反不变量会 panic；可选位置访问应直接使用本文件的 `borrow*` API。
- `item` 的具体类型不由类型系统静态约束。错误的 `downcast` 在不同动作中可能转成 `None`、回退值或 `expect` panic；扩展动作时必须让生产方与消费方的具体类型完全一致。
- `take` 会把源槽重置为默认值。重复消费同一槽不会再次取得原对象；这一移动语义是防止 trait object 被复制或双重拥有的关键约束。
- 本文件不生成解析错误；语法表、goto、栈不变量和恢复错误由 `parser_runtime.rs::yyParse` 通过 `yyLexer::AppendError` 处理。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道或 I/O。`Rhs<'a>` 的可变借用把规约期间的并发修改排除在编译期；它不能逸出被借用的 `values` 切片。`yySymType` 也没有声明 `Clone`，动态 `Any` 和 AST trait object 依靠所有权移动完成生命周期交接。

资源生命周期是“词法值 → 解析栈槽 → 规约右部 → 规约输出/最终 AST”。`mem::take` 在移动后留下安全的默认值，栈截断随后释放未被转移的动态对象；解析结束时 `values.clear()` 释放残留语义载荷但保留 `Vec` 容量，再将空缓存放回 `Parser`。因此缓存复用不等于复用上一次解析的 AST 对象。

`yyDebug` 等全局解析器调试状态属于 `parser_runtime.rs`，不在本文件管理范围。单个 `Parser` 的可变解析调用也要求调用者独占借用；本文件没有额外提供跨线程共享保证。

## 与 Go 版本的对应关系

Go 对照位于 [`parser.go`](parser.go)。Go `yySymType` 同样含 `yys`、`offset`、`item`、`ident`、`expr`、`statement`：Rust 将 Go `int` 映射为 `isize`/`i32`，将 `interface{}` 映射为 `Option<Box<dyn Any>>`，将可空接口型 AST 映射为 `Option` 和 trait object。Rust 的 `semantic_complete` 是当前 Rust 命名动作迁移用于传播覆盖完整性的附加字段，Go 结构中没有该字段。

Go 运行时以 `yyS` 栈、`yypt` 顶点和 `yyS[yypt-k]` 访问规约右部，并令 `parser.yyVAL` 指向规约结果槽；Rust 则以切片包装 `Rhs`，把同一位置关系表示为一基 `$1..$n`，动作层的 `rhs_len - k` 对应 Go 的 `yypt-k`。Go 栈槽赋值可复制接口头，Rust 不能复制 `Box<dyn Any>`/AST trait object，因此用 `take` 明确转移所有权。

Go 的生成运行时通过复用 `parser.cache` 扩容后的 `[]yySymType` 降低分配；Rust `yyParse` 同样取出并归还 `Parser.cache`。Rust 的 `take_default` 显式实现 goyacc 无动作产生式的默认值传递，并由独立单元测试固定其只移动第一个右部符号的行为。

## 扩展指南

- 新增一种可由统一 AST 类型表达的表达式或语句时，优先继续使用 `expr`/`statement`；只有需要跨多条产生式传递的异构辅助值才放入 `item`。生产动作和所有消费动作必须同步约定准确的 `T`，并覆盖错误/空分支。
- 修改 `yySymType` 字段或默认值时，应同步检查 `lexer.rs`、`yy_parser.rs`、`parser_runtime.rs`、`parser_semantic_support.rs` 和全部 `parser_actions/*`，并对照 Go `parser.go` 的结构与栈语义。不要移除现有 PingCAP、ql 或 AsterSQL 版权说明。
- 修改 `Rhs` 的位置规则会影响所有生成动作。必须保持 `$1` 是左端第一个符号、`$0` 无效、越界安全返回 `None`；若动作继续通过 `Index` 访问，还要保持其有效位置 panic 契约。
- 改动所有权传递时不得用浅复制或不受控共享替代 `take`。尤其要验证无动作规约的 `$$ = $1`、重复消费后的默认值、空右部和动态 `item` 向下转型。
- 测试逻辑应保持在独立文件。最直接的同步位置是 [`parser_runtime_aster_unit_test.rs`](parser_runtime_aster_unit_test.rs)，其中已有一基借用/移动和默认值移动测试；默认 `item` 分配、缓存复用和完整解析行为位于 [`parser_3_aster_unit_test.rs`](parser_3_aster_unit_test.rs)。文法右部长度与动作覆盖变化还应同步检查 [`parser_manifest_aster_unit_test.rs`](parser_manifest_aster_unit_test.rs) 及对应动作模块的独立测试。
- 该文件标为生成产物；若生成链能够产出它，应修改生成源/模板并重新生成，而不是让手工修改在下一次生成时丢失。当前证据只确认文件头和组装方式，未在本任务中验证具体生成命令。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引有效（11,467 个文件、307,296 个节点），索引时间晚于 `parser_value.rs`；`files --filter pkg/parser/parser_value.rs` 确认目标被索引且含 11 个符号；`node --file ... --offset 1 --limit 400` 核对了文件全部 81 行。`query yySymType`、`query Rhs --kind struct` 找到本文件类型；方法调用查询因同名歧义未给出可靠调用者，未将其结果当作调用证据。
- 源码与入口：[`parser_value.rs`](parser_value.rs)、[`parser.rs`](parser.rs)、[`lib.rs`](lib.rs)、[`parser_runtime.rs`](parser_runtime.rs)、[`parser_actions/mod.rs`](parser_actions/mod.rs)、[`parser_semantic_support.rs`](parser_semantic_support.rs)、[`lexer.rs`](lexer.rs)、[`yy_parser.rs`](yy_parser.rs)。精确引用搜索确认 `Rhs::new` 位于规约动作调用和默认值传递两条路径，并确认各动作模块对 `Rhs` 的使用。
- crate/Go 对照：[`Cargo.toml`](Cargo.toml) 与 Go 生成文件 [`parser.go`](parser.go)，重点核对 `yySymType` 字段、`yyS`/`yypt` 右部访问、规约结果槽和缓存复用。
- 独立测试：[`parser_runtime_aster_unit_test.rs`](parser_runtime_aster_unit_test.rs) 验证一基左右顺序、可变借用、越界、移动后默认值及 `$1` 默认传递；[`parser_3_aster_unit_test.rs`](parser_3_aster_unit_test.rs) 验证默认 `item` 不分配、生成动作覆盖与长 SQL 栈缓存复用；[`parser_manifest_aster_unit_test.rs`](parser_manifest_aster_unit_test.rs) 验证生成归约的右部长度和动作标记。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核本文只描述上述代码和测试可证实的当前行为。
