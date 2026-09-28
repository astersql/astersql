// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! `go test --exec=xprog` helper: relocate the test binary for later use.
//! Go package: `main` (`tools/check/xprog/xprog.go`).
//!
//! 该工具充当 `go test --exec` 的轻量包装器：Go 在临时目录生成 `.test` 二进制后，
//! 这里会根据 `importcfg.link` 反推出真实包路径，再把产物移动回仓库对应目录，
//! 方便后续检查脚本复用已构建好的测试程序。
//!
//! Rust 版本保持 Go 的退出码和 panic 契约，而不是把所有异常统一改写成 `Result`，
//! 这样上层 `tools/check` 流程仍可按既有约定判断失败原因。

use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::stubs;

/// Process entry matching Go `main`. Non-zero codes call `process::exit`.
/// 进程入口只负责桥接 `run` 的返回值，保持与 Go `os.Exit` 相同的外部可观察行为。
pub fn main() {
    let code = run(&std::env::args().collect::<Vec<_>>());
    if code != 0 {
        std::process::exit(code);
    }
}

/// Core of Go `main`, returning the process exit code (`0` = success).
///
/// Exit codes match Go `os.Exit`:
/// - `-1` — cannot open `importcfg.link`
/// - `-2` — cannot read the first line
/// - `-3` — package is not under `github.com/pingcap/tidb`
/// - `-4` — rename and cross-device move both failed
///
/// 这里不直接执行测试，而是拦截 Go 传给 `--exec` 的测试二进制路径，
/// 依据 `importcfg.link` 中记录的包名推导目标位置，然后把二进制改名为
/// `<pkg>/<leaf>.test.bin`，以便仓库内其他检查步骤重用。
pub fn run(args: &[String]) -> i32 {
    // See https://github.com/golang/go/issues/15513#issuecomment-773994959
    // go test --exec=xprog ./...
    // Command line args looks like:
    // '$CWD/xprog /tmp/go-build2662369829/b1382/aggfuncs.test -test.paniconexit0 -test.timeout=10m0s'
    // This program moves the test binary /tmp/go-build2662369829/b1382/aggfuncs.test to someplace else for later use.

    // Extract the current work directory
    // Go 版本通过“去掉已知后缀”的方式回到仓库根目录，
    // 这里保留同样的字符串切片语义：Go 只按已知后缀长度裁剪，
    // 不会额外校验 `argv[0]` 的实际后缀内容。
    // Go: cwd := os.Args[0]; cwd = cwd[:len(cwd)-len(filepath.Join("tools", "bin", "xprog"))]
    let suffix = stubs::filepath_join(&["tools", "bin", "xprog"]);
    let arg0 = &args[0];
    let cwd = &arg0[..arg0.len() - suffix.len()];

    let test_binary_path = PathBuf::from(stubs::filepath_clean(&args[1]));
    let dir = test_binary_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(""));

    // Extract the package info from /tmp/go-build2662369829/b1382/importcfg.link
    // `go test` 的临时目录名不稳定，因此不能从路径本身猜包名，
    // 必须读取编译阶段生成的 `importcfg.link` 才能恢复出 `pkg/...` 对应关系。
    let mut pkg = match get_package_info(&dir) {
        Ok(p) => p,
        Err(code) => return code,
    };

    let prefix = stubs::filepath_join(&["github.com", "pingcap", "tidb"]);
    if !pkg.starts_with(&prefix) {
        // 只接受本仓库包，避免把第三方依赖测试产物误搬回工作树。
        return -3;
    }

    // github.com/pingcap/tidb/pkg/util/topsql.test => /pkg/util/topsql
    // 切掉仓库前缀与 `.test` 后缀后，保留前导 `/`，
    // 让后续 `filepath_join(cwd, pkg, ...)` 继续遵循 Go 的清理结果。
    // (leading slash kept; filepath.Join then cleans `cwd + /pkg/...` into `$CWD/pkg/...`)
    let test_suffix = ".test";
    pkg = pkg[prefix.len()..pkg.len() - test_suffix.len()].to_string();

    let file = Path::new(&pkg)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // The path of the destination file looks like $CWD/util/topsql/topsql.test.bin
    // 目标文件名固定追加 `.test.bin`，从而与源码目录中其他文件区分开，
    // 也让后续工具能够按统一命名规则发现这些缓存下来的测试程序。
    let new_name = stubs::filepath_join(&[cwd, &pkg, &format!("{file}.test.bin")]);
    let new_name = stubs::filepath_clean(&new_name);

    if fs::rename(&test_binary_path, &new_name).is_err() {
        // Rename fail, handle error like "invalid cross-device link"
        // 临时目录与仓库目录可能位于不同文件系统；此时 `rename`
        // 会因 cross-device link 失败，必须退化为 copy + chmod + remove，
        // 才能与 Go `MoveFile` 的容错行为保持一致。
        if move_file(&test_binary_path, Path::new(&new_name)).is_err() {
            return -4;
        }
    }
    0
}

/// Read `importcfg.link` and extract the package path from the first line.
///
/// Go: `packagefile github.com/pingcap/tidb/pkg/session.test=/cache/...`
/// 该辅助函数只关心首行，因为 Go 版本同样只读取一次 `ReadLine`，
/// 并假定首条 `packagefile` 记录就是当前测试二进制对应的包名。
pub fn get_package_info(dir: &Path) -> Result<String, i32> {
    let path = stubs::filepath_join(&[
        &stubs::filepath_clean(&dir.to_string_lossy()),
        "importcfg.link",
    ]);
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return Err(-1),
    };

    // Go's default bufio.Reader has a 4096-byte buffer. ReadLine returns only
    // the first fragment when a line exceeds that buffer; the Go code ignores
    // isPrefix, so delimiter lookup and slicing operate on that fragment.
    let mut reader = BufReader::with_capacity(4096, file);
    let buffer = match reader.fill_buf() {
        Ok([]) | Err(_) => return Err(-2),
        Ok(buffer) => buffer,
    };
    let line_end = buffer
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(buffer.len());
    let mut line = &buffer[..line_end];
    if line.ends_with(b"\r") {
        line = &line[..line.len() - 1];
    }

    // packagefile github.com/pingcap/tidb/pkg/session.test=/home/...
    // 分隔符缺失被视为违反 Go 工具链约定，因而沿用 panic，
    // 而不是返回新的错误码去扩大原有命令的状态空间。
    let start = match line.iter().position(|byte| *byte == b' ') {
        Some(i) => i,
        None => panic!("importcfg.link line missing space"),
    };
    let end = match line.iter().position(|byte| *byte == b'=') {
        Some(i) => i,
        None => panic!("importcfg.link line missing '='"),
    };
    Ok(String::from_utf8(line[start + 1..end].to_vec())
        .expect("importcfg.link package path is not UTF-8"))
}

/// Move a file from `source_path` to `dest_path` via copy + chmod + remove.
/// Matches Go `MoveFile` (cross-device rename fallback).
/// 顺序必须先复制内容、再复制权限、最后删除源文件；
/// 任一步失败都保留源文件，避免把测试二进制丢失在中间状态。
pub fn move_file(source_path: &Path, dest_path: &Path) -> io::Result<()> {
    // Go calls filepath.Clean but discards the results — no-op; keep comment parity.
    let _ = stubs::filepath_clean(&source_path.to_string_lossy());
    let _ = stubs::filepath_clean(&dest_path.to_string_lossy());

    let mut input_file = fs::File::open(source_path)
        .map_err(|err| io::Error::new(err.kind(), format!("Couldn't open source file: {err}")))?;

    let mut output_file = match fs::File::create(dest_path) {
        Ok(f) => f,
        Err(err) => {
            drop(input_file);
            return Err(io::Error::new(
                err.kind(),
                format!("Couldn't open dest file: {err}"),
            ));
        }
    };

    if let Err(err) = io::copy(&mut input_file, &mut output_file) {
        return Err(io::Error::new(
            err.kind(),
            format!("Writing to output file failed: {err}"),
        ));
    }
    // Go: inputFile.Close() after Copy; output deferred Close.
    drop(input_file);
    drop(output_file);

    // Handle the permissions
    // 目标文件先按默认权限创建，再显式同步源文件权限，
    // 这样最终可执行位与 Go 测试产物保持一致。
    let metadata = fs::metadata(source_path)
        .map_err(|err| io::Error::new(err.kind(), format!("Stat error: {err}")))?;
    fs::set_permissions(dest_path, metadata.permissions())
        .map_err(|err| io::Error::new(err.kind(), format!("Chmod error: {err}")))?;

    // The copy was successful, so now delete the original file
    // 只有在复制与 chmod 都成功后才删除源文件，
    // 保证跨设备回退仍尽量接近原子 rename 的结果。
    fs::remove_file(source_path).map_err(|err| {
        io::Error::new(err.kind(), format!("Failed removing original file: {err}"))
    })?;
    Ok(())
}
