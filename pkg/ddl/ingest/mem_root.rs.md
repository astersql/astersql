# `pkg/ddl/ingest/mem_root.rs`

## 文件定位

本文件属于 `astersql-ddl-ingest` crate，由 [`lib.rs`](lib.rs) 以 `pub mod mem_root` 公开。它位于 DDL add-index ingest/reorg 的资源控制层：本地引擎和 writer 在创建时先查询共享内存预算，再把预留量登记到这个根计数器；配置选择和错误消息也从这里读取当前用量与上限。

它不是系统内存采样器、分配器或 DDL job 持久化组件。`MemRootImpl` 只维护调用者显式报告的进程内账目，不会阻止真实分配，也不主动触发 flush/import。Rust 仓库中生产模块已经通过 `Arc<dyn MemRoot>` 接受并传播这一接口，但精确检索到的 `MemRootImpl::new` 构造点仅位于 Rust 单元测试和 `tests/realtikvtest/addindextest3/functional_test.rs`；Go 版本则由 `env.go` 初始化全局 `LitMemRoot`。因此当前 Rust 默认实现应视为已公开、可被局部链路使用且有测试覆盖的移植组件，不能据此宣称已经完成与 Go 全局 ingest 环境等价的生产接线。

## 核心职责

- 用 `MemRoot` trait 固定内存配额组件所需的查询、增减、按标签记账和刷新接口，并要求实现满足 `Send + Sync`。
- 用 `MemRootImpl` 保存最大配额、总用量和每个字符串标签的用量，使多个 backend/engine/writer 可通过 trait object 共享同一账本。
- 用 `check_consume` 做“当前用量 + 申请量是否不超过上限”的只读预检；真正记账由后续 `consume` 或 `consume_with_tag` 完成。
- 用 `release_with_tag` 原子删除一个标签并从总量扣除该标签累计值，支持 writer/engine 生命周期结束时幂等归还。
- 保留与 Go `MemRoot` 同形的 `refresh_consumption` 接口；当前两端默认实现都是空操作。

## 主要符号

- `pub trait MemRoot: Send + Sync`：公开抽象。方法包括配额设置/读取 `set_max_memory_quota`、`max_memory_quota`，总量查询与增减 `current_usage`、`consume`、`release`，标签查询与增减 `current_usage_with_tag`、`consume_with_tag`、`release_with_tag`，预检 `check_consume`，以及保留刷新点 `refresh_consumption`。
- `Usage`：私有账本，`current: i64` 保存总量，`by_tag: BTreeMap<String, i64>` 保存分账。未出现的标签查询为零；有序 map 的顺序在本文件没有对外语义。
- `pub struct MemRootImpl`：默认实现。`max_quota: Mutex<i64>` 与 `usage: Mutex<Usage>` 分别保护配额和账本；类型本身没有后台线程或外部资源。
- `MemRootImpl::new(max_quota: i64) -> Self`：以指定上限和空账本构造实例。它不验证上限是否为负，也不读取机器物理内存。
- `impl MemRoot for MemRootImpl`：所有算术更新使用 `wrapping_add`/`wrapping_sub`，锁获取使用 `unwrap`。目标文件没有常量、条件编译项或自定义错误类型。

## 执行流程

1. 拥有者构造 `MemRootImpl`，通常再包装为 `Arc<dyn MemRoot>`，并把同一对象传给 `BackendContextBuilder::build`、`BackendContext` 和其创建的 `EngineInfo`。
2. 创建 writer 时，[`EngineInfo::create_writer`](engine.rs) 先调用 `check_consume(writer_memory)`。若结果为假，返回 `"memory used up"`；若为真，以 `job-…-index-…-writer-…` 标签调用 `consume_with_tag`，再返回 writer。
3. `consume_with_tag` 在一把 `usage` 锁内同时增加总量和标签量；同一标签重复登记会累计。无标签的 `consume` 只改变总量，不产生分账。
4. writer 的 `Drop` 和 engine 的 `close` 调用 `release_with_tag`。该方法在同一临界区移除标签并扣除其完整累计值；标签不存在时不改变总量，因此重复释放是幂等的。
5. [`config.rs`](config.rs) 用 `current_usage` 和 `max_memory_quota` 选择激进配置或缩小缓存；[`message.rs`](message.rs) 在构造分配失败信息时读取这两个快照。
6. `refresh_consumption` 当前立即返回，不刷新任何外部统计。

`check_consume` 的“检查”与调用方随后的“登记”不是一个原子操作：它先分别读取总量和配额，锁随后即释放。两个并发调用者可能都通过检查再各自登记，最终超过上限；Go 文件中的 TODO 也明确指出这一 TOCTOU（检查与使用之间的时间窗口）问题。

## 数据与状态

`current` 是所有无标签增减与标签增减的代数和。`by_tag` 只包含通过 `consume_with_tag` 建立且尚未 `release_with_tag` 删除的条目；普通 `release(size)` 不会同步减少任何标签，所以混用两套 API 时，调用者必须自己维持“总量与各分账用途”的配对关系。标签总和也不必等于 `current`，因为允许无标签消费。

所有数值均为有符号 `i64`，实现不钳制负配额、负消费、超额释放或负标签值。`mem_root_test.rs::test_memory_root_preserves_signed_accounting` 验证负值会按原始代数语义保留。加减明确采用 wrapping 算术，因此越过 `i64` 边界时按二进制补码回绕，不 panic、不饱和，也不返回错误。

`check_consume(size)` 的判断是 `current_usage().wrapping_add(size) <= max_memory_quota()`，等于上限时允许；它不会预留额度或改变状态。测试验证初始配额 1024 时申请 1024 为真、1025 为假，已有 512 时再申请 512 为真、513 为假。

## 依赖与调用关系

本文件的直接实现依赖只有标准库 `BTreeMap` 和 `Mutex`；[`Cargo.toml`](Cargo.toml) 定义 crate 名、`lib.rs` 入口及 Go 包映射，但其中 `fs2`、`fail`、`astersql-util-dbterror` 和 Windows 条件依赖都没有被本文件直接使用。

Rust 上游/消费者关系由源码引用核对如下：

- [`backend_mgr.rs::BackendContextBuilder::build`](backend_mgr.rs) 接收 `Arc<dyn MemRoot>`，交给 [`backend.rs::BackendContext`](backend.rs)；`BackendContext::register` 再克隆给每个 `EngineInfo`。
- [`engine.rs::EngineInfo::create_writer`](engine.rs) 调用 `check_consume` 和 `consume_with_tag`；`EngineInfo::close` 与 `WriterContext::drop` 调用 `release_with_tag`。这是当前 Rust 中配额预检与标签生命周期的主要直接执行链。
- [`config.rs::adjust_import_memory`](config.rs) 与 `try_aggressive_memory` 读取配额/用量来选择缓存大小，但不记账。
- [`message.rs::engine_alloc_memory_failed`](message.rs) 与 `writer_alloc_memory_failed` 读取用量快照填充 `IngestMemoryError`。
- Rust 单元测试及 RealTiKV 测试构造 `MemRootImpl`；仓库精确检索没有找到非测试 Rust 生产代码中的默认实现构造点。

RustCodeGraph 的文件节点报告 `mem_root.rs` 被 16 个文件引用，并定位了 `MemRoot`、`MemRootImpl` 和所有 trait 方法；精确 `callers/callees` 查询在限时内未返回可用边，以上动态 trait 调用关系因此由已定位文件的直接引用补证。

## 错误处理与边界

- 本文件所有公开方法均不返回 `Result`。它表达记账，不负责把配额不足转换为业务错误；调用方必须检查布尔值并选择错误或降级路径。
- `Mutex::lock().unwrap()` 在锁被其他线程 panic 毒化后会继续 panic，而不是恢复内部值或返回可处理错误。`refresh_consumption` 也不会清除 poison 或重算账本。
- `check_consume` 是非原子预检，不能提供并发下“检查成功即保证预留成功”的承诺。当前 writer 创建还持有 engine 状态锁，但不同 engine 共享同一 `MemRoot` 时仍可并发通过预检。
- `set_max_memory_quota` 可把上限降到当前用量以下；实现不会自动释放、报错或触发导入，后续正数预检通常会失败。
- `release(size)` 可把总量降为负数；`release_with_tag` 只在标签存在时扣减，重复释放同一标签无副作用。不存在标签的查询为零。
- wrapping 算术避免 debug/release 构建差异，但可能让溢出后的配额判断失真；调用者应保证现实字节计数远离 `i64` 边界。

## 并发与资源生命周期

`MemRoot: Send + Sync` 允许以 `Arc<dyn MemRoot>` 跨线程共享；`MemRootImpl` 用两把独立 mutex 保护最大配额和用量。单次 `consume_with_tag`/`release_with_tag` 对总量与标签表的修改在 `usage` 锁内原子完成，普通查询或增减也在各自锁域内串行。

两把锁没有被同一方法同时持有：`check_consume` 先调用 `current_usage`，释放 `usage` 锁后再调用 `max_memory_quota` 获取配额锁，因此不会在本文件内形成双锁死锁，但得到的是两个不同时刻的值。独立锁也允许配额在检查的两次读取之间变化。

典型资源生命周期是“共享根构造 → engine/writer 按唯一标签登记 → writer drop / engine close 删除标签 → 最后一个 `Arc` 释放时账本销毁”。本类型没有显式 `Drop`、线程、任务、通道、文件句柄或持久化；进程重启后状态不会恢复。若调用方遗失标签或不执行关闭/drop 配对，账目会一直保留到根对象销毁。

## 与 Go 版本的对应关系

Rust `MemRoot`/`MemRootImpl` 分别对应 [`mem_root.go`](mem_root.go) 的 `MemRoot`/`memRootImpl`。两端都提供相同类别的方法、允许相等于配额的申请、让同一标签累计、让重复标签释放成为空操作，并把 `RefreshConsumption` 保留为空操作。Rust 独立测试 [`mem_root_test.rs`](mem_root_test.rs) 重现了 Go [`mem_root_test.go`](mem_root_test.go) 的 1024 配额、普通增减、标签累计/释放和混合记账主流程，另补了有符号值与刷新空操作的边界覆盖。

明确差异与迁移状态如下：

- Go 用一把 `sync.RWMutex` 同时保护 `maxLimit`、`currUsage` 和 `structSize`，因此一次 `CheckConsume` 在同一读锁下取得用量和上限；Rust 使用两把 `Mutex` 并分两次读取，快照一致性更弱。两端仍都存在“检查后再消费”的跨调用 TOCTOU。
- Go `init` 通过 `unsafe.Sizeof(engineInfo{})` 和 `unsafe.Sizeof(writerContext{})` 初始化结构体大小，`engine.go`/`engine_mgr.go` 用这些值预检并记账；Rust `mem_root.rs` 没有对应常量或初始化，Rust `engine.rs` 只按传入的 `writer_memory` 登记 writer 额度。
- Go `env.go` 依据物理内存创建全局 `LitMemRoot`，`testutil` 可替换它；Rust 当前没有检索到生产默认实例或全局环境接线，主要通过参数注入 trait object。
- Go 的整数 `+=`/`-=` 保留原始 `int64` 语义；Rust 显式使用 wrapping 算术来保证溢出时不受构建模式影响。两端都没有把溢出定义为可恢复错误。
- Go 使用普通 `map[string]int64`，Rust 使用 `BTreeMap<String, i64>`；排序只是实现选择，本接口不暴露标签遍历，因此不构成行为契约。

## 扩展指南

- 若修复 TOCTOU，应优先为 trait 增加原子“尝试预留”操作，或把检查与登记合并到同一锁内；同步修改 `EngineInfo::create_writer` 和独立测试 [`mem_root_test.rs`](mem_root_test.rs)、[`engine_test.rs`](engine_test.rs)，并核对 Go API 的兼容策略。仅在现有 `check_consume` 外再加调用方锁无法保护跨 engine 共享场景。
- 若补齐 Go 的 engine/writer 结构体大小记账，应明确 Rust 对象堆内数据、trait object 与缓存容量的估算口径，不可机械照搬 `size_of`；同时核对 `engine.rs` 的标签生命周期、`backend.rs` 的注册失败清理以及 Go `engine.go`/`engine_mgr.go`。
- 若建立生产默认实例，接线点应与全局 ingest 环境的启动/关闭生命周期一致，并把同一 `Arc<dyn MemRoot>` 注入 backend；需要验证多 job 共享、配置热更新、owner 切换以及进程重启后的计数边界。当前代码事实不能支持全局单例已经存在的结论。
- 若改变负值或溢出策略，必须评估与 Go `int64` 行为和已有测试的兼容性，补充 `i64::{MIN,MAX}`、超额释放、负配额及标签/无标签混用测试。
- 若改变锁粒度或 poison 策略，应增加多线程并发预留、配额更新与重复释放测试，确认不会丢计数或死锁。性能风险主要是所有 engine/writer 共享单一 `usage` mutex 的竞争。
- Rust 测试应继续放在独立的 `mem_root_test.rs` 或相关模块测试文件中，不要把测试嵌入 `mem_root.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件；`files --filter pkg/ddl/ingest` 列出目标、Go 对照和测试；`node --file pkg/ddl/ingest/mem_root.rs --offset 1 --limit 400` 完整读取 120 行并报告 16 个引用文件；`query MemRoot`、`query MemRootImpl --kind struct --json` 及方法查询定位 trait、实现和主要 API。精确 `callers/callees` 查询超时且无可用输出，调用边以源码精确检索补证。
- 目标与 crate 边界：[`mem_root.rs`](mem_root.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。前者确认 27 个索引符号、两类状态和全部方法；后两者确认公开模块、crate 名 `astersql-ddl-ingest`、库入口及 Go package 元数据。
- Rust 直接调用者：[`engine.rs`](engine.rs)、[`backend.rs`](backend.rs)、[`backend_mgr.rs`](backend_mgr.rs)、[`config.rs`](config.rs)、[`message.rs`](message.rs)。它们分别证明标签预留/释放、共享传播、构造注入、配置读取和错误快照关系。
- Go 对照：[`mem_root.go`](mem_root.go)、[`engine.go`](engine.go)、[`engine_mgr.go`](engine_mgr.go)、[`env.go`](env.go)；用于核对接口、锁语义、结构体大小记账和全局初始化。Rust 当前生产构造缺口由全仓 `MemRootImpl::new` 精确检索确认。
- 测试：[`mem_root_test.rs`](mem_root_test.rs) 覆盖配额边界、普通/标签记账、重复释放、混合用法、负值与刷新空操作；[`mem_root_test.go`](mem_root_test.go) 提供原始主流程；`tests/realtikvtest/addindextest3/functional_test.rs` 提供 Rust RealTiKV 测试构造证据。按任务约束，本次纯文档分析未运行 Cargo。
- DDL 语境：`pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md` 仅作为 add-index/reorg 入口背景，本文关于实际行为、接线和差异的结论均由上述源码与测试复核。
