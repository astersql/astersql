# `br/pkg/conn/util/stubs.rs`

## 文件定位

[`stubs.rs`](stubs.rs) 位于 `astersql-br-pkg-conn-util` crate 内，是 `kvproto::metapb` 的本地最小兼容层。crate 入口 [`lib.rs`](lib.rs) 通过 `#[path = "stubs.rs"] pub mod stubs` 装入该文件，再以 `pub use stubs::kvproto` 将其公开成与真实 `kvproto` 相近的模块路径；因此同 crate 的 [`util.rs`](util.rs) 可以统一写成 `crate::kvproto::metapb::{self, Store}`。

这不是网络客户端、PD 客户端或 protobuf 编解码器。其存在原因由文件头注释和 [`Cargo.toml`](Cargo.toml) 共同限定：当前 Darwin ARM64 精简构建不引入 `kvproto/grpcio`，但连接辅助逻辑仍需要 `metapb::Store`、`StoreLabel` 和 `StoreState` 的数据形状与 getter/setter 接口。虽然文件名是 `stubs`，它会参与该 crate 的正常编译，不能仅视为测试代码。

## 核心职责

该文件只承担三项职责：

1. 用 `kvproto::metapb` 两层公开模块复现调用路径，降低上层从真实 kvproto 类型切换到本地类型时的接线差异。
2. 提供 `StoreState`、`StoreLabel`、`Store` 三个纯内存数据类型，并实现当前连接工具需要的默认值、克隆、比较和字段访问能力。
3. 为 [`util.rs`](util.rs) 的 TiFlash 标签识别、Store 状态筛选、错误信息组装和 TiKV status 地址处理提供数据载体，也让 [`parity_test.rs`](parity_test.rs) 能构造覆盖 Go 契约的样本。

它明确不提供 protobuf wire format、未知枚举值保留、反射、字段存在性、gRPC、PD 拉取或任何 I/O。把它替换为真实 kvproto 时，不能仅以类型名相同判断完全兼容。

## 主要符号

- `pub mod kvproto` / `pub mod metapb`：构造兼容命名空间。`lib.rs` 重导出前者，实际类型全部定义在 `metapb` 内。
- `pub enum StoreState`：以 `#[repr]` 未显式声明的 Rust 枚举表达 `Up = 0`、`Offline = 1`、`Tombstone = 2`；`#[default]` 指定 `Up`。派生 `Clone + Copy + Debug + Default + PartialEq + Eq`，便于按值读取和比较。
- `pub struct StoreLabel`：私有字段 `key: String`、`value: String`。`new` 等价于 `default`；`get_key`/`get_value` 返回借用字符串，`set_key`/`set_value` 接管传入 `String`。
- `pub struct Store`：保存 `id`、服务地址 `address`、状态 HTTP 地址 `status_address`、raft peer 地址 `peer_address`、`state`、`last_heartbeat` 和 `labels`。所有字段私有，通过成对 getter/setter 访问。
- `Store::get_labels` 返回只读切片，避免只读调用者改变容量或元素；`Store::mut_labels` 暴露可变 `Vec`，用于原地增删；`Store::set_labels` 整体替换并取得向量所有权。

文件没有 trait、异步函数、条件编译项、全局常量或内部错误类型。全部构造和访问方法都是公开 API。

## 执行流程

该文件本身没有主动执行入口，典型流程由消费者驱动：

1. 调用者通过 `Store::default()` 或 `Store::new()` 得到零值对象：数值为 `0`，字符串和标签为空，状态为 `StoreState::Up`。
2. PD 适配层或测试构造器调用 `set_id`、地址 setter、`set_state` 和标签相关方法填充快照。
3. [`util.rs`](util.rs) 的 `GetAllTiKVStores` 遍历 `Store`，`is_store_tiflash` 经 `get_labels`、`StoreLabel::get_key/get_value` 将标签适配给 engine 工具，随后按 `StoreBehavior` 过滤或报错。
4. `GetConfigFromTiKVStores` 通过 `get_state` 跳过非 `Up` 节点；`HandleTiKVAddress` 通过 `get_id`、`get_address`、`get_status_address` 校验并规范化 status URL。
5. `Store` 以值形式进入返回向量；类型的 `Clone` 也允许 `StoreMeta` 测试替身返回快照副本。

`peer_address`、`last_heartbeat`、`mut_labels` 目前没有被该 crate 的生产逻辑直接消费，但保留了 metapb Store 常见访问面；它们不应被描述为已经参与连接决策。

## 数据与状态

`StoreState` 的默认值是 `Up`，这会使未显式设置状态的新 `Store` 被 `GetConfigFromTiKVStores` 当作在线节点。构造真实 PD 数据时必须主动写入实际状态，不能依赖默认值表达“未知”。当前枚举只有三个变体，也没有从原始整数安全解析未知值的 API。

`StoreLabel` 与 `Store` 都是拥有所有数据的值类型：字符串和标签向量不会借用外部缓冲区。getter 返回的引用生命周期绑定到对象自身；setter 会移动传入值。`Clone` 执行深拷贝，包括所有字符串和整个 `Vec<StoreLabel>`。

重要不变量来自消费者而非类型本身：类型不校验 ID 是否非零、地址是否合法、标签键是否唯一、heartbeat 的单位或范围，也不限制 `StoreState` 与地址/标签组合。`Store` 只是容器，语义校验位于 [`util.rs`](util.rs) 或更上层。

## 依赖与调用关系

下游依赖仅为 Rust 标准库中的 `String`、`Vec`、切片和派生 trait；[`Cargo.toml`](Cargo.toml) 没有为本文件增加第三方依赖，并明确记录“不使用 kvproto/grpcio”的精简边界。

直接装配和调用边如下：

- [`lib.rs`](lib.rs)：声明 `stubs` 并将 `stubs::kvproto` 重导出为 crate 公共模块。
- [`util.rs`](util.rs)：导入 `metapb::{self, Store}`；`is_store_tiflash` 读取标签，`GetAllTiKVStores` 读取 ID/地址，`GetConfigFromTiKVStores` 比较 `StoreState::Up`，`HandleTiKVAddress` 读取 ID 和两个地址。
- [`parity_test.rs`](parity_test.rs)：导入 `Store`、`StoreLabel`、`StoreState`，通过 setter 构造普通 TiKV、TiFlash 和带地址的 Store，验证过滤、错误、URL 和配置请求契约。
- [`util_test.rs`](util_test.rs)：不直接构造这些类型，主要验证同一模块中的时间戳组合；因此不是本桩类型的直接行为覆盖。

RustCodeGraph 对精确文件的 `node` 结果列出 25 个符号；`explore` 结果确认同目录调用集中在上述 `util.rs` 与 `parity_test.rs`。由于 getter/setter 名称在全仓库大量重复，宽泛的全局 callers 结果会混入同名符号，不能据此宣称跨 crate 调用。

## 错误处理与边界

本文件所有方法均为无失败返回：没有 `Result`、panic 分支、日志或错误包装。非法或不完整数据会被原样保存，错误只会在消费者执行语义检查时出现，例如 `HandleTiKVAddress` 对空 `status_address` 返回错误，或 `GetAllTiKVStores` 在 `ErrorOnTiFlash` 策略下拒绝带 `engine=tiflash` 标签的 Store。

主要兼容边界是它不是生成的 protobuf 消息：

- 不能序列化或反序列化 metapb wire bytes，也没有 unknown fields。
- `StoreState` 无未知整数兜底；未来 kvproto 新增状态时必须显式同步，否则无法表达该状态。
- `new`/getter/setter 名称相似不代表完整复刻真实生成 API；目前只承诺仓库已有调用所需的子集。
- 默认 `Up` 对测试构造方便，但可能掩盖漏设状态；新增生产适配应明确赋值。

## 并发与资源生命周期

这些类型不含 `Arc`、锁、原子变量、通道、任务句柄、文件描述符或网络资源，也没有 `Drop` 实现。所有权遵循普通 Rust 值语义：移动 `Store` 即转移其字符串和标签，克隆则生成独立副本，引用 getter 的有效期不超过被借用对象。

类型是否可跨线程由其字段和自动 trait 推导；文件没有建立共享可变状态或同步协议。若调用者需要并发更新 Store 快照，应在本文件之外选择锁、消息传递或不可变快照替换，不能依赖 `mut_labels` 提供并发安全。资源释放完全由 `String`/`Vec` 的 RAII 完成。

## 与 Go 版本的对应关系

Go 同路径 [`util.go`](util.go) 直接依赖 `github.com/pingcap/kvproto/pkg/metapb`，并使用 `*metapb.Store`。Rust 的本地类型对应 Go 消费到的字段：`Id`、`Address`、`StatusAddress`、`State` 和 `Labels`；额外保留的 `PeerAddress`、`LastHeartbeat` 也属于 Store 元数据形状，但当前同目录 Rust 业务未读取。

可观察语义的对齐点包括：`StoreState::Up` 对应 `metapb.StoreState_Up`；getter 提供与 Go 生成消息相似的零值读取；标签 `engine=tiflash` 驱动同样的 TiFlash 判断；ID 和地址进入相同的错误与 URL 处理流程。Go 使用指针切片 `[]*metapb.Store` 并在过滤时原地压缩，Rust 使用拥有值的 `Vec<Store>` 并建立新向量，这是实现方式差异，不应改变筛选结果。

尚未对齐的能力也必须保留认知：Go 类型来自真实 protobuf 生成代码，具有消息编解码和更完整字段/枚举兼容能力；Rust 桩仅是当前 crate 所需的结构子集。[`parity_test.rs`](parity_test.rs) 的 `go_rust_public_contract_matches` 覆盖普通/TiFlash Store、地址和错误流程，但不证明 protobuf wire 兼容。

## 扩展指南

新增功能时先判断需求属于“本地模型字段访问”还是“真实 protobuf/PD 能力”。前者可在 `Store`/`StoreLabel` 上补字段与访问器，并同步：

1. `Default` 的零值语义，尤其避免把未知状态误当在线。
2. [`util.rs`](util.rs) 中的实际消费者或适配代码。
3. 独立测试文件 [`parity_test.rs`](parity_test.rs)，不要把测试嵌入 `stubs.rs`；至少覆盖正常值、零值/空值和 Go 可观察行为。
4. 若 Go `metapb.Store` 或同路径 [`util.go`](util.go) 的消费字段发生变化，记录 Rust 子集是否仍足够。

若需求涉及 wire format、未知字段、gRPC 或 kvproto 新枚举，应优先评估恢复带发布 tag 的上游依赖，而不是在本文件手写半套 protobuf。根据仓库规则，外部 Rust 依赖必须在独立上游仓库移植并以 tag 引用，不能 vendor 或用本地 `[patch]`。兼容风险集中在默认状态和 API 子集；性能风险主要来自 `Clone` 深拷贝标签/字符串，批量 Store 路径不应无必要克隆。

## 验证依据

- 源码与符号：[`stubs.rs`](stubs.rs)；RustCodeGraph `node --file br/pkg/conn/util/stubs.rs --offset 1 --limit 220`，确认模块、3 个类型及其全部公开方法。
- crate 边界：[`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)；前者确认 package、`lib.rs` 入口和 kvproto/grpcio 精简说明，后者确认模块装配与公开重导出。
- Rust 调用链：[`util.rs`](util.rs)；RustCodeGraph `node` 检查 `is_store_tiflash`、`GetAllTiKVStores`、`GetConfigFromTiKVStores`、`HandleTiKVAddress` 的字段读取路径。
- Go 对照：[`util.go`](util.go)；确认真实 `metapb.Store` 在 TiFlash 过滤、在线状态筛选和 status 地址处理中的使用方式。
- 独立测试：[`parity_test.rs`](parity_test.rs) 的 `store_with_engine`、`plain_store`、`store_with_addresses` 和 `go_rust_public_contract_matches`；[`util_test.rs`](util_test.rs) 已检查，但不直接覆盖桩类型。
- 引用复核：`rg` 在 `br/pkg/conn/util/**/*.rs` 中确认 `Store`/`StoreLabel`/`StoreState` 的直接使用；全仓 Cargo/Rust 搜索未发现通过包名直接引用该 crate 的其他 manifest/source 路径。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核没有把桩描述为 protobuf 或网络实现。
