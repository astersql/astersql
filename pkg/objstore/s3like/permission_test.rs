// Copyright 2026 PingCAP, Inc.
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

// CheckPermissions 集成测试。
//
// 用 MockPrefixClient 验证各权限失败时的错误包装、未知权限拒绝，
// 以及全部权限通过时的成功路径。

// The crate under test is auto-linked by its package name for `[[test]]` targets;
// alias it to the short name used across the porting codebase.
extern crate astersql_objstore_s3like as s3like;

use anyhow::anyhow;
use s3like_mock::client_mock::{MockPrefixClient, NewMockPrefixClient};

/// 单权限失败用例：权限枚举与配置 mock 失败的函数。
struct PermissionCase {
    /// 待校验的权限。
    perm: storeapi::Permission,
    /// 配置对应 Check* 方法返回错误。
    configure_failure: fn(&mut MockPrefixClient),
}

/// 让 CheckBucketExistence 失败一次。
fn fail_bucket(mock: &mut MockPrefixClient) {
    mock.EXPECT()
        .expect_CheckBucketExistence()
        .times(1)
        .return_once(|_| Err(anyhow!("some error")));
}

/// 让 CheckListObjects 失败一次。
fn fail_list(mock: &mut MockPrefixClient) {
    mock.EXPECT()
        .expect_CheckListObjects()
        .times(1)
        .return_once(|_| Err(anyhow!("some error")));
}

/// 让 CheckGetObject 失败一次。
fn fail_get(mock: &mut MockPrefixClient) {
    mock.EXPECT()
        .expect_CheckGetObject()
        .times(1)
        .return_once(|_| Err(anyhow!("some error")));
}

/// 让 CheckPutAndDeleteObject 失败一次。
fn fail_put_and_delete(mock: &mut MockPrefixClient) {
    mock.EXPECT()
        .expect_CheckPutAndDeleteObject()
        .times(1)
        .return_once(|_| Err(anyhow!("some error")));
}

/// 覆盖：各权限失败包装、未知权限、全部成功。
#[test]
fn test_check_permissions() {
    let ctx = storeapi::Context::default();

    for case in [
        PermissionCase {
            perm: storeapi::AccessBuckets,
            configure_failure: fail_bucket,
        },
        PermissionCase {
            perm: storeapi::ListObjects,
            configure_failure: fail_list,
        },
        PermissionCase {
            perm: storeapi::GetObject,
            configure_failure: fail_get,
        },
        PermissionCase {
            perm: storeapi::PutAndDeleteObject,
            configure_failure: fail_put_and_delete,
        },
    ] {
        let mut mock_cli = NewMockPrefixClient();
        (case.configure_failure)(&mut mock_cli);

        let error = s3like::CheckPermissions(&ctx, &mock_cli, &[case.perm]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("permission {}: some error", case.perm.as_str()))
        );
        assert_eq!(
            error.chain().nth(1).map(ToString::to_string).as_deref(),
            Some("some error"),
            "permission context must preserve the underlying error like Go errors.Annotatef"
        );
    }

    let mock_cli = NewMockPrefixClient();
    let error = s3like::CheckPermissions(&ctx, &mock_cli, &[storeapi::PutObject]).unwrap_err();
    assert!(error.to_string().contains("unknown permission: PutObject"));

    let mut mock_cli = NewMockPrefixClient();
    mock_cli
        .EXPECT()
        .expect_CheckBucketExistence()
        .times(1)
        .return_once(|_| Ok(()));
    mock_cli
        .EXPECT()
        .expect_CheckListObjects()
        .times(1)
        .return_once(|_| Ok(()));
    mock_cli
        .EXPECT()
        .expect_CheckGetObject()
        .times(1)
        .return_once(|_| Ok(()));
    mock_cli
        .EXPECT()
        .expect_CheckPutAndDeleteObject()
        .times(1)
        .return_once(|_| Ok(()));

    s3like::CheckPermissions(
        &ctx,
        &mock_cli,
        &[
            storeapi::AccessBuckets,
            storeapi::ListObjects,
            storeapi::GetObject,
            storeapi::PutAndDeleteObject,
        ],
    )
    .unwrap();
}
