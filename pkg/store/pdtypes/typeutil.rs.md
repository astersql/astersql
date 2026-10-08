# [`pkg/store/pdtypes/typeutil.rs`](typeutil.rs)

## 文件定位

本文件属于 `astersql-store-pdtypes` crate，提供 PD 配置模型使用的字符串列表兼容类型。crate 入口 `pkg/store/pdtypes/lib.rs` 通过 `pub mod typeutil` 公开本模块；工作区根 `Cargo.toml` 以 `facade_store_pdtypes` 注册该 crate，`pkg/lib.rs` 又在 `store::pdtypes` 下整体再导出。因此 `StringSlice` 是公开的数据边界类型，但本文件本身不访问 PD、不读取配置，也不执行任何存储或调度操作。

当前生产 Rust 直接使用点是 `pkg/store/pdtypes/config.rs::ReplicationConfig::LocationLabels`。RustCodeGraph 的文件关系还识别出独立测试 `pkg/store/pdtypes/migration_aster_unit_test.rs`；精确仓库搜索没有发现其他 Rust 生产调用者。它在完整应用中的作用是固定 PD 复制配置中 `location-labels` 的 wire 表示，而不是一条独立运行的业务主链。

## 核心职责

本文件只承担一种协议适配：把拥有的 `Vec<String>` 包装为 `StringSlice`，并将其编码成“所有元素以逗号连接后的单个字符串”，而不是通常的 JSON 字符串数组。它提供两套一致入口：

1. Go 风格公开方法 `MarshalJSON` / `UnmarshalJSON`，供显式 JSON 字节编解码使用。
2. `serde::Serialize` / `serde::Deserialize` 自定义实现，供包含 `StringSlice` 的结构体（目前是 `ReplicationConfig`）自动参与 Serde 编解码。

该类型不负责校验标签是否合法、不去重、不裁剪空白，也不对逗号做转义。逗号既是唯一分隔符，也是数据中无法区分的普通字符，因此这是一项必须由调用方遵守的输入约束。

## 主要符号

### `StringSlice`

`pub struct StringSlice(pub Vec<String>)` 是文件唯一类型。它是公开 tuple struct，调用方既可用 `StringSlice(vec![...])` 构造，也可直接读写 `.0`。派生的 `Clone`、`Debug`、`Default`、`Eq` 和 `PartialEq` 分别提供深克隆、调试输出、空向量默认值及值相等比较；`Serialize` 和 `Deserialize` 没有派生，而由本文件手写以改变 wire 形态。

### `StringSlice::MarshalJSON`

签名为 `pub fn MarshalJSON(&self) -> anyhow::Result<Vec<u8>>`。它先以 `Vec<String>::join(",")` 生成中间字符串，再交给 `serde_json::to_vec` 生成合法 JSON 字符串字面量，所以引号、反斜杠和控制字符由 JSON 库转义。失败时用 `anyhow::Context` 添加 `marshal StringSlice as JSON string` 上下文。

### `StringSlice::UnmarshalJSON`

签名为 `pub fn UnmarshalJSON(&mut self, text: &[u8]) -> anyhow::Result<()>`。它先把输入完整解析为 JSON `String`；只有解析成功后才按逗号拆分并替换 `.0`。空字符串单独映射为空向量，非空字符串使用 `split(',')`，因此会保留首尾或连续分隔符产生的空元素。解析失败时添加 `unquote StringSlice JSON` 上下文，并且不会修改原值。

### `Serialize::serialize`

自定义 trait 方法把 `.0.join(",")` 直接传给 `Serializer::serialize_str`。它与 `MarshalJSON` 使用相同的连接规则，但不绑定 JSON；最终格式和错误类型由调用方选择的 Serializer 决定。

### `Deserialize::deserialize`

自定义 trait 方法先要求 Deserializer 产生一个 `String`，再应用与 `UnmarshalJSON` 相同的空串/逗号拆分规则并构造新 `StringSlice`。非字符串输入由具体 Deserializer 拒绝。

本文件没有模块级常量、trait 声明、枚举、条件编译项或私有辅助函数。

## 执行流程

显式序列化流程为：调用方构造或持有 `StringSlice` → `MarshalJSON` 按顺序用逗号连接全部元素 → `serde_json::to_vec` 对整个连接结果进行 JSON 字符串转义 → 返回拥有的字节向量。示例 `['zone', 'rack']` 对应 JSON 字节 `"zone,rack"`。

显式反序列化流程为：`UnmarshalJSON` 接收借用字节 → `serde_json::from_slice::<String>` 校验 UTF-8、JSON 语法和顶层字符串类型 → 空字符串生成空向量，否则按每个逗号拆分并复制各段 → 成功后一次性替换 `self.0`。先解析、后赋值这一顺序保证错误原子性。

嵌套在 `ReplicationConfig` 中时没有调用 Go 风格方法。派生的 `ReplicationConfig::serialize` 访问 `LocationLabels` 后触发 `StringSlice::serialize`；反序列化则触发 `StringSlice::deserialize`。迁移测试验证 `LocationLabels: ["zone", "rack"]` 最终表现为 JSON 字段 `"location-labels": "zone,rack"`。

## 数据与状态

唯一持久状态是 `StringSlice.0` 拥有的 `Vec<String>`。类型不保存借用、缓存、解析器或全局状态；克隆会深拷贝向量和每个字符串，默认值是空向量。编码过程会至少分配连接后的中间字符串；`MarshalJSON` 还分配输出字节，解码则分配顶层字符串、结果向量和每个分段字符串。

wire 格式存在以下不变量和归一化行为：

- 元素顺序被保留，普通的不含逗号字符串可以往返。
- 空向量编码为 `""`；单元素空字符串也编码为 `""`，解码后统一变为空向量，两者不可区分。
- 元素内的逗号没有转义机制。例如单元素 `"a,b"` 解码后会成为两个元素。
- 非空输入中的连续、开头或结尾逗号会产生空元素，例如 `"a,,b"` 得到三个元素。
- 空格没有特殊意义，不会自动裁剪。

因此 `StringSlice` 更接近受约束的 PD 配置 wire 类型，不是任意字符串向量的无损通用序列化格式。

## 依赖与调用关系

直接外部依赖有三项：`serde` 提供 Serializer/Deserializer trait；`serde_json` 完成显式 JSON 字节编解码；`anyhow` 为公开方法统一结果类型并附加错误上下文。`pkg/store/pdtypes/Cargo.toml` 明确声明了这三项，其中 `serde` 启用 `derive` 是同 crate 其他模型派生 Serde 所需，本文件本身只使用 trait。

模块和公开路径是：

`pkg/store/pdtypes/lib.rs` → `typeutil` → `StringSlice`

工作区 facade 路径是：

根 `Cargo.toml::facade_store_pdtypes` → `pkg/lib.rs::store::pdtypes` → `astersql-store-pdtypes` 的公开项

已核实的生产数据流是：

`ReplicationConfig::LocationLabels`（`pkg/store/pdtypes/config.rs`）→ `StringSlice::{serialize, deserialize}` → 调用方选择的 Serde 格式

RustCodeGraph 的 `node StringSlice` 报告 `config.rs` 的导入、迁移测试的导入和两个测试实例化点；仓库精确搜索交叉确认生产使用仅为 `ReplicationConfig::LocationLabels`。`MarshalJSON` / `UnmarshalJSON` 的直接 Rust 调用目前只出现在 `migration_string_slice_matches_go_json_contract` 测试中。图对常见函数名的 callee 可能发生跨文件同名误配，因此下游库调用以本文件源码中的 `join`、`serde_json::to_vec`、`serde_json::from_slice`、`serialize_str` 和 `String::deserialize` 为准。

## 错误处理与边界

`MarshalJSON` 的理论错误来自 `serde_json::to_vec`，并被 `anyhow::Context` 包装；对普通内存字符串，主要现实风险是分配失败，而 Rust 分配失败通常不会作为此 `Result` 返回。自定义 `Serialize` 不转换错误，直接返回具体 Serializer 的 `S::Error`。

`UnmarshalJSON` 会拒绝无效 JSON、无效 UTF-8，以及数组、对象、数字、布尔值和 `null` 等非字符串顶层值。它先解析到局部 `String`，所以错误不会部分更新接收者；迁移测试用 `not-json` 验证返回错误后原有 `['preserved']` 不变。自定义 `Deserialize` 同样只接受字符串，但它构造新值，不涉及原地回滚，并直接使用 `D::Error`。

本文件不拒绝空标签、重复标签、含空白标签或含逗号标签。尤其是逗号和空字符串的歧义属于既有 Go wire 契约，而不是解析错误。若业务要求无损表达这些值，应在协议层新增兼容方案，不能仅在一端把表示改为 JSON 数组。

## 并发与资源生命周期

本文件没有锁、原子量、异步任务、通道、事务、文件、网络连接或其他外部资源。`StringSlice` 的生命周期完全由 Rust 所有权管理：构造或解码后由调用方拥有，克隆产生独立数据，离开作用域时释放向量及字符串。

所有编解码只读取当前值或在成功末尾替换当前值，不访问共享全局状态。类型自身不提供内部可变性或同步保证；若多个线程需要修改同一个实例，仍须由调用方使用常规同步机制。不同实例之间可独立编解码，本文件没有跨调用资源需要清理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/pdtypes/typeutil.go`。Go 的 `type StringSlice []string` 与 Rust 的 tuple struct 都封装字符串切片；Go `MarshalJSON` 使用 `strings.Join` 后 `strconv.Quote`，Rust 使用 `join` 后 `serde_json::to_vec`，两者目标都是合法的单个 JSON 字符串。Go `UnmarshalJSON` 先 `strconv.Unquote`，再把空串设为空切片、其他值用 `strings.Split`；Rust 保持相同的空串分支和逗号拆分语义。

两端都不会转义分隔用逗号，因此都具有相同的非无损边界。Rust 额外实现通用 Serde trait，使 `ReplicationConfig` 的派生编解码能自动采用该格式；Go 则由 `encoding/json` 自动发现命名为 `MarshalJSON` / `UnmarshalJSON` 的方法。Rust 保留同名公开方法，是为了显式调用与迁移对照，而不是 Serde 自动调用它们。

错误模型存在语言差异：Go `MarshalJSON` 固定返回 `nil` 错误，Rust 返回 `anyhow::Result` 并传播 JSON 库错误；Go 解码用 `errors.WithStack` 包装，Rust 用带固定消息的 `anyhow::Context`。Go 的显式方法直接调用 `strconv.Unquote`，Rust 的显式方法则要求严格 JSON 字符串，因此对脱离 `encoding/json` 直接传入的 Go 原始字符串字面量等扩展语法不能声称完全等价。两边都在成功解引号/解析后才赋值，因此非法输入不破坏旧值。当前目录没有同名 Go 测试，Go 语义证据来自生产实现；Rust 的兼容回归位于独立文件 `pkg/store/pdtypes/migration_aster_unit_test.rs`。

## 扩展指南

若要增加便捷 API，应优先保持 `StringSlice` 的 wire 契约不变，并把测试放在独立测试文件中，不能内嵌回 `typeutil.rs`。最直接的同步位置是 `pkg/store/pdtypes/migration_aster_unit_test.rs::migration_string_slice_matches_go_json_contract`；新增边界时应覆盖空向量、单空元素、连续分隔符、元素含逗号、JSON 转义字符、非字符串 JSON 和失败不修改原值。若行为需要与 Go 同步，必须同时核对 `pkg/store/pdtypes/typeutil.go`。

修改 `serialize`/`deserialize` 时还要检查 `pkg/store/pdtypes/config.rs::ReplicationConfig::LocationLabels` 及测试 `migration_configuration_and_placement_keep_pd_json_shape`，因为即使显式方法仍正确，Serde trait 的偏差也会改变 PD 配置 JSON。反之，修改 `MarshalJSON`/`UnmarshalJSON` 时要防止两套入口产生不一致结果。

若未来必须支持含逗号的任意字符串，兼容风险高：直接改成 JSON 数组会改变既有 PD/Go wire shape，加入转义又可能被旧端按字面拆分。应先确认上游 PD 协议、Go 行为和已有持久化/网络数据，再设计版本化或双读方案。性能上，现实现每次连接和拆分都会分配；除非实际出现超大标签列表，否则保持简单协议通常比引入缓存更安全。

## 验证依据

本说明依据以下直接证据编写：

- `pkg/store/pdtypes/typeutil.rs`：`StringSlice`、两个公开 JSON 方法和两个 Serde trait 实现的完整源码。
- `pkg/store/pdtypes/config.rs`：唯一已找到的生产 Rust 字段 `ReplicationConfig::LocationLabels` 及其派生 Serde 调用场景。
- `pkg/store/pdtypes/lib.rs`：`typeutil` 模块公开方式与独立迁移测试的装配。
- `pkg/store/pdtypes/Cargo.toml`：`astersql-store-pdtypes` crate 边界、`anyhow`/`serde`/`serde_json` 依赖和 Go package 迁移元数据。
- 根 `Cargo.toml` 与 `pkg/lib.rs`：`facade_store_pdtypes` 注册和 `store::pdtypes` 再导出路径。
- `pkg/store/pdtypes/typeutil.go`：Go 类型及 `MarshalJSON`/`UnmarshalJSON` 的直接对照实现。
- `pkg/store/pdtypes/migration_aster_unit_test.rs`：逗号连接输出、Serde 输出、正常/空串解码、错误不修改旧值，以及嵌入 `ReplicationConfig` 后的 JSON shape。
- RustCodeGraph `status`、`files --filter pkg/store/pdtypes`、目标文件节点、`StringSlice`/`MarshalJSON`/`serialize`/`deserialize` 节点与调用关系查询：索引覆盖目标 Rust、Go 和测试文件，并确认主要符号及已知使用点。对同名符号产生歧义或错误跨文件边的结果未作为结论，另用精确路径搜索交叉核验。

人工复核结论：该文件存在是为了让 PD 字符串列表遵循 Go 侧的单字符串 JSON 契约；运行时行为由显式 JSON 方法或 Serde trait 被动触发；安全扩展的关键是维持两套入口一致、保留失败原子性，并同步独立迁移测试。按任务约束未运行 Cargo。
