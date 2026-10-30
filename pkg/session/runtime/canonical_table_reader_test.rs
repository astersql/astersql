// Copyright 2026 AsterSQL.

use super::*;

struct Subset {
    data: Vec<u8>,
    evidence: kv::CopRuntimeEvidence,
}
impl kv::ResultSubset for Subset {
    fn CopRuntimeEvidence(&self) -> Option<kv::CopRuntimeEvidence> {
        Some(self.evidence)
    }
    fn GetData(&self) -> &[u8] {
        &self.data
    }
    fn GetStartKey(&self) -> kv::Key {
        kv::Key(Vec::new())
    }
    fn MemSize(&self) -> i64 {
        self.data.len() as i64
    }
    fn RespTime(&self) -> Duration {
        Duration::ZERO
    }
}
struct Response(VecDeque<Subset>);
impl kv::Response for Response {
    fn Next(
        &mut self,
        _ctx: &kv::Context,
    ) -> Result<Option<Box<dyn kv::ResultSubset>>, kv::errors::SharedError> {
        Ok(self
            .0
            .pop_front()
            .map(|subset| Box::new(subset) as Box<dyn kv::ResultSubset>))
    }
    fn Close(&mut self) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }
}

#[test]
fn port_74dc8d6e87_table_reader_keeps_backend_totals_across_responses() {
    let (domain, _session) = super::super::CreateAnalyzeSession().unwrap();
    // Backend filtering can return no rows while still reporting real scan work.
    let data = tipb::SelectResponse::new().write_to_bytes().unwrap();
    let responses = [123, 77]
        .into_iter()
        .map(|total_bytes| Subset {
            data: data.clone(),
            evidence: kv::CopRuntimeEvidence {
                total_keys: 10,
                processed_keys: 2,
                processed_bytes: 6,
                total_bytes,
                tikv_response_bytes: Some(9),
            },
        })
        .collect();
    let shared = Arc::new(std::sync::Mutex::new(None));
    let mut reader = CanonicalTableReaderExecutor {
        domain: domain.clone(),
        request: None,
        response: Some(Box::new(Response(responses))),
        columns: vec![],
        schema: vec![],
        pending: VecDeque::new(),
        initial_capacity: 32,
        maximum_chunk_size: 1024,
        scanned_rows: 0,
        plan_id: 42,
        evidence: Default::default(),
        shared_evidence: shared.clone(),
        opened: true,
        closed: false,
    };
    assert!(reader.fetch_response().unwrap());
    assert_eq!(shared.lock().unwrap().unwrap().1.total_bytes, 123);
    assert!(reader.fetch_response().unwrap());
    assert!(!reader.fetch_response().unwrap());
    let frozen = shared.lock().unwrap().unwrap().1;
    assert_eq!(frozen.total_bytes, 200);
    assert_eq!(
        (
            frozen.total_keys,
            frozen.processed_keys,
            frozen.processed_bytes
        ),
        (20, 4, 12)
    );
    assert_eq!(frozen.tikv_response_bytes, Some(18));
    reader.Close().unwrap();
    assert_eq!(shared.lock().unwrap().unwrap().1, frozen);
    domain.close();
}
