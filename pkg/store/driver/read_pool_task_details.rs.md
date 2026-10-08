# `pkg/store/driver/read_pool_task_details.rs`

## 文件定位

[对应源码](read_pool_task_details.rs) 位于 `astersql-store-driver` crate 内，是 TiKV 客户端/本仓库 Coprocessor 读池统计到规范 KV 接口类型之间的边界适配器。crate 由 `pkg/store/driver/Cargo.toml` 定义，模块在 `pkg/store/driver/lib.rs` 中以 `mod read_pool_task_details` 装配；其中 `read_pool_task_details` 被公开再导出，供 session 层处理点读统计，`cop_read_pool_task_details` 保持 crate 内模块路径调用，供 `kv_adapter.rs` 转换 Coprocessor 响应。

它不负责采集、聚合、格式化或合并统计，只把已有聚合从两种来源类型完整搬运为 `astersql_kv::kv::PoolTaskDetails`。因此它处在“客户端/传输实现类型”与“SQL 执行层可消费的规范 KV 类型”之间，避免上层直接依赖 `tikv-client` 或 `astersql-store-copr` 的内部数据结构。

## 核心职责

- `read_pool_task_details` 接收 `tikv_client::PoolTaskDetails`，用于点读等直接来自 client-rust 的统计；`pkg/session/runtime/scan_adapter_runtime.rs` 的 `OnFinishStatement` 和 `pkg/session/runtime/explain_read.rs` 是已核实的上游调用点。
- `cop_read_pool_task_details` 接收 `astersql_store_copr::pool_task_details::PoolTaskDetails`，用于 Coprocessor 响应；`pkg/store/driver/kv_adapter.rs` 的 `impl From<copr::CopResponse> for CopResultSubset` 是已核实的直接调用点。
- 两个入口都把 `task_count == 0` 视为无统计，返回 `None`；否则返回 `Some(astersql_kv::kv::PoolTaskDetails)`。
- 对非空输入逐字段保留 27 个聚合字段，包含任务/轮询/调度计数，任务墙钟时间，队列与唤醒等待，公平队列切片，以及轮询 CPU/墙钟时间的总量和极值。这里不重新计算平均值，也不重新解释最小值的样本口径。

## 主要符号

### `pub fn read_pool_task_details(details: &tikv_client::PoolTaskDetails) -> Option<PoolTaskDetails>`

公开的 client-rust 转换入口。参数是共享借用，函数不消费、不修改来源聚合；输出拥有全部字段的规范 KV 值。`pkg/store/driver/lib.rs` 通过 `pub use read_pool_task_details::read_pool_task_details` 将它暴露到 crate 根。

### `pub fn cop_read_pool_task_details(details: &astersql_store_copr::pool_task_details::PoolTaskDetails) -> Option<PoolTaskDetails>`

Coprocessor 内部聚合的转换入口。该函数本身声明为 `pub`，但没有从 crate 根再导出；当前直接使用者通过 `crate::read_pool_task_details::cop_read_pool_task_details` 调用，因此其有效用途仍是 driver crate 内部接线。

### `astersql_kv::kv::PoolTaskDetails`

两个函数共同构造的目标类型。其真实定义位于 `pkg/util/execdetails/internal/group1/lib.rs`，由 `pkg/kv` 的规范接口路径暴露。字段使用 Go 风格名称以保持迁移接口一致；其 `Empty`、`Merge`、`String` 等行为不在本文件实现。

本文件没有模块常量、trait、impl、宏或条件编译项，也没有本地可变辅助状态。

## 执行流程

直接 client-rust 路径如下：

1. session 在语句结束或 EXPLAIN 读取完成时，从 client-rust 统计对象取得 `tikv_client::PoolTaskDetails`。
2. 调用 crate 根再导出的 `astersql_store_driver::read_pool_task_details`。
3. 函数先检查 `details.task_count`。为零则返回 `None`，使上层合并逻辑忽略空聚合。
4. 非零时构造规范 `PoolTaskDetails`，所有字段均按同名语义直接复制。
5. session 将返回的 `Option` 以借用形式交给 `SyncExecDetails::MergeReadPoolTaskDetails`；后者承担跨请求合并和锁保护。

Coprocessor 路径如下：

1. `CopResultSubset::from(copr::CopResponse)` 优先读取 `response.detail.read_pool_task_details`，不存在时再读取协议响应中的 `response.read_pool_task_details`。
2. 选中的聚合经 `Option::and_then(cop_read_pool_task_details)` 转换；来源缺失或 `task_count == 0` 都形成 `None`。
3. 转换结果保存在 `CopResultSubset.read_pool`，由 `kv::ResultSubset::ReadPoolTaskDetails` 克隆返回。
4. executor/session 的消费方把它合并到同步执行明细，最终可进入慢日志、系统表或 EXPLAIN 诊断输出。

两个转换函数都不执行聚合：总量、最大值、最小值和样本数必须已由来源层按正确口径计算。

## 数据与状态

目标值包含以下语义组：

- 任务基数：`TaskCount`，也是空值判定的权威字段。
- 轮询与调度：`PollCount`/`MaxPollCount`/`MinPollCount` 和 `DispatchCount`/`MaxDispatchCount`/`MinDispatchCount`。
- 任务墙钟：`TotalWallTime`、`TaskWallTimeSampleCount`、`MaxTaskWallTime`、`MinTaskWallTime`。样本数独立存在，不能用任务数替代。
- 等待时间：queue wait 与 wake wait 各保留 total/max/min。
- 公平队列：`FairQueueSampleCount` 及 waited task slices 的 total/max/min；样本数也承担是否启用/是否存在公平队列样本的语义。
- 轮询耗时：CPU time 与 wall time 各保留 total/max/min，平均值由后续展示层根据 `PollCount` 计算。

所有时间字段沿用来源对象的 `std::time::Duration`，没有单位转换或精度损失。构造完成后结果与输入不共享内部可变状态；输入仅在调用期间被借用。

重要不变量是：当且仅当 `task_count != 0` 时本文件产生值。对于非空聚合，本文件不校验各样本数、总量与极值之间是否一致，也不修正零最小值；来源层必须保证这些关系。

## 依赖与调用关系

上游调用关系：

- `pkg/session/runtime/scan_adapter_runtime.rs::OnFinishStatement`：取得点读统计，仅在一次语句收尾中合并一次，并调用公开的 `read_pool_task_details`。
- `pkg/session/runtime/explain_read.rs`：EXPLAIN 读取路径取得 client-rust 统计后调用同一公开入口。
- `pkg/store/driver/kv_adapter.rs::CopResultSubset::from`：从标准 Coprocessor 响应的两个可能位置选取统计，并调用 `cop_read_pool_task_details`。

下游依赖关系：

- `tikv-client`：`Cargo.toml` 固定到 AsterSQL 上游仓库 tag `v0.4.2-aster.10`，提供直接客户端来源类型。
- `astersql-store-copr`：同工作区路径依赖，提供 Coprocessor 聚合来源类型；其聚合和 protobuf 合并逻辑位于 `pkg/store/copr/pool_task_details.rs`。
- `astersql-kv`：同工作区路径依赖，提供 driver 向 session/executor 暴露的规范 `PoolTaskDetails`。

RustCodeGraph 对目标文件给出的文件级使用者是 `pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/store/driver/kv_adapter.rs` 和装配文件 `pkg/store/driver/lib.rs`；精确文本搜索还确认 `pkg/session/runtime/explain_read.rs` 通过 crate 根再导出入口调用。图的函数级 callers 查询未返回调用边，因此调用点以这些源码位置补证。

## 错误处理与边界

本文件不返回 `Result`、不分配外部资源，也没有会主动产生错误的操作。唯一显式边界分支是 `task_count == 0`：两个入口均返回 `None`，与规范类型的 `Empty()` 语义和执行明细合并层“忽略空聚合”的行为一致。

来源本身为 `None` 的处理发生在调用方：Coprocessor 路径用 `Option::and_then` 跳过转换；直接客户端路径由调用方先取得一个具体统计值，再由本文件判断是否为空。

本文件刻意不处理以下异常组合：非零任务数但样本数为零、最小值大于最大值、总量与样本不匹配、超大计数或时长。逐字段复制能保留诊断事实，但也意味着来源层的错误会原样进入后续日志。新增校验若改变丢弃或修正策略，会成为兼容性行为变化，不应悄然加入此适配层。

## 并发与资源生命周期

两个函数都是只读、无锁、无全局状态的同步纯转换；相同输入产生相同输出，可由多个线程并发调用。函数只在栈上读取字段并构造拥有所有权的结果，不启动任务、不持有通道、不打开连接，也不延长来源引用生命周期。

并发聚合不属于本文件职责。点读统计的“一次性收尾”由 `scan_adapter_runtime.rs` 的 `point_read_pool_merged` 门控；跨请求执行明细由 `pkg/util/execdetails/execdetails.rs::MergeReadPoolTaskDetails` 在互斥保护下合并。Coprocessor 响应的 stream 关闭、重试和 transport 生命周期也由 `kv_adapter.rs` 及 coprocessor 层负责。

性能上每次转换是固定字段数的 O(1) 工作，没有堆集合遍历；保留直接字段复制可避免在热读路径增加额外聚合成本。

## 与 Go 版本的对应关系

仓库中没有 `pkg/store/driver/read_pool_task_details.go` 的一对一 Go 文件。该 Rust 文件是为类型分层新增的显式桥接：Go 主链可直接使用 client-go 的 `util.PoolTaskDetails`，而 Rust 同时存在 client-rust、`astersql-store-copr` 与规范 KV 三种类型，必须在 driver 边界转换。

语义对应由以下 Go 证据核对：

- `pkg/util/execdetails/execdetails.go::SyncExecDetails.MergeReadPoolTaskDetails` 把空聚合视为无操作，并在锁内克隆或合并非空统计；Rust 转换返回 `None` 后由对应 Rust 合并函数执行同一空值语义。
- `pkg/executor/slow_query_sql_test.go::TestReadPoolTaskDetailsInDiagnostics` 注入 protobuf 读池统计，并断言点读、批量点读和 Coprocessor 等诊断路径输出完整字段。其期望字符串覆盖 poll、dispatch、wall、queue、wake、fair queue、poll CPU 和 poll wall 各组数据。
- `pkg/store/copr/pool_task_details.rs::merge_from_pb` 对 protobuf 单任务明细进行聚合，承担计数、样本和极值计算；本文件接收其结果后不得重复聚合。

Rust 相比 Go 的可见差异是用 `Option<PoolTaskDetails>` 表达 Go 的 nil/empty 边界，并显式保留两套来源类型的转换函数。字段映射和空聚合语义应继续与 Go 保持一致。

## 扩展指南

当上游读池统计新增字段时，应按以下顺序扩展：

1. 先确认 client-rust 类型、`astersql-store-copr::pool_task_details::PoolTaskDetails` 与规范 `astersql_kv::kv::PoolTaskDetails` 的字段和单位已经对齐。
2. 同时更新 `read_pool_task_details` 与 `cop_read_pool_task_details`，防止点读和 Coprocessor 两条路径产生不同诊断能力。
3. 在独立测试 `pkg/store/driver/read_pool_task_details_test.rs` 的 `sample` 中加入非默认值，并扩展完整输出断言；不要把测试逻辑嵌入生产源文件。
4. 同步检查 `pkg/store/driver/coprocessor_adapter_test.rs::canonical_dag_read_pool_diagnostics_survive_region_retry`，确保重试和规范响应边界仍保留新字段。
5. 若字段影响聚合或展示，分别同步 `pkg/store/copr/pool_task_details_test.rs`、`pkg/util/execdetails/execdetails_test.rs` 以及 Go 侧诊断回归。

若要改变“空”的定义，应同时审查两转换函数、规范类型的 `Empty`、`SyncExecDetails::MergeReadPoolTaskDetails` 和所有 `Option` 消费方。风险包括：错误丢弃零任务但带其他数据的聚合、平均值分母改变、最小值零哨兵处理错误，以及慢日志格式兼容性变化。

## 验证依据

- 目标源码：`pkg/store/driver/read_pool_task_details.rs`，核实两个函数、`task_count == 0` 分支及完整逐字段映射。
- crate 边界：`pkg/store/driver/Cargo.toml` 和 `pkg/store/driver/lib.rs`，核实 crate 名、三项核心依赖、模块可见性与公开再导出。
- 调用点：`pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/session/runtime/explain_read.rs`、`pkg/store/driver/kv_adapter.rs`。
- 规范类型与合并：`pkg/util/execdetails/internal/group1/lib.rs`、`pkg/util/execdetails/execdetails.rs`。
- Coprocessor 来源聚合：`pkg/store/copr/pool_task_details.rs`。
- 独立 Rust 测试：`pkg/store/driver/read_pool_task_details_test.rs`、`pkg/store/driver/coprocessor_adapter_test.rs`、`pkg/store/copr/pool_task_details_test.rs`、`pkg/util/execdetails/execdetails_test.rs`。
- Go 对照：`pkg/util/execdetails/execdetails.go` 和 `pkg/executor/slow_query_sql_test.go`；未发现同路径 Go 转换文件，因此没有把 Rust 桥接误写成逐函数机械翻译。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/store/driver/read_pool_task_details.rs` 核实源码及文件级使用者；`query` 核实两个函数；`callees` 指向读池统计结构，函数级 `callers` 无返回，故用精确源码搜索补齐调用证据。
- 任务为纯文档分析，按计划不运行 Cargo；验收以固定章节结构检查和上述人工事实复核为准。
