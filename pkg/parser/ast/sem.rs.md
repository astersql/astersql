# `pkg/parser/ast/sem.rs`

## 文件定位

本文件属于 `astersql-parser-ast` crate，由 [`lib.rs`](lib.rs) 中的 `pub mod sem` 公开。它位于 SQL 已被解析成 AST 节点之后，提供一层不依赖 SQL 还原文本的“语句命令类别”元数据：调用方通过 `SEMCommand::sem_command` 得到稳定的静态字符串，例如 `SelectStmt -> "SELECT"`、`CreateTableStmt -> "CREATE TABLE"`。

该层的设计用途是供权限、审计或其他只关心语句类别的逻辑使用，但仓库内 Rust 生产源码目前没有检索到 `sem_command()` 的外部调用；可确认的调用者均位于独立测试文件。因此应把它理解为已公开、已覆盖测试的 AST 分类能力，而不是声称它已经接入 Rust SQL 执行主链。依据是本文件第 14--23 行、[`lib.rs`](lib.rs) 的模块声明，以及全仓库对 `.sem_command()`/`SEMCommand::sem_command` 的检索结果。

## 核心职责

1. 用 `SEMCommand` trait 统一各类 AST 节点的命令分类接口，返回 `&'static str`，避免从格式化后的 SQL 文本反推类型。
2. 声明 225 个公开命令常量，覆盖 DDL、DML、`SHOW`、`ADMIN`、BRIE、事务控制、权限、统计和存储过程等类别；常量值是兼容契约，例如 `ShowRegionsCommand` 的值是 `"SHOW TABLE REGIONS"`。
3. 为字段不影响分类的节点批量生成固定映射；为分类取决于节点字段的类型显式实现分支逻辑。
4. 提供 `SemStatement` 这个独立分类值，允许不持有具体 AST 节点时表达固定命令以及 DROP/INSERT/EXPLAIN 的动态类别。

本文件只负责分类，不解析 SQL、不还原 SQL、不执行语句，也不检查权限。命令字符串相同只表示分类相同，例如多个存储过程内部节点都归入 `ProcedureCommand`。

## 主要符号

- `pub trait SEMCommand { fn sem_command(&self) -> &'static str; }`：唯一公开行为接口。静态返回值约束了实现只能返回程序生命周期内有效的字符串；本文件实现均返回公开常量或 `SemStatement::Fixed` 携带的静态字符串。
- 225 个 `pub const *Command: &str`：稳定的分类词表。`UnknownCommand` 是无法识别类别时的哨兵；`SetOprCommand`、`ProcedureCommand` 等是比具体 SQL 文本更粗的类别。
- `fixed_sem_command!`：`typed_sem_impls` 私有模块中的宏，为一百余个具体 AST 类型生成“忽略节点字段、固定返回一个常量”的 `SEMCommand` 实现。宏只消除重复实现，不引入运行时表或动态分派。
- 显式动态实现：
  - `CancelMaterializedViewJobStmt` 按 `Tp` 返回刷新任务、日志清理任务或 `UNKNOWN`。
  - `ShowStmt` 按 `ShowStmtType` 返回对应 SHOW 子类；`None` 返回 `UNKNOWN`，`ReplicaStatus` 有意归为通用 `SHOW`。
  - `AdminStmt` 按 `statement_type` 分类；`ShowDdlJobQueries` 与 `ShowDdlJobQueriesWithRange` 合并到同一命令。
  - `BRIEStmt` 按 `Kind` 区分备份、恢复、日志流和任务查询/取消。
  - `DropTableStmt`、`InsertStmt`、`ExplainStmt` 分别读取 `IsView`、`IsReplace`、`analyze` 决定最终命令。
- `pub enum SemStatement`：有 `Fixed(&'static str)`、`DropTable { view }`、`Insert { replace }`、`Explain { analyze }` 四个变体，并派生 `Clone + Copy + Debug + Eq + PartialEq`。

文件中第 571--711 行的大段 Show/Admin/BRIE 实现位于块注释内，是旧字段/旧枚举形式的迁移残迹，不参与编译；真实生效实现位于第 270--403 行。文件没有 feature 或条件编译项。

## 执行流程

典型流程如下：

1. 解析器或上层代码已经构造某个具体 AST 节点；本文件不参与构造。
2. 调用方把 `SEMCommand` trait 引入作用域，并在节点上调用 `sem_command()`。
3. 若节点由 `fixed_sem_command!` 覆盖，生成的实现直接返回对应常量，时间和空间开销均为常数。
4. 若节点分类依赖字段，显式实现读取一个判别字段并执行 `match` 或布尔分支。例如 `InsertStmt.IsReplace=true` 返回 `ReplaceCommand`，否则返回 `InsertCommand`。
5. 返回值是借用的静态字符串，不分配内存，也不修改节点。

`SemStatement` 的路径与具体节点相同：`Fixed` 原样返回携带的静态字符串，其他三个动态变体根据布尔字段在两项常量之间选择。它不会把任意字符串校验为已声明命令，因此 `SemStatement::Fixed` 的正确性由构造方负责。

## 数据与状态

模块的持久数据只有编译期字符串常量，没有全局可变状态、缓存或注册表。具体节点实现只读取 AST 中已有的枚举/布尔字段；`sem_command(&self)` 不取得可变引用。

需要保持的主要不变量是：同一语义类别始终返回同一个公开常量；固定映射不得依赖无关节点字段；会改变 SQL 动词的字段必须在显式实现中保留。`UnknownCommand` 只用于明确的未知/空判别值，不能作为遗漏新枚举分支的便捷默认值。Rust 的穷尽 `match` 会在枚举新增变体而未同步映射时产生编译错误，这是 Show/Admin/BRIE 映射的重要维护保护。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 确认：包名为 `astersql-parser-ast`，库入口是 `lib.rs`。本文件自身没有使用 Cargo 外部依赖；它通过 `crate::*` 或精确导入依赖同一 crate 中定义的 AST 节点及判别枚举，如 `ShowStmtType`、`AdminStmtType`、`BRIEKind` 和各种 `*Stmt`。

RustCodeGraph 对 `sem_command` 找到 trait 声明及 8 组实现定义，并给出实现到命令常量的引用边：例如 `ShowStmt::sem_command -> Show*Command`、`AdminStmt::sem_command -> Admin*Command`、`BRIEStmt::sem_command -> Backup/Restore/Stream*Command`。宏生成的具体实现也以下游常量为终点，不调用 I/O、解析器或执行器。

上游方面，全仓库 Rust 检索仅发现测试调用：[`sem_test.rs`](sem_test.rs)、[`model_7_aster_unit_test.rs`](model_7_aster_unit_test.rs)、[`go_merge_13_test.rs`](go_merge_13_test.rs) 和 [`go_merge_16_test.rs`](go_merge_16_test.rs)。因此目前不能从本仓库证据证明权限或审计生产路径已经消费该 trait。

## 错误处理与边界

接口没有 `Result` 或 `Option`，分类过程本身不会返回运行时错误。边界行为通过分类值表达：`ShowStmtType::None` 和未知的物化视图取消类型返回 `UnknownCommand`。其他生效的 Rust 枚举映射是穷尽的，不存在 Go 式整数枚举的通用 `default` 分支。

几个容易误改的兼容边界是：DROP TABLE/VIEW、INSERT/REPLACE、EXPLAIN/EXPLAIN ANALYZE 必须读取动态字段；`ShowStmtType::ReplicaStatus` 返回通用 `ShowCommand`；两个 ADMIN DDL 查询变体共享 `AdminShowDDLJobQueriesCommand`；刷新物化视图的普通节点和实现节点共享 `RefreshMaterializedViewCommand`。本文件不校验 AST 的其他字段是否合法，因此即使一个节点无法成功还原 SQL，它的 SEM 分类仍可能成功；[`go_merge_16_test.rs`](go_merge_16_test.rs) 对无效物化视图取消类型同时验证了还原报错和 SEM 返回 `UNKNOWN`。

## 并发与资源生命周期

所有常量均为不可变静态数据，`sem_command` 只持有调用期间的共享借用并返回 `&'static str`。模块不创建线程、任务、锁、通道、文件句柄、网络连接或事务，也没有清理阶段。只要具体 AST 节点本身能按调用方约束共享，分类操作不会增加额外并发风险；返回字符串与节点生命周期无关。

性能特征是零分配、常数时间：固定实现是一次返回，动态实现是一次布尔判断或对小型枚举的一次匹配。扩展时应维持这一性质，避免把 SQL 格式化或外部状态查询引入分类路径。

## 与 Go 版本的对应关系

直接对照文件是 [`sem.go`](sem.go)，Rust 的公开常量值和具体节点映射总体逐项复刻 Go 的 `SEMCommand() string`。Go 为每个 AST 类型分别声明方法；Rust 用 trait 加 `fixed_sem_command!` 合并重复样板。两者都为 `DropTableStmt`、`InsertStmt`、`ExplainStmt`、`ShowStmt`、`AdminStmt`、`BRIEStmt` 和物化视图取消节点保留字段驱动分支。

语义表示存在两点差异：

- Go 的 Show/Admin/BRIE 判别类型可通过整数落入 `default -> UnknownCommand`；Rust 使用带 `Unknown`/`None` 变体或穷尽枚举。当前 `ShowStmt` 的 `None` 映射为 `UNKNOWN`，物化视图取消类型的 `Unknown(_)` 也映射为 `UNKNOWN`，而 Admin/BRIE 的生效 Rust 枚举没有额外未知分支。
- `SemStatement` 是 Rust 为独立分类和迁移接线提供的辅助枚举，Go 同路径文件没有对应类型；它不能替代新增具体 AST 节点的 trait 实现。

Go 测试 [`sem_test.go`](sem_test.go) 通过遍历枚举计数检查 Show/Admin/BRIE 不返回 `UNKNOWN`，并覆盖物化视图命令。Rust 的 [`sem_test.rs`](sem_test.rs) 使用显式 `(变体, 命令)` 表核对实际值，另由合并回归测试覆盖新增 Show 类型和物化视图行为。Rust 的显式表更精确，但新增枚举或常量时必须人工同步测试表。

## 扩展指南

新增语句类别时按以下位置接入：

1. 在命令常量区增加与 Go 值一致的 `pub const`；若是 Go 移植，先核对 [`sem.go`](sem.go) 的精确字符串和对应节点方法。
2. 分类不依赖字段时，把具体节点加入 `fixed_sem_command!`；分类依赖字段时，在 `typed_sem_impls` 内写显式 `impl SEMCommand`。Show/Admin/BRIE 的新变体应直接更新文件顶部的生效 `match`，不要编辑第 571--711 行的注释代码。
3. 同步独立测试 [`sem_test.rs`](sem_test.rs)。涉及物化视图或合并提交行为时还要检查 [`go_merge_16_test.rs`](go_merge_16_test.rs)；涉及 `SemStatement` 时同步 [`model_7_aster_unit_test.rs`](model_7_aster_unit_test.rs)。测试逻辑不要内嵌回生产源文件。
4. 对照 Go 的特殊合并和动态分支，尤其避免把字段相关类型误放入固定映射。若新增 `Unknown` 变体，应明确它返回 `UNKNOWN` 还是独立命令。
5. 搜索实际调用者，确认下游是否依赖字符串的精确拼写。常量值属于兼容面，改名 Rust 标识符与修改字符串值的风险不同。

主要风险是兼容性而非性能：拼写变化会影响按字符串聚合的潜在审计/权限消费者；漏映射会导致未知分类或编译失败；错误地固定映射会混淆两个 SQL 动词。当前没有生产调用证据，因此新增功能若要接入执行主链，还需在目标消费模块单独设计和验证，不能只增加这里的实现。

## 验证依据

- RustCodeGraph：`status` 显示目标仓库索引包含 `pkg/parser/ast/sem.rs`；`files --filter` 显示该文件有 242 个索引符号；`query SEMCommand`、`query sem_command` 和 `node sem_command` 定位 trait/实现，并展示 Show/Admin/BRIE/物化视图分支到命令常量的引用边。通用 `callers/callees` 未给出可靠的宏展开调用者，因此用精确源码检索补足。
- 生产源码：完整阅读 [`sem.rs`](sem.rs)；读取 [`lib.rs`](lib.rs) 的公开模块声明；读取 [`Cargo.toml`](Cargo.toml) 的 crate 名称、入口、依赖和 Go 迁移元数据。
- Go 对照：阅读 [`sem.go`](sem.go) 的常量、固定节点方法以及 Drop/Insert/Show/Admin/BRIE/Explain/物化视图动态分支。
- 测试证据：阅读 [`sem_test.rs`](sem_test.rs)、[`sem_test.go`](sem_test.go)、[`model_7_aster_unit_test.rs`](model_7_aster_unit_test.rs)、[`go_merge_13_test.rs`](go_merge_13_test.rs) 与 [`go_merge_16_test.rs`](go_merge_16_test.rs) 中直接涉及 SEM 的用例。
- 静态计数与调用检索：`rg -c '^pub const ' pkg/parser/ast/sem.rs` 得到 225；全仓库 Rust 检索 `.sem_command()` 和 `SEMCommand::sem_command` 只命中上述测试文件。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核本文区分了源码事实、设计用途和未接线状态。
