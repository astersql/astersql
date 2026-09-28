// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 存储后端 URL 解析单测：覆盖各 scheme、query 参数与凭证校验。
//
// 对应 Go `parse_test.go`：校验非法 URL、local/hdfs/s3/ks3/gcs/azure 解析、
// FormatBackendURL、`+` 密钥保留、profile 选项与 force-path-style 默认行为。

use std::fs;
use std::path::Path;

use crate::parse::{
    AzblobBackendOptions, AzureBlobStorage, BackendOptions, FormatBackendURL, GCSBackendOptions,
    Gcs, IsLocalPath, KS3SDKProvider, Local, ParseBackend, ParseRawURL, S3, S3BackendOptions,
    StorageBackend,
};

/// 断言并取出 S3 后端，便于测试断言字段。
fn s3(backend: StorageBackend) -> S3 {
    match backend {
        StorageBackend::S3(value) => value,
        other => panic!("expected S3 backend, got {other:?}"),
    }
}

/// 断言并取出 GCS 后端。
fn gcs(backend: StorageBackend) -> Gcs {
    match backend {
        StorageBackend::Gcs(value) => value,
        other => panic!("expected GCS backend, got {other:?}"),
    }
}

/// 断言并取出 Azure 后端。
fn azure(backend: StorageBackend) -> AzureBlobStorage {
    match backend {
        StorageBackend::AzureBlobStorage(value) => value,
        other => panic!("expected Azure backend, got {other:?}"),
    }
}

/// 对 query 参数值做 form-urlencoded，模拟 URL 中的转义。
fn query_escape(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// 主路径：非法 scheme、各后端解析、query 覆盖与凭证文件读入。
#[test]
fn test_create_storage() {
    let error = ParseBackend("1invalid:", None).unwrap_err();
    assert!(
        error.to_string().contains("relative URL without a base"),
        "unexpected parse error: {error:#}"
    );

    let error = ParseBackend("net:storage", None).unwrap_err();
    assert!(
        error.to_string().contains("storage net not support yet"),
        "unexpected unsupported-storage error: {error:#}"
    );

    for (raw, expected_path) in [
        ("local:///tmp/storage", "/tmp/storage"),
        ("file:///tmp/storage", "/tmp/storage"),
    ] {
        assert_eq!(
            ParseBackend(raw, None).unwrap(),
            StorageBackend::Local(Local {
                path: expected_path.to_owned()
            })
        );
    }
    assert_eq!(ParseBackend("noop://", None).unwrap(), StorageBackend::Noop);
    assert_eq!(
        ParseBackend("hdfs://127.0.0.1:1231/backup", None).unwrap(),
        StorageBackend::Hdfs(crate::parse::Hdfs {
            remote: "hdfs://127.0.0.1:1231/backup".to_owned(),
        })
    );

    let error = ParseBackend(
        "s3:///bucket/more/prefix/",
        Some(&BackendOptions::default()),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("please specify the bucket for s3 in s3:///bucket/more/prefix/"),
        "unexpected missing-bucket error: {error:#}"
    );

    let s3_options = BackendOptions {
        s3: S3BackendOptions {
            endpoint: "https://s3.example.com/".to_owned(),
            ..S3BackendOptions::default()
        },
        ..BackendOptions::default()
    };
    let parsed = s3(ParseBackend("s3://bucket2/prefix/", Some(&s3_options)).unwrap());
    assert_eq!(parsed.bucket, "bucket2");
    assert_eq!(parsed.prefix, "prefix");
    assert_eq!(parsed.endpoint, "https://s3.example.com");
    assert!(!parsed.force_path_style);

    let parsed = s3(ParseBackend("ks3://bucket2/prefix/", Some(&s3_options)).unwrap());
    assert_eq!(parsed.bucket, "bucket2");
    assert_eq!(parsed.prefix, "prefix");
    assert_eq!(parsed.endpoint, "https://s3.example.com");
    assert_eq!(parsed.provider, KS3SDKProvider);
    assert!(!parsed.force_path_style);

    let parsed = s3(
        ParseBackend(
            "s3://bucket3/prefix/path?endpoint=https://127.0.0.1:9000&force_path_style=0&SSE=aws:kms&sse-kms-key-id=TestKey&xyz=abc",
            None,
        )
        .unwrap(),
    );
    assert_eq!(parsed.bucket, "bucket3");
    assert_eq!(parsed.prefix, "prefix/path");
    assert_eq!(parsed.endpoint, "https://127.0.0.1:9000");
    assert!(!parsed.force_path_style);
    assert_eq!(parsed.sse, "aws:kms");
    assert_eq!(parsed.sse_kms_key_id, "TestKey");

    let parsed = s3(ParseBackend(
        "s3://bucket4/prefix/path?access-key=******&secret-access-key=******+&session-token=******",
        None,
    )
    .unwrap());
    assert_eq!(parsed.bucket, "bucket4");
    assert_eq!(parsed.prefix, "prefix/path");
    assert_eq!(parsed.access_key, "******");
    assert_eq!(parsed.secret_access_key, "******+");
    assert_eq!(parsed.session_token, "******");
    assert!(parsed.force_path_style);

    let role_arn = "arn:aws:iam::888888888888:role/my-role";
    let external_id = "abcd1234";
    let parsed = s3(ParseBackend(
        &format!(
            "s3://bucket5/prefix/path?role-arn={}&external-id={}",
            query_escape(role_arn),
            query_escape(external_id)
        ),
        None,
    )
    .unwrap());
    assert_eq!(parsed.bucket, "bucket5");
    assert_eq!(parsed.prefix, "prefix/path");
    assert_eq!(parsed.role_arn, role_arn);
    assert_eq!(parsed.external_id, external_id);

    // GCS：用临时凭证文件验证 credentials_file → credentials_blob。
    let temporary = tempfile::tempdir().unwrap();
    let credentials_file = temporary.path().join("fakeCredentialsFile");
    fs::write(&credentials_file, "fakeCredentials").unwrap();
    let mut gcs_options = BackendOptions {
        gcs: GCSBackendOptions {
            endpoint: "https://gcs.example.com/".to_owned(),
            ..GCSBackendOptions::default()
        },
        ..BackendOptions::default()
    };
    for (raw, expected_bucket, expected_prefix) in [
        ("gcs://bucket2/prefix/", "bucket2", "prefix"),
        ("gcs://bucket2", "bucket2", ""),
    ] {
        let parsed = gcs(ParseBackend(raw, Some(&gcs_options)).unwrap());
        assert_eq!(parsed.bucket, expected_bucket);
        assert_eq!(parsed.prefix, expected_prefix);
        assert_eq!(parsed.endpoint, "https://gcs.example.com/");
        assert!(parsed.credentials_blob.is_empty());
    }

    gcs_options.gcs.credentials_file = credentials_file.to_string_lossy().into_owned();
    let parsed = gcs(ParseBackend("gcs://bucket/more/prefix/", Some(&gcs_options)).unwrap());
    assert_eq!(parsed.bucket, "bucket");
    assert_eq!(parsed.prefix, "more/prefix");
    assert_eq!(parsed.endpoint, "https://gcs.example.com/");
    assert_eq!(parsed.credentials_blob, "fakeCredentials");

    let parsed = gcs(ParseBackend(
        "gcs://bucket?endpoint=http://127.0.0.1/",
        Some(&gcs_options),
    )
    .unwrap());
    assert_eq!(parsed.endpoint, "http://127.0.0.1/");

    fs::write(&credentials_file, "fakeCreds2").unwrap();
    let parsed = gcs(ParseBackend(
        &format!(
            "gs://bucket4/backup/?credentials-file={}",
            query_escape(&credentials_file.to_string_lossy())
        ),
        None,
    )
    .unwrap());
    assert_eq!(parsed.bucket, "bucket4");
    assert_eq!(parsed.prefix, "backup");
    assert_eq!(parsed.credentials_blob, "fakeCreds2");

    let parsed = azure(
        ParseBackend(
            "azure://bucket1/prefix/path?account-name=user&account-key=cGFzc3dk&endpoint=http://127.0.0.1/user",
            None,
        )
        .unwrap(),
    );
    assert_eq!(parsed.bucket, "bucket1");
    assert_eq!(parsed.prefix, "prefix/path");
    assert_eq!(parsed.endpoint, "http://127.0.0.1/user");
    assert_eq!(parsed.account_name, "user");
    assert_eq!(parsed.shared_key, "cGFzc3dk");

    // 无 scheme 的绝对路径视为本地存储。
    assert_eq!(
        ParseBackend("/test", None).unwrap(),
        StorageBackend::Local(Local {
            path: Path::new("/test").to_string_lossy().into_owned()
        })
    );
}

/// `FormatBackendURL` 应输出不含敏感信息的规范 URL。
#[test]
fn test_format_backend_url() {
    let cases = [
        (
            StorageBackend::Local(Local {
                path: "/tmp/file".to_owned(),
            }),
            "local:///tmp/file",
        ),
        (StorageBackend::Noop, "noop:///"),
        (
            StorageBackend::S3(S3 {
                bucket: "bucket".to_owned(),
                prefix: "/some prefix/".to_owned(),
                endpoint: "https://s3.example.com/".to_owned(),
                ..S3::default()
            }),
            "s3://bucket/some%20prefix/",
        ),
        (
            StorageBackend::Gcs(Gcs {
                bucket: "bucket".to_owned(),
                prefix: "/some prefix/".to_owned(),
                endpoint: "https://gcs.example.com/".to_owned(),
                ..Gcs::default()
            }),
            "gcs://bucket/some%20prefix/",
        ),
        (
            StorageBackend::AzureBlobStorage(AzureBlobStorage {
                bucket: "bucket".to_owned(),
                prefix: "/some prefix/".to_owned(),
                endpoint: "https://azure.example.com/".to_owned(),
                ..AzureBlobStorage::default()
            }),
            "azure://bucket/some%20prefix/",
        ),
    ];
    for (backend, expected) in cases {
        assert_eq!(FormatBackendURL(&backend), expected);
    }
}

/// 密钥中含 `/` 或 `+` 时，解析后 secret 应原样保留。
#[test]
fn test_parse_raw_url() {
    let cases = [
        (
            "s3://bucket/prefix/path?access-key=NXN7IPIOSAAKDEEOLMAF&secret-access-key=nREY/7DtPaIbYKrKlEEMMF/ExCiJEX=XMLPUANw",
            "nREY/7DtPaIbYKrKlEEMMF/ExCiJEX=XMLPUANw",
        ),
        (
            "s3://bucket/prefix/path?access-key=NXN7IPIOSAAKDEEOLMAF&secret-access-key=nREY/7Dt+PaIbYKrKlEEMMF/ExCiJEX=XMLPUANw",
            "nREY/7Dt+PaIbYKrKlEEMMF/ExCiJEX=XMLPUANw",
        ),
    ];
    for (raw, expected_secret) in cases {
        let parsed_url = ParseRawURL(raw).unwrap();
        assert_eq!(parsed_url.scheme, "s3");
        assert_eq!(parsed_url.host, "bucket");
        assert_eq!(parsed_url.path, "/prefix/path");

        let parsed = s3(ParseBackend(raw, None).unwrap());
        assert_eq!(parsed.access_key, "NXN7IPIOSAAKDEEOLMAF");
        assert_eq!(parsed.secret_access_key, expected_secret);
    }
}

/// Go `url.URL.Host` retains an explicit port; bucket extraction must do the same.
#[test]
fn test_parse_raw_url_and_backend_preserve_host_port() {
    let raw = "s3://bucket.example:9000/prefix";
    let parsed_url = ParseRawURL(raw).unwrap();
    assert_eq!(parsed_url.host, "bucket.example:9000");

    let parsed = s3(ParseBackend(raw, None).unwrap());
    assert_eq!(parsed.bucket, "bucket.example:9000");
    assert_eq!(parsed.prefix, "prefix");
}

/// Go `filepath.Abs` cleans `.`/`..` lexically and does not require the path to exist.
#[test]
fn test_relative_local_path_is_lexically_cleaned() {
    let raw = "target/objstore-parse-missing/../objstore-parse-result";
    let expected = std::env::current_dir()
        .unwrap()
        .join("target/objstore-parse-result")
        .to_string_lossy()
        .into_owned();
    let StorageBackend::Local(local) = ParseBackend(raw, None).unwrap() else {
        panic!("expected local backend");
    };
    assert_eq!(local.path, expected);
}

/// `IsLocalPath`：本地相对/绝对/`file`/`local` 为真，s3 为假。
#[test]
fn test_is_local() {
    let invalid = IsLocalPath(":").unwrap_err();
    assert!(invalid.to_string().contains("relative URL without a base"));

    for (path, expected) in [
        ("~/tmp/file", true),
        (".", true),
        ("..", true),
        ("./tmp/file", true),
        ("/tmp/file", true),
        ("local:///tmp/file", true),
        ("file:///tmp/file", true),
        ("s3://bucket/tmp/file", false),
    ] {
        assert_eq!(IsLocalPath(path).unwrap(), expected, "path: {path}");
    }
}

/// profile 可来自 URL query 或 BackendOptions，并可与 region/endpoint 并存。
#[test]
fn test_s3_profile_option() {
    let profile = "my-test-profile";
    let parsed = s3(ParseBackend(
        &format!("s3://bucket/prefix/?profile={}", query_escape(profile)),
        None,
    )
    .unwrap());
    assert_eq!(parsed.bucket, "bucket");
    assert_eq!(parsed.prefix, "prefix");
    assert_eq!(parsed.profile, profile);

    let options = BackendOptions {
        s3: S3BackendOptions {
            profile: "profile-from-options".to_owned(),
            ..S3BackendOptions::default()
        },
        ..BackendOptions::default()
    };
    let parsed = s3(ParseBackend("s3://bucket2/prefix/", Some(&options)).unwrap());
    assert_eq!(parsed.bucket, "bucket2");
    assert_eq!(parsed.prefix, "prefix");
    assert_eq!(parsed.profile, "profile-from-options");

    let parsed = s3(ParseBackend(
        "s3://bucket3/prefix/?profile=dev-profile&region=us-west-2&endpoint=https://s3.example.com",
        None,
    )
    .unwrap());
    assert_eq!(parsed.bucket, "bucket3");
    assert_eq!(parsed.prefix, "prefix");
    assert_eq!(parsed.profile, "dev-profile");
    assert_eq!(parsed.region, "us-west-2");
    assert_eq!(parsed.endpoint, "https://s3.example.com");

    let parsed = s3(ParseBackend("s3://bucket4/prefix/", None).unwrap());
    assert_eq!(parsed.bucket, "bucket4");
    assert_eq!(parsed.prefix, "prefix");
    assert!(parsed.profile.is_empty());
}

/// 指定 profile 时允许缺省 AK/SK；也可与显式 access-key 共存。
#[test]
fn test_s3_profile_credentials_validation() {
    let parsed = s3(ParseBackend(
        "s3://bucket/prefix/?profile=production&region=us-west-2",
        None,
    )
    .unwrap());
    assert_eq!(parsed.profile, "production");
    assert_eq!(parsed.region, "us-west-2");
    assert!(parsed.access_key.is_empty());
    assert!(parsed.secret_access_key.is_empty());

    let parsed = s3(ParseBackend(
        "s3://bucket/prefix/?profile=dev&access-key=override-key",
        None,
    )
    .unwrap());
    assert_eq!(parsed.profile, "dev");
    assert_eq!(parsed.access_key, "override-key");
    assert!(parsed.secret_access_key.is_empty());

    let parsed = s3(
        ParseBackend(
            "s3://bucket/prefix/?profile=staging&access-key=explicit-access&secret-access-key=explicit-secret",
            None,
        )
        .unwrap(),
    );
    assert_eq!(parsed.profile, "staging");
    assert_eq!(parsed.access_key, "explicit-access");
    assert_eq!(parsed.secret_access_key, "explicit-secret");
}

/// 无 profile 时 AK/SK 必须成对；仅 region 时允许空凭证。
#[test]
fn test_s3_no_profile_credentials_validation() {
    let options = BackendOptions {
        s3: S3BackendOptions {
            access_key: "only-access-key".to_owned(),
            ..S3BackendOptions::default()
        },
        ..BackendOptions::default()
    };
    let error = ParseBackend("s3://bucket/prefix/", Some(&options)).unwrap_err();
    assert!(error.to_string().contains("secret_access_key not found"));

    let options = BackendOptions {
        s3: S3BackendOptions {
            access_key: "test-access".to_owned(),
            secret_access_key: "test-secret".to_owned(),
            ..S3BackendOptions::default()
        },
        ..BackendOptions::default()
    };
    let parsed = s3(ParseBackend("s3://bucket/prefix/", Some(&options)).unwrap());
    assert_eq!(parsed.access_key, "test-access");
    assert_eq!(parsed.secret_access_key, "test-secret");

    let options = BackendOptions {
        s3: S3BackendOptions {
            region: "us-east-1".to_owned(),
            ..S3BackendOptions::default()
        },
        ..BackendOptions::default()
    };
    let parsed = s3(ParseBackend("s3://bucket/prefix/", Some(&options)).unwrap());
    assert!(parsed.access_key.is_empty());
    assert!(parsed.secret_access_key.is_empty());
}

/// ParseBackend 不回写 options；URL query 覆盖 options 中同名字段。
#[test]
fn test_parse_backend() {
    let options = BackendOptions::default();
    ParseBackend(
        "s3://bucket3/prefix/path?endpoint=https://127.0.0.1:9000&force_path_style=0&sse-kms-key-id=TestKey&xyz=abc",
        Some(&options),
    )
    .unwrap();
    assert!(options.s3.sse_kms_key_id.is_empty());
    ParseBackend(
        "gcs://bucket?endpoint=http://127.0.0.1&predefined-acl=1234",
        Some(&options),
    )
    .unwrap();
    assert!(options.gcs.predefined_acl.is_empty());
    ParseBackend(
        "azure://bucket1/prefix/path?account-name=user&account-key=cGFzc3dk&endpoint=http://127.0.0.1/user&encryption-scope=test",
        Some(&options),
    )
    .unwrap();
    assert!(options.azblob.encryption_scope.is_empty());

    let options = BackendOptions {
        s3: S3BackendOptions {
            storage_class: "test".to_owned(),
            ..S3BackendOptions::default()
        },
        gcs: GCSBackendOptions {
            storage_class: "test".to_owned(),
            ..GCSBackendOptions::default()
        },
        azblob: AzblobBackendOptions {
            access_tier: "test".to_owned(),
            ..AzblobBackendOptions::default()
        },
    };
    let parsed = s3(
        ParseBackend(
            "s3://bucket3/prefix/path?endpoint=https://127.0.0.1:9000&force_path_style=0&sse-kms-key-id=TestKey&xyz=abc",
            Some(&options),
        )
        .unwrap(),
    );
    assert_eq!(options.s3.storage_class, "test");
    assert_eq!(
        parsed,
        S3 {
            endpoint: "https://127.0.0.1:9000".to_owned(),
            bucket: "bucket3".to_owned(),
            prefix: "prefix/path".to_owned(),
            storage_class: "test".to_owned(),
            sse_kms_key_id: "TestKey".to_owned(),
            ..S3::default()
        }
    );
    let parsed = gcs(ParseBackend(
        "gcs://bucket?endpoint=http://127.0.0.1&predefined-acl=1234",
        Some(&options),
    )
    .unwrap());
    assert_eq!(options.gcs.storage_class, "test");
    assert_eq!(
        parsed,
        Gcs {
            endpoint: "http://127.0.0.1".to_owned(),
            bucket: "bucket".to_owned(),
            storage_class: "test".to_owned(),
            predefined_acl: "1234".to_owned(),
            ..Gcs::default()
        }
    );
    let parsed = azure(
        ParseBackend(
            "azure://bucket1/prefix/path?account-name=user&account-key=cGFzc3dk&endpoint=http://127.0.0.1/user&encryption-scope=test",
            Some(&options),
        )
        .unwrap(),
    );
    assert_eq!(options.azblob.access_tier, "test");
    assert_eq!(
        parsed,
        AzureBlobStorage {
            endpoint: "http://127.0.0.1/user".to_owned(),
            bucket: "bucket1".to_owned(),
            prefix: "prefix/path".to_owned(),
            storage_class: "test".to_owned(),
            account_name: "user".to_owned(),
            shared_key: "cGFzc3dk".to_owned(),
            encryption_scope: "test".to_owned(),
            ..AzureBlobStorage::default()
        }
    );

    assert_eq!(
        s3(
            ParseBackend(
                "s3://bucket3/prefix/path?endpoint=https://127.0.0.1:9000&force_path_style=0&sse-kms-key-id=TestKey&xyz=abc",
                None,
            )
            .unwrap(),
        ),
        S3 {
            endpoint: "https://127.0.0.1:9000".to_owned(),
            bucket: "bucket3".to_owned(),
            prefix: "prefix/path".to_owned(),
            sse_kms_key_id: "TestKey".to_owned(),
            ..S3::default()
        }
    );
    assert_eq!(
        s3(
            ParseBackend(
                "s3://bucket3/prefix/path?endpoint=https://127.0.0.1:9000&sse-kms-key-id=TestKey&xyz=abc",
                None,
            )
            .unwrap(),
        ),
        S3 {
            endpoint: "https://127.0.0.1:9000".to_owned(),
            bucket: "bucket3".to_owned(),
            prefix: "prefix/path".to_owned(),
            force_path_style: true,
            sse_kms_key_id: "TestKey".to_owned(),
            ..S3::default()
        }
    );
    assert_eq!(
        gcs(ParseBackend(
            "gcs://bucket?endpoint=http://127.0.0.1&predefined-acl=1234",
            None,
        )
        .unwrap(),),
        Gcs {
            endpoint: "http://127.0.0.1".to_owned(),
            bucket: "bucket".to_owned(),
            predefined_acl: "1234".to_owned(),
            ..Gcs::default()
        }
    );
    assert_eq!(
        azure(
            ParseBackend(
                "azure://bucket1/prefix/path?account-name=user&account-key=cGFzc3dk&endpoint=http://127.0.0.1/user&encryption-scope=test",
                None,
            )
            .unwrap(),
        ),
        AzureBlobStorage {
            endpoint: "http://127.0.0.1/user".to_owned(),
            bucket: "bucket1".to_owned(),
            prefix: "prefix/path".to_owned(),
            account_name: "user".to_owned(),
            shared_key: "cGFzc3dk".to_owned(),
            encryption_scope: "test".to_owned(),
            ..AzureBlobStorage::default()
        }
    );
}

/// AWS endpoint 或显式 false 关闭 path-style；显式 true 则开启。
#[test]
fn test_s3_default_force_style_path() {
    assert!(
        !s3(ParseBackend(
            "s3://bucket3/prefix/path?endpoint=http://xxx.amazonaws.com",
            None,
        )
        .unwrap(),)
        .force_path_style
    );
    assert!(
        !s3(ParseBackend("s3://bucket3/prefix/path?force-path-style=false", None,).unwrap(),)
            .force_path_style
    );
    assert!(
        s3(ParseBackend("s3://bucket3/prefix/path?force-path-style=true", None,).unwrap(),)
            .force_path_style
    );
}
