// Copyright 2026 AsterSQL.

use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::Duration;

use super::*;

struct Body(Cursor<Vec<u8>>);

impl Read for Body {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.0.read(output)
    }
}

impl ReadCloser for Body {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FailingBody;

impl Read for FailingBody {
    fn read(&mut self, _output: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "read failed",
        ))
    }
}

impl ReadCloser for FailingBody {
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct ReopenFailClient;
static PRESIGN_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl PrefixClient for ReopenFailClient {
    fn CheckBucketExistence(&self, _: &storeapi::Context) -> anyhow::Result<()> {
        unreachable!()
    }
    fn CheckListObjects(&self, _: &storeapi::Context) -> anyhow::Result<()> {
        unreachable!()
    }
    fn CheckGetObject(&self, _: &storeapi::Context) -> anyhow::Result<()> {
        unreachable!()
    }
    fn CheckPutAndDeleteObject(&self, _: &storeapi::Context) -> anyhow::Result<()> {
        unreachable!()
    }
    fn GetObject(
        &self,
        _: &storeapi::Context,
        _: &str,
        _: i64,
        _: i64,
    ) -> anyhow::Result<Option<GetResp>> {
        Err(anyhow::anyhow!("reopen failed"))
    }
    fn PutObject(&self, _: &storeapi::Context, _: &str, _: &[u8]) -> anyhow::Result<()> {
        unreachable!()
    }
    fn DeleteObject(&self, _: &storeapi::Context, _: &str) -> anyhow::Result<()> {
        unreachable!()
    }
    fn DeleteObjects(&self, _: &storeapi::Context, _: &[String]) -> anyhow::Result<()> {
        unreachable!()
    }
    fn HeadObject(&self, _: &storeapi::Context, _: &str) -> anyhow::Result<Option<HeadObjectResp>> {
        unreachable!()
    }
    fn IsObjectExists(&self, _: &storeapi::Context, _: &str) -> anyhow::Result<bool> {
        unreachable!()
    }
    fn ListObjects(
        &self,
        _: &storeapi::Context,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: isize,
    ) -> anyhow::Result<Option<ListResp>> {
        unreachable!()
    }
    fn CopyObject(&self, _: &storeapi::Context, _: &CopyInput) -> anyhow::Result<()> {
        unreachable!()
    }
    fn MultipartWriter(
        &self,
        _: &storeapi::Context,
        _: &str,
    ) -> anyhow::Result<Option<Box<dyn objectio::Writer>>> {
        unreachable!()
    }
    fn MultipartUploader(&self, _: &str, _: i64, _: i32) -> Option<Box<dyn Uploader>> {
        unreachable!()
    }
    fn PresignObject(&self, _: &storeapi::Context, _: &str, _: Duration) -> anyhow::Result<String> {
        PRESIGN_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("https://example.com/object".to_owned())
    }
}

#[test]
fn presign_file_rejects_non_positive_expiration_before_calling_client() {
    PRESIGN_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    let storage = storage();
    let ctx = storeapi::Context::default();

    for expire in [Duration::ZERO] {
        let error = storage.PresignFile(&ctx, "object", expire).unwrap_err();
        assert!(error.to_string().contains("expiration must be positive"));
    }
    assert_eq!(PRESIGN_CALLS.load(std::sync::atomic::Ordering::SeqCst), 0);
}

fn storage() -> Storage {
    NewStorage(
        ReopenFailClient,
        storeapi::NewBucketPrefix("bucket", "root"),
        backuppb::S3::default(),
        None,
    )
}

#[test]
fn read_preserves_original_error_when_reopen_fails() {
    let mut reader = S3ObjectReader::new(
        Arc::new(storage()),
        "object".to_owned(),
        Box::new(FailingBody),
        RangeInfo {
            Start: 0,
            End: 9,
            Size: 10,
        },
        storeapi::Context::default(),
        0,
    );

    let error = reader.read(&mut [0; 1]).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert_eq!(error.to_string(), "read failed");
}

#[test]
fn short_seek_reports_eof_when_no_byte_was_discarded() {
    let mut reader = S3ObjectReader::new(
        Arc::new(storage()),
        "object".to_owned(),
        Box::new(Body(Cursor::new(Vec::new()))),
        RangeInfo {
            Start: 0,
            End: 9,
            Size: 10,
        },
        storeapi::Context::default(),
        0,
    );

    let error = reader.seek(SeekFrom::Current(1)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    assert_eq!(error.to_string(), io::ErrorKind::UnexpectedEof.to_string());
}
