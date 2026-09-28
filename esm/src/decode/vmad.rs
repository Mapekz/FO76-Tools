use super::node::{Node, RawReason};
use super::*;

/// The `_raw` fallback (reason "VMAD truncated") for VMAD data
/// that ends early.
fn vmad_truncated(rest: &[u8]) -> Node {
    Node::raw(Some(rest), RawReason::Malformed("VMAD truncated".into()))
}

/// Decode a VMAD (Papyrus scripts) subrecord into its boundary JSON.
pub fn decode_vmad(ctx: &DecodeContext<'_>, data: &[u8]) -> Value {
    vmad_node(ctx, data).into_json(ctx)
}

/// Decode a VMAD (Papyrus scripts) subrecord into a structured value.
///
/// VMAD stores Papyrus script attachments with properties in a compact binary format.
/// Never panics on truncated or malformed input — returns a raw hex fallback instead.
pub(super) fn vmad_node(ctx: &DecodeContext<'_>, data: &[u8]) -> Node {
    let mut pos = 0usize;

    macro_rules! need {
        ($n:expr) => {
            if pos + $n > data.len() {
                return vmad_truncated(&data[pos..]);
            }
        };
    }

    macro_rules! read_u16 {
        () => {{
            need!(2);
            let v = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            v
        }};
    }

    macro_rules! read_wstring {
        () => {{
            let len = read_u16!() as usize;
            need!(len);
            let s = String::from_utf8_lossy(&data[pos..pos + len]).into_owned();
            pos += len;
            s
        }};
    }

    let version = read_u16!();
    let obj_format = read_u16!();
    let script_count = read_u16!();

    let mut scripts = Vec::new();
    for _ in 0..script_count {
        let name = read_wstring!();
        need!(1);
        let status = data[pos];
        pos += 1;
        let prop_count = read_u16!();
        let mut props = Vec::new();
        for _ in 0..prop_count {
            let prop_name = read_wstring!();
            need!(2);
            let prop_type = data[pos];
            pos += 1;
            let _prop_status = data[pos];
            pos += 1;
            let value = decode_vmad_property(ctx, data, &mut pos, prop_type, obj_format);
            props.push(Node::obj([
                ("name", Node::str(prop_name)),
                ("type", Node::int(prop_type)),
                ("value", value),
            ]));
        }
        scripts.push(Node::obj([
            ("name", Node::str(name)),
            ("status", Node::int(status)),
            ("properties", Node::Array(props)),
        ]));
    }

    Node::obj([
        ("version", Node::int(version)),
        ("scripts", Node::Array(scripts)),
    ])
}

/// Decode a `wbVMADFragmentedQUST` VMAD subrecord.
///
/// Extends the flat `decode_vmad` output with the `wbVMADFragmentedQUST`-specific
/// tail: a **Script Fragments** struct (extra bind data version, fragment count,
/// script name + optional script data, then N quest-stage fragments) followed by
/// an **Aliases** array (each alias carries a FormID/alias-ID, format version,
/// and its own script entries).
///
/// On any bounds-check failure the function returns the same `{"_raw": true,
/// "reason": "VMAD truncated", ...}` sentinel as `decode_vmad`, so callers can
/// treat both uniformly.
pub(super) fn decode_vmad_qust(ctx: &DecodeContext<'_>, data: &[u8]) -> Node {
    let mut pos = 0usize;

    macro_rules! need {
        ($n:expr) => {
            if pos + $n > data.len() {
                return vmad_truncated(&data[pos..]);
            }
        };
    }
    macro_rules! read_u16 {
        () => {{
            need!(2);
            let v = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            v
        }};
    }
    macro_rules! read_u32 {
        () => {{
            need!(4);
            let v = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            v
        }};
    }
    macro_rules! read_wstring {
        () => {{
            let len = read_u16!() as usize;
            need!(len);
            let s = String::from_utf8_lossy(&data[pos..pos + len]).into_owned();
            pos += len;
            s
        }};
    }

    // ── Header + scripts (same layout as the flat decoder) ───────────────────
    let version = read_u16!();
    let obj_format = read_u16!();
    let script_count = read_u16!();
    let mut scripts = Vec::new();
    for _ in 0..script_count {
        let name = read_wstring!();
        need!(1);
        let status = data[pos];
        pos += 1;
        let prop_count = read_u16!();
        let mut props = Vec::new();
        for _ in 0..prop_count {
            let prop_name = read_wstring!();
            need!(2);
            let prop_type = data[pos];
            pos += 1;
            let _prop_status = data[pos];
            pos += 1;
            let value = decode_vmad_property(ctx, data, &mut pos, prop_type, obj_format);
            props.push(Node::obj([
                ("name", Node::str(prop_name)),
                ("type", Node::int(prop_type)),
                ("value", value),
            ]));
        }
        scripts.push(Node::obj([
            ("name", Node::str(name)),
            ("status", Node::int(status)),
            ("properties", Node::Array(props)),
        ]));
    }

    // ── Script Fragments (wbVMADFragmentedQUST tail) ─────────────────────────
    // Some QUST records carry only the plain VMAD header without a script-fragments tail.
    // Treat end-of-data here as a successful no-fragments result.
    if pos >= data.len() {
        return Node::obj([
            ("version", Node::int(version)),
            ("scripts", Node::Array(scripts)),
        ]);
    }
    need!(1);
    let extra_bind_data_version = data[pos] as i8;
    pos += 1;
    let frag_count = read_u16!() as usize;
    let script_name = read_wstring!();
    // Script union: if script_name == "" then wbNull, else Script Data
    let script_data = if !script_name.is_empty() {
        need!(3); // flags u8 + prop_count u16
        let flags = data[pos];
        pos += 1;
        let pc = read_u16!() as usize;
        let mut props = Vec::new();
        for _ in 0..pc {
            let pn = read_wstring!();
            need!(2);
            let pt = data[pos];
            pos += 1;
            let _ps = data[pos];
            pos += 1;
            let val = decode_vmad_property(ctx, data, &mut pos, pt, obj_format);
            props.push(Node::obj([
                ("name", Node::str(pn)),
                ("type", Node::int(pt)),
                ("value", val),
            ]));
        }
        Node::obj([
            ("flags", Node::int(flags)),
            ("properties", Node::Array(props)),
        ])
    } else {
        Node::Null
    };
    let mut fragments = Vec::new();
    for _ in 0..frag_count {
        let quest_stage = read_u32!();
        let quest_stage_index = read_u32!();
        need!(1);
        pos += 1; // unknown byte
        let frag_script_name = read_wstring!();
        let fragment_name = read_wstring!();
        fragments.push(Node::obj([
            ("quest_stage", Node::int(quest_stage)),
            ("quest_stage_index", Node::int(quest_stage_index)),
            ("script_name", Node::Str(frag_script_name)),
            ("fragment_name", Node::Str(fragment_name)),
        ]));
    }
    let script_fragments = Node::obj([
        (
            "extra_bind_data_version",
            Node::int(extra_bind_data_version),
        ),
        ("script_name", Node::Str(script_name)),
        ("script_data", script_data),
        ("fragments", Node::Array(fragments)),
    ]);

    // ── Aliases (wbArrayS, u16-prefixed) ─────────────────────────────────────
    let alias_count = read_u16!() as usize;
    let mut aliases = Vec::new();
    for _ in 0..alias_count {
        // ScriptPropertyObject, per xEdit ground truth (wbDefinitionsFO76.pas
        // `wbScriptPropertyObject`, the same union `decode_vmad_property`'s
        // `base_type == 1` arm reads): objFormat == 1 selects "Object v1"
        // (FormID, Alias, Unused — FormID first); anything else, including the
        // objFormat == 2 SeventySix.esm actually carries, selects "Object v2"
        // (Unused, Alias, FormID — FormID last). Either layout is 8 bytes.
        //
        // `Alias` is itS16: -1 means "None" (a script attached to the quest
        // itself rather than to one of its aliases), so it cannot be widened
        // to an unsigned type. Reading `Unused` as the alias id — which this
        // parser did until the layouts were reconciled — reports every alias
        // as id 0, which erases the only field that tells two of a quest's
        // aliases apart and makes `array_diff` fall back to an unkeyed diff
        // that reprints every alias whenever one of them changes.
        let (alias_id, form_id) = if obj_format == 1 {
            need!(8);
            let f = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            let a = i16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 4; // Alias (2) + Unused (2)
            (a, f)
        } else {
            need!(8);
            let a = i16::from_le_bytes([data[pos + 2], data[pos + 3]]);
            let f =
                u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
            pos += 8;
            (a, f)
        };
        need!(4);
        let _version = i16::from_le_bytes([data[pos], data[pos + 1]]);
        pos += 2;
        let alias_obj_format = u16::from_le_bytes([data[pos], data[pos + 1]]);
        pos += 2;
        // Alias Scripts: u16-prefixed array of script entries
        let alias_script_count = read_u16!() as usize;
        let mut alias_scripts = Vec::new();
        for _ in 0..alias_script_count {
            let name = read_wstring!();
            need!(1);
            let status = data[pos];
            pos += 1;
            let pc = read_u16!() as usize;
            let mut props = Vec::new();
            for _ in 0..pc {
                let pn = read_wstring!();
                need!(2);
                let pt = data[pos];
                pos += 1;
                let _ps = data[pos];
                pos += 1;
                let val = decode_vmad_property(ctx, data, &mut pos, pt, alias_obj_format);
                props.push(Node::obj([
                    ("name", Node::str(pn)),
                    ("type", Node::int(pt)),
                    ("value", val),
                ]));
            }
            alias_scripts.push(Node::obj([
                ("name", Node::str(name)),
                ("status", Node::int(status)),
                ("properties", Node::Array(props)),
            ]));
        }
        aliases.push(Node::obj([
            ("alias_id", Node::int(alias_id)),
            (
                "form_id",
                Node::FormId {
                    id: FormId::new(form_id),
                    curve: false,
                },
            ),
            ("alias_scripts", Node::Array(alias_scripts)),
        ]));
    }

    Node::obj([
        ("version", Node::int(version)),
        ("scripts", Node::Array(scripts)),
        ("script_fragments", script_fragments),
        ("aliases", Node::Array(aliases)),
    ])
}

/// Parse the common VMAD header + scripts section, returning `(version, obj_format, scripts, pos)`
/// on success or a truncation `Value` on failure.
fn vmad_parse_header(
    ctx: &DecodeContext<'_>,
    data: &[u8],
) -> Result<(u16, u16, Vec<Node>, usize), Node> {
    let mut pos = 0usize;
    macro_rules! need {
        ($n:expr) => {
            if pos + $n > data.len() {
                return Err(vmad_truncated(&data[pos..]));
            }
        };
    }
    macro_rules! read_u16 {
        () => {{
            need!(2);
            let v = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            v
        }};
    }
    macro_rules! read_wstring {
        () => {{
            let len = read_u16!() as usize;
            need!(len);
            let s = String::from_utf8_lossy(&data[pos..pos + len]).into_owned();
            pos += len;
            s
        }};
    }
    let version = read_u16!();
    let obj_format = read_u16!();
    let script_count = read_u16!();
    let mut scripts = Vec::new();
    for _ in 0..script_count {
        let name = read_wstring!();
        need!(1);
        let status = data[pos];
        pos += 1;
        let prop_count = read_u16!() as usize;
        let mut props = Vec::new();
        for _ in 0..prop_count {
            let pn = read_wstring!();
            need!(2);
            let pt = data[pos];
            pos += 1;
            let _ps = data[pos];
            pos += 1;
            let val = decode_vmad_property(ctx, data, &mut pos, pt, obj_format);
            props.push(Node::obj([
                ("name", Node::str(pn)),
                ("type", Node::int(pt)),
                ("value", val),
            ]));
        }
        scripts.push(Node::obj([
            ("name", Node::str(name)),
            ("status", Node::int(status)),
            ("properties", Node::Array(props)),
        ]));
    }
    Ok((version, obj_format, scripts, pos))
}

/// Read a single script entry (name + status + props) from `data[*pos..]`.
/// Returns None on truncation; advances `*pos` on success.
fn vmad_read_script_entry(
    ctx: &DecodeContext<'_>,
    data: &[u8],
    pos: &mut usize,
    obj_format: u16,
) -> Option<Node> {
    fn read_wstr(data: &[u8], pos: &mut usize) -> Option<String> {
        if *pos + 2 > data.len() {
            return None;
        }
        let len = u16::from_le_bytes([data[*pos], data[*pos + 1]]) as usize;
        *pos += 2;
        if *pos + len > data.len() {
            return None;
        }
        let s = String::from_utf8_lossy(&data[*pos..*pos + len]).into_owned();
        *pos += len;
        Some(s)
    }
    let name = read_wstr(data, pos)?;
    if *pos >= data.len() {
        return None;
    }
    let status = data[*pos];
    *pos += 1;
    if *pos + 2 > data.len() {
        return None;
    }
    let pc = u16::from_le_bytes([data[*pos], data[*pos + 1]]) as usize;
    *pos += 2;
    let mut props = Vec::new();
    for _ in 0..pc {
        let pn = read_wstr(data, pos)?;
        if *pos + 2 > data.len() {
            return None;
        }
        let pt = data[*pos];
        *pos += 1;
        let _ps = data[*pos];
        *pos += 1;
        let val = decode_vmad_property(ctx, data, pos, pt, obj_format);
        props.push(Node::obj([
            ("name", Node::str(pn)),
            ("type", Node::int(pt)),
            ("value", val),
        ]));
    }
    Some(Node::obj([
        ("name", Node::str(name)),
        ("status", Node::int(status)),
        ("properties", Node::Array(props)),
    ]))
}

/// Shared inner decoder for INFO/PACK/SCEN Script Fragments section.
/// `flag_mask` controls how many bits of the flags byte map to fragments:
/// 0x03 for INFO/SCEN (OnBegin|OnEnd), 0x07 for PACK (OnBegin|OnEnd|OnChange).
/// Returns (flags, script_entry_value, fragments_vec, pos) or a truncation error.
fn vmad_read_flags_fragments(
    ctx: &DecodeContext<'_>,
    data: &[u8],
    pos: &mut usize,
    obj_format: u16,
    flag_mask: u8,
) -> Result<(u8, Node, Vec<Node>), Node> {
    macro_rules! trunc {
        () => {
            return Err(vmad_truncated(&data[*pos..]))
        };
    }
    macro_rules! read_wstring {
        () => {{
            if *pos + 2 > data.len() {
                trunc!();
            }
            let len = u16::from_le_bytes([data[*pos], data[*pos + 1]]) as usize;
            *pos += 2;
            if *pos + len > data.len() {
                trunc!();
            }
            let s = String::from_utf8_lossy(&data[*pos..*pos + len]).into_owned();
            *pos += len;
            s
        }};
    }
    if *pos >= data.len() {
        trunc!();
    }
    let extra_bind_data_version = data[*pos] as i8;
    *pos += 1;
    if *pos >= data.len() {
        trunc!();
    }
    let flags = data[*pos];
    *pos += 1;
    let frag_count = (flags & flag_mask).count_ones() as usize;
    let script_entry = vmad_read_script_entry(ctx, data, pos, obj_format)
        .ok_or_else(|| vmad_truncated(&data[*pos..]))?;
    let mut fragments = Vec::new();
    for _ in 0..frag_count {
        if *pos >= data.len() {
            trunc!();
        }
        let _unknown = data[*pos];
        *pos += 1;
        let script_name = read_wstring!();
        let fragment_name = read_wstring!();
        fragments.push(Node::obj([
            (
                "extra_bind_data_version",
                Node::int(extra_bind_data_version),
            ),
            ("script_name", Node::Str(script_name)),
            ("fragment_name", Node::Str(fragment_name)),
        ]));
    }
    Ok((flags, script_entry, fragments))
}

/// Decode a fragmented VMAD for INFO records (wbVMADFragmentedINFO).
/// Script Fragments: extra_bind_data_version(s8) + flags(u8) + script_entry + fragments
/// Fragment count = popcount(flags & 0x03) — OnBegin (bit 0) and OnEnd (bit 1).
///
/// Some INFO records have scripts but no script-fragments tail (they use plain VMAD
/// layout even though INFO is classified as a fragmented type). When data ends right
/// after the header, return a successful result with no script_fragments key.
pub(super) fn decode_vmad_info(ctx: &DecodeContext<'_>, data: &[u8]) -> Node {
    let (version, obj_format, scripts, mut pos) = match vmad_parse_header(ctx, data) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if pos >= data.len() {
        return Node::obj([
            ("version", Node::int(version)),
            ("scripts", Node::Array(scripts)),
        ]);
    }
    match vmad_read_flags_fragments(ctx, data, &mut pos, obj_format, 0x03) {
        Err(e) => e,
        Ok((flags, script_entry, fragments)) => Node::obj([
            ("version", Node::int(version)),
            ("scripts", Node::Array(scripts)),
            (
                "script_fragments",
                Node::obj([
                    ("flags", Node::int(flags)),
                    ("script_entry", script_entry),
                    ("fragments", Node::Array(fragments)),
                ]),
            ),
        ]),
    }
}

/// Decode a fragmented VMAD for PACK records (wbVMADFragmentedPACK).
/// Script Fragments: extra_bind_data_version(s8) + flags(u8) + script_entry + fragments
/// Fragment count = popcount(flags & 0x07) — OnBegin, OnEnd, OnChange (bits 0-2).
///
/// Like INFO, some PACK records carry only the plain VMAD header without a
/// script-fragments tail. Return a no-fragments result when data ends after the header.
pub(super) fn decode_vmad_pack(ctx: &DecodeContext<'_>, data: &[u8]) -> Node {
    let (version, obj_format, scripts, mut pos) = match vmad_parse_header(ctx, data) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if pos >= data.len() {
        return Node::obj([
            ("version", Node::int(version)),
            ("scripts", Node::Array(scripts)),
        ]);
    }
    match vmad_read_flags_fragments(ctx, data, &mut pos, obj_format, 0x07) {
        Err(e) => e,
        Ok((flags, script_entry, fragments)) => Node::obj([
            ("version", Node::int(version)),
            ("scripts", Node::Array(scripts)),
            (
                "script_fragments",
                Node::obj([
                    ("flags", Node::int(flags)),
                    ("script_entry", script_entry),
                    ("fragments", Node::Array(fragments)),
                ]),
            ),
        ]),
    }
}

/// Decode a fragmented VMAD for PERK records (wbVMADFragmentedPERK).
/// Script Fragments: extra_bind_data_version(s8) + script_entry + u16-count fragments
/// Each fragment: fragment_index(u32) + unknown(1) + script_name(wstring) + fragment_name(wstring)
/// Followed by trailing unknown bytes (wbUnknown — consumed but not decoded).
///
/// Some PERK records carry only the plain VMAD header without a script-fragments tail.
/// Return a no-fragments result when data ends after the header.
pub(super) fn decode_vmad_perk(ctx: &DecodeContext<'_>, data: &[u8]) -> Node {
    let (version, obj_format, scripts, mut pos) = match vmad_parse_header(ctx, data) {
        Ok(v) => v,
        Err(e) => return e,
    };
    macro_rules! trunc {
        () => {
            return vmad_truncated(&data[pos..])
        };
    }
    macro_rules! read_u16 {
        () => {{
            if pos + 2 > data.len() {
                trunc!();
            }
            let v = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            v
        }};
    }
    macro_rules! read_u32 {
        () => {{
            if pos + 4 > data.len() {
                trunc!();
            }
            let v = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            v
        }};
    }
    macro_rules! read_wstring {
        () => {{
            let len = read_u16!() as usize;
            if pos + len > data.len() {
                trunc!();
            }
            let s = String::from_utf8_lossy(&data[pos..pos + len]).into_owned();
            pos += len;
            s
        }};
    }
    if pos >= data.len() {
        return Node::obj([
            ("version", Node::int(version)),
            ("scripts", Node::Array(scripts)),
        ]);
    }
    let extra_bind_data_version = data[pos] as i8;
    pos += 1;
    let script_entry = match vmad_read_script_entry(ctx, data, &mut pos, obj_format) {
        Some(v) => v,
        None => trunc!(),
    };
    let frag_count = read_u16!() as usize;
    let mut fragments = Vec::new();
    for _ in 0..frag_count {
        let fragment_index = read_u32!();
        if pos >= data.len() {
            trunc!();
        }
        let _unknown = data[pos];
        pos += 1;
        let script_name = read_wstring!();
        let fragment_name = read_wstring!();
        fragments.push(Node::obj([
            ("fragment_index", Node::int(fragment_index)),
            ("script_name", Node::Str(script_name)),
            ("fragment_name", Node::Str(fragment_name)),
        ]));
    }
    Node::obj([
        ("version", Node::int(version)),
        ("scripts", Node::Array(scripts)),
        (
            "script_fragments",
            Node::obj([
                (
                    "extra_bind_data_version",
                    Node::int(extra_bind_data_version),
                ),
                ("script_entry", script_entry),
                ("fragments", Node::Array(fragments)),
            ]),
        ),
    ])
}

/// Decode a fragmented VMAD for SCEN records (wbVMADFragmentedSCEN).
/// Script Fragments: extra_bind_data_version(s8) + flags(u8) + script_entry + fragments + phase_fragments
/// Fragment count = popcount(flags & 0x03) — OnBegin (bit 1) and OnEnd (bit 2 in Pascal, but flags byte bits 0-1).
/// Phase fragments: u16-count-prefixed array; each = phase_flag(u8) + phase_index(u32) + unknown(1) + script_name + fragment_name.
pub(super) fn decode_vmad_scen(ctx: &DecodeContext<'_>, data: &[u8]) -> Node {
    let (version, obj_format, scripts, mut pos) = match vmad_parse_header(ctx, data) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // Some SCEN records carry only the plain VMAD header without a script-fragments tail.
    if pos >= data.len() {
        return Node::obj([
            ("version", Node::int(version)),
            ("scripts", Node::Array(scripts)),
        ]);
    }
    let (flags, script_entry, fragments) =
        match vmad_read_flags_fragments(ctx, data, &mut pos, obj_format, 0x03) {
            Ok(v) => v,
            Err(e) => return e,
        };
    macro_rules! trunc {
        () => {
            return vmad_truncated(&data[pos..])
        };
    }
    macro_rules! read_u16 {
        () => {{
            if pos + 2 > data.len() {
                trunc!();
            }
            let v = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            v
        }};
    }
    macro_rules! read_u32 {
        () => {{
            if pos + 4 > data.len() {
                trunc!();
            }
            let v = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            v
        }};
    }
    macro_rules! read_wstring {
        () => {{
            let len = read_u16!() as usize;
            if pos + len > data.len() {
                trunc!();
            }
            let s = String::from_utf8_lossy(&data[pos..pos + len]).into_owned();
            pos += len;
            s
        }};
    }
    let phase_frag_count = read_u16!() as usize;
    let mut phase_fragments = Vec::new();
    for _ in 0..phase_frag_count {
        if pos >= data.len() {
            trunc!();
        }
        let phase_flag = data[pos];
        pos += 1;
        let phase_index = read_u32!();
        if pos >= data.len() {
            trunc!();
        }
        let _unknown = data[pos];
        pos += 1;
        let script_name = read_wstring!();
        let fragment_name = read_wstring!();
        phase_fragments.push(Node::obj([
            ("phase_flag", Node::int(phase_flag)),
            ("phase_index", Node::int(phase_index)),
            ("script_name", Node::Str(script_name)),
            ("fragment_name", Node::Str(fragment_name)),
        ]));
    }
    Node::obj([
        ("version", Node::int(version)),
        ("scripts", Node::Array(scripts)),
        (
            "script_fragments",
            Node::obj([
                ("flags", Node::int(flags)),
                ("script_entry", script_entry),
                ("fragments", Node::Array(fragments)),
                ("phase_fragments", Node::Array(phase_fragments)),
            ]),
        ),
    ])
}

fn decode_vmad_property(
    ctx: &DecodeContext<'_>,
    data: &[u8],
    pos: &mut usize,
    prop_type: u8,
    obj_format: u16,
) -> Node {
    fn read_vmad_wstring(data: &[u8], pos: &mut usize) -> Option<String> {
        if *pos + 2 > data.len() {
            return None;
        }
        let len = u16::from_le_bytes([data[*pos], data[*pos + 1]]) as usize;
        *pos += 2;
        if *pos + len > data.len() {
            return None;
        }
        let s = String::from_utf8_lossy(&data[*pos..*pos + len]).into_owned();
        *pos += len;
        Some(s)
    }

    // Nested `fn` items don't capture the enclosing function's variables, so
    // `ctx` must be threaded through explicitly rather than closed over.
    fn decode_vmad_struct(
        ctx: &DecodeContext<'_>,
        data: &[u8],
        pos: &mut usize,
        obj_format: u16,
    ) -> Node {
        if *pos + 4 > data.len() {
            return Node::Null;
        }
        let count = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]])
            as usize;
        *pos += 4;
        let mut members = Vec::with_capacity(count.min(256));
        for _ in 0..count {
            let Some(name) = read_vmad_wstring(data, pos) else {
                break;
            };
            if *pos >= data.len() {
                break;
            }
            let member_type = data[*pos];
            *pos += 1;
            if *pos >= data.len() {
                break;
            }
            let _member_status = data[*pos];
            *pos += 1;
            let before = *pos;
            let value = decode_vmad_property(ctx, data, pos, member_type, obj_format);
            // Type 0 (None) is zero-width — pos not advancing is correct, not a stall.
            if member_type != 0 && *pos == before {
                break;
            }
            members.push(Node::obj([
                ("name", Node::str(name)),
                ("type", Node::int(member_type)),
                ("value", value),
            ]));
        }
        Node::Array(members)
    }

    fn read_scalar(
        ctx: &DecodeContext<'_>,
        data: &[u8],
        pos: &mut usize,
        base_type: u8,
        obj_format: u16,
    ) -> Node {
        match base_type {
            1 => {
                // Scripted object: always 8 bytes (FormID + Alias + Unused).
                if *pos + 8 > data.len() {
                    return Node::Null;
                }
                // xEdit ground truth (wbDefinitionsFO76.pas `wbScriptPropertyObject`
                // + `wbGetScriptObjFormat`): objFormat == 1 selects "Object v1"
                // (FormID, Alias, Unused — FormID first); anything else (incl. the
                // common objFormat == 2) selects "Object v2" (Unused, Alias, FormID
                // — FormID last).
                let form_off = if obj_format == 1 { 0 } else { 4 };
                let form_id = u32::from_le_bytes([
                    data[*pos + form_off],
                    data[*pos + form_off + 1],
                    data[*pos + form_off + 2],
                    data[*pos + form_off + 3],
                ]);
                *pos += 8;
                Node::FormId {
                    id: FormId::new(form_id),
                    curve: false,
                }
            }
            2 => {
                if *pos + 2 > data.len() {
                    return Node::Null;
                }
                let len = u16::from_le_bytes([data[*pos], data[*pos + 1]]) as usize;
                *pos += 2;
                if *pos + len > data.len() {
                    return Node::Null;
                }
                let s = String::from_utf8_lossy(&data[*pos..*pos + len]).into_owned();
                *pos += len;
                Node::Str(s)
            }
            3 => {
                if *pos + 4 > data.len() {
                    return Node::Null;
                }
                let v = i32::from_le_bytes([
                    data[*pos],
                    data[*pos + 1],
                    data[*pos + 2],
                    data[*pos + 3],
                ]);
                *pos += 4;
                Node::from(v)
            }
            4 => {
                if *pos + 4 > data.len() {
                    return Node::Null;
                }
                let v = f32::from_le_bytes([
                    data[*pos],
                    data[*pos + 1],
                    data[*pos + 2],
                    data[*pos + 3],
                ]);
                *pos += 4;
                Node::Float(v)
            }
            5 => {
                if *pos >= data.len() {
                    return Node::Null;
                }
                let v = data[*pos] != 0;
                *pos += 1;
                Node::from(v)
            }
            // Type 6 = Variable: 1-byte type discriminator, then value of that type.
            6 => {
                if *pos >= data.len() {
                    return Node::Null;
                }
                let sub_type = data[*pos];
                *pos += 1;
                read_scalar(ctx, data, pos, sub_type, obj_format)
            }
            // Type 7 = Struct: u32 member count, then N × (name + type + status + value).
            7 => decode_vmad_struct(ctx, data, pos, obj_format),
            _ => Node::obj([
                (markers::RAW, Node::Bool(true)),
                ("type", Node::int(base_type)),
            ]),
        }
    }

    // Type 0 = None: zero bytes, null value.
    if prop_type == 0 {
        return Node::Null;
    }

    if prop_type == 7 {
        return decode_vmad_struct(ctx, data, pos, obj_format);
    }

    if (11..=15).contains(&prop_type) {
        let base_type = prop_type - 10;
        if *pos + 4 > data.len() {
            return Node::Null;
        }
        let count = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]])
            as usize;
        *pos += 4;
        let mut items = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let before = *pos;
            items.push(read_scalar(ctx, data, pos, base_type, obj_format));
            if *pos == before {
                break;
            }
        }
        return Node::Array(items);
    }

    if prop_type == 17 {
        if *pos + 4 > data.len() {
            return Node::Null;
        }
        let count = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]])
            as usize;
        *pos += 4;
        let mut items = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let before = *pos;
            items.push(decode_vmad_struct(ctx, data, pos, obj_format));
            if *pos == before {
                break;
            }
        }
        return Node::Array(items);
    }

    if prop_type == 16 {
        if *pos + 4 > data.len() {
            return Node::Null;
        }
        let count = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]])
            as usize;
        *pos += 4;
        let mut items = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            if *pos >= data.len() {
                break;
            }
            let elem_type = data[*pos];
            *pos += 1;
            let before = *pos;
            items.push(decode_vmad_property(ctx, data, pos, elem_type, obj_format));
            if *pos == before {
                break;
            }
        }
        return Node::obj([
            ("_variable_array", Node::Bool(true)),
            ("items", Node::Array(items)),
        ]);
    }

    read_scalar(ctx, data, pos, prop_type, obj_format)
}
