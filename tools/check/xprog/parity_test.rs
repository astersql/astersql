// Copyright 2026 AsterSQL.

//! Parity tests for `tools/check/xprog` vs Go `xprog.go`.
//! 这组测试只验证对外可观察契约，不重写实现细节，因此更适合作为 Go/Rust 行为对齐的护栏。
//! 核心目标是固定 `run`、`get_package_info` 与 `move_file` 的返回码、路径推导与资源清理语义。
//! 各子场景分别覆盖正常流程、边界路径、错误码映射，以及跨设备复制后的句柄与源文件清理。
//! 测试数据全部在临时目录中构造，避免依赖真实仓库内容，同时保留与 Go 命令行形态一致的输入结构。

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::stubs::{filepath_clean, filepath_join};
use crate::xprog::{get_package_info, move_file, run};

#[test]
fn go_rust_public_contract_matches() {
    // 入口测试只负责串起四类契约检查，保持失败定位与 Go 端场景划分一致。
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

#[test]
fn run_slices_argv0_by_length_without_validating_suffix() {
    let tmp = tmpdir("xprog-argv0-suffix");
    let build_dir = tmp.join("go-build").join("b001");
    fs::create_dir_all(&build_dir).unwrap();
    fs::write(
        build_dir.join("importcfg.link"),
        "packagefile github.com/pingcap/tidb/pkg/session.test=/cache/x\n",
    )
    .unwrap();

    let repo = tmp.join("repo");
    fs::create_dir_all(repo.join("pkg").join("session")).unwrap();
    let test_bin = build_dir.join("session.test");
    fs::write(&test_bin, b"test-binary").unwrap();

    // Go blindly removes len("tools/bin/xprog") bytes. An equally long,
    // different suffix therefore still derives the repository root.
    let arg0 = repo
        .join("tools")
        .join("bin")
        .join("wrong")
        .to_string_lossy()
        .into_owned();
    assert_eq!(run(&[arg0, test_bin.to_string_lossy().into_owned()]), 0);
    assert!(repo.join("pkg/session/session.test.bin").is_file());

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn run_panics_when_test_binary_argument_is_missing() {
    let arg0 = "/tmp/repo/tools/bin/xprog".to_string();
    let result = std::panic::catch_unwind(|| run(&[arg0]));
    assert!(result.is_err(), "Go panics while indexing os.Args[1]");
}

#[test]
fn run_panics_when_package_is_too_short_to_trim_test_suffix() {
    let tmp = tmpdir("xprog-short-package");
    let build_dir = tmp.join("go-build").join("b001");
    fs::create_dir_all(&build_dir).unwrap();
    fs::write(
        build_dir.join("importcfg.link"),
        "packagefile github.com/pingcap/tidb/x=/cache/x\n",
    )
    .unwrap();

    let repo = tmp.join("repo");
    let arg0 = repo
        .join("tools")
        .join("bin")
        .join("xprog")
        .to_string_lossy()
        .into_owned();
    let test_bin = build_dir.join("x").to_string_lossy().into_owned();
    let result = std::panic::catch_unwind(|| run(&[arg0, test_bin]));
    assert!(
        result.is_err(),
        "Go panics when the slice end precedes the tidb prefix"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Normal: parse importcfg.link, rename into place, MoveFile fallback copy.
/// 正常路径验证三件事：读包名成功、目标路径推导正确，以及 rename/复制回退都能落到同一契约结果。
fn contract_normal_paths() {
    // 每个场景都单独创建根临时目录，避免前一个断言留下的文件状态污染后续检查。
    let tmp = tmpdir("xprog-normal");
    let build_dir = tmp.join("go-build").join("b001");
    fs::create_dir_all(&build_dir).unwrap();

    // 这里伪造 Go 编译目录里的 importcfg.link，首行格式必须与生产工具读取的文本完全一致。
    let pkg_line = "packagefile github.com/pingcap/tidb/pkg/util/topsql.test=/cache/abc-d\n";
    fs::write(build_dir.join("importcfg.link"), pkg_line).unwrap();

    // 先单独验证解析函数，避免后续 `run` 失败时无法区分是读配置还是搬运逻辑出错。
    let info = get_package_info(&build_dir).unwrap();
    assert_eq!(info, "github.com/pingcap/tidb/pkg/util/topsql.test");

    // Destination tree under fake repo root (cwd suffix strip).
    // 目标目录刻意构造成 tidb 仓库内的包路径，验证前缀裁剪后确实落到包目录旁。
    let repo = tmp.join("repo");
    let dest_dir = repo.join("pkg").join("util").join("topsql");
    fs::create_dir_all(&dest_dir).unwrap();

    // 生成一个最小可执行测试二进制占位物，并保留权限位，后面会验证 rename 后内容未变。
    let test_bin = build_dir.join("topsql.test");
    fs::write(&test_bin, b"#!/bin/true\ntest-binary\n").unwrap();
    let mut perms = fs::metadata(&test_bin).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&test_bin, perms).unwrap();

    let arg0 = repo
        .join("tools")
        .join("bin")
        .join("xprog")
        .to_string_lossy()
        .into_owned();
    // Ensure tools/bin exists so path looks real (binary itself need not exist).
    // `run` 会从 `argv[0]` 尾部剥掉 `tools/bin/xprog`，因此这里必须模拟出真实安装路径。
    fs::create_dir_all(repo.join("tools").join("bin")).unwrap();

    let code = run(&[arg0, test_bin.to_string_lossy().into_owned()]);
    assert_eq!(code, 0, "run should succeed via rename");

    // 成功路径的核心证据是：目标文件出现、源文件消失，并且字节内容与原始测试二进制一致。
    let dest = dest_dir.join("topsql.test.bin");
    assert!(dest.is_file(), "dest binary should exist at {dest:?}");
    assert!(
        !test_bin.exists(),
        "source test binary should be gone after rename"
    );
    assert_eq!(fs::read(&dest).unwrap(), b"#!/bin/true\ntest-binary\n");

    // MoveFile path: copy + remove + chmod.
    // 再单测 `move_file`，显式覆盖跨设备 rename 失败后的复制分支，而不是只依赖 `run` 间接经过。
    let src2 = tmp.join("cross-src.bin");
    let dst2 = tmp.join("cross-dst.bin");
    fs::write(&src2, b"moved-bytes").unwrap();
    let mut p = fs::metadata(&src2).unwrap().permissions();
    p.set_mode(0o640);
    fs::set_permissions(&src2, p).unwrap();
    move_file(&src2, &dst2).unwrap();
    // 复制分支除了内容一致，还要求删除源文件并继承权限位，这正是 Go `MoveFile` 的对齐点。
    assert_eq!(fs::read(&dst2).unwrap(), b"moved-bytes");
    assert!(!src2.exists());
    assert_eq!(
        fs::metadata(&dst2).unwrap().permissions().mode() & 0o777,
        0o640
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Boundary: prefix strip, `.test` trim, filepath Join/Clean match Go.
/// 边界场景主要固定 Go 路径语义，防止 Rust 误用 `Path::join` 后把后续绝对路径片段当成重置根目录。
fn contract_boundary() {
    // 这些断言锁定 `filepath_join`/`filepath_clean` 的 Go 风格结果，后续目标路径拼装全依赖它们。
    assert_eq!(
        filepath_join(&["github.com", "pingcap", "tidb"]),
        "github.com/pingcap/tidb"
    );
    assert_eq!(filepath_join(&["tools", "bin", "xprog"]), "tools/bin/xprog");
    assert_eq!(
        filepath_clean("/tmp/go-build/../go-build/b1/"),
        "/tmp/go-build/b1"
    );

    // Go filepath.Join concatenates then Clean — leading `/` on a later
    // element does NOT discard prior elements (unlike path.Join / Path::join).
    // 这是整份测试里最容易在 Rust 里写错的点：后续元素带 `/` 仍然要保留前缀仓库路径。
    let joined = filepath_join(&["/repo/", "/pkg/util/topsql", "topsql.test.bin"]);
    assert_eq!(joined, "/repo/pkg/util/topsql/topsql.test.bin");

    // 下面再把路径工具放回真实 `importcfg.link` 场景里，确认裁剪逻辑与目标文件名推导连起来仍成立。
    let tmp = tmpdir("xprog-boundary");
    let build_dir = tmp.join("b");
    fs::create_dir_all(&build_dir).unwrap();
    // 包路径故意不含 `pkg/` 前缀中的更多层级变化之外的信息，专门验证 `.test` 裁剪后的子路径。
    fs::write(
        build_dir.join("importcfg.link"),
        "packagefile github.com/pingcap/tidb/util/topsql.test=/x\n",
    )
    .unwrap();
    let pkg = get_package_info(&build_dir).unwrap();
    let prefix = filepath_join(&["github.com", "pingcap", "tidb"]);
    let stripped = &pkg[prefix.len()..pkg.len() - ".test".len()];
    // 裁剪后保留前导 `/`，因为 Go 版本依赖后续 Join+Clean 把它折叠回仓库绝对路径。
    assert_eq!(stripped, "/util/topsql");
    let file = Path::new(stripped).file_name().unwrap().to_string_lossy();
    assert_eq!(file, "topsql");

    // Dest under cwd after Join+Clean.
    // 最终目标名必须是 `包目录/包名.test.bin`，否则后续消费测试二进制的脚本就找不到文件。
    let dest = filepath_join(&["/Users/foo/tidb/", stripped, "topsql.test.bin"]);
    assert_eq!(dest, "/Users/foo/tidb/util/topsql/topsql.test.bin");

    let _ = fs::remove_dir_all(&tmp);
}

/// Error: missing importcfg (-1), empty file (-2), bad prefix (-3), move fail (-4).
/// 错误路径把 Go 中约定俗成的退出码逐一固定下来，避免重构后把不同失败原因揉成同一个返回值。
fn contract_error_paths() {
    // 该组断言按返回码从前到后排列，便于和 `run`/`get_package_info` 的分支顺序一一对应。
    let tmp = tmpdir("xprog-err");

    // -1: no importcfg.link
    // 缺文件代表构建目录不完整，`get_package_info` 必须直接返回 -1。
    let missing = tmp.join("missing");
    fs::create_dir_all(&missing).unwrap();
    assert_eq!(get_package_info(&missing), Err(-1));

    // -2: empty importcfg.link → EOF
    // 空文件在 Go 中等价于首行读取失败，这里要求映射成同一个退出码。
    let empty_dir = tmp.join("empty");
    fs::create_dir_all(&empty_dir).unwrap();
    fs::write(empty_dir.join("importcfg.link"), "").unwrap();
    assert_eq!(get_package_info(&empty_dir), Err(-2));

    // -3: package outside tidb prefix
    // 前缀校验保护工具只处理 tidb 仓库内包，其他模块即使格式正确也必须拒绝。
    let other = tmp.join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        other.join("importcfg.link"),
        "packagefile github.com/other/mod.test=/cache/x\n",
    )
    .unwrap();
    let repo = tmp.join("repo3");
    fs::create_dir_all(repo.join("tools").join("bin")).unwrap();
    // 即使测试二进制文件本身存在，只要包前缀不属于 tidb，也必须在路径重写前提前失败。
    let test_bin = other.join("mod.test");
    fs::write(&test_bin, b"x").unwrap();
    let arg0 = repo
        .join("tools")
        .join("bin")
        .join("xprog")
        .to_string_lossy()
        .into_owned();
    let code = run(&[arg0, test_bin.to_string_lossy().into_owned()]);
    assert_eq!(code, -3);

    // -4: rename + MoveFile both fail (source missing)
    // 这里预先创建目标目录，确保失败原因来自源文件缺失，而不是目录不存在导致的混淆。
    let build = tmp.join("failbuild");
    fs::create_dir_all(&build).unwrap();
    fs::write(
        build.join("importcfg.link"),
        "packagefile github.com/pingcap/tidb/pkg/session.test=/c\n",
    )
    .unwrap();
    let repo4 = tmp.join("repo4");
    fs::create_dir_all(repo4.join("tools").join("bin")).unwrap();
    // Create dest parent so failure is from missing source, not missing dest dir.
    // 这样可以证明 `-4` 真正表示“rename 与 copy 都失败”，而不是目的目录准备不足。
    fs::create_dir_all(repo4.join("pkg").join("session")).unwrap();
    let missing_src = build.join("session.test");
    let arg0 = repo4
        .join("tools")
        .join("bin")
        .join("xprog")
        .to_string_lossy()
        .into_owned();
    let code = run(&[arg0, missing_src.to_string_lossy().into_owned()]);
    assert_eq!(code, -4);

    // MoveFile open-source error shape
    // 额外校验错误消息文本，确保 Rust 端对外暴露的诊断形状与 Go 迁移预期一致。
    let err = move_file(Path::new("/no/such/src"), Path::new("/tmp/xprog-no-dst")).unwrap_err();
    assert!(
        err.to_string().contains("Couldn't open source file"),
        "got: {err}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Resource cleanup: MoveFile closes handles and removes the source after copy.
/// 资源清理场景验证复制成功与失败两侧的残局状态，防止文件句柄泄漏或误删源文件。
fn contract_resource_cleanup() {
    // 这一组不再关心包路径解析，而是只盯住复制流程结束后的文件系统残局。
    let tmp = tmpdir("xprog-cleanup");
    let src = tmp.join("src.bin");
    let dst = tmp.join("dst.bin");
    fs::write(&src, b"payload").unwrap();
    move_file(&src, &dst).unwrap();
    // 成功后必须删除源文件，否则调用方会同时看到旧路径和新路径，偏离 Go 工具行为。
    assert!(
        !src.exists(),
        "source must be removed after successful MoveFile"
    );
    assert_eq!(fs::read(&dst).unwrap(), b"payload");

    // Dest create failure: source stays.
    // 失败分支反过来要保留源文件，避免在目标不可写时把唯一副本也丢掉。
    let src2 = tmp.join("src2.bin");
    fs::write(&src2, b"keep").unwrap();
    let no_parent = tmp.join("nope").join("child.bin");
    let err = move_file(&src2, &no_parent).unwrap_err();
    assert!(
        err.to_string().contains("Couldn't open dest file"),
        "got: {err}"
    );
    assert!(src2.exists(), "source must remain when dest create fails");
    assert!(!no_parent.exists());

    let _ = fs::remove_dir_all(&tmp);
}

fn tmpdir(name: &str) -> PathBuf {
    // 用进程号和纳秒时间戳拼临时目录名，降低并发测试或重复运行时的碰撞概率。
    // 返回前主动创建目录，让调用方可以直接写文件，保持测试主体聚焦在契约断言本身。
    let base = std::env::temp_dir().join(format!(
        "{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&base).unwrap();
    base
}
