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

// `metricsutil::common` 指标注册与 Keyspace 标签相关单元测试。
//
// Keyspace 是多租户命名空间；本测试验证可观测性标签与 `keyspace_id`
// 常量标签在注册后正确合并。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_config as config;
use astersql_metrics_common as metricscommon;

use super::{
    KeyspaceMeta, MetricsUtilError, PdClient, PdClientFactory, PdErrorKind, RegisterMetricsForBR,
    SecurityOption, SetPdClientFactory, TlsConfig, cloneConstLabels, getKeyspaceMetaWithRetry,
    registerMetrics, setKeyspaceIDConstLabel,
};

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct FactoryReset;

impl Drop for FactoryReset {
    fn drop(&mut self) {
        SetPdClientFactory(None);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FactoryCall {
    component: String,
    addresses: Vec<String>,
    security: SecurityOption,
    timeout: Duration,
    init_metrics: bool,
}

struct MockClientState {
    responses: Mutex<VecDeque<Result<KeyspaceMeta, MetricsUtilError>>>,
    names: Mutex<Vec<String>>,
    closed: AtomicBool,
}

struct MockClient {
    state: Arc<MockClientState>,
}

impl PdClient for MockClient {
    fn LoadKeyspace(&self, keyspace_name: &str) -> Result<KeyspaceMeta, MetricsUtilError> {
        self.state
            .names
            .lock()
            .expect("names lock")
            .push(keyspace_name.to_string());
        self.state
            .responses
            .lock()
            .expect("responses lock")
            .pop_front()
            .expect("scripted response")
    }

    fn Close(&self) {
        self.state.closed.store(true, Ordering::Release);
    }
}

struct RecordingFactory {
    call: Mutex<Option<FactoryCall>>,
    state: Arc<MockClientState>,
    creation_error: Option<MetricsUtilError>,
}

impl PdClientFactory for RecordingFactory {
    fn NewClient(
        &self,
        component: &str,
        addresses: &[String],
        security: &SecurityOption,
        timeout: Duration,
        init_metrics: bool,
    ) -> Result<Box<dyn PdClient>, MetricsUtilError> {
        *self.call.lock().expect("factory call lock") = Some(FactoryCall {
            component: component.to_string(),
            addresses: addresses.to_vec(),
            security: security.clone(),
            timeout,
            init_metrics,
        });
        if let Some(error) = &self.creation_error {
            return Err(error.clone());
        }
        Ok(Box::new(MockClient {
            state: Arc::clone(&self.state),
        }))
    }
}

fn error(kind: PdErrorKind, message: &str) -> MetricsUtilError {
    MetricsUtilError {
        kind,
        message: message.to_string(),
    }
}

fn mock_state(
    responses: impl IntoIterator<Item = Result<KeyspaceMeta, MetricsUtilError>>,
) -> Arc<MockClientState> {
    Arc::new(MockClientState {
        responses: Mutex::new(responses.into_iter().collect()),
        names: Mutex::new(Vec::new()),
        closed: AtomicBool::new(false),
    })
}

/// Corresponds to Go `TestRegisterMetricsWithKeyspaceObservabilityValues`.
/// 验证 registerMetrics 合并 KeyspaceObservability 标签，以及 setKeyspaceIDConstLabel。
#[test]
fn test_register_metrics_with_keyspace_observability_values() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    let restore = config::restore_func();
    struct Restore(Option<Box<dyn FnOnce()>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(restore) = self.0.take() {
                restore();
            }
            metricscommon::SetConstLabels(&[]);
        }
    }
    let _restore = Restore(Some(Box::new(restore)));

    let mut labels = cloneConstLabels();
    labels.insert("label_a".to_string(), "value_a".to_string());
    assert_eq!(labels["label_a"], "value_a");

    metricscommon::SetConstLabels(&["base_label".into(), "base_value".into()]);
    config::update_global(|conf: &mut config::Config| {
        conf.keyspace_observability_values = config::KeyspaceObservabilityValues {
            MetricLabels: HashMap::from([("label_a".to_string(), "value_a".to_string())]),
            ..Default::default()
        };
    });

    registerMetrics().expect("registerMetrics");
    registerMetrics().expect("registerMetrics remains safe when called again");
    labels = metricscommon::GetConstLabels();
    assert_eq!(labels["base_label"], "base_value");
    assert_eq!(labels["label_a"], "value_a");

    metricscommon::SetConstLabels(&["keyspace_name".into(), "ks".into()]);
    setKeyspaceIDConstLabel(42);
    labels = metricscommon::GetConstLabels();
    assert_eq!(labels["keyspace_name"], "ks");
    assert_eq!(labels["keyspace_id"], "42");
}

#[test]
fn test_tls_is_enabled_matches_br_tls_config() {
    assert!(
        !TlsConfig {
            cert_path: "client.pem".to_string(),
            ..Default::default()
        }
        .IsEnabled()
    );
    assert!(
        TlsConfig {
            ca_path: "ca.pem".to_string(),
            ..Default::default()
        }
        .IsEnabled()
    );
}

#[test]
fn test_register_metrics_for_br_has_a_production_pd_factory() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    SetPdClientFactory(None);

    let error = RegisterMetricsForBR(&[], &TlsConfig::default(), "keyspace")
        .expect_err("an empty PD address list must fail");

    assert_eq!(error.message, "at least one PD endpoint is required");
}

#[test]
fn test_register_metrics_for_br_passes_pd_options_retries_and_closes() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    let _factory_reset = FactoryReset;
    let state = mock_state([
        Err(error(PdErrorKind::NotBootstrapped, "NOT_BOOTSTRAPPED")),
        Err(error(PdErrorKind::KeyspaceNotExist, "ENTRY_NOT_FOUND")),
        Ok(KeyspaceMeta { id: 42 }),
    ]);
    let factory = Arc::new(RecordingFactory {
        call: Mutex::new(None),
        state: Arc::clone(&state),
        creation_error: None,
    });
    SetPdClientFactory(Some(factory.clone()));
    metricscommon::SetConstLabels(&[]);

    RegisterMetricsForBR(
        &["http://pd-1:2379".to_string(), "pd-2:2379".to_string()],
        &TlsConfig {
            ca_path: "ca.pem".to_string(),
            cert_path: "client.pem".to_string(),
            key_path: "client-key.pem".to_string(),
        },
        "analytics",
    )
    .expect("BR metrics registration");

    assert_eq!(
        *factory.call.lock().expect("factory call lock"),
        Some(FactoryCall {
            component: "tidb-metrics-util".to_string(),
            addresses: vec!["http://pd-1:2379".to_string(), "pd-2:2379".to_string()],
            security: SecurityOption {
                ca_path: "ca.pem".to_string(),
                cert_path: "client.pem".to_string(),
                key_path: "client-key.pem".to_string(),
            },
            timeout: Duration::from_secs(10),
            init_metrics: false,
        })
    );
    assert_eq!(
        *state.names.lock().expect("names lock"),
        vec!["analytics", "analytics", "analytics"]
    );
    assert!(state.closed.load(Ordering::Acquire));
    assert_eq!(metricscommon::GetConstLabels()["keyspace_id"], "42");
    metricscommon::SetConstLabels(&[]);
}

#[test]
fn test_register_metrics_for_br_closes_after_unexpected_load_error() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    let _factory_reset = FactoryReset;
    let state = mock_state([Err(error(PdErrorKind::Unexpected, "permission denied"))]);
    let factory = Arc::new(RecordingFactory {
        call: Mutex::new(None),
        state: Arc::clone(&state),
        creation_error: None,
    });
    SetPdClientFactory(Some(factory));

    let result = RegisterMetricsForBR(&["pd:2379".to_string()], &TlsConfig::default(), "ks");

    assert_eq!(
        result.expect_err("unexpected PD error").message,
        "permission denied"
    );
    assert_eq!(*state.names.lock().expect("names lock"), vec!["ks"]);
    assert!(state.closed.load(Ordering::Acquire));
}

#[test]
fn test_register_metrics_for_br_propagates_creation_error_without_a_client() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    let _factory_reset = FactoryReset;
    let state = mock_state([]);
    let factory = Arc::new(RecordingFactory {
        call: Mutex::new(None),
        state: Arc::clone(&state),
        creation_error: Some(error(PdErrorKind::Unexpected, "dial failed")),
    });
    SetPdClientFactory(Some(factory));

    let result = RegisterMetricsForBR(&["pd:2379".to_string()], &TlsConfig::default(), "ks");

    assert_eq!(result.expect_err("dial error").message, "dial failed");
    assert!(!state.closed.load(Ordering::Acquire));
}

#[test]
fn test_get_keyspace_meta_retries_to_exhaustion_with_go_backoff_shape() {
    let state = mock_state([
        Err(error(PdErrorKind::KeyspaceNotExist, "missing-1")),
        Err(error(PdErrorKind::KeyspaceNotExist, "missing-2")),
        Err(error(PdErrorKind::KeyspaceNotExist, "missing-3")),
    ]);
    let client = MockClient {
        state: Arc::clone(&state),
    };
    let mut sleeps = Vec::new();

    let result = getKeyspaceMetaWithRetry(
        &client,
        "missing",
        3,
        Duration::from_millis(7),
        |duration| sleeps.push(duration),
    );

    assert_eq!(result.expect_err("retry exhaustion").message, "missing-3");
    assert_eq!(
        *state.names.lock().expect("names lock"),
        vec!["missing", "missing", "missing"]
    );
    assert_eq!(
        sleeps,
        vec![
            Duration::from_millis(7),
            Duration::from_millis(14),
            Duration::from_millis(21),
        ]
    );
}
