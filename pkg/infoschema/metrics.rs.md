# `pkg/infoschema/metrics.rs` 逻辑说明

## 文件定位

`pkg/infoschema/metrics.rs` 属于 `astersql-infoschema` crate；模块由 `pkg/infoschema/lib.rs` 以 `pub mod metrics` 装配。它是 InfoSchema V2 表缓存与指标子 crate 之间的适配层：上游契约是 `pkg/infoschema/sieve.rs` 的 `SieveStatusHook`，下游指标句柄来自 `astersql-infoschema-metrics`（路径依赖声明在 `pkg/infoschema/Cargo.toml`，实现位于 `pkg/infoschema/metrics/lib.rs`）。

该文件不定义 SIEVE 淘汰算法，也不创建或注册整个指标体系。缓存算法及回调时机在 `pkg/infoschema/sieve.rs`，指标描述符和全局共享句柄在 `pkg/infoschema/metrics/lib.rs`；本文件只把两者连接起来。当前 Rust 生产构造链还有一个重要边界：`pkg/infoschema/infoschema_v2.rs::Data::new` 创建 `Sieve` 时没有调用 `newSieveStatusHookImpl`，所以默认仍使用 `EmptySieveStatusHook`。与之不同，Go 的 `pkg/infoschema/infoschema_v2.go::NewData` 会自动安装此指标钩子。

## 核心职责

- `sieveStatusHookImpl` 保存三个 `prometheus::Counter` 和三个 `prometheus::Gauge` 句柄，分别表示驱逐、命中、未命中、对象数、内存用量和内存上限。
- `impl SieveStatusHook for sieveStatusHookImpl` 将缓存事件翻译为 Prometheus 操作：离散事件累加 Counter，当前状态覆盖 Gauge。
- `newSieveStatusHookImpl` 从 `astersql_infoschema_metrics` 的共享指标向量/Gauge 克隆句柄，并把 CounterVec 固定绑定到 `evict`、`hit`、`miss` 三个 `type` label。
- 六个公开快照方法 `evictions`、`hits`、`misses`、`object_count`、`memory_usage`、`memory_limit` 读取当前句柄值并返回 `u64`；它们不改变指标。

本文件的职责是可观测性适配，不决定缓存命中、淘汰、容量或大小计算规则。那些规则由 `Sieve` 调用钩子的具体位置决定。

## 主要符号

- `pub struct sieveStatusHookImpl`：具体钩子类型。虽然类型公开，但字段私有，外部只能通过构造函数取得完整绑定的实例，不能构造缺少某个指标的半初始化值。
- `sieveStatusHookImpl::{evictions,hits,misses}`：读取三个 Counter 的累计值。Prometheus Counter 的底层读数是 `f64`，这里转换为 `u64`。
- `sieveStatusHookImpl::{object_count,memory_usage,memory_limit}`：读取三个 Gauge 的瞬时值并转换为 `u64`。
- `SieveStatusHook::{on_evict,on_hit,on_miss}` 的实现：分别对对应 Counter 调用 `inc()`。
- `SieveStatusHook::on_update` 的实现：把 `size` 写入 `InfoSchemaV2CacheMemUsage`，把 `count` 写入 `InfoSchemaV2CacheObjCnt`。
- `SieveStatusHook::on_update_limit` 的实现：把 `limit` 写入 `InfoSchemaV2CacheMemLimit`。
- `pub fn newSieveStatusHookImpl() -> sieveStatusHookImpl`：唯一构造入口；绑定共享指标而非创建一套私有指标。

文件没有模块级常量、枚举、条件编译项或自定义错误类型。`#![allow(non_camel_case_types, non_snake_case)]` 保留了 Go 移植符号的命名形态。

## 执行流程

1. 调用方通过 `newSieveStatusHookImpl` 取得钩子。构造函数对 `InfoSchemaV2CacheCounter` 调用三次 `with_label_values`，固定得到 `evict`、`hit`、`miss` 子序列；三个 Gauge 则克隆全局句柄。
2. 钩子需要被包装成 `Arc<dyn SieveStatusHook>` 并交给 `Sieve::SetStatusHook`，缓存才会把事件发送到它。`Sieve::new` 默认安装的是空实现 `EmptySieveStatusHook`。
3. `Sieve::Get` 命中时调用 `on_hit`，未命中时调用 `on_miss`；钩子使相应 Counter 加一。
4. 新插入、删除、清空或淘汰导致缓存大小/对象数变化时，`Sieve::Set` 或 `Sieve::remove_entry` 调用 `on_update`，钩子用新快照覆盖两个 Gauge。
5. `Sieve::evict` 在删除条目并更新大小/数量后调用 `on_evict`，因此一次淘汰会同时更新状态 Gauge 和驱逐 Counter。
6. `Sieve::SetCapacity` 先写缓存容量，再调用 `on_update_limit`；钩子更新容量 Gauge。`SetCapacityAndWaitEvict` 复用这一路径，随后可能继续触发驱逐与状态更新。

RustCodeGraph 对 `newSieveStatusHookImpl` 的调用边只找到 `pkg/infoschema/metrics_test.rs::sieve_status_hook_updates_shared_prometheus_metrics`；trait 回调通过 `dyn SieveStatusHook` 动态分派，图中没有为每个具体实现建立静态 caller 边。结合 `pkg/infoschema/infoschema_v2.rs::Data::new` 的源码可确认：当前 Rust 生产构造路径没有执行上述第 1、2 步。

## 数据与状态

`sieveStatusHookImpl` 自身没有独立业务状态；六个字段都是 Prometheus 句柄。`CounterVec::with_label_values` 和 Gauge 的 `clone` 指向共享的指标序列，因此多个钩子实例观察、修改的是同一组全局时间序列。`pkg/infoschema/metrics_test.rs` 在回调前保存 Counter 基线，正是为了兼容跨实例、跨测试共享且只增不减的 Counter。

Counter 表示累计事件，只有递增操作；Gauge 表示最新快照，可以被后续回调覆盖。`on_update(size, count)` 的参数顺序不可交换：`size` 的单位由 `Sieve` 定义为字节，`count` 为缓存条目数；`on_update_limit(limit)` 的 `limit` 同样是字节。底层指标名称、help、namespace、subsystem 和 `type` label 定义在 `pkg/infoschema/metrics/lib.rs`，分别形成 `tidb_domain_infoschema_v2_cache`、`..._count`、`..._size`、`..._limit` 系列。

六个读取方法从 `f64` 转成 `u64`，适用于本文件只写入的非负整数值；若未来有别处向共享 Gauge 写入负数、小数或超出 `u64` 精确表示范围的值，这些方法会丢失相应信息，不能充当通用 Prometheus 数值读取接口。

## 依赖与调用关系

- crate 内依赖：`crate::sieve::SieveStatusHook` 提供回调契约；`pkg/infoschema/sieve.rs::Sieve` 在 `Get`、`Set`、`SetCapacity`、`remove_entry` 和 `evict` 路径调用契约。
- workspace 依赖：`pkg/infoschema/Cargo.toml` 直接依赖 `prometheus = "0.14"` 与路径 crate `astersql-infoschema-metrics = "metrics"`。
- 指标定义：`pkg/infoschema/metrics/lib.rs` 用 `LazyLock` 创建 CounterVec 和 Gauge。`pkg/metrics/infoschema.rs::InitInfoSchemaV2Metrics` 克隆相同句柄，`pkg/metrics/metrics.rs` 将这些外部子系统指标纳入注册集合。
- 模块入口：`pkg/infoschema/lib.rs` 公开 `metrics` 和 `sieve` 模块，但没有从 crate 根重新导出 `newSieveStatusHookImpl`；调用方应使用模块路径。
- Rust 直接调用者：已索引代码中仅 `pkg/infoschema/metrics_test.rs::sieve_status_hook_updates_shared_prometheus_metrics` 调用构造函数。`pkg/infoschema/infoschemav2_cache_test.rs` 安装的是测试专用 `CountingHook`，验证 SIEVE 的回调序列而非本文件的 Prometheus 适配器。
- Go 主链：`pkg/infoschema/infoschema_v2.go::NewData` 调用 `tableCache.SetStatusHook(newSieveStatusHookImpl())`，这是 Go 生产环境接线；Rust `pkg/infoschema/infoschema_v2.rs::Data::SetStatusHook` 提供接线能力，但 `Data::new` 尚未使用本文件的构造函数。

## 错误处理与边界

本文件的公开函数和回调均不返回 `Result`，也没有恢复分支。构造时 `CounterVec::with_label_values` 使用正确的单个 `type` label；底层 CounterVec/Gauge 的创建错误已在 `pkg/infoschema/metrics/lib.rs` 的 `LazyLock` 初始化闭包中通过 `unwrap()` 处理，因此描述符非法会在首次初始化时 panic，而不是由本文件传播错误。

回调只接受无符号整数，并转换为 `f64` 写入 Gauge。非常大的 `u64` 可能不能被 `f64` 精确表示；正常缓存容量、大小和数量通常远低于该精度边界，但扩展为更大计量范围时应重新评估。快照读取再转回 `u64` 也不是无损的通用转换。

钩子没有对事件顺序或成对调用做校验：例如直接调用 `on_evict` 只会增加驱逐计数，不会自动更新大小 Gauge。保持一致性依赖 `Sieve` 的调用协议。未安装钩子时不会报错，`EmptySieveStatusHook` 会静默丢弃事件；这正是当前 Rust `Data::new` 的生产可观测性缺口，而非运行时故障。

## 并发与资源生命周期

`SieveStatusHook` 要求 `Send + Sync`，`sieveStatusHookImpl` 所含 Prometheus Counter/Gauge 句柄可安全共享；预期安装形态是 `Arc<dyn SieveStatusHook>`。本文件不创建线程、任务、通道或锁，也不持有需要显式关闭的资源。

实际回调发生在 `Sieve` 持有其内部 `state: Mutex<State<...>>` 的期间；`Sieve` 会先从单独的 hook Mutex 克隆 `Arc`，再进入状态操作。钩子自身只进行 Prometheus 原子更新，不反向访问缓存，因此当前实现没有锁重入路径。扩展回调时不应执行阻塞 I/O、再次调用同一个 `Sieve`，或引入长耗时操作，否则会延长缓存临界区甚至造成死锁。

全局指标通过 `LazyLock` 按首次访问初始化，并随进程存活；克隆句柄不会复制指标值或建立独立生命周期。替换 `Sieve` 的 hook 只会更换后续事件接收者，不会重置既有 Counter/Gauge。

## 与 Go 版本的对应关系

`pkg/infoschema/metrics.go` 是直接语义来源：两端都有同名 `sieveStatusHookImpl`、三个 Counter、三个 Gauge、同样的 `evict/hit/miss` label，以及五个一一对应的回调。Rust 的 `on_*` 名称对应 Go 的 `onHit/onMiss/onEvict/onUpdate/onUpdateLimit`；Rust 字段 `object_count/memory_usage/memory_limit` 对应 Go 的 `objCnt/memUsage/memLimit`。

实现层差异如下：

- Go 构造函数返回指针，Rust 返回拥有句柄的值，真正交给缓存时再通过 `Arc<dyn SieveStatusHook>` 共享。
- Rust 额外提供六个公开快照读取方法，Go 具体钩子没有同类 getter。
- Go `NewData` 在构造 InfoSchema V2 数据时自动安装真实指标钩子；Rust `Data::new` 仅使用 `newSieve` 的空钩子，当前没有自动接线。因此“适配器行为已经实现并经过独立测试”成立，但“Rust InfoSchema V2 生产缓存默认上报这些指标”不成立。
- Go `pkg/infoschema/infoschemav2_cache_test.go` 使用 mock hook 验证回调协议；Rust 对应的 `pkg/infoschema/infoschemav2_cache_test.rs` 使用 `CountingHook` 验证事件序列。二者都不证明 Rust 的默认构造链安装了本文件的 Prometheus 钩子。

## 扩展指南

若新增一种缓存事件，先在 `pkg/infoschema/sieve.rs::SieveStatusHook` 增加回调并确定 `Sieve` 中唯一、准确的触发点，再在 `sieveStatusHookImpl` 增加对应句柄和实现；若需要新时间序列或 label，还要同步 `pkg/infoschema/metrics/lib.rs`、`pkg/metrics/infoschema.rs` 的桥接/注册集合、Go 的 `pkg/metrics/infoschema.go` 与 `pkg/infoschema/metrics.go`。同步扩展 `pkg/infoschema/metrics_test.rs` 验证具体共享指标值，并扩展 `pkg/infoschema/infoschemav2_cache_test.rs` 验证缓存事件序列。

若目标是补齐当前 Rust 生产接线，最小入口是 `pkg/infoschema/infoschema_v2.rs::Data::new`：创建缓存后安装 `Arc::new(crate::metrics::newSieveStatusHookImpl())`，语义对齐 Go `NewData`。这属于运行时代码变更，不是本文档任务的一部分；实现时应增加回归测试，证明通过正常 `Data`/InfoSchema V2 操作而非直接调用 hook 就能改变共享指标。

兼容性风险主要是指标名称和 label 值属于监控契约，修改会破坏仪表盘与告警；正确性风险是回调次数、顺序或 `size/count` 对应关系错误；性能风险是每次缓存访问进入指标更新，不能在回调中加入分配密集、阻塞或锁重入逻辑。由于共享 Gauge 不是按 `Data` 实例分组，多个同时活跃的 InfoSchema V2 缓存会互相覆盖瞬时值；新增实例维度前必须评估 Prometheus 基数和 Go 兼容性。

## 验证依据

- RustCodeGraph 状态：本仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `pkg/infoschema/metrics.rs` 已索引，共识别 16 个符号。
- RustCodeGraph 查询：对 `sieveStatusHookImpl`、`newSieveStatusHookImpl`、`on_evict`、`on_hit`、`on_miss`、`on_update`、`on_update_limit` 执行了 `query`；对构造函数及回调执行了 `node`、`callers`、`callees`，并读取了目标文件的完整索引源码。
- 调用流证据：`pkg/infoschema/sieve.rs::{Get,Set,SetCapacity,remove_entry,evict}` 定义回调时机；`pkg/infoschema/infoschema_v2.rs::{Data::new,Data::SetStatusHook}` 证明 Rust 默认空钩子和可选接线入口；`pkg/infoschema/metrics_test.rs::sieve_status_hook_updates_shared_prometheus_metrics` 是具体适配器的直接调用测试。
- crate/指标证据：读取了 `pkg/infoschema/Cargo.toml`、`pkg/infoschema/lib.rs`、`pkg/infoschema/metrics/Cargo.toml`、`pkg/infoschema/metrics/lib.rs`、`pkg/infoschema/metrics/metrics.rs`，并核对 `pkg/metrics/infoschema.rs` 与 `pkg/metrics/metrics.rs` 中的桥接和注册引用。
- Go 对照：读取了 `pkg/infoschema/metrics.go`、`pkg/infoschema/infoschema_v2.go::NewData`、`pkg/infoschema/sieve.go` 与 `pkg/infoschema/infoschemav2_cache_test.go`；Rust 独立测试还读取了 `pkg/infoschema/infoschemav2_cache_test.rs`。
- 未运行 Cargo 或运行时测试，符合本任务“纯文档分析、不运行 Cargo”的限制；事实验证依赖已存在的源码、独立测试和 RustCodeGraph 索引，结构验证按任务给定命令执行。
