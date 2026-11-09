//! Deterministic identifiers, seeded fake data and local avatars.

use sha2::{Digest, Sha256};

const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

const ADJECTIVES: &[&str] = &[
    "星", "月", "风", "云", "雨", "雪", "霜", "雾", "山", "海", "林", "森", "花", "叶", "光", "夕",
    "晨", "夏", "秋", "冬", "竹", "兰", "梅", "松", "鹤", "鹿", "鲸", "猫", "兔", "熊", "青", "白",
    "玄", "黛", "浅", "深", "远", "近", "微", "小", "大", "长", "短", "轻", "暖", "冷", "晴", "阴",
    "朝", "暮",
];
const NOUNS: &[&str] = &[
    "野", "见", "禾", "满", "川", "岛", "原", "音", "玲", "汐", "柠", "糖", "团", "子", "酱", "喵",
    "眠", "梦", "歌", "语", "书", "画", "舟", "桥", "灯", "窗", "巷", "井", "泉", "溪", "柚", "栗",
    "桃", "杏", "莓", "橘", "葵", "芽", "苗", "遥", "归", "行", "止", "问", "答", "呼", "吸",
];
const CITIES: &[&str] = &[
    "上海", "北京", "广州", "深圳", "杭州", "成都", "武汉", "南京", "西安", "重庆", "苏州", "长沙",
];
const BEHAVIORS: &[&str] = &["silent", "normal", "chatty", "spammer"];
const LOREM: &[&str] = &[
    "在的，有什么可以帮你？",
    "收到，我看看。",
    "这个功能我测过了，没问题。",
    "稍等，我先查下日志。",
    "哈哈，确实。",
    "好耶。",
    "我先潜水了，有事 @ 我。",
    "今天群里好热闹。",
    "这个接口的限流有点紧。",
    "刚上线，来打个招呼。",
];

/// Tiny xorshift64* PRNG: deterministic, dependency-free, good enough for fake data.
#[derive(Debug, Clone)]
pub struct Rng64 {
    state: u64,
}

impl Rng64 {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed
                .wrapping_mul(2_862_933_555_777_941_757)
                .wrapping_add(3_037_000_493)
                | 1,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    pub fn range(&mut self, low: i64, high: i64) -> i64 {
        if high <= low {
            return low;
        }
        low + self.below((high - low + 1) as u64) as i64
    }

    pub fn choice<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }

    pub fn weighted(&mut self, weights: &[u64]) -> usize {
        let total: u64 = weights.iter().sum();
        let mut pick = self.below(total);
        for (index, weight) in weights.iter().enumerate() {
            if pick < *weight {
                return index;
            }
            pick -= *weight;
        }
        weights.len() - 1
    }
}

pub fn short_token(n: usize) -> String {
    // OsRng-free: ids are fake by definition, but they must not collide.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let mut rng = Rng64::new(nanos ^ (std::process::id() as u64) << 17);
    let mut rng2 = Rng64::new(nanos.rotate_left(23) ^ 0x9E37_79B9_7F4A_7C15);
    (0..n)
        .map(|i| {
            let v = if i % 2 == 0 {
                rng.next_u64()
            } else {
                rng2.next_u64()
            };
            ALPHABET[(v % ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

pub fn new_msg_id(prefix: &str) -> String {
    format!("{prefix}_{}", short_token(24))
}

pub fn new_event_id() -> String {
    format!("MQS_EVT_{}", short_token(20))
}

pub fn new_session_id() -> String {
    short_token(32)
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// QQ openids are scoped per bot; derive them deterministically.
pub fn openid_for(bot_app_id: &str, kind: &str, internal_id: &str) -> String {
    sha256_hex(&format!("{bot_app_id}:{kind}:{internal_id}"))[..32].to_uppercase()
}

pub fn union_openid_for(internal_id: &str) -> String {
    sha256_hex(&format!("union:{internal_id}"))[..32].to_uppercase()
}

pub fn media_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Deterministic local avatar: background colour encodes the entity kind.
pub fn avatar_svg(seed: &str, label: &str, kind: &str) -> String {
    let digest = sha256_hex(seed);
    let h = u64::from_str_radix(&digest[..8], 16).unwrap_or(0);
    let (base, spread) = match kind {
        "bot" => (262_i64, 12_i64),
        "npc" => (150, 14),
        "group" => (28, 16),
        _ => (194, 18),
    };
    let hue = (base + (h as i64 % spread) - spread / 2).rem_euclid(360);
    let hue2 = (hue + 28) % 360;
    let bg1 = format!("hsl({hue},72%,62%)");
    let bg2 = format!("hsl({hue2},68%,46%)");
    let text = if label.trim().is_empty() {
        seed
    } else {
        label.trim()
    };
    let mut initial: String = text.chars().take(1).collect();
    if !initial
        .chars()
        .next()
        .map(|c| c.is_alphanumeric())
        .unwrap_or(false)
    {
        initial = text.chars().take(2).collect();
    }
    if initial.is_empty() {
        initial = "?".into();
    }
    let escaped = initial
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"96\" height=\"96\" viewBox=\"0 0 96 96\">\
<defs><linearGradient id=\"g\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\">\
<stop offset=\"0\" stop-color=\"{bg1}\"/><stop offset=\"1\" stop-color=\"{bg2}\"/></linearGradient></defs>\
<rect width=\"96\" height=\"96\" rx=\"24\" fill=\"url(#g)\"/>\
<text x=\"48\" y=\"62\" font-size=\"42\" font-family=\"PingFang SC, Microsoft YaHei, sans-serif\" \
font-weight=\"700\" text-anchor=\"middle\" fill=\"#fff\">{escaped}</text></svg>"
    )
}

/// Seeded generator: same seed + same call order => same world.
#[derive(Debug, Clone)]
pub struct FakeData {
    pub seed: i64,
    rng: Rng64,
    used_names: std::collections::HashSet<String>,
}

impl FakeData {
    pub fn new(seed: i64) -> Self {
        Self {
            seed,
            rng: Rng64::new(seed as u64),
            used_names: Default::default(),
        }
    }

    pub fn reseed(&mut self, seed: i64) {
        self.seed = seed;
        self.rng = Rng64::new(seed as u64);
        self.used_names.clear();
    }

    pub fn nickname(&mut self) -> String {
        for _ in 0..200 {
            let name = format!("{}{}", self.rng.choice(ADJECTIVES), self.rng.choice(NOUNS));
            if self.used_names.insert(name.clone()) {
                return name;
            }
        }
        let name = format!(
            "{}{}{}",
            self.rng.choice(ADJECTIVES),
            self.rng.choice(NOUNS),
            self.rng.range(0, 999)
        );
        self.used_names.insert(name.clone());
        name
    }

    pub fn city(&mut self) -> String {
        self.rng.choice(CITIES).to_string()
    }

    pub fn gender(&mut self) -> i64 {
        self.rng.range(0, 2)
    }

    pub fn age(&mut self) -> i64 {
        self.rng.range(14, 40)
    }

    pub fn behavior(&mut self) -> String {
        let index = self.rng.weighted(&[3, 5, 3, 1]);
        BEHAVIORS[index].to_string()
    }

    pub fn sentence(&mut self) -> String {
        self.rng.choice(LOREM).to_string()
    }

    pub fn scopes(&mut self) -> Vec<String> {
        let pool: &[&[&str]] = &[
            &["group_message"],
            &["group_message", "c2c_message"],
            &["group_message", "c2c_message", "media"],
            &["group_message", "media"],
            &["group_message", "member"],
            &["group_message", "c2c_message", "media", "member"],
        ];
        self.rng
            .choice(pool)
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    pub fn group_name(&mut self) -> String {
        let prefixes = [
            "摸鱼", "测试", "深夜", "开发", "临时", "内测", "文档", "摸虾",
        ];
        let suffixes = ["小组", "交流群", "指挥部", "后援会", "水群", "互助会"];
        format!(
            "{}{}{}",
            self.rng.choice(&prefixes[..]),
            self.rng.choice(&suffixes[..]),
            self.rng.range(1, 99)
        )
    }

    pub fn bot_name(&mut self) -> String {
        let prefixes = ["月", "星", "云", "风", "雪"];
        format!(
            "{}{}{}",
            self.rng.choice(&prefixes[..]),
            "见BOT",
            self.rng.range(1, 99)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_fake_data_is_reproducible() {
        let mut a = FakeData::new(42);
        let mut b = FakeData::new(42);
        for _ in 0..5 {
            assert_eq!(a.nickname(), b.nickname());
            assert_eq!(a.city(), b.city());
            assert_eq!(a.behavior(), b.behavior());
            assert_eq!(a.age(), b.age());
        }
    }

    #[test]
    fn openids_are_scoped_per_bot() {
        assert_ne!(
            openid_for("10000", "user", "u_1"),
            openid_for("10001", "user", "u_1")
        );
        assert_eq!(
            openid_for("10000", "user", "u_1"),
            openid_for("10000", "user", "u_1")
        );
        assert_eq!(openid_for("10000", "group", "g_1").len(), 32);
    }

    #[test]
    fn tokens_have_expected_shape() {
        assert_eq!(new_session_id().len(), 32);
        assert!(new_msg_id("MQS_MSG").starts_with("MQS_MSG_"));
        assert_eq!(new_event_id().len(), "MQS_EVT_".len() + 20);
    }

    #[test]
    fn avatars_are_svg() {
        let svg = avatar_svg("u_000001", "小满", "user");
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("小"));
    }
}
