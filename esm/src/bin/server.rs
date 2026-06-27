//! esm-server: HTTP REST + MCP stdio server for the FO76 ESM reader.
//!
//! Daemon mode: `esm-server --daemon` (loopback, OS-assigned port, discovery file)
//! Legacy UI:    `esm-server <ESM> [--compare <ESM2>] [--port 3000]`
//! MCP proxy:    `esm-server --mcp-stdio <ESM>`

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use clap::Parser;
use esm::backend::{
    generate_token, remove_daemon_info, shared_registry, write_daemon_info, DaemonInfo,
    QueryBackend, RemoteBackend, SharedRegistry,
};
use esm::ipc::{dispatch, Request, Response as OpResponse};
use esm::Database;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

// ── Config ───────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "esm-server", about = "FO76 ESM HTTP/MCP server")]
struct Cli {
    /// Path to the ESM file (optional in daemon mode)
    esm: Option<PathBuf>,
    /// Optional second ESM for diff/compare mode (legacy UI)
    #[arg(long)]
    compare: Option<PathBuf>,
    /// HTTP port for legacy UI mode (default 3000)
    #[arg(long, default_value_t = 3000)]
    port: u16,
    /// Run as resident daemon on loopback with OS-assigned port
    #[arg(long)]
    daemon: bool,
    /// Run as MCP server over stdio (proxies to resident daemon)
    #[arg(long)]
    mcp_stdio: bool,
    /// Eagerly warm the xref index on ESM open
    #[arg(long)]
    warm_xref: bool,
}

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    registry: SharedRegistry,
    token: String,
    /// Legacy single-ESM path for browser UI routes
    default_esm: Option<PathBuf>,
    compare_esm: Option<PathBuf>,
    shutdown: Arc<tokio::sync::Mutex<bool>>,
}

// ── Error ────────────────────────────────────────────────────────────────────

struct ApiError(StatusCode, String);

impl ApiError {
    fn not_found(msg: impl std::fmt::Display) -> Self {
        Self(StatusCode::NOT_FOUND, msg.to_string())
    }
    fn bad_request(msg: impl std::fmt::Display) -> Self {
        Self(StatusCode::BAD_REQUEST, msg.to_string())
    }
    fn unauthorized(msg: impl std::fmt::Display) -> Self {
        Self(StatusCode::UNAUTHORIZED, msg.to_string())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        let msg = e.to_string();
        if msg.contains("not found") || msg.contains("Not Found") || msg.contains("Not found") {
            Self(StatusCode::NOT_FOUND, msg)
        } else {
            Self(StatusCode::INTERNAL_SERVER_ERROR, msg)
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({"error": self.1});
        (self.0, Json(body)).into_response()
    }
}

fn check_auth(headers: &HeaderMap, token: &str) -> Result<(), ApiError> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let expected = format!("Bearer {}", token);
    if auth == expected || token.is_empty() {
        Ok(())
    } else {
        Err(ApiError::unauthorized("invalid or missing bearer token"))
    }
}

// ── Static assets ─────────────────────────────────────────────────────────────

static INDEX_HTML: &str = include_str!("../../static/index.html");
static COMPARE_HTML: &str = include_str!("../../static/compare.html");

// ── Handlers ─────────────────────────────────────────────────────────────────

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn compare_page() -> Html<&'static str> {
    Html(COMPARE_HTML)
}

async fn health(State(state): State<AppState>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    check_auth(&headers, &state.token)?;
    Ok(StatusCode::OK)
}

async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_auth(&headers, &state.token)?;
    let residents = state.registry.list_resident();
    Ok(Json(serde_json::json!({
        "resident_esms": residents,
    })))
}

async fn op_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<Request>,
) -> Result<Json<OpResponse>, ApiError> {
    check_auth(&headers, &state.token)?;

    if matches!(&req.op, esm::Op::Shutdown) {
        *state.shutdown.lock().await = true;
        let registry = state.registry.clone();
        tokio::spawn(async move {
            registry.clear();
            let _ = remove_daemon_info();
        });
        return Ok(Json(OpResponse::Ok {
            data: serde_json::Value::Null,
        }));
    }

    let registry = state.registry.clone();
    let response = tokio::task::spawn_blocking(move || dispatch(&registry, &req))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(response))
}

async fn db_for_legacy(state: &AppState) -> Result<Arc<StdMutex<Database>>, ApiError> {
    let path = state.default_esm.as_ref().ok_or_else(|| {
        ApiError::bad_request("no default ESM; use POST /op or start with an ESM path")
    })?;
    state.registry.get_or_open(path).map_err(ApiError::from)
}

async fn info(State(state): State<AppState>) -> impl IntoResponse {
    let db_arc = match db_for_legacy(&state).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    let db = db_arc.lock().unwrap();
    match db.file_info() {
        Ok(info) => Json(serde_json::to_value(info).unwrap_or_default()).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn record_by_formid(
    State(state): State<AppState>,
    Path(formid): Path<String>,
) -> impl IntoResponse {
    let db_arc = match db_for_legacy(&state).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    let mut db = db_arc.lock().unwrap();

    // The path segment is auto-detected: a FormID-looking token is resolved as a
    // FormID, otherwise it falls back to an EditorID lookup.
    if !esm::looks_like_formid(&formid) {
        return match db.record_by_edid(&formid) {
            Ok(rec) => Json(serde_json::to_value(&rec).unwrap_or_default()).into_response(),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("not found")
                    || msg.contains("Not found")
                    || msg.contains("Not Found")
                {
                    ApiError::not_found(format!("EditorID {} not found", formid)).into_response()
                } else {
                    ApiError::from(e).into_response()
                }
            }
        };
    }

    let fid = match esm::parse_form_id_input(&formid) {
        Ok(f) => f,
        Err(e) => return ApiError::bad_request(e).into_response(),
    };
    match db.record_by_formid(fid) {
        Ok(rec) => Json(serde_json::to_value(&rec).unwrap_or_default()).into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("Not found") || msg.contains("Not Found") {
                ApiError::not_found(format!("FormID {} not found", formid)).into_response()
            } else {
                ApiError::from(e).into_response()
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct RecordsQuery {
    id: Option<String>,
    edid: Option<String>,
    r#type: Option<String>,
    limit: Option<usize>,
}

async fn records_query(
    State(state): State<AppState>,
    Query(params): Query<RecordsQuery>,
) -> impl IntoResponse {
    let db_arc = match db_for_legacy(&state).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    let mut db = db_arc.lock().unwrap();
    if let Some(id) = params.id {
        if esm::looks_like_formid(&id) {
            let fid = match esm::parse_form_id_input(&id) {
                Ok(f) => f,
                Err(e) => return ApiError::bad_request(e).into_response(),
            };
            return match db.record_by_formid(fid) {
                Ok(rec) => Json(serde_json::to_value(&rec).unwrap_or_default()).into_response(),
                Err(e) => ApiError::from(e).into_response(),
            };
        }
        return match db.record_by_edid(&id) {
            Ok(rec) => Json(serde_json::to_value(&rec).unwrap_or_default()).into_response(),
            Err(e) => ApiError::from(e).into_response(),
        };
    }
    if let Some(edid) = params.edid {
        return match db.record_by_edid(&edid) {
            Ok(rec) => Json(serde_json::to_value(&rec).unwrap_or_default()).into_response(),
            Err(e) => ApiError::from(e).into_response(),
        };
    }
    if let Some(sig) = params.r#type {
        let limit = params.limit.unwrap_or(50).min(1000);
        return match db.list_by_type(&sig, limit) {
            Ok(entries) => Json(serde_json::to_value(&entries).unwrap_or_default()).into_response(),
            Err(e) => ApiError::from(e).into_response(),
        };
    }
    ApiError::bad_request("specify ?id=, ?edid=, ?type=, or use /records/:formid").into_response()
}

async fn list_groups(State(state): State<AppState>) -> impl IntoResponse {
    let db_arc = match db_for_legacy(&state).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    let db = db_arc.lock().unwrap();
    let groups = db.list_groups();
    Json(serde_json::to_value(&groups).unwrap_or_default()).into_response()
}

#[derive(serde::Deserialize)]
struct ChildrenQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

async fn group_children(
    State(state): State<AppState>,
    Path(sig): Path<String>,
    Query(params): Query<ChildrenQuery>,
) -> impl IntoResponse {
    let db_arc = match db_for_legacy(&state).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(100).min(500);
    let db = db_arc.lock().unwrap();
    match db.list_type_children(&sig, offset, limit) {
        Ok(children) => Json(serde_json::to_value(&children).unwrap_or_default()).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn record_stub_at_offset(
    State(state): State<AppState>,
    Path(offset): Path<u64>,
) -> impl IntoResponse {
    let db_arc = match db_for_legacy(&state).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    let db = db_arc.lock().unwrap();
    match db.record_stub_at(offset) {
        Ok(stub) => Json(serde_json::to_value(&stub).unwrap_or_default()).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn diff_route(State(state): State<AppState>) -> impl IntoResponse {
    let cmp_path = match &state.compare_esm {
        Some(p) => p.clone(),
        None => {
            return ApiError::not_found("no compare file loaded; start with --compare <file.esm>")
                .into_response();
        }
    };
    let default = match &state.default_esm {
        Some(p) => p.clone(),
        None => return ApiError::bad_request("no default ESM loaded").into_response(),
    };
    let registry = state.registry.clone();
    let result = tokio::task::spawn_blocking(move || {
        let arc_a = registry.get_or_open(&default)?;
        let arc_b = registry.get_or_open(&cmp_path)?;
        let db_a = arc_a.lock().unwrap();
        let db_b = arc_b.lock().unwrap();
        esm::diff::diff_databases(&db_a, &db_b)
    })
    .await;

    match result {
        Ok(Ok(diff)) => Json(serde_json::to_value(&diff).unwrap_or_default()).into_response(),
        Ok(Err(e)) => ApiError::from(e).into_response(),
        Err(e) => ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── MCP stdio (proxy to daemon) ──────────────────────────────────────────────

async fn run_mcp_stdio(esm_path: PathBuf) -> anyhow::Result<()> {
    use esm::backend::QueryBackend;
    use esm::ipc::Op;
    use std::io::{BufRead, Write};

    eprintln!("esm-server: MCP stdio mode (proxying to resident daemon).");

    let mut backend = RemoteBackend::connect_or_spawn()?;
    // Warm the ESM in the daemon
    let _ = backend.run(&esm_path, Op::FileInfo)?;

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0", "id": null,
                    "error": {"code": -32700, "message": format!("Parse error: {}", e)}
                });
                writeln!(stdout.lock(), "{}", resp)?;
                continue;
            }
        };

        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        if method == "notifications/initialized" {
            continue;
        }

        let result: serde_json::Value = match method {
            "initialize" => serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "esm-server", "version": "0.1.0"}
            }),
            "ping" => serde_json::json!({}),
            "tools/list" => serde_json::json!({
                "tools": [
                    {
                        "name": "esm_file_info",
                        "description": "Get ESM file metadata (version, record count, masters)",
                        "inputSchema": {"type": "object", "properties": {}}
                    },
                    {
                        "name": "esm_get_record",
                        "description": "Get a decoded record by FormID or EditorID",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "description": "FormID or EditorID (auto-detected)"},
                                "formid": {"type": "string"},
                                "edid": {"type": "string"}
                            }
                        }
                    },
                    {
                        "name": "esm_list_records",
                        "description": "List records of a given 4-character type signature",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "type": {"type": "string"},
                                "limit": {"type": "integer"}
                            },
                            "required": ["type"]
                        }
                    },
                    {
                        "name": "esm_search",
                        "description": "Search records by EditorID and/or display name",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "pattern": {"type": "string"},
                                "types": {"type": "array", "items": {"type": "string"}},
                                "limit": {"type": "integer"}
                            },
                            "required": ["pattern"]
                        }
                    },
                    {
                        "name": "esm_refs",
                        "description": "List records that reference a given FormID or EditorID",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "description": "FormID or EditorID (auto-detected)"},
                                "formid": {"type": "string"},
                                "edid": {"type": "string"},
                                "limit": {"type": "integer"}
                            }
                        }
                    }
                ]
            }),
            "tools/call" => {
                let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                match call_tool_proxy(&mut backend, &esm_path, tool_name, &args) {
                    Ok(text) => serde_json::json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    }),
                    Err(e) => serde_json::json!({
                        "content": [{"type": "text", "text": e.to_string()}],
                        "isError": true
                    }),
                }
            }
            _ => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": format!("Method not found: {}", method)}
                });
                writeln!(stdout.lock(), "{}", resp)?;
                continue;
            }
        };

        let resp = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
        writeln!(stdout.lock(), "{}", resp)?;
        let _ = stdout.lock().flush();
    }
    Ok(())
}

fn call_tool_proxy(
    backend: &mut RemoteBackend,
    esm_path: &std::path::Path,
    name: &str,
    args: &serde_json::Value,
) -> anyhow::Result<String> {
    use esm::ipc::{Op, RecordSel};
    use esm::SearchField;

    match name {
        "esm_file_info" => {
            let info = backend.file_info(esm_path)?;
            Ok(serde_json::to_string_pretty(&info)?)
        }
        "esm_get_record" => {
            let formid = args.get("formid").and_then(|v| v.as_str());
            let edid = args.get("edid").and_then(|v| v.as_str());
            let id = args.get("id").and_then(|v| v.as_str());
            let sel = if let Some(fid_str) = formid {
                RecordSel::FormId(esm::parse_form_id_input(fid_str)?)
            } else if let Some(e) = edid {
                RecordSel::Edid(e.to_string())
            } else if let Some(id) = id {
                RecordSel::from_input(id)?
            } else {
                anyhow::bail!("specify 'id', 'formid', or 'edid' argument");
            };
            let v = backend.run(
                esm_path,
                Op::Record {
                    sel,
                    depth: esm::ResolveDepth::None,
                },
            )?;
            Ok(serde_json::to_string_pretty(&v)?)
        }
        "esm_list_records" => {
            let sig = args
                .get("type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("'type' argument is required"))?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let entries = backend.list_by_type(esm_path, sig, limit.min(500))?;
            Ok(serde_json::to_string_pretty(&entries)?)
        }
        "esm_search" => {
            let pattern = args
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("'pattern' argument is required"))?;
            let types: Vec<String> = args
                .get("types")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_uppercase()))
                        .collect()
                })
                .unwrap_or_default();
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
            let results = backend.search(esm_path, pattern, types, SearchField::Both, limit)?;
            Ok(serde_json::to_string_pretty(&results)?)
        }
        "esm_refs" => {
            let formid = args.get("formid").and_then(|v| v.as_str());
            let edid = args.get("edid").and_then(|v| v.as_str());
            let id = args.get("id").and_then(|v| v.as_str());
            let sel = if let Some(fid_str) = formid {
                RecordSel::FormId(esm::parse_form_id_input(fid_str)?)
            } else if let Some(e) = edid {
                RecordSel::Edid(e.to_string())
            } else if let Some(id) = id {
                RecordSel::from_input(id)?
            } else {
                anyhow::bail!("specify 'id', 'formid', or 'edid' argument");
            };
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
            let refs = backend.referenced_by(esm_path, sel, limit)?;
            Ok(serde_json::to_string_pretty(&refs)?)
        }
        _ => anyhow::bail!("unknown tool: {}", name),
    }
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/compare", get(compare_page))
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/op", post(op_handler))
        .route("/info", get(info))
        .route("/records/{formid}", get(record_by_formid))
        .route("/records", get(records_query))
        .route("/groups", get(list_groups))
        .route("/groups/{sig}/children", get(group_children))
        .route("/stub/{offset}", get(record_stub_at_offset))
        .route("/diff", get(diff_route))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.mcp_stdio {
        let esm = cli
            .esm
            .ok_or_else(|| anyhow::anyhow!("--mcp-stdio requires an ESM path"))?;
        return run_mcp_stdio(esm).await;
    }

    if cli.daemon {
        return run_daemon(cli.warm_xref).await;
    }

    // Legacy UI mode: require ESM path
    let esm = cli
        .esm
        .ok_or_else(|| anyhow::anyhow!("ESM path required (or use --daemon)"))?;

    eprintln!("Opening ESM: {}", esm.display());
    let registry = shared_registry(cli.warm_xref);
    registry.get_or_open(&esm)?;
    if let Some(ref cmp_path) = cli.compare {
        eprintln!("Opening compare ESM: {}", cmp_path.display());
        registry.get_or_open(cmp_path)?;
    }

    let has_compare = cli.compare.is_some();
    let state = AppState {
        registry,
        token: String::new(),
        default_esm: Some(esm),
        compare_esm: cli.compare,
        shutdown: Arc::new(tokio::sync::Mutex::new(false)),
    };

    let app = build_router(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], cli.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("esm-server listening on http://127.0.0.1:{}", cli.port);
    if has_compare {
        eprintln!("  Diff view: http://127.0.0.1:{}/compare", cli.port);
    }

    axum::serve(listener, app).await?;
    Ok(())
}

async fn run_daemon(warm_xref: bool) -> anyhow::Result<()> {
    let token = generate_token();
    let registry = shared_registry(warm_xref);
    let shutdown = Arc::new(tokio::sync::Mutex::new(false));

    let state = AppState {
        registry: registry.clone(),
        token: token.clone(),
        default_esm: None,
        compare_esm: None,
        shutdown: shutdown.clone(),
    };

    let app = build_router(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let port = listener.local_addr()?.port();

    let info = DaemonInfo {
        port,
        token: token.clone(),
        pid: std::process::id(),
    };
    write_daemon_info(&info)?;

    eprintln!(
        "esm-daemon listening on http://127.0.0.1:{} (pid {})",
        port, info.pid
    );

    let shutdown_flag = shutdown.clone();
    let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
        loop {
            if *shutdown_flag.lock().await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        registry.clear();
        let _ = remove_daemon_info();
    });

    serve.await?;
    Ok(())
}
