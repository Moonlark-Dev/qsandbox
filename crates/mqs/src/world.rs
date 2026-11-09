//! The world model: users, bots, groups, conversations, messages, events.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use serde_json::{json, Map, Value};

use crate::clock::Clock;
use crate::config::ProfilePaths;
use crate::db::{self, b, conn_exec, conn_one, i, json_col, s, s_opt, Database, Row, V};
use crate::error::{
    internal, invalid_app_id, invalid_param, invalid_secret, invalid_token, token_expired,
    MqsResult,
};
use crate::gateway::GatewayManager;
use crate::hub::Hub;
use crate::ids::{
    media_hash, new_event_id, new_session_id, openid_for, union_openid_for, FakeData,
};
use crate::p;
use crate::ratelimit::RateLimiter;

pub const BOT_SCOPE_GROUP: &str = "group_message";
pub const BOT_SCOPE_C2C: &str = "c2c_message";
pub const BOT_SCOPE_MEDIA: &str = "media";
pub const BOT_SCOPE_MEMBER: &str = "member";

#[derive(Debug, Clone)]
pub struct UploadSession {
    pub bot_id: String,
    pub kind: String,
    pub target_openid: String,
    pub file_type: i64,
    pub file_name: String,
    pub size: i64,
    pub mime: String,
    pub block_size: i64,
    pub parts: HashMap<usize, Vec<u8>>,
    pub finished: HashSet<usize>,
    pub srv_send_msg: bool,
}

pub struct World {
    pub db: Arc<Database>,
    pub clock: Arc<Clock>,
    pub profile: String,
    pub paths: ProfilePaths,
    pub settings: RwLock<Map<String, Value>>,
    pub fake: Mutex<FakeData>,
    pub hub: Hub,
    pub rate: Mutex<RateLimiter>,
    pub gateway: OnceLock<Arc<GatewayManager>>,
    pub started_at: i64,
    pub boot_id: String,
    pub uploads: Mutex<HashMap<String, UploadSession>>,
    pub frame_writes: AtomicU64,
    pub sim_base: RwLock<String>,
    me: OnceLock<Weak<World>>,
}

impl World {
    pub fn new(
        db: Arc<Database>,
        clock: Arc<Clock>,
        profile: &str,
        paths: ProfilePaths,
    ) -> Arc<Self> {
        let world = Arc::new(Self {
            db,
            clock: clock.clone(),
            profile: profile.to_string(),
            paths,
            settings: RwLock::new(Map::new()),
            fake: Mutex::new(FakeData::new(42)),
            hub: Hub::new(),
            rate: Mutex::new(RateLimiter::new(clock, "certified")),
            gateway: OnceLock::new(),
            started_at: chrono::Utc::now().timestamp_millis(),
            boot_id: new_session_id(),
            uploads: Mutex::new(HashMap::new()),
            frame_writes: AtomicU64::new(0),
            sim_base: RwLock::new(String::new()),
            me: OnceLock::new(),
        });
        let _ = world.me.set(Arc::downgrade(&world));
        world
    }

    pub fn arc(&self) -> Arc<World> {
        self.me
            .get()
            .and_then(|weak| weak.upgrade())
            .expect("world Arc is alive")
    }

    // ------------------------------------------------------------------ lifecycle
    pub fn start(&self) -> MqsResult<()> {
        let settings = self.db.all_settings()?;
        let seed = settings.get("seed").and_then(|v| v.as_i64()).unwrap_or(42);
        self.fake.lock().unwrap().reseed(seed);
        let mode = settings
            .get("clock_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("real")
            .to_string();
        let now_ms = settings
            .get("clock_now_ms")
            .and_then(|v| v.as_i64())
            .filter(|v| *v > 0)
            .unwrap_or_else(|| self.clock.now_ms());
        self.clock.restore(&mode, now_ms);
        let rate_mode = settings
            .get("rate_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("certified");
        let _ = self.rate.lock().unwrap().set_mode(rate_mode);
        *self.settings.write().unwrap() = settings;
        self.bootstrap()?;
        self.db.prune_frames(2000)?;
        Ok(())
    }

    fn bootstrap(&self) -> MqsResult<()> {
        let bots = self.db.one("SELECT COUNT(*) AS c FROM bots", &[])?;
        if bots.map(|row| i(&row, "c")).unwrap_or(0) == 0 {
            self.create_bot(
                Some("10000"),
                Some("mqs-secret-10000"),
                Some("QSandbox 测试 Bot"),
                None,
                Some(vec![
                    BOT_SCOPE_GROUP.to_string(),
                    BOT_SCOPE_C2C.to_string(),
                    BOT_SCOPE_MEDIA.to_string(),
                    BOT_SCOPE_MEMBER.to_string(),
                ]),
                "ws",
                false,
                false,
            )?;
        }
        let users = self.db.one("SELECT COUNT(*) AS c FROM users", &[])?;
        if users.map(|row| i(&row, "c")).unwrap_or(0) == 0 {
            self.create_user(Some("小满"), None, 0, 0, "", "human", "normal", None)?;
            for name in ["青禾", "月汐", "阿柚"] {
                self.create_user(Some(name), None, 0, 0, "", "npc", "silent", None)?;
            }
        }
        let groups = self.db.one("SELECT COUNT(*) AS c FROM groups", &[])?;
        if groups.map(|row| i(&row, "c")).unwrap_or(0) == 0 {
            let owner = self
                .db
                .one(
                    "SELECT id FROM users WHERE actor_type='human' ORDER BY id LIMIT 1",
                    &[],
                )?
                .map(|row| s(&row, "id"));
            let members = self.db.all("SELECT id FROM users ORDER BY id", &[])?;
            if let Some(owner_id) = owner {
                let member_ids: Vec<String> = members
                    .iter()
                    .map(|row| s(row, "id"))
                    .filter(|id| id != &owner_id)
                    .collect();
                self.create_group(
                    Some("QSandbox 测试群"),
                    Some(&owner_id),
                    &member_ids,
                    &[],
                    "open",
                    0,
                )?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ settings
    pub fn setting(&self, key: &str) -> Value {
        self.settings
            .read()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or_else(|| {
                db::default_settings()
                    .get(key)
                    .cloned()
                    .unwrap_or(Value::Null)
            })
    }

    pub fn setting_str(&self, key: &str) -> String {
        self.setting(key).as_str().unwrap_or("").to_string()
    }

    pub fn setting_i64(&self, key: &str) -> i64 {
        self.setting(key).as_i64().unwrap_or(0)
    }

    pub fn setting_bool(&self, key: &str) -> bool {
        match self.setting(key) {
            Value::Bool(v) => v,
            Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
            Value::String(text) => text == "1" || text == "true",
            _ => false,
        }
    }

    pub fn update_settings(&self, patch: &Map<String, Value>) -> MqsResult<()> {
        for (incoming, value) in patch.iter() {
            if value.is_null() {
                continue;
            }
            let mut key = incoming.clone();
            match key.as_str() {
                "feature_whitelist" => key = "feature.whitelist".into(),
                "feature_expose" => key = "feature.expose".into(),
                "chat_auto_at_bot" => key = "chat.auto_at_bot".into(),
                _ => {}
            }
            if key == "clock_mode" {
                let mode = value.as_str().unwrap_or("real").to_string();
                self.clock.set_mode(&mode);
                self.db
                    .set_setting("clock_mode", &Value::String(mode.clone()))?;
                let now = self.clock.now_ms();
                self.db.set_setting("clock_now_ms", &Value::from(now))?;
                let mut settings = self.settings.write().unwrap();
                settings.insert("clock_mode".into(), Value::String(mode));
                settings.insert("clock_now_ms".into(), Value::from(now));
                continue;
            }
            let mut value = value.clone();
            if key == "seed" {
                if let Some(seed) = value.as_i64() {
                    self.fake.lock().unwrap().reseed(seed);
                }
            }
            if key == "rate_mode" {
                let mode = match &value {
                    Value::Bool(false) => "off".to_string(),
                    Value::Bool(true) => "certified".to_string(),
                    other => other.as_str().unwrap_or("certified").to_string(),
                };
                self.rate
                    .lock()
                    .unwrap()
                    .set_mode(&mode)
                    .map_err(invalid_param)?;
                value = Value::String(self.rate.lock().unwrap().mode.clone());
            }
            self.db.set_setting(&key, &value)?;
            self.settings.write().unwrap().insert(key, value);
        }
        Ok(())
    }

    pub fn persist_clock(&self) -> MqsResult<()> {
        self.db
            .set_setting("clock_mode", &Value::String(self.clock.mode()))?;
        self.db
            .set_setting("clock_now_ms", &Value::from(self.clock.now_ms()))?;
        let mut settings = self.settings.write().unwrap();
        settings.insert("clock_mode".into(), Value::String(self.clock.mode()));
        settings.insert("clock_now_ms".into(), Value::from(self.clock.now_ms()));
        Ok(())
    }

    // ------------------------------------------------------------------ serializers
    pub fn avatar(entity_id: &str, label: &str, kind: &str) -> String {
        let encoded: String = percent_encoding::utf8_percent_encode(
            if label.is_empty() { entity_id } else { label },
            percent_encoding::NON_ALPHANUMERIC,
        )
        .to_string();
        format!("/avatars/{entity_id}.svg?label={encoded}&kind={kind}")
    }

    pub fn user_out(&self, row: &Row) -> Value {
        let mut data = row.clone();
        let kind = if s(row, "actor_type") == "npc" {
            "npc"
        } else {
            "user"
        };
        let id = s(row, "id");
        let nickname = s(row, "nickname");
        data.insert(
            "avatar".into(),
            Value::String(Self::avatar(&id, &nickname, kind)),
        );
        Value::Object(data)
    }

    pub fn bot_out(&self, row: &Row) -> Value {
        let mut data = row.clone();
        let id = s(row, "id");
        let name = s(row, "name");
        data.insert(
            "avatar".into(),
            Value::String(Self::avatar(&format!("bot_{id}"), &name, "bot")),
        );
        data.insert(
            "scopes".into(),
            json_col(row, "scopes", Value::Array(vec![])),
        );
        data.insert("whitelisted".into(), Value::Bool(b(row, "whitelisted")));
        data.insert(
            "allow_proactive_msg".into(),
            Value::Bool(b(row, "allow_proactive_msg")),
        );
        let online = self
            .gateway
            .get()
            .map(|gateway| gateway.is_online(&id))
            .unwrap_or(false);
        data.insert("online".into(), Value::Bool(online));
        Value::Object(data)
    }

    pub fn group_out(&self, row: &Row) -> Value {
        let mut data = row.clone();
        let gid = s(row, "id");
        let name = s(row, "name");
        let count = self
            .db
            .one(
                "SELECT COUNT(*) AS c FROM group_members WHERE group_id=?",
                &p![&gid],
            )
            .ok()
            .flatten()
            .map(|row| i(&row, "c"))
            .unwrap_or(0);
        let bots = self
            .db
            .all(
                "SELECT b.id, b.name FROM group_bots gb JOIN bots b ON b.id=gb.bot_id WHERE gb.group_id=?",
                &p![&gid],
            )
            .unwrap_or_default();
        let owner = self
            .db
            .one(
                "SELECT user_id FROM group_members WHERE group_id=? AND role='owner'",
                &p![&gid],
            )
            .ok()
            .flatten()
            .map(|row| s(&row, "user_id"));
        data.insert(
            "avatar".into(),
            Value::String(Self::avatar(&gid, &name, "group")),
        );
        data.insert("member_count".into(), Value::from(count));
        data.insert(
            "bot_ids".into(),
            Value::Array(bots.iter().map(|b| Value::String(s(b, "id"))).collect()),
        );
        data.insert(
            "bots".into(),
            Value::Array(
                bots.iter()
                    .map(|b| json!({"id": s(b, "id"), "name": s(b, "name")}))
                    .collect(),
            ),
        );
        data.insert(
            "owner_id".into(),
            owner.map(Value::String).unwrap_or(Value::Null),
        );
        data.insert(
            "conversation_id".into(),
            Value::String(format!("group:{gid}")),
        );
        data.insert("tags".into(), json_col(row, "tags", Value::Array(vec![])));
        Value::Object(data)
    }

    // ------------------------------------------------------------------ users
    #[allow(clippy::too_many_arguments)]
    pub fn create_user(
        &self,
        nickname: Option<&str>,
        avatar: Option<&str>,
        gender: i64,
        age: i64,
        city: &str,
        actor_type: &str,
        behavior: &str,
        seed: Option<i64>,
    ) -> MqsResult<Value> {
        let seed = seed.unwrap_or_else(|| self.fake.lock().unwrap().seed);
        let nickname = match nickname {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => self.fake.lock().unwrap().nickname(),
        };
        let city = if city.is_empty() {
            self.fake.lock().unwrap().city()
        } else {
            city.to_string()
        };
        let now = self.clock.now_ms();
        let out = self.db.tx(|conn| {
            let count = conn_one(conn, "SELECT COUNT(*) AS c FROM users", &[])?
                .map(|row| i(&row, "c"))
                .unwrap_or(0);
            let uid = format!("u_{:06}", count + 1);
            let avatar = avatar
                .map(|v| v.to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| {
                    Self::avatar(&uid, &nickname, if actor_type == "npc" { "npc" } else { "user" })
                });
            conn_exec(
                conn,
                "INSERT INTO users(id,nickname,avatar,gender,age,city,actor_type,seed,behavior,created_at) \
                 VALUES(?,?,?,?,?,?,?,?,?,?)",
                &p![&uid, &nickname, &avatar, gender, age, &city, actor_type, seed, behavior, now],
            )?;
            conn_one(conn, "SELECT * FROM users WHERE id=?", &p![&uid])
        })?;
        Ok(self.user_out(&out.expect("inserted user")))
    }

    pub fn create_random_users(
        &self,
        count: i64,
        seed: Option<i64>,
        behavior: Option<&str>,
        actor_type: &str,
        group_id: Option<&str>,
    ) -> MqsResult<Vec<Value>> {
        if !(1..=500).contains(&count) {
            return Err(invalid_param("count must be between 1 and 500"));
        }
        let base_seed = seed.unwrap_or_else(|| self.fake.lock().unwrap().seed);
        let mut created = Vec::new();
        for index in 0..count {
            let (gender, age, city, behavior) = {
                let mut fake = self.fake.lock().unwrap();
                (
                    fake.gender(),
                    fake.age(),
                    fake.city(),
                    behavior
                        .map(|b| b.to_string())
                        .unwrap_or_else(|| fake.behavior()),
                )
            };
            let user = self.create_user(
                None,
                None,
                gender,
                age,
                &city,
                actor_type,
                &behavior,
                Some(base_seed + index),
            )?;
            if let Some(gid) = group_id {
                self.add_member(gid, user["id"].as_str().unwrap(), "member")?;
            }
            created.push(user);
        }
        Ok(created)
    }

    pub fn list_users(&self, actor_type: Option<&str>, available: bool) -> MqsResult<Vec<Value>> {
        let mut sql = String::from("SELECT * FROM users");
        let mut params: Vec<V> = Vec::new();
        if let Some(kind) = actor_type.filter(|k| !k.is_empty()) {
            sql.push_str(" WHERE actor_type=?");
            params.push(V::from(kind));
        } else if available {
            sql.push_str(" WHERE actor_type='human'");
        }
        sql.push_str(" ORDER BY id");
        Ok(self
            .db
            .all(&sql, &params)?
            .iter()
            .map(|row| self.user_out(row))
            .collect())
    }

    pub fn get_user(&self, user_id: &str) -> MqsResult<Value> {
        let row = self
            .db
            .one("SELECT * FROM users WHERE id=?", &p![user_id])?
            .ok_or_else(|| invalid_param(format!("user not found: {user_id}")))?;
        Ok(self.user_out(&row))
    }

    pub fn update_user(&self, user_id: &str, patch: &Map<String, Value>) -> MqsResult<Value> {
        self.get_user(user_id)?;
        let allowed = [
            "nickname",
            "avatar",
            "gender",
            "age",
            "city",
            "behavior",
            "actor_type",
        ];
        let fields: Vec<(String, Value)> = patch
            .iter()
            .filter(|(key, value)| !value.is_null() && allowed.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if !fields.is_empty() {
            let cols: Vec<String> = fields.iter().map(|(key, _)| format!("{key}=?")).collect();
            let mut params: Vec<V> = fields.iter().map(|(_, value)| V::from(value)).collect();
            params.push(V::from(user_id));
            self.db.execute(
                &format!("UPDATE users SET {} WHERE id=?", cols.join(", ")),
                &params,
            )?;
        }
        self.get_user(user_id)
    }

    pub fn delete_user(&self, user_id: &str) -> MqsResult<()> {
        self.get_user(user_id)?;
        self.db
            .execute("DELETE FROM openid_map WHERE entity_id=?", &p![user_id])?;
        self.db
            .execute("DELETE FROM users WHERE id=?", &p![user_id])?;
        Ok(())
    }

    // ------------------------------------------------------------------ openids
    pub fn openid(&self, bot_id: &str, kind: &str, entity_id: &str) -> MqsResult<String> {
        self.db.tx(|conn| {
            let bot = conn_one(conn, "SELECT * FROM bots WHERE id=?", &p![bot_id])?
                .ok_or_else(|| invalid_param(format!("bot not found: {bot_id}")))?;
            Self::openid_tx(conn, &bot, kind, entity_id)
        })
    }

    pub fn openid_tx(
        conn: &rusqlite::Connection,
        bot: &Row,
        kind: &str,
        entity_id: &str,
    ) -> MqsResult<String> {
        let bot_id = s(bot, "id");
        if let Some(row) = conn_one(
            conn,
            "SELECT openid FROM openid_map WHERE bot_id=? AND kind=? AND entity_id=?",
            &p![&bot_id, kind, entity_id],
        )? {
            return Ok(s(&row, "openid"));
        }
        let openid = openid_for(&s(bot, "app_id"), kind, entity_id);
        conn_exec(
            conn,
            "INSERT INTO openid_map(bot_id,kind,entity_id,openid,union_openid) VALUES(?,?,?,?,?)",
            &p![
                &bot_id,
                kind,
                entity_id,
                &openid,
                union_openid_for(entity_id)
            ],
        )?;
        Ok(openid)
    }

    pub fn resolve_openid(&self, bot_id: &str, openid: &str) -> MqsResult<Row> {
        self.db
            .one(
                "SELECT * FROM openid_map WHERE bot_id=? AND openid=?",
                &p![bot_id, openid],
            )?
            .ok_or_else(|| invalid_param(format!("unknown openid for this bot: {openid}")))
    }

    // ------------------------------------------------------------------ bots
    #[allow(clippy::too_many_arguments)]
    pub fn create_bot(
        &self,
        app_id: Option<&str>,
        secret: Option<&str>,
        name: Option<&str>,
        avatar: Option<&str>,
        scopes: Option<Vec<String>>,
        delivery: &str,
        random: bool,
        whitelisted: bool,
    ) -> MqsResult<Value> {
        let out = self.db.tx(|conn| {
            let app_id = match app_id.filter(|v| !v.is_empty()) {
                Some(value) => value.to_string(),
                None => {
                    let max = conn_one(conn, "SELECT COALESCE(MAX(CAST(app_id AS INTEGER)),10000) AS m FROM bots", &[])?
                        .map(|row| i(&row, "m"))
                        .unwrap_or(10000);
                    (max + 1).to_string()
                }
            };
            if conn_one(conn, "SELECT id FROM bots WHERE app_id=?", &p![&app_id])?.is_some() {
                return Err(invalid_param(format!("bot with app_id {app_id} already exists")));
            }
            let bot_id = app_id.clone();
            let secret = secret
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
                .unwrap_or_else(|| format!("mqs-secret-{app_id}"));
            let (name, scopes) = {
                let mut fake = self.fake.lock().unwrap();
                let name = match name.filter(|v| !v.is_empty()) {
                    Some(name) => name.to_string(),
                    None => fake.bot_name(),
                };
                let scopes = if random {
                    fake.scopes()
                } else {
                    scopes.clone().unwrap_or_else(|| {
                        vec![
                            BOT_SCOPE_GROUP.to_string(),
                            BOT_SCOPE_C2C.to_string(),
                            BOT_SCOPE_MEDIA.to_string(),
                        ]
                    })
                };
                (name, scopes)
            };
            let avatar = avatar
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
                .unwrap_or_else(|| Self::avatar(&format!("bot_{bot_id}"), &name, "bot"));
            conn_exec(
                conn,
                "INSERT INTO bots(id,app_id,secret,name,avatar,scopes,delivery,whitelisted,created_at) \
                 VALUES(?,?,?,?,?,?,?,?,?)",
                &p![
                    &bot_id,
                    &app_id,
                    &secret,
                    &name,
                    &avatar,
                    Value::Array(scopes.iter().map(|v| Value::String(v.clone())).collect()),
                    delivery,
                    whitelisted,
                    self.clock.now_ms()
                ],
            )?;
            conn_one(conn, "SELECT * FROM bots WHERE id=?", &p![&bot_id])
        })?;
        Ok(self.bot_out(&out.expect("inserted bot")))
    }

    pub fn list_bots(&self) -> MqsResult<Vec<Value>> {
        Ok(self
            .db
            .all("SELECT * FROM bots ORDER BY id", &[])?
            .iter()
            .map(|row| self.bot_out(row))
            .collect())
    }

    pub fn get_bot(&self, bot_id: &str) -> MqsResult<Value> {
        Ok(self.bot_out(&self.bot_by_id(bot_id)?))
    }

    pub fn bot_by_id(&self, bot_id: &str) -> MqsResult<Row> {
        self.db
            .one("SELECT * FROM bots WHERE id=?", &p![bot_id])?
            .ok_or_else(|| invalid_param(format!("bot not found: {bot_id}")))
    }

    pub fn bot_by_app_id(&self, app_id: &str) -> MqsResult<Option<Row>> {
        self.db
            .one("SELECT * FROM bots WHERE app_id=?", &p![app_id])
    }

    pub fn update_bot(&self, bot_id: &str, patch: &Map<String, Value>) -> MqsResult<Value> {
        self.bot_by_id(bot_id)?;
        let allowed = [
            "name",
            "avatar",
            "scopes",
            "delivery",
            "whitelisted",
            "allow_proactive_msg",
        ];
        let fields: Vec<(String, Value)> = patch
            .iter()
            .filter(|(key, value)| !value.is_null() && allowed.contains(&key.as_str()))
            .map(|(key, value)| {
                let value = match (key.as_str(), value) {
                    ("whitelisted", Value::Bool(v)) => Value::from(*v),
                    ("allow_proactive_msg", Value::Bool(v)) => Value::from(*v),
                    _ => value.clone(),
                };
                (key.clone(), value)
            })
            .collect();
        if !fields.is_empty() {
            let cols: Vec<String> = fields.iter().map(|(key, _)| format!("{key}=?")).collect();
            let mut params: Vec<V> = fields.iter().map(|(_, value)| V::from(value)).collect();
            params.push(V::from(bot_id));
            self.db.execute(
                &format!("UPDATE bots SET {} WHERE id=?", cols.join(", ")),
                &params,
            )?;
        }
        self.get_bot(bot_id)
    }

    pub fn delete_bot(&self, bot_id: &str) -> MqsResult<()> {
        self.bot_by_id(bot_id)?;
        self.db
            .execute("DELETE FROM openid_map WHERE bot_id=?", &p![bot_id])?;
        self.db
            .execute("DELETE FROM app_tokens WHERE bot_id=?", &p![bot_id])?;
        self.db
            .execute("DELETE FROM bots WHERE id=?", &p![bot_id])?;
        Ok(())
    }

    pub fn rotate_secret(&self, bot_id: &str) -> MqsResult<Value> {
        self.bot_by_id(bot_id)?;
        let secret = format!("mqs-secret-{bot_id}-{}", &new_session_id()[..10]);
        self.db
            .execute("UPDATE bots SET secret=? WHERE id=?", &p![&secret, bot_id])?;
        self.db
            .execute("DELETE FROM app_tokens WHERE bot_id=?", &p![bot_id])?;
        Ok(json!({"bot_id": bot_id, "secret": secret}))
    }

    pub fn issue_token(&self, app_id: &str, secret: &str) -> MqsResult<Value> {
        let bot = self
            .bot_by_app_id(app_id)?
            .ok_or_else(|| invalid_app_id("app_id not found in this sandbox"))?;
        if secret != s(&bot, "secret") {
            return Err(invalid_secret("client_secret does not match"));
        }
        let bot_id = s(&bot, "id");
        let now = self.clock.now_ms();
        // Official behaviour: the same token is returned while it is still valid;
        // a new one is only issued inside the last 60 seconds of its lifetime.
        if let Some(existing) = self.db.one(
            "SELECT token, expires_at FROM app_tokens WHERE bot_id=? AND expires_at > ? \
             ORDER BY expires_at DESC LIMIT 1",
            &p![&bot_id, now + 60_000],
        )? {
            let remaining = (i(&existing, "expires_at") - now) / 1000;
            return Ok(json!({
                "access_token": s(&existing, "token"),
                "expires_in": remaining.to_string(),
            }));
        }
        let token = format!("MQS_{}", new_session_id());
        let expires_at = now + 7200 * 1000;
        self.db.execute(
            "INSERT INTO app_tokens(token,bot_id,expires_at,created_at) VALUES(?,?,?,?)",
            &p![&token, &bot_id, expires_at, now],
        )?;
        Ok(json!({"access_token": token, "expires_in": "7200"}))
    }

    pub fn authenticate_bot(&self, token: &str) -> MqsResult<Value> {
        let token = token.strip_prefix("QQBot ").unwrap_or(token);
        let row = self.db.one(
            "SELECT b.*, t.expires_at AS token_expires FROM app_tokens t JOIN bots b ON b.id=t.bot_id WHERE t.token=?",
            &p![token],
        )?;
        let row = row.ok_or_else(|| invalid_token("access token is not valid for this sandbox"))?;
        if i(&row, "token_expires") < self.clock.now_ms() {
            return Err(token_expired("access token expired"));
        }
        Ok(self.bot_out(&row))
    }

    // ------------------------------------------------------------------ groups
    pub fn create_group(
        &self,
        name: Option<&str>,
        owner_id: Option<&str>,
        member_ids: &[String],
        bot_ids: &[String],
        join_policy: &str,
        random_members: i64,
    ) -> MqsResult<Value> {
        let mut member_ids: Vec<String> = member_ids.to_vec();
        if random_members > 0 {
            let extra = self.create_random_users(random_members, None, None, "human", None)?;
            member_ids.extend(
                extra
                    .iter()
                    .filter_map(|u| u["id"].as_str().map(str::to_string)),
            );
        }
        let owner_id = match owner_id.filter(|v| !v.is_empty()) {
            Some(id) => Some(id.to_string()),
            None => self
                .db
                .one(
                    "SELECT id FROM users WHERE actor_type='human' ORDER BY id LIMIT 1",
                    &[],
                )?
                .or(self
                    .db
                    .one("SELECT id FROM users ORDER BY id LIMIT 1", &[])?)
                .map(|row| s(&row, "id")),
        };
        let name = name
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string())
            .unwrap_or_else(|| self.fake.lock().unwrap().group_name());
        let bot_ids: Vec<String> = if bot_ids.is_empty() {
            self.db
                .all("SELECT id FROM bots", &[])?
                .iter()
                .map(|row| s(row, "id"))
                .collect()
        } else {
            bot_ids.to_vec()
        };
        let out = self.db.tx(|conn| {
            let count = conn_one(conn, "SELECT COUNT(*) AS c FROM groups", &[])?
                .map(|row| i(&row, "c"))
                .unwrap_or(0);
            let gid = format!("g_{:06}", count + 1);
            let now = self.clock.now_ms();
            let avatar = Self::avatar(&gid, &name, "group");
            conn_exec(
                conn,
                "INSERT INTO groups(id,name,owner_id,join_policy,avatar,created_at) VALUES(?,?,?,?,?,?)",
                &p![&gid, &name, owner_id.clone(), join_policy, &avatar, now],
            )?;
            conn_exec(
                conn,
                "INSERT OR IGNORE INTO conversations(id,type,peer_id,bot_id,last_seq) VALUES(?,?,?,NULL,0)",
                &p![format!("group:{gid}"), "group", &gid],
            )?;
            let mut members: Vec<String> = Vec::new();
            for id in &member_ids {
                if !members.contains(id) {
                    members.push(id.clone());
                }
            }
            if let Some(owner) = &owner_id {
                members.retain(|id| id != owner);
                members.insert(0, owner.clone());
            }
            for uid in &members {
                let role = if Some(uid) == owner_id.as_ref() { "owner" } else { "member" };
                conn_exec(
                    conn,
                    "INSERT OR IGNORE INTO group_members(group_id,user_id,role,joined_at) VALUES(?,?,?,?)",
                    &p![&gid, uid, role, now],
                )?;
            }
            let mut unique_bots: Vec<String> = Vec::new();
            for id in &bot_ids {
                if !unique_bots.contains(id) {
                    unique_bots.push(id.clone());
                }
            }
            for bid in &unique_bots {
                conn_exec(
                    conn,
                    "INSERT OR IGNORE INTO group_bots(group_id,bot_id,joined_at) VALUES(?,?,?)",
                    &p![&gid, bid, now],
                )?;
                if let Some(bot) = conn_one(conn, "SELECT * FROM bots WHERE id=?", &p![bid])? {
                    Self::openid_tx(conn, &bot, "group", &gid)?;
                }
            }
            conn_one(conn, "SELECT * FROM groups WHERE id=?", &p![&gid])
        })?;
        let out = out.expect("inserted group");
        let gid = s(&out, "id");
        for bid in &bot_ids {
            if self
                .db
                .one(
                    "SELECT 1 FROM group_bots WHERE group_id=? AND bot_id=?",
                    &p![&gid, bid],
                )?
                .is_some()
            {
                self.emit_group_robot_event(bid, &gid, "GROUP_ADD_ROBOT", owner_id.as_deref())?;
            }
        }
        Ok(self.group_out(&out))
    }

    pub fn list_groups(&self) -> MqsResult<Vec<Value>> {
        Ok(self
            .db
            .all("SELECT * FROM groups ORDER BY id", &[])?
            .iter()
            .map(|row| self.group_out(row))
            .collect())
    }

    pub fn get_group(&self, group_id: &str) -> MqsResult<Value> {
        let row = self
            .db
            .one("SELECT * FROM groups WHERE id=?", &p![group_id])?
            .ok_or_else(|| invalid_param(format!("group not found: {group_id}")))?;
        Ok(self.group_out(&row))
    }

    pub fn update_group(&self, group_id: &str, patch: &Map<String, Value>) -> MqsResult<Value> {
        self.get_group(group_id)?;
        let allowed = [
            "name",
            "owner_id",
            "join_policy",
            "finger_memo",
            "class_text",
            "tags",
        ];
        let fields: Vec<(String, Value)> = patch
            .iter()
            .filter(|(key, value)| !value.is_null() && allowed.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if !fields.is_empty() {
            let cols: Vec<String> = fields.iter().map(|(key, _)| format!("{key}=?")).collect();
            let mut params: Vec<V> = fields.iter().map(|(_, value)| V::from(value)).collect();
            params.push(V::from(group_id));
            self.db.execute(
                &format!("UPDATE groups SET {} WHERE id=?", cols.join(", ")),
                &params,
            )?;
        }
        self.get_group(group_id)
    }

    pub fn delete_group(&self, group_id: &str) -> MqsResult<()> {
        self.get_group(group_id)?;
        self.db
            .execute("DELETE FROM groups WHERE id=?", &p![group_id])?;
        Ok(())
    }

    pub fn list_members(&self, group_id: &str) -> MqsResult<Vec<Value>> {
        let rows = self.db.all(
            "SELECT gm.*, u.nickname, u.avatar, u.actor_type, u.behavior FROM group_members gm \
             JOIN users u ON u.id=gm.user_id WHERE gm.group_id=? ORDER BY gm.joined_at, gm.user_id",
            &p![group_id],
        )?;
        Ok(rows
            .iter()
            .map(|row| {
                let mut data = row.clone();
                let uid = s(row, "user_id");
                let nickname = s(row, "nickname");
                let kind = if s(row, "actor_type") == "npc" {
                    "npc"
                } else {
                    "user"
                };
                data.insert(
                    "avatar".into(),
                    Value::String(Self::avatar(&uid, &nickname, kind)),
                );
                Value::Object(data)
            })
            .collect())
    }

    pub fn add_member(&self, group_id: &str, user_id: &str, role: &str) -> MqsResult<Value> {
        self.get_group(group_id)?;
        self.get_user(user_id)?;
        let now = self.clock.now_ms();
        self.db.tx(|conn| {
            if role == "owner" {
                conn_exec(
                    conn,
                    "UPDATE group_members SET role='admin' WHERE group_id=? AND role='owner'",
                    &p![group_id],
                )?;
            }
            conn_exec(
                conn,
                "INSERT INTO group_members(group_id,user_id,role,joined_at) VALUES(?,?,?,?) \
                 ON CONFLICT(group_id,user_id) DO UPDATE SET role=excluded.role",
                &p![group_id, user_id, role, now],
            )?;
            Ok(())
        })?;
        let members = self.list_members(group_id)?;
        Ok(members.last().cloned().unwrap_or(Value::Null))
    }

    pub fn update_member(
        &self,
        group_id: &str,
        user_id: &str,
        patch: &Map<String, Value>,
    ) -> MqsResult<Value> {
        let mut fields: Vec<(String, Value)> = Vec::new();
        if let Some(role) = patch.get("role").and_then(|v| v.as_str()) {
            fields.push(("role".into(), Value::String(role.to_string())));
        }
        if let Some(seconds) = patch.get("mute_seconds").and_then(|v| v.as_i64()) {
            fields.push((
                "mute_until".into(),
                Value::from(self.clock.now_ms() + seconds * 1000),
            ));
        }
        if !fields.is_empty() {
            let cols: Vec<String> = fields.iter().map(|(key, _)| format!("{key}=?")).collect();
            let mut params: Vec<V> = fields.iter().map(|(_, value)| V::from(value)).collect();
            params.push(V::from(group_id));
            params.push(V::from(user_id));
            self.db.execute(
                &format!(
                    "UPDATE group_members SET {} WHERE group_id=? AND user_id=?",
                    cols.join(", ")
                ),
                &params,
            )?;
        }
        let members = self.list_members(group_id)?;
        members
            .into_iter()
            .find(|m| m["user_id"].as_str() == Some(user_id))
            .ok_or_else(|| invalid_param(format!("member not found: {user_id}")))
    }

    pub fn remove_member(&self, group_id: &str, user_id: &str) -> MqsResult<()> {
        self.db.execute(
            "DELETE FROM group_members WHERE group_id=? AND user_id=?",
            &p![group_id, user_id],
        )?;
        Ok(())
    }

    pub fn add_bot_to_group(&self, group_id: &str, bot_id: &str) -> MqsResult<Value> {
        self.get_group(group_id)?;
        self.bot_by_id(bot_id)?;
        self.db.execute(
            "INSERT OR IGNORE INTO group_bots(group_id,bot_id,joined_at) VALUES(?,?,?)",
            &p![group_id, bot_id, self.clock.now_ms()],
        )?;
        self.openid(bot_id, "group", group_id)?;
        let group = self.get_group(group_id)?;
        self.emit_group_robot_event(
            bot_id,
            group_id,
            "GROUP_ADD_ROBOT",
            group["owner_id"].as_str(),
        )?;
        Ok(group)
    }

    pub fn remove_bot_from_group(&self, group_id: &str, bot_id: &str) -> MqsResult<()> {
        let group = self
            .db
            .one("SELECT * FROM groups WHERE id=?", &p![group_id])?;
        self.emit_group_robot_event(
            bot_id,
            group_id,
            "GROUP_DEL_ROBOT",
            group
                .as_ref()
                .and_then(|row| s_opt(row, "owner_id"))
                .as_deref(),
        )?;
        self.db.execute(
            "DELETE FROM group_bots WHERE group_id=? AND bot_id=?",
            &p![group_id, bot_id],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------ conversations
    pub fn list_conversations(&self) -> MqsResult<Vec<Value>> {
        let mut out: Vec<Value> = Vec::new();
        for group in self.db.all("SELECT * FROM groups ORDER BY id", &[])? {
            let gid = s(&group, "id");
            let name = s(&group, "name");
            let count = self
                .db
                .one(
                    "SELECT COUNT(*) AS c FROM group_members WHERE group_id=?",
                    &p![&gid],
                )?
                .map(|row| i(&row, "c"))
                .unwrap_or(0);
            let member_ids: Vec<Value> = self
                .db
                .all(
                    "SELECT user_id FROM group_members WHERE group_id=?",
                    &p![&gid],
                )?
                .iter()
                .map(|row| Value::String(s(row, "user_id")))
                .collect();
            let bots = self.db.all(
                "SELECT b.id, b.name FROM group_bots gb JOIN bots b ON b.id=gb.bot_id WHERE gb.group_id=?",
                &p![&gid],
            )?;
            out.push(json!({
                "id": format!("group:{gid}"),
                "type": "group",
                "peer_id": gid,
                "name": name,
                "avatar": Self::avatar(&gid, &name, "group"),
                "member_count": count,
                "member_ids": member_ids,
                "bot_ids": bots.iter().map(|b| Value::String(s(b, "id"))).collect::<Vec<_>>(),
                "last_message": self.last_message(&format!("group:{gid}"))?,
            }));
        }
        for dm in self.db.all(
            "SELECT * FROM conversations WHERE type='dm' ORDER BY id",
            &[],
        )? {
            let user = self
                .db
                .one("SELECT * FROM users WHERE id=?", &p![s(&dm, "peer_id")])?;
            let bot = self
                .db
                .one("SELECT * FROM bots WHERE id=?", &p![s(&dm, "bot_id")])?;
            let (Some(user), Some(bot)) = (user, bot) else {
                continue;
            };
            out.push(json!({
                "id": s(&dm, "id"),
                "type": "dm",
                "peer_id": s(&dm, "peer_id"),
                "name": format!("{} ↔ {}", s(&user, "nickname"), s(&bot, "name")),
                "avatar": Self::avatar(&s(&user, "id"), &s(&user, "nickname"), "user"),
                "bot_id": s(&dm, "bot_id"),
                "user_id": s(&user, "id"),
                "last_message": self.last_message(&s(&dm, "id"))?,
            }));
        }
        out.sort_by_key(|item| {
            std::cmp::Reverse(item["last_message"]["sent_at"].as_i64().unwrap_or(0))
        });
        Ok(out)
    }

    pub fn last_message(&self, conversation_id: &str) -> MqsResult<Value> {
        let Some(row) = self.db.one(
            "SELECT * FROM messages WHERE conversation_id=? ORDER BY seq DESC LIMIT 1",
            &p![conversation_id],
        )?
        else {
            return Ok(Value::Null);
        };
        let mut data = row.clone();
        let raw = json_col(&row, "raw_json", json!({}));
        data.remove("raw_json");
        data.insert("raw".into(), raw.clone());
        if let Some(at_bots) = raw.get("at_bots") {
            data.insert("at_bots".into(), at_bots.clone());
        }
        Ok(Value::Object(data))
    }

    pub fn ensure_conversation(&self, conversation_id: &str) -> MqsResult<()> {
        let (kind, peer) = conversation_id
            .split_once(':')
            .unwrap_or((conversation_id, ""));
        let (conv_type, peer_id, bot_id) = match kind {
            "group" => ("group", peer.to_string(), Value::Null),
            "dm" => {
                let (bot, user) = peer.split_once(':').unwrap_or(("", peer));
                ("dm", user.to_string(), Value::String(bot.to_string()))
            }
            _ => return Ok(()),
        };
        self.db.execute(
            "INSERT INTO conversations(id,type,peer_id,bot_id,last_seq) VALUES(?,?,?,?,0) \
             ON CONFLICT(id) DO NOTHING",
            &p![conversation_id, conv_type, &peer_id, bot_id],
        )?;
        Ok(())
    }

    pub fn resolve_conversation(
        &self,
        to: Option<&str>,
        conversation_id: Option<&str>,
    ) -> MqsResult<String> {
        let candidate = conversation_id
            .filter(|v| !v.is_empty())
            .or(to.filter(|v| !v.is_empty()))
            .ok_or_else(|| invalid_param("conversation_id or to is required"))?
            .to_string();
        if candidate.starts_with("group:") {
            if self
                .db
                .one("SELECT id FROM conversations WHERE id=?", &p![&candidate])?
                .is_none()
            {
                let gid = candidate.split_once(':').map(|(_, b)| b).unwrap_or("");
                if self
                    .db
                    .one("SELECT id FROM groups WHERE id=?", &p![gid])?
                    .is_none()
                {
                    return Err(invalid_param(format!(
                        "conversation not found: {candidate}"
                    )));
                }
                self.ensure_conversation(&candidate)?;
            }
            return Ok(candidate);
        }
        if candidate.starts_with("dm:") {
            if self
                .db
                .one("SELECT id FROM conversations WHERE id=?", &p![&candidate])?
                .is_none()
            {
                let rest = candidate.trim_start_matches("dm:");
                let (bot, user) = rest.split_once(':').unwrap_or(("", rest));
                let bot_exists = self.bot_by_app_id(bot)?.is_some();
                let user_exists = self
                    .db
                    .one("SELECT id FROM users WHERE id=?", &p![user])?
                    .is_some();
                if !bot_exists && !user_exists {
                    return Err(invalid_param(format!(
                        "conversation not found: {candidate}"
                    )));
                }
                self.ensure_conversation(&candidate)?;
            }
            return Ok(candidate);
        }
        if self
            .db
            .one("SELECT id FROM groups WHERE id=?", &p![&candidate])?
            .is_some()
        {
            let conv = format!("group:{candidate}");
            self.ensure_conversation(&conv)?;
            return Ok(conv);
        }
        if let Some((bot, user)) = candidate.split_once(':') {
            let bot_exists = self.bot_by_app_id(bot)?.is_some();
            let user_exists = self
                .db
                .one("SELECT id FROM users WHERE id=?", &p![user])?
                .is_some();
            if bot_exists && user_exists {
                let conv = format!("dm:{bot}:{user}");
                self.ensure_conversation(&conv)?;
                return Ok(conv);
            }
        }
        Err(invalid_param(format!(
            "cannot resolve conversation: {candidate}"
        )))
    }

    // ------------------------------------------------------------------ messages (read side)
    pub fn list_messages(
        &self,
        conversation_id: &str,
        limit: i64,
        before_seq: Option<i64>,
    ) -> MqsResult<Vec<Value>> {
        let mut sql = String::from("SELECT * FROM messages WHERE conversation_id=?");
        let mut params: Vec<V> = vec![V::from(conversation_id)];
        if let Some(seq) = before_seq {
            sql.push_str(" AND seq < ?");
            params.push(V::from(seq));
        }
        sql.push_str(" ORDER BY seq DESC LIMIT ?");
        params.push(V::from(limit.clamp(1, 200)));
        let rows = self.db.all(&sql, &params)?;
        let mut out = Vec::new();
        for row in rows.iter().rev() {
            out.push(self.message_out(row)?);
        }
        Ok(out)
    }

    pub fn message_out(&self, row: &Row) -> MqsResult<Value> {
        let mut data = row.clone();
        let raw = json_col(row, "raw_json", json!({}));
        data.remove("raw_json");
        let sender_type = s(row, "sender_type");
        let sender_id = s(row, "sender_id");
        let sender = if sender_type == "user" {
            self.db
                .one(
                    "SELECT id,nickname,avatar FROM users WHERE id=?",
                    &p![&sender_id],
                )?
                .map(|user| {
                    json!({
                        "id": s(&user, "id"),
                        "name": s(&user, "nickname"),
                        "avatar": Self::avatar(&s(&user, "id"), &s(&user, "nickname"), "user"),
                    })
                })
        } else {
            self.db
                .one("SELECT id,name,avatar FROM bots WHERE id=?", &p![&sender_id])?
                .map(|bot| {
                    json!({
                        "id": s(&bot, "id"),
                        "name": s(&bot, "name"),
                        "avatar": Self::avatar(&format!("bot_{}", s(&bot, "id")), &s(&bot, "name"), "bot"),
                    })
                })
        };
        data.insert("sender".into(), sender.unwrap_or(Value::Null));
        data.insert("raw".into(), raw.clone());
        if data.get("content").and_then(|v| v.as_str()).is_none() {
            data.insert("content".into(), Value::String(String::new()));
        }
        for key in [
            "markdown",
            "keyboard",
            "ark",
            "embed",
            "media",
            "message_reference",
            "mentions",
            "at_bots",
            "attachments",
        ] {
            if let Some(value) = raw.get(key) {
                data.insert(key.into(), value.clone());
            }
        }
        if raw.get("media").is_some() && !data.contains_key("attachments") {
            if let Some(file_info) = raw["media"].get("file_info").and_then(|v| v.as_str()) {
                let mhash = file_info.strip_prefix("mqs-file-").unwrap_or(file_info);
                if let Some(media) = self.media_by_hash(mhash)? {
                    data.insert(
                        "attachments".into(),
                        json!([{
                            "content_type": s(&media, "mime"),
                            "filename": &mhash[..mhash.len().min(12)],
                            "size": i(&media, "size"),
                            "url": format!("/media/{mhash}"),
                            "id": mhash,
                        }]),
                    );
                }
            }
        }
        Ok(Value::Object(data))
    }

    // ------------------------------------------------------------------ events
    pub fn list_events(
        &self,
        since_seq: i64,
        limit: i64,
        conversation_id: Option<&str>,
    ) -> MqsResult<Vec<Value>> {
        let mut sql = String::from("SELECT * FROM events WHERE seq > ?");
        let mut params: Vec<V> = vec![V::from(since_seq)];
        if let Some(conv) = conversation_id.filter(|v| !v.is_empty()) {
            sql.push_str(" AND conversation_id=?");
            params.push(V::from(conv));
        }
        sql.push_str(" ORDER BY seq LIMIT ?");
        params.push(V::from(limit.clamp(1, 500)));
        Ok(self
            .db
            .all(&sql, &params)?
            .iter()
            .map(|row| {
                let mut data = row.clone();
                let payload = json_col(row, "payload_json", json!({}));
                data.remove("payload_json");
                data.insert("payload".into(), payload);
                Value::Object(data)
            })
            .collect())
    }

    pub fn emit_group_robot_event(
        &self,
        bot_id: &str,
        group_id: &str,
        event_type: &str,
        operator_id: Option<&str>,
    ) -> MqsResult<Option<Value>> {
        let bot = self.db.one("SELECT * FROM bots WHERE id=?", &p![bot_id])?;
        let group = self
            .db
            .one("SELECT * FROM groups WHERE id=?", &p![group_id])?;
        let (Some(bot), Some(group)) = (bot, group) else {
            return Ok(None);
        };
        let op_id = operator_id
            .map(str::to_string)
            .or_else(|| s_opt(&group, "owner_id"))
            .unwrap_or_default();
        let group_openid = self.openid(bot_id, "group", group_id)?;
        let op_openid = if op_id.is_empty() {
            String::new()
        } else {
            self.openid(bot_id, "user", &op_id)?
        };
        let payload = json!({
            "timestamp": self.clock.iso(None),
            "group_openid": group_openid,
            "op_member_openid": op_openid,
        });
        let event_id = new_event_id();
        let conversation_id = format!("group:{group_id}");
        self.ensure_conversation(&conversation_id)?;
        let seq = self.db.execute(
            "INSERT INTO events(event_id,type,payload_json,conversation_id,target_bot_id,created_at) \
             VALUES(?,?,?,?,?,?)",
            &p![
                &event_id,
                event_type,
                payload.to_string(),
                &conversation_id,
                bot_id,
                self.clock.now_ms()
            ],
        )? as i64;
        self.spawn_deliver(
            bot_id,
            seq,
            event_type,
            &event_id,
            &payload,
            &self.bot_out(&bot),
            Some(&conversation_id),
        );
        Ok(Some(
            json!({"event_id": event_id, "seq": seq, "payload": payload}),
        ))
    }

    // ------------------------------------------------------------------ rules
    pub fn list_rules(&self) -> MqsResult<Vec<Value>> {
        Ok(self
            .db
            .all("SELECT * FROM rules ORDER BY created_at", &[])?
            .iter()
            .map(|row| {
                let mut data = row.clone();
                let condition = json_col(row, "condition_json", json!({}));
                let action = json_col(row, "action_json", json!({}));
                data.remove("condition_json");
                data.remove("action_json");
                data.insert("condition".into(), condition);
                data.insert("action".into(), action);
                data.insert("enabled".into(), Value::Bool(b(row, "enabled")));
                Value::Object(data)
            })
            .collect())
    }

    pub fn create_rule(&self, data: &Map<String, Value>) -> MqsResult<Value> {
        let rid = format!("rule_{}", &new_session_id()[..12]);
        self.db.execute(
            "INSERT INTO rules(id,name,event_type,condition_json,action_json,delay_ms,enabled,created_at) \
             VALUES(?,?,?,?,?,?,?,?)",
            &p![
                &rid,
                data.get("name").and_then(|v| v.as_str()).unwrap_or("rule"),
                data.get("event_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("group_at_message_create"),
                data.get("condition").cloned().unwrap_or(json!({})).to_string(),
                data.get("action").cloned().unwrap_or(json!({})).to_string(),
                data.get("delay_ms").and_then(|v| v.as_i64()).unwrap_or(0),
                data.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                self.clock.now_ms()
            ],
        )?;
        Ok(self
            .list_rules()?
            .into_iter()
            .find(|rule| rule["id"].as_str() == Some(rid.as_str()))
            .unwrap_or(Value::Null))
    }

    pub fn delete_rule(&self, rule_id: &str) -> MqsResult<()> {
        self.db
            .execute("DELETE FROM rules WHERE id=?", &p![rule_id])?;
        Ok(())
    }

    pub fn maybe_run_rules(
        &self,
        event_type: &str,
        payload: &Value,
        conversation_id: &str,
    ) -> MqsResult<()> {
        for rule in self.list_rules()? {
            if !rule["enabled"].as_bool().unwrap_or(false) {
                continue;
            }
            let rule_type = rule["event_type"].as_str().unwrap_or("").to_lowercase();
            if rule_type != event_type.to_lowercase() && rule_type != "message" {
                continue;
            }
            let condition = &rule["condition"];
            if let Some(conv) = condition.get("conversation_id").and_then(|v| v.as_str()) {
                if conv != conversation_id {
                    continue;
                }
            }
            if let Some(needle) = condition.get("content_contains").and_then(|v| v.as_str()) {
                let content = payload
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !content.contains(needle) {
                    continue;
                }
            }
            let delay = rule["delay_ms"].as_i64().unwrap_or(0);
            let world = self.arc();
            let rule = rule.clone();
            let conversation_id = conversation_id.to_string();
            let payload = payload.clone();
            tokio::spawn(async move {
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay.min(60_000) as u64))
                        .await;
                }
                let action = &rule["action"];
                if action
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("send_as_user")
                    != "send_as_user"
                {
                    return;
                }
                let Some(user_id) = action.get("user_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let content = payload
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let text = action
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .replace("{content}", content);
                if text.is_empty() {
                    return;
                }
                let at_bots: Option<Vec<String>> = action.get("at_bots").and_then(|v| {
                    v.as_array().map(|items| {
                        items
                            .iter()
                            .filter_map(|i| i.as_str().map(str::to_string))
                            .collect()
                    })
                });
                let _ = world
                    .send_as_user(
                        &conversation_id,
                        user_id,
                        &text,
                        0,
                        &[],
                        at_bots.as_deref(),
                        None,
                        None,
                        &[],
                    )
                    .await;
            });
        }
        Ok(())
    }

    // ------------------------------------------------------------------ media
    pub fn store_media(&self, data: &[u8], mime: &str) -> MqsResult<Row> {
        let mhash = media_hash(data);
        let path = self.paths.media().join(&mhash);
        if !path.exists() {
            std::fs::create_dir_all(self.paths.media())?;
            std::fs::write(&path, data)?;
        }
        self.db.execute(
            "INSERT INTO media(hash,mime,size,path,created_at) VALUES(?,?,?,?,?) ON CONFLICT(hash) DO NOTHING",
            &p![&mhash, mime, data.len(), path.to_string_lossy().to_string(), self.clock.now_ms()],
        )?;
        self.db
            .one("SELECT * FROM media WHERE hash=?", &p![&mhash])?
            .ok_or_else(|| internal("media row missing"))
    }

    pub fn media_by_hash(&self, mhash: &str) -> MqsResult<Option<Row>> {
        self.db.one("SELECT * FROM media WHERE hash=?", &p![mhash])
    }

    pub fn prepare_upload(
        &self,
        bot_id: &str,
        kind: &str,
        target_openid: &str,
        body: &Map<String, Value>,
    ) -> MqsResult<(String, i64, i64)> {
        let mapping = self.resolve_openid(bot_id, target_openid)?;
        if s(&mapping, "kind") != kind {
            return Err(invalid_param("openid kind mismatch"));
        }
        let file_type = body.get("file_type").and_then(|v| v.as_i64()).unwrap_or(1);
        if file_type == 4 {
            return Err(invalid_param("file_type 4 (generic file) is not supported"));
        }
        if !(1..=3).contains(&file_type) {
            return Err(invalid_param(format!("invalid file_type: {file_type}")));
        }
        let limit = match file_type {
            1 => 10 * 1024 * 1024,
            2 => 30 * 1024 * 1024,
            _ => 5 * 1024 * 1024,
        };
        let size = body.get("file_size").and_then(|v| v.as_i64()).unwrap_or(0);
        if size <= 0 || size > limit {
            return Err(invalid_param(format!("invalid file_size: {size}")));
        }
        let block_size = 4 * 1024 * 1024;
        let count = (size + block_size - 1) / block_size;
        let upload_id = format!("MQS_UP_{}", &new_session_id()[..20]);
        let file_name = body
            .get("file_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut mime = mime_for_name(&file_name);
        if file_type == 1 && !mime.starts_with("image/") {
            mime = "image/png".into();
        }
        self.uploads.lock().unwrap().insert(
            upload_id.clone(),
            UploadSession {
                bot_id: bot_id.to_string(),
                kind: kind.to_string(),
                target_openid: target_openid.to_string(),
                file_type,
                file_name,
                size,
                mime,
                block_size,
                parts: HashMap::new(),
                finished: HashSet::new(),
                srv_send_msg: body
                    .get("srv_send_msg")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
            },
        );
        Ok((upload_id, block_size, count))
    }

    pub fn put_upload_part(&self, upload_id: &str, index: usize, data: Vec<u8>) -> MqsResult<()> {
        let mut uploads = self.uploads.lock().unwrap();
        let session = uploads
            .get_mut(upload_id)
            .ok_or_else(|| invalid_param("unknown upload_id"))?;
        let expected = (session.size + session.block_size - 1) / session.block_size;
        if index as i64 >= expected.max(1) {
            return Err(invalid_param("invalid part index"));
        }
        session.parts.insert(index, data);
        Ok(())
    }

    pub fn finish_upload_part(
        &self,
        upload_id: &str,
        index: usize,
        block_size: i64,
        md5: &str,
    ) -> MqsResult<()> {
        use md5::{Digest, Md5};
        let mut uploads = self.uploads.lock().unwrap();
        let session = uploads
            .get_mut(upload_id)
            .ok_or_else(|| invalid_param("unknown upload_id"))?;
        let data = session
            .parts
            .get(&index)
            .ok_or_else(|| invalid_param("part has not been uploaded"))?
            .clone();
        if !md5.is_empty() {
            let actual = Md5::digest(&data)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            if actual != md5 {
                return Err(invalid_param("part md5 mismatch"));
            }
        }
        if block_size != 0 && block_size != data.len() as i64 {
            return Err(invalid_param("part block_size mismatch"));
        }
        session.finished.insert(index);
        Ok(())
    }

    pub async fn finalize_upload(
        &self,
        bot_id: &str,
        kind: &str,
        target_openid: &str,
        upload_id: &str,
        srv_send_msg: Option<bool>,
    ) -> MqsResult<Value> {
        let session = {
            let mut uploads = self.uploads.lock().unwrap();
            let session = uploads
                .get(upload_id)
                .ok_or_else(|| invalid_param("unknown upload_id"))?
                .clone();
            if session.bot_id != bot_id || session.target_openid != target_openid {
                return Err(invalid_param("upload_id does not belong to this target"));
            }
            uploads.remove(upload_id);
            session
        };
        let expected =
            ((session.size + session.block_size - 1) / session.block_size).max(1) as usize;
        if session.finished.len() != expected
            || (0..expected).any(|index| !session.finished.contains(&index))
        {
            return Err(invalid_param("upload is incomplete"));
        }
        let mut data = Vec::with_capacity(session.size as usize);
        for index in 0..expected {
            data.extend_from_slice(&session.parts[&index]);
        }
        if data.len() as i64 != session.size {
            return Err(invalid_param("assembled size mismatch"));
        }
        let media = self.store_media(&data, &session.mime)?;
        let mhash = s(&media, "hash");
        let mut result = json!({
            "file_uuid": mhash,
            "file_info": format!("mqs-file-{mhash}"),
            "ttl": 0,
        });
        let send = srv_send_msg.unwrap_or(session.srv_send_msg);
        if send {
            let sent = self
                .send_as_bot(
                    bot_id,
                    kind,
                    target_openid,
                    &json!({"msg_type": 7, "content": "", "media": {"file_info": result["file_info"]}}),
                )
                .await?;
            result["message"] = sent;
        }
        Ok(result)
    }

    // ------------------------------------------------------------------ debug frames
    pub fn log_frame(
        &self,
        direction: &str,
        kind: &str,
        summary: &str,
        data: &Value,
        bot_id: Option<&str>,
        session_id: Option<&str>,
    ) {
        let ts = self.clock.now_ms();
        let summary = summary.chars().take(300).collect::<String>();
        let payload = data.to_string();
        let payload = if payload.len() > 20000 {
            &payload[..20000]
        } else {
            &payload
        };
        let _ = self.db.execute(
            "INSERT INTO frames(ts,direction,kind,bot_id,session_id,summary,data_json) VALUES(?,?,?,?,?,?,?)",
            &p![
                ts,
                direction,
                kind,
                bot_id,
                session_id,
                &summary,
                payload
            ],
        );
        let count = self.frame_writes.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_multiple_of(50) {
            let _ = self.db.prune_frames(2000);
        }
        self.hub.publish(&json!({
            "kind": "frame",
            "frame": {
                "ts": ts,
                "direction": direction,
                "kind": kind,
                "bot_id": bot_id,
                "session_id": session_id,
                "summary": summary,
                "data": data,
            }
        }));
    }

    pub fn list_frames(
        &self,
        since_id: i64,
        limit: i64,
        bot_id: Option<&str>,
        kind: Option<&str>,
        q: Option<&str>,
    ) -> MqsResult<Vec<Value>> {
        let mut sql = String::from("SELECT * FROM frames WHERE id > ?");
        let mut params: Vec<V> = vec![V::from(since_id)];
        if let Some(bot) = bot_id.filter(|v| !v.is_empty()) {
            sql.push_str(" AND bot_id=?");
            params.push(V::from(bot));
        }
        if let Some(kind) = kind.filter(|v| !v.is_empty()) {
            sql.push_str(" AND kind=?");
            params.push(V::from(kind));
        }
        sql.push_str(" ORDER BY id LIMIT ?");
        params.push(V::from(limit.clamp(1, 1000)));
        let mut out: Vec<Value> = self
            .db
            .all(&sql, &params)?
            .iter()
            .map(|row| {
                let mut data = row.clone();
                let payload = json_col(row, "data_json", json!({}));
                data.remove("data_json");
                data.insert("data".into(), payload);
                Value::Object(data)
            })
            .collect();
        if let Some(needle) = q.filter(|v| !v.is_empty()) {
            let needle = needle.to_lowercase();
            out.retain(|frame| frame.to_string().to_lowercase().contains(&needle));
        }
        Ok(out)
    }

    // ------------------------------------------------------------------ status
    pub fn counts(&self) -> MqsResult<Map<String, Value>> {
        let mut out = Map::new();
        for table in [
            "users",
            "bots",
            "groups",
            "messages",
            "events",
            "gateway_sessions",
            "frames",
            "media",
            "rules",
        ] {
            let count = self
                .db
                .one(&format!("SELECT COUNT(*) AS c FROM {table}"), &[])?
                .map(|row| i(&row, "c"))
                .unwrap_or(0);
            out.insert(table.to_string(), Value::from(count));
        }
        Ok(out)
    }

    pub fn status(&self) -> MqsResult<Value> {
        let counts = self.counts()?;
        let max_event_seq = self
            .db
            .one("SELECT COALESCE(MAX(seq),0) AS m FROM events", &[])?
            .map(|row| i(&row, "m"))
            .unwrap_or(0);
        let max_message_id = self
            .db
            .one("SELECT COALESCE(MAX(id),0) AS m FROM messages", &[])?
            .map(|row| i(&row, "m"))
            .unwrap_or(0);
        let max_frame_id = self
            .db
            .one("SELECT COALESCE(MAX(id),0) AS m FROM frames", &[])?
            .map(|row| i(&row, "m"))
            .unwrap_or(0);
        let events_last_minute = self
            .db
            .one(
                "SELECT COUNT(*) AS c FROM events WHERE created_at > ?",
                &p![self.clock.now_ms() - 60_000],
            )?
            .map(|row| i(&row, "c"))
            .unwrap_or(0);
        let rate_stats = self.rate.lock().unwrap().stats();
        let gateway_stats = self
            .gateway
            .get()
            .map(|gateway| gateway.stats())
            .unwrap_or_else(|| json!({"connections": 0, "sessions": 0, "bots": [], "shards": []}));
        Ok(json!({
            "profile": self.profile,
            "boot_id": self.boot_id,
            "started_at": self.started_at,
            "uptime_ms": (chrono::Utc::now().timestamp_millis() - self.started_at).max(0),
            "clock": self.clock.snapshot(),
            "counts": counts,
            "max_event_seq": max_event_seq,
            "max_message_id": max_message_id,
            "max_frame_id": max_frame_id,
            "events_last_minute": events_last_minute,
            "rate": rate_stats,
            "gateway": gateway_stats,
            "db_size": self.db.size_bytes(),
            "settings": Value::Object(self.settings.read().unwrap().clone()),
            "coverage": crate::contract::summary(),
            "hub_subscribers": self.hub.subscriber_count(),
        }))
    }

    pub fn iso(&self, ms: i64) -> String {
        self.clock.iso(Some(ms))
    }
}

pub fn mime_for_name(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "amr" => "audio/amr",
        "silk" => "audio/silk",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Binary sniffing for uploads without a content type (mirrors the Python helper).
pub fn sniff_mime(data: &[u8]) -> String {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png".into()
    } else if data.starts_with(b"\xff\xd8\xff") {
        "image/jpeg".into()
    } else if data.starts_with(b"GIF8") {
        "image/gif".into()
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp".into()
    } else if data.len() > 8 && &data[4..8] == b"ftyp" {
        "video/mp4".into()
    } else if data.starts_with(b"#!AMR") {
        "audio/amr".into()
    } else if data.starts_with(b"\x02#!SILK") {
        "audio/silk".into()
    } else {
        "application/octet-stream".into()
    }
}
