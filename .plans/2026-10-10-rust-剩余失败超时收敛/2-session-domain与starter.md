# 任务 2: 收敛 session Domain 与 Starter 超时

批次：【批次 2】 依赖批次 1

状态：未开始

目的：修复两个 session 初始化类超时，不删减大用户表、密码历史和 Domain claim/warning 断言。

来源任务：`starter_privilege_reset_batches_large_user_table_and_preserves_password_history`、`global_variable_init_domain_skips_claim_and_serving_domain_warns_once`。

预计会话范围：同属 `astersql-session` 的 Domain/bootstrap 初始化路径，可在一个聚焦会话比较 setup、主体和 teardown 成本。

## 文件

- 修改：`pkg/session/starter_bootstrap_file.rs` 或任务 1 证明的拥有路径
- 修改：`pkg/session/tidb.rs` 或任务 1 证明的拥有路径
- 测试：`pkg/session/starter_bootstrap_file_test.rs`
- 测试：`pkg/session/tidb_test.rs`

## 上下文

- RustCodeGraph 显示两测试分别经过 Starter privilege reset 和 `NewDomainWithEtcdClient`/`Init`/`Close`；必须先分段计时，不允许只提高超时。

## Cargo 共享槽位规则（仅 Rust/Cargo 任务）

1. 共用 `target/rust-slot-1` 至 `target/rust-slot-10`，不得另建 Cargo 目录。 2. 用原子 `mkdir target/rust-slot-locks/slot-N.lock` 领取槽位。 3. 锁内记录计划、任务、会话和 PID。 4. 设置绝对 `CARGO_TARGET_DIR="$PWD/target/rust-slot-N"`。 5. 所有 Cargo 子进程继承该变量。 6. 阶段结束删除自己的记录并 `rmdir`，保留缓存。 7. 正常、失败、中断均清理。 8. 不删除无法确认的活跃锁。 9. 槽位不解除其他资源互斥。 10. 最终记录槽位、路径、命令、退出码和测试数；代码修改后先 `cargo fmt --all`。

### 完整执行契约

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

- 行为：原规模 privilege batching 与 Domain warning/claim 语义不变，两个测试默认预算内通过。
- 失败验证命令：`cargo nextest run --locked --package astersql-session -E 'test(=starter_bootstrap_file_test::starter_privilege_reset_batches_large_user_table_and_preserves_password_history) | test(=tidb_test::global_variable_init_domain_skips_claim_and_serving_domain_warns_once)' --no-capture`
- 预期失败原因：任务 1 确认的重复 bootstrap、固定等待或 teardown 热点。
- 通过验证命令：同上；再运行两个文件所属的窄测试集合。
- 模拟策略：使用现有真实内存 store/Domain 和测试 etcd 边界。

## 步骤

1. 添加能固定重复初始化或等待热点的回归断言。
2. 证明修复前超时，采样最热调用路径。
3. 只优化共享初始化、批处理或关闭路径，保留所有行为断言。
4. `cargo fmt --all` 后运行精确测试和 `make lint`。

## 验证

- 预期：2 个有效测试退出码 0，各自小于默认 10 秒。
- 所需证据：前后耗时、采样热点消失、测试规模未缩减、diff 自审。

## 完成

记录实际修改的 session 符号与测试证据，标记完成后用 `$git-commit` 独立提交。
