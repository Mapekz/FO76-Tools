//! esm-server: HTTP REST + MCP stdio server for the FO76 ESM reader.
//!
//! Start with: `cargo run --features server --bin esm-server -- <ESM> [--compare <ESM2>]`
//! MCP stdio mode: add `--mcp-stdio`

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json, Response},
    routing::get,
    Router,
};
use clap::Parser;
use esm::Database;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

// ── Config ───────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "esm-server", about = "FO76 ESM HTTP/MCP server")]
struct Cli {
    /// Path to the ESM file
    esm: PathBuf,
    /// Optional second ESM for diff/compare mode
    #[arg(long)]
    compare: Option<PathBuf>,
    /// HTTP port (default 3000)
    #[arg(long, default_value_t = 3000)]
    port: u16,
    /// Run as MCP server over stdio (disables HTTP)
    #[arg(long)]
    mcp_stdio: bool,
}

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Database>>,
    compare_db: Option<Arc<Mutex<Database>>>,
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

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn info(State(state): State<AppState>) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db.file_info() {
        Ok(info) => Json(serde_json::to_value(info).unwrap_or_default()).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn record_by_formid(
    State(state): State<AppState>,
    Path(formid): Path<String>,
) -> impl IntoResponse {
    let fid = match esm::parse_form_id_input(&formid) {
        Ok(f) => f,
        Err(e) => return ApiError::bad_request(e).into_response(),
    };
    let mut db = state.db.lock().await;
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
    edid: Option<String>,
    r#type: Option<String>,
    limit: Option<usize>,
}

async fn records_query(
    State(state): State<AppState>,
    Query(params): Query<RecordsQuery>,
) -> impl IntoResponse {
    let mut db = state.db.lock().await;
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
    ApiError::bad_request("specify ?edid=, ?type=, or use /records/:formid").into_response()
}

async fn list_groups(State(state): State<AppState>) -> impl IntoResponse {
    let db = state.db.lock().await;
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
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(100).min(500);
    let mut db = state.db.lock().await;
    match db.list_type_children(&sig, offset, limit) {
        Ok(children) => Json(serde_json::to_value(&children).unwrap_or_default()).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn record_stub_at_offset(
    State(state): State<AppState>,
    Path(offset): Path<u64>,
) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db.record_stub_at(offset) {
        Ok(stub) => Json(serde_json::to_value(&stub).unwrap_or_default()).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn diff_route(State(state): State<AppState>) -> impl IntoResponse {
    let Some(ref cmp) = state.compare_db else {
        return ApiError::not_found("no compare file loaded; start with --compare <file.esm>")
            .into_response();
    };
    // Both db and compare_db must be locked. diff_databases takes &Database.
    let db_a = state.db.lock().await;
    let db_b = cmp.lock().await;
    match esm::diff::diff_databases(&db_a, &db_b) {
        Ok(result) => Json(serde_json::to_value(&result).unwrap_or_default()).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

// ── MCP stdio ────────────────────────────────────────────────────────────────

async fn run_mcp_stdio(db: Arc<Mutex<Database>>) -> anyhow::Result<()> {
    use std::io::{BufRead, Write};

    eprintln!("esm-server: MCP stdio mode. Send JSON-RPC 2.0 messages on stdin.");

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

        // Notifications don't get a response
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
                                "formid": {"type": "string", "description": "FormID in hex, e.g. 0x1234ABCD"},
                                "edid": {"type": "string", "description": "EditorID string, e.g. AssaultRifle"}
                            }
                        }
                    },
                    {
                        "name": "esm_list_records",
                        "description": "List records of a given 4-character type signature",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "type": {"type": "string", "description": "4-char record signature, e.g. WEAP"},
                                "limit": {"type": "integer", "description": "Max records to return (default 50, max 500)"}
                            },
                            "required": ["type"]
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
                match call_tool(&db, tool_name, &args).await {
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

async fn call_tool(
    db: &Arc<Mutex<Database>>,
    name: &str,
    args: &serde_json::Value,
) -> anyhow::Result<String> {
    match name {
        "esm_file_info" => {
            let db = db.lock().await;
            let info = db.file_info()?;
            Ok(serde_json::to_string_pretty(&info)?)
        }
        "esm_get_record" => {
            let formid = args.get("formid").and_then(|v| v.as_str());
            let edid = args.get("edid").and_then(|v| v.as_str());
            let mut db = db.lock().await;
            let rec = if let Some(fid_str) = formid {
                let fid = esm::parse_form_id_input(fid_str)?;
                db.record_by_formid(fid)?
            } else if let Some(e) = edid {
                db.record_by_edid(e)?
            } else {
                anyhow::bail!("specify 'formid' or 'edid' argument");
            };
            Ok(serde_json::to_string_pretty(&rec)?)
        }
        "esm_list_records" => {
            let sig = args
                .get("type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("'type' argument is required"))?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let limit = limit.min(500);
            let mut db = db.lock().await;
            let entries = db.list_by_type(sig, limit)?;
            Ok(serde_json::to_string_pretty(&entries)?)
        }
        _ => anyhow::bail!("unknown tool: {}", name),
    }
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    eprintln!("Opening ESM: {}", cli.esm.display());
    let db = Database::open(&cli.esm)?;
    let db = Arc::new(Mutex::new(db));

    let compare_db = if let Some(cmp_path) = &cli.compare {
        eprintln!("Opening compare ESM: {}", cmp_path.display());
        let cmp = Database::open(cmp_path)?;
        Some(Arc::new(Mutex::new(cmp)))
    } else {
        None
    };

    if cli.mcp_stdio {
        return run_mcp_stdio(db).await;
    }

    let state = AppState { db, compare_db };

    let app = Router::new()
        .route("/", get(index))
        .route("/compare", get(compare_page))
        .route("/health", get(health))
        .route("/info", get(info))
        .route("/records/{formid}", get(record_by_formid))
        .route("/records", get(records_query))
        .route("/groups", get(list_groups))
        .route("/groups/{sig}/children", get(group_children))
        .route("/stub/{offset}", get(record_stub_at_offset))
        .route("/diff", get(diff_route))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], cli.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("esm-server listening on http://localhost:{}", cli.port);
    if cli.compare.is_some() {
        eprintln!("  Diff view: http://localhost:{}/compare", cli.port);
    }

    axum::serve(listener, app).await?;
    Ok(())
}
