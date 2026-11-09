//! Webhook delivery mode (the second half of QQ's event delivery story).

use std::sync::Arc;

use ed25519_dalek::{Signer, SigningKey};
use serde_json::Value;

use crate::db::{i, s, Row};
use crate::world::World;

/// Official derivation: repeat the AppSecret until it is at least 32 bytes,
/// then truncate to the Ed25519 seed. (The docs' example vector confirms it.)
pub fn signing_key(secret: &str) -> SigningKey {
    let mut seed = [0u8; 32];
    let bytes = secret.as_bytes();
    if bytes.is_empty() {
        return SigningKey::from_bytes(&seed);
    }
    for (index, slot) in seed.iter_mut().enumerate() {
        *slot = bytes[index % bytes.len()];
    }
    SigningKey::from_bytes(&seed)
}

pub fn sign(secret: &str, payload: &[u8]) -> String {
    // ed25519-dalek's Display is uppercase hex; QQ expects lowercase.
    signing_key(secret)
        .sign(payload)
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn sign_handshake(secret: &str, event_ts: &str, plain_token: &str) -> String {
    sign(secret, format!("{event_ts}{plain_token}").as_bytes())
}

pub fn endpoints(world: &World, bot_id: &str) -> Vec<Row> {
    world
        .db
        .all(
            "SELECT * FROM webhook_endpoints WHERE bot_id=? AND enabled=1",
            &crate::p![bot_id],
        )
        .unwrap_or_default()
}

/// Fire-and-forget delivery: QQ's webhook mode is asynchronous by definition.
pub fn deliver(world: &Arc<World>, bot_id: &str, frame: &Value) {
    let rows = endpoints(world, bot_id);
    for endpoint in rows {
        let world = world.clone();
        let frame = frame.clone();
        tokio::spawn(async move {
            post(&world, endpoint, &frame).await;
        });
    }
}

async fn post(world: &Arc<World>, endpoint: Row, frame: &Value) {
    let url = s(&endpoint, "url");
    let secret = s(&endpoint, "secret");
    let body = frame.to_string();
    let timestamp = (chrono::Utc::now().timestamp()).to_string();
    let mut signed = timestamp.clone().into_bytes();
    signed.extend_from_slice(body.as_bytes());
    let signature = sign(&secret, &signed);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();
    for attempt in 0..2 {
        let response = client
            .post(&url)
            .header("content-type", "application/json")
            .header("x-signature-ed25519", &signature)
            .header("x-signature-timestamp", &timestamp)
            .header(
                "x-mqs-event",
                frame.get("t").and_then(|v| v.as_str()).unwrap_or(""),
            )
            .header("x-tps-trace-id", crate::error::new_trace_id())
            .body(body.clone())
            .send()
            .await;
        match response {
            Ok(resp) => {
                let status = resp.status().as_u16();
                world.log_frame(
                    "out",
                    "http",
                    &format!(
                        "webhook {} -> {url} [{status}]",
                        frame.get("t").and_then(|v| v.as_str()).unwrap_or("")
                    ),
                    &serde_json::json!({"url": url, "status": status, "body": body.chars().take(4000).collect::<String>()}),
                    Some(&s(&endpoint, "bot_id")),
                    None,
                );
                if status < 500 {
                    return;
                }
            }
            Err(err) => {
                world.log_frame(
                    "out",
                    "http",
                    &format!(
                        "webhook {} -> {url} error: {err}",
                        frame.get("t").and_then(|v| v.as_str()).unwrap_or("")
                    ),
                    &serde_json::json!({"url": url, "error": err.to_string()}),
                    Some(&s(&endpoint, "bot_id")),
                    None,
                );
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200 * (attempt + 1))).await;
    }
    let _ = i(&endpoint, "enabled");
}

#[cfg(test)]
mod vector_tests {
    use super::sign_handshake;

    #[test]
    fn official_handshake_vector() {
        // From the QQ Bot docs (webhook URL verification example).
        let signature = sign_handshake("DG5g3B4j9X2KOErG", "1725442341", "Arq0D5A61EgUu4OxUvOp");
        assert_eq!(
            signature,
            "87befc99c42c651b3aac0278e71ada338433ae26fcb24307bdc5ad38c1adc2d01bcfcadc0842edac85e85205028a1132afe09280305f13aa6909ffc2d652c706"
        );
    }
}
