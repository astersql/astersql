# 任务 7: 修复 session 测试接线漂移

批次：【批次 1】 无

状态：已完成，待回归验证

目的：修复 session 相关测试因函数签名、集成测试 crate 路径和缺失 dev-dependency 造成的三个编译断点。

来源任务：无

预计会话范围：三个问题都属于 session 测试接线且不改生产行为：merge helper 参数顺序、外部集成测试的 crate 路径、variable 测试的 serde_json dev-dependency。

## 文件

- 修改：`pkg/session/runtime/modify_column_cloud_store_test.rs`
- 修改：`pkg/session/tests/system_session.rs`
- 修改：`pkg/session/test/variable/Cargo.toml`
- 测试：上述两个测试文件与 `pkg/session/test/variable/variable_test.rs`

## 上下文

- merge helper 从 13 个参数变为 12 个；`pkg/session/tests/system_session.rs` 是独立 integration test，不能使用 `crate::runtime`；variable_test 直接解析 JSON 但其 crate 未声明 serde_json。

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

## 测试计划

- 行为：三个既有 session 测试目标均编译并执行原断言，不删除参数语义、系统会话行为或 JSON 字段验证。
- 失败验证测试：使用上述目标的现有测试/编译目标；若涉及行为分支，先补聚焦回归。
- 失败验证命令：``cargo check -p astersql-session -p astersql-session-test-variable --all-targets --locked``
- 预期失败原因：分别出现 E0061、integration test 根无 runtime 的 E0433，以及 serde_json 未声明的 E0433。
- 通过验证命令：`cargo test -p astersql-session --test system_session --locked -- --nocapture && cargo test -p astersql-session-test-variable --locked -- --nocapture && cargo test -p astersql-session --lib modify_column_cloud_store --locked -- --nocapture`
- 模拟策略：沿用真实 session runtime、object store 测试夹具与 JSON 解析，不替换现有行为测试。

## 步骤

1. 分别记录三个目标的失败；核对 merge 函数当前签名和相邻调用。
2. 按当前 API 修正参数顺序/数量，将 integration test 调用改用已导入的外部 crate API，并添加 serde_json dev-dependency。
3. 运行 fmt、三个聚焦测试和双 package 全 targets check。

## 验证

- 运行：`cargo test -p astersql-session --test system_session --locked -- --nocapture && cargo test -p astersql-session-test-variable --locked -- --nocapture && cargo test -p astersql-session --lib modify_column_cloud_store --locked -- --nocapture`
- 预期：三个编译错误组消失，聚焦测试均有非零执行数量并通过。
- 所需证据：每个错误的前后输出、测试数、退出码、Cargo manifest 差异与实际槽位。

## 完成

不得删除 failing assertion 或把测试移回源文件；Cargo manifest 无本地 patch。完成后使用 `$git-commit` 仅提交本任务变更。

## 实施记录

- 修正 `modify_column_cloud_store_test.rs` 对 `merge_overlapping_files_internal` 的旧 13 参数调用，保留当前 API 的 prefix、writer ID、block size 与重复键语义。
- 将独立 integration test 中的 `crate::runtime::CreateAnalyzeSession` 改为已导入的外部 crate API `CreateAnalyzeSession`。
- 为 `astersql-session-test-variable` 增加直接 `serde_json` dev-dependency，并同步 `Cargo.lock`；未增加本地 patch。
- 失败基线：槽位 5，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-5`，`cargo check -p astersql-session -p astersql-session-test-variable --all-targets --locked` 退出码 101，确认 E0061 与 integration test E0433。
- 格式化：槽位 3，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-3`，`cargo fmt --all` 退出码 0。
- 聚焦验证：槽位 3，`cargo test -p astersql-session --test system_session --locked -- --nocapture` 退出码 0，9 passed；`cargo test -p astersql-session-test-variable --locked -- --nocapture` 退出码 101，11 个测试中 9 passed、2 failed。
- variable crate 的两个失败是本任务接线以外的既有行为断言：`correct_scope_error_registers_and_unregisters_sysvars` 的 GLOBAL 变量错误文案差异，以及 `last_query_info_exposes_finalized_ru_v2_consumption` 的 `ru_v2_consumption` 仍为 0.0。未删除或放宽断言，留待回归修复。
- 剩余验证：槽位 2，`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`，`cargo test -p astersql-session --lib modify_column_cloud_store --locked -- --nocapture` 退出码 0，2 passed；`cargo check -p astersql-session -p astersql-session-test-variable --all-targets --locked` 退出码 0。
- Ready 门禁：`make lint` 退出码 0；该目标不调用 Cargo，未占用 Rust 槽位。
- 所有本会话槽位锁均已释放，构建缓存保留。
