# `pkg/parser/ast/procedure.rs`

## 文件定位

本文件属于 `astersql-parser-ast` crate（见 [`Cargo.toml`](Cargo.toml) 的 `[package]` 与 `[lib] path = "lib.rs"`），由 [`lib.rs`](lib.rs) 通过 `pub mod procedure` 公开。它提供一套以 `ProcedureNode::restore() -> Result<String, String>` 为核心的存储过程 SQL 文本还原模型，覆盖参数、声明、复合语句、条件/循环、游标、处理器、标签和跳转。

需要区分两套同名类型：本文件中的类型位于 `astersql_parser_ast::procedure` 子模块；解析器主 AST 的 `ProcedureInfo`、`ProcedureLabelBlock`、`ProcedureJump` 等仍定义在 [`lib.rs`](lib.rs)，`parser_actions/misc.rs` 构造的是后者。因此，本文件目前是可独立组装和还原 SQL 的移植层，并不是 Rust 解析动作直接产出的主 AST 表示。RustCodeGraph 的文件查询显示本文件已被索引（587 行、89 个符号），但对本文件 `ProcedureInfo`/`ProcedureLabel` 的 callers 查询没有返回生产调用者；直接使用证据主要来自独立测试。

## 核心职责

- 用 `ProcedureNode` 统一所有过程片段的文本还原接口，并用 `Box<dyn ProcedureNode>` 递归组合异构子节点。
- 按 Go [`procedure.go`](procedure.go) 的 `Restore` 语义拼接关键字、分隔符和空格；`name()`、`string()` 分别负责反引号标识符与单引号字面量转义。
- 用 `DeclNode` 限制 `ProcedureBlock::procedure_vars` 只能放变量、游标或 handler 声明，用 `ErrNode` 限制 handler 条件类型。
- 在 `ProcedureLabel::restore` 中执行首尾标签一致性校验，并通过 `RestoreResult` 传播子节点失败。
- `RawProcedureNode` 为尚未统一到完整表达式/语句 AST 的内容提供原样文本叶节点；它不解析或验证传入 SQL。

本文件只建模和格式化语法，不负责解析 SQL、执行存储过程、访问 catalog、管理事务或验证过程语义。

## 主要符号

- 常量：`MODE_IN`、`MODE_OUT`、`MODE_INOUT` 表示参数方向；`PROCEDUR_CONTINUE`/`PROCEDUR_EXIT` 表示 handler 动作；`PROCEDUR_SQLWARNING`、`PROCEDUR_NOT_FOUND`、`PROCEDUR_SQLEXCEPTION`、`PROCEDUR_END` 表示 handler 条件类别或结束哨兵。拼写 `PROCEDUR_*` 保留 Go API。
- 核心抽象：`RestoreResult = Result<String, String>`；`ProcedureNode::restore` 是递归还原入口；`DeclNode`、`ErrNode` 是无新增方法的分类 trait；`LabelInfo` 暴露标签错误状态、名称、块类型和被包装节点。
- 辅助叶节点与函数：`RawProcedureNode` 原样返回内容；`name()` 将内部反引号加倍，`string()` 将内部单引号加倍；`restore_many()` 按顺序还原节点并给每项追加指定后缀。
- 定义与块：`StoreParameter` 输出参数模式、引用后的名称和类型字符串；`ProcedureDecl` 输出变量列表、类型及可选默认值；`ProcedureBlock` 先输出声明再输出过程语句；`ProcedureInfo`/`DropProcedureStmt` 分别输出 CREATE/DROP PROCEDURE。
- 分支：`ProcedureIfInfo`、`ProcedureIfBlock`、`ProcedureElseIfBlock`、`ProcedureElseBlock` 组成 IF 链；`SimpleCaseStmt`/`SimpleWhenThenStmt` 与 `SearchCaseStmt`/`SearchWhenThenStmt` 分别表示简单 CASE 和搜索 CASE。
- 循环与游标：`ProcedureWhileStmt`、`ProcedureRepeatStmt`；`ProcedureCursor`、`ProcedureOpenCur`、`ProcedureFetchInto`、`ProcedureCloseCur`。
- handler 条件：`ProcedureErrorControl` 组合动作、条件列表与处理语句；`ProcedureErrorVal`、`ProcedureErrorState`、`ProcedureErrorCon` 分别表示数字错误码、SQLSTATE 和预定义条件类。
- 标签与跳转：`ProcedureLabel` 保存起止名称、主体和 `is_block`；`ProcedureLabelBlock`/`ProcedureLabelLoop` 是实现 `ProcedureNode` 的包装器；`ProcedureJump` 根据 `is_leave` 输出 LEAVE 或 ITERATE。
- `ProcedureDeclInfo`、`ProcedureErrorCondition` 是与 Go 结构名对齐的零大小占位类型，当前不承载字段，也未嵌入其他节点。

## 执行流程

1. 调用方直接构造本模块节点树；叶子通常是 `RawProcedureNode`，组合节点以 `Box<dyn ProcedureNode>`、`Box<dyn DeclNode>` 或 `Box<dyn ErrNode>` 持有子节点。
2. 对根节点调用 `restore()`。例如 `ProcedureInfo::restore` 先生成 `CREATE PROCEDURE` 和可选 `IF NOT EXISTS`，依次调用 `StoreParameter::restore`，再调用过程体的动态分派 `restore`。
3. `ProcedureBlock::restore` 严格先还原 `procedure_vars`，再还原 `procedure_proc_stmts`；每个非空分段中的元素后追加分号，最终包入 `BEGIN ... END`。
4. IF/CASE/循环节点先还原条件或分支，再通过 `restore_many(..., ";")` 还原语句列表。任一子节点返回 `Err` 时，`?` 立即终止父节点并向上传播。
5. 游标和 handler 节点将保存的名称、条件与动作按固定顺序拼接。handler 多条件以 `", "` 分隔。
6. 标签节点先还原主体并形成起始标签文本，然后比较 `label_name` 与 `label_end`；不相等则返回错误，相等才输出结束标签。`ProcedureJump` 不依赖子节点，其固有 `restore() -> String` 通过 trait 实现完成格式化。

这些步骤只描述本文件的直接调用链。解析主链则从 `parser_actions/misc.rs` 的 `CreateProcedureStmtAlt01` 等规则构造 [`lib.rs`](lib.rs) 中的主 AST 类型，并由 [`walk.rs`](walk.rs) 和 [`sem.rs`](sem.rs) 提供遍历子节点与 `ProcedureCommand` 分类；它不会转换成本文件的同名还原节点。

## 数据与状态

节点均是调用方拥有的普通 Rust 值，没有全局可变状态。组合关系通过拥有型 `Box`/`Vec`/`Option` 表达：过程体和条件是单一所有权子树，语句/声明/条件集合保留插入顺序，可选 ELSE 或 DEFAULT 用 `Option` 表示。

`param_status`、`control_handle`、`error_con` 使用 `i32` 而非封闭枚举，以保持 Go `iota` 整数形态；未知值不会报错，而是输出空模式、空控制字或空条件文本。`param_type`、`decl_type` 是已格式化字符串，模块不验证它们是否为合法 SQL 类型。`procedure_param_str` 当前保存在 `ProcedureInfo` 中但不参与 `restore`。`is_block` 仅由 `LabelInfo::is_block` 暴露；`ProcedureLabel::new` 固定设为 `true`，循环包装器若需反映循环属性必须由调用方直接构造内部 `ProcedureLabel`。

## 依赖与调用关系

- crate 边界：[`Cargo.toml`](Cargo.toml) 声明该 crate 依赖 `parser-auth`、`parser-charset`、`parser-mysql`、`parser-types`、`serde`、`serde_json` 和 `url`；但本文件自身只使用 Rust 标准库的字符串、集合、trait object 和 `Result`，没有直接使用这些外部 crate。
- 模块入口：[`lib.rs`](lib.rs) 的 `pub mod procedure` 使本模块可公开访问，并仅在 `cfg(test)` 下编入 [`procedure_test.rs`](procedure_test.rs)。
- 本文件内部下游：所有组合节点最终调用 `ProcedureNode::restore`；`ProcedureDecl` 还调用 `name()` 并给默认表达式错误加上下文，CASE/IF/循环调用 `restore_many()`，标签和跳转调用 `name()`/`string()`。
- 直接测试调用者：[`procedure_test.rs`](procedure_test.rs) 构造并还原几乎所有节点；[`model_7_aster_unit_test.rs`](model_7_aster_unit_test.rs) 额外覆盖参数未知模式、LEAVE/ITERATE 和标签不匹配错误。
- 解析器主链（相邻但非本模块调用者）：[`parser_actions/misc.rs`](../parser_actions/misc.rs) 构造 [`lib.rs`](lib.rs) 的同名主 AST；[`walk.rs`](walk.rs) 声明其子节点遍历关系；[`sem.rs`](sem.rs) 将其分类为 `ProcedureCommand`。扩展时必须先决定改的是独立 restore 模型、主 AST，还是两者都要同步。

RustCodeGraph 对 `ProcedureNode`、`ProcedureInfo`、`ProcedureLabel` 的精确查询确认了本文件定义，并显示 `restore` 的内部边主要落在 trait 动态调用、`name` 和常量引用；由于大量仓库符号同名，按裸 `restore` 查询会产生跨模块歧义，不能据此宣称存在生产调用。

## 错误处理与边界

- `ProcedureDecl::restore` 将默认表达式错误包装为 `An error occur while restore expr: ...`；其他组合节点通常原样传播第一个子节点错误。
- `ProcedureLabel::restore` 是唯一主动进行结构一致性校验的节点：首尾标签不同返回含 begin/end 名称的错误。它会先调用主体 `restore`，因此主体错误优先于标签不匹配错误。
- `name()` 与 `string()` 只完成引用字符加倍；不会校验标识符、字符集 introducer、SQLSTATE 长度或错误码范围。
- 未知参数模式、handler 动作或预定义条件分别降级为空前缀/空关键字/空字符串，与当前 Go `switch` 无 default 输出的行为一致，但可能生成语义无效 SQL。
- 空块输出 `BEGIN  END`；空 handler 条件列表可形成双空格；空 CASE 分支、缺少 ELSE 等均按字段现状格式化，不做语法完整性检查。
- `RawProcedureNode` 会原样注入文本，调用者必须负责其可信性与语法正确性。`ProcedureJump` 使用单引号字符串格式化跳转名，而 Go 版通过 `WriteName` 生成标识符；当前 Rust 测试明确锁定了单引号输出，这是可见的移植差异，修改前需确认兼容目标。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、文件、网络连接、事务或外部资源。`restore(&self)` 只读节点，返回新分配的 `String`，因此没有跨调用缓存或隐式状态；能否在线程间共享取决于具体 trait object 是否满足调用方所需的 `Send`/`Sync`，本文件没有为 `ProcedureNode` 增加这些约束。

递归生命周期由 Rust 所有权管理：父节点销毁时其 `Box` 和 `Vec` 子树一并释放。性能主要取决于递归深度、节点数量以及重复 `String` 分配/拼接；`restore_many` 和多个 `collect::<Vec<_>>().join(...)` 会产生中间字符串。当前实现没有循环引用，也没有显式深度限制。

## 与 Go 版本的对应关系

Go 基准文件是 [`procedure.go`](procedure.go)，其类型集合和常量分组与本文件大体一一对应。参数方向、DECLARE/BEGIN-END、CREATE/DROP、IF/CASE、WHILE/REPEAT、游标、handler、标签的关键字顺序和空格主要按各 `Restore` 方法移植；[`procedure_test.go`](procedure_test.go) 通过真实 parser、visitor 和 round-trip restore 覆盖了广泛 SQL 形态。

当前 Rust 版并非完整替换：

- Go 节点实现统一的 `Node`/`StmtNode`、`Accept(Visitor)` 与 `format.RestoreCtx`；本文件改为较小的 `ProcedureNode` trait，没有 visitor、源码文本和格式上下文。
- Go 的类型字段使用 `*types.FieldType`、`ExprNode`、`StmtNode`、`TableName`；本文件以 `String` 和 `Box<dyn ProcedureNode>` 简化表示。表达式、名称和类型的合法性不在此处验证。
- Go `ProcedureBlock.Accept` 有意只遍历声明、不遍历过程语句；本文件没有对应 visitor 语义。
- Go parser 产出主 AST（Rust 的对应物目前在 [`lib.rs`](lib.rs)），而本文件节点由测试/调用方手工组装。不能仅凭本文件的 restore 测试推断 Rust parser 已产出这些子模块类型。
- `ProcedureParamStr` 在 Go 与本文件中都存在但不参与展示出的 `Restore` 路径；本文件保留字段以对齐结构。
- Go 跳转名称使用标识符写法，本文件 `ProcedureJump` 当前使用 `string()` 输出单引号；这是测试确认的现状，不应写成完全等价。

## 扩展指南

1. 新增过程语法时，先确定是否属于 parser 主 AST。若语法需要从 SQL 解析得到，至少同步 [`lib.rs`](lib.rs) 的主类型、`parser_actions` 构造、[`walk.rs`](walk.rs) 遍历和 [`sem.rs`](sem.rs) 分类；只改本文件不会自动接入解析主链。
2. 若扩展本还原模型，为新节点实现 `ProcedureNode`；声明类同时实现 `DeclNode`，handler 条件同时实现 `ErrNode`。组合节点应使用 `?` 保留第一个子错误，并明确分号和空格所有权，避免父子双方重复添加。
3. 若新增整数状态，优先评估改为枚举是否会破坏 Go 对齐和既有调用；继续接受未知值时，应在文档和测试中固定降级行为。
4. 涉及标识符或字面量时复用 `name()`/`string()`，但跳转标签应先核对 Go `WriteName` 语义，再决定是否修正当前单引号差异。
5. 测试必须放在独立文件：主要同步 [`procedure_test.rs`](procedure_test.rs)；跨模型契约可同步 [`model_7_aster_unit_test.rs`](model_7_aster_unit_test.rs)，parser 接线则同步 [`parser_actions/misc_test.rs`](../parser_actions/misc_test.rs)。同时参考 Go [`procedure_test.go`](procedure_test.go) 的对应案例，不能用更窄的 Rust 案例替代既有语义。
6. 兼容风险集中在精确文本（大小写、空格、逗号、分号、引用方式）和错误先后顺序；性能风险主要是深层/大型过程树的递归与中间字符串分配。任何行为修改都应覆盖成功、空集合、未知状态、子节点失败和标签不匹配。

## 验证依据

- RustCodeGraph：`status` 确认本地索引包含 11,467 个文件；`files --filter pkg/parser/ast/procedure.rs` 确认目标文件被索引；`node --file pkg/parser/ast/procedure.rs --offset 1/499` 阅读完整 587 行；`query ProcedureNode/ProcedureInfo/ProcedureLabel` 核对主要定义；`callers ProcedureInfo/ProcedureLabel` 未发现本子模块类型的明确生产调用；精确 `node` 查询核对了 `lib.rs`、`parser_actions/misc.rs`、`walk.rs`、`sem.rs` 和测试证据。
- 读取的 Rust 源与配置：[`procedure.rs`](procedure.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`parser_actions/misc.rs`](../parser_actions/misc.rs)、[`walk.rs`](walk.rs)、[`sem.rs`](sem.rs)。
- 独立 Rust 测试：[`procedure_test.rs`](procedure_test.rs) 覆盖基本节点、默认表达式错误、参数类型、DROP、循环、游标、handler、IF/CASE 和标签；[`model_7_aster_unit_test.rs`](model_7_aster_unit_test.rs) 覆盖未知参数模式、跳转文本和不匹配标签错误。
- Go 对照：[`procedure.go`](procedure.go) 核对类型、`Restore`、visitor 和标签接口；[`procedure_test.go`](procedure_test.go) 核对 parser/visitor/round-trip 的真实语法覆盖。
- 本任务为纯文档分析，按计划不运行 Cargo。交付结构验证要求目标文档存在，且固定二级标题恰好为 11 个；此外人工复核本说明明确回答了文件存在目的、直接执行流程、未接入解析主链的边界及安全扩展位置。
