// Copyright 2026 AsterSQL.

#[cfg(unix)]
#[test]
fn get_storage_size_does_not_depend_on_df_or_path() {
    let temp_dir = std::env::temp_dir();
    if std::env::var_os("ASTERSQL_STORAGE_UNIX_PATH_CHILD").is_some() {
        let size = crate::storage_unix::GetStorageSize(temp_dir.to_str().expect("UTF-8 temp dir"))
            .expect("GetStorageSize must use statvfs rather than an external df command");
        assert!(size.Capacity > 0);
        assert!(size.Available > 0);
        return;
    }

    let empty_path = temp_dir.join(format!("astersql-empty-path-{}", std::process::id()));
    std::fs::create_dir_all(&empty_path).expect("create isolated PATH directory");
    let output = std::process::Command::new(std::env::current_exe().expect("current test binary"))
        .args([
            "--exact",
            "storage_unix_test::get_storage_size_does_not_depend_on_df_or_path",
            "--nocapture",
        ])
        .env("PATH", &empty_path)
        .env("ASTERSQL_STORAGE_UNIX_PATH_CHILD", "1")
        .output()
        .expect("run isolated child test");
    let _ = std::fs::remove_dir(&empty_path);
    assert!(
        output.status.success(),
        "isolated child failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
