# `br/pkg/mock/backend.rs`

## 文件定位

本文件位于 `astersql-br-pkg-mock` crate，crate 边界由 [`br/pkg/mock/Cargo.toml`](Cargo.toml) 定义，入口是 [`br/pkg/mock/lib.rs`](lib.rs)。`lib.rs` 通过 `pub mod backend` 加载本文件，再以 `pub use backend::*` 平铺导出其中的公开符号。该 crate 的 Cargo 清单没有外部依赖，并明确说明它用本地 stubs 代替 PD、TiKV、gRPC 与 Lightning 服务边界。

它是 Go 生成文件 [`br/pkg/mock/backend.go`](backend.go) 的手写 Rust 移植，模拟 Lightning `Backend`、`EngineWriter`、`TargetInfoGetter` 三组接口。它属于测试替身层，不打开真实引擎、不写 SST，也不访问目标集群。当前 Rust 文件没有条件编译项；真正的测试模块由 `lib.rs` 在 `#[cfg(test)]` 下独立加载，测试逻辑没有嵌入本源文件。

## 核心职责

本文件把三类外部协作协议转换成统一的 `Controller` 期望录制与回放：

1. `MockBackend` 模拟引擎的打开、关闭、清理、刷盘、导入，以及 writer 创建和导入策略查询。
2. `MockEngineWriter` 模拟行追加、关闭后刷盘状态与同步状态，并实现 `stubs::EngineWriter`，因此可作为 `Box<dyn EngineWriter>` 传给被测代码。
3. `MockTargetInfoGetter` 模拟导入前置检查以及远端库表元数据读取。
4. 每个 mock 都有对应 recorder；`EXPECT()` 返回 recorder，recorder 将方法名登记到同一个 `Controller`，实际调用再按方法名消费预设返回值。

这里验证的是调用协议和错误分支，不是 Lightning 后端的真实行为。尤其是 `OpenEngine`、`ImportEngine` 等方法返回成功，只表示测试预设了成功结果，不能证明数据已经导入 TiKV。

## 主要符号

- `MockBackend` / `MockBackendMockRecorder`：共享一个可克隆的 `Controller`。构造器 `NewMockBackend(Controller) -> MockBackend` 同时创建 recorder；`EXPECT` 暴露录制入口，`ISGOMOCK` 是无副作用的 GoMock 身份兼容方法。
- `MockBackend::{CleanupEngine, CloseEngine, FlushAllEngines, FlushEngine, ImportEngine, OpenEngine}`：将参数装箱后交给 `Controller::Call`，再由 `take_error` 把返回槽解释为 `Result<()>`。`Close` 只消费调用，不返回错误。
- `MockBackend::LocalWriter`：先从第一个返回槽向下转型为 `Box<dyn EngineWriter>`，缺槽或类型不符时回落到 `NilEngineWriter`；剩余槽由 `take_error` 解释。
- `MockBackend::{RetryImportDelay, ShouldPostProcess}`：分别用 `take_one::<Duration>` 和 `take_one::<bool>` 提取标量；缺槽或类型不符时返回类型默认值。
- `MockEngineWriter` / `MockEngineWriterMockRecorder`：构造与录制结构同上。其 `EngineWriter` trait 实现只是转发到同名固有方法；`AppendRows` 在转发前把 `&[String]` 复制为 `Vec<String>`。
- `MockEngineWriter::Close`：首槽向下转型为 `Box<dyn ChunkFlushStatus>`；缺失或类型错误时回落到 `SimpleChunkFlushStatus { flushed: false }`，随后传播错误槽。
- `MockTargetInfoGetter` / `MockTargetInfoGetterMockRecorder`：提供 `CheckRequirements`、`FetchRemoteDBModels` 与 `FetchRemoteTableModels`。后两者在值槽缺失或类型不符时分别回落为空 `Vec<DBInfo>`、空 `HashMap<String, TableInfo>`。
- 所有 recorder 方法返回 `Call`，便于继续调用 `Return`、`Return1`、`Return2` 或 `ReturnError`。当前 recorder 的 `&dyn Any` 参数仅保留 Go 生成 API 的形状，登记时传给控制器的是空参数表。

文件没有模块级常量、枚举、类型别名或条件编译项，也没有定义 Rust `Backend` / `TargetInfoGetter` trait；只有 `MockEngineWriter` 明确实现了本 crate 的 `EngineWriter` trait。

## 执行流程

典型流程由独立测试 [`br/pkg/mock/parity_test.rs`](parity_test.rs) 直接展示：

1. 测试创建 `Controller::new()`，再调用 `NewMockBackend`、`NewMockEngineWriter` 或 `NewMockTargetInfoGetter`；构造器 clone 控制器给 recorder，保证录制和回放共享同一份期望队列。
2. `mock.EXPECT().Method(...)` 调用对应 recorder。recorder 先执行无操作的 `Helper()`，然后以方法名和描述字符串调用 `RecordCallWithMethodType`。
3. 测试在返回的 `Call` 上配置返回槽。例如 `ReturnError(None)` 表示成功，`Return1(true)` 表示单值，`Return2(value, Some(error))` 表示“值加错误”。
4. 被测调用进入 mock 固有方法；方法再次调用 `Helper()`，把拥有所有权的参数放进 `Vec<Box<dyn Any + Send>>`，按方法名调用 `Controller::Call`。
5. `Controller::Call` 从尚未消费的期望中寻找第一个同名项并移除；它不要求登记顺序，也不比较参数。没有同名期望时会在调用点 panic。
6. 方法根据签名提取返回槽：纯错误方法调用 `take_error`，无错误标量调用 `take_one`，对象或集合返回先自行 downcast，再把剩余槽交给 `take_error`。
7. 测试可用 `Controller::remaining()` 断言所有期望都已消费。`parity_test.rs::go_rust_public_contract_matches` 对 `MockBackend` 的正常调用、writer 关闭状态及目标模型读取执行了这一类检查。

因此，同一方法登记多次时按该方法的登记先后消费；不同方法之间可乱序调用。该行为由 `parity_test.rs::gomock_expectations_match_out_of_registration_order` 对同一控制器的其他 mock 明确验证，也由 `Controller::Call` 的同名位置搜索实现保证。

## 数据与状态

mock 本身只持有 `ctrl` 和 `recorder`；真正的可变状态位于 `Controller` 内部的 `Arc<Mutex<ControllerInner>>`。`ControllerInner.expected` 是待消费期望的 `VecDeque`，每个 `ExpectedCall` 保存方法名以及受 `Mutex` 保护的动态返回槽。构造器 clone 的是控制器句柄，不是复制期望队列。

输入数据均来自 [`br/pkg/mock/stubs.rs`](stubs.rs)：`Context` 只有 `cancelled` 标志，`UUID` 是 16 字节替身且 `UUID::new()` 当前生成全零值，`EngineConfig`、`LocalWriterConfig`、`CheckCtx` 是零字段占位，`RowsHandle` 只保存 id，`DBInfo` / `TableInfo` 只保存名称。这些简化类型足以验证调用形状，却不代表生产类型的完整字段与约束。

动态返回值存为 `Box<dyn Any + Send>`。取值是一次性的：期望被消费时，返回向量通过 `std::mem::take` 移走；同一个 `Call` 不会被自动重复使用。集合与 trait object 的错误类型会触发安全默认值，而不是暴露 downcast 错误。

## 依赖与调用关系

下游仅依赖标准库 `HashMap`、`Duration` 与本 crate 的 `stubs`：`Controller` / `Call` 承担录制回放，`take_error` / `take_one` 承担返回槽解释，其他类型保持接口外形。`Cargo.toml` 的空 `[dependencies]` 证明该 mock crate 没有直接接入真实 Lightning、PD 或 TiKV crate。

上游装配点是 `br/pkg/mock/lib.rs` 的 `pub mod backend` 与 `pub use backend::*`。仓库文本引用显示，本文件三个构造器的直接 Rust 使用集中在 `br/pkg/mock/parity_test.rs`：`NewMockBackend` 覆盖 `OpenEngine`、`CleanupEngine`、策略查询和 `Close`；`NewMockEngineWriter` 覆盖 `IsSynced` 与 `Close`；`NewMockTargetInfoGetter` 覆盖前置检查以及库表模型返回。其他 crate 可通过 `astersql_br_pkg_mock` 的平铺导出取得这些符号，但当前检索没有找到额外直接使用者。

RustCodeGraph 将 `backend.rs` 报告为被 `br/pkg/task/common_test.rs` 使用，但对 `NewMockBackend`、`NewMockEngineWriter`、`NewMockTargetInfoGetter` 的精确 `callers` / `callees` 查询没有输出调用边；文本复核也未在该 Rust 测试中找到这些符号。因此这里不把该文件索引关系解释成已验证的具体调用链。Go 图则显示 `backend.go` 被 `br/pkg/task/common_test.go`、`restore.go`、`restore_test.go`、`stream_test.go` 使用，说明 Go 侧应用面明显更广，不能直接外推为 Rust 侧已接线。

## 错误处理与边界

- 未登记同名期望：`Controller::Call` 以 `Unexpected call to {method}` panic；这是测试失败机制，不返回 `Result`。
- `take_error`：空槽与 `Option<Error>::None` 为成功，`Some(Error)` 或裸 `Error` 为失败；未知类型被宽松地视为成功。
- `LocalWriter`、writer `Close`、两个模型读取方法：值槽缺失或 downcast 失败时使用安全默认对象/空集合，然后再读取错误槽。错误槽存在时仍返回错误，默认值不会掩盖显式的 `Error`。
- `RetryImportDelay` 与 `ShouldPostProcess`：缺失或类型错误分别变为 `Duration::default()` 和 `false`。若测试关心策略值，应总是显式 `Return1`，否则默认值可能让遗漏期望的返回配置不够醒目。
- recorder 不保存参数匹配器，控制器也忽略实际参数，所以当前实现只能证明“某方法被调用”，不能证明 UUID、配置、列名或模型筛选参数正确。
- `Context.cancelled` 只被装箱透传，mock 不主动检查取消；取消语义必须在更高层测试或更真实的实现中验证。
- 本文件没有真实 I/O、重试、引擎状态机或元数据校验。方法名为 `Flush`、`Import` 或 `CheckRequirements` 不意味着相应动作真的发生。

## 并发与资源生命周期

`Controller` 的期望队列与每条期望的返回槽均由 `Mutex` 保护，句柄通过 `Arc` 共享，并且动态参数/返回值要求 `Send`；`EngineWriter` 和 `ChunkFlushStatus` trait 也要求 `Send`。这些约束允许控制器和 trait object 在线程间移动，但不提供调用顺序、事务隔离或真实后端线程安全的证明。

期望的生命周期是“登记一次、成功匹配后移除一次”。`MockBackend::Close`、`MockEngineWriter::Close` 等方法不会替调用方释放真实磁盘、网络或引擎资源，因为这里不存在这些资源。`NilEngineWriter` 的 `AppendRows` 总是成功、`Close` 返回 `flushed=false`、`IsSynced` 返回 false；它只用于缺省回落，不能作为写入完成证据。

同一个控制器可被多个 mock 共享；由于匹配键只有方法名，跨 mock 使用相同方法名（例如多个 `Close`）可能相互消费期望。并发或组合测试应优先给独立 mock 使用独立 `Controller`，或至少用 `remaining()` 严格检查，避免期望表交叉污染。

## 与 Go 版本的对应关系

Go 来源文件声明它由 MockGen 从 Lightning `Backend,EngineWriter,TargetInfoGetter` 生成；Rust 版本保留了三组类型、构造器、`EXPECT` / `ISGOMOCK` 以及方法名称和大体签名。`ImportEngine` 的两个 `i64`、writer 的列名/Rows、目标库表的 list/map 形状均可逐项对应。

主要语义差异如下：

- Go 构造器接收 `*gomock.Controller` 并返回指针；Rust 接收可克隆的 `Controller` 并按值返回 mock。
- Go `EngineConfig`、`LocalWriterConfig`、`CheckCtx` 使用指针；Rust 使用本地占位类型的值。Go 模型集合保存模型指针，Rust 保存简化模型值。
- Go recorder 将实际参数匹配器交给 gomock，并用反射记录方法类型；Rust recorder 当前丢弃 recorder 参数，`Controller` 也仅按方法名匹配。
- Go 的错误和值通过类型断言取得，缺失值通常得到 nil/零值；Rust 为 trait object 提供 `NilEngineWriter` 或 `SimpleChunkFlushStatus`，为集合提供空集合，从而避免 `Option`/downcast panic，但这也改变了 nil 可观察性。
- Go mock 直接满足真实 Lightning 三个接口；Rust 只有 `MockEngineWriter` 实现本地 `EngineWriter` trait。`MockBackend` 与 `MockTargetInfoGetter` 当前是同名固有方法集合，没有对应本地 trait 实现。
- Go gomock 支持参数 matcher、调用次数、顺序组合和完成时校验；本地 `Controller` 明确不支持 `Times`、`After`、`DoAndReturn` 等高级能力。

因此，本文件是面向当前 Rust 测试的兼容替身，不是 Go MockGen 行为的完整等价实现。

## 扩展指南

新增或修改被模拟接口时，应保持以下同步顺序：

1. 在对应 mock 的固有 `impl` 中增加实际调用方法，严格保持参数装箱顺序和返回槽布局。
2. 在对应 recorder `impl` 中增加同名录制方法，并确保 `RecordCallWithMethodType` 的方法键与实际调用完全一致。
3. 若扩展 `EngineWriter`，同步修改 `stubs::EngineWriter`、`MockEngineWriter` 的 trait 转发以及 `NilEngineWriter`；不要把测试逻辑内嵌到 `backend.rs`。
4. 若返回新动态对象或集合，明确缺槽、类型错和显式错误的优先级，避免默认值吞掉错误；如需严格 Go nil 语义，应先调整接口表示并增加回归测试，而不是继续增加静默回落。
5. 在独立的 `br/pkg/mock/parity_test.rs` 增加正常、显式错误、未预设/错误类型边界测试，并与 `br/pkg/mock/backend.go` 的同名方法核对。若功能开始被业务 crate 使用，再在最近的调用方独立测试中验证集成路径。
6. 参数匹配、调用次数或跨 mock 同名隔离属于 `stubs::Controller` 的横切能力；需要这些能力时应在那里设计并测试，而不是在单个 mock 方法里临时实现。

兼容风险主要是 Go/Rust 返回值空值语义差异与参数不匹配无法被发现；正确性风险是返回槽次序或方法字符串写错；性能风险很低，但大对象参数会被拥有并装箱，`EngineWriter::AppendRows` 还会复制列名。扩展时应避免把真实网络、磁盘或重试逻辑放入本文件。

## 验证依据

- 源码：`br/pkg/mock/backend.rs`，核对 6 个 struct、3 个构造器、3 组固有 impl、3 组 recorder impl，以及唯一的 `EngineWriter for MockEngineWriter` trait 实现。
- crate 装配：`br/pkg/mock/Cargo.toml` 与 `br/pkg/mock/lib.rs`，核对 crate 名、空依赖、本地 stub 边界、模块加载、平铺导出和独立测试模块。
- 控制器与类型语义：`br/pkg/mock/stubs.rs` 的 `Controller::{Call,RecordCallWithMethodType,remaining}`、`Call::{Return,Return1,Return2,ReturnError}`、`take_error`、`take_one`、`EngineWriter`、`NilEngineWriter` 及简化数据类型。
- Go 对照：`br/pkg/mock/backend.go`，核对 MockGen 来源、三个接口的方法集合、签名、返回槽和 recorder 形状。
- 独立 Rust 测试：`br/pkg/mock/parity_test.rs::go_rust_public_contract_matches`，覆盖 backend 成功路径与标量、writer 状态对象、目标模型 list/map 和剩余期望；`gomock_expectations_match_out_of_registration_order` 佐证控制器的跨方法乱序匹配。仓库检索未发现专属于 `backend.rs` 的其他 Rust 测试文件。
- RustCodeGraph：`status` 显示索引包含本文件；`files --filter br/pkg/mock` 确认同目录 Rust/Go/测试集合；`node --file br/pkg/mock/backend.rs` 与 `node --file br/pkg/mock/backend.go` 读取完整实现；精确查询三个构造器后执行 `callers` / `callees` 未返回调用边，因此上游引用以 `lib.rs` 与文本检索结果为准。
- 结构验证应确认本文恰有任务规定的 11 个二级标题。该任务是纯文档分析，按计划不运行 Cargo。
