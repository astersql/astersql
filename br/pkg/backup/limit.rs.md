# `br/pkg/backup/limit.rs`

## 文件定位

本文说明的真实源文件是 [`br/pkg/backup/limit.rs`](limit.rs)。它属于 `astersql-br-pkg-backup` library crate；该 crate 的入口是 `br/pkg/backup/lib.rs`，入口通过 `#[path = "limit.rs"] pub mod limit` 注册模块，再以 `pub use limit::*` 平铺导出公共符号。`br/pkg/backup/Cargo.toml` 把 Go 包映射记录为 `br/pkg/backup`，且本文件只依赖 Rust 标准库的 `std::sync::{Condvar, Mutex}`，不依赖该 crate 声明的 `serde` 或 `serde_json`。

它位于备份请求主链的资源控制点：`client.rs::BackupClient::BackupRanges` 根据 `rangeLimit` 创建一个 `ResourceConcurrentLimiter`，存入 `MainBackupLoop::Limiter`；主循环把同一个 `Arc` 克隆给各 store 的异步发送任务，最终由 `store.rs::doSendBackup` 在发起一次 `Backup` 调用前后申请和释放资源量。它限制的是并发请求所携带的 range 数量估算，而不是线程个数、字节数或完整响应流的存活时间。

## 核心职责

- 用 `ResourceConcurrentLimiter` 保存阈值、当前累计占用量和条件变量，向多个线程提供共享的资源计数闸门。
- `Acquire` 在“申请前的当前值已经达到阈值”时等待；一旦允许进入，就把本次 `resource` 整体加入计数并返回加入后的值。
- `Release` 从计数中减去资源量并唤醒所有等待者，使它们重新持锁检查条件。
- 保持 `br/pkg/backup/limit.go` 的宽松阈值语义：阈值不是结果的严格上界。例如阈值为 100、当前值为 90 时，`Acquire(30)` 可以返回 120；只有后续申请者会因当前值已达到阈值而等待。

本文件不负责计算业务资源量，也不拥有线程池或备份流。实际计量规则在 `store.rs::doSendBackup`：`req.SubRanges.len() + 1`，其中额外的 1 代表主 range。

## 主要符号

- `pub struct ResourceConcurrentLimiter`：公共限流器类型。三个字段均为模块私有：`cond: Condvar` 用于等待和广播，`current: Mutex<isize>` 同时保存占用量并充当条件变量关联锁，`threshold: isize` 是构造后不变的放行阈值。类型未自行实现 `Clone`；应用主链通过 `Arc<ResourceConcurrentLimiter>` 共享同一实例。
- `pub fn NewResourceMemoryLimiter(threshold: isize) -> ResourceConcurrentLimiter`：公共构造器。以 `current = 0` 创建新实例，直接保存调用方给出的阈值。名称和大写风格有意对应 Go API；`lib.rs` 的 crate 级 lint 配置允许该命名。
- `pub fn ResourceConcurrentLimiter::Acquire(&self, resource: isize) -> isize`：公共阻塞申请接口。返回更新后的累计占用量，测试用该返回值检查宽松上界。
- `pub fn ResourceConcurrentLimiter::Release(&self, resource: isize)`：公共释放接口。没有返回值，完成扣减后调用 `Condvar::notify_all`。

文件中没有模块级常量、trait、枚举、条件编译项或异步函数。

## 执行流程

构造与接线流程如下：

1. `client.rs::BackupClient::BackupRanges` 接收 `rangeLimit: isize`，调用 `NewResourceMemoryLimiter(rangeLimit)`，再用 `Arc` 包装并保存到 `MainBackupLoop::Limiter`。
2. `client.rs::MainBackupLoop::RunLoop` 在每轮为活跃 store 调用 `BackupSender::SendAsync` 时克隆该 `Arc`，因此所有 store、分片任务和重试共同竞争同一个累计额度。
3. `store.rs::startBackup` 按并发度拆分请求，把 `Arc` 克隆到工作池的每个 job；每次重试仍调用同一实例上的 `doSendBackup`。
4. `store.rs::doSendBackup` 计算 `reqRangeSize = req.SubRanges.len() + 1`，调用 `Acquire(reqRangeSize as isize)`；取得额度后调用 `client.Backup(ctx, &req)`，随后立即 `Release`。因此资源覆盖的是发起 `Backup`/取得流的阶段，不覆盖后续逐包 `Recv` 和 `CloseSend` 的整个生命周期。

一次 `Acquire(resource)` 的内部步骤是：锁住 `current`；当 `current >= threshold` 时，以同一互斥锁进入 `Condvar::wait`（等待期间释放锁，醒来后重新取得）；条件不成立后执行 `current += resource`，返回新值，最后随 guard 离开作用域解锁。一次 `Release(resource)` 则锁住计数、执行减法、广播唤醒，并在返回时解锁。

## 数据与状态

核心状态只有两个数值：不可变的 `threshold` 与受互斥锁保护的 `current`。正常调用约定下，`current` 表示已成功申请但尚未释放的资源量总和；每个调用方应以相同的 `resource` 成对执行 `Acquire` 和 `Release`。

关键不变量不是 `current <= threshold`，而是：只有观察到申请前 `current < threshold` 的线程才能新增占用。因此单次申请可以把结果推过阈值，但超阈值后不会再放行其他申请，直到释放使计数重新低于阈值。`limit_test.rs::test_resource_concurrent_limiter2` 以阈值 100、每次申请 30 验证当前实现的峰值不超过 120；这来自请求粒度 30，而非通用的固定“阈值加 20”保证。

接口使用 `isize`，实现没有校验参数。零或负阈值会使初始状态满足 `current >= threshold`，首次正向申请可能永久等待；负 `resource`、重复释放或过量释放可以把 `current` 降到负数；极端加减还受 Rust 整数溢出规则影响。当前生产调用把 `usize` 的 range 数转换为正 `isize`，但构造器的 `rangeLimit` 仍依赖上游配置保证有效。

## 依赖与调用关系

下游依赖仅为标准库同步原语：`Mutex::lock`、`Condvar::wait`、`Condvar::notify_all`。两处锁获取和等待都在锁中毒时使用 `PoisonError::into_inner` 继续工作，而不是把中毒转成业务错误。

RustCodeGraph 对 `br/pkg/backup/limit.rs::NewResourceMemoryLimiter` 的节点追踪确认了 `client_test.rs`、`parity_test.rs`、`store_test.rs` 的直接构造调用；对 `ResourceConcurrentLimiter` 的节点追踪给出 `store.rs` 和 `parity_test.rs` 的导入边。生产代码的直接文本证据补全了主链：

- `client.rs::BackupRanges` 创建限流器；`MainBackupLoop::Limiter` 持有 `Arc<ResourceConcurrentLimiter>`。
- `client.rs::RunLoop` 将限流器传给 `BackupSender::SendAsync`。
- `store.rs::startBackup` 将其克隆给工作池 job。
- `store.rs::doSendBackup` 是生产代码中执行 `Acquire`/`Release` 的位置。

`lib.rs` 将本模块公共符号重新导出到 crate 根。当前 Cargo 清单没有 feature 控制本模块，也没有为同步原语引入外部依赖。

## 错误处理与边界

本文件没有 `Result` 或显式错误类型。互斥锁或条件变量等待因其他线程 panic 而中毒时，代码通过 `unwrap_or_else(|poisoned| poisoned.into_inner())` 取回 guard 并继续更新状态；这提高了可继续运行性，但不验证中毒前状态是否仍满足业务配对关系。

实现没有超时、取消上下文或非阻塞申请接口。等待者只有在某次 `Release` 广播或发生虚假唤醒时重新检查条件；`while` 循环正确防御虚假唤醒。如果资源一直不释放，`Acquire` 可以无限等待。`notify_all` 会造成所有等待线程竞争同一把锁，且实现不承诺公平性或 FIFO 顺序。

调用边界还包括：

- 单次大申请不会因 `resource > threshold` 被拒绝；只要进入前 `current < threshold` 就会整体通过。
- 参数没有正数检查，调用方必须保证阈值和资源量合理并维持申请/释放配对。
- `doSendBackup` 在 `client.Backup` 返回错误时仍会先执行 `Release`，因为错误值只是保存在 `stream_result` 中；但若底层调用在 Rust 栈上 panic，当前代码没有 RAII 额度 guard，释放不会自动执行。

## 并发与资源生命周期

`Mutex<isize>` 使条件检查和计数修改位于同一临界区，避免检查后再加计数的竞态；`Condvar::wait` 原子地释放锁并睡眠，返回时重新持锁。`Release` 在持锁期间减计数并广播，随后函数结束释放锁，所以被唤醒线程必须等释放者退出临界区后才能继续检查。

限流器本身没有后台任务和显式关闭过程。它由 `BackupRanges` 创建，在 `MainBackupLoop`、store 发送线程和工作池 job 之间通过 `Arc` 延长生命周期；最后一个 `Arc` 释放时，`Condvar`、`Mutex` 和状态一并销毁。正常业务路径要求所有已取得资源的 job 完成配对释放，否则其他 job 可能一直阻塞，进而阻碍工作池和备份流程收尾。

广播策略与 Go 的 `sync.Cond.Broadcast` 一致，适合释放后让所有不同资源量的等待者重新判断；代价是高并发释放时可能出现惊群和不公平竞争。文件没有提供可观测指标，`Acquire` 的返回值是当前唯一暴露的瞬时占用信息。

## 与 Go 版本的对应关系

`br/pkg/backup/limit.go` 是逐符号对照来源：Go 与 Rust 都有 `ResourceConcurrentLimiter`、`NewResourceMemoryLimiter`、`Acquire` 和 `Release`，都以同一把锁保护 `current`，都在 `current >= threshold` 时循环等待，并在释放后广播。

主要语言映射如下：Go 的 `*sync.Cond` 加内部 `sync.Mutex` 对应 Rust 的内嵌 `Condvar` 和 `Mutex<isize>`；Go 构造器返回指针，Rust 构造器返回值并由调用方在生产主链中包装为 `Arc`；Go 的 `int` 对应 Rust 的 `isize`。Rust 额外明确处理锁中毒，而 Go 没有对应概念。除这些所有权和运行时差异外，阈值判断时机、整笔加减以及广播语义保持一致。

独立测试也保持对应：`limit_test.go::TestResourceConcurrentLimiter` 与 `limit_test.rs::test_resource_concurrent_limiter` 验证达到 100 后大额申请被阻塞，释放后恢复；第二组测试以 20 个并发执行单元和每次 30 的申请验证峰值允许达到但不超过 120。Rust 的 `parity_test.rs::go_rust_public_contract_matches` 还覆盖了顺序累计/释放以及限流器在 `doSendBackup` 错误和资源清理路径中的接线。

## 扩展指南

若改变阈值语义，首要修改点是 `Acquire` 的等待条件与加计数策略，并必须同步独立文件 `br/pkg/backup/limit_test.rs` 和 Go 对照测试意图。例如改成严格上界需要判断 `current + resource`，但这会改变大于阈值的单次请求是否永远无法执行，不能作为局部优化直接替换。

若增加超时、取消或 `try_acquire`，应避免只在 `limit.rs` 增加接口而不改调用链；需要检查 `store.rs::doSendBackup`、`startBackup` 的错误传播，以及 `client.rs::MainBackupLoop` 的取消和重试行为。若延长额度持有期到完整流生命周期，应把释放绑定到流/guard 的析构或明确的所有退出路径，并评估吞吐与死锁风险。

若增加输入校验，需决定兼容策略：构造器或 `Acquire` 当前不返回错误，改签名会影响 `BackupRanges`、测试及 crate 根的公共 API。至少应覆盖零/负阈值、零/负申请、过量释放、单次申请大于阈值、底层 `Backup` 失败与 panic 安全性。测试逻辑应继续放在独立的 `limit_test.rs`，不要嵌入生产源文件。

性能调整需保留“条件判断与计数修改同锁”的原子性。把 `notify_all` 改为单唤醒可能减少惊群，但在不同请求大小、公平性和持续超阈值场景下会改变进度特征，必须用并发回归证明不会让等待者饥饿。

## 验证依据

- 生产源码：`br/pkg/backup/limit.rs`（类型、构造、等待循环、计数与锁中毒处理）；`br/pkg/backup/lib.rs`（模块注册、测试模块和公共再导出）；`br/pkg/backup/client.rs`（`MainBackupLoop::Limiter`、`BackupRanges` 构造、`RunLoop` 传递）；`br/pkg/backup/store.rs`（`startBackup` 克隆及 `doSendBackup` 的资源量和申请/释放边界）。
- crate 边界：`br/pkg/backup/Cargo.toml`（library 入口、Go 包映射、依赖与无 feature 条件）。
- Go 对照：`br/pkg/backup/limit.go`、`br/pkg/backup/client.go`、`br/pkg/backup/store.go`。
- 独立测试：`br/pkg/backup/limit_test.rs`、`br/pkg/backup/limit_test.go`；补充接线证据来自 `br/pkg/backup/parity_test.rs`、`store_test.rs` 和 `client_test.rs`。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件（Rust 7,032）；`query ResourceConcurrentLimiter --kind struct` 和 `query NewResourceMemoryLimiter --kind function` 定位 Rust/Go 双版本；`node br/pkg/backup/limit.rs::ResourceConcurrentLimiter`、`node br/pkg/backup/limit.rs::NewResourceMemoryLimiter` 复核源码、导入边和直接测试调用。组合 `explore` 及独立 `callers` 查询曾在 30 秒窗口内超时，故具体生产调用点由上述已索引节点加精确源码搜索补齐，未据超时结果推断不存在调用边。
- 人工边界复核：实现是当前已接入备份请求链的真实同步组件，不是门面、生成代码或桩；文档没有把宽松阈值描述为严格容量上限，也没有声称存在当前源码未提供的取消、公平性、参数校验或 RAII 释放。
