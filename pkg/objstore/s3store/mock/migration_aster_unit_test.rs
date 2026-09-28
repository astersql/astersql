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

// S3API mock 的迁移单元测试。
//
// 验证 mockall 生成的 `MockS3API` 能正确转发参数/选项、传播错误，
// 并覆盖 Go `S3API` 接口中的完整方法集合（分片上传、复制、列举等）。

use super::{
    AbortMultipartUploadInput, AbortMultipartUploadOutput, CompleteMultipartUploadInput,
    CompleteMultipartUploadOutput, Context, CopyObjectInput, CopyObjectOutput,
    CreateMultipartUploadInput, CreateMultipartUploadOutput, DeleteObjectInput, DeleteObjectOutput,
    DeleteObjectsInput, DeleteObjectsOutput, GetObjectInput, GetObjectLockConfigurationInput,
    GetObjectLockConfigurationOutput, GetObjectOutput, HeadBucketInput, HeadBucketOutput,
    HeadObjectInput, HeadObjectOutput, ListObjectsInput, ListObjectsOutput, ListObjectsV2Input,
    ListObjectsV2Output, NewMockS3API, OptionFn, PutObjectInput, S3Api, S3Error, UploadPartInput,
    UploadPartOutput,
};

/// 构造启用 path-style 寻址的选项回调（对应 Go 的 `func(*s3.Options)`）。
fn option() -> OptionFn {
    OptionFn::new(|options| options.force_path_style = true)
}

/// 期望：GetObject 校验 ctx/input/options，并返回配置好的 ContentLength。
#[test]
fn mock_forwards_arguments_options_and_configured_output() {
    let mut mock = NewMockS3API();
    mock.EXPECT()
        .expect_GetObject()
        .withf(|ctx, input, options| {
            ctx.request_id == "request-1"
                && input.bucket() == Some("bucket-a")
                && input.key() == Some("object-a")
                && options.len() == 2
        })
        .times(1)
        .return_once(|_, _, _| Ok(GetObjectOutput::builder().content_length(7).build()));

    let input = GetObjectInput::builder()
        .bucket("bucket-a")
        .key("object-a")
        .build()
        .expect("valid get-object input");
    let output = mock
        .GetObject(&Context::new("request-1"), &input, &[option(), option()])
        .expect("configured call succeeds");

    assert_eq!(output.content_length(), Some(7));
    mock.ISGOMOCK();
}

/// 期望：PutObject 按配置返回错误，并校验入参与空 options。
#[test]
fn mock_propagates_errors_and_checks_call_count() {
    let mut mock = NewMockS3API();
    mock.EXPECT()
        .expect_PutObject()
        .withf(|_, input, options| {
            input.bucket() == Some("bucket-a")
                && input.key() == Some("denied")
                && options.is_empty()
        })
        .times(1)
        .return_once(|_, _, _| Err(S3Error::new("access denied")));

    let input = PutObjectInput::builder()
        .bucket("bucket-a")
        .key("denied")
        .build()
        .expect("valid put-object input");
    assert_eq!(
        mock.PutObject(&Context::default(), &input, &[]),
        Err(S3Error::new("access denied"))
    );
}

/// 逐一调用 Go S3API 方法集中除 Get/Put 外的其余操作，确认 mock 均已接线。
#[test]
fn mock_covers_complete_go_method_set() {
    let mut mock = NewMockS3API();
    // 为每个方法注册一次成功返回，覆盖分片、复制、删除、列举、Head 等路径。
    mock.EXPECT()
        .expect_AbortMultipartUpload()
        .return_once(|_, _, _| Ok(AbortMultipartUploadOutput::builder().build()));
    mock.EXPECT()
        .expect_CompleteMultipartUpload()
        .return_once(|_, _, _| Ok(CompleteMultipartUploadOutput::builder().build()));
    mock.EXPECT()
        .expect_CopyObject()
        .return_once(|_, _, _| Ok(CopyObjectOutput::builder().build()));
    mock.EXPECT()
        .expect_CreateMultipartUpload()
        .return_once(|_, _, _| Ok(CreateMultipartUploadOutput::builder().build()));
    mock.EXPECT()
        .expect_DeleteObject()
        .return_once(|_, _, _| Ok(DeleteObjectOutput::builder().build()));
    mock.EXPECT()
        .expect_DeleteObjects()
        .return_once(|_, _, _| Ok(DeleteObjectsOutput::builder().build()));
    mock.EXPECT()
        .expect_GetObjectLockConfiguration()
        .return_once(|_, _, _| Ok(GetObjectLockConfigurationOutput::builder().build()));
    mock.EXPECT()
        .expect_HeadBucket()
        .return_once(|_, _, _| Ok(HeadBucketOutput::builder().build()));
    mock.EXPECT()
        .expect_HeadObject()
        .return_once(|_, _, _| Ok(HeadObjectOutput::builder().build()));
    mock.EXPECT()
        .expect_ListObjects()
        .return_once(|_, _, _| Ok(ListObjectsOutput::builder().build()));
    mock.EXPECT()
        .expect_ListObjectsV2()
        .return_once(|_, _, _| Ok(ListObjectsV2Output::builder().build()));
    mock.EXPECT()
        .expect_UploadPart()
        .return_once(|_, _, _| Ok(UploadPartOutput::builder().build()));

    let ctx = Context::default();
    let options = [option()];
    assert!(
        mock.AbortMultipartUpload(
            &ctx,
            &AbortMultipartUploadInput::builder()
                .bucket("b")
                .key("k")
                .upload_id("u")
                .build()
                .unwrap(),
            &options
        )
        .is_ok()
    );
    assert!(
        mock.CompleteMultipartUpload(
            &ctx,
            &CompleteMultipartUploadInput::builder()
                .bucket("b")
                .key("k")
                .upload_id("u")
                .build()
                .unwrap(),
            &options
        )
        .is_ok()
    );
    assert!(
        mock.CopyObject(
            &ctx,
            &CopyObjectInput::builder()
                .bucket("b")
                .key("k")
                .copy_source("source")
                .build()
                .unwrap(),
            &options
        )
        .is_ok()
    );
    assert!(
        mock.CreateMultipartUpload(
            &ctx,
            &CreateMultipartUploadInput::builder()
                .bucket("b")
                .key("k")
                .build()
                .unwrap(),
            &options
        )
        .is_ok()
    );
    assert!(
        mock.DeleteObject(
            &ctx,
            &DeleteObjectInput::builder()
                .bucket("b")
                .key("k")
                .build()
                .unwrap(),
            &options
        )
        .is_ok()
    );
    let delete = aws_sdk_s3::types::Delete::builder()
        .objects(
            aws_sdk_s3::types::ObjectIdentifier::builder()
                .key("k")
                .build()
                .unwrap(),
        )
        .build()
        .unwrap();
    assert!(
        mock.DeleteObjects(
            &ctx,
            &DeleteObjectsInput::builder()
                .bucket("b")
                .delete(delete)
                .build()
                .unwrap(),
            &options
        )
        .is_ok()
    );
    assert!(
        mock.GetObjectLockConfiguration(
            &ctx,
            &GetObjectLockConfigurationInput::builder()
                .bucket("b")
                .build()
                .unwrap(),
            &options
        )
        .is_ok()
    );
    assert!(
        mock.HeadBucket(
            &ctx,
            &HeadBucketInput::builder().bucket("b").build().unwrap(),
            &options
        )
        .is_ok()
    );
    assert!(
        mock.HeadObject(
            &ctx,
            &HeadObjectInput::builder()
                .bucket("b")
                .key("k")
                .build()
                .unwrap(),
            &options
        )
        .is_ok()
    );
    assert!(
        mock.ListObjects(
            &ctx,
            &ListObjectsInput::builder().bucket("b").build().unwrap(),
            &options
        )
        .is_ok()
    );
    assert!(
        mock.ListObjectsV2(
            &ctx,
            &ListObjectsV2Input::builder().bucket("b").build().unwrap(),
            &options
        )
        .is_ok()
    );
    assert!(
        mock.UploadPart(
            &ctx,
            &UploadPartInput::builder()
                .bucket("b")
                .key("k")
                .part_number(1)
                .upload_id("u")
                .build()
                .unwrap(),
            &options
        )
        .is_ok()
    );
}
