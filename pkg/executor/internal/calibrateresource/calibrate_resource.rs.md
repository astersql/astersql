# `pkg/executor/internal/calibrateresource/calibrate_resource.rs`

## 文件定位

`fetch_store_metrics`、`fetch_server_cpu_quota`、`get_values_from_metrics` 与四个 SQL 构造函数：以注入闭包/纯数据形式保留 metrics 获取和解析语义，便于脱离网络与 SQL 运行时测试。

## 核心职责

- `workload_costs` 与 `static_calibrate`：把工作负载基准、RU 单价、TiDB/TiKV/TiFlash CPU 配额换算为静态 RU 容量。
- `parse_duration`、`parse_calibrate_duration`、`parse_calibrate_duration_text`：解析字符串时长，补全起止时间，并限制校准窗口为至少 1 分钟、至多 24 小时加 1 分钟缓冲。
- `TimeSeriesValues::advance`、`dynamic_tidb_quota`、`dynamic_tiflash_quota`、`setup_quotas`：对齐历史 RU/CPU 时序，过滤低利用率点，去除两端异常值后估算动态容量。
- `fetch_store_metrics`、`fetch_server_cpu_quota`、`get_values_from_metrics` 与四个 SQL 构造函数：以注入闭包/纯数据形式保留 metrics 获取和解析语义，便于脱离网络与 SQL 运行时测试。
- `Executor::{next_static,next_dynamic}`：模拟 Go `Next` 的资源控制开关和只产出一次结果的状态机。

## 主要符号

- 常量 `VALUABLE_USAGE_THRESHOLD = 0.2`、`LOW_USAGE_THRESHOLD = 0.1`、`DISCARD_RATE = 0.1`、`MIN_DURATION`、`MAX_DURATION`：分别控制动态样本采用条件、双端裁剪比例和时间窗口边界。
- `WorkloadType`：支持 `None`、TPCC、三种 OLTP 和 TPCH10；`None` 只在静态校准时回落到 TPCC。
- `BaseResourceCost`：每个 TiKV CPU 的基准资源率，包含 TiDB/TiKV CPU 比、KV CPU 秒、读写字节和请求数。
- `RuConfig`：读请求、CPU 毫秒、读字节、写请求、写字节五类 RU 单价。
- `ServerInfo`：静态估算所需的实例类型与单机 CPU 核数；实现按类型计数，并取该类型第一个实例的 `cpu_cores` 乘实例数，以对齐 Go“读取首个可用 metrics 配额后乘实例数”的行为。
- `TimePointValue` 与 `TimeSeriesValues`：动态估算的采样点和有序游标；`new` 会先按 `SystemTime` 排序。
- `MetricsServer`、`MetricsResponse`：将状态地址、实例地址和 HTTP 响应简化为纯 Rust 数据；真实 I/O 由 `fetch_store_metrics` 的 `request` 闭包提供。
- `Executor { workload, enabled, done }`：精简执行器；`done` 私有且在首次调用开始时即置位。

## 执行流程

静态路径从 `Executor::next_static` 开始：先检查 `done`，首次调用将其置为 `true`，资源控制关闭则报错，否则调用 `static_calibrate`。TPCH10 只计算 TiFlash 的 CPU 与读字节成本；其他 workload 将 `None` 归一为 TPCC，要求至少一个 TiKV，按首个 TiKV 配额乘实例数得到总核数，再用 `min(TiKV 总核数, TiDB 总核数 / 工作负载 CPU 比)` 限制有效 KV 核数。五项 RU 成本之和乘有效核数后截断为 `u64`。

动态路径从 `Executor::next_dynamic` 开始，同样先执行单次产出和开关检查。`dynamic_tidb_quota` 将 RU、TiKV CPU、TiDB CPU 分别排序，并在循环中取三者当前最大时间作为目标；三条序列都能在严格小于 10 秒的窗口内对齐才计算利用率。TiKV 或 TiDB 任一利用率大于 0.2，或者两者都不低于 0.1，样本才以 `RU / max(TiKV 利用率, TiDB 利用率)` 纳入。`dynamic_tiflash_quota` 以相同时间对齐规则处理两条序列，只保留利用率大于 0.1 的点。两路都失败时返回 TiDB 路径错误；任一路成功时，失败一路按 0 处理并求和。

两种动态估算最终调用 `setup_quotas`：少于两个样本直接报低负载；否则降序排序，按 `round(len * 0.1)` 从首尾各丢弃相同数量，再对中间段求均值。

metrics 辅助流程中，四个 `get_*_query` 仅生成与 Go 相同的 `METRICS_SCHEMA` SQL。`get_values_from_metrics` 传播整次查询错误、忽略单行解析错误，并由 `TimeSeriesValues::new` 排序。`fetch_store_metrics` 过滤类型不符或无状态地址的实例，依次请求，记住首个请求错误；取得首个响应后立即交给回调并返回。`fetch_server_cpu_quota` 要求 HTTP 200，并从第一个以指定指标名开头且格式为“名称 + 单个空格 + 浮点数”的行读取值。

## 数据与状态

文件没有全局可变状态。基准成本表每次由 `workload_costs` 新建 `BTreeMap`；动态输入切片会被克隆进各自的 `TimeSeriesValues` 并排序，因此调用者原始数据不被修改。`TimeSeriesValues.index` 是消费游标，只有 `advance` 和 `next` 改变它。

`Executor.done` 是最重要的不变量：首次 `next_static`/`next_dynamic` 无论成功还是因开关、校准错误失败，都会先置为 `true`；随后调用返回 `Ok(None)`。这与 Go `Executor.Next` 在业务检查前设置 `done` 的顺序一致，但该类型不提供重置方法。

数值输出会从 `f64` 转为 `u64`。静态路径在转换前用 `.max(0.0)` 防止负成本产生负容量；动态合并直接使用 Rust 浮点到整数转换语义。CPU 核数必须为正：TiDB/TiKV 动态路径显式拒绝非正核数；TiFlash 非正核数通过空样本进入低负载错误。

## 依赖与调用关系

可编译实现只依赖标准库 `BTreeMap`、`Duration`、`SystemTime`。`Cargo.toml` 的 AsterSQL 依赖全部位于 `target.'cfg(windows)'.dependencies`，与文件上半部注释迁移草稿所列 domain、exec、infoschema、kv、parser、sessionctx、staleread、chunk、sqlexec 等完整 Go 接线相呼应，但当前活跃代码没有引用它们。

已验证的内部调用边包括：

- `Executor::next_static -> static_calibrate -> workload_costs`。
- `Executor::next_dynamic -> dynamic_tidb_quota/dynamic_tiflash_quota -> TimeSeriesValues::{new,advance,next} -> setup_quotas`。
- `parse_calibrate_duration_text -> parse_duration -> parse_calibrate_duration`。
- `fetch_server_cpu_quota -> fetch_store_metrics -> request/on_response` 注入闭包。

上游方面，`calibrate_resource_test.rs` 直接调用所有核心纯函数和两个 executor 入口；crate 由 `pkg/executor/Cargo.toml` 依赖并由 `pkg/lib.rs` facade 暴露。未找到 Rust 生产调用者。Go 主链则是 `pkg/executor/builder.go` 根据 `CalibrateResourceStmt` 构造 `calibrateresource.Executor`，再由执行框架调用 `calibrate_resource.go::Executor.Next`。

## 错误处理与边界

- 时间：缺少 start 与 duration、开始时间下溢、结束早于开始、窗口短于 1 分钟或长于 24 小时 1 分钟都会返回 `String` 错误。恰好 1 分钟有效；超过上限 60 秒仍有效，超过 61 秒失败。
- duration 文本：拒绝空串、负数、无单位、未知单位和非有限累计值；支持 `ns/us/µs/μs/ms/s/m/h` 以及多段组合。它是常见 Go duration 格式的局部实现，不应推断为完整替代 Go 标准解析器。
- 动态样本：不足两个有效配额报低负载；`setup_quotas` 对不可比较浮点值以 `Equal` 排序，不主动过滤 NaN。上游必须保证 metrics 数值有效。
- 时序对齐：边界是严格小于 10 秒；正好相差 10 秒不匹配。`get_time/get_value` 要求调用方先检查 `is_end`。
- 静态资源：非 TPCH10 路径没有 TiKV 立即报错；未知 workload 报错。TPCH10 找不到 TiFlash 时返回 0，而非报错。
- metrics：整次查询错误直接传播，单行时间解析错误被忽略；请求失败会尝试后续实例，但首个成功响应的回调错误会立即返回，不再尝试下一实例。非 200、指标缺失、指标格式或浮点解析错误都有明确错误文本。
- 动态合并：TiDB/TiKV 与 TiFlash 两路只有都失败才整体失败，且返回 TiDB 路径错误；单路失败会被 0 替代。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或事务；所有运算同步完成。`Executor` 的 `&mut self` 接口和私有 `done` 防止同一实例并发复用，但类型没有显式线程安全承诺，调用方若跨线程共享仍需自行同步。

采样序列通过拥有的 `Vec` 管理生命周期，函数退出后自动释放。`fetch_store_metrics` 不拥有真实网络句柄，`MetricsResponse.body` 是 `String`；请求和资源关闭责任被抽象进调用者提供的闭包。与此不同，Go `fetchStoreMetrics` 在回调后显式关闭 `resp.Body`，循环中的请求失败则继续尝试下一实例。若未来把 Rust 适配层替换为真实 HTTP 客户端，必须恢复“每个响应在退出该轮前关闭”的资源约束。

## 与 Go 版本的对应关系

数值常量、四种 OLTP/TPCC 基准、TPCH10 常量、1 分钟/24 小时时间边界、10 秒严格对齐、0.1/0.2 利用率阈值、10% 双端裁剪，以及“两路动态结果至少一路成功即可”均直接对应 `calibrate_resource.go`。

重要差异是 Rust `Executor` 不持有 `BaseExecutor`、AST option list 或 session context，不执行 `CalculateAsOfTsExpr`、restricted SQL、cluster-info 查询、domain controller 获取、内部事务来源标记、failpoint 和真实 HTTP；它要求调用者预先提供时间、采样、CPU 核数和 RU 配置。

独立 Rust 测试 `calibrate_resource_test.rs` 的活跃部分验证 Go fixture 的静态结果、TiDB 瓶颈、首实例配额语义、时长边界、极值裁剪、低负载、严格 10 秒对齐、TiFlash、单次产出、资源控制错误、metrics 查询文本、坏行忽略和多实例请求回退；其 `#[cfg(any())]` 部分仅保存不可执行的 Go 测试草稿。

## 扩展指南

- 调整静态模型时修改 `workload_costs`、`BaseResourceCost` 或 `static_calibrate`，并同步独立 Rust 测试 `calibrate_resource_test.rs` 中的 Go fixture 期望值；还应对照 Go `workloadBaseRUCostMap` 与 `TestCalibrateResource`，避免成本单位或整数截断漂移。
- 调整动态采样时优先修改 `TimeSeriesValues::advance`、`dynamic_tidb_quota`、`dynamic_tiflash_quota` 或 `setup_quotas`。必须增加独立测试覆盖 10 秒开区间边界、低利用率组合、两端裁剪和不足两个样本，不能把测试嵌回生产文件。
- 扩展 duration 语法时修改 `parse_duration`；若目标是完全兼容 Go `duration.ParseDuration`，应先列出其全部语法差异并增加表驱动测试，不能仅以现有常见单位覆盖宣称完全兼容。
- 接入真实 Rust SQL 主链时，扩展点是 `Executor`：需要引入 AST options、会话时区、restricted SQL、cluster info、resource-group config 和网络 metrics adapter，并在 Rust builder 中构造它。应保留 `done` 设置顺序、内部请求标记、两路动态错误合并与响应关闭语义；这会扩大依赖与生命周期范围，必须在独立测试中补真实 builder/执行器集成覆盖。
- 修改 metrics 获取时保持 `fetch_store_metrics` 的“跳过无状态地址、请求错误继续、首个响应即返回”顺序，以及 `get_values_from_metrics` 的“查询错误传播、坏行忽略”边界。真实 I/O 接入还需评估超时、取消、TLS 和响应体关闭。
- 性能风险主要来自动态路径对每条输入序列的克隆和排序（约为 `O(n log n)`）以及每次静态调用重建成本表；当前数据规模未在文件中声明，优化前应先测量，且不得改变稳定排序无关的时间对齐语义。
