# `pkg/parser/mysql/state.rs`

## 文件定位

`state.rs` 属于 `astersql-parser-mysql` crate，并由 `pkg/parser/mysql/lib.rs` 以公开模块 `pub mod state` 导出。它位于 MySQL 协议错误元数据层：把 `errcode.rs` 定义的 `u16` 数字错误码关联到客户端可见的五字符 SQLSTATE。该文件不解析 SQL，也不负责生成错误消息；相邻的 `error.rs::NewErr` 和 `error.rs::NewErrf` 在构造 `SQLError` 时消费这里的映射。

`pkg/parser/mysql/Cargo.toml` 将 crate 根设为 `lib.rs`。本文件只使用标准库 `std::collections::HashMap` 和同 crate 的 `super::errcode::*`，不直接使用清单中的外部依赖，不访问网络、磁盘或系统表。

## 核心职责

本文件承担两项协议兼容职责：

1. `DefaultMySQLState` 提供未分类错误的通用回退值 `HY000`。
2. `MySQLState()` 建立 243 个已知错误码到 SQLSTATE 的显式映射，保留 Go 版本选择的具体分类，而不是根据错误名或码段动态推断。

这些状态码让 MySQL 客户端在数字错误码和消息之外判断错误类别。例如 `ErrDupEntry` 映射为完整性约束类 `23000`，`ErrNoSuchTable` 映射为表不存在 `42S02`，`ErrLockDeadlock` 映射为事务回滚类 `40001`，`ErrInvalidJSONText` 映射为 JSON 数据异常 `22032`。分类是协议元数据，修改会改变客户端所观察到的错误语义。

## 主要符号

- `pub const DefaultMySQLState: &str = "HY000"`：静态生命周期的默认 SQLSTATE。`error.rs::NewErr` 和 `NewErrf` 在数字码没有专用映射时将其复制到 `SQLError.State: String`。
- `pub fn MySQLState() -> HashMap<u16, &'static str>`：公开的映射构造函数。键来自 `errcode.rs` 的错误码常量，值是静态五字符字符串；数组经 `.into_iter().collect()` 生成拥有所有权的 `HashMap`。
- 映射条目没有独立类型、trait、impl 或条件编译分支。文件也没有私有辅助函数和可变全局变量。

函数名和常量名保留 Go 风格大小写；crate 根 `lib.rs` 通过 `#![allow(non_snake_case, non_upper_case_globals)]` 允许这种迁移接口命名。

## 执行流程

直接调用链以错误构造为入口：

1. 调用方把 `u16` 错误码及格式化参数传给 `error.rs::NewErr`，或把错误码、自定义格式串、脱敏位置和参数传给 `error.rs::NewErrf`。
2. 两个函数都调用 `state.rs::MySQLState()`；函数遍历静态写出的键值对并收集成一个新的 `HashMap`。
3. 错误构造函数以 `get(&err_code)` 查询映射。命中时复制 `&'static str` 并转换为拥有所有权的 `String`；未命中时复制 `DefaultMySQLState`。
4. 选出的状态写入 `error.rs::SQLError.State`，最终由 `SQLError::Error` / `Display` 组合成 `ERROR <code> (<state>): <message>` 形状。

`MySQLState()` 本身没有业务分支或错误返回；条目的差异完全由表中数据表达。RustCodeGraph 将 `error.rs` 和两份独立测试识别为本文件的直接使用者，并显示 `MySQLState()` 没有函数级被调用项。

## 数据与状态

映射键是 `u16`，与 `errcode.rs` 的协议错误码宽度一致；值是 `&'static str`，来源均为编译进二进制的字符串字面量。当前 243 个条目覆盖的常见类别包括：

- `01xxx` 警告，如 `WarnDataTruncated -> 01000`；
- `08xxx` 连接异常，如 `ErrHandshake -> 08S01`；
- `21xxx` 基数不匹配，如 `ErrWrongValueCount -> 21S01`；
- `22xxx` 数据异常，如 `ErrDivisionByZero -> 22012`；
- `23xxx` 完整性约束，如 `ErrDupEntry -> 23000`；
- `24xxx` 游标状态，如 `ErrSpCursorNotOpen -> 24000`；
- `25xxx` 事务状态，如 `ErrReadOnlyTransaction -> 25000`；
- `40xxx` 事务回滚，如 `ErrLockDeadlock -> 40001`；
- `42xxx` 语法或访问规则，如 `ErrParse -> 42000`；
- MySQL/XA 专用值，如 `ErrQueryInterrupted -> 70100`、`ErrXaerNota -> XAE04`。

每次调用返回独立 `HashMap`；调用者可以修改自己的副本，不会影响后续调用或其他线程。相应代价是每次错误构造都会重新分配并填充 243 个条目，这一点与 Go 的包级 map 生命周期不同。

## 依赖与调用关系

上游直接依赖如下：

- `pkg/parser/mysql/error.rs::NewErr`：查询专用 SQLSTATE，并在缺项时使用默认值。
- `pkg/parser/mysql/error.rs::NewErrf`：使用同一选择规则，为自定义消息模板构造 `SQLError`。
- `pkg/parser/mysql/error_3_aster_unit_test.rs`：抽样验证四类映射以及未知码回退。
- `pkg/parser/mysql/unit_test.rs`：验证已知码存在、未知 `u16::MAX` 不存在，并通过 `NewErr` / `NewErrf` 验证集成路径。

下游依赖只有 `std::collections::HashMap` 与 `pkg/parser/mysql/errcode.rs` 的常量集合。模块装配位于 `pkg/parser/mysql/lib.rs`；Cargo 边界位于 `pkg/parser/mysql/Cargo.toml`。本文件不直接依赖 SQL parser、session、executor 或存储层，它通过 `SQLError` 构造路径间接影响所有向 MySQL 客户端暴露这些错误的上层模块。

## 错误处理与边界

`MySQLState()` 是全函数，不返回 `Result`，当前数据均为编译期字面量，因此没有运行时解析错误。调用方必须处理“数字码不在表中”这一正常边界：`NewErr` 和 `NewErrf` 使用 `HY000`，而直接调用 `MySQLState().get(...)` 的代码得到 `None`。`unit_test.rs::parser_mysql_error_metadata_maps_known_codes` 明确验证 `u16::MAX` 不在表中。

表内的 `HY000` 条目（例如 `ErrSignalException`）与缺项回退结果相同，但含义不同：前者是显式协议选择，后者是未知码兜底。扩展代码不能以状态字符串是否为 `HY000` 判断错误码是否已登记。

`collect()` 遇到重复键时会保留后出现的值而不会报错，因此新增条目时必须通过 Go 对照和测试防止重复或误覆盖。文件当前不校验字符串恰为五字符，正确性依赖显式表、对照检查和测试。

## 并发与资源生命周期

本文件没有锁、原子量、通道、异步任务、事务或外部资源。`DefaultMySQLState` 和映射值都是只读静态字符串；`MySQLState()` 创建的 map 归调用方所有，在返回值离开作用域时正常释放。不同线程调用该函数不会共享 map，也没有数据竞争。

独立所有权简化了并发语义，但会在热错误路径上重复申请和释放 map。若未来为减少分配而改为惰性静态表，必须同时评估公开返回类型、调用方是否依赖可变副本、初始化线程安全及错误路径性能，不能只把函数机械替换为共享引用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/mysql/state.go`。两端都定义 `DefaultMySQLState = "HY000"`，并使用相同的错误码常量和 SQLSTATE 字符串。对 Rust 与 Go 的条目进行规范化后，两边各有 243 项，键值对逐项 `diff` 为空。

主要实现差异是生命周期与类型：Go 使用包级 `var MySQLState = map[uint16]string{...}`，进程初始化后复用同一个 map；Rust 使用 `fn MySQLState() -> HashMap<u16, &'static str>`，每次调用创建新 map，随后 `error.rs` 再把命中的静态字符串转成 `String`。因此协议数据目前一致，但分配行为并非逐字等价。

Rust 的回退逻辑不在 `state.rs` 内，而在 `error.rs::NewErr` 与 `NewErrf` 内显式实现；这与 Go 消费方按缺项回退的职责划分一致。当前证据未显示 Rust 侧自动生成或运行时同步 Go 表，后续新增 Go 错误码时需要人工同步本文件。

## 扩展指南

新增或调整错误状态时，应按以下边界修改：

1. 先确认 `pkg/parser/mysql/errcode.rs` 已存在对应 `u16` 常量，并从权威 MySQL/TiDB 或同路径 Go 行为确认具体 SQLSTATE；不要仅按类别前缀猜测。
2. 在 `MySQLState()` 的数组中按 Go 表相邻位置加入或调整一项，保持 `pkg/parser/mysql/state.go` 与 Rust 键值一致，避免重复键。
3. 在独立测试文件中补充针对该类别的断言。表数据的聚焦回归应放在 `pkg/parser/mysql/error_3_aster_unit_test.rs` 或 `pkg/parser/mysql/unit_test.rs`，不要把测试嵌入 `state.rs`。
4. 若改变未知码回退规则，同时检查 `error.rs::NewErr`、`NewErrf` 和 `SQLError` 展示测试；这会产生客户端兼容风险。
5. 若优化为共享静态映射，保留公开调用语义或逐一迁移调用者，并用基准或分配证据评估收益。当前错误路径每次建表的性能风险是真实存在的，但不属于本说明任务的行为修改范围。

兼容性上最敏感的是同一数字错误码被映射到不同 SQLSTATE；性能上最敏感的是扩大表后继续在每次错误构造时全量分配；正确性上要同时覆盖已知码命中、未知码缺项以及显式 `HY000` 三种情况。

## 验证依据

- RustCodeGraph `status`：索引包含目标仓库，目标文件可读取；查询时间为本任务执行时的本地索引状态。
- RustCodeGraph `node --file pkg/parser/mysql/state.rs`：确认完整 280 行源码、两个公开符号、243 个显式映射条目及 `errcode`/`HashMap` 依赖。
- RustCodeGraph `callees MySQLState`：未发现函数级下游调用；文件使用关系指向 `error.rs`、`error_3_aster_unit_test.rs` 和 `unit_test.rs`。
- `pkg/parser/mysql/error.rs`：`NewErr`（第 64 行起）与 `NewErrf`（第 86 行起）证明查询、复制、回退和写入 `SQLError.State` 的流程。
- `pkg/parser/mysql/lib.rs` 与 `pkg/parser/mysql/Cargo.toml`：证明公开模块装配、crate 名称、库入口和依赖边界。
- `pkg/parser/mysql/state.go`：Go 权威对照；规范化 Rust/Go 条目后，两侧均为 243 项且 `diff -u` 无输出。
- `pkg/parser/mysql/error_3_aster_unit_test.rs::sql_error_matches_go_state_formatting_and_redaction`：验证 `ErrNoDB -> 3D000` 和未知码回退；`sql_states_flags_and_type_defaults_match_go` 抽样验证 `23000`、`42S02`、`40001`、`22032`。
- `pkg/parser/mysql/unit_test.rs::parser_mysql_error_metadata_maps_known_codes`：验证已知条目和未知 `u16::MAX` 缺项；`parser_mysql_sql_error_builds_default_and_custom_messages` 验证通过错误构造器传播状态及默认回退。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另以固定标题命令验证文档恰含十一个要求章节。
