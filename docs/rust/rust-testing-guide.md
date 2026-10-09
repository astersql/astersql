# Rust 编译与测试指南

以下命令均在 AsterSQL 仓库根目录执行。

## 全量测试

先安装 `cargo-nextest`：

```bash
cargo install cargo-nextest --locked
```

测试 Cargo workspace 中全部包的非文档测试 target（包括单元测试、集成测试、examples 和 benches）：

```bash
make rust-test
```

该命令使用 `cargo-nextest` 并行调度各个 crate 和测试 target。默认并发数等于
逻辑 CPU 数；测试运行 5 秒后会被标记为慢，运行满 10 秒后会被终止并判为失败。
超时策略统一作用于整个 workspace，不为已知慢测设置例外。
测试输出会实时显示在终端，并同时保存到仓库的 `./target` 目录。命令开始和
结束时都会打印实际日志文件路径，例如 `./target/rust-test.AbCd12`。

Doc-tests 单独运行，也可通过 `PACKAGE=<package-name>` 指定单包：

```bash
make rust-doc-test
```

只运行全部包的库单元测试：

```bash
make rust-unit-test
```

## 单包测试

通过 Cargo package 名称选择一个包：

```bash
make rust-test PACKAGE=astersql-types
```

查找目录对应的 package 名称：

```bash
cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[] | "\(.name)\t\(.manifest_path)"' \
  | rg 'pkg/types'
```

也可以直接进入包目录运行 Cargo：

```bash
cd pkg/types
cargo test --locked
```

## 单个测试

`RUST_TEST_ARGS` 会原样传给 `cargo nextest run`。测试名称默认采用子字符串匹配：

```bash
make rust-test \
  PACKAGE=astersql-types \
  RUST_TEST_ARGS='test_name'
```

精确执行一个测试并立即显示它的标准输出：

```bash
make rust-test \
  PACKAGE=astersql-types \
  RUST_TEST_ARGS='-E "test(=module::tests::test_name)" --no-capture'
```

先列出包中的测试，以获得完整测试名称：

```bash
make rust-test \
  PACKAGE=astersql-types \
  RUST_TEST_ARGS='-- --list'
```

## 单个测试 target

只运行 `tests/<name>.rs` 对应的集成测试 target：

```bash
make rust-test \
  PACKAGE=<package-name> \
  RUST_TEST_TARGETS='--test <target-name>'
```

只运行包的库单元测试：

```bash
make rust-test \
  PACKAGE=astersql-types \
  RUST_TEST_TARGETS='--lib'
```

## 仅编译

只编译单包的测试程序，不执行测试：

```bash
make rust-test \
  PACKAGE=astersql-types \
  RUST_TEST_ARGS='--no-run'
```

只检查单包能否通过编译，不生成测试程序：

```bash
env -u LDFLAGS cargo check --locked --package astersql-types
```

## 常用附加参数

```bash
# 立即显示测试输出
RUST_TEST_ARGS='test_name --no-capture'

# 精确匹配测试名称
RUST_TEST_ARGS='-E "test(=module::tests::test_name)"'

# 控制 nextest 的测试并发数（只影响执行阶段，不会减少编译量）
RUST_TEST_ARGS='--test-threads=4'

# 选择目标类型（默认 --all-targets，不含 Doc-tests）
RUST_TEST_TARGETS='--lib'
```

若出现 `Blocking waiting for file lock on artifact directory`，表示另一个 Cargo
进程正在使用同一个 `target` 目录。等待该进程结束，或停止重复启动的构建任务后重试。
