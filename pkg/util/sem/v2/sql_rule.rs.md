# `pkg/util/sem/v2/sql_rule.rs`

## 文件定位

本文件是源码 [`pkg/util/sem/v2/sql_rule.rs`](sql_rule.rs) 的逻辑说明。源码属于 `astersql-util-sem-v2` crate 的 SQL 限制规则与命令分类层；crate 入口 `pkg/util/sem/v2/lib.rs` 将该模块声明为私有模块并通过 `pub use sql_rule::*` 重导出公共项。`pkg/util/sem/v2/Cargo.toml` 表明它直接依赖 `astersql-parser-ast` 与 `astersql-objstore`，测试阶段另依赖完整 parser。它位于配置和运行时之间：`config.rs::validateSEMConfig` 用这里的 `sqlRuleNameMap` 校验规则名，`sem.rs::buildSEMSqlValidateFunction` 将规则函数与配置的命令字符串编译成运行时判定闭包，最终由 `sem.rs::IsRestrictedSQL` 查询。

## 核心职责

文件承担两类互补职责：

1. 将 `&dyn ast::Node` 分类成 SEM 使用的规范命令名。`semCommand` 对少数带状态的语句分别处理，例如 `DropTableStmt::IsView`、`InsertStmt::IsReplace`、`ExplainStmt::analyze`，并把 `ShowStmt`、`AdminStmt`、`BRIEStmt` 的枚举子类型交给专用映射函数；其余已知 AST 类型通过 `fixed_command!` 返回 `ast::sem` 中的常量，未知节点返回 `UnknownCommand`。
2. 实现配置字段 `restricted_sql.rule` 可引用的五个命名规则。它们检查 TTL 表选项、ALTER TABLE ATTRIBUTES、SELECT INTO OUTFILE 和本地文件导入等不能仅靠语句大类表达的条件；`import_with_external_id` 仅保留兼容规则名，当前明确恒为 `false`。

`CommandStatement` 是 Rust 侧的轻量适配节点：当调用方只有已知命令名、没有完整 parser AST 时，它仍可走同一 `IsRestrictedSQL` 链路。其构造器会先去除首尾空白再转为大写，从而与运行时对配置命令的规范化一致。

## 主要符号

- `pub struct CommandStatement`：保存一个默认 `ast::base::AstNode` 和私有规范化命令字符串；实现 `ast::Node` 的文本访问、`Any` 下转型以及 visitor 的 enter/leave 调用。`new(&str)` 是其公共构造入口。
- `pub type SQLRule = fn(&dyn ast::Node) -> bool`：命名规则的统一函数指针类型；`true` 表示语句受限。函数指针不捕获环境，便于放入全局注册表和运行时 map。
- `fn checkTTLOptions(&[ast::TableOption]) -> bool`：TTL 规则的内部助手，命中 `TTL`、`TTLEnable` 或 `TTLJobInterval` 任一表选项。
- `pub static sqlRuleNameMap: LazyLock<HashMap<&'static str, SQLRule>>`：规则名注册表，包含 `time_to_live`、`alter_table_attributes`、`import_with_external_id`、`select_into_file`、`import_from_local`。它既是配置合法性白名单，也是运行时解析表。
- `TimeToLiveSQLRule`：CREATE TABLE 检查 `Options`；ALTER TABLE 检查 `RemoveTTL`，或 `Option` spec 中的 TTL 选项；其他节点返回 `false`。
- `AlterTableAttributesRule`：只对 `AlterTableStmt` 生效，命中 `Attributes` 或 `PartitionAttributes` spec。
- `ImportWithExternalIDRule`：兼容占位规则，无论输入节点为何均返回 `false`。
- `SelectIntoFileRule`：只对 `SelectStmt` 生效，并以 `SelectIntoOpt.is_some()` 判断。
- `ImportFromLocalRule`：处理 `ImportIntoStmt` 和 `LoadDataStmt`；具体分支见“执行流程”。
- `pub(crate) fn semCommand(&dyn ast::Node) -> String`：crate 内命令分类总入口；公共调用方经 `IsRestrictedSQL` 间接使用。
- `showCommand`、`adminCommand`、`brieCommand`：三个私有、穷举枚举到静态命令常量的映射函数。

本文件没有条件编译项。测试装配位于 `lib.rs` 的 `#[cfg(test)]` 模块中，测试逻辑独立存放在 `sql_rule_test.rs`，没有嵌入生产源文件。

## 执行流程

启用 SEM 时，`sem.rs::EnableBy` 校验配置并调用 `buildSEMFromConfig`。后者调用 `buildSEMSqlValidateFunction`：先从 `sqlRuleNameMap` 解析所有规则名，再将配置中的 SQL 命令去空白、转大写、丢弃空串，最后生成一个闭包。每次 `IsRestrictedSQL(stmt)` 到达该闭包时，先判断 `sqlCommands.contains(&semCommand(stmt))`，再以“任一规则返回 true”的方式执行命名规则；两条路径是逻辑或关系。

`semCommand` 的分派顺序如下：

1. 若节点是 `CommandStatement`，直接克隆其规范化命令。
2. 对 DROP TABLE/VIEW、INSERT/REPLACE、EXPLAIN/EXPLAIN ANALYZE 读取节点字段决定细分类别。
3. 对 SHOW、ADMIN、BRIE 读取枚举种类，并交给相应专用函数。
4. 对大量一对一 AST 类型使用 `fixed_command!` 返回固定常量；过程类节点统一归为 `ProcedureCommand`。
5. 没有任何类型命中时返回 `UnknownCommand`，而不是报错或猜测类别。

`ImportFromLocalRule` 的分支尤其重要。对 `ImportIntoStmt`，若存在 `Select` 子查询立即放行，因为此时不是本地文件导入；否则使用 `objstore::parse::ParseRawURL` 解析 `Path`，解析成功后用 `IsLocal` 判定。对 `LoadDataStmt`，客户端 `LOCAL`（`FileLocRef::Client`）立即放行，只有服务端读取路径才继续解析并判断本地性。路径解析失败统一得到 `false`。其他 AST 类型也得到 `false`。

## 数据与状态

`sqlRuleNameMap` 是唯一的模块级可变初始化状态，但暴露的是 `LazyLock<HashMap<...>>`：首次访问时一次性构造，此后只读。key 和函数指针均为静态数据，不依赖请求或会话生命周期。

规则本身是纯查询：它们只读取 AST 字段，不修改节点、配置或全局 SEM 状态。`CommandStatement` 自有 `String`，构造时完成标准化；`semCommand` 返回拥有所有权的 `String`，因此命令集合比较不借用 AST 内存。SHOW/ADMIN/BRIE 辅助函数返回 `&'static str`，随后由 `semCommand` 转成拥有的字符串。

文件不持有配置集合；命令集合和已选择规则的生命周期属于 `sem.rs::buildSEMSqlValidateFunction` 创建的 `Arc<dyn Fn + Send + Sync>`。这一区分意味着修改注册表只影响后续构建的 SEM 实例，不会在已构建闭包内动态替换函数。

## 依赖与调用关系

上游直接证据如下：

- `config.rs::validateSEMConfig → sqlRuleNameMap.contains_key`：启用前拒绝未知命名规则。
- `sem.rs::buildSEMSqlValidateFunction → sqlRuleNameMap.get`：将规则名解析为 `SQLRule` 函数指针。
- `sem.rs::buildSEMSqlValidateFunction` 生成的闭包 `→ semCommand` 以及各 `SQLRule`：完成命令与细粒度规则的联合判定。
- `sem.rs::IsRestrictedSQL → SemImpl::isRestrictedSQL → restrictedSQL`：应用运行期的公共入口。
- `pkg/util/sem/compat/sem_integration_test.rs` 使用 `CommandStatement::new` 验证只有命令字符串时的兼容层行为。

RustCodeGraph 对目标文件记录 25 个符号；图查询确认 `semCommand → showCommand/adminCommand/brieCommand` 和 `TimeToLiveSQLRule → checkTTLOptions`。静态表读取和闭包内函数指针调用未形成完整的跨文件 callers 边，因此上述上游关系由 `config.rs`、`sem.rs` 和测试中的直接引用补证。

下游依赖主要是 `ast` 与 `objstore`。`ast::Node::as_any` 提供运行时类型识别，具体 AST 结构和 `ast::sem::*Command` 常量提供分类依据；`objstore::parse::{ParseRawURL, IsLocal}` 定义“本地路径”的统一语义。`std::any::Any` 支持轻量节点与具体 AST 下转型，`HashMap` 和 `LazyLock` 支持注册表。

## 错误处理与边界

所有 `SQLRule` 都是布尔判定接口，不返回错误。节点类型不匹配一律安全返回 `false`；`semCommand` 对未覆盖节点返回 `UNKNOWN`。因此未知或解析失败默认“不因本规则受限”，这是一种明确的放行边界，调用方不能把它误解为语句已获其他权限检查批准。

`ImportFromLocalRule` 使用 `ParseRawURL(...).map(IsLocal).unwrap_or(false)`，URL 解析错误不会传播，也不会 panic。IMPORT FROM SELECT 和 LOAD DATA LOCAL 是显式放行分支。`CommandStatement::new` 接受空白字符串，结果可能为空，但运行时构建命令集合时会过滤配置侧空命令，所以通常不会命中。

SHOW、ADMIN 和 BRIE 的 match 当前对其枚举变体穷举；枚举新增通常会触发编译期非穷举错误，迫使维护者补映射。相反，`fixed_command!` 依赖手工列出 AST 类型：新增节点不会自动触发本文件编译失败，而会在运行时回落到 `UnknownCommand`。这是扩展时最需要防守的兼容边界。

`ast::Node` 的 visitor 实现仅对 `CommandStatement` 本身调用 `enter`/`leave`，因为它没有子节点；返回值直接使用 visitor 的 `leave` 结果。节点文本字段使用默认值，命令匹配不依赖该字段。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。`LazyLock` 保证注册表在并发首次访问时只初始化一次；初始化后 `HashMap` 只读，规则函数也是无捕获函数指针，因此可以被 `sem.rs` 的 `Send + Sync` 判定闭包并发调用。

AST 与 `CommandStatement` 都通过共享引用传入，规则执行期间不取得可变引用。路径解析只产生调用栈内临时 URL 值，函数返回时释放。全局 SEM 的启用、替换和锁语义由 `sem.rs::globalSem` 管理，不属于本文件；本文件只提供可被该生命周期持有的静态规则函数。

## 与 Go 版本的对应关系

`pkg/util/sem/v2/sql_rule.go` 是五个命名规则的直接对照：两侧具有相同规则名、相同 AST 分支和相同真假边界。Rust 的 `SQLRule` 是函数指针类型，Go 是函数类型；Rust 的 `LazyLock<HashMap>` 对应 Go 包级 map。`ImportWithExternalIDRule` 两侧均是为既有配置保留的恒 false 兼容入口，外部 ID 检查不在该规则列表中执行。

主要结构差异在命令分类。Go 的 `sem.go::buildSEMSqlValidateFunction` 调用 `stmt.SEMCommand()`，各节点实现集中在 `pkg/parser/ast/sem.go`；Rust 运行时本文件的 `semCommand` 通过 `Any` 下转型集中列出 AST 类型，并引用 `pkg/parser/ast/sem.rs` 的命令常量。Rust parser AST 自身也有 `SEMCommand::sem_command` 实现，但本运行时入口没有对任意 trait 对象直接调用它。因此，Go 新 AST 节点加入自己的方法后，Rust 仍需显式更新本文件的映射，否则会得到 `UNKNOWN`。

`pkg/util/sem/v2/sql_rule_test.rs::test_sql_rules` 与 Go 的 `sql_rule_test.go::TestSQLRules` 使用同一组真实 SQL：TTL 创建/修改/移除、表 ATTRIBUTES、external-id 兼容规则、SELECT INTO OUTFILE、本地路径与 `file://`、服务端/客户端 LOAD DATA。Rust 另有 `go_merge_18_show_storage_class_transitions_command` 验证新增 SHOW 子类型映射；`migration_aster_unit_test.rs::migration_restricted_sql_commands_and_rules_match_go` 覆盖命令规范化、真实 AST 命令映射与 TTL 规则联合生效。

## 扩展指南

新增命名规则时，应同时：实现签名为 `fn(&dyn ast::Node) -> bool` 的纯判定函数；在 `sqlRuleNameMap` 注册稳定的配置 key；在独立的 `sql_rule_test.rs` 增加正例、反例、错误输入和相邻类型用例；同步 Go 对照实现及 `sql_rule_test.go`，或明确记录有意差异。因为 `config.rs::validateSEMConfig` 与 `sem.rs::buildSEMSqlValidateFunction` 共享注册表，正常情况下不应另建第二份白名单。

新增或调整 SQL AST 类型时，先判断命令是否依赖字段：一对一类型加入 `fixed_command!`；DROP/INSERT/EXPLAIN 一类动态分类放在宏之前；SHOW/ADMIN/BRIE 新枚举补专用 match。同步检查 Go 的 `pkg/parser/ast/sem.go`、Rust 命令常量/trait 实现 `pkg/parser/ast/sem.rs`，并在 `sql_rule_test.rs` 添加具体命令断言。尤其要测试未知或默认枚举值应返回何种命令，避免无意把 `UNKNOWN` 变成受限或放行命令。

修改本地导入判断时，应复用 `objstore` 的 URL 解析和本地性定义，不在本文件复制 scheme 列表；测试至少覆盖裸路径、`file://`、远端 scheme、无效 URL、IMPORT FROM SELECT、LOAD DATA LOCAL 与服务端 LOAD DATA。规则的失败默认是 `false`，改变这一点会有兼容和安全语义风险。

性能上，`semCommand` 每次返回新 `String`，并多次执行 `Any` 类型检查；当前路径是每条受检查 SQL 一次，修改时应避免增加 I/O、锁或重复解析。若要重构为 AST trait 分派，必须证明与 Go 的动态命令分类、`CommandStatement` 适配和所有现有映射完全等价，而不能只以编译通过作为证据。

## 验证依据

- 源文件：`pkg/util/sem/v2/sql_rule.rs`，核对全部 480 行、25 个索引符号、公开/私有边界、五个规则和完整命令映射。
- crate 与模块：`pkg/util/sem/v2/Cargo.toml`、`pkg/util/sem/v2/lib.rs`；确认 crate 名、`ast`/`objstore` 依赖、公共重导出和独立测试装配。该目录不存在 `doc.go`。
- 运行时与配置：`pkg/util/sem/v2/config.rs`、`pkg/util/sem/v2/sem.rs`；确认规则名校验、闭包构建、命令规范化及 `IsRestrictedSQL` 调用链。
- Go 对照：`pkg/util/sem/v2/sql_rule.go`、`pkg/util/sem/v2/sem.go`、`pkg/parser/ast/sem.go`；确认规则语义以及 Go 的 `stmt.SEMCommand()` 分派方式。
- Rust AST 对照：`pkg/parser/ast/sem.rs`；确认命令常量与 AST `SEMCommand` trait 实现的位置。
- 测试：`pkg/util/sem/v2/sql_rule_test.rs`、`pkg/util/sem/v2/sql_rule_test.go`、`pkg/util/sem/v2/migration_aster_unit_test.rs`、`pkg/util/sem/compat/sem_integration_test.rs`，以及配置未知规则用例 `pkg/util/sem/v2/config_test.rs`/`.go`。
- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件；`files --filter pkg/util/sem/v2` 覆盖目标及相邻 Go/Rust 文件；`node --file ...sql_rule.rs` 读取全文件；`query` 定位 `CommandStatement`、`sqlRuleNameMap`、`TimeToLiveSQLRule`、`ImportFromLocalRule`、`semCommand`；`callees` 验证上述关键下游边。图未识别静态表引用的完整 callers，已用直接源码引用补证。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核没有把未知节点、解析失败或兼容占位描述成已受限。
