//! Community surface: group management (invite-only APIs) on top of the world.

use serde_json::{json, Value};

use crate::db::{b, i, json_col, s, Row};
use crate::error::{invalid_param, no_permission, MqsResult};
use crate::ids::{new_event_id, openid_for, union_openid_for};
use crate::p;
use crate::world::World;

impl World {
    fn group_bot_row(&self, group_id: &str, bot_id: &str) -> MqsResult<Row> {
        self.db
            .one(
                "SELECT gb.*, b.app_id AS bot_app_id, b.name AS bot_name FROM group_bots gb \
                 JOIN bots b ON b.id=gb.bot_id WHERE gb.group_id=? AND gb.bot_id=?",
                &p![group_id, bot_id],
            )?
            .ok_or_else(|| no_permission("bot is not a member of this group"))
    }

    pub fn group_info(&self, group_id: &str, bot_id: &str) -> MqsResult<Value> {
        let group = self.get_group(group_id)?;
        self.group_bot_row(group_id, bot_id)?;
        let count = self
            .db
            .one(
                "SELECT COUNT(*) AS c FROM group_members WHERE group_id=?",
                &p![group_id],
            )?
            .map(|row| i(&row, "c"))
            .unwrap_or(0);
        let row = self
            .db
            .one("SELECT * FROM groups WHERE id=?", &p![group_id])?
            .unwrap();
        Ok(json!({
            "group_openid": self.openid(bot_id, "group", group_id)?,
            "group_name": group["name"],
            "group_finger_memo": s(&row, "finger_memo"),
            "group_class_text": s(&row, "class_text"),
            "group_tags": json_col(&row, "tags", json!([])),
            "group_member_num": count,
        }))
    }

    pub fn group_bot_state(&self, group_id: &str, bot_id: &str) -> MqsResult<Value> {
        let row = self.group_bot_row(group_id, bot_id)?;
        Ok(json!({
            "member_openid": openid_for(&s(&row, "bot_app_id"), "bot", bot_id),
            "joined_at": self.iso(i(&row, "joined_at")),
            "allow_proactive_msg": b(&row, "allow_proactive_msg"),
            "recv_msg_setting": if s(&row, "recv_msg_setting").is_empty() { "all".to_string() } else { s(&row, "recv_msg_setting") },
            "member_role": "member",
        }))
    }

    pub fn update_group_bot_settings(
        &self,
        group_id: &str,
        bot_id: &str,
        patch: &serde_json::Map<String, Value>,
    ) -> MqsResult<Value> {
        self.group_bot_row(group_id, bot_id)?;
        let mut fields: Vec<(String, Value)> = Vec::new();
        if let Some(value) = patch.get("allow_proactive_msg").and_then(|v| v.as_bool()) {
            fields.push(("allow_proactive_msg".into(), Value::from(value)));
        }
        if let Some(value) = patch.get("recv_msg_setting").and_then(|v| v.as_str()) {
            if !["all", "only_mention", "mention_and_context"].contains(&value) {
                return Err(invalid_param(
                    "recv_msg_setting must be all/only_mention/mention_and_context",
                ));
            }
            fields.push(("recv_msg_setting".into(), Value::String(value.to_string())));
        }
        if !fields.is_empty() {
            let cols: Vec<String> = fields.iter().map(|(key, _)| format!("{key}=?")).collect();
            let mut params: Vec<crate::db::V> = fields
                .iter()
                .map(|(_, value)| crate::db::V::from(value))
                .collect();
            params.push(crate::db::V::from(group_id));
            params.push(crate::db::V::from(bot_id));
            self.db.execute(
                &format!(
                    "UPDATE group_bots SET {} WHERE group_id=? AND bot_id=?",
                    cols.join(", ")
                ),
                &params,
            )?;
        }
        self.group_bot_state(group_id, bot_id)
    }

    pub fn group_mute_settings(&self, group_id: &str, bot_id: &str) -> MqsResult<Value> {
        self.group_bot_row(group_id, bot_id)?;
        let rows = self.db.all(
            "SELECT gm.user_id, gm.mute_until, u.nickname FROM group_members gm \
             JOIN users u ON u.id=gm.user_id WHERE gm.group_id=? AND gm.mute_until>? ORDER BY gm.mute_until",
            &p![group_id, self.clock.now_ms()],
        )?;
        let mut members = Vec::new();
        for row in rows {
            members.push(json!({
                "member_openid": self.openid(bot_id, "user", &s(&row, "user_id"))?,
                "mute_expire_at": self.iso(i(&row, "mute_until")),
                "username": s(&row, "nickname"),
                "union_openid": union_openid_for(&s(&row, "user_id")),
            }));
        }
        Ok(json!({
            "global_rule": {"mode": "none", "schedule_rules": [], "recurring_rules": []},
            "members": members,
        }))
    }

    pub fn set_group_mute(
        &self,
        group_id: &str,
        bot_id: &str,
        members: &[Value],
    ) -> MqsResult<Value> {
        self.group_bot_row(group_id, bot_id)?;
        if members.is_empty() {
            return Ok(json!({}));
        }
        if members.len() > 20 {
            return Err(invalid_param("at most 20 members per request"));
        }
        let now = self.clock.now_ms();
        let max_expire = now + 30 * 24 * 60 * 60 * 1000;
        for item in members {
            let op = item.get("op").and_then(|v| v.as_str()).unwrap_or("");
            if !["add", "update", "del"].contains(&op) {
                return Err(invalid_param("op must be add/update/del"));
            }
            let member_openid = item
                .get("member_openid")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mapping = self.resolve_openid(bot_id, member_openid)?;
            if s(&mapping, "kind") != "user" {
                return Err(invalid_param("member_openid does not refer to a user"));
            }
            let user_id = s(&mapping, "entity_id");
            let member = self.db.one(
                "SELECT role FROM group_members WHERE group_id=? AND user_id=?",
                &p![group_id, &user_id],
            )?;
            let member = member.ok_or_else(|| invalid_param("member is not in this group"))?;
            let role = s(&member, "role");
            if role == "owner" || role == "admin" {
                return Err(no_permission("cannot mute the group owner or an admin"));
            }
            let expire = if op == "del" {
                0
            } else {
                let text = item
                    .get("mute_expire_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if text.is_empty() {
                    return Err(invalid_param("mute_expire_at is required for add/update"));
                }
                crate::clock::parse_ts(text)
                    .map_err(invalid_param)?
                    .min(max_expire)
            };
            self.db.execute(
                "UPDATE group_members SET mute_until=? WHERE group_id=? AND user_id=?",
                &p![expire, group_id, &user_id],
            )?;
        }
        Ok(json!({}))
    }

    pub fn group_blacklist_page(
        &self,
        group_id: &str,
        bot_id: &str,
        cursor: &str,
        limit: i64,
    ) -> MqsResult<Value> {
        self.group_bot_row(group_id, bot_id)?;
        let offset: i64 = cursor.parse().unwrap_or(0).max(0);
        let limit = limit.clamp(1, 100);
        let rows = self.db.all(
            "SELECT gb.user_id, gb.banned_at, u.nickname FROM group_blacklist gb \
             JOIN users u ON u.id=gb.user_id WHERE gb.group_id=? ORDER BY gb.banned_at, gb.user_id LIMIT ? OFFSET ?",
            &p![group_id, limit + 1, offset],
        )?;
        let page: Vec<Value> = rows
            .iter()
            .take(limit as usize)
            .map(|row| {
                Ok(json!({
                    "union_openid": union_openid_for(&s(row, "user_id")),
                    "member_openid": self.openid(bot_id, "user", &s(row, "user_id"))?,
                    "username": s(row, "nickname"),
                    "banned_at": self.iso(i(row, "banned_at")),
                    "bot": false,
                }))
            })
            .collect::<MqsResult<Vec<Value>>>()?;
        Ok(json!({
            "users": page,
            "next_cursor": if rows.len() as i64 > limit { (offset + limit).to_string() } else { String::new() },
        }))
    }

    pub fn update_group_blacklist(
        &self,
        group_id: &str,
        bot_id: &str,
        op: &str,
        member_openids: &[String],
    ) -> MqsResult<Value> {
        self.group_bot_row(group_id, bot_id)?;
        if op != "add" && op != "del" {
            return Err(invalid_param("op must be add/del"));
        }
        if member_openids.len() > 20 {
            return Err(invalid_param("at most 20 members per request"));
        }
        let mut fail = Vec::new();
        let now = self.clock.now_ms();
        for member_openid in member_openids {
            let Ok(mapping) = self.resolve_openid(bot_id, member_openid) else {
                fail.push(Value::String(member_openid.clone()));
                continue;
            };
            if s(&mapping, "kind") != "user" {
                fail.push(Value::String(member_openid.clone()));
                continue;
            }
            let user_id = s(&mapping, "entity_id");
            if op == "add" {
                if self
                    .db
                    .one(
                        "SELECT 1 FROM group_members WHERE group_id=? AND user_id=?",
                        &p![group_id, &user_id],
                    )?
                    .is_some()
                {
                    fail.push(Value::String(member_openid.clone()));
                    continue;
                }
                self.db.execute(
                    "INSERT INTO group_blacklist(group_id,user_id,banned_at) VALUES(?,?,?) \
                     ON CONFLICT(group_id,user_id) DO NOTHING",
                    &p![group_id, &user_id, now],
                )?;
            } else {
                self.db.execute(
                    "DELETE FROM group_blacklist WHERE group_id=? AND user_id=?",
                    &p![group_id, &user_id],
                )?;
            }
        }
        Ok(json!({"fail_openids": fail}))
    }

    pub fn batch_remove_group_members(
        &self,
        group_id: &str,
        bot_id: &str,
        member_openids: &[String],
        add_blacklist: bool,
    ) -> MqsResult<Value> {
        self.group_bot_row(group_id, bot_id)?;
        if member_openids.len() > 20 {
            return Err(invalid_param("at most 20 members per request"));
        }
        let mut failed_blacklist = Vec::new();
        let now = self.clock.now_ms();
        for member_openid in member_openids {
            let Ok(mapping) = self.resolve_openid(bot_id, member_openid) else {
                continue;
            };
            if s(&mapping, "kind") != "user" {
                continue;
            }
            let user_id = s(&mapping, "entity_id");
            let member = self.db.one(
                "SELECT role FROM group_members WHERE group_id=? AND user_id=?",
                &p![group_id, &user_id],
            )?;
            let Some(member) = member else { continue };
            let role = s(&member, "role");
            if role == "owner" || role == "admin" {
                continue;
            }
            self.db.execute(
                "DELETE FROM group_members WHERE group_id=? AND user_id=?",
                &p![group_id, &user_id],
            )?;
            if add_blacklist {
                self.db.execute(
                    "INSERT INTO group_blacklist(group_id,user_id,banned_at) VALUES(?,?,?) \
                     ON CONFLICT(group_id,user_id) DO NOTHING",
                    &p![group_id, &user_id, now],
                )?;
            } else {
                failed_blacklist.push(Value::String(member_openid.clone()));
            }
        }
        Ok(json!({
            "remove_members_result": "success",
            "add_to_member_blacklist_fail_openids": if add_blacklist { failed_blacklist } else { vec![] },
        }))
    }

    fn join_request_out(&self, row: &Row, bot_id: &str) -> MqsResult<Value> {
        Ok(json!({
            "join_request_id": s(row, "id"),
            "risk_tips": s(row, "risk_tips"),
            "union_openid": union_openid_for(&s(row, "user_id")),
            "member_openid": self.openid(bot_id, "user", &s(row, "user_id"))?,
            "username": s(row, "nickname"),
            "apply_at": self.iso(i(row, "apply_at")),
            "apply_source": s(row, "source"),
            "invited_by": s(row, "invited_by"),
            "bot": false,
            "verify_info": {
                "method": "verify_message",
                "verify_message": s(row, "verify_message"),
                "review_qa_list": [],
            },
        }))
    }

    pub fn join_requests_page(
        &self,
        group_id: &str,
        bot_id: &str,
        cursor: &str,
        limit: i64,
    ) -> MqsResult<Value> {
        self.group_bot_row(group_id, bot_id)?;
        let offset: i64 = cursor.parse().unwrap_or(0).max(0);
        let limit = limit.clamp(1, 50);
        let rows = self.db.all(
            "SELECT jr.*, u.nickname FROM group_join_requests jr JOIN users u ON u.id=jr.user_id \
             WHERE jr.group_id=? AND jr.status='pending' ORDER BY jr.apply_at, jr.id LIMIT ? OFFSET ?",
            &p![group_id, limit + 1, offset],
        )?;
        let page: Vec<Value> = rows
            .iter()
            .take(limit as usize)
            .map(|row| self.join_request_out(row, bot_id))
            .collect::<MqsResult<Vec<Value>>>()?;
        Ok(json!({
            "list": page,
            "next_cursor": if rows.len() as i64 > limit { (offset + limit).to_string() } else { String::new() },
        }))
    }

    pub fn create_join_request(
        &self,
        group_id: &str,
        user_id: &str,
        verify_message: &str,
        source: &str,
    ) -> MqsResult<Value> {
        self.get_group(group_id)?;
        self.get_user(user_id)?;
        if self
            .db
            .one(
                "SELECT 1 FROM group_members WHERE group_id=? AND user_id=?",
                &p![group_id, user_id],
            )?
            .is_some()
        {
            return Err(invalid_param("user is already a member of this group"));
        }
        if self
            .db
            .one(
                "SELECT 1 FROM group_blacklist WHERE group_id=? AND user_id=?",
                &p![group_id, user_id],
            )?
            .is_some()
        {
            return Err(no_permission("user is blacklisted in this group"));
        }
        if let Some(existing) = self.db.one(
            "SELECT * FROM group_join_requests WHERE group_id=? AND user_id=? AND status='pending'",
            &p![group_id, user_id],
        )? {
            return Ok(Value::Object(existing));
        }
        let request_id = new_event_id();
        self.db.execute(
            "INSERT INTO group_join_requests(id,group_id,user_id,apply_at,source,verify_message) \
             VALUES(?,?,?,?,?,?)",
            &p![
                &request_id,
                group_id,
                user_id,
                self.clock.now_ms(),
                source,
                verify_message
            ],
        )?;
        Ok(Value::Object(
            self.db
                .one(
                    "SELECT * FROM group_join_requests WHERE id=?",
                    &p![&request_id],
                )?
                .unwrap_or_default(),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn approve_join_request(
        &self,
        group_id: &str,
        bot_id: &str,
        member_openid: &str,
        op: &str,
        join_request_id: &str,
        reject_reason: &str,
        add_to_blacklist: bool,
    ) -> MqsResult<Value> {
        self.group_bot_row(group_id, bot_id)?;
        if op != "approve" && op != "decline" {
            return Err(invalid_param("op must be approve/decline"));
        }
        let mapping = self.resolve_openid(bot_id, member_openid)?;
        if s(&mapping, "kind") != "user" {
            return Err(invalid_param("member_openid does not refer to a user"));
        }
        let user_id = s(&mapping, "entity_id");
        let row = if !join_request_id.is_empty() {
            self.db.one(
                "SELECT * FROM group_join_requests WHERE id=? AND group_id=? AND status='pending'",
                &p![join_request_id, group_id],
            )?
        } else {
            self.db.one(
                "SELECT * FROM group_join_requests WHERE group_id=? AND user_id=? AND status='pending'",
                &p![group_id, &user_id],
            )?
        };
        let row = row.ok_or_else(|| invalid_param("pending join request not found"))?;
        let now = self.clock.now_ms();
        if op == "approve" {
            self.db.execute(
                "INSERT INTO group_members(group_id,user_id,role,joined_at) VALUES(?,?,?,?) \
                 ON CONFLICT(group_id,user_id) DO NOTHING",
                &p![group_id, &user_id, "member", now],
            )?;
            self.db.execute(
                "UPDATE group_join_requests SET status='approved', decided_at=? WHERE id=?",
                &p![now, s(&row, "id")],
            )?;
        } else {
            self.db.execute(
                "UPDATE group_join_requests SET status='declined', decided_at=?, reject_reason=? WHERE id=?",
                &p![now, reject_reason, s(&row, "id")],
            )?;
            if add_to_blacklist
                && self
                    .db
                    .one(
                        "SELECT 1 FROM group_members WHERE group_id=? AND user_id=?",
                        &p![group_id, &user_id],
                    )?
                    .is_none()
            {
                self.db.execute(
                    "INSERT INTO group_blacklist(group_id,user_id,banned_at) VALUES(?,?,?) \
                     ON CONFLICT(group_id,user_id) DO NOTHING",
                    &p![group_id, &user_id, now],
                )?;
            }
        }
        Ok(json!({}))
    }

    pub fn group_members_page(
        &self,
        group_id: &str,
        bot_id: &str,
        cursor: &str,
        limit: i64,
    ) -> MqsResult<Value> {
        let offset: i64 = cursor.parse().unwrap_or(0).max(0);
        let mut members: Vec<(i64, Value)> = Vec::new();
        for row in self.db.all(
            "SELECT gm.user_id, gm.role, gm.joined_at, u.nickname FROM group_members gm \
             JOIN users u ON u.id=gm.user_id WHERE gm.group_id=? ORDER BY gm.joined_at, gm.user_id",
            &p![group_id],
        )? {
            members.push((
                i(&row, "joined_at"),
                json!({
                    "member_openid": self.openid(bot_id, "user", &s(&row, "user_id"))?,
                    "username": s(&row, "nickname"),
                    "member_role": s(&row, "role"),
                    "bot": false,
                    "joined_at": self.iso(i(&row, "joined_at")),
                    "union_openid": union_openid_for(&s(&row, "user_id")),
                }),
            ));
        }
        for row in self.db.all(
            "SELECT gb.bot_id, gb.joined_at, b.name AS name, b.app_id AS app_id FROM group_bots gb \
             JOIN bots b ON b.id=gb.bot_id WHERE gb.group_id=? ORDER BY gb.joined_at, gb.bot_id",
            &p![group_id],
        )? {
            members.push((
                i(&row, "joined_at"),
                json!({
                    "member_openid": openid_for(&s(&row, "app_id"), "bot", &s(&row, "bot_id")),
                    "username": s(&row, "name"),
                    "member_role": "member",
                    "bot": true,
                    "joined_at": self.iso(i(&row, "joined_at")),
                    "union_openid": union_openid_for(&s(&row, "bot_id")),
                }),
            ));
        }
        members.sort_by_key(|(joined, _)| *joined);
        let total = members.len() as i64;
        let page: Vec<Value> = members
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(_, value)| value)
            .collect();
        Ok(json!({
            "members": page,
            "next_cursor": if offset + limit < total { (offset + limit).to_string() } else { String::new() },
        }))
    }

    pub fn group_member_detail(
        &self,
        group_id: &str,
        bot_id: &str,
        member_openid: &str,
    ) -> MqsResult<Value> {
        if let Some(mapping) = self.db.one(
            "SELECT * FROM openid_map WHERE bot_id=? AND openid=?",
            &p![bot_id, member_openid],
        )? {
            if s(&mapping, "kind") == "user" {
                let row = self.db.one(
                    "SELECT gm.*, u.nickname FROM group_members gm JOIN users u ON u.id=gm.user_id \
                     WHERE gm.group_id=? AND gm.user_id=?",
                    &p![group_id, s(&mapping, "entity_id")],
                )?;
                let row = row.ok_or_else(|| invalid_param("member is not in this group"))?;
                return Ok(json!({
                    "member_openid": member_openid,
                    "username": s(&row, "nickname"),
                    "member_role": s(&row, "role"),
                    "bot": false,
                    "joined_at": self.iso(i(&row, "joined_at")),
                    "union_openid": union_openid_for(&s(&row, "user_id")),
                }));
            }
        }
        let bot = self.bot_by_id(bot_id)?;
        if member_openid == openid_for(&s(&bot, "app_id"), "bot", bot_id) {
            return Ok(json!({
                "member_openid": member_openid,
                "username": s(&bot, "name"),
                "member_role": "member",
                "bot": true,
                "joined_at": self.iso(i(&bot, "created_at")),
                "union_openid": union_openid_for(bot_id),
            }));
        }
        Err(invalid_param("member openid not found"))
    }
}
