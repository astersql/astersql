# 任务 1: 删除 Rust nogo 镜像

批次：【批次 1】 无

状态：已完成

目的：删除 Rust nogo/linter 镜像及所有专用接线、依赖和文档索引，保持 Go/Bazel nogo 不变。

来源任务：用户直接要求删除无生产意义的 Rust 静态检查移植。

预计会话范围：所有目标文件属于单一 `build` 静态检查边界，验证集中在根 crate、引用完整性和仓库 Ready 检查。

## 文件

- 删除：`build/**/*.rs`、`build/**/*.rs.md` 中属于 config/debug-linter/linter 的文件
- 修改：`pkg/lib.rs`
- 修改：`Cargo.toml`
- 修改：`docs/rust-architecture/README.md`
- 修改：`docs/rust-architecture/architecture.md`
- 修改：`docs/rs-docs-manifest.md`

## 上下文

- RustCodeGraph 显示 Rust analyzer 没有生产调用链；根 crate 主要通过测试模块引用这些文件。
- Go/Bazel 的实际 nogo 文件必须保留。

## Cargo 共享槽位规则（仅 Rust/Cargo 任务）

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得为任务、会话或计划另建构建目录，也不得按任务编号取模固定分配。
2. 每次构建/验证阶段开始前，创建仓库根目录下的 `target/rust-slot-locks`；依次用原子命令 `mkdir target/rust-slot-locks/slot-N.lock` 尝试领取槽位，`N` 为 1–10。仅 `mkdir` 成功者获得该槽位；这些相对路径命令从仓库根目录执行。
3. 领取成功后，在自己的锁目录中记录计划、任务、会话标识和持有进程 PID。同一槽位同时只允许一个持有者；已占用时尝试下一个，仅全部 10 个槽位都占用时等待并重试。
4. 将 `CARGO_TARGET_DIR` 设置为领取槽位的绝对路径，例如领取槽位 3 后，从仓库根目录执行 `export CARGO_TARGET_DIR="$PWD/target/rust-slot-3"`。示例编号不代表固定分配。
5. 所有 Cargo 构建、检查、测试以及调用 Cargo 的脚本和子进程均继承该设置，不得通过 `--target-dir` 或其他配置绕过槽位；生成的二进制也从该槽位定位。
6. 该阶段的 Cargo、脚本以及使用槽位产物的子进程全部结束后，删除自己写入的占用记录，并用 `rmdir` 释放自己的锁目录；保留槽位编译缓存供后续任务复用。
7. 正常结束、失败和中断均须清理自己的锁；在后台进程或其他使用槽位产物的进程仍运行时不得提前释放。规划清理步骤时覆盖这三种退出路径。
8. 遗留锁须先核实持有会话及相关进程已结束；无法确认则保持占用，不得删除其他活跃任务的锁、进程或缓存。
9. 生成或更新计划时，统一移除与这些规则冲突的单一 Rust 共享构建空间、逐任务独立目录和 Cargo 缓存全局独占窗口要求；仍须遵守源码文件占用、真实业务依赖和其他共享验证资源的互斥，不能仅因缓存分槽就把任务判定为可并行。
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能替代行为验收证据。

## 验证计划

- 删除前固定 Rust 文件与外部引用清单。
- 删除后用 `rg` 确认不存在指向已删 `build/**/*.rs` 文件的接线和 Markdown 链接。
- 运行 `cargo fmt --all`、根 crate 聚焦测试与仓库 Ready lint。

## 步骤

1. 删除目标 Rust 源码、测试、fixture 和说明文件。
2. 清理 `pkg/lib.rs`、`Cargo.toml` 与 Rust 文档索引。
3. 搜索悬空引用并修正。
4. 执行格式化、测试和 lint，自审差异。

## 验证

- 运行：`cargo fmt --all`
- 运行：`cargo test -p astersql --lib`
- 运行：`make lint`
- 预期：命令退出码为 0，根 crate 测试仍有有效测试执行，Go/Bazel nogo 文件保持未修改。
- 所需证据：删除文件数、零残留引用、测试数量、命令退出码和差异审查结果。

## 完成

只有在 Rust nogo 镜像完整删除、Go/Bazel 文件未变且验证证据齐全后，才将状态标记为已完成。
