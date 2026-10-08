# `pkg/util/watcher/event.rs`

## 文件定位

本文件是 `astersql-util-watcher` crate 的事件模型层，源码入口为 [`event.rs`](event.rs)，由 [`lib.rs`](lib.rs) 以 `pub mod event` 声明并通过 `pub use event::*` 在 crate 根重新导出。crate 的清单 [`Cargo.toml`](Cargo.toml) 指定 `lib.rs` 为库入口，运行时唯一外部依赖是 `crossbeam-channel`；该依赖由监视器实现使用，本文件自身只依赖 Rust 标准库的 `Metadata`、`PathBuf` 和 `SystemTime`。

它位于轮询式文件监视链路的模型边界：[`watcher.rs`](watcher.rs) 获取文件元数据快照、比较新旧状态并构造 `Event`，调用方从 `Watcher::Events` 接收这些值。本文件不扫描文件系统、不启动线程，也不负责通道投递。

## 核心职责

1. 用 `Op = u32` 和六个互斥位定义可组合的文件操作类别：`Create`、`Remove`、`Modify`、`Rename`、`Chmod`、`Move`。
2. 用 `OpString` 按固定顺序把已知操作位转换成稳定的可读文本。
3. 用 `FileInfo` 封装 `std::fs::Metadata`，为轮询差异比较提供目录、修改时间、大小、权限模式和文件身份查询。
4. 用 `Event` 携带事件发生时的元数据快照、受影响路径和操作位，并提供目录判断与操作匹配。
5. 用 `IsDirEventOption`、`HasOpsOption` 表达 Go 方法可在 `nil` 接收者上调用的兼容语义；Rust 的 `None` 一律返回 `false`。

这些职责都能在 [`event.rs`](event.rs) 的公开类型和函数中直接核验；事件产生策略位于 [`watcher.rs`](watcher.rs) 的 `poll_events`、`event` 和 `send_event`，不属于本文件。

## 主要符号

- `pub type Op = u32`：操作位的公开别名。它不是新类型，因此调用方可以传入任意 `u32`，类型系统不会拒绝未知位。
- `Create`、`Remove`、`Modify`、`Rename`、`Chmod`、`Move`：依次占用第 0 至第 5 位，数值分别为 `1 << 0` 到 `1 << 5`。常量名保留 Go API 风格。
- `pub fn OpString(op: Op) -> String`：按 Create、Remove、Modify、Rename、Chmod、Move 的顺序收集命中的已知位，以 `|` 连接；零值或仅含未知位时返回空字符串。
- `pub struct FileInfo`：内部仅保存私有字段 `metadata: Metadata`。`new`、`ModTime`、`Size`、`Mode`、`same_file` 是 crate 内接口；`IsDir` 是公开查询接口。
- `FileInfo::Mode`：Unix 版本返回完整 `u32` mode；非 Unix 版本只返回 `permissions().readonly()` 的布尔值。这是条件编译下的不同签名，只会有一个版本进入目标平台。
- `FileInfo::same_file`：Unix 通过 `dev` 与 `ino` 同时相等判断同一底层文件；非 Unix 恒为 `false`。
- `pub struct Event { pub FileInfo, pub Path, pub Op }`：三个字段均公开，分别保存元数据、`PathBuf` 和操作位。
- `Event::IsDirEvent(&self) -> bool`：转发到 `FileInfo::IsDir`。
- `Event::HasOps(&self, ops: &[Op]) -> bool`：只要事件操作与任一参数存在非零位交集即返回 `true`。参数本身可为组合位；其语义是“任一位命中”，不是要求完整包含组合值。
- `IsDirEventOption(Option<&Event>)`、`HasOpsOption(Option<&Event>, &[Op])`：对 `Some` 调用对应方法，对 `None` 返回 `false`。

## 执行流程

事件主流程由本文件与 [`watcher.rs`](watcher.rs) 共同完成：

1. `listForName` 调用 `fs::metadata`/目录项 `metadata`，再用 crate 内的 `FileInfo::new` 建立当前快照。
2. `poll_events` 比较前后 `FileInfo`：修改时间或大小不同产生 `Modify`，`Mode` 不同产生 `Chmod`。
3. 对旧快照中消失和新快照中出现的路径，`poll_events` 使用 `same_file` 配对；父目录相同标为 `Rename`，否则标为 `Move`。未配对项最终分别成为 `Remove` 和 `Create`。
4. `watcher.rs::event` 克隆 `FileInfo`、复制路径并写入操作位，构造本文件定义的 `Event`；`send_event` 将其送入 `Watcher::Events`。
5. 消费方可先用 `IsDirEvent` 跳过目录事件，再以 `HasOps` 匹配关心的操作。此用法由 [`watcher_test.rs`](watcher_test.rs) 的 `assert_event` 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `wait_for` 直接验证。
6. 仅需显示操作时，`OpString` 从低位到高位扫描已知常量，保证组合值输出稳定，例如 `Create | Modify | Move` 得到 `CREATE|MODIFY|MOVE`。

## 数据与状态

`FileInfo` 持有一次文件系统查询得到的 `Metadata` 快照；其 `Clone` 用于在 `State.files`、临时差异集合和 `Event` 之间传递快照，而不是重新查询路径。`ModTime`、`Size`、`Mode` 和 `same_file` 都读取该快照，因此一次比较内部保持同一观测时点的元数据。

`Event.Path` 是拥有所有权的 `PathBuf`，不借用监视器内部路径；`Event.Op` 是可组合的 `u32` 位集；`Event.FileInfo` 是构造事件时克隆的元数据。字段公开意味着下游可以读取或替换它们，本文件不维护额外不变量，也不校验 `Path` 是否仍存在、是否与 `FileInfo` 对应或 `Op` 是否只含已知位。

状态真正存放在 [`watcher.rs`](watcher.rs) 的 `State.files: HashMap<PathBuf, FileInfo>`。本文件没有全局变量、缓存或可变共享状态。

## 依赖与调用关系

- 上游模块装配：[`lib.rs`](lib.rs) 声明并重新导出 `event`，所以用户既可经 `event` 模块也可经 crate 根访问公开符号。
- 主要生产调用者：[`watcher.rs`](watcher.rs) 导入六个操作常量、`Event` 和 `FileInfo`；`listForName` 调用 `FileInfo::new`，`poll_events` 调用 `ModTime`、`Size`、`Mode`、`same_file`，`event` 构造 `Event`，`Watcher::Events` 暴露 `Receiver<Event>`。
- 直接测试调用者：[`watcher_test.rs`](watcher_test.rs) 使用 `Event::IsDirEvent` 与 `Event::HasOps` 验证异步事件；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 另外验证 `OpString` 和两个 `Option` 辅助函数。
- 下游标准库依赖：`std::fs::Metadata` 提供类型、长度、修改时间和权限信息；Unix 下 `std::os::unix::fs::MetadataExt` 提供 mode/device/inode；`PathBuf` 承载事件路径。
- crate 边界：[`Cargo.toml`](Cargo.toml) 将本目录声明为 `astersql-util-watcher`，根工作区 [`Cargo.toml`](../../../Cargo.toml) 以 `facade_util_watcher` 路径依赖引用它。仓库 Rust 搜索未发现该 facade 在其他 `.rs` 文件中的使用，因此当前可证实的 Rust 生产调用链局限于本 crate 内。

RustCodeGraph 对 `event.rs` 识别出 16 个符号，并可定位 `OpString`、`FileInfo`、`IsDirEventOption`、`HasOpsOption`；但针对这些入口的 `callers`/`callees` 查询没有返回调用边。因此上述调用关系以同 crate 源码和仓库 `rg` 结果为直接证据，不把缺失图边解释为“无人调用”。

## 错误处理与边界

- `OpString` 忽略第 0 至第 5 位以外的未知位；没有已知位时返回空字符串，不返回错误。
- `FileInfo::ModTime` 将 `Metadata::modified()` 的平台错误转换为 `None`。在 `poll_events` 中，`Option<SystemTime>` 直接参与比较，因此从 `Some` 变为 `None`（或反向变化）也会被视为 `Modify`，两个 `None` 则不会仅因时间触发修改。
- `Size`、`IsDir` 和 Unix `Mode` 都读取已有 `Metadata`，不执行新的路径 I/O；获取元数据时的 I/O 错误由 [`watcher.rs`](watcher.rs) 的 `listForName` 包装为 `WatcherError::Io`。
- 非 Unix `Mode` 只能观察只读标志，无法表示 Unix 式完整权限位差异；非 Unix `same_file` 恒为 `false`，因此当前实现不能在这些平台将消失/出现路径配成 `Rename` 或 `Move`，而会落入 `Remove` 与 `Create`。
- `Event::HasOps` 对空切片返回 `false`；对组合参数采用任一位交集语义。`IsDirEventOption(None)` 与 `HasOpsOption(None, ...)` 均返回 `false`。
- 事件携带的是轮询时的快照和路径；收到事件时路径可能已经再次变化，本 API 不承诺实时性或重新验证。

## 并发与资源生命周期

本文件自身不创建线程、不加锁、不打开文件句柄，也不持有通道。并发控制与生命周期由 [`watcher.rs`](watcher.rs) 的 `Arc<Inner>`、`Mutex`、原子变量、后台线程和 crossbeam 通道负责。

`FileInfo` 与 `Event` 都实现 `Clone`，轮询器在锁保护的快照中保存 `FileInfo`，构造事件时再克隆并转移到通道。事件一旦发送即拥有自己的 `PathBuf` 和元数据值，不借用 `Watcher`；接收方可以在监视器下一轮轮询或关闭后继续持有事件。`SystemTime` 仅作为查询返回值临时参与比较。

无缓冲事件通道带来的发送阻塞、关闭唤醒和工作线程退出属于 `Watcher` 的资源协议，而非 `Event` 的内部行为。扩展本文件时仍需考虑新增字段是否可安全克隆和跨线程传递，因为 `Watcher::Events` 会把完整 `Event` 从后台线程交给接收方。

## 与 Go 版本的对应关系

直接对照文件为 [`event.go`](event.go)：

- Rust `Op = u32` 与 Go `type Op uint32` 数值布局一致，六个常量顺序一致；Rust 使用显式移位，Go 使用 `iota`。
- Rust `OpString(op)` 对应 Go `(Op).String()`；两者都按固定顺序输出大写名称、以 `|` 分隔、对零值返回空串。Rust 是自由函数，因为类型别名不能实现固有方法或外部 trait。
- Go `Event.FileInfo` 使用 `os.FileInfo` 接口，Rust 用私有 `Metadata` 包装 `FileInfo`，对监视器只暴露所需查询。Go 路径是 `string`，Rust 路径是能保留平台原生表示的 `PathBuf`。
- Go 的 `(*Event).IsDirEvent`、`(*Event).HasOps` 会显式接受 `nil` 接收者并返回 `false`。Rust 的实例方法要求 `&Event`，因此另设两个接收 `Option<&Event>` 的兼容辅助函数。
- Go `HasOps(ops ...Op)` 是可变参数，Rust `HasOps(&[Op])` 用切片表达相同的“任一操作位命中”行为。
- Rust 为轮询差异逻辑增加了 `FileInfo` 的 `ModTime`、`Size`、`Mode`、`same_file` 接口；Go watcher 直接调用 `os.FileInfo` 的对应方法以及 `os.SameFile`。Unix 的 device/inode 比较与 Go 在支持平台上的同文件判定目标一致，但 Rust 非 Unix 版本恒 false，是需要保留关注的平台差异。

[`watcher_test.go`](watcher_test.go) 与 [`watcher_test.rs`](watcher_test.rs) 都验证 Create、Modify、Chmod、Rename、Remove、Move 序列以及目录事件过滤；Rust 的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 额外固定了组合字符串与 `None` 兼容语义。

## 扩展指南

- 新增操作类型时，应在 `event.rs` 增加不冲突的位常量，并同步更新 `OpString` 的有序映射；同时在 [`event.go`](event.go) 保持数值和文本顺序一致，并在独立的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 增加单值、组合值、零值/未知位断言。
- 若让 watcher 产生新操作，还必须修改 [`watcher.rs`](watcher.rs) 的 `poll_events` 或相关快照逻辑，并扩展 [`watcher_test.rs`](watcher_test.rs)；不要把测试嵌入 `event.rs`。
- 修改 `FileInfo` 的比较依据前，应明确平台能力。尤其是 `Mode` 的跨平台返回类型和 `same_file` 的非 Unix 降级行为，会直接影响 `Chmod`、`Rename`、`Move` 分类；应添加带 `#[cfg]` 的独立测试并核对 Go 行为。
- 若给 `Event` 新增字段，需同步 `watcher.rs::event` 的构造、所有结构体字面量、crate 的公开 API 文档和 Go 对照；评估字段克隆成本、后台线程到接收方的所有权，以及公开字段带来的兼容性。
- 若希望拒绝未知位或实现更强的位集不变量，应考虑从 `type Op = u32` 迁移为新类型。这会是公开 API 兼容性变化，不能只改格式化函数。
- 性能风险主要来自每轮元数据快照克隆、rename/move 候选的两层配对，以及事件字段的克隆。仅调整本文件时不要误称已经优化 watcher；性能结论需在 `poll_events` 的真实路径上验证。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/watcher` 定位到本模块 8 个 Go/Rust 文件；`node --file pkg/util/watcher/event.rs --offset 1 --limit 260` 返回完整 145 行与 16 个符号；`query` 精确定位 `OpString`、`FileInfo`、`IsDirEventOption`、`HasOpsOption`。相应 `callers`/`callees` 无返回，已在“依赖与调用关系”中声明限制。
- 生产源码：[`event.rs`](event.rs)（目标模型）、[`watcher.rs`](watcher.rs)（构造、差异分类与发送）、[`lib.rs`](lib.rs)（模块声明与重新导出）。
- crate 配置：[`pkg/util/watcher/Cargo.toml`](Cargo.toml) 与根 [`Cargo.toml`](../../../Cargo.toml)，确认库入口、依赖、Go 包迁移元数据和工作区别名。
- Go 对照：[`event.go`](event.go)（操作位、字符串、事件方法）、[`watcher.go`](watcher.go)（`os.FileInfo`/`os.SameFile` 在轮询中的实际使用）。
- 独立测试：[`watcher_test.rs`](watcher_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 和 [`watcher_test.go`](watcher_test.go)。本任务按计划仅作纯文档分析，未运行 Cargo；测试文件用于核验既有意图与边界，不作为本次新执行结果。
- 仓库搜索：`rg` 确认事件模型的 Rust 引用集中在当前 crate 的 `watcher.rs` 与两个独立测试模块，工作区只在根 `Cargo.toml` 声明 `facade_util_watcher` 路径依赖。
- 结构验收命令：`test -f pkg/util/watcher/event.rs.md && test "$(rg -c '^## (文件定位|核心职责|主要符号|执行流程|数据与状态|依赖与调用关系|错误处理与边界|并发与资源生命周期|与 Go 版本的对应关系|扩展指南|验证依据)$' pkg/util/watcher/event.rs.md)" -eq 11`。交付前应以退出码 0 确认固定章节恰好 11 个。
