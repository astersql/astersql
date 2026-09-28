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

// s3store 集成测试共享夹具：可排队响应的 Mock S3API 与 `CreateS3Suite`。
//
// 被 `client_test` 等通过 `#[path]` 引入，在不访问真实对象存储的前提下
// 验证 Client/Storage 的请求映射、权限探测与分片上传行为。

#![allow(dead_code, non_snake_case)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use s3store::*;

/// 记录 Mock 收到的各 API 调用参数与请求选项。
#[derive(Default)]
pub struct Calls {
    pub head_buckets: Vec<(HeadBucketInput, RequestOptions)>,
    pub lists: Vec<(ListObjectsV2Input, RequestOptions)>,
    pub gets: Vec<(GetObjectInput, RequestOptions)>,
    pub puts: Vec<(PutObjectInput, RequestOptions)>,
    pub deletes: Vec<(DeleteObjectInput, RequestOptions)>,
    pub delete_batches: Vec<(DeleteObjectsInput, RequestOptions)>,
    pub heads: Vec<(HeadObjectInput, RequestOptions)>,
    pub copies: Vec<(CopyObjectInput, RequestOptions)>,
    pub creates: Vec<(CreateMultipartUploadInput, RequestOptions)>,
    pub uploads: Vec<(UploadPartInput, RequestOptions)>,
    pub completes: Vec<(CompleteMultipartUploadInput, RequestOptions)>,
}

/// 可注入预置结果队列的 S3API Mock；未预置时回退到默认成功响应。
#[derive(Default)]
pub struct MockS3 {
    pub calls: Mutex<Calls>,
    head_bucket_results: Mutex<VecDeque<Result<()>>>,
    list_results: Mutex<VecDeque<Result<ListObjectsV2Output>>>,
    get_results: Mutex<VecDeque<Result<GetObjectOutput>>>,
    put_results: Mutex<VecDeque<Result<()>>>,
    delete_results: Mutex<VecDeque<Result<()>>>,
    delete_batch_results: Mutex<VecDeque<Result<()>>>,
    head_results: Mutex<VecDeque<Result<HeadObjectOutput>>>,
    copy_results: Mutex<VecDeque<Result<()>>>,
    create_results: Mutex<VecDeque<Result<CreateMultipartUploadOutput>>>,
    upload_results: Mutex<VecDeque<Result<UploadPartOutput>>>,
    complete_results: Mutex<VecDeque<Result<()>>>,
    expected_get_ranges: Mutex<VecDeque<Option<String>>>,
}

impl MockS3 {
    /// 排队下一次 HeadBucket 结果。
    pub fn push_head_bucket(&self, result: Result<()>) {
        self.head_bucket_results.lock().unwrap().push_back(result);
    }

    /// 排队下一次 ListObjectsV2 结果。
    pub fn push_list(&self, result: Result<ListObjectsV2Output>) {
        self.list_results.lock().unwrap().push_back(result);
    }

    /// 排队下一次 GetObject 结果。
    pub fn push_get(&self, result: Result<GetObjectOutput>) {
        self.get_results.lock().unwrap().push_back(result);
    }

    /// 排队下一次 PutObject 结果。
    pub fn push_put(&self, result: Result<()>) {
        self.put_results.lock().unwrap().push_back(result);
    }

    /// 排队下一次 DeleteObject 结果。
    pub fn push_delete(&self, result: Result<()>) {
        self.delete_results.lock().unwrap().push_back(result);
    }

    /// 排队下一次 DeleteObjects（批量删除）结果。
    pub fn push_delete_batch(&self, result: Result<()>) {
        self.delete_batch_results.lock().unwrap().push_back(result);
    }

    /// 排队下一次 HeadObject 结果。
    pub fn push_head(&self, result: Result<HeadObjectOutput>) {
        self.head_results.lock().unwrap().push_back(result);
    }

    /// 排队下一次 CopyObject 结果。
    pub fn push_copy(&self, result: Result<()>) {
        self.copy_results.lock().unwrap().push_back(result);
    }

    /// 排队下一次 CreateMultipartUpload 结果。
    pub fn push_create(&self, result: Result<CreateMultipartUploadOutput>) {
        self.create_results.lock().unwrap().push_back(result);
    }

    /// 排队下一次 UploadPart 结果。
    pub fn push_upload(&self, result: Result<UploadPartOutput>) {
        self.upload_results.lock().unwrap().push_back(result);
    }

    /// 排队下一次 CompleteMultipartUpload 结果。
    pub fn push_complete(&self, result: Result<()>) {
        self.complete_results.lock().unwrap().push_back(result);
    }

    /// 断言所有预置结果队列已耗尽，避免测试漏消费。
    pub fn assert_drained(&self) {
        assert!(self.head_bucket_results.lock().unwrap().is_empty());
        assert!(self.list_results.lock().unwrap().is_empty());
        assert!(self.get_results.lock().unwrap().is_empty());
        assert!(self.put_results.lock().unwrap().is_empty());
        assert!(self.delete_results.lock().unwrap().is_empty());
        assert!(self.delete_batch_results.lock().unwrap().is_empty());
        assert!(self.head_results.lock().unwrap().is_empty());
        assert!(self.copy_results.lock().unwrap().is_empty());
        assert!(self.create_results.lock().unwrap().is_empty());
        assert!(self.upload_results.lock().unwrap().is_empty());
        assert!(self.complete_results.lock().unwrap().is_empty());
        assert!(self.expected_get_ranges.lock().unwrap().is_empty());
    }
}

/// 从队列弹出预置结果；队列空时用 `fallback` 构造成功值。
fn pop_or<T>(queue: &Mutex<VecDeque<Result<T>>>, fallback: impl FnOnce() -> T) -> Result<T> {
    queue
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| Ok(fallback()))
}

impl S3API for MockS3 {
    fn head_bucket(
        &self,
        _: &storeapi::Context,
        input: &HeadBucketInput,
        options: RequestOptions,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .head_buckets
            .push((input.clone(), options));
        pop_or(&self.head_bucket_results, || ())
    }

    fn list_objects_v2(
        &self,
        _: &storeapi::Context,
        input: &ListObjectsV2Input,
        options: RequestOptions,
    ) -> Result<ListObjectsV2Output> {
        self.calls
            .lock()
            .unwrap()
            .lists
            .push((input.clone(), options));
        pop_or(&self.list_results, ListObjectsV2Output::default)
    }

    fn get_object(
        &self,
        _: &storeapi::Context,
        input: &GetObjectInput,
        options: RequestOptions,
    ) -> Result<GetObjectOutput> {
        // 若测试预设了期望 Range，则在返回前校验请求 Range 是否匹配。
        if let Some(expected) = self.expected_get_ranges.lock().unwrap().pop_front() {
            assert_eq!(input.range, expected);
        }
        self.calls
            .lock()
            .unwrap()
            .gets
            .push((input.clone(), options));
        pop_or(&self.get_results, || GetObjectOutput {
            body: Box::new(MemoryBody::new(Vec::new())),
            content_length: Some(0),
            content_range: None,
        })
    }

    fn put_object(
        &self,
        _: &storeapi::Context,
        input: &PutObjectInput,
        options: RequestOptions,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .puts
            .push((input.clone(), options));
        pop_or(&self.put_results, || ())
    }

    fn delete_object(
        &self,
        _: &storeapi::Context,
        input: &DeleteObjectInput,
        options: RequestOptions,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .deletes
            .push((input.clone(), options));
        pop_or(&self.delete_results, || ())
    }

    fn delete_objects(
        &self,
        _: &storeapi::Context,
        input: &DeleteObjectsInput,
        options: RequestOptions,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .delete_batches
            .push((input.clone(), options));
        pop_or(&self.delete_batch_results, || ())
    }

    fn head_object(
        &self,
        _: &storeapi::Context,
        input: &HeadObjectInput,
        options: RequestOptions,
    ) -> Result<HeadObjectOutput> {
        self.calls
            .lock()
            .unwrap()
            .heads
            .push((input.clone(), options));
        pop_or(&self.head_results, HeadObjectOutput::default)
    }

    fn copy_object(
        &self,
        _: &storeapi::Context,
        input: &CopyObjectInput,
        options: RequestOptions,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .copies
            .push((input.clone(), options));
        pop_or(&self.copy_results, || ())
    }

    fn create_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CreateMultipartUploadInput,
        options: RequestOptions,
    ) -> Result<CreateMultipartUploadOutput> {
        self.calls
            .lock()
            .unwrap()
            .creates
            .push((input.clone(), options));
        pop_or(&self.create_results, || CreateMultipartUploadOutput {
            bucket: input.bucket.clone(),
            key: input.key.clone(),
            upload_id: "upload-id".to_owned(),
        })
    }

    fn upload_part(
        &self,
        _: &storeapi::Context,
        input: &UploadPartInput,
        options: RequestOptions,
    ) -> Result<UploadPartOutput> {
        self.calls
            .lock()
            .unwrap()
            .uploads
            .push((input.clone(), options));
        pop_or(&self.upload_results, || UploadPartOutput {
            e_tag: Some("etag".to_owned()),
        })
    }

    fn complete_multipart_upload(
        &self,
        _: &storeapi::Context,
        input: &CompleteMultipartUploadInput,
        options: RequestOptions,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .completes
            .push((input.clone(), options));
        pop_or(&self.complete_results, || ())
    }
}

/// 一套绑定在一起的 Mock、Client 与高层 Storage，方便集成测试复用。
pub struct Suite {
    pub MockS3: Arc<MockS3>,
    pub Client: S3Client,
    pub Storage: s3like::Storage,
}

/// 使用默认桶/前缀配置创建测试套件（无访问统计）。
pub fn CreateS3Suite() -> Suite {
    CreateS3SuiteWithRec(None)
}

/// 创建测试套件，可选注入对象存储访问统计记录器。
pub fn CreateS3SuiteWithRec(access_rec: Option<Arc<objectio::recording::AccessStats>>) -> Suite {
    let mock = Arc::new(MockS3::default());
    let options = s3store::backuppb::S3 {
        Region: "us-west-2".to_owned(),
        Bucket: "bucket".to_owned(),
        Prefix: "prefix/".to_owned(),
        Acl: "acl".to_owned(),
        Sse: "sse".to_owned(),
        StorageClass: "sc".to_owned(),
        ..Default::default()
    };
    let client = S3Client::new(
        mock.clone(),
        storeapi::NewBucketPrefix("bucket", "prefix/"),
        options.clone(),
        false,
    );
    let storage = NewS3StorageForTest(mock.clone(), &options, access_rec);
    Suite {
        MockS3: mock,
        Client: client,
        Storage: storage,
    }
}

impl Suite {
    /// 为 Range/全文读取场景预置 GetObject 响应与期望的 Range 头。
    pub fn ExpectedCalls<F>(&self, data: Vec<u8>, start_offsets: &[usize], new_reader: F)
    where
        F: Fn(Vec<u8>, usize) -> Box<dyn prefetch::reader::ReadCloser>,
    {
        for &offset in start_offsets {
            self.MockS3
                .expected_get_ranges
                .lock()
                .unwrap()
                .push_back((offset > 0).then(|| format!("bytes={offset}-")));
            self.MockS3.push_get(Ok(GetObjectOutput {
                body: new_reader(data.clone(), offset),
                content_length: (offset == 0).then_some(data.len() as i64),
                content_range: (offset > 0)
                    .then(|| format!("bytes {offset}-{}/{}", data.len() - 1, data.len())),
            }));
        }
    }
}
