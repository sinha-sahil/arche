use std::collections::HashMap;
use std::io::{Cursor, Read};

use serde::Deserializer;
use serde::de::DeserializeSeed;
use serde_json::{self as json, Value};
use tokio::io::BufReader;
use tokio::sync::{mpsc, oneshot};
use tokio::task;
use tokio_util::io::SyncIoBridge;

use crate::error::AppError;

use super::stream::JsonArrayStream;
use super::visitor::{ArrayStreamSeed, ObjectStreamVisitor};

pub struct JsonClient;

pub struct JsonSource {
    reader: Box<dyn Read + Send + 'static>,
}

impl Default for JsonClient {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonClient {
    pub fn new() -> Self {
        JsonClient
    }

    pub async fn from_s3(
        self,
        s3_client: &aws_sdk_s3::Client,
        bucket: &str,
        key: &str,
    ) -> Result<JsonSource, AppError> {
        let response = s3_client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| AppError::internal_error(format!("S3 error: {e}"), None))?;

        let async_reader = response.body.into_async_read();
        let buf_reader = BufReader::with_capacity(64 * 1024, async_reader);
        let sync_reader = SyncIoBridge::new(buf_reader);

        Ok(JsonSource {
            reader: Box::new(sync_reader),
        })
    }

    pub fn from_bytes(self, bytes: &[u8]) -> JsonSource {
        JsonSource {
            reader: Box::new(Cursor::new(bytes.to_vec())),
        }
    }
}

impl JsonSource {
    pub async fn stream_array(self, field_name: &str) -> JsonArrayStream {
        let (element_tx, element_rx) = mpsc::channel::<Result<Value, AppError>>(4);
        let (metadata_tx, metadata_rx) = oneshot::channel();
        let field = field_name.to_string();

        let error_tx = element_tx.clone();
        let handle = task::spawn_blocking(move || {
            let mut de = json::Deserializer::from_reader(self.reader);
            let visitor = ObjectStreamVisitor {
                target_field: field,
                metadata_tx: Some(metadata_tx),
                element_tx,
            };
            if let Err(e) = de.deserialize_map(visitor) {
                let _ = error_tx.blocking_send(Err(AppError::internal_error(
                    format!("JSON parse error: {e}"),
                    None,
                )));
            }
        });

        let metadata = metadata_rx.await.unwrap_or_default();

        JsonArrayStream::new(element_rx, metadata, handle)
    }

    pub fn stream_root_array(self) -> JsonArrayStream {
        let (element_tx, element_rx) = mpsc::channel::<Result<Value, AppError>>(4);

        let handle = task::spawn_blocking(move || {
            let mut de = json::Deserializer::from_reader(self.reader);
            let seed = ArrayStreamSeed {
                element_tx: &element_tx,
            };
            let result = DeserializeSeed::deserialize(seed, &mut de);
            if let Err(e) = result {
                let _ = element_tx.blocking_send(Err(AppError::internal_error(
                    format!("JSON parse error: {e}"),
                    None,
                )));
            }
        });

        JsonArrayStream::new(element_rx, HashMap::new(), handle)
    }
}
