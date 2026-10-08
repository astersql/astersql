# `pkg/sessionctx/vardef/sysvar.rs`

## 文件定位

[`sysvar.rs`](sysvar.rs) 是 `astersql-sessionctx-vardef` crate 中的 MySQL/TiDB 系统变量名称词汇表。crate 根文件 [`lib.rs`](lib.rs) 以私有模块 `mod sysvar` 装入它，再通过 `pub use sysvar::*` 将全部常量暴露给上层；因此调用方通常写 `vardef::AutoCommit` 或 `astersql_sessionctx_vardef::CharacterSetConnection`，而不直接访问 `sysvar` 模块。

该文件处于“名称定义”和“行为实现”的边界：它为系统变量注册、`SET`/`SHOW`、表达式求值上下文、会话规划以及诊断输出提供统一且大小写固定的键，但不定义变量的默认值、作用域、校验器、读写钩子或持久化方式。这些行为主要位于相邻的 [`tidb_vars.rs`](tidb_vars.rs)、上层 `pkg/sessionctx/variable` 以及具体会话/表达式实现中。

## 核心职责

1. 提供 159 个 `&'static str` 常量，消除系统变量名和少量枚举取值在调用方中的重复字面量。名称覆盖字符集与排序规则、连接和网络、事务、MySQL/InnoDB 兼容项、SQL 执行限制、服务端能力以及 `validate_password` 插件变量。
2. 用 `SetNamesVariables: [&str; 3]` 固定 `SET NAMES` 同时影响的 `character_set_client`、`character_set_connection`、`character_set_results` 顺序。
3. 用 `SetCharsetVariables: [&str; 2]` 固定 `SET CHARACTER SET`/`SET CHARSET` 影响的 client 与 results 两项；connection 的处理由语句语义另行决定。
4. 提供非变量名协议值：`MaskPwd` 是敏感值展示掩码，`PessimisticTxnMode` 与 `OptimisticTxnMode` 是 `tidb_txn_mode` 的规范取值。

本文件不是注册表。仅添加一个常量不会让变量自动出现在系统中，也不会赋予 `SET`/`SHOW` 行为；注册和校验仍须由 `pkg/sessionctx/variable` 等消费者完成。

## 主要符号

- `SetNamesVariables: [&str; 3]`：顺序为 `CharacterSetClient`、`CharacterSetConnection`、`CharacterSetResults`。顺序由独立测试 `set_statement_variable_groups_match_go_order` 锁定。
- `SetCharsetVariables: [&str; 2]`：顺序为 `CharacterSetClient`、`CharacterSetResults`。
- `MaskPwd: &str = "******"`：诊断性全局变量快照在遇到非空敏感值时使用它替换原值，见 `pkg/server/handler/tikvhandler/global_variables.rs::global_variables`。
- `PessimisticTxnMode`、`OptimisticTxnMode`：事务模式的规范字符串；`pkg/sessionctx/variable` 的默认值逻辑和测试直接比较这些常量。
- 字符集组：`CharacterSetClient`、`CharacterSetConnection`、`CharacterSetResults`、`CharacterSetServer`，以及 `CollationConnection`、`CollationDatabase`、`CollationServer`、`DefaultCollationForUTF8MB4`。它们既参与注册，也作为表达式和会话路径的分派键。
- 核心会话组：`AutoCommit`、`SQLModeVar`、`TimeZone`、`Timestamp`、`TxnIsolation`、`TransactionIsolation`、`MaxAllowedPacket`、`MaxExecutionTime`、`TiDBDMLMaxExecutionTime` 等。
- 服务器/兼容组：`MaxConnections`、`WaitTimeout`、`NetWriteTimeout`、`Version`、`VersionComment`、`TiDBEnableDDL`、`TiDBEnableStatsOwner`，以及一组保留 MySQL/InnoDB 名称的兼容常量。
- 密码策略组：`ValidatePasswordEnable` 到 `ValidatePasswordDictionary`，对应 `validate_password.*` 的点分名称。

文件共有 161 个公开常量：159 个 `&str` 与上述 2 个定长数组；没有函数、结构体、枚举、trait、类型别名或条件编译项。顶部 `allow` 属性允许沿用 Go/MySQL 风格的导出名，而不是改成 Rust 的全大写常量命名。

## 执行流程

该文件自身没有可执行流程；其运行时作用是被编译期内联的键值参与以下链路：

1. `lib.rs` 重导出常量，消费 crate 通过 `vardef::*` 或 crate 根路径引用。
2. 系统启动/首次使用时，`pkg/sessionctx/variable/sysvar_builtins.rs` 的 `register_builtin_sysvars` 用这些名称构造 `SysVar`，为名称绑定默认值、作用域、数值范围、校验器和钩子。例如 `CharacterSetClient`/`CharacterSetResults` 注册为字符串变量，`AutoCommit` 注册为 session/global 布尔变量。
3. 会话或表达式路径以规范名称查表或分派。`pkg/expression/exprstatic/evalctx.rs::parse_system_vars` 将输入名转为小写后，以 `SQLModeVar`、`TimeZone`、`CharacterSetConnection` 等匹配并解析；未知或非特例变量回落到 `SessionVars::SetSystemVar` 校验。
4. 执行阶段读取同一键。示例：`pkg/session/runtime/planning.rs` 读取 `SQLSelectLimit`、`CharacterSetConnection` 和 `CollationConnection` 形成规划/表达式上下文；`pkg/session/runtime/control.rs` 对 `CharacterSetResults`、`WarningCount`、`ErrorCount` 等实施会话读写分支。
5. 对外输出时复用协议值。`global_variables` 遍历已注册变量，敏感且非空的值统一替换为 `MaskPwd`，避免泄漏凭据。

因此，名称常量的正确性属于跨层协议：注册端和读取端必须使用同一个字面量，但具体错误与状态转换均发生在调用方。

## 数据与状态

所有数据都具有静态生命周期且不可变：`&'static str` 指向二进制中的字符串字面量，两个数组也在编译期由这些静态引用组成。文件不分配堆内存，不包含锁、原子变量、缓存、环境探测或延迟初始化。

名称值均为 MySQL/TiDB 对外可见的小写形式，包括带下划线的常规名称和 `validate_password.enable` 这类点分名称。Rust 标识符保留 Go 的 PascalCase，值才是协议层键；调用方不应把标识符拼写当作 SQL 名称。

数组顺序是可观察数据而非集合细节：批量执行 `SET NAMES`/`SET CHARSET` 时，上层会按该顺序处理。改变长度、成员或顺序都需要视为行为变更并同步语句级测试。

## 依赖与调用关系

`sysvar.rs` 不导入任何 crate，也不调用任何函数。它只依赖同文件内先后声明的字符集常量来构造两个数组；Rust 允许常量引用后文定义的常量。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：包名为 `astersql-sessionctx-vardef`，库入口是 `lib.rs`，可选 `nextgen` feature 仅转发给 `kerneltype/nextgen`。本文件本身不使用该 feature，也不使用 crate 声明的 `chrono`、`kerneltype`、`sysinfo` 依赖。

RustCodeGraph 的文件节点报告 6 个直接使用文件，列出的代表包括：

- `pkg/sessionctx/variable/sysvar_builtins.rs`：把名称绑定到实际 `SysVar` 元数据，是最主要的下游注册端。
- `pkg/sessionctx/variable/session.rs`：会话/全局系统变量处理边界。
- `pkg/session/runtime/planning.rs`：读取字符集、排序规则和 SQL 限制等变量供规划阶段使用。
- `pkg/server/handler/tikvhandler/global_variables.rs`：使用 `MaskPwd` 隐藏敏感全局变量。
- `pkg/server/handler/tikvhandler/global_variables_test.rs`：验证掩码行为。

此外，精确符号搜索确认 `pkg/expression/exprstatic/evalctx.rs`、`pkg/expression/exprstatic/exprctx.rs`、`pkg/session/runtime/control.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/planner/core/expression_rewriter.rs` 等通过 crate 重导出消费常量。RustCodeGraph 对常见常量名存在跨语言同名碰撞，故这些补充边使用路径限定的 `rg` 核验。

## 错误处理与边界

本文件没有返回值、错误类型或 panic 路径。错误处理全部属于消费者：例如表达式上下文根据 `CharacterSetConnection` 查字符集失败时构造无效系统变量错误，`SQLModeVar` 解析失败时传播 SQL mode 错误，注册表负责范围和作用域校验。

主要边界如下：

- “已定义名称”不等于“已完整支持”。一组 InnoDB/MySQL 兼容名可能在注册层是 noop、只读或有限支持；必须查看对应 `SysVar` 注册项后才能声称行为。
- 名称比较通常由调用方先执行 ASCII 小写化或使用不区分大小写比较；常量本身不负责正规化。
- `MaskPwd` 只提供替换文本，不执行敏感性判定。判定依据是注册项的 `IsSensitive`，空敏感值仍保持为空。
- `SetNamesVariables` 与 `SetCharsetVariables` 只表达成员和顺序，不执行赋值、回滚或字符集合法性检查。
- `#![allow(dead_code, ...)]` 表示某些迁移常量当前可能尚无 Rust 调用者；不能以“存在常量”为依据推断路径已经接线。

## 并发与资源生命周期

文件没有运行时资源生命周期。常量在程序整个生命周期内有效，可由任意线程无锁共享；读取不产生竞争、阻塞或所有权转移。数组元素同样是静态字符串引用，复制数组或引用均不需要清理。

并发语义只可能出现在消费者管理的系统变量状态、全局原子配置或会话对象中，不能归因于本文件。例如注册表更新、会话变量写入和全局钩子的同步策略应在 `pkg/sessionctx/variable` 的实现与测试中验证。

## 与 Go 版本的对应关系

直接对照文件是 [`sysvar.go`](sysvar.go)。Rust 保留 Go 的公开标识符和字符串值，并基本保持声明顺序，便于逐项审查。关键表示差异是：

- Go 的 `SetNamesVariables` 和 `SetCharsetVariables` 是可变长度 `[]string` 包变量；Rust 使用不可变、编译期定长的 `[&str; 3]` 与 `[&str; 2]`，从类型层固定成员数并消除运行时分配。
- Go `const` 对应 Rust `pub const &'static str`，语义均是不变字面量；Rust 通过 `lib.rs` 重导出模拟 Go 包级访问方式。
- 名称集合比较显示 Rust 比当前同路径 Go 文件多 `TiDBDMLMaxExecutionTime = "tidb_dml_max_execution_time"`。该项是 Rust 侧额外名称，不能表述为当前 Go 文件的逐项复刻；若调整它，应先查明上游 Go 版本或其他 Go 定义位置。
- `InnodbFtEnableStopword` 的 Go 行尾 `#nosec G101` 在 Rust 中保留为相邻注释；它只是安全扫描抑制说明，不改变字符串值。

独立 Rust 测试 [`runtime_1_aster_unit_test.rs`](runtime_1_aster_unit_test.rs) 中的 `set_statement_variable_groups_match_go_order` 明确校验两个数组的顺序、密码掩码和事务模式值。事务模式还由 `pkg/sessionctx/variable/tests/variable_test.rs::dynamic_global_defaults_match_classic_and_next_gen_go_branches` 与 `pkg/sessionctx/variable/sysvar_test.rs::TestGlobalSystemVariableInitialValue` 覆盖；敏感值的实际使用由 `pkg/server/handler/tikvhandler/global_variables_test.rs` 覆盖。同目录没有专名 `sysvar_test.rs`，当前常量测试遵守“测试与源文件分离”的仓库规则。

## 扩展指南

新增或修改系统变量时，应按以下边界接线：

1. 在本文件加入规范名称常量，并与权威 Go 定义、SQL 对外名称及大小写保持一致；若是 TiDB 专有默认值或运行时状态，应先判断是否更适合放在 `tidb_vars.rs`。
2. 在 `pkg/sessionctx/variable/sysvar_builtins.rs` 或相应注册模块创建/更新 `SysVar`，明确默认值、scope、类型、范围、只读/noop/敏感属性和读写钩子。仅改本文件不足以交付功能。
3. 若名称影响特殊解析或执行分支，同步 `pkg/expression/exprstatic`、`pkg/session/runtime`、planner/server 等实际消费者；不要依靠字符串重复。
4. 若修改 `SET NAMES`/`SET CHARSET` 成员或顺序，更新独立 Rust 测试 `runtime_1_aster_unit_test.rs`，并补充相应语句级/回滚边界测试。其他变量应优先扩展 `pkg/sessionctx/variable/sysvar_test.rs` 或 `pkg/sessionctx/variable/tests/variable_test.rs`；敏感展示变更同步 `global_variables_test.rs`。
5. 兼容性风险集中在拼写、别名、scope 和默认值：改动常量值可能使注册端与读取端失配，也可能改变 SQL 客户端可见协议。性能风险通常很低，但新增特殊分派不应引入每次求值的重复解析或分配。
6. 不要把单元测试内嵌到 `sysvar.rs`；按仓库约定放入独立测试文件，并保持 Go 测试意图和分支覆盖。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标目录已索引；`files --filter pkg/sessionctx/vardef` 找到 `sysvar.rs`、Go 对照和相关独立测试；`node --file pkg/sessionctx/vardef/sysvar.rs --offset 1/261` 完整读取 374 行并报告 6 个使用文件；对常量的 `query/callers/explore` 显示常见名称存在跨语言同名碰撞，因此调用点以路径限定搜索补强。
- 源码：完整检查 `pkg/sessionctx/vardef/sysvar.rs`，确认 161 个 `pub const`，且不存在函数、类型、trait、可变静态项或条件编译项。
- crate：检查 `pkg/sessionctx/vardef/Cargo.toml` 与 `pkg/sessionctx/vardef/lib.rs`，确认包名、入口、`nextgen` feature 和重导出边界。
- Go 对照：检查 `pkg/sessionctx/vardef/sysvar.go`；抽取 Go/Rust 名称集合后确认两个数组的表示差异以及 Rust 额外的 `TiDBDMLMaxExecutionTime`。
- 调用与测试：检查 `pkg/sessionctx/variable/sysvar_builtins.rs`、`pkg/expression/exprstatic/evalctx.rs`、`pkg/session/runtime/planning.rs`、`pkg/server/handler/tikvhandler/global_variables.rs`、`pkg/sessionctx/vardef/runtime_1_aster_unit_test.rs`、`pkg/sessionctx/variable/sysvar_test.rs`、`pkg/sessionctx/variable/tests/variable_test.rs`、`pkg/server/handler/tikvhandler/global_variables_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；最终使用任务指定的标题计数命令做结构验证，并人工复核本说明没有把名称常量误写成已注册或已支持的运行时行为。
