# `br/pkg/mock/mocklocal/stubs.rs`

## 文件定位

`stubs.rs` 属于 Cargo crate `astersql-br-pkg-mock-mocklocal`。该 crate 的入口 `br/pkg/mock/mocklocal/lib.rs` 以 `#[path = "stubs.rs"] pub mod stubs` 装入本文件，并通过 `pub use stubs::*` 平铺导出其公开项。`br/pkg/mock/mocklocal/Cargo.toml` 将此 crate 标记为 Go 包 `br/pkg/mock/mocklocal` 的 library 移植，且只依赖父级 `astersql-br-pkg-mock`；注释明确它为 Darwin/ARM64 精简了 `kv`、`domain`、`kvproto`、`grpcio` 和 Lightning 原生依赖。

本文件不是生产存储或 RPC 实现，而是 `br/pkg/mock/mocklocal/local.rs` 这份 Go MockGen 移植代码的本地类型边界。RustCodeGraph 将它识别为由 `local.rs` 使用的文件；其中结构体替代 Go mock 签名中的外部类型，`take_ts` 则替代 Go 对 gomock 多返回值的类型断言。

## 核心职责

本文件承担两类职责：

1. 定义 `EngineFileSize`、`Range`、`Codec` 三个纯数据替身，使 mock API 能保持 Go 侧接口的大致形状，而无需链接真实 Lightning backend、`import_sstpb` 或 TiKV client。
2. 用 `take_ts` 将 `Controller::Call` 返回的 `Vec<Box<dyn Any + Send>>` 解包为 `MockStoreHelper::GetTS` 所需的 `(physical, logical, error)` 三元组。

这些替身只支持测试录制、回放和相等性断言。它们不读取引擎目录、不验证键范围、不编码键，也不执行 TiKV RPC；真实能力不能从这些类型的名字推断出来。

## 主要符号

- `pub struct EngineFileSize`：对应 Go `backend.EngineFileSize` 的 mock 返回载体。字段 `UUID: [u8; 16]`、`DiskSize: i64`、`MemSize: i64`、`IsImporting: bool` 分别保存引擎标识、磁盘占用、内存占用和导入状态。它派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`，便于通过 gomock 风格返回值传递和断言。
- `pub struct Range`：对应 Go `import_sstpb.Range` 的轻量字节区间，只有公开的 `start: Vec<u8>` 与 `end: Vec<u8>`。它不保留 protobuf 行为，也不检查 `start <= end`。
- `pub struct Codec`：对应 Go `tikv.Codec` 接口位置的占位对象，以 `id: i64` 区分测试实例。它不是 trait，也没有编解码方法。
- `pub fn take_ts(mut rets: Vec<Box<dyn Any + Send>>) -> (i64, i64, Option<astersql_br_pkg_mock::Error>)`：按槽位消费 Controller 返回值。前两槽尝试下转为 `i64`，第三槽接受 `Option<Error>` 或裸 `Error`。

四个公开符号都由 `lib.rs` 再导出；实际 mock 实现通过 `crate::stubs::{Codec, EngineFileSize, Range, take_ts}` 引入。文件没有 trait、impl、常量、静态变量或条件编译项。

## 执行流程

结构体本身没有执行流程；关键路径发生在 `take_ts`：

1. `br/pkg/mock/mocklocal/local.rs` 的 `MockStoreHelper::GetTS` 调用 `self.ctrl.Call("GetTS", vec![Box::new(arg0)])`，取得动态返回值向量。
2. `take_ts` 检查第一槽。向量为空时令 `physical = 0`；否则移除首元素，只有成功下转为 `i64` 才保留该值，类型不符也回落为 `0`。
3. 对新的第一槽重复同一规则，得到 `logical`。由于每次都 `remove(0)`，槽位顺序严格对应 Go 的 physical、logical、error。
4. 第三槽缺失时返回 `None`。若动态类型是 `Option<astersql_br_pkg_mock::Error>`，直接取出该可选错误；若是裸 `Error`，包装成 `Some`；其他类型被忽略为 `None`。
5. 返回 `(physical, logical, err)`。多余的第四槽及以后随局部向量一起丢弃。

其余类型在 `local.rs` 中的流向为：`EngineFileSize` 是 `MockDiskUsage::EngineFileSizes` 的向量元素；`Range` 被 `MockTiKVModeSwitcher::{ToImportMode, ToNormalMode}` 克隆并逐个装箱为变参；`Codec` 由 `MockStoreHelper::GetTiKVCodec` 经父 mock crate 的 `take_one` 解包。

## 数据与状态

本文件没有全局状态、内部缓存或隐藏状态。三个结构体的全部状态都在公开字段中，默认值分别是全零 UUID/尺寸/标志、空起止键和 `id = 0`。

`take_ts` 获得返回向量所有权，并通过 `remove(0)` 逐槽消费；被取出的 `Box<dyn Any + Send>` 在下转或分支结束后释放。函数不修改 `Controller`，Controller 的期望队列已经在调用本函数前由 `MockStoreHelper::GetTS` 消费。本文件使用 `Send` 限定动态值，满足跨线程传递的类型边界，但没有在这里启动线程或提供共享同步。

## 依赖与调用关系

上游装配链为 `lib.rs -> stubs.rs`，随后 `lib.rs` 同时装入 `local.rs` 并平铺再导出两者。RustCodeGraph 的文件关系显示 `stubs.rs` 由 `local.rs` 使用；精确源码调用边是 `MockStoreHelper::GetTS -> take_ts`。

下游依赖很窄：

- 标准库 `std::any::Any` 提供运行时类型识别与 `downcast`。
- `astersql_br_pkg_mock::Error` 是 `take_ts` 第三槽的错误类型。
- `local.rs` 消费全部四个符号，并依赖父 crate 的 `Controller`、`Call`、`Context` 和 `stubs::take_one` 完成完整 mock 回放。

`Cargo.toml` 没有 feature 声明，也没有真实 backend、protobuf 或 TiKV client 依赖。因此这个 crate 的可移植性来自明确缩小的契约；若调用方需要真实外部类型或方法，不能在不评估依赖成本的情况下把本文件当作兼容实现。

## 错误处理与边界

`take_ts` 有意采用宽松解包：缺少 physical/logical 槽或槽位类型错误均产生 `0`；缺少错误槽、错误槽类型未知或显式 `None::<Error>` 均产生 `None`；裸 `Error` 与 `Some(Error)` 都产生 `Some`。函数内部仅在先用 `is::<T>()` 确认第三槽类型后调用 `downcast::<T>().unwrap()`，所以这两个 `unwrap` 的类型前提已被同一对象检查。

这种宽松策略与 Go MockGen 中 `ret[i].(T)` 的逗号-ok 类型断言类似，但存在重要边界：错误的 mock 返回类型可能被静默转成零值或无错误，而不是立即暴露配置错误。相对地，返回槽过少在这里不会像直接索引 Go 切片那样触发越界。多余槽位不报错。`Range` 不保证端点有序或互斥，`EngineFileSize` 不禁止负数尺寸，`Codec.id` 也没有唯一性约束。

`br/pkg/mock/mocklocal/parity_test.rs` 验证了 `GetTS` 的非零数值及 nil 错误、`Some(Error)` 和裸 `Error` 两种错误形态；它还通过 `#[should_panic]` 验证未登记的 mock 方法调用由 Controller 失败，但该 panic 属于父 mock 控制器而非本文件。

## 并发与资源生命周期

本文件没有锁、原子变量、任务、通道、文件句柄、网络连接或事务。三个替身均由拥有者按普通 Rust 值语义管理，并因派生 `Clone` 可复制其数据；`Vec<u8>` 克隆会复制键字节，`EngineFileSize` 与 `Codec` 的字段则为定长/标量数据。

`take_ts` 的输入元素要求 `Any + Send`，但函数只在当前调用栈同步消费它们。动态对象在下转后转移给局部值，未使用或多余对象在函数返回时析构。Controller 的共享、期望耗尽及并发语义由 `astersql-br-pkg-mock` 负责；`parity_test.rs` 通过 `remaining() == 0` 和显式 `drop` 检查回放耗尽及析构不挂起，本文件不额外提供清理协议。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/mock/mocklocal/local.go`。Go 文件由 MockGen 从 `pkg/ingestor/ingestctrl` 的 `DiskUsage`、`TiKVModeSwitcher`、`StoreHelper` 接口生成，并直接导入 `backend.EngineFileSize`、`import_sstpb.Range` 和 `tikv.Codec`；Rust 的 `stubs.rs` 正是为解除这些重依赖而新增的适配层，因此没有同名 Go `stubs.go`。

对应关系如下：

- Rust `EngineFileSize` 保存 Go mock 的 `[]backend.EngineFileSize` 所需字段，但不是 backend 包的真实类型。
- Rust `Range` 为 Go `*import_sstpb.Range` 的必要字段替身。Go 以指针变参传递；Rust `local.rs` 接收 `&[Range]` 后克隆每个值装箱。
- Rust `Codec` 只用 `id` 支撑返回与断言；Go 返回真正的 `tikv.Codec` 接口，二者能力不等价。
- Go `GetTS` 对 `ret[0]`、`ret[1]`、`ret[2]` 分别做 `int64`、`int64`、`error` 断言；Rust `take_ts` 保持相同槽位顺序，并额外接受 `Option<Error>` 与裸 `Error` 两种录制表示。

`parity_test.rs` 是相关的独立 Rust 测试文件，覆盖这些公开契约；源文件和测试保持分离。Go 侧当前目录没有独立测试文件，语义依据主要来自生成的 `local.go` 签名与调用形状。

## 扩展指南

若 Go MockGen 输出引用新的外部类型，优先判断测试究竟需要字段形状还是需要真实行为。仅需录制/回放时，可在本文件新增最小数据替身，并在 `local.rs` 的签名和独立 `parity_test.rs` 中同步覆盖；需要编码、protobuf、RPC 或磁盘行为时，应接入真实 canonical crate，而不是继续扩张无行为桩。

修改 `take_ts` 时必须保持 Go 返回槽顺序，并为缺槽、错类型、`None<Error>`、`Some(Error)`、裸 `Error` 及多余槽位明确决定策略。新增或收紧错误行为应在 `parity_test.rs` 中加入回归用例，且不要把测试嵌回 `stubs.rs`。由于 `local.rs` 当前录制器对部分参数只按方法名和次序匹配，扩充 `Range` 不能被误写成已获得内容匹配保证。

兼容风险主要是公开字段或动态返回类型变化使既有测试无法构造/下转；性能风险主要来自 `Range` 变参路径的字节向量克隆和 `take_ts` 的头部 `remove(0)`。当前返回槽固定为三个，后者成本可忽略；若将此工具泛化到长向量，应改用迭代器按所有权取值，并保持现有边界语义。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/mock/mocklocal` 找到 `lib.rs`、`local.rs`、`stubs.rs`、`parity_test.rs` 与 Go 对照 `local.go`。
- RustCodeGraph `node --file br/pkg/mock/mocklocal/stubs.rs`：核对 93 行完整源码、三个公开结构体与 `take_ts` 的逐槽解包逻辑；文件关系标明它由 `local.rs` 使用。
- RustCodeGraph `node`：核对 `lib.rs` 的模块装配/再导出、`local.rs` 中四个符号的消费位置，以及 `parity_test.rs` 的正常、边界、错误、变参与意外调用用例。
- RustCodeGraph `query take_ts --kind function`：唯一命中 `stubs.rs:62`；调用源码在 `local.rs` 的 `MockStoreHelper::GetTS` 中明确为 `take_ts(ret)`。宽泛 `explore` 结果存在同名噪声，因此调用结论以精确文件节点为准。
- `br/pkg/mock/mocklocal/Cargo.toml`：核对 crate 名、`lib.rs` 入口、Go 包元数据、唯一父 mock 依赖及精简重依赖的边界说明。
- `br/pkg/mock/mocklocal/local.go`：核对 MockGen 来源、三个 Go 外部类型及 `GetTS` 的 physical/logical/error 返回顺序。
- `rg` 测试引用：确认当前目录的独立 Rust 测试为 `br/pkg/mock/mocklocal/parity_test.rs`，且没有同目录 Go 测试；本任务未运行 Cargo，符合纯文档计划约束。
