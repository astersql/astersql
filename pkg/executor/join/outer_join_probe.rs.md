# `pkg/executor/join/outer_join_probe.rs`

## 文件定位

本文件是 `astersql-executor-join` crate 中 Left Outer Join 与 Right Outer Join 共用的 Hash Join probe 实现。模块由 `pkg/executor/join/lib.rs` 以 `pub mod outer_join_probe` 暴露；实例并非在本文件内直接构造，而是由 `pkg/executor/join/base_join_probe.rs::new_join_probe` 根据 `JoinType` 和 `right_as_build_side` 创建，并以 `Box<dyn Probe>` 交给上层执行流程。

文件的结构需要特别区分：第 19—419 行是对 Go `outerJoinProbe` 的映射注释，不参与 Rust 执行；可运行实现从 `use crate::...`、`OuterJoinProbe` 及其 `impl Probe` 开始。当前 Rust 实现使用内存中的 `Vec<Row>`、build 行下标和 `WorkerResult`，而注释保留了 Go v2 的 chunk、tagged pointer、SQL killer 与行表细节，不能把注释中的能力当作当前 Rust 路径已经接线的事实。

`pkg/executor/join/Cargo.toml` 将 crate 根指定为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/executor/join"` 声明 Go 对照包。该文件的可执行代码只直接引用同 crate 的 `base_join_probe` 与 `joiner` 模块，没有自己的 feature 或条件编译项。

## 核心职责

`OuterJoinProbe` 负责维持外连接的核心不变量：outer 侧的每一行最终至少产生一行输出。实现按 outer 侧位于 build 还是 probe 分成两种互补路径：

- `outer_side_build == false`：probe 行就是 outer 行。`probe` 将候选 build 行作为 inner 交给 `Joiner::try_to_match_inners`；候选全部处理完仍无有效匹配时，立即通过 `Joiner::on_miss_match` 为 inner 侧补默认行。
- `outer_side_build == true`：build 行就是 outer 行。`probe` 将候选 build 行作为 outers 交给 `Joiner::try_to_match_outers`，并把返回状态为 `OuterRowStatus::Matched` 的 build 行记入 `HashJoinContext::build_row_used`；probe 阶段结束后，`scan_row_table` 再输出所有未使用 build 行并为 inner/probe 侧补默认值。

它还把 chunk 装载、spill、完成度判断和重置委托给 `BaseJoinProbe`，并以 `max_chunk_size` 控制一次 `WorkerResult` 的产量，使一个 probe 行的重复匹配可以跨多次 `probe()` 调用续跑。

## 主要符号

- `pub struct OuterJoinProbe`：唯一生产类型，派生 `Clone`，包含 `base: BaseJoinProbe`、`outer_side_build: bool` 和 `right_side_build: bool`。`right_side_build` 记录物理 build 方向，当前文件的运行方法不直接读取它；结果左右列顺序由构造进 `HashJoinContext` 的 `Joiner` 负责。
- `impl Probe for OuterJoinProbe`：实现统一 probe 生命周期。`set_chunk_for_probe`、`set_restored_chunk_for_probe`、`spill_remaining_probe_chunks`、`is_current_chunk_probe_done`、`reset_probe` 是对 `BaseJoinProbe` 的薄委托。
- `probe(&mut self) -> WorkerResult`：主入口。按输出余量截取当前 probe 行的 build 候选，调用 Joiner，更新候选/行游标，并把条件计算错误封装到 `WorkerResult.error`。
- `need_scan_row_table(&self) -> bool`：仅在 outer 为 build 侧时返回 true，通知上层 probe 后还有补行阶段。
- `init_for_scan_row_table`、`scan_row_table`、`is_scan_row_table_done`：outer-build 专用的 build 行表扫描协议；错误方向调用会以 `assert!` 触发 panic。
- `NaajType::Unknown`：传给 `try_to_match_inners`。本文件服务普通 Left/Right Outer Join，不执行 null-aware anti join 的特殊分支。
- `OuterRowStatus::Matched`：outer-build 路径中决定哪些 build 行已被真正匹配；只有该状态才写 `build_row_used`。

本文件没有模块级常量、自由函数、额外 trait、宏或条件编译项。

## 执行流程

1. `base_join_probe.rs::new_join_probe` 先创建 `BaseJoinProbe`。Left Outer 在左侧 build 时、Right Outer 在右侧 build 时将 `outer_side_build` 置为 true；另一个方向置为 false。
2. 上层通过 `set_chunk_for_probe` 或 `set_restored_chunk_for_probe` 装载输入。`BaseJoinProbe` 校验 probe key 下标、序列化键、查哈希桶并以完整键过滤候选，同时将 `current_probe_row` 与 `current_candidate` 归零。
3. `probe` 循环处理当前 probe 行，停止条件是 probe chunk 耗尽或本次 `rows.len()` 达到 `context.max_chunk_size`。它从 `matched_rows[index]` 的 `current_candidate` 起截取本次可容纳的候选，并克隆相应 build 行交给 Joiner。
4. outer-build 路径调用 `try_to_match_outers(build_rows, probe_row, rows)`。返回的状态与本次 `build_indices` 按位置对应；状态为 `Matched` 的 build 下标被置为 used。若 Joiner 报错，立即返回已产生的行和错误，不再推进本批候选游标。
5. inner-build 路径调用 `try_to_match_inners(probe_row, build_rows, rows, NaajType::Unknown)`。命中时调用 `BaseJoinProbe::mark_build_rows_used(index)`；未命中且当前切片已经覆盖该 probe 行的全部候选时，才调用 `on_miss_match`，避免候选因容量分批时过早伪造 unmatched 行。
6. 每个成功处理的候选切片都会推进 `current_candidate`。到达该 probe 行候选末尾后，`finish_current_lookup_loop` 将 `current_probe_row` 加一并清零候选游标。空候选也会走这一步，因此 inner-build 的空候选行会获得默认 inner 输出。
7. 若 `need_scan_row_table` 为 true，上层必须调用 `init_for_scan_row_table`，再重复调用 `scan_row_table` 直到 `is_scan_row_table_done`。扫描以 `scan_row_index` 顺序跳过 used build 行，未使用行经 `on_miss_match(false, build, rows)` 输出，每批仍受 `max_chunk_size` 限制。

## 数据与状态

`OuterJoinProbe` 自身只增加两个方向标志，绝大部分可变状态位于 `BaseJoinProbe` 与其 `HashJoinContext`：

- `probe_chunk` 保存当前输入；`matched_rows[probe_row]` 保存按 join key 真正相等的 build 下标，而非仅同 hash 的候选。
- `current_probe_row` 与 `current_candidate` 组成可恢复游标。前者定位 probe 行，后者允许同一行的大量重复匹配跨输出批次继续。
- `context.max_chunk_size` 是单次输出上限；候选切片至少尝试一个元素，但外层循环只在当前输出未满时进入。
- `context.build_row_used` 与 `context.build_rows` 下标对齐，是 outer-build 补行阶段的持久标记。`scan_row_index` 是扫描游标。
- `spilled_chunks` 由基座管理；`spill_remaining_probe_chunks` 保存从当前 probe 行开始的尾部，随后把当前 chunk 标为处理完。恢复路径重新走正常 key 准备逻辑。
- `WorkerResult` 同时携带 `rows` 与 `Option<String>` 错误。错误发生前已经追加到局部 `rows` 的内容会随错误一并返回，调用者必须先检查 `error`，不能把“有行”解释为整批成功。

`reset_probe` 清空当前 probe 缓冲、hash/key/候选、probe 游标、冲突计数与 `scan_row_index`，但 `BaseJoinProbe::reset_probe` 明确不清除 `context.build_row_used`。因此它是 worker probe 状态重置，不是重新初始化 build 哈希表。

## 依赖与调用关系

上游构造边是 `base_join_probe.rs::new_join_probe -> OuterJoinProbe`。该工厂对 Left Outer 使用 `outer_side_build = !right_as_build_side`，对 Right Outer 使用 `outer_side_build = right_as_build_side`，准确表达“左/右外连接的保留侧是否为 build 侧”。上层通过 `Probe` trait 调用本文件，而不依赖具体类型。

主要下游边如下：

- `OuterJoinProbe::{set_chunk_for_probe,set_restored_chunk_for_probe,spill_remaining_probe_chunks,is_current_chunk_probe_done,reset_probe}` -> 对应 `BaseJoinProbe` 方法。
- `OuterJoinProbe::probe` -> `Joiner::try_to_match_outers` 或 `Joiner::try_to_match_inners`；随后可能调用 `BaseJoinProbe::mark_build_rows_used`、`BaseJoinProbe::finish_current_lookup_loop`、`Joiner::on_miss_match`。
- `OuterJoinProbe::scan_row_table` -> `HashJoinContext::{build_rows,build_row_used,max_chunk_size}` 与 `Joiner::on_miss_match`。

RustCodeGraph 将目标文件识别为 14 个符号，并报告它被 `base_join_probe.rs`、`left_outer_join_probe_test.rs`、`left_outer_semi_join_probe.rs`、`outer_join_probe_test.rs` 引用。精确运行时构造关系由 `base_join_probe.rs::new_join_probe` 源码核实；测试还从 `right_outer_join_probe_test.rs` 经工厂覆盖相同类型。Cargo 层面，这些模块都属于同一 `astersql-executor-join` crate，因此本文件没有跨 crate API 调用。

## 错误处理与边界

- chunk 装载返回 `Result<(), String>`；键列越界、前一个 chunk 尚未完成等错误来自 `BaseJoinProbe`，本文件原样传播。
- Joiner 的 predicate/条件错误不会 panic，而是作为 `WorkerResult.error` 返回。`left_outer_propagates_other_condition_error_without_fabricating_a_miss` 与右连接同名测试证明条件错误时不会额外生成 unmatched 行。
- `init_for_scan_row_table`、`scan_row_table`、`is_scan_row_table_done` 仅允许 outer-build 路径。三个方法都以 `assert!(outer_side_build, "should not reach here")` 防御误用，`outer_join_probe_test.rs` 分别验证 panic。
- probe key 含 NULL 时，候选集合由基座置空，因此普通外连接不会把 NULL join key 当作相等；Left/Right Outer 的复合键测试同时验证所有 key 分量必须匹配。
- 候选很多且输出容量很小时，`candidate_complete` 防止中间批次把尚未扫完的 outer probe 行判成未匹配；左右连接的容量边界测试验证可继续调用直至 `is_current_chunk_probe_done()`。
- `max_chunk_size` 应为正数。当前 `probe` 用剩余容量计算候选范围；若外部构造零容量上下文，循环不能推进。现有构造与测试都使用正值，本文件没有单独校验这一前置条件。
- 与 Go 注释路径不同，可运行 Rust 代码没有 `SQLKiller` 检查、unsafe row pointer 或 chunk incomplete 标志；这些不能作为当前 Rust 实现的取消或内存安全机制来依赖。

## 并发与资源生命周期

本类型的方法都要求 `&mut self`，单个实例在一次调用中串行推进游标；本文件不创建线程、锁、任务或通道。并行度应由上层为各 worker 持有独立 probe 实例来提供，不能在没有额外同步的情况下共享并发修改同一 `OuterJoinProbe`。

生命周期依次为：工厂绑定 build 上下文 -> 装载普通或恢复的 probe chunk -> 重复 `probe` 直至 chunk 完成 -> 必要时 spill/恢复 -> outer-build 情况下初始化并分批扫描 row table -> `reset_probe` 后复用。`Clone` 会克隆 `BaseJoinProbe` 及其上下文数据；它不是共享 used bitmap 的轻量句柄，调用者不应假设克隆实例之间会自动合并匹配标记。

`probe` 与 `scan_row_table` 的输出 `Vec<Row>` 拥有其行数据；候选 build 行和 probe 行在传给 Joiner 前被克隆，所以本文件没有借用跨调用存活，也没有显式资源释放。spill 在当前简化实现中是内存中的 `Vec<Vec<Row>>` 转移，并非本文件直接管理磁盘文件。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/join/outer_join_probe.go`。两版保持了最重要的语义分叉：inner side build 时由 probe/outer 行跟踪匹配并即时补默认 inner；outer side build 时标记 build outer 行并在 probe 完成后扫描未匹配行。两版也都禁止在 inner-build 路径调用 row-table 扫描协议，并支持额外条件拒绝键匹配后仍保留 outer 行。

当前 Rust 是面向 `Row`/`Vec<Row>` 的独立实现，不是 Go 文件逐语句的运行时等价物。主要差异包括：

- Go 通过 `isNotMatchedRows`、`selected` 和 vectorized expression filter 维护逻辑行状态；Rust 把匹配判定与条件执行封装在 `Joiner::{try_to_match_inners,try_to_match_outers}` 的返回值中。
- Go 根据 `buildColUsed`、`probeColUsed` 和结果 chunk offset 复制投影列；Rust 将列顺序和 `children_used` 投影交给 `Joiner`。`right_outer_used_column_matrix_preserves_go_projection_contract` 提供了对应回归证据。
- Go row table 使用 tagged pointer、used flag、chunk 与 `rowIter`；Rust 使用 build 下标、`Vec<bool>` 和 `scan_row_index`。
- Go probe 检查 `SQLKiller`、处理 incomplete chunk，并把错误写入 worker result；Rust 当前只传播装载错误与 Joiner 字符串错误，没有取消检查。
- Go `ResetProbe` 清除 `rowIter` 后重置基座；Rust 基座重置 `scan_row_index`，但保留 build used 状态。Rust 的 `left_outer_reset_clears_partial_probe_and_scan_state` 明确覆盖这一当前契约。

Go 的 left/right outer probe 测试位于 `left_outer_join_probe_test.go` 与 `right_outer_join_probe_test.go`；Rust 的对应行为测试位于独立的 `left_outer_join_probe_test.rs`、`right_outer_join_probe_test.rs` 和 `outer_join_probe_test.rs`，符合源文件与测试文件分离约束。

## 扩展指南

修改 probe 主状态机时优先落在 `OuterJoinProbe::probe`，并同步检查 `BaseJoinProbe::{set_chunk_for_probe,finish_current_lookup_loop,mark_build_rows_used}` 的游标和 used 标记契约。新增 outer-build 补行行为则修改 `scan_row_table`，但必须保持 `need_scan_row_table`/初始化/完成判断三段协议一致。

安全扩展时应守住以下不变量：outer probe 行只能在全部候选处理完且没有有效匹配后补一次默认 inner；outer build 行只有 Joiner 确认 `Matched` 才能标记 used；输出达到容量时必须保留 `current_candidate` 以便续跑；predicate 错误不能转化为 unmatched 输出；左右表顺序和 inline projection 必须继续由 Joiner 契约保证。

测试应继续放在独立文件而非 `outer_join_probe.rs` 内。通用扫描误用放在 `outer_join_probe_test.rs`；Left/Right Outer 的匹配、条件、投影、NULL/复合键、容量续跑、spill 与 reset 分别扩展 `left_outer_join_probe_test.rs` 和 `right_outer_join_probe_test.rs`。若引入 Go 已有的 SQL kill、磁盘 spill 或 chunk 特性，需要先确认相关 Rust 基础设施真实接线，不能只照抄本文件顶部的 Go 映射注释。

性能方面，当前每批会克隆 probe 行、build 下标切片和 build 行；扩大候选规模或增加状态时应关注这些分配。兼容性方面，修改 `outer_side_build` 的工厂计算、`Joiner::on_miss_match` 输入方向或 used bitmap 生命周期会同时影响 Left 与 Right Outer Join，必须成对验证。

## 验证依据

- RustCodeGraph：`status` 显示索引含目标文件；`files --filter pkg/executor/join/outer_join_probe.rs` 报告 1 个文件、14 个符号；`node --file ... --offset 1 --limit 500` 与后续 500—568 行查询读取了完整文件，并给出被 `base_join_probe.rs` 及相关测试引用的文件级关系；`query OuterJoinProbe --limit 20 --json` 同时定位 Rust 结构体与 Go `outerJoinProbe`/`newOuterJoinProbe`。对 struct 的 `callers`/`callees` 查询未返回可用边，因此调用关系又由工厂、trait 和测试源码直接核验，未把缺失图边推断成事实。
- Rust 源码：`pkg/executor/join/outer_join_probe.rs`（完整文件）、`base_join_probe.rs`（`Probe`、`BaseJoinProbe`、`new_join_probe`）、`joiner.rs`（`try_to_match_inners`、`try_to_match_outers`、`on_miss_match`）、`lib.rs`（模块装配）。
- crate 声明：`pkg/executor/join/Cargo.toml`，确认 crate 名、`lib.rs` 入口、Go 包映射和依赖边界。
- Go 对照：`pkg/executor/join/outer_join_probe.go`；并以 `left_outer_join_probe_test.go`、`right_outer_join_probe_test.go` 的 RustCodeGraph 符号检索确认 Go 回归面。
- Rust 独立测试：`pkg/executor/join/outer_join_probe_test.rs`、`left_outer_join_probe_test.rs`、`right_outer_join_probe_test.rs`，覆盖扫描方向断言、匹配/补默认值、other condition、outer-build 扫描、spill 恢复、复合键/NULL、容量续跑、错误传播、投影与 reset。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `rg` 命令验证本文恰有十一个固定二级标题，并人工复核本文区分了注释映射与可执行 Rust 事实。
