use crate::error::AppError;
use csv_async::AsyncSerializer;
use serde::Serialize;
use tokio::io::AsyncWrite;

use super::map_csv_error;

pub struct CsvRecordWriter<W: AsyncWrite + Unpin + Send> {
    pub(super) inner: AsyncSerializer<W>,
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
