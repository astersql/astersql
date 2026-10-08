# `pkg/planner/util/fixcontrol/set.rs`

## 文件定位

`set.rs` 是 `astersql-planner-util-fixcontrol` crate 中的 fix-control 文本解析器。crate 根在 [`lib.rs`](./lib.rs) 中以 `pub mod set` 声明本模块，并用 `pub use set::*` 将唯一公开函数 `ParseToMap` 重导出到 crate 根。[`Cargo.toml`](./Cargo.toml) 将该 crate 定义为不发布的 workspace 库，Go 包对应路径是 `pkg/planner/util/fixcontrol`，本文件的唯一外部依赖是 `anyhow`。

在完整应用中，它把 `tidb_opt_fix_control` 风格的会话变量文本转成优化器可查询的 `HashMap<u64, String>`。直接生产使用证据包括 [`pkg/sessionctx/variable/sysvar_builtins.rs`](../../../sessionctx/variable/sysvar_builtins.rs) 中的系统变量校验回调，以及 [`pkg/planner/core/operator/physicalop/index_join_probe.rs`](../../core/operator/physicalop/index_join_probe.rs) 中的 `fix_map`。它只负责解析和生成重复赋值警告，不决定具体 fix 编号的含义；后者由同 crate 的 [`get.rs`](./get.rs) 常量与类型化读取 API 承担。

## 核心职责

- 将逗号分隔的 `key:value` 序列解析为新建的 `HashMap<u64, String>`；key 必须是十进制 `u64`，value 保留为字符串。依据：`ParseToMap`。
- 支持无引号值、单引号值和双引号值；引号内的逗号属于值，不是条目分隔符。依据：`ParseToMap` 中 `quote` / `relativeEnd` 分支，以及 `migration_aster_unit_test.rs::parse_to_map_matches_go_quotes_duplicates_and_empty_values`。
- 检测同一 key 的异值重复赋值，追加一条警告，同时以新值覆盖旧值；同值重复不告警。依据：`m.get(&key)` 和 `m.insert(key, value)`，以及 `fixcontrol_test.rs::TestFixControl`。
- 在缺冒号、key 不能解析为 `u64`、或引号未闭合时立即失败，不向调用者暴露部分 map 或已累积的警告。依据：`ParseToMap` 的返回类型与三处 `?`/`Err` 早退路径。

## 主要符号

- `pub fn ParseToMap(mut s: &str) -> anyhow::Result<(HashMap<u64, String>, Vec<String>)>`：本文件唯一生产符号，同时也是公开 API。输入是借用字符串切片；成功值为拥有所有权的 map 和按发现顺序排列的警告文本。
- 局部 `m`：当次调用的解析结果；每次调用都重新创建，不共享状态。
- 局部 `warnMsgs`：仅记录“同 key、不同 value”的重复赋值。警告使用 `{:?}` 格式化旧值与新值，因而与 Go `%q` 的现有用例输出一样包含双引号。
- 局部 `s`：指向尚未解析的输入尾部；函数只移动该切片，不改写原字符串。

本文件没有模块级常量、`struct`、`enum`、trait、`impl` 或条件编译项。`#![allow(non_snake_case)]` 只是为了保留与 Go 同名的 `ParseToMap`。

## 执行流程

1. `ParseToMap` 创建空 map 和空警告列表。空输入不进入循环，直接返回两个空容器。
2. 每轮先在剩余文本中寻找第一个 `:`。找不到时返回 `invalid fix control: expected colon not found`。
3. 冒号前文本经 `trim()` 后用 `parse::<u64>()` 转成 key。空、负数、小数或非数字 key 都会把 `ParseIntError` 透传为 `anyhow::Error`。
4. 冒号后先 `trim_start()`。若首字符是单引号或双引号，则查找下一个同类引号，取两者之间的原始文本为 value，并把 `s` 推进到闭合引号之后。未找到闭合引号时立即报错。
5. 在当前 `s` 中寻找第一个逗号，它同时确定当前条目的结束和下一条的起点。如果之前没有得到非空的引号值，就将逗号前文本 `trim()` 后作为 value。因此 `123:` 是合法的空值，末尾逗号后只有空白也不会产生额外条目。
6. 如果 map 已含同 key 且值不同，先写入警告；随后无条件 `insert`，实现“后值覆盖前值”。
7. 将 `s` 移到逗号后并 `trim()`，重复直到剩余文本为空，最后返回 `(m, warnMsgs)`。

## 数据与状态

输出 map 的 key 是 fix-control 编号，value 是尚未做布尔、整数或浮点转换的原始字符串。类型化解释由 `get.rs` 中的 `GetBool*` / `GetInt*` / `GetFloat*` 完成；本函数不校验 value 是否适合某个具体 fix。

所有可变状态都是函数局部状态。`main_test.rs::TestHarnessCasesAreIsolated` 和 `fixcontrol_test.rs::TestFixControl` 通过连续独立解析验证不会泄漏前一次调用的 map。警告也是返回值，不由本文件写入会话；`sysvar_builtins.rs` 的调用者负责把它们追加到 `StmtCtx`。

## 依赖与调用关系

下游依赖很小：标准库 `str` 查找/裁切/去空白、`str::parse::<u64>`、`HashMap`、`String` 和 `format!`，以及 `anyhow::Result` / `anyhow!`。RustCodeGraph 的 `callees` 查询没有返回仓库内函数调用边，与源码只使用库/trait 操作的事实一致。

上游关系分为：

- `lib.rs` 导出 `set` 模块和 `ParseToMap`，使依赖 crate 可以通过 `fixcontrol::ParseToMap` 调用。
- `pkg/sessionctx/variable/sysvar_builtins.rs` 的 `validate_fix_control` 在 session/global `tidb_opt_fix_control` 设置时解析文本：错误阻止设置，重复赋值警告进入 `StmtCtx`。当前 Rust 回调在该处丢弃解析出的 map，不能像 Go `sysvar.go` 的 `SetSession` 那样单凭这条路径证明 `SessionVars.OptimizerFixControl` 已更新。
- `pkg/planner/core/operator/physicalop/index_join_probe.rs::fix_map` 从 `PlanContext` 读取 `tidb_opt_fix_control` 文本，调用本函数得到 map，然后由 `access_rows_floor` 通过 `GetBoolWithDefault` 读取 `Fix44855`。该调用点明确忽略警告，并将解析错误降级为空 map。
- RustCodeGraph `callers` 查询直接识别到 `fixcontrol_test.rs` 的 `TestParseToMapEmptyValue`、`TestFixControl` 和 `TestFixControlErrors`。上述两条生产调用是因图未返回跨 crate 边后，用 `rg` 和源文件阅读补充核对的。

`Cargo.toml` 将 Go 对照包记录为 `pkg/planner/util/fixcontrol`。workspace 根用 `facade_planner_util_fixcontrol` 别名声明本 crate；`pkg/sessionctx/variable/Cargo.toml` 和 `pkg/planner/core/operator/physicalop/Cargo.toml` 则分别以 `fixcontrol` 名称依赖它，与上述调用点相符。

## 错误处理与边界

- 空输入成功返回空 map/空警告；`123:` 成功返回 `{123: ""}`。
- 缺少冒号和未闭合引号使用稳定的自定义消息；非 `u64` key 保留 `ParseIntError` 的错误类别。`fixcontrol_test.rs::TestFixControlErrors` 覆盖小数 key、负数 key、缺冒号和引号未闭合。
- 不支持引号转义：搜索的是开头引号后第一个同字符；文件中没有反斜杠逃逸规则。
- 引号值可包含逗号，但闭合引号之后到下一个逗号的非空白文本没有被单独拒绝；对非空引号值，该尾部不会进入 value。扩展语法或严格化校验时必须考虑 Go 兼容性。
- 空引号值使 `value.is_empty()` 为真，因而会再从闭合引号后到逗号前的文本取值；Go 实现同样使用 `len(value) == 0` 分支。这是现有语义，不应在本文件中单方面“修正”。
- 发生任何语法/key 错误时，`Result` 只返回错误，因而调用者拿不到之前已解析的条目和警告。Go 版本也在这些路径返回 `nil, nil, err`。

## 并发与资源生命周期

`ParseToMap` 无全局、静态或线程局部状态，也不创建锁、任务、通道、事务或 I/O 资源。它在调用期间借用输入 `&str`，成功时返回独立拥有的 `String`/`HashMap`/`Vec`；调用结束后对输入没有悬挂引用。因此多线程可并行调用它，彼此不共享解析状态；上层如何把返回 map 存入会话、如何同步会话状态，不在本文件职责内。

资源成本随输入和输出大小线性增长：每个值会分配 `String`，map 和警告列表也会扩容。实现通过移动 `&str` 切片游标避免为“未处理尾部”反复分配新字符串。

## 与 Go 版本的对应关系

直接对照是 [`set.go`](./set.go) 的 `ParseToMap`。Rust 版保留了其函数名、`u64 -> string` map、警告文本、后值覆盖、同值不告警、两种引号、引号内逗号、空白处理、末尾逗号、空值以及错误时不交付部分结果的语义。`fixcontrol_test.rs` 回放 Go `fix_control_suite` 中的成功与失败样例，`testdata/fix_control_suite_{in,out}.json` 是 SQL 层期望的原始证据。

表达差异是：Go 返回 `(map, warnings, error)`，Rust 将成功数据放入 `anyhow::Result` 的 `Ok((map, warnings))`；Go `strconv.ParseUint` 错误和 Rust `ParseIntError` 的完整文本不同，Rust 测试因而校验错误类别而不是强行复制 Go 字串。Go 用字节下标处理 ASCII 引号/分隔符，Rust 用 `chars().next()` 识别开头引号并用 `len_utf8()` 计算其字节长度；对当前只允许的 ASCII `'`/`"` 而言结果等价。

上层接线尚非完全等价：Go `sysvar.go` 的 session setter 会把 `newMap` 写入 `SessionVars.OptimizerFixControl`，而当前 Rust `sysvar_builtins.rs` 中对应回调只做校验和警告转发。这一差异属于调用者接线，不是 `set.rs::ParseToMap` 本身的解析差异。

## 扩展指南

- 扩展文法、分隔符或引号规则时，修改点集中在 `ParseToMap`。必须先与 `set.go::ParseToMap` 对齐，特别保护引号内逗号、空引号值、闭合引号后文本和末尾逗号等非显然边界。
- 新增 key 范围或 value 的业务校验前，先确认是通用解析器责任还是某个 fix 的类型化读取责任；后者通常应放在 `get.rs` 或具体消费者，避免破坏其他 fix 的自由字符串值。
- 改变错误或警告文本会影响 SQL `SHOW WARNINGS`、Go testdata 和上层错误匹配；应同步更新独立测试 [`fixcontrol_test.rs`](./fixcontrol_test.rs) 和 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，并核对 Go [`fixcontrol_test.go`](./fixcontrol_test.go) 及 `testdata/`。不要把 Rust 测试嵌入 `set.rs`。
- 改变返回类型或重复键策略时，需同时审查 `sysvar_builtins.rs` 对警告/错误的处理和 `index_join_probe.rs::fix_map` 的降级策略。若要完善 session map 接线，应在上层 setter 修复并增加会话级回归，不应让纯解析函数直接持有会话。
- 性能优化时保留单调游标和线性扫描特性；可评估按条目数预留 map/vector 容量，但应以实际输入规模基准为依据，避免为通常很短的会话变量增加复杂度。

## 验证依据

- 目标源码：[`set.rs`](./set.rs) 全文；公开符号只有 `ParseToMap`。
- crate 边界：[`lib.rs`](./lib.rs) 的模块声明/重导出，以及 [`Cargo.toml`](./Cargo.toml) 的 `astersql-planner-util-fixcontrol` 包名、`anyhow` 依赖和 Go 包映射。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/planner/util/fixcontrol` 列出目标包 11 个已索引文件；`query ParseToMap --kind function` 定位 Go/Rust 两个实现；`node pkg/planner/util/fixcontrol/set.rs::ParseToMap` 核对完整函数；`callers` 识别三个 `fixcontrol_test.rs` 测试；`callees` 无仓库内函数边。
- 生产调用补充核对：`rg` 定位并逐段阅读 [`pkg/sessionctx/variable/sysvar_builtins.rs`](../../../sessionctx/variable/sysvar_builtins.rs) 和 [`pkg/planner/core/operator/physicalop/index_join_probe.rs`](../../core/operator/physicalop/index_join_probe.rs)；其 Cargo manifest 均声明了 fixcontrol crate 依赖。
- Go 对照：[`set.go`](./set.go)、[`fixcontrol_test.go`](./fixcontrol_test.go)、`testdata/fix_control_suite_in.json` 和 `testdata/fix_control_suite_out.json`；上层 setter 对照为 `pkg/sessionctx/variable/sysvar.go`。
- Rust 独立测试：[`fixcontrol_test.rs`](./fixcontrol_test.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 和 [`main_test.rs`](./main_test.rs)。测试覆盖成功输入、空值、引号内逗号、重复键覆盖/警告、非法 key、缺冒号、未闭合引号与调用隔离。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证应确认本文件存在且恰好包含任务规定的 11 个二级标题。
