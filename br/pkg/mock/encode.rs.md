# `br/pkg/mock/encode.rs`

## 文件定位

`br/pkg/mock/encode.rs` 属于 `astersql-br-pkg-mock` crate，是 Go 生成文件 `br/pkg/mock/encode.go` 的手写 Rust 移植，模拟 Lightning 编码边界中的 `Encoder`、`EncodingBuilder`、`Rows` 和 `Row` 四组接口。模块由 `br/pkg/mock/lib.rs` 通过 `#[path = "encode.rs"]` 装入，并用 `pub use encode::*` 平铺导出。

该文件服务隔离测试，不执行真实 SQL 行到 TiKV KV 的编码。真实 Rust 接口位于 `pkg/lightning/backend/encode/encode.rs`；本文件使用 `br/pkg/mock/stubs.rs` 中的本地 `Controller`、`Datum`、`EncodingConfig`、`RowHandle` 和 `RowsHandle`，且四个 mock 没有实现真实 `Encoder`、`EncodingBuilder`、`Rows`、`Row` trait。因此它当前是测试契约替身，而不是可直接注入真实 Lightning 编码流水线的 trait 实现。

`br/pkg/mock/Cargo.toml` 将 crate 声明为 library，入口为 `lib.rs`，`package.metadata.porting.go-package` 指向 `br/pkg/mock`；依赖表为空，说明该 mock 依靠 crate 内桩类型保持轻量，而不链接 Lightning、KV 或 gRPC 依赖。

## 核心职责

- 为每个被模拟接口提供一个 mock 对象和一个 recorder：mock 在调用时消费预先登记的期望，recorder 通过 `EXPECT()` 登记期望并返回可配置返回值的 `Call`。
- 通过 `Controller::Helper`、`Controller::Call` 和 `Controller::RecordCallWithMethodType` 复刻 Go MockGen 的基本调用形状，使测试可以用 `EXPECT().方法(...).Return*()` 描述行为。
- 把动态返回值恢复为本地强类型：`Encode` 和 `NewEncoder` 组合业务值与错误，`MakeEmptyRows`、`Clear`、`Size` 提取单个返回值，`Close`、`ClassifyAndAppend` 只验证调用发生。
- 保持 Go 公开名称和方法分组，便于移植测试对照；`ISGOMOCK` 是身份标记空方法。

职责边界很窄：参数当前不参与期望匹配，mock 不编码数据、不维护行批内容、不更新校验和，也不验证 `Context` 或 `EncodingConfig`。这些限制来自本文件 recorder 传入空参数列表以及 `stubs.rs` 的 `Controller` 忽略 `_args` 的实现。

## 主要符号

- `MockEncoder` / `MockEncoderMockRecorder`：模拟单行编码器。`NewMockEncoder(Controller)` 同时构造调用对象和共享同一控制器的 recorder；`Close()` 消费 `"Close"` 期望；`Encode(Vec<Datum>, i64, Vec<i32>, i64) -> Result<RowHandle>` 消费 `"Encode"` 期望。
- `MockEncodingBuilder` / `MockEncodingBuilderMockRecorder`：模拟编码器工厂。`MakeEmptyRows() -> RowsHandle` 返回空行集合的句柄；`NewEncoder(Context, EncodingConfig) -> Result<MockEncoder>` 返回预置 mock，或者在未提供/类型不符时用同一个控制器构造 `MockEncoder`。
- `MockRows` / `MockRowsMockRecorder`：模拟可清空的编码行集合。`Clear() -> RowsHandle` 从期望返回值中提取一个新句柄，对应 Go 常见的 `rows = rows.Clear()` 使用方式。
- `MockRow` / `MockRowMockRecorder`：模拟一条已编码行。`ClassifyAndAppend(&mut RowsHandle, &mut KVChecksum, &mut RowsHandle, &mut KVChecksum)` 记录分类追加调用；`Size() -> u64` 返回预置 KV 总大小。
- 四个 `NewMock*` 构造器都是公开函数；四个 mock 的 `EXPECT`、`ISGOMOCK` 及业务方法也公开。recorder 的 `ctrl` 字段是私有的，调用方只能通过 recorder 方法登记期望。
- 文件没有常量、枚举、trait、条件编译项或模块内测试；独立测试位于 `br/pkg/mock/parity_test.rs`。

## 执行流程

典型测试流程如下：

1. 调用方创建 `Controller::new()`，再用某个 `NewMock*` 构造器创建 mock。构造器克隆控制器给 recorder，因此登记与消费访问同一份期望队列。
2. 调用方通过 `mock.EXPECT().Method(...)` 进入 recorder。recorder 先调用 `Helper()`，再以固定方法名和描述字符串调用 `RecordCallWithMethodType`，得到 `Call`。
3. 测试在 `Call` 上使用 `Return`、`Return1`、`Return2` 或 `ReturnError` 写入动态返回值。
4. 被测路径调用 mock 业务方法。业务方法再次调用 `Helper()`，然后通过相同方法名调用 `Controller::Call`。控制器寻找并移除首个同名期望；它不按参数比较，也不强制不同方法之间的全局登记顺序。
5. 有返回值的方法进行动态类型恢复。`Encode` 先取第一槽 `RowHandle`，再由 `take_error` 处理剩余错误槽；`NewEncoder` 同理，但值槽缺失或类型不符时回落到 `NewMockEncoder(self.ctrl.clone())`。`MakeEmptyRows`、`Clear` 和 `Size` 使用 `take_one`。
6. 测试可用 `Controller::remaining()` 检查期望是否全部被消费；本文件本身不在析构时自动做该断言。

`br/pkg/mock/parity_test.rs::go_rust_public_contract_matches` 展示了实际可达链：为 `Encode` 配置 `Return2(RowHandle { id: 7, size: 11 }, None)` 后断言句柄字段，为 `Close` 配置空返回；随后验证 `MakeEmptyRows`、`Clear`、`Size` 的预置返回，并调用 `ClassifyAndAppend` 消费期望。

## 数据与状态

每个 mock 只保存两个字段：公开的 `ctrl: Controller` 和公开的 `recorder`。每个 recorder 再持有一份克隆的 `Controller`。`Controller` 内部使用 `Arc<Mutex<ControllerInner>>` 保存 `VecDeque<ExpectedCall>`，所以控制器克隆共享期望状态，而不是复制队列。

调用参数使用 `Box<dyn Any + Send>` 暂时装箱。`MockEncoder::Encode` 把拥有所有权的 Datum 列表、行号、列置换和偏移装箱；`MockEncodingBuilder::NewEncoder` 装箱上下文和配置；`MockRow::ClassifyAndAppend` 则装箱四个可变参数的克隆快照。当前 `Controller::Call` 将参数命名为 `_args` 并完全忽略，recorder 方法也向 `RecordCallWithMethodType` 传空向量，因此这些值既不匹配也不持久化。

`RowHandle`、`RowsHandle`、`Datum`、`EncodingConfig`、`KVChecksum` 都是 `stubs.rs` 中的本地替身。尤其 `ClassifyAndAppend` 传入的是克隆值，控制器没有回调机制，故该方法不会修改调用方的两组行句柄或校验和；它只消费一次期望。

## 依赖与调用关系

上游装配关系是 `br/pkg/mock/lib.rs -> encode.rs`，随后 crate 根平铺导出全部公开符号。仓库检索显示，目标构造器在 Rust 测试中的直接使用点集中于 `br/pkg/mock/parity_test.rs`；其他依赖 `astersql-br-pkg-mock` 的 crate（如 `br/pkg/utiltest`、`br/pkg/mock/mocklocal`）当前使用集群、错误或控制器等其他导出，没有直接调用本文件构造器。

下游依赖全部来自 `crate::stubs`：

- 调度层：`Controller`、`Call`、`take_one`、`take_error`；
- 入参替身：`Context`、`Datum`、`EncodingConfig`；
- 返回/状态替身：`RowHandle`、`RowsHandle`、`KVChecksum`；
- 错误契约：本地 `Result<T>` 与 `Error`。

概念上的真实接口位于 `pkg/lightning/backend/encode/encode.rs`：真实 `Encoder::Encode` 返回 `Box<dyn Row>`，真实 `Rows::Clear` 返回 `Box<dyn Rows>`，真实 `Row::ClassifyAndAppend` 会追加数据/索引并更新校验和。本文件用不透明句柄和控制器期望替代这些对象及副作用，不能由“方法同名”推断为真实 trait 接线。

RustCodeGraph 的文件节点确认 `encode.rs` 含 35 个符号；符号查询同时定位到 Rust 和 Go 的 `MockEncoder`、`MockRows`、`MockRow`。调用边探索对本文件多数方法只报告文件内同名 recorder/base 配对，未发现生产主链调用；精确 `callers/callees` 命令在本次环境中超时，因此上游直接使用点另由仓库 `rg` 结果和独立测试源码核实。

## 错误处理与边界

- 如果业务方法没有对应的已登记期望，`Controller::Call` 会以 `Unexpected call to {method}` panic；本文件不将该错误转换为 `Result`。
- `Encode` 的值槽缺失或不能 downcast 为 `RowHandle` 时回落 `RowHandle::default()`；其剩余返回槽由 `take_error` 解释为 `Option<Error>` 或 `Error`。未知错误槽类型被宽松视为成功。
- `NewEncoder` 的值槽缺失或类型不符时创建共享当前控制器的新 `MockEncoder`；随后才处理剩余错误槽。若错误槽是 `Some(Error)`，仍返回错误而不暴露回落对象。
- `MakeEmptyRows`、`Clear`、`Size` 使用 `take_one`，已登记期望若未配置返回值或类型错误，会分别得到类型的 `Default` 值。该宽松行为与 Go 类型断言失败得到零值的效果相近，但会隐藏测试返回类型配置错误。
- `Close` 和 `ClassifyAndAppend` 丢弃控制器返回向量；为它们配置返回值没有业务效果。
- recorder 的参数虽然接受 `&dyn Any`，却不把实参写入期望；控制器也忽略实际参数。因此当前实现只能验证方法名与调用次数，不能验证 Datum、row ID、列映射、偏移、上下文、配置、句柄或校验和内容。
- `ISGOMOCK` 没有返回 Go 版本的空结构值，在 Rust 中只是可调用的标记方法。

## 并发与资源生命周期

共享控制器的期望队列和每条期望的返回向量都由 `Mutex` 保护，`Controller` 通过 `Arc` 克隆，可安全地在多个持有者之间共享队列状态。一次 `Call` 会原子地从队列中移除匹配期望，再取走其返回向量，所以同一条期望只能被消费一次。

本文件不创建线程、异步任务、通道、文件、网络连接或事务。`MockEncoder::Close` 不释放真实资源，仅消费 `"Close"` 期望；它也不设置 closed 状态，所以 Close 后能否再次 Encode 完全取决于是否另行登记了 `"Encode"` 期望。mock 和 recorder 的生命周期由普通 Rust 所有权管理，没有自定义 `Drop`，销毁时不会自动检查遗漏期望。

并发测试应注意：同一控制器上的同名期望按队列中首个同名项被消费，但线程调度会决定哪个调用取得哪个返回值；参数又不参与匹配，因此需要确定性返回顺序时应避免并发共享同名期望，或先增强控制器契约。

## 与 Go 版本的对应关系

`br/pkg/mock/encode.go` 是 MockGen 从 `pkg/lightning/backend/encode` 的四个接口生成的对照文件。Rust 保留了四组 mock/recorder、四个构造器、`EXPECT`、`ISGOMOCK` 以及全部业务方法名称，基础的“登记期望—调用—取返回值”结构与 Go 一致。

主要差异如下：

- Go mock 直接实现接口所需的方法并返回 `encode.Row`、`encode.Rows`、`encode.Encoder` 接口值；Rust mock 返回本地 `RowHandle`、`RowsHandle` 或具体 `MockEncoder`，且没有实现真实 Rust traits。
- Go recorder 将期望参数和反射方法类型交给 gomock，可使用 matcher 验证参数；Rust recorder 丢弃传入参数，`Controller` 仅按方法名匹配。
- Go `ClassifyAndAppend` 把指针交给 gomock，Do/DoAndReturn 等机制可以观察或修改它们；Rust 版本只克隆快照且本地 `Call` 不支持回调，所以没有出参副作用。
- Go gomock 提供更丰富的次数、顺序、matcher 和测试失败报告；`stubs.rs` 明确只提供轻量的 `Return*` 与方法名消费机制。
- Rust 的 `Encode` 和 `NewEncoder` 对缺失/错误类型的返回值采用默认值或回落 mock，并对未知错误类型宽松成功；这比完整 gomock 更容错，也更可能掩盖错误配置。

这些差异说明当前目标是让移植测试保持公开形状和基本返回传播，而不是完整复刻 gomock 或真实编码器行为。

## 扩展指南

- 新增或变更被模拟接口方法时，应同步修改 Go 对照认知、对应 `Mock*` 业务方法和 `Mock*MockRecorder` 方法，并确保两侧使用完全一致的方法名字符串；还应在 `br/pkg/mock/parity_test.rs` 增加独立覆盖，不能把测试内嵌进 `encode.rs`。
- 若要支持参数断言，需要同时扩展 recorder 传参、`Controller::RecordCallWithMethodType` 的保存结构和 `Controller::Call` 的匹配逻辑；只改本文件 recorder 不足以形成真实匹配。兼容风险是现有测试使用 `&()` 作为任意参数占位，升级时需设计明确 matcher 语义。
- 若要让这些 mock 注入真实 Rust Lightning trait，应优先评估在独立适配层实现 trait，而不是把句柄替身直接冒充 `Box<dyn Row/Rows/Encoder>`。需要处理 `&mut self`、trait object、真实 `EncodeError` 与本地 `Error` 的转换，以及 `ClassifyAndAppend` 的可变副作用。
- 若增强 `ClassifyAndAppend`，应测试数据行与索引行句柄、两组 `KVChecksum` 的实际回写；当前 parity 测试只验证方法可达，不能证明分类或校验逻辑。
- 若改变默认返回或错误 downcast 策略，应补充 `Encode`/`NewEncoder` 的成功、显式错误、缺失返回和错误类型四类边界测试。严格化可能暴露已有测试的宽松配置，属于兼容性风险。
- 性能通常不是该测试桩的主约束；但 `Encode` 对向量取所有权、`ClassifyAndAppend` 克隆四个对象、动态 `Any` 装箱和互斥锁都会产生开销，不应据此评估真实编码路径性能。

## 验证依据

- 目标实现：`br/pkg/mock/encode.rs`，RustCodeGraph `node --file ... --offset 1 --limit 500` 显示全文件 346 行和 35 个符号；`query MockEncoder/MockRows/MockRow/NewMockEncodingBuilder` 核对了 Rust/Go 对应符号位置。
- crate 边界：`br/pkg/mock/Cargo.toml`、`br/pkg/mock/lib.rs`，确认 library 入口、空依赖表、模块装配和平铺导出。
- 控制器语义：`br/pkg/mock/stubs.rs` 的 `Controller::{new,Call,RecordCallWithMethodType,remaining}`、`Call::{Return,Return1,Return2,ReturnError}`、`take_error`、`take_one`，确认共享队列、方法名匹配、动态返回值和默认/错误行为。
- Go 对照：`br/pkg/mock/encode.go`，确认 MockGen 来源、四组接口方法、Go 返回类型与 recorder 参数传递。
- 真实接口：`pkg/lightning/backend/encode/encode.rs` 与 `pkg/lightning/backend/encode/encode.go`，确认编码器、行集合、分类追加和大小语义；本文件没有实现真实 Rust traits。
- 独立测试：`br/pkg/mock/parity_test.rs::go_rust_public_contract_matches`，覆盖 `Encode` 成功返回、`Close`、`MakeEmptyRows`、`Clear`、`Size` 和 `ClassifyAndAppend` 的调用可达性。仓库 `rg` 未发现其他 Rust 测试直接调用四个 `NewMock*` 构造器。
- 调用图限制：RustCodeGraph `explore "br/pkg/mock/encode.rs symbols callers callees"` 返回文件上下文和局部 blast radius；后续精确 `callers/callees` 查询超时，故未把未获得的生产调用边写成已验证事实，并用模块入口、crate 依赖检索及测试引用交叉验证当前接线。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证目标文档存在且恰含 11 个固定二级章节。
