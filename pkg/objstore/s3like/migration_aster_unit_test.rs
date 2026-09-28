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

// s3like 迁移对齐单元测试。
//
// 用内存 MockClient 覆盖权限短路、重试规则、范围解析、删除分批、
// WalkDir 分页、读侧重试与 Create 同步/并发上传等行为，校验与 Go 侧一致。

use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;

/// 可成功读取的测试正文。
struct Body(Cursor<Vec<u8>>);

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl ReadCloser for Body {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 每次 read 都失败的正文，用于触发 reader 重试。
struct FailBody;

impl Read for FailBody {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "read: connection reset",
        ))
    }
}

impl ReadCloser for FailBody {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Mock 客户端可变状态。
#[derive(Default)]
struct ClientState {
    /// 下一次权限检查要返回的错误；消费后清空。
    permission_error: Option<&'static str>,
    /// 已调用方法名记录。
    calls: Vec<&'static str>,
    /// 预置的 ListObjects 分页响应队列。
    objects: VecDeque<ListResp>,
    /// DeleteObjects 收到的各批次键名。
    deleted_batches: Vec<Vec<String>>,
    /// HeadObject 返回的复制状态。
    replication_status: String,
    /// GetObject 提供的对象数据。
    data: Vec<u8>,
    /// 下一次 GetObject 是否返回 FailBody。
    fail_next_body: bool,
    /// Put/分片上传累计写入的字节。
    uploaded: Vec<u8>,
}

/// 线程安全的 PrefixClient 测试替身。
#[derive(Clone, Default)]
struct MockClient(Arc<Mutex<ClientState>>);

impl MockClient {
    /// 记录调用并可能返回一次性权限错误。
    fn permission(&self, name: &'static str) -> anyhow::Result<()> {
        let mut state = self.0.lock().unwrap();
        state.calls.push(name);
        match state.permission_error.take() {
            Some(message) => Err(anyhow::anyhow!(message)),
            None => Ok(()),
        }
    }
}

/// 把写入内容收集到 ClientState.uploaded 的 Writer。
struct CollectWriter(Arc<Mutex<ClientState>>);

impl objectio::Writer for CollectWriter {
    fn write(&mut self, _ctx: &objectio::Context, data: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().uploaded.extend_from_slice(data);
        Ok(data.len())
    }

    fn close(&mut self, _ctx: &objectio::Context) -> io::Result<()> {
        Ok(())
    }
}

/// 把可读流内容收集到 ClientState.uploaded 的 Uploader。
struct CollectUploader(Arc<Mutex<ClientState>>);

impl Uploader for CollectUploader {
    fn Upload(&self, _ctx: &storeapi::Context, reader: &mut dyn Read) -> anyhow::Result<()> {
        reader.read_to_end(&mut self.0.lock().unwrap().uploaded)?;
        Ok(())
    }
}

impl PrefixClient for MockClient {
    fn CheckBucketExistence(&self, _ctx: &storeapi::Context) -> anyhow::Result<()> {
        self.permission("bucket")
    }
    fn CheckListObjects(&self, _ctx: &storeapi::Context) -> anyhow::Result<()> {
        self.permission("list")
    }
    fn CheckGetObject(&self, _ctx: &storeapi::Context) -> anyhow::Result<()> {
        self.permission("get")
    }
    fn CheckPutAndDeleteObject(&self, _ctx: &storeapi::Context) -> anyhow::Result<()> {
        self.permission("put-delete")
    }
    fn GetObject(
        &self,
        _ctx: &storeapi::Context,
        _name: &str,
        start: i64,
        end: i64,
    ) -> anyhow::Result<Option<GetResp>> {
        let mut state = self.0.lock().unwrap();
        state.calls.push("get-object");
        let len = state.data.len() as i64;
        // end>start 时截取闭开区间切片；否则读到末尾。
        let last = if end > start { end.min(len) } else { len };
        let bytes = state.data[start as usize..last as usize].to_vec();
        let full = start == 0 && end == 0;
        let content_range = (!full).then(|| format!("bytes {}-{}/{}", start, last - 1, len));
        let body: Box<dyn ReadCloser> = if state.fail_next_body {
            state.fail_next_body = false;
            Box::new(FailBody)
        } else {
            Box::new(Body(Cursor::new(bytes)))
        };
        Ok(Some(GetResp {
            Body: body,
            IsFullRange: full,
            ContentLength: full.then_some(len),
            ContentRange: content_range,
        }))
    }
    fn PutObject(&self, _ctx: &storeapi::Context, _name: &str, data: &[u8]) -> anyhow::Result<()> {
        self.0.lock().unwrap().uploaded.extend_from_slice(data);
        Ok(())
    }
    fn DeleteObject(&self, _ctx: &storeapi::Context, _name: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn DeleteObjects(&self, _ctx: &storeapi::Context, names: &[String]) -> anyhow::Result<()> {
        self.0.lock().unwrap().deleted_batches.push(names.to_vec());
        Ok(())
    }
    fn HeadObject(
        &self,
        _ctx: &storeapi::Context,
        _name: &str,
    ) -> anyhow::Result<Option<HeadObjectResp>> {
        Ok(Some(HeadObjectResp {
            ReplicationStatus: self.0.lock().unwrap().replication_status.clone(),
        }))
    }
    fn IsObjectExists(&self, _ctx: &storeapi::Context, _name: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
    fn ListObjects(
        &self,
        _ctx: &storeapi::Context,
        _extra_prefix: &str,
        _start_after: &str,
        _continuation_token: Option<&str>,
        _max_keys: isize,
    ) -> anyhow::Result<Option<ListResp>> {
        self.0
            .lock()
            .unwrap()
            .objects
            .pop_front()
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("missing list response"))
    }
    fn CopyObject(&self, _ctx: &storeapi::Context, _params: &CopyInput) -> anyhow::Result<()> {
        Ok(())
    }
    fn MultipartWriter(
        &self,
        _ctx: &storeapi::Context,
        _name: &str,
    ) -> anyhow::Result<Option<Box<dyn objectio::Writer>>> {
        Ok(Some(Box::new(CollectWriter(Arc::clone(&self.0)))))
    }
    fn MultipartUploader(
        &self,
        _name: &str,
        _part_size: i64,
        _concurrency: i32,
    ) -> Option<Box<dyn Uploader>> {
        Some(Box::new(CollectUploader(Arc::clone(&self.0))))
    }
}

/// 用给定 MockClient 构造带固定桶前缀的 Storage。
fn storage(client: MockClient) -> Storage {
    NewStorage(
        client,
        storeapi::NewBucketPrefix("bucket", "root"),
        backuppb::S3 {
            Bucket: "bucket".to_owned(),
            Prefix: "root/".to_owned(),
            ..Default::default()
        },
        Some(Arc::new(objectio::recording::AccessStats::default())),
    )
}

/// 权限检查保持 Go 的短路顺序，并包装错误上下文；未知权限立即失败。
#[test]
fn permissions_preserve_go_order_short_circuit_and_error_context() {
    let client = MockClient::default();
    let context = storeapi::Context::default();
    for (permission, display, call) in [
        (storeapi::AccessBuckets, "AccessBucket", "bucket"),
        (storeapi::ListObjects, "ListObjects", "list"),
        (storeapi::GetObject, "GetObject", "get"),
        (
            storeapi::PutAndDeleteObject,
            "PutAndDeleteObject",
            "put-delete",
        ),
    ] {
        client.0.lock().unwrap().permission_error = Some("denied");
        let err = CheckPermissions(&context, &client, &[permission]).unwrap_err();
        assert_eq!(err.to_string(), format!("permission {display}: denied"));
        assert_eq!(client.0.lock().unwrap().calls.last(), Some(&call));
    }

    let err = CheckPermissions(&context, &client, &[storeapi::PutObject]).unwrap_err();
    assert_eq!(err.to_string(), "unknown permission: PutObject");

    client.0.lock().unwrap().calls.clear();
    CheckPermissions(
        &context,
        &client,
        &[
            storeapi::AccessBuckets,
            storeapi::ListObjects,
            storeapi::GetObject,
            storeapi::PutAndDeleteObject,
        ],
    )
    .unwrap();
    assert_eq!(
        client.0.lock().unwrap().calls,
        ["bucket", "list", "get", "put-delete"]
    );
}

/// 可配置的 StandardRetryer 替身，用于探测回退与元数据错误路径。
struct Standard {
    /// 标准层是否认为错误可重试。
    fallback: bool,
    /// 是否判定为实例元数据错误。
    metadata: bool,
    /// RetryDelay 返回值。
    delay: Duration,
    /// IsErrorRetryable 被调用次数。
    calls: Arc<AtomicUsize>,
}

impl StandardRetryer for Standard {
    fn IsErrorRetryable(&self, _err: &anyhow::Error) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.fallback
    }
    fn MaxAttempts(&self) -> i32 {
        7
    }
    fn RetryDelay(&self, _attempt: i32, _err: &anyhow::Error) -> anyhow::Result<Duration> {
        Ok(self.delay)
    }
    fn GetRetryToken(
        &self,
        _ctx: &storeapi::Context,
        _err: &anyhow::Error,
    ) -> anyhow::Result<ReleaseToken> {
        Ok(Box::new(|_| Ok(())))
    }
    fn GetInitialToken(&self) -> ReleaseToken {
        Box::new(|_| Ok(()))
    }
    fn IsInstanceMetadataError(&self, _err: &anyhow::Error) -> bool {
        self.metadata
    }
}

/// 校验连接类/EOF 重试规则与最短 1s 延迟下限。
#[test]
fn retryer_matches_connection_rules_and_one_second_floor() {
    let calls = Arc::new(AtomicUsize::new(0));
    let retryer = NewRetryer(Box::new(Standard {
        fallback: false,
        metadata: false,
        delay: Duration::from_millis(20),
        calls: Arc::clone(&calls),
    }));
    assert!(retryer.IsErrorRetryable(&anyhow::anyhow!("read: connection reset by peer")));
    assert!(!retryer.IsErrorRetryable(&anyhow::anyhow!("dial: connection refused")));
    assert!(retryer.IsErrorRetryable(&anyhow::anyhow!("unexpected EOF")));
    assert!(!retryer.IsErrorRetryable(&anyhow::anyhow!("ordinary")));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        retryer.RetryDelay(1, &anyhow::anyhow!("x")).unwrap(),
        Duration::from_secs(1)
    );
    assert_eq!(retryer.MaxAttempts(), 7);
}

/// 元数据 deadline 错误应快速失败，不进入标准可重试回退。
#[test]
fn metadata_deadline_fast_fails_without_standard_fallback() {
    let calls = Arc::new(AtomicUsize::new(0));
    let retryer = NewRetryer(Box::new(Standard {
        fallback: true,
        metadata: true,
        delay: Duration::from_secs(2),
        calls: Arc::clone(&calls),
    }));
    assert!(!retryer.IsErrorRetryable(&anyhow::anyhow!("context deadline exceeded")));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// Content-Range 解析与 S3BackendOptions.Apply 边界与 Go 对齐。
#[test]
fn range_parser_and_backend_options_match_go_edges() {
    let range = ParseRangeInfo(Some("bytes 10-19/100")).unwrap();
    assert_eq!(
        (range.Start, range.End, range.Size, range.RangeSize()),
        (10, 19, 100, 10)
    );
    assert!(
        ParseRangeInfo(None)
            .unwrap_err()
            .to_string()
            .contains("ContentRange is empty")
    );
    assert!(
        ParseRangeInfo(Some("garbage"))
            .unwrap_err()
            .to_string()
            .contains("invalid content range")
    );

    let mut target = backuppb::S3::default();
    let options = S3BackendOptions {
        Endpoint: "https://example.com/".to_owned(),
        AccessKey: "ak".to_owned(),
        SecretAccessKey: "sk".to_owned(),
        Region: "r1".to_owned(),
        ..Default::default()
    };
    options.Apply(&mut target).unwrap();
    assert_eq!(target.Endpoint, "https://example.com");
    assert_eq!(
        (target.AccessKey.as_str(), target.SecretAccessKey.as_str()),
        ("ak", "sk")
    );
    assert!(
        S3BackendOptions {
            Endpoint: "example.com".to_owned(),
            ..Default::default()
        }
        .Apply(&mut target)
        .unwrap_err()
        .to_string()
        .contains("scheme not found")
    );
    assert!(
        S3BackendOptions {
            SecretAccessKey: "sk".to_owned(),
            ..Default::default()
        }
        .Apply(&mut target)
        .unwrap_err()
        .to_string()
        .contains("access_key not found")
    );
}

#[test]
fn range_size_wraps_like_go_int64_arithmetic() {
    let range = RangeInfo {
        Start: -1,
        End: i64::MAX,
        Size: i64::MAX,
    };
    assert_eq!(range.RangeSize(), i64::MIN.wrapping_add(1));
}

#[test]
fn seek_current_overflow_reports_go_wrapped_offset() {
    let client = MockClient::default();
    let mut reader = S3ObjectReader::new(
        Arc::new(storage(client)),
        "overflow".to_owned(),
        Box::new(Body(Cursor::new(Vec::new()))),
        RangeInfo {
            Start: i64::MAX,
            End: i64::MAX,
            Size: i64::MAX,
        },
        storeapi::Context::default(),
        0,
    );
    let error = reader.seek(SeekFrom::Current(1)).unwrap_err();
    assert!(error.to_string().contains(&i64::MIN.to_string()));
}

#[test]
fn go_metric_public_names_remain_available() {
    assert_eq!(BackendS3, BACKEND_S3);
    assert_eq!(BackendOSS, BACKEND_OSS);
    assert_eq!(BackendKS3, BACKEND_KS3);
    assert_eq!(APICallListObjects, API_CALL_LIST_OBJECTS);
    assert_eq!(APICallHeadObjects, API_CALL_HEAD_OBJECTS);
    assert_eq!(APICallPutObject, API_CALL_PUT_OBJECT);
    assert!(std::ptr::eq(&*S3APICallCounter, &*S3_API_CALL_COUNTER));
}

/// ForcePathStyle 启发式与 DefineS3Flags/ParseFromFlags 决策保持 Go 语义。
#[test]
fn force_path_style_and_flags_keep_go_decisions() {
    let mut options = S3BackendOptions {
        ForcePathStyle: true,
        Provider: "aws".to_owned(),
        ..Default::default()
    };
    options.SetForcePathStyle("s3://bucket/prefix");
    assert!(!options.ForcePathStyle);
    let mut explicit = S3BackendOptions {
        ForcePathStyle: true,
        Provider: "aws".to_owned(),
        ..Default::default()
    };
    explicit.SetForcePathStyle("s3://bucket?force-path-style=true");
    assert!(explicit.ForcePathStyle);

    let mut flags = pflag::FlagSet::default();
    DefineS3Flags(&mut flags);
    flags.Set("s3.endpoint", "https://s3.example/").unwrap();
    flags.Set("s3.region", "r2").unwrap();
    let mut parsed = S3BackendOptions::default();
    parsed.ParseFromFlags(&flags).unwrap();
    assert_eq!(parsed.Endpoint, "https://s3.example");
    assert_eq!(parsed.Region, "r2");
    assert!(parsed.ForcePathStyle);
}

/// DeleteFiles 按 1000 分批；FileSynced 按复制状态映射 true/false/错误。
#[test]
fn storage_batches_deletes_and_maps_replication_states() {
    let client = MockClient::default();
    let store = storage(client.clone());
    let files: Vec<String> = (0..2001).map(|i| format!("file-{i}")).collect();
    store
        .DeleteFiles(&storeapi::Context::default(), &files)
        .unwrap();
    let sizes: Vec<usize> = client
        .0
        .lock()
        .unwrap()
        .deleted_batches
        .iter()
        .map(Vec::len)
        .collect();
    assert_eq!(sizes, [1000, 1000, 1]);

    for (status, expected) in [
        ("COMPLETE", Some(true)),
        ("COMPLETED", Some(true)),
        ("REPLICA", Some(true)),
        ("PENDING", Some(false)),
    ] {
        client.0.lock().unwrap().replication_status = status.to_owned();
        assert_eq!(
            store
                .FileSynced(&storeapi::Context::default(), "f")
                .unwrap(),
            expected.unwrap()
        );
    }
    client.0.lock().unwrap().replication_status = "FAILED".to_owned();
    assert!(
        store
            .FileSynced(&storeapi::Context::default(), "f")
            .unwrap_err()
            .to_string()
            .contains("FAILED")
    );
}

/// WalkDir 分页、剥离公共前缀并跳过空目录键。
#[test]
fn walk_dir_paginates_trims_prefix_and_skips_empty_directories() {
    let client = MockClient::default();
    client.0.lock().unwrap().objects.extend([
        ListResp {
            NextContinuationToken: Some("next".to_owned()),
            IsTruncated: true,
            Objects: vec![
                Object {
                    Key: "root/dir/".to_owned(),
                    Size: 0,
                },
                Object {
                    Key: "root/a".to_owned(),
                    Size: 2,
                },
            ],
        },
        ListResp {
            NextContinuationToken: None,
            IsTruncated: false,
            Objects: vec![Object {
                Key: "root/b".to_owned(),
                Size: 3,
            }],
        },
    ]);
    let store = storage(client);
    let mut seen = Vec::new();
    store
        .WalkDir(&storeapi::Context::default(), None, |name, size| {
            seen.push((name.to_owned(), size));
            Ok(())
        })
        .unwrap();
    assert_eq!(seen, [("a".to_owned(), 2), ("b".to_owned(), 3)]);
    assert_eq!(store.URI(), "s3://bucket/root/");
}

/// Open 读路径在正文错误时 reopen，并保持 seek/EOF 语义。
#[test]
fn open_reader_retries_body_errors_and_preserves_seek_semantics() {
    let client = MockClient::default();
    {
        let mut state = client.0.lock().unwrap();
        state.data = b"0123456789".to_vec();
        state.fail_next_body = true;
    }
    let store = storage(client.clone());
    let ctx = storeapi::Context::default();
    let mut reader = store.Open(ctx, "f", None).unwrap();
    let mut first = [0; 4];
    reader.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"0123");
    assert_eq!(reader.seek(SeekFrom::Current(2)).unwrap(), 6);
    let mut tail = Vec::new();
    reader.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, b"6789");
    assert_eq!(reader.seek(SeekFrom::End(1)).unwrap(), 10);
    assert_eq!(reader.read(&mut first).unwrap(), 0);
    assert_eq!(
        client
            .0
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|call| **call == "get-object")
            .count(),
        2
    );
}

/// Create 同时覆盖同步 MultipartWriter 与并发 Uploader 路径。
#[test]
fn create_supports_sync_and_concurrent_upload_paths() {
    let client = MockClient::default();
    let store = storage(client.clone());
    let ctx = storeapi::Context::default();

    let mut sync_writer = store.Create(ctx.clone(), "sync", None).unwrap();
    sync_writer.write(&ctx, b"abc").unwrap();
    sync_writer.close(&ctx).unwrap();

    let option = storeapi::WriterOption {
        Concurrency: 2,
        PartSize: 2,
    };
    let mut async_writer = store.Create(ctx.clone(), "async", Some(&option)).unwrap();
    async_writer.write(&ctx, b"defg").unwrap();
    async_writer.close(&ctx).unwrap();
    assert_eq!(client.0.lock().unwrap().uploaded, b"abcdefg");
}

/// RecordAPICall 使用 backend+api 标签递增计数器。
#[test]
fn api_call_metric_uses_backend_and_api_labels() {
    let before = S3_API_CALL_COUNTER
        .with_label_values(&[BACKEND_S3, API_CALL_LIST_OBJECTS])
        .get();
    RecordAPICall(BACKEND_S3, API_CALL_LIST_OBJECTS);
    let after = S3_API_CALL_COUNTER
        .with_label_values(&[BACKEND_S3, API_CALL_LIST_OBJECTS])
        .get();
    assert_eq!(after, before + 1.0);
}
