# `br/pkg/logutil/stubs.rs`

## 文件定位

该文件是 `astersql-br-pkg-logutil` crate 的本地依赖适配层，由 [`lib.rs`](./lib.rs) 通过 `#[path = "stubs.rs"] pub mod stubs` 无条件装配，并把 `kvproto`、`KeyRange`、`KvKey` 再导出给 crate 使用者。它服务于 [`logging.rs`](./logging.rs) 的日志字段构造和格式化，不是 BR 备份协议、Raft 元数据或 TiKV RPC 的权威实现。

[`Cargo.toml`](./Cargo.toml) 明确记录了这一边界：为了在 Darwin arm64 上避免引入 `kvproto`、`grpcio`、TiDB `kv` 和 `util-redact`，本 crate 用本文件提供与 Go 调用形状相近的最小类型。文件顶部注释同样声明这些类型只模拟 getter/setter，不提供完整 protobuf 编解码能力。因此，虽然文件位于生产模块且会进入普通编译产物，其角色仍是“可编译、可测试的局部替身”，不能当作跨进程数据模型。

## 核心职责

本文件承担三组职责：

1. 提供脱敏子集：进程级 `NEED_REDACT`、测试切换函数 `set_need_redact_for_test`，以及与 `util/redact` 同名的 `NeedRedact`、`Value`、`Key`。这些函数被 `logging.rs` 的 key、范围和通用字段格式化路径消费。
2. 提供 TiDB KV 范围的最小载体：`KvKey(Vec<u8>)` 和半开区间 `KeyRange { StartKey, EndKey }`，供 `StringifyRange` / `StringifyKeys` 转换和展示。
3. 在 `kvproto::{brpb, import_sstpb, metapb}` 命名空间中提供日志格式化所需的 protobuf 形状子集，包括备份文件、流备份任务、SST 范围与元数据、重写规则、Region epoch、Peer 和 Region。

职责边界很窄：本文件不做序列化/反序列化、不验证字段组合、不发起网络调用、不实现 protobuf unknown fields，也不包含业务恢复流程。字段的实际消费和日志键名位于 `logging.rs`，Go 的完整数据结构来自 `github.com/pingcap/kvproto` 与 TiDB `pkg/kv`。

## 主要符号

- `NEED_REDACT: AtomicBool`：全局脱敏状态，初始值为 `false`。
- `set_need_redact_for_test(on: bool)`：以 `SeqCst` 写入脱敏状态；公开但按注释只供测试切换。
- `NeedRedact() -> bool`：以 `SeqCst` 读取全局开关；`logging::Redact` 与 `logging::RedactAny` 直接调用它。
- `Value(&str) -> String`：开关打开时返回 `"?"`，否则复制输入字符串。
- `Key(&[u8]) -> String`：开关打开时返回 `"?"`；否则逐字节生成两位大写十六进制。需要注意，`logging.rs` 自身的通用 `hex_encode` 输出小写，但敏感 key 经过本函数时使用大写字母格式。
- `KvKey(Vec<u8>)`：拥有字节数据，并实现 `AsRef<[u8]>`；派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`、`Hash`。
- `KeyRange`：由公开的 `StartKey: KvKey` 与 `EndKey: KvKey` 组成，表达 `[start, end)`；空 `EndKey` 的“正无穷”解释由 `logging::StringifyRange` 完成，而不是类型自身保证。
- `kvproto::brpb::File`：保存 `name`、`cf`、`sha256`、起止 key、起止版本、KV/字节统计、`crc64xor` 和 `size`；`FileMarshaler` 与 `FilesMarshaler` 使用这些 getter。
- `kvproto::brpb::StreamBackupTaskInfo`：保存任务名、起止 TS 和表过滤器列表；同时提供整体替换与 `mut_table_filter()` 原位修改入口。
- `kvproto::import_sstpb::{Range, RewriteRule, SstMeta}`：分别保存 SST 字节范围、旧/新 key 前缀与新时间戳、以及日志所需的 SST 元数据。`SstMeta` 对嵌套 `Range` 同时提供只读 getter、整体 setter 和可变 getter。
- `kvproto::metapb::{RegionEpoch, Peer, Region}`：提供 Region 日志格式化所需字段；`Region` 的 peers 支持切片读取、整体替换与 `mut_peers()` 追加。

所有 protobuf 替身都派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`，字段保持私有，通过 `new()`（等价于 `Default`）以及 `get_*` / `set_*` 访问。文件没有 trait 定义、条件编译项或自定义错误类型。

## 执行流程

典型日志流程如下：

1. 调用方构造本文件中的替身对象，并通过 `set_*` 或 `mut_*` 填充字段。例如 `logging_test.rs::test_region` 先创建 `RegionEpoch` 和两个 `Peer`，再放入 `Region`。
2. 调用方把对象交给 `logging.rs` 暴露的 `File`、`Files`、`StreamBackupTaskInfo`、`RewriteRule`、`Region`、`Leader`、`Peer`、`SSTMeta`、`SSTMetas` 或 `BriefSSTMetas`。
3. 对应 marshaler 读取 `get_*` 值并写入日志 encoder。敏感 key 路径调用本文件的 `Key`；`Redact` / `RedactAny` 先调用 `NeedRedact` 决定保留结构还是用问号替换。
4. 对 `KeyRange`，`logging.rs` 的 `From<KeyRange> for StringifyRange` 消费两个 `KvKey` 的所有权；展示时起点调用 `Key`，终点为空则把 `"inf"` 交给 `Value`，否则同样调用 `Key`。

本文件自己的 getter/setter 不包含分支、校验或失败路径；有意义的分支集中在脱敏函数：开关关闭时保留内容，打开时统一返回问号。`Key` 的非脱敏分支按输入长度预分配 `2 * len` 字符容量，并对每个字节进行十六进制格式化。

## 数据与状态

唯一共享可变状态是 `NEED_REDACT`。它是静态 `AtomicBool`，默认关闭，作用域覆盖整个进程而非单次日志、线程或上下文；测试若调用 `set_need_redact_for_test`，必须恢复原状态，避免影响同进程中并行执行的其他用例。

其余类型均拥有自己的数据：字符串、字节向量、数值、布尔值或嵌套结构，没有借用宿主对象，也没有内部共享。`Default` 会产生空字符串/空向量、数值零、布尔 `false` 和默认嵌套对象。该便利行为也意味着“缺失字段”和“显式零值”无法区分；与真实 protobuf 的 presence、optional/message 指针语义并不等价。

getter 返回标量副本或对内部字符串、切片、嵌套对象的共享借用；setter 接收拥有所有权的值并整体替换。`mut_table_filter`、`mut_range`、`mut_peers` 暴露内部容器或对象的可变借用，修改遵循普通 Rust 独占借用规则。没有缓存、延迟初始化或派生字段，读取值始终反映最近一次写入。

## 依赖与调用关系

直接下游依赖只有 Rust 标准库的 `std::sync::atomic::{AtomicBool, Ordering}`；数据容器全部使用标准 `String` 和 `Vec`。`Cargo.toml` 没有因本文件引入 `kvproto`、`grpcio` 或 redact crate，这正是本地桩存在的原因。

直接上游是 crate 根和 `logging.rs`：

- `lib.rs` 声明 `pub mod stubs`，并公开再导出 `kvproto`、`KeyRange`、`KvKey`。
- `logging.rs` 导入 `Key as RedactKey`、`Value as RedactValue`、`NeedRedact`、`KeyRange`，以及三个 `kvproto` 子模块中的类型。
- RustCodeGraph 将 `stubs.rs` 标记为被 84 个索引文件使用；具体主链集中在 `logging.rs`。例如 `NeedRedact` 被 `Redact` / `RedactAny` 调用，`SstMeta::get_range` 被 `SSTMetaMarshaler` 和 `BriefSSTMetas` 调用，`Region::get_peers` 被 `RegionMarshaler` 调用。
- `logging_test.rs` 与 `parity_test.rs` 通过 crate 根再导出的 `kvproto` 构造 fixtures，验证这些形状最终生成的 JSON；它们是本文件最接近的独立 Rust 测试，没有把测试逻辑内嵌在 `stubs.rs`。

调用方向是“调用方填充替身对象 → logging marshaler 读取 → encoder 输出”，本文件不反向依赖 `logging.rs`，因此没有循环依赖。

## 错误处理与边界

所有构造、getter、setter 和脱敏函数都是不可失败 API，不返回 `Result` 或 `Option`。这并不代表输入已经有效：本文件不验证 UUID 长度、范围顺序、Region epoch 是否已设置、peer 唯一性、校验和、时间戳或文件统计的一致性。

重要边界包括：

- 空字节 key 在非脱敏模式下编码为空字符串；空范围终点是否表示无穷由 `StringifyRange` 的消费者语义决定。
- `Key` 使用大写 hex；Go `redact.Key` 在现有测试的数字字节上与之相同，但包含 `a`–`f` 的字节需要留意格式契约。普通 `logging::hex_encode` 仍是小写。
- 嵌套消息始终有默认值，而真实 Go protobuf 字段可能是 `nil`。例如 `SstMeta::get_region_epoch()` 和 `get_range()` 永远返回引用，因而无法复现 Go 对 nil 嵌套消息的行为。
- `SstMeta` 接受任意 UUID 字节；无效 UUID 的回退文本由 `logging.rs::SSTMetaMarshaler` 产生，并由测试覆盖。
- `u64` 聚合溢出、无效业务组合和协议兼容都不由本文件处理；调试构建下的算术行为也不应被误认为输入验证。

## 并发与资源生命周期

`NEED_REDACT` 使用 `SeqCst` load/store，保证不同线程对开关更新具有单一全序可见性；但 API 没有作用域 guard、引用计数或自动恢复，所以两个并发测试互相切换时仍可能发生语义干扰。安全扩展时应避免把它误用成请求级配置。

其余结构不启动线程、异步任务或定时器，不持有锁、通道、文件、网络连接、事务或外部句柄。对象生命周期完全由所有权决定：传入 `logging.rs` 的公开构造函数时通常按值移动，数组 marshaler 需要时克隆 `SstMeta`；getter 借用仅在对象存活期间有效。对象析构只释放其拥有的 `String`、`Vec` 和嵌套值，没有额外清理协议。

## 与 Go 版本的对应关系

Go 同路径 [`logging.go`](./logging.go) 直接依赖真实 `backuppb`、`import_sstpb`、`metapb`、`kv.KeyRange` 和 `util/redact`；Go 目录中不存在 `stubs.go`。因此，本文件不是某个 Go 文件的逐行移植，而是为 Rust `logging.rs` 保持 Go 日志契约而新增的适配边界。

对应关系如下：

- `KvKey` / `KeyRange` 对应 `tidb/pkg/kv.Key` 与 `kv.KeyRange` 的日志展示所需部分。
- `kvproto` 三个子模块只复制 Go `logging.go` 实际读取的字段和生成代码式 getter/setter 形状；真实 Go 类型仍具有 protobuf 编解码、字段 presence 和更多字段。
- `NeedRedact`、`Key`、`Value` 对应 `pkg/util/redact` 的调用点，使 Rust 的 `RedactAny`、`Redact`、key 与范围展示遵守同一“开关打开则问号遮罩”的意图。
- Rust 接口大量按值传递，而 Go 接口普遍接收 protobuf 指针；Rust 的嵌套消息用默认值代替 Go 的可空指针。

现有 [`logging_test.go`](./logging_test.go) 的 `TestRewriteRule`、`TestRegion`、`TestLeader`、`TestSSTMeta` 与 Rust `logging_test.rs` 中同名语义用例给出交叉证据；`parity_test.rs::go_rust_public_contract_matches` 进一步集中核对公开 JSON 契约。当前测试验证了主要字段形状和未开启脱敏时的输出，但没有直接调用 `set_need_redact_for_test(true)` 覆盖开关开启与恢复场景。

## 扩展指南

新增日志字段时，先从 `logging.rs` 的真实读取点反推所需最小数据面，再扩展对应替身结构和 getter/setter；不要为了“看起来像完整 protobuf”复制未使用字段。若新增真实协议行为、wire 编解码、RPC 或字段 presence 需求，应改为评估接入正式依赖，而不是继续扩大本桩的职责。

修改时至少同步检查：

- `br/pkg/logutil/logging.rs` 中相应 marshaler 的字段读取与日志键名；
- `br/pkg/logutil/logging_test.rs` 中最接近的独立单元测试；
- `br/pkg/logutil/parity_test.rs` 的 Go/Rust 公开契约覆盖；
- `br/pkg/logutil/logging.go` 和 `logging_test.go` 的源语义；
- `lib.rs` 的公开再导出，以及 `Cargo.toml` 中“不引入重依赖”的约束。

扩展风险主要有三类：字段默认值掩盖 Go 的 nil/presence 分支，getter 命名或所有权形状与调用方不兼容，以及脱敏遗漏导致敏感 key 落日志。若修改全局脱敏开关，应增加独立测试并串行化或使用自动恢复 guard，避免并发污染；Rust 单元测试继续放在 `logging_test.rs` / `parity_test.rs` 等独立文件中，不应写回 `stubs.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/logutil` 确认 `stubs.rs`、`logging.rs`、`lib.rs`、两个独立 Rust 测试及 Go 对照文件均在索引中。
- RustCodeGraph：`node --file br/pkg/logutil/stubs.rs` 分段读取了全部 584 行，确认全部公开函数、结构、模块、字段访问器以及唯一原子状态；`explore 'br/pkg/logutil/stubs.rs symbols callers callees role in logutil crate'` 给出了 `NeedRedact`、protobuf getter/setter与 `logging.rs` / 测试的调用关系。
- RustCodeGraph：读取 `logging.rs` 的 `FileMarshaler`、`StreamBackupTaskInfoMarshaler`、`RewriteRuleMarshaler`、`RegionMarshaler`、`SSTMetaMarshaler`、`BriefSSTMetas`、`Redact`、`RedactAny`、`StringifyRange` 和 `StringifyKeys`，核对替身字段的真实消费路径。
- 源与配置：读取 `br/pkg/logutil/Cargo.toml`、`lib.rs`、`logging.go` 和 `logging_test.go`，确认 crate 边界、Darwin arm64 精简依赖目的及 Go 的真实依赖/日志契约；目标包未发现 `doc.go`。
- 独立 Rust 测试：读取 `br/pkg/logutil/logging_test.rs` 与 `parity_test.rs`，确认 `File`、`RewriteRule`、`RegionEpoch`、`Peer`、`Region`、`Range`、`SstMeta` 的构造与 JSON 断言。未运行 Cargo，符合本任务“纯文档分析，不运行 Cargo”的明确限制。
