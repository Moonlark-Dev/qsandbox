//! Embedded GUI assets (built by build.rs from `web/dist`).

include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

pub const FALLBACK_HTML: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><title>QSandbox</title>
<style>body{font-family:system-ui,sans-serif;background:#0f1115;color:#e6e6e6;display:grid;place-items:center;height:100vh;margin:0}
.card{max-width:640px;padding:32px;border:1px solid #2a2f3a;border-radius:16px;background:#171a21;line-height:1.7}
code{background:#0b0d12;padding:2px 6px;border-radius:6px;color:#9ecbff} a{color:#7cc4ff}</style></head>
<body><div class="card"><h1>QSandbox 控制台</h1>
<p>前端尚未构建。开发模式：</p>
<pre><code>cd web &amp;&amp; npm install &amp;&amp; npm run dev</code></pre>
<p>生产构建：</p>
<pre><code>cd web &amp;&amp; npm install &amp;&amp; npm run build</code></pre>
<p>也可以直接用 CLI：<code>mqs status --json</code></p>
<p><a href="/_mqs/health">健康检查</a></p>
<p style="color:#8a8f99;font-size:13px">QSandbox 是非官方沙箱，与腾讯无关联。</p></div></body></html>
"#;

pub fn has_gui() -> bool {
    !WEB_ASSETS.is_empty() && WEB_ASSETS.iter().any(|(path, _, _)| *path == "index.html")
}

pub fn get(path: &str) -> Option<(&'static [u8], &'static str)> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    WEB_ASSETS
        .iter()
        .find(|(name, _, _)| *name == path)
        .map(|(_, body, mime)| (*body, *mime))
}
