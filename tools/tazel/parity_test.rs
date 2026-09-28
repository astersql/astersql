// Copyright 2026 AsterSQL.

//! Parity tests for `tools/tazel` vs Go `main.go` / `ast.go` / `util.go`.
//!
//! 这组测试不是覆盖某一个小函数，而是把 Rust 迁移版当成一个对外契约来校验：
//! 只要这些断言仍成立，就说明 `tazel` 对 BUILD 文件的补丁策略、测试数量统计、
//! 跳过名单以及资源清理行为，仍与 Go 工具维持同一语义。
//!
//! 文件内故意把断言拆成四个子场景：
//! - 正常路径：验证常见仓库布局下会补哪些属性；
//! - 边界路径：验证已有属性、单测试目录和跳过名单等特殊条件；
//! - 错误路径：验证入口前置条件和解析桩的失败/成功边界；
//! - 资源清理：验证全局计数表重置和临时目录回收。
//!
//! 这样组织的目的，是让测试结构继续贴近 Go 版公共行为，而不是绑定 Rust
//! 迁移过程中的内部实现细节。

use std::fs;
use std::path::PathBuf;

use crate::ast::{addTestMap, initCount, scan, test_count_for};
use crate::main::{maxShardCount, patch_go_test_file, run_from};
use crate::stubs::build;
use crate::util::{skipFlaky, skipShardCount, skipTazel, write};

#[test]
fn go_rust_public_contract_matches() {
    // 顶层测试只负责串联四类场景，便于在单个失败点出现时仍能从函数名看出
    // “哪一类 Go 对齐契约”被破坏。
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

#[test]
fn scan_uses_go_syntax_instead_of_matching_comment_text() {
    initCount();
    let dir = tmpdir("scan-syntax");
    let source = dir.join("syntax_test.go");
    fs::write(
        &source,
        br#"package syntax
/*
func TestCommentOnly(t *testing.T) {}
func TestAnotherCommentOnly(t *testing.T) {}
*/
func /* an allowed comment */ TestReal(t *testing.T) {}
"#,
    )
    .unwrap();

    scan(source.to_str().unwrap()).unwrap();
    let abs_dir = fs::canonicalize(&dir).unwrap();
    assert_eq!(test_count_for(&abs_dir.to_string_lossy()), Some(1));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_edit_preserves_unrelated_syntax_and_rejects_invalid_input() {
    let dir = tmpdir("build-preservation");
    let build_path = dir.join("BUILD.bazel");
    let data = br#"load("@io_bazel_rules_go//go:def.bzl", "go_test")

go_library(
    name = "lib",
    srcs = ["lib.go"],
)

go_test(
    name = "first_test",
    embed = [":lib"],
)

go_test(
    name = "second_test",
    timeout = "long",
)
"#
    .to_vec();
    fs::write(&build_path, &data).unwrap();

    let mut file = patch_go_test_file("pkg/x/BUILD.bazel", &build_path, data).unwrap();
    write(build_path.to_str().unwrap(), &mut file).unwrap();
    let out = fs::read_to_string(&build_path).unwrap();
    assert!(out.contains("load(\"@io_bazel_rules_go//go:def.bzl\", \"go_test\")"));
    assert!(out.contains("name = \"lib\""));
    assert!(out.contains("name = \"first_test\""));
    assert!(out.contains("embed = [\":lib\"]"));
    assert!(out.contains("name = \"second_test\""));
    assert!(out.contains("timeout = \"long\""));

    assert!(build::ParseBuild("BUILD.bazel", b"go_test(\n".to_vec()).is_err());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_edit_keeps_single_line_rule_syntactically_valid() {
    let dir = tmpdir("single-line-build");
    let build_path = dir.join("BUILD.bazel");
    let data = b"go_test(name = \"unit_test\")\n".to_vec();
    fs::write(&build_path, &data).unwrap();

    let mut file = patch_go_test_file("pkg/x/BUILD.bazel", &build_path, data).unwrap();
    write(build_path.to_str().unwrap(), &mut file).unwrap();
    let out = fs::read_to_string(&build_path).unwrap();
    assert!(out.contains("name = \"unit_test\",\n"), "{out}");
    assert!(build::ParseBuild("BUILD.bazel", out.into_bytes()).is_ok());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_edit_only_treats_top_level_calls_as_rules() {
    let dir = tmpdir("top-level-rules");
    let build_path = dir.join("BUILD.bazel");
    let data = br#"wrapper(
    go_test(
        name = "nested_test",
    ),
)

go_test(
    name = "top_level_test",
)
"#
    .to_vec();
    fs::write(&build_path, &data).unwrap();

    let mut file = patch_go_test_file("pkg/x/BUILD.bazel", &build_path, data).unwrap();
    write(build_path.to_str().unwrap(), &mut file).unwrap();
    let out = fs::read_to_string(&build_path).unwrap();

    let nested = out.split("go_test(").nth(1).unwrap();
    let top_level = out.split("go_test(").nth(2).unwrap();
    assert!(!nested.contains("timeout = \"short\""), "{out}");
    assert!(!nested.contains("flaky = True"), "{out}");
    assert!(top_level.contains("timeout = \"short\""), "{out}");
    assert!(top_level.contains("flaky = True"), "{out}");

    let _ = fs::remove_dir_all(&dir);
}

fn tmpdir(name: &str) -> PathBuf {
    // 临时目录名带上进程号，避免并发测试进程复用系统临时目录时互相踩踏。
    let p = std::env::temp_dir().join(format!("tazel-{name}-{}", std::process::id()));
    // 先尝试删旧目录，使重复运行同一测试时仍从干净状态开始。
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn contract_normal_paths() {
    // 常规路径同时覆盖常量、路径过滤和完整 run_from 写回流程。
    assert_eq!(maxShardCount, 50);

    // `skipTazel` 会屏蔽工具自身或不应自动改写的 BUILD 文件。
    assert!(skipTazel("build/BUILD.bazel"));
    assert!(!skipTazel("pkg/foo/BUILD.bazel"));
    // `skipFlaky` 只对少数已知不稳定目录保留人工控制权。
    assert!(skipFlaky("tests/realtikvtest/addindextest/BUILD.bazel"));
    assert!(!skipFlaky("pkg/foo/BUILD.bazel"));

    let dir = tmpdir("normal");
    // `run_from` 只有在仓库根存在 `WORKSPACE` 时才会进入 BUILD 遍历。
    fs::write(dir.join("WORKSPACE"), b"").unwrap();
    let pkg = dir.join("pkg").join("demo");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(
        pkg.join("demo_test.go"),
        b"package demo\nfunc TestA(t *testing.T) {}\nfunc TestB(t *testing.T) {}\nfunc TestMain(m *testing.M) {}\nfunc (s *S) TestMethod(t *testing.T) {}\n",
    )
    .unwrap();
    fs::write(pkg.join("BUILD.bazel"), b"go_test(\n)\n").unwrap();

    // 这里走完整入口，验证预扫描计数与 BUILD 回写能串起来工作。
    run_from(&dir).unwrap();

    let out = fs::read_to_string(pkg.join("BUILD.bazel")).unwrap();
    // 2 个顶层 `Test*` 使分片数为 2；`TestMain` 与带接收者方法都不应计数。
    assert!(out.contains("timeout = \"short\""), "{out}");
    assert!(out.contains("flaky = True"), "{out}");
    assert!(out.contains("shard_count = 2"), "{out}");

    let _ = fs::remove_dir_all(&dir);
}

fn contract_boundary() {
    // 跳过名单中的正反例一起校验，防止目录前缀匹配被改坏。
    assert!(skipShardCount("tests/readonlytest/x/BUILD.bazel"));
    assert!(skipShardCount("pkg/util/foo/BUILD.bazel"));
    assert!(!skipShardCount("pkg/util/admin/BUILD.bazel"));
    assert!(!skipShardCount("pkg/util/chunk/BUILD.bazel"));

    // Existing timeout is preserved.
    // Rust 版本不能为了“补默认值”覆盖已有超时配置，否则会偏离 Go 工具。
    let data = b"go_test(\n    timeout = \"long\",\n)\n".to_vec();
    let tmp = tmpdir("preserve");
    let build_path = tmp.join("BUILD.bazel");
    fs::write(&build_path, &data).unwrap();
    let f = patch_go_test_file("pkg/x/BUILD.bazel", &build_path, data).unwrap();
    assert_eq!(f.rules[0].AttrString("timeout"), "long");

    // cnt == 1 deletes shard_count
    // 这里单独搭一个只含 1 个测试函数的目录，验证历史遗留的 `shard_count`
    // 会被显式删除，而不是保留成过度分片。
    initCount();
    let dir = tmpdir("boundary");
    let pkg = dir.join("only");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(
        pkg.join("a_test.go"),
        b"package only\nfunc TestOne(t *testing.T) {}\n",
    )
    .unwrap();
    scan(pkg.join("a_test.go").to_str().unwrap()).unwrap();
    let abs_pkg = fs::canonicalize(&pkg).unwrap();
    assert_eq!(test_count_for(&abs_pkg.to_string_lossy()), Some(1));

    let build_path = pkg.join("BUILD.bazel");
    fs::write(&build_path, b"go_test(\n    shard_count = 9,\n)\n").unwrap();
    // 直接检查解析后的 AST 属性是否为空，比只读回文本更能证明删除发生在
    // 补丁阶段而不是写回阶段的偶然格式化。
    let mut file = patch_go_test_file(
        "only/BUILD.bazel",
        &build_path,
        fs::read(&build_path).unwrap(),
    )
    .unwrap();
    assert!(file.rules[0].AttrLiteral("shard_count").is_empty());
    write(build_path.to_str().unwrap(), &mut file).unwrap();

    let _ = fs::remove_dir_all(&tmp);
    let _ = fs::remove_dir_all(&dir);
}

fn contract_error_paths() {
    // 缺少 `WORKSPACE` 时必须拒绝执行，避免把任意目录误判为仓库根。
    let dir = tmpdir("err");
    let err = run_from(&dir).unwrap_err();
    assert!(err.contains("project root"));

    // 解析桩至少要接受最小合法 `go_test` 规则，否则后续补丁流程无法工作。
    let bad = build::ParseBuild("BUILD.bazel", b"go_test(\n)\n".to_vec());
    assert!(bad.is_ok());

    let _ = fs::remove_dir_all(&dir);
}

fn contract_resource_cleanup() {
    // `initCount` 需要像 Go 的包级全局表那样可重复清空，避免跨测试污染。
    initCount();
    addTestMap("/tmp/tazel-cleanup-a");
    assert_eq!(test_count_for("/tmp/tazel-cleanup-a"), Some(1));
    initCount();
    assert_eq!(test_count_for("/tmp/tazel-cleanup-a"), None);

    // 成功执行后的临时工作区也应能被删除，说明没有残留文件句柄阻止清理。
    let dir = tmpdir("cleanup");
    fs::write(dir.join("WORKSPACE"), b"").unwrap();
    run_from(&dir).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    assert!(!dir.exists());
}
