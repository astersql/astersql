// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// etcd 后端识别与客户端空输入路径的单元测试。
//
// 通过同时提供 PD 地址和元数据 etcd 地址的模拟存储，验证地址解析只采用
// `EtcdBackend::EtcdAddrs`，并确认缺少存储时不会尝试建立网络连接。

use etcd_client::TlsOptions;
use keyspace_dependency::{ApiVersion, BasicCodec, Codec};

use crate::{EtcdBackend, GetEtcdAddrs, NewEtcdCli, Storage, StoreError};

/// 同时实现存储与 etcd 后端能力，用于区分 PD 地址和元数据 etcd 地址。
struct MockEtcdBackend {
    pd_addrs: Vec<String>,
    meta_addrs: Vec<String>,
    codec: BasicCodec,
}

impl Storage for MockEtcdBackend {
    fn GetKeyspace(&self) -> &str {
        ""
    }

    fn GetCodec(&self) -> &dyn Codec {
        &self.codec
    }

    fn AsEtcdBackend(&self) -> Option<&dyn EtcdBackend> {
        Some(self)
    }
}

impl EtcdBackend for MockEtcdBackend {
    fn EtcdAddrs(&self) -> Result<Vec<String>, StoreError> {
        Ok(self.meta_addrs.clone())
    }

    fn GetPDAddrs(&self) -> Result<Vec<String>, StoreError> {
        Ok(self.pd_addrs.clone())
    }

    fn TLSConfig(&self) -> Option<TlsOptions> {
        None
    }

    fn StartGCWorker(&self) -> Result<(), StoreError> {
        Ok(())
    }
}

#[tokio::test]
async fn test_new_etcd_cli_get_etcd_addrs() {
    // 空存储既没有可返回的后端，也不应产生任何 etcd 地址。
    let (backend, addresses) = GetEtcdAddrs(None).unwrap();
    assert!(backend.is_none());
    assert!(addresses.is_empty());

    let store = MockEtcdBackend {
        pd_addrs: vec!["localhost:2379".to_owned()],
        meta_addrs: vec!["localhost:2389".to_owned()],
        codec: BasicCodec {
            api_version: ApiVersion::V1,
            keyspace_id: 0,
        },
    };
    // 地址解析必须使用后端的元数据 etcd 地址，不能退回到 PD 地址。
    let (backend, addresses) = GetEtcdAddrs(Some(&store)).unwrap();
    assert!(backend.is_some());
    assert_eq!(addresses, ["localhost:2389"]);

    // 无存储时客户端构造应短路为 None，避免发起实际连接。
    assert!(NewEtcdCli(None).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires ASTER_ETCD_TEST_ENDPOINT for a real etcd server"]
async fn new_etcd_cli_uses_metadata_group_and_codec_namespace_for_kv() {
    let endpoint = std::env::var("ASTER_ETCD_TEST_ENDPOINT").unwrap();
    let store = MockEtcdBackend {
        pd_addrs: vec!["invalid-pd:2379".into()],
        meta_addrs: vec![endpoint.clone()],
        codec: BasicCodec {
            api_version: ApiVersion::V2,
            keyspace_id: 42,
        },
    };
    let mut client = NewEtcdCli(Some(&store)).await.unwrap().unwrap();
    client.Put("direct-key", b"meta-group").await.unwrap();
    assert_eq!(
        client.Get("direct-key", false).await.unwrap(),
        vec![(b"direct-key".to_vec(), b"meta-group".to_vec())]
    );
    let mut raw = etcd_client::Client::connect([endpoint], None)
        .await
        .unwrap();
    let entries = raw.get("/keyspaces/tidb/42direct-key", None).await.unwrap();
    assert_eq!(entries.kvs().len(), 1);
    assert_eq!(entries.kvs()[0].value(), b"meta-group");
    raw.delete("/keyspaces/tidb/42direct-key", None)
        .await
        .unwrap();
}
