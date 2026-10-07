# `br/pkg/utiltest/fakecluster/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-utiltest-fakecluster` 的 crate 根，而不是假集群状态机的实现文件。`br/pkg/utiltest/fakecluster/Cargo.toml` 的 `[lib] path = "lib.rs"` 将它指定为库入口，`[package.metadata.porting]` 又把该 crate 对应到 Go 包 `br/pkg/utiltest/fakecluster`。该 crate 属于 BR 的测试工具层，面向 `streamhelper`、CRR/DRR 测试夹具等调用方，不是 TiDB/BR 的生产运行时集群客户端。

入口文件通过 `#[path = "stubs.rs"] pub mod stubs` 和 `#[path = "core.rs"] pub mod core` 装配两个实现模块，再通过 `pub use core::*` 和对 `stubs` 的选择性 `pub use` 提供扁平 API。真正的内存集群、region/store、flush 与 checkpoint 行为在 `core.rs`；为避免 macOS arm64 测试构建引入 kvproto、grpcio、domain/kv 等重依赖而定义的本地协议替身在 `stubs.rs`（依据：`lib.rs:19-31`、`Cargo.toml:8-19`、`stubs.rs:16-22`）。

## 核心职责

本文件只承担四项边界职责：

1. 声明 `stubs` 与 `core` 两个公开子模块，确定 crate 的源码组成。
2. 将 `core.rs` 的公开符号全部提升到 crate 根，使调用方可以写 `fakecluster::Cluster`、`fakecluster::NewRegion`，而不必写 `fakecluster::core::...`。
3. 将 `stubs.rs` 中经过挑选的请求/响应、错误、上下文、键范围、锁以及 `codec`/`oracle` 工具提升到 crate 根；未列入 `pub use stubs::{...}` 的桩符号仍只能经 `stubs::` 路径访问。
4. 仅在 `cfg(test)` 下装入 `parity_test.rs` 与 `core_test.rs`，保持测试逻辑与生产源文件分离（依据：`lib.rs:25-39`）。

因此，`lib.rs` 不分配集群、不执行 RPC、不持有锁，也不改变 checkpoint；它定义的是“调用方看到什么”的公共契约。修改再导出列表会直接改变下游源码的可用路径，即使 `core.rs`/`stubs.rs` 行为完全未变。

## 主要符号

- crate 级 `#![allow(...)]`：统一允许移植代码中的 Go 风格命名、未用导入/变量、死代码及 Clippy 告警。作用域覆盖整个 crate，目的是保留 Go/Rust 对照名称和测试桩的可选接口；它不表示这些 API 已用于生产（`lib.rs:9-17`）。
- `pub mod stubs`：公开本地边界替身。主要内容包括 `KeyRange`、flush/checkpoint 请求响应、`Lock`、`Code`/`StatusError`/`Error`、可取消 `Context`，以及 `oracle`、`codec`（`lib.rs:19-20`；`stubs.rs:28-267`）。
- `pub mod core`：公开内存假集群实现。主要公开族为 `FlushSimulator`、`Region`/`RegionState`、`Store`、`Cluster`，构造入口 `New`、`NewBasicCluster`、`NewRegion`，以及这些类型上的拓扑、TSO、GC safepoint、flush 与 checkpoint 方法（`lib.rs:22-25`；`core.rs:64-1465`）。
- `pub use core::*`：通配提升 `core` 的全部公开项。这是当前最宽的公共面；在 `core.rs` 新增 `pub` 项会自动成为 crate 根 API。
- `pub use stubs::{...}`：显式提升 `CancelHandle`、`Code`、`Context`、`Error`、`ErrorPb`、各类 flush/checkpoint 消息、`KeyRange`、`Lock`、`RegionCheckpoint`/`RegionIdentity`、`Result`、`StatusError`、`codec`、`oracle` 与 `status_error`（`lib.rs:26-31`）。
- `mod parity_test`、`mod core_test`：测试构建时的私有测试模块；前者验证 Go/Rust 公共契约，后者锁定缺失 store/region 的 panic 行为（`lib.rs:33-39`）。

## 执行流程

`lib.rs` 没有可调用函数，其运行效果发生在编译与链接阶段：

1. Cargo 以 `lib.rs` 为 crate 根，先应用 crate 级 lint allow。
2. 编译器按显式 `#[path]` 载入 `stubs.rs` 和 `core.rs`。`core.rs` 通过 `crate::stubs::{...}` 使用协议替身，因此模块声明顺序也清楚表达了边界依赖（`core.rs:57-62`）。
3. `pub use` 建立根级 API。下游 CRR 代码以 `use astersql_br_pkg_utiltest_fakecluster::{self as fakecluster, Context as FcContext, oracle}` 导入门面；`PDSim` 以 `fakecluster::New`/`NewRegion` 构建拓扑，再由 `StoreClientAdapter` 调用 `GetLastFlushTSOfRegion` 和 `SubscribeFlushEvent`（`br/pkg/utiltest/crr/pd_sim.rs:31,56-123`；`pd_sim_service.rs:88-167`）。
4. 在测试构建中，两个 `cfg(test)` 模块通过 `use crate::{...}` 从根级 API 验证同一公共面。普通依赖构建不会编译这两个测试模块。

一个典型数据流是：`NewPDSimWithTestContext` 创建 `Cluster`、`Store`、`Region`并开启 flush subscription；`StoreClientAdapter::SubscribeFlushEvents` 经根级再导出的 `Context` 和请求类型订阅 `Store`；`Cluster::ApplyCheckpointToStore` 更新 region checkpoint 并发送编码后的 `FlushEvent`；适配器解码键并把事件交给 `streamhelper`（依据：`pd_sim.rs:65-124`、`pd_sim_service.rs:138-167`、`core.rs:1350-1425`）。

## 数据与状态

本入口自身没有字段或全局可变状态。它暴露的主要状态模型如下：

- `Cluster`：用 `Mutex` 保护集群级临界区、store 映射和 region 列表，用 `AtomicU64` 保存 ID 高水位、TSO 与 GC safepoint 标志；同时保存可注入的 `OnGetClient`/`OnClearCache` hook（`core.rs:708-755`）。
- `Store`：保存 store ID、region 映射、订阅支持状态/代数/发送端集合、checkpoint hook、legacy RPC 开关和 flush 任务名（`core.rs:361-415`）。
- `Region`：保存 `[StartKey, EndKey)`、leader、epoch、checkpoint、flush epoch 与锁列表。共享所有权使用 `Arc<Region>`，可变字段由 `Mutex` 或原子量保护（`core.rs:115-164`）。
- `Context`：`Arc` 共享取消状态，以 `AtomicBool + Mutex<Option<String>> + Condvar` 支持检查、错误读取和等待取消；`CancelHandle` 负责一次性记录取消原因并唤醒等待者（`stubs.rs:182-267`）。

关键不变量由独立测试固定：基本三 store 集群的 ID 为 1、2、3，初始 region ID 为 4；region 分裂后左右 epoch 同时增加且 flush epoch 清零；region 列表按起始键排序；checkpoint 只能向前应用；TSO 的物理部分单调前进（`parity_test.rs:25-119,276-335`）。

## 依赖与调用关系

编译依赖由 `Cargo.toml` 限定为两个已移植的 BR crate、`rand` 和 `tracing`：`astersql-br-pkg-streamhelper` 提供扫描结果、store 和 task event 类型，`astersql-br-pkg-streamhelper-spans` 提供范围重叠/字节边界比较，`rand` 用于 scatter/leader 随机化，`tracing` 记录 checkpoint 等事件。kvproto、gRPC、oracle、codec 与事务锁接口由 `stubs.rs` 在本 crate 内替代（`Cargo.toml:14-19`；`core.rs:47-62`）。

已核实的上游消费路径包括：

- `br/pkg/utiltest/crr/pd_sim.rs` 持有 `fakecluster::Cluster`，调用 `New`、`NewRegion`、`AllocTSO` 和 snapshot/region 方法；这是当前 Rust 代码中直接依赖该 Cargo 包的主要门面。
- `br/pkg/utiltest/crr/pd_sim_service.rs` 把 `Cluster`/`Store` 适配成 `streamhelper` 的服务 trait，转换 checkpoint 请求响应，并在后台线程中桥接 flush 订阅。
- `br/pkg/utiltest/crr/pd_sim_test.rs` 直接使用根级 `Context` 和 `SubscribeFlushEventRequest`，证明这些选择性再导出是测试公共契约的一部分。
- `br/pkg/utiltest/crr/Cargo.toml` 以路径依赖 `../fakecluster` 接入该 crate。

RustCodeGraph 对 `lib.rs` 的文件节点显示它被 `br/pkg/utiltest/syncpoint/syncpoint.rs` 使用；精确符号查询还定位到上述 CRR import。由于 `lib.rs` 只是再导出门面，图中的方法调用边归属于 `core.rs` 方法而非 `lib.rs` 自身；不能把整个仓库中同名 `Cluster`/`SubscribeFlushEvent` 的结果无差别归给本 crate。

## 错误处理与边界

入口不产生运行时错误，但它公开了三种不同失败形态，扩展 API 时必须保持区分：

- 可恢复错误：`Result<T>`/`Error` 与 `StatusError`。例如未开启订阅返回 `Code::Unimplemented` 和 `"meow?"`；禁用 legacy checkpoint RPC 返回 `Unimplemented`；上下文取消返回 `Canceled`；缺失 store 返回包含 store ID 的错误（`core.rs:464-490,516-594,1000-1008`）。
- 协议内逐 region 错误：`GetLastFlushTSOfRegion` 的整体调用可以成功，但单项 `RegionCheckpoint.Err` 可能是 `not found`、`not flushed`、`flushed epoch not match` 或 `epoch not match`（`core.rs:83-101,533-587`）。
- 与 Go 对齐的显式 panic：不一致键空间、未知 store 的 `RemoveStore`、未知 region 的 `UpdateRegion`，以及 store 不足时 `SplitAndScatter` 的切片越界。`core_test.rs` 与 `parity_test.rs` 明确要求这些场景不能静默跳过（`core.rs:1059-1069,1139-1145,1189-1234`；`core_test.rs:7-20`；`parity_test.rs:341-353`）。

边界还包括：`RegionScan(limit = 0)` 返回空；空 `EndKey` 表示正无穷；`FlushExcept` 只 flush 本 store 的 leader region，并跳过包含任一排除键的范围；订阅缓冲为 1024，普通 `emitFlushEvents` 在满缓冲时丢弃而不阻塞；`ApplyCheckpointToStore` 则等待发送并允许 `Context` 取消终止（`parity_test.rs:121-165`；`core.rs:475-490,600-650,1399-1424`）。

## 并发与资源生命周期

`lib.rs` 不启动线程，生命周期规则来自它公开的实现：

- `Arc<Region>` 与 `Arc<Store>` 允许 cluster、store map、调用方和订阅适配器共享对象；内部变更由 `Mutex`/`AtomicU64` 保护。原子操作使用 `SeqCst`，优先保证测试状态的直观可见性（`core.rs:35-45,119-134,379-392,712-735`）。
- `Store::SubscribeFlushEvent` 创建容量 1024 的同步通道，为每个订阅分配 ID，并启动清理线程等待 `Context` 取消；取消后从 subscriber map 删除发送端。`parity_test.rs` 轮询断言订阅数从 1 降到 0（`core.rs:464-490`；`parity_test.rs:295-318`）。
- `trivialFlushStream::Recv` 每 10ms 轮询事件，以便同时响应取消；取消后会先尝试取出已到达事件，空通道才返回 `Canceled`，发送端全断开则返回 EOF 语义（`core.rs:272-315`）。
- `ApplyCheckpointToStore` 在修改前先验证所有 region 的 checkpoint，避免部分更新；复制 subscriber 发送端后释放 store client 锁，再逐个发送，满缓冲时短暂休眠并检查取消，避免持锁阻塞（`core.rs:1350-1424`）。
- 锁顺序或临界区的扩展必须谨慎：`Cluster::SplitAndScatter`/TSO/GC 操作持有集群粗锁，store/region 集合另有细锁。新增跨对象操作应避免在等待通道或外部 hook 时长期持有这些锁。

## 与 Go 版本的对应关系

Go 权威对照文件是同目录 `core.go`；它与 Rust `core.rs` 的主要公开模型和方法集合逐项对应：`FlushSimulator`、`Region`、`Store`、`RegionState`、`Cluster`，以及 `New`/`NewBasicCluster`/`NewRegion`、region 分裂/散射、TSO、GC safepoint、checkpoint 查询和 flush subscription（`core.go:51-924`；`core.rs:64-1465`）。Rust 测试命名 `go_rust_public_contract_matches`，直接锁定构造、边界、错误文案、订阅清理和事件广播等语义。

需要注意的表示差异：Go 直接使用 `kvproto`、gRPC、TiKV oracle/txnlock 等真实依赖类型；Rust crate 为精简 arm64 构建在 `stubs.rs` 中定义局部等价子集，因此它证明的是本测试夹具所需契约，而非完整网络/protobuf 兼容性。Go 的普通字段和 `sync.Mutex` 在 Rust 中多映射为 `AtomicU64`、`Mutex` 与 `Arc`；Go goroutine/channel 的订阅清理映射为 Rust thread/`sync_channel`；Go 指针可空行为在 Rust 中分别映射为 `Option`、`Result` 或有意保留的 panic。

门面层也不完全对称：Go 包天然以目录形成命名空间，没有单独的 `lib.rs`；Rust 必须由本文件显式声明模块和再导出 API。故对 Go 新增符号进行移植时，除实现 `core.rs`/`stubs.rs` 外，还要判断是否需要加入根级选择性再导出。

## 扩展指南

- 新增集群行为：优先在 `core.rs` 的相应类型实现，并逐项对照 `core.go` 的增量；不要把逻辑写入 `lib.rs`。若是全新 Rust 自用辅助，需明确它不是 Go 契约。
- 新增协议/错误/上下文类型：放入 `stubs.rs`，只实现测试调用链实际需要的字段与语义；若下游需要根级路径，再显式加入 `pub use stubs::{...}`。避免无意扩大 API，尤其不要用 `pub use stubs::*` 替代当前白名单。
- 新增 `core.rs` 公开项前要意识到 `pub use core::*` 会自动把它暴露到 crate 根；若不应成为公共测试 API，应保持私有或 `pub(crate)`。
- 行为修改必须同步独立测试文件，通常扩展 `parity_test.rs`；与 panic/内部边界直接相关的测试可放 `core_test.rs`。不得把 Rust 测试内嵌回 `lib.rs` 或 `core.rs`。
- 修改订阅时同时覆盖未支持、取消清理、通道满/断开、事件编码和 checkpoint 单调性；修改拓扑时覆盖排序、空边界、epoch、leader/peer 与 store 数不足；修改 TSO/GC 时覆盖单调性和禁止回退。
- 兼容风险主要是根级符号路径、错误文本/分类、事件键编码和 Go 对齐的 panic；性能风险主要来自通配再导出本身之外的实现变化，例如扩大锁临界区、为每个订阅创建线程以及满通道重试。该 crate 是测试夹具，性能优化不能以删减 Go 行为为代价。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；`files --filter br/pkg/utiltest/fakecluster` 列出 `core.go`、`core.rs`、`core_test.rs`、`lib.rs`、`parity_test.rs`、`stubs.rs`。`node --file br/pkg/utiltest/fakecluster/lib.rs` 确认 39 行入口及模块/再导出/测试声明。
- RustCodeGraph 对 `NewBasicCluster`、`SubscribeFlushEvent`、`ApplyCheckpointToStore`、`GetLastFlushTSOfRegion` 的精确查询确认 Go/Rust 对应符号及 Rust 定义位置；对 `fakecluster` 的查询确认 CRR 的 `pd_sim.rs`、`pd_sim_service.rs` 与 `pd_sim_test.rs` import。图工具未为这些按 ID 请求的 `callers`/`callees` 返回边，因此调用关系另外由上述调用点源码核实，没有据此虚构图边。
- 已读源码/配置：`lib.rs`、RustCodeGraph 中的 `core.rs`/`stubs.rs`、`Cargo.toml`、`BUILD.bazel`、Go 对照 `core.go`。
- 已读独立测试：`parity_test.rs`、`core_test.rs`；已读直接消费者：`br/pkg/utiltest/crr/pd_sim.rs`、`pd_sim_service.rs`，并用仓库搜索核实 `crr/Cargo.toml` 和 `pd_sim_test.rs` 的依赖/导入。
- 人工复核结论：本文件存在的理由是定义精简假集群 crate 的模块边界与稳定根级 API；运行行为由 `core.rs` 和 `stubs.rs` 提供；安全扩展点分别是实现模块、显式再导出列表及两个独立测试模块。

本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求该文件存在，并且恰好包含本页的 11 个固定二级标题。
