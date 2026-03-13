mod client;
mod stream;
mod visitor;

pub use client::{JsonClient, JsonSource};
pub use stream::JsonArrayStream;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Item {
        id: u32,
        name: String,
    }

    #[tokio::test]
    async fn test_stream_array() {
        let json = br#"{"items": [{"id": 1, "name": "a"}, {"id": 2, "name": "b"}]}"#;
        let mut stream = JsonClient::new()
            .from_bytes(json)
            .stream_array("items")
            .await;

        let item1: Item = stream.next().await.unwrap().unwrap();
        assert_eq!(
            item1,
            Item {
                id: 1,
                name: "a".to_string()
            }
        );

        let item2: Item = stream.next().await.unwrap().unwrap();
        assert_eq!(
            item2,
            Item {
                id: 2,
                name: "b".to_string()
            }
        );

        assert!(stream.next::<Item>().await.is_none());
    }

    #[tokio::test]
    async fn test_stream_root_array() {
        let json = br#"[{"id": 1, "name": "a"}, {"id": 2, "name": "b"}]"#;
        let mut stream = JsonClient::new().from_bytes(json).stream_root_array();

        let item1: Item = stream.next().await.unwrap().unwrap();
        assert_eq!(
            item1,
            Item {
                id: 1,
                name: "a".to_string()
            }
        );

        let item2: Item = stream.next().await.unwrap().unwrap();
        assert_eq!(
            item2,
            Item {
                id: 2,
                name: "b".to_string()
            }
        );

        assert!(stream.next::<Item>().await.is_none());
    }

    #[tokio::test]
    async fn test_metadata_capture() {
        let json = br#"{"version": "1.0", "count": 2, "items": [{"id": 1, "name": "a"}]}"#;
        let mut stream = JsonClient::new()
            .from_bytes(json)
            .stream_array("items")
            .await;

        let version: String = stream.field("version").unwrap();
        assert_eq!(version, "1.0");

        let count: u32 = stream.field("count").unwrap();
        assert_eq!(count, 2);

        let item: Item = stream.next().await.unwrap().unwrap();
        assert_eq!(
            item,
            Item {
                id: 1,
                name: "a".to_string()
            }
        );
    }

    #[tokio::test]
    async fn test_empty_array() {
        let json = br#"{"items": []}"#;
        let mut stream = JsonClient::new()
            .from_bytes(json)
            .stream_array("items")
            .await;

        assert!(stream.next::<Item>().await.is_none());
    }

    #[tokio::test]
    async fn test_missing_target_field() {
        let json = br#"{"version": "1.0", "count": 42}"#;
        let mut stream = JsonClient::new()
            .from_bytes(json)
            .stream_array("items")
            .await;

        assert!(stream.next::<Item>().await.is_none());

        let version: String = stream.field("version").unwrap();
        assert_eq!(version, "1.0");

        let count: u32 = stream.field("count").unwrap();
        assert_eq!(count, 42);
    }

    #[tokio::test]
    async fn test_malformed_json() {
        let json = br#"not valid json"#;
        let mut stream = JsonClient::new()
            .from_bytes(json)
            .stream_array("items")
            .await;

        let result = stream.next::<Item>().await;
        assert!(matches!(result, Some(Err(AppError::InternalError { .. }))));
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct WrongType {
        x: Vec<String>,
    }

    #[tokio::test]
    async fn test_type_mismatch() {
        let json = br#"{"items": [{"id": 1, "name": "a"}]}"#;
        let mut stream = JsonClient::new()
            .from_bytes(json)
            .stream_array("items")
            .await;

        let result = stream.next::<WrongType>().await;
        assert!(matches!(result, Some(Err(AppError::InternalError { .. }))));
    }

    #[tokio::test]
    async fn test_batch_api() {
        let items: Vec<serde_json::Value> = (1..=7)
            .map(|i| serde_json::json!({"id": i, "name": format!("item{}", i)}))
            .collect();
        let json = serde_json::to_vec(&serde_json::json!({"items": items})).unwrap();

        let mut stream = JsonClient::new()
            .from_bytes(&json)
            .stream_array("items")
            .await;

        let batch1 = stream.next_batch::<Item>(3).await.unwrap().unwrap();
        assert_eq!(batch1.len(), 3);
        assert_eq!(batch1[0].id, 1);
        assert_eq!(batch1[2].id, 3);

        let batch2 = stream.next_batch::<Item>(3).await.unwrap().unwrap();
        assert_eq!(batch2.len(), 3);
        assert_eq!(batch2[0].id, 4);

        let batch3 = stream.next_batch::<Item>(3).await.unwrap().unwrap();
        assert_eq!(batch3.len(), 1);
        assert_eq!(batch3[0].id, 7);

        assert!(stream.next_batch::<Item>(3).await.is_none());
    }

    #[tokio::test]
    async fn test_early_drop() {
        let items: Vec<serde_json::Value> = (1..=1000)
            .map(|i| serde_json::json!({"id": i, "name": format!("item{}", i)}))
            .collect();
        let json = serde_json::to_vec(&serde_json::json!({"items": items})).unwrap();

        let mut stream = JsonClient::new()
            .from_bytes(&json)
            .stream_array("items")
            .await;

        let item: Item = stream.next().await.unwrap().unwrap();
        assert_eq!(item.id, 1);

        drop(stream);
        // If we get here without hanging, the background task exited cleanly
    }
}
