# `pkg/parser/mview_stmt_options.rs`

## 文件定位

该文件属于 `astersql-parser` crate 的解析器内部实现。`pkg/parser/Cargo.toml` 将 crate 的库入口设为 `lib.rs`；`pkg/parser/lib.rs` 又在私有实现模块 `parser_impl` 中通过 `include!("mview_stmt_options.rs")` 展开本文件。因此这里的两个结构体与词法/语法分析器、生成的语法表以及 `parser_actions` 处于同一模块作用域，而不是独立的公开模块。

本文件只定义 `mviewCreateOptions` 和 `mlogCreateOptions` 两个私有中间状态类型，不定义函数、trait、impl、常量或条件编译项。它们不是最终 AST，也不直接执行 SQL；其作用是让物化视图相关语法归约在构造最终 `parser_ast` 节点之前暂存并合并建表选项。实际归约逻辑位于 `pkg/parser/parser_actions/mview.rs::apply`。

## 核心职责

- `mviewCreateOptions` 汇集 `CREATE MATERIALIZED VIEW` 的 `COMMENT`、`SHARD_ROW_ID_BITS` 和 `PRE_SPLIT_REGIONS`。布尔字段记录某类选项是否已经出现，用于识别重复项；`comment` 保存注释文本；`options` 按 SQL 中出现的顺序保存需进入最终 AST 的 `ast::TableOption`。
- `mlogCreateOptions` 汇集 `CREATE MATERIALIZED VIEW LOG` 的 `SHARD_ROW_ID_BITS` 和 `PRE_SPLIT_REGIONS`。它不含注释字段，因为对应语法不接受 `COMMENT`。
- 两个类型都派生 `Default`，为空产生式提供“尚未出现任何选项”的初始状态：布尔值为 `false`、字符串为空、向量为空。

这些职责可由本文件字段、`pkg/parser/parser.y` 中的 `MViewTableOptionListOpt`/`MLogCreateOptionListOpt` 产生式，以及 `pkg/parser/parser_actions/mview.rs::apply` 的同名归约分支相互核对。

## 主要符号

`mviewCreateOptions`（私有结构体）：

- `hasComment: bool`：标记 `COMMENT` 是否已出现；它与空字符串不同，因而即使 `COMMENT = ''` 也能被判定为已经设置。
- `comment: String`：保存注释字面量，最终写入 `ast::CreateMaterializedViewStmt::Comment`。
- `hasShardRowIDBits: bool`：标记 `SHARD_ROW_ID_BITS` 是否已出现。
- `hasPreSplitRegion: bool`：标记 `PRE_SPLIT_REGIONS` 是否已出现。
- `options: Vec<ast::TableOption>`：保存两个数值表选项；最终移动到 `ast::CreateMaterializedViewStmt::Options`。注释不放入该向量，而是通过独立字段进入最终 AST。

`mlogCreateOptions`（私有结构体）：

- `hasShardRowIDBits: bool` 与 `hasPreSplitRegion: bool`：分别承担物化视图日志选项的重复检测状态。
- `options: Vec<ast::TableOption>`：最终移动到 `ast::CreateMaterializedViewLogStmt::Options`。

两者均没有显式可见性修饰符，也没有在 `pkg/parser/lib.rs` 中再导出，因此不是 crate 的公共 API。命名沿用 Go 版本；crate 根部的 `#![allow(non_camel_case_types, non_snake_case)]` 允许这种移植命名。

## 执行流程

1. 调用者通过 `New().ParseOneStmt(...)` 进入 parser；生成的主语法表把物化视图产生式交给 `pkg/parser/parser_actions/mview.rs::apply`。
2. 空的 `MViewTableOptionListOpt` 或 `MLogCreateOptionListOpt` 分支用 `Default::default()` 创建空累加器；非空分支转交已有累加器。
3. 单个物化视图选项由 `mviewtableoption_` 分支构造成 `mviewCreateOptions`：注释写入 `hasComment`/`comment`，数值选项则设置对应存在标志并追加一个 `ast::TableOption`。日志选项由 `mlogcreateoption_` 分支以相同模式构造 `mlogCreateOptions`。
4. 列表归约把右侧累加器合并到左侧累加器。若同类存在标志在左右两边同时为真，parser 追加重复选项错误；随后仍按左到右顺序扩展 `options`，由解析器统一返回已记录的错误。
5. `creatematerializedviewstmt_` 分支取出 `mviewCreateOptions`，把 `comment` 与 `options` 移入 `ast::CreateMaterializedViewStmt`；`creatematerializedviewlogstmt_` 则把 `mlogCreateOptions::options` 移入 `ast::CreateMaterializedViewLogStmt`。存在标志只服务于归约期校验，不进入最终 AST。

`pkg/parser/go_merge_33_test.rs::go_merge_33_materialized_view_options_and_log` 验证了两个数值选项按输入顺序进入最终 AST，也验证了日志选项和注释的落地结果。

## 数据与状态

这两个结构体是单次语法归约过程中的短生命周期、拥有所有权的值对象。`String` 与 `Vec<ast::TableOption>` 都由累加器拥有；`parser_actions/mview.rs::apply` 通过 `Box<dyn Any>` 在语法符号槽位中传递它们，再以 `downcast` 取回具体类型并移动字段，不依赖借用外部字符串或全局状态。

关键不变量如下：

- `hasComment == true` 表示语法中确实出现过注释，不能用 `comment.is_empty()` 替代。
- 每个 `has*` 标志只描述对应种类是否出现，不描述数值是否合法；数值的词法/语法约束由 `LengthNum` 等产生式负责。
- `options` 的追加顺序必须保持输入顺序，因为最终 AST 的还原输出与后续消费者都能观察该顺序。
- 默认状态必须表示“零选项”，以支持合法的无选项 `CREATE MATERIALIZED VIEW ... AS ...` 和无选项日志语句。

## 依赖与调用关系

直接类型依赖只有 `ast::TableOption`。在 `pkg/parser/lib.rs::parser_impl` 中，`use parser_ast as ast` 提供该别名；`pkg/parser/Cargo.toml` 通过本地路径依赖 `parser-ast = { package = "astersql-parser-ast", path = "ast" }` 声明 crate 边界。

上游关系为：

- `pkg/parser/lib.rs::parser_impl` 通过 `include!` 装入本文件。
- `pkg/parser/parser_actions/mview.rs` 从父模块引入两个结构体。
- `parser_actions::mview::apply` 在物化视图选项和最终语句相关归约分支中创建、合并并消费它们。RustCodeGraph 将 `apply` 记录为这些类型的实例化使用者。

下游关系为：

- `mviewCreateOptions::comment` 和 `options` 分别进入 `ast::CreateMaterializedViewStmt::Comment` 与 `Options`。
- `mlogCreateOptions::options` 进入 `ast::CreateMaterializedViewLogStmt::Options`。
- `hasComment`、`hasShardRowIDBits`、`hasPreSplitRegion` 终止于解析阶段，只驱动重复检测，不向 AST 或执行层传播。

## 错误处理与边界

本文件自身没有方法，因此不直接创建或返回错误。错误语义由使用这些状态的 `parser_actions/mview.rs::apply` 实现：物化视图重复 `COMMENT`、重复 `SHARD_ROW_ID_BITS` 或重复 `PRE_SPLIT_REGIONS`，以及物化视图日志重复后两者时，会通过 lexer 追加带具体语句类型的错误消息。

`downcast` 失败等内部语义值类型不匹配时，相关列表归约返回 `Ok(false)`，由更上层的 action 分派处理；构造最终语句时缺失或类型错误的累加器会退回空的默认值。这是归约框架的防御边界，不代表语法层会接受任意缺失项。

选项的相对位置由 `pkg/parser/parser.y` 的产生式限定：物化视图表选项位于刷新子句和属性子句之前。`pkg/parser/parser_test.go::TestMaterializedViewCreateOptionOrder` 与 Rust 对应测试 `pkg/parser/go_merge_35_test.rs::go_merge_35_materialized_view_option_errors` 验证刷新/属性之后再放表选项、重复刷新或重复属性会解析失败。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。两个累加器随单次 parser 归约创建，在语法符号栈中以拥有所有权的装箱值传递，最终字段被移动进 AST 后即结束生命周期；未被消费的值由 Rust 自动释放。

类型没有 `pub`、没有共享引用，也未显式实现 `Send`/`Sync`。是否可并发使用 parser 由 parser 实例及其 lexer 状态决定，不能由这两个普通数据结构推导出跨线程共享保证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/mview_stmt_options.go`。Rust 的两个结构体保留了 Go 的类型名和字段名，并保持字段集合一致：

- Go `string` 对应 Rust `String`。
- Go `[]*ast.TableOption` 对应 Rust `Vec<ast::TableOption>`；Rust 向量直接拥有表选项值，而不是保存指针，但对当前的构造、顺序追加和最终 AST 传递语义等价。
- Go 零值与 Rust `#[derive(Default)]` 均产生 false/空字符串/空列表状态。

Go 的核心语义依据位于 `pkg/parser/parser.y`，生成结果可在 `pkg/parser/parser.go` 的相应 action 中看到；Rust 将同样的逻辑手工落在 `pkg/parser/parser_actions/mview.rs::apply`。Go 测试 `pkg/parser/parser_test.go::TestMaterializedViewDDLStatements`、`TestMaterializedViewDuplicateOptionsErrMsg`、`TestMaterializedViewCreateOptionOrder` 和 `TestMaterializedViewLogCreatePurgeClauseSyntax`，分别在 Rust 的 `pkg/parser/go_merge_35_test.rs` 中有还原与错误行为对照；`pkg/parser/go_merge_33_test.rs` 还直接断言最终 AST 字段和选项顺序。

## 扩展指南

新增物化视图建表选项时，不能只向本文件加字段。应按语义同步检查和修改：

1. 若选项需要重复检测，在对应累加器增加独立的 `has*` 标志；若需要把非 `TableOption` 数据直接写入最终 AST，再增加拥有该值的字段。
2. 在 `pkg/parser/parser.y` 增加或调整 Go 语法及 action，并在 `pkg/parser/parser_actions/mview.rs::owns`/`apply` 的 Rust 分派中同步单项构造、列表合并、错误文本和最终 AST 落地。
3. 若该值属于通用表选项，同步 `pkg/parser/ast` 中的 `TableOption` 类型、还原逻辑及其独立测试；保持 `options` 的输入顺序。
4. Rust 测试必须继续放在独立测试文件中。优先扩展 `pkg/parser/go_merge_33_test.rs` 的 AST 字段断言和 `pkg/parser/go_merge_35_test.rs` 的 Go 对齐还原/错误用例；Go 侧同步扩展 `pkg/parser/parser_test.go` 的对应测试。

主要兼容风险是 Rust 与 Go 字段或重复检测规则漂移、错误文本变化、空字符串选项被误判为未出现，以及追加次序变化导致 SQL 还原不同。当前状态量很小，性能风险主要来自不必要的克隆；扩展时应沿用移动所有权和向量追加的模式。

## 验证依据

- 目标源码：`pkg/parser/mview_stmt_options.rs`，确认仅有两个派生 `Default` 的私有结构体及其字段。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引可用；`query mviewCreateOptions --kind struct`、`query mlogCreateOptions --kind struct` 和对应 `node` 定位 Rust/Go 定义；调用轨迹将 `pkg/parser/parser_actions/mview.rs::apply` 标为实例化使用者；按文件读取该 action 核对构造、合并、报错及 AST 落地分支。
- crate 与装配：`pkg/parser/Cargo.toml`、`pkg/parser/lib.rs`。
- Rust 直接实现：`pkg/parser/parser_actions/mview.rs::apply`。
- Go 对照：`pkg/parser/mview_stmt_options.go`、`pkg/parser/parser.y`、生成的 `pkg/parser/parser.go`。
- Rust 测试：`pkg/parser/go_merge_33_test.rs::go_merge_33_materialized_view_options_and_log`、`pkg/parser/go_merge_35_test.rs::go_merge_35_materialized_view_restore`、`go_merge_35_materialized_view_option_errors`。
- Go 测试：`pkg/parser/parser_test.go::TestMaterializedViewDDLStatements`、`TestMaterializedViewDuplicateOptionsErrMsg`、`TestMaterializedViewCreateOptionOrder`、`TestMaterializedViewLogCreatePurgeClauseSyntax`。
- 本任务为纯文档分析，按计划不运行 Cargo；验证限于索引查询、源码/语法/Cargo/测试交叉核对和文档结构检查。
