# `pkg/objstore/mockobjstore/objstore_mock.rs`

## 文件定位

本文件实现 `astersql-objstore-mockobjstore` crate 的核心测试替身：一个严格的 `storeapi::Storage` Mock。crate 入口 `pkg/objstore/mockobjstore/lib.rs` 以 `#[path = "objstore_mock.rs"]` 声明 `mockobjstore` 模块并公开重导出其符号；workspace 根 `Cargo.toml` 再把该 crate 绑定为 `facade_objstore_mockobjstore`，由 `pkg/lib.rs` 暴露在 `objstore::mockobjstore` 门面下。

它不连接本地文件系统或云对象存储，也不保存真实对象数据。测试先通过 `MockStorage::EXPECT` 登记每个 `Storage` 方法将返回的结果，受测代码再把它当作 `dyn Storage` 调用；Mock 记录可观察参数并按方法各自的 FIFO 队列消费结果。RustCodeGraph 将本文件识别为 58 个符号，并显示文件被 41 个文件纳入依赖图；仓库内可确认的直接构造与行为验证集中在独立测试 `pkg/objstore/mockobjstore/migration_aster_unit_test.rs`。

## 核心职责

- `MockStorage` 完整实现 `pkg/objstore/storeapi/storage.rs` 中要求的 12 个 `Storage` 方法：`WriteFile`、`ReadFile`、`FileExists`、`DeleteFile`、`Open`、`DeleteFiles`、`WalkDir`、`URI`、`Create`、`Rename`、`PresignFile` 和 `Close`。
- `MockStorageMockRecorder` 为每个方法登记一次预设结果；同一方法登记多次时，以 `VecDeque` 保持先进先出。
- `Call` 把真实调用的关键参数复制成自有快照，使测试能通过 `calls()` 检查跨方法的全局调用顺序。`Context` 没有进入快照，也不会参与行为判断。
- 严格性由两端共同保证：未登记便调用会立即 panic；调用完成后，`verify()` 会断言所有已登记期望均已耗尽。
- `WalkDir` 额外模拟目录遍历：录制器保存条目序列，执行时依次调用测试传入的回调，然后返回预设的最终结果。

该实现只负责确定性的测试编排，不模拟权限、网络、持久化、重试、取消或对象存储一致性。

## 主要符号

- `type MockResult<T> = Result<T, String>`：内部预设结果。错误用字符串保存，在调用时通过 `anyhow!(message)` 转为 crate 公共 API 使用的 `anyhow::Error`。该别名未公开。
- `pub enum Call`：可比较、可克隆的调用快照。路径和 URI 使用自有 `String`，批量路径和写入内容使用 `Vec`，超时保留为 `Duration`；`WriterOption`、`ReaderOption`、`WalkOption` 被压平成元组以保留其字段值。
- `struct State`：内部共享状态，包含一个全局 `calls: Vec<Call>`、`Close` 的未消费计数，以及其他 11 个方法各自的期望队列。它不对外公开。
- `pub struct MockStorage`：持有 `Arc<Mutex<State>>` 的严格 Mock；`Clone` 只克隆 `Arc`，所以所有克隆共享期望和调用历史。
- `pub struct MockStorageMockRecorder`：与 `MockStorage` 共享同一个 `State`；其同名方法只入队结果，不登记参数匹配规则。
- `pub fn NewMockStorage() -> MockStorage`：构造空状态。空状态下任何 `Storage` 方法都没有期望，首次调用即失败。
- `fn lock(...)`：统一取得互斥锁。即使锁已因其他线程 panic 而毒化，也取回内部状态继续测试。
- `fn take<T>(...)`：从指定方法队列头部取一项；空队列 panic，`Ok` 原样返回，`Err(String)` 转成 `anyhow::Error`。
- `walk_option`、`writer_option`、`reader_option`：把借用的可选配置转为自有、可比较的调用快照。
- `MockStorage::{EXPECT, ISGOMOCK, calls, verify}`：分别取得录制器、提供 GoMock 风格标记、复制调用历史、检查残留期望。`verify` 不是析构钩子，测试必须显式调用。
- `impl Storage for MockStorage`：Mock 的执行入口。公共方法签名由 `Storage: Send + Sync` 契约约束。

## 执行流程

1. 测试调用 `NewMockStorage()` 得到空 Mock，再调用 `EXPECT()` 得到共享同一状态的录制器。
2. 每次 `expect.Method(...)` 在锁内把结果压入该方法的队尾；`Close` 没有返回值，因此只增加计数。
3. 受测代码通过 `Storage` trait 调用 Mock。普通方法先取得锁，把一个 `Call` 追加到全局历史，再用 `take` 弹出本方法队首的预设结果。
4. `Open` 和 `Create` 的队列存储 `Box<dyn Reader + Send>` / `Box<dyn Writer + Send>`，返回时向上转型为 trait 所要求的 `Box<dyn Reader>` / `Box<dyn Writer>`。调用记录同时保存 option 的字段快照。
5. `WalkDir` 在锁内记录调用并取走 `(entries, result)`，随即释放锁；随后按登记顺序执行回调。任一回调错误会通过 `?` 立即返回，只有全部条目成功时才返回登记的最终结果。
6. `Close` 记录调用后检查剩余计数大于零并减一；`URI` 类似普通方法，但队列元素不带错误分支。
7. 测试可随时调用 `calls()` 取得历史副本，最后调用 `verify()` 确认没有遗漏任何登记调用。

这里的 FIFO 是“每个方法自己的期望顺序”，不是跨方法的调度约束。例如先登记 `ReadFile` 再登记 `WriteFile`，实际仍可先执行 `WriteFile`；跨方法真实顺序只能事后由 `calls()` 断言。

## 数据与状态

唯一可变状态位于 `State`，由 `Arc<Mutex<_>>` 共享。`calls` 是所有方法共用的追加日志；各方法的期望互相独立，因此消费一个方法不会移动其他方法的队列。`MockStorage::clone`、`EXPECT()` 返回的录制器和原 Mock 都指向同一状态，不会复制一套独立脚本。

参数快照刻意取得所有权：`WriteFile` 复制字节切片，`DeleteFiles` 复制字符串切片，路径复制为 `String`；三类 option 则只复制当前 Rust 结构已定义的字段。这保证调用返回后仍能稳定比较，也意味着大字节写入会产生与数据大小线性相关的测试内存和复制成本。

`Context` 参数在全部方法中命名为 `_ctx`，没有被读取或记录；Mock 因而不会响应取消或截止时间。返回的 reader/writer 对象由测试在登记期望时创建，Mock 只转移所有权，不管理其关闭状态。`AccessRequestSnapshot` 没有在本文件覆盖，沿用 `Storage` trait 的默认 `None`。

## 依赖与调用关系

- 上游契约：`pkg/objstore/storeapi/storage.rs::Storage` 定义方法集合并要求实现类型 `Send + Sync`；同文件的 `ReaderOption`、`WriterOption` 和 `WalkOption` 决定调用快照字段。
- 流对象：`pkg/objstore/objectio` 通过 crate 依赖名 `objectio` 提供 `Context`、`Reader` 和 `Writer`。`Create` / `Open` 返回测试预置的 trait object。
- 错误层：`anyhow::{Result, anyhow}` 提供 `Storage` 方法的统一错误返回，并承载预置字符串错误。
- 装配层：`pkg/objstore/mockobjstore/lib.rs` 声明并重导出本模块；根 `Cargo.toml` 的 `facade_objstore_mockobjstore` 和 `pkg/lib.rs::objstore::mockobjstore` 形成仓库公共门面。
- 已验证的直接调用者：`pkg/objstore/mockobjstore/migration_aster_unit_test.rs` 调用 `NewMockStorage`、`EXPECT`、全部 `Storage` 方法、`calls` 和 `verify`，覆盖成功、错误、option 快照、回调与线程安全类型约束。

`pkg/objstore/mockobjstore/Cargo.toml` 声明了 `anyhow`、`objectio` 和 `astersql-objstore-storeapi` 等依赖；本文件实际直接使用的是前三者对应的 API。manifest 中的 `http`、`reqwest`、`uuid` 未在本文件源码出现，不能据此归因于本实现的运行逻辑。

## 错误处理与边界

- 普通返回型方法在没有期望时由 `take` panic；`URI`、`WalkDir` 和 `Close` 各自执行等价的显式检查。panic 文本包含方法名，便于定位漏配。
- 登记的 `Err(String)` 在消费时成为 `anyhow::Error`，只保留字符串消息，不附加错误类型或上下文链。
- `verify()` 只检查剩余期望，不校验调用参数，也不会确认没有发生已经消费完的额外调用；额外调用会在发生时因队列为空而 panic。
- 录制器不接收预期参数，因此它不能像 GoMock 的 matcher 那样在调用时拒绝路径、数据或 option 不匹配。需要参数约束时，测试必须检查 `calls()`。
- `WalkDir` 的回调错误优先于登记的最终错误：一旦回调返回错误，后续条目不再执行，已经弹出的最终结果也不会再返回；该期望仍视为已消费。
- 调用会在取结果之前写入 `calls`，所以即使随后返回错误或 panic，历史中也保留此次尝试。`WalkDir` 在回调失败时同样保留调用记录。
- `lock` 对毒化锁执行恢复，方便继续取得诊断证据；它不撤销导致毒化的部分状态变更，也不保证该状态在业务意义上完整。

## 并发与资源生命周期

`Arc<Mutex<State>>` 使 `MockStorage` 及录制器能够跨线程共享，且满足 `Storage: Send + Sync`。独立测试 `mock_storage_is_send_and_sync` 以编译期泛型约束验证这一点。所有队列修改和调用日志更新都受同一把互斥锁串行化，因此调用记录反映线程取得锁的顺序，而不是线程启动顺序。

普通方法持锁直到记录与结果弹出完成；其后锁守卫随函数返回释放。`WalkDir` 特意在执行用户回调前释放锁，避免回调重入同一 Mock 时自锁死锁。相对地，`calls()` 和 `verify()` 读取时也会短暂独占该锁。

Mock 本身没有后台任务、通道、事务或网络连接。`Arc` 最后一个持有者释放时，剩余队列和调用日志直接析构；没有自动 `verify`。reader/writer 生命周期归取得它们的调用方，Mock 不代替调用方执行 `close`。因此严格收尾依赖测试显式调用 `verify()`，并按被测协议关闭流对象。

## 与 Go 版本的对应关系

同目录 `pkg/objstore/mockobjstore/objstore_mock.go` 是由 MockGen 针对 Go `storeapi.Storage` 生成的 Mock。Rust 保留了相同的核心入口和方法名：`MockStorage`、`MockStorageMockRecorder`、`NewMockStorage`、`EXPECT`、`ISGOMOCK`，以及整组 `Storage` 方法；`Duration`、路径、字节和 option 的可观察内容也被保留。

两者实现机制不同。Go 版本把控制权交给 `gomock.Controller`，录制器方法接收预期参数并返回 `*gomock.Call`，因而可使用 matcher、次数、调用顺序和动态行为。Rust 版本没有 controller：录制器仅登记返回值，按方法 FIFO 消费，并用 `calls()` 做事后参数与跨方法顺序检查；构造函数也不接收 controller。Rust 的 `WalkDir` 还会主动遍历预置条目并调用回调，而 Go 生成代码只是把回调本身交给 controller，具体回调行为需由 Go 测试期望配置。

因此本文件是对 GoMock 测试用途的手写语义迁移，不是生成代码的逐行翻译，也不具备 GoMock 的全部匹配能力。独立 Rust 测试验证了当前迁移目标：各方法消费单次期望、调用顺序记录、option 区分、错误转发、遍历回调、`Duration` 保存以及 `Send + Sync`。

## 扩展指南

- `Storage` 新增必需方法时，应同步增加 `Call` 变体、`State` 队列或计数、录制器方法、`verify()` 检查和 `impl Storage` 方法；随后在独立文件 `pkg/objstore/mockobjstore/migration_aster_unit_test.rs` 增加成功、错误、未配置和调用快照覆盖，不要把测试写回生产源文件。
- option 结构新增字段时，必须同步更新 `walk_option`、`writer_option` 或 `reader_option` 的元组形状及 `Call` 变体，否则 `calls()` 会静默丢失新参数。修改公开 `Call` 形状可能破坏下游测试的模式匹配，需评估兼容性。
- 若需要 GoMock 式参数即时校验，应设计显式 matcher/期望对象，而不是仅在现有返回队列上附加零散条件；同时定义并发调用时的匹配与顺序语义。
- 若增加回调型方法，应仿照 `WalkDir` 在锁内取出期望、在锁外运行用户代码，避免重入死锁；还需明确“回调错误”和“预置最终错误”的优先级。
- 若需要自动检查残留期望，不能简单在每个 clone 的 `Drop` 中调用 `verify()`；应避免中间 clone 析构误报，并处理测试线程正在 panic 时的双重 panic 风险。
- 大 payload 场景若出现测试内存压力，可考虑可配置的摘要记录方式，但这会改变 `Call::WriteFile` 的精确断言能力，属于兼容性与诊断性权衡。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/objstore/mockobjstore` 列出 `lib.rs`、本文件、Go 对照和迁移测试；`node --file pkg/objstore/mockobjstore/objstore_mock.rs` 读取完整 360 行并报告 58 个符号、41 个文件级使用关系；`query MockStorage --kind struct` 与 `query NewMockStorage --kind function` 消除了仓库内同名类型歧义。
- 源码事实：`pkg/objstore/mockobjstore/objstore_mock.rs` 的 `Call`、`State`、`MockStorage`、`MockStorageMockRecorder`、辅助函数及 `impl Storage`；`pkg/objstore/storeapi/storage.rs` 的 options 与 `Storage: Send + Sync` 契约。
- crate 与门面：`pkg/objstore/mockobjstore/Cargo.toml`、`pkg/objstore/mockobjstore/lib.rs`、根 `Cargo.toml` 的 `facade_objstore_mockobjstore`、`pkg/lib.rs` 的 `objstore::mockobjstore` 重导出。
- Go 对照：`pkg/objstore/mockobjstore/objstore_mock.go` 的 MockGen 生成类型、controller 调用和录制器签名。
- 独立测试：`pkg/objstore/mockobjstore/migration_aster_unit_test.rs` 中 `forwards_expected_storage_calls_and_results_in_order`、`walk_and_presign_preserve_go_callback_and_duration_behavior`、`create_open_and_delete_methods_consume_one_expectation_each`、`create_and_open_calls_preserve_distinct_options`、`mock_storage_is_send_and_sync`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以固定标题结构命令和人工事实复核验证文档。
