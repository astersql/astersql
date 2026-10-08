# `pkg/util/redact/redact.rs`

## 文件定位

`redact.rs` 是 `astersql-util-redact` crate 的业务实现文件，由 `pkg/util/redact/lib.rs` 以 `pub mod redact` 声明并全量重导出。crate 的 `Cargo.toml` 将库入口设为 `lib.rs`，直接依赖 `kvproto` 的 BR protobuf 类型和 `path-clean`。根 workspace 又通过 `facade_util_redact` 将它纳入顶层 facade。

它不是 SQL 脱敏的通用解析器，而是面向日志、EXPLAIN 展示、反脱敏工具和 BR 备份任务展示的底层实用 API。当前可直接确认的生产接线包括：`pkg/session/nontransactional.rs::redact_sql` 调用 `String`，`pkg/session/runtime/explain_select.rs` 在构造执行计划字面量时调用 `WriteRedact`。`DeRedactFile`、全局开关 API 和 `TaskInfoRedacted` 在本 crate 中有独立测试证据，但本次检索未证实它们已由其他生产 Rust 代码直接调用。

## 核心职责

- 按 `OFF` / `ON` / `MARKER` 三种 `tidb_redact_log` 模式处理字符串：原样保留、完全隐去，或用 `‹…›` 标记敏感片段。
- 通过 `FmtStringer` / `redactStringer` 把同样的策略延迟应用到可字符串化对象。
- 用 `DeRedact` 对标记日志逐行解析：可以保留内容而去掉标记，也可以用 `?` 移除整个敏感片段；`DeRedactFile` 提供文件/stdout 适配层。
- 管理进程内全局脱敏开关，并为普通值与二进制 key 提供简便展示函数。
- 在不修改原始 protobuf 对象的前提下，遮盖 BR `StreamBackupTaskInfo` 中 S3、GCS 和 Azure 后端的凭证字段。

## 主要符号

- `REDACT_LOG_DISABLE` / `REDACT_LOG_ENABLE` / `REDACT_LOG_MARKER`：内部模式字符串常量，值分别是 `OFF`、`ON`、`MARKER`。
- `REDACT_LOG_ENABLED: OnceLock<RwLock<String>>`：延迟初始化的进程全局模式，默认为 `OFF`。`redact_log_enabled` 是唯一的初始化入口。
- `RedactResult<T>`：文件与 I/O 路径使用的 boxed error 结果类型，允许 `Send + Sync`。
- `FmtStringer::String(&self) -> String`：对齐 Go `fmt.Stringer` 的本地 trait。`redactStringer` 保存模式与借用的 trait object，其 `String` 实现先求底层文本，再调用顶层 `String`。
- `String(mode, input)`：三模式的核心纯函数。`MARKER` 会包裹文本，并将输入中已有的 `‹` / `›` 各写两次，防止反解析误认边界。
- `Stringer(mode, input)`：构造延迟脱敏包装，返回的值借用 `input`，不获取对象所有权。
- `DeRedactFile(remove, input, output)`：清理路径后打开输入；输出为 `-` 时锁定 stdout，否则截断/创建普通文件，然后用换行符调用 `DeRedact`。Unix 创建模式显式设为 `0644`。
- `DeRedact(remove, input, output, sep)`：泛型 `Read`/`Write` 实现，是标记状态机与错误传播的核心。
- `InitRedact` / `NeedRedact` / `Value` / `Key`：分别设置全局 `ON/OFF`、判定是否脱敏、展示普通字符串、以大写十六进制展示 key。脱敏开启时后两者都返回 `?`。
- `WriteRedact(build, v, redact)`：直接追加到调用者的 `String`；`MARKER` 包裹、`ON` 写 `?`，其他值原样追加。与 `String` 不同，此函数不对 `v` 中已有的标记字符进行加倍转义。
- `TaskInfoRedacted { Info }`：借用可选 `StreamBackupTaskInfo`。其 `FmtStringer` 实现在副本上替换凭证，再通过 kvproto 的紧凑 `Debug`/`PbPrint` 格式输出。

## 执行流程

`String` 首先匹配模式。`OFF` 复制输入，`ON` 返回空串，`MARKER` 预分配缓冲区，写入左边界，遍历 Unicode 字符并把已有边界加倍，最后写入右边界。非法模式在 debug 构建触发断言，其他构建回退为空串。

`DeRedact` 用 `BufReader::lines` 逐行处理。每行维护 `start` 布尔状态与一个敏感片段缓冲区：

1. 区间外遇到 `‹` 时进入区间并清空缓冲；其他字符直接输出。
2. 区间内的普通字符暂存。`‹‹` 解码为一个字面 `‹`；如果第二个字符不是 `‹`，两者都被保留在缓冲区。
3. 区间内遇到 `›`时检查下一字符：`››` 解码为字面 `›`；否则闭合区间，`remove=true` 输出 `?`，`remove=false` 输出缓冲内容，且非标记的“下一字符”留给下一轮处理。
4. 行末仍未闭合时，将左标记与缓冲内容原样写回，避免因破损文本而丢数据。每行后追加调用者给定的 `sep`，完成后显式 `flush`。

`TaskInfoRedacted::String` 先处理 `None -> "nil"`，否则克隆整个 task info。如果有 storage，再克隆 backend：S3 替换 `access_key`、`secret_access_key`、`sse_kms_key_id`；GCS 替换 `credentials_blob`；Azure 替换 `shared_key`、`access_sig` 并用新 `AzureCustomerKey` 替换 encryption key。未列出的 backend 保持不变，最后只格式化副本。

## 数据与状态

唯一的共享可变状态是 `REDACT_LOG_ENABLED`。`OnceLock` 保证锁对象只初始化一次，`RwLock<String>` 容许多读单写。`InitRedact(bool)` 只能存入 `ON` 或 `OFF`；`NeedRedact` 却保留 Go 的更广判定契约：当前模式既不是 `OFF` 也不是空串时都视为需要脱敏。

`DeRedact` 的解析状态限于当前行，不跨行保留 `start` 或缓冲区；因此一个标记区间不能跨越换行被识别为同一区间。`TaskInfoRedacted` 保存的是带生命周期的不可变借用；展示过程只修改克隆副本，原始任务信息不变。

## 依赖与调用关系

- 向下，标准库提供文件、缓冲 I/O、路径、格式化和同步原语；`path_clean::PathClean` 对齐 Go `filepath.Clean`；`kvproto::brpb` 提供备份任务与存储后端 protobuf 类型。
- 文件内部，`Stringer` 构造 `redactStringer`，后者调用 `String`；`DeRedactFile` 始终委托 `DeRedact`；`Value` 和 `Key` 都委托 `NeedRedact`；`InitRedact` 和 `NeedRedact` 共享 `redact_log_enabled`。
- 向上，`pkg/session/nontransactional.rs::redact_sql` 在生成非事务 SQL 相关文本时调用 `String`；`pkg/session/runtime/explain_select.rs` 从 session 状态取出 `redact_log`，通过 `WriteRedact` 渲染谓词字面量。`pkg/planner/core/tests/redact/redact_test.rs` 也使用 `String` / `WriteRedact` 验证计划展示。
- `pkg/util/redact/lib.rs` 对外重导出全部符号，并仅在测试构建中声明 `REDACT_TEST_LOCK`及两个独立测试模块。

RustCodeGraph 将目标文件标记为被 6 个文件使用，但对精确符号执行 `callers` / `callees` 未返回边；因此上述业务调用关系只列出了可由精确文本检索和相邻源码直接复核的路径，没有将全仓同名函数误认为本 crate 调用者。

## 错误处理与边界

`DeRedactFile` 通过 `?` 传播路径、打开、创建、写入和 flush 错误，返回 `Box<dyn Error + Send + Sync>`。文件句柄依赖 RAII 自动关闭。`DeRedact` 遇到标记区间内行末单独的 `‹` 时返回 `UnexpectedEof`；而一般“已开始但没有右标记”的区间会被原样写回。这两种边界不应混淆。

`BufRead::lines` 移除行终止符，代码再统一写入 `sep`。与 Go `bufio.Scanner` 相比，Rust 实现没有 Scanner 的默认 token 大小上限；这是已识别的实现差异，不应在没有兼容性评估时随意改动。`WriteRedact` 对未知模式选择原样写入；`String` 对未知模式则是 debug 断言加空串回退，两者的非法输入契约不同。

`TaskInfoRedacted` 只明确遮盖当前列出的 S3/GCS/Azure 字段；其他 backend 或未来新增凭证字段会落入不修改分支，因此 protobuf schema 演进时必须同步审查这个 match。`String` 和 `WriteRedact` 都不返回错误，调用者必须传入约定的模式字符串。

## 并发与资源生命周期

全局模式可以由多线程并发读取，写入由 `RwLock` 串行化。代码使用 `expect` 处理 lock poisoning，所以持锁期间如果有线程 panic，后续 `InitRedact` 或 `NeedRedact` 会 panic，而不是返回可恢复错误。这与 Go 原子存储的失败模型不同。

`DeRedactFile` 中的输入/输出文件、stdout lock、`BufReader` 和 `BufWriter` 都是函数局部资源。`DeRedact` 在成功返回前显式 flush；早退错误时 `BufWriter` 的 Drop 不保证可报告最终 flush 错误，因此调用者只能依赖函数已返回的首个错误。`Stringer` 与 `TaskInfoRedacted` 都用借用生命周期限制底层对象在格式化期间存活；没有后台任务、通道、异步 runtime 或长生事务。

独立测试使用 `pkg/util/redact/lib.rs::REDACT_TEST_LOCK` 串行化会修改全局脱敏状态或进程 umask 的用例，并在用例末尾恢复这些进程级状态。

## 与 Go 版本的对应关系

Rust 文件以 `pkg/util/redact/redact.go` 为直接对照，`Cargo.toml` 的 `package.metadata.porting.go-package` 也指向 `pkg/util/redact`。主要对应关系如下：

- Go `String` / `redactStringer` / `Stringer` 对应 Rust 同名 API 与 `FmtStringer`；三模式、Unicode rune/char 遍历和标记加倍语义一致。
- Go `DeRedactFile` / `DeRedact` 对应 Rust 同名函数；路径清理、`-` 表示 stdout、输出文件截断与 Unix `0644`、标记转义、未闭合文本保留、每行分隔符语义一致。实现级差异是 Rust `lines` 没有 Go Scanner 的 token 上限。
- Go 的 `errors.RedactLogEnabled` 原子容器被 Rust `OnceLock<RwLock<String>>` 取代；可观察的 `InitRedact` / `NeedRedact` / `Value` / `Key` 常规路径保持一致，但 Rust 额外有锁中毒 panic 边界。
- Go `WriteRedact` 与 Rust 实现的三分支相同：`MARKER` 包裹，`ON` 写问号，其他原样输出。
- Go `TaskInfoRedacted.String` 使用浅层结构复制再逐后端复制；Rust 使用 protobuf `clone` 得到深副本。两者都遮盖 S3/GCS/Azure 的同组凭证，不改原对象，并生成紧凑 protobuf 文本。

`pkg/util/redact/redact_test.go` 是基础对照测试。Rust `redact_test.rs` 覆盖同样的模式、反脱敏和全局开关，并增加输出文件 `0644` 验证；`migration_aster_unit_test.rs` 补充 Unicode、转义标记、自定义行分隔符、大写 key 编码以及三种云存储凭证的不泄露/不修改原对象契约。

## 扩展指南

- 新增或改变脱敏模式时，必须联合审查 `String`、`WriteRedact`、`InitRedact` / `NeedRedact` 以及所有从 session 传入模式的调用点。特别注意 `String(ON)` 返回空串，而 `WriteRedact(ON)` 写入 `?`，这是不同场景的现有契约。
- 修改标记编码时，必须同时验证 `String` 的转义与 `DeRedact` 的逆向状态机，包括空区间、重复标记、非法嵌套、行末单独左标记、未闭合区间、Unicode 和多行分隔。
- 新增 BR 存储后端或 protobuf 凭证字段时，必须扩展 `TaskInfoRedacted::String` 的 match，且同时断言敏感值不出现、非敏感定位字段仍保留、原 protobuf 对象未变。
- 全局开关如果需要支持更多状态或可恢复的锁错误，应先与 Go `errors.RedactLogEnabled` 的语义对齐，再调整 `REDACT_LOG_ENABLED`；不要仅为了 Rust 实现便利而改变对外观察结果。
- 生产逻辑必须继续放在 `redact.rs`，测试修改同步放在独立的 `redact_test.rs` 或 `migration_aster_unit_test.rs`，不将 `#[cfg(test)]` 测试逻辑内嵌到生产文件。Go 可观察行为变更时还需同步核对 `redact.go` / `redact_test.go`。
- 性能上，`String(MARKER)` 是线性遍历，`DeRedact` 会为每行创建 `Vec<char>`，`Key` 逐字节使用 `format!`，`TaskInfoRedacted` 会克隆整个 protobuf。优化这些路径时应保留 Unicode、错误边界、不修改原对象和输出格式等契约。

## 验证依据

- 源码：`pkg/util/redact/redact.rs`，RustCodeGraph `node --file ... --offset 1 --limit 500` 返回完整 368 行与 29 个符号。
- crate 边界：`pkg/util/redact/Cargo.toml` 和 `pkg/util/redact/lib.rs`；前者确认 crate 名、入口、`kvproto` / `path-clean` 依赖与 Go package 映射，后者确认重导出和独立测试接线。
- Go 对照：`pkg/util/redact/redact.go` 与 `pkg/util/redact/redact_test.go`。
- Rust 测试：`pkg/util/redact/redact_test.rs` 和 `pkg/util/redact/migration_aster_unit_test.rs`；后者明确覆盖 S3/GCS/Azure 遮盖与原对象不变。
- 直接上游证据：`pkg/session/nontransactional.rs::redact_sql`、`pkg/session/runtime/explain_select.rs` 字面量闭包，以及 `pkg/planner/core/tests/redact/redact_test.rs`。
- RustCodeGraph 查询：`status`、`files --filter pkg/util/redact`、`explore 'pkg/util/redact/redact.rs ...'`、完整 `node --file`、`query` 主要符号，以及精确 `callers` / `callees`。最后一组未返回边，因此使用 `rg` 精确检索补足并限定上游结论。
- 人工复核结论：文档覆盖了文件存在原因、三类执行流程、共享状态与 I/O 生命周期、Go 对齐点、已知差异、扩展接入点及应同步的独立测试。本任务是纯文档分析，按计划不运行 Cargo。
