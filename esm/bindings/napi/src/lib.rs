#![deny(clippy::all)]

//! Node binding for the `esm` engine: one [`EsmHost`] that opens ESMs and runs
//! any [`esm::ops::Op`] against them, the same `Host::run` the CLI and
//! `esm batch` use. Ops travel as JSON (`{"op": "<tag>", ...args}`); the
//! TypeScript `Op` and `OpOutput` types generated from the Rust registry
//! (`esm/src/ops/mod.rs`) type both sides. Every call runs on a blocking
//! worker thread, so the JavaScript thread never waits on decoding or a cache
//! build.

use esm::FormId;
use esm::host::Host;
use napi_derive::napi;
use std::path::PathBuf;
use std::sync::Arc;

fn js_err(e: impl std::fmt::Display) -> napi::Error {
    napi::Error::from_reason(e.to_string())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> napi::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| js_err(format!("worker thread failed: {e}")))?
        .map_err(|e| js_err(format!("{e:#}")))
}

/// The ESMs this process has open, and the one entry point for running ops
/// against them.
#[napi]
pub struct EsmHost {
    host: Arc<Host>,
}

impl Default for EsmHost {
    fn default() -> Self {
        Self::new()
    }
}

#[napi]
impl EsmHost {
    #[napi(constructor)]
    pub fn new() -> Self {
        EsmHost {
            host: Arc::new(Host::new()),
        }
    }

    /// Open the ESM at `path` (a `.esm` file or its data folder), building its
    /// cache if needed, and return its file info. Later `run` calls reuse it.
    #[napi(ts_return_type = "Promise<unknown>")]
    pub async fn open(&self, path: String) -> napi::Result<serde_json::Value> {
        let host = self.host.clone();
        blocking(move || {
            Ok(serde_json::to_value(
                host.open(path.as_ref())?.file_info()?,
            )?)
        })
        .await
    }

    /// Run one op (`{"op": "<tag>", ...args}`) against the ESM at `esm`.
    /// `diff`'s `b` names the other ESM by path.
    #[napi(
        ts_args_type = "esm: string, op: object",
        ts_return_type = "Promise<unknown>"
    )]
    pub async fn run(&self, esm: String, op: serde_json::Value) -> napi::Result<serde_json::Value> {
        let host = self.host.clone();
        blocking(move || {
            let op: esm::ops::Op = serde_json::from_value(op)?;
            host.run(&PathBuf::from(esm), &op)
        })
        .await
    }

    /// Forget the ESM at `esm`; the next `open`/`run` reopens it.
    #[napi]
    pub fn close(&self, esm: String) -> napi::Result<()> {
        self.host
            .close(esm.as_ref())
            .map_err(|e| js_err(format!("{e:#}")))
    }
}

/// Parse a FormID token (hex, `0x`-prefixed or bare) to its display form.
#[napi]
pub fn parse_form_id(s: String) -> napi::Result<String> {
    let fid: FormId = s
        .parse()
        .map_err(|e: anyhow::Error| js_err(format!("{e:#}")))?;
    Ok(fid.display())
}
