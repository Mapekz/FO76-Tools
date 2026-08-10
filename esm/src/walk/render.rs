//! Turns a computed [`super::Digest`] (or a whole [`super::WalkResult`]) into
//! human-readable text — the CLI's default (non-`--json`) `walk` output.
//! This is the only place in `esm::walk` that builds a formatted line; every
//! `digest_*` function in the parent module computes values, never prose
//! (see that module's docs). Kept as a private submodule with two `pub`
//! re-exports ([`render_text`], [`render_digest`]) rather than merged back
//! into `mod.rs`, mirroring `decode.rs`/`decode/vmad.rs`'s existing
//! compute/sub-concern split in this crate.

use super::{
    AvifDigest, ConsumerGroup, Digest, ExplDigest, GenericDigest, GlobDigest, KywdDigest,
    LvliDigest, MagicEffectRow, MagicItemDigest, MgefDigest, OmodDigest, PerkDigest, PerkEffectRow,
    ProjDigest, WalkResult, WeapDigest,
};
use crate::chase::{
    Evidence, FetchDirection, Hop, HopKind, first_array_container, is_truthy, named,
};
use serde_json::Value;

/// Display cap on rendered KYWD/AVIF consumer rows per record-type group —
/// distinct from [`super::CONSUMER_REF_LIMIT`], the *fetch* cap (see that
/// constant's docs). [`ConsumerGroup::total`] is used for the "+N more" line
/// once this cap is hit.
const CONSUMER_ROWS_SHOWN: usize = 10;

/// Cap on pretty-printed lines in the generic fallback digest before a
/// truncation trailer is emitted (mirrors the TS original's `MAX = 120`).
const GENERIC_DUMP_MAX_LINES: usize = 120;

// ─── generic value formatting ───────────────────────────────────────────────

/// Render a JSON value the way the TS original's template-literal
/// interpolation would (unquoted strings, `None`/`True`/`False` for
/// null/bool), with one deliberate improvement: whole-number floats print
/// without a trailing `.0` (matching JS's own number-to-string behavior,
/// which the TS original relies on implicitly) rather than Rust's
/// `serde_json::Number::to_string()`, which always keeps the decimal point.
fn pyish(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(f) = n.as_f64()
                && f.is_finite()
                && f.fract() == 0.0
                && f.abs() < 1e15
            {
                return format!("{}", f as i64);
            }
            n.to_string()
        }
        other => other.to_string(),
    }
}

fn pyish_opt(v: Option<&Value>) -> String {
    v.map(pyish).unwrap_or_else(|| "?".to_string())
}

/// "0xID EditorID" — the universal reference rendering (mirrors the TS
/// original's `ref()`, minus the extra round-trip: Stub resolution already
/// annotated `v` when its enclosing record was fetched).
fn fmt_ref(v: &Value) -> String {
    match v.as_object() {
        Some(obj) if obj.contains_key("formid") => {
            let fid = obj.get("formid").and_then(Value::as_str).unwrap_or("?");
            let edid = obj.get("editor_id").and_then(Value::as_str).unwrap_or("");
            if edid.is_empty() {
                fid.to_string()
            } else {
                format!("{fid} {edid}")
            }
        }
        _ => pyish(v),
    }
}

/// "EditorID=Value" — the magnitude/duration GLOB annotation (mirrors the TS
/// original's `globValue()`; no leading hex, unlike condition operand
/// rendering). `v` is already GLOB-resolved (see
/// `super::resolve_glob_ref` — it carries `"resolved_value"` when known).
fn fmt_glob_annotation(v: &Value) -> String {
    let Some(obj) = v.as_object() else {
        return "?".to_string();
    };
    let edid = obj.get("editor_id").and_then(Value::as_str).unwrap_or("?");
    let value = obj
        .get("resolved_value")
        .map(pyish)
        .unwrap_or_else(|| "?".to_string());
    format!("{edid}={value}")
}

/// "0xID<EditorID[=Value]>" — the inline condition-operand rendering
/// (mirrors the TS original's `fmtConditionsResolved`). `v` is already
/// GLOB-resolved (see `super::resolve_condition_row`).
fn fmt_condition_operand(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::Object(map)) if map.contains_key("formid") => {
            let fid = map.get("formid").and_then(Value::as_str).unwrap_or("?");
            let edid = map.get("editor_id").and_then(Value::as_str).unwrap_or("");
            match map.get("resolved_value") {
                Some(val) => format!("{fid}<{edid}={}>", pyish(val)),
                None => format!("{fid}<{edid}>"),
            }
        }
        Some(other) => pyish(other),
    }
}

/// `Function(Param1) Operator ComparisonValue[ on RunOn][ [OR]]` — the
/// condition line format (mirrors the TS original's `fmtConditions` +
/// `fmtConditionsResolved` combined into one pass). `row` is already
/// GLOB-resolved (see `super::resolve_condition_row`).
fn fmt_condition_row(row: &Value) -> String {
    let function = row.get("Function").and_then(Value::as_str).unwrap_or("?");
    let operator = row.get("Operator").and_then(Value::as_str).unwrap_or("==");
    let param1 = fmt_condition_operand(row.get("Parameter 1"));
    let cmp = fmt_condition_operand(row.get("Comparison Value"));
    let mut out = format!("{function}({param1}) {operator} {cmp}");
    if let Some(run_on) = row.get("Run On").and_then(Value::as_str)
        && run_on != "Subject"
    {
        out.push_str(&format!(" on {run_on}"));
    }
    if row.get("AND/OR").and_then(Value::as_str) == Some("OR") {
        out.push_str(" [OR]");
    }
    out
}

/// "(x,y)(x,y)...  [curve_path]" — curve points are already decoded onto the
/// `Curve Table` field regardless of resolve depth (see [`crate::decode`]'s
/// CURV branch); this just reads them back out.
fn fmt_curve(v: &Value) -> Option<String> {
    let points = v.get("curve")?.as_array()?;
    if points.is_empty() {
        return None;
    }
    let pts: String = points
        .iter()
        .map(|p| {
            let x = p.get("x").map(pyish).unwrap_or_default();
            let y = p.get("y").map(pyish).unwrap_or_default();
            format!("({x},{y})")
        })
        .collect();
    match v.get("curve_path").and_then(Value::as_str) {
        Some(path) => Some(format!("{pts}  [{path}]")),
        None => Some(pts),
    }
}

/// `a or b` (Python truthiness), rendered as text — used wherever the
/// original chase.py-derived logic does `x.get("editor_id") or
/// x.get("formid")`. Moved here from `chase.rs` along with
/// [`summarize_effect`]/[`fmt_stub`] — see this module's doc comment.
fn py_or_display(a: Option<&Value>, b: Option<&Value>) -> String {
    if is_truthy(a) {
        return pyish(a.unwrap());
    }
    if is_truthy(b) {
        return pyish(b.unwrap());
    }
    "None".to_string()
}

fn truncate_json(v: &Value, max_chars: usize) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    if s.chars().count() > max_chars {
        s.chars().take(max_chars).collect()
    } else {
        s
    }
}

/// Recursively find every `Condition Data`-shaped object (has both `Function`
/// and `Operator`) inside `obj` and render it compactly. SPEL and PERK nest
/// conditions differently (`Conditions.Conditions[]` vs `Perk
/// Conditions[].Perk Condition.Conditions[]`) — this walks either. Used only
/// by [`summarize_effect`] (a *different*, path-sliced-single-effect
/// rendering from [`fmt_condition_row`]'s per-row line, which operates on
/// already-flattened/GLOB-resolved condition rows from `super`'s digest
/// builders).
fn extract_conditions(obj: &Value, acc: &mut Vec<String>) {
    match obj {
        Value::Object(map) => {
            if map.contains_key("Function") && map.contains_key("Operator") {
                let fn_ = pyish(map.get("Function").unwrap_or(&Value::Null));
                let op = pyish(map.get("Operator").unwrap_or(&Value::Null));
                let val = pyish(map.get("Comparison Value").unwrap_or(&Value::Null));
                let param = map.get("Parameter 1");
                let line = match param {
                    Some(Value::Object(pmap)) => {
                        let param_txt = py_or_display(pmap.get("editor_id"), pmap.get("formid"));
                        format!("{fn_}({param_txt}) {op} {val}")
                    }
                    Some(p) if !p.is_null() => format!("{fn_}({}) {op} {val}", pyish(p)),
                    _ => format!("{fn_} {op} {val}"),
                };
                acc.push(line);
            } else {
                for v in map.values() {
                    extract_conditions(v, acc);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr {
                extract_conditions(v, acc);
            }
        }
        _ => {}
    }
}

/// Keys already surfaced explicitly by [`summarize_effect`] — anything else on
/// the effect object is scanned generically so fields like PERK's "Function
/// Parameter 3 (Actor Value)" (e.g. the AVIF a Kill-Streak-style perk reads)
/// aren't silently dropped just because they're not one of the handful of
/// well-known keys.
const HANDLED_EFFECT_KEYS: &[&str] = &[
    "Base Effect",
    "Entry Point",
    "Effect Item Data",
    "Float",
    "Conditions",
    "Perk Conditions",
    "Effect Header",
    "Effect Flags",
    "Cooldown Duration",
    "Effect End",
];

/// Render one `Effects[]` element (`{"Effect": {...}}`, SPEL or PERK shape) as
/// a single compact line: base effect / entry point, magnitude, duration, any
/// other FormID reference on the effect, and any gating conditions found
/// anywhere inside it. This crate's one shared "what does this effect entry
/// do" formatter — used for both a forward-fetched target's own `Effects[]`
/// ([`render_forward_evidence`]) and a reverse-chased consumer's path-sliced
/// gated `Effects[N]` row ([`render_reverse_evidence`]).
///
/// Moved here from `chase.rs` (see this module's doc comment) — nothing on
/// `chase`'s own `ChaseTree`-JSON-emitting path ever called it; only
/// `esm::walk`'s rendering did.
fn summarize_effect(effect_entry: &Value) -> String {
    let inner = match effect_entry.as_object() {
        Some(map) => map.get("Effect").unwrap_or(effect_entry),
        None => return truncate_json(effect_entry, 200),
    };
    let Some(inner_map) = inner.as_object() else {
        return truncate_json(effect_entry, 200);
    };

    let mut parts: Vec<String> = Vec::new();

    if let Some(base) = inner_map.get("Base Effect").and_then(Value::as_object) {
        parts.push(py_or_display(base.get("editor_id"), base.get("formid")));
    }

    if let Some(entry_point) = inner_map.get("Entry Point").and_then(Value::as_object) {
        let ep_name = named(entry_point.get("Entry Point"));
        let fn_name = named(entry_point.get("Function"));
        if is_truthy(Some(&ep_name)) {
            if is_truthy(Some(&fn_name)) {
                parts.push(format!("{}/{}", pyish(&ep_name), pyish(&fn_name)));
            } else {
                parts.push(pyish(&ep_name));
            }
        }
    }

    if let Some(item_data) = inner_map.get("Effect Item Data").and_then(Value::as_object) {
        if let Some(mag) = item_data.get("Magnitude")
            && !mag.is_null()
        {
            parts.push(format!("Magnitude={}", pyish(mag)));
        }
        if is_truthy(item_data.get("Duration")) {
            parts.push(format!(
                "Duration={}",
                pyish(item_data.get("Duration").unwrap())
            ));
        }
    }

    if let Some(f) = inner_map.get("Float") {
        parts.push(format!("Float={}", pyish(f)));
    }

    // Generic pass: any other FormID-stub field on the effect itself (not
    // nested under Conditions, handled separately below) — e.g. a PERK's
    // "Function Parameter N (Actor Value)" pointing at an AVIF.
    for (key, val) in inner_map {
        if HANDLED_EFFECT_KEYS.contains(&key.as_str()) {
            continue;
        }
        if let Some(obj) = val.as_object()
            && obj.contains_key("formid")
        {
            parts.push(format!(
                "{key}={}",
                py_or_display(obj.get("editor_id"), obj.get("formid"))
            ));
        }
    }

    let mut text = parts.join("  ");
    let conditions_src = inner_map
        .get("Conditions")
        .filter(|v| is_truthy(Some(*v)))
        .or_else(|| {
            inner_map
                .get("Perk Conditions")
                .filter(|v| is_truthy(Some(*v)))
        });
    if let Some(cs) = conditions_src {
        let mut acc = Vec::new();
        extract_conditions(cs, &mut acc);
        if !acc.is_empty() {
            if !text.is_empty() {
                text.push_str("  ");
            }
            text.push_str("Conditions: ");
            text.push_str(&acc.join("; "));
        }
    }

    if text.is_empty() {
        truncate_json(inner, 200)
    } else {
        text
    }
}

/// "record_type formid editor_id" — the universal stub rendering this
/// module's tests and the OMOD mechanism-slice renderer use to name a
/// classified hop's target or a reverse-chased consumer. Moved here from
/// `chase.rs` (see this module's doc comment) — `esm::chase`'s own
/// `ChaseTree` JSON never called it, only `esm::walk`'s rendering did.
fn fmt_stub(stub: &Value) -> String {
    let rt = stub
        .get("record_type")
        .filter(|v| is_truthy(Some(*v)))
        .map(pyish)
        .unwrap_or_else(|| "?".to_string());
    let fid = stub
        .get("formid")
        .filter(|v| is_truthy(Some(*v)))
        .map(pyish)
        .unwrap_or_else(|| "?".to_string());
    let edid = stub
        .get("editor_id")
        .filter(|v| is_truthy(Some(*v)))
        .map(pyish)
        .unwrap_or_default();
    format!("{rt} {fid} {edid}").trim_end().to_string()
}

/// Left-align each column to its widest cell, two-space-joined. A minimal,
/// string-returning stand-in for `print_record_table` (`src/bin/cli.rs`,
/// binary-crate-private and `println!`s directly rather than returning lines
/// for `WalkNode::digest`) — not worth promoting/sharing for one caller.
fn align_table(headers: &[&str], rows: &[Vec<String>]) -> Vec<String> {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let pad = |cells: &[String]| -> String {
        cells
            .iter()
            .enumerate()
            .take(cols)
            .map(|(i, c)| format!("{c:<width$}", width = widths[i]))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let header_row: Vec<String> = headers.iter().map(|h| (*h).to_string()).collect();
    let mut out = vec![pad(&header_row)];
    out.extend(rows.iter().map(|r| pad(r)));
    out
}

fn format_drop_notes(notes: &[crate::lvli::DropNote]) -> String {
    use crate::lvli::DropNote;
    notes
        .iter()
        .map(|n| match n {
            DropNote::Gated { function } => format!("gated:{function}"),
            DropNote::Cycle => "cycle".to_string(),
            DropNote::DepthCapped => "depth-capped".to_string(),
            DropNote::PoolCapped => "pool-capped".to_string(),
            DropNote::QuantityOnSublist => "qty-on-sublist".to_string(),
            DropNote::Unresolved { reason } => format!("unresolved: {reason}"),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

// ─── per-type renderers ─────────────────────────────────────────────────────

fn render_glob(d: &GlobDigest, lines: &mut Vec<String>) {
    lines.push(format!("value {}", pyish_opt(d.value.as_ref())));
}

fn render_consumers(groups: &[ConsumerGroup], lines: &mut Vec<String>) {
    for g in groups {
        lines.push(format!("{} consumers (gate on this):", g.record_type));
        for r in g.rows.iter().take(CONSUMER_ROWS_SHOWN) {
            let via = r
                .via
                .as_deref()
                .map(|p| format!("  via {p}"))
                .unwrap_or_default();
            lines.push(format!("  {} {}{via}", r.formid, r.editor_id));
        }
        if g.total > CONSUMER_ROWS_SHOWN {
            lines.push(format!("  … +{} more", g.total - CONSUMER_ROWS_SHOWN));
        }
    }
}

fn render_avif(d: &AvifDigest, lines: &mut Vec<String>) {
    let abbrev = d.abbreviation.as_deref().unwrap_or("—");
    lines.push(format!(
        "abbrev {abbrev}  default {}  max {}",
        pyish_opt(d.default_value.as_ref()),
        pyish_opt(d.maximum_value.as_ref()),
    ));
    render_consumers(&d.consumers, lines);
}

fn render_kywd(d: &KywdDigest, lines: &mut Vec<String>) {
    render_consumers(&d.consumers, lines);
}

fn render_mgef(d: &MgefDigest, lines: &mut Vec<String>) {
    lines.push(format!(
        "archetype {}  casting {}",
        d.archetype.as_deref().unwrap_or("?"),
        d.casting_type.as_deref().unwrap_or("?"),
    ));
    if let Some(av) = &d.target_av {
        lines.push(format!("target AV {}", fmt_ref(av)));
    }
    if let Some(rv) = &d.resist_av {
        lines.push(format!(
            "resist AV {} (element carrier for Damage archetype)",
            fmt_ref(rv)
        ));
    }
    if let Some(p) = &d.perk_to_apply {
        lines.push(format!("Perk to Apply → {}", fmt_ref(p)));
    }
    if let Some(eq) = &d.equip_ability {
        lines.push(format!("Equip Ability → {}", fmt_ref(eq)));
    }
    if let Some(desc) = &d.description {
        lines.push(format!("description \"{desc}\""));
    }
}

fn render_magic_item(d: &MagicItemDigest, lines: &mut Vec<String>) {
    for row in &d.effects {
        render_magic_effect_row(row, lines);
    }
}

fn render_magic_effect_row(row: &MagicEffectRow, lines: &mut Vec<String>) {
    let archetype = row.archetype.as_deref().unwrap_or("?");
    let av_part = row
        .actor_value
        .as_ref()
        .map(|v| format!(", AV {}", fmt_ref(v)))
        .unwrap_or_default();
    lines.push(format!(
        "effect[{}] → MGEF {} ({archetype}{av_part})",
        row.index,
        row.base_effect
            .as_ref()
            .map(fmt_ref)
            .unwrap_or_else(|| "None".to_string())
    ));

    lines.push(format!(
        "  magnitude {}  duration {}",
        pyish(&row.magnitude),
        pyish(&row.duration)
    ));

    if let Some(mag_glob) = &row.magnitude_glob {
        let g = fmt_glob_annotation(mag_glob);
        let flat_mag = row.magnitude.as_f64().unwrap_or(0.0);
        if flat_mag == 0.0 {
            lines.push(format!("  magnitude GLOB {g}  ← real value (flat is 0)"));
        } else {
            lines.push(format!(
                "  sibling Magnitude GLOB {g}  ← IGNORE (flat wins; survival scale const)"
            ));
        }
    }
    if let Some(dur_glob) = &row.duration_glob {
        lines.push(format!("  duration GLOB {}", fmt_glob_annotation(dur_glob)));
    }

    if let Some(curve_str) = row.curve_table.as_ref().and_then(fmt_curve) {
        lines.push(format!("  curve {curve_str}"));
        if let Some(av) = &row.curve_input_av {
            lines.push(format!("  curve INPUT axis: AV {}", fmt_ref(av)));
        }
    }

    for cond in &row.conditions {
        lines.push(format!("  cond: {}", fmt_condition_row(cond)));
    }

    if let Some(perk) = &row.perk_to_apply {
        lines.push(format!("  Perk to Apply → {}", fmt_ref(perk)));
    }
    if let Some(eq) = &row.equip_ability {
        lines.push(format!("  Equip Ability → {}", fmt_ref(eq)));
    }
}

fn render_perk(d: &PerkDigest, lines: &mut Vec<String>) {
    if let Some(desc) = &d.description {
        lines.push(format!("description \"{desc}\""));
    }
    let playable = d.playable.as_deref().unwrap_or("?");
    let mut header = format!(
        "ranks {}  playable {playable}",
        pyish_opt(d.num_ranks.as_ref())
    );
    if let Some(next) = &d.next_perk {
        header.push_str(&format!("  next → {}", fmt_ref(next)));
    }
    lines.push(header);

    let Some(effects) = &d.effects else {
        lines.push("NO effects — bonus is engine/script-side (description only)".to_string());
        return;
    };
    for row in effects {
        match row {
            PerkEffectRow::Ability { index, target } => {
                lines.push(format!(
                    "effect[{index}] Ability → SPEL {}",
                    fmt_ref(target)
                ));
            }
            PerkEffectRow::EntryPoint {
                index,
                entry_point_name,
                function_name,
                float_value,
                actor_value,
                conditions,
            } => {
                let ep_name = entry_point_name.as_deref().unwrap_or("?");
                let fn_name = function_name.as_deref().unwrap_or("?");
                let mut l = format!("effect[{index}] Entry Point \"{ep_name}\"  fn {fn_name}");
                if let Some(f) = float_value {
                    l.push_str(&format!("  value {}", pyish(f)));
                }
                if let Some(av) = actor_value {
                    l.push_str(&format!("  AV {}", fmt_ref(av)));
                }
                lines.push(l);
                for cond in conditions {
                    lines.push(format!("  cond: {}", fmt_condition_row(cond)));
                }
            }
            PerkEffectRow::Other { index, type_name } => {
                lines.push(format!("effect[{index}] {type_name}"));
            }
        }
    }
}

fn render_weap(d: &WeapDigest, lines: &mut Vec<String>) {
    lines.push(format!(
        "keywords: {}",
        if d.relevant_keywords.is_empty() {
            "(none damage-relevant)".to_string()
        } else {
            d.relevant_keywords.join(", ")
        }
    ));
    lines.push(format!(
        "apCost {}  speed {}  reloadSpeed {}",
        pyish_opt(d.ap_cost.as_ref()),
        pyish_opt(d.speed.as_ref()),
        pyish_opt(d.reload_speed.as_ref()),
    ));
    let levels: Vec<String> = d.eligible_levels.iter().map(pyish).collect();
    lines.push(format!(
        "eligible levels: {}  attach slots: {}",
        if levels.is_empty() {
            "—".to_string()
        } else {
            levels.join(",")
        },
        d.attach_slots
    ));
    if d.has_object_template {
        lines.push(
            "has Object Template (instance-template mods = POSSIBLE loadouts, never auto-apply)"
                .to_string(),
        );
    }
}

fn render_proj(d: &ProjDigest, lines: &mut Vec<String>) {
    if let Some(t) = &d.proj_type {
        lines.push(format!("type {t}"));
    }
    if let Some(s) = &d.speed {
        lines.push(format!("speed {}", pyish(s)));
    }
    if let Some(expl) = &d.explosion {
        lines.push(format!("explosion → {}", fmt_stub(expl)));
    }
}

fn render_expl(d: &ExplDigest, lines: &mut Vec<String>) {
    render_explosion_detail_lines(&d.detail, lines, "");
}

/// Shared EXPL field lines (radius/force/stagger/impact/chain/damage) used by
/// both the OMOD projectile-evidence slice and the EXPL digest arm.
fn render_explosion_detail_lines(detail: &Value, lines: &mut Vec<String>, indent: &str) {
    if let Some(radius) = detail.get("radius").and_then(Value::as_array) {
        let inner = radius.first().map(pyish).unwrap_or_else(|| "?".to_string());
        let outer = radius.get(1).map(pyish).unwrap_or_else(|| "?".to_string());
        lines.push(format!("{indent}radius {inner}/{outer}"));
    }
    let mut phys: Vec<String> = Vec::new();
    if let Some(f) = detail.get("force") {
        phys.push(format!("force {}", pyish(f)));
    }
    if let Some(s) = detail.get("stagger").and_then(Value::as_str) {
        phys.push(format!("stagger {s}"));
    }
    if !phys.is_empty() {
        lines.push(format!("{indent}{}", phys.join("  ")));
    }
    if let Some(ipds) = detail.get("impact_data_set").and_then(Value::as_str) {
        lines.push(format!("{indent}impact {ipds}"));
    }
    if detail.get("chain").and_then(Value::as_bool) == Some(true) {
        lines.push(format!("{indent}chain"));
    }
    if let Some(placed) = detail.get("placed_object").filter(|v| is_ref_stub_like(v)) {
        lines.push(format!("{indent}placed object → {}", fmt_stub(placed)));
    }
    if let Some(spawn) = detail
        .get("spawn_projectile")
        .filter(|v| is_ref_stub_like(v))
    {
        lines.push(format!("{indent}spawn projectile → {}", fmt_stub(spawn)));
    }
    if let Some(damage) = detail.get("damage").and_then(Value::as_array) {
        if damage.is_empty() {
            // Empty + chain signals arc falloff elsewhere; empty alone is a
            // utility explosion (radius/force/stagger IS the effect).
            if detail.get("chain").and_then(Value::as_bool) != Some(true) {
                lines.push(format!("{indent}damage (none)"));
            }
        } else {
            for row in damage {
                lines.push(format!("{indent}damage {}", format_damage_row(row)));
            }
        }
    }
}

/// A decoded FormID reference at [`crate::ResolveDepth::Stub`] is a
/// `{"formid", "editor_id", "record_type"}` object — the same shape check
/// `super::is_ref_stub` makes on the compute side, duplicated here (rather
/// than shared) since it's the only compute-time check this render module
/// still needs, on data (`Evidence.detail`) that arrives from `chase.rs`
/// rather than through `super`'s own digest builders.
fn is_ref_stub_like(v: &Value) -> bool {
    matches!(v, Value::Object(map) if map.contains_key("formid"))
}

fn format_damage_row(row: &Value) -> String {
    if let Some(flat) = row.get("flat") {
        return format!("flat {}", pyish(flat));
    }
    if let Some(m) = row.get("base_weapon_mult") {
        return format!("base weapon mult {}", pyish(m));
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = row.get("type").and_then(Value::as_str) {
        parts.push(t.to_string());
    }
    if let Some(c) = row.get("curve").and_then(Value::as_str) {
        parts.push(format!("via {c}"));
    }
    if let Some(range) = row.get("range").and_then(Value::as_array)
        && range.len() >= 2
    {
        parts.push(format!("[{}–{}]", pyish(&range[0]), pyish(&range[1])));
    }
    if let Some(amount) = row.get("amount") {
        parts.push(format!("amount {}", pyish(amount)));
    }
    if parts.is_empty() {
        pyish(row)
    } else {
        parts.join("  ")
    }
}

fn render_lvli(d: &LvliDigest, lines: &mut Vec<String>) {
    let table = &d.table;
    let model_name = match table.model {
        crate::lvli::SelectionModel::Pool => "pool (pick one, weighted by passing subset)",
        crate::lvli::SelectionModel::UseAll => "Use All (every passing entry dispensed)",
        crate::lvli::SelectionModel::UseFirstMatch => "Use First Match (ordered cascade)",
    };
    lines.push(format!(
        "drop odds  model {model_name}  level {}  p(nothing) {:.1}%{}",
        table.level,
        table.p_nothing * 100.0,
        if table.truncated {
            "  [approximated in places — see notes]"
        } else {
            ""
        },
    ));

    if table.rows.is_empty() {
        lines.push("  (no eligible entries at this level)".to_string());
    } else {
        let headers = ["item", "expected", "p(>=1)", "notes"];
        let rows: Vec<Vec<String>> = table
            .rows
            .iter()
            .map(|r| {
                vec![
                    format!("{} {} {}", r.record_type, r.formid, r.editor_id),
                    format!("{:.4}", r.expected_count),
                    format!("{:.2}%", r.p_at_least_one * 100.0),
                    format_drop_notes(&r.notes),
                ]
            })
            .collect();
        for line in align_table(&headers, &rows) {
            lines.push(format!("  {line}"));
        }
    }
}

fn render_generic(d: &GenericDigest, lines: &mut Vec<String>) {
    let dump = serde_json::to_string_pretty(&d.trimmed).unwrap_or_default();
    let dump_lines: Vec<&str> = dump.lines().collect();
    for l in dump_lines.iter().take(GENERIC_DUMP_MAX_LINES) {
        lines.push((*l).to_string());
    }
    if dump_lines.len() > GENERIC_DUMP_MAX_LINES {
        lines.push(format!(
            "… {} more lines (use `esm get` for the full record)",
            dump_lines.len() - GENERIC_DUMP_MAX_LINES
        ));
    }
}

// ─── OMOD mechanism rendering ───────────────────────────────────────────────

fn render_omod(d: &OmodDigest, lines: &mut Vec<String>) {
    render_omod_hops(&d.hops, lines);
    for target in &d.includes {
        lines.push(format!("include → {}", fmt_stub(target)));
    }
    if d.includes_total > d.includes.len() {
        lines.push(format!(
            "  … +{} more includes (truncated)",
            d.includes_total - d.includes.len()
        ));
    }
}

/// Render classified [`Hop`]s in classifier order. Consecutive
/// [`HopKind::TagKeyword`] hops collapse into one `tags` block; include-
/// sourced hops (`source_omod.is_some()`) are skipped here — walk enqueues
/// includes as their own BFS nodes instead of folding them into the
/// includer's digest. A directly-attached ENCH property renders through the
/// same `DirectProperty` path as a PROJ or SPEL (no separate ENCH-follow
/// pass to suppress against — see `super::omod_hops_enqueue`).
pub(super) fn render_omod_hops(hops: &[Hop], lines: &mut Vec<String>) {
    let mut i = 0;
    while i < hops.len() {
        let hop = &hops[i];
        if hop.source_omod.is_some() {
            i += 1;
            continue;
        }
        if hop.target.is_none() {
            i += 1;
            continue;
        }
        if hop.kind == HopKind::TagKeyword {
            let start = i;
            i += 1;
            while i < hops.len() {
                let next = &hops[i];
                if next.source_omod.is_some() || next.kind != HopKind::TagKeyword {
                    break;
                }
                i += 1;
            }
            render_tag_keyword_block(&hops[start..i], lines);
            continue;
        }
        render_omod_hop(hop, lines);
        i += 1;
    }
}

/// One `tags` block for a contiguous run of [`HopKind::TagKeyword`] hops —
/// editor_id plus Notes when present; never a dead-end caveat.
fn render_tag_keyword_block(hops: &[Hop], lines: &mut Vec<String>) {
    if hops.is_empty() {
        return;
    }
    lines.push("tags".to_string());
    for hop in hops {
        let Some(target) = &hop.target else {
            continue;
        };
        let edid = target
            .get("editor_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let notes = hop
            .evidence
            .first()
            .and_then(|ev| ev.detail.get("notes"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        match notes {
            Some(n) => lines.push(format!("    {edid} — {n}")),
            None => lines.push(format!("    {edid}")),
        }
    }
}

/// Render one classified [`Hop`] as indented mechanism lines. Skips bare-
/// number properties (`hop.target.is_none()` — nothing to chase, unchanged
/// from before this walk had a dedicated OMOD digest). A directly-attached
/// ENCH property renders through the plain `DirectProperty` arm below, same
/// as a direct SPEL attachment.
fn render_omod_hop(hop: &Hop, lines: &mut Vec<String>) {
    let Some(target) = &hop.target else {
        return;
    };
    let target_rt = target
        .get("record_type")
        .and_then(Value::as_str)
        .unwrap_or("");

    match hop.kind {
        HopKind::PerkGrant => {
            lines.push(format!("perk grant → {}", fmt_stub(target)));
            render_forward_evidence(&hop.evidence, lines);
        }
        HopKind::KeywordHook => {
            lines.push(format!("keyword hook → {}", fmt_stub(target)));
            render_reverse_evidence(&hop.evidence, lines);
        }
        HopKind::TagKeyword => {
            // Consecutive runs are flushed by [`render_omod_hops`]; a lone
            // call still renders a one-entry tags block.
            render_tag_keyword_block(std::slice::from_ref(hop), lines);
        }
        // An AVIF direct-property target is reverse-chased exactly like a
        // KYWD keyword_hook (see `esm::chase::omod_chase`) even though its
        // `HopKind` stays `DirectProperty` — `HopKind` alone doesn't
        // distinguish "direct" from "AV hook" (both are `DirectProperty`;
        // see `CONTEXT.md`'s **Mechanism** entry), so dispatch on
        // `hop.resolution` (the typed forward/reverse fact `classify_property_row`
        // computes) instead of string-matching `target.record_type`.
        HopKind::DirectProperty if hop.resolution == Some(FetchDirection::Reverse) => {
            lines.push(format!("AV hook → {}", fmt_stub(target)));
            render_reverse_evidence(&hop.evidence, lines);
        }
        HopKind::DirectProperty if target_rt == "PROJ" => {
            lines.push(format!("direct property → {}", fmt_stub(target)));
            render_projectile_evidence(&hop.evidence, lines);
        }
        // Direct SPEL attachment (the other `FORWARD_FETCH_TYPES` member
        // besides ENCH/PERK) — forward-fetched the same way a perk grant is.
        HopKind::DirectProperty => {
            lines.push(format!("direct property → {}", fmt_stub(target)));
            render_forward_evidence(&hop.evidence, lines);
        }
    }
}

/// Compact PROJ/EXPL summary from chase's projectile evidence detail.
fn render_projectile_evidence(evidence: &[Evidence], lines: &mut Vec<String>) {
    for ev in evidence {
        let d = &ev.detail;
        let mut parts: Vec<String> = Vec::new();
        if let Some(t) = d.get("type").and_then(Value::as_str) {
            parts.push(format!("type {t}"));
        }
        if let Some(s) = d.get("speed") {
            parts.push(format!("speed {}", pyish(s)));
        }
        if !parts.is_empty() {
            lines.push(format!("  {}", parts.join("  ")));
        }
        if let Some(expl) = d.get("explosion").filter(|v| is_ref_stub_like(v)) {
            lines.push(format!("  explosion → {}", fmt_stub(expl)));
        }
        render_explosion_detail_lines(d, lines, "  ");
    }
}

/// Render forward-fetched [`Evidence`] (perk_grant / direct SPEL attachment):
/// the target's own `Effects[]` array (already capped by
/// `esm::chase`'s `forward_evidence`), path-labeled by array position since
/// there's no gating condition to slice down to one entry — plus a trailing
/// MGEF pass-through line when the target's own Base Effect carries "Perk to
/// Apply"/"Equip Ability" (named, not expanded further — same as `chase`).
fn render_forward_evidence(evidence: &[Evidence], lines: &mut Vec<String>) {
    for ev in evidence {
        if let Some(effects) = ev.detail.get("effects").and_then(Value::as_array) {
            for (i, eff) in effects.iter().enumerate() {
                lines.push(format!("  Effects[{i}] {}", summarize_effect(eff)));
            }
            if let Some(trunc) = ev
                .detail
                .get("effects_truncated")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
            {
                lines.push(format!("  … +{trunc} more effects (truncated)"));
            }
        }
        if let Some(p) = ev.detail.get("perk_to_apply").filter(|v| !v.is_null()) {
            lines.push(format!("  Perk to Apply → {}", fmt_stub(p)));
        }
        if let Some(e) = ev.detail.get("equip_ability").filter(|v| !v.is_null()) {
            lines.push(format!("  Equip Ability → {}", fmt_stub(e)));
        }
    }
}

/// Render reverse-chased [`Evidence`] (keyword_hook / AVIF consumer lookup):
/// group consecutive entries sharing the same gating consumer under one
/// `"gates <consumer>"` header, then the exact path-sliced `Effects[N]` row
/// each evidence entry names — never the consumer's full digest. Empty
/// evidence (no SPEL/PERK condition ever references this target) renders one
/// dead-end line instead of silently vanishing.
fn render_reverse_evidence(evidence: &[Evidence], lines: &mut Vec<String>) {
    if evidence.is_empty() {
        lines.push(
            "  (no SPEL/PERK condition references this — dead end; may be UI-only, \
             native-engine-consumed, or a shared/common tag)"
                .to_string(),
        );
        return;
    }
    let mut last_source: Option<String> = None;
    for ev in evidence {
        let source_fid = ev
            .source
            .get("formid")
            .and_then(Value::as_str)
            .unwrap_or("");
        if last_source.as_deref() != Some(source_fid) {
            lines.push(format!("  gates {}", fmt_stub(&ev.source)));
            last_source = Some(source_fid.to_string());
        }
        if let Some(effect) = ev.detail.get("effect") {
            let label = ev
                .via
                .as_deref()
                .and_then(first_array_container)
                .unwrap_or_else(|| "effect".to_string());
            lines.push(format!("    {label} {}", summarize_effect(effect)));
        } else if let Some(note) = ev.detail.get("note").and_then(Value::as_str) {
            lines.push(format!("    {note}"));
        }
    }
}

// ─── top-level dispatch ─────────────────────────────────────────────────────

/// Turn one computed [`Digest`] into its rendered lines — the single place
/// that dispatches on the 12-variant [`Digest`] enum for text output.
pub fn render_digest(digest: &Digest) -> Vec<String> {
    let mut lines = Vec::new();
    match digest {
        Digest::Glob(d) => render_glob(d, &mut lines),
        Digest::Avif(d) => render_avif(d, &mut lines),
        Digest::Kywd(d) => render_kywd(d, &mut lines),
        Digest::Mgef(d) => render_mgef(d, &mut lines),
        Digest::MagicItem(d) => render_magic_item(d, &mut lines),
        Digest::Perk(d) => render_perk(d, &mut lines),
        Digest::Weap(d) => render_weap(d, &mut lines),
        Digest::Proj(d) => render_proj(d, &mut lines),
        Digest::Expl(d) => render_expl(d, &mut lines),
        Digest::Lvli(d) => render_lvli(d, &mut lines),
        Digest::Omod(d) => render_omod(d, &mut lines),
        Digest::Generic(d) => render_generic(d, &mut lines),
    }
    lines
}

/// Render a [`WalkResult`] as human-readable text — the CLI's default
/// (non-`--json`) output.
pub fn render_text(result: &WalkResult) -> String {
    let mut out: Vec<String> = Vec::new();

    if let Some(nf) = &result.not_found {
        let suffix = if nf.matches.is_empty() {
            " No search matches either.".to_string()
        } else {
            " Search matches:".to_string()
        };
        out.push(format!("\"{}\" not found by get.{suffix}", nf.target));
        for m in &nf.matches {
            out.push(format!(
                "  {} {} {} {}",
                m.form_id,
                m.record_type.as_deref().unwrap_or("?"),
                m.editor_id.as_deref().unwrap_or(""),
                m.name.as_deref().unwrap_or("")
            ));
        }
        return out.join("\n");
    }

    for node in &result.nodes {
        out.push(String::new());
        let marker = "▸".repeat(node.depth + 1);
        let name = node
            .name
            .as_deref()
            .map(|n| format!(" \"{n}\""))
            .unwrap_or_default();
        let via = node
            .via
            .as_deref()
            .map(|v| format!("  (via {v})"))
            .unwrap_or_default();
        out.push(format!(
            "{marker} {} {} {}{name}{via}",
            node.sig, node.formid, node.editor_id
        ));
        for l in render_digest(&node.digest) {
            out.push(format!("  {l}"));
        }
    }

    if let Some(refs) = &result.refs {
        out.push(String::new());
        out.push("reverse refs:".to_string());
        if refs.groups.is_empty() {
            out.push(
                "  NO reverse references — normal for script/VMAD quest rewards, vendor grants, \
                 and account-side (ATX) items; check the rescue lists before assuming junk."
                    .to_string(),
            );
        } else {
            for g in &refs.groups {
                let more = if g.count > g.sample.len() {
                    ", …"
                } else {
                    ""
                };
                let tag = g.tag.as_deref().unwrap_or("");
                out.push(format!(
                    "  {} ×{}: {}{more}{tag}",
                    g.record_type,
                    g.count,
                    g.sample.join(", ")
                ));
            }
            out.push(
                "  Reminder: the record graph cannot distinguish shipped from UNRELEASED content \
                 (P62/The Drifter looked obtainable). Confirm release status before rescuing."
                    .to_string(),
            );
        }
    }

    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyish_drops_trailing_zero_on_whole_floats() {
        assert_eq!(pyish(&Value::from(40.0)), "40");
        assert_eq!(pyish(&Value::from(12.5)), "12.5");
    }

    #[test]
    fn fmt_condition_operand_renders_resolved_glob_value() {
        let v = serde_json::json!({
            "formid": "0x1", "editor_id": "LGND_Threshold", "record_type": "GLOB",
            "resolved_value": 40.0,
        });
        assert_eq!(fmt_condition_operand(Some(&v)), "0x1<LGND_Threshold=40>");
    }

    #[test]
    fn fmt_condition_operand_renders_unresolved_ref_without_value() {
        let v = serde_json::json!({"formid": "0x1", "editor_id": "SomeKywd"});
        assert_eq!(fmt_condition_operand(Some(&v)), "0x1<SomeKywd>");
    }

    /// Moved from `chase.rs`'s own `#[cfg(test)]` block along with
    /// `summarize_effect` itself (see this module's doc comment) — this
    /// function is private and not reachable from an external `tests/`
    /// integration crate, so its test stays colocated (see esm/CLAUDE.md's
    /// testing conventions).
    #[test]
    fn summarize_effect_renders_base_effect_magnitude_and_conditions() {
        let effect = serde_json::json!({
            "Effect": {
                "Base Effect": {"formid": "0x500031", "editor_id": "TestSpellEffect"},
                "Effect Item Data": {"Magnitude": 25},
                "Conditions": {
                    "Conditions": [
                        {
                            "Function": "WornHasKeyword",
                            "Operator": "EqualTo",
                            "Comparison Value": 1.0,
                            "Parameter 1": {"formid": "0x500010", "editor_id": "if_tmp_TestTag"},
                        }
                    ]
                }
            }
        });
        let text = summarize_effect(&effect);
        assert!(text.contains("TestSpellEffect"));
        assert!(text.contains("Magnitude=25"));
        assert!(text.contains("Conditions: WornHasKeyword(if_tmp_TestTag) EqualTo 1"));
    }

    #[test]
    fn fmt_stub_renders_record_type_formid_editor_id() {
        let v = serde_json::json!({
            "formid": "0x00500020", "editor_id": "TestGrantedPerk", "record_type": "PERK",
        });
        assert_eq!(fmt_stub(&v), "PERK 0x00500020 TestGrantedPerk");
    }

    #[test]
    fn fmt_stub_omits_missing_fields_and_trims_trailing_space() {
        let v = serde_json::json!({"formid": "0x1"});
        assert_eq!(fmt_stub(&v), "? 0x1");
    }
}
