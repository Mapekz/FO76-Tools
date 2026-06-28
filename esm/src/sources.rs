//! Deterministic reverse-reference walk: find terminal "sources" for an item.

use crate::formid::parse_formid;
use crate::reader::edid_from_subrecords;
use crate::{Database, FormId, RecordRow};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Default maximum recursion depth through leveled-list intermediates.
pub const DEFAULT_MAX_DEPTH: usize = 6;

/// Classification of a terminal drop/source node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    LeveledList,
    Container,
    Recipe,
    Quest,
    NpcDrop,
    Vendor,
    World,
}

/// One hop on the path from the target item to a terminal source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcePathNode {
    pub form_id: String,
    pub record_type: Option<String>,
    pub editor_id: Option<String>,
}

/// A terminal source that ultimately drops or references the target item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    pub form_id: String,
    pub record_type: String,
    pub editor_id: Option<String>,
    pub name: Option<String>,
    pub path: Vec<SourcePathNode>,
}

/// Result of [`sources_of`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceList {
    pub target: String,
    pub sources: Vec<Source>,
}

/// Options for the sources walk.
#[derive(Debug, Clone, Copy)]
pub struct SourcesOptions {
    pub max_depth: usize,
}

impl Default for SourcesOptions {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }
}

fn is_leveled_list(sig: &str) -> bool {
    matches!(sig, "LVLI" | "LVLN" | "LVLP" | "LVPC")
}

fn classify_terminal(sig: &str) -> Option<SourceKind> {
    match sig {
        "LVLI" | "LVLN" | "LVLP" | "LVPC" => Some(SourceKind::LeveledList),
        "CONT" => Some(SourceKind::Container),
        "COBJ" => Some(SourceKind::Recipe),
        "QUST" => Some(SourceKind::Quest),
        "NPC_" => Some(SourceKind::NpcDrop),
        "VEND" => Some(SourceKind::Vendor),
        "LCTN" | "WRLD" | "CELL" | "REFR" | "ACHR" | "ENC_" | "REGN" | "ECZN" => {
            Some(SourceKind::World)
        }
        _ => None,
    }
}

fn record_type(db: &Database, form_id: FormId) -> Option<String> {
    db.index.get_by_formid(form_id).map(|m| m.signature.clone())
}

fn path_node(db: &Database, row: &RecordRow) -> SourcePathNode {
    let record_type = parse_formid(&row.form_id)
        .ok()
        .and_then(|fid| record_type(db, fid));
    SourcePathNode {
        form_id: row.form_id.clone(),
        record_type,
        editor_id: row.editor_id.clone(),
    }
}

fn path_node_from_id(db: &Database, form_id: FormId) -> SourcePathNode {
    let meta = db.index.get_by_formid(form_id);
    let record_type = meta.map(|m| m.signature.clone());
    let editor_id = meta.and_then(|m| {
        db.esm
            .parse_record_at(m.offset)
            .ok()
            .and_then(|rec| edid_from_subrecords(&rec.subrecords))
    });
    SourcePathNode {
        form_id: form_id.display(),
        record_type,
        editor_id,
    }
}

fn row_for(db: &Database, form_id: FormId) -> anyhow::Result<RecordRow> {
    let meta = db
        .index
        .get_by_formid(form_id)
        .ok_or_else(|| anyhow::anyhow!("FormID {} not in index", form_id.display()))?;
    let rec = db.esm.parse_record_at(meta.offset)?;
    let editor_id = edid_from_subrecords(&rec.subrecords);
    let name = crate::reader::lstring_id_from_subrecords(&rec.subrecords, "FULL").and_then(|id| {
        db.localization.as_ref().and_then(|l| {
            l.lookup(crate::strings::StringKind::Strings, id)
                .map(|s| s.to_owned())
        })
    });
    Ok(RecordRow {
        form_id: form_id.display(),
        editor_id,
        name,
        offset: meta.offset,
    })
}

fn emit_source(
    kind: SourceKind,
    row: &RecordRow,
    sig: &str,
    path: &[SourcePathNode],
    out: &mut Vec<Source>,
    seen_terminals: &mut HashSet<FormId>,
) {
    let fid = match parse_formid(&row.form_id) {
        Ok(f) => f,
        Err(_) => return,
    };
    if !seen_terminals.insert(fid) {
        return;
    }
    out.push(Source {
        kind,
        form_id: row.form_id.clone(),
        record_type: sig.to_string(),
        editor_id: row.editor_id.clone(),
        name: row.name.clone(),
        path: path.to_vec(),
    });
}

#[allow(clippy::too_many_arguments)]
fn walk(
    db: &mut Database,
    current: FormId,
    depth: usize,
    path: &mut Vec<SourcePathNode>,
    opts: &SourcesOptions,
    out: &mut Vec<Source>,
    seen_terminals: &mut HashSet<FormId>,
    visiting: &mut HashSet<FormId>,
) -> anyhow::Result<()> {
    if depth > opts.max_depth {
        return Ok(());
    }
    if !visiting.insert(current) {
        return Ok(());
    }

    let referencers = db.referenced_by(current)?;
    if referencers.is_empty() {
        if depth > 0 {
            if let Some(sig) = record_type(db, current) {
                if is_leveled_list(&sig) {
                    let row = row_for(db, current)?;
                    emit_source(
                        SourceKind::LeveledList,
                        &row,
                        &sig,
                        path,
                        out,
                        seen_terminals,
                    );
                }
            }
        }
        visiting.remove(&current);
        return Ok(());
    }

    for row in referencers {
        let Some(sig) = parse_formid(&row.form_id)
            .ok()
            .and_then(|fid| record_type(db, fid))
        else {
            continue;
        };

        path.push(path_node(db, &row));
        let referencer = parse_formid(&row.form_id).unwrap();

        if is_leveled_list(&sig) {
            walk(
                db,
                referencer,
                depth + 1,
                path,
                opts,
                out,
                seen_terminals,
                visiting,
            )?;
        } else if let Some(kind) = classify_terminal(&sig) {
            emit_source(kind, &row, &sig, path, out, seen_terminals);
        }

        path.pop();
    }

    visiting.remove(&current);
    Ok(())
}

/// Walk reverse references from `target`, recursing through leveled lists until
/// terminal sources (containers, NPCs, quests, recipes, vendors, world nodes).
pub fn sources_of(
    db: &mut Database,
    target: FormId,
    opts: &SourcesOptions,
) -> anyhow::Result<SourceList> {
    let mut path = vec![path_node_from_id(db, target)];
    let mut sources = Vec::new();
    let mut seen_terminals = HashSet::new();
    let mut visiting = HashSet::new();

    walk(
        db,
        target,
        0,
        &mut path,
        opts,
        &mut sources,
        &mut seen_terminals,
        &mut visiting,
    )?;

    sources.sort_by(|a, b| a.form_id.cmp(&b.form_id));

    Ok(SourceList {
        target: target.display(),
        sources,
    })
}
