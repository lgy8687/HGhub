//! HGhub 网页版服务端。
//!
//! 复用 src-tauri 的纯 Rust 核心（net / media / session / 加密 / 线路），
//! 把 Tauri invoke 换成 HTTP，把 hg:// 协议换成 /hg/* 代理，并加一层密码登录门。
//! 默认只绑 127.0.0.1:8900；外网访问应通过出站隧道，不要直接开入站端口。

#[path = "core/net.rs"]
mod net;
#[path = "core/media.rs"]
mod media;
#[path = "core/session.rs"]
mod session;
#[path = "core/network_policy.rs"]
mod network_policy;
#[path = "core/lines.rs"]
mod lines;
#[path = "core/playback_log.rs"]
mod playback_log;
#[path = "core/logging.rs"]
mod logging;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode, Uri},
    middleware::{self, Next},
    response::{Html, IntoResponse, Json, Redirect, Response},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tower_http::services::{ServeDir, ServeFile};

/* ───────────────────────── 业务层 ───────────────────────── */

struct WebApi {
    session: session::SessionStore,
}

impl WebApi {
    fn new(dir: &PathBuf) -> Self {
        let s = session::SessionStore::new();
        s.bind(dir.clone());
        Self { session: s }
    }

    fn token(&self) -> Option<String> {
        self.session.token(&net::base())
    }

    async fn get(&self, path: &str) -> Result<Value, String> {
        let origin = net::base();
        net::edge(&origin, "GET", path, None, self.token().as_deref()).await
    }

    async fn post(&self, path: &str, body: Option<Value>) -> Result<Value, String> {
        let origin = net::base();
        net::edge(&origin, "POST", path, body, self.token().as_deref()).await
    }

    async fn authed(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let origin = net::base();
        let token = self.token().ok_or_else(|| "请先登录".to_string())?;
        net::edge(&origin, method, path, body, Some(&token)).await
    }
}

fn arg_str(a: &Value, k: &str) -> String {
    a.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
}
fn arg_u32(a: &Value, k: &str, d: u32) -> u32 {
    a.get(k)
        .and_then(|v| v.as_u64())
        .unwrap_or(d as u64)
        .min(u32::MAX as u64) as u32
}
fn arg_i64(a: &Value, k: &str) -> i64 {
    a.get(k)
        .and_then(|v| v.as_i64())
        .or_else(|| a.get(k).and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}
fn arg_bool(a: &Value, k: &str) -> bool {
    a.get(k).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn normalize_play_info(info: &mut Value) {
    let non_empty = |v: Option<&Value>| -> Option<String> {
        v.and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let main = non_empty(info.get("streams").and_then(|s| s.get("main")));
    let hevc = non_empty(info.get("streams").and_then(|s| s.get("hevc")));
    if let Some(obj) = info.as_object_mut() {
        for (field, stream) in [("video_url", main), ("video_url_hevc", hevc)] {
            if let Some(url) = stream {
                if non_empty(obj.get(field)).is_none() {
                    obj.insert(field.into(), json!(url));
                }
            }
        }
    }
}

fn has_play_url(info: &Value) -> bool {
    info.get("video_url")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
}

async fn dispatch(api: &WebApi, cmd: &str, a: &Value) -> Result<Value, String> {
    let origin = net::base();
    Ok(match cmd {
        "app_boot" => json!({
            "user": api.session.user(&origin),
            "logged_in": api.token().is_some(),
            "base": origin,
            "strict_security": network_policy::strict_security(),
            "cli_relaxed": false,
        }),

        "api_login" => {
            let data = net::edge(
                &origin,
                "POST",
                "/api/account/loginByPassword",
                Some(json!({
                    "username": arg_str(a, "username"),
                    "password": arg_str(a, "password"),
                })),
                None,
            )
            .await?;
            let token = data
                .get("token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "登录返回缺少 token".to_string())?
                .to_string();
            let user = data.get("user").cloned().unwrap_or(Value::Null);
            api.session.set(&origin, token, user.clone());
            json!({ "user": user })
        }
        "api_register" => {
            let pw = arg_str(a, "password");
            let mut body = json!({
                "username": arg_str(a, "username"),
                "password": pw,
                "confirm_password": pw,
            });
            let email = arg_str(a, "email");
            if !email.trim().is_empty() {
                body["email"] = json!(email);
            }
            let data = net::edge(
                &origin,
                "POST",
                "/api/account/registerByPassword",
                Some(body),
                None,
            )
            .await?;
            let token = data
                .get("token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "注册返回缺少 token".to_string())?
                .to_string();
            let user = data.get("user").cloned().unwrap_or(Value::Null);
            api.session.set(&origin, token, user.clone());
            json!({ "user": user })
        }
        "api_logout" => {
            let _ = net::edge(
                &origin,
                "POST",
                "/api/account/logout",
                None,
                api.token().as_deref(),
            )
            .await;
            api.session.clear();
            json!({ "ok": true })
        }
        "api_me" => api.get("/api/account/me").await?,

        "api_feed" => {
            api.get(&format!(
                "/api/videos?page={}&size={}",
                arg_u32(a, "page", 1),
                arg_u32(a, "size", 20)
            ))
            .await?
        }
        "api_category" => {
            api.get(&format!(
                "/api/videos/category/{}?page={}&size={}",
                arg_str(a, "cate"),
                arg_u32(a, "page", 1),
                arg_u32(a, "size", 20)
            ))
            .await?
        }
        "api_rank" => api.get(&format!("/api/ranks/{}", arg_str(a, "kind"))).await?,
        "api_search" => {
            let q = percent_encoding::utf8_percent_encode(
                &arg_str(a, "q"),
                percent_encoding::NON_ALPHANUMERIC,
            )
            .to_string();
            api.get(&format!(
                "/api/search?q={q}&page={}&size={}",
                arg_u32(a, "page", 1),
                arg_u32(a, "size", 20)
            ))
            .await?
        }
        "api_posts" => {
            api.get(&format!(
                "/api/posts?page={}&size={}",
                arg_u32(a, "page", 1),
                arg_u32(a, "size", 20)
            ))
            .await?
        }
        "api_post" => api.get(&format!("/api/posts/{}", arg_i64(a, "id"))).await?,
        "api_video" => api.get(&format!("/api/videos/{}", arg_i64(a, "id"))).await?,

        "api_play" => {
            let id = arg_i64(a, "id");
            let ep = arg_u32(a, "ep", 1);
            let primary = api
                .post(
                    &format!("/api/videos/{id}/episodes/{ep}/playback"),
                    Some(json!({})),
                )
                .await
                .map(|mut info| {
                    normalize_play_info(&mut info);
                    info
                });
            if let Ok(info) = &primary {
                if has_play_url(info) {
                    return Ok(info.clone());
                }
            }
            match api
                .get(&format!("/api/videos/{id}/play?ep={ep}&page_texts=0"))
                .await
            {
                Ok(mut legacy) => {
                    normalize_play_info(&mut legacy);
                    if has_play_url(&legacy) || primary.is_err() {
                        legacy
                    } else {
                        primary?
                    }
                }
                Err(_) => primary?,
            }
        }

        "api_comments" => {
            api.get(&format!(
                "/api/videos/{}/comments?ep={}&page={}&size={}",
                arg_i64(a, "id"),
                arg_u32(a, "ep", 1),
                arg_u32(a, "page", 1),
                arg_u32(a, "size", 20)
            ))
            .await?
        }
        "api_comment_send" => {
            let id = arg_i64(a, "id");
            let ep = arg_u32(a, "ep", 0);
            let mut body = json!({ "text": arg_str(a, "text") });
            if let Some(rid) = a.get("replyId").or_else(|| a.get("reply_id")) {
                if !rid.is_null() {
                    body["reply_id"] = rid.clone();
                }
            }
            let path = if ep > 0 {
                format!("/api/videos/{id}/comments?ep={ep}")
            } else {
                format!("/api/videos/{id}/comments")
            };
            api.authed("POST", &path, Some(body)).await?
        }
        "api_like" => {
            let method = if arg_bool(a, "on") { "POST" } else { "DELETE" };
            api.authed(
                method,
                &format!(
                    "/api/videos/{}/like?ep={}",
                    arg_i64(a, "id"),
                    arg_u32(a, "ep", 1)
                ),
                None,
            )
            .await?
        }
        "api_collect" => {
            let method = if arg_bool(a, "on") { "POST" } else { "DELETE" };
            api.authed(
                method,
                &format!("/api/videos/{}/collect", arg_i64(a, "id")),
                None,
            )
            .await?
        }
        "api_follow" => {
            let method = if arg_bool(a, "on") { "POST" } else { "DELETE" };
            api.authed(
                method,
                &format!("/api/users/{}/follow", arg_i64(a, "uid")),
                None,
            )
            .await?
        }
        "api_post_like" => {
            let method = if arg_bool(a, "on") { "POST" } else { "DELETE" };
            api.authed(
                method,
                &format!("/api/posts/{}/like", arg_i64(a, "id")),
                None,
            )
            .await?
        }
        "api_post_collect" => {
            let method = if arg_bool(a, "on") { "POST" } else { "DELETE" };
            api.authed(
                method,
                &format!("/api/posts/{}/collect", arg_i64(a, "id")),
                None,
            )
            .await?
        }
        "api_post_comments" => {
            api.get(&format!(
                "/api/posts/{}/comments?page={}&size={}",
                arg_i64(a, "id"),
                arg_u32(a, "page", 1),
                arg_u32(a, "size", 20)
            ))
            .await?
        }

        "line_state" => json!({
            "base": origin,
            "default": net::DEFAULT_BASE,
            "fixed": lines::fixed(),
        }),
        "line_set" => {
            let previous = origin.clone();
            let next = net::set_base(&arg_str(a, "url"))?;
            if previous != next {
                api.session.clear();
            }
            json!(next)
        }
        "line_mirrors" => {
            net::refresh_client();
            lines::mirrors().await?
        }
        "line_probe" => {
            net::refresh_client();
            let urls: Vec<String> = a
                .get("urls")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            lines::probe(urls).await
        }
        "net_diag" => net::diagnose().await,
        "security_set" => {
            network_policy::set_strict_security(arg_bool(a, "strict"))?;
            net::refresh_client();
            json!(network_policy::strict_security())
        }
        "cache_clear" => {
            media::clear_caches();
            json!(null)
        }

        // 桌面 / 原生专属能力：网页版返回中性值，前端的 .catch 会直接吞掉。
        "debug_log" | "set_orientation" | "open_url" => json!(null),
        "playback_log_status" => json!({ "enabled": playback_log::enabled() }),
        "playback_log_set" | "playback_log_write" => json!(null),
        "playback_log_read" => json!(""),
        "check_update" => json!({ "has_update": false, "current": env!("CARGO_PKG_VERSION") }),

        other => return Err(format!("网页版不支持命令：{other}")),
    })
}

/* ───────────────────────── 登录门 ───────────────────────── */

struct AppState {
    api: WebApi,
    web_sessions: Mutex<HashSet<String>>,
    pw_hash: String,
}

fn sha256_hex(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_hex() -> String {
    use std::io::Read;
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("无法读取 /dev/urandom");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

const LOGIN_HTML: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="apple-mobile-web-app-capable" content="yes">
<meta name="apple-mobile-web-app-status-bar-style" content="black-translucent">
<title>HGhub · 登录</title>
<style>
*{box-sizing:border-box}body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;
background:#f5f7f2;color:#1d2b21;font:15px/1.6 -apple-system,BlinkMacSystemFont,"PingFang SC",sans-serif;padding:24px}
.card{width:100%;max-width:340px;background:#fff;border:1px solid #e2e8e0;border-radius:16px;padding:28px}
h1{font-size:19px;font-weight:600;margin:0 0 6px}p{color:#6b7a70;font-size:13px;margin:0 0 20px}
input{width:100%;padding:12px 14px;border:1px solid #d7e0d8;border-radius:10px;font-size:16px;outline:none;background:#fff}
input:focus{border-color:#1d9e75}button{width:100%;margin-top:14px;padding:12px;border:0;border-radius:10px;
background:#1d9e75;color:#fff;font-size:15px;font-weight:500;cursor:pointer}button:disabled{opacity:.5}
.err{color:#a32d2d;font-size:13px;min-height:20px;margin-top:10px}
</style></head><body>
<div class="card"><h1>HGhub</h1><p>请输入访问密码</p>
<input id="pw" type="password" autocomplete="current-password" placeholder="访问密码" autofocus>
<button id="go">进入</button><div class="err" id="err"></div></div>
<script>
const pw=document.getElementById('pw'),go=document.getElementById('go'),err=document.getElementById('err');
async function submit(){if(!pw.value)return;go.disabled=true;err.textContent='';
try{const r=await fetch('/api/auth/login',{method:'POST',headers:{'Content-Type':'application/json'},
body:JSON.stringify({password:pw.value}),credentials:'same-origin'});
if(r.ok){location.replace('/');return}const j=await r.json().catch(()=>({}));err.textContent=j.error||'登录失败';}
catch(e){err.textContent='网络错误'}finally{go.disabled=false}}
go.onclick=submit;pw.addEventListener('keydown',e=>{if(e.key==='Enter')submit()});
</script></body></html>"#;

async fn login_page() -> Html<&'static str> {
    Html(LOGIN_HTML)
}

async fn login_submit(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let pw = body.get("password").and_then(|v| v.as_str()).unwrap_or("");
    if sha256_hex(pw) != st.pw_hash {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "密码不对" })),
        )
            .into_response();
    }
    let sid = random_hex();
    st.web_sessions.lock().unwrap().insert(sid.clone());
    let cookie = format!(
        "hghub_sid={sid}; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000"
    );
    (
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

async fn auth_guard(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    // 只有登录页和登录接口公开；连静态 JS 都不给未登录者。
    if path == "/login" || path == "/api/auth/login" {
        return next.run(req).await;
    }
    let ok = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| {
            c.split(';')
                .find_map(|kv| kv.trim().strip_prefix("hghub_sid="))
        })
        .is_some_and(|sid| st.web_sessions.lock().unwrap().contains(sid));
    if ok {
        return next.run(req).await;
    }
    if path.starts_with("/api/") || path.starts_with("/hg/") {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "未登录" })),
        )
            .into_response();
    }
    Redirect::to("/login").into_response()
}

/* ───────────────────────── HTTP 接口 ───────────────────────── */

async fn cmd(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let name = body.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
    let args = body.get("args").cloned().unwrap_or_else(|| json!({}));
    match dispatch(&st.api, name, &args).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))).into_response(),
    }
}

async fn hg_proxy(path: &str, uri: &Uri) -> Response {
    let query = uri.query().unwrap_or("");
    let upstream = media::handle(path, query, "/hg").await;
    let (parts, body) = upstream.into_parts();
    let mut builder = Response::builder().status(parts.status);
    for (k, v) in parts.headers.iter() {
        builder = builder.header(k.clone(), v.clone());
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn hg_img(uri: Uri) -> Response {
    hg_proxy("/img", &uri).await
}
async fn hg_hls(uri: Uri) -> Response {
    hg_proxy("/hls", &uri).await
}
async fn hg_seg(uri: Uri) -> Response {
    hg_proxy("/seg", &uri).await
}

#[tokio::main]
async fn main() {
    let data_dir = PathBuf::from(
        std::env::var("HGWEB_DATA_DIR").unwrap_or_else(|_| "./data".to_string()),
    );
    std::fs::create_dir_all(&data_dir).expect("无法创建数据目录");
    net::bind_base(&data_dir);
    playback_log::bind(&data_dir);

    let password = std::env::var("HGWEB_PASSWORD")
        .ok()
        .or_else(|| {
            std::fs::read_to_string(data_dir.join("password"))
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .expect("必须设置 HGWEB_PASSWORD 环境变量或 data/password 文件");

    let state = Arc::new(AppState {
        api: WebApi::new(&data_dir),
        web_sessions: Mutex::new(HashSet::new()),
        pw_hash: sha256_hex(&password),
    });
    drop(password);

    let web_dir = std::env::var("HGWEB_WEB_DIR").unwrap_or_else(|_| "web".to_string());
    let index = format!("{web_dir}/index.html");
    let static_files = ServeDir::new(&web_dir).fallback(ServeFile::new(index));

    let app = Router::new()
        .route("/login", get(login_page))
        .route("/api/auth/login", post(login_submit))
        .route("/api/cmd", post(cmd))
        .route("/hg/img", get(hg_img))
        .route("/hg/hls", get(hg_hls))
        .route("/hg/seg", get(hg_seg))
        .fallback_service(static_files)
        .layer(middleware::from_fn_with_state(state.clone(), auth_guard))
        .with_state(state);

    let addr = std::env::var("HGWEB_BIND").unwrap_or_else(|_| "127.0.0.1:8900".to_string());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("无法绑定 {addr}: {e}"));
    println!("hghub-web listening on http://{addr}");
    axum::serve(listener, app).await.expect("服务异常退出");
}
