//! 素材管理（material-management）：prepare → 预签名 PUT → complete → 引用/查询/删除。

mod common;

use std::collections::HashSet;

use common::Test;
use serde_json::{json, Value};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nMQS-material-image-payload";
const SHA1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn md5_hex(data: &[u8]) -> String {
    use md5::{Digest, Md5};
    Md5::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn prepare(
    test: &Test,
    authorization: &str,
    file_name: &str,
    content_type: &str,
    data: &[u8],
) -> Value {
    let response = test
        .http()
        .post(format!("{}/v2/media-images/prepare", test.sim))
        .header("authorization", authorization)
        .json(&json!({
            "file_name": file_name,
            "file_size": data.len(),
            "content_type": content_type,
            "md5": md5_hex(data),
            "sha1": SHA1,
        }))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "prepare failed: {body}");
    body
}

async fn put_binary(test: &Test, url: &str, content_type: &str, data: &[u8]) -> reqwest::Response {
    test.http()
        .put(url)
        .header("content-type", content_type)
        .body(data.to_vec())
        .send()
        .await
        .unwrap()
}

async fn complete(test: &Test, authorization: &str, upload_id: &str, md5: &str) -> Value {
    let response = test
        .http()
        .post(format!("{}/v2/media-images/complete", test.sim))
        .header("authorization", authorization)
        .json(&json!({"upload_id": upload_id, "md5": md5}))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "complete failed: {body}");
    body
}

async fn create_image(test: &Test, authorization: &str, name: &str, data: &[u8]) -> Value {
    let prepared = prepare(test, authorization, name, "image/png", data).await;
    let md5 = md5_hex(data);
    let uploaded = put_binary(
        test,
        prepared["presigned_url"].as_str().unwrap(),
        "image/png",
        data,
    )
    .await;
    assert_eq!(uploaded.status().as_u16(), 200);
    let etag = uploaded
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .unwrap()
        .trim_matches('"')
        .to_string();
    assert_eq!(etag, md5);
    complete(
        test,
        authorization,
        prepared["upload_id"].as_str().unwrap(),
        &md5,
    )
    .await
}

async fn list_images(test: &Test, authorization: &str, cursor: &str) -> Value {
    let suffix = if cursor.is_empty() {
        String::new()
    } else {
        format!("?cursor={cursor}")
    };
    let response = test
        .http()
        .get(format!("{}/v2/media-images{suffix}", test.sim))
        .header("authorization", authorization)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    response.json().await.unwrap()
}

async fn send_markdown(
    test: &Test,
    authorization: &str,
    openid: &str,
    content: &str,
) -> (u16, Value) {
    let response = test
        .http()
        .post(format!("{}/v2/groups/{openid}/messages", test.sim))
        .header("authorization", authorization)
        .json(&json!({"msg_type": 2, "markdown": {"content": content}}))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

async fn advance(test: &Test, duration: &str) {
    test.ctl_ok(
        "POST",
        "/_mqs/clock/advance",
        Some(json!({"duration": duration})),
    )
    .await;
}

async fn advance_ms(test: &Test, ms: i64) {
    test.ctl_ok("POST", "/_mqs/clock/advance", Some(json!({"ms": ms})))
        .await;
}

#[tokio::test]
async fn material_image_lifecycle_and_sliding_expiry() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let authorization = test.bot_headers("10000").await;

    let prepared = prepare(&test, &authorization, "kline.png", "image/png", PNG).await;
    assert!(prepared["upload_id"]
        .as_str()
        .unwrap()
        .starts_with("upload_"));
    assert_eq!(prepared["expires_in"], 3600);
    let presigned = prepared["presigned_url"].as_str().unwrap().to_string();
    assert!(presigned.contains("/v2/media-images/uploads/"));
    assert!(presigned.contains("sign="));

    let uploaded = put_binary(&test, &presigned, "image/png", PNG).await;
    assert_eq!(uploaded.status().as_u16(), 200);
    let etag = uploaded
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .unwrap()
        .trim_matches('"')
        .to_string();
    assert_eq!(etag, md5_hex(PNG));

    let completed = complete(
        &test,
        &authorization,
        prepared["upload_id"].as_str().unwrap(),
        &etag,
    )
    .await;
    let image_id = completed["image_id"].as_str().unwrap().to_string();
    assert!(image_id.starts_with("10000_"), "{image_id}");
    let url = completed["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("qqbot-media://image/v1/"), "{url}");
    assert_eq!(completed["ttl"], 7_776_000);
    let preview = completed["preview_url"].as_str().unwrap().to_string();
    let response = test.http().get(&preview).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.bytes().await.unwrap().as_ref(), PNG);

    let listed = list_images(&test, &authorization, "").await;
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["next_cursor"], "");
    assert_eq!(listed["images"].as_array().unwrap().len(), 1);
    let first_expiry = listed["images"][0]["expires_at"].as_i64().unwrap();
    assert_eq!(listed["images"][0]["image_id"], image_id);
    assert_eq!(listed["images"][0]["file_name"], "kline.png");

    // 第一次引用成功：有效期顺延到「现在 + 90 天」。
    advance(&test, "1h").await;
    let authorization = test.bot_headers("10000").await;
    let markdown = format!("![分时K线 #800px #400px]({url})");
    let (status, body) = send_markdown(&test, &authorization, &openid, &markdown).await;
    assert_eq!(status, 200, "send failed: {body}");
    let listed = list_images(&test, &authorization, "").await;
    let second_expiry = listed["images"][0]["expires_at"].as_i64().unwrap();
    assert!(
        second_expiry >= first_expiry + 3600,
        "{first_expiry} -> {second_expiry}"
    );

    // 图片在原始 90 天窗口之后仍可用，因为上一次引用顺延了它。
    advance_ms(&test, 90 * 24 * 60 * 60 * 1000 - 1000).await;
    let authorization = test.bot_headers("10000").await;
    let (status, body) = send_markdown(&test, &authorization, &openid, &markdown).await;
    assert_eq!(
        status, 200,
        "sliding expiry should keep the image valid: {body}"
    );
    advance_ms(&test, 90 * 24 * 60 * 60 * 1000 + 1000).await;
    let authorization = test.bot_headers("10000").await;
    let (status, body) = send_markdown(&test, &authorization, &openid, &markdown).await;
    assert_eq!(status, 400, "expired reference must be rejected: {body}");
    assert_eq!(body["code"], 40035009);
    let listed = list_images(&test, &authorization, "").await;
    assert_eq!(listed["total"], 0);
    assert_eq!(listed["images"].as_array().unwrap().len(), 0);

    // 已过期的图片删除也按「不存在」处理。
    let response = test
        .http()
        .delete(format!("{}/v2/media-images/{image_id}/delete", test.sim))
        .header("authorization", &authorization)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035008);
}

#[tokio::test]
async fn material_image_delete_invalidates_reference() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let authorization = test.bot_headers("10000").await;
    let image = create_image(&test, &authorization, "a.png", PNG).await;
    let url = image["url"].as_str().unwrap().to_string();
    let image_id = image["image_id"].as_str().unwrap().to_string();

    let markdown = format!("![图]({url})");
    let (status, _) = send_markdown(&test, &authorization, &openid, &markdown).await;
    assert_eq!(status, 200);

    let response = test
        .http()
        .delete(format!("{}/v2/media-images/{image_id}/delete", test.sim))
        .header("authorization", &authorization)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let deleted: Value = response.json().await.unwrap();
    assert_eq!(deleted, json!({}));

    let listed = list_images(&test, &authorization, "").await;
    assert_eq!(listed["total"], 0);

    let (status, body) = send_markdown(&test, &authorization, &openid, &markdown).await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], 40035009);

    // 重复删除 → 40035008。
    let response = test
        .http()
        .delete(format!("{}/v2/media-images/{image_id}/delete", test.sim))
        .header("authorization", &authorization)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
async fn material_image_prepare_validation() {
    let test = Test::start().await;
    let authorization = test.bot_headers("10000").await;

    let cases = [
        (
            json!({"file_name": "a.bmp", "file_size": 10, "content_type": "image/png", "md5": md5_hex(PNG), "sha1": SHA1}),
            40035001,
        ),
        (
            json!({"file_name": "a.png", "file_size": 10, "content_type": "image/jpeg", "md5": md5_hex(PNG), "sha1": SHA1}),
            40035001,
        ),
        (
            json!({"file_name": "a.png", "file_size": 10, "content_type": "image/bmp", "md5": md5_hex(PNG), "sha1": SHA1}),
            40035001,
        ),
        (
            json!({"file_name": "a.png", "file_size": 10 * 1024 * 1024 + 1, "content_type": "image/png", "md5": md5_hex(PNG), "sha1": SHA1}),
            40035002,
        ),
        (
            json!({"file_name": "a.png", "file_size": 10, "content_type": "image/png", "md5": "abc", "sha1": SHA1}),
            10001,
        ),
        (
            json!({"file_name": "a.png", "file_size": 10, "content_type": "image/png", "md5": md5_hex(PNG), "sha1": "abc"}),
            10001,
        ),
    ];
    for (body, code) in cases {
        let response = test
            .http()
            .post(format!("{}/v2/media-images/prepare", test.sim))
            .header("authorization", &authorization)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let payload: Value = response.json().await.unwrap();
        assert_eq!(
            payload["code"], code,
            "unexpected code for {body}: {payload}"
        );
        assert!(status >= 400, "{status}");
    }

    // jpg 与 image/jpeg 是合法组合。
    prepare(&test, &authorization, "photo.jpg", "image/jpeg", PNG).await;
}

#[tokio::test]
async fn material_image_upload_task_expiry_and_md5() {
    let test = Test::start().await;
    let authorization = test.bot_headers("10000").await;
    let md5 = md5_hex(PNG);

    let prepared = prepare(&test, &authorization, "expire.png", "image/png", PNG).await;
    let upload_id = prepared["upload_id"].as_str().unwrap().to_string();
    let presigned = prepared["presigned_url"].as_str().unwrap().to_string();
    advance_ms(&test, 4 * 60 * 60 * 1000 + 1000).await;
    let authorization = test.bot_headers("10000").await;
    let response = put_binary(&test, &presigned, "image/png", PNG).await;
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035003);
    let response = test
        .http()
        .post(format!("{}/v2/media-images/complete", test.sim))
        .header("authorization", &authorization)
        .json(&json!({"upload_id": upload_id, "md5": md5}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035003);

    // 预签名 URL 1 小时后失效，但 4 小时内的 upload_id 仍可 complete。
    let prepared = prepare(&test, &authorization, "presign.png", "image/png", PNG).await;
    let presigned = prepared["presigned_url"].as_str().unwrap().to_string();
    advance(&test, "2h").await;
    let authorization = test.bot_headers("10000").await;
    let response = put_binary(&test, &presigned, "image/png", PNG).await;
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035003);

    // complete 之前必须 PUT；md5 不匹配按上传任务无效处理。
    let prepared = prepare(&test, &authorization, "md5.png", "image/png", PNG).await;
    let upload_id = prepared["upload_id"].as_str().unwrap().to_string();
    let response = test
        .http()
        .post(format!("{}/v2/media-images/complete", test.sim))
        .header("authorization", &authorization)
        .json(&json!({"upload_id": upload_id, "md5": md5}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035003);

    // Content-Type 与 prepare 不一致：模拟 COS 返回 403。
    let response = put_binary(
        &test,
        prepared["presigned_url"].as_str().unwrap(),
        "image/jpeg",
        PNG,
    )
    .await;
    assert_eq!(response.status().as_u16(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035001);

    let response = put_binary(
        &test,
        prepared["presigned_url"].as_str().unwrap(),
        "image/png",
        PNG,
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let wrong_md5 = "0".repeat(32);
    let response = test
        .http()
        .post(format!("{}/v2/media-images/complete", test.sim))
        .header("authorization", &authorization)
        .json(&json!({"upload_id": upload_id, "md5": wrong_md5}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035003);
    let completed = complete(&test, &authorization, &upload_id, &md5).await;
    assert!(completed["url"]
        .as_str()
        .unwrap()
        .starts_with("qqbot-media://"));
}

#[tokio::test]
async fn material_image_quota_is_enforced_and_released() {
    let test = Test::start().await;
    test.settings(json!({"media_image_limit": 1})).await;
    let authorization = test.bot_headers("10000").await;
    let image = create_image(&test, &authorization, "one.png", PNG).await;
    let image_id = image["image_id"].as_str().unwrap().to_string();

    let response = test
        .http()
        .post(format!("{}/v2/media-images/prepare", test.sim))
        .header("authorization", &authorization)
        .json(&json!({
            "file_name": "two.png",
            "file_size": PNG.len(),
            "content_type": "image/png",
            "md5": md5_hex(PNG),
            "sha1": SHA1,
        }))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let trace = response
        .headers()
        .get("x-tps-trace-id")
        .and_then(|value| value.to_str().ok())
        .unwrap()
        .to_string();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 400);
    assert_eq!(body["code"], 40035007);
    assert_eq!(body["trace_id"], trace);

    test.http()
        .delete(format!("{}/v2/media-images/{image_id}/delete", test.sim))
        .header("authorization", &authorization)
        .send()
        .await
        .unwrap();
    let second = create_image(&test, &authorization, "two.png", PNG).await;
    assert!(second["image_id"].is_string());
}

#[tokio::test]
async fn material_image_list_paginates_and_enforces_bot_ownership() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    // 固定虚拟时钟，让 21 张图片的 created_at 完全相同，验证游标用 id 兜底。
    test.ctl_ok(
        "POST",
        "/_mqs/clock/set",
        Some(json!({"ms": 1_800_000_000_000_i64})),
    )
    .await;
    let authorization = test.bot_headers("10000").await;
    let group = test.group_id().await;

    let mut images = Vec::new();
    for index in 0..21 {
        images.push(create_image(&test, &authorization, &format!("p{index}.png"), PNG).await);
    }
    let first = list_images(&test, &authorization, "").await;
    assert_eq!(first["total"], 21);
    assert_eq!(first["images"].as_array().unwrap().len(), 20);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    assert!(!cursor.is_empty());
    let second = list_images(&test, &authorization, &cursor).await;
    assert_eq!(second["total"], 21);
    assert_eq!(second["images"].as_array().unwrap().len(), 1);
    assert_eq!(second["next_cursor"], "");
    let mut ids = HashSet::new();
    for image in first["images"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["images"].as_array().unwrap().iter())
    {
        assert!(ids.insert(image["image_id"].as_str().unwrap().to_string()));
    }
    assert_eq!(ids.len(), 21);

    // 先用 10000 成功引用一张，控制台消息应带回预览映射。
    let own_openid = test.group_openid("10000", &group).await;
    let markdown = format!("![自己的图]({})", images[0]["url"].as_str().unwrap());
    let (status, body) = send_markdown(&test, &authorization, &own_openid, &markdown).await;
    assert_eq!(status, 200, "send failed: {body}");

    // 第二个 Bot 不能引用第一个 Bot 的图片。
    test.ctl_ok(
        "POST",
        "/_mqs/bots",
        Some(json!({
            "app_id": "20000",
            "secret": "mqs-secret-20000",
            "name": "另一个 Bot",
            "scopes": ["group_message", "c2c_message", "media"]
        })),
    )
    .await;
    test.ctl_ok("POST", &format!("/_mqs/groups/{group}/bots/20000"), None)
        .await;
    let second_bot = test.bot_headers("20000").await;
    let openid = test.group_openid("20000", &group).await;
    let markdown = format!("![偷图]({})", images[0]["url"].as_str().unwrap());
    let (status, body) = send_markdown(&test, &second_bot, &openid, &markdown).await;
    assert_eq!(status, 403);
    assert_eq!(body["code"], 40035010);

    // 未知 token 同样按无效引用处理。
    let (status, body) = send_markdown(
        &test,
        &authorization,
        &test.group_openid("10000", &group).await,
        "![x](qqbot-media://image/v1/UNKNOWN)",
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(body["code"], 40035010);

    // 控制台消息渲染附带 token → 预览 URL 映射。
    let conversations = test.conversations().await;
    let conversation_id = conversations
        .as_array()
        .unwrap()
        .iter()
        .find(|conv| conv["type"] == "group")
        .and_then(|conv| conv["id"].as_str())
        .unwrap()
        .to_string();
    let messages = test
        .ctl_ok(
            "GET",
            &format!("/_mqs/conversations/{conversation_id}/messages"),
            None,
        )
        .await;
    let message = messages["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["direction"] == "out" && message["media_images"].is_array())
        .expect("outbound message with media_images");
    assert!(message["media_images"][0]["preview_url"]
        .as_str()
        .unwrap()
        .starts_with("http://"));
}

#[tokio::test]
async fn material_image_upload_rejects_bad_signature_expiry_and_size() {
    let test = Test::start().await;
    let authorization = test.bot_headers("10000").await;
    let prepared = prepare(&test, &authorization, "sig.png", "image/png", PNG).await;
    let good = prepared["presigned_url"].as_str().unwrap().to_string();
    let upload_id = prepared["upload_id"].as_str().unwrap();

    let bad_sign = good.replace("sign=", "sign=0");
    let response = put_binary(&test, &bad_sign, "image/png", PNG).await;
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035003);

    let bad_expires = good
        .split("&expires=")
        .next()
        .map(|prefix| format!("{prefix}&expires=1"))
        .unwrap();
    let response = put_binary(&test, &bad_expires, "image/png", PNG).await;
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035003);

    let unknown = format!(
        "{}/v2/media-images/uploads/UNKNOWN?sign=x&expires=1",
        test.sim
    );
    let response = put_binary(&test, &unknown, "image/png", PNG).await;
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035003);

    let oversize = vec![0u8; 10 * 1024 * 1024 + 1];
    let response = put_binary(&test, &good, "image/png", &oversize).await;
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 40035002);

    // 超限的 PUT 不改变上传任务状态，正常大小的重试仍可完成。
    let response = put_binary(&test, &good, "image/png", PNG).await;
    assert_eq!(response.status().as_u16(), 200);
    complete(&test, &authorization, upload_id, &md5_hex(PNG)).await;
}

#[tokio::test]
async fn material_image_list_is_per_bot_and_rejects_bad_cursor() {
    let test = Test::start().await;
    let authorization = test.bot_headers("10000").await;
    create_image(&test, &authorization, "mine.png", PNG).await;

    test.ctl_ok(
        "POST",
        "/_mqs/bots",
        Some(json!({
            "app_id": "20000",
            "secret": "mqs-secret-20000",
            "name": "另一个 Bot",
            "scopes": ["group_message", "c2c_message", "media"]
        })),
    )
    .await;
    let other = test.bot_headers("20000").await;
    let listed = list_images(&test, &other, "").await;
    assert_eq!(listed["total"], 0);
    assert_eq!(listed["images"].as_array().unwrap().len(), 0);

    let response = test
        .http()
        .get(format!("{}/v2/media-images?cursor=not-a-cursor", test.sim))
        .header("authorization", &authorization)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], 10001);
}

#[tokio::test]
async fn material_image_prepare_is_idempotent_by_sha1() {
    let test = Test::start().await;
    let authorization = test.bot_headers("10000").await;
    let first = prepare(&test, &authorization, "same.png", "image/png", PNG).await;
    let second = prepare(&test, &authorization, "same.png", "image/png", PNG).await;
    assert_eq!(first["upload_id"], second["upload_id"]);
    assert!(second["presigned_url"].as_str().unwrap().contains("sign="));

    let response = put_binary(
        &test,
        second["presigned_url"].as_str().unwrap(),
        "image/png",
        PNG,
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    complete(
        &test,
        &authorization,
        second["upload_id"].as_str().unwrap(),
        &md5_hex(PNG),
    )
    .await;
}

#[tokio::test]
async fn material_image_send_validates_content_and_does_not_slide_on_failure() {
    let test = Test::start().await;
    test.settings(json!({"require_online": false})).await;
    let group = test.group_id().await;
    let openid = test.group_openid("10000", &group).await;
    let authorization = test.bot_headers("10000").await;

    // complete 允许内容不合法；发送时才按文件头检查格式（40035001）。
    let bad = b"this is not a png".to_vec();
    let prepared = prepare(&test, &authorization, "fake.png", "image/png", &bad).await;
    let response = put_binary(
        &test,
        prepared["presigned_url"].as_str().unwrap(),
        "image/png",
        &bad,
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let image = complete(
        &test,
        &authorization,
        prepared["upload_id"].as_str().unwrap(),
        &md5_hex(&bad),
    )
    .await;
    let (status, body) = send_markdown(
        &test,
        &authorization,
        &openid,
        &format!("![bad]({})", image["url"].as_str().unwrap()),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], 40035001);

    // 引用失败（另一个 token 未知）不改变有效图片的 expires_at。
    let good = create_image(&test, &authorization, "good.png", PNG).await;
    let url = good["url"].as_str().unwrap().to_string();
    let expires_at = |listed: &Value| {
        listed["images"]
            .as_array()
            .unwrap()
            .iter()
            .find(|image| image["url"] == url)
            .and_then(|image| image["expires_at"].as_i64())
            .unwrap()
    };
    let before = expires_at(&list_images(&test, &authorization, "").await);
    let (status, body) = send_markdown(
        &test,
        &authorization,
        &openid,
        &format!("![good]({url}) ![bad](qqbot-media://image/v1/UNKNOWN)"),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(body["code"], 40035010);
    let after = expires_at(&list_images(&test, &authorization, "").await);
    assert_eq!(before, after);

    // 单聊 Markdown 也能引用素材图片。
    let user = test.user_id().await;
    let user_openid = test.user_openid("10000", &user).await;
    let response = test
        .http()
        .post(format!("{}/v2/users/{user_openid}/messages", test.sim))
        .header("authorization", &authorization)
        .json(&json!({"msg_type": 2, "markdown": {"content": format!("![good]({url})")}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
}

#[tokio::test]
async fn expired_material_image_releases_quota() {
    let test = Test::start().await;
    test.settings(json!({"media_image_limit": 1})).await;
    let authorization = test.bot_headers("10000").await;
    create_image(&test, &authorization, "old.png", PNG).await;

    advance_ms(&test, 90 * 24 * 60 * 60 * 1000 + 1000).await;
    let authorization = test.bot_headers("10000").await;
    let second = create_image(&test, &authorization, "new.png", PNG).await;
    assert!(second["image_id"].is_string());
    let listed = list_images(&test, &authorization, "").await;
    assert_eq!(listed["total"], 1);
}

// ---------------------------------------------------------------- control panel (`/_mqs/media-images`)

async fn panel_list(test: &Test, query: &str) -> Value {
    test.ctl_ok("GET", &format!("/_mqs/media-images{query}"), None)
        .await
}

#[tokio::test]
async fn control_panel_lists_edits_and_deletes_media_images() {
    let test = Test::start().await;
    let authorization = test.bot_headers("10000").await;
    let image = create_image(&test, &authorization, "panel.png", PNG).await;
    let image_id = image["image_id"].as_str().unwrap().to_string();

    let listed = panel_list(&test, "").await;
    assert_eq!(
        listed["counts"],
        json!({"total": 1, "valid": 1, "expired": 0, "deleted": 0})
    );
    assert_eq!(listed["limit"], 100);
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item["image_id"], image_id);
    assert_eq!(item["bot_id"], "10000");
    assert_eq!(item["file_name"], "panel.png");
    assert_eq!(item["content_type"], "image/png");
    assert_eq!(item["size"].as_i64().unwrap(), PNG.len() as i64);
    assert_eq!(item["md5"], md5_hex(PNG));
    assert_eq!(item["sha1"], SHA1);
    assert_eq!(item["expired"], false);
    assert_eq!(item["deleted"], false);
    assert_eq!(item["deleted_at"], Value::Null);
    assert!(item["preview_url"].as_str().unwrap().starts_with(&test.sim));
    assert_eq!(
        item["markdown"].as_str().unwrap(),
        format!("![panel.png]({})", image["url"].as_str().unwrap())
    );

    // 改名 + 顺延到期：官方的 Bot 视角列表立刻读到新文件名与新有效期。
    let expires_at = item["expires_at"].as_i64().unwrap() + 3600;
    let patched = test
        .ctl_ok(
            "PATCH",
            &format!("/_mqs/media-images/{image_id}"),
            Some(json!({"file_name": "renamed.png", "expires_at": expires_at})),
        )
        .await;
    assert_eq!(patched["file_name"], "renamed.png");
    assert_eq!(patched["expires_at"], expires_at);
    assert_eq!(patched["expired"], false);
    let listed = list_images(&test, &authorization, "").await;
    assert_eq!(listed["images"][0]["file_name"], "renamed.png");
    assert_eq!(listed["images"][0]["expires_at"], expires_at);

    // 改造：扩展名与 content_type 不匹配 → 40035001；空文件名 → 10001。
    let (status, body) = test
        .ctl(
            "PATCH",
            &format!("/_mqs/media-images/{image_id}"),
            Some(json!({"file_name": "renamed.jpg"})),
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], 40035001);
    let (status, body) = test
        .ctl(
            "PATCH",
            &format!("/_mqs/media-images/{image_id}"),
            Some(json!({"file_name": "  "})),
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], 10001);

    // 面板删除：默认列表看不到，include=all 能看到 deleted 标记。
    let deleted = test
        .ctl_ok("DELETE", &format!("/_mqs/media-images/{image_id}"), None)
        .await;
    assert_eq!(deleted["deleted"], true);
    assert!(deleted["deleted_at"].as_i64().is_some());
    let listed = panel_list(&test, "").await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 0);
    assert_eq!(listed["counts"]["deleted"], 1);
    let listed = panel_list(&test, "?include=all").await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["deleted"], true);

    // 重复删除、修改已删除的素材、未知 id 都是 404 + 40035008。
    for (method, body) in [
        ("DELETE", None),
        ("PATCH", Some(json!({"file_name": "again.png"}))),
    ] {
        let (status, payload) = test
            .ctl(method, &format!("/_mqs/media-images/{image_id}"), body)
            .await;
        assert_eq!(status, 404, "{method}: {payload}");
        assert_eq!(payload["code"], 40035008);
    }
    let (status, body) = test
        .ctl("DELETE", "/_mqs/media-images/unknown_id", None)
        .await;
    assert_eq!(status, 404);
    assert_eq!(body["code"], 40035008);
}

#[tokio::test]
async fn control_panel_filters_bots_and_shows_expired_images() {
    let test = Test::start().await;
    let authorization = test.bot_headers("10000").await;
    let image = create_image(&test, &authorization, "expire.png", PNG).await;
    let image_id = image["image_id"].as_str().unwrap().to_string();

    // 第二个 Bot 的素材不能被 10000 的过滤条件带出来。
    test.ctl_ok(
        "POST",
        "/_mqs/bots",
        Some(json!({"app_id": "20000", "name": "第二个 Bot"})),
    )
    .await;
    let authorization2 = test.bot_headers("20000").await;
    let other = create_image(&test, &authorization2, "other.png", PNG).await;
    let other_id = other["image_id"].as_str().unwrap().to_string();

    let listed = panel_list(&test, "").await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 2);
    assert_eq!(listed["counts"]["valid"], 2);

    let listed = panel_list(&test, "?bot_id=20000").await;
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["image_id"], other_id);
    assert_eq!(
        listed["counts"],
        json!({"total": 1, "valid": 1, "expired": 0, "deleted": 0})
    );

    let listed = panel_list(&test, "?bot_id=99999").await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 0);
    assert_eq!(listed["counts"]["total"], 0);

    // 把素材改到过去 → 默认列表隐藏，include=expired 能看到 expired 标记。
    let current = panel_list(&test, "?bot_id=10000").await;
    let created = current["items"][0]["created_at"].as_i64().unwrap();
    test.ctl_ok(
        "PATCH",
        &format!("/_mqs/media-images/{image_id}"),
        Some(json!({"expires_at": created})),
    )
    .await;
    let listed = panel_list(&test, "?bot_id=10000").await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 0);
    assert_eq!(
        listed["counts"],
        json!({"total": 1, "valid": 0, "expired": 1, "deleted": 0})
    );
    let listed = panel_list(&test, "?bot_id=10000&include=expired").await;
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["expired"], true);

    // 官方接口拒绝删除已过期图片（40035008），面板允许——调试完清理现场。
    test.ctl_ok("DELETE", &format!("/_mqs/media-images/{image_id}"), None)
        .await;
    let listed = panel_list(&test, "?bot_id=10000&include=all").await;
    assert_eq!(listed["items"][0]["deleted"], true);

    // 非法 include → 10001。
    let (status, body) = test
        .ctl("GET", "/_mqs/media-images?include=bogus", None)
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], 10001);
}
