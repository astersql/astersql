# `pkg/util/memory/heap_profile.rs`

## 文件定位

本文件属于 `astersql-util-memory` crate，由同目录 [`lib.rs`](./lib.rs) 以 `pub mod heap_profile` 暴露。它不是独立运行的 profiler 服务，而是全局内存仲裁运行时的一项诊断能力：[`global_arbitrator.rs`](./global_arbitrator.rs) 在首次创建 `MemArbitrator` 时构造 `HeapProfileCollector::new_default(<state-dir>/heap_profiles)`，随后 `HandleGlobalMemArbitratorRuntime` 在每次内存采样后通过 `HeapProfileRuntime::should_check` 和 `try_capture` 驱动它。

文件把 `MemArbitrator` 的内存使用快照转换为阈值触发的 pprof 文件和配套 JSON 元数据。生产写入器由 `rpprof` 提供，时间和写入器也可注入以支持同目录独立测试 [`heap_profile_test.rs`](./heap_profile_test.rs)。直接 Go 对照是 [`heap_profile.go`](./heap_profile.go)，文件名、阈值、冷却、保留和元数据协议均以该实现为迁移基准。

## 核心职责

1. 通过 `Snapshot::from_arbitrator` 一次性读取堆分配、堆在用、总内存在用、配额分配、失控内存和限制，避免一次判定中混用不同采样时刻的值。
2. 在内存占限制的 70%、80%、85% 三档抓取诊断快照；低于 65% 时重置一轮阈值状态，达到 90% 后关闭普通抓取，避免 profiler 自身在高风险区继续增加压力。
3. 对“已处于 OOM 风险、配额分配为 0、失控内存大于 0”的诊断盲区，每 30 秒允许一次 95% 紧急抓取；紧急抓取绕过普通的风险/90% 截止检查，但仍服从工作模式启用和正限制条件。
4. 使用同目录私有临时文件写入并持久化 `.pprof`，再原子写入 `.meta.json`；写入过程中遗留的临时文件由 `tempfile` 或下次保留清理处理。
5. 将可识别的 profile/metadata 按同一 basename 分组，只保留最新 10 组，删除孤立元数据与本模块格式的陈旧临时文件，同时保留无法识别的外部文件。
6. 实现 `HeapProfileRuntime`，把 collector 接到全局仲裁器的串行运行时 tick，并用一个互斥状态保护检查节流和触发状态。

## 主要符号

- `LEVELS = [(700, 70), (800, 80), (850, 85)]`：前者是千分比触发线，后者进入文件名和元数据的百分比。`RESET_MILLI = 650`、`CUTOFF_MILLI = 900` 定义一轮触发的迟滞区间。
- `MIN_INTERVAL`、`EMERGENCY_INTERVAL`、`CHECK_INTERVAL`：分别是同级/降级普通抓取的 60 秒冷却、紧急抓取的 30 秒周期和全局调用侧的 1 秒检查节流。更高且尚未尝试的阈值可绕过普通冷却。
- `ProfileWriter`、`ProfileClock`：线程安全、可克隆的注入钩子。生产构造使用真实系统时间和 `rpprof`，测试构造使用确定性时钟/字节写入器。
- `TriggerState`：保存最近普通抓取时间、最近紧急抓取时间、最近限制、最近阈值、已尝试档位位图和普通通道关闭标志。
- `CollectorState`：在 `TriggerState` 外增加 `last_check_at`，整体由 `HeapProfileCollector::state: Mutex<_>` 保护。
- `HeapProfileCollector`：公开 collector 类型，拥有输出目录、时钟、写入器和互斥状态。`new_default`、`new`、`new_with_hooks` 分别服务生产、定制写入器和完整测试注入。
- `Snapshot::from_arbitrator`：从 `MemArbitrator::HeapProfileCounters`、`Limit`、`Allocated`、`OutOfControl` 采样，并用 `multiRatio(limit, 900)` 预计算普通抓取截止值。
- `capture`/`capture_locked`：显式抓取入口及已持锁内部实现。返回值表示写入器是否已被调用，而不是最终两个文件是否都成功持久化。
- `HeapProfileRuntime::{reset_trigger_state, should_check, try_capture}`：全局运行时使用的三项接口；分别重置整套状态、执行一秒节流和进行阈值判定。
- `write_metadata_atomically`、`enforce_retention`：元数据临时写入/替换和目录清理。
- `parse_heap_profile_file_name`：只接受 `<时间>.<70|80|85|95>pct.pprof` 或同 basename 的 `.meta.json`，返回 basename、带固定偏移的时间和是否为 profile。
- `private_permissions`、`create_private_dir`：Unix 下分别使用 `0600` 文件权限和 `0750` 递归目录权限；非 Unix 下使用当前可执行文件的权限模板及普通 `create_dir_all`。

## 执行流程

生产初始化与周期驱动如下：

1. `global_arbitrator.rs::SetGlobalMemArbitratorWorkMode` 首次创建全局仲裁器时，根据日志目录或临时目录选择状态根，并安装 `HeapProfileCollector::new_default(base_dir.join("heap_profiles"))`。
2. `new_default` 构造 `rpprof` 写入闭包；`new_with_hooks` 建立空的 `CollectorState`，随即调用 `enforce_retention` 清理上次运行残留并执行数量限制。
3. 每次 `HandleGlobalMemArbitratorRuntime` 先更新 `MemArbitrator` 的运行时统计，再调用 `should_check`；距上次检查不足一秒时跳过，否则进入 `try_capture`。
4. `try_capture` 在限制非正时退出。限制变化会清空 attempted、紧急时间和 closed；内存比率低于 65% 也清空这三项，从而允许新一轮普通抓取。
5. 若处于特殊 OOM 盲区，且距上次紧急尝试至少 30 秒，则先调用 `capture_locked(..., 95)`。随后比率达到 90% 会把普通通道永久关闭，直到低于 65%、限制变化或外部重置。
6. 普通通道遍历三档阈值，累积所有已达到档位的位图，并选择最高的未尝试档。若本次阈值不高于上次阈值且未过 60 秒则退出；一旦 `capture_locked` 返回 true，会一次性标记所有已经达到的档位，避免从 85% 回头重复抓 70%/80%。

单次 `capture_locked` 的持久化顺序如下：

1. 重新取得 `Snapshot`；禁用模式、限制非正直接返回 false。非 95% 抓取若仲裁器已处于内存风险或 `mem_inuse >= 90% limit` 也返回 false。
2. 在任何目录/文件操作前记录最近抓取时间和阈值。因此初始化失败会进入普通冷却，但由于返回 false，不会消耗 attempted 位。
3. 创建 `0750` 目录和目录内 `.heap-profile.*.tmp`，把临时文件权限设为 `0600`；写入开始前再次记录时间，并由本地时区生成 `<timestamp>.<threshold>pct` basename。
4. 调用 profile writer、`sync_all`，再把临时文件 persist 为 `.pprof`。从写入器被调用开始，无论后续写入、同步或 persist 是否失败，函数都返回 true；未 persist 的临时文件由 `NamedTempFile` 析构删除。
5. profile 成功后计算耗时，生成 version 1 JSON，写入六项状态字段；metadata 失败被忽略，不回滚已完成的 profile。最后执行保留清理并返回 true。

## 数据与状态

`HeapProfileCollector` 的配置字段在构造后不变，运行态集中在单个 `Mutex<CollectorState>` 中。`attempted` 的低三位对应 `LEVELS`，当一次跳升直接达到 85% 时，成功启动写入会把 70/80/85 三位全部置位。`closed` 只关闭普通路径，不关闭 95% 紧急路径。`reset_trigger_state` 同时清除 `last_check_at` 和触发状态，供全局工作模式重置后立即重新评估。

每次落盘的 basename 为本地固定偏移时间 `%Y-%m-%dT%H-%M-%S%z` 加合法阈值，例如 `2026-08-14T10-00-00+0800.85pct`。profile 后缀为 `.pprof`，metadata 后缀为 `.meta.json`。元数据包含 `start_time`、`version: 1`、`threshold_pct`、`duration_ms`，以及 `state` 下的 `heap_alloc_bytes`、`heap_inuse_bytes`、`mem_inuse_bytes`、`quota_alloc_bytes`、`out_of_control_bytes`、`limit_bytes`。

保留逻辑以合法 basename 为组键，profile 是组存在的必要条件，metadata 可缺失。孤立 metadata 会被删除；孤立 profile 会保留。合法组按解析后的时间升序、再按 basename 排序，超过 `MAX_GROUPS = 10` 时删除最旧组的现有文件。未知阈值、非法时间、其它后缀和普通 `manual.pprof` 不归本模块管理。

## 依赖与调用关系

上游调用链由 RustCodeGraph 和源码共同确认：

- `pkg/util/memory/lib.rs` → `pub mod heap_profile`，并在 `#[cfg(test)]` 下把独立 `heap_profile_test.rs` 接入 crate 测试。
- `pkg/util/memory/global_arbitrator.rs::SetGlobalMemArbitratorWorkMode` → `HeapProfileCollector::new_default`，把 collector 放入 `Arc<dyn HeapProfileRuntime>` 槽位。
- `pkg/util/memory/global_arbitrator.rs::HandleGlobalMemArbitratorRuntime` → `reset_trigger_state`（模式重置时）以及 `should_check` → `try_capture`（采样更新后）。该 handler 自身由 `runtime_handler.try_lock` 串行化。
- `pkg/util/memory/heap_profile_test.rs` 直接调用公开构造器、`capture`、`enforce_retention`、`parse_heap_profile_file_name`，并通过 trait 方法验证运行时行为。

主要下游是 `arbitrator.rs::MemArbitrator` 的计数器、限制、模式与风险 API，crate 内 `calcRatio`/`multiRatio`，以及 `chrono`、`serde_json`、`tempfile`、标准库文件系统/同步/时间 API。生产 profile 编码依赖 Cargo 中固定 tag `rpprof v0.18.1`，使用其 allocator sampler、heap report、pprof 转换和 protobuf `Message::encode`；`Cargo.toml` 还明确声明 `chrono`、`serde_json`、`tempfile`。本文件不受空的 `mem-arbitrator` feature 条件控制。

## 错误处理与边界

- 禁用模式、限制不正、普通路径已进入内存风险或达到 90% 截止时，`capture_locked` 返回 false 且不会调用 writer；紧急阈值只绕过后两项风险检查。
- 默认 writer 在 `rpprof::alloc::is_active() == false` 时返回 `io::Error`。生成报告期间先停止 sampler，并用局部 `ResumeSampling` 的 `Drop` 无论成功或错误都重新启动，避免报告分配重入 recorder 的锁。
- 目录创建、临时文件创建或权限设置失败发生在 writer 之前，返回 false；最近时间/阈值已经记录，所以相同或更低档位需等 60 秒，但更高档位仍可尝试。writer 一旦被调用，即使失败也返回 true 并消耗达到的阈值，这是防止每个 tick 重试昂贵失败的明确约定。
- profile persist 成功后，metadata 的序列化、同步或 persist 错误会被忽略，可能形成无 metadata 的合法 profile；保留逻辑允许这种组存在。metadata 单独存在则会在清理时删除。
- `duration_since` 遇到时钟倒退以 0 代替，毫秒值饱和到 `i64::MAX`；节流和冷却在时钟倒退时同样把差值视为 0，因此会继续抑制尝试。
- `parse_heap_profile_file_name` 要求阈值文本规范化：`085` 即使可解析也被拒绝，90 等未登记阈值也被拒绝；时间必须包含可由 `chrono` 解析的固定偏移。
- 目录遍历、entry 类型查询和删除失败均按 best effort 忽略；目录不存在时 retention 直接返回。锁中毒则通过 `expect("heap profile lock poisoned")` panic。

## 并发与资源生命周期

`HeapProfileCollector` 可作为 `Arc<dyn HeapProfileRuntime + Send + Sync>` 跨线程共享。`ProfileClock` 和 `ProfileWriter` 自身要求 `Send + Sync`；所有检查、阈值判定和完整 profile 写入都在 `state` 互斥锁持有期间完成。因此同一 collector 不会并发生成 profile，也不会在写入中途被 `reset_trigger_state` 改写，但一次符号解析、编码、同步和 metadata 写入会阻塞其它调用者。外层全局 handler 还有 `try_lock` 串行保护，正常生产路径形成双层但同序的串行化。

默认 writer 的 sampler 生命周期由外部启动；本文件只在生成 report 的临界区临时 stop，并通过 RAII 恢复 start，不负责最终关闭 sampler。profile 临时文件由 `NamedTempFile` 拥有，只有 `persist` 成功才转移为最终文件；失败/提前返回时析构清理。metadata 采用相同的临时文件所有权和同目录 rename。collector 构造时以及每次 profile 成功后执行 retention，因而陈旧临时文件和超额组可跨进程启动恢复清理。

Unix 权限是安全协议的一部分：目录 `0750`，profile 和 metadata `0600`。非 Unix 分支没有等价的 mode 位，而是沿用当前可执行文件权限；扩展跨平台行为时不能假设数值权限一致。

## 与 Go 版本的对应关系

[`heap_profile.go`](./heap_profile.go) 是逐项对照文件。两版共同使用 70/80/85% 普通阈值、65% 重置线、90% 安全截止、95% 紧急阈值、60 秒普通冷却、30 秒紧急周期、1 秒检查间隔和 10 组保留上限；触发位图、限制变化重置、最高未尝试档选择、writer 开始后即视为一次尝试、原子临时文件、metadata schema、文件命名验证和 orphan 清理语义均一致。

实现层的主要差异是 profile 来源：Go 用 `runtime/pprof.Lookup("heap").WriteTo(w, 0)`，Rust 用 `rpprof` allocator sampler 生成 protobuf pprof，并显式暂停/恢复 sampler 以避免锁重入。Go collector 的可变字段由运行时 handler 的串行锁保护；Rust 把 `lastCheckAt` 和 trigger 收进自身 `Mutex`，使 trait 对象本身满足线程安全。Go 用可空函数字段并在 `currentTime` 回退到全局 `now()`；Rust 构造后始终持有非空 `Arc<Fn>`。

Rust 独立测试名称带 `go_merge_28`，覆盖 Go 测试的关键意图：逐级触发与重置、最高阈值、冷却、设置失败重试、writer 失败消耗档位、90% 截止、限制变化、禁用模式、紧急周期、真实 pprof、metadata 和 retention。当前 Rust 测试还直接验证 `rpprof` protobuf 可解码且含 sample，这是 Rust 特有实现证据，不应反推 Go 采用同一编码库。

## 扩展指南

- 新增或修改阈值时，必须同步 `LEVELS`、`parse_heap_profile_file_name` 的合法集合、位图宽度假设、Go 的 `heapProfileLevels`/`isHeapProfileThreshold`，以及两侧独立测试。阈值还进入持久文件名，改变它具有运维工具兼容风险。
- 修改冷却/截止策略时，应在 `HeapProfileRuntime::try_capture` 保持“限制变化与低水位可重开、90% 只关闭普通路径、紧急路径周期独立”的状态机，并补充跨阈值跳升、降级、时钟倒退和写入失败测试。
- 演进 metadata 时，应保留 `version` 并明确新旧 reader 的兼容策略；若要求 profile 与 metadata 强一致，需要重新设计当前“profile 优先、metadata best effort”的提交协议和 retention 行为。
- 替换 profiler 后端应集中修改 `new_default` 的 `ProfileWriter`，评估采样器启停、分配重入、符号解析延迟、文件体积和 protobuf 兼容性；确定性逻辑测试继续使用 `new_with_hooks`，真实格式测试保留在独立 `heap_profile_test.rs`。
- 修改并发模型时要注意 `capture_locked` 当前在持有状态锁时执行全部 I/O。若缩小锁范围，必须为 concurrent reset、同 basename 冲突、attempted 提交时机和 writer 并发增加测试，不能只为降低阻塞而破坏单写者不变量。
- 修改文件清理时只应删除能由 `parse_heap_profile_file_name` 或 `is_heap_profile_temp` 明确认领的文件，避免误删管理员放入同目录的诊断文件；测试需覆盖未知后缀、未知阈值、孤立两侧、同时间排序和删除失败。
- 测试继续放在同目录独立 `heap_profile_test.rs`，不要把测试模块嵌入生产文件。性能风险集中在持锁生成/同步 profile，正确性风险集中在状态机和失败返回语义，兼容风险集中在文件名、JSON 字段和 pprof 编码。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/memory` 确认目标 Rust、Go、测试与模块入口均已索引。`explore "pkg/util/memory/heap_profile.rs HeapProfileCollector ..."` 和 `node --file ... --offset ...` 用于核对目标文件全部 414 行及 52 个符号。
- RustCodeGraph 调用证据：针对 `global_arbitrator.rs HeapProfileRuntime should_check try_capture reset_trigger_state HeapProfileCollector new_default` 的 `explore` 显示 `HandleGlobalMemArbitratorRuntime` 调用三项 trait 方法；源码节点进一步确认 collector 在 `SetGlobalMemArbitratorWorkMode` 初始化，并在运行时统计更新后执行 `should_check` → `try_capture`。精确 `callers`/`callees` 命令未产生可用输出，本文未用其空结果扩张结论。
- 已读生产/配置路径：`pkg/util/memory/heap_profile.rs`、`global_arbitrator.rs` 的初始化与 handler 调用段、`lib.rs`、`Cargo.toml`、Go 对照 `heap_profile.go`。Cargo 证据确认 crate 名、`lib.rs` 入口及 `chrono`、`serde_json`、`tempfile`、带 tag 的 `rpprof` 依赖。
- 已读独立测试：Rust `pkg/util/memory/heap_profile_test.rs` 全部 431 行；Go `pkg/util/memory/heap_profile_test.go` 全部 440 行。它们共同证明阈值状态机、冷却/截止/重置、失败语义、紧急路径、metadata、权限、保留和全局安装；Rust 测试另证明默认 writer 生成可解码且非空的 pprof。
- 本任务仅新增本文，不改变 Rust、Go、Cargo 或总计划，也按任务要求未运行 Cargo。交付前执行任务指定的结构命令确认文件存在且恰有 11 个固定二级标题，并人工检查所有行为描述均能回溯到上述符号、调用点或测试。
