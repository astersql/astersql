# `pkg/util/memory/global_arbitrator.rs`

## 文件定位

本文件是 `astersql-util-memory` crate 的进程级全局内存仲裁门面。模块由 [`lib.rs`](./lib.rs) 以 `pub mod global_arbitrator` 暴露，核心算法不在这里，而在 [`arbitrator.rs`](./arbitrator.rs) 的 `MemArbitrator` 中；本文件负责把核心仲裁器接到服务器配置、运行时内存采样、堆剖析和状态持久化上。

生产主链有三类入口。会话创建在 `pkg/session/runtime/session.rs` 调用 `InitializeGlobalMemArbitratorMode` 应用默认/持久化系统变量；`pkg/session/runtime/control.rs` 通过 `SetGlobalMemArbitratorWorkMode`、`SetGlobalMemArbitratorSoftLimit` 和 `SetGlobalMemArbitratorLimit` 响应配置变化，并通过 `GlobalMemArbitrator` 为语句建立仲裁上下文；`pkg/util/servermemorylimit/servermemorylimit.rs` 每 100ms 调用 `HandleGlobalMemArbitratorRuntime` 推进运行时采样。`tracker.rs`、GC tuner、内存告警等消费者则通过 `GlobalMemArbitrator` 或 `UsingGlobalMemArbitration` 判断是否接入全局策略。

## 核心职责

1. 用 `GlobalState`/`state()` 管理单进程唯一的 `MemArbitrator`、配置原文、启用状态和运行时辅助对象。
2. 把工作模式文本解析为 `WorkMode`，在第一次启用时惰性创建仲裁器，恢复先前调优状态，并启动其 10ms 自动运行循环。
3. 按 Go 的解析顺序处理软限制：特殊值 `"0"`、`"auto"`，随后尝试无符号整数，再尝试 `(0, 1]` 比例，非法值退回禁用。
4. 串行采样进程内存，更新核心仲裁器的风险状态，触发堆剖析，并统计运行时更新及状态写入成败。
5. 用 `RuntimeMemStateRecorder` 对 `mem-state.v1.json` 做原子替换、回读和限频更新，使内存放大率与中型池建议容量可跨进程恢复。
6. 提供独立测试所需的全局状态、采样器和 profiler 注入/清理接口；测试实现位于独立的 `global_arbitrator_test.rs` 与 `global_arbitrator_3_aster_unit_test.rs`，没有嵌入生产源文件。

## 主要符号

- `DefaultGlobalMemArbitratorModeName: &str = "priority"`：服务器默认工作模式；`InitializeGlobalMemArbitratorMode` 只在用户尚未显式配置模式时应用它。
- `MemArbitratorStateDir(log_filename, temp_dir, port) -> PathBuf`：日志文件有非空父目录时使用 `<log-dir>/mem_arbitrator`，否则使用 `<temp-dir>/mem_arbitrator-<port>`。
- `HeapProfileRuntime`：将堆剖析器抽象成 `reset_trigger_state`、`should_check`、`try_capture` 三个线程安全回调；真实实现是 `heap_profile.rs` 的 `HeapProfileCollector`。
- `parse_soft_limit(&str) -> (i64, f64, SoftLimitMode)`：返回绝对字节数、比例和模式三元组。整数必须大于 1 且不超过 `i64::MAX`；字符串 `"1"` 按浮点比例 1.0 接受。
- `GlobalState`/`state()`：私有进程单例。其字段分为仲裁器与配置、运行时采样与持久化、计数器、串行化互斥量和 profiler 六组。
- `SetGlobalMemArbitratorWorkMode`：模式切换的主入口，返回“核心模式是否实际改变”。未知文本经 `WorkMode::from_text` 退回 `Disable`，但配置原文仍保留。
- `GlobalMemArbitrator() -> Option<Arc<MemArbitrator>>`：仅在核心存在且模式非 `Disable` 时返回共享句柄；`UsingGlobalMemArbitration` 则读取单独的 `enabled` 原子标志，覆盖“正在从禁用切到启用”的窗口。
- `SetGlobalMemArbitratorLimit`/`AjustGlobalMemArbitratorLimit`：保存服务器限制并同步给已启用核心；`Ajust` 拼写为兼容 Go API 而保留。
- `HandleGlobalMemArbitratorRuntime`：运行时周期处理入口，负责采样、风险更新、profiler、状态落盘和三个本地计数器。
- `GlobalArbitratorMetrics`/`GlobalMemArbitratorMetrics`：暴露 `runtime_updates`、`record_success`、`record_failure` 的快照，不等同于 Go 文件中完整的 Prometheus 指标集。
- `RuntimeMemStateRecorder`：持有目录、固定版本文件路径、最近成功状态和最近记录时间；`store`、`load`、`persist_pool_medium_if_changed`、`persist_magnif_if_decreased` 是其主要方法。
- `SetupGlobalMemArbitratorForTest`、`CleanupGlobalMemArbitratorForTest`、`SetRuntimeMemStatsSamplerForTest`、`SetHeapProfileRuntimeForTest`：测试生命周期接口；`GLOBAL_TEST_LOCK` 用于串行保护共享单例测试。

## 执行流程

模式初始化/切换流程如下：

1. `InitializeGlobalMemArbitratorMode` 持有 `mode_initialization`，仅当 `mode_configured == false` 时调用 `SetGlobalMemArbitratorWorkMode`，因此之后显式选择的 `disable` 不会被默认 `priority` 覆盖。
2. `SetGlobalMemArbitratorWorkMode` 先标记模式已配置；文本未变化直接返回 `false`。否则更新原文并取得仲裁器槽位的写锁。
3. 槽位为空时读取全局服务器配置，选择状态目录，创建默认 `HeapProfileCollector` 和 `RuntimeMemStateRecorder`，尝试读取 `mem-state.v1.json`；服务器限制为 0 时使用 `GetMemTotalIgnoreErr`。随后 `NewMemArbitrator(limit)`，并从 JSON 的 `magnif` 与 `pool-medium-cap` 恢复核心状态。加载错误被当作“无可恢复状态”，不会阻止初始化。
4. 目标为 `Disable` 时，先修改核心模式，再清除 `enabled`，设置 `runtime_reset`，供下一次运行时 tick 重置 profiler 和本地指标。
5. 从 `Disable` 启用时，先置 `enabled = true`，再同步实际限制和已保存的软限制，启动 10ms 自动循环，最后设置目标模式。非禁用模式间切换只更新工作模式。

运行时 tick 流程如下：

1. `HandleGlobalMemArbitratorRuntime` 用 `runtime_handler.try_lock()` 保证同一时刻只有一个 handler；重入或并发 tick 直接跳过而不等待。
2. 若观察到 `runtime_reset`，重置 profiler 触发状态和三个本地计数器；若全局仲裁器禁用则结束。
3. 调用可替换的 `runtime_sampler`，把结果交给 `MemArbitrator::HandleRuntimeStats`，增加 `runtime_updates`，然后按 profiler 的 `should_check` 结果尝试抓取。
4. 当风险状态本轮首次从否变为是、软限制为 `Auto`、已分配配额大于 0 且堆分配超过配额时，生成含 `last-risk`、`magnif`、`pool-medium-cap` 的版本 1 JSON。`magnif` 为 `heap_alloc * 1000 / quota + 100`，上限 10000。
5. 每轮还尝试持久化变化后的中型池容量以及降低后的放大率；写入结果只更新成功/失败计数，不向调用者传播。

## 数据与状态

`GlobalState` 由 `OnceLock` 惰性创建，进程内不销毁。`arbitrator`、配置文本、采样器、recorder 和 profiler 使用 `RwLock`，使读取者可以并发取得克隆或函数指针；核心仲裁器通过 `Arc` 在 session/tracker 等消费者间共享。`enabled`、`mode_configured`、`runtime_reset`、服务器限制和计数器使用原子变量；代码分别使用 `SeqCst` 或 Acquire/Release/AcqRel 表达跨线程可见性。

持久化文件固定为 `<base-dir>/mem-state.v1.json`。当前 JSON 字段包括数字版本 `version: 1`、最后风险快照 `last-risk.heap/quota`、千分制放大率 `magnif` 和建议中型池容量 `pool-medium-cap`。初始化仅从当前固定文件名加载，不扫描或迁移其它版本；缺失文件且目录存在时返回 `Ok(None)`，目录本身不存在则保留 I/O 错误。

`RuntimeMemStateRecorder::last_state` 只在成功 `store` 或成功 `load` 后更新，因此写失败不会污染内存中的最近成功快照。中型池容量相同、容量非正、或距上次成功写入不足 10 秒时不会再次写入；放大率只有严格下降时才持久化，且两个更新都会保留已有 `last-risk`。

## 依赖与调用关系

上游调用关系由源码检索确认：

- `pkg/session/runtime/session.rs` → `InitializeGlobalMemArbitratorMode`：会话初始化后应用服务器默认/系统变量模式。
- `pkg/session/runtime/control.rs` → 模式、软限制、服务器限制 setter；同文件多个语句生命周期入口以及 `pkg/session/runtime/planning.rs` → `GlobalMemArbitrator`。
- `pkg/util/servermemorylimit/servermemorylimit.rs` → `HandleGlobalMemArbitratorRuntime`：100ms 监控循环中的周期 tick；`pkg/util/servermemorylimit/lib.rs` 对该函数与 `UsingGlobalMemArbitration` 做再导出。
- `pkg/util/memory/tracker.rs` → `GlobalMemArbitrator`/`UsingGlobalMemArbitration`：把 tracker 接到根池和预算仲裁。
- `pkg/util/gctuner/memory_limit_tuner.rs`、`pkg/util/memoryusagealarm/memoryusagealarm.rs` → `UsingGlobalMemArbitration`：在全局仲裁启用时调整各自策略。

下游依赖包括 `arbitrator.rs` 的构造、模式/限额、自动循环、运行时风险、池移除与状态恢复 API；`meminfo.rs::GetMemTotalIgnoreErr`；`utils.rs::SampleRuntimeMemStats`；`heap_profile.rs::HeapProfileCollector`；`config_crate::get_global_config`；以及 `serde_json`、标准库文件系统/同步/时间 API 和 `tempfile::NamedTempFile`。`Cargo.toml` 声明 crate 名为 `astersql-util-memory`、库入口为 `lib.rs`，包含空的 `mem-arbitrator` feature；本文件当前逻辑没有受该 feature 的 `cfg` 控制。

## 错误处理与边界

- 锁中毒通过 `expect` 立即 panic；该文件把全局同步结构损坏视为不可恢复的进程级错误。
- 模式文本未知时由 `WorkMode::from_text` 映射为禁用；软限制非法、整数为 0/1（除 `"1"` 可按比例解析）、超过 `i64::MAX`、比例越界时均禁用软限制。
- 初始化加载状态使用 `recorder.load().ok().flatten()`，因此文件缺失、目录缺失、JSON 损坏和其它 I/O 错误都不会阻止创建核心，但也不会在此处暴露诊断。
- 恢复 JSON 不校验结构版本字段；缺少或类型错误的 `magnif`/`pool-medium-cap` 以 0 代替。文件名承担版本选择，内容兼容性由调用方约定。
- `RuntimeMemStateRecorder::store/load` 保留真实 `io::Result`；JSON 编解码错误转为 `io::Error::other`。写入通过同目录临时文件 flush 后 persist/rename，避免读到部分 JSON，但没有显式 `fsync` 文件或目录的断电持久性保证。
- 周期 handler 有意吞掉状态记录错误，只累加 `record_failure`；时间早于 Unix epoch 时按 0 处理，毫秒值饱和到 `i64::MAX`。
- `SetGlobalMemArbitratorLimit` 接受 `i64`，同步时负值压到 0；初次构造时非零负值会直接传给 `NewMemArbitrator(i64)`，调用方应传入有效非负限制。

## 并发与资源生命周期

工作模式初始化用 `mode_initialization` 串行化默认值判定；仲裁器槽位写锁保证单例只创建一次。普通消费者取得 `Arc<MemArbitrator>` 后不依赖槽位锁继续工作。`enabled` 在从禁用切换时先于核心模式置位，符合 Go 注释中“切换期间也应返回 true”的约束；禁用时顺序相反，先改核心再清标志。

`runtime_handler.try_lock` 防止周期回调与 profiler 回调重入：测试中的 profiler 会递归调用 handler，第二次调用立即返回，证明不会死锁。recorder 自己的 `last_state: Mutex<_>` 覆盖写入及内存快照更新，使克隆出的 recorder 共享一致状态；`last_record_unix_milli` 用原子值执行限频。

仲裁器从首次模式设置开始存活到进程结束，或由测试专用 `CleanupGlobalMemArbitratorForTest` 停止自动循环并清空槽位。切到普通 `Disable` 不销毁核心，而是停止暴露它、设置重置信号；再次启用可复用核心并重启自动循环。测试 setup 会先停止旧循环、删除陈旧状态文件、安装限制为 0 的新核心并清零辅助状态，测试必须持有 `GLOBAL_TEST_LOCK` 以免相互污染。

## 与 Go 版本的对应关系

直接对照文件是 [`global_arbitrator.go`](./global_arbitrator.go)。两版共同保留了默认模式、模式/软限制原文、全局仲裁器访问、启用标志、运行时 handler 串行化、状态目录规则、`mem-state.v1.json` 临时文件原子替换，以及 `AjustGlobalMemArbitratorLimit` 的历史拼写。`parse_soft_limit` 明确复制 Go 的“先 uint、后 float”顺序，所以 `"1"` 作为比例而非绝对 1 字节。

Rust 版的局部接线并非逐字段同构：Go 的 `globalArbitrator.metrics` 会向指标包报告等待任务、配额、根池和执行计数，Rust 本文件只维护三个可查询的本地计数；Go 的自动循环传入 GC、运行时统计更新和日志动作，Rust 调用核心的 `StartAutoRun(Duration::from_millis(10))`。Rust 还提供 `InitializeGlobalMemArbitratorMode`，用 `mode_configured` 防止服务器默认值覆盖显式模式。

状态方面，Go 的 recorder 接受强类型 `RuntimeMemStateV1`，Rust 使用 `serde_json::Value`；Rust handler 在首次自动风险时构造状态，并额外把中型池变化和放大率下降持续落盘。Rust 的 `load` 会同步 `last_state`，且失败写不会覆盖它，这些行为由独立 Rust 回归测试覆盖。以上差异应视为当前实现事实；不能据此推断 Go 中未出现的 Rust 行为已由其它组件完全等价替代。

## 扩展指南

- 增加工作模式或配置文本时，应同时修改 `arbitrator.rs::ArbitratorWorkMode::from_text`、本文件的切换分支、session 系统变量校验，并在 `global_arbitrator_test.rs` 增加显式/默认配置与重入场景；还要核对 Go 同名 setter 的回退语义。
- 修改软限制语法时，优先改 `parse_soft_limit`，保留“整数优先”的兼容顺序，并扩展 `global_arbitrator_3_aster_unit_test.rs::soft_limit_parsing_matches_go_fallbacks`，覆盖边界 0、1、`i64::MAX`、溢出、NaN/无穷及非法文本。
- 演进状态格式时，不应原地改变 `v1` 语义；新增版本常量/文件名、明确迁移与回退策略，并覆盖旧文件共存、损坏文件、目录缺失、写失败不污染 `last_state` 和原子替换。当前恢复代码还需要同步字段读取与版本验证。
- 增加周期动作应接入 `HandleGlobalMemArbitratorRuntime` 的单持有者区间，评估 100ms 外部 tick 的延迟预算，避免在持有 `runtime_handler` 时阻塞或再次获取顺序相反的锁。可注入采样器和 `HeapProfileRuntime` 适合做确定性测试。
- 改变生命周期时需保持 `enabled` 与核心模式之间的时序不变量，并确保所有自动循环在测试 cleanup 中停止。测试仍应放在同目录独立 `*_test.rs` 文件，使用 `GLOBAL_TEST_LOCK`，不要写入生产文件。
- 性能风险集中在每 tick 的采样、JSON 写盘和锁竞争；兼容风险集中在公开的 Go 风格函数名、未知值回退、状态文件名/字段，以及 session/tracker 对 `Option<Arc<_>>` 的禁用语义。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件且目标文件已索引；`files --filter pkg/util/memory/global_arbitrator.rs` 报告 59 个符号；`query SetGlobalMemArbitratorWorkMode --kind function`、`query HandleGlobalMemArbitratorRuntime --kind function` 和 `query RuntimeMemStateRecorder` 均定位到本文件及 Go 对照。精确 `callers`/`callees` 查询未返回可用输出，因此没有据此声称调用边，而用下述源码检索补证。
- 已读生产路径：`pkg/util/memory/global_arbitrator.rs`、`pkg/util/memory/arbitrator.rs` 的被调用符号、`pkg/util/memory/lib.rs`、`pkg/util/memory/Cargo.toml`、`pkg/util/memory/global_arbitrator.go`，以及直接调用位置 `pkg/session/runtime/session.rs`、`pkg/session/runtime/control.rs`、`pkg/session/runtime/planning.rs`、`pkg/util/servermemorylimit/servermemorylimit.rs`、`pkg/util/memory/tracker.rs`、`pkg/util/gctuner/memory_limit_tuner.rs`、`pkg/util/memoryusagealarm/memoryusagealarm.rs`。
- 已读独立 Rust 测试：`pkg/util/memory/global_arbitrator_test.rs` 和 `pkg/util/memory/global_arbitrator_3_aster_unit_test.rs`。它们验证状态目录、惰性恢复、profiler 重入/重置、精确版本文件名、失败写保留旧状态、中型池与放大率持久化、陈旧状态清理、默认模式不覆盖显式禁用、软限制回退和运行时风险更新。
- 已读 Go 对照测试入口：`pkg/util/memory/heap_profile_test.go` 及 `pkg/util/memory/global_arbitrator.go` 中 recorder/handler 语义；调用边另以 `rg` 对上述公开符号在非测试 Rust 文件中的精确出现位置复核。
- 本任务只新增说明文档，不改变 Rust/Go/Cargo 行为，按计划不运行 Cargo。交付前使用任务指定命令校验本文恰有 11 个固定二级标题，并人工复核所有“已支持”结论均可回溯到上述符号或测试。
