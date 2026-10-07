# `pkg/dxf/framework/schstatus/tune.rs`

## 文件定位

[`tune.rs`](./tune.rs) 属于 `astersql-dxf-framework-schstatus` crate，该 crate 的入口 [`lib.rs`](./lib.rs) 声明 `pub mod tune` 并通过 `pub use tune::*` 重导出本文件的公开 API。[`Cargo.toml`](./Cargo.toml) 将该 crate 映射到 Go 包 `pkg/dxf/framework/schstatus`，并且只直接依赖 `chrono`、`serde` 和 `serde_json`；其中本文件实际使用后两者，TTL 的时间表示由同 crate 的 [`status.rs`](./status.rs) 提供。

它不负责读写存储或执行资源调度，而是 DXF（Distributed eXecution Framework）调度调优配置的数据契约层：定义取值边界、默认值、持久化 JSON 形状和带 TTL 的封装。直接上游包括 `framework/handle`、`framework/storage`、`framework/scheduler` 以及 server handler，这些依赖在各自 `Cargo.toml` 中指向本 crate。

## 核心职责

1. 用 `MinAmplifyFactor`、`MaxAmplifyFactor` 和 `defaultAmplifyFactor` 表达放大因子的契约边界与业务默认值。
2. 用 `TuneFactors` 承载资源估算所需的 `AmplifyFactor`，并将它序列化为 `amplify_factor`。
3. 用 `TTLTuneFactors` 将 `status::TTLInfo` 与 `TuneFactors` 展平为同一层 JSON，对齐 Go 的匿名结构体嵌入，供持久化和接口输出。
4. 通过 `TTLTuneFactors::String` 提供 Go `fmt.Stringer` 式的 JSON 文本，通过 `GetDefaultTuneFactors` 提供明确的业务默认构造器。

本文件只定义数据与转换：TTL 是否过期由 [`handle.rs`](../handle/handle.rs) 的 `GetScheduleTuneFactors` 判断，放大因子如何影响资源数量由 [`autoscaler.rs`](../scheduler/autoscaler.rs) 的 `ResourceCalc` 实现。

## 主要符号

- `fn is_zero(value: &f64) -> bool`：私有 serde 谓词。仅在数值精确等于 `0.0` 时返回 `true`，用于 `AmplifyFactor` 的 `skip_serializing_if`。
- `pub const MinAmplifyFactor: f64 = 1.0`：对外公开的最小允许值，与 Go 常量一致。
- `pub const MaxAmplifyFactor: f64 = 10.0`：对外公开的最大允许值，与 Go 常量一致。
- `pub const defaultAmplifyFactor: f64 = MinAmplifyFactor`：本 crate 公开但按 Go 包内名命保留的默认值，当前为 `1.0`。
- `pub struct TuneFactors { pub AmplifyFactor: f64 }`：可克隆、可比较、可 serde 序列化/反序列化的调优因子集合。`#[serde(rename = "amplify_factor", default, skip_serializing_if = "is_zero")]` 决定 JSON 字段名、缺失字段的反序列化值及零值省略行为。
- `pub struct TTLTuneFactors { pub TTLInfo: TTLInfo, pub TuneFactors: TuneFactors }`：持久化容器。两个字段都标注 `#[serde(flatten)]`，所以 JSON 顶层直接包含 `ttl`、`expire_time` 和可选的 `amplify_factor`，不会出现 `TTLInfo` 或 `TuneFactors` 嵌套对象。
- `pub fn TTLTuneFactors::String(&self) -> String`：调用 `serde_json::to_string(self)`，成功时返回紧凑 JSON，失败时通过 `unwrap_or_default()` 返回空字符串。
- `pub fn GetDefaultTuneFactors() -> TuneFactors`：每次按值返回 `TuneFactors { AmplifyFactor: 1.0 }`。Rust 的所有权值对应 Go 每次返回新 `*TuneFactors` 的独立对象语义。

## 执行流程

默认配置流程如下：

1. `handle::GetScheduleTuneFactors` 从运行时/存储层请求当前 keyspace 的 `Option<TTLTuneFactors>`。
2. 未找到配置，或 `factors.TTLInfo.ExpireTime < SystemTime::now()` 时，调用本文件的 `GetDefaultTuneFactors`，返回放大因子 `1.0`。
3. 配置仍在有效期内时，从 `TTLTuneFactors` 移出其 `TuneFactors` 并返回。
4. `scheduler::ResourceCalc::new`/`for_add_index` 克隆 `TuneFactors`，后续 `amplified_data_size` 用 `AmplifyFactor * (1 + index_size_ratio) * data_size` 放大有效数据量；两个最大节点数计算还用同一因子放大节点上限。

持久化/输出流程如下：调用方组合 `TTLInfo` 和 `TuneFactors` 形成 `TTLTuneFactors`；serde 遍历两个 `flatten` 字段；`TTLInfo` 按 [`status.rs`](./status.rs) 的自定义规则将 TTL 写为纳秒整数、将过期时间写为 RFC3339；`TuneFactors` 写入非零 `amplify_factor`；`String` 最终返回 JSON 文本。

## 数据与状态

本文件没有全局可变状态。三个 `f64` 常量在编译期固定，`TuneFactors` 和 `TTLTuneFactors` 都是由调用方拥有的普通值对象。

需要区分两种“默认”：

- `TuneFactors::default()` 来自 `#[derive(Default)]`，因此 `AmplifyFactor == 0.0`。JSON 反序列化缺失 `amplify_factor` 时，字段上的 `serde(default)` 也会得到 `0.0`。
- `GetDefaultTuneFactors()` 是业务默认构造器，返回 `AmplifyFactor == 1.0`。调度业务不应用 `TuneFactors::default()` 代替它。

`TTLTuneFactors::default()` 同样会包含放大因子 `0.0`；其 `TTLInfo::default()` 则由 [`status.rs`](./status.rs) 定义为 `TTL = 0` 和 Go 零时间形状的 `ExpireTime`。在 JSON 中，`0.0` 的 `amplify_factor` 会被省略，但 `expire_time` 没有 `skip_serializing_if`，仍会输出。

## 依赖与调用关系

- 内部依赖：`TTLTuneFactors` 使用 `super::status::TTLInfo`；该类型决定 TTL 与过期时间的 Rust 类型及 JSON 编码。
- 外部依赖：`serde::{Serialize, Deserialize}` 生成数据转换，`serde_json::to_string` 实现 `String`。本文件不直接使用 `chrono`。
- 上游获取：[`handle.rs`](../handle/handle.rs) 的 `GetScheduleTuneFactors` 调用 `GetDefaultTuneFactors`，并在配置有效时返回 `TTLTuneFactors.TuneFactors`。[`status_testkit_test.rs`](../handle/status_testkit_test.rs) 锁定了缺失、未过期和已过期三个分支。
- 下游消费：[`autoscaler.rs`](../scheduler/autoscaler.rs) 的 `ResourceCalc` 保存 `TuneFactors` 副本，并在 `amplified_data_size`、`max_node_count_for_add_index` 和 `max_node_count_for_import_into` 中使用 `AmplifyFactor`。[`autoscaler_test.rs`](../scheduler/autoscaler_test.rs) 验证了 `1.0`、`1.5`、`2.0`、`5.0`、`10.0` 等因子对槽位和节点数的影响。
- 存储边界：`framework/storage` 通过 Cargo 别名 `schstatus-crate` 依赖本 crate，`handle` 运行时接口以 `Option<TTLTuneFactors>` 传递持久化值。
- HTTP 边界：Go [`pkg/server/handler/tikvhandler/dxf.go`](../../../server/handler/tikvhandler/dxf.go) 使用两个边界常量校验 POST 输入。Rust [`pkg/server/handler/tests/dxf_test.rs`](../../../server/handler/tests/dxf_test.rs) 也直接验证默认因子等于下限且不超过上限。

RustCodeGraph 对本文件报告 6 个符号和 9 个引用文件，其 `explore` 结果明确给出 `GetDefaultTuneFactors -> handle::GetScheduleTuneFactors` 及相关测试边；精确 `callers`/`callees` 命令未输出更细边，因此上述其他边由直接引用搜索和源码核对，没有将缺失的图边当作已验证事实。

## 错误处理与边界

`tune.rs` 不返回 `Result`，也不校验 `AmplifyFactor` 是否落在 `[1.0, 10.0]`。边界常量是供入口层执行校验的契约，而不是 `TuneFactors` 的类型不变式：公开字段、`Default` 和 serde 反序列化都能构造出范围外值。Go server handler 明确在持久化前执行下限/上限判断；新的 Rust 入口也必须在边界层保留同等校验。

`TTLTuneFactors::String` 有意对齐 Go 中忽略 `json.Marshal` 错误的做法：Rust 序列化失败不向上传播，而是变成空串。例如 JSON 无法表示的非有限浮点值会进入此错误路径。因此调用方不能把空串视为有效 JSON，也不应依赖此方法获取可诊断的序列化错误。

TTL 边界也不在本文件执行：`GetScheduleTuneFactors` 使用严格小于当前时间判断过期，而 `TTLTuneFactors` 本身可以携带任意 `TTLInfo`。serde `flatten` 还意味着新增字段时必须检查两个被展平结构之间的 JSON 键冲突。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或 I/O 资源。所有类型都是拥有型数据，`String` 只在调用栈上借用 `&self` 并创建新 `String`，`GetDefaultTuneFactors` 每次创建新值。

并发与生命周期责任在上游：存储读取和事务由 `handle`/存储适配器管理，过期时间在每次获取时与 `SystemTime::now()` 比较，`ResourceCalc` 则通过 `Clone` 保存配置快照。这意味着后续更改原 `TuneFactors` 不会追溯修改已创建的资源计算器。

## 与 Go 版本的对应关系

直接对照文件为 [`tune.go`](./tune.go)：

- 三个常量的数值和默认值关系相同：`1.0`、`10.0`、`default = min`。
- Go `TuneFactors.AmplifyFactor float64 \`json:"amplify_factor,omitempty"\`` 对应 Rust 的 rename、default 与零值省略组合。
- Go `TTLTuneFactors` 匿名嵌入 `TTLInfo` 和 `TuneFactors`；Rust 以命名字段加 `serde(flatten)` 复制 JSON 形状。[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 确认序列化结果顶层含 `ttl`、`expire_time`、`amplify_factor`，且不含 Rust 字段名嵌套层。
- Go `(*TTLTuneFactors).String()` 忽略 `json.Marshal` 错误；Rust `String(&self)` 以 `unwrap_or_default()` 保留“不向上返错”的契约。Rust 是固有方法而非 `Display` trait 实现，因此不能自动用 `{}` 格式化。
- Go `GetDefaultTuneFactors() *TuneFactors` 返回指针；Rust 返回拥有值。两者都给调用者独立配置，但 Rust 调用方通过借用或显式克隆传递。
- Go 没有为这些结构定义业务化 `Default`；Rust 的派生 `Default` 为零值语义，它对齐 Go 结构体零值，但不等于 `GetDefaultTuneFactors` 的业务默认值。

Go 目录没有独立 `tune_test.go`；相关 Go 行为由 [`handle/status_testkit_test.go`](../handle/status_testkit_test.go) 的 `TestGetScheduleTuneFactors` 和 [`scheduler/autoscaler_test.go`](../scheduler/autoscaler_test.go) 的 `TestTuneFactors` 覆盖。Rust 对应测试位于独立文件，未嵌入生产源文件。

## 扩展指南

- 新增调优因子时，修改 `TuneFactors`，同步更新 Go [`tune.go`](./tune.go)、默认构造器、JSON 命名/省略规则和真正消费该因子的 `ResourceCalc` 逻辑。先决定缺失 JSON 字段应该是 Go 零值还是业务默认值，不要无意混用 `derive(Default)` 与 `GetDefaultTuneFactors`。
- 改变上下限时，同时检查所有输入入口的范围校验和 autoscaler 中“放大数据量又放大节点上限”的算法效果。兼容风险包括旧持久化 JSON、Go/Rust 对等性以及资源预算突变；性能风险是过大因子可显著增加槽位和节点请求。
- 扩展 `TTLTuneFactors` 的 JSON 时，保持 `flatten` 后的键唯一，并考虑旧读取器是否能忽略新字段。如需更可靠的错误诊断，应新增返回 `Result` 的 API，而不实质改变已对齐 Go 的 `String` 容错行为。
- 测试应保持在独立文件。默认值、边界和 JSON 展平应扩展 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)；缺失/过期语义应扩展 [`status_testkit_test.rs`](../handle/status_testkit_test.rs)；资源估算影响应扩展 [`autoscaler_test.rs`](../scheduler/autoscaler_test.rs)；HTTP 输入边界应扩展 server handler 的独立测试。
- 本文件没有条件编译项；如果引入 feature 分支，必须同时说明持久化数据在不同 feature 组合下的兼容性。

## 验证依据

- RustCodeGraph：`status` 确认本仓库索引包含 7,032 个 Rust 文件；`files --filter pkg/dxf/framework/schstatus` 找到本文件、crate 入口、Go 对照和独立测试；`node --file pkg/dxf/framework/schstatus/tune.rs --offset 1 --limit 260` 读取全部 81 行并报告 9 个引用文件；`query GetDefaultTuneFactors --limit 10` 确认 Go/Rust 各有一个对应定义；`explore "pkg/dxf/framework/schstatus/tune.rs symbols callers callees"` 确认 `GetDefaultTuneFactors` 的 handle 与测试边。`callers`/`callees` 命令本次无额外输出，已用直接引用搜索补齐证据。
- 源码与 crate 边界：[`tune.rs`](./tune.rs)、[`status.rs`](./status.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)、[`handle.rs`](../handle/handle.rs)、[`autoscaler.rs`](../scheduler/autoscaler.rs)，以及 `handle`、`scheduler`、`storage`、server handler 的 Cargo 依赖声明。
- Go 对照：[`tune.go`](./tune.go)、[`handle.go`](../handle/handle.go)、[`autoscaler.go`](../scheduler/autoscaler.go)、[`pkg/server/handler/tikvhandler/dxf.go`](../../../server/handler/tikvhandler/dxf.go)。
- 独立测试：[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 验证默认/边界/JSON 展平；[`status_testkit_test.rs`](../handle/status_testkit_test.rs) 验证缺失、有效与过期分支；[`autoscaler_test.rs`](../scheduler/autoscaler_test.rs) 验证资源计算效果；Go 对照测试为 [`handle/status_testkit_test.go`](../handle/status_testkit_test.go) 和 [`scheduler/autoscaler_test.go`](../scheduler/autoscaler_test.go)。同目录未发现独立 `tune_test.go`。
- 人工复核结论：本文件存在于稳定调度调优的跨语言数据契约；运行时由 handle 选择默认或有效配置，由 scheduler 消费；安全扩展需同步 Go/Rust JSON、业务默认、入口校验和三层独立测试。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 shell 命令校验文件存在且恰有 11 个固定二级章节。
