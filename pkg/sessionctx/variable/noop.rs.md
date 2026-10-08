# `pkg/sessionctx/variable/noop.rs`

## 文件定位

本文件属于 `astersql-sessionctx-variable` crate。crate 入口 `pkg/sessionctx/variable/lib.rs` 以公开模块 `pub mod noop` 暴露它；`pkg/sessionctx/variable/Cargo.toml` 的 `[lib] path = "lib.rs"` 说明它不是独立 crate，而是会话变量子系统的一部分。

它保存 MySQL 客户端可能探测或设置、但 AsterSQL/TiDB 暂不实现实际功能的兼容系统变量元数据。运行时接线位于 `pkg/sessionctx/variable/sysvar_builtins.rs`：`register_builtin_sysvars` 在其他完整变量注册完成后调用 `register_noop_compatibility_vars`，后者把本文件的静态表转换为通用 `SysVar` 并仅补注册尚不存在的名字。因此，本文件位于“兼容元数据源”而非具体 SQL 执行逻辑层。

## 核心职责

1. 用 `NOOP_SYS_VARS` 保存 423 条 noop 系统变量的名称、作用域、默认值、类型、范围、枚举值、别名及若干行为标记。这里的“noop”表示变量可被系统变量框架识别，但设置它通常不改变数据库功能；兼容目的也写在 Go 文件 `pkg/sessionctx/variable/noop.go` 的表注释中。
2. 用 `Scope`、`SysVarType` 和 `NoopSysVar` 提供比运行时 `SysVar` 更适合静态表声明的轻量模型，再由 `sysvar_builtins.rs::noop_sys_var` 转换为 `vardef::ScopeFlag`、`vardef::Type*` 和拥有所有权的字符串集合。
3. 用 `NoopSysVar::validate` 表达少量高风险兼容变量的独立校验语义：只读/离线类能力受 `NoopMode` 控制，`secure_auth` 不允许关闭。
4. 用 `register_noop_sysvars` 返回静态表的拥有副本，供测试或需要独立快照的调用者使用。当前仓库搜索只发现 `pkg/sessionctx/variable/error_1_aster_unit_test.rs` 直接调用它；生产注册路径直接遍历 `NOOP_SYS_VARS`。

## 主要符号

- `Scope::{None, Global, Session, GlobalAndSession}`：本地作用域枚举。它与 `vardef::ScopeNone`、`ScopeGlobal`、`ScopeSession` 及两者按位或的映射集中在 `sysvar_builtins.rs::noop_sys_var`。
- `NoopMode::{Off, On, Warn}`：对 `tidb_enable_noop_functions` 三种语义状态的抽象。`Off` 拒绝开启受保护功能，`Warn` 对会话类赋值放行并返回警告，`On` 放行。
- `SysVarType::{String, Bool, Unsigned, Int, Enum}`：静态表支持的五种类型；由 `noop_sys_var` 映射到通用变量类型。
- `ValidationResult { value, warning }`：校验成功值；保留规范化字符串，并可携带警告。
- `ValidationError { value, message }`：校验失败值；同时提供建议回落值和错误文本。本类型是本文件自有结果，不等同于运行时 `VariableError`。
- `NoopSysVar`：单条元数据。`aliases` 表示等价变量名；`auto_convert_negative_bool`、`is_hint_updatable_verified`、`read_only` 原样传入通用 `SysVar`；字符串形式的 `min_value`/`max_value` 由转换层解析。
- `NoopSysVar::validate(...)`：本文件唯一包含分支行为的方法，输入已规范化值、原始值、赋值作用域、noop 模式及 `offline` 标记。
- `SECONDS_PER_YEAR`：值为 31,536,000；转换层识别表中的字符串标记 `"secondsPerYear"` 并替换为该常量。
- `NOOP_SYS_VARS`：423 项的只读静态切片。首部含 `tx_read_only` 等受保护变量，末部含复制兼容变量；条目不能被当成真实功能实现。
- `register_noop_sysvars() -> Vec<NoopSysVar>`：克隆整个静态切片。每个条目只含静态字符串和小型枚举，克隆不会共享可变状态。

本文件没有 trait、宏、异步函数或条件编译项；所有上述类型、常量、静态表和注册辅助函数均为公开符号。

## 执行流程

运行时注册流程如下：

1. `SessionVars::new`（`pkg/sessionctx/variable/variable.rs`）调用 `register_builtin_sysvars`。
2. `register_builtin_sysvars` 受 `std::sync::Once` 保护，先注册具备完整行为的系统变量，最后调用 `register_noop_compatibility_vars`。
3. `register_noop_compatibility_vars` 遍历 `crate::noop::NOOP_SYS_VARS`。若 `GetSysVar(variable.name)` 已存在，则保留先前的完整定义；否则调用 `noop_sys_var` 转换并 `RegisterSysVar`。
4. `noop_sys_var` 映射作用域和类型，解析数值上下界，复制枚举值与别名，设置 `IsNoop = true`，并为 `init_slave` 设置敏感标记。
5. 通用系统变量注册表随后负责 `SHOW`、查找和 `SET` 的标准处理；本文件本身不访问会话、全局存储或 SQL 执行器。

`NoopSysVar::validate` 的独立流程是：先判断名称是否属于 `tx_read_only`、`transaction_read_only`、`offline_mode`、`super_read_only`、`read_only` 或 `sql_auto_is_null`，且规范化值是否为 `1`/`ON`；再按 `offline`/名称选择报错中的功能名，并根据作用域和 `NoopMode` 拒绝、警告或放行。之后单独拒绝把 `secure_auth` 设置为 `0`/`OFF`；其余输入原样成功返回。

需要注意，`sysvar_builtins.rs::noop_sys_var` 当前没有把 `NoopSysVar::validate` 安装到生成的 `SysVar.Validation`。仓库内对该方法的直接调用仅见独立测试；`TestReadOnlyNoop`、`TestSecureAuth` 和 `TestSQLAutoIsNull` 覆盖的是较早注册的完整运行时定义。这是当前接线事实，不应推断所有由兼容表补注册的变量都会执行本方法。

## 数据与状态

`NOOP_SYS_VARS` 是编译期静态只读数据，条目使用 `&'static str` 和静态切片，不含堆内可变状态。`register_noop_sysvars` 创建 `Vec<NoopSysVar>` 及条目克隆；字段引用仍指向静态字符串，因此不存在借用外部会话的生命周期问题。

表中元数据决定通用系统变量的可见外形：

- `scope` 决定变量能否作为全局、会话或只读展示项访问。
- `value` 是兼容默认值；它不代表对应 MySQL 功能已实现。
- `var_type`、上下界和 `possible_values` 交给通用 `SysVar` 的标准规范化/范围检查使用。
- `aliases` 保留如 `tx_read_only` 与 `transaction_read_only` 的对应关系。
- 三个布尔标记分别控制负布尔值兼容转换、`SET_VAR` hint 可更新性验证和只读属性。

本文件不保存当前变量值。会话值、全局访问器和全局 `SYS_VARS` 注册表分别由 `SessionVars`、`GlobalVarAccessor` 和 `pkg/sessionctx/variable/variable.rs` 管理。

## 依赖与调用关系

本文件只依赖 Rust 标准库隐式提供的 `String`、`Vec`、`Option`、`Result` 和格式化能力，没有直接使用 `Cargo.toml` 中的外部 crate。其下游关系是 `validate` 构造 `ValidationResult`/`ValidationError`，`register_noop_sysvars` 克隆 `NOOP_SYS_VARS`。

已核实的上游关系包括：

- `pkg/sessionctx/variable/lib.rs` 公开声明 `noop` 模块。
- `pkg/sessionctx/variable/sysvar_builtins.rs::noop_sys_var` 消费 `NoopSysVar` 的全部元数据字段。
- `register_noop_compatibility_vars` 直接遍历 `NOOP_SYS_VARS`，通过 `GetSysVar` 和 `RegisterSysVar` 补齐运行时注册表。
- `register_builtin_sysvars` 在 `Once::call_once` 内调用上述补注册函数；`SessionVars::new` 是触发该幂等初始化的主要入口之一。
- `pkg/sessionctx/variable/sysvar_test.rs::TestNoopCompatibilitySysVarsAreRegistered` 遍历静态表，断言每个名字已注册，并检查 `event_scheduler` 的全局、noop 和可设置属性。
- `pkg/sessionctx/variable/error_1_aster_unit_test.rs::noop_removed_and_hint_metadata_match_go_behavior` 通过 `register_noop_sysvars` 检查表未被大幅缩减、别名及 `Off`/`Warn` 分支。

RustCodeGraph 将本文件识别为 9 个符号，并报告被多个文件使用；但对 `register_noop_sysvars`、`NoopSysVar` 和 `validate` 的精确 `callers/callees` 查询未返回调用边，因此上述精确关系以 `rg` 和相邻源码为补充证据。

## 错误处理与边界

`validate` 不会 panic。受保护变量开启失败时返回 `ValidationError`，并把回落值固定为 `OFF`；`Warn` 只在会话或 `GlobalAndSession` 输入作用域分支生成警告。全局作用域只有 `Off` 明确拒绝，`Warn` 不返回警告。`Scope::None` 不触发这些作用域分支。比较使用 ASCII 大小写无关的 `1`/`ON` 与 `0`/`OFF`，其他文本由上游规范化或类型校验负责。

`secure_auth` 关闭时，错误中的 `value` 保留传入的规范化值，消息中保留 `original`，便于向用户报告原始输入。除此之外，该方法不验证类型、数值范围、枚举成员、作用域是否合法，也不处理别名同步。

真正可能 panic 的边界位于相邻转换函数 `noop_sys_var`：若表内 `min_value`/`max_value` 既不是受支持的符号常量也不能解析为整数，注册时会以 `invalid noop sysvar minimum/maximum` panic。这意味着编辑静态表时必须保证字符串格式可解析。重复名称不会覆盖先注册的完整实现，因为 `register_noop_compatibility_vars` 先调用 `GetSysVar`。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务、文件句柄或网络资源。静态表在进程整个生命周期内有效，读取无需同步；返回的 `Vec` 由调用者拥有并按普通 Rust 所有权释放。

并发安全由调用侧承担：`register_builtin_sysvars` 使用 `std::sync::Once` 保证整套内置注册只执行一次，通用 `SYS_VARS` 使用 `LazyLock<RwLock<HashMap<...>>>` 保存注册结果。因此多会话构造不会反复克隆和覆盖这 423 条兼容定义。本文件的 `validate` 仅使用输入参数和局部字符串，没有共享状态，可并发调用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/sessionctx/variable/noop.go`。两边都以大表表达“为 MySQL 兼容而存在、设置后无实际功能”的变量，并保留作用域、默认值、类型、范围、枚举值、别名及标记。Rust 表实测含 423 个 `NoopSysVar` 条目；首尾变量及特殊常量映射与 Go 表相对应。

Go 使用完整 `SysVar` 结构和闭包：`tx_read_only` 等变量的 `Validation` 调用 `checkReadOnly`，`secure_auth`、字符集等条目也可各自附带校验。Rust 则把大部分元数据压缩进 `NoopSysVar`，再在 `sysvar_builtins.rs` 转成通用 `SysVar`。Rust 的 `NoopSysVar::validate` 对齐了只读/离线、`sql_auto_is_null` 和 `secure_auth` 的部分语义，但当前转换函数没有把它接入通用 `Validation`，而字符集等 Go 条目中的专用闭包也不由本文件表示。

这不等同于相关运行时行为全部缺失：注册顺序会优先保留 `sysvar_builtins.rs` 中已实现的同名完整变量。Go `sysvar_test.go::TestReadOnlyNoop`、`TestSQLAutoIsNull` 的意图在 Rust `sysvar_test.rs` 中有对应测试；Rust 静态表自身及独立校验辅助逻辑则由 `error_1_aster_unit_test.rs::noop_removed_and_hint_metadata_match_go_behavior` 覆盖。新增或修改条目时必须同时判断它是单纯兼容元数据，还是需要在完整注册层实现 Go 的 hook/校验。

## 扩展指南

新增普通 noop 变量时，应在 `NOOP_SYS_VARS` 中按 Go `noopSysVars` 的真实字段加入条目，并核对 `scope`、默认值、`SysVarType`、上下界、枚举值、别名和三个标记。若上下界使用符号字符串，还必须同步扩展 `sysvar_builtins.rs::noop_sys_var` 的解析分支；不要填入无法解析的说明性文本。

若变量需要会话状态、全局持久化、警告、专用错误、setter/getter 或实际功能，不能只增加静态条目。应在 `sysvar_builtins.rs` 的完整注册阶段创建 `SysVar` 并安装对应 hook，使其在 noop 补注册之前存在；必要时再扩展 `NoopSysVar::validate`，但必须明确把结果适配为运行时 `VariableError`/警告机制，不能假定现有方法会自动生效。

测试逻辑必须继续放在独立文件。元数据与注册覆盖宜扩展 `pkg/sessionctx/variable/sysvar_test.rs::TestNoopCompatibilitySysVarsAreRegistered`；本地表和 `NoopMode` 分支宜扩展 `pkg/sessionctx/variable/error_1_aster_unit_test.rs::noop_removed_and_hint_metadata_match_go_behavior` 或新建同目录独立 `*_test.rs`；Go 对齐行为应参考 `pkg/sessionctx/variable/sysvar_test.go`。重点风险是错误地宣称功能已实现、覆盖已有完整定义、破坏别名/作用域兼容，以及使字符串上下界在进程首次注册时 panic。静态表规模还会增加启动期一次性转换和注册成本，但查找成本由通用哈希表承担。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/sessionctx/variable` 确认目标及测试；`node --file pkg/sessionctx/variable/noop.rs --offset 1 --limit 240` 与尾段查询确认文件符号、校验分支、静态表和注册函数；精确 `callers/callees` 查询无返回，已记录为图覆盖限制。
- 源文件：`pkg/sessionctx/variable/noop.rs`，重点为 `Scope`、`NoopMode`、`SysVarType`、`ValidationResult`、`ValidationError`、`NoopSysVar::validate`、`SECONDS_PER_YEAR`、`NOOP_SYS_VARS` 和 `register_noop_sysvars`。
- crate 与接线：`pkg/sessionctx/variable/Cargo.toml`、`pkg/sessionctx/variable/lib.rs`、`pkg/sessionctx/variable/sysvar_builtins.rs::{noop_sys_var,register_noop_compatibility_vars,register_builtin_sysvars}`、`pkg/sessionctx/variable/variable.rs::{SessionVars::new,RegisterSysVar,SYS_VARS}`。
- Go 对照：`pkg/sessionctx/variable/noop.go::noopSysVars`，以及 `pkg/sessionctx/variable/sysvar_test.go::{TestReadOnlyNoop,TestSQLAutoIsNull}`。
- Rust 独立测试：`pkg/sessionctx/variable/error_1_aster_unit_test.rs::noop_removed_and_hint_metadata_match_go_behavior`、`pkg/sessionctx/variable/sysvar_test.rs::{TestReadOnlyNoop,TestSecureAuth,TestSQLAutoIsNull,TestNoopCompatibilitySysVarsAreRegistered}`。
- 文本检索确认 Rust 表中有 423 个 `NoopSysVar` 条目；确认 `NoopSysVar::validate` 的直接调用只出现在独立测试，生产注册直接遍历 `NOOP_SYS_VARS`。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前另运行任务指定的 11 章节结构校验，并人工复核只有 `pkg/sessionctx/variable/noop.rs.md` 为文档产物。
