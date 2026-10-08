# `pkg/sessionctx/variable/removed.rs`

## 文件定位

该文件属于 `astersql-sessionctx-variable` crate（见 `pkg/sessionctx/variable/Cargo.toml`），保存已经从系统变量注册表移除、但仍需要被识别并给出迁移说明的变量名。crate 根 `pkg/sessionctx/variable/lib.rs` 以 `pub mod removed` 暴露整个模块，并单独再导出 `CheckSysVarIsRemoved`。

它位于“查不到正常系统变量”之后的兼容诊断路径，而不是系统变量的正常注册、读取或写入路径。当前 Rust 生产调用证据位于 `pkg/planner/core/expression_rewriter.rs::rewriteSystemVariable`：表达式重写器在 `GetSysVar` 返回空时先调用 `CheckSysVarIsRemoved`，从而把已移除变量与普通未知变量区分开。仓库内没有找到 Rust SET 执行路径对 `IsRemovedSysVar` 的生产调用；Go 对照实现则在 `pkg/executor/set.go::SetExecutor.setSysVariable` 中使用它实现 SET 场景的兼容忽略。

## 核心职责

- `REMOVED_SYS_VARS` 集中维护 13 个已移除变量及其移除原因或替代建议，避免调用者只得到笼统的“未知系统变量”。
- `IsRemovedSysVar` 提供无错误分配的成员判断，适用于只需要知道变量是否已移除的调用者。
- `CheckSysVarIsRemoved` 将同一份表转换为面向用户的诊断文本；命中时返回 `Err(String)`，未命中时返回 `Ok(())`。
- 该模块只负责识别和解释，不注册变量、不保存变量值，也不决定 SET、SHOW 或 SELECT 各自如何处理结果；这些策略由上层调用者决定。

## 主要符号

- `pub const REMOVED_SYS_VARS: &[(&str, &str)]`：静态只读切片。每一项的第一个元素是规范化的小写变量名，第二个元素是原因或替代建议。当前项目包括已永久启用的能力、由统一内存配额替代的细粒度变量、停止支持的 streaming，以及由其他变量替代的配置。
- `pub fn IsRemovedSysVar(var_name: &str) -> bool`：用 `iter().any(...)` 做精确字符串比较。名称沿用 Go 导出函数，因此虽不符合 Rust snake_case 习惯，仍由 crate 根的 `#![allow(non_snake_case)]` 接受。
- `pub fn CheckSysVarIsRemoved(var_name: &str) -> Result<(), String>`：用 `iter().find(...)` 找到原因；命中时生成 `option '<name>' is no longer supported. Reason: <reason>`，否则成功返回空值。

文件没有结构体、枚举、trait、`impl`、宏或条件编译项。三个模块级符号均为公开 API，但 `lib.rs` 只在 crate 根直接再导出 `CheckSysVarIsRemoved`；常量和布尔检查函数通过 `removed::...` 路径访问。

## 执行流程

1. 上层先按正常系统变量注册表查找名称。Rust 的 `rewriteSystemVariable` 会先执行 `Name.to_lowercase()`，再调用 `variable::GetSysVar(&name)`。
2. 如果正常变量存在，本文件完全不参与；后续继续执行作用域、权限、noop 警告等检查。
3. 如果正常变量不存在，planner 调用 `CheckSysVarIsRemoved(&name)`。
4. `CheckSysVarIsRemoved` 从头线性扫描 `REMOVED_SYS_VARS`。命中时取出同项的原因并格式化错误字符串；未命中时返回 `Ok(())`。
5. planner 将命中的字符串包装成通用 `errors::Error`；未命中则构造 `ErrUnknownSystemVar`。因此 SELECT/表达式引用已移除变量时能得到具体原因，普通拼错名称仍得到未知变量错误。

`IsRemovedSysVar` 走相同的线性精确匹配，但只返回布尔值。Go 的 SET 执行器在正常注册表查找失败后通过该函数判断：命中即直接成功返回，实现“parse-but-ignore”；当前仓库搜索没有发现等价的 Rust SET 生产接线。

## 数据与状态

全部业务数据编译进 `REMOVED_SYS_VARS`，元素和切片均为 `'static` 字符串引用。模块没有可变全局状态、缓存、会话字段或持久化数据；两个函数都是由输入名称和静态表决定结果的纯查询。

匹配严格区分大小写。`error_1_aster_unit_test.rs::noop_removed_and_hint_metadata_match_go_behavior` 证明小写 `tidb_enable_streaming` 命中，而大写 `TIDB_ENABLE_STREAMING` 不命中。生产 planner 在调用前转成小写，所以 SQL 表达式路径通常不受这一内部约束影响；任何新的直接调用者都必须自行完成与其输入协议一致的规范化。

表当前仅有 13 项，两个 API 的时间复杂度都是 O(n)，额外空间为 O(1)。只有 `CheckSysVarIsRemoved` 命中时的 `format!` 会分配新的错误字符串。表较小时线性切片保持顺序、内容和 Go 映射容易人工对照；若规模显著增长，改变数据结构前应先衡量初始化成本、查找频率和 Go 对齐要求。

## 依赖与调用关系

本文件只依赖 Rust 标准库的切片迭代器、`Result`、`String` 和 `format!`，没有第三方 crate 依赖或 feature 开关。`pkg/sessionctx/variable/Cargo.toml` 将其所在 crate 定义为 `astersql-sessionctx-variable`，`lib.rs` 是 crate 入口并声明 `pub mod removed`。

已验证的 Rust 上游关系如下：

- `pkg/sessionctx/variable/lib.rs` 声明模块，并将 `CheckSysVarIsRemoved` 再导出到 crate 根。
- `pkg/planner/core/expression_rewriter.rs::rewriteSystemVariable` 在正常 `GetSysVar` 查找失败时调用再导出的检查函数；这是当前可见的生产调用边。
- `pkg/sessionctx/variable/removed_test.rs::test_removed_opt` 直接测试两个 API。
- `pkg/sessionctx/variable/tests/main_test.rs::removed_sysvar_registry_is_available_to_the_test_package` 通过外部 crate 视角测试模块路径、根再导出和错误原因。
- `pkg/sessionctx/variable/error_1_aster_unit_test.rs::noop_removed_and_hint_metadata_match_go_behavior` 补充验证精确大小写和原因文本。

下游关系仅为遍历 `REMOVED_SYS_VARS` 和构造字符串，没有 I/O、网络、存储或其他子系统调用。

## 错误处理与边界

未命中不是本模块的错误：`CheckSysVarIsRemoved` 返回 `Ok(())`，由调用者继续生成未知变量错误或采用其他策略。命中时错误文案同时包含原输入名称和表内原因，便于用户确认名称有效但功能已移除，并找到替代方案。

空字符串、拼写错误、大小写不同或尚未列入表的名称都会视为“未移除”。模块不会先检查正常系统变量表，因此若同一个名称同时存在于正常注册表和移除表，直接调用仍会报告已移除；维护者必须保持两份注册数据互斥，上层当前通过“先 `GetSysVar`、失败后再检查”的顺序降低了生产路径上的冲突影响。

Rust 实现返回普通 `String`。Go 的 `CheckSysVarIsRemoved` 使用 `ErrVariableNoLongerSupported.GenWithStackByArgs`，携带标准错误码 8136；Rust 的 `pkg/sessionctx/variable/error.rs` 虽已定义同名 `ErrorDescriptor`（类别 `VARIABLE`、代码 8136），本文件尚未使用它。因此当前 Rust 路径保留了用户可见文本，但是否保留标准错误类别、代码和堆栈语义没有在本文件中实现，不能宣称与 Go 完全等价。

## 并发与资源生命周期

静态切片在进程整个生命周期内只读存在，函数不修改共享数据，天然可被多线程并发调用，无需锁、原子变量、通道或异步任务。返回的布尔值和空成功值不持有资源；错误字符串归调用者所有，随普通 Rust 所有权规则释放。

该文件不打开文件、连接、事务或后台任务，也没有取消和清理协议。并发安全依赖于表保持不可变；如果未来引入运行时可配置注册表，就必须重新设计同步、快照一致性以及读写生命周期，不能直接沿用当前无锁假设。

## 与 Go 版本的对应关系

`pkg/sessionctx/variable/removed.go` 是直接语义来源。两边当前包含相同的 13 组变量名与原因，并都提供 `IsRemovedSysVar` 和 `CheckSysVarIsRemoved`。Rust 用静态元组切片替代 Go 的 `map[string]string`：结果一致，但 Go 平均为常数时间查找，Rust 当前为线性查找。

`pkg/sessionctx/variable/removed_test.rs::test_removed_opt` 对齐 Go 的 `pkg/sessionctx/variable/removed_test.go::TestRemovedOpt`：仍支持的 `TiDBEnable1PC` 不报错且不属于移除表，`tidb_enable_alter_placement` 报错且属于移除表。Rust 的外部测试还检查 streaming 的原因文本。

应用接线尚有两个可见差异：第一，Go 的 `pkg/executor/set.go::SetExecutor.setSysVariable` 对移除变量执行 SET 时成功忽略，仓库内未找到 Rust 对应生产调用；第二，Go 返回标准 8136 数据库错误，Rust 当前仅返回字符串。SELECT/系统变量表达式路径两边都会先把名称转成小写，正常注册表未命中后再用本模块生成比“未知变量”更具体的错误；对应实现分别是 Go 与 Rust 的 `rewriteSystemVariable`。

## 扩展指南

新增或调整已移除变量时，应同时修改 `REMOVED_SYS_VARS` 和 `pkg/sessionctx/variable/removed.go::removedSysVars`，保持名称、原因和替代建议逐项一致，并确认该名称已不在正常系统变量注册表中。新增名称应使用调用链采用的规范化小写形式，原因文本应能直接指导迁移。

测试必须放在独立测试文件，不要嵌入 `removed.rs`。至少同步扩展 `pkg/sessionctx/variable/removed_test.rs` 与 Go 的 `removed_test.go`；若修改错误文本、大小写或公共可见性，还应更新 `pkg/sessionctx/variable/tests/main_test.rs` 和相关的 `error_1_aster_unit_test.rs` 断言。应覆盖命中、未命中、原因文本和大小写边界。

若要补齐 Go SET 的 parse-but-ignore 行为，应在 Rust SET 执行器的“正常系统变量不存在”分支接线，而不是让本文件自行吞掉错误；同时需要执行器级独立回归测试，确认 SET 被忽略而 SELECT 仍报具体错误。若要补齐标准错误兼容，应评估将返回类型改为项目错误类型并使用 `error.rs::ErrVariableNoLongerSupported`，同时检查 planner 的 `.map(errors::New)` 适配及错误码断言。两类改动都属于运行时行为修改，不是简单的数据表扩展。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/sessionctx/variable` 确认目标源、独立测试和 Go 对照均在索引中。
- RustCodeGraph `explore "pkg/sessionctx/variable/removed.rs RemovedSysVarError check_removed_sys_var"` 与 `node --file pkg/sessionctx/variable/removed.rs`：核对 77 行完整实现、三个公开符号，并得到测试调用关系。
- RustCodeGraph `node --file pkg/planner/core/expression_rewriter.rs --offset 3345 --limit 65`：核对生产调用顺序、调用前小写化、移除错误与未知变量错误的分流。
- RustCodeGraph 对 `pkg/sessionctx/variable/tests/main_test.rs`、`error_1_aster_unit_test.rs` 的节点读取：核对外部可见性、原因文本和大小写边界。
- 直接读取 `pkg/sessionctx/variable/Cargo.toml`、`lib.rs`、`removed.go`、`removed_test.rs`、`removed_test.go`、`error.rs`，以及 Go 的 `pkg/executor/set.go` 和 `pkg/planner/core/expression_rewriter.go`：核对 crate 边界、再导出、Go 映射/错误码、SET 与 SELECT 语义和测试对应关系。
- 仓库范围符号搜索 `REMOVED_SYS_VARS|IsRemovedSysVar|CheckSysVarIsRemoved|removed::`：确认当前可见调用位置，并发现 Rust planner 生产调用与 Go SET 调用的接线差异。
- 本任务是纯文档分析，按计划不运行 Cargo；最终使用任务指定的标题计数命令验证文件存在且恰好包含 11 个固定二级章节。
