//! Declarative scenario engine: stimulus in, assertions on events/messages out.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use serde_yaml::Value as Yaml;

use crate::clock::parse_duration;
use crate::db::{i, s};
use crate::error::{invalid_param, MqsResult};
use crate::world::World;

/// Built-in templates, embedded from `scenarios/` so a packaged binary keeps them.
pub static BUILTINS: &[(&str, &str)] = &[
    ("smoke", include_str!("../../../scenarios/smoke.yaml")),
    ("c2c", include_str!("../../../scenarios/c2c.yaml")),
    (
        "c2c_passive",
        include_str!("../../../scenarios/c2c_passive.yaml"),
    ),
    (
        "rate_limit",
        include_str!("../../../scenarios/rate_limit.yaml"),
    ),
    ("media", include_str!("../../../scenarios/media.yaml")),
    ("quote", include_str!("../../../scenarios/quote.yaml")),
    (
        "permission",
        include_str!("../../../scenarios/permission.yaml"),
    ),
    (
        "group_multi",
        include_str!("../../../scenarios/group_multi.yaml"),
    ),
    (
        "passive_window",
        include_str!("../../../scenarios/passive_window.yaml"),
    ),
    (
        "duplicate_seq",
        include_str!("../../../scenarios/duplicate_seq.yaml"),
    ),
];

pub fn builtin(name: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, text)| *text)
}

pub fn catalog(world: &World) -> Vec<Value> {
    let mut items = Vec::new();
    for (name, text) in BUILTINS {
        let doc: Yaml = serde_yaml::from_str(text).unwrap_or(Yaml::Null);
        items.push(json!({
            "name": name,
            "source": "builtin",
            "description": ystr(doc.get("description")),
            "steps": doc.get("steps").and_then(|v| v.as_sequence()).map(|v| v.len()).unwrap_or(0),
        }));
    }
    let mut roots = vec![repository_scenarios_dir()];
    roots.push(world.paths.directory().join("scenarios"));
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut paths: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .map(|ext| ext == "yaml" || ext == "yml")
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        for path in paths {
            let doc: Yaml = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_yaml::from_str(&text).ok())
                .unwrap_or(Yaml::Null);
            items.push(json!({
                "name": path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                "source": "file",
                "path": path.to_string_lossy(),
                "description": ystr(doc.get("description")),
                "steps": doc.get("steps").and_then(|v| v.as_sequence()).map(|v| v.len()).unwrap_or(0),
            }));
        }
    }
    items
}

fn repository_scenarios_dir() -> std::path::PathBuf {
    // Source checkout: <repo>/scenarios. Installed: next to the executable.
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("scenarios"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("scenarios"));
        }
    }
    candidates.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scenarios"));
    candidates
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("scenarios"))
}

fn ystr(value: Option<&Yaml>) -> String {
    match value {
        Some(Yaml::String(text)) => text.clone(),
        Some(Yaml::Number(number)) => number.to_string(),
        Some(Yaml::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn yget<'a>(value: &'a Yaml, key: &str) -> Option<&'a Yaml> {
    value.get(key)
}

struct Runner<'a> {
    world: &'a Arc<World>,
    sim_base: String,
    vars: HashMap<String, String>,
    bots: HashMap<String, String>,
    users: HashMap<String, String>,
    groups: HashMap<String, String>,
    baseline_event_seq: i64,
    baseline_message_row: i64,
    consumed_events: HashSet<i64>,
    consumed_messages: HashSet<i64>,
}

struct Step {
    name: String,
    ok: bool,
    detail: String,
    duration_ms: i64,
}

struct Outcome {
    name: String,
    ok: bool,
    steps: Vec<Step>,
    duration_ms: i64,
    error: String,
}

impl Outcome {
    fn junit(&self) -> String {
        let failures = self.steps.iter().filter(|step| !step.ok).count();
        let mut lines = vec![
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_string(),
            format!(
                "<testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"0\" time=\"{:.3}\">",
                escape(&self.name),
                self.steps.len(),
                failures,
                self.duration_ms as f64 / 1000.0
            ),
        ];
        for step in &self.steps {
            lines.push(format!(
                "  <testcase classname=\"mqs.scenario\" name=\"{}\" time=\"{:.3}\">",
                escape(&step.name),
                step.duration_ms as f64 / 1000.0
            ));
            if !step.ok {
                lines.push(format!(
                    "    <failure message=\"{}\" />",
                    escape(&step.detail.chars().take(500).collect::<String>())
                ));
            }
            lines.push("  </testcase>".to_string());
        }
        lines.push("</testsuite>".to_string());
        lines.join("\n")
    }

    fn tap(&self) -> String {
        let mut lines = vec![
            "TAP version 13".to_string(),
            format!("1..{}", self.steps.len()),
        ];
        for (index, step) in self.steps.iter().enumerate() {
            if step.ok {
                lines.push(format!("ok {} - {}", index + 1, step.name));
            } else {
                lines.push(format!("not ok {} - {}", index + 1, step.name));
                lines.push("  ---".to_string());
                lines.push(format!(
                    "  message: {}",
                    step.detail.chars().take(500).collect::<String>()
                ));
                lines.push("  ...".to_string());
            }
        }
        lines.join("\n")
    }

    fn as_value(&self) -> Value {
        json!({
            "name": self.name,
            "ok": self.ok,
            "duration_ms": self.duration_ms,
            "error": self.error,
            "steps": self.steps.iter().map(|step| json!({
                "name": step.name,
                "ok": step.ok,
                "detail": step.detail,
                "duration_ms": step.duration_ms,
            })).collect::<Vec<_>>(),
            "junit": self.junit(),
            "tap": self.tap(),
        })
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl<'a> Runner<'a> {
    async fn run(&mut self, script: &str, name: &str) -> Outcome {
        let started = Instant::now();
        let doc: Yaml = match serde_yaml::from_str(script) {
            Ok(doc) => doc,
            Err(err) => {
                return Outcome {
                    name: name.to_string(),
                    ok: false,
                    steps: vec![Step {
                        name: "setup".into(),
                        ok: false,
                        detail: format!("invalid YAML: {err}"),
                        duration_ms: 0,
                    }],
                    duration_ms: 0,
                    error: format!("setup failed: invalid YAML: {err}"),
                }
            }
        };
        let scenario_name = {
            let value = ystr(yget(&doc, "name"));
            if value.is_empty() {
                name.to_string()
            } else {
                value
            }
        };
        let mut outcome = Outcome {
            name: scenario_name,
            ok: true,
            steps: Vec::new(),
            duration_ms: 0,
            error: String::new(),
        };
        self.baseline_event_seq = self
            .world
            .db
            .one("SELECT COALESCE(MAX(seq),0) AS m FROM events", &[])
            .ok()
            .flatten()
            .map(|row| i(&row, "m"))
            .unwrap_or(0);
        self.baseline_message_row = self
            .world
            .db
            .one("SELECT COALESCE(MAX(id),0) AS m FROM messages", &[])
            .ok()
            .flatten()
            .map(|row| i(&row, "m"))
            .unwrap_or(0);
        if let Some(setup) = yget(&doc, "setup") {
            if let Err(failure) = self.setup(setup).await {
                outcome.ok = false;
                outcome.error = format!("setup failed: {}", failure.detail);
                outcome.steps.push(failure);
                outcome.duration_ms = started.elapsed().as_millis() as i64;
                return outcome;
            }
        }
        if let Some(steps) = yget(&doc, "steps").and_then(|v| v.as_sequence()) {
            for (index, raw) in steps.iter().enumerate() {
                let label = step_label(index, raw);
                let step_started = Instant::now();
                match self.step(raw).await {
                    Ok(detail) => outcome.steps.push(Step {
                        name: label,
                        ok: true,
                        detail,
                        duration_ms: step_started.elapsed().as_millis() as i64,
                    }),
                    Err(detail) => {
                        outcome.steps.push(Step {
                            name: label,
                            ok: false,
                            detail: detail.clone(),
                            duration_ms: step_started.elapsed().as_millis() as i64,
                        });
                        outcome.ok = false;
                        outcome.error = detail;
                        break;
                    }
                }
            }
        }
        outcome.duration_ms = started.elapsed().as_millis() as i64;
        outcome
    }

    async fn setup(&mut self, setup: &Yaml) -> Result<(), Step> {
        if let Some(settings) = yget(setup, "settings") {
            let map: serde_json::Map<String, Value> = serde_json::from_str(
                &serde_json::to_string(&yaml_to_json(settings)).unwrap_or_default(),
            )
            .unwrap_or_default();
            if !map.is_empty() {
                self.world
                    .update_settings(&map)
                    .map_err(|err| fail("setup", &err.message))?;
            }
        }
        if let Some(bots) = yget(setup, "bots").and_then(|v| v.as_sequence()) {
            for spec in bots {
                let (alias, bot) = self
                    .ensure_bot(spec)
                    .map_err(|err| fail("setup", &err.message))?;
                self.bots
                    .insert(alias, bot["id"].as_str().unwrap_or("").to_string());
            }
        }
        if let Some(users) = yget(setup, "users").and_then(|v| v.as_sequence()) {
            for spec in users {
                let alias = first_nonempty(&[
                    ystr(yget(spec, "alias")),
                    ystr(yget(spec, "name")),
                    format!("user{}", self.users.len() + 1),
                ]);
                let user = self
                    .world
                    .create_user(
                        nonempty(ystr(yget(spec, "name"))).as_deref(),
                        None,
                        0,
                        0,
                        "",
                        &nonempty(ystr(yget(spec, "actor_type"))).unwrap_or_else(|| "human".into()),
                        &nonempty(ystr(yget(spec, "behavior"))).unwrap_or_else(|| "normal".into()),
                        yget(spec, "seed").and_then(|v| v.as_i64()),
                    )
                    .map_err(|err| fail("setup", &err.message))?;
                let id = user["id"].as_str().unwrap_or("").to_string();
                self.users.insert(alias.clone(), id.clone());
                self.vars.insert(alias, id);
            }
        }
        if let Some(groups) = yget(setup, "groups").and_then(|v| v.as_sequence()) {
            for spec in groups {
                let alias = first_nonempty(&[
                    ystr(yget(spec, "alias")),
                    ystr(yget(spec, "name")),
                    format!("group{}", self.groups.len() + 1),
                ]);
                let owner_alias = ystr(yget(spec, "owner"));
                let owner_id = if owner_alias.is_empty() {
                    None
                } else {
                    Some(
                        self.users
                            .get(&owner_alias)
                            .cloned()
                            .unwrap_or(owner_alias.clone()),
                    )
                };
                let owner_id = match owner_id {
                    Some(id) => Some(id),
                    None => {
                        let users = self
                            .world
                            .list_users(None, false)
                            .map_err(|err| fail("setup", &err.message))?;
                        users
                            .first()
                            .and_then(|user| user["id"].as_str().map(str::to_string))
                    }
                };
                let member_ids: Vec<String> = yget(spec, "members")
                    .and_then(|v| v.as_sequence())
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| {
                                let raw = ystr(Some(item));
                                if raw == owner_alias {
                                    String::new()
                                } else {
                                    self.users.get(&raw).cloned().unwrap_or(raw)
                                }
                            })
                            .filter(|id| !id.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                let mut bot_ids: Vec<String> = yget(spec, "bots")
                    .and_then(|v| v.as_sequence())
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| {
                                let raw = ystr(Some(item));
                                self.bots.get(&raw).cloned().unwrap_or(raw)
                            })
                            .collect()
                    })
                    .unwrap_or_else(|| self.bots.values().cloned().collect());
                if bot_ids.is_empty() {
                    bot_ids = self
                        .world
                        .list_bots()
                        .map_err(|err| fail("setup", &err.message))?
                        .iter()
                        .filter_map(|bot| bot["id"].as_str().map(str::to_string))
                        .collect();
                }
                let group = self
                    .world
                    .create_group(
                        nonempty(ystr(yget(spec, "name"))).as_deref(),
                        owner_id.as_deref(),
                        &member_ids,
                        &bot_ids,
                        "open",
                        0,
                    )
                    .map_err(|err| fail("setup", &err.message))?;
                let recv_setting = ystr(yget(spec, "recv_msg_setting"));
                if !recv_setting.is_empty() {
                    for bot_id in &bot_ids {
                        let mut patch = serde_json::Map::new();
                        patch.insert(
                            "recv_msg_setting".into(),
                            Value::String(recv_setting.clone()),
                        );
                        self.world
                            .update_group_bot_settings(
                                group["id"].as_str().unwrap_or(""),
                                bot_id,
                                &patch,
                            )
                            .map_err(|err| fail("setup", &err.message))?;
                    }
                }
                let gid = group["id"].as_str().unwrap_or("").to_string();
                self.groups.insert(alias.clone(), gid.clone());
                self.vars.insert(alias.clone(), gid.clone());
                self.vars
                    .insert(format!("{alias}_conv"), format!("group:{gid}"));
            }
        }
        Ok(())
    }

    fn ensure_bot(&mut self, spec: &Yaml) -> MqsResult<(String, Value)> {
        let mut spec = spec.clone();
        if spec.is_string() {
            let mut map = serde_yaml::Mapping::new();
            map.insert(Yaml::from("app_id"), spec.clone());
            spec = Yaml::Mapping(map);
        }
        let app_id = first_nonempty(&[ystr(yget(&spec, "app_id")), ystr(yget(&spec, "id"))]);
        let app_id = if app_id.is_empty() {
            "10000".to_string()
        } else {
            app_id
        };
        let alias = first_nonempty(&[
            ystr(yget(&spec, "alias")),
            ystr(yget(&spec, "name")),
            app_id.clone(),
        ]);
        if let Some(existing) = self.world.bot_by_app_id(&app_id)? {
            return Ok((alias, self.world.bot_out(&existing)));
        }
        let name =
            nonempty(ystr(yget(&spec, "name"))).unwrap_or_else(|| format!("MQS Bot {app_id}"));
        let scopes: Option<Vec<String>> = yget(&spec, "scopes")
            .and_then(|v| v.as_sequence())
            .map(|items| items.iter().map(|item| ystr(Some(item))).collect());
        let bot = self.world.create_bot(
            Some(&app_id),
            Some(
                &nonempty(ystr(yget(&spec, "secret")))
                    .unwrap_or_else(|| format!("mqs-secret-{app_id}")),
            ),
            Some(&name),
            None,
            scopes,
            "ws",
            false,
            false,
        )?;
        Ok((alias, bot))
    }

    fn subst(&self, text: &str) -> Result<String, String> {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                return Err(format!("unterminated template: {text}"));
            };
            let key = after[..end].trim();
            match self.vars.get(key) {
                Some(value) => out.push_str(value),
                None => return Err(format!("unknown variable in template: {key}")),
            }
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }

    fn subst_value(&self, value: &Value) -> Result<Value, String> {
        Ok(match value {
            Value::String(text) => Value::String(self.subst(text)?),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.subst_value(item))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| Ok((key.clone(), self.subst_value(item)?)))
                    .collect::<Result<serde_json::Map<_, _>, String>>()?,
            ),
            other => other.clone(),
        })
    }

    async fn step(&mut self, raw: &Yaml) -> Result<String, String> {
        let Yaml::Mapping(mapping) = raw else {
            return Err(format!("invalid step: {raw:?}"));
        };
        let Some((key, value)) = mapping.iter().next() else {
            return Err(format!("invalid step: {raw:?}"));
        };
        let key = ystr(Some(key));
        let value = if value.is_null() {
            Yaml::Mapping(Default::default())
        } else {
            value.clone()
        };
        match key.as_str() {
            "send" => self.send(&value).await,
            "expect_event" => self.expect_event(&value).await,
            "expect_message" => self.expect_message(&value).await,
            "advance" => {
                let ms = parse_duration(&ystr(Some(&value)))?;
                self.world.clock.advance(ms);
                self.world.persist_clock().map_err(|e| e.message)?;
                Ok(format!(
                    "clock advanced by {ms}ms -> {}",
                    self.world.clock.iso(None)
                ))
            }
            "wait" => {
                let ms = parse_duration(&ystr(Some(&value)))?;
                tokio::time::sleep(Duration::from_millis(ms as u64)).await;
                Ok(format!("waited {ms}ms"))
            }
            "http" => self.http(&value).await,
            other => Err(format!("unknown step type: {other}")),
        }
    }

    async fn send(&mut self, value: &Yaml) -> Result<String, String> {
        let user_ref = ystr(yget(value, "as"));
        let Some(user) = self.users.get(&user_ref).cloned() else {
            return Err("send.as is required".into());
        };
        let target = first_nonempty(&[ystr(yget(value, "to")), ystr(yget(value, "conversation"))]);
        if target.is_empty() {
            return Err("send.to is required".into());
        }
        let target = self.subst(&target)?;
        let conversation = if let Some(gid) = self.groups.get(&target) {
            format!("group:{gid}")
        } else {
            self.world
                .resolve_conversation(Some(&target), None)
                .map_err(|err| err.message)?
        };
        let at_bots: Option<Vec<String>> = yget(value, "at_bots")
            .and_then(|v| v.as_sequence())
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let raw = ystr(Some(item));
                        self.bots.get(&raw).cloned().unwrap_or(raw)
                    })
                    .collect()
            });
        let mentions = yget(value, "mentions")
            .and_then(|v| v.as_sequence())
            .map(|items| {
                items
                    .iter()
                    .map(|item| ystr(Some(item)))
                    .collect::<Vec<_>>()
            });
        let reply_to = nonempty(ystr(yget(value, "reply_to")))
            .map(|text| self.subst(&text))
            .transpose()?;
        let result = self
            .world
            .send_as_user(
                &conversation,
                &user,
                &ystr(yget(value, "text")),
                0,
                mentions.as_deref().unwrap_or(&[]),
                at_bots.as_deref(),
                None,
                reply_to.as_deref(),
                &[],
            )
            .await
            .map_err(|err| err.message)?;
        let msg_id = result["message"]["msg_id"]
            .as_str()
            .unwrap_or("")
            .to_string();
        self.vars.insert("last_message_id".into(), msg_id.clone());
        self.vars.insert("incoming_msg_id".into(), msg_id.clone());
        self.vars
            .insert("last_conversation".into(), conversation.clone());
        Ok(format!("sent {msg_id} to {conversation}"))
    }

    async fn expect_event(&mut self, value: &Yaml) -> Result<String, String> {
        let event_type =
            first_nonempty(&[ystr(yget(value, "type")), ystr(yget(value, "event"))]).to_uppercase();
        if event_type.is_empty() {
            return Err("expect_event.type is required".into());
        }
        let mut conversation = ystr(yget(value, "conversation"));
        if let Some(gid) = self.groups.get(&conversation) {
            conversation = format!("group:{gid}");
        }
        if !conversation.is_empty() {
            conversation = self.subst(&conversation)?;
        }
        let contains = ystr(yget(value, "content_contains"));
        let payload_contains = ystr(yget(value, "payload_contains"));
        let timeout = parse_duration(&first_nonempty(&[
            ystr(yget(value, "timeout")),
            "3s".into(),
        ]))? as f64
            / 1000.0;
        let baseline = self.baseline_event_seq;
        let deadline = Instant::now() + Duration::from_secs_f64(timeout);
        while Instant::now() < deadline {
            let rows = self
                .world
                .db
                .all(
                    "SELECT * FROM events WHERE seq > ? ORDER BY seq",
                    &crate::p![baseline],
                )
                .map_err(|err| err.message)?;
            for row in rows {
                let seq = i(&row, "seq");
                if self.consumed_events.contains(&seq) {
                    continue;
                }
                if s(&row, "type") != event_type {
                    continue;
                }
                if !conversation.is_empty() && s(&row, "conversation_id") != conversation {
                    continue;
                }
                let payload: Value =
                    serde_json::from_str(&s(&row, "payload_json")).unwrap_or(json!({}));
                if !contains.is_empty()
                    && !payload
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .contains(&contains)
                {
                    continue;
                }
                if !payload_contains.is_empty() && !payload.to_string().contains(&payload_contains)
                {
                    continue;
                }
                self.consumed_events.insert(seq);
                self.vars
                    .insert("last_event_id".into(), s(&row, "event_id"));
                self.vars.insert("last_event_seq".into(), seq.to_string());
                return Ok(format!("event {event_type} seq={seq}"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(format!(
            "timeout after {timeout:.1}s: no matching event ({event_type})"
        ))
    }

    async fn expect_message(&mut self, value: &Yaml) -> Result<String, String> {
        let mut conversation =
            first_nonempty(&[ystr(yget(value, "conversation")), ystr(yget(value, "to"))]);
        if let Some(gid) = self.groups.get(&conversation) {
            conversation = format!("group:{gid}");
        }
        if conversation.is_empty() {
            return Err("expect_message.conversation is required".into());
        }
        let conversation = self.subst(&conversation)?;
        let direction = ystr(yget(value, "direction"));
        let contains = ystr(yget(value, "content_contains"));
        let timeout = parse_duration(&first_nonempty(&[
            ystr(yget(value, "timeout")),
            "5s".into(),
        ]))? as f64
            / 1000.0;
        let baseline = self.baseline_message_row;
        let deadline = Instant::now() + Duration::from_secs_f64(timeout);
        while Instant::now() < deadline {
            let mut sql = String::from("SELECT * FROM messages WHERE id > ? AND conversation_id=?");
            let mut params: Vec<crate::db::V> = crate::p![baseline, &conversation].to_vec();
            if !direction.is_empty() {
                sql.push_str(" AND direction=?");
                params.push(crate::db::V::from(direction.as_str()));
            }
            sql.push_str(" ORDER BY id");
            let rows = self
                .world
                .db
                .all(&sql, &params)
                .map_err(|err| err.message)?;
            for row in rows {
                let id = i(&row, "id");
                if self.consumed_messages.contains(&id) {
                    continue;
                }
                if !contains.is_empty() && !s(&row, "content").contains(&contains) {
                    continue;
                }
                self.consumed_messages.insert(id);
                self.vars
                    .insert("last_message_id".into(), s(&row, "msg_id"));
                return Ok(format!(
                    "message {} ({})",
                    s(&row, "msg_id"),
                    s(&row, "direction")
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(format!(
            "timeout after {timeout:.1}s: no matching message in {conversation}"
        ))
    }

    async fn http(&mut self, value: &Yaml) -> Result<String, String> {
        let method = first_nonempty(&[ystr(yget(value, "method")), "POST".into()]).to_uppercase();
        let bot_ref = first_nonempty(&[ystr(yget(value, "bot"))]);
        let bot_id = if bot_ref.is_empty() {
            self.bots
                .values()
                .next()
                .cloned()
                .unwrap_or_else(|| "10000".into())
        } else {
            self.bots.get(&bot_ref).cloned().unwrap_or(bot_ref)
        };
        let bot = self.world.bot_by_id(&bot_id).map_err(|err| err.message)?;
        let mut path = self.subst(&ystr(yget(value, "path")))?;
        if path.contains("{openid}") {
            let kind = first_nonempty(&[ystr(yget(value, "target")), "group".into()]);
            let openid = if kind == "group" {
                let group_ref = first_nonempty(&[
                    ystr(yget(value, "group")),
                    ystr(yget(value, "conversation")),
                ]);
                let gid = self.groups.get(&group_ref).cloned().unwrap_or(group_ref);
                self.world
                    .openid(&bot_id, "group", &gid)
                    .map_err(|err| err.message)?
            } else {
                let user_ref = ystr(yget(value, "user"));
                let uid = self.users.get(&user_ref).cloned().unwrap_or(user_ref);
                self.world
                    .openid(&bot_id, "user", &uid)
                    .map_err(|err| err.message)?
            };
            path = path.replace("{openid}", &openid);
        }
        let token = self
            .world
            .issue_token(&s(&bot, "app_id"), &s(&bot, "secret"))
            .map_err(|err| err.message)?;
        let token = token["access_token"].as_str().unwrap_or("").to_string();
        let url = format!("{}{}", self.sim_base.trim_end_matches('/'), path);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|err| err.to_string())?;
        let mut request = client
            .request(
                method
                    .parse::<reqwest::Method>()
                    .map_err(|err| err.to_string())?,
                &url,
            )
            .header("authorization", format!("QQBot {token}"));
        if let Some(json_body) = yget(value, "json") {
            let body = self.subst_value(&yaml_to_json(json_body))?;
            request = request.json(&body);
        }
        if let Some(form) = yget(value, "form") {
            let body = self.subst_value(&yaml_to_json(form))?;
            let mut form = reqwest::multipart::Form::new();
            if let Some(map) = body.as_object() {
                for (key, item) in map {
                    form = form.text(key.clone(), item.to_string().trim_matches('"').to_string());
                }
            }
            request = request.multipart(form);
        }
        if let Some(multipart) = yget(value, "multipart") {
            let mut form = reqwest::multipart::Form::new();
            if let Some(spec) = yget(multipart, "file") {
                let filename = first_nonempty(&[ystr(yget(spec, "filename")), "file.bin".into()]);
                let mime =
                    first_nonempty(&[ystr(yget(spec, "mime")), "application/octet-stream".into()]);
                let data = ystr(yget(spec, "base64"));
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(&data)
                    .map_err(|err| format!("invalid base64 in scenario: {err}"))?;
                let part = reqwest::multipart::Part::bytes(decoded)
                    .file_name(filename)
                    .mime_str(&mime)
                    .map_err(|err| err.to_string())?;
                form = form.part("file", part);
            }
            if let Some(fields) = yget(multipart, "fields") {
                let body = self.subst_value(&yaml_to_json(fields))?;
                if let Some(map) = body.as_object() {
                    for (key, item) in map {
                        form =
                            form.text(key.clone(), item.to_string().trim_matches('"').to_string());
                    }
                }
            }
            request = request.multipart(form);
        }
        let response = request.send().await.map_err(|err| err.to_string())?;
        let status = response.status().as_u16();
        let payload: Value = response.json().await.unwrap_or(Value::Null);
        if let Some(expected) = yget(value, "expect_status").and_then(|v| v.as_i64()) {
            if status as i64 != expected {
                return Err(format!("expected HTTP {expected}, got {status}: {payload}"));
            }
        }
        if let Some(expected) = yget(value, "expect_code").and_then(|v| v.as_i64()) {
            let code = payload.get("code").and_then(|v| v.as_i64());
            if code != Some(expected) {
                return Err(format!("expected code {expected}, got {code:?}: {payload}"));
            }
        }
        if let Some(capture) = yget(value, "capture").and_then(|v| v.as_mapping()) {
            for (alias, dotted) in capture {
                let alias = ystr(Some(alias));
                let dotted = ystr(Some(dotted));
                let mut node = payload.clone();
                for part in dotted.split('.') {
                    node = if let Some(map) = node.as_object() {
                        map.get(part).cloned().unwrap_or(Value::Null)
                    } else if let Some(items) = node.as_array() {
                        part.parse::<usize>()
                            .ok()
                            .and_then(|index| items.get(index).cloned())
                            .unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    };
                }
                if !node.is_null() {
                    self.vars.insert(
                        alias,
                        match &node {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        },
                    );
                }
            }
        }
        if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
            self.vars.insert("last_message_id".into(), id.to_string());
        }
        Ok(format!("{method} {path} -> {status}"))
    }
}

fn yaml_to_json(value: &Yaml) -> Value {
    match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(value) => Value::Bool(*value),
        Yaml::Number(number) => {
            if let Some(value) = number.as_i64() {
                Value::from(value)
            } else if let Some(value) = number.as_f64() {
                Value::from(value)
            } else {
                Value::Null
            }
        }
        Yaml::String(text) => Value::String(text.clone()),
        Yaml::Sequence(items) => Value::Array(items.iter().map(yaml_to_json).collect()),
        Yaml::Mapping(mapping) => Value::Object(
            mapping
                .iter()
                .map(|(key, value)| (ystr(Some(key)), yaml_to_json(value)))
                .collect(),
        ),
        Yaml::Tagged(tagged) => yaml_to_json(&tagged.value),
    }
}

fn step_label(index: usize, raw: &Yaml) -> String {
    let Yaml::Mapping(mapping) = raw else {
        return format!("step-{}", index + 1);
    };
    let Some((key, value)) = mapping.iter().next() else {
        return format!("step-{}", index + 1);
    };
    let kind = ystr(Some(key));
    if value.is_mapping() {
        let hint = first_nonempty(&[
            ystr(yget(value, "name")),
            ystr(yget(value, "text")),
            ystr(yget(value, "type")),
            ystr(yget(value, "path")),
        ]);
        format!("{kind}: {hint}").trim_end_matches(": ").to_string()
    } else {
        format!("{kind}: {}", ystr(Some(value)))
    }
}

fn fail(name: &str, detail: &str) -> Step {
    Step {
        name: name.to_string(),
        ok: false,
        detail: detail.to_string(),
        duration_ms: 0,
    }
}

fn nonempty(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn first_nonempty(values: &[String]) -> String {
    values
        .iter()
        .find(|value| !value.is_empty())
        .cloned()
        .unwrap_or_default()
}

pub async fn run_payload(
    world: &Arc<World>,
    script: Option<&str>,
    path: Option<&str>,
    sim_base: &str,
) -> MqsResult<Value> {
    let (text, name) = if let Some(script) = script.filter(|v| !v.is_empty()) {
        (script.to_string(), "inline".to_string())
    } else if let Some(path) = path.filter(|v| !v.is_empty()) {
        let candidate = std::path::PathBuf::from(path);
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            let from_cwd = std::env::current_dir().unwrap_or_default().join(&candidate);
            if from_cwd.exists() {
                from_cwd
            } else {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../")
                    .join(&candidate)
            }
        };
        if !candidate.exists() {
            return Err(invalid_param(format!("scenario not found: {path}")));
        }
        let name = candidate
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "scenario".into());
        (std::fs::read_to_string(&candidate)?, name)
    } else {
        return Err(invalid_param("script or path is required"));
    };
    let mut runner = Runner {
        world,
        sim_base: sim_base.to_string(),
        vars: HashMap::new(),
        bots: HashMap::new(),
        users: HashMap::new(),
        groups: HashMap::new(),
        baseline_event_seq: 0,
        baseline_message_row: 0,
        consumed_events: HashSet::new(),
        consumed_messages: HashSet::new(),
    };
    let outcome = runner.run(&text, &name).await;
    Ok(outcome.as_value())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_has_ten_templates() {
        assert_eq!(BUILTINS.len(), 10);
        for (name, text) in BUILTINS {
            let doc: Yaml = serde_yaml::from_str(text)
                .unwrap_or_else(|err| panic!("{name} is not valid YAML: {err}"));
            assert!(
                doc.get("steps").and_then(|v| v.as_sequence()).is_some(),
                "{name} has no steps"
            );
        }
    }
}
