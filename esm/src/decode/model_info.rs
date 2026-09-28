use super::node::{Fields, Node, RawReason};

/// Decodes the structured `wbModelInfo` (FO4/FO76 non-TES5) layout shared by every "Model
/// Information" subrecord (`MODT`, `DMDT`, `MO2T`..`MO5T`, `NAM2`, `NAM5`):
///
/// ```text
/// wbStruct('', [
///   wbArray('Counters', wbInteger(itU32), -1, ['Textures','Addon Nodes','SRGB','Materials']),
///   wbArray('Textures', wbStruct[File Hash: u32, Extension: char[4], Folder Hash: u32])
///     .SetCountPath('Counters\[0]'),
///   wbArray('Addon Nodes', wbInteger(itU32)).SetCountPath('Counters\[1]'),
///   wbArray('Materials', wbStruct[File Hash: u32, Extension: char[4], Folder Hash: u32])
///     .SetCountPath('Counters\[3]'),
/// ])
/// ```
/// (`wbDefinitionsCommon.pas` `wbModelInfo`, non-TES5 branch — `wbArray('Counters', ...)`
/// is itself count-prefixed, so the leading u32 is *how many* counters follow, not a counter
/// itself.) `Counters[2]` ("SRGB") gates no array of its own — it's a plain count value with
/// no associated bytes.
///
/// Because the layout isn't expressible in the static schema model (it's a self-describing
/// blob, not a signature-keyed struct), the extractor stubs every "Model Information" member
/// as a byte array, and this function is dispatched from the `MemberDef::Bytes` arm by field
/// name. An empty subrecord decodes to `null`. Falls back to a `_raw` value (reason: the
/// counters don't match the size) whenever the declared counters don't exactly account
/// for the subrecord's length — covering the TES5-style 2-counter layout,
/// corrupt data, and any record this heuristic doesn't actually fit — rather than panicking,
/// consistent with the decoder-must-never-panic invariant.
pub(super) fn decode_model_info(data: &[u8]) -> Node {
    fn raw(data: &[u8]) -> Node {
        Node::raw(
            Some(data),
            RawReason::Malformed("counters don't match the model information size".into()),
        )
    }
    fn read_u32(data: &[u8], off: usize) -> Option<u32> {
        data.get(off..off + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    // A file entry is { File Hash: u32, Extension: char[4], Folder Hash: u32 } — 12 bytes.
    fn read_file_entry(data: &[u8], off: usize) -> Node {
        let file_hash = read_u32(data, off).unwrap_or(0);
        let ext = data.get(off + 4..off + 8).unwrap_or(&[]);
        let folder_hash = read_u32(data, off + 8).unwrap_or(0);
        Node::obj([
            ("File Hash", Node::Str(format!("0x{:08X}", file_hash))),
            (
                "Extension",
                Node::str(String::from_utf8_lossy(ext).trim_end_matches('\0')),
            ),
            ("Folder Hash", Node::Str(format!("0x{:08X}", folder_hash))),
        ])
    }

    if data.is_empty() {
        // An empty subrecord: no model information, not a malformed one.
        return Node::Null;
    }
    let Some(num_counters) = read_u32(data, 0) else {
        return raw(data);
    };
    let num_counters = num_counters as usize;
    // Real layouts use 2 (TES5) or 4 (FO4/FO76) counters — bound generously but reject
    // anything that can't plausibly be this format.
    if !(1..=8).contains(&num_counters) {
        return raw(data);
    }

    let mut counters = Vec::with_capacity(num_counters);
    for i in 0..num_counters {
        match read_u32(data, 4 + i * 4) {
            Some(c) => counters.push(c as usize),
            None => return raw(data),
        }
    }

    let num_textures = counters.first().copied().unwrap_or(0);
    let num_addon_nodes = counters.get(1).copied().unwrap_or(0);
    let num_materials = counters.get(3).copied().unwrap_or(0);

    // Validate the total byte length up front, via checked arithmetic, before doing any
    // per-entry allocation — so a corrupt/huge count can't trigger an OOM abort, and once
    // this passes every read below is guaranteed in-bounds.
    let header_len = 4 + num_counters * 4;
    let total_len = (|| {
        header_len
            .checked_add(num_textures.checked_mul(12)?)?
            .checked_add(num_addon_nodes.checked_mul(4)?)?
            .checked_add(num_materials.checked_mul(12)?)
    })();
    if total_len != Some(data.len()) {
        return raw(data);
    }

    let counter_names = ["Textures", "Addon Nodes", "SRGB", "Materials"];
    let mut counters_obj = Fields::new();
    for (i, &c) in counters.iter().enumerate() {
        let name = counter_names.get(i).copied().unwrap_or("Unknown");
        counters_obj.insert(name.to_string(), Node::Int(c as i64));
    }

    let mut off = header_len;
    let textures: Vec<Node> = (0..num_textures)
        .map(|_| {
            let entry = read_file_entry(data, off);
            off += 12;
            entry
        })
        .collect();
    let addon_nodes: Vec<Node> = (0..num_addon_nodes)
        .map(|_| {
            let v = Node::int(read_u32(data, off).unwrap_or(0));
            off += 4;
            v
        })
        .collect();
    let materials: Vec<Node> = (0..num_materials)
        .map(|_| {
            let entry = read_file_entry(data, off);
            off += 12;
            entry
        })
        .collect();

    Node::obj([
        ("Counters", Node::Struct(counters_obj)),
        ("Textures", Node::Array(textures)),
        ("Addon Nodes", Node::Array(addon_nodes)),
        ("Materials", Node::Array(materials)),
    ])
}
