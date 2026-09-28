# 生成 Rust UCA 权重表

本文介绍如何从仓库内置的 Unicode Collation Algorithm（UCA）`allkeys` 数据重新生成 Rust 校对权重表，并验证生成结果可重复、可编译且与现有 Go 数据语义一致。

## 生成内容

生成器位于 `pkg/util/collate/ucadata/generator`，输入数据已通过 `include_str!` 内嵌，无需联网下载：

- Unicode 4.0.0：`allkeys-4.0.0.txt`
- Unicode 9.0.0：`allkeys-9.0.0.txt`

生成结果为：

- `pkg/util/collate/ucadata/unicode_ci_data_generated.rs`
- `pkg/util/collate/ucadata/unicode_0900_ai_ci_data_generated.rs`

生成器根据输出文件名选择 Unicode 版本和 Go/Rust 渲染后端。输出目录可以改变，但文件名必须是生成器支持的四个名称之一；未知文件名会返回非零退出状态。

## 前置条件

在仓库根目录执行以下命令，并确保 Rust 工具链包含 `rustfmt`：

```bash
rustc --version
cargo --version
rustfmt --version
```

如果没有安装 `rustfmt`，可执行：

```bash
rustup component add rustfmt
```

本仓库使用 Rust 2024 edition，生成器会显式以 2024 edition 调用 `rustfmt`。

## 生成两张 Rust 表

从仓库根目录运行：

```bash
cargo run -p astersql-util-collate-ucadata-generator \
  --bin ucadata-generator -- \
  pkg/util/collate/ucadata/unicode_ci_data_generated.rs

cargo run -p astersql-util-collate-ucadata-generator \
  --bin ucadata-generator -- \
  pkg/util/collate/ucadata/unicode_0900_ai_ci_data_generated.rs
```

命令会读取内嵌 `allkeys`、补齐隐式权重、渲染完整静态表，再通过 `rustfmt` 格式化并写入给定路径。生成文件较大，提交前应确认差异只包含预期的结构、注释或权重数据变化。

## 验证重复生成字节一致

先保存第一次生成结果，再运行第二次生成并用 `cmp` 比较：

```bash
snapshot_dir="$(mktemp -d /tmp/astersql-uca.XXXXXX)"

cp pkg/util/collate/ucadata/unicode_ci_data_generated.rs \
  "$snapshot_dir/unicode_ci_data_generated.rs"
cp pkg/util/collate/ucadata/unicode_0900_ai_ci_data_generated.rs \
  "$snapshot_dir/unicode_0900_ai_ci_data_generated.rs"

cargo run -p astersql-util-collate-ucadata-generator \
  --bin ucadata-generator -- \
  pkg/util/collate/ucadata/unicode_ci_data_generated.rs
cargo run -p astersql-util-collate-ucadata-generator \
  --bin ucadata-generator -- \
  pkg/util/collate/ucadata/unicode_0900_ai_ci_data_generated.rs

cmp "$snapshot_dir/unicode_ci_data_generated.rs" \
  pkg/util/collate/ucadata/unicode_ci_data_generated.rs
cmp "$snapshot_dir/unicode_0900_ai_ci_data_generated.rs" \
  pkg/util/collate/ucadata/unicode_0900_ai_ci_data_generated.rs
```

两个 `cmp` 都不输出内容且退出码为 `0`，表示重复生成字节一致。

## 运行回归测试

```bash
cargo test -p astersql-util-collate-ucadata-generator
cargo test -p astersql-util-collate-ucadata
cargo test -p astersql-util-collate
```

这些测试覆盖生成器目标选择与错误行为、两张表的长度和代表值、长权重布局，以及 collate 运行时的聚焦回归。

最后检查工作区：

```bash
git diff --check
git diff --stat -- pkg/util/collate/ucadata
```

## 常见问题

### `no bin target named ucadata-generator`

确认当前分支的 `pkg/util/collate/ucadata/generator/Cargo.toml` 包含名为 `ucadata-generator` 的 binary target。

### `unsupported ucadata output target`

生成器只按文件名识别以下目标：

- `unicode_ci_data_generated.go`
- `unicode_0900_ai_ci_data_generated.go`
- `unicode_ci_data_generated.rs`
- `unicode_0900_ai_ci_data_generated.rs`

请保留目标文件名，仅按需改变其目录。

### 无法启动 `rustfmt`

运行 `rustup component add rustfmt`，并确认 `rustfmt --version` 可正常执行。
