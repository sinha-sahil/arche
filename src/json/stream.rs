use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::error::AppError;

pub struct JsonArrayStream {
    receiver: mpsc::Receiver<Result<Value, AppError>>,
    metadata: HashMap<String, Value>,
    _handle: JoinHandle<()>,
}

impl JsonArrayStream {
    pub(crate) fn new(
        receiver: mpsc::Receiver<Result<Value, AppError>>,
        metadata: HashMap<String, Value>,
        handle: JoinHandle<()>,
    ) -> Self {
        Self {
            receiver,
            metadata,
            _handle: handle,
        }
    }

    pub fn field<T: DeserializeOwned>(&self, name: &str) -> Result<T, AppError> {
        let value = self.metadata.get(name).ok_or_else(|| {
            AppError::internal_error(format!("JSON field not found: {name}"), None)
        })?;
        serde_json::from_value(value.clone())
            .map_err(|e| AppError::internal_error(format!("JSON field type mismatch: {e}"), None))
    }

    pub fn metadata(&self) -> &HashMap<String, Value> {
        &self.metadata
    }

    pub async fn next<T: DeserializeOwned>(&mut self) -> Option<Result<T, AppError>> {
        match self.receiver.recv().await {
            Some(Ok(value)) => Some(serde_json::from_value(value).map_err(|e| {
                AppError::internal_error(format!("JSON deserialization error: {e}"), None)
            })),
            Some(Err(e)) => Some(Err(e)),
            None => None,
        }
    }

    pub async fn next_batch<T: DeserializeOwned>(
        &mut self,
        batch_size: usize,
    ) -> Option<Result<Vec<T>, AppError>> {
        let mut batch = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            match self.next::<T>().await {
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
}
