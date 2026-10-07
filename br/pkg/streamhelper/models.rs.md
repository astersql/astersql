# `br/pkg/streamhelper/models.rs`

## 文件定位

本文件是 `astersql-br-pkg-streamhelper` crate 的任务元数据模型与 etcd 键布局定义。crate 根在 [`br/pkg/streamhelper/lib.rs`](lib.rs) 中以 `pub mod models` 挂载本文件，并通过 `pub use models::*` 将公开符号提升为 crate 级 API；[`br/pkg/streamhelper/Cargo.toml`](Cargo.toml) 声明该 crate 对应 Go 包 `br/pkg/streamhelper`，属于 library、porting lane 2。

它不负责访问 etcd、调度备份或推进 checkpoint，而是向这些流程提供稳定的键名、二进制编码和任务构造规则。实际读写者主要是 [`client.rs`](client.rs) 的 `MetaDataClient`/`Task`，监听者还包括 [`advancer_cliext.rs`](advancer_cliext.rs)。当前 Rust 移植通过 [`stubs.rs`](stubs.rs) 的本地 `KeyRange`、`StorageBackend`、`StreamBackupTaskInfo` 表示协议边界，并未直接依赖 Go 版使用的 kvproto/backuppb 类型。

## 核心职责

1. 固化 `/tidb/br-stream` 下的任务、范围、暂停、checkpoint、外部存储 checkpoint 和最近错误的键空间。`RangesOf`、`CheckPointsOf`、`PrefixOfPause`、`LastErrorPrefixOf` 特意返回带尾斜杠的前缀，避免前缀扫描串入名称相近的任务。
2. 对必须携带任意二进制 start key 的 range 键进行无损拼接。`RangeKeyOf` 先生成 ASCII 前缀，再直接追加 `startKey` 字节，不把它当 UTF-8，也不让路径清洗改变 `//`、零字节或高位字节。
3. 用八字节大端序编码 checkpoint/TS 数值，保证与 Go `encoding/binary.BigEndian` 及读取端 `u64::from_be_bytes` 一致。
4. 提供值语义的 `TaskInfo` 建造器，并在 `Check` 中执行写入前的最小合法性检查：存储存在、表过滤链非空、任务名只包含 ASCII 字母数字或下划线。

这些字符串与字节格式是跨进程、跨版本的持久化协议，不只是内部命名约定；修改会影响现有 etcd 数据、watch、前缀删除和 Go/Rust 互操作。

## 主要符号

- `streamKeyPrefix` 及 `taskInfoPath`、`taskCheckpointPath`、`storageCheckPoint`、`taskRangesPath`、`taskPausePath`、`taskLastErrorPath`：组成持久化键空间。`checkpointTypeGlobal` 的值是 `central_global`；`checkpointTypeRegion`、`checkpointTypeStore` 为 checkpoint 类型标签，并由 `client.rs` 的解析逻辑使用。
- `TASK_NAME_RE: LazyLock<Regex>`：首次执行 `TaskInfo::Check` 的名称分支时惰性编译 `^[0-9a-zA-Z_]+$`；正则字面量固定，因此初始化失败只会是编程错误并通过 `expect` panic。
- `path_join(parts: &[&str]) -> String`：私有的 Go `path.Join` 兼容辅助函数，去除空段和 `.`，以栈方式处理 `..`，并按首段是否以 `/` 开头保留绝对路径形态。它只用于文本路径；二进制 range 后缀明确绕过它。
- `PrefixOfTask`、`TaskOf`、`RangesOf`、`CheckPointsOf`、`GlobalCheckpointOf`、`StorageCheckpointOf`、`Pause`、`PrefixOfPause`、`LastErrorPrefixOf`：公开的键/前缀构造 API。带“Prefix”语义的若干函数会显式补一个尾 `/`。
- `RangeKeyOf(name, startKey) -> Vec<u8>`：返回完整二进制 etcd 键；与其他返回 `String` 的路径函数不同。
- `encodeUint64(num) -> Vec<u8>`：返回固定长度 8 的大端序字节。
- `Ranges = Vec<KeyRange>`、`Range = KeyRange`：保持 Go 包级别命名的类型别名；实际结构来自 `stubs.rs`，表示 `[StartKey, EndKey)` 半开区间。
- `TaskInfo { PBInfo, Ranges, Pausing }`：把可持久化的任务 protobuf 视图、单独存放的 range 列表和暂停初态合并成客户端写入模型。
- `NewTaskInfo`：只初始化 `PBInfo.Name`，其余字段取默认值；随后由 `WithRange`、`WithRanges`、`FromTS`、`UntilTS`、`WithTableFilter`、`ToStorage` 链式补齐。
- `TaskInfo::Check(self) -> Result<Self, String>`：消费并在成功时返回任务本身，便于把校验放在建造器链末端。

## 执行流程

典型创建路径见 `integration_test.rs::simple_task`：调用 `NewTaskInfo`，设置起止 TS，追加 ranges，设置表过滤和外部存储，最后调用 `Check`。`Check` 按“存储、过滤链、名称”的固定次序返回首个错误；成功后保持所有字段原样返回。

持久化时，`client.rs::MetaDataClient::PutTask` 遍历 `TaskInfo.Ranges`，将每个区间写成 `RangeKeyOf(task_name, StartKey) -> EndKey`，再把序列化后的 `PBInfo` 写到 `TaskOf(task_name)`。Rust 内存实现最后发布 task 键，使观察到任务新增事件时 ranges 已可读取；`Pausing` 为真时还会写 `Pause(task_name)`。

读取时，`MetaDataClient::GetTask` 从 `TaskOf` 反序列化 `StreamBackupTaskInfo`；`Task::Ranges` 扫描 `RangesOf`，从键中剥离前缀还原 start key，并把值作为 end key。`Task::NextBackupTSList` 扫描 `CheckPointsOf`，`Task::GetStorageCheckpoint` 扫描 `StorageCheckpointOf`，`Task::LastError` 扫描 `LastErrorPrefixOf`。`MetaDataClient::DeleteTask` 使用同一组函数清理任务及其所有附属键。

监听路径中，`advancer_cliext.rs` 使用 `PrefixOfTask` 与 `PrefixOfPause` 建立任务/暂停 watch，使用 `GlobalCheckpointOf` 和 `encodeUint64` 读写全局 checkpoint。因此路径函数同时服务点查、范围扫描和事件路由。

## 数据与状态

`TaskInfo.PBInfo` 包含 `Name`、`StartTs`、`EndTs`、`TableFilter` 和可选 `Storage`；它是 `TaskOf` 键下序列化的主体。`Ranges` 不嵌入该主体，而是拆成多个 range 键，支持按任务前缀扫描。`Pausing` 也不嵌入 `PBInfo`，而是决定是否存在独立的 pause 键。

所有建造器都按值接收并返回 `TaskInfo`。`WithRange` 会复制传入的起止字节，`WithRanges` 会克隆切片内的 `KeyRange`，`WithTableFilter` 会复制各字符串；建造完成后不借用调用方缓冲区。`RangeKeyOf` 同样把 start key 复制进新 `Vec<u8>`。

路径 API 每次生成新的 `String`/`Vec<u8>`，没有模块级可变状态。唯一的进程级状态是只读的惰性正则。TS 编码固定为八字节大端序，字典序因而与无符号数值序一致。

## 依赖与调用关系

直接外部依赖只有 `regex::Regex`；`std::sync::LazyLock` 管理正则初始化。数据类型依赖本 crate 的 `stubs::{KeyRange, StorageBackend, StreamBackupTaskInfo}`。`Cargo.toml` 中的 `serde`/`serde_json` 由这些 stub 和 `client.rs` 的持久化路径使用，`models.rs` 自身不执行序列化。

RustCodeGraph 对本文件的文件节点列出 7 个使用文件，包括 `client.rs`、`advancer_cliext.rs`、`models_test.rs`、`integration_test.rs` 和 `advancer_cliext_test.rs`。精确查询显示：`PrefixOfTask` 驱动 `client.rs` 的任务枚举和 advancer 的 watch；`TaskOf` 驱动任务增删查写；`RangeKeyOf` 的生产调用者是 `client.rs::PutTask`；`GlobalCheckpointOf` 被客户端和 advancer 的上传、读取、等待及清理路径共享；`LastErrorPrefixOf` 被最近错误读取、清理和任务删除使用。

下游关键关系为：`models.rs` 生成协议键和模型，`client.rs` 执行 KV I/O，`advancer_cliext.rs` 监听或推进 checkpoint。上游调用方不得自行拼接等价字符串，否则容易遗漏尾斜杠规范化或破坏二进制 range key。

## 错误处理与边界

`TaskInfo::Check` 返回 `Result<Self, String>`。缺存储时返回 `the storage backend is null`；空过滤链提示添加 `*.*`；非法名称的错误包含正则和实际名称。这些文案与 Go 实现保持一致，但 Rust 当前没有保留 Go `ErrPiTRInvalidTaskInfo` 的可判别错误类别，调用者只能处理字符串。

名称检查允许空字符串之外的 ASCII 字母、数字和下划线组合，拒绝 `/`、连字符、空白和非 ASCII 字符。因为正则使用 `+`，空名称也会失败。`Check` 当前不要求 ranges 非空，不检查 start/end 次序、范围重叠、`StartTs > 0` 或 `EndTs >= StartTs`；Go 源码也把 TS 与重叠检查保留为 TODO。扩展者不能把这些未实现约束描述成已保证的不变量。

`path_join` 会清洗斜杠、`.` 与 `..`，但任务名应先经 `Check` 拒绝路径字符。直接调用公开路径函数不会自动验证名称，因此未校验输入仍可能被清洗成意外路径。`RangeKeyOf` 刻意不清洗二进制后缀，这是数据正确性要求。

`encodeUint64` 本身无失败分支。正则构造的 `expect` 理论上可 panic，但模式是编译期固定常量。其余路径构造只分配内存，未显式处理分配失败。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄。所有公开构造函数除正则初始化外均为纯计算；返回值拥有自己的内存，可以在线程间按其字段类型的 `Send`/`Sync` 能力移动。

`TASK_NAME_RE` 使用 `LazyLock` 保证并发首次访问只初始化一次，之后只读共享。`TaskInfo` 的建造器独占 `self`，不会产生共享可变状态。etcd 原子性和写入顺序不由本文件保证；当前顺序由 `client.rs::PutTask` 决定，删除也由 `MetaDataClient::DeleteTask` 逐项执行，因此不能从这些模型函数推断多键事务语义。

## 与 Go 版本的对应关系

Go 对照文件是 [`br/pkg/streamhelper/models.go`](models.go)。常量值、路径形状、八字节大端编码、`TaskInfo` 字段、建造器顺序和 `Check` 的三项约束均按 Go 语义移植。Rust 的 `path_join` 替代 Go `path.Join`，`RangeKeyOf` 则与 Go 一样绕开路径清洗并保留原始 start key 字节。

主要语言适配差异如下：Go `NewTaskInfo` 和建造器返回同一个可变指针，Rust 使用消费 `self` 的值语义；Go `WithRanges` 是 variadic，Rust 接收 `&[KeyRange]`；Go `ToStorage` 接收可空指针，Rust 接收非空值并存为 `Some`；Go `Check` 返回带 `ErrPiTRInvalidTaskInfo` 根因的 annotated error，Rust 返回字符串；Go `RangeKeyOf` 以 `string` 承载任意字节，Rust 显式返回 `Vec<u8>`，避免无效 UTF-8 转换。

Go 文件还提供 `TaskInfo::ZapTaskInfo`，用于生成 protobuf、暂停状态和 range 数量的日志字段；Rust `models.rs` 没有对应方法。当前 Rust 的 protobuf/kv 类型来自 `stubs.rs`，属于移植期最小替身，不能据此宣称已经接入真实 etcd/kvproto/backuppb。两侧都尚未实现 TS 合法性和 range 重叠校验。

## 扩展指南

- 新增或修改键空间时，应集中修改本文件的常量与构造函数，并同步审查 `client.rs` 的写入、扫描、删除以及 `advancer_cliext.rs` 的 watch/检查点路径。持久化路径是兼容性协议，若不能同时读取旧布局，需要明确迁移策略。
- 新增前缀函数时，应决定是否必须带尾 `/`，并加入名称前缀碰撞用例。不要用 `path_join` 拼接任意二进制键；参照 `RangeKeyOf` 在规范化文本前缀后逐字节追加。
- 扩展 `TaskInfo` 校验时，优先保持 Go `models.go` 的验证顺序、错误含义与边界一致；若引入 TS/range 检查，应同时更新 Rust 独立测试 `models_test.rs`、`integration_test.rs`、`parity_test.rs` 以及 Go 对照测试，避免单侧收紧导致互操作差异。
- 扩展任务字段时，需要同步 `stubs.rs::StreamBackupTaskInfo`、序列化兼容性和 `client.rs` 的读写路径。若未来替换为真实 protobuf 类型，还需复核默认值、可选字段和 JSON/二进制编码，不应只做类型名替换。
- 建议把纯路径/校验回归放在独立 `models_test.rs`，跨模型与客户端的生命周期放在 `integration_test.rs`，Go/Rust 公共契约放在 `parity_test.rs`；不要把测试嵌回 `models.rs`。

兼容性风险集中在持久化键字节、尾斜杠、checkpoint 类型字符串和错误契约；性能风险主要是大批 ranges 时的逐项复制与逐键写入，但本文件本身没有 I/O。任何优化都必须保留二进制 start key 的逐字节一致性。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter br/pkg/streamhelper` 确认目标及相邻实现/测试已索引；`node --file br/pkg/streamhelper/models.rs --offset 1 --limit 500` 读取目标文件全部 229 行并列出 7 个使用文件；`explore "NewTaskInfo TaskInfo::Check RangeKeyOf in br/pkg/streamhelper only"` 核对上述生产调用边和测试调用边。单独的 `callers` 批量查询在本地 30 秒窗口内未返回，因此调用边又以该精确 `explore` 结果及下列源文件交叉核验。
- 源码与 crate 边界：`br/pkg/streamhelper/models.rs`、`br/pkg/streamhelper/lib.rs`、`br/pkg/streamhelper/Cargo.toml`、`br/pkg/streamhelper/stubs.rs`、`br/pkg/streamhelper/client.rs`、`br/pkg/streamhelper/advancer_cliext.rs`。
- Go 对照：`br/pkg/streamhelper/models.go`，以及调用侧 `br/pkg/streamhelper/client.go`、`br/pkg/streamhelper/advancer_cliext.go` 的同名 API 使用点。
- Rust 独立测试：`models_test.rs` 验证含 `0xff`、`//`、零字节等 start key 的无损拼接及实际 KV 写入；`parity_test.rs::go_rust_public_contract_matches` 验证路径、大端编码和校验约束，`task_metadata_methods_match_go_contract` 验证 ranges、暂停、checkpoint、错误和删除生命周期；`integration_test.rs::test_checking` 与 `simple_task` 覆盖建造器和错误分支。
- 本任务只生成文档，按计划不运行 Cargo。交付前使用任务规定的 `rg -c` 命令验证恰有 11 个固定二级章节，并人工复核文档没有把 stub、未实现校验或多键事务写成已具备能力。
