// Copyright 2026 AsterSQL.

use crate::ImportSizeEstimator;
use astersql_lightning_mydump::{Compression, SourceFileMeta};
use astersql_objstore_storeapi::{Context, Storage};
use std::io::{self, Read};
use std::sync::Arc;

struct MeasuredReader {
    reader: Box<dyn astersql_objstore_objectio::Reader>,
    context: Context,
    consumed: Arc<std::sync::atomic::AtomicU64>,
    limit: Option<u64>,
}

impl Read for MeasuredReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.context.check()?;
        let consumed = self.consumed.load(std::sync::atomic::Ordering::Relaxed);
        let allowed = self.limit.map_or(bytes.len(), |limit| {
            bytes.len().min(limit.saturating_sub(consumed) as usize)
        });
        if allowed == 0 {
            return Ok(0);
        }
        let count = self.reader.read(&mut bytes[..allowed])?;
        self.consumed
            .fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(count)
    }
}

impl Drop for MeasuredReader {
    fn drop(&mut self) {
        let _ = self.reader.close();
    }
}

fn decode_reader<'a>(
    reader: MeasuredReader,
    compression: Compression,
) -> Result<Box<dyn Read + 'a>, String> {
    match compression {
        Compression::Gz => Ok(Box::new(flate2::read::MultiGzDecoder::new(reader))),
        Compression::Snappy => Ok(Box::new(snap::read::FrameDecoder::new(reader))),
        Compression::Zstd => zstd::stream::read::Decoder::new(reader)
            .map(|decoder| Box::new(decoder) as Box<dyn Read>)
            .map_err(|error| error.to_string()),
        other => Err(format!("unsupported compressed import source: {other:?}")),
    }
}

fn calculate_file_bytes(
    context: &Context,
    path: &str,
    compression: Compression,
    storage: &dyn Storage,
    limit: Option<u64>,
) -> Result<(usize, u64), String> {
    context.check().map_err(|error| error.to_string())?;
    let reader = storage
        .Open(context, path, None)
        .map_err(|error| error.to_string())?;
    let consumed = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let measured = MeasuredReader {
        reader,
        context: context.clone(),
        consumed: consumed.clone(),
        limit,
    };
    let mut decoded = decode_reader(measured, compression)?;
    let mut buffer = [0_u8; 4096];
    let mut total = 0;
    if limit.is_none() {
        match decoded.read(&mut buffer) {
            Ok(count) => total = count,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {}
            Err(error) => return Err(error.to_string()),
        }
    } else {
        loop {
            match decoded.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => total += count,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    context.check().map_err(|error| error.to_string())?;
    Ok((total, consumed.load(std::sync::atomic::Ordering::Relaxed)))
}

/// Go's two-pass compressed-prefix ratio; Parquet metadata remains a host decoder boundary.
pub struct HostImportSizeEstimator {
    pub ParquetEstimator: Arc<dyn ImportSizeEstimator>,
}

impl ImportSizeEstimator for HostImportSizeEstimator {
    fn EstimateRealSize(
        &self,
        context: &Context,
        file: &SourceFileMeta,
        storage: &dyn Storage,
    ) -> Result<i64, String> {
        if file.compression == Compression::None {
            return Ok(file.file_size);
        }
        if !matches!(
            file.compression,
            Compression::Gz | Compression::Snappy | Compression::Zstd
        ) {
            return Ok(file.file_size);
        }
        let ratio = (|| {
            let (_, offset) =
                calculate_file_bytes(context, &file.path, file.compression, storage, None)?;
            if offset == 0 {
                return Err("compressed sample read zero bytes".to_owned());
            }
            let (decoded, _) =
                calculate_file_bytes(context, &file.path, file.compression, storage, Some(offset))?;
            Ok::<_, String>(decoded as f64 / offset as f64)
        })();
        match ratio {
            Ok(ratio) => Ok((ratio * file.file_size as f64) as i64),
            Err(error) if context.is_cancelled() => Err(error),
            Err(_) => Ok(file.file_size),
        }
    }

    fn ParquetExpansionRatio(
        &self,
        context: &Context,
        file_path: &str,
        file_size: i64,
        storage: &dyn Storage,
    ) -> Result<f64, String> {
        self.ParquetEstimator
            .ParquetExpansionRatio(context, file_path, file_size, storage)
    }
}
