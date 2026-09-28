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

// OSS 存储集成风格单测：内存 Fake API 覆盖读写、分片与目录遍历。
//
// 用 `MemoryOssApi` 模拟对象与 multipart 状态，经 `new_oss_storage_for_test`
// 接到 `s3like::Storage`，验证 Write/Read/Copy/Walk 及内网 endpoint 辅助函数。

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use task_ossstore::*;

/// 一次 multipart 上传的内存状态：目标 bucket/key 与已上传分片。
#[derive(Default)]
struct UploadState {
    bucket: String,
    key: String,
    parts: BTreeMap<i32, Vec<u8>>,
}

/// 进程内 Fake OSS API：对象表 + multipart 会话表。
#[derive(Default)]
struct MemoryOssApi {
    objects: Mutex<BTreeMap<(String, String), Vec<u8>>>,
    uploads: Mutex<HashMap<String, UploadState>>,
    next_upload: AtomicUsize,
}

impl MemoryOssApi {
    /// 按 bucket+key 克隆对象内容；不存在则返回 `None`。
    fn object(&self, bucket: &str, key: &str) -> Option<Vec<u8>> {
        self.objects
            .lock()
            .unwrap()
            .get(&(bucket.to_owned(), key.to_owned()))
            .cloned()
    }
}

impl API for MemoryOssApi {
    fn is_bucket_exist(&self, _: &storeapi::Context, _: &str) -> Result<bool> {
        Ok(true)
    }

    fn bucket_location(&self, _: &storeapi::Context, _: &str) -> Result<String> {
        Ok("oss-cn-hangzhou".to_owned())
    }

    fn head_object(&self, _: &storeapi::Context, input: &HeadObjectInput) -> Result<()> {
        self.object(&input.bucket, &input.key)
            .map(|_| ())
            .ok_or_else(|| api_error("NoSuchKey", input.key.clone()))
    }

    fn get_object(&self, _: &storeapi::Context, input: &GetObjectInput) -> Result<GetObjectOutput> {
        let data = self
            .object(&input.bucket, &input.key)
            .ok_or_else(|| api_error("NoSuchKey", input.key.clone()))?;
        let Some(range) = input.range.as_deref() else {
            return Ok(GetObjectOutput::from_bytes(data, None));
        };
        // 解析 `bytes=start-end` 区间读；空 end 表示读到对象末尾。
        let range = range
            .strip_prefix("bytes=")
            .ok_or_else(|| anyhow!("invalid range {range}"))?;
        let (start, end) = range
            .split_once('-')
            .ok_or_else(|| anyhow!("invalid range {range}"))?;
        let start = start.parse::<usize>()?;
        let end = if end.is_empty() {
            data.len().saturating_sub(1)
        } else {
            end.parse::<usize>()?.min(data.len().saturating_sub(1))
        };
        if start > end || start >= data.len() {
            return Err(api_error("InvalidRange", input.key.clone()));
        }
        Ok(GetObjectOutput::from_bytes(
            data[start..=end].to_vec(),
            Some(format!("bytes {start}-{end}/{}", data.len())),
        ))
    }

    fn put_object(&self, _: &storeapi::Context, input: &PutObjectInput) -> Result<()> {
        self.objects.lock().unwrap().insert(
            (input.bucket.clone(), input.key.clone()),
            input.body.clone(),
        );
        Ok(())
    }

    fn copy_object(&self, _: &storeapi::Context, input: &CopyObjectInput) -> Result<()> {
        let data = self
            .object(&input.source_bucket, &input.source_key)
            .ok_or_else(|| api_error("NoSuchKey", input.source_key.clone()))?;
        self.objects
            .lock()
            .unwrap()
            .insert((input.bucket.clone(), input.key.clone()), data);
        Ok(())
    }

    fn delete_object(&self, _: &storeapi::Context, input: &DeleteObjectInput) -> Result<()> {
        self.objects
            .lock()
            .unwrap()
            .remove(&(input.bucket.clone(), input.key.clone()));
        Ok(())
    }

    fn delete_objects(&self, _: &storeapi::Context, input: &DeleteObjectsInput) -> Result<()> {
        let mut objects = self.objects.lock().unwrap();
        for key in &input.keys {
            objects.remove(&(input.bucket.clone(), key.clone()));
        }
        Ok(())
    }

    fn list_objects_v2(
        &self,
        _: &storeapi::Context,
        input: &ListObjectsV2Input,
    ) -> Result<ListObjectsV2Output> {
        // 按 prefix/start_after 过滤后排序，再用 continuation_token 做偏移分页。
        let mut objects = self
            .objects
            .lock()
            .unwrap()
            .iter()
            .filter(|((bucket, key), _)| {
                bucket == &input.bucket
                    && key.starts_with(&input.prefix)
                    && input
                        .start_after
                        .as_ref()
                        .is_none_or(|start_after| key > start_after)
            })
            .map(|((_, key), data)| ListedObject {
                key: key.clone(),
                size: i64::try_from(data.len()).unwrap_or(i64::MAX),
            })
            .collect::<Vec<_>>();
        objects.sort_by(|left, right| left.key.cmp(&right.key));

        let offset = input
            .continuation_token
            .as_deref()
            .unwrap_or("0")
            .parse::<usize>()?;
        let limit = usize::try_from(input.max_keys.max(1)).unwrap_or(1000);
        let end = (offset + limit).min(objects.len());
        let is_truncated = end < objects.len();
        Ok(ListObjectsV2Output {
            next_continuation_token: is_truncated.then(|| end.to_string()),
            is_truncated,
            contents: objects[offset.min(objects.len())..end].to_vec(),
        })
    }

    fn initiate_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CreateMultipartUploadInput,
    ) -> Result<CreateMultipartUploadOutput> {
        let upload_id = format!(
            "upload-{}",
            self.next_upload.fetch_add(1, Ordering::SeqCst) + 1
        );
        self.uploads.lock().unwrap().insert(
            upload_id.clone(),
            UploadState {
                bucket: input.bucket.clone(),
                key: input.key.clone(),
                parts: BTreeMap::new(),
            },
        );
        Ok(CreateMultipartUploadOutput {
            bucket: input.bucket.clone(),
            key: input.key.clone(),
            upload_id,
        })
    }

    fn upload_part(
        &self,
        _: &storeapi::Context,
        input: &UploadPartInput,
    ) -> Result<UploadPartOutput> {
        self.uploads
            .lock()
            .unwrap()
            .get_mut(&input.upload_id)
            .ok_or_else(|| anyhow!("unknown upload {}", input.upload_id))?
            .parts
            .insert(input.part_number, input.body.clone());
        Ok(UploadPartOutput {
            etag: format!("etag-{}", input.part_number),
        })
    }

    fn complete_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CompleteMultipartUploadInput,
    ) -> Result<()> {
        // 按 Complete 请求中的分片顺序拼接，写入最终对象并移除会话。
        let upload = self
            .uploads
            .lock()
            .unwrap()
            .remove(&input.upload_id)
            .ok_or_else(|| anyhow!("unknown upload {}", input.upload_id))?;
        let mut data = Vec::new();
        for part in &input.parts {
            data.extend_from_slice(
                upload
                    .parts
                    .get(&part.part_number)
                    .ok_or_else(|| anyhow!("missing part {}", part.part_number))?,
            );
        }
        self.objects
            .lock()
            .unwrap()
            .insert((upload.bucket, upload.key), data);
        Ok(())
    }

    fn abort_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &AbortMultipartUploadInput,
    ) -> Result<()> {
        self.uploads.lock().unwrap().remove(&input.upload_id);
        Ok(())
    }
}

/// 用给定 Fake API 与 bucket/prefix 构造测试用 `Storage`。
fn new_store(api: Arc<MemoryOssApi>, bucket: &str, prefix: &str) -> s3like::Storage {
    new_oss_storage_for_test(
        api,
        s3like::backuppb::S3 {
            Bucket: bucket.to_owned(),
            Prefix: prefix.to_owned(),
            ..Default::default()
        },
        None,
    )
}

/// 覆盖整文件读写、区间读、批量删、大文件分片写、跨 store 拷贝与 WalkDir。
#[test]
fn test_store() {
    let ctx = storeapi::Context::default();
    let api = Arc::new(MemoryOssApi::default());
    let store = new_store(api.clone(), "tidbx-gsort", "test-prefix");

    let key = "test-obj";
    let marker = b"hello oss store. length 25";
    let mut data = vec![0_u8; 100 * 1024];
    data[100..100 + marker.len()].copy_from_slice(marker);
    store.WriteFile(&ctx, key, &data).unwrap();
    assert_eq!(store.ReadFile(&ctx, key).unwrap(), data);
    assert!(store.FileExists(&ctx, key).unwrap());

    // 区间 Open：只读 marker 所在字节范围。
    let mut reader = store
        .Open(
            ctx.clone(),
            key,
            Some(&storeapi::ReaderOption {
                StartOffset: Some(100),
                EndOffset: Some(i64::try_from(100 + marker.len()).unwrap()),
                ..Default::default()
            }),
        )
        .unwrap();
    let mut ranged = Vec::new();
    reader.read_to_end(&mut ranged).unwrap();
    assert_eq!(ranged, marker);

    store.DeleteFile(&ctx, key).unwrap();
    assert!(!store.FileExists(&ctx, key).unwrap());

    let mut created = Vec::new();
    for index in 0..10 {
        let key = format!("create-{index}");
        let mut writer = store.Create(ctx.clone(), &key, None).unwrap();
        assert_eq!(writer.Write(&ctx, &data).unwrap(), data.len());
        writer.Close(&ctx).unwrap();
        created.push(key);
    }
    store.DeleteFiles(&ctx, &created).unwrap();
    assert!(
        created
            .iter()
            .all(|key| !store.FileExists(&ctx, key).unwrap())
    );

    // 大对象：小 PartSize + 多次 Write，走 multipart 路径。
    let mut writer = store
        .Create(
            ctx.clone(),
            "large-file",
            Some(&storeapi::WriterOption {
                PartSize: 5 * 1024 * 1024,
                Concurrency: 20,
            }),
        )
        .unwrap();
    let part = vec![7_u8; 3 * 1024 * 1024];
    for _ in 0..20 {
        assert_eq!(writer.Write(&ctx, &part).unwrap(), part.len());
    }
    writer.Close(&ctx).unwrap();
    assert_eq!(
        store.ReadFile(&ctx, "large-file").unwrap().len(),
        60 * 1024 * 1024
    );

    let source = new_store(api.clone(), "tidbx-gsort-2", "p-for-src-files");
    source
        .WriteFile(&ctx, "src-file", b"hello oss store copy")
        .unwrap();
    store
        .CopyFrom(
            &ctx,
            &source,
            &storeapi::CopySpec {
                From: "src-file".to_owned(),
                To: "dst-file".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(
        store.ReadFile(&ctx, "dst-file").unwrap(),
        b"hello oss store copy"
    );

    // WalkDir：多子目录文件，校验分页、SubDir、ObjPrefix 过滤。
    let walk_store = new_store(api, "tidbx-gsort", "walk-test");
    let mut all_file_names = Vec::with_capacity(1500);
    let mut files_in_dir = BTreeMap::<String, Vec<String>>::new();
    for sub in ["a", "aa", "aaa", "b", "c"] {
        for index in 0..300 {
            let name = format!("{sub}/file-{index:03}.txt");
            walk_store.WriteFile(&ctx, &name, b"empty").unwrap();
            all_file_names.push(name.clone());
            files_in_dir.entry(sub.to_owned()).or_default().push(name);
        }
    }

    let walk = |option: storeapi::WalkOption| {
        let mut files = Vec::new();
        walk_store
            .WalkDir(&ctx, Some(&option), |path, size| {
                assert_eq!(size, 5);
                files.push(path.to_owned());
                Ok(())
            })
            .unwrap();
        files
    };
    assert_eq!(walk(Default::default()), all_file_names);
    assert_eq!(
        walk(storeapi::WalkOption {
            ListCount: 333,
            ..Default::default()
        }),
        all_file_names
    );
    assert_eq!(
        walk(storeapi::WalkOption {
            SubDir: "a".to_owned(),
            ..Default::default()
        }),
        files_in_dir["a"]
    );
    assert_eq!(
        walk(storeapi::WalkOption {
            SubDir: "aa".to_owned(),
            ObjPrefix: "file-1".to_owned(),
            ListCount: 13,
            ..Default::default()
        }),
        files_in_dir["aa"][100..200]
    );
    assert_eq!(
        walk(storeapi::WalkOption {
            SubDir: "aa".to_owned(),
            ObjPrefix: "file-11".to_owned(),
            ..Default::default()
        }),
        files_in_dir["aa"][110..120]
    );
    assert_eq!(
        walk(storeapi::WalkOption {
            SubDir: "b".to_owned(),
            ..Default::default()
        }),
        files_in_dir["b"]
    );
}

/// 校验默认公网/内网 endpoint 拼装与 Region 前缀裁剪。
#[test]
fn test_internal_endpoint() {
    assert_eq!(
        endpoint_for_region("cn-hangzhou", true),
        "https://oss-cn-hangzhou-internal.aliyuncs.com"
    );
    assert_eq!(trim_oss_region_id("oss-cn-hangzhou"), "cn-hangzhou");
}

/// 仅同 Region 且 ECS Region 非空时才允许内网 endpoint。
#[test]
fn test_can_use_internal_endpoint() {
    assert!(!can_use_internal_endpoint("", "cn-hangzhou"));
    assert!(!can_use_internal_endpoint("cn-beijing", "cn-hangzhou"));
    assert!(can_use_internal_endpoint("cn-hangzhou", "cn-hangzhou"));
}
