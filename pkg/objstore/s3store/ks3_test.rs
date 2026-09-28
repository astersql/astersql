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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use s3store::*;

#[path = "main_test.rs"]
mod support;
use support::MockS3;

fn new_ks3(mock: Arc<MockS3>, bucket: &str, prefix: &str) -> KS3Storage {
    NewKS3StorageForTest(
        mock,
        &backuppb::S3 {
            Bucket: bucket.to_owned(),
            Prefix: prefix.to_owned(),
            ..Default::default()
        },
        None,
    )
}

#[test]
fn uri_uses_ks3_scheme() {
    let storage = new_ks3(Arc::new(MockS3::default()), "bucket", "prefix/");

    assert_eq!(storeapi::Storage::URI(&storage), "ks3://bucket/prefix/");
}

#[test]
fn copy_from_deletes_existing_target_and_retries() {
    let mock = Arc::new(MockS3::default());
    mock.push_copy(Err(api_error("ObjectAlreayExists", "target exists")));
    mock.push_delete(Ok(()));
    mock.push_copy(Ok(()));
    let source = new_ks3(mock.clone(), "source-bucket", "source-prefix/");
    let destination = new_ks3(mock.clone(), "destination-bucket", "destination-prefix/");
    let ctx = storeapi::Context::default();

    destination
        .CopyFrom(
            &ctx,
            &source,
            &storeapi::CopySpec {
                From: "from-object".to_owned(),
                To: "to-object".to_owned(),
            },
        )
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.copies.len(), 2);
    assert_eq!(calls.deletes.len(), 1);
    assert_eq!(calls.deletes[0].0.bucket, "destination-bucket");
    assert_eq!(calls.deletes[0].0.key, "destination-prefix/to-object");
    drop(calls);
    mock.assert_drained();
}

#[derive(Default)]
struct ListV1Probe {
    called: AtomicBool,
}

impl S3API for ListV1Probe {
    fn list_objects(
        &self,
        _: &storeapi::Context,
        input: &ListObjectsInput,
        _: RequestOptions,
    ) -> anyhow::Result<ListObjectsOutput> {
        assert_eq!(input.bucket, "bucket");
        assert_eq!(input.prefix, "prefix/");
        assert_eq!(input.max_keys, 1);
        self.called.store(true, Ordering::SeqCst);
        Ok(ListObjectsOutput::default())
    }
}

#[test]
fn permission_helpers_match_ks3_sdk_error_and_list_v1_semantics() {
    let ctx = storeapi::Context::default();
    let options = backuppb::S3 {
        Bucket: "bucket".to_owned(),
        Prefix: "prefix/".to_owned(),
        ..Default::default()
    };
    let list_probe = ListV1Probe::default();
    listObjectsCheckKS3(&ctx, &list_probe, &options).unwrap();
    assert!(list_probe.called.load(Ordering::SeqCst));

    let get_mock = MockS3::default();
    get_mock.push_get(Err(anyhow::anyhow!("plain transport error")));
    getObjectCheckKS3(&ctx, &get_mock, &options).unwrap();
    get_mock.push_get(Err(api_error("AccessDenied", "denied")));
    assert!(getObjectCheckKS3(&ctx, &get_mock, &options).is_err());

    let put_mock = MockS3::default();
    put_mock.push_put(Ok(()));
    put_mock.push_delete(Err(api_error("NoSuchKey", "missing cleanup object")));
    assert!(putAndDeleteObjectCheckKS3(&ctx, &put_mock, &options).is_err());
}

#[test]
fn put_input_and_error_code_helpers_match_go_fields() {
    let options = backuppb::S3 {
        Bucket: "bucket".to_owned(),
        Prefix: "prefix/".to_owned(),
        Acl: "private".to_owned(),
        Sse: "AES256".to_owned(),
        SseKmsKeyId: "kms-key".to_owned(),
        StorageClass: "STANDARD_IA".to_owned(),
        ..Default::default()
    };
    let input = buildPutObjectInputKS3(&options, "object", b"data");
    assert_eq!(input.bucket, "bucket");
    assert_eq!(input.key, "prefix/object");
    assert_eq!(input.body, b"data");
    assert_eq!(input.acl.as_deref(), Some("private"));
    assert_eq!(input.server_side_encryption.as_deref(), Some("AES256"));
    assert_eq!(input.sse_kms_key_id.as_deref(), Some("kms-key"));
    assert_eq!(input.storage_class.as_deref(), Some("STANDARD_IA"));
    assert_eq!(int64p(7), Some(7));
    assert_eq!(boolP(false), Some(false));
    assert!(maybeObjectAlreadyExists(&api_error(
        "ObjectAlreadyExists",
        "exists"
    )));
    assert!(maybeObjectAlreadyExists(&api_error(
        "ObjectAlreayExists",
        "exists"
    )));
}
