// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Keyspace 迁移对齐单元测试：核对 etcd 命名空间、API 上下文、配置与用户名策略。
//
// 与 Go 行为逐项对比；修改全局配置/部署模式的用例使用 `serial` 串行化，
// 避免并行测试互相污染共享状态。

use crate::*;
use exeerrors_dependency::{errno::ErrUsername, errors, exeerrors::ErrUserNameNeedPrefix};
use serial_test::serial;

/// 测试用编解码器：可指定 API 版本与 keyspace ID。
#[derive(Clone, Debug, Eq, PartialEq)]
struct TestCodec {
    /// API 版本（V1/V2）。
    api_version: ApiVersion,
    /// Keyspace 数值 ID。
    keyspace_id: u32,
}

impl Codec for TestCodec {
    fn api_version(&self) -> ApiVersion {
        self.api_version
    }

    fn keyspace_id(&self) -> u32 {
        self.keyspace_id
    }
}

/// 测试用日志核心：记录 `with_field` 追加的键值对。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TestCore(Vec<(String, String)>);

impl LogCore for TestCore {
    fn with_field(mut self, key: &str, value: &str) -> Self {
        self.0.push((key.to_owned(), value.to_owned()));
        self
    }
}

/// V1 命名空间为空，V2 拼出 `/keyspaces/tidb/{id}`；空名称构建 V1 API 上下文。
#[test]
fn namespace_and_api_context_follow_go_versions() {
    let v1 = TestCodec {
        api_version: ApiVersion::V1,
        keyspace_id: 42,
    };
    let v2 = TestCodec {
        api_version: ApiVersion::V2,
        keyspace_id: 42,
    };

    assert_eq!(MakeKeyspaceEtcdNamespace(&v1), "");
    assert_eq!(MakeKeyspaceEtcdNamespaceSlash(&v1), "");
    assert_eq!(MakeKeyspaceEtcdNamespace(&v2), "/keyspaces/tidb/42");
    assert_eq!(MakeKeyspaceEtcdNamespaceSlash(&v2), "/keyspaces/tidb/42/");
    assert_eq!(BuildAPIContext(""), ApiContext::V1);
    assert_eq!(
        BuildAPIContext("tenant"),
        ApiContext::V2("tenant".to_owned())
    );
}

/// 全局 keyspace 配置应反映到名称读取、字节缓存与日志字段注入。
#[test]
#[serial]
fn settings_and_log_fields_follow_global_keyspace() {
    let restore = config::restore_func();
    config::update_global(|conf| conf.keyspace_name = "ks-log".to_owned());

    assert_eq!(GetKeyspaceNameBySettings(), "ks-log");
    assert!(!IsKeyspaceNameEmpty(&GetKeyspaceNameBySettings()));
    let expected_bytes: &[u8] = if kerneltype::IsNextGen() {
        b"ks-log"
    } else {
        b""
    };
    assert_eq!(GetKeyspaceNameBytesBySettings(), expected_bytes);
    assert_eq!(
        WrapZapcoreWithKeyspace(TestCore::default()),
        TestCore(vec![("keyspaceName".to_owned(), "ks-log".to_owned())])
    );

    restore();
}

/// Premium 走默认策略；Starter 强制 keyspace 前缀，变体与去前缀与 Go 一致。
#[test]
#[serial]
fn username_policies_match_default_and_starter_go_behavior() {
    let restore = config::restore_func();
    let original_mode = deploymode::Get();
    config::update_global(|conf| conf.keyspace_name = "ks".to_owned());

    // Premium：默认策略，不校验前缀。
    if kerneltype::IsNextGen() {
        deploymode::Set(deploymode::Premium).unwrap();
    } else {
        assert!(deploymode::Set(deploymode::Premium).is_err());
    }
    let policy = GetUsernamePolicy();
    assert!(policy.ValidateUsername("user").is_ok());
    assert!(policy.GetUsernameVariants("user").is_empty());
    assert_eq!(policy.GetOriginalUsername("ks.user"), "");

    if kerneltype::IsNextGen() {
        // Starter：必须带 `ks.` 前缀；格式、变体、原始用户名与 Go 对齐。
        deploymode::Set(deploymode::Starter).unwrap();
        let policy = GetUsernamePolicy();
        assert!(policy.ValidateUsername("ks.user").is_ok());
        assert!(policy.ValidateUsernameFormat("other.user"));
        assert!(!policy.ValidateUsernameFormat("other.user.extra"));
        let error = policy.ValidateUsername("user").unwrap_err();
        assert!(
            ErrUserNameNeedPrefix.Equal(Some(&error)),
            "username error must equal the Go ErrUserNameNeedPrefix prototype"
        );
        let root = errors::Cause(Some(&error)).expect("username error must have a root cause");
        let normalized = root
            .downcast_ref::<errors::Error>()
            .expect("username error must retain its normalized DDL class and MySQL code");
        assert_eq!(normalized.Code(), ErrUsername as i32);
        assert_eq!(normalized.RFCCode(), format!("ddl:{ErrUsername}"));
        assert_eq!(
            normalized.GetMsg(),
            "User name must start with `ks.` (use `ks.user` instead)"
        );
        assert_eq!(policy.GetUsernameVariants("user"), vec!["ks.user"]);
        assert!(policy.GetUsernameVariants("ks.user").is_empty());
        assert_eq!(
            policy.GetUsernameVariants("other.user.extra"),
            vec!["ks.other.user.extra"]
        );
        assert_eq!(policy.GetOriginalUsername("ks.user"), "user");
        assert_eq!(policy.GetOriginalUsername("other.user"), "");

        deploymode::Set(original_mode).unwrap();
    } else {
        assert!(deploymode::Set(deploymode::Starter).is_err());
    }
    restore();
}
