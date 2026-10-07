# `pkg/executor/join/base_semi_join.rs`

## 文件定位

本文件属于 Cargo crate `astersql-executor-join`，由同目录的 `lib.rs` 以公开模块 `base_semi_join` 装配。它位于 Hash Join 的 probe（探测）阶段：`base_join_probe.rs::new_join_probe` 根据 `JoinType` 创建 `SemiJoinProbe`、`AntiSemiJoinProbe` 或 `LeftOuterSemiJoinProbe` 时，把 `BaseJoinProbe` 包进本文件的 `BaseSemiJoin`。因此，本文件不是完整执行器，也不负责建哈希表；它是 Semi、Anti Semi、Left Outer Semi 和 Anti Left Outer Semi 四类探测器共享的状态与小型操作层。

文件第 22—257 行是注释化的 Go 形状/早期移植草稿，不参与编译。当前有效 Rust 实现从 `use crate::base_join_probe::{BaseJoinProbe, WorkerResult}` 开始（第 258 行），公开 API 是 `MAX_MATCHED_ROW_NUM`、`BaseSemiJoin` 及其五个方法。理解和扩展时必须以第 258—347 行的有效代码为准，不能把注释中的 `matchMultiBuildRows`、`concatenateProbeAndBuildRows` 或 Chunk 指针接口视为已实现功能。

## 核心职责

`BaseSemiJoin` 负责三类共享工作：

1. 持有底层 `BaseJoinProbe`，并记录左/右哪一侧是 build side（`is_left_side_build`）。具体 probe 仍负责按 build side 选择执行分支。
2. 为当前 probe chunk 维护逐行的 `matched` 和 `has_null` 状态，并把右侧 build 路径的一行 probe 交给 `Joiner::try_to_match_inners` 求值。
3. 在左侧 build 路径结束后，按 `build_row_used` 标记和 `max_chunk_size` 分批扫描 build 行，以实现 Semi 输出“已使用”行、Anti Semi 输出“未使用”行。

它不直接负责候选键查找：`BaseJoinProbe::set_chunk_for_probe` 已经生成 `matched_rows`，`BaseJoinProbe::candidate_rows` 再为 `match_probe_row` 提供候选行。它也不自行决定最终行形状；匹配和未匹配输出由 `joiner.rs::Joiner` 按 `JoinType`、投影列和三值逻辑生成。

## 主要符号

- `MAX_MATCHED_ROW_NUM: usize = 4`：对应 Go 的 `maxMatchedRowNum` 概念，意图限制带 other condition 时单个 probe 行造成的中间结果膨胀。当前有效 Rust 代码没有读取这个常量；实际 `Joiner::try_to_match_inners` 对半连接族命中后直接停止。因此它目前只是公开的移植语义标记，不是有效的容量控制开关。
- `BaseSemiJoin`：`#[derive(Clone)]` 的共享状态对象。
  - `base: BaseJoinProbe`：哈希候选、当前 probe chunk、游标、共享 `HashJoinContext` 和 build 扫描游标的实际所有者。
  - `is_left_side_build: bool`：由 `new_join_probe` 依据 `right_as_build_side` 设置；Left Outer Semi 系列只允许右侧 build，工厂会对不合法组合返回错误。
  - `matched: Vec<bool>`：与当前 `base.probe_chunk` 等长，累计每个 probe 行是否命中。
  - `has_null: Vec<bool>`：与当前 probe chunk 等长，累计 other condition 的 SQL NULL 结果；具体输出由 `Joiner::on_miss_match` 解释。
  - `unfinished_probe_rows: VecDeque<usize>`：记录候选未完全消费的 probe 行。当前半连接族的 `try_to_match_inners` 在命中时把 `consumed` 设置为全部候选数，未命中时也遍历全部候选，因此正常成功路径下该队列通常为空；各具体 probe 仍将其纳入 `is_current_chunk_probe_done` 判断，为分段消费语义保留接口。
- `BaseSemiJoin::new(base, is_left_side_build) -> Self`：接管底层 probe，并以空向量/空队列初始化本批状态。
- `reset_probe_state(&mut self)`：按当前 `probe_chunk.len()` 把 `matched`、`has_null` 重建为全 `false`，并清空未完成队列。
- `match_probe_row(&mut self, probe_index, output, naaj) -> Result<MatchResult, String>`：克隆当前 outer/probe 行及其候选 build 行，调用 `Joiner::try_to_match_inners`，累计匹配与 NULL 状态，并在未消费完候选时入队。
- `produce_probe_mismatches(&self, expected_match, output)`：筛选 `matched == expected_match` 的 probe 行，并交给 `Joiner::on_miss_match`。RustCodeGraph 与源码引用搜索均未发现当前生产调用者；这是可复用公开辅助方法，不应描述为主链必经步骤。
- `scan_build_rows(&mut self, expected_used) -> WorkerResult`：从 `base.scan_row_index` 继续扫描 `context.build_rows`，只复制 `build_row_used == expected_used` 的行，单批最多返回 `max_chunk_size` 行。

## 执行流程

创建流程如下：`base_join_probe.rs::new_join_probe` 先创建 `BaseJoinProbe`；Semi 和 Anti Semi 传入 `!right_as_build_side`，Left Outer Semi / Anti Left Outer Semi 只在右侧 build 时传入 `false`。随后具体 probe 的 `set_chunk_for_probe` 先让 base 计算序列化键和候选 build 行，再调用 `reset_probe_state` 建立与本批行数一致的标志数组。

右侧作为 build side 时，`SemiJoinProbe::probe`、`AntiSemiJoinProbe::probe` 和 `LeftOuterSemiJoinProbe::probe` 逐行调用 `match_probe_row`：

1. 用 `probe_index` 取当前 outer 行，并从 `BaseJoinProbe` 取得相同 key 的候选 build 行。
2. `Joiner::try_to_match_inners` 依次拼接 inner/outer，执行所有 other conditions。条件全部为真才算命中；出现 false 立即判该候选失败；没有 false 但出现 NULL 时记录 `has_null`。
3. Semi 命中时输出投影后的 outer 行；Anti Semi 命中时不输出；Left Outer Semi / Anti Left Outer Semi 输出带布尔或 NAAJ 标记的 outer 行。半连接族首次命中即停止扫描剩余候选。
4. `match_probe_row` 把本次 `MatchResult` OR 到 `matched[probe_index]`、`has_null[probe_index]`。若未命中，Anti/Outer 具体 probe 随后调用 `Joiner::on_miss_match` 生成未匹配结果；普通 Semi 不补行。
5. 具体 probe 调用 `BaseJoinProbe::finish_current_lookup_loop` 推进到下一 probe 行，并以 `max_chunk_size` 控制本批输出。

左侧作为 build side 时，本文件的 `match_probe_row` 不在主分支中使用。Semi/Anti Semi 具体 probe 调用 `Joiner::try_to_match_outers`，把对应的 build 行标为 used；probe 阶段结束后分别调用 `scan_build_rows(true)` 或 `scan_build_rows(false)`，分批输出已命中或未命中的左侧 build 行。

## 数据与状态

`matched`、`has_null` 和 `unfinished_probe_rows` 都是“当前 probe chunk”级状态，必须在 `set_chunk_for_probe` 或 `reset_probe` 后由 `reset_probe_state` 清空。前两个向量的下标与 `base.probe_chunk` 严格对应；调用者若绕过重置、传入越界 `probe_index`，Rust 下标访问会 panic。代码没有单独返回这种编程错误。

`match_probe_row` 会克隆 outer 行和候选 build 行集合，然后把结果行追加到调用者提供的 `Vec<Row>`。这种所有权设计避免借用 `self.base` 的同时可变更新本地状态，但候选多、行宽大时会产生复制成本。`matched` 与 `has_null` 使用 `|=` 累计，允许理论上的分段重入不丢失早先状态。

`scan_build_rows` 的持续位置保存在 `BaseJoinProbe::scan_row_index`；每检查一行就递增，即使该行不符合 `expected_used`。返回容量按“已选中的输出行数”而不是“已扫描行数”限制，所以稀疏命中时一次调用可能遍历大量 build 行。返回的 `WorkerResult.error` 固定为 `None`。

`build_row_used` 属于 `HashJoinContext`。本文件只读该数组；左侧 build 路径由 `semi_join_probe.rs` / `anti_semi_join_probe.rs` 在 `try_to_match_outers` 后写标记。右侧 build 的 `match_probe_row` 不应修改它，这一点由 `base_semi_join_test.rs::matching_probe_row_does_not_mark_every_candidate_build_row_used` 直接验证。

## 依赖与调用关系

上游装配链是：Hash Join 执行路径 → `base_join_probe.rs::new_join_probe` → 具体 `Probe` 实现 → `BaseSemiJoin`。直接生产使用者为：

- `semi_join_probe.rs`：重置状态、右 build 时调用 `match_probe_row`、左 build 时调用 `scan_build_rows(true)`。
- `anti_semi_join_probe.rs`：重置状态、右 build 时调用 `match_probe_row` 并在未命中时补行、左 build 时调用 `scan_build_rows(false)`。
- `left_outer_semi_join_probe.rs`：重置状态，并带 `NaajType` 调用 `match_probe_row`；它不扫描 build row table。

直接下游依赖为 `base_join_probe.rs::{BaseJoinProbe, WorkerResult}`、`joiner.rs::{MatchResult, NaajType, Row}` 和标准库 `VecDeque`。`match_probe_row` 的关键调用边是 `BaseJoinProbe::candidate_rows` 与 `Joiner::try_to_match_inners`；`produce_probe_mismatches` 的关键调用边是 `Joiner::on_miss_match`。

`Cargo.toml` 声明 crate 名为 `astersql-executor-join`，库入口为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/executor/join"` 标出 Go 对照包。当前文件只使用同 crate 模块和 `std`，没有直接使用 Cargo 中列出的外部 crate；大量依赖仅在 Windows target 下声明，也不改变本文件的模块内调用关系。

## 错误处理与边界

`match_probe_row` 唯一的可恢复错误来自 `Joiner::try_to_match_inners`：other condition (`Predicate`) 返回 `Err(String)` 时通过 `?` 原样传播。三个具体 probe 把它装入 `WorkerResult.error` 并保留此前已生成的 `rows`。本文件不包装上下文，因此扩展错误信息时应在条件求值或具体 probe 边界统一处理，避免不同 join 类型的消息不一致。

边界行为包括：空候选返回默认 `MatchResult`；空 probe chunk 经重置得到空状态；`max_chunk_size` 控制 build 扫描输出量；键含 NULL 的 build 行在 `HashJoinContext::new` 阶段不会进入等值哈希表。普通 Left Outer Semi 的条件 NULL 最终形成 NULL marker，Anti Semi 在非 null-aware 模式且 `has_null` 为真时不会由 `on_miss_match` 输出。NAAJ 的 probe/build key NULL 判断位于 `left_outer_semi_join_probe.rs`，不由本文件自行推导。

需要特别注意两个当前限制：`MAX_MATCHED_ROW_NUM` 没有接入有效 Rust 路径；`unfinished_probe_rows` 虽会在 `consumed < inners.len()` 时入队，但本文件没有恢复候选偏移的状态，当前具体 probe 也没有显式出队续跑逻辑。现有 `Joiner` 对半连接族总会在成功返回时消费全部候选，因而通常不会触发该缺口；若未来改为容量驱动的部分消费，必须同时设计候选游标和续跑流程，不能只让队列变为非空。

## 并发与资源生命周期

`BaseSemiJoin` 自身没有锁、原子变量、通道、异步任务或裸指针。它由单个具体 probe worker 持有并以 `&mut self` 串行推进；`#[derive(Clone)]` 会深拷贝 `BaseJoinProbe`、`HashJoinContext`、行数据、状态向量和队列，并不是共享可变上下文。`Predicate` 内部使用 `Arc<dyn Fn + Send + Sync>`，因此 clone 后条件闭包共享，但本文件不管理其生命周期。

每个 probe chunk 的生命周期是“设置 chunk → `reset_probe_state` → 多次 `probe` → 可选 build row table 扫描 → `reset_probe`”。`scan_row_index` 属于 base，需要在具体 probe 的 `init_for_scan_row_table` 中置零；`scan_build_rows` 自身不会自动重置。结果行通过 clone 离开 build/probe 存储，不借用内部缓冲，因此 `WorkerResult` 返回后不依赖 `BaseSemiJoin` 的存活。

Go 版以 worker 私有 `baseSemiJoin` 配合共享哈希表、原子 used 标记和 Chunk/queue 复用；当前 Rust 简化模型的 `HashJoinContext` 是值拥有结构，不能据此推断已经具备 Go 版的并发共享与原子竞争语义。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/join/base_semi_join.go`。概念映射为：`baseSemiJoin` → `BaseSemiJoin`，`newBaseSemiJoin` → `new`，`resetProbeState` → `reset_probe_state`，Go 的 `isMatchedRows` / `isNulls` → Rust 的 `matched` / `has_null`，Go 的 build row 扫描语义 → Rust 的 `scan_build_rows`。两版都服务于 Semi/Anti Semi 家族，并都区分左右 build side、other condition、匹配与 NULL 状态。

但当前 Rust 不是 Go 文件的逐函数等价实现：

- Go 的 `maxMatchedRowNum`、`matchMultiBuildRows`、`concatenateProbeAndBuildRows` 用中间 joined Chunk 和可复用队列限制 other-condition 膨胀，并在批尾检查 SQL killer；Rust 有常量和队列字段，但有效代码直接调用行式 `Joiner::try_to_match_inners`，没有 SQL killer 检查或 Chunk 拼接路径。
- Go 的 `generateResultChkForRightBuildNoOtherCondition` / `generateResultChkForRightBuildWithOtherCondition` 负责列式复制和零列虚拟行数；Rust 以 `Vec<Row>` 和 `Joiner` 投影输出，没有虚拟行数概念。
- Go 的 `resetProbeState` 仅在右侧 build 时重置 `isMatchedRows`，并仅在有 other condition 时维护未完成队列；Rust 每批无条件重置 `matched`、`has_null` 和队列。
- Go 的左 build 路径使用 row iterator 与原子 used 标记；Rust 由具体 probe 写 `Vec<bool>`，本文件用索引扫描。

Go 回归测试提供更宽的行为基线：`semi_join_probe_test.go` 覆盖左右 build、重复 key、other condition、spill 与多 key；`anti_semi_join_probe_test.go` 还覆盖 NULL/NOT IN；`left_outer_semi_join_probe_test.go` 与 `left_outer_anti_semi_join_probe_test.go` 覆盖 marker、过滤、选择向量和 spill。Rust 独立测试目前只直接锁定“匹配一条 probe 行不会误标记所有候选 build 行 used”，不能据此宣称上述 Go 矩阵已全部在 Rust 中得到同等验证。

## 扩展指南

新增或修改半连接行为时，先按职责选择接入点：候选键/游标属于 `BaseJoinProbe`；逐候选条件、三值逻辑和输出形状属于 `Joiner`；Semi/Anti 或左右 build 的调度属于三个具体 probe；只有跨这些 probe 共享的匹配状态或 build 扫描才应放进 `BaseSemiJoin`。

若让 `MAX_MATCHED_ROW_NUM` 真正生效或支持候选分段消费，需要同时修改 `match_probe_row`、`BaseJoinProbe` 的候选游标、`unfinished_probe_rows` 的出入队流程以及三个具体 probe 的完成判断，并新增独立测试覆盖：单 key 超过 4 个候选、输出容量耗尽后续跑、首次命中提前结束、条件全 false/NULL、错误发生在中段时的状态。不能只截断 `inners`，否则会把“尚未处理”误判为“不匹配”。

修改 `matched` / `has_null` 语义时，应同步 `base_semi_join_test.rs`，并在 `semi_join_probe_test.rs`、`anti_semi_join_probe_test.rs` 或 `left_outer_semi_join_probe_test.rs` 中按受影响类型增加独立测试；Rust 测试逻辑不得内嵌回本生产文件。还应对照四个 Go probe 测试中的左右 build、重复 key、NULL、other condition、空投影和 spill 场景，避免用较小的 Rust 行模型删减 SQL 语义。

性能上重点关注 `match_probe_row` 对 outer/候选行的 clone，以及 `scan_build_rows` 在稀疏命中时的全表扫描。兼容性上重点关注 NAAJ 的 NULL marker、Anti Semi 的未匹配输出，以及 Left Outer Semi 只允许右侧 build 的工厂约束。若引入共享并发上下文，必须另行明确 `build_row_used` 的同步协议，不能依赖当前 clone 值语义。

## 验证依据

- RustCodeGraph `status`：索引可用，覆盖 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph 文件/符号读取：`base_semi_join.rs`（完整 347 行）、`base_join_probe.rs::{HashJoinContext, BaseJoinProbe, new_join_probe}`、`joiner.rs::{MatchResult, try_to_match_inners, on_miss_match}`、`semi_join_probe.rs`、`anti_semi_join_probe.rs`、`left_outer_semi_join_probe.rs`。
- RustCodeGraph 调用证据：`new_join_probe` 构造三类 `BaseSemiJoin`；`reset_probe_state` 与 `match_probe_row` 有具体 probe/测试调用；`scan_build_rows` 由 Semi/Anti 的 row-table 扫描入口调用；图中未发现 `produce_probe_mismatches` 的生产调用者。对图中方法自调用噪声又以限定目录的源码引用搜索复核。
- crate 与模块证据：`pkg/executor/join/Cargo.toml`、`pkg/executor/join/lib.rs`。该目录没有 `doc.go`，因此最近的模块契约取自 Rust `lib.rs`。
- Rust 测试证据：`pkg/executor/join/base_semi_join_test.rs::matching_probe_row_does_not_mark_every_candidate_build_row_used`。
- Go 对照证据：`pkg/executor/join/base_semi_join.go`、`semi_join_probe_test.go`、`anti_semi_join_probe_test.go`、`left_outer_semi_join_probe_test.go`、`left_outer_anti_semi_join_probe_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证要求目标文件存在且恰有上述 11 个固定二级标题。
