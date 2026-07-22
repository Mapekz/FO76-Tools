use super::*;

pub(super) fn take_first<'a>(
    by_sig: &mut HashMap<String, VecDeque<&'a OwnedSubrecord>>,
    sig: &str,
) -> Option<&'a OwnedSubrecord> {
    by_sig.get_mut(sig).and_then(|v| v.pop_front())
}

/// Like `take_first`, but only pops the front subrecord when its `doc_index`
/// falls inside `ctx`'s current scope.
///
/// `by_sig` is one global FIFO queue per signature across the *entire*
/// record, with no per-element partitioning. For a mandatory, always-present
/// member (e.g. an `Effect`'s `EFID`/`EFIT`) that FIFO order happens to align
/// correctly element-by-element. But an *optional* trailing sig-bearing
/// member (e.g. an ALCH/SPEL `Effect`'s `CVT0`/`MAGA`/`DURG`/`MAGG`/`CODV`)
/// is not guaranteed to be present on every element, so a bare `pop_front`
/// can steal a later element's subrecord when an earlier element lacks its
/// own. Scoping the pop to `[scope_min_doc_index, scope_max_doc_index)` (set
/// up per-element by `MemberDef::RArray`) keeps each element's optional
/// members bound to that element's own document span. Returns `None`
/// (without popping) when the front subrecord is out of scope, leaving it
/// queued for the correctly-scoped element to consume later.
pub(super) fn take_first_in_scope<'a>(
    by_sig: &mut HashMap<String, VecDeque<&'a OwnedSubrecord>>,
    sig: &str,
    ctx: &DecodeContext<'_>,
) -> Option<&'a OwnedSubrecord> {
    let front_idx = by_sig
        .get(sig)
        .and_then(|v| v.front())
        .map(|sr| sr.doc_index)?;
    if doc_index_in_present_signature_scope(ctx, front_idx) {
        by_sig.get_mut(sig).and_then(|v| v.pop_front())
    } else {
        None
    }
}

pub(super) fn doc_index_in_present_signature_scope(
    ctx: &DecodeContext<'_>,
    doc_index: usize,
) -> bool {
    if let Some(min) = ctx.scope_min_doc_index {
        if doc_index < min {
            return false;
        }
    }
    if let Some(max) = ctx.scope_max_doc_index {
        if doc_index >= max {
            return false;
        }
    }
    true
}

fn first_anchor_doc_index(
    by_sig: &HashMap<String, VecDeque<&OwnedSubrecord>>,
    member: &MemberDef,
) -> Option<usize> {
    if let Some(sig) = member.sig() {
        return by_sig
            .get(sig)
            .and_then(|subrecords| subrecords.front())
            .map(|subrecord| subrecord.doc_index);
    }
    match member {
        // Recurse into every sub-member and take the MINIMUM doc_index, not
        // just the first schema-order hit — mirrors the fix in
        // `rstruct_present_signature_scope` below and for the same reason:
        // a nested rstruct's schema-first sig-bearing member (e.g. ARMA
        // Male's `XFLG`, which is declared before `ENLT`/`ENLS`/`AUUV` in
        // the schema but shares its sig queue with a sibling group) is not
        // reliably the one with the lowest doc_index. Using `find_map` here
        // let a later sibling's anchor leak in as this rstruct's reported
        // "first" anchor, which then propagated outward as an artificially
        // high floor when a parent rstruct intersected its own scope with
        // this one's (see ARMA's Biped Model → Male/Female nesting).
        MemberDef::RStruct { members, .. } => members
            .iter()
            .filter_map(|m| first_anchor_doc_index(by_sig, m))
            .min(),
        // An RArray's own earliest anchor is its element's earliest anchor —
        // e.g. SCEN's "Start Scene" rstruct's `Scenes` rarray of
        // `[LCEP, INTT, SSPN, CITC, Conditions]` elements is anchored by
        // `LCEP`. Without this arm, an RArray member contributed no anchor
        // at all, so a later sibling member's anchor (or none) could win.
        MemberDef::RArray { element, .. } => first_anchor_doc_index(by_sig, element),
        _ => None,
    }
}

/// Bounds for `PresentSignature` inside repeated QUST alias bodies: from the
/// struct's opening anchor subrecord up to (but not including) the next `ALED`.
///
/// `scope_min` is the MINIMUM doc_index across *all* of `members`' own first
/// anchors, not just the first one hit in schema declaration order. Schema
/// order and document order are unrelated — e.g. ARMA's "Biped Model" rstruct
/// declares its `Male` sub-rstruct before `Female`, but a record can easily
/// have `Female`'s anchor (e.g. `MOD3`) appear earlier in the file than
/// anything in `Male`'s own group. Taking the first schema-order hit as
/// `scope_min` could exclude `Male`'s own genuinely-earlier subrecords from
/// its own scope.
pub(super) fn rstruct_present_signature_scope(
    by_sig: &HashMap<String, VecDeque<&OwnedSubrecord>>,
    members: &[MemberDef],
) -> (Option<usize>, Option<usize>) {
    let scope_min = members
        .iter()
        .filter_map(|member| first_anchor_doc_index(by_sig, member))
        .min();
    let scope_max = scope_min.and_then(|min| {
        by_sig.get("ALED").and_then(|subs| {
            subs.iter()
                .map(|sr| sr.doc_index)
                .filter(|&idx| idx > min)
                .min()
        })
    });
    (scope_min, scope_max)
}

/// Returns the first sig-bearing member's signature for `member`, if any.
/// Used by the `stop_before` RArray check to identify each element's anchor.
pub(super) fn anchor_sig(member: &MemberDef) -> Option<&str> {
    if let Some(sig) = member.sig() {
        return Some(sig);
    }
    match member {
        MemberDef::RStruct { members, .. } => members.iter().find_map(anchor_sig),
        _ => None,
    }
}

/// Returns the terminator signature when `element` is an `RStruct` whose
/// *last* member is a sig-bearing `Empty` — e.g. GMRW Reward's trailing
/// `ITME` "Reward End Marker". Used by `MemberDef::RArray` to partition
/// elements by terminator doc_index instead of by leading anchor, for
/// element shapes whose leading anchor is optional and can be absent on
/// every element in a given record.
///
/// Rejects a candidate whose sig is *also* used by an earlier member of the
/// same element: SCEN's "Action" rstruct, for instance, reuses `ANAM` for
/// both its leading "Type" field and its trailing "End Marker" — a single
/// global FIFO queue serves both roles, so by the time the trailing member's
/// turn comes around, the queue's *front* has already moved on to some other
/// action's leading "Type" popped in between, and treating it as this
/// element's own terminator massively over-restricts every other member's
/// scope. GMRW's `ITME`, by contrast, is unique to the terminator, so the
/// front of its queue always really is this element's own end.
pub(super) fn element_terminator_sig(element: &MemberDef) -> Option<&str> {
    let MemberDef::RStruct { members, .. } = element else {
        return None;
    };
    let (last, rest) = members.split_last()?;
    let MemberDef::Empty { sig: Some(sig), .. } = last else {
        return None;
    };
    if rest.iter().any(|member| member.contains_sig(sig)) {
        return None;
    }
    Some(sig.as_str())
}

pub(super) fn rarray_count(
    count: Option<&ArrayCount>,
    out: &Map<String, Value>,
    ctx: &DecodeContext<'_>,
) -> Option<usize> {
    match count {
        Some(ArrayCount::Fixed(n)) => Some(*n),
        Some(ArrayCount::CountPath(path)) => field_int_value(out, path)
            .or_else(|| {
                ctx.outer_struct
                    .as_ref()
                    .and_then(|o| field_int_value(o, path))
            })
            .map(|n| n as usize),
        // Prefix counts live inside a single payload-backed `Array`; repeated
        // subrecord arrays are bounded by sibling fields or document order.
        Some(ArrayCount::CountPrefix(_)) | Some(ArrayCount::FillToEnd) | None => None,
    }
}

/// Returns `true` when at least one `stop_before` sig has a lower `doc_index`
/// than the next occurrence of `anchor_sig` in `by_sig`. When this is true,
/// the calling RArray should halt iteration.
pub(super) fn stop_before_check(
    by_sig: &HashMap<String, VecDeque<&OwnedSubrecord>>,
    anchor: &str,
    stop_before: &[String],
) -> bool {
    let anchor_idx = match by_sig.get(anchor).and_then(|v| v.front()) {
        Some(sr) => sr.doc_index,
        None => return true, // nothing left to consume
    };
    stop_before.iter().any(|sig| {
        by_sig
            .get(sig.as_str())
            .and_then(|v| v.front())
            .is_some_and(|sr| sr.doc_index < anchor_idx)
    })
}

pub(super) fn take_all<'a>(
    by_sig: &mut HashMap<String, VecDeque<&'a OwnedSubrecord>>,
    sig: &str,
) -> Vec<&'a OwnedSubrecord> {
    by_sig
        .remove(sig)
        .map(|d| d.into_iter().collect())
        .unwrap_or_default()
}
