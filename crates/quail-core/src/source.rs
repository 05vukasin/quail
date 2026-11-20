use crate::blob::Blob;
use crate::error::Result;
use crate::record::FileRecord;

/// One file handed to the engine.
#[derive(Clone, Debug)]
pub struct SourceItem {
    /// Stable name of the file: a path, `s3://bucket/key`, ...
    pub locator: String,
    /// Its bytes.
    pub blob: Blob,
    /// Last modification time, seconds since the Unix epoch, when the source knows it.
    pub modified: Option<i64>,
}

/// Where files come from: a directory walk, an S3 listing, a test fixture list. The engine
/// pulls from it at its own pace, which is the backpressure that bounds memory.
pub trait Source: Send {
    /// The next file, `None` when done. An error skips one item, it does not end the source.
    fn next_item(&mut self) -> Option<Result<SourceItem>>;
}

/// Where records go: JSONL, Arrow/Parquet, a database, a channel to the service.
pub trait RecordSink: Send {
    /// Writes one record.
    fn write(&mut self, record: &FileRecord) -> Result<()>;

    /// Flushes and closes.
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}
