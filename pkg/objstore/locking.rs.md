# `pkg/objstore/locking.rs`

源文件：[`locking.rs`](locking.rs)

## 文件定位

本文件属于 `astersql-objstore` crate，由 [`pkg/objstore/lib.rs`](lib.rs) 以公开模块 `objstore::locking` 装配；crate 边界和依赖声明见 [`pkg/objstore/Cargo.toml`](Cargo.toml)。它在通用 `StorageRef` 抽象之上用对象名、条件检查和 JSON 元数据实现跨进程远程锁，因而可供本地文件、内存或对象存储后端复用，而不绑定某个云厂商。

当前仓库的直接 Rust 引用证据主要在独立测试：[`locking_test.rs`](locking_test.rs)、[`helper_2_aster_unit_test.rs`](helper_2_aster_unit_test.rs) 和 S3 根前缀回归测试 [`s3store/s3_test.rs`](s3store/s3_test.rs)。公开 API 可由 crate 外部调用；本次 RustCodeGraph 文件关系没有显示生产 Rust 调用者，因此不能据此声称它已经接入某条更上层业务主链。

## 核心职责

- `TryLockRemote` 提供单对象互斥锁：锁对象就是调用者给出的 `path`。
- `TryLockRemoteRead` / `TryLockRemoteWrite` 通过 `{path}.READ.<随机值>` 与 `{path}.WRIT` 命名实现多读共享、读写互斥和写写互斥。
- `ConditionalPut::commit_to` 用 INTENT 对象执行两阶段竞争检查，使强一致存储上的同前缀竞争最多只有一个提交者成功。
- `RemoteLock` 保存本地事务 ID，并在删除远端锁对象前校验对象内的 `txn_id`，防止一个过期句柄直接删除后来持有者的锁。
- `LockMeta`、`ErrLocked`、`LockBlocker` 和 `LockConflictLogFields` 提供兼容的持久化元数据、有限采样的冲突诊断和结构化日志字段。
- `LockWithRetry` 为任意同签名加锁函数增加指数退避、固定于本轮调用的随机抖动和取消感知。

这里的锁依赖对象存储的强一致列举/读写语义。`ConditionalPut::commit_to` 对不保证强一致的后端只输出警告，代码注释和实现都明确表示这种后端上的并发安全没有保证。

## 主要符号

- 私有常量 `LOCK_BLOCKER_ERROR_LIMIT`、`LOCK_BLOCKER_META_LIMIT`、`LOCK_BLOCKER_LOG_LIMIT` 都为 3，分别限制错误文本、读取到内存的 blocker 元数据和日志输出；`LOCK_RETRY_TIMES` 为 60。
- 私有 `ConditionalPut { target, content, verify, local }` 描述一次条件写；`ContentFn` 根据事务 UUID 生成目标内容，`VerifyFn` 插入互斥规则。
- 私有 `VerifyWriteContext` 持有 `Context`、目标路径、`StorageRef` 和事务 UUID。`intent_file_name` 生成 `{target}.INTENT.{32位简单UUID}`；`conflicting_objects_of_prefix_expect` 列举包含 tombstone 的同前缀对象并排除期望对象；`assert_only_my_intent` 保证只剩本事务的 INTENT。
- 公共 `LockMetaInput` 是调用侧诊断输入：`owner_id`、`lock_type`、`hint`。其中 `lock_type` 只是标签，不决定锁兼容性；兼容性完全由调用的锁函数和对象名布局决定。
- 公共 `LockMeta` 是 JSON 持久化格式，包含 UTC 时间、主机、PID、base64 编码的事务 ID 以及调用侧字段。`owner_id` 与 `lock_type` 有 serde 默认值，兼容缺少新字段的旧对象。
- 公共 `LockBlocker` 保存冲突对象路径、可读取的 `LockMeta` 或元数据读取错误；公共 `ErrLocked` 保存目标、本地请求、远端代表元数据、冲突总数及最多三个样本，并实现 `Error`。
- `MakeLockMeta` 填入当前 UTC 时间、主机名和进程 ID；主机名读取失败时写入 `UnknownHost(err=...)`，而不是终止加锁。
- 公共 `RemoteLock` 封装私有 `txn_id`、`storage` 和 `path`；`Unlock` 返回错误，`UnlockOnCleanUp` 是尽力清理且不向调用者传播错误。
- 公共入口 `TryLockRemote`、`TryLockRemoteWrite`、`TryLockRemoteRead` 返回 `Result<RemoteLock>`；`LockWithRetry<F>` 接受同形状闭包；`LockConflictLogFields` 返回 `(String, String)` 字段列表。

## 执行流程

互斥锁入口 `TryLockRemote` 先构造 `ConditionalPut`，其目标就是 `path`，内容回调调用 `serialize_lock_meta`。`commit_to` 创建 UUID，随后执行：

1. 调用可选业务校验，并用 `assert_only_my_intent` 检查目标前缀没有任何其它对象。
2. 写入空的 `{target}.INTENT.{txn}` 对象。
3. 重复同一组校验；并发参与者此时能互相看见 INTENT，因此只有一个能继续。
4. 将带相同事务 UUID 的 `LockMeta` JSON 写入目标对象。
5. 无论第 3/4 步成功与否都尝试删除 INTENT；清理失败只打印人工清理提示，提交错误仍按原结果传播。

写锁 `TryLockRemoteWrite` 把目标改为 `{path}.WRIT`，其额外校验列举原始 `path` 前缀，因此会发现其它 INTENT、写锁及所有 `.READ.*`。读锁 `TryLockRemoteRead` 为每个持有者生成 `{path}.READ.<16位十六进制随机值>`，额外校验只列举 `{path}.WRIT` 前缀；不同读锁对象可以共存，但写锁会阻止新读锁。两种入口仍会执行各自目标的 INTENT 检查。

成功后 `RemoteLock::Unlock` 重新读取并解析远端 JSON；只有远端 `txn_id` 与句柄 UUID 字节完全一致才删除对象。`LockWithRetry` 每轮调用传入的 locker，失败后按 1、2、4、8、16、32、60 秒封顶的指数延迟加一次 2.5–7.5 秒抖动；初次尝试后最多重试 60 次。`Context::wait_timeout` 一旦因取消等原因失败，就把等待错误附在最后一次加锁错误上并立即返回。

## 数据与状态

持久状态全部位于对象存储：临时 INTENT、最终互斥/写/读锁对象，以及最终对象内的 `LockMeta` JSON。进程内 `ConditionalPut` 和 `VerifyWriteContext` 只服务单次提交；`RemoteLock` 是成功提交后的所有权凭据，不自动在 `Drop` 时解锁。

JSON 的 `txn_id` 使用标准 base64；UUID 原始 16 字节由 `serialize_lock_meta` 写入。`LockMeta::Display` 有意不展示事务 ID，只展示时间、主机、PID、hint 和非空 owner/type。冲突扫描始终保留 `blocker_count` 总数，但只读取前三个对象的元数据，因此高竞争时诊断成本和输出有界；错误文本和日志也各自最多展示三个样本。

读锁后缀来自 `rand::thread_rng()` 的非负 63 位值。它用于降低对象名碰撞概率，不是所有权或密码学令牌；真正用于解锁校验的是 UUID。`Arc<dyn Storage>`、`Arc<dyn Fn + Send + Sync>` 允许锁流程跨线程持有共享后端与回调，但本文件没有常驻任务、通道或进程内互斥状态。

## 依赖与调用关系

上游边界是 `pkg/objstore/lib.rs` 的 `pub mod locking`。RustCodeGraph `node --file pkg/objstore/locking.rs` 报告该文件被 `helper_2_aster_unit_test.rs` 与 `s3store/s3_test.rs` 使用；`lib.rs` 还单独装配 `locking_test.rs`。仓库文本引用验证了这些测试分别调用互斥/读写入口和 S3 根路径前缀错误路径。

下游主要是 `crate::storage::{Context, StorageRef, WalkOption}`：冲突发现调用 `WalkDir`，元数据读取调用 `ReadFile`，INTENT/锁提交调用 `WriteFile`，释放与清理调用 `DeleteFile`，显示句柄时调用 `URI`，一致性判断调用 `is_strong_consistent`。`Context` 还提供 `clone`、`is_cancelled`、`background` 和 `wait_timeout`。

外部 crate 依赖由 `pkg/objstore/Cargo.toml` 声明：`anyhow` 负责上下文错误链，`chrono` 负责 UTC 时间及 serde，`serde`/`serde_json` 负责持久化，`uuid` 生成 v4 事务 ID，`base64` 编解码 UUID 字节，`hostname` 读取主机名，`rand` 生成读锁名和重试抖动。

## 错误处理与边界

- `WalkDir`、`ReadFile`、`WriteFile`、`DeleteFile` 的错误以 `anyhow::Context` 增加阶段或路径信息；初检和 INTENT 后复检可由错误文本区分。
- 真实锁冲突使用可 downcast 的 `ErrLocked`。`with_lock_context` 与 `annotate_lock_attempt_error` 补全 path、本地输入和远端元数据，同时保留原始错误链；`locking_test.rs::test_try_lock_remote_write_preserves_original_error_when_enriching_err_locked` 专门验证这一点。
- blocker 元数据读取失败不会丢失冲突：对应 `LockBlocker` 保留路径及错误字符串。目标元数据无法读取时，错误富化仍返回原加锁错误而非把解析失败替换成主错误。
- `serialize_lock_meta` 对这个固定结构的 JSON 序列化使用 `expect`，把序列化失败视为不可达；这不同于存储 I/O，后者全部返回 `Result`。
- `Unlock` 遇到不存在/损坏的锁对象、事务 ID 不匹配或删除失败都会返回错误。它没有存储级 compare-and-swap：读取校验与删除之间仍有窗口，因此实现只能检测读到的覆盖，不能原子阻止随后发生的覆盖。
- `UnlockOnCleanUp` 在输入 context 已取消时改用无截止时间的 background context，并吞掉清理错误、只打印提示；调用方不能用它确认锁一定释放。
- 路径拆分用最后一个 `/`；根路径的目录为空而不是 `.`。`s3store/s3_test.rs::test_try_lock_remote_root_path_prefix` 验证根路径列举前缀仍是完整锁名。

## 并发与资源生命周期

安全并发依赖强一致的“列举后可见”语义。两个竞争者先后写 INTENT 后，二次扫描会发现对方；测试 `locking_test.rs::test_concurrent_lock` 用两个线程验证同一互斥路径恰好一个成功。弱一致后端可能漏看 INTENT 或锁对象，所以警告不是安全降级机制。

INTENT 的正常生命周期局限于 `commit_to`；目标写入之后才删除 INTENT。删除 INTENT 失败会留下 tombstone/对象，而扫描明确包含 tombstone，这可能持续阻塞后续加锁并需要人工清理。最终锁对象持续存在到显式 `Unlock`；句柄离开作用域不会释放。

读锁之间没有共享计数器，每个持有者独占一个对象，因此单个读者可独立释放。写锁扫描整个资源前缀，成本随该前缀对象数增长；调用者应给锁选择不会与非锁业务对象混杂的专用路径前缀。`LockWithRetry` 同步等待，不启动后台线程；取消由 `Context` 在等待阶段观察。

## 与 Go 版本的对应关系

直接对照文件是 [`locking.go`](locking.go)，行为与命名基本逐项移植：`conditionalPut`/`VerifyWriteContext` 两阶段 INTENT 协议、三种 TryLock 入口、`.WRIT`/`.READ.*` 布局、`RemoteLock`、元数据 JSON、blocker 三项采样、60 次重试和日志字段都有对应实现。独立 Go 测试 [`locking_test.go`](locking_test.go) 与 Rust `locking_test.rs` 覆盖相同核心场景，包括并发唯一胜者、旧 JSON 兼容、冲突元数据、错误采样和取消时保留错误链。

已核实的实现差异如下：Go 用 failpoint 精确控制两阶段并发时序，Rust 并发测试使用线程屏障但没有阶段 failpoint；Go `UnlockOnCleanUp` 在原 context 完成后创建 30 秒超时 context，Rust 使用无截止时间的 `Context::background()`；Go 产出 `zap.Field` 并记录每次重试，Rust 返回字符串字段且本函数本身不输出重试日志；Go 的错误类型可保留多 cause，Rust 通过 `anyhow` 上下文和可 downcast 的 `ErrLocked` 保留可观察信息。Rust 的核心锁兼容规则和持久化字段未做简化。

## 扩展指南

- 新增锁兼容模式时，优先复用 `ConditionalPut`，在 `VerifyFn` 中明确要扫描的对象前缀和需排除的本事务 INTENT；同时检查新命名不会被现有写锁的 `path` 前缀扫描误伤或漏检。
- 修改持久化字段时更新 `LockMetaInput`、`LockMeta`、`MakeLockMeta`、显示/日志字段及 Go 同名结构。新字段若需读取旧锁对象，应提供 serde 默认并在 `locking_test.rs` 增加旧 JSON 回归。
- 修改冲突采样策略时同步三个 limit、`ErrLocked::remote_blocker_count`、`Display` 和 `LockConflictLogFields`，保留“总数准确、样本有界”的不变量，并同步 Go 测试意图。
- 改动释放协议时必须正视当前“读后删”不是原子 CAS 的边界；若底层 `Storage` 增加条件删除能力，应在 `RemoteLock::Unlock` 接入并为覆盖竞争增加独立测试。
- 修改重试时同步 `LOCK_RETRY_TIMES`、退避上限、抖动范围和取消错误链测试。避免在通用锁层硬编码某一业务的 owner/type 语义。
- Rust 单元测试继续放在独立的 `pkg/objstore/locking_test.rs`，不要内嵌到生产文件；涉及模块级通用行为可同步 `helper_2_aster_unit_test.rs`，后端特有前缀语义放在相应后端测试中。

兼容性风险集中在对象命名和 JSON 字段；正确性风险集中在一致性保证、前缀选择、INTENT 残留及非原子解锁；性能风险集中在每次校验的前缀列举与最多三次 blocker 元数据读取。

## 验证依据

- RustCodeGraph：`status` 显示本地索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/objstore/locking.rs --offset 1 --limit 400` 与 `--offset 380 --limit 380` 覆盖源文件全貌，并报告 `helper_2_aster_unit_test.rs`、`s3store/s3_test.rs` 两个使用文件。精确 `callers TryLockRemote` 和后续 `explore` 在 30 秒内没有返回内容，所以上游结论由模块装配与直接引用搜索补证，未臆造生产调用边。
- 源码与装配：`pkg/objstore/locking.rs`、`pkg/objstore/lib.rs`；该目录没有 `doc.go`，因此模块契约以 crate 入口注释、源码与测试为准。
- crate 边界：`pkg/objstore/Cargo.toml` 的 package 名、`lib.rs` 路径、依赖和 `package.metadata.porting.go-package = "pkg/objstore"`。
- Rust 测试：`pkg/objstore/locking_test.rs`；补充覆盖为 `pkg/objstore/helper_2_aster_unit_test.rs::remote_lock_enforces_mutex_and_read_write_compatibility` 与 `pkg/objstore/s3store/s3_test.rs::test_try_lock_remote_root_path_prefix`。
- Go 对照：`pkg/objstore/locking.go`、`pkg/objstore/locking_test.go`，逐项核对两阶段协议、读写规则、重试、清理和诊断语义。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节、文件链接/路径、变更范围和事实一致性。
