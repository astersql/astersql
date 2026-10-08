# `pkg/util/tiflashcompute/dispatch_policy.rs`

## 文件定位

本文件属于 `astersql-util-tiflashcompute` crate。该 crate 由同目录 `Cargo.toml` 定义，入口是 `pkg/util/tiflashcompute/lib.rs`；入口将 `dispatch_policy` 声明为公开模块并再次导出其全部公开符号。因此，调用方既可以经模块路径访问，也可以从 crate 根导入这些符号。

它位于 TiFlash Compute 系统变量与实际 MPP 调度之间的表示转换边界：本文件只定义策略值、合法字符串及双向转换，不选择计算节点，也不实现轮询或一致性哈希算法。当前 Rust 上游主要通过 `pkg/sessionctx/variable/lib.rs` 对 `tiflashcompute` crate 的再导出接入；系统变量注册、会话状态写入以及会话运行时 SET/GET 路径再调用这里的转换函数。

## 核心职责

- 以 `DispatchPolicy` 及三个常量表达轮询、一致性哈希和非法哨兵值。
- 以 `GetValidDispatchPolicy` 提供可由用户配置的两个规范字符串，且维持固定顺序：`consistent_hash`、`round_robin`。
- 以 `GetDispatchPolicyByStr` 严格解析规范字符串；未知、空字符串、大小写变化或额外空白都不会被自动归一化，而会返回 `DispatchPolicyError`。
- 以 `GetDispatchPolicy` 将策略值转回字符串；所有未识别的整数（包括显式哨兵值 `2`）都稳定降级为 `invalid`。

本文件不是策略执行器。它不持有拓扑、不分发任务，也不访问网络；实际消费策略的调度实现属于其他模块。

## 主要符号

- `pub type DispatchPolicy = isize`：策略的公开整数别名。它不是封闭枚举，因此调用方可以构造任意 `isize`，未知值必须由转换函数的兜底分支处理。
- `DispatchPolicyRR = 0`：轮询策略。数值与 Go `iota` 的首项一致。
- `DispatchPolicyConsistentHash = 1`：一致性哈希策略。
- `DispatchPolicyInvalid = 2`：非法策略的命名哨兵；解析失败时 Rust API 返回 `Err`，不会把该值一并放入 `Result`。
- `DispatchPolicyError(String)`：私有载荷、公开类型的错误。派生 `Debug`、`PartialEq`、`Eq` 和 `thiserror::Error`，显示文本就是内部字符串；外部调用方能传播、显示和比较错误，但不能直接构造或读取载荷。
- `GetValidDispatchPolicy() -> Vec<&'static str>`：每次调用新建一个两元素 `Vec`，元素借用 `vardef` 中的静态常量。
- `GetDispatchPolicyByStr(&str) -> Result<DispatchPolicy, DispatchPolicyError>`：按完整字符串精确匹配两个合法值；错误文本同时带合法列表和原始输入。
- `GetDispatchPolicy(DispatchPolicy) -> &'static str`：将 `1`、`0` 分别映射到规范字符串，其他整数统一映射为 `vardef::DispatchPolicyInvalidStr`。

源文件的 `#![allow(non_snake_case, non_upper_case_globals)]` 保留了 Go 导出符号的命名方式，减少移植调用面的差异。

## 执行流程

解析配置字符串时，调用方把原始 `&str` 传给 `GetDispatchPolicyByStr`。函数先与 `vardef::DispatchPolicyConsistentHashStr` 比较，再与 `vardef::DispatchPolicyRRStr` 比较；命中后分别返回数值 `1` 或 `0`。两者都未命中时，它调用 `GetValidDispatchPolicy`，用单个空格连接合法名称，并构造形如 `unexpected tiflash_compute dispatch policy, expect [consistent_hash round_robin], got <输入>` 的错误。

反向展示时，`GetDispatchPolicy` 对一致性哈希和轮询做显式匹配，返回相应静态字符串。`DispatchPolicyInvalid` 没有单独分支，而是与所有未知整数一起进入 `_` 分支并返回 `invalid`；这保证了损坏或未来未识别值不会被误报成某个有效策略。

应用主链中的典型路径为：`pkg/sessionctx/variable/sysvar_builtins.rs` 注册 `tiflash_compute_dispatch_policy` 并用 `GetDispatchPolicyByStr` 校验、用 `GetDispatchPolicy` 规范化；`pkg/session/runtime/control.rs` 的 SET 路径解析并保存策略值，GET 路径再把值转换成字符串。`pkg/sessionctx/variable/sysvar.rs::setTiFlashComputeDispatchPolicy` 也使用解析函数，并且仅在解析成功后更新 `SessionVars.TiFlashComputeDispatchPolicy`。

## 数据与状态

本文件没有全局可变状态、缓存或对象生命周期。策略值是按值复制的 `isize`；字符串结果是 `vardef` 常量的 `&'static str`。唯一的临时分配发生在：

- `GetValidDispatchPolicy` 每次建立新的 `Vec`；
- 非法解析路径为合法列表执行 `join`，再由 `format!` 创建错误消息 `String`。

关键不变量是数值对应关系 `RR = 0`、`ConsistentHash = 1`、`Invalid = 2`，以及合法列表的固定顺序。该顺序既影响系统变量可选值的展示，也进入精确错误文本，因此改变顺序属于可观察兼容性变化。字符串的单一来源是 `pkg/sessionctx/vardef/tidb_vars.rs`：`round_robin`、`consistent_hash`、`invalid`；默认系统变量值 `DefTiFlashComputeDispatchPolicy` 指向 `consistent_hash`。

## 依赖与调用关系

直接源码依赖只有 `crate::vardef` 和 `thiserror` 派生宏。`pkg/util/tiflashcompute/Cargo.toml` 将 `vardef` 映射到本地包 `astersql-sessionctx-vardef`，并声明 `thiserror = "2"`；本文件不直接使用该 crate 的网络、日志、序列化或配置依赖。

RustCodeGraph 的文件节点显示本文件被 `pkg/session/runtime/control.rs`、`pkg/sessionctx/variable/sysvar.rs`、`pkg/sessionctx/variable/sysvar_builtins.rs` 三个文件使用，但函数级 callers/callees 查询没有建模出边。源码引用补充确认：

- `sysvar.rs::setTiFlashComputeDispatchPolicy` 解析字符串，成功后原子式地替换会话字段，失败则包装为 `SysVarError`。
- `sysvar_builtins.rs` 的系统变量注册回调解析并反向映射，以返回规范字符串。
- `control.rs` 在会话 SET 路径调用解析函数，在 GET 路径调用反向转换函数。
- `pkg/sessionctx/variable/lib.rs` 通过 `pub use tiflashcompute` 把本 crate 暴露给上述会话层调用者。

`GetValidDispatchPolicy` 在生产 Rust 代码中只由本文件的错误分支直接调用；它还被独立测试直接验证。Go 侧则在 TiFlash 集成测试中遍历该列表，驱动两种策略的系统变量和下游调度路径。

## 错误处理与边界

解析 API 使用 `Result`，唯一错误条件是输入不等于两个规范字符串之一。比较是区分大小写且不修剪空白的；是否预处理引号或空白由上游负责，例如 `pkg/session/runtime/control.rs` 在调用前去掉外围引号。错误保留原始传入文本并列出合法集合，便于定位配置错误。

反向映射是总函数，不返回错误：哨兵值 `2`、负数和任意其他未知 `isize` 均返回 `invalid`。这适合展示和诊断，但调用方不能把返回 `invalid` 当作策略可执行的证明。

与 Go 有一个类型层面的差异：Go 解析失败返回 `(DispatchPolicyInvalid, error)`，Rust 的 `Result` 在 `Err` 分支不携带策略值。现有上游只在 `Ok` 后更新状态，因此失败不会写入哨兵值。`DispatchPolicy` 是开放的类型别名而非 Rust `enum`，新增数值时必须同步所有匹配点与测试，否则新值会被旧代码显示为 `invalid`。

## 并发与资源生命周期

三个函数都是无副作用的同步纯转换，不使用锁、原子变量、线程、任务、通道、事务、文件句柄或网络连接。所有返回的字符串借用编译期静态常量，可跨线程安全读取；返回的 `Vec` 和错误 `String` 由调用方独占并按普通 Rust 所有权规则释放。

因此本文件没有内部竞态或清理顺序。并发会话之间的策略隔离由持有 `DispatchPolicy` 的上游会话/全局状态实现，不由这里保证；全局值更新与任务调度的同步语义也不在本文件范围内。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/tiflashcompute/dispatch_policy.go`。Rust 保留了 Go 的公开符号名、常量数值、合法字符串顺序、精确匹配规则及未知数值映射为 `invalid` 的行为。`pkg/sessionctx/vardef/tidb_vars.rs` 与对应 Go 常量文件也使用相同的三个字符串，默认值同为 `consistent_hash`。

主要语言差异如下：

- Go 的 `DispatchPolicy` 底层为 `int`，Rust 为 `isize`；两者都是平台字宽整数，但都不提供封闭枚举约束。
- Go 的合法列表是 `[]string`，Rust 是拥有容器但借用静态元素的 `Vec<&'static str>`。
- Go 用 `errors.Errorf` 返回错误并同时返回 `DispatchPolicyInvalid`；Rust 用 `thiserror` 类型及 `Result`，错误分支没有策略值。
- 两边错误内容语义一致。Rust 显式用空格连接列表，使显示结果保持 `[consistent_hash round_robin]`。

相关 Go 行为测试位于 `pkg/executor/test/tiflashtest/tiflash_test.go::TestTiFlashComputeDispatchPolicy` 所在测试段；它还验证两种合法策略进入系统变量和调度路径后的表现。Rust 对应覆盖位于 `pkg/util/tiflashcompute/migration_aster_unit_test.rs::dispatch_policy_matches_go_mappings_and_errors`，另有 `pkg/executor/test/tiflashtest/tiflash_test.rs::tiflash_dispatch_policy_round_trips_canonical_names` 覆盖精确错误与 SQL 层往返。

## 扩展指南

若新增策略，最小安全改动面包括：在本文件增加稳定数值常量，并同时更新 `GetValidDispatchPolicy`、`GetDispatchPolicyByStr` 和 `GetDispatchPolicy`；在 `pkg/sessionctx/vardef/tidb_vars.rs` 增加规范字符串；再同步 Go 对照实现及其 `vardef` 常量。数值不能重排，否则已保存或跨层传递的整数语义会变化；合法列表顺序和错误文本也应视为兼容接口。

新增策略名称时，应继续采用精确、唯一的规范字符串，并明确旧版本遇到新数值时降级为 `invalid` 是否可接受。真正的调度算法必须接入消费策略的 store/coprocessor 或相应执行模块，不能只增加这里的转换常量就宣称策略已实现。

测试应保持在独立测试文件中，不要嵌入 `dispatch_policy.rs`。至少同步扩展：

- `pkg/util/tiflashcompute/migration_aster_unit_test.rs`：合法列表、双向映射、非法输入和未知整数；
- `pkg/executor/test/tiflashtest/tiflash_test.rs`：系统变量默认值、SET/GET 往返及精确错误；
- Go 对照测试 `pkg/executor/test/tiflashtest/tiflash_test.go`，用于确认跨语言行为没有漂移。

主要风险是数值 ABI/持久语义不兼容、合法列表或错误文案变化、系统变量接受了策略但下游调度未实现，以及每次构造列表/错误产生的少量分配。若该路径将来进入高频热循环，可考虑静态切片，但需要保持公开返回类型与调用兼容性。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/util/tiflashcompute` 确认目标与对照文件被索引；目标文件节点确认 76 行源码、5 个顶层符号及三个文件级使用者。
- RustCodeGraph 精确查询：`GetValidDispatchPolicy`、`GetDispatchPolicyByStr`、`GetDispatchPolicy` 和 `DispatchPolicyError` 均定位到本文件；对三个 Rust 函数执行 callers/callees 查询均未返回函数级边，因此上游调用关系又以源码引用补证，没有把缺失图边误写为“没有调用者”。
- 源与 crate 边界：`pkg/util/tiflashcompute/dispatch_policy.rs`、`pkg/util/tiflashcompute/lib.rs`、`pkg/util/tiflashcompute/Cargo.toml`。
- 字符串和默认值：`pkg/sessionctx/vardef/tidb_vars.rs`。
- Rust 上游：`pkg/sessionctx/variable/lib.rs`、`pkg/sessionctx/variable/sysvar.rs`、`pkg/sessionctx/variable/sysvar_builtins.rs`、`pkg/session/runtime/control.rs`。
- Go 对照：`pkg/util/tiflashcompute/dispatch_policy.go`。
- 独立测试：`pkg/util/tiflashcompute/migration_aster_unit_test.rs::dispatch_policy_matches_go_mappings_and_errors`、`pkg/executor/test/tiflashtest/tiflash_test.rs::tiflash_dispatch_policy_round_trips_canonical_names`，以及 Go 的 `pkg/executor/test/tiflashtest/tiflash_test.go` 对应测试段。
- 本任务仅新增说明文档，按计划不运行 Cargo。交付前以任务指定命令检查文件存在且固定二级标题恰好为 11 个，并人工复核所有职责、边界、调用关系和 Go 差异均可回溯到上述路径。
