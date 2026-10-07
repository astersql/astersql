# [`pkg/objstore/flags.rs`](flags.rs)

## 文件定位

该文件属于 `astersql-objstore` crate；crate 由 `pkg/objstore/Cargo.toml` 定义，并在 `pkg/objstore/lib.rs` 以 `pub mod flags` 暴露本模块。它把 S3、GCS 和 Azure Blob 的命令行选项统一注册到一个轻量 `FlagSet`，再将字符串值解析为本文件定义的 `BackendOptions`。

当前接线边界需要特别注意：RustCodeGraph 和 `rg` 只找到本文件包装函数、`pkg/objstore/flags_test.rs`、`pkg/objstore/azblob_1_aster_unit_test.rs` 以及 `pkg/objstore/gcs.rs` 对这些类型的直接引用，未找到 Rust 生产命令调用 `flags::DefineFlags`、`flags::HiddenFlagsForStream` 或本文件 `BackendOptions::parse_from_flags` 的证据。此外，`pkg/objstore/parse.rs` 另有一个同名但独立的 `BackendOptions`，因此不能把本文件的解析结果直接视为已经进入 URL/后端创建主链。

## 核心职责

1. `S3_FLAGS`、`GCS_FLAGS`、`AZURE_FLAGS` 集中列出三类后端共 21 个旗标名，其中分别为 10、4、7 个。
2. `FlagSet` 提供注册、赋值、读取、隐藏状态这组最小接口，用字符串映射模拟 Go `pflag.FlagSet` 在本模块使用到的子集。
3. `define_flags` 以 S3、GCS、Azure 的顺序注册所有选项，默认值均为空字符串。
4. `BackendOptions::parse_from_flags` 按 S3、GCS、Azure 的顺序将旗标投影到后端选项；S3 解析还执行端点规范化并设置路径风格默认值。
5. `hidden_flags_for_stream` 隐藏 GCS 与 Azure 选项，使流式备份场景只保留 S3 选项可见；`DefineFlags` 和 `HiddenFlagsForStream` 是保留 Go 命名的兼容包装。

## 主要符号

- `FlagSet { values, hidden }`：公开结构、私有字段。`values: BTreeMap<String, String>` 保存已注册值，`hidden: BTreeSet<String>` 保存隐藏名称；派生 `Clone`、`Debug`、`Default`。
- `FlagSet::register(&mut self, name, default_value)`：公开注册入口；重复名称通过 `assert!` 触发 panic，消息为 `flag redefined: {name}`。
- `FlagSet::set` / `FlagSet::get`：公开写入与克隆读取接口；名称未注册时返回 `anyhow::Error`，文本为 `flag not defined: {name}`。
- `FlagSet::hide` / `FlagSet::is_hidden`：公开隐藏与查询接口；`hide` 只接受已注册名称。
- `S3BackendOptions`：本文件的 S3 旗标快照，包含 endpoint、region、storage class、SSE、ACL、provider、AssumeRole 相关字段、profile 和 `force_path_style`；其 `parse_from_flags` 为私有方法。
- `BackendOptions`：公开聚合结构，字段为本文件的 `S3BackendOptions`、`pkg/objstore/gcs.rs::GCSBackendOptions` 和 `pkg/objstore/azblob.rs::AzblobBackendOptions`；公开方法 `parse_from_flags` 完成三组解析。
- `define_flags` / `hidden_flags_for_stream`：Rust 风格公开函数；`DefineFlags` / `HiddenFlagsForStream` 仅转发给前两者，并用 `#[allow(non_snake_case)]` 保留 Go 风格名称。
- 本文件没有 trait、异步函数、条件编译项或全局可变状态。

## 执行流程

注册流程从 `define_flags` 开始：它把 `S3_FLAGS.iter()` 与 GCS、Azure 切片串接，逐项调用 `FlagSet::register(name, "")`。任何名称已经存在都会立即 panic，后续名称不会继续注册；正常完成后 21 个名称均存在且值为空。

赋值由调用方显式调用 `FlagSet::set` 完成。解析时，`BackendOptions::parse_from_flags` 先调用私有的 `S3BackendOptions::parse_from_flags`。S3 按 endpoint、region、SSE、KMS key、ACL、storage class、provider、role ARN、external ID、profile 的次序读取；endpoint 使用 `strip_suffix('/')`，因此最多移除一个尾部斜杠；成功读到 storage class 后才把 `force_path_style` 设为 `true`。随后调用 `GCSBackendOptions::parse_from_flags` 读取四个 GCS 值，最后按常量次序逐个读取七个 Azure 值。

流式隐藏流程由 `hidden_flags_for_stream` 遍历 GCS 和 Azure 名称并调用 `FlagSet::hide`。它故意丢弃每次隐藏的错误，因此即使部分名称没有注册，也会继续处理其余名称并最终返回 `Ok(())`。S3 名称不在遍历范围内。

## 数据与状态

`FlagSet` 的全部状态由调用者拥有并通过 `&mut FlagSet` 修改；文件本身没有单例或环境状态。选择 `BTreeMap`/`BTreeSet` 使键集合具有确定顺序，但本文件没有公开枚举接口，不能依赖该顺序形成外部协议。值统一存储为 `String`，这里不解析布尔、数字，也不保留 Go `pflag` 的帮助文本或“用户是否显式设置”状态。

解析目标同样由调用者以 `&mut BackendOptions` 提供。解析不是事务性的：每次成功读取都会立刻覆盖对应字段，后续错误不会回滚先前写入。`pkg/objstore/flags_test.rs::s3_parse_error_preserves_go_assignment_order` 验证了缺少 `s3.storage-class` 时 endpoint、SSE、KMS key 和 ACL 已更新，而 `force_path_style` 尚未改为 `true`。同理，如果 GCS 或 Azure 阶段失败，先完成的 S3 或 GCS 字段会保留。

敏感字段如 `s3.external-id`、Azure account key、SAS token 和 encryption key 都以普通 `String` 存放；本文件不负责脱敏、清零、读取凭据文件或创建客户端，这些行为属于后续配置应用层。

## 依赖与调用关系

直接外部依赖只有 `anyhow::{Result, anyhow}`，由 `pkg/objstore/Cargo.toml` 的 `anyhow = "1"` 提供。标准库提供有序映射和集合。GCS 旗标常量及 `GCSBackendOptions` 来自 `pkg/objstore/gcs.rs`；Azure 旗标常量及 `AzblobBackendOptions` 来自 `pkg/objstore/azblob.rs`。

RustCodeGraph 给出的关键边包括：`define_flags -> FlagSet::register`，并引用三组常量；其调用者为 `DefineFlags`、`flags_test.rs` 中两个测试和 `azblob_1_aster_unit_test.rs::flags_and_hdfs_command_match_go`。`hidden_flags_for_stream -> FlagSet::hide`，其调用者为 `HiddenFlagsForStream`、对应独立测试和同一个跨模块契约测试。`BackendOptions::parse_from_flags` 向下调用本文件 S3 解析与 `gcs.rs::GCSBackendOptions::parse_from_flags`，Azure 部分则直接调用 `FlagSet::get` 赋值。

`pkg/objstore/lib.rs` 公开模块，并通过 `#[path = "flags_test.rs"]` 将独立测试编入测试构建。代码搜索没有发现本文件公开 API 的 Rust 生产调用者；Go 侧则由 Dumpling、BR 等配置链调用 `objstore.DefineFlags`/`BackendOptions.ParseFromFlags`，这只是移植语义参照，不能当成 Rust 接线证据。

## 错误处理与边界

- 未注册名称：`set`、`get`、`hide` 返回 `flag not defined: <name>`；解析函数使用 `?` 原样向上传播 `anyhow::Error`。
- 重复注册：`register` panic，而不是返回 `Result`；`pkg/objstore/flags_test.rs::duplicate_flag_registration_panics_like_go_pflag` 固定了这一兼容行为。
- 部分更新：解析遇错即停，但不回滚之前写入的字段。扩展读取顺序时必须考虑这一可观察行为。
- endpoint 规范化：S3 仅移除一个尾部 `/`。输入 `https://s3.invalid//` 的结果为 `https://s3.invalid/`，测试明确与 Go `strings.TrimSuffix` 对齐。
- 隐藏缺失项：`hidden_flags_for_stream` 丢弃 `hide` 错误并总是返回 `Ok(())`；返回 `Result` 目前不表达实际失败。`hiding_stream_flags_ignores_missing_flags_like_go` 验证了只有一个 Azure 名称注册时仍能成功隐藏它。
- 默认值：只有先执行 `define_flags`，完整解析才会成功；`FlagSet::default()` 本身不含任何旗标。

## 并发与资源生命周期

该文件不创建线程、异步任务、锁、通道、文件、网络连接或运行时。所有修改要求独占的 `&mut` 引用，读取要求共享 `&` 引用，因此并发同步由 Rust 借用规则和更上层的所有者负责；类型没有内部同步机制。

资源生命周期限于内存中的字符串和有序容器：注册或设置时复制输入字符串，`get` 再克隆当前值，结构体释放时由 Rust 自动回收。解析不会消费 `FlagSet`，同一快照可以顺序解析到多个选项对象。敏感字符串也只依赖普通析构，不保证安全擦除。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/objstore/flags.go`、`pkg/objstore/s3like/store.go`、`pkg/objstore/gcs.go` 和 `pkg/objstore/azblob.go`。Rust `define_flags` 对应 Go `DefineFlags` 依次调用 `s3like.DefineS3Flags`、`defineGCSFlags`、`defineAzblobFlags`；名称集合与空字符串默认值对齐。Rust 聚合解析也保持 Go 的 S3、GCS、Azure 顺序及各组内部赋值顺序，S3 的单个尾斜杠裁剪和 `ForcePathStyle = true` 的赋值位置有独立回归测试。

Rust `hidden_flags_for_stream` 与 Go `HiddenFlagsForStream` 一样只隐藏 GCS/Azure，并忽略 `MarkHidden` 错误。差异是 Go 函数无返回值，而 Rust 函数及兼容包装返回一个实际恒为 `Ok(())` 的 `Result<()>`。

Rust `FlagSet` 只是 `pflag.FlagSet` 的局部模拟：不支持命令行 token 解析、类型化 getter、help 文本、changed 状态或 shorthand。Rust 本文件的 S3 类型也只是 Go `s3like.S3BackendOptions` 的旗标字段子集，不含 access key、secret key、session token、accelerate endpoint 等其他配置来源字段。更重要的是，Go `BackendOptions` 定义在 `parse.go` 并直接用于后端解析，而 Rust 在 `flags.rs` 与 `parse.rs` 各自定义同名类型，当前没有转换或统一接线证据。

## 扩展指南

新增后端旗标时，应同时更新对应名称常量/切片、`define_flags` 注册逻辑、目标 options 字段与 `BackendOptions::parse_from_flags` 的读取顺序；若流式命令不应展示该选项，还要更新 `hidden_flags_for_stream`。同步检查 Go 对照文件，避免名称、默认值、尾斜杠处理或部分赋值时序漂移。

测试必须放在独立文件 `pkg/objstore/flags_test.rs`（或确有跨模块契约需求时放入现有独立测试文件），不要内嵌到 `flags.rs`。至少覆盖正常值、默认值、未注册名称、重复注册、解析中途失败后的状态和流式隐藏可见性。若改变 GCS/Azure 旗标，还应检查 `pkg/objstore/azblob_1_aster_unit_test.rs::flags_and_hdfs_command_match_go`。

若要把该层接入生产 Rust CLI，优先解决 `flags.rs::BackendOptions` 与 `parse.rs::BackendOptions`/其他命令自有 `FlagSet` 的类型边界，并增加真实入口到注册、命令行赋值、解析、后端创建的集成证据；不要仅添加一个同名包装便宣称完成接线。兼容风险主要是旗标名和错误/部分赋值顺序，安全风险是凭据字符串暴露，性能风险较低且主要来自不必要的字符串克隆。

## 验证依据

- 源码与模块：`pkg/objstore/flags.rs`、`pkg/objstore/lib.rs`、`pkg/objstore/Cargo.toml`。
- 直接依赖实现：`pkg/objstore/gcs.rs::GCSBackendOptions::parse_from_flags`、`pkg/objstore/azblob.rs::AzblobBackendOptions`。
- Go 对照：`pkg/objstore/flags.go`、`pkg/objstore/s3like/store.go::DefineS3Flags`/`ParseFromFlags`、`pkg/objstore/gcs.go::defineGCSFlags`/`hiddenGCSFlags`/`parseFromFlags`、`pkg/objstore/azblob.go::defineAzblobFlags`/`hiddenAzblobFlags`/`parseFromFlags`。
- Rust 测试：`pkg/objstore/flags_test.rs` 的四个测试；`pkg/objstore/azblob_1_aster_unit_test.rs::flags_and_hdfs_command_match_go`；测试装配见 `pkg/objstore/lib.rs`。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；查询了 `FlagSet`、`BackendOptions`、`define_flags`、`hidden_flags_for_stream`、`parse_from_flags`，并用节点 trail 核对上述调用者与被调用者。由于同名类型和函数较多，结论均限定到 `pkg/objstore/flags.rs` 路径。
- 静态搜索：对 Rust 文件搜索 `objstore::flags`、`crate::flags` 和相关公开符号，未找到测试与 `gcs.rs` 之外的直接生产调用；这是“当前未证实生产接线”的依据，而不是断言未来或动态调用永不存在。
