use crate::error::AppError;
use csv_async::AsyncReader;
use serde::Deserialize;
use std::future::Future;
use std::path::PathBuf;
use tokio::io::AsyncRead;

use super::stream::CsvRecordStream;
use super::{CsvClient, map_csv_error};

pub struct CsvReadBuilder<'c> {
    pub(super) client: &'c CsvClient,
}

impl<'c> CsvReadBuilder<'c> {
    pub fn from_bytes<'d>(self, data: &'d [u8]) -> CsvBytesReader<'c, 'd> {
        CsvBytesReader {
            client: self.client,
            data,
        }
    }

    pub fn from_file(self, path: impl AsRef<std::path::Path>) -> CsvFileReader<'c> {
        CsvFileReader {
            client: self.client,
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn from_url(self, url: &str) -> CsvUrlReader<'c> {
        CsvUrlReader {
            client: self.client,
            url: url.to_owned(),
        }
    }
}

pub struct CsvBytesReader<'c, 'd> {
    client: &'c CsvClient,
    data: &'d [u8],
}

impl<'c, 'd> CsvBytesReader<'c, 'd> {
    pub async fn deserialize<T>(self) -> Result<Vec<T>, AppError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut rdr = self.client.build_reader(self.data);
        collect_deserialized(&mut rdr).await
    }

    pub async fn records(self) -> Result<Vec<csv_async::StringRecord>, AppError> {
        let mut rdr = self.client.build_reader(self.data);
        collect_records(&mut rdr).await
    }

    pub async fn deserialize_batched<T, F, Fut>(
        self,
        batch_size: usize,
        f: F,
    ) -> Result<(), AppError>
    where
        T: for<'de> Deserialize<'de>,
        F: Fn(Vec<T>) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        let mut rdr = self.client.build_reader(self.data);
        batched_deserialize(&mut rdr, batch_size, f).await
    }

    pub async fn records_batched<F, Fut>(self, batch_size: usize, f: F) -> Result<(), AppError>
    where
        F: Fn(Vec<csv_async::StringRecord>) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        let mut rdr = self.client.build_reader(self.data);
        batched_records(&mut rdr, batch_size, f).await
    }

    pub fn stream(self) -> CsvRecordStream<&'d [u8]> {
        self.client.stream(self.data)
    }
}

pub struct CsvFileReader<'c> {
    client: &'c CsvClient,
    path: PathBuf,
}

impl<'c> CsvFileReader<'c> {
    pub async fn deserialize<T>(self) -> Result<Vec<T>, AppError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let file = open_csv_file(&self.path).await?;
        let mut rdr = self.client.build_reader(file);
        collect_deserialized(&mut rdr).await
    }

    pub async fn records(self) -> Result<Vec<csv_async::StringRecord>, AppError> {
        let file = open_csv_file(&self.path).await?;
        let mut rdr = self.client.build_reader(file);
        collect_records(&mut rdr).await
    }

    pub async fn deserialize_batched<T, F, Fut>(
        self,
        batch_size: usize,
        f: F,
    ) -> Result<(), AppError>
    where
        T: for<'de> Deserialize<'de>,
        F: Fn(Vec<T>) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        let file = open_csv_file(&self.path).await?;
        let mut rdr = self.client.build_reader(file);
        batched_deserialize(&mut rdr, batch_size, f).await
    }

    pub async fn records_batched<F, Fut>(self, batch_size: usize, f: F) -> Result<(), AppError>
    where
        F: Fn(Vec<csv_async::StringRecord>) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        let file = open_csv_file(&self.path).await?;
        let mut rdr = self.client.build_reader(file);
        batched_records(&mut rdr, batch_size, f).await
    }

    pub async fn stream(self) -> Result<CsvRecordStream<tokio::fs::File>, AppError> {
        let file = open_csv_file(&self.path).await?;
        Ok(self.client.stream(file))
    }
}

pub struct CsvUrlReader<'c> {
    client: &'c CsvClient,
    url: String,
}

impl<'c> CsvUrlReader<'c> {
    pub async fn deserialize<T>(self) -> Result<Vec<T>, AppError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let bytes = fetch_url_bytes(&self.url).await?;
        let mut rdr = self.client.build_reader(bytes.as_slice());
        collect_deserialized(&mut rdr).await
    }

    pub async fn records(self) -> Result<Vec<csv_async::StringRecord>, AppError> {
        let bytes = fetch_url_bytes(&self.url).await?;
        let mut rdr = self.client.build_reader(bytes.as_slice());
        collect_records(&mut rdr).await
    }

    pub async fn deserialize_batched<T, F, Fut>(
        self,
        batch_size: usize,
        f: F,
    ) -> Result<(), AppError>
    where
        T: for<'de> Deserialize<'de>,
        F: Fn(Vec<T>) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        let bytes = fetch_url_bytes(&self.url).await?;
        let mut rdr = self.client.build_reader(bytes.as_slice());
        batched_deserialize(&mut rdr, batch_size, f).await
    }

    pub async fn records_batched<F, Fut>(self, batch_size: usize, f: F) -> Result<(), AppError>
    where
        F: Fn(Vec<csv_async::StringRecord>) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        let bytes = fetch_url_bytes(&self.url).await?;
        let mut rdr = self.client.build_reader(bytes.as_slice());
        batched_records(&mut rdr, batch_size, f).await
    }
}

async fn fetch_url_bytes(url: &str) -> Result<Vec<u8>, AppError> {
    let response = reqwest::get(url).await.map_err(|e| {
        AppError::internal_error(
            e.to_string(),
            Some("Failed to fetch CSV from URL".to_string()),
        )
    })?;
    Ok(response
        .bytes()
        .await
        .map_err(|e| {
            AppError::internal_error(
                e.to_string(),
                Some("Failed to read CSV response body".to_string()),
            )
        })?
        .to_vec())
}

async fn open_csv_file(path: impl AsRef<std::path::Path>) -> Result<tokio::fs::File, AppError> {
    tokio::fs::File::open(path).await.map_err(|e| {
        AppError::internal_error(e.to_string(), Some("Failed to open CSV file".to_string()))
    })
}

async fn collect_deserialized<T, R>(rdr: &mut AsyncReader<R>) -> Result<Vec<T>, AppError>
where
    T: for<'de> Deserialize<'de>,
    R: AsyncRead + Unpin + Send,
{
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

async fn collect_records<R>(
    rdr: &mut AsyncReader<R>,
) -> Result<Vec<csv_async::StringRecord>, AppError>
where
    R: AsyncRead + Unpin + Send,
{
    let mut records = Vec::new();
    let mut record = csv_async::StringRecord::new();
    while rdr.read_record(&mut record).await.map_err(map_csv_error)? {
        records.push(record.clone());
    }
    Ok(records)
}

async fn batched_deserialize<T, R, F, Fut>(
    rdr: &mut AsyncReader<R>,
    batch_size: usize,
    f: F,
) -> Result<(), AppError>
where
    T: for<'de> Deserialize<'de>,
    R: AsyncRead + Unpin + Send,
    F: Fn(Vec<T>) -> Fut,
    Fut: Future<Output = Result<(), AppError>>,
{
    let headers = rdr.headers().await.map_err(map_csv_error)?.clone();
    let header_ref = if headers.is_empty() {
        None
    } else {
        Some(&headers)
    };
    let mut batch = Vec::with_capacity(batch_size);
    let mut record = csv_async::StringRecord::new();
    while rdr.read_record(&mut record).await.map_err(map_csv_error)? {
        let item: T = record.deserialize(header_ref).map_err(map_csv_error)?;
        batch.push(item);
        if batch.len() >= batch_size {
            f(batch).await?;
            batch = Vec::with_capacity(batch_size);
        }
    }
    if !batch.is_empty() {
        f(batch).await?;
    }
    Ok(())
}

async fn batched_records<R, F, Fut>(
    rdr: &mut AsyncReader<R>,
    batch_size: usize,
    f: F,
) -> Result<(), AppError>
where
    R: AsyncRead + Unpin + Send,
    F: Fn(Vec<csv_async::StringRecord>) -> Fut,
    Fut: Future<Output = Result<(), AppError>>,
{
    let mut batch = Vec::with_capacity(batch_size);
    let mut record = csv_async::StringRecord::new();
    while rdr.read_record(&mut record).await.map_err(map_csv_error)? {
        batch.push(record.clone());
        if batch.len() >= batch_size {
            f(batch).await?;
            batch = Vec::with_capacity(batch_size);
        }
    }
    if !batch.is_empty() {
        f(batch).await?;
    }
    Ok(())
}
