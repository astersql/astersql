# `pkg/util/disk/tracker.rs`

## 文件定位

本文件是 `astersql-util-disk` crate 的磁盘用量跟踪兼容门面。模块入口 `pkg/util/disk/lib.rs` 以 `pub mod tracker` 声明它，并通过 `pub use tracker::*` 把这里的三个公开名字提升到 crate 根，因此调用者可以写 `disk::Tracker`、`disk::NewTracker` 和 `disk::NewGlobalTracker`。它不实现独立的磁盘计量算法，而是复用 `astersql-util-memory` 的跟踪器实现；`pkg/util/disk/Cargo.toml` 中的 `tidb-memory = { package = "astersql-util-memory", path = "../memory" }` 以及 `lib.rs` 的 `memory` 转发模块共同建立了这条依赖。

该门面的意义是保留“磁盘用量”这一领域名称，同时让内存与磁盘计量共享同一套树形记账、配额和并发语义。真实使用点包括 `pkg/util/chunk/row_in_disk.rs::DataInDiskByRows::New`、`pkg/util/chunk/row_container.rs::RowContainer::New`、`pkg/util/chunk/chunk_in_disk.rs::NewDataInDiskByChunks`，以及 `pkg/util/cteutil/storage.rs::GetDiskTracker` 所在的 CTE 存储接口。

## 核心职责

本文件只有两项职责：

1. 用 `pub type Tracker = crate::memory::tracker::Tracker` 为底层跟踪器提供磁盘领域的类型名；这是类型别名，不是新类型，不增加字段、封装层或运行时转换。
2. 用 `pub use crate::memory::tracker::{NewGlobalTracker, NewTracker}` 原样再导出普通和全局跟踪器构造函数；调用直接进入 `pkg/util/memory/tracker.rs`，本文件不插入校验、错误映射或额外初始化。

因此，“磁盘”只表示调用场景。实际字节数仍由使用者在写入、截断或释放磁盘数据时调用继承来的 `Tracker` 方法记账；本文件本身不读取文件系统容量，也不观察临时文件大小。

## 主要符号

- `pub type Tracker = crate::memory::tracker::Tracker`：公开类型别名。它完整暴露底层 `Tracker` 的方法集合，例如 `Consume`、`BytesConsumed`、`AttachTo`、`Label` 和 `GetBytesLimit`；具体定义与不变量归 `pkg/util/memory/tracker.rs::Tracker` 所有。
- `pub use ...::NewTracker`：函数签名来自底层实现，即 `fn NewTracker(label: i32, bytesLimit: i64) -> Box<Tracker>`。`label` 用于识别计量节点；`bytesLimit <= 0` 表示无上限。构造过程安装默认硬限动作，并把节点标记为非全局 tracker（`pkg/util/memory/tracker.rs::NewTracker`）。
- `pub use ...::NewGlobalTracker`：函数签名同样为 `(i32, i64) -> Box<Tracker>`，但底层把 `isGlobal` 设为 `true`，用于减少全局节点维护 children 所造成的锁竞争（`pkg/util/memory/tracker.rs::NewGlobalTracker`）。

文件没有模块级常量、结构体、trait、`impl`、私有函数或条件编译项。三个名字全部是公开 API，并由 `pkg/util/disk/lib.rs` 再导出。

## 执行流程

以 `pkg/util/chunk/row_container.rs::RowContainer::New` 为例，实际流程如下：

1. 容器调用 `disk::NewTracker(memory::LabelForRowContainer, -1)`，领域门面把调用直接解析到 `memory::tracker::NewTracker`。
2. 底层构造 `Box<Tracker>`，保存 label，将非正配额归一化为无上限配置，安装默认硬限动作，并将其标记为普通 tracker。
3. 调用者通常把 `Box` 转为 `Arc`，保存在容器的 `diskTracker` 字段中；这一步发生在调用方，不属于本文件。
4. 后续落盘实现按实际写入或释放的字节调用该 tracker 的记账方法。上层可通过 `GetDiskTracker` 取得共享句柄，并以 `BytesConsumed` 读取累计值；`pkg/util/cteutil/storage.rs::GetDiskBytes` 展示了这一读取链。

`pkg/util/chunk/chunk_in_disk.rs::NewDataInDiskByChunks` 则直接把返回的 `Box<Tracker>` 解包存为 `disk::Tracker`，说明类型别名没有要求固定的所有权容器。`NewGlobalTracker` 走相同构造主线，差异只在底层全局标志和相应的 children 管理策略。

## 数据与状态

本文件不拥有任何状态。所有状态都在别名指向的 `memory::tracker::Tracker` 内，包括 label、硬/软字节上限、当前与峰值消费、父指针、子节点集合、超限动作和全局节点标志（见 `pkg/util/memory/tracker.rs::Tracker`）。

需要保持的关键语义是：

- `label: i32` 是诊断和树节点分类标识，不代表磁盘路径或文件描述符。
- `bytesLimit <= 0` 表示无上限；迁移测试 `pkg/util/disk/migration_aster_unit_test.rs::check_and_create_dir_and_tracker_aliases_match_go_behavior` 明确验证 `NewGlobalTracker(8, 0).GetBytesLimit() == -1`。
- 普通 tracker 可以进入父子树，子节点消费会向祖先累计；全局 tracker 使用底层专门的全局行为。
- 文件大小与 tracker 数值之间没有自动同步关系。调用者若漏记或重复记账，门面无法检测或修正。

## 依赖与调用关系

下游依赖只有 `crate::memory::tracker`：`Tracker`、`NewTracker`、`NewGlobalTracker` 的实现均来自此模块。crate 级依赖由 `pkg/util/disk/Cargo.toml` 的 `tidb-memory` 项提供，`pkg/util/disk/lib.rs::memory` 将该依赖转发为本 crate 内的 `crate::memory` 路径。该 Cargo manifest 没有为 tracker 声明专属 feature；文件也没有 `cfg` 分支。

已核对的直接使用关系包括：

- `pkg/util/chunk/row_in_disk.rs::DataInDiskByRows::New` 创建按行落盘容器的 tracker，并由 `GetDiskTracker` 返回共享句柄。
- `pkg/util/chunk/row_container.rs::RowContainer::New` 分别创建内存 tracker 和磁盘 tracker，spill 后由同一容器对外报告两类用量。
- `pkg/util/chunk/chunk_in_disk.rs::NewDataInDiskByChunks` 创建按 chunk 落盘的 tracker，文件名还使用其 `Label()`；`GetDiskTracker` 返回引用。
- `pkg/util/cteutil/storage.rs::GetDiskTracker` 从行容器转发磁盘 tracker，`GetDiskBytes` 随后调用 `BytesConsumed()`。

RustCodeGraph 对 `pkg/util/disk/tracker.rs` 给出的文件级使用边是 `pkg/util/memory/action.rs`；精确源码核对显示后者直接依赖的是同一个底层 `memory::tracker::Tracker`，并非通过磁盘门面调用。因此该边只能证明别名目标与动作接口共享类型，不能当作磁盘门面的真实上游调用。真实 Rust 调用点以上述 `disk::...` 源码引用为准。

## 错误处理与边界

本文件没有 `Result`、`Option` 分支、panic 路径或错误类型，也不捕获底层错误。两个构造函数在当前底层签名中直接返回 `Box<Tracker>`；别名和再导出不会产生额外失败点。

边界应按底层契约理解：非正配额是“无限制”而不是非法参数；label 没有在门面中做范围检查；构造 tracker 不创建临时目录、不打开文件，也不保证调用方后续的字节记账准确。磁盘 I/O 错误属于 `row_in_disk.rs`、`chunk_in_disk.rs` 等存储实现的职责，不能归因于本文件。

由于这是类型别名，底层 `Tracker` 的公开 API 或行为变化会立即影响 `disk::Tracker`，不存在版本隔离层。若未来需要磁盘特有校验或错误，继续使用类型别名可能不足，应先评估是否改为新类型包装；这会是 API 兼容性变化。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件或析构逻辑。生命周期完全由返回值的所有者管理：调用者可保留 `Box<Tracker>`，也可像 `DataInDiskByRows` 和 `RowContainer` 那样转为 `Arc<Tracker>` 共享。

并发保证来自底层 `memory::tracker::Tracker`。其源码说明只有消费读取、消费更新和挂接等指定树操作是线程安全的，其他树操作不能据此推断为并发安全；内部使用原子计数和互斥保护部分状态。普通 tracker 与全局 tracker 的 children 管理策略不同，调用者不应通过“磁盘 tracker”这一名称推断出额外同步保证。

tracker 的销毁也不会自动删除临时文件或把剩余计数归零。临时文件清理由磁盘容器及 `tempDir` 模块负责；父子挂接、释放记账和共享句柄回收则由使用者遵循底层 tracker API 完成。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/disk/tracker.go`。两边结构逐项对应：

- Go `type Tracker = memory.Tracker` 对应 Rust `pub type Tracker = crate::memory::tracker::Tracker`，均为真正的类型别名。
- Go `var NewTracker = memory.NewTracker` 对应 Rust 的函数再导出 `pub use ...::NewTracker`。
- Go `var NewGlobalTracker = memory.NewGlobalTracker` 对应 Rust 的函数再导出 `pub use ...::NewGlobalTracker`。

Rust 没有复制或简化 Go 的磁盘算法，因为 Go 文件本来也没有独立算法。语言层面的差异是 Go 构造函数以函数变量别名暴露，Rust 以 `pub use` 建立同一函数项的公开路径；Rust 返回 `Box<Tracker>`，调用者常显式转换为 `Arc`，而 Go 指针天然可由多个持有者共享。`pkg/util/disk/migration_aster_unit_test.rs::check_and_create_dir_and_tracker_aliases_match_go_behavior` 对构造后的 label、正配额和零配额归一化进行了 Rust 侧回归验证。

## 扩展指南

若只需新增磁盘使用场景，应继续通过 `disk::NewTracker` 创建节点，在真实 I/O 成功、回滚和清理路径上对称记账，并把 tracker 接入合适的父节点；不要在本文件复制 `memory::Tracker` 逻辑。新增测试应放在独立 `*_test.rs` 文件中：门面契约可扩展 `pkg/util/disk/migration_aster_unit_test.rs`，底层树形记账、配额、动作或并发语义应同步修改并验证 `pkg/util/memory/tracker_test.rs`（必要时还包括分片的 `tracker_*_aster_unit_test.rs`）。

若要新增公开构造器或别名，需要同时检查四处：`pkg/util/memory/tracker.rs` 的真实实现、`pkg/util/disk/tracker.rs` 的再导出、`pkg/util/disk/lib.rs` 的 crate 根暴露，以及 `pkg/util/disk/tracker.go` 的 Go 对照。还应更新 `pkg/util/disk/migration_aster_unit_test.rs` 验证别名没有改变 label、配额或全局/普通节点语义。

需要重点评估的风险包括：类型别名导致的 API 全量暴露、普通/全局节点选择错误引起的锁竞争或树统计偏差、写入与释放不对称导致的长期虚高/负值，以及用 tracker 配额误代替真实磁盘容量检查。磁盘容量探测、临时目录管理和 I/O 错误处理应放在各自模块，而不是塞入该兼容门面。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/util/disk` 确认模块、Go 对照与独立测试；`node --file pkg/util/disk/tracker.rs` 核对本文件全部 27 行及文件级使用边。
- RustCodeGraph 源码节点：`pkg/util/memory/tracker.rs` 的 `Tracker`、`NewTracker`、`NewGlobalTracker`；`pkg/util/disk/lib.rs` 的模块声明和再导出；`pkg/util/chunk/row_in_disk.rs`、`row_container.rs`、`chunk_in_disk.rs` 及 `pkg/util/cteutil/storage.rs` 的真实使用链。
- crate 边界：`pkg/util/disk/Cargo.toml` 的 package、lib path、`tidb-memory` 依赖和无专属 feature 配置。
- Go 对照：`pkg/util/disk/tracker.go` 的类型别名及两个函数别名。
- 独立 Rust 测试：`pkg/util/disk/migration_aster_unit_test.rs::check_and_create_dir_and_tracker_aliases_match_go_behavior`；底层详细行为另由 `pkg/util/memory/tracker_test.rs` 和 `pkg/util/memory/tracker_4_aster_unit_test.rs` 覆盖。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前以固定 11 个二级标题的结构命令校验文档形状，并人工复核所有“已支持”陈述均可回指上述源码或测试。
