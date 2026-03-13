use std::collections::HashMap;
use std::fmt;
use std::mem;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::error::AppError;

pub(crate) struct ObjectStreamVisitor {
    pub target_field: String,
    pub metadata_tx: Option<oneshot::Sender<HashMap<String, Value>>>,
    pub element_tx: mpsc::Sender<Result<Value, AppError>>,
}

impl<'de> Visitor<'de> for ObjectStreamVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<M>(self, mut map: M) -> Result<(), M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut metadata = HashMap::new();
        let mut metadata_tx = self.metadata_tx;
        let element_tx = self.element_tx;
        let target_field = self.target_field;

        while let Some(key) = map.next_key::<String>()? {
            if key == target_field {
                if let Some(tx) = metadata_tx.take() {
                    let _ = tx.send(mem::take(&mut metadata));
                }
                map.next_value_seed(ArrayStreamSeed {
                    element_tx: &element_tx,
                })?;
            } else {
                let value: Value = map.next_value()?;
                metadata.insert(key, value);
            }
        }

        if let Some(tx) = metadata_tx {
            let _ = tx.send(metadata);
        }

        Ok(())
    }
}

pub(crate) struct ArrayStreamSeed<'a> {
    pub element_tx: &'a mpsc::Sender<Result<Value, AppError>>,
}

impl<'de, 'a> DeserializeSeed<'de> for ArrayStreamSeed<'a> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_seq(self)
    }
}

impl<'de, 'a> Visitor<'de> for ArrayStreamSeed<'a> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON array")
    }

    fn visit_seq<S>(self, mut seq: S) -> Result<(), S::Error>
    where
        S: SeqAccess<'de>,
    {
        while let Some(value) = seq.next_element::<Value>()? {
            if self.element_tx.blocking_send(Ok(value)).is_err() {
                break;
            }
        }
        Ok(())
    }
}
