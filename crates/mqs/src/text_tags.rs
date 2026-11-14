//! QQ Markdown 里的文本交互标签（官方 `trans/text-chain.html`）。
//!
//! 支持四种标签：
//!
//! - `<qqbot-at-user id="" />`：@ 某个群成员 / 单聊用户，`id` 是被 @ 用户的 openid；
//! - `<qqbot-at-everyone />`：@ 全体成员；
//! - `<qqbot-cmd-enter text="" />`：点击后直接发送 `text`；
//! - `<qqbot-cmd-input text="" show="" reference="" />`：点击后把 `text` 填进输入框，
//!   `show` 是消息里显示的文案（默认取 `text`），`reference="true"` 表示带消息原文引用。
//!
//! 官方要求 `text` / `show` 传 urlencode 后的值，因此这里统一做百分号解码。
//! 标签是 Markdown 正文的一部分：沙箱不改写正文（与真机一致），只把解析结果
//! 用于渲染（GUI）和点击推送（指令标签）。

use percent_encoding::percent_decode_str;

const TAG_PREFIX: &str = "<qqbot-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextTag {
    AtUser {
        id: String,
    },
    AtEveryone,
    CmdEnter {
        text: String,
    },
    CmdInput {
        text: String,
        show: String,
        reference: bool,
    },
}

impl TextTag {
    pub fn is_command(&self) -> bool {
        matches!(self, TextTag::CmdEnter { .. } | TextTag::CmdInput { .. })
    }
}

/// 按出现顺序解析正文里的标签；不是受支持标签的 `<qqbot-…>` 原样跳过。
///
/// 代码块里的标签是示例文本而不是标签（GUI 会把它渲染成代码），所以先屏蔽
/// 围栏代码块和行内代码，否则前端不生成按钮、后端却计入下标，点击就会错位。
pub fn parse(source: &str) -> Vec<TextTag> {
    let masked = mask_code(source);
    let mut tags = Vec::new();
    let mut rest = masked.as_str();
    while let Some(start) = rest.find(TAG_PREFIX) {
        let after = &rest[start + TAG_PREFIX.len()..];
        match parse_tag(after) {
            Some((tag, consumed)) => {
                tags.push(tag);
                rest = &after[consumed..];
            }
            // 正文里出现一个长得很像标签的普通文本时不要卡住，跳过 `<` 继续找。
            None => rest = &rest[start + 1..],
        }
    }
    tags
}

/// 把 Markdown 代码（围栏块与行内代码）替换成空格，长度与原文一致。
fn mask_code(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut fence = false;
    for line in source.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let trimmed = body.trim();
        let fenced = trimmed.starts_with("```")
            && (fence
                || trimmed[3..]
                    .trim()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-'));
        if fenced {
            fence = !fence;
            out.push_str(&" ".repeat(body.len()));
        } else if fence {
            out.push_str(&" ".repeat(body.len()));
        } else {
            mask_inline_code(body, &mut out);
        }
        out.push_str(&line[body.len()..]);
    }
    out
}

fn mask_inline_code(line: &str, out: &mut String) {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'`' {
            let char_len = line[index..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            out.push_str(&line[index..index + char_len]);
            index += char_len;
            continue;
        }
        // 行内代码不跨行：`` `...` `` 且内容非空。
        match line[index + 1..].find('`') {
            Some(offset) if offset > 0 && !line[index + 1..index + 1 + offset].contains('\n') => {
                out.push_str(&" ".repeat(offset + 2));
                index += offset + 2;
            }
            _ => {
                out.push('`');
                index += 1;
            }
        }
    }
}

fn parse_tag(source: &str) -> Option<(TextTag, usize)> {
    let name_end = source.find(|c: char| c.is_whitespace() || c == '/' || c == '>')?;
    let name = &source[..name_end];
    let (attrs, consumed) = parse_attributes(source, name_end)?;
    // 重复属性以最后一个为准，与 GUI 的解析保持一致。
    let get = |key: &str| {
        attrs
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };
    // 空 `id`/`text` 的标签在 GUI 里只是普通文本（不生成按钮），这里也不计入。
    let tag = match name {
        "at-user" => {
            let id = decode(&get("id")?);
            if id.is_empty() {
                return None;
            }
            TextTag::AtUser { id }
        }
        "at-everyone" => TextTag::AtEveryone,
        "cmd-enter" => {
            let text = decode(&get("text")?);
            if text.is_empty() {
                return None;
            }
            TextTag::CmdEnter { text }
        }
        "cmd-input" => {
            let text = decode(&get("text")?);
            if text.is_empty() {
                return None;
            }
            let show = get("show")
                .map(|value| decode(&value))
                .unwrap_or_else(|| text.clone());
            let reference = get("reference")
                .map(|value| value.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            TextTag::CmdInput {
                text,
                show,
                reference,
            }
        }
        _ => return None,
    };
    Some((tag, consumed))
}

/// 从 `index` 开始读属性，直到 `/>`（或 `>`）；返回属性和标签结束的字节位置。
fn parse_attributes(source: &str, mut index: usize) -> Option<(Vec<(String, String)>, usize)> {
    let bytes = source.as_bytes();
    let mut attrs: Vec<(String, String)> = Vec::new();
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            return None;
        }
        if source[index..].starts_with("/>") {
            return Some((attrs, index + 2));
        }
        if bytes[index] == b'>' {
            return Some((attrs, index + 1));
        }
        let name_start = index;
        // 与 GUI 的正则保持同一套属性名语法：字母开头，后接字母/数字/连字符。
        if !bytes[index].is_ascii_alphabetic() {
            return None;
        }
        while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'-')
        {
            index += 1;
        }
        let name = source[name_start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&b'=') {
            return None;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let quote = match bytes.get(index) {
            Some(b'"') => b'"',
            Some(b'\'') => b'\'',
            _ => return None,
        };
        index += 1;
        let value_start = index;
        while index < bytes.len() && bytes[index] != quote {
            index += 1;
        }
        if index >= bytes.len() {
            return None;
        }
        attrs.push((name, source[value_start..index].to_string()));
        index += 1;
    }
}

fn decode(raw: &str) -> String {
    percent_decode_str(raw).decode_utf8_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_four_official_tags() {
        let source = concat!(
            "<qqbot-at-user id=\"2FB3AC4036B00194FB277954260A2860\" />",
            "<qqbot-at-everyone />",
            "<qqbot-cmd-enter text=\"!start\" />",
            "<qqbot-cmd-input text=\"!sweet-potato pass 2\" show=\"林柚\" reference=\"true\" />",
        );
        assert_eq!(
            parse(source),
            vec![
                TextTag::AtUser {
                    id: "2FB3AC4036B00194FB277954260A2860".into()
                },
                TextTag::AtEveryone,
                TextTag::CmdEnter {
                    text: "!start".into()
                },
                TextTag::CmdInput {
                    text: "!sweet-potato pass 2".into(),
                    show: "林柚".into(),
                    reference: true,
                },
            ]
        );
    }

    #[test]
    fn decodes_percent_encoded_attributes() {
        // 官方要求 text/show 传 urlencode 后的值（`%21` = `!`、`%20` = 空格）。
        let tags = parse(
            "<qqbot-cmd-input text=\"%21sweet-potato%20pass%202\" show=\"%E6%9E%97%E6%9F%9A\" />",
        );
        assert_eq!(
            tags,
            vec![TextTag::CmdInput {
                text: "!sweet-potato pass 2".into(),
                show: "林柚".into(),
                reference: false,
            }]
        );
    }

    #[test]
    fn show_defaults_to_text_and_reference_is_strict() {
        let tags = parse("<qqbot-cmd-input text=\"/help\" reference=\"false\" />");
        assert_eq!(
            tags,
            vec![TextTag::CmdInput {
                text: "/help".into(),
                show: "/help".into(),
                reference: false,
            }]
        );
    }

    #[test]
    fn tolerates_attribute_order_whitespace_and_single_quotes() {
        let tags = parse(
            "<qqbot-cmd-enter  text = 'a b' />\n<qqbot-at-user\n id='U1'\n/>\n<qqbot-at-everyone/>",
        );
        assert_eq!(
            tags,
            vec![
                TextTag::CmdEnter { text: "a b".into() },
                TextTag::AtUser { id: "U1".into() },
                TextTag::AtEveryone,
            ]
        );
    }

    #[test]
    fn attribute_names_are_case_insensitive_and_last_wins() {
        let tags = parse("<qqbot-cmd-input TEXT=\"a\" Text=\"b\" />");
        assert_eq!(
            tags,
            vec![TextTag::CmdInput {
                text: "b".into(),
                show: "b".into(),
                reference: false,
            }]
        );
    }

    #[test]
    fn tags_inside_code_are_not_tags() {
        let source = concat!(
            "示例：`<qqbot-cmd-enter text=\"a\" />`\n",
            "```md\n",
            "<qqbot-cmd-input text=\"b\" />\n",
            "```\n",
            "真实：<qqbot-cmd-enter text=\"c\" />",
        );
        let parsed = parse(source);
        assert_eq!(parsed, vec![TextTag::CmdEnter { text: "c".into() }]);
    }

    #[test]
    fn mask_keeps_byte_length() {
        let source = "a `b` c\n```\nd\n```\n";
        assert_eq!(mask_code(source).len(), source.len());
    }

    #[test]
    fn ignores_unknown_and_malformed_tags() {
        let source = "<qqbot-unknown id=\"1\" /> <qqbot-cmd-enter text=x> <qqbot-at-user />";
        let tags = parse(source);
        assert!(tags.is_empty(), "{tags:?}");
    }

    #[test]
    fn commands_are_ordered_and_counted_apart_from_mentions() {
        let source = "<qqbot-at-user id=\"U1\" /><qqbot-cmd-input text=\"a\" /><qqbot-cmd-enter text=\"b\" />";
        let parsed = parse(source);
        let commands: Vec<&TextTag> = parsed.iter().filter(|tag| tag.is_command()).collect();
        assert_eq!(commands.len(), 2);
        assert!(matches!(commands[0], TextTag::CmdInput { text, .. } if text == "a"));
        assert!(matches!(commands[1], TextTag::CmdEnter { text } if text == "b"));
    }

    #[test]
    fn plain_text_is_untouched() {
        assert!(parse("普通消息 <@!2FB3AC40> @全体成员").is_empty());
    }
}
