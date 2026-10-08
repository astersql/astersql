# `pkg/resourcegroup/runaway/checker.rs`

## 文件定位

`checker.rs` 位于 `astersql-resourcegroup-runaway` crate 内，是 runaway（失控查询）机制的“单查询检查器”。`pkg/resourcegroup/runaway/lib.rs` 公开 `checker` 模块，并定义本文件直接使用的 `RunawaySettings`、`RunawayAction`、`RunawayWatchType`、`CopRequest`、`RUDetails`、`Error` 和微秒时间戳。`pkg/resourcegroup/runaway/Cargo.toml` 以 `lib.rs` 为 crate 入口，并用 `package.metadata.porting.go-package = "pkg/resourcegroup/runaway"` 指向 Go 对照目录；清单中的本地依赖当前均放在 `cfg(windows)` 条件下，而本文件只直接依赖标准库原子类型和同 crate 的 `Manager`/公共类型。

它处在 SQL 编译与分布式执行之间：`pkg/session/runtime/scan_adapter_runtime.rs::RunawayBeforeExecutor` 在资源控制启用且 Domain 已绑定 `Manager` 时调用 `Manager::DeriveChecker`，随后立即调用 `Checker::BeforeExecutor` 检查 quarantine watch。检查器再由 `pkg/session/runtime/typed_runaway_checker.rs::SessionRunawayChecker` 包装成 KV 层 trait 对象，经 `pkg/store/driver/runaway_adapter.rs::KVRunawayChecker` 进入 Coprocessor 请求发送、响应阈值检查和迭代结束重置链路。

本文件不负责资源组配置加载、watch 同步或记录刷盘。资源组目录、内存 watch 列表和记录队列由 `manager.rs` 管理；本文件只持有构造时的设置快照、查询标识和运行中计数，判定何时以及以什么动作标记当前查询。

## 核心职责

1. `Manager::DeriveChecker` 依据资源组是否存在、plan digest 是否存在、是否有 runaway 设置或活跃 watch，决定是否为当前查询创建 `Checker`。
2. `Checker::NewChecker` 把毫秒执行时限转换为微秒绝对 `deadline`，并快照 RU、processed keys 阈值及动作/watch 配置。
3. `BeforeExecutor` 在真正执行前按“原始 SQL → SQL digest → plan digest”顺序查询 watch 列表；命中后让 Kill 立即报错，让 SwitchGroup 返回经校验的目标组，让 CoolDown 留待 Cop 请求阶段执行。
4. `BeforeCopRequest` 在每次 Cop 请求发出前检查耗时阈值，并按当前动作缩短请求超时、降低优先级、切换资源组或中断查询。
5. `CheckThresholds` 在 Cop 响应后累计 processed keys，同时检查 deadline、RU 和累计 keys；`CheckRuleKillAction` 提供只按时间轮询 Kill 的入口。
6. 第一次规则超限时，`markRunawayByIdentifyInRunawaySettings` 用 CAS 保证只生成一次 runaway 记录；配置允许且快照仍与目录一致时，还生成 quarantine watch。watch 命中也生成 runaway 记录，但优先于规则动作。
7. `CheckAction` 向下游暴露当前生效动作，`ResetTotalProcessedKeys` 为新一轮 Cop 扫描清空累计值。

## 主要符号

- `READ_TIMEOUT_MEDIUM_MILLIS = 60_000`：只有 Kill 的剩余 deadline 小于 60 秒且仍为正数时，`BeforeCopRequest` 才把它写入 `CopRequest::max_execution_duration_ms`。
- `Checker`：绑定一次查询的状态对象。公开字段 `manager`、`resource_group_name`、`original_sql`、`sql_digest`、`plan_digest` 也供 `Manager::markRunaway` 生成审计记录；阈值、设置和可变标记保持私有。
- `NewChecker(...) -> Checker`：公开构造函数。没有设置时三个阈值均为 0；耗时阈值使用 `saturating_mul(1000)` 和 `saturating_add`，避免极端配置导致整数溢出。
- `Manager::DeriveChecker(...) -> Option<Checker>`：定义在本文件的 `Manager` 扩展方法，是生产路径的主要构造入口。资源组查找失败/不存在、plan digest 为空、或既无设置也无活跃 watch 时返回 `None`。
- `BeforeExecutor(&mut self) -> Result<String>`：执行前 watch 入口。空字符串表示不切组；Kill 返回 `Error::Quarantined`。
- `BeforeCopRequest(&self, &mut CopRequest) -> Result<()>`：发送前规则入口，可原地修改低优先级、资源组和最大执行时长；规则 Kill 返回 `Error::QueryInterrupted`。
- `CheckThresholds(&self, Option<&RUDetails>, i64, Option<Error>) -> Option<Error>`：响应后检查入口。未产生 Kill 时保留原错误，Kill 超限时以 `QueryInterrupted` 替换它。
- `exceedsThresholds(...) -> String`：纯判定函数，按耗时、RU、processed keys 的固定优先级返回首个原因；空串代表未超限。
- `CheckAction` / `CheckRuleKillAction` / `ResetTotalProcessedKeys`：分别供下游查询动作、轮询规则 Kill、重置累计 keys。
- `checkSwitchGroupName`：通过 `Manager::resourceGroup` 校验目标组；空名、查找错误或不存在都降级为空串。
- `markRunawayByIdentifyInRunawaySettings`：规则命中的单次提交点；`AtomicBool::compare_exchange` 成功者负责记录及可选 quarantine。
- `markRunawayByQueryWatchRule`：在可变的执行前阶段保存 watch 动作，并写一条 `match_type = "watch"` 的记录。
- `markQuarantine` / `getSettingConvictIdentifier`：前者在配置快照仍有效时提交 watch，后者按 Plan/Similar/Exact 选择 plan digest、SQL digest 或原始 SQL。

## 执行流程

创建及执行前流程：

1. `RunawayBeforeExecutor` 清理上一语句状态，检查全局资源控制开关，从 Domain 获取 `Manager`，并把语句起始时间转换为微秒。
2. `Manager::DeriveChecker` 从目录读取资源组。缺少资源组或 plan digest 时直接跳过；没有规则时，只有该组存在活跃 watch 才继续创建。
3. `NewChecker` 固化语句文本、两个 digest、资源组设置和绝对 deadline。此后目录配置变更不会改写该查询正在使用的规则快照。
4. `BeforeExecutor` 依序调用 `Manager::examineWatchList`。命中的 watch 若动作是 `NoneAction`，则回退到资源组设置的动作和切组名；若设置也不存在，继续检查下一个 convict。
5. 有效命中先经 `markRunawayByQueryWatchRule` 保存 watch 状态并入队审计记录，再执行动作：Kill 报 `Quarantined`；SwitchGroup 返回存在的目标组；CoolDown/DryRun 返回空串。
6. Session 仅在上述步骤成功后把 `Checker` 包进 `Arc<SessionRunawayChecker>`，保存到会话状态并供 KV/Cop 链共享。因此非原子的 watch 字段在共享前已经初始化完成。

Cop 请求与响应流程：

1. `SessionRunawayChecker::BeforeCopRequest` 把 KV 请求转换为本 crate 的 `CopRequest`，调用本文件后再回写字段；Store 的 `KVRunawayChecker` 继续把这些字段映射到线上的 Cop 请求。
2. `BeforeCopRequest` 先应用 watch CoolDown。没有资源组设置时到此结束，已有 watch Kill/SwitchGroup 已在执行前处理。
3. 有设置时先以当前时间调用 `exceedsThresholds`。尚未超限且尚未被规则标记时，Kill 规则可把正的剩余 deadline（小于 60 秒）写入请求，然后返回。
4. 已经超限或已被其他并发路径标记时，函数尝试单次记录，再按设置执行 Kill、CoolDown、SwitchGroup 或无操作。有效切组名才覆盖请求资源组。
5. Cop 响应到达后，`CheckThresholds` 对 processed keys 做原子累加。只有原错误文本以 `Coprocessor task terminated due to exceeding the deadline` 开头时才用当前时间检查 deadline；普通响应不会重复以墙钟时间判定耗时。
6. 阈值优先级固定为 deadline、读写 RU 之和、累计 processed keys。命中后尝试单次标记；只有动作是 Kill 才返回新的 `QueryInterrupted`，其余动作保留调用者传入的原错误，等待后续请求阶段应用。
7. Cop 迭代一轮结束时，下游调用 `ResetTotalProcessedKeys`；并发 worker 也可以通过 `CheckAction` 观察 watch 优先、规则次之的当前动作。

## 数据与状态

`Checker` 中的查询身份包括资源组名、原始 SQL、SQL digest 和 plan digest。原始 SQL 用于 Exact watch，SQL digest 用于 Similar watch，plan digest 用于 Plan watch；执行前匹配会依次尝试三者，而自动写 quarantine 时只按 `settings.watch.kind` 选择一个标识。

`settings: Option<RunawaySettings>` 是构造快照，派生出的 `deadline`、`ru_threshold` 和 `processed_keys_threshold` 也在构造后不变。数值 0 是禁用标记：`deadline == 0` 不检查时间，两个资源阈值为 0 时不检查对应资源。比较使用 `>=`，恰好达到阈值即算超限。RU 以 `write_ru + read_ru` 的浮点和转为 `i64` 后比较，这与当前 Rust 实现的截断行为一致。

运行时可变状态分为两类：`total_processed_keys: AtomicI64` 累加所有 Cop 任务的 keys；`marked_by_identify: AtomicBool` 表示规则已首次命中。二者可在共享引用下并发更新。`marked_by_query_watch_rule` 与 `watch_action` 是普通字段，只能由 `BeforeExecutor(&mut self)` 写入；生产接线在把对象放进 `Arc` 前完成这一步，之后只读。

watch 与规则可以同时命中，但 `CheckAction` 永远先返回 watch 动作。规则的 CAS 仍可能在之后变为 true，不过 `markRunawayByIdentifyInRunawaySettings` 发现已由 watch 命中时不会再添加 quarantine，避免同一查询由规则重复扩散 watch。

## 依赖与调用关系

主要上游调用链为：

- `pkg/session/runtime/scan_adapter_runtime.rs::RunawayBeforeExecutor` → `Manager::DeriveChecker` → `Checker::BeforeExecutor`。
- `pkg/session/runtime/typed_runaway_checker.rs::SessionRunawayChecker` 把 `Checker` 适配为 `astersql_kv::resourcegroup::RunawayChecker`，转发 `BeforeCopRequest`、`CheckThresholds`、`CheckAction` 和 `ResetTotalProcessedKeys`。
- `pkg/store/driver/runaway_adapter.rs::KVRunawayChecker` 再把 KV trait 适配为 `astersql_store_copr::RunawayChecker`，由 `pkg/store/copr/coprocessor.rs` 在请求发送、响应处理和迭代结束路径调用。RustCodeGraph 的 explore 结果明确列出 `handle_task_once` 调用 `before_cop_request`/`check_thresholds`，以及 `next_lite`/Drop 路径调用 reset。
- `CheckRuleKillAction` 当前的直接 Rust 证据主要来自 `checker_test.rs`；未在上述生产适配 trait 中暴露，不能据 Go 接口假定它已接入 Rust 执行主链。

主要下游为 `Manager`：`BeforeExecutor` 调用 `examineWatchList`；切组与 quarantine 快照校验调用 `resourceGroup`；`markRunaway` 把查询审计记录加入 `runaway_records`；`markQuarantine` 同时更新本地 watch 列表并加入 `quarantine_records`。队列满或 Manager 内部锁中毒时可能静默丢弃记录，这是 `manager.rs` 的边界，不会反向让当前查询失败。

RustCodeGraph 将 `checker.rs` 列为被 `pkg/session/runtime/scan_adapter_runtime.rs`、`typed_runaway_checker.rs`、`pkg/store/driver/runaway_adapter.rs` 等链路间接消费，并识别出目标函数到 `Manager` 方法的调用关系。精确 `callers` 命令对这些 Rust 关联函数未返回完整结果，因此调用点又以限定 `*.rs` 的 `rg` 和索引 `node --file` 源码复核；本文不把图中缺失的边解释为“无调用”。

## 错误处理与边界

- `DeriveChecker` 把目录错误和资源组不存在都降级为 `None`，不阻断语句；Rust 当前不像 Go 版本那样记录该目录错误日志。
- `BeforeExecutor` 的 Kill 使用 `Error::Quarantined`，表示在执行前命中 watch；规则 Kill 使用携带原因的 `Error::QueryInterrupted`，两者语义不可互换。
- SwitchGroup 目标为空、不存在或目录查询失败时返回空串/保持原资源组，而不是报错。这让查询继续执行，但会隐藏配置错误。
- `CheckThresholds` 没有设置时原样返回 `original_error`；有设置但未超限时同样保留它。若 Kill 阈值命中，则 `QueryInterrupted` 覆盖原错误。非 Kill 动作即使已标记 runaway，也不吞掉原错误。
- deadline 只在 `now != 0` 时参与 `exceedsThresholds`。`CheckThresholds` 仅通过特定英文错误前缀识别 Cop deadline 终止；上游若改变文案，响应路径将不再据此判定耗时。
- 原始错误在 Session trait 适配层被包装成 `Error::Storage(String)`，之后通过“返回值是否仍等于原错误”决定是保持成功还是转换成字符串错误；新增错误变体时必须同步检查这一映射。
- `markQuarantine` 只有在设置含 watch，且目录中当前 `runaway_settings` 与构造快照完全相等时才执行，避免资源组配置更新后按旧规则隔离后续查询。目录错误同样静默跳过。
- Manager 的审计/quarantine 队列是有界、非阻塞式的；CAS 成功只证明本 `Checker` 已标记，不保证记录最终持久化。
- `NewChecker` 的时间换算饱和处理溢出；但 `nowMicros` 和 RU 浮点转整数仍受各自表示范围/转换规则约束。

## 并发与资源生命周期

生产路径先以独占 `&mut Checker` 完成 watch 检查，再创建 `Arc<Checker>`，因此普通的 `marked_by_query_watch_rule`/`watch_action` 不与执行中 worker 并发写。后续并发入口都只接收 `&self`：processed keys 以 `fetch_add(Ordering::AcqRel)` 累加，重置以 `store(Ordering::Release)` 清零，规则标记以 `compare_exchange(false, true, AcqRel, Acquire)` 竞争唯一写记录者。

`CheckThresholds` 与 `ResetTotalProcessedKeys` 可以并发。清零与累加各自是原子的，但二者之间没有代际屏障：重置附近到达的某次累加可能被计入清零前或清零后。这适合“无数据竞争的滚动累计”，不提供某个精确逻辑批次的线性化快照；调用者必须在扫描轮次边界控制 reset 时机。Rust 测试只证明并发操作安全完成，并不证明所有 interleaving 下的业务总和固定。

多个响应或请求可以同时发现超限，但只有首次 CAS 成功者调用 `Manager::markRunaway` 和可选 `markQuarantine`。后续调用仍按设置返回动作/错误，因此“副作用一次”不等于“只有一个调用者观察到 Kill”。

`Checker` 自身不创建线程、定时器或任务，也没有显式关闭操作。它通过可克隆的 `Manager` 保持共享管理状态存活；当 Session、KV 请求及 Cop worker 持有的最后一个 `Arc` 释放时，Checker 自然析构。记录的异步刷盘生命周期属于 Manager/外部驱动，不由本文件等待。

## 与 Go 版本的对应关系

直接基线是 `pkg/resourcegroup/runaway/checker.go`，回归基线是同目录 `checker_test.go`：

- Rust 保留了 Go 的字段划分、三个 convict 的检查顺序、watch 动作回退、watch 优先于规则、阈值顺序、processed keys 累计、CAS 单次记录、设置一致后再 quarantine，以及 Exact/Similar/Plan 标识选择。
- Rust 用 `Timestamp = i64` 微秒值代替 `time.Time`，用本地 `CopRequest`/`RUDetails`/`Error` 代替 TiKV client 与 protobuf 类型；Session/Store 两层适配器负责转换。耗时原因字符串因此记录微秒整数，而 Go 使用 RFC3339 时间文本；RU 原因也使用数值和，而非 Go `RUDetails.String()`。
- Go 的 nil `*Checker` 方法是安全 no-op；Rust 以 `Option<Checker>`/trait object 表达“没有检查器”，具体 `Checker` 方法不接受空对象。这是类型层面的差异，不是缺少 nil 分支。
- Go `DeriveChecker` 会增加 `RunawayCheckerCounter` 并记录资源组查询警告；Rust 当前只做派生条件判断，没有对应指标与日志。Go `markRunaway` 也增加按 group/match/action 标记的指标，Rust只向 Manager 入队。
- Go `CheckThresholds` 含 `checkThresholds` failpoint；Rust没有该故障注入，但 Rust 独立测试通过显式时间/keys/错误输入覆盖相同行为分支。
- Rust 的乘法、加法和 watch TTL 使用饱和运算，属于额外的整数溢出保护。Rust `BeforeCopRequest` 只在剩余毫秒严格大于 0 时写超时字段。
- Go 在 `CheckThresholds` 中先 `atomic.AddInt64` 再 Load；Rust用一次 `fetch_add(...).saturating_add(process_keys)` 得到本次观察值。并发下两者都保证原子更新，但 reset 竞争时读到的具体累计时序不承诺完全一致。
- Go `CheckRuleKillAction` 的入口还保留 watch-only 条件判断；Rust没有 settings 时直接返回空/false。对当前规则 Kill 输出等价，但 Rust函数不承担 watch-only 动作查询，watch 动作应通过 `CheckAction` 读取。

Rust `checker_test.rs` 明确覆盖：阈值优先级和两次 5 keys 累加至 10、watch Kill/CoolDown/SwitchGroup、无效目标组、原错误保留与 Kill 覆盖、quarantine 内容、convict 类型、派生提前返回、轮询 Kill 只首次标记，以及并发 reset/check。Go 测试额外直接固定 nil receiver 安全和 50 个 goroutine 下 CAS 仅入队一次；Rust 的 CAS 语义由实现和单条记录断言覆盖，但目前没有等量的多线程 CAS 压测。

## 扩展指南

- 新增阈值类型时，应同时修改 `Checker` 字段、`NewChecker` 快照、`exceedsThresholds` 的明确优先级、原因格式和 `CheckThresholds` 输入；在独立 `checker_test.rs` 增加恰好等于阈值、低于阈值、与其他阈值同时命中的用例，并对照 Go 同名实现。
- 新增动作时，必须审查 `BeforeExecutor`、`BeforeCopRequest`、`CheckAction` 和 `markRunaway` 四处；还要同步 `typed_runaway_checker.rs` 与 `store/driver/runaway_adapter.rs` 的枚举转换，否则动作可能在 crate 内被识别却在执行链丢失。
- 修改 watch 匹配顺序或 convict 类型时，同步更新 `getSettingConvictIdentifier`、Manager 的 watch key 语义和 `setting_convict_identifier_matches_watch_type`；顺序变化会改变多个 watch 同时命中时的最终动作。
- 修改并发字段时保持“共享前写 watch 状态、共享后只原子更新规则状态”的生命周期。不应把 `BeforeExecutor` 改成可在 worker 间并发调用，除非把 watch 标记/动作改成同步状态并证明只记录一次。
- 若要求 reset 与响应批次严格隔离，需要引入代际计数或由调用层串行化，不能仅调整 Atomic ordering。新增并发测试应放在 `checker_test.rs`，不要内嵌到生产文件。
- 修改 deadline 错误识别时，应避免依赖易变的人类可读字符串，优先在 Session/KV/Store 适配层传递结构化错误；同步覆盖原错误保持、deadline Kill 和非 Kill 动作。
- 补齐 Go 指标/日志时应在 `DeriveChecker` 和单次 CAS 成功路径计数，避免每个 Cop 响应重复上报；同时确认 crate 依赖和非 Windows 构建边界，而不是只在现有 `cfg(windows)` 清单下增加代码。
- 调整 quarantine 行为时必须保留目录设置一致性检查和 watch 优先不重复隔离的不变量，并同步 `manager.rs`/`manager_test.rs` 中队列容量、即时本地生效和持久化生命周期验证。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/resourcegroup/runaway` 确认目标、Go 对照、crate 入口和独立测试均已索引。
- 目标源码：`rustcodegraph node --file pkg/resourcegroup/runaway/checker.rs --offset 1 --limit 1200` 覆盖全部 424 行；`query` 精确确认 `NewChecker`、`DeriveChecker`、`BeforeExecutor`、`BeforeCopRequest`、`CheckAction`、`CheckRuleKillAction`、`CheckThresholds`、`exceedsThresholds`、两类 mark 方法和 reset 的 Rust/Go 定义。
- 调用图：`rustcodegraph explore "pkg/resourcegroup/runaway/checker.rs RunawayChecker check_threshold mark_candidate identify action switch_group"` 给出 Session、KV、Store/Cop 入口及内部 Manager 调用。精确 `callers` 对 Rust 关联函数未产出完整结果，随后使用 `rg -n` 限定 Rust 文件并以 RustCodeGraph `node --file` 阅读实际调用点，避免把索引缺边当作无调用。
- Rust 直接证据：`pkg/resourcegroup/runaway/lib.rs`、`manager.rs`、`checker_test.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`、`typed_runaway_checker.rs`、`pkg/store/driver/runaway_adapter.rs`、`pkg/resourcegroup/checker.rs` 和 `pkg/store/copr/coprocessor.rs`。
- Go 对照证据：`pkg/resourcegroup/runaway/checker.go` 与 `checker_test.go`，覆盖构造、动作优先级、阈值/错误、nil receiver、CAS、并发 reset 和 convict 选择。
- crate 边界：`pkg/resourcegroup/runaway/Cargo.toml` 的 package、lib path、Go porting metadata 及条件依赖；目标包不存在 `doc.go`，包级 Rust 说明来自 `lib.rs`。
- 本任务只新增本文档，不修改运行时代码，按计划不运行 Cargo。交付验证使用任务指定的 11 章节结构命令，并人工复核文档回答了文件存在目的、运行主链、安全扩展点和当前未接线边界。
