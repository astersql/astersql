# `pkg/executor/join/joinversion/join_version.rs`

## 文件定位

该文件是 `astersql-executor-join-joinversion` crate 的核心实现，负责表达 Hash Join v1/v2 的版本名、非 GA join 的 v2 总开关，以及目标平台是否能安全采用 v2 指针表示的能力判断。crate 根 `pkg/executor/join/joinversion/lib.rs` 通过 `pub mod join_version` 声明模块，并用 `pub use join_version::*` 对外重导出本文件的公开项；`pkg/executor/join/joinversion/Cargo.toml` 指定 `lib.rs` 为库入口，且不声明第三方依赖。

根 `Cargo.toml` 将该 crate 纳入 workspace，并以 `facade_executor_join_joinversion` 登记；`pkg/executor/Cargo.toml`、`pkg/executor/join/Cargo.toml` 和 `pkg/distsql/Cargo.toml` 都声明了路径依赖。不过，RustCodeGraph 与全仓 Rust 符号检索显示，目前本文件函数的实际 Rust 调用者仅在同 crate 的独立测试中，尚未发现 executor、planner 或 distsql 的 Rust 生产调用点。因而它是已实现、已测试、已进入依赖图的迁移模块，但不能据此声称 Rust SQL 执行主链已经使用它。

## 核心职责

1. 用 `HASH_JOIN_VERSION_LEGACY`、`HASH_JOIN_VERSION_OPTIMIZED` 和 `TIFLASH_HASH_JOIN_VERSION_DEFAULT` 统一版本字符串及 TiFlash 默认值。
2. 用 `USE_HASH_JOIN_V2_FOR_NON_GA_JOIN` 保存进程级开关，并通过 `use_hash_join_v2_for_non_ga_join` / `set_use_hash_join_v2_for_non_ga_join` 提供顺序一致的并发读写。
3. 用 `is_optimized_version` 按 Go `strings.ToLower` 的相关语义判断配置是否精确选择 `optimized`，拒绝空串、空白、别名或字符数不同的输入。
4. 用 `is_hash_join_v2_supported` 验证 v2 行表指针编码所需的两个平台前提：堆对象地址不因运行时移动，以及 `usize` 至少能容纳原始指针。
5. 保留 `HashJoinVersionLegacy`、`IsOptimizedVersion` 等 Go 风格兼容导出，供机械迁移的下游逐步过渡；新 Rust 代码应优先使用 snake_case/全大写名称。

本文件只负责“版本选择原语和平台能力”，不负责判断某种 join 类型、NULL-aware 条件、cross join 或 spill 是否受 v2 支持；这些业务条件在 Go 对照实现中由 planner/physical join 层继续组合判断。

## 主要符号

- `HASH_JOIN_VERSION_LEGACY: &str = "legacy"`：Hash Join v1 的规范值。
- `HASH_JOIN_VERSION_OPTIMIZED: &str = "optimized"`：Hash Join v2 的规范值，也是 `is_optimized_version` 唯一接受的目标值。
- `TIFLASH_HASH_JOIN_VERSION_DEFAULT`：直接别名到 legacy，保持 TiFlash 默认仍选择 v1。
- `USE_HASH_JOIN_V2_FOR_NON_GA_JOIN: AtomicBool`：初值为 `true` 的全局原子开关，对应 Go 包 `init` 执行后的值。兼容名 `UseHashJoinV2ForNonGAJoin` 重导出同一个原子对象，并非副本。
- `use_hash_join_v2_for_non_ga_join() -> bool` 与 `set_use_hash_join_v2_for_non_ga_join(bool)`：分别以 `Ordering::SeqCst` load/store 全局开关。
- `is_optimized_version(&str) -> bool`：逐 Unicode scalar 与 ASCII 目标 `optimized` 比较，随后再次核对字符数，避免 `zip` 只比较公共前缀。
- `go_simple_lowercase_matches(char, char) -> bool`：私有辅助函数。普通字符使用 `to_ascii_lowercase`；另显式接受 `U+0130 LATIN CAPITAL LETTER I WITH DOT ABOVE` 对目标 `i`，复现本场景下 Go 单 rune `unicode.ToLower` 的特殊映射。
- `SIZE_OF_UINTPTR` / `SIZE_OF_POINTER`：编译期取得 `usize` 与 `*const ()` 的字节数。
- `heap_objects_can_move() -> bool`：私有 `const fn`，固定返回 `false`，表达 Rust 无移动式 GC、移动拥有型指针不会搬迁堆分配的运行时事实。
- `is_hash_join_v2_supported() -> bool`：组合 `!heap_objects_can_move()` 与 `SIZE_OF_UINTPTR >= SIZE_OF_POINTER`。
- `HashJoinVersionLegacy`、`HashJoinVersionOptimized`、`TiFlashHashJoinVersionDefVal`、`UseHashJoinV2ForNonGAJoin`、`IsOptimizedVersion`、`IsHashJoinV2Supported`：仅用于 Go 风格兼容的常量、重导出或薄包装。

## 执行流程

版本字符串判断从 `is_optimized_version` 进入：它将输入字符与 `"optimized"` 的字符逐个 `zip`；每对字符交给 `go_simple_lowercase_matches`，要求 ASCII 小写后相等，或仅在实际字符为 `U+0130`、期望字符为 `i` 时相等；所有配对成功后，再比较双方 `chars().count()`。最后一步保证 `"optimized-suffix"` 不会因前缀匹配而误判，逐字符比较也保证前后空格或 `v2` 别名不会命中。

非 GA 开关流程是直接的进程级状态访问：读取函数执行 `AtomicBool::load(SeqCst)`；设置函数执行 `AtomicBool::store(SeqCst)`。调用者若临时修改，必须自行恢复原值；现有 `non_ga_join_switch_is_initialized_and_mutable` 测试遵循“设为 false、断言、恢复 true”的顺序。

平台能力判断从 `is_hash_join_v2_supported` 进入：先确认 `heap_objects_can_move()` 为 false，再确认 `usize` 宽度不小于裸指针。当前实现的第一个条件是编译期恒真，第二个条件由目标 ABI 的 `size_of` 决定；函数不探测 join 类型或查询配置。Go 生产主链会将这一结果与会话变量 `UseHashJoinV2` 以及 `CanUseHashJoinV2()` 的业务能力同时相与，见 `pkg/executor/builder.go` 和 `pkg/planner/core/exhaust_physical_plans.go`。

## 数据与状态

三个版本值都是静态字符串常量，无分配、无所有权转移；TiFlash 默认值与 legacy 共享同一常量语义。两个尺寸值均在编译期由当前 target ABI 决定。`heap_objects_can_move` 没有运行时状态，其固定返回值依赖 Rust 当前的非移动堆分配模型。

唯一可变状态是 `USE_HASH_JOIN_V2_FOR_NON_GA_JOIN`。它属于进程全局而非会话、查询或 executor 实例，默认值 `true` 与 Go 源文件中先初始化为 false、再由包 `init()` 设置为 true 的初始化后结果一致。该状态没有 RAII guard，也不会自动按测试或查询恢复；并行修改它的测试可能相互影响，即使原子操作本身不存在数据竞争。

`SeqCst` 为所有该原子上的读写提供单一全序，是强于这里只保存布尔值通常所需的内存序，但让跨线程观察最直观。兼容重导出 `UseHashJoinV2ForNonGAJoin` 指向同一 `AtomicBool`，不会产生两个可能分叉的开关。

## 依赖与调用关系

下游依赖仅来自 Rust 标准库：`std::sync::atomic::{AtomicBool, Ordering}`、`std::mem::size_of`、字符串 `chars`/`zip` 和字符 ASCII 小写转换。RustCodeGraph 给出的内部调用边是 `is_optimized_version -> go_simple_lowercase_matches`；`IsOptimizedVersion -> is_optimized_version` 与 `IsHashJoinV2Supported -> is_hash_join_v2_supported` 是兼容包装；平台判断还直接读取两个尺寸常量并调用 `heap_objects_can_move`。

已验证的 Rust 上游调用者为：`migration_aster_unit_test.rs` 调用开关 getter/setter、版本判断和平台判断，`join_version_test.rs` 调用版本判断。RustCodeGraph 的 blast radius 分别报告这些测试调用者，未报告生产调用者；全仓 `*.rs` 检索结果一致。Cargo 依赖关系仅证明 crate 可被 executor/join/distsql 引用，不能替代实际符号调用证据。

Go 侧应用链可作为移植语义的直接对照：`pkg/sessionctx/variable/session.go` 和 `sysvar.go` 用 `IsOptimizedVersion` 把系统变量转换为会话态；`pkg/planner/core/exhaust_physical_plans.go`、`pkg/executor/builder.go` 将会话态、`IsHashJoinV2Supported` 和具体 join 能力联合决定 v1/v2；`pkg/planner/core/operator/physicalop/physical_hash_join.go` 使用非 GA 开关并判断 TiFlash v2；`pkg/distsql/distsql.go` 将 TiFlash 版本判断写入出站 metadata。它们是 Go 调用点，并非已经存在的 Rust 调用边。

## 错误处理与边界

所有公开函数均为无失败返回的纯判断或原子访问，不产生 `Result`、异常、日志或 I/O。无效版本值统一返回 false：包括空串、`legacy`、前后空格、`v2`、多余后缀以及字符数不同的 Unicode 组合。

大小写语义并不是通用 Unicode case folding。实现只需把输入与纯 ASCII 的 `optimized` 比较：ASCII 大小写可匹配，并特判 Go 会把 `U+0130` 单 rune 映射为 `i` 的情况；由 `i` 加组合点构成的两个 scalar 会因字符数和位置不同而拒绝。若未来目标版本名含非 ASCII 字符，当前辅助函数不能被直接当作完整的 Go `strings.ToLower` 兼容实现。

平台能力函数只验证指针表示前提，不代表所有查询都能使用 v2。调用层仍必须检查会话选择、join 类型、join key、NullEQ、null-aware key、cross join、TiFlash spill 等限制。尺寸比较使用 `>=` 与 Go 实现一致，虽然常见目标上 `usize` 与指针等宽。全局开关 setter 不校验调用来源，也不提供作用域恢复机制。

## 并发与资源生命周期

文件不创建线程、任务、锁、通道、事务、文件句柄或堆资源；版本判断和平台判断都只使用栈上迭代器与编译期常量。`str::chars()` 不分配新字符串，避免了为了大小写转换构造完整临时值。

并发关注点集中于全局 `AtomicBool`：`SeqCst` 使单次访问线程安全且全序可见，但复合的“读取旧值—临时修改—恢复”不是原子事务。现有测试恢复默认值能降低串行测试污染，却无法隔离与其他同时修改该开关的测试；新增并行测试应避免共享修改，或在测试基础设施中串行化并用作用域 guard 确保 panic 时也恢复。生产侧若接入 setter，也应明确其进程级生命周期，不能误当作 session 变量。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/join/joinversion/join_version.go`。三项常量值完全对应；Rust 原子变量的初值 `true` 对应 Go `UseHashJoinV2ForNonGAJoin` 经过 `init()` 后的最终值，而不是 Go 声明处短暂的 false。Rust getter/setter 是为安全可变性新增的惯用接口，Go 则直接读写包变量。

Go `IsOptimizedVersion` 使用 `strings.ToLower(hashJoinVersion) == "optimized"`。Rust 没有构造完整 lowercase 字符串，而是逐 scalar 比较，并为 `U+0130` 补足 Go 的单 rune lower 映射；`join_version_test.rs` 专门验证 `"optİmized"` 为真、`"optI\u{0307}mized"` 为假。该实现精确服务当前 ASCII 目标词，不应外推为任意 Unicode 字符串的等价转换。

Go 通过 `//go:linkname` 调用运行时 `heapObjectsCanMove`，Rust 则以语言/运行时模型为依据固定返回 false；双方都比较整数指针容器与裸指针宽度。Rust 另保留 Go 风格导出供迁移代码兼容，但当前生产 Rust 调用尚未在索引或文本检索中发现。Go 的实际主链调用仍是理解预期集成位置的权威证据。

## 扩展指南

- 新增或重命名版本值时，应同时修改规范常量、`is_optimized_version` 的目标语义、Go 风格兼容导出，并同步 `migration_aster_unit_test.rs` 的常量和有效/无效输入用例；还需评估 Go 系统变量校验与 metadata 协议的兼容性。
- 扩展 Unicode 匹配前，先以 Go `strings.ToLower` 的逐 rune 行为建立独立测试。不要简单换成 Rust 完整 Unicode lowercase 后假定等价，因为单字符展开会改变 scalar 数量。
- 修改平台能力前，必须保留“堆地址稳定”和“整数足以容纳指针”两个不变量，并核查 hash row table 的实际指针编码消费者；不要把具体 join 功能矩阵塞入本文件。
- 若把模块接入 Rust 生产主链，优先调用 snake_case API，并分别在会话变量转换、planner 候选生成、executor 构建与 TiFlash metadata 边界增加独立测试。Cargo 清单已有依赖不等于已接线，新增调用后应重新查询 callers/callees。
- 若生产代码需要修改非 GA 开关，应考虑把裸 setter 提升为明确的配置初始化或带恢复 guard 的接口，避免跨查询、跨测试污染；对应测试仍应放在独立的 `migration_aster_unit_test.rs` 或 `join_version_test.rs`，不要内嵌进生产源文件。
- 性能风险主要在高频版本判断和原子读：当前字符串比较无分配，但执行两次 `chars()` 遍历；优化时必须保留前缀拒绝和 Go Unicode 特例。兼容风险主要是默认值、接受输入集合和 Go 风格导出变化。

## 验证依据

- 源码与模块边界：`pkg/executor/join/joinversion/join_version.rs`、`lib.rs`、`Cargo.toml`；workspace/消费者声明见根 `Cargo.toml`、`pkg/executor/Cargo.toml`、`pkg/executor/join/Cargo.toml`、`pkg/distsql/Cargo.toml`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/executor/join/joinversion` 覆盖本 crate 的 Go/Rust 源和测试；`explore` 核对本文件 8 个索引符号、内部调用和测试 blast radius；`query` 唯一定位 `is_optimized_version` 与 `is_hash_join_v2_supported`；`callees is_optimized_version` 确认其调用私有匹配函数，平台判断无图中 callee。若干独立 `callers` 命令在本地查询时超时，调用者结论由同次 `explore` 的 blast radius 与全仓精确符号检索交叉确认。
- Rust 独立测试：`pkg/executor/join/joinversion/migration_aster_unit_test.rs` 覆盖常量、ASCII 大小写、无效输入、平台支持和全局开关可变性；`pkg/executor/join/joinversion/join_version_test.rs` 覆盖 Go/Rust Unicode 小写差异。
- Go 对照与主链：`pkg/executor/join/joinversion/join_version.go`、`pkg/sessionctx/variable/session.go`、`pkg/sessionctx/variable/sysvar.go`、`pkg/planner/core/exhaust_physical_plans.go`、`pkg/planner/core/operator/physicalop/physical_hash_join.go`、`pkg/executor/builder.go`、`pkg/distsql/distsql.go`。
- 人工复核结论：文件存在是为了集中版本常量、非 GA 灰度开关与 v2 平台前提；其运行路径由字符串判断、原子状态访问和 ABI/运行时能力判断组成；安全扩展必须同步独立测试、维持 Go 语义，并明确 Rust 生产接线目前尚未得到调用证据。
