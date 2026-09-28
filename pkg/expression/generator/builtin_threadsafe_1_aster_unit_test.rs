// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// builtin_threadsafe 及相关向量化生成器的单元测试。
//
// 验证：AST 安全/不安全分类、目录扫描过滤与排序、compare/control/other/string
// 生成内容覆盖 Go 签名矩阵，以及 gofmt 可解析性。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// 创建带唯一后缀的临时目录，避免并行测试冲突。
fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("builtin-threadsafe-{name}-{nonce}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 单文件 AST：仅基类嵌入与 specialSafe 为安全；多字段为不安全；别名/非 builtin 忽略。
#[test]
fn collects_safe_and_unsafe_builtin_signatures_like_go_ast() {
    let dir = temp_dir("collect");
    let source = dir.join("builtin_sample.go");
    fs::write(
        &source,
        r#"package expression
type builtinSafeSig struct { baseBuiltinFunc }
type builtinCastSig struct { baseBuiltinCastFunc }
type builtinUnsafeSig struct {
    baseBuiltinFunc
    state int
}
type builtinInIntSig struct { baseBuiltinFunc; state int }
type builtinAliasSig = int
type notBuiltinSig struct { baseBuiltinFunc }
"#,
    )
    .unwrap();

    let (safe, unsafe_names) =
        crate::builtin_threadsafe::collect_thread_safe_builtin_funcs(&source).unwrap();
    assert_eq!(
        safe,
        ["builtinSafeSig", "builtinCastSig", "builtinInIntSig"]
    );
    assert_eq!(unsafe_names, ["builtinUnsafeSig"]);
    fs::remove_dir_all(dir).unwrap();
}

/// 仅扫描 `builtin_*.go` 且排除 `_test`；安全名按字典序出现在生成物中。
#[test]
fn scans_only_builtin_non_test_go_files_and_sorts_safe_names() {
    let dir = temp_dir("scan");
    fs::write(
        dir.join("builtin_z.go"),
        "package expression\ntype builtinZSig struct { baseBuiltinFunc }\n",
    )
    .unwrap();
    fs::write(
        dir.join("builtin_a.go"),
        "package expression\ntype builtinASig struct { baseBuiltinFunc }\n",
    )
    .unwrap();
    fs::write(
        dir.join("builtin_skip_test.go"),
        "package expression\ntype builtinSkipSig struct { baseBuiltinFunc }\n",
    )
    .unwrap();
    fs::write(
        dir.join("ordinary.go"),
        "package expression\ntype builtinOrdinarySig struct { baseBuiltinFunc }\n",
    )
    .unwrap();

    let (safe, unsafe_code) =
        crate::builtin_threadsafe::gen_builtin_thread_safe_code(&dir).unwrap();
    let safe = String::from_utf8(safe).unwrap();
    let unsafe_code = String::from_utf8(unsafe_code).unwrap();
    assert!(safe.find("builtinASig").unwrap() < safe.find("builtinZSig").unwrap());
    assert!(!safe.contains("builtinSkipSig"));
    assert!(!safe.contains("builtinOrdinarySig"));
    assert!(!unsafe_code.contains("builtinSkipSig"));
    fs::remove_dir_all(dir).unwrap();
}

/// compare 生成器保留 Go 分派矩阵：跳过 LT/NullEQ 的 Int，覆盖 Real/String/Coalesce/JSON。
#[test]
fn compare_generator_preserves_go_dispatch_matrix() {
    let source = String::from_utf8(
        crate::compare_vec::generate_dot_go(
            crate::compare_vec::COMPARES_MAP,
            crate::compare_vec::TYPES_MAP,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(!source.contains("builtinLTIntSig"));
    assert!(!source.contains("builtinNullEQIntSig"));
    assert!(source.contains("builtinLTRealSig"));
    assert!(source.contains("builtinNullEQStringSig"));
    assert!(source.contains("builtinCoalesceIntSig"));
    assert!(source.contains("types.CompareBinaryJSON"));
    assert_eq!(source.matches(" vectorized() bool").count(), 49);
}

/// control/other/string 生成器覆盖全部 Go 签名与关键辅助函数引用。
#[test]
fn control_other_and_string_generators_cover_all_go_signatures() {
    let control = String::from_utf8(crate::control_vec::generate_dot_go().unwrap()).unwrap();
    for builtin in ["CaseWhen", "IfNull", "If"] {
        for ty in [
            "Int", "Real", "Decimal", "String", "Time", "Duration", "JSON",
        ] {
            assert!(
                control.contains(&format!("builtin{builtin}{ty}Sig")),
                "missing {builtin}/{ty}"
            );
        }
    }
    assert!(control.contains("fallbackEvalString"));

    let other = String::from_utf8(crate::other_vec::generate_dot_go().unwrap()).unwrap();
    for sig in crate::other_vec::IN_SIGS_TMPL {
        assert!(other.contains(sig.sig_name), "missing {}", sig.sig_name);
    }
    assert!(other.contains("compareSignedAndUnsignedInts"));
    assert!(other.contains("collate.GetCollator"));

    let string = String::from_utf8(crate::string_vec::generate_dot_go().unwrap()).unwrap();
    assert!(string.contains("builtinFieldIntSig"));
    assert!(string.contains("builtinFieldRealSig"));
    assert!(string.contains("builtinFieldStringSig"));
    assert_eq!(string.matches(" vectorized() bool").count(), 3);
}

/// 各生成器默认输出均为成对的 `.go` / `_test.go`，且模板占位符已展开。
#[test]
fn every_generator_returns_go_and_test_output_paths() {
    let compare = crate::compare_vec::default_outputs().unwrap();
    let control = crate::control_vec::default_outputs().unwrap();
    let other = crate::other_vec::default_outputs().unwrap();
    let string = crate::string_vec::default_outputs().unwrap();
    for outputs in [compare, control, other, string] {
        assert!(outputs[0].0.to_string_lossy().ends_with(".go"));
        assert!(outputs[1].0.to_string_lossy().ends_with("_test.go"));
        assert!(!outputs[0].1.is_empty());
        assert!(!outputs[1].1.is_empty());
        assert!(!String::from_utf8_lossy(&outputs[0].1).contains("{{"));
        assert!(!String::from_utf8_lossy(&outputs[1].1).contains("{{"));
    }
}

/// 生成的 Go 源码须能被 gofmt 解析（语法合法）。
#[test]
fn every_generated_file_is_valid_go_syntax() {
    let outputs = [
        crate::compare_vec::default_outputs().unwrap(),
        crate::control_vec::default_outputs().unwrap(),
        crate::other_vec::default_outputs().unwrap(),
        crate::string_vec::default_outputs().unwrap(),
    ];
    for pair in outputs {
        for (path, source) in pair {
            let mut child = Command::new("gofmt")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            child.stdin.take().unwrap().write_all(&source).unwrap();
            let result = child.wait_with_output().unwrap();
            assert!(
                result.status.success(),
                "{}: {}",
                path.display(),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}
