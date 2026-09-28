// Copyright 2026 AsterSQL.

use std::fs;

#[test]
fn struct_aliases_are_classified_like_go_ast() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("builtin_alias.go");
    fs::write(
        &source,
        r#"package expression
type builtinAliasSafeSig = struct { baseBuiltinFunc }
type builtinAliasUnsafeSig = struct {
    baseBuiltinFunc
    state int
}
type builtinNamedSafeSig struct { base baseBuiltinFunc }
type builtinTaggedSafeSig struct { baseBuiltinFunc `json:"base"` }
"#,
    )
    .unwrap();

    let (safe, unsafe_names) =
        crate::builtin_threadsafe::collect_thread_safe_builtin_funcs(&source).unwrap();

    assert_eq!(
        safe,
        [
            "builtinAliasSafeSig",
            "builtinNamedSafeSig",
            "builtinTaggedSafeSig"
        ]
    );
    assert_eq!(unsafe_names, ["builtinAliasUnsafeSig"]);
}

#[cfg(unix)]
#[test]
fn generator_scans_symlinked_go_files_like_os_read_dir() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("source");
    fs::create_dir(&target).unwrap();
    fs::write(
        target.join("definition.go"),
        "package expression\ntype builtinLinkedSig struct { baseBuiltinFunc }\n",
    )
    .unwrap();
    symlink(
        target.join("definition.go"),
        dir.path().join("builtin_link.go"),
    )
    .unwrap();

    let (safe, _) = crate::builtin_threadsafe::gen_builtin_thread_safe_code(dir.path()).unwrap();
    assert!(
        String::from_utf8(safe)
            .unwrap()
            .contains("builtinLinkedSig")
    );
}

#[test]
fn generator_can_write_both_main_outputs() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("builtin_sample.go"),
        "package expression\ntype builtinSampleSig struct { baseBuiltinFunc }\n",
    )
    .unwrap();

    crate::builtin_threadsafe::write_generated_outputs(dir.path()).unwrap();

    let safe = fs::read_to_string(dir.path().join("builtin_threadsafe_generated.go")).unwrap();
    let unsafe_code =
        fs::read_to_string(dir.path().join("builtin_threadunsafe_generated.go")).unwrap();
    let go_license_prefix =
        "// Copyright 2024 PingCAP, Inc.\n//\n// Licensed under the Apache License";
    assert!(safe.starts_with(go_license_prefix));
    assert!(unsafe_code.starts_with(go_license_prefix));
    assert!(safe.contains("func (s *builtinSampleSig) SafeToShareAcrossSession() bool"));
    assert!(unsafe_code.contains("package expression"));
}

#[test]
fn generate_code_formats_and_validates_go_source() {
    let names = vec!["builtinSampleSig".to_owned()];
    let formatted = crate::builtin_threadsafe::generate_code(
        &names,
        "package expression\n",
        "func(s *%s) f( ){return}\n",
    )
    .unwrap();

    assert_eq!(
        String::from_utf8(formatted).unwrap(),
        "package expression\n\nfunc (s *builtinSampleSig) f() { return }\n"
    );
    assert!(
        crate::builtin_threadsafe::generate_code(&[], "not go source", "").is_err(),
        "Go format.Source rejects syntactically invalid generated source"
    );
}
