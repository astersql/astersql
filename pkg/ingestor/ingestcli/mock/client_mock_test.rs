// Copyright 2026 AsterSQL.

use super::*;
use astersql_ingestor_ingestcli::{
    Client, IngestRequest, Pair, WriteClient, WriteRequest, WriteResponse,
};

fn ingest_request(region_id: u64) -> IngestRequest {
    let mut request = IngestRequest::default();
    request.region.region.id = region_id;
    request
}

fn write_request(key: &[u8]) -> WriteRequest {
    WriteRequest {
        pairs: vec![Pair {
            key: key.to_vec(),
            value: Vec::new(),
        }],
    }
}

#[test]
fn client_matches_later_expectations_without_consuming_earlier_ones() {
    let client = NewMockClient();
    let recorder = client.EXPECT();
    recorder
        .Ingest(|request| request.region.region.id == 1, |_| Ok(()))
        .Ingest(|request| request.region.region.id == 2, |_| Ok(()))
        .WriteClient(Some(1), || Ok(Box::new(NewMockWriteClient())))
        .WriteClient(Some(2), || Ok(Box::new(NewMockWriteClient())));

    client.ingest(&(), ingest_request(2)).unwrap();
    client.ingest(&(), ingest_request(1)).unwrap();
    client.write_client(&(), 2).unwrap();
    client.write_client(&(), 1).unwrap();

    assert_eq!(client.calls().len(), 4);
    assert_eq!(client.verify(), Ok(()));
}

#[test]
fn write_client_matches_later_write_expectation_without_consuming_earlier_one() {
    let mut client = NewMockWriteClient();
    client
        .EXPECT()
        .Write(|request| request.pairs[0].key == b"first", |_| Ok(()))
        .Write(|request| request.pairs[0].key == b"second", |_| Ok(()))
        .Recv(|| Ok(WriteResponse::default()))
        .Close();

    client.write(write_request(b"second")).unwrap();
    client.write(write_request(b"first")).unwrap();
    client.recv().unwrap();
    client.close();

    assert_eq!(client.calls().len(), 4);
    assert_eq!(client.verify(), Ok(()));
}

#[test]
fn unexpected_result_returning_calls_are_reported_by_verify() {
    let client = NewMockClient();
    assert!(client.ingest(&(), IngestRequest::default()).is_err());
    assert!(client.write_client(&(), 42).is_err());
    let error = client.verify().unwrap_err();
    assert!(
        error
            .messages
            .iter()
            .any(|message| message.contains("Ingest"))
    );
    assert!(
        error
            .messages
            .iter()
            .any(|message| message.contains("WriteClient"))
    );

    let mut write_client = NewMockWriteClient();
    assert!(write_client.write(WriteRequest::default()).is_err());
    assert!(write_client.recv().is_err());
    let error = write_client.verify().unwrap_err();
    assert!(
        error
            .messages
            .iter()
            .any(|message| message.contains("Write"))
    );
    assert!(
        error
            .messages
            .iter()
            .any(|message| message.contains("Recv"))
    );
}
