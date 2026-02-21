mod read;
mod stream;
mod writer;

pub use read::{CsvBytesReader, CsvFileReader, CsvReadBuilder, CsvUrlReader};
pub use stream::CsvRecordStream;
pub use writer::CsvRecordWriter;

use crate::error::AppError;
use csv_async::{AsyncReaderBuilder, AsyncWriterBuilder};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};

pub(crate) fn map_csv_error(err: csv_async::Error) -> AppError {
    AppError::internal_error(err.to_string(), Some("CSV processing error".to_string()))
}

#[derive(Debug, Clone)]
pub struct CsvClient {
    delimiter: u8,
    has_headers: bool,
    flexible: bool,
    quoting: bool,
    quote: u8,
    escape: Option<u8>,
    comment: Option<u8>,
}

impl Default for CsvClient {
    fn default() -> Self {
        Self {
            delimiter: b',',
            has_headers: true,
            flexible: false,
            quoting: true,
            quote: b'"',
            escape: None,
            comment: None,
        }
    }
}

impl CsvClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delimiter(mut self, delimiter: u8) -> Self {
        self.delimiter = delimiter;
        self
    }

    pub fn has_headers(mut self, has_headers: bool) -> Self {
        self.has_headers = has_headers;
        self
    }

    pub fn flexible(mut self, flexible: bool) -> Self {
        self.flexible = flexible;
        self
    }

    pub fn quoting(mut self, quoting: bool) -> Self {
        self.quoting = quoting;
        self
    }

    pub fn quote(mut self, quote: u8) -> Self {
        self.quote = quote;
        self
    }

    pub fn escape(mut self, escape: u8) -> Self {
        self.escape = Some(escape);
        self
    }

    pub fn comment(mut self, comment: u8) -> Self {
        self.comment = Some(comment);
        self
    }

    pub(crate) fn build_reader<R: AsyncRead + Unpin + Send>(
        &self,
        reader: R,
    ) -> csv_async::AsyncReader<R> {
        let mut builder = AsyncReaderBuilder::new();
        builder.delimiter(self.delimiter);
        builder.has_headers(self.has_headers);
        builder.flexible(self.flexible);
        builder.quoting(self.quoting);
        builder.quote(self.quote);
        builder.escape(self.escape);
        builder.comment(self.comment);
        builder.create_reader(reader)
    }

    fn build_serializer<W: AsyncWrite + Unpin + Send>(
        &self,
        writer: W,
    ) -> csv_async::AsyncSerializer<W> {
        let mut builder = AsyncWriterBuilder::new();
        builder.delimiter(self.delimiter);
        builder.has_headers(self.has_headers);
        if self.quoting {
            builder.quote_style(csv_async::QuoteStyle::Always);
        } else {
            builder.quote_style(csv_async::QuoteStyle::Never);
        }
        builder.quote(self.quote);
        if let Some(escape) = self.escape {
            builder.escape(escape);
        }
        builder.create_serializer(writer)
    }

    pub fn read(&self) -> CsvReadBuilder<'_> {
        CsvReadBuilder { client: self }
    }

    pub fn stream<R: AsyncRead + Unpin + Send>(&self, reader: R) -> CsvRecordStream<R> {
        CsvRecordStream {
            inner: self.build_reader(reader),
            headers: None,
        }
    }

    pub async fn write_all<T: Serialize>(&self, records: &[T]) -> Result<Vec<u8>, AppError> {
        let mut ser = self.build_serializer(Vec::new());
        for record in records {
            ser.serialize(record).await.map_err(map_csv_error)?;
        }
        ser.into_inner().await.map_err(|e| {
            AppError::internal_error(
                e.to_string(),
                Some("Failed to flush CSV writer".to_string()),
            )
        })
    }

    pub async fn write_file<T: Serialize>(
        &self,
        path: impl AsRef<std::path::Path>,
        records: &[T],
    ) -> Result<(), AppError> {
        let file = tokio::fs::File::create(path).await.map_err(|e| {
            AppError::internal_error(e.to_string(), Some("Failed to create CSV file".to_string()))
        })?;
        let mut ser = self.build_serializer(file);
        for record in records {
            ser.serialize(record).await.map_err(map_csv_error)?;
        }
        ser.into_inner().await.map_err(|e| {
            AppError::internal_error(
                e.to_string(),
                Some("Failed to flush CSV writer".to_string()),
            )
        })?;
        Ok(())
    }

    pub fn writer<W: AsyncWrite + Unpin + Send>(&self, writer: W) -> CsvRecordWriter<W> {
        CsvRecordWriter {
            inner: self.build_serializer(writer),
        }
    }

    pub async fn writer_to_file(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<CsvRecordWriter<tokio::fs::File>, AppError> {
        let file = tokio::fs::File::create(path).await.map_err(|e| {
            AppError::internal_error(e.to_string(), Some("Failed to create CSV file".to_string()))
        })?;
        Ok(self.writer(file))
    }
}
