# `pkg/parser/opcode/opcode.rs`

## 文件定位

本文件是 `astersql-parser-opcode` crate 的核心实现，定义跨 parser/expression 边界使用的表达式运算符编号及其文本元数据。crate 入口 `pkg/parser/opcode/lib.rs` 通过 `pub mod opcode` 和 `pub use opcode::*` 导出这里的 `Op`，并把 `astersql_parser_format` 再导出为 `crate::format`；`pkg/parser/opcode/Cargo.toml` 也表明该 crate 唯一的直接依赖是相邻的 `astersql-parser-format`。

它不解析 SQL，也不执行运算。其职责是为运算符提供稳定判别值、内部名称、SQL 字面量、关键字分类，以及将单个运算符格式化或恢复成 SQL 片段的能力。工作区中 `pkg/expression`、`pkg/session`、`pkg/ddl` 和 `pkg/planner/indexadvisor` 的 Cargo manifest 直接依赖此 crate；实际 Rust 使用示例包括 `pkg/expression/planner_bridge.rs::RefineComparedConstant` 和 `pkg/expression/util.rs::symmetricOp`。

需要区分另一个当前实现：`pkg/parser/ast/expressions.rs` 自己定义了用于 Rust AST 恢复与优先级判断的 `expressions::Op`。该枚举包含 `MemberOf`、`Between`、`Collate` 等额外成员，当前并非这里的 `opcode::Op` 的别名。本文件因此是共享 opcode/Go 兼容边界，而不是当前 Rust AST 表达式恢复的唯一运算符实现。

## 核心职责

1. `Op` 固定 31 个有效运算符的数值编号，范围为 1 至 31，并以 `#[repr(usize)]` 支持作为元数据表下标。
2. 私有 `ops` 表把每个编号映射到三个属性：稳定内部名 `name`、SQL 输出 `literal`、是否按关键字处理 `isKeyword`。
3. `Op::String` 返回内部名，供名称比较或诊断使用；它不是 SQL 输出接口。
4. `Op::Format` 把原始 SQL 字面量写入任意 `std::io::Write`，并刻意忽略写错误以对齐 Go 行为。
5. `Op::IsKeyword` 暴露关键字分类；`Op::Restore` 据此选择 `RestoreCtx::WriteKeyWord` 或 `RestoreCtx::WritePlain`，使关键字服从恢复上下文的大小写策略，而标点运算符保持原样。

本文件不负责运算符优先级、结合性、AST 括号判定或求值。这些行为由调用方维护，例如当前 Rust AST 的优先级逻辑位于 `pkg/parser/ast/expressions.rs::Op::precedence`。

## 主要符号

- `pub enum Op`：公开的、可复制和可哈希的运算符标识。派生 `Clone`、`Copy`、`Debug`、`Eq`、`Hash`、`PartialEq`；首项 `LogicAnd = 1`，其余成员依声明顺序递增，末项 `IsFalsity = 31`。
- `struct OpInfo`：私有、只读的元数据记录，字段均为静态字符串或布尔值。调用方不能绕过 `Op` 方法修改表内容。
- `const EMPTY_OP`：下标 0 的空哨兵，保留 Go `iota + 1` 留下的编号空洞；其三个字段分别为 `""`、`""`、`false`。
- `static ops: [OpInfo; 32]`：编号到元数据的一一映射。数组长度必须始终等于最大有效判别值加一。
- `Op::String(self) -> &'static str`：以 `ops[self as usize].name` 查表；例如 `Plus` 返回 `"plus"`。
- `Op::Format<W: Write>(self, writer: &mut W)`：写入 `literal`，返回 `()`；`let _ = writer.write_all(...)` 明确丢弃错误。
- `Op::IsKeyword(self) -> bool`：返回表中的关键字标志。当前关键字包括逻辑关键字、`DIV`、`IN`、`LIKE`、`CASE`、`REGEXP` 与三种 `IS ...` 运算符；标点符号不是关键字。
- `Op::Restore(self, ctx: &mut RestoreCtx) -> io::Result<()>`：关键字走 `WriteKeyWord`，其他字面量走 `WritePlain`，并通过 `?` 传播上下文写入错误。
- `mod opcode_test`：仅在 `cfg(test)` 下装入独立文件 `pkg/parser/opcode/opcode_test.rs`，测试逻辑没有与生产实现写在同一文件中。

## 执行流程

构造和消费 `Op` 时，调用方通常先把语法或函数名称映射为某个枚举成员，再按用途分流。例如 `pkg/expression/builtin.rs` 将 `eq/ne/lt/le/gt/ge/nulleq` 名称映射为比较 opcode；`pkg/expression/planner_bridge.rs::RefineComparedConstant` 根据 `LT/GE`、`LE/GT`、`NullEQ/EQ` 分支选择常量取整策略；`pkg/expression/util.rs::symmetricOp` 则记录交换比较两侧后应使用的 opcode。

文本输出路径完全由表驱动：

1. `self as usize` 得到 1..=31 的表下标。
2. `String` 读取 `name`；`Format` 读取 `literal`；`IsKeyword` 读取 `isKeyword`。
3. `Restore` 同时读取 `literal` 和 `isKeyword`。关键字调用 `RestoreCtx::WriteKeyWord`，因此可被 `RestoreKeyWordLowercase` 等 flag 改写；符号调用 `WritePlain`，不参与关键字大小写变换。
4. `Restore` 只处理一个运算符片段，不添加通用的前后空格或括号。唯一内嵌空格是 `Not` 的字面量 `"not "`，这是与 Go 表保持一致的元数据。

`NE` 固定输出 `!=`，源码和 Go 对照都注明另一种 SQL 拼写可能是 `<>`，但当前契约没有选择它。

## 数据与状态

所有运行时数据均为进程只读静态数据：`Op` 自身是一个 `usize` 表示的枚举值，`OpInfo` 持有 `&'static str`，`ops` 是编译期确定的 32 项数组。没有堆分配、缓存、全局可变变量或惰性初始化。

核心不变量是“判别值等于 `ops` 下标”：0 只属于 `EMPTY_OP`，31 个有效枚举值必须连续且每项都具有对应元数据。`#[repr(usize)]`、声明顺序和数组顺序共同形成兼容协议；在中间插入、删除或重排成员会改变后续编号，可能影响跨 crate 行为及与 Go 的对应关系。

内部名和 SQL 字面量是不同命名空间。例如 `And` 的内部名为 `bitand`、字面量为 `&`；`LogicAnd` 的内部名为 `and`、字面量为 `AND`。调用者必须按意图选择 `String` 或 `Format/Restore`，不能假设两者相同。

## 依赖与调用关系

下游依赖只有标准库 `std::io::{self, Write}` 和 `crate::format::RestoreCtx`。`Format` 调用 `Write::write_all`；`Restore` 调用格式化 crate 提供的 `RestoreCtx::WriteKeyWord`/`WritePlain`。

上游按用途可分为两类：

- 运算语义消费者：`pkg/expression/planner_bridge.rs::RefineComparedConstant` 以 `opcode::Op` 决定比较常量的精化方向；`pkg/expression/util.rs::symmetricOp` 以它作为哈希表键和值；`pkg/expression/builtin.rs` 在比较函数和真假判断函数中保存或匹配它。
- 文本契约消费者：同 crate 的 `pkg/parser/opcode/opcode_test.rs` 调用 `String` 和 `Format`；`pkg/parser/opcode/migration_aster_unit_test.rs` 对全部 31 项调用 `String`、`Format`、`IsKeyword` 和 `Restore`，并验证关键字大小写分流与错误契约。

RustCodeGraph 索引将本文件标记为被 36 个文件使用，但对同名 `Op`、`String`、`Format`、`Restore` 的精确调用边存在名称碰撞；因此上述生产调用关系还以直接的 Rust 引用搜索核对。当前 Rust AST 的 `pkg/parser/ast/expressions.rs` 走其本地 `Op::sql/is_keyword/precedence`，不应被描述成调用本文件的 `Restore`。

## 错误处理与边界

`Format` 与 `Restore` 的错误语义不同。`Format` 有意忽略 `write_all` 的结果，这与 `pkg/parser/opcode/opcode.go::Op.Format` 忽略 `io.WriteString` 返回值一致；`migration_aster_unit_test.rs::format_ignores_writer_errors_like_go` 用总是失败的 writer 固化了该行为。调用方若需要感知输出失败，应使用 `Restore` 路径或在更高层另行设计，不能依赖 `Format` 报错。

`Restore` 返回 `io::Result<()>`，对 `WriteKeyWord` 或 `WritePlain` 的错误使用 `?` 原样传播，成功后返回 `Ok(())`。它不校验 SQL 上下文、操作数数量或运算符位置。

安全 Rust 无法构造判别值 0 或大于 31 的 `Op`。因此查表在正常安全构造下不会越界；不要通过 `unsafe` 转换任意整数为 `Op`。Go 的 `Op` 是整数别名，可用 0 表示哨兵；Rust 测试只能直接检查 `ops[0].name` 为空，不能安全复刻 Go 的无效枚举值调用。

## 并发与资源生命周期

`Op` 和 `OpInfo` 都只包含可复制的值或静态引用，`ops` 只读，因此本文件没有锁、原子变量、线程、任务、通道或事务生命周期。多个线程可并发读取同一元数据表，不会产生共享可变状态。

输出资源由调用方拥有：`Format` 只在借用期间使用 `&mut W`；`Restore` 只在调用期间借用 `&mut RestoreCtx`。本文件不创建、刷新或关闭 writer，也不保存上下文引用。是否缓冲、何时提交输出以及失败后的恢复均由上层决定。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/opcode/opcode.go`。Rust 的 `Op` 成员顺序和数值与 Go 的 `type Op int`、`iota + 1` 对齐；`EMPTY_OP` 和 `ops[0]` 对应 Go 数组因首个编号为 1 而留下的零值槽；31 项 `name/literal/isKeyword` 与 Go 表逐项一致。

方法映射如下：Go `String() string` 对应 Rust `String() -> &'static str`；Go `Format(io.Writer)` 对应泛型 `Format<W: Write>`；Go `IsKeyword() bool` 直接对应；Go `Restore(*format.RestoreCtx) error` 对应 Rust `Restore(&mut RestoreCtx) -> io::Result<()>`。两边 `Format` 都忽略 writer 错误，两边 `Restore` 都按关键字标志分流。

存在语言模型差异：Go 可表示任意整数 opcode，Rust `enum` 只允许声明过的有效判别值；Go 测试可把 `op` 设为 0 后调用 `String`，Rust 只能验证零号表项。Rust 还通过类型系统把表下标绑定为 `usize` 并把静态字符串生命周期显式化。这些差异不改变有效 1..=31 范围内的移植语义。

## 扩展指南

新增运算符时，至少需要同步修改 `Op`、`ops` 和测试用例表。为保持 Go 对齐，应先确定 Go `pkg/parser/opcode/opcode.go` 的目标编号；若必须保持已有编号稳定，优先追加而不是在中间插入。随后调整数组长度，并保证新成员的判别值、表下标、内部名、SQL 字面量和关键字标志完全对应。

应同步更新独立测试文件 `pkg/parser/opcode/opcode_test.rs` 的 `ALL_OPS`，以及 `pkg/parser/opcode/migration_aster_unit_test.rs` 的 `CASES`；如果 Go 行为也改变，还要同步 `pkg/parser/opcode/opcode_test.go` 或增加相应 Go 回归。测试至少覆盖编号、`String`、`Format`、`IsKeyword`、默认和小写关键字 `Restore`，以及 writer 错误契约。

若新运算符参与真实表达式语义，还需按用途检查 `pkg/expression/builtin.rs`、`pkg/expression/planner_bridge.rs`、`pkg/expression/util.rs`。若它也应进入当前 Rust AST 恢复链，则必须另外评估 `pkg/parser/ast/expressions.rs` 的本地 `Op`、SQL 文本、关键字分类、优先级和括号逻辑；仅扩展本文件不会自动扩展该 AST 枚举。

兼容风险主要是编号重排和文本变化；性能风险很低，因为访问是常量时间数组索引且无分配。把符号误标为关键字会改变 `RestoreCtx` 大小写处理，把关键字误标为普通文本则会绕过恢复 flag；修改 `Not` 的尾随空格或 `NE` 的拼写也可能改变生成 SQL 的精确文本。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/parser/opcode` 确认目标 Rust、Go 和测试文件均已索引。
- RustCodeGraph `node --file pkg/parser/opcode/opcode.rs`：核对 `Op`、`OpInfo`、`EMPTY_OP`、`ops` 及四个公开方法的完整实现；索引报告该文件被 36 个文件使用。
- RustCodeGraph `node`：阅读 `pkg/parser/opcode/lib.rs`、`pkg/parser/opcode/opcode.go`、`pkg/parser/opcode/opcode_test.rs`、`pkg/parser/opcode/opcode_test.go`、`pkg/parser/opcode/migration_aster_unit_test.rs`，并核对 `pkg/parser/ast/expressions.rs`、`pkg/expression/planner_bridge.rs`、`pkg/expression/util.rs` 的直接上下文。
- 配置读取：`pkg/parser/opcode/Cargo.toml` 证明 crate 名、入口、唯一直接依赖和 Go 包迁移元数据；工作区 Cargo 引用搜索确认 session、expression、DDL、indexadvisor 与根 facade 的依赖声明。
- 调用核对：RustCodeGraph 的同名符号查询存在碰撞后，使用 Rust 源码引用搜索确认 `opcode::Op` 在表达式内建、常量精化与对称映射中的使用，并确认本地 opcode 测试对四个方法的直接覆盖。
- 未运行 Cargo 或代码测试：本任务是纯文档分析，计划明确禁止 Cargo；结论建立在已索引源码、Cargo 配置、Go 对照和既有独立测试上。
