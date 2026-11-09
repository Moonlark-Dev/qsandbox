//! Media: multipart, JSON base64, chunked upload, download, path safety.

mod common;

use common::Test;
use reqwest::multipart;
use serde_json::{json, Value};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nMQS-fake-png-payload";

#[tokio::test]
async fn upload_and_send_media() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let token = test.token("10000").await;
    let headers = format!("QQBot {token}");

    let form = multipart::Form::new()
        .part(
            "file",
            multipart::Part::bytes(PNG.to_vec())
                .file_name("a.png")
                .mime_str("image/png")
                .unwrap(),
        )
        .text("file_type", "1");
    let uploaded: Value = test
        .http()
        .post(format!("{}/v2/groups/{openid}/files", test.sim))
        .header("authorization", &headers)
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(uploaded["file_info"]
        .as_str()
        .unwrap()
        .starts_with("mqs-file-"));
    assert_eq!(uploaded["ttl"], 0);

    // sending needs a passive window when require_online is off
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", &headers)
        .json(&json!({"msg_type": 7, "content": "", "media": {"file_info": uploaded["file_info"]}}))
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    let hash = uploaded["file_uuid"].as_str().unwrap();

    let download = test
        .http()
        .get(format!("{}/media/{hash}", test.sim))
        .send()
        .await
        .unwrap();
    assert_eq!(download.status().as_u16(), 200);
    assert_eq!(download.bytes().await.unwrap().as_ref(), PNG);
    assert!(body["id"].is_string() || body["code"] == 40040);
}

#[tokio::test]
async fn upload_by_media_url() {
    let test = Test::start().await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let token = test.token("10000").await;
    let headers = format!("QQBot {token}");
    let media = test.world().store_media(PNG, "image/png").unwrap();
    let hash = mqs::db::s(&media, "hash");
    let body: Value = test
        .http()
        .post(format!("{}/v2/groups/{openid}/files", test.sim))
        .header("authorization", &headers)
        .json(&json!({"file_type": 1, "url": format!("/media/{hash}"), "srv_send_msg": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["file_uuid"], hash);

    let missing = test
        .http()
        .post(format!("{}/v2/groups/{openid}/files", test.sim))
        .header("authorization", &headers)
        .json(&json!({"file_type": 1, "url": format!("/media/{}", "0".repeat(64))}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 400);
}

#[tokio::test]
async fn upload_rejects_bad_type_and_oversize() {
    let test = Test::start().await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let token = test.token("10000").await;
    let headers = format!("QQBot {token}");

    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/files", test.sim))
        .header("authorization", &headers)
        .json(&json!({"file_type": 4, "url": "/media/x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);

    let big = vec![0u8; 5 * 1024 * 1024 + 1];
    let form = multipart::Form::new()
        .part(
            "file",
            multipart::Part::bytes(big)
                .file_name("a.amr")
                .mime_str("audio/amr")
                .unwrap(),
        )
        .text("file_type", "3");
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/files", test.sim))
        .header("authorization", &headers)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
}

#[tokio::test]
async fn upload_json_file_data() {
    let test = Test::start().await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let token = test.token("10000").await;
    let encoded = base64_encode(PNG);
    let body: Value = test
        .http()
        .post(format!("{}/v2/groups/{openid}/files", test.sim))
        .header("authorization", format!("QQBot {token}"))
        .json(&json!({"file_type": 1, "file_data": encoded}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["file_uuid"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn chunked_upload_flow() {
    let test = Test::start().await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let token = test.token("10000").await;
    let headers = format!("QQBot {token}");
    // two 4 MiB blocks + tail
    let data = vec![7u8; 4 * 1024 * 1024 + 1000];
    let prepare: Value = test
        .http()
        .post(format!("{}/v2/groups/{openid}/upload_prepare", test.sim))
        .header("authorization", &headers)
        .json(&json!({"file_type": 1, "file_name": "big.png", "file_size": data.len(), "srv_send_msg": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let upload_id = prepare["upload_id"].as_str().unwrap().to_string();
    assert_eq!(prepare["block_size"], 4 * 1024 * 1024);
    let block = 4 * 1024 * 1024;
    let parts: Vec<&[u8]> = data.chunks(block).collect();
    for (index, part) in parts.iter().enumerate() {
        let response = test
            .http()
            .put(format!("{}/v2/uploads/{upload_id}/parts/{index}", test.sim))
            .body(part.to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 204);
        let md5 = md5_hex(part);
        let response = test
            .http()
            .post(format!(
                "{}/v2/groups/{openid}/upload_part_finish",
                test.sim
            ))
            .header("authorization", &headers)
            .json(&json!({
                "upload_id": upload_id,
                "part_index": index,
                "block_size": part.len(),
                "md5": md5,
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 204);
    }
    let done: Value = test
        .http()
        .post(format!("{}/v2/groups/{openid}/files", test.sim))
        .header("authorization", &headers)
        .json(&json!({"upload_id": upload_id, "file_type": 1, "srv_send_msg": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(done["file_info"].as_str().unwrap().starts_with("mqs-file-"));
}

#[tokio::test]
async fn media_path_is_safe() {
    let test = Test::start().await;
    for bad in ["..%2f..%2fetc%2fpasswd", "not-a-hash"] {
        let response = test
            .http()
            .get(format!("{}/media/{bad}", test.sim))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 400, "{bad}");
    }
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn md5_hex(data: &[u8]) -> String {
    use md5::{Digest, Md5};
    Md5::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
