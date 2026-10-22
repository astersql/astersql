# `tools/tazel/util.rs`

## 文件定位

`skipFlaky`、`skipTazel`、`skipShardCount` 集中保存 Go 工具既有的例外路径，供主流程决定是否设置 `flaky`、是否处理整个 BUILD 文件、是否更新 `shard_count`。

## 核心职责

本文件有两组职责，均直接镜像 [`tools/tazel/util.go`](util.go)：

1. `write` 将已经修改的 `build::File` 交给 BUILD 兼容层重写、格式化，再覆盖写回指定路径。
2. `skipFlaky`、`skipTazel`、`skipShardCount` 集中保存 Go 工具既有的例外路径，供主流程决定是否设置 `flaky`、是否处理整个 BUILD 文件、是否更新 `shard_count`。

这些函数本身不决定 BUILD 属性的具体值。`timeout = "short"`、`flaky = True`、分片数上限和删除旧分片值等策略都在 `main.rs::patch_go_test_file` 中；本文件只提供写回动作和布尔判定。

## 主要符号

- `pub fn write(path: &str, f: &mut build::File) -> io::Result<()>`：先调用 `build::Rewrite(f)`，再调用 `build::Format(f)` 得到字节，最后用 `OpenOptions` 的 `create + write + truncate` 打开并写入文件。Unix 新建文件请求 `0644` 模式；返回打开或写入阶段的 `io::Error`。
- `pub fn skipFlaky(path: &str) -> bool`：仅精确匹配 `tests/realtikvtest/addindextest/BUILD.bazel`。命中时，`main.rs::patch_go_test_file` 不自动补 `flaky = True`。
- `pub fn skipTazel(path: &str) -> bool`：仅精确匹配 `build/BUILD.bazel`。命中时，`main.rs::walk_build_files` 跳过该文件，不读取、解析或写回。
- `pub fn skipShardCount(path: &str) -> bool`：跳过所有以 `tests/readonlytest` 开头的路径；也跳过以 `pkg/util` 开头的路径，但 `pkg/util/admin`、`pkg/util/chunk`、`pkg/util/topsql`、`pkg/util/stmtsummary`、`pkg/util/workloadrepo` 五个前缀例外不跳过。

本文件没有自定义类型、trait、模块级可变状态或条件编译函数；唯一条件编译项是 Unix 平台下导入 `OpenOptionsExt` 并设置创建模式。

## 执行流程

一次完整处理中的调用顺序如下：

1. `main.rs::walk_build_files` 递归发现名为 `BUILD.bazel` 的文件，并把仓库相对路径交给 `skipTazel`；命中即继续遍历，不处理该文件。
2. 未跳过的文件被读取并传入 `main.rs::patch_go_test_file`。该函数解析首个顶层 `go_test` 规则。
3. 当规则尚无 `flaky` 属性时，`patch_go_test_file` 先询问 `skipFlaky`；只有返回 `false` 才添加 `flaky = True`。
4. `patch_go_test_file` 先询问 `skipShardCount`；只有返回 `false` 才根据 `ast.rs` 预先统计的目录测试数更新或删除 `shard_count`。
5. `walk_build_files` 将修改后的 `build::File` 交给 `write`。`write` 调用 `Rewrite`、`Format`，随后截断并覆盖原 BUILD 文件。

当前 `stubs.rs::Rewrite` 是保持 buildtools API 形状的空操作；`stubs.rs::Format` 才会把变更后的属性应用到原始文本，并尽量保留无关字节。因此文档不能把当前 Rust `Rewrite` 描述成独立的规范化算法。

## 数据与状态

三个跳过函数都只依赖传入的 `&str`，没有全局状态。`skipFlaky` 和 `skipTazel` 每次调用都会新建一个局部 `HashSet<&str>`、插入唯一名单项并执行精确查找；集合在函数返回时销毁。`skipShardCount` 不分配集合，只按固定顺序执行字符串前缀判断。

路径契约是仓库相对、使用 `/` 分隔的字符串。生产调用方 `main.rs::walk_path_string` 会尝试相对 `root` 取路径并去掉 `./` 前缀，再调用这些判定。判定函数本身不做路径规范化，也不解析 `..`、符号链接或平台分隔符。

`write` 的输入状态是可变的 `build::File`。虽然当前 `Rewrite` 不修改它，签名仍保留可变引用以匹配 Go buildtools 的调用顺序，并允许兼容层以后实现重写。格式化结果 `Vec<u8>` 在本函数作用域内持有，写完后释放。

## 依赖与调用关系

上游直接调用关系经 RustCodeGraph 文件索引与源码核对如下：

- `main.rs::patch_go_test_file -> skipFlaky`
- `main.rs::patch_go_test_file -> skipShardCount`
- `main.rs::walk_build_files -> skipTazel`
- `main.rs::walk_build_files -> write`
- `parity_test.rs` 直接调用全部四个函数，验证公开迁移契约；其中多个 BUILD 保真场景直接调用 `write`。

下游关系为：

- `write -> stubs::build::Rewrite -> stubs::build::Format -> std::fs::OpenOptions::open -> Write::write_all`
- `skipFlaky`、`skipTazel -> std::collections::HashSet`
- `skipShardCount -> str::starts_with`

RustCodeGraph 将 `util.rs` 识别为被 `ast_test.rs` 和 `parity_test.rs` 使用；精确符号引用搜索表明真正直接调用四个函数的是 `main.rs` 与 `parity_test.rs`。`ast_test.rs` 通过同一 crate 测试模块装配被关联，但不直接调用本文件函数。

## 错误处理与边界

`write` 只返回文件打开和 `write_all` 的 I/O 错误，使用 `?` 原样向上传播；调用方 `walk_build_files` 再把错误交给 `run_from`。`build::Rewrite` 和 `build::Format` 当前签名不返回错误，所以此处没有格式化错误分支。解析错误更早发生在 `patch_go_test_file` 调用 `ParseBuild` 时，不属于本文件。

写回不是原子替换：文件以 `truncate(true)` 打开后再写入，若写入中途失败，磁盘文件可能已被截断或只含部分内容。代码也没有显式 `flush`、`sync_all`、备份或回滚。Unix 的 `mode(0o644)` 只影响新建文件；覆盖已存在文件时不会主动重设其权限。非 Unix 平台不设置显式模式。

跳过规则的边界必须按当前字符串语义理解：`skipFlaky` 和 `skipTazel` 是完整字符串精确匹配；`skipShardCount` 是无路径段边界检查的前缀匹配，因此例如 `tests/readonlytest-extra` 或 `pkg/utility` 也会满足对应 `starts_with` 条件。生产调用方虽提供规范化的相对路径，但独立调用者必须自行遵守同一契约。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。三个判定函数仅操作调用栈上的局部数据，因此函数之间没有共享可变状态。

`write` 在函数内独占一个 `File` 句柄，`write_all` 返回后由 RAII 关闭。它没有文件锁；如果多个进程或线程同时写同一路径，最后结果取决于竞争顺序，且可能发生交错或截断风险。正常主流程 `walk_build_files` 是同步递归、逐文件调用，因此单次 `tazel` 执行内部不会并发调用它。`parity_test.rs::contract_resource_cleanup` 在完整运行后能够删除临时目录，间接验证正常成功路径没有遗留打开的文件句柄。

## 扩展指南

- 新增或删除跳过路径时，应先确认它属于 `flaky`、整个 tazel 处理还是 `shard_count` 三类中的哪一类，只修改对应函数；同时同步 `util.go`，除非任务明确允许 Rust/Go 产生差异。
- 精确名单增长时可继续使用集合；若改为模块级静态数据，应评估是否确有必要，避免为少量字符串引入初始化或同步复杂度。前缀规则变更时要明确是否要求路径段边界，不能无意改变现有 `starts_with` 语义。
- 路径输入格式若改变，应优先在 `main.rs::walk_path_string` 统一规范化，不要让三个判定函数各自形成不同规则。
- 若要增强写回可靠性，修改入口是 `write`；采用临时文件、权限继承、`sync_all` 或 rename 会改变失败语义和跨平台行为，必须与 Go 版本及调用方预期一并评审。
- 测试逻辑应继续放在独立的 [`tools/tazel/parity_test.rs`](parity_test.rs)，不要内嵌进 `util.rs`。名单变更至少增加命中与未命中断言；写回变更应覆盖原内容保留、错误路径、权限/清理行为和必要的平台差异。

兼容性风险主要来自路径匹配范围和写回语义；性能风险较低，但三个过滤函数会对遍历到的 BUILD 文件反复调用，若名单显著增大，应避免每次重建大型集合。正确性上尤其要防止扩大 `skipTazel` 导致文件完全漏处理，或缩小 `skipShardCount` 导致不适合分片的包被自动改写。
