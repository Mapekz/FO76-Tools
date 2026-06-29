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
use std::time::{Duration, Instant};
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
    /// Tracks the last time a real op was processed (daemon mode only).
    ///
    /// `None` in legacy UI mode — no idle-TTL in that mode.
    /// Updated by `op_handler` on every real op (Shutdown excluded).
    /// `/health` and `/status` do NOT update this, so liveness pings from
    /// `esm -p` clients don't keep the daemon alive indefinitely.
    last_activity: Option<Arc<StdMutex<Instant>>>,
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

    // Record activity for idle-TTL watchdog (daemon mode only).
    if let Some(ref la) = state.last_activity {
        *la.lock().unwrap() = Instant::now();
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
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<esm::diff::DiffResult> {
        let arc_a = registry.get_or_open(&default)?;
        let arc_b = registry.get_or_open(&cmp_path)?;
        let mut diff = {
            let db_a = arc_a.lock().unwrap();
            let db_b = arc_b.lock().unwrap();
            esm::diff::diff_databases(&db_a, &db_b)?
        };
        // Sources pass requires &mut — take a fresh lock after the immutable borrows end.
        let mut db_b = arc_b.lock().unwrap();
        esm::diff::enrich_added_sources(
            &mut db_b,
            &mut diff,
            &esm::sources::SourcesOptions::default(),
        )?;
        Ok(diff)
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
                        "description": "Return header metadata for the current ESM: version, author, description, total record count, and the list of master (.esm) dependencies. Call this first to orient yourself on the file.",
                        "annotations": {"readOnlyHint": true},
                        "inputSchema": {"type": "object", "properties": {}}
                    },
                    {
                        "name": "esm_search",
                        "description": "Search for records by EditorID and/or display name using case-insensitive wildcard patterns (use * for any substring). This is the first step to turn a fuzzy name into concrete FormIDs. Each result includes the record type (e.g. WEAP, MISC, OMOD) so you can disambiguate between similarly-named records — for example, the droppable loose-mod MISC item vs the abstract OMOD object-mod. Narrow the result set with the optional 'types' filter.",
                        "annotations": {"readOnlyHint": true},
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "pattern": {
                                    "type": "string",
                                    "description": "Wildcard pattern matched against EditorID and display name (e.g. \"Bully*\", \"*Rifle*\"). Use * freely; matching is case-insensitive."
                                },
                                "types": {
                                    "type": "array",
                                    "items": {"type": "string"},
                                    "description": "Optional list of 4-character record-type signatures to restrict results (e.g. [\"WEAP\", \"MISC\"]). Omit to search all types. Call esm_list_groups first if you are unsure which signatures exist."
                                },
                                "limit": {
                                    "type": "integer",
                                    "description": "Maximum results to return (default 100)."
                                }
                            },
                            "required": ["pattern"]
                        }
                    },
                    {
                        "name": "esm_get_record",
                        "description": "Fetch and decode a single record by FormID or EditorID. Use this to inspect the full field layout of a record after identifying it with esm_search. The 'resolve' parameter controls how nested FormID references are rendered: 'stub' (default) annotates each reference inline with its EditorID and display name — saving round-trips; 'full' inlines the complete referenced records (richer but larger payloads); 'none' leaves raw hex FormIDs unchanged. Supply exactly one of 'id', 'formid', or 'edid'.",
                        "annotations": {"readOnlyHint": true},
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "FormID (hex e.g. 0x00463F, or decimal) or EditorID — auto-detected by format."
                                },
                                "formid": {
                                    "type": "string",
                                    "description": "FormID as a hex string (e.g. \"0x00463F\") or decimal integer."
                                },
                                "edid": {
                                    "type": "string",
                                    "description": "EditorID string (exact match)."
                                },
                                "resolve": {
                                    "type": "string",
                                    "enum": ["none", "stub", "full"],
                                    "default": "stub",
                                    "description": "How to render nested FormID references. 'stub' (default): annotate each reference with its EditorID + name — avoids extra round-trips. 'full': inline entire referenced records up to 2 levels. 'none': leave raw hex FormIDs unchanged."
                                }
                            }
                        }
                    },
                    {
                        "name": "esm_list_groups",
                        "description": "Return the top-level GRUP inventory of the ESM — the full table of contents showing which record-type signatures are present and how many records each contains. Call this before esm_list_records or esm_search when you are unsure which 4-character type signatures exist (e.g. WEAP, MISC, NPC_, LVLI).",
                        "annotations": {"readOnlyHint": true},
                        "inputSchema": {"type": "object", "properties": {}}
                    },
                    {
                        "name": "esm_list_records",
                        "description": "List records of a given 4-character type signature (e.g. WEAP, NPC_, MISC). Returns FormID, EditorID, and display name for each record, up to the limit. Useful for enumerating all records of a type before narrowing with esm_search. Use esm_list_groups first to discover valid type signatures. Capped at 500 rows.",
                        "annotations": {"readOnlyHint": true},
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "type": {
                                    "type": "string",
                                    "description": "4-character record-type signature (e.g. \"WEAP\", \"MISC\", \"NPC_\"). Call esm_list_groups to discover which signatures are present."
                                },
                                "limit": {
                                    "type": "integer",
                                    "description": "Maximum rows to return (default 50, max 500)."
                                }
                            },
                            "required": ["type"]
                        }
                    },
                    {
                        "name": "esm_refs",
                        "description": "List all records that directly reference a given FormID or EditorID (single-level reverse lookup). Useful for finding what uses a specific record — e.g. which leveled lists include a particular item, or which NPCs carry a specific weapon. This is a ONE-LEVEL lookup only. If you want to answer 'where does this item drop / who drops it / how do I obtain it', use esm_sources instead — it walks the full reverse-reference graph through leveled lists automatically.",
                        "annotations": {"readOnlyHint": true},
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "FormID (hex e.g. 0x00463F, or decimal) or EditorID — auto-detected by format."
                                },
                                "formid": {"type": "string", "description": "FormID as hex or decimal."},
                                "edid": {"type": "string", "description": "EditorID string (exact match)."},
                                "limit": {
                                    "type": "integer",
                                    "description": "Maximum rows to return (default 100)."
                                }
                            }
                        }
                    },
                    {
                        "name": "esm_sources",
                        "description": "Find all terminal drop sources for an item by walking the reverse-reference graph recursively through leveled lists (LVLI/LVLN) up to max_depth levels. This is THE tool to answer questions like 'what drops this item?', 'where do I get X?', 'list all sources of Y'. Do NOT hand-roll this with repeated esm_refs calls — esm_sources already does the full recursive walk with deduplication and cycle safety. Each result has a 'kind' field (LeveledList, Container, Recipe, Quest, NpcDrop, Vendor, World) and a 'path' array showing the leveled-list chain that leads to the terminal source.",
                        "annotations": {"readOnlyHint": true},
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "FormID (hex e.g. 0x00463F, or decimal) or EditorID of the item to trace — auto-detected by format."
                                },
                                "formid": {"type": "string", "description": "FormID as hex or decimal."},
                                "edid": {"type": "string", "description": "EditorID string (exact match)."},
                                "max_depth": {
                                    "type": "integer",
                                    "description": "Maximum leveled-list recursion depth (default 6). Increase only if the graph is known to be unusually deep."
                                }
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

/// Parse `{"id"|"formid"|"edid": ...}` args into a `RecordSel`.
fn sel_from_args(args: &serde_json::Value) -> anyhow::Result<esm::ipc::RecordSel> {
    use esm::ipc::RecordSel;
    let formid = args.get("formid").and_then(|v| v.as_str());
    let edid = args.get("edid").and_then(|v| v.as_str());
    let id = args.get("id").and_then(|v| v.as_str());
    if let Some(fid_str) = formid {
        Ok(RecordSel::FormId(esm::parse_form_id_input(fid_str)?))
    } else if let Some(e) = edid {
        Ok(RecordSel::Edid(e.to_string()))
    } else if let Some(id) = id {
        Ok(RecordSel::from_input(id)?)
    } else {
        anyhow::bail!("specify 'id', 'formid', or 'edid' argument");
    }
}

fn call_tool_proxy(
    backend: &mut RemoteBackend,
    esm_path: &std::path::Path,
    name: &str,
    args: &serde_json::Value,
) -> anyhow::Result<String> {
    use esm::ipc::Op;
    use esm::SearchField;

    match name {
        "esm_file_info" => {
            let info = backend.file_info(esm_path)?;
            Ok(serde_json::to_string_pretty(&info)?)
        }
        "esm_get_record" => {
            let sel = sel_from_args(args)?;
            let depth = match args
                .get("resolve")
                .and_then(|v| v.as_str())
                .unwrap_or("stub")
            {
                "full" => esm::ResolveDepth::Full,
                "none" => esm::ResolveDepth::None,
                _ => esm::ResolveDepth::Stub, // "stub" is the default
            };
            let v = backend.run(esm_path, Op::Record { sel, depth })?;
            Ok(serde_json::to_string_pretty(&v)?)
        }
        "esm_list_groups" => {
            let v = backend.run(esm_path, Op::ListGroups)?;
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
            let sel = sel_from_args(args)?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
            let refs = backend.referenced_by(esm_path, sel, limit)?;
            Ok(serde_json::to_string_pretty(&refs)?)
        }
        "esm_sources" => {
            let sel = sel_from_args(args)?;
            let max_depth = args
                .get("max_depth")
                .and_then(|v| v.as_u64())
                .map(|d| d as usize);
            let sources = backend.sources(esm_path, sel, max_depth)?;
            Ok(serde_json::to_string_pretty(&sources)?)
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
        last_activity: None, // no idle-TTL in legacy UI mode
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
    // ── Idle-TTL configuration ───────────────────────────────────────────────
    // Read `ESM_DAEMON_IDLE_SECS` from the environment (default 600 = 10 min).
    // Set to 0 to disable auto-shutdown entirely.
    let idle_secs: u64 = std::env::var("ESM_DAEMON_IDLE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);

    let token = generate_token();
    let registry = shared_registry(warm_xref);
    let shutdown = Arc::new(tokio::sync::Mutex::new(false));
    let last_activity = Arc::new(StdMutex::new(Instant::now()));

    let state = AppState {
        registry: registry.clone(),
        token: token.clone(),
        default_esm: None,
        compare_esm: None,
        shutdown: shutdown.clone(),
        last_activity: Some(last_activity.clone()),
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
    if idle_secs > 0 {
        eprintln!(
            "esm-daemon: idle-TTL = {}s (ESM_DAEMON_IDLE_SECS=0 to disable)",
            idle_secs
        );
    }

    // ── Idle-TTL watchdog ────────────────────────────────────────────────────
    if idle_secs > 0 {
        let la = last_activity.clone();
        let shutdown_flag = shutdown.clone();
        let ttl = Duration::from_secs(idle_secs);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                let elapsed = la.lock().unwrap().elapsed();
                if elapsed >= ttl {
                    eprintln!(
                        "esm-daemon: idle for {:.0?} (TTL {:.0?}), shutting down.",
                        elapsed, ttl
                    );
                    *shutdown_flag.lock().await = true;
                    break;
                }
            }
        });
    }

    let shutdown_flag = shutdown.clone();
    let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
        loop {
            if *shutdown_flag.lock().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        registry.clear();
        let _ = remove_daemon_info();
    });

    serve.await?;
    Ok(())
}
