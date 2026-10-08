# `pkg/store/copr/pool_task_details.rs`

## 文件定位

本文件属于 `astersql-store-copr` crate。crate 入口 `pkg/store/copr/lib.rs` 将其声明为公开模块 `pool_task_details` 并整体再导出；`pkg/store/copr/Cargo.toml` 表明该 crate 对应 Go 包 `pkg/store/copr`，本文件直接依赖 `std::time::Duration` 和该 crate 的 `kvproto` 依赖。

它位于 TiKV coprocessor 响应的协议解析边界：`pkg/store/copr/network_backend.rs::pool_task_details` 从 `kvrpcpb::ExecDetailsV2.read_pool_task_details` 取出单次任务的 protobuf 统计，调用本文件的 `PoolTaskDetails::merge_from_pb` 转成 Rust 聚合值，再放入 `CopProtocolResponse.read_pool_task_details`。随后 `pkg/store/driver/read_pool_task_details.rs::cop_read_pool_task_details` 把它逐字段转换为上层统一的 `astersql_kv::PoolTaskDetails`。

## 核心职责

本文件只负责两件事：定义 read-pool 任务统计的内部聚合结构 `PoolTaskDetails`，以及把一条 `kvrpcpb::PoolTaskDetails` 样本合并进该聚合。它不发 RPC、不持有响应、不格式化慢日志，也不负责跨响应的加锁聚合。

聚合同时保存总量、最大值和最小值，并为“零值表示尚无样本”的字段维护显式或隐式样本存在性。关键规则见 `merge_from_pb`：每次合并都增加 `task_count`；wall、queue、wake 和 poll 耗时的最小值只在相应样本有效时更新；公平队列统计只在 protobuf 的 `fair_queue_enabled` 为真时纳入。

## 主要符号

- `pub struct PoolTaskDetails`：公开、可克隆、可比较且有零值默认值的聚合 DTO。字段分为任务/轮询/调度计数，任务 wall time，queue/wake wait，fair-queue waited slices，以及 poll CPU/wall time。所有 protobuf 纳秒字段在此转换成 `Duration`。
- `fn merge_min<T: Ord + Copy>(current, sample, had_samples) -> T`：文件内部的首样本辅助函数。已有样本时返回较小值；尚无样本时直接采用新值，避免默认零值错误地成为最小值。
- `pub fn PoolTaskDetails::merge_from_pb(&mut self, details: &kvrpcpb::PoolTaskDetails)`：唯一公开行为入口。它先根据合并前的累计状态捕获各类 `had_*` 标志，再按字段类别累加总量、更新极值和样本计数。

文件没有 trait、枚举、模块级常量或条件编译项。测试通过 `pkg/store/copr/lib.rs` 中独立的 `#[path = "pool_task_details_test.rs"]` 模块接入，未内嵌在生产源文件中。

## 执行流程

1. `network_backend::pool_task_details` 先检查 `ExecDetailsV2::has_read_pool_task_details()`；缺少该 protobuf 子消息时返回 `None`，本文件不会被调用。
2. `merge_from_pb` 在修改状态前记录是否已有 poll、queue、wake、fair-queue、task-wall 和 task 样本，保证本轮样本不会误判为历史样本。
3. 每条 protobuf 消息令 `task_count += 1`。`poll_count` 与 `dispatch_count` 分别累计，同时更新最大值；最小值以“此前是否已有任务”作为首样本判据，因此零计数也是任务级有效样本。
4. `total_wall_nanos` 总是加入 `total_wall_time`；只有非零 wall 样本才增加 `task_wall_time_sample_count` 并更新 wall 最大/最小值。
5. queue 和 wake 的 total 总是累加、max 总是与 protobuf max 比较；只有对应 total 非零时才用 protobuf min 更新累计 min。
6. 仅当 `fair_queue_enabled` 为真时，才把本任务的 `dispatch_count` 加入 `fair_queue_sample_count`，累加 waited slices 并更新其最大/最小值。
7. poll CPU/wall 的 total 与 max 总是更新；仅当本任务 `poll_count > 0` 时更新两类 min，历史样本存在性由合并前累计的 `self.poll_count > 0` 决定。
8. 生成的聚合随普通或批量 `CopProtocolResponse` 返回；`coprocessor.rs` 将其复制到 `CopRuntimeStats`，driver 再转换到 KV 层供执行详情、Explain 与慢日志链路继续聚合和展示。

## 数据与状态

`PoolTaskDetails` 是纯值类型，所有状态都由调用者拥有。`Default` 的全零状态表示尚未合并任何任务；真正的存在性由 `task_count` 判断，driver 的 `cop_read_pool_task_details` 会把 `task_count == 0` 转成 `None`。

字段的不变量和口径如下：

- `task_count` 是调用 `merge_from_pb` 的次数；它不是 protobuf 中读取的字段。
- `poll_count`、`dispatch_count` 及各类 total 是求和结果，max/min 是被接纳样本的极值。
- `task_wall_time_sample_count` 只统计非零 `total_wall_nanos`；`fair_queue_sample_count` 在公平队列启用时增加该任务的 `dispatch_count`，不是简单增加一。
- queue/wake 是否已有最小值由累计 total 是否非零推断；poll CPU/wall 是否已有最小值由累计 `poll_count` 是否非零推断。
- `Duration::from_nanos` 保留 protobuf 的纳秒单位，不做毫秒截断。

该结构不保存原始 protobuf，也不记录平均值；平均值由上层根据总量和对应计数计算。

## 依赖与调用关系

上游直接调用者是 `pkg/store/copr/network_backend.rs::pool_task_details`。该辅助函数分别用于 `pb_response` 的顶层响应和 `batch_responses` 子响应解析；因此本文件同时覆盖普通 coprocessor 响应与批响应中的 read-pool 诊断。

中间承载类型是 `pkg/store/copr/coprocessor.rs::CopProtocolResponse` 和 `CopRuntimeStats`，以及 `pkg/store/copr/batch_coprocessor.rs::CopRuntimeStats`。`coprocessor.rs` 在缓存结果、普通响应和批响应子任务路径中克隆该值，避免统计随原响应生命周期消失。

下游边界是 `pkg/store/driver/kv_adapter.rs::From<CopResponse> for CopResultSubset`：它优先从 runtime detail、否则从 protocol response 取统计，然后调用 `pkg/store/driver/read_pool_task_details.rs::cop_read_pool_task_details` 逐字段转换。规范 KV 类型定义在 `pkg/util/execdetails/internal/group1/lib.rs::PoolTaskDetails`，之后由 `SyncExecDetails::MergeReadPoolTaskDetails` 等上层逻辑继续合并。

直接外部依赖只有 `kvproto::kvrpcpb::PoolTaskDetails`；`Cargo.toml` 将 `kvproto` 固定到 tag `v0.0.2-aster.20260929` 并启用 `protobuf-codec`。协议字段变更必须同时核对该依赖版本和生成 getter。

## 错误处理与边界

本文件没有 `Result` 或显式错误分支。protobuf getter 对未设置的标量返回零值，因此缺字段会自然进入零样本规则；整个 `read_pool_task_details` 子消息是否存在，由上游 `network_backend::pool_task_details` 判定。

零值处理是主要边界：零 wall 不计入 wall 样本；零 queue/wake total 不更新相应 min；零 poll count 不更新 poll CPU/wall min；未启用公平队列时该组字段全部忽略。相反，poll/dispatch 的任务级最小值仍会接纳零计数，因为判据是此前是否已有任务。

实现使用普通 `u64`、`Duration` 加法，没有饱和或 checked 处理；它假设来自 TiKV 的计数和累计耗时在一次查询聚合周期内不会溢出。扩展时不能把默认零直接用于新字段的 min，必须同时定义该字段何时构成有效样本。

## 并发与资源生命周期

`PoolTaskDetails` 内部没有锁、原子、通道、任务或异步资源。`merge_from_pb` 需要 `&mut self`，Rust 借用规则保证单次合并期间独占该聚合；跨线程共享和同步必须由调用者负责。

`network_backend::pool_task_details` 为每个存在 read-pool 明细的响应创建一个临时默认聚合，合并一次后按值移入响应。后续路径使用 `Clone` 把统计附着到 `CopRuntimeStats`；跨多个响应的线程安全合并发生在上层执行详情容器中，不属于本文件职责。所有 `Duration` 和整数均为拥有型值，没有外部缓冲区或 protobuf 借用生命周期泄漏。

## 与 Go 版本的对应关系

同路径 Go 入口位于 `pkg/store/copr/coprocessor.go`：处理 `ExecDetailsV2` 时取得 `GetReadPoolTaskDetails()`，确保 `copStats.ReadPoolTaskDetails` 已分配后调用 client-go `util.PoolTaskDetails.MergeFromPB`。Rust 将这一外部聚合行为本地化为当前文件的 `PoolTaskDetails::merge_from_pb`，再由 `network_backend.rs` 接入响应解析。

字段集合与上层 Go 使用的 `util.PoolTaskDetails` 对齐：任务、poll、dispatch、task wall、queue/wake、fair queue、poll CPU/wall 的 total/max/min 均被保留。Rust 特有差异是使用 snake_case 字段和 `Option` 表示诊断缺失，并在 `pkg/store/driver/read_pool_task_details.rs` 显式转为 Go 风格命名的规范 KV 类型。

`pkg/store/copr/pool_task_details_test.rs::go_merge_48_pool_task_details_merge_samples_and_minima` 验证两个 protobuf 样本合并后的任务数、计数总量/极值、wall/queue 时间和 fair-queue waited slices，与 Go `MergeFromPB` 的聚合意图一致。当前仓库没有同路径独立 Go 单元测试专测该方法；Go 侧直接实现来自 client-go，仓库内的直接证据是 `coprocessor.go` 的调用接线。

## 扩展指南

新增 protobuf 统计字段时，应先明确它是每任务值、有效样本值还是总量，并在 `PoolTaskDetails` 与 `merge_from_pb` 同步增加字段。若需要 min，必须增加正确的历史样本判据；若零值表示“缺失”，不要把它纳入样本数或最小值。

协议到 SQL 可见诊断是一条逐字段链路，通常还需同步：`pkg/store/driver/read_pool_task_details.rs::cop_read_pool_task_details`、`pkg/util/execdetails/internal/group1/lib.rs::PoolTaskDetails` 的合并/展示逻辑，以及使用这些统计的 explain/slow-log 测试。遗漏 driver 转换会让字段在 copr 层存在却在 KV 边界静默丢失。

测试应继续放在独立的 `pkg/store/copr/pool_task_details_test.rs`，至少覆盖首样本、多个样本、零值不应更新 min、公平队列关闭、poll count 为零、wake/poll CPU/poll wall 等现有测试尚未直接断言的分支。还应保留 `pkg/store/driver/read_pool_task_details_test.rs` 的端到端字段保真断言。兼容风险主要是 protobuf 版本/字段口径漂移；性能风险较低，但该函数位于每个带执行详情响应的热路径，避免分配和格式化。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 `pkg/store/copr/pool_task_details.rs`；`query pool_task`、`query merge_from_pb` 和 `query PoolTaskDetails` 定位本文件 6 个索引符号、上游 `network_backend.rs::pool_task_details`、driver 转换和规范 KV 类型；`node --file pkg/store/copr/pool_task_details.rs --offset 1 --limit 200` 核对完整 148 行实现。图对方法未返回 callers/callees 边，因此又用精确引用检索补齐直接调用关系。
- 生产源码：`pkg/store/copr/pool_task_details.rs`、`pkg/store/copr/network_backend.rs`、`pkg/store/copr/coprocessor.rs`、`pkg/store/copr/batch_coprocessor.rs`、`pkg/store/driver/read_pool_task_details.rs`、`pkg/store/driver/kv_adapter.rs`、`pkg/util/execdetails/internal/group1/lib.rs`。
- crate/模块边界：`pkg/store/copr/Cargo.toml`、`pkg/store/copr/lib.rs`。
- Go 对照：`pkg/store/copr/coprocessor.go` 中 `GetReadPoolTaskDetails` 与 `MergeFromPB` 接线，以及 `pkg/util/execdetails/execdetails.go::MergeReadPoolTaskDetails` 的后续聚合入口。
- 独立测试：`pkg/store/copr/pool_task_details_test.rs` 验证 protobuf 样本合并；`pkg/store/driver/read_pool_task_details_test.rs` 验证 copr 响应跨 KV 边界后字段与格式保真。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前执行任务规定的 11 章节结构命令，并人工复核上述符号、边界和扩展链路。
