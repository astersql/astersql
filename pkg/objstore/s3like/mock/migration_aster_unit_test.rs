// Copyright 2025 PingCAP, Inc.
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

// s3like mock 包迁移对齐单元测试。
//
// 验证 MockPrefixClient 能转发权限/变更方法、按期望返回值与错误，
// 以及配置 MultipartWriter/Uploader 组件。

use std::io::{self, Cursor, Read};
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use s3like::{
    CopyInput, GetResp, HeadObjectResp, ListResp, Object, PrefixClient, ReadCloser, Uploader,
};
use storeapi::{BucketPrefix, Context};

use super::client_mock::NewMockPrefixClient;

/// 测试用可读可关闭正文。
struct TestReadCloser(Cursor<Vec<u8>>);

impl Read for TestReadCloser {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl ReadCloser for TestReadCloser {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 收集写入字节的测试 Writer。
#[derive(Default)]
struct TestWriter {
    written: Vec<u8>,
}

impl objectio::Writer for TestWriter {
    fn write(&mut self, _ctx: &objectio::Context, data: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(data);
        Ok(data.len())
    }

    fn close(&mut self, _ctx: &objectio::Context) -> io::Result<()> {
        Ok(())
    }
}

/// 把上传流内容写入共享缓冲的 Uploader。
struct TestUploader(Arc<Mutex<Vec<u8>>>);

impl Uploader for TestUploader {
    fn Upload(&self, _ctx: &Context, reader: &mut dyn Read) -> anyhow::Result<()> {
        reader.read_to_end(&mut self.0.lock().unwrap())?;
        Ok(())
    }
}

/// 权限探测与变更类方法可按期望被调用并成功返回。
#[test]
fn prefix_client_mock_forwards_go_permission_and_mutation_methods() {
    let mut mock = NewMockPrefixClient();
    mock.EXPECT()
        .expect_CheckBucketExistence()
        .times(1)
        .return_once(|_| Ok(()));
    mock.EXPECT()
        .expect_CheckGetObject()
        .times(1)
        .return_once(|_| Ok(()));
    mock.EXPECT()
        .expect_CheckListObjects()
        .times(1)
        .return_once(|_| Ok(()));
    mock.EXPECT()
        .expect_CheckPutAndDeleteObject()
        .times(1)
        .return_once(|_| Ok(()));
    mock.EXPECT()
        .expect_CopyObject()
        .withf(|_, input| input.FromKey == "source/key" && input.ToKey == "dest/key")
        .times(1)
        .return_once(|_, _| Ok(()));
    mock.EXPECT()
        .expect_DeleteObject()
        .withf(|_, name| name == "single")
        .times(1)
        .return_once(|_, _| Ok(()));
    mock.EXPECT()
        .expect_DeleteObjects()
        .withf(|_, names| names == ["first".to_owned(), "second".to_owned()])
        .times(1)
        .return_once(|_, _| Ok(()));
    mock.EXPECT()
        .expect_PutObject()
        .withf(|_, name, data| name == "written" && data == b"payload")
        .times(1)
        .return_once(|_, _, _| Ok(()));

    let ctx = Context::default();
    mock.CheckBucketExistence(&ctx).unwrap();
    mock.CheckGetObject(&ctx).unwrap();
    mock.CheckListObjects(&ctx).unwrap();
    mock.CheckPutAndDeleteObject(&ctx).unwrap();
    mock.CopyObject(
        &ctx,
        &CopyInput {
            FromLoc: BucketPrefix::default(),
            FromKey: "source/key".into(),
            ToKey: "dest/key".into(),
        },
    )
    .unwrap();
    mock.DeleteObject(&ctx, "single").unwrap();
    mock.DeleteObjects(&ctx, &["first".into(), "second".into()])
        .unwrap();
    mock.PutObject(&ctx, "written", b"payload").unwrap();
    mock.ISGOMOCK();
}

/// Get/Head/List/Exists 返回配置值，Delete 可传播错误。
#[test]
fn prefix_client_mock_returns_values_and_propagates_errors() {
    let mut mock = NewMockPrefixClient();
    mock.EXPECT()
        .expect_GetObject()
        .withf(|_, name, start, end| name == "range" && *start == 2 && *end == 8)
        .times(1)
        .return_once(|_, _, _, _| {
            Ok(Some(GetResp {
                Body: Box::new(TestReadCloser(Cursor::new(b"content".to_vec()))),
                IsFullRange: false,
                ContentLength: Some(7),
                ContentRange: Some("bytes 2-8/10".into()),
            }))
        });
    mock.EXPECT()
        .expect_HeadObject()
        .withf(|_, name| name == "head")
        .times(1)
        .return_once(|_, _| {
            Ok(Some(HeadObjectResp {
                ReplicationStatus: "COMPLETED".into(),
            }))
        });
    mock.EXPECT()
        .expect_IsObjectExists()
        .withf(|_, name| name == "present")
        .times(1)
        .return_once(|_, _| Ok(true));
    mock.EXPECT()
        .expect_ListObjects()
        .withf(|_, prefix, start, token, max| {
            prefix == "extra/" && start == "after" && *token == Some("token") && *max == 25
        })
        .times(1)
        .return_once(|_, _, _, _, _| {
            Ok(Some(ListResp {
                NextContinuationToken: Some("next".into()),
                IsTruncated: true,
                Objects: vec![Object {
                    Key: "extra/object".into(),
                    Size: 42,
                }],
            }))
        });
    mock.EXPECT()
        .expect_DeleteObject()
        .withf(|_, name| name == "denied")
        .times(1)
        .return_once(|_, _| Err(anyhow!("access denied")));

    let ctx = Context::default();
    let mut get = mock.GetObject(&ctx, "range", 2, 8).unwrap().unwrap();
    let mut body = String::new();
    get.Body.read_to_string(&mut body).unwrap();
    assert_eq!(body, "content");
    assert_eq!(get.ContentRange.as_deref(), Some("bytes 2-8/10"));
    assert_eq!(
        mock.HeadObject(&ctx, "head")
            .unwrap()
            .unwrap()
            .ReplicationStatus,
        "COMPLETED"
    );
    assert!(mock.IsObjectExists(&ctx, "present").unwrap());
    let list = mock
        .ListObjects(&ctx, "extra/", "after", Some("token"), 25)
        .unwrap()
        .unwrap();
    assert!(list.IsTruncated);
    assert_eq!(list.Objects[0].Key, "extra/object");
    assert_eq!(
        mock.DeleteObject(&ctx, "denied").unwrap_err().to_string(),
        "access denied"
    );
}

/// MultipartWriter/Uploader 返回测试替身并可实际写入/上传。
#[test]
fn prefix_client_mock_returns_configured_multipart_components() {
    let uploaded = Arc::new(Mutex::new(Vec::new()));
    let uploaded_for_mock = Arc::clone(&uploaded);
    let mut mock = NewMockPrefixClient();
    mock.EXPECT()
        .expect_MultipartWriter()
        .withf(|_, name| name == "writer-key")
        .times(1)
        .return_once(|_, _| Ok(Some(Box::new(TestWriter::default()))));
    mock.EXPECT()
        .expect_MultipartUploader()
        .withf(|name, part_size, concurrency| {
            name == "upload-key" && *part_size == 8 << 20 && *concurrency == 4
        })
        .times(1)
        .return_once(move |_, _, _| Some(Box::new(TestUploader(uploaded_for_mock))));

    let ctx = Context::default();
    let mut writer = mock.MultipartWriter(&ctx, "writer-key").unwrap().unwrap();
    assert_eq!(writer.Write(&ctx, b"chunk").unwrap(), 5);
    writer.Close(&ctx).unwrap();

    let uploader = mock.MultipartUploader("upload-key", 8 << 20, 4).unwrap();
    uploader
        .Upload(&ctx, &mut Cursor::new(b"parts".to_vec()))
        .unwrap();
    assert_eq!(&*uploaded.lock().unwrap(), b"parts");
}

/// GoMock can carry every value in the Go interface domain: signed `int`
/// arguments and nil interface/pointer returns paired with a nil error.
#[test]
fn prefix_client_mock_preserves_signed_and_nil_success_values() {
    let mut mock = NewMockPrefixClient();
    mock.EXPECT()
        .expect_GetObject()
        .times(1)
        .return_once(|_, _, _, _| Ok(None));
    mock.EXPECT()
        .expect_HeadObject()
        .times(1)
        .return_once(|_, _| Ok(None));
    mock.EXPECT()
        .expect_ListObjects()
        .withf(|_, _, _, _, max_keys| *max_keys == -7)
        .times(1)
        .return_once(|_, _, _, _, _| Ok(None));
    mock.EXPECT()
        .expect_MultipartWriter()
        .times(1)
        .return_once(|_, _| Ok(None));
    mock.EXPECT()
        .expect_MultipartUploader()
        .times(1)
        .return_once(|_, _, _| None);

    let ctx = Context::default();
    assert!(mock.GetObject(&ctx, "nil-get", 0, 0).unwrap().is_none());
    assert!(mock.HeadObject(&ctx, "nil-head").unwrap().is_none());
    assert!(mock.ListObjects(&ctx, "", "", None, -7).unwrap().is_none());
    assert!(mock.MultipartWriter(&ctx, "nil-writer").unwrap().is_none());
    assert!(mock.MultipartUploader("nil-uploader", 1, 1).is_none());
}
