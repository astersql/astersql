# `pkg/store/driver/runaway_adapter.rs`

## 文件定位

[对应源码](runaway_adapter.rs) 是 `astersql-store-driver` crate 内部的 Coprocessor runaway/resource-control 类型桥。`pkg/store/driver/lib.rs` 以私有 `mod runaway_adapter` 装配它；唯一生产调用点位于 `pkg/store/driver/kv_adapter.rs::cop_request`，该调用点把 canonical `astersql_kv::Request` 中的共享检查器和 RU 拦截器转换为 `astersql_store_copr::CopRequest` 所需的 trait object。

文件不实现 runaway 判定、RU 计费算法、RPC、重试或任务调度。它只在 `astersql-kv` 的 Go 风格资源组接口与 `astersql-store-copr` 的执行接口之间转换请求/响应数据、动作枚举和错误。`pkg/store/driver/Cargo.toml` 对两者均使用工作区路径依赖，说明该边界属于 driver crate 内部接线，而不是对外公开 API。

## 核心职责

- `KVRunawayChecker` 将 `kv::resourcegroup::SharedRunawayChecker` 包装为 `Arc<dyn copr::RunawayChecker>`，转发发送前检查、响应阈值检查、累计 processed-keys 重置和当前动作查询。
- `KVCopRUInterceptor` 将 `kv::resourcegroup::SharedCopRUInterceptor` 包装为 `Arc<dyn copr::CopRUInterceptor>`，在网络发送前执行可取消的 RU 预扣，并在收到响应后传递真实消耗以完成结算。
- `request_info` 把当前 `CopTask` 和 `CopWireRequest` 压缩成 canonical `CopRPCRequestInfo`：资源组、请求类型、Region、store 地址、请求字节数、预测读取字节和低优先级标志。
- 响应结算会汇总主响应及全部 `batch_responses` 的 payload 字节、扫描键、MVCC read bytes 与 TiKV CPU 毫秒，并按固定优先级提取可报告错误。
- 适配器把底层检查器拒绝统一映射到 Coprocessor 错误类型，但不自行重试或吞掉拒绝。

## 主要符号

### `pub(crate) struct KVRunawayChecker`

只保存一个 `kv::resourcegroup::SharedRunawayChecker`。`new(inner) -> Arc<dyn copr::RunawayChecker>` 隐藏具体包装类型，使 `cop_request` 可直接写入 `copr::CopRequest.runaway_checker`。其 `Debug` 实现只输出类型名 `KVRunawayChecker`，不会要求或泄露内部动态对象的调试状态。

`copr::RunawayChecker` 实现包含四个行为入口：

- `before_cop_request`：构造精简的 canonical `CopRequest`，调用 `BeforeCopRequest`，成功后把允许修改的 priority、resource-group name 和最大执行时长写回 wire request。
- `check_thresholds`：转换可选 RU，保留 processed keys，把可选 `BatchError` 格式化为字符串后调用 `CheckThresholds`。
- `reset_total_processed_keys`：直接转发 `ResetTotalProcessedKeys`。
- `check_action`：逐项映射 `CoolDown`、`Kill`、`None`；canonical 接口没有 `DryRun`，因此本文件也没有该分支。

### `pub(crate) struct KVCopRUInterceptor`

只保存一个 `kv::resourcegroup::SharedCopRUInterceptor`。`new(inner) -> Arc<dyn copr::CopRUInterceptor>` 供 `cop_request` 安装到标准 Coprocessor 请求。其 `Debug` 同样只输出固定类型名。

### `fn request_info(task, wire) -> kv::resourcegroup::CopRPCRequestInfo`

私有、无状态的请求快照构造器。`request_type` 采用 `Debug` 字符串而不是共享枚举；`data_bytes` 是当前 wire payload 长度；`predicted_read_bytes` 用于分页 RU 预扣。Region ID 与 store 地址来自已路由的 `CopTask`，而不是原始 canonical 请求。

### `CopRUInterceptor::{on_request_wait,on_response_wait}`

`on_request_wait` 调用 canonical 的 `OnRequestWaitCancellable`，并传入 `wire.resource_control_cancel`；`on_response_wait` 先构造汇总后的 `CopRPCResponseInfo`，再调用 `OnResponseWait`。两个入口均把返回的 `RUDetails` 逐字段转换为 `copr::CopRUDetails`。

本文件没有模块常量、宏、条件编译项或测试模块。

## 执行流程

1. `pkg/store/driver/kv_adapter.rs::cop_request` 读取 `kv::Request.RunawayChecker` 与 `ResourceControlInterceptor`。存在时分别调用两个 `new` 构造适配对象；如果只有 runaway checker，则使用 `copr::ProductionCopRUInterceptor` 作为 RU 计算回退。
2. `copr::Store` 构建任务后会查询 `check_action`。动作是 `CoolDown` 时将普通并发降为 1，并禁用 small-task 并发。
3. 每次任务尝试发送前，worker 调用 `KVRunawayChecker::before_cop_request`。底层 checker 可把 wire request 降为低优先级、切换资源组或设置最大执行时长；底层拒绝会立即变为 `BatchError::QueryInterrupted`。
4. 仅当 wire 的资源组名非空时，worker 启用 resource-control interceptor。`on_request_wait` 在实际网络发送之前调用；适配器向底层传入 worker 的共享取消原子标志，使关闭 response/iterator 可以中止等待。
5. worker 把预扣返回的 RU 加入 `CopRequest.resource_control_ru` 的共享累计值。网络错误发生时，累计 RU 和格式化后的原始错误会交给 `check_thresholds`，之后才返回网络错误；若阈值检查拒绝，则查询中断错误优先返回。
6. 网络成功后，`on_response_wait` 汇总主响应及 store-batch 子响应数据并取得结算 RU；worker继续累加 RU，然后对主响应和每个子响应分别调用 `check_thresholds`，传递各自 processed keys 与错误。
7. 请求完成时，Coprocessor 层在 lite iterator 耗尽或最后一个并发 worker 析构时调用 `reset_total_processed_keys`；生命周期控制不在本适配文件内。

## 数据与状态

两个包装器唯一的长期状态都是 `Arc<dyn ...>`；它们不复制底层 checker/interceptor 的内部状态。`new` 返回新的外层 `Arc`，内部再持有调用方给出的共享 `Arc`，因此多个 worker 会观察同一份阈值累计、令牌桶或统计状态。

`before_cop_request` 的 canonical 临时值只包含三个可改写字段。写回时，priority 只有在底层把 `priority_low` 置为 `true` 时才强制设为 `Low`；底层不能借此把原本的低优先级提升回 normal。资源组名和最大执行时长则总是以返回值覆盖 wire 字段。

RU 值只包含 `read_ru` 与 `write_ru`，转换不改单位、不截断。请求信息中的 `data_bytes` 不包含协议额外开销。响应 `data_bytes`、`processed_keys`、`read_bytes` 和 `kv_cpu_ms` 都是主响应与所有批子响应之和；使用普通数值加法，文件没有显式饱和或溢出策略。

响应错误只保留一个字符串，优先级为 `region_error`，其次是非空 `other_error`，最后是存在锁信息时的固定字符串 `"locked"`。批子响应的错误不会并入这份 interceptor 结算信息；它们随后由 Coprocessor worker 分别传给 runaway checker。

## 依赖与调用关系

生产上游只有 `pkg/store/driver/kv_adapter.rs::cop_request`：它把本文件构造的 trait objects 放进 `copr::CopRequest`。该请求随后由 `impl kv::Client for TikvStore::Send` 交给 `copr::Store::get_client().send`。RustCodeGraph 的文件级关系也将 `kv_adapter.rs` 与独立测试 `coprocessor_adapter_test.rs` 列为目标文件的两个使用者。

下游依赖分为两侧：

- `astersql-kv`（`pkg/kv/lib.rs::resourcegroup`）定义 `CopRequest`、`CopRPCRequestInfo`、`CopRPCResponseInfo`、`RUDetails`、`RunawayAction` 以及两套共享 trait。
- `astersql-store-copr`（`pkg/store/copr/coprocessor.rs`）定义 wire/task/protocol response、`RunawayChecker`、`CopRUInterceptor`、`BatchError` 和实际调用顺序。

动态 trait 调用使 RustCodeGraph 的函数级 `callers`/`callees` 没有返回额外边；真实调用点由 `coprocessor.rs` 中的 `before_cop_request`、`on_request_wait`、`on_response_wait`、`check_thresholds`、`reset_total_processed_keys` 与 `check_action` 调用逐一核对。

## 错误处理与边界

`KVRunawayChecker::before_cop_request` 和 `check_thresholds` 丢弃 canonical checker 返回的具体字符串，统一映射为 `BatchError::QueryInterrupted`。这是查询控制语义边界：调用方能知道请求被中断，但不能从此路径取得底层诊断文本。传给底层 `CheckThresholds` 的原始 Coprocessor 错误则通过 `ToString` 保存语义文本。

`KVCopRUInterceptor::on_request_wait` 特判精确字符串 `"resource control cancelled"` 为 `BatchError::Cancelled`；其他字符串成为 `BatchError::OtherResponse(error)`。因此取消分类依赖字符串协议，修改 canonical interceptor 的错误文本时必须同步本适配器和测试。`on_response_wait` 的任何底层错误都映射为 `OtherResponse`，没有取消特判。

资源组名为空时，Coprocessor 层不会调用 interceptor。响应错误提取不会同时保留多个错误，也不会纳入 batch child 错误；这是当前接口的数据容量边界。`request_type` 使用调试格式，若枚举的 `Debug` 输出变化，下游观察到的字符串也会变化。

本文件自身没有锁获取、I/O 或 panic 分支；但它调用的底层动态对象可以等待、返回错误或维护内部状态。整数/浮点汇总没有检查溢出、NaN 或负 RU，这些值会原样传入 canonical 实现。

## 并发与资源生命周期

两套上下游 trait 都要求 `Send + Sync`，包装器通过 `Arc` 在 Coprocessor worker 间共享。适配器没有自己的 mutex、atomic、task 或 channel；并发正确性由底层 checker/interceptor 实现负责。`Debug` 不访问内部对象，避免日志格式化引入额外锁或 trait 约束。

资源控制等待的取消生命周期来自 `CopWireRequest.resource_control_cancel`：worker 将 iterator/response 的共享 finish 标志放入 wire，本文件借用该 `AtomicBool` 传给 `OnRequestWaitCancellable`。`precharge_close_cancels_before_network_send` 验证 Close 能在 500ms 内结束长等待、恢复令牌且不发出 RPC。

runaway processed-keys 的清理由 Coprocessor iterator 管理：并发路径使用 `RunawayWorkerCompletion` 的原子剩余计数，最后一个 worker 析构时重置；lite 路径在迭代耗尽时重置。本文件只转发重置操作，不能提前按单次 RPC 清空共享累计。

性能上，请求阶段会克隆资源组名和 store 地址，响应阶段会遍历全部 batch children 做 O(n) 汇总；没有复制 child payload 本身。新增字段时应避免在每次重试的热路径引入不必要的大对象克隆或额外锁。

## 与 Go 版本的对应关系

仓库没有同名 `pkg/store/driver/runaway_adapter.go`。Go 的 `pkg/kv/kv.go::Request` 直接持有 `resourcegroup.RunawayChecker`，`pkg/store/copr/coprocessor.go` 在发送前调用 `BeforeCopRequest`，在发送错误和成功响应统计后调用 `CheckThresholds`；`CoolDown` 同样影响 Coprocessor 并发。这是 `KVRunawayChecker` 所保持的主流程语义。

Rust 需要额外适配器，是因为 canonical `astersql-kv` 与独立 `astersql-store-copr` crate 使用不同的请求、响应、RU、动作和错误类型。Go 主链可把同一 interface 直接向下传递，不需要这层显式类型转换。Go 的 resource-control interceptor 由 `pkg/domain/runaway.go` 通过 `tikv.SetResourceControlInterceptor(control)` 安装到 client-go；Rust 则允许 `kv::Request.ResourceControlInterceptor` 随请求传入，并在本文件转换后由本仓库 Coprocessor worker执行预扣和结算。

对应关系不是逐行机械翻译：Rust 的 interceptor 汇总 store-batch 子响应、传递可取消等待标志，以及在 adapter 中按字符串区分取消错误，均应以当前 Rust 源码和独立测试为准。Go 对照提供调用时机与 runaway 行为基线，但不能证明 Rust 特有桥接字段已经自动同步。

## 扩展指南

- 增加 runaway 可改写字段时，应同步修改 `pkg/kv/lib.rs::resourcegroup::CopRequest`、本文件 `before_cop_request` 的双向映射、`copr::CopWireRequest` 以及 `pkg/store/driver/coprocessor_adapter_test.rs::kv_request_runaway_checker_reaches_cop_wire_and_response_thresholds`。需明确字段是单向收紧还是允许恢复，避免无意提升优先级或放宽超时。
- 增加 RU 请求上下文时，修改 `request_info` 并扩展独立 adapter 测试；检查每次重试都能得到正确 Region/store/prediction。修改 `request_type` 表示时要考虑字符串兼容。
- 增加响应计费字段时，同时处理主响应与全部 batch children，并扩展 `kv_adapter_preserves_mvcc_settlement_data`。要分别审查 payload bytes、MVCC read bytes、CPU、processed keys 的单位与溢出策略，不能把 payload 长度当成存储读取字节。
- 改变错误分类时，保持 region/other/locked 优先级和取消语义，增加独立测试覆盖多错误同时存在、child error 与精确取消文本；若可行，应优先演进为结构化错误，避免继续扩大字符串协议。
- 修改共享状态或 reset 时机时，需要同时审查 `RunawayWorkerCompletion`、lite iterator 完成路径、并发/重试行为与 `pkg/resourcegroup/runaway/checker_test.rs` 的累计阈值测试。

测试必须继续放在独立文件中。最直接的回归面是 `pkg/store/driver/coprocessor_adapter_test.rs`；底层 checker 逻辑属于 `pkg/resourcegroup/runaway/checker_test.rs`，底层分页 interceptor 逻辑属于 `pkg/kv/paging_resource_control_test.rs`。兼容风险集中在错误类型/文本、动作枚举和字段单位；性能风险集中在每次 RPC 重试的克隆、batch 汇总遍历及底层共享锁等待。

## 验证依据

- 目标源码：`pkg/store/driver/runaway_adapter.rs`，核实两个包装器、两个构造器、固定 `Debug`、四个 runaway 转发入口、请求快照及响应汇总。
- crate 装配：`pkg/store/driver/Cargo.toml`、`pkg/store/driver/lib.rs`，核实 crate 名、`astersql-kv`/`astersql-store-copr` 路径依赖和私有模块可见性。
- 生产调用链：`pkg/store/driver/kv_adapter.rs::cop_request` 与 `impl kv::Client for TikvStore::Send`；`pkg/store/copr/coprocessor.rs` 的 worker 请求流程、`RunawayWorkerCompletion` 和 `Store::send` 并发降级逻辑。
- canonical 契约：`pkg/kv/lib.rs::resourcegroup`，核实请求/响应信息、RU、动作、共享 trait 及 cancellable 默认实现。
- 独立 Rust 测试：`pkg/store/driver/coprocessor_adapter_test.rs` 中 `kv_request_runaway_checker_reaches_cop_wire_and_response_thresholds`、`production_cop_resource_control_ru_reaches_runaway_across_retry`、`kv_adapter_preserves_predicted_read_bytes`、`precharge_waits_before_real_kv_send`、`precharge_close_cancels_before_network_send`、`kv_adapter_preserves_mvcc_settlement_data`；底层语义另由 `pkg/resourcegroup/runaway/checker_test.rs` 与 `pkg/kv/paging_resource_control_test.rs` 覆盖。
- Go 对照：`pkg/kv/kv.go`、`pkg/store/copr/coprocessor.go`、`pkg/resourcegroup/checker.go`、`pkg/resourcegroup/runaway/checker.go`、`pkg/domain/runaway.go` 及相邻 Go 测试。未发现同路径 Go adapter，因此文档区分了直接接口传递与 Rust 类型桥接。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标文件 16 个符号；`node --file` 核实目标源码和相关 Rust 调用实现；`query` 定位两个结构体及关键 trait 方法；文件级关系给出 `kv_adapter.rs` 和 `coprocessor_adapter_test.rs`。函数级动态 trait 调用未产生额外 callers/callees 输出，故以精确源码引用补证。
- 本任务只新增文档，按总计划不运行 Cargo；完成验证使用固定 11 章节的结构检查，并人工复核上述调用链、边界和扩展点均有本地源码或独立测试依据。
