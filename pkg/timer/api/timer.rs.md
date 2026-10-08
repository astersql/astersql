# `pkg/timer/api/timer.rs` 逻辑说明

## 文件定位

`timer.rs` 是 `astersql-timer-api` crate 的定时器领域模型与调度计算核心。crate 根模块 `pkg/timer/api/lib.rs` 以 `pub mod timer` 声明本模块，并通过 `pub use timer::*` 再导出其公共 API；`pkg/timer/api/Cargo.toml` 则将 crate 根设为 `lib.rs`，声明 `chrono`、`chrono-tz`、`cron`、`fail` 等直接依赖，并以 `package.metadata.porting.go-package = "pkg/timer/api"` 记录 Go 对照包。

该文件位于存储抽象与 runtime 之间：它定义存储和运行时共同交换的 `TimerSpec`、`TimerRecord`、手动请求、事件状态、时区位置以及调度策略。它不负责持久化、轮询或执行 Hook；这些职责分别位于 `pkg/timer/api/store.rs`、`pkg/timer/api/mem_store.rs`、`pkg/timer/runtime/*` 和 `pkg/timer/api/hook.rs`。

## 核心职责

- 用 `SchedEventPolicy` 统一 INTERVAL 与 CRON 两种“根据 watermark 求下一触发时间”的策略，并由 `CreateSchedEventPolicy` 按字符串类型构造具体实现。
- 用 `TimerSpec` 表示用户配置，用 `TimerRecord` 组合配置、当前事件、手动触发、创建时间、版本和已解析时区；`Deref`/`DerefMut` 使记录可直接访问规格字段。
- 校验命名空间、键、时区、策略类型与表达式，保持错误信息与 `pkg/timer/api/timer.go` 的公开语义一致。
- 处理 Cron 五段表达式到 Rust `cron` crate 六段格式的转换，并修正 Go `robfig/cron` 与 Rust crate 对星期数字定义的差异。
- 提供 `parse_location`、`ValidateTimeZone`、`in_location` 和 `now_timestamp`，统一定时器边界上的时间表示与时区换算。

## 主要符号

- `Timestamp = DateTime<FixedOffset>`：可携带固定 UTC 偏移的持久化/交换时间类型；可选时间用 `Option<Timestamp>` 表达 Go 零值时间。
- `SchedEventInterval`、`SchedEventCron`：策略类型协议值 `INTERVAL`、`CRON`；`SchedEventIdle`、`SchedEventTrigger`：事件状态协议值 `IDLE`、`TRIGGER`。这些值会跨 store/runtime 边界流转，不宜任意改名。
- `SchedEventPolicy: Send + Sync`：策略接口，`NextEventTime` 返回 `(Option<Timestamp>, bool)`；时间值表示具体结果，布尔值表示是否存在可调度结果。
- `SchedIntervalPolicy { expr, interval }` 与 `NewSchedIntervalPolicy`：保留原表达式并通过 `ParseDuration` 得到 `std::time::Duration`；支持 `overwrite-ttl-job-interval` failpoint。
- `CronPolicy { schedule }` 与 `NewCronPolicy`：解析 Cron，私有 `schedule` 供策略实现及 `TimerRecord` 的命名时区专用路径使用。
- `normalize_standard_cron`、`shift_cron_day_of_week`、`shift_cron_day_tokens`、`cron_day_contains_out_of_range_value`：兼容 Go 标准五段 Cron 的内部辅助函数；步长分母保持不变，只平移星期数字/范围端点。
- `ManualRequest`：保存请求 ID、发起时间、超时、处理标记和事件 ID；`IsManualRequesting` 判断“ID 非空且未处理”，`SetProcessed` 返回处理后的克隆而不原地修改调用者。
- `EventExtra`：保存手动请求关联 ID 与事件触发时 watermark。
- `TimerSpec`：静态配置；`Clone` 返回深度由 Rust `Clone` 决定的独立值，`Validate` 执行配置校验，`CreateSchedEventPolicy` 将自身字段交给全局工厂。
- `TimerLocation::{Named, Fixed}`：分别保存 IANA `chrono_tz::Tz` 和数值 `FixedOffset`。
- `TimerRecord`：持久化记录；包含 `TimerSpec`、ID、`ManualRequest`、事件字段、摘要、创建时间、CAS `Version` 与解析后的 `Location`。`NextEventTime` 是面向 runtime 的主要计算入口，`Validate` 委托规格校验。
- `ValidateTimeZone`、`parse_location`、`now_timestamp`、`in_location`：时间边界辅助 API；其中 `parse_numeric_offset` 是数值偏移的私有解析器。

## 执行流程

1. 配置进入系统时，调用方构造 `TimerSpec`/`TimerRecord` 并调用 `Validate`。校验依次检查 `Namespace`、`Key`、`TimeZone`、非空策略类型，最后实际构造策略以验证“类型 + 表达式”组合；因此错误顺序是公开行为的一部分（见 `timer_test.rs::test_timer_validate`）。
2. runtime 或其他调度调用方调用 `TimerRecord::NextEventTime`。若 `Enable == false`，立即返回 `Ok((None, false))`，不解析可能已损坏的表达式。
3. 若存在 watermark，先通过 `in_location` 转到记录的 `Location`。INTERVAL 及无命名时区的 CRON 走策略接口；INTERVAL 将解析后的时长加到 watermark，CRON 从 `Schedule::after` 取严格晚于 watermark 的第一个候选。
4. CRON 且 `Location` 为 `TimerLocation::Named` 时，`TimerRecord::NextEventTime` 将 watermark 转为 `chrono_tz::Tz` 后直接查询 `CronPolicy.schedule`，再转回固定偏移。这条专用路径保留夏令时规则，避免只使用当下固定偏移导致跨 DST 计算错误。
5. 五段 Cron 构造时，`NewCronPolicy` 先拒绝星期字段中大于 6 的数值，再由 `normalize_standard_cron` 前置零秒字段，并把星期数字 `0..=6` 平移为 Rust crate 使用的 `1..=7`。宏表达式或非五段输入原样交给 crate 解析。

## 数据与状态

`TimerSpec` 是期望配置：`Namespace + Key` 表示业务身份，`Tags/Data/HookClass` 承载业务属性，`TimeZone/SchedPolicyType/SchedPolicyExpr/Watermark/Enable` 决定调度。`TimerRecord` 在其上增加实际运行状态；通过 `DerefMut` 修改 `record.Enable` 等字段，实际修改的是内嵌 `TimerSpec`。

`Watermark` 是上次成功调度的进度基准，不是“当前时间”。INTERVAL 在其上加固定时长；CRON 寻找其后的第一个匹配点。缺失 watermark 的两种策略语义不同：INTERVAL 返回 `(None, true)`，表示策略有效但没有可计算基点；CRON 返回 `(None, false)`。上层 `TimerRecord` 还以 `Enable == false` 返回 `(None, false)`，调用者必须同时解释值和布尔位，不能只检查 `Option`。

`EventStatus`、`EventID`、`EventData`、`EventStart` 和 `EventExtra` 描述当前事件；本文件只提供载体，不强制字段间不变量。Go 源码注释规定 IDLE 时事件 ID/数据为空、开始时间为零值，这些约束由 store/runtime 更新流程及测试维护。`Version` 是 store 更新使用的乐观并发版本，实际 CAS 逻辑不在本文件。

## 依赖与调用关系

下游依赖如下：

- `crate::error::{TimerError, TimerResult}`：统一配置和解析错误；本文件用 `TimerError::message` 补充字段/表达式上下文。
- `parser_duration::ParseDuration`：通过 `#[path = "../../parser/duration/duration.rs"]` 引入解析实现；Cargo 同时声明 `astersql-parser-duration` 路径依赖，但本文件当前直接使用 path module，这是需要避免继续分叉的接线事实。
- `chrono`、`chrono-tz`：时间戳、固定偏移、本地偏移、IANA 时区及有界加法。
- `cron::Schedule`：Cron 解析与候选迭代；本文件承担与 Go parser 的格式适配。
- `fail`：`NewSchedIntervalPolicy` 的 TTL 间隔覆盖测试点。

RustCodeGraph（索引状态：11,467 文件、307,296 节点）确认的重要调用边包括：`TimerSpec::Validate -> ValidateTimeZone/CreateSchedEventPolicy`，`TimerRecord::NextEventTime -> NewCronPolicy/CreateSchedEventPolicy/SchedEventPolicy::NextEventTime/in_location`，`NewCronPolicy -> cron_day_contains_out_of_range_value/normalize_standard_cron`。上游直接调用包括 `pkg/timer/tablestore/store.rs::checkUpdateConstraints -> CreateSchedEventPolicy/ValidateTimeZone`、`pkg/timer/api/store.rs::apply -> parse_location`、`pkg/timer/api/mem_store.rs::{Create,Update} -> now_timestamp` 以及 runtime cache 对 `TimerRecord::Clone`/调度字段的使用。`pkg/timer/api/lib.rs` 再导出这些符号，使 client、store、runtime、tablestore 和 TTL timer 同步代码可经 crate 根访问。

## 错误处理与边界

- `NewSchedIntervalPolicy` 将 duration 解析错误包装为 `invalid schedule event expr '<expr>': ...`；`TimerSpec::Validate` 再包装为 `schedule event configuration is not valid: ...`。
- INTERVAL 把 `std::time::Duration` 转为 `chrono::Duration` 失败时返回 `(None, false)`；时间加法用 `checked_add_signed`，溢出会得到 `None`。当前实现仍将布尔位返回为 `true`，因此极端溢出下 `(None, true)` 是现有事实，扩展时不可未经兼容评估改义。
- Cron 解析错误包含原表达式；无未来匹配（例如 2 月 30 日）返回 `(None, false)`，不是错误。
- `cron_day_contains_out_of_range_value` 只对五段表达式的星期字段执行数值上界预检；完整合法性仍由 `Schedule::from_str` 决定。
- `parse_location` 接受空串和不区分大小写的 `SYSTEM`（均取进程本地当前偏移）、IANA 名，以及 `+0800`、`+08:00`、`-6:00` 等数值偏移。正偏移上限 `+14:00`，负偏移拒绝小于 `-12:59` 的值；分钟必须不超过 59。未知值返回 `Unknown or incorrect time zone: '<tz>'`。
- 空时区在 `ValidateTimeZone` 中合法，但其实际集群时区语义依赖调用方是否为记录填充 `Location`；`parse_location("")` 本身采用进程本地当前固定偏移，而不是动态保留命名时区规则。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或 I/O。策略为短生命周期纯计算对象，`SchedEventPolicy: Send + Sync` 允许其在线程间安全传递/共享；`CronPolicy.schedule` 和 interval 值随对象所有权释放，无显式清理。

`TimerRecord` 是可克隆的值快照，不包含共享可变引用。`Clone` 会复制字符串、向量和嵌套结构；它不是与原记录联动的视图。并发更新的一致性由 `Version` 及 store 层条件更新保障，本文件只携带版本号。`Location::Fixed` 固化偏移，`Location::Named` 则在每次 Cron 计算时保留时区数据库规则；若系统本地时区在进程运行期间变化，已经由空串/`SYSTEM` 解析得到的固定偏移不会自动刷新。

## 与 Go 版本的对应关系

主要一一对应关系来自 `pkg/timer/api/timer.go`：Rust 的策略常量、`SchedEventPolicy`、两种策略、`ManualRequest`、`EventExtra`、`TimerSpec`、状态常量、`TimerRecord`、`NextEventTime`、`Clone` 和 `ValidateTimeZone` 均复刻同名 Go 概念。`pkg/timer/api/timer_test.go` 与 `schedule_policy_test.go` 的校验顺序、错误片段、固定间隔、时区换算、Cron 宏/标准表达式和无未来日期案例，在 Rust 的 `timer_test.rs` 与 `schedule_policy_test.rs` 中有对应覆盖。

语言映射差异包括：Go `time.Time` 零值在 Rust 中主要映射为 `Option<Timestamp>::None`；Go 指针返回值映射为拥有所有权的值或 `Box<dyn SchedEventPolicy>`；Go 匿名嵌入通过 Rust `TimerSpec` 字段加 `Deref/DerefMut` 模拟；Go `*time.Location` 映射为显式 `TimerLocation` 枚举。

Rust 还包含必要兼容接线：Go `robfig/cron.ParseStandard` 接受五段表达式并以 0 表示星期日，而 Rust `cron` crate 使用秒字段并采用不同星期编号，所以 Rust 增加标准化函数；`schedule_policy_test.rs::cron_day_of_week_step_matches_robfig_standard_parser` 和 `cron_rejects_day_seven_like_robfig` 专门锁定该语义。Rust 的 `TimerRecord::NextEventTime` 对命名时区增加 DST 安全路径，这是为了保持 Go `time.Location` 的运行效果，而不是新的业务策略。

## 扩展指南

- 新增策略类型时，必须同时增加协议常量、具体 `SchedEventPolicy` 实现和 `CreateSchedEventPolicy` 分支，并扩展 `schedule_policy_test.rs`；还应同步评估 Go `timer.go`/`schedule_policy_test.go`、store 编解码与持久化协议，避免只在 Rust 接受新值。
- 修改下一事件语义时，优先修改具体策略的 `NextEventTime`；涉及命名时区 Cron 时还必须同步审查 `TimerRecord::NextEventTime` 的专用分支，防止 trait 路径与 DST 路径分叉。
- 扩充时区格式时，修改 `parse_location`/`parse_numeric_offset` 和 `ValidateTimeZone`，并同步 `timer_test.rs::time_zone_parser_matches_tidb_system_colon_and_bounds`；需确认 `pkg/timer/api/store.rs`、`mem_store.rs` 和 `pkg/timer/tablestore/store.rs` 对 `Location` 的构造/持久化仍一致。
- 添加 `TimerSpec` 或 `TimerRecord` 字段时，除了本文件，还要检查 `store.rs` 的更新条件、`mem_store.rs` 的规范化、`tablestore` SQL 编解码、client options、runtime cache/worker 和 TTL 同步。字段若参与 CAS 或事件状态机，不能只依赖 `Clone`/derive 默认行为。
- 保持测试逻辑独立在 `timer_test.rs`、`schedule_policy_test.rs` 等测试文件中；不要把测试内嵌回生产源文件。性能上，策略当前按调用构造并解析表达式，若引入缓存，需要明确失效条件、线程共享方式以及时区数据库更新语义。

## 验证依据

直接读取并核对的生产/配置文件：`pkg/timer/api/timer.rs`、`pkg/timer/api/lib.rs`、`pkg/timer/api/Cargo.toml`、Go 对照 `pkg/timer/api/timer.go`。该目录不存在 `doc.go`。

直接读取并核对的测试：`pkg/timer/api/timer_test.rs`、`pkg/timer/api/schedule_policy_test.rs`、`pkg/timer/api/timer_test.go`、`pkg/timer/api/schedule_policy_test.go`。其中覆盖必填字段和错误顺序、合法/非法时区、启停行为、INTERVAL 解析与计算、Cron 宏/五段表达式、星期步长、拒绝星期 7、闰日与无未来日期。

RustCodeGraph 执行过 `status`、`files --filter pkg/timer/api`、针对 `timer.rs TimerRecord TimerSpec NextEventTime CreateSchedEventPolicy` 的 `explore`、`query TimerRecord`、`query NextEventTime`，以及索引文件的 `node --file` 查询。图证据确认本文件 34 个符号、由 20 个文件使用，并确认策略构造/校验/调度计算的内部调用边和 store、runtime、tablestore、TTL 相关上游使用点。最终结构以任务指定命令验证固定 11 个二级章节；本任务为纯文档分析，未运行 Cargo。
