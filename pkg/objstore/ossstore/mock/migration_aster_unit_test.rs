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

// ossstore mock 子包迁移对齐单元测试。
//
// 验证 `MockAPI` 参数转发/错误传播/完整 Go 方法集覆盖，
// 以及凭证提供者 mock 的零参方法行为。

use super::api_mock::{
    Api, Context, GetObjectRequest, GetObjectResult, OssError, PutObjectRequest, PutObjectResult,
};
use super::provider_mock::{Credentials, CredentialsProvider};

/// 配置 IsBucketExist/GetObject 期望，断言参数与返回值正确转发。
#[test]
fn api_mock_forwards_arguments_and_returns_configured_results() {
    let mut mock = super::api_mock::NewMockAPI();
    mock.EXPECT()
        .expect_IsBucketExist()
        .withf(|ctx, request, options| {
            ctx.request_id == "request-1" && request == "bucket-a" && options.len() == 2
        })
        .times(1)
        .return_once(|_, _, _| Ok(true));
    mock.EXPECT()
        .expect_GetObject()
        .withf(|_, request, options| request.key == "source/key" && options.len() == 1)
        .times(1)
        .return_once(|_, _, _| {
            Ok(GetObjectResult {
                body: b"payload".to_vec(),
            })
        });

    let ctx = Context {
        request_id: "request-1".into(),
    };
    let options = vec![Default::default(), Default::default()];
    assert_eq!(mock.IsBucketExist(&ctx, "bucket-a", &options,), Ok(true));
    assert_eq!(
        mock.GetObject(
            &ctx,
            &GetObjectRequest {
                key: "source/key".into()
            },
            &options[..1],
        ),
        Ok(GetObjectResult {
            body: b"payload".to_vec()
        })
    );
}

/// PutObject 配置返回错误时，调用方收到同等 `OssError`。
#[test]
fn api_mock_propagates_errors_and_checks_call_count() {
    let mut mock = super::api_mock::NewMockAPI();
    mock.EXPECT()
        .expect_PutObject()
        .withf(|_, request, options| request.key == "dest/key" && options.is_empty())
        .times(1)
        .return_once(|_, _, _| Err(OssError::new("denied")));

    let result: Result<PutObjectResult, OssError> = mock.PutObject(
        &Context::default(),
        &PutObjectRequest {
            key: "dest/key".into(),
            body: b"value".to_vec(),
        },
        &[],
    );
    assert_eq!(result, Err(OssError::new("denied")));
}

/// 对其余 Go 方法集各调用一次，确保 mock 生成完整。
#[test]
fn api_mock_covers_the_complete_go_method_set() {
    use super::api_mock::*;

    let mut mock = NewMockAPI();
    mock.EXPECT()
        .expect_AbortMultipartUpload()
        .times(1)
        .return_once(|_, _, _| Ok(Default::default()));
    mock.EXPECT()
        .expect_CompleteMultipartUpload()
        .times(1)
        .return_once(|_, _, _| Ok(Default::default()));
    mock.EXPECT()
        .expect_CopyObject()
        .times(1)
        .return_once(|_, _, _| Ok(Default::default()));
    mock.EXPECT()
        .expect_DeleteMultipleObjects()
        .times(1)
        .return_once(|_, _, _| Ok(Default::default()));
    mock.EXPECT()
        .expect_DeleteObject()
        .times(1)
        .return_once(|_, _, _| Ok(Default::default()));
    mock.EXPECT()
        .expect_HeadObject()
        .times(1)
        .return_once(|_, _, _| Ok(Default::default()));
    mock.EXPECT()
        .expect_InitiateMultipartUpload()
        .times(1)
        .return_once(|_, _, _| Ok(Default::default()));
    mock.EXPECT()
        .expect_ListObjectsV2()
        .times(1)
        .return_once(|_, _, _| Ok(Default::default()));
    mock.EXPECT()
        .expect_ListParts()
        .times(1)
        .return_once(|_, _, _| Ok(Default::default()));
    mock.EXPECT()
        .expect_UploadPart()
        .times(1)
        .return_once(|_, _, _| Ok(Default::default()));

    let ctx = Context::default();
    let options = [OptionFn::default()];
    assert_eq!(
        mock.AbortMultipartUpload(&ctx, &Default::default(), &options),
        Ok(Default::default())
    );
    assert_eq!(
        mock.CompleteMultipartUpload(&ctx, &Default::default(), &options),
        Ok(Default::default())
    );
    assert_eq!(
        mock.CopyObject(&ctx, &Default::default(), &options),
        Ok(Default::default())
    );
    assert_eq!(
        mock.DeleteMultipleObjects(&ctx, &Default::default(), &options),
        Ok(Default::default())
    );
    assert_eq!(
        mock.DeleteObject(&ctx, &Default::default(), &options),
        Ok(Default::default())
    );
    assert_eq!(
        mock.HeadObject(&ctx, &Default::default(), &options),
        Ok(Default::default())
    );
    assert_eq!(
        mock.InitiateMultipartUpload(&ctx, &Default::default(), &options),
        Ok(Default::default())
    );
    assert_eq!(
        mock.ListObjectsV2(&ctx, &Default::default(), &options),
        Ok(Default::default())
    );
    assert_eq!(
        mock.ListParts(&ctx, &Default::default(), &options),
        Ok(Default::default())
    );
    assert_eq!(
        mock.UploadPart(&ctx, &Default::default(), &options),
        Ok(Default::default())
    );
    mock.ISGOMOCK();
}

/// CredentialsProvider mock：GetProviderName / GetCredentials 零参方法。
#[test]
fn credentials_provider_mock_matches_go_zero_argument_methods() {
    let mut mock = super::provider_mock::NewMockCredentialsProvider();
    mock.EXPECT()
        .expect_GetCredentials()
        .times(1)
        .return_once(|| {
            Ok(Credentials {
                access_key_id: "ak".into(),
                access_key_secret: "sk".into(),
                security_token: "token".into(),
            })
        });
    mock.EXPECT()
        .expect_GetProviderName()
        .times(1)
        .return_const("ram-role".to_owned());

    assert_eq!(mock.GetProviderName(), "ram-role");
    assert_eq!(mock.GetCredentials().unwrap().access_key_id, "ak");
    mock.ISGOMOCK();
}
