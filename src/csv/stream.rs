use crate::error::AppError;
use csv_async::AsyncReader;
use serde::Deserialize;
use tokio::io::AsyncRead;

use super::map_csv_error;

pub struct CsvRecordStream<R: AsyncRead + Unpin + Send> {
    pub(super) inner: AsyncReader<R>,
    pub(super) headers: Option<csv_async::StringRecord>,
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

    pub async fn next_batch_deserialized<T>(
        &mut self,
        batch_size: usize,
    ) -> Option<Result<Vec<T>, AppError>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut batch = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            match self.next_deserialized::<T>().await {
                Some(Ok(item)) => batch.push(item),
                Some(Err(e)) => return Some(Err(e)),
                None => break,
            }
        }
        if batch.is_empty() {
            None
        } else {
            Some(Ok(batch))
        }
    }

    pub async fn next_batch(
        &mut self,
        batch_size: usize,
    ) -> Option<Result<Vec<csv_async::StringRecord>, AppError>> {
        let mut batch = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            match self.next_record().await {
                Some(Ok(record)) => batch.push(record),
                Some(Err(e)) => return Some(Err(e)),
                None => break,
            }
        }
        if batch.is_empty() {
            None
        } else {
            Some(Ok(batch))
        }
    }

    pub fn into_inner(self) -> AsyncReader<R> {
        self.inner
    }
}
