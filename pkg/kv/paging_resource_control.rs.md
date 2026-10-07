# `pkg/kv/paging_resource_control.rs`

## 文件定位

本文件属于 `astersql-kv` crate（`pkg/kv/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/kv/lib.rs` 以公开模块 `paging_resource_control` 接入。它在 KV coprocessor 的 `CopRUInterceptor` 边界实现“分页读取预估值的请求前 RU 预扣 + 响应后结算”，供存储驱动把准入等待放在真正发送网络请求之前。

该模块只维护某一个资源组的本地令牌桶和观测指标。文件头注释以及 `PagingRUInterceptor::reconfigure` 的接口表明，令牌设置和 grant 来自资源组所有者；它不实现 PD 资源组控制器或分配循环。

## 核心职责

- `PagingRUInterceptor` 校验请求资源组，并按 `0.125 + 0.5 * 0.7 + predicted_read_bytes / 65536` 计算请求侧 read RU 预扣量（`request`）。
- `wait` 在令牌足够、可在 `max_wait` 内补足、收到新 grant、取消或重试耗尽之间协调准入；成功预留时立即把令牌扣为可能的负数。
- `OnResponseWait` 用实际 MVCC 读取字节和 KV CPU 时间结算预估误差：`(actual - predicted) / 65536 + kv_cpu_ms / 3`。
- `PagingMetrics` 按资源组和 keyspace 记录是否预扣、预估/实际字节以及带符号的预测残差。

## 主要符号

- `PagingTokenConfig`：公开配置/授予载体。`fill_rate` 是每秒补充量；`burst < 0` 表示无限制，`burst == 0` 表示容量不封顶，`burst > 0` 是补充后的容量上限；`tokens` 是初始令牌或重配置时追加的 grant；其余字段限定单次等待和失败预留的重试策略。
- `Bucket`：受互斥锁保护的内部状态，保存当前配置、上次结算时刻、令牌余额和 `generation`。`generation` 只用于识别 `reconfigure` 是否发生。
- `PagingRUInterceptor`：公开拦截器，持有资源组名、`Mutex<Bucket>`、唤醒重试者的 `Condvar`、共享指标句柄以及无锁读取的 `AtomicBool throttled`。
- `new`、`reconfigure`、`set_throttled`、`available_tokens`：分别负责构造、应用 grant 并唤醒等待者、切换无提示大响应的限流状态、读取按当前时间补充后的余额。
- `refill`、`adjust`、`wait`、`request`：内部令牌算法。`adjust` 用于响应结算，允许余额形成债务；`wait` 用于尚未完成工作的准入，受等待上限、重试和取消约束。
- `CopRUInterceptor::{OnRequestWait, OnRequestWaitCancellable, OnResponseWait}`：KV 层公开 trait 的实现入口。可取消入口与普通入口最终都调用 `request`。
- `PagingMetrics::{new, observe_request, observe_response}`：通过全局 `OnceLock` 只注册一次 Prometheus 指标，并为每个实例克隆句柄。

## 执行流程

1. 资源组所有者用 `PagingRUInterceptor::new` 建立一个组的桶；后续 grant 通过 `reconfigure` 追加。重配置先按旧配置补充到当前时刻，再加 `config.tokens`，然后替换配置、递增代次并 `notify_all`。
2. 驱动适配层把 wire request 转成 `CopRPCRequestInfo`，调用 `OnRequestWaitCancellable`。`request` 先严格比较 `resource_group_name`，再计算固定请求成本、读请求固定成本和预测字节成本。
3. `wait` 在锁内计算当前可用令牌。无限配置直接放行；余额足够时立即预留；余额不足但按填充速率能在 `max_wait` 内补齐时，先记下负余额，再循环以至多 10ms 的片段等待到成熟时刻，以便及时检查取消或重配置通知。
4. 如果当前预留无法在上限内成熟，`wait` 不改余额，而是在 `retry_interval` 内等待 `generation` 变化；每次 grant 可提前唤醒重试。耗尽 `retry_times` 后返回节流错误。
5. 发送完成后，驱动用聚合后的真实 `read_bytes`、`kv_cpu_ms` 调用 `OnResponseWait`。已有正预测值时立即 `adjust`：低估形成债务，高估产生退款，完成的工作不会再次排队。
6. 没有预测提示时，仅当结算成本为正才处理。小于 4 MiB 的响应或当前未处于 throttled 状态可直接记债；否则走 `wait`，大响应可能被限流。
7. 请求成功预扣后记录请求指标；响应结算后记录实际值与残差。无预测提示的响应不进入 actual/residual 两项。

## 数据与状态

令牌余额使用 `f64`，可为负数。负数既可表示已预约、尚待时间补足的额度，也可表示已完成请求结算产生的债务。`last` 使用单调时钟 `Instant`，更新采用 `max(now)`，避免状态时间倒退；耗时计算使用 `saturating_duration_since`。

`burst` 的三态是关键不变量：负数绕过等待和结算扣款；零不限制容量但仍执行速率与余额逻辑；正数只在 `refill` 时封顶。`fill_rate == f64::MAX` 也被视为无限制。普通负填充率由 `refill` 钳为零，但 `wait` 只有正填充率才计算成熟时间。

指标标签固定为 `resource_group` 与 `keyspace_name`。`keyspace::GetKeyspaceNameBySettings()` 在观测发生时读取当前 keyspace；指标名由 `PagingMetrics::new` 固定注册，`paging_resource_control_test.rs::paging_metrics_export_exact_dashboard_series` 验证仪表盘依赖的精确名称和残差桶。

## 依赖与调用关系

上游接口定义在 `pkg/kv/lib.rs::resourcegroup`：`CopRPCRequestInfo`、`CopRPCResponseInfo`、`RUDetails` 和 `CopRUInterceptor`。模块直接依赖标准库同步/时间原语、`keyspace` crate 和 `prometheus`（后者在 `pkg/kv/Cargo.toml` 中声明）。

实际接线位于 `pkg/store/driver/runaway_adapter.rs::KVCopRUInterceptor`：`on_request_wait` 调用可取消请求入口，并把字符串取消错误映射为 `BatchError::Cancelled`；`on_response_wait` 汇总主响应和 batch 子响应的 read bytes、CPU、扫描键及数据长度，再调用本实现的响应入口。RustCodeGraph 还显示模块由 `pkg/kv/lib.rs`、独立单测和 `pkg/store/driver/coprocessor_adapter_test.rs` 使用。

下游调用边为：`request -> wait + PagingMetrics::observe_request`，`OnResponseWait -> adjust/wait + PagingMetrics::observe_response`，而 `adjust`、`available_tokens`、`reconfigure` 和 `wait` 都通过 `refill` 计算实时余额。

## 错误处理与边界

- 资源组不匹配返回 `resource group … is not configured`，不会扣令牌或记请求指标。
- 取消在预留前、预留成熟等待中和重试等待中均检查；成熟等待中取消会按最初预留时刻补回额度，返回固定字符串 `resource control cancelled`。适配层依赖这个精确字符串完成错误分类。
- 无法在 `max_wait` 内成熟且重试耗尽时返回 `resource group throttled: reservation exceeds maximum wait`；失败的预留不应改变余额。
- `retry_times == 0` 会直接走到节流错误；配置有效性没有在本文件中额外校验。`Duration::try_from_secs_f64` 失败也等价于本轮不可等待。
- 所有锁获取和指标注册失败均使用 `expect`/`unwrap`，即锁中毒、重复/非法指标注册属于进程级不变量破坏而不是可恢复业务错误。
- RU 结果允许为负：响应实际字节少于预测时，负 `read_ru` 表示退款，而非错误。

## 并发与资源生命周期

`PagingRUInterceptor` 可经 trait object 在并发请求间共享：令牌桶串行化在一个 `Mutex` 下，`Condvar` 只在同一把锁上等待，`throttled` 用 Release 写/Acquire 读。`reconfigure` 在持锁时递增代次并广播，使所有等待 grant 的线程重新评估新配置。

等待采用 10ms 上限的分段 `wait_timeout`，因此即使没有通知也能检查外部 `AtomicBool` 取消标志。该标志由调用者拥有，本模块只借用并读取。模块不创建后台线程、异步任务或网络资源；计时、唤醒和取消都发生在调用线程。`PagingMetrics` 的 collector 进程级存活，实例只克隆已注册句柄。

## 与 Go 版本的对应关系

仓库内不存在 `pkg/kv/paging_resource_control.go`，因此本文件不是同路径 Go 文件的逐函数翻译。可直接核验的 Go 主链位于 `pkg/store/copr/coprocessor.go`：`predictedReadBytesForTask` 从 EMA 给出提示，`handleTaskOnce` 将其写入 `tikvrpc.Request.PredictedReadBytes`；`pkg/store/copr/coprocessor_test.go::TestBuildCopTasksWithPagingSizeBytes` 验证 4 MiB 字节预算即使不启用行数分页也会保留预测提示。

Rust 侧由 `pkg/store/driver/runaway_adapter.rs` 把同一提示转成 KV trait 输入，并在真实发送前等待。取消退款旁的源码注释说明它对齐 Go/client-go `WaitReservations` 的“按原预约时刻取消”语义，但该外部实现不在本仓库此文件内；这里只把现有 Rust 源码、独立测试和适配测试所证明的行为视为已验证。PD 的 grant/资源组分配算法也明确在本模块范围外。

## 扩展指南

- 修改 RU 公式或 4 MiB 阈值时，首先改 `request`/`OnResponseWait`，并同步 `pkg/kv/paging_resource_control_test.rs` 中退款、债务和无提示阈值用例；公式变化还需评估与外部资源管理器单位的兼容性。
- 修改令牌语义时应集中在 `refill`、`wait`、`adjust`、`reconfigure`，保持“失败预留不扣款、已完成工作不二次排队、取消退还预约、grant 唤醒重试”四个不变量。测试继续放在独立的 `pkg/kv/paging_resource_control_test.rs`，不要嵌入生产文件。
- 修改取消错误文字前必须同步 `pkg/store/driver/runaway_adapter.rs` 的字符串映射；更稳妥的演进方向是跨 crate 的结构化错误，但这超出本文件当前契约。
- 新增响应字段或 batch 聚合成本时，需要同时修改 `CopRPCResponseInfo`、`KVCopRUInterceptor::on_response_wait` 和适配层测试，不能只改本模块公式。
- 修改指标名、标签或桶会影响监控兼容性；同步更新 `paging_metrics_export_exact_dashboard_series` 以及相关 dashboard/告警消费者。高基数资源组或 keyspace 标签还需评估内存开销。
- 若接入新的资源组所有者，复用 `reconfigure` 提交 grant，并由所有者决定何时 `set_throttled`；不要把 PD 分配循环复制进本模块。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；查询目标文件得到完整 326 行源码及三个直接使用文件。
- RustCodeGraph 关键调用边：`request -> wait/observe_request`；`OnResponseWait -> adjust/wait/observe_response`；`refill` 被 `adjust`、`available_tokens`、`reconfigure`、`wait` 调用。
- 源码与边界：`pkg/kv/paging_resource_control.rs`、`pkg/kv/lib.rs`、`pkg/kv/Cargo.toml`、`pkg/store/driver/runaway_adapter.rs`。
- Rust 独立测试：`pkg/kv/paging_resource_control_test.rs` 覆盖预扣结算、失败余额、取消退款、grant 唤醒、无提示阈值、无限/突发配置和指标；`pkg/store/driver/coprocessor_adapter_test.rs` 覆盖提示穿透、发送前等待、关闭取消及真实响应聚合。
- Go 对照：`pkg/store/copr/coprocessor.go::{predictedReadBytesForTask, handleTaskOnce}` 与 `pkg/store/copr/coprocessor_test.go::TestBuildCopTasksWithPagingSizeBytes`。仓库内未找到同路径 Go 控制器，此限制已在上一节明确记录。
- 本任务是纯文档分析，未运行 Cargo；交付结构检查要求本文恰含规定的 11 个二级标题。
