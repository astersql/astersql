# `pkg/util/execdetails/ruv2_metrics.rs`

## 文件定位

本文件实现语句级 RUv2 指标容器中仍被当前语句 RU 计算使用的最小数据面：是否绕过计量，以及 TiKV Coprocessor 响应字节数。源码注释明确说明旧 RU v2 模型已移除，目前仅保留这两项能力（[`ruv2_metrics.rs`](./ruv2_metrics.rs) 第 16–18 行）。

它并非由根 `pkg/util/execdetails/lib.rs` 直接声明为同级模块，而是被 `pkg/util/execdetails/internal/ruv2/lib.rs` 的私有 `ruv2_metrics` 模块用 `include!("../../ruv2_metrics.rs")` 纳入 `astersql-util-execdetails-ruv2` 子 crate，再由该子 crate `pub use ruv2_metrics::*` 导出。最外层 `astersql-util-execdetails` crate 依赖这个子 crate，并在公开的 `ruv2_metrics` 模块中再次整体重导出。因此应用侧实际使用的路径是 `astersql_util_execdetails::ruv2_metrics::*`。

该模块处于“原始 RU 明细 → 语句级快照 → 语句 RU 计算”的中间层：`tikvutil::RUDetails` 暂存 RPC 收集的原始 `kvrpcpb::Ruv2`，本文件将其中的 `coprocessor_response_bytes` 排空并累加到 `RUV2Metrics`，执行器随后把该累计值当作网络字节证据。直接证据见 `pkg/executor/adapter.rs` 的 `finalizeStatementRUV2Metrics`、`pkg/executor/statement_ru_plan_walk.rs` 的 `TiKVCoprocessorResponseBytes` 读取，以及 `pkg/server/internal/resultset/resultset.rs` 的游标增量同步。

## 核心职责

1. 用 `RUV2Metrics` 保存语句级、可并发更新的旁路标志和 TiKV Coprocessor 响应字节累计值。
2. 用 `UpdateRUV2MetricsFromRUV2` 从一份 protobuf `kvrpcpb::Ruv2` 中只抽取 `coprocessor_response_bytes`，不再保留已废弃 RU v2 模型的其他计数器。
3. 用 `SyncRUV2MetricsFromRUDetails` 调用 `RUDetails::DrainRUV2`，把自上次排空以来的增量转移到语句级容器；重复同步在没有新输入时不会重复累计。
4. 用 `RUV2MetricsFromContext` 提供 Go API 同名的上下文查询门面，优先尝试 `StmtExecDetails`，再回退到独立上下文键。

第 4 项在当前 `astersql-util-execdetails-ruv2` 子 crate 中受桩实现限制：`internal/ruv2/lib.rs` 定义的 `context::Context::value` 与 `StmtExecDetails::getRUV2Metrics` 都恒返 `None`，所以从这个子 crate 重导出的 `RUV2MetricsFromContext` 当前恒返 `None`。完整上下文继承逻辑位于另一个 `execdetails-util` 子 crate（`pkg/util/execdetails/internal/util/lib.rs`），它拥有自己的上下文类型和指标类型；两者不能据名称相同就视为同一实现。

## 主要符号

- `ruv2MetricsKeyType`：零大小上下文键类型。它是公开类型但沿用 Go 命名，主要用于避免与其他 context 值发生键碰撞。
- `RUV2MetricsCtxKey`：上述类型的唯一静态键，供 `RUV2MetricsFromContext` 的回退查询使用。
- `RUV2MetricsFromContext(&context::Context) -> Option<RUV2Metrics>`：先以 `StmtExecDetailKey` 读取 `StmtExecDetails` 并调用 `getRUV2Metrics`，不存在时再按 `RUV2MetricsCtxKey` 读取直接绑定值。返回的是按值的 `RUV2Metrics`；当前子 crate 桩上下文使两条路径都返回 `None`。
- `UpdateRUV2MetricsFromRUV2(Option<&RUV2Metrics>, Option<&kvrpcpb::Ruv2>)`：两个输入任一缺失或指标已旁路时立即返回；否则读取响应字节，非零时以原子加法合并。
- `SyncRUV2MetricsFromRUDetails(Option<&RUV2Metrics>, Option<&tikvutil::RUDetails>)`：空输入或旁路时不排空；正常路径先 `DrainRUV2`，再调用上一函数完成转移。
- `RUV2Metrics { bypass: AtomicBool, tikvCoprocessorResponseBytes: AtomicI64 }`：字段私有，只能经方法访问。`Default` 初始化为“未旁路、零字节”；手写 `Clone` 对两个原子值做时点快照，克隆后不共享后续更新。
- `NewRUV2Metrics() -> RUV2Metrics`：值语义构造器，等价于 `Default::default()`。
- `SetBypass` / `Bypass`：以 `Ordering::Relaxed` 写入、读取旁路开关。
- `AddTiKVCoprocessorResponseBytes`：未旁路时以 `fetch_add` 加上任意 `i64` 增量；源码未禁止负数。
- `TiKVCoprocessorResponseBytes`：以 relaxed load 返回当前累计值。

RustCodeGraph 对本文件识别出 15 个符号；其 callee 图确认 `SyncRUV2MetricsFromRUDetails → UpdateRUV2MetricsFromRUV2 → Bypass`，并确认直接累加方法也调用 `Bypass`。

## 执行流程

正常 RPC/执行完成路径如下：

1. 下游执行过程把原始 `kvrpcpb::Ruv2` 合并进 `tikvutil::RUDetails`；`internal/ruv2/lib.rs::RUDetails::AddRUV2` 会累计 protobuf 的多个原始字段。
2. 语句完成时，`pkg/executor/adapter.rs::finalizeStatementRUV2Metrics` 取得会话的 `RUV2Metrics` 与 `RUDetails`，调用 `SyncRUV2MetricsFromRUDetails`。游标路径由 `pkg/server/internal/resultset/resultset.rs` 在创建 tracker 和报告 delta 时调用同一函数。
3. `SyncRUV2MetricsFromRUDetails` 先检查两个 `Option` 和 `Bypass`。只有正常路径才调用 `DrainRUV2`，所以旁路时待处理原始值仍留在 `RUDetails` 中。
4. `RUDetails::DrainRUV2` 在互斥锁内用默认值替换当前 protobuf 并返回旧值，实现“取走一次”的增量语义。
5. `UpdateRUV2MetricsFromRUV2` 再次检查旁路状态，只读取 `get_coprocessor_response_bytes()`；零值不做原子写，非零值转换为 `i64` 后 `fetch_add`。
6. `pkg/executor/statement_ru_plan_walk.rs` 在语句 RU 计算时读取 `TiKVCoprocessorResponseBytes`，将它作为 `net_bytes` 输入。

直接调用流程更短：调用方可用 `AddTiKVCoprocessorResponseBytes(delta)` 注入已知字节增量；该入口同样服从旁路开关。`pkg/executor/statement_ru_plan_walk_test.rs` 使用此入口构造语句 RU 计算场景。

## 数据与状态

`RUV2Metrics` 只有两个状态单元，均为原子类型，因此共享引用就能修改，无需 `&mut self`：

- `bypass` 默认 `false`。为 `true` 时，两个累加入口均跳过更新；读取已有累计值不受影响。
- `tikvCoprocessorResponseBytes` 默认 `0`。`UpdateRUV2MetricsFromRUV2` 把 protobuf 的无符号响应字节转换为 `i64` 后累加，`AddTiKVCoprocessorResponseBytes` 则直接接受有符号增量。`go_merge_20_test.rs` 明确验证 `-2` 会从既有值中扣减，说明负增量是当前允许行为而非输入校验错误。

所有操作使用 `Ordering::Relaxed`。这里保证单个原子变量的读写和加法不发生数据竞争，但不建立与其他内存状态的 happens-before 关系。旁路检查与随后 `fetch_add` 不是一个原子事务：若另一线程在两步之间切换旁路状态，当前一次更新可能仍按检查时观察到的状态执行。这是现有实现的弱一致性边界。

`Clone` 分别读取旁路值和字节值，再创建两个新原子；这不是跨字段一致快照，且克隆对象与原对象完全独立。`NewRUV2Metrics` 也返回裸值，跨线程共享由上层选择 `Arc<RUV2Metrics>` 完成，例如执行器 adapter 与 resultset tracker。

## 依赖与调用关系

编译边界由两层 Cargo/模块装配构成：

- `pkg/util/execdetails/internal/ruv2/Cargo.toml` 定义实际编译本文件的 `astersql-util-execdetails-ruv2` crate；其依赖 `protobuf`、`prometheus` 和 `astersql-util-resourcegrouptag`。`kvrpcpb::Ruv2` 经后者的 kvproto 绑定重导出。
- `pkg/util/execdetails/Cargo.toml` 定义聚合 crate `astersql-util-execdetails`，通过路径依赖 `execdetails-ruv2` 引入该子 crate；`pkg/util/execdetails/lib.rs` 再把 API 暴露在 `ruv2_metrics` 命名空间。
- 标准库依赖只有 `AtomicBool`、`AtomicI64` 和 `Ordering`。
- `tikvutil::RUDetails` 由 `internal/ruv2/lib.rs` 实现，其 `DrainRUV2` 是同步入口的关键下游调用；内部互斥锁负责 protobuf 累计与排空的互斥。

已核验的 Rust 上游包括：

- `pkg/executor/adapter.rs`：语句结束时同步 pending RUDetails，并读取最终 Coprocessor 响应字节。
- `pkg/executor/statement_ru_plan_walk.rs`：把该指标作为 statement RU 的网络字节输入。
- `pkg/server/internal/resultset/resultset.rs`：游标创建与后续结果批次报告时逐次排空 delta。
- `pkg/distsql/context/lib.rs`：向 distsql 上下文重导出 `RUV2Metrics` 类型。

上下文相关 API 另有同名实现：`pkg/util/execdetails/internal/util/lib.rs` 实现可工作的 `Arc<RUV2Metrics>` 上下文继承与同步。由于根聚合 crate 的 `util` 和 `ruv2_metrics` 分别重导出两个内部 crate，扩展时必须先确认调用路径和类型归属，不能把两套同名符号混接。

## 错误处理与边界

本文件没有 `Result`、显式错误类型或 panic 路径。缺失指标、缺失原始 RU、缺失 `RUDetails`、旁路状态和零字节均采用无副作用提前返回。这个设计让语句收尾和游标上报可以无条件调用同步函数。

需要注意以下边界：

- `SyncRUV2MetricsFromRUDetails` 在旁路时不会调用 `DrainRUV2`。若之后关闭旁路并复用同一 `RUDetails`，旁路期间积累的值仍可能被后续同步转移。`pkg/server/conn_test.rs` 覆盖了关闭旁路后继续计数的行为，但没有单独证明“旁路期间 pending RUDetails 应丢弃”。修改此语义前应先与 Go 行为及调用方生命周期对齐。
- protobuf 字段是无符号值，源码使用 `as i64` 转换。超过 `i64::MAX` 的值会按 Rust 转换规则变为负数；当前没有范围检查。
- 原子 `fetch_add` 对溢出没有业务级防护；该计数应由上层保证处于实际可表示范围。
- `RUV2MetricsFromContext` 的参数是不可空引用，因此没有 Go 版本的 `ctx == nil` 分支；当前内部 Context 又是桩，实际只能返回 `None`。
- 锁中毒只可能发生在本文件下游的 `RUDetails::DrainRUV2`；它使用 `expect("ruv2 lock poisoned")`，因此锁中毒会 panic，而非由本文件恢复。

## 并发与资源生命周期

指标容器自身无堆分配、锁、任务或通道；生命周期由持有者管理。两个字段使用原子操作，允许多个线程通过共享引用累加和读取。实际应用通常用 `Arc<RUV2Metrics>` 把同一语句指标交给执行器、游标 tracker 等组件。

原始数据的生命周期与指标容器不同：`RUDetails` 以 `Mutex<kvrpcpb::Ruv2>` 保存尚未上报的 delta，`DrainRUV2` 在锁内取出并清零；`RUV2Metrics` 则保存语句生命周期内的累计快照。由此形成“锁保护的待转移增量 → 原子累计的长期语句值”的两阶段模型。`pkg/util/execdetails/go_merge_20_test.rs` 和 `execdetails_test.rs` 都验证连续同步两次只累计一次。

`Ordering::Relaxed` 适合独立统计计数，但旁路切换与计数更新只有最终一致的观察语义。若将来要求“开启旁路后绝不再接受任何并发中的更新”或要求多个字段形成一致快照，就需要更强的同步协议，不能仅更换一个 Ordering 后假定已解决。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/execdetails/ruv2_metrics.go`。两版保留一致的公共意图：上下文中查找指标、从原始 RUv2 只汇入 Coprocessor 响应字节、从 `RUDetails` 排空增量、支持旁路以及直接增减/读取字节数。

主要差异如下：

- Go 的 `NewRUV2Metrics` 返回指针，Rust 返回值；Rust 上层需要共享时再包 `Arc`。
- Go 为响应字节使用懒分配的 `atomic.Pointer[ruv2MetricsExtra]`，零值读取不分配；Rust 直接内嵌 `AtomicI64`，对象更简单但始终占用该字段空间。
- Go 的 `RUV2MetricsFromContext` 可处理 nil context，并返回共享指针；Rust 接受非空引用并按值返回。更关键的是，当前编译本文件的 RUv2 子 crate 只提供恒空 Context/StmtExecDetails 桩，因此这个 Rust 入口没有达到 Go 的真实上下文语义；完整 Rust 上下文行为由独立 `execdetails-util` crate 提供。
- Go 把原始字段合并封装在私有 `applyRawCounters` 中；Rust 在 `UpdateRUV2MetricsFromRUV2` 内直接读取并原子累加。
- 两版都在旁路判断之后才 drain，并都只抽取 `CoprocessorResponseBytes`。Go 文件注释明确把连续调用描述为“每次只转移 delta”，Rust 测试已验证相同行为。
- Rust `Clone` 是额外的值快照能力；Go 指针模型没有对应的结构体深拷贝 API。

历史证据显示该文件先由 Rust 移植建立，随后与 Go 一起收敛到“只收集 Coprocessor 响应字节”，并在移除已废弃 RU v2 accounting 后保留当前最小模型；不能从名称推断旧版 RUv2 的其他计数仍受支持。

## 扩展指南

若新增语句级指标字段，最可能需要同步修改：

1. `RUV2Metrics` 字段、`Default`、`Clone`、读取方法与直接累加方法；保持字段私有，避免绕开旁路和原子协议。
2. `UpdateRUV2MetricsFromRUV2` 的抽取逻辑，以及 `internal/ruv2/lib.rs::RUDetails::AddRUV2`/`DrainRUV2` 是否已保存相应原始字段。
3. 消费方 `pkg/executor/statement_ru_plan_walk.rs` 或其他计费/观测路径，明确新指标如何进入计算，避免只采集不消费。
4. Go 对照 `pkg/util/execdetails/ruv2_metrics.go`，保持 nil、旁路、零值、负增量和 drain 时机的语义一致。

测试不得内嵌到本生产文件。优先扩展独立文件 `pkg/util/execdetails/go_merge_20_test.rs`（原始字段筛选、None、旁路、零值、负增量和 drain-once）及 `pkg/util/execdetails/execdetails_test.rs`（聚合 crate 公开 API）；涉及真实上游时同步扩展 `pkg/server/internal/resultset/resultset_aster_unit_test.rs`、`pkg/server/conn_test.rs` 或 `pkg/executor/statement_ru_plan_walk_test.rs` 中的对应路径。

兼容性风险集中在公开名称和返回/共享语义；正确性风险集中在旁路与 drain 的先后顺序、重复累计、类型转换和溢出；性能风险集中在热路径原子操作、增加额外锁，以及为稀疏字段改变当前内嵌布局。若要修复上下文门面，应先统一两个内部 crate 的类型与所有权边界，而不是直接让本文件依赖另一套同名桩或复制逻辑。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/util/execdetails/ruv2_metrics.rs`，完整 109 行；列出 1 个键类型、1 个静态键、3 个模块级函数、1 个结构体、`Default`/`Clone` 及 4 个公开方法。
- 模块与 Cargo：`pkg/util/execdetails/internal/ruv2/lib.rs`（`include!`、`RUDetails`、Context/StmtExecDetails 桩）、`pkg/util/execdetails/internal/ruv2/Cargo.toml`、`pkg/util/execdetails/lib.rs`、`pkg/util/execdetails/Cargo.toml`。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`node --file` 确认目标源码和六个使用文件摘要；`query` 确认核心符号及 Go 同名符号；`callees` 确认同步、更新与旁路调用边。其 `callers` 未返回明细，因此上游引用由精确 `rg` 补证。
- Rust 上游：`pkg/executor/adapter.rs`、`pkg/executor/statement_ru_plan_walk.rs`、`pkg/server/internal/resultset/resultset.rs`、`pkg/distsql/context/lib.rs`。
- Rust 独立测试：`pkg/util/execdetails/go_merge_20_test.rs`、`pkg/util/execdetails/execdetails_test.rs`、`pkg/util/execdetails/util_3_aster_unit_test.rs`、`pkg/server/internal/resultset/resultset_aster_unit_test.rs`、`pkg/server/conn_test.rs`、`pkg/executor/statement_ru_plan_walk_test.rs`。其中前两者直接覆盖本文件的筛选、空输入、旁路、负增量和重复 drain；context 测试覆盖的是独立 util crate 的同名实现，不能直接证明本文件桩门面的行为。
- Go 对照与调用：`pkg/util/execdetails/ruv2_metrics.go`、`pkg/util/execdetails/util.go`、`pkg/server/internal/resultset/resultset.go`、`pkg/executor/adapter.go`、`pkg/executor/statement_ru_plan_walk.go`；相关边界还见 `pkg/server/conn_stmt_test.go`。

人工复核结论：该文件存在是为了在废弃旧 RU v2 计量模型后，继续为当前 statement-RU 和游标 delta 路径保留最小、并发安全的 Coprocessor 响应字节快照；安全扩展必须同时维护原始 RUDetails 累计/排空、语句级原子快照、Go 对照、消费方与独立测试，并明确当前上下文 API 的子 crate 桩限制。
