// Copyright 2026 AsterSQL.

//! Physical TiKV import transport. Match the Go region-job protocol: locate
//! each encoded key range in PD, stream one SST UUID to every region peer,
//! then ingest the leader's returned SST metadata at one fixed commit TSO.
use std::{collections::HashMap, sync::Arc, time::Duration};
use tikv_client::proto::{import_sstpb as sst, kvrpcpb, metapb, pdpb};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

type Result<T> = std::result::Result<T, ImportError>;
type PdClient = pdpb::pd_client::PdClient<Channel>;
type ImportClient = sst::import_sst_client::ImportSstClient<Channel>;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ImportError {
    message: String,
    retryable: bool,
}
impl ImportError {
    fn permanent(error: impl std::fmt::Display) -> Self {
        Self {
            message: error.to_string(),
            retryable: false,
        }
    }
    fn retry(error: impl std::fmt::Display) -> Self {
        Self {
            message: error.to_string(),
            retryable: true,
        }
    }
}
impl From<tonic::Status> for ImportError {
    fn from(error: tonic::Status) -> Self {
        Self {
            retryable: matches!(
                error.code(),
                tonic::Code::Unavailable
                    | tonic::Code::DeadlineExceeded
                    | tonic::Code::ResourceExhausted
            ),
            message: error.to_string(),
        }
    }
}

#[derive(Default, Debug)]
pub struct ImportStats {
    pub keys: usize,
    pub bytes: usize,
    pub write_rpcs: usize,
    pub ingest_rpcs: usize,
}

/// TiKV/PD region boundaries use memcomparable bytes; WriteBatch pairs retain
/// the original user keys, as in Go codec.EncodeBytes + import_sstpb.Pair.
fn region_key(key: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity((key.len() / 8 + 1) * 9);
    for group in key.chunks_exact(8) {
        encoded.extend_from_slice(group);
        encoded.push(0xff);
    }
    let tail = &key[key.len() / 8 * 8..];
    encoded.extend_from_slice(tail);
    let padding = 8 - tail.len();
    encoded.resize(encoded.len() + padding, 0);
    encoded.push(0xff - padding as u8);
    encoded
}

async fn channel(address: &str, tls: Option<&crate::TlsConfig>) -> Result<Channel> {
    let uri = if address.contains("://") {
        address.to_owned()
    } else {
        format!(
            "{}://{address}",
            if tls.is_some() { "https" } else { "http" }
        )
    };
    let mut endpoint = Endpoint::from_shared(uri)
        .map_err(ImportError::permanent)?
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60));
    if let Some(tls) = tls {
        let mut config = ClientTlsConfig::new().ca_certificate(Certificate::from_pem(
            std::fs::read(&tls.ca_path).map_err(ImportError::permanent)?,
        ));
        if !tls.cert_path.is_empty() {
            config = config.identity(Identity::from_pem(
                std::fs::read(&tls.cert_path).map_err(ImportError::permanent)?,
                std::fs::read(&tls.key_path).map_err(ImportError::permanent)?,
            ));
        }
        endpoint = endpoint
            .tls_config(config)
            .map_err(ImportError::permanent)?;
    }
    endpoint.connect().await.map_err(ImportError::retry)
}

fn check_header(header: Option<&pdpb::ResponseHeader>) -> Result<()> {
    if let Some(error) = header.and_then(|header| header.error.as_ref()) {
        return Err(ImportError::retry(format!("PD: {}", error.message)));
    }
    Ok(())
}

struct Connection {
    pd: PdClient,
    cluster_id: u64,
    tls: Option<crate::TlsConfig>,
    stores: HashMap<u64, ImportClient>,
}
impl Connection {
    async fn connect(endpoints: &[String], tls: Option<crate::TlsConfig>) -> Result<Self> {
        let mut last = ImportError::permanent("no PD endpoints configured");
        for endpoint in endpoints {
            let connection = async {
                let mut pd = PdClient::new(channel(endpoint, tls.as_ref()).await?);
                let members = pd
                    .get_members(pdpb::GetMembersRequest::default())
                    .await?
                    .into_inner();
                check_header(members.header.as_ref())?;
                let cluster_id = members
                    .header
                    .ok_or_else(|| ImportError::permanent("PD response has no header"))?
                    .cluster_id;
                if let Some(leader_url) = members
                    .leader
                    .and_then(|leader| leader.client_urls.into_iter().next())
                {
                    pd = PdClient::new(channel(&leader_url, tls.as_ref()).await?);
                }
                Ok(Self {
                    pd,
                    cluster_id,
                    tls: tls.clone(),
                    stores: HashMap::new(),
                })
            }
            .await;
            match connection {
                Ok(connection) => return Ok(connection),
                Err(error) => last = error,
            }
        }
        Err(last)
    }
    fn header(&self) -> Option<pdpb::RequestHeader> {
        Some(pdpb::RequestHeader {
            cluster_id: self.cluster_id,
            ..Default::default()
        })
    }
    async fn store(&mut self, id: u64) -> Result<ImportClient> {
        if let Some(client) = self.stores.get(&id) {
            return Ok(client.clone());
        }
        let response = self
            .pd
            .get_store(pdpb::GetStoreRequest {
                header: self.header(),
                store_id: id,
            })
            .await?
            .into_inner();
        check_header(response.header.as_ref())?;
        let store = response
            .store
            .ok_or_else(|| ImportError::retry(format!("PD store {id} missing")))?;
        let client = ImportClient::new(channel(&store.address, self.tls.as_ref()).await?);
        self.stores.insert(id, client.clone());
        Ok(client)
    }
    async fn import_region(
        &mut self,
        pairs: &[(Vec<u8>, Vec<u8>)],
        commit_ts: u64,
        stats: &mut ImportStats,
        keyspace_id: Option<u32>,
        options: &astersql_kv::SSTImportOptions,
    ) -> Result<usize> {
        let response = self
            .pd
            .get_region(pdpb::GetRegionRequest {
                header: self.header(),
                region_key: region_key(&pairs[0].0),
                need_buckets: false,
            })
            .await?
            .into_inner();
        check_header(response.header.as_ref())?;
        let region = response
            .region
            .ok_or_else(|| ImportError::retry("PD region missing"))?;
        let leader = response
            .leader
            .ok_or_else(|| ImportError::retry("PD region has no leader"))?;
        let count = pairs.partition_point(|(key, _)| {
            region.end_key.is_empty() || region_key(key) < region.end_key
        });
        if count == 0 {
            return Err(ImportError::retry("PD returned a stale region range"));
        }
        let pairs = Arc::new(pairs[..count].to_vec());
        let meta = sst::SstMeta {
            uuid: uuid::Uuid::new_v4().as_bytes().to_vec(),
            region_id: region.id,
            region_epoch: region.region_epoch.clone(),
            api_version: if keyspace_id.is_some() {
                kvrpcpb::ApiVersion::V2 as i32
            } else {
                kvrpcpb::ApiVersion::V1 as i32
            },
            range: Some(sst::Range {
                start: region_key(&pairs[0].0),
                end: region_key(&pairs[count - 1].0),
            }),
            ..Default::default()
        };
        let mut writes = Vec::new();
        for peer in &region.peers {
            let mut client = self.store(peer.store_id).await?;
            let pairs = pairs.clone();
            let context = context(&region, peer, keyspace_id);
            let first = sst::WriteRequest {
                context: Some(context.clone()),
                chunk: Some(sst::write_request::Chunk::Meta(meta.clone())),
            };
            let options = options.clone();
            let write_failure = Arc::new(std::sync::Mutex::new(None));
            let failure = write_failure.clone();
            let store_id = peer.store_id;
            let batches = futures::stream::unfold((pairs, 0), move |(pairs, start)| {
                let context = context.clone();
                let options = options.clone();
                let failure = failure.clone();
                async move {
                    if start == pairs.len() {
                        return None;
                    }
                    let mut end = start;
                    let mut size = 0;
                    while end < pairs.len() && (end == start || size < 1024 * 1024) {
                        size += pairs[end].0.len() + pairs[end].1.len();
                        end += 1;
                    }
                    if options.context.is_cancelled() {
                        *failure.lock().unwrap() =
                            Some(ImportError::permanent("SST import cancelled"));
                        return None;
                    }
                    if let Some(limiter) = options.write_limiter {
                        let cancel = options.context.clone();
                        let waiting = tokio::task::spawn_blocking(move || {
                            limiter.WaitN(&cancel, store_id, size)
                        });
                        match waiting.await {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => {
                                *failure.lock().unwrap() = Some(ImportError::permanent(error));
                                return None;
                            }
                            Err(error) => {
                                *failure.lock().unwrap() = Some(ImportError::permanent(error));
                                return None;
                            }
                        }
                    }
                    let batch = sst::WriteBatch {
                        commit_ts,
                        pairs: pairs[start..end]
                            .iter()
                            .map(|(key, value)| sst::Pair {
                                key: key.clone(),
                                value: value.clone(),
                                ..Default::default()
                            })
                            .collect(),
                    };
                    Some((
                        sst::WriteRequest {
                            context: Some(context),
                            chunk: Some(sst::write_request::Chunk::Batch(batch)),
                        },
                        (pairs, end),
                    ))
                }
            });
            let stream = futures::StreamExt::chain(futures::stream::once(async { first }), batches);
            let peer_id = peer.id;
            writes.push(async move {
                let response = client.write(stream).await?.into_inner();
                if let Some(error) = write_failure.lock().unwrap().take() {
                    return Err(error);
                }
                if let Some(error) = response.error {
                    return Err(ImportError::retry(format!("TiKV SST write: {error:?}")));
                }
                Ok::<_, ImportError>((peer_id, response.metas))
            });
        }
        stats.write_rpcs += writes.len();
        let responses = futures::future::try_join_all(writes).await?;
        let metas = responses
            .into_iter()
            .find(|(id, _)| *id == leader.id)
            .ok_or_else(|| ImportError::retry("leader is absent from region peers"))?
            .1;
        if metas.is_empty() {
            return Err(ImportError::permanent(
                "leader returned no SST metadata for nonempty input",
            ));
        }
        let mut client = self.store(leader.store_id).await?;
        stats.ingest_rpcs += 1;
        let response = client
            .multi_ingest(sst::MultiIngestRequest {
                context: Some(context(&region, &leader, keyspace_id)),
                ssts: metas,
            })
            .await?
            .into_inner();
        if let Some(error) = response.error {
            return Err(ImportError::retry(format!("TiKV SST ingest: {error:?}")));
        }
        stats.keys += count;
        stats.bytes += pairs
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum::<usize>();
        Ok(count)
    }
}

fn context(
    region: &metapb::Region,
    peer: &metapb::Peer,
    keyspace_id: Option<u32>,
) -> kvrpcpb::Context {
    kvrpcpb::Context {
        region_id: region.id,
        region_epoch: region.region_epoch.clone(),
        peer: Some(peer.clone()),
        request_source: "internal_lightning:import".into(),
        txn_source: 1,
        api_version: if keyspace_id.is_some() {
            kvrpcpb::ApiVersion::V2 as i32
        } else {
            kvrpcpb::ApiVersion::V1 as i32
        },
        keyspace_id: keyspace_id.unwrap_or_default(),
        ..Default::default()
    }
}

/// Physically import strictly ordered unique KV pairs. Retries keep the same
/// commit timestamp and relocate the unfinished range, including after a split
/// or an ambiguous ingest response. No transactional Put/Commit fallback exists.
pub fn write_and_ingest(
    endpoints: &[String],
    tls: Option<crate::TlsConfig>,
    commit_ts: u64,
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<ImportStats> {
    write_and_ingest_with_options(
        endpoints,
        tls,
        commit_ts,
        pairs,
        None,
        astersql_kv::SSTImportOptions::default(),
    )
}

/// Runtime import controls and actual API V2 keyspace identity.
pub fn write_and_ingest_with_options(
    endpoints: &[String],
    tls: Option<crate::TlsConfig>,
    commit_ts: u64,
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
    keyspace_id: Option<u32>,
    options: astersql_kv::SSTImportOptions,
) -> Result<ImportStats> {
    if commit_ts == 0 || commit_ts > i64::MAX as u64 {
        return Err(ImportError::permanent(
            "SST import requires a valid PD commit timestamp",
        ));
    }
    if pairs.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
        return Err(ImportError::permanent(
            "SST input keys must be strictly increasing",
        ));
    }
    if pairs.is_empty() {
        return Ok(ImportStats::default());
    }
    let codec = match keyspace_id {
        Some(id) => astersql_store_copr::network_backend::KeyCodec::v2(String::new(), id)
            .map_err(ImportError::permanent)?,
        None => astersql_store_copr::network_backend::KeyCodec::v1(),
    };
    let pairs = pairs
        .into_iter()
        .map(|(key, value)| (codec.encode_key(&key), value))
        .collect::<Vec<_>>();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ImportError::permanent)?;
    runtime.block_on(async {
        let importing = async {
            let mut connection = Connection::connect(endpoints, tls.clone()).await?;
            let mut stats = ImportStats::default();
            let mut cursor = 0;
            let mut attempts = 0;
            while cursor < pairs.len() {
                match connection
                    .import_region(
                        &pairs[cursor..],
                        commit_ts,
                        &mut stats,
                        keyspace_id,
                        &options,
                    )
                    .await
                {
                    Ok(count) => {
                        cursor += count;
                        attempts = 0;
                    }
                    Err(error) if error.retryable && attempts < 29 => {
                        attempts += 1;
                        tokio::time::sleep(Duration::from_millis((100 * attempts).min(1000))).await;
                        connection = Connection::connect(endpoints, tls.clone()).await?;
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(stats)
        };
        tokio::select! {
            _ = options.context.cancelled() => Err(ImportError::permanent("SST import cancelled")),
            result = importing => result,
        }
    })
}

#[cfg(test)]
#[path = "sst_import_test.rs"]
mod tests;
