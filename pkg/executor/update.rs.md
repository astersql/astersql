# `pkg/executor/update.rs`

## 文件定位

`update.rs` 位于 `astersql-executor` crate，crate 根由 [`pkg/executor/Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 指定；[`pkg/executor/lib.rs`](lib.rs) 通过 `pub mod update` 将它作为公开模块编入，并在 `cfg(test)` 下把独立的 [`pkg/executor/update_test.rs`](update_test.rs) 挂为测试模块。该文件不拥有具体的 session、chunk、table 或 TiKV 类型，而是用 `UpdateRuntime` 关联类型和方法描述生产边界，再由 `UpdateExec<R>` 编排 UPDATE 的 Open/Next/Close 及逐行辅助步骤。

它对应 Go 的 [`pkg/executor/update.go`](update.go)，但当前不是 Go 实现的等量移植：Go `UpdateExec` 直接持有表达式、表、句柄缓存、事务、内存追踪器及外键执行器，Rust 文件只实现运行时无关的控制流和少量纯函数/统计结构。RustCodeGraph 对文件给出的直接文件级使用者是 `pkg/executor/select.rs` 与 `pkg/executor/update_test.rs`，而精确符号检索没有找到生产代码对 `UpdateExec` 的实例化；因此当前可验证定位是“公开的 UPDATE 执行协议与可测试骨架”，不能据此声称它已接入完整 SQL 执行主链。

## 核心职责

- `UpdateRuntime` 定义行预处理、普通列/生成列合成、存储写回、子执行器生命周期、统计、内存和外键钩子，使算法层不绑定具体存储类型。
- `UpdateExec<R>` 将生命周期固定为：`Open` 重置统计和 drained 状态、按赋值性质初始化求值缓冲并打开子计划；第一次 `Next` 批量更新且记账；后续 `Next` 空操作；`Close` 生成客户端消息、注册统计、关闭子计划并清零内存。
- `UpdateDupKeyCheckMode` 与 `optimizeDupKeyCheckForUpdate` 表达流水线、悲观事务及 `IGNORE` 对重复键检查时机的优先级。
- `unmatchedOuterRow` 和 `handleUpdateError` 提供最小的边界适配点；前者在 Rust 中只接收已计算的 handle-null 布尔值，后者只把错误交给运行时包装。
- `UpdateRuntimeStats` 保存三段 `Duration`，支持格式化、克隆和逐字段累加。

## 主要符号

- `UpdateDupKeyCheckMode::{Lazy, InPlace}`：分别表示延迟至提交/加锁阶段检查与写行时立即检查。枚举是 `Copy + Eq`，可安全作为每行写回策略值传递。
- `UpdateRuntime`：核心 trait。`Context`、`Request`、`Row`、`Schema`、`ForeignKeyCheck`、`ForeignKeyCascade`、`Error` 七个关联类型隔离具体实现；35 个方法（源码第 44–115 行）覆盖行合成、写回、状态、统计、子执行器和外键接口。trait 没有默认实现，运行时必须显式提供所有行为。
- `UpdateExec<R> { pub runtime: R }`：唯一状态就是注入的运行时。`prepare`、`mergeNonGenerated`、`mergeGenerated`、`exec`、`updateRows`、`fastComposeNewRow`、`composeNewRow`、`setMessage`、`collectRuntimeStatsEnabled`、`GetFKChecks`、`GetFKCascades` 都是薄委托；`Open`、`Next`、`Close` 和 `HasFKCascades` 含本文件自己的分支/顺序语义。
- `unmatchedOuterRow(handle_is_null)`：原样返回布尔值。Go 版会从 `TblColPosInfo.HandleCols` 找到第一 handle 列并检查 datum 是否 NULL，Rust 调用方必须先完成这一步。
- `handleUpdateError(runtime, row_index, error)`：把行号与错误传给 `UpdateRuntime::handle_update_error`。具体的数据过长、溢出、时间戳转换等映射不在本文件实现。
- `UpdateRuntimeStats { fetch, compose, check_and_update }`：三个阶段的累计耗时；`String` 固定输出 `fetch:{:?}, compose:{:?}, check-and-update:{:?}`，`Clone` 生成快照，`Merge` 逐字段相加，`Tp` 返回静态名称 `UpdateRuntimeStats`。
- `optimizeDupKeyCheckForUpdate(pessimistic, pipelined, ignore)`：纯决策函数，优先级为 pipelined → `Lazy`，否则 ignore → `InPlace`，否则 pessimistic → `Lazy`，最后 optimistic → `InPlace`。

文件没有模块级常量、条件编译项或额外 `impl`；`#![allow(non_snake_case)]` 是为了保留与 Go 方法名相近的公开接口。

## 执行流程

1. 构造阶段由外部提供完整的 `R: UpdateRuntime`，装入 `UpdateExec.runtime`；本文件没有构造器，也不负责把物理计划转换为该执行器。
2. `Open(context)` 首先调用 `reset_write_runtime_stats`，保证重复打开不会沿用上次写统计；然后 `set_drained(false)`。仅当 `all_assignments_are_constant()` 为假时调用 `initialize_evaluation_buffer()`，最后 `open_child(context)`。缓冲初始化发生在打开子计划之前，错误只可能由 `open_child` 返回。
3. `Next(context, request)` 总是先 `reset_request(request)`。若 runtime 已 drained，立即成功返回；否则调用一次 `update_rows(context)`，成功后以返回的首次匹配行数调用 `record_write_cpu_work`，再设置 drained。若 `update_rows` 出错，后两步不会执行，drained 保持 false，调用方可观察到错误而不是虚假完成。
4. 具体批处理应由 runtime 的 `update_rows` 实现。Go 对照的真实流程是：创建子 chunk、选择常量或通用 compose 路径、取得事务和重复键模式、循环拉取 child chunk、逐行 `prepare`、合成新行、合并非生成列、处理生成列并 `updateRecord`、累计总行数；Rust trait 为这些步骤预留了同名委托，却没有在 `UpdateExec::updateRows` 内重现该循环。
5. 单行辅助入口依次可调用 `prepare`、`fastComposeNewRow`/`composeNewRow`、`mergeNonGenerated`、两阶段 `mergeGenerated` 与 `exec`。这些方法不自行排序，正确顺序由 runtime 的批处理实现负责。
6. `Close()` 先执行 `set_message`、再 `register_runtime_stats`，然后保存 `close_child()` 的结果，无论关闭成功与否都执行 `reset_memory_usage()`，最终原样返回子执行器关闭结果。

## 数据与状态

`UpdateExec` 本身不复制状态，只拥有可变的 `runtime`。drained、赋值是否全为常量、求值缓冲、内存计数、首次匹配行数、外键集合等全部由 runtime 持有并通过 trait 方法访问。这一设计便于测试控制顺序，但也意味着本文件不能独立保证 Go 版的以下状态不变量：同一 `(table alias, handle)` 只更新一次、多表更新的 merged row 缓存、分区表 handle 去重、matched/changed/matches 数组一致性以及内存增减配平。

可由本文件直接保证的不变量是：一次成功 `Open` 后 drained 被清零；一次成功的首个 `Next` 后 drained 为 true；同一轮后续 `Next` 不再次更新或重复记账；`record_write_cpu_work` 只接收成功 `update_rows` 返回的首次匹配行数；`Close` 即使遇到 child close 错误也会请求 runtime 清零内存。

`UpdateRuntimeStats` 使用 `std::time::Duration`，`Merge` 是简单加法，没有锁和饱和策略。三个字段均公开，创建者可以单独构造/更新。重复键模式也是无内部状态的值对象。

## 依赖与调用关系

文件的唯一直接标准库依赖是 `std::time::Duration`。它没有直接引用 `Cargo.toml` 中大量 executor 依赖；session、expression、kv、table、chunk、memory、foreign-key 等能力均由 `UpdateRuntime` 的关联类型和方法间接提供。crate feature `nextgen` 只转发到 `astersql-dxf-importinto/nextgen`，本文件没有 `cfg(feature = ...)` 分支。

上游方面，`lib.rs` 公开 `update` 模块，`update_test.rs` 直接导入并实例化 `UpdateExec<WriteRuntime>`。RustCodeGraph 的 `query` 能分别定位 Rust/Go 的 `UpdateExec`、`UpdateRuntime` 和 `optimizeDupKeyCheckForUpdate`，但 `callers` 对 Rust `UpdateExec`、`unmatchedOuterRow`、策略函数与统计类型未返回生产调用边；因此生产接线为“未验证”。文件级图把 `select.rs` 标为使用者，但在针对符号的仓库搜索中未发现这些 API 的直接引用，不把这条粗粒度关系解释为运行时调用。

下游调用都表现为 trait dispatch：`Open` → 统计复位/状态/缓冲/child open；`Next` → request reset/状态查询/批量更新/CPU 记账；`Close` → 消息/统计/child close/内存复位；行级委托 → prepare/merge/compose/write/error hooks；外键查询 → runtime 返回的引用向量。Go 真实下游则包括 `exec.Next`、事务 snapshot/options、`updateRecord`、表写入、外键检查/级联、statement context 和 memory tracker，这些只能作为迁移对照，不能当成 Rust 已接线证据。

## 错误处理与边界

所有可能失败的运行时操作统一返回 `R::Error`，本文件不增加错误枚举、不转换错误链。`prepare`、合成、合并、`exec`、`updateRows`、`open_child`、`close_child` 的错误均原样传播。`handleUpdateError` 是唯一显式包装入口，但包装规则完全由 runtime 决定。

`Next` 的关键边界是“先成功更新，后记账并 drained”：失败不会记录 CPU work，也不会把执行器错误标成已耗尽。`Close` 则刻意先保存 child close 结果、再清内存，保证清理不依赖关闭成功；相反，`set_message` 或 `register_runtime_stats` 的 trait 签名不返回错误，runtime 只能在其内部处理失败。`Open` 若 `open_child` 失败，统计和 drained 已被复位，且可能已初始化缓冲，本文件没有回滚钩子。

重复键决策的边界顺序必须保持：流水线 bulk 即使同时 `IGNORE` 也选择 `Lazy`；非流水线的 `IGNORE` 必须 `InPlace` 以便立即拿到并忽略重复键错误；普通悲观事务 `Lazy`；乐观事务始终 `InPlace`。`unmatchedOuterRow` 不校验列位置，只信任调用方传入的 handle-null 结果。`UpdateRuntimeStats::Merge` 不处理不同类型或缺失子统计，和 Go 的嵌套 snapshot/allocator 统计语义不同。

## 并发与资源生命周期

`UpdateExec` 的方法都通过 `&mut self` 串行修改 runtime，文件没有 `Send`/`Sync` 约束、线程、异步任务、锁或通道；同一个实例不能在安全 Rust 中被两个调用者同时可变执行。若 runtime 内部共享状态或并行写行，其同步责任不属于本文件。

生命周期以 `Open → 一个有效 Next → 若干空 Next → Close` 为主。`Open` 每轮重置写统计和 drained；非常量赋值才分配/初始化求值缓冲。`Close` 负责消息与统计收尾、关闭 child，并无条件请求清零内存。外键检查/级联以借用引用组成的临时 `Vec` 返回，引用生命周期绑定到 `&self`；本文件不拥有或启动外键任务。统计合并适合由外部在 worker 完成后串行归并，但 `UpdateRuntimeStats` 自身不提供并发同步。

Go 版额外管理 chunk 内存计费、handle/merged-row 缓存、事务 snapshot 统计和 `MayFlush`；Rust 文件没有对应资源对象。新增并行 worker 或流水线 runtime 时，需要在 runtime 中证明统计合并、写顺序、重复键检查和 close 清理在并发下仍成立。

## 与 Go 版本的对应关系

Rust 名称刻意对齐 Go `UpdateExec` 的 `prepare`、`mergeNonGenerated`、`mergeGenerated`、`exec`、`Next`、`updateRows`、`fastComposeNewRow`、`composeNewRow`、`Open`、`Close`、外键访问器和重复键优化函数。策略函数的四条结果与 Go 第 706–741 行一致，Rust 把 `txn.IsPipelined()`/`txn.IsPessimistic()` 预先折叠为两个布尔参数。

重要差异如下：

- Go `UpdateExec` 直接保存 assignment、表、handle map、merged row、matched 计数、内存 tracker、外键 map 等；Rust 只保存 runtime。
- Go `updateRows` 实现完整 chunk 拉取、表达式求值、事务 option、逐行写回与 `MayFlush`；Rust `updateRows` 只是一次 trait 委托。
- Go `unmatchedOuterRow` 自行定位 handle datum；Rust 只判断传入布尔值。
- Go `handleUpdateError` 明确重写 data-too-long、overflow 和 timestamp truncated 错误并补一基行号；Rust 仅委托，是否对齐取决于 runtime。
- Go `updateRuntimeStats` 合并 snapshot 与 allocator 统计并返回整数类型标识；Rust 统计的是 fetch/compose/check-and-update 三段耗时，`Tp` 返回字符串。两者结构和语义并不等价。
- Go `Open` 创建并挂接 memory tracker，按需创建 write stats；当前 Rust `Open` 还负责 drained 与求值缓冲复位，但具体分配和挂接都由 runtime 完成。Go `Next` 把总记录数写入 statement context，Rust 只用 matched rows 做 write CPU 记账。

Go 独立测试 [`pkg/executor/update_test.go`](update_test.go) 验证悲观事务主键延迟重复键、未变化键锁、UPDATE IGNORE 重试后重复键、ON UPDATE 与生成列索引一致性；这些是完整 Go 执行器的行为证据，不等同于 Rust 单测覆盖。Rust 独立测试只覆盖纯策略/统计和 Open/Next 的一次记账语义。

## 扩展指南

若要接入真实 Rust SQL UPDATE，优先实现一个生产 `UpdateRuntime`，而不是在 `UpdateExec` 中硬编码具体 session/table 类型。实现必须补齐：child chunk 循环、全局行号、常量/通用表达式求值、生成列前后两阶段合并、同一 handle 去重、多表/分区更新、事务选项与 flush、外键检查/级联、statement message、内存和 runtime stats。随后在 executor builder 或 adapter 的真实构造路径实例化该 runtime；当前文件没有这一入口。

修改生命周期顺序时重点关注 `Open` 失败后的半初始化状态、`Next` 错误时不得 drained/记账、`Close` 错误时仍清理内存。修改重复键策略时要保持 pipelined 高于 IGNORE 的优先级，并同步 Go 行为与 issue 注释所描述的 optimistic/pessimistic 限制。扩展统计时需决定是继续阶段耗时模型还是对齐 Go snapshot/allocator 模型，不能只改 `Tp` 字符串宣称兼容。

测试必须继续放在独立文件 [`pkg/executor/update_test.rs`](update_test.rs)，不要内嵌进生产源文件。新增 runtime 行为应覆盖：非常量赋值触发缓冲初始化、`update_rows` 失败不记账不 drained、child open/close 失败的状态与清理、外键列表/级联判断、所有重复键组合、错误行号映射、生成列双阶段顺序及多表同句柄场景。若完成生产接线，还应补与 Go `update_test.go` 和 [`pkg/executor/test/oomtest/oom_test.rs`](test/oomtest/oom_test.rs) 等价的 SQL/内存回归验证。兼容风险集中在 MySQL affected/matched/warning 消息、UPDATE IGNORE 错误降级、生成列索引一致性；性能风险集中在重复键检查时机、row clone、缓存内存和统计热路径。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；据此读取 `pkg/executor/update.rs` 全 323 行，并确认文件级使用者为 `pkg/executor/select.rs` 与 `pkg/executor/update_test.rs`。
- RustCodeGraph `query UpdateExec --kind struct`、`query UpdateRuntime --kind trait`、`query optimizeDupKeyCheckForUpdate --kind function`：定位 Rust/Go 同名符号；对关键 Rust 符号执行 `callers` 未得到生产级调用边，因此本文将真实生产接线标记为未验证。
- 已读生产与装配文件：[`pkg/executor/update.rs`](update.rs)、[`pkg/executor/update.go`](update.go)、[`pkg/executor/Cargo.toml`](Cargo.toml)、[`pkg/executor/lib.rs`](lib.rs)。Cargo 元数据确认 crate 名为 `astersql-executor`、Go 包映射为 `pkg/executor`，且本文件无专属 feature。
- 已读独立测试：[`pkg/executor/update_test.rs`](update_test.rs) 覆盖五组重复键决策、outer-row 布尔边界、统计 clone/merge/string，以及 Open 后首次 Next 只记账一次；[`pkg/executor/update_test.go`](update_test.go) 提供完整 Go 事务/锁/生成列语义；`pkg/executor/test/oomtest/oom_test.rs` 与 `.go` 是内存行为的相关回归入口。
- 人工复核结论：本文区分了 trait 承诺与已接线实现，未把 Go 行为或粗粒度文件关系写成 Rust 已支持事实；所有扩展建议均指向独立测试文件。
