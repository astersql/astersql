# [`dumpling/context/stubs.rs`](./stubs.rs)

## 文件定位

`stubs.rs` 是 `astersql-dumpling-context` crate 对 Go 标准库 `context` 的本地最小替身，而不是 dumpling 自定义上下文包装层本身。crate 入口 `dumpling/context/lib.rs` 以私有模块 `stubs` 装入本文件；`dumpling/context/context.rs` 再以 `crate::stubs as gcontext` 使用它，把这里的取消上下文与 dumpling logger 组合为公开的包装 `Context`。

`dumpling/context/Cargo.toml` 将该目录声明为库 crate，入口为 `lib.rs`，唯一直接依赖是相邻的 `astersql-dumpling-log`。本文件自身只使用 Rust 标准库的 `Arc`、`AtomicBool` 和原子内存序，不引入异步运行时。`lib.rs` 将包级 `Background`、`WithCancel` 分别重导出为 `GoBackground`、`GoWithCancel`，并直接重导出 `Context` 为 `GoContext`、`CancelFunc` 与 `Canceled`；因此模块虽不公开，其 API 仍构成 crate 的公开兼容面。

## 核心职责

本文件只实现 dumpling 当前需要的 Go `context` 取消子集：创建永不主动取消的根上下文、从父上下文派生可取消子上下文、显式触发取消、查询当前节点或祖先是否取消，以及把取消状态映射成固定错误。核心契约由 `Context::{Background, TODO, WithCancel, Done, Err}`、`CancelFunc::call` 和自由函数 `Background`、`WithCancel` 共同提供。

它不是完整的 Go `context`：`Done()` 返回即时布尔值而非可等待的 channel；没有 deadline、timeout、value 传递、取消原因或任务唤醒机制。调用方必须主动轮询。文件头注释和现有实现都将这一限制写成当前事实，扩展时不能把它误当成通用异步取消设施。

## 主要符号

- `Canceled`：无字段错误类型，实现 `Clone`、`Debug`、`PartialEq`、`Eq`、`Display` 与 `std::error::Error`。`Display` 固定输出 `context canceled`，供 Go/Rust 语义比对。
- `CancelFunc { flag: Arc<AtomicBool> }`：指向某个派生节点自身的取消位。`call(&self)` 用 `store(true, Ordering::SeqCst)` 置位；接收共享借用使同一 handle 可重复调用，行为幂等。`Clone` 产生指向同一取消位的另一个 handle。
- `Context { cancelled, parent }`：`cancelled: Arc<AtomicBool>` 是节点自身状态，`parent: Option<Arc<Context>>` 保存父链。`Clone` 共享状态和父链；`Default` 来自字段默认值，得到未取消且无父节点的根状上下文，其观测行为与 `Background()` 相同。
- `Context::Background()`：显式构造未取消、无父节点的根上下文。
- `Context::TODO()`：直接委托 `Background()`；当前实现不区分 TODO 与 Background。
- `Context::WithCancel(&parent)`：新建独立取消位，把 `parent.clone()` 放入 `Arc` 作为父节点，并返回共享该新取消位的 `CancelFunc`。
- `Context::Done()`：先读取自身取消位；若未取消则递归查询父节点，根节点最终返回 `false`。
- `Context::Err()`：通过 `Done()` 统一判断本地或祖先取消；已取消返回 `Some(Canceled)`，否则返回 `None`。
- 自由函数 `Background()` 与 `WithCancel(parent: Context)`：提供接近 Go 包函数的调用形态；后者消费传入值，但内部仍通过借用调用关联函数，返回的子节点保存父值的共享克隆。

## 执行流程

典型路径从 `dumpling/context/context.rs::Background` 开始：它调用本文件自由函数 `Background()`，后者转发到 `Context::Background()`，再与 `log::Zap()` 组合为 dumpling 包装上下文。调用包装层 `Context::WithCancel()` 时，`context.rs` 调用本文件的 `Context::WithCancel(&self.Context)`，获得子上下文和 cancel handle，同时保留原 logger。

派生时，子节点创建值为 `false` 的独立 `AtomicBool`，父上下文被克隆后保存在 `parent` 链上，`CancelFunc` 克隆子节点的 `cancelled` 指针。调用 `CancelFunc::call()` 只把该节点的原子位设为 `true`；它不会改写父节点。之后查询子节点 `Done()` 会先观察本地位，未命中才沿父链递归，因此父取消向子传播，而子取消不会反向传播给父。`Err()` 复用同一查询结果并构造 `Canceled`。

实际 dumpling 路径将布尔查询用于协作式停止。例如 `dumpling/export/dump.rs` 从包装层 `Background().WithCancel()` 保存 cancel handle，并在导出流程多处查询 `tctx.Done()`；`dumpling/export/status.rs` 同样派生上下文并在状态循环中检查取消。这里没有阻塞等待或自动调度，停止响应速度取决于上层检查频率。

## 数据与状态

每个 `Context` 节点只有一个单向状态转移：`cancelled` 从 `false` 变为 `true`，没有复位操作。所有该节点的 `Context` 克隆和 `CancelFunc` 克隆共享同一个 `Arc<AtomicBool>`，所以任一 cancel handle 置位后，所有克隆都会观察到取消。`SeqCst` 为这些读写提供最强的全局原子顺序；实现没有携带取消时附加数据。

父关系由 `Option<Arc<Context>>` 形成只向祖先的不可变链。派生节点拥有父快照的共享克隆，而父克隆内部的原子位仍与原父共享，因此后续父取消可以被子节点观察。没有子指针，取消动作无需遍历或修改后代；代价是每次未命中本地取消时，`Done()` 都会按深度递归读取祖先。

`Canceled` 是零大小值，`Err()` 每次按状态新建它，不保存错误对象。删除最后一个子上下文、父链克隆和 cancel handle 后，相应 `Arc` 自动释放；父子结构是单向的，不会由本文件形成引用环。

## 依赖与调用关系

下游依赖仅为 `std::sync::Arc`、`std::sync::atomic::{AtomicBool, Ordering}` 以及用于错误展示的 `std::fmt`/`std::error` trait。RustCodeGraph 显示自由函数 `WithCancel` 调用关联函数 `Context::WithCancel`，`Context::Err` 调用 `Context::Done`，而 `Done` 在存在父节点时递归调用自身。

直接上游是 `dumpling/context/context.rs`：其 `Background` 调用本文件 `Background`，其包装 `Context::WithCancel` 调用本文件关联函数 `WithCancel`，包装层 `Done`/`Err` 分别转发到底层同名方法。`dumpling/context/lib.rs` 是公开边界，将标准库风格符号以 `GoBackground`、`GoWithCancel`、`GoContext`、`CancelFunc`、`Canceled` 暴露。再上游包括 `dumpling/export/dump.rs`、`status.rs`、`http_handler.rs`、`consistency.rs` 和 `conn.rs` 等，它们通过公开包装上下文查询或触发取消，而非直接访问私有字段。

## 错误处理与边界

本文件所有构造和取消操作都是无返回错误的内存操作。唯一领域错误是 `Canceled`：只有 `Err()` 观察到本地或祖先取消时才返回它，文本严格为 `context canceled`；实现不区分由哪个祖先取消，也不表示 deadline exceeded。未取消时 `Err()` 必须为 `None`。

边界语义包括：重复 `call()` 不 panic 且状态保持取消；丢弃未调用的 `CancelFunc` 不会触发取消；丢弃某个 handle 克隆也不影响其余 handle；取消子节点不取消父节点；替换到另一棵 context 树后只观察新树。由于 `Done()` 是递归布尔查询，调用方不能像 Go 那样对 `<-ctx.Done()` 做阻塞选择，也不会收到唤醒通知。极深且人为构造的父链还会增加线性查询成本及递归栈深度。

## 并发与资源生命周期

`Context` 和 `CancelFunc` 由 `Arc<AtomicBool>` 组成，可跨克隆共享取消状态；取消与查询使用 `SeqCst` 原子操作，不需要锁。并发调用多个 `CancelFunc::call()` 是幂等的，并发查询要么尚未看到置位、要么看到永久置位后的状态，不存在恢复到未取消的路径。

父传播是查询时计算，不会在取消时启动线程、任务或广播事件。因此本文件不管理线程、异步任务、channel、锁、文件或网络资源；它也不会替调用方清理这些资源。资源所有权完全由 `Arc` 引用计数管理。`CancelFunc` 没有 `Drop` 副作用，这一点由 `dumpling/context/parity_test.rs::contract_resource_cleanup` 明确锁定。

## 与 Go 版本的对应关系

Go 同路径 `dumpling/context/context.go` 并不重新实现标准库，而是嵌入 `context.Context`，在 `Background()` 中调用 `context.Background()`，在 `(*Context).WithCancel()` 中调用 `context.WithCancel(c.Context)`。Rust 的 `context.rs` 保留这层 dumpling 包装结构，本文件则补足 Rust 环境中所需的标准库替身。

已对齐的可见语义包括：根上下文初始未取消；`WithCancel` 返回派生上下文和显式 cancel 函数；取消幂等；父取消可由子观察；子取消不反向影响父；`Err()` 在取消后显示 `context canceled`。`dumpling/context/parity_test.rs` 的 `contract_error_cancel`、`contract_boundary_chaining` 和 `contract_resource_cleanup` 分别覆盖这些行为。

未对齐且当前未声称支持的 Go 语义包括：`Done() <-chan struct{}` 的关闭与 select/wakeup 行为、deadline/timeout、context value、`Deadline()`、取消原因，以及 Go 运行时维护的后代通知结构。Rust 自由函数 `WithCancel` 还采用按值接收父 `Context`，而 Go 包函数接收接口值；这不改变共享取消位的传播结果，但属于 API 形态差异。`TODO()` 目前仅是 Background 的别名，且没有单独的包级自由函数重导出。

## 扩展指南

若只需增加现有取消模型上的行为，应优先修改本文件相应符号，并保持 `dumpling/context/context.rs` 包装转发及 `dumpling/context/lib.rs` 重导出一致。例如增加取消原因会涉及 `Context` 状态、cancel handle 的写入接口、`Err()` 返回模型以及公开导出；新增 deadline/value 则不能简单塞进当前布尔位，必须先明确线程安全、克隆和继承规则。

若要支持真正可等待的取消通知，应把它视为 API 与运行模型变更：当前所有调用者按布尔值轮询，不能在不审计 `dumpling/export` 调用点的情况下把 `Done()` 改为 future 或 channel。还应评估是否允许引入异步依赖；`Cargo.toml` 当前刻意没有 tokio 等运行时依赖。

测试必须放在独立测试文件，不能内嵌到 `stubs.rs`。最接近的同步位置是 `dumpling/context/parity_test.rs`：正常构造、父子传播、重复取消、错误文本和 drop 语义都应继续在那里扩充；若修改影响导出循环，还需同步 `dumpling/export/status_test.rs`、`dump_test.rs` 或相应的独立测试。兼容风险主要是改变公开重导出的签名或 Go 对齐文本；性能风险主要是加深父链后的递归查询，以及将当前无锁原子路径替换为锁或分配更重的通知结构。

## 验证依据

- RustCodeGraph 索引状态：仓库索引包含 7,032 个 Rust 文件；`dumpling/context/stubs.rs` 被识别为 109 行、13 个符号的 Rust 文件。
- RustCodeGraph 源码/符号查询：`node --file dumpling/context/stubs.rs`；`node dumpling/context/stubs.rs::{CancelFunc,WithCancel,Done,Err,call}`。调用边确认自由函数 `WithCancel` → 关联函数 `WithCancel`，`Err` → `Done`，以及 `Done` 的父链递归。
- RustCodeGraph 直接入口查询：`dumpling/context/context.rs::{WithCancel,Done,Err}`，并读取 `context.rs` 与 `lib.rs` 的索引源码，确认包装转发和公开重导出。
- crate 边界：`dumpling/context/Cargo.toml`，确认库入口、Go 包元数据和唯一直接依赖 `astersql-dumpling-log`。
- Go 对照：`dumpling/context/context.go`，确认 Go 包装层使用标准库 `context.Background`/`context.WithCancel` 且保留 logger。
- 独立测试：`dumpling/context/parity_test.rs`，其中 `contract_normal_paths`、`contract_boundary_chaining`、`contract_error_cancel`、`contract_resource_cleanup` 覆盖未取消状态、树替换、父子传播、幂等取消、错误文本及 handle drop 行为。
- 上游使用搜索：`dumpling/export/dump.rs`、`status.rs`、`http_handler.rs`、`consistency.rs`、`conn.rs` 及相关独立测试，确认取消状态在导出与状态流程中以主动查询方式消费。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前另执行任务指定的 11 章节结构检查并人工复核上述限制未被描述成已支持能力。
