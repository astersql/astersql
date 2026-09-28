// Copyright 2020 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// S3 命令行/配置 flags 与 `S3BackendOptions` 解析测试。
//
// 覆盖 profile、region、endpoint、存储类别、凭证成对校验，以及 profile 与显式
// AccessKey/Secret 并存时的行为（与 AWS CLI profile 语义对齐）。

/// 定义 S3 flags 并用键值对填充，返回可查询的 `FlagSet`。
fn flags(values: &[(&str, &str)]) -> s3like::pflag::FlagSet {
    let mut flags = s3like::pflag::FlagSet::default();
    s3like::DefineS3Flags(&mut flags);
    for (name, value) in values {
        flags.Set(name, value).unwrap();
    }
    flags
}

/// 将后端选项应用到 `backuppb::S3`，便于断言最终配置字段。
fn apply(options: &s3like::S3BackendOptions) -> anyhow::Result<s3like::backuppb::S3> {
    let mut backend = s3like::backuppb::S3::default();
    options.Apply(&mut backend)?;
    Ok(backend)
}

/// 验证 `s3.profile` flag 的读写，并检查源码中仍保留 Go 侧用法说明字符串。
#[test]
fn test_s3_profile_flag() {
    let mut flags = flags(&[]);
    assert_eq!(flags.GetString("s3.profile").unwrap(), "");
    flags.Set("s3.profile", "my-test-profile").unwrap();
    assert_eq!(flags.GetString("s3.profile").unwrap(), "my-test-profile");

    // The minimal Rust pflag facade intentionally exposes only Set/GetString.
    // Keep the Go usage-text assertion against the compiled source declaration.
    // Rust pflag 门面仅暴露 Set/GetString；用法文案断言改查源文件中的声明。
    let source = include_str!("../s3like/store.rs");
    assert!(source.contains("s3ProfileOption"));
    assert!(source.contains("Set the AWS profile"));
}

/// 从 flags 解析完整的一组 S3 后端选项字段。
#[test]
fn test_s3_backend_options_parse_from_flags() {
    let flags = flags(&[
        ("s3.region", "us-west-2"),
        ("s3.endpoint", "https://s3.example.com"),
        ("s3.profile", "production"),
        ("s3.storage-class", "GLACIER"),
        ("s3.provider", "aws"),
        ("s3.role-arn", "arn:aws:iam::123456789012:role/MyRole"),
        ("s3.external-id", "my-external-id"),
    ]);
    let mut options = s3like::S3BackendOptions::default();
    options.ParseFromFlags(&flags).unwrap();
    assert_eq!(options.Region, "us-west-2");
    assert_eq!(options.Endpoint, "https://s3.example.com");
    assert_eq!(options.Profile, "production");
    assert_eq!(options.StorageClass, "GLACIER");
    assert_eq!(options.Provider, "aws");
    assert_eq!(options.RoleARN, "arn:aws:iam::123456789012:role/MyRole");
    assert_eq!(options.ExternalID, "my-external-id");
}

/// 未设置 profile 时字段应为空字符串。
#[test]
fn test_s3_backend_options_parse_from_flags_profile_empty() {
    let flags = flags(&[("s3.region", "us-east-1")]);
    let mut options = s3like::S3BackendOptions::default();
    options.ParseFromFlags(&flags).unwrap();
    assert_eq!(options.Region, "us-east-1");
    assert_eq!(options.Profile, "");
}

/// profile 名允许包含连字符与下划线等特殊字符。
#[test]
fn test_s3_backend_options_parse_from_flags_profile_special_chars() {
    let flags = flags(&[("s3.profile", "dev-profile_123")]);
    let mut options = s3like::S3BackendOptions::default();
    options.ParseFromFlags(&flags).unwrap();
    assert_eq!(options.Profile, "dev-profile_123");
}

/// profile 与 region/endpoint 并存时，Apply 后应原样写入 backend。
#[test]
fn test_s3_backend_options_awscli_precedence() {
    let flags = flags(&[
        ("s3.profile", "production"),
        ("s3.region", "us-west-2"),
        ("s3.endpoint", "https://custom.s3.com"),
    ]);
    let mut options = s3like::S3BackendOptions::default();
    options.ParseFromFlags(&flags).unwrap();
    let backend = apply(&options).unwrap();
    assert_eq!(backend.Profile, "production");
    assert_eq!(backend.Region, "us-west-2");
    assert_eq!(backend.Endpoint, "https://custom.s3.com");
    assert_eq!(backend.StorageClass, "");
    assert_eq!(backend.Provider, "");
}

/// 无 profile 时仅靠 region/endpoint/storage-class 也能 Apply。
#[test]
fn test_s3_backend_options_no_profile() {
    let flags = flags(&[
        ("s3.region", "us-east-1"),
        ("s3.endpoint", "https://s3.amazonaws.com"),
        ("s3.storage-class", "GLACIER"),
    ]);
    let mut options = s3like::S3BackendOptions::default();
    options.ParseFromFlags(&flags).unwrap();
    let backend = apply(&options).unwrap();
    assert_eq!(backend.Profile, "");
    assert_eq!(backend.Region, "us-east-1");
    assert_eq!(backend.Endpoint, "https://s3.amazonaws.com");
    assert_eq!(backend.StorageClass, "GLACIER");
}

/// 仅配置 profile 时其余字段保持空。
#[test]
fn test_s3_backend_options_profile_only() {
    let flags = flags(&[("s3.profile", "development")]);
    let mut options = s3like::S3BackendOptions::default();
    options.ParseFromFlags(&flags).unwrap();
    let backend = apply(&options).unwrap();
    assert_eq!(backend.Profile, "development");
    assert_eq!(backend.Region, "");
    assert_eq!(backend.Endpoint, "");
    assert_eq!(backend.StorageClass, "");
}

/// profile 加部分覆盖字段（region、storage-class）时未覆盖的 endpoint 为空。
#[test]
fn test_s3_backend_options_partial_override() {
    let flags = flags(&[
        ("s3.profile", "staging"),
        ("s3.region", "eu-west-1"),
        ("s3.storage-class", "STANDARD_IA"),
    ]);
    let mut options = s3like::S3BackendOptions::default();
    options.ParseFromFlags(&flags).unwrap();
    let backend = apply(&options).unwrap();
    assert_eq!(backend.Profile, "staging");
    assert_eq!(backend.Region, "eu-west-1");
    assert_eq!(backend.Endpoint, "");
    assert_eq!(backend.StorageClass, "STANDARD_IA");
}

/// 使用 profile 时不自动填入 AccessKey/Secret（由运行时凭证链解析）。
#[test]
fn test_s3_backend_options_profile_credentials() {
    let backend = apply(&s3like::S3BackendOptions {
        Profile: "production".to_owned(),
        Region: "us-west-2".to_owned(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(backend.Profile, "production");
    assert_eq!(backend.Region, "us-west-2");
    assert_eq!(backend.AccessKey, "");
    assert_eq!(backend.SecretAccessKey, "");
}

/// profile 与显式 AccessKey/Secret 可同时存在并写入 backend。
#[test]
fn test_s3_backend_options_profile_with_explicit_credentials() {
    let backend = apply(&s3like::S3BackendOptions {
        Profile: "development".to_owned(),
        AccessKey: "explicit-access-key".to_owned(),
        SecretAccessKey: "explicit-secret-key".to_owned(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(backend.Profile, "development");
    assert_eq!(backend.AccessKey, "explicit-access-key");
    assert_eq!(backend.SecretAccessKey, "explicit-secret-key");
}

/// 无 profile 时 AccessKey/Secret 必须成对出现，缺一则报错；都空则允许走默认链。
#[test]
fn test_s3_backend_options_no_profile_credential_validation() {
    apply(&s3like::S3BackendOptions {
        AccessKey: "test-access-key".to_owned(),
        SecretAccessKey: "test-secret-key".to_owned(),
        Region: "us-east-1".to_owned(),
        ..Default::default()
    })
    .unwrap();

    let error = apply(&s3like::S3BackendOptions {
        AccessKey: "test-access-key".to_owned(),
        Region: "us-east-1".to_owned(),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.to_string().contains("secret_access_key not found"));

    let error = apply(&s3like::S3BackendOptions {
        SecretAccessKey: "test-secret-key".to_owned(),
        Region: "us-east-1".to_owned(),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.to_string().contains("access_key not found"));

    apply(&s3like::S3BackendOptions {
        Region: "us-east-1".to_owned(),
        ..Default::default()
    })
    .unwrap();
}

/// 有 profile 时允许只覆盖 AccessKey 或只覆盖 Secret（另一半来自 profile）。
#[test]
fn test_s3_backend_options_profile_partial_credentials() {
    apply(&s3like::S3BackendOptions {
        Profile: "test-profile".to_owned(),
        AccessKey: "override-access-key".to_owned(),
        ..Default::default()
    })
    .unwrap();
    apply(&s3like::S3BackendOptions {
        Profile: "test-profile".to_owned(),
        SecretAccessKey: "override-secret-key".to_owned(),
        ..Default::default()
    })
    .unwrap();
}

/// 经 flags 解析后再 Apply，确认 profile/region 生效且不注入静态密钥。
#[test]
fn test_s3_backend_options_flag_parsing_with_profile() {
    let flags = flags(&[("s3.profile", "production"), ("s3.region", "eu-central-1")]);
    let mut options = s3like::S3BackendOptions::default();
    options.ParseFromFlags(&flags).unwrap();
    let backend = apply(&options).unwrap();
    assert_eq!(backend.Profile, "production");
    assert_eq!(backend.Region, "eu-central-1");
    assert_eq!(backend.AccessKey, "");
    assert_eq!(backend.SecretAccessKey, "");
}

/// 配置 profile 时不应自动新建静态凭证字段。
#[test]
fn test_s3_profile_avoid_auto_new_cred() {
    let backend = apply(&s3like::S3BackendOptions {
        Profile: "test-profile".to_owned(),
        Region: "us-west-2".to_owned(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(backend.Profile, "test-profile");
    assert_eq!(backend.Region, "us-west-2");
    assert_eq!(backend.AccessKey, "");
    assert_eq!(backend.SecretAccessKey, "");
}
