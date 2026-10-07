# `pkg/executor/join/hash_join_test_util.rs`

## 文件定位

本文件是 `astersql-executor-join` crate 的 Hash Join 测试辅助层。`pkg/executor/join/lib.rs` 以公开模块 `hash_join_test_util` 装配它，而其直接使用方是同 crate 的 `hash_join_test_util_test.rs`、`hash_join_v1_test.rs`，以及 `pkg/executor/test/jointest/hashjoin/hash_join_test.rs`。它不属于 SQL 请求的生产入口，也不实现哈希表或连接算法；它把测试参数转换成 `HashJoinV1Exec`，构造确定性输入，驱动执行器生命周期并规范化结果，供测试聚焦连接语义。

`pkg/executor/join/Cargo.toml` 表明该文件归属 `astersql-executor-join`，crate 根为同目录 `lib.rs`，Go 对照包为 `pkg/executor/join`。目标文件自身没有条件编译项；尽管模块名称带 `test_util`，`lib.rs` 对它使用的是无 `#[cfg(test)]` 的 `pub mod`，因此它会进入常规 crate API，而独立测试模块才受 `#[cfg(test)]` 限制。

## 核心职责

- `HashJoinInfo` 汇总连接类型、两侧键列、输入 chunk、outer/null-aware 开关、并发度、输出批大小、未匹配默认行、附加谓词和列投影，减少测试重复装配。
- `build_hash_join_v1_exec` 把参数分别下沉到 `HashJoinCtxV1`、`Joiner` 和 `HashJoinV1Exec`，形成可执行的 v1 测试实例。
- `execute_hash_join_exec` 及其包装函数统一 `open -> next* -> close` 的拉取过程，特别保证正常结束和错误结束都会执行 `close`。
- 比较与排序函数把并发或执行策略造成的不稳定输出顺序转为可重复断言；数据构造函数提供简单、确定性的 `(Int, Text)` 行与 schema 下标。

本文件的职责边界是“测试装配和断言便利性”。真正的 build/probe、取消检测、spill、outer 行补发与连接谓词处理位于 `hash_join_v1.rs`、`hash_join_base.rs` 和 `joiner.rs`。

## 主要符号

- `pub struct HashJoinInfo`：唯一的模块级类型。所有字段均公开且类型实现 `Clone`，让测试和 v1/v2 对照代码能复用同一份参数。`validate(&self) -> Result<(), String>` 只做第一层检查：build/probe 键数量一致、`concurrency` 与 `max_chunk_size` 非零。
- `build_hash_join_v1_exec(&HashJoinInfo) -> Result<HashJoinV1Exec, String>`：主要装配入口。它克隆拥有所有权的数据，构造 `HashJoinContextBase::default()`、`HashJoinCtxV1` 和 `Joiner`，最后调用 `HashJoinV1Exec::new`。
- `generate_cmp_func() -> impl Fn(&Row, &Row) -> Ordering`、`sort_rows(Vec<Chunk>) -> Vec<Row>`：按列词典序比较并展平、排序结果。内部私有 `compare_rows` 和 `compare_value` 复用相同规则。
- `build_join_key_int_datums`、`build_join_key_string_datums`：分别生成 `0..count` 的整数值和其十进制字符串值；输出唯一且确定，不使用随机数。
- `build_left_and_right_data_source`、`mock_data_source`：构造两列 `(Int(index), Text("row-{index}"))` 测试行并按 `chunk_size` 切块；右侧由前者指定为逆序。
- `build_schema(column_count) -> Vec<usize>`：返回连续列下标，而不是生产表达式 schema。
- `execute_hash_join_exec`：收集每次 `next` 返回的非空行块；空块表示耗尽；同时识别 `HashJoinWorkerResult.error` 和 `next` 的外层错误。
- `execute_hash_join_exec_and_get_error`：丢弃成功值，仅暴露错误字符串。
- `execute_hash_join_exec_for_random_fail_test`：按已产出 chunk 数触发 `context.base.cancel()`；“random”来自 Go 测试用途命名，Rust 实现由 `fail_after_chunks` 明确控制，并非随机选择失败点。
- `get_sorted_results`、`check_results`：前者执行并排序，后者逐行比较长度和内容，返回带索引的描述性错误。

## 执行流程

1. 测试创建 `HashJoinInfo`。`hash_join_v1_test.rs::info` 和 jointest 中的 `hash_join_info` 是两个现有构造入口。
2. `build_hash_join_v1_exec` 先运行 `HashJoinInfo::validate`，再将执行参数传给 `HashJoinCtxV1`，将行拼接参数传给 `Joiner::new`，最后把上下文、joiner 及两侧 chunk 交给 `HashJoinV1Exec::new`。后两层仍会执行更完整的合法性检查。
3. `execute_hash_join_exec` 显式 `open`。根据 `hash_join_v1.rs`，`open` 重置共享上下文和游标、构建哈希表，并把状态置为 `Open`。
4. 辅助函数循环调用 `next`。v1 首次拉取时完成 probe，把输出按 `max_chunk_size` 分批返回；返回空行块代表 `Exhausted`。辅助函数保存每个非空 `rows` 块。
5. 无论循环因空结果、结果内错误或 `next` 错误退出，函数都会调用 `close`，随后返回之前保存的成功结果或原错误。`close` 会取消共享上下文、关闭并移除哈希表、清空输出并设置 `Closed`。
6. 若调用 `get_sorted_results`，所有 chunk 被展平成行并按统一值顺序排序；调用方随后可用 `check_results` 或普通断言比较期望结果。

取消测试流程略有不同：`execute_hash_join_exec_for_random_fail_test` 在每次拉取前检查已成功获得的 chunk 数，达到阈值便调用 `cancel`；下一次执行器操作观察取消并返回错误，辅助函数仍在返回前关闭执行器。`hash_join_test_util_test.rs::execution_helpers_close_executor_like_go` 以阈值 `Some(0)` 验证立即取消得到 `"hash join cancelled"`，且正常和取消路径最终状态都为 `Closed`。

## 数据与状态

`Row` 是 `Vec<Value>`，`Chunk` 是行集合；辅助层对输入使用克隆语义，因此构造出的执行器拥有独立的 build/probe 数据以及默认行、谓词和投影配置。`HashJoinInfo` 本身不维护执行状态。

排序的类型秩固定为 `Null < Bool < Int < UInt < Float < Bytes < Text`；同类型再比较值，其中浮点使用 `total_cmp`，所以包括 NaN 在内也有全序。行先按对应列逐一比较，公共前缀相等时短行排在长行前。由于异类型先比较类型秩，`compare_value` 的异类型兜底 `Equal` 在当前控制流中不会被使用。

`mock_data_source` 的 `row_count == 0` 会返回空 chunk 列表；`reverse` 只改变行顺序，不改变值；`chunk_size` 决定切块边界但不填充尾块。`build_join_key_*` 的 `count` 只影响连续序列长度。执行期间的哈希表、输出缓冲、游标和 `ExecutorState` 均属于 `HashJoinV1Exec`，不在本文件中复制状态。

## 依赖与调用关系

上游调用关系：

- `hash_join_test_util_test.rs` 调用构造和两种执行辅助函数，验证关闭与取消契约。
- `hash_join_v1_test.rs` 使用 `HashJoinInfo`、`build_hash_join_v1_exec` 和比较器，覆盖 inner/outer 行语义、outer build 补发、重复打开和运行时统计。
- `pkg/executor/test/jointest/hashjoin/hash_join_test.rs` 用同一 `HashJoinInfo` 分别构造 v1 与 v2，排序后比较两个实现；这里证明该参数包也是跨实现语义对照的桥梁。

下游调用关系：

- `build_hash_join_v1_exec -> HashJoinInfo::validate -> Joiner::new -> HashJoinV1Exec::new`。
- `execute_hash_join_exec -> HashJoinV1Exec::{open,next,close}`；`execute_hash_join_exec_and_get_error` 和 `get_sorted_results` 再包装它。
- `execute_hash_join_exec_for_random_fail_test -> HashJoinContextBase::cancel + HashJoinV1Exec::{open,next,close}`。
- `sort_rows -> generate_cmp_func -> compare_value`；`check_results -> compare_rows -> generate_cmp_func`。
- 数据源构造只依赖 `row_table_builder::{Chunk, Value}`；schema 构造只生成 `usize` 下标。

这些依赖均为 crate 内模块引用或 Rust 标准库 `std::cmp::Ordering`。目标文件没有直接使用 `Cargo.toml` 中列出的外部 crate；它仍随 `astersql-executor-join` 统一编译。

## 错误处理与边界

构建时有分层校验。`HashJoinInfo::validate` 拒绝键数量不等和零并发/零 chunk 大小；`Joiner::new` 还拒绝零 chunk 大小及非 anti join 的 null-aware 模式；`HashJoinV1Exec::new` 通过 `HashJoinCtxV1::validate` 拒绝空键、非法 null-aware 组合、旧式 outer-build 的 full outer 配置，并核对 context 与 joiner 的连接类型。因此不能把第一层通过视为完整合法。

`mock_data_source` 对 `chunk_size == 0` 使用 `assert!`，会 panic；这与返回 `Result` 的执行器装配函数不同。测试若要检查无效 chunk 大小，应明确使用 panic 断言，或优先经 `HashJoinInfo` 构建路径验证返回错误。

`execute_hash_join_exec` 在 `open` 失败时直接返回，因执行器尚未成功打开而不会调用 `close`；一旦 `open` 成功，循环中的所有退出分支都会先关闭。`close` 本身无返回值，所以没有“关闭错误覆盖执行错误”的问题。函数保存并返回原始字符串错误，不添加上下文。

`check_results` 先检查行数，再报告第一个不同的行索引以及实际/期望调试值；它依赖排序后的输入才能忽略输出顺序。比较器仅按本地 `Value` 枚举排序，不考虑 SQL collation、字符集、decimal、时间类型或类型强制转换，不能替代生产 SQL 值比较。

## 并发与资源生命周期

`HashJoinInfo.concurrency` 会进入 `HashJoinCtxV1`，但辅助函数本身不创建线程、锁或通道；实际 worker/build-probe 协调由执行器及其 `HashJoinContextBase` 承担。结果排序是执行完成后的单线程原地排序，用来消除并发输出顺序差异，不声明任何生产顺序保证。

资源生命周期由辅助执行函数显式包围。成功 `open` 后，正常耗尽、worker 结果携带错误、`next` 返回错误和测试主动取消都会到达 `close`。独立回归测试检查了最终 `ExecutorState::Closed`。`close` 会向共享上下文发出取消、释放哈希表和输出；重复 `close` 的具体幂等性属于 `HashJoinV1Exec`，本辅助层不额外保护。

本文件没有异步任务所有权、文件句柄或临时目录。输入和配置被克隆到执行器，因此调用者修改原 `HashJoinInfo` 不会改变已构造实例；代价是测试数据、谓词和投影列表的额外内存复制。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/join/hash_join_test_util.go`。两者保留了相同的辅助角色和主要命名族：参数包、执行器构造、比较/排序、键数据与数据源构造、执行/取错/随机失败、排序结果与结果检查。

当前 Rust 并非逐类型等价复刻：

- Go `hashJoinInfo` 携带 session context、schema、plan ID、真实子执行器、表达式列/类型与 v2 特有字段；Rust `HashJoinInfo` 使用简化的本地 `Row`/`Chunk`、列下标和 `Predicate`，主要装配 v1，也供 jointest 构造 v2。
- Go `buildHashJoinV2Exec` 创建 v2 worker、分区和类型/字符集信息；Rust 本文件构造的是 `HashJoinV1Exec`，v2 对照在 jointest 的 `execute_hash_join_v2` 中单独完成。
- Go 比较器由 `FieldType` 选择 `chunk.CompareFunc`；Rust 比较器按 `Value` 变体秩与值排序，未实现 SQL 类型和 collation 语义。
- Go 键数据和 mock 数据源包含随机唯一值、NDV、选择向量及 50,000 行规模；Rust 使用连续确定值和简单两列行，适合轻量单元测试。
- Go 执行辅助用 `testing.T` 立即断言错误，并操作真实 `chunk.Chunk`；Rust 返回 `Result`/`Option`，把断言策略交给调用者。两版都在完整拉取后关闭执行器，Rust 另有独立测试显式固定这一契约。
- Rust 的 `fail_after_chunks` 提供确定性取消注入；Go 的同名 random-fail 辅助本身只持续拉取，失败通常由外围 failpoint 注入。

因此扩展时应保持“意图对齐”，但不能假设字段或比较语义已经覆盖 Go 的全部生产能力。

## 扩展指南

- 新增 v1 构造参数时，先把字段加入 `HashJoinInfo`，再在 `build_hash_join_v1_exec` 中明确接入 `HashJoinCtxV1`、`Joiner` 或执行器构造器；同步更新 `hash_join_test_util_test.rs`、`hash_join_v1_test.rs` 和 jointest 中的结构体字面量。避免只添加字段而未下传。
- 扩展无效配置时，应区分本文件的快速校验与 `HashJoinCtxV1`/`Joiner` 的权威校验；回归测试放在独立的 `*_test.rs`，不要内嵌到源文件。
- 新增 `Value` 变体必须同时更新 `compare_value` 的类型秩和同类型比较，否则排序会遗漏或无法编译；若目标是 SQL 级排序，应引入显式类型/collation 上下文，而不是继续扩大当前测试专用秩规则。
- 改动执行辅助时，必须保持成功打开后的所有退出路径调用 `close`，并为正常、错误、取消至少各保留一个状态断言。若未来 `close` 可失败，需要定义原执行错误与关闭错误的优先级。
- 扩充数据生成器时保持确定性，除非测试明确控制随机种子；大数据或 NDV/selection 行为应对照 Go 工具另设专用构造器，避免让轻量单元测试默认承担昂贵数据量。
- 若新增 v2 通用装配能力，应先检查 jointest 现有 `execute_hash_join_v2`，避免在此建立与 v1 字段含义不一致的第二套路径。兼容风险集中在 outer/null-aware 方向、列投影、默认内表行布局和 SQL 类型比较；性能风险集中在深克隆大 chunk 与对全部输出排序。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件；`files --filter pkg/executor/join/hash_join_test_util.rs` 确认目标含 23 个符号。
- RustCodeGraph 文件节点 `hash_join_test_util.rs:1-267`：确认所有类型、函数、私有比较器及 3 个直接使用文件；精确 `callers/callees` 查询未返回额外可用边，因此调用关系又以直接引用和下游源码核验。
- RustCodeGraph 文件节点 `hash_join_v1.rs:1465-1964`：核验 `HashJoinCtxV1::validate`、`HashJoinV1Exec::{new,open,next,close}` 的状态、错误和资源生命周期。
- RustCodeGraph 文件节点 `joiner.rs:1340-1439`：核验 `Joiner::new` 的 chunk/null-aware 校验及匹配职责。
- `pkg/executor/join/Cargo.toml`、`pkg/executor/join/lib.rs`：核验 crate 名称、crate 根、Go 包映射、公开模块和测试模块边界；`pkg/executor` 下没有 `doc.go`。
- `pkg/executor/join/hash_join_test_util.go`：核验 Go 对照的参数、v2 构造、类型感知比较、数据生成与执行辅助语义。
- `pkg/executor/join/hash_join_test_util_test.rs`：核验正常/取消执行均关闭、阈值零触发取消错误。
- `pkg/executor/join/hash_join_v1_test.rs`：核验 v1 helper 的直接使用、inner/outer 行语义、重复打开与统计场景。
- `pkg/executor/test/jointest/hashjoin/hash_join_test.rs`：核验同一参数包用于 v1/v2 结果对照。
- 本任务只新增说明文档，未运行 Cargo；交付前以任务指定命令验证恰有 11 个固定二级章节，并人工复核文件定位、执行链、边界和安全扩展入口均有上述源码证据。
