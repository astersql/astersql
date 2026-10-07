# `pkg/executor/aggfuncs/func_group_concat.rs`

## 文件定位

本文件位于 `astersql-executor-aggfuncs` crate，由 `pkg/executor/aggfuncs/lib.rs` 以 `pub mod func_group_concat` 暴露。它定义一个可克隆的 `GroupConcat` 部分状态，用字节序列表示 SQL `GROUP_CONCAT` 的输入、分隔符和结果，并为 DISTINCT 合并及 spill 恢复保留去重键。

当前生产接线比 Go 的完整 `GROUP_CONCAT` 实现窄：`builder.rs::build_group_concat` 会选择普通、DISTINCT、ORDER BY、DISTINCT+ORDER BY 四类 `AggImplementation`，但 `aggfuncs.rs::BuiltAggFunc::spill_function` 仅将 `GroupConcatDistinctOriginal` 和 `GroupConcatDistinctPartial` 绑定到本文件的 `GroupConcat`。因此，本类型当前主要是 DISTINCT partial state 及其 spill/merge 载体，不应视为已经覆盖 Go 文件中的表达式求值、排序、警告生成和全部执行器接口。

`pkg/executor/aggfuncs/Cargo.toml` 声明该目录是独立库 crate，入口为 `lib.rs`。本文件自身只直接使用标准库 `std::collections::HashMap`，没有条件编译项，也没有直接引用 Cargo 中列出的其他 workspace crate；序列化能力由相邻的 `spill_serialize_helper.rs` 通过外部 trait impl 补上。

## 核心职责

- `GroupConcat::new` 固化分隔符、最大结果字节数以及是否启用 DISTINCT，并创建空的 partial state。
- `GroupConcat::update` 忽略 NULL，将每个非 NULL 字节值按输入次序拼接；DISTINCT 模式先按原始字节值去重。
- `GroupConcat::update_keyed` 允许调用者分别提供 collation key 和展示值，使恢复或上游表达式求值能够按比较键去重、按原值输出。
- `GroupConcat::merge` 合并另一个 partial state：DISTINCT 目标逐项重放对侧键值，非 DISTINCT 目标把对侧已拼接缓冲当成一个输入段追加。
- `GroupConcat::result` 区分“从未出现非 NULL 输入”和“出现过非 NULL 空串”；`GroupConcat::truncated` 暴露生命周期内是否发生过长度截断。
- `reset` 清空当前分组的结果与去重集合，但故意保留 `truncated` 哨兵，与 Go 的“一次聚合函数生命周期只报告一次截断”语义对齐。

## 主要符号

`pub struct GroupConcat` 是本文件唯一类型，也是唯一公开 API 的载体：

- `separator: Vec<u8>`：行间分隔符，私有且在构造后不变。
- `maximum_len: usize`：结果最大字节长度；值为 `0` 表示不设上限。
- `pub(crate) value: Vec<u8>`：当前拼接结果。crate 内可见是为了 spill codec 直接读写。
- `pub(crate) distinct: Option<HashMap<Vec<u8>, Vec<u8>>>`：`None` 代表非 DISTINCT；`Some` 保存“比较键 -> 输出值”。使用映射而非集合，才能保留 collation key 对应的原始输出字节。
- `truncated: bool`：该聚合对象自创建以来是否至少截断过一次；`reset` 不清零。
- `pub(crate) has_value: bool`：记录是否处理过非 NULL 输入，弥补空 `Vec<u8>` 无法区分 NULL 组与空字符串结果的问题。

公开方法包括 `new`、`reset`、`update`、`update_keyed`、`merge`、`result` 和 `truncated`。文件没有模块级常量、自由函数、trait 定义、trait impl 或条件编译分支。`#[derive(Clone, Debug, PartialEq)]` 支持模板复制、spill round-trip 对比和测试断言，但没有 `Default`，因为分隔符、长度上限和 DISTINCT 模式都是必要配置。

## 执行流程

普通更新由 `GroupConcat::update` 完成：

1. `rows.into_iter().flatten()` 直接跳过 `None`。
2. 在去重判断前把 `has_value` 设为真；因此非 NULL 空串和重复值仍能证明该组不是全 NULL。
3. 若存在 `distinct` 映射，将 `row.clone()` 同时作为键和值插入；已有键使本行立即跳过。
4. 若此前已有非 NULL 值，先追加 `separator`，随后追加当前行字节。
5. 当 `maximum_len > 0` 且缓冲超限时，按字节截到上限并把 `truncated` 设为真。后续输入仍会经过更新流程，但结果每次超限都会再次被截到同一上限。

带比较键更新由 `update_keyed` 完成。它先检查 key 是否存在；若不存在，暂时取走 `distinct`，调用普通 `update` 追加展示值，再把调用者提供的 key/value 放回映射。暂时取走映射可避免普通 `update` 额外以原始 value 作为键插入。

`merge` 根据目标对象的模式分流：DISTINCT 目标迭代 `source.distinct` 并调用 `update_keyed`，所以合并仍执行去重、分隔符和长度限制；非 DISTINCT 目标仅在 `source.has_value` 时调用一次 `update([source.value.clone()])`，把来源缓冲作为一个完整 partial 段追加。DISTINCT 使用 `HashMap` 迭代，因此跨分区合并后的输出排列不稳定，相关 Rust 测试只接受合法排列而不承诺固定顺序。

spill 路径在 `spill_serialize_helper.rs` 的 `impl SpillState for GroupConcat` 中完成：DISTINCT 状态写出条目数量及每个 key/value，非 DISTINCT 状态写出 `has_value` 和可选结果缓冲；读取时先 `reset`，再分别通过 `update_keyed` 或 `update` 恢复。`aggfuncs.rs::merge_spilled_partial_result` 通过 downcast 找到 `GroupConcat` 后调用 `merge`。

## 数据与状态

配置状态 `separator`、`maximum_len` 和 `distinct` 的“是否为 Some”在对象生命周期内确定。分组状态是 `value`、`distinct` 内的条目和 `has_value`；生命周期状态是 `truncated`。

关键不变量如下：

- `has_value == false` 时 `result()` 返回 `None`；一旦收到非 NULL 值，即使值为空字节串，`result()` 也返回 `Some(&[])`。
- DISTINCT 模式下，每个比较键至多保留一个输出值。普通 `update` 的比较键就是原始值；`update_keyed` 可保存不同的 collation key。
- 分隔符只放在两个已接受的逻辑值/partial 段之间，不放在开头；空字符串仍是一个已接受值，因而可能产生开头或结尾看得见的分隔符。
- `maximum_len == 0` 表示无限制，不是空结果。
- 截断按 `Vec<u8>` 长度执行，单位是字节而不是 Unicode 字符；本类型允许任意二进制数据，截断点也可能落在 UTF-8 多字节字符中间。
- `reset` 以新的 `Vec`/`HashMap` 替换旧容器，释放原有容量，并把 `has_value` 清零；它不改变配置，也不清除 `truncated`。

内存随 `value`、DISTINCT 键和值线性增长。类型本身不计算更新内存增量；spill 恢复端在 `SpillState::read_spill` 中按恢复的 key/value 长度及映射容量估算堆内存。

## 依赖与调用关系

上游构建链为 `builder.rs::build` -> `builder.rs::build_group_concat`。后者从最后一个常量参数取得 separator，从 `AggFuncBuildContext::group_concat_max_len` 取得长度上限，并按 `has_distinct`、聚合模式与 `order_by_items` 选择实现变体。

当前直接生产调用关系主要集中在 spill：

- `aggfuncs.rs::BuiltAggFunc::spill_function` 为 `GroupConcatDistinctOriginal/Partial` 调用 `GroupConcat::new(..., true)`，生成 serializer 模板和空 partial result。
- `spill_serialize_helper.rs::SpillState for GroupConcat` 调用 `clone`、`reset`、`update_keyed` 和 `update`，完成 partial state 的复制、写出与恢复。
- `aggfuncs.rs::merge_spilled_partial_result` downcast 后调用 `GroupConcat::merge`，并按 DISTINCT 映射新增条目数返回一个近似内存增量。

`lib.rs` 公开模块但没有 `pub use GroupConcat`；外部代码应经 `func_group_concat::GroupConcat` 路径访问。RustCodeGraph 将目标文件标为被执行器/测试侧文件使用；精确引用检索进一步确认生产接线在上述 builder、spill 与 merge 位置，其他直接方法调用主要来自独立测试。

需要注意两个相邻但不同的状态模型：`aggfuncs.rs::GroupConcatPartialResult` 是基于 `Cursor<Vec<u8>>` 的另一种 spill 数据结构；本文件的 `GroupConcat` 则是 DISTINCT accumulator/spill state。不能仅凭名称把两者当成同一表示。

## 错误处理与边界

本文件所有 API 都不返回 `Result`，因此不会执行表达式错误传播、类型转换报错或 SQL warning 记录。输入已被上游求值为 `Option<Vec<u8>>`：`None` 被忽略，任意字节序列均可接受。

达到上限时仅截断并设置布尔标志。相比之下，Go 的 `baseGroupConcat4String::handleTruncateError` 会根据类型上下文选择返回错误或追加 `ErrCutValueGroupConcat` warning，并用原子哨兵保证生命周期内只生成一次。Rust 调用者若要达到完整 SQL 兼容性，必须在更高层读取 `truncated()` 并负责错误/警告策略；本文件中没有该接线证据。

模式不匹配也是静默边界：DISTINCT 目标合并一个 `source.distinct == None` 的来源时不会合入 `source.value`；非 DISTINCT 目标可合并任何 `source.has_value` 的来源，但只读取其整体缓冲。构造与合并方必须保持 separator、maximum_len 和 DISTINCT 模式一致，本类型不校验这些配置。

`maximum_len` 从 builder 的 `u64` 以 `as usize` 转换；在 64 位目标上通常等宽，在较窄目标上可能截断。当前文件不做 checked conversion。使用 `HashMap` 也意味着 DISTINCT 无 ORDER BY 时结果顺序不稳定，这与测试明确记录的并行合并语义一致。

## 并发与资源生命周期

`GroupConcat` 使用普通 `Vec` 和 `HashMap`，没有锁、原子、通道、异步任务或内部共享所有权。所有修改方法需要 `&mut self`，并发隔离由执行器为每个 worker/分组持有独立 partial state 来保证；跨 worker 汇总通过 `merge` 顺序修改目标对象。

`Clone` 用于 spill serializer 模板和 partial state 复制，克隆会深拷贝结果缓冲及 DISTINCT 键值。`reset` 开始新分组时丢弃容器容量，避免上一组的大缓冲长期滞留；`truncated` 则跨分组保留，直至整个聚合对象销毁。此生命周期设计与 Go 将 `truncated *int32` 放在聚合函数对象而非 partial result 中的意图相同，但 Rust 字段不是原子值，也不支持多个线程共享同一实例并并发置位。

spill 生命周期是“模板复制 -> 从字节恢复 -> 与内存中的目标 partial 合并”。序列化只保存 partial 数据，不保存 separator、maximum_len 或 DISTINCT 模式；这些配置来自 `BuiltAggFunc` 创建的模板，因此恢复时模板必须与写出端配置一致。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/executor/aggfuncs/func_group_concat.go`。本文件保留了以下核心语义：跳过 NULL、多参数求值后的字节结果拼接、行间 separator、`maxLen == 0` 无限制、全 NULL 返回 NULL、非 NULL 空串返回空结果、普通 partial 合并、DISTINCT partial 合并以及跨分组保留截断哨兵。

对应关系并非结构同构：

- Rust 的一个 `GroupConcat` 同时用 `Option<HashMap<...>>` 表达普通与 DISTINCT；Go 分为 `groupConcat`、`baseGroupConcatDistinct4String`、`groupPartialConcatDistinct` 等类型。
- Rust 输入已经是求值完成的单个 `Vec<u8>`。Go `UpdatePartialResult` 会遍历 `e.args`，任一参数为 NULL 就跳过整行，并把多个字符串参数直接连接。Rust 测试把多参数行表示为预先连接好的字节串。
- Go DISTINCT 以每个参数的 collator key 经 codec 编码后组成联合键，同时保存原始拼接值；Rust 的 `update_keyed` 能承载这种“键和值分离”的结果，但本文件不负责生成 collation key。
- Go 完整实现含 ORDER BY 的 `topNRows`/`groupConcatOrder`、内存 delta 统计、chunk 输出、spill helper、类型上下文错误/警告和原子截断标志；本文件均未实现。ORDER BY 对应的 `AggImplementation` 也没有在此类型的 spill 绑定中出现。
- Go DISTINCT 最终遍历映射值，因此同样不承诺无 ORDER BY 的固定输出顺序。Rust 的并行 DISTINCT 测试接受 `a,b` 与 `b,a` 两种排列。

Go 测试 `func_group_concat_test.go` 还覆盖普通/多参数/ORDER BY/DISTINCT+ORDER BY、长度 4 至 7 的截断以及精确内存增量；这些是 Go 完整实现证据，不应误算成本文件已覆盖的 Rust 能力。

## 扩展指南

若扩展当前状态语义，优先修改本文件对应方法，并同步独立测试 `pkg/executor/aggfuncs/func_group_concat_test.rs`；不要把测试内嵌回生产源文件。建议按变更类型选择接入点：

- 修改 NULL、空串、separator 或上限语义：调整 `update`/`result`，补充精确字节边界和连续更新测试。
- 修改 collation-aware DISTINCT：保持 `update_keyed` 的 key/value 分离，并同步 `spill_serialize_helper.rs::SpillState` round-trip 测试，特别覆盖二进制 key、相同 key 不同 value 和恢复后的 merge。
- 修改 partial 合并：调整 `merge`，同步 `func_distinct_agg_test.rs` 与 `go_scenario_coverage_test.rs` 的分区合并场景；不要为无 ORDER BY 结果强加稳定顺序。
- 增加完整 SQL 执行能力：需在本文件范围之外接入表达式求值、collator、错误/警告上下文、精确内存计量及普通/ORDER BY 变体，并核对 `builder.rs`、`aggfuncs.rs` 和聚合执行器调用链。仅增加本类型方法不能证明 SQL 路径已支持。
- 修改 spill 格式：必须同步 `SpillState::write_spill/read_spill` 及 `spill_group_concat_keeps_binary_collation_keys_and_allocates_result_buffer`；格式没有版本字段，改变字段顺序或编码会带来兼容风险。

性能风险集中在每行 clone、`HashMap<Vec<u8>, Vec<u8>>` 双份字节保存、merge 重放以及 `reset` 放弃容量。若优化分配，必须保留空串/NULL 区分、collation key 和截断时机，并重新核对内存计量口径。正确性风险集中在 UTF-8 字节截断、配置不一致的 partial 合并，以及将 HashMap 顺序误当成 SQL 顺序保证。

## 验证依据

本说明以以下直接证据为准：

- 源文件：`pkg/executor/aggfuncs/func_group_concat.rs`，完整核对 `GroupConcat` 的 6 个字段及 7 个方法。
- crate 边界：`pkg/executor/aggfuncs/Cargo.toml` 和 `pkg/executor/aggfuncs/lib.rs`，确认库入口、公开模块、依赖范围与独立测试模块。
- 构建/接线：`pkg/executor/aggfuncs/builder.rs::{build, build_group_concat}`、`pkg/executor/aggfuncs/aggfuncs.rs::{BuiltAggFunc::spill_function, merge_spilled_partial_result}`、`pkg/executor/aggfuncs/spill_serialize_helper.rs` 中的 `impl SpillState for GroupConcat`。
- Rust 测试：`pkg/executor/aggfuncs/func_group_concat_test.rs`（NULL、空串、separator、长度上限、reset/truncated）、`func_distinct_agg_test.rs` 与 `go_scenario_coverage_test.rs`（并行 DISTINCT merge 和非确定顺序）、`spill_helper_test.rs::spill_group_concat_keeps_binary_collation_keys_and_allocates_result_buffer`（二进制 key、恢复内存和重复合并）。
- Go 对照：`pkg/executor/aggfuncs/func_group_concat.go` 的 `baseGroupConcat4String`、`groupConcat`、DISTINCT 与 ORDER BY 类型，以及 `pkg/executor/aggfuncs/func_group_concat_test.go` 的功能和内存测试。
- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件并包含目标；`files --filter pkg/executor/aggfuncs` 确认源、Go 对照及测试均在索引中；文件限定 `node` 读取了目标、builder、公共聚合状态、spill trait impl 与测试。通用 `explore`/方法名 callers 查询存在同名符号噪声，因此最终调用边以文件限定节点和精确引用交叉核对。

人工复核结论：该文件存在的理由是为 GROUP_CONCAT 提供可复制、可合并、可 spill 的字节 partial state；真实运行核心是 `update/update_keyed/merge`；安全扩展必须同时维护 spill codec、独立测试以及与 Go 在 NULL、collation、截断和无序 DISTINCT 上的兼容边界。
