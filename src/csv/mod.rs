use crate::error::AppError;
use csv_async::{AsyncReader, AsyncReaderBuilder, AsyncSerializer, AsyncWriterBuilder};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

fn map_csv_error(err: csv_async::Error) -> AppError {
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

    fn build_reader<R: AsyncRead + Unpin + Send>(&self, reader: R) -> AsyncReader<R> {
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

    fn build_serializer<W: AsyncWrite + Unpin + Send>(&self, writer: W) -> AsyncSerializer<W> {
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

    pub async fn read_all<T>(&self, data: &[u8]) -> Result<Vec<T>, AppError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut rdr = self.build_reader(data);
        let headers = rdr.headers().await.map_err(map_csv_error)?.clone();
        let header_ref = if headers.is_empty() {
            None
        } else {
            Some(&headers)
        };
        let mut records = Vec::new();
        let mut record = csv_async::StringRecord::new();
        while rdr.read_record(&mut record).await.map_err(map_csv_error)? {
            let item: T = record.deserialize(header_ref).map_err(map_csv_error)?;
            records.push(item);
        }
        Ok(records)
    }

    pub async fn read_file<T>(&self, path: impl AsRef<std::path::Path>) -> Result<Vec<T>, AppError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let file = tokio::fs::File::open(path).await.map_err(|e| {
            AppError::internal_error(e.to_string(), Some("Failed to open CSV file".to_string()))
        })?;
        let mut rdr = self.build_reader(file);
        let headers = rdr.headers().await.map_err(map_csv_error)?.clone();
        let header_ref = if headers.is_empty() {
            None
        } else {
            Some(&headers)
        };
        let mut records = Vec::new();
        let mut record = csv_async::StringRecord::new();
        while rdr.read_record(&mut record).await.map_err(map_csv_error)? {
            let item: T = record.deserialize(header_ref).map_err(map_csv_error)?;
            records.push(item);
        }
        Ok(records)
    }

    pub async fn read_records(
        &self,
        data: &[u8],
    ) -> Result<Vec<csv_async::StringRecord>, AppError> {
        let mut rdr = self.build_reader(data);
        let mut records = Vec::new();
        let mut record = csv_async::StringRecord::new();
        while rdr.read_record(&mut record).await.map_err(map_csv_error)? {
            records.push(record.clone());
        }
        Ok(records)
    }

    pub async fn read_file_records(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<Vec<csv_async::StringRecord>, AppError> {
        let file = tokio::fs::File::open(path).await.map_err(|e| {
            AppError::internal_error(e.to_string(), Some("Failed to open CSV file".to_string()))
        })?;
        let mut rdr = self.build_reader(file);
        let mut records = Vec::new();
        let mut record = csv_async::StringRecord::new();
        while rdr.read_record(&mut record).await.map_err(map_csv_error)? {
            records.push(record.clone());
        }
        Ok(records)
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

    pub fn reader<R: AsyncRead + Unpin + Send>(&self, reader: R) -> CsvRecordStream<R> {
        CsvRecordStream {
            inner: self.build_reader(reader),
            headers: None,
        }
    }

    pub async fn reader_from_file(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<CsvRecordStream<tokio::fs::File>, AppError> {
        let file = tokio::fs::File::open(path).await.map_err(|e| {
            AppError::internal_error(e.to_string(), Some("Failed to open CSV file".to_string()))
        })?;
        Ok(self.reader(file))
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

pub struct CsvRecordStream<R: AsyncRead + Unpin + Send> {
    inner: AsyncReader<R>,
    headers: Option<csv_async::StringRecord>,
}

impl<R: AsyncRead + Unpin + Send> CsvRecordStream<R> {
    pub async fn next_deserialized<T>(&mut self) -> Option<Result<T, AppError>>
    where
        T: for<'de> Deserialize<'de>,
    {
        if self.headers.is_none() {
            match self.inner.headers().await {
                Ok(h) => self.headers = Some(h.clone()),
                Err(e) => return Some(Err(map_csv_error(e))),
            }
        }
        let header_ref = self
            .headers
            .as_ref()
            .and_then(|h| if h.is_empty() { None } else { Some(h) });
        let mut record = csv_async::StringRecord::new();
        match self.inner.read_record(&mut record).await {
            Ok(true) => Some(record.deserialize(header_ref).map_err(map_csv_error)),
            Ok(false) => None,
            Err(e) => Some(Err(map_csv_error(e))),
        }
    }

    pub async fn next_record(&mut self) -> Option<Result<csv_async::StringRecord, AppError>> {
        let mut record = csv_async::StringRecord::new();
        match self.inner.read_record(&mut record).await {
            Ok(true) => Some(Ok(record)),
            Ok(false) => None,
            Err(e) => Some(Err(map_csv_error(e))),
        }
    }

    pub async fn headers(&mut self) -> Result<&csv_async::StringRecord, AppError> {
        if self.headers.is_none() {
            let h = self.inner.headers().await.map_err(map_csv_error)?.clone();
            self.headers = Some(h);
        }
        Ok(self.headers.as_ref().unwrap())
    }

    pub fn into_inner(self) -> AsyncReader<R> {
        self.inner
    }
}

pub struct CsvRecordWriter<W: AsyncWrite + Unpin + Send> {
    inner: AsyncSerializer<W>,
}

impl<W: AsyncWrite + Unpin + Send> CsvRecordWriter<W> {
    pub async fn serialize<T: Serialize>(&mut self, record: &T) -> Result<(), AppError> {
        self.inner.serialize(record).await.map_err(map_csv_error)
    }

    pub async fn write_fields<I, T>(&mut self, fields: I) -> Result<(), AppError>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<[u8]>,
    {
        let string_fields: Vec<String> = fields
            .into_iter()
            .map(|f| String::from_utf8_lossy(f.as_ref()).into_owned())
            .collect();
        self.inner
            .serialize(string_fields)
            .await
            .map_err(map_csv_error)
    }

    pub async fn finish(self) -> Result<W, AppError> {
        self.inner.into_inner().await.map_err(|e| {
            AppError::internal_error(
                e.to_string(),
                Some("Failed to flush CSV writer".to_string()),
            )
        })
    }
}
