//! VMAD (Papyrus script attachment) subrecords.
//!
//! Every VMAD starts with the same header and script list; records with
//! script fragments (xEdit's `wbVMADFragmented*`) add a tail whose layout the
//! schema member names (`fragments`). The header, script list and fragment
//! tails read through one [`VmadCursor`]: any read past the end stops the
//! decode, and the value becomes a `_raw` "VMAD truncated" fallback holding
//! the bytes from where it stopped. Property values are more forgiving: a
//! property that runs short decodes to `null` (see [`decode_vmad_property`]).

use super::node::{Node, RawReason};
use super::*;
use crate::schema::VmadFragments;

/// The `_raw` fallback (reason "VMAD truncated") for VMAD data that ends early.
fn vmad_truncated(rest: &[u8]) -> Node {
    Node::raw(Some(rest), RawReason::Malformed("VMAD truncated".into()))
}

/// Decode a plain VMAD (Papyrus scripts) subrecord into its boundary JSON.
pub fn decode_vmad(ctx: &DecodeContext<'_>, data: &[u8]) -> Value {
    vmad_node(ctx, data, None).into_json(ctx)
}

/// Decode a VMAD subrecord whose fragment tail (if any) has layout
/// `fragments`. Never panics on truncated or malformed input.
pub(super) fn vmad_node(
    ctx: &DecodeContext<'_>,
    data: &[u8],
    fragments: Option<VmadFragments>,
) -> Node {
    let mut cur = VmadCursor { data, pos: 0 };
    match read_vmad(ctx, &mut cur, fragments) {
        Some(node) => node,
        None => vmad_truncated(&data[cur.pos.min(data.len())..]),
    }
}

/// A read position in a VMAD payload. Each read either consumes its bytes or
/// returns `None` without consuming them (a string's length prefix, once
/// read, stays consumed).
struct VmadCursor<'d> {
    data: &'d [u8],
    pos: usize,
}

impl VmadCursor<'_> {
    fn need(&self, n: usize) -> Option<()> {
        (self.pos + n <= self.data.len()).then_some(())
    }

    fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.need(N)?;
        let bytes = self.data[self.pos..self.pos + N].try_into().ok()?;
        self.pos += N;
        Some(bytes)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|[b]| b)
    }

    fn i8(&mut self) -> Option<i8> {
        self.u8().map(|b| b as i8)
    }

    fn u16(&mut self) -> Option<u16> {
        self.take().map(u16::from_le_bytes)
    }

    fn i16(&mut self) -> Option<i16> {
        self.take().map(i16::from_le_bytes)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_le_bytes)
    }

    /// A u16-length-prefixed string.
    fn wstring(&mut self) -> Option<String> {
        let len = self.u16()? as usize;
        self.need(len)?;
        let s = String::from_utf8_lossy(&self.data[self.pos..self.pos + len]).into_owned();
        self.pos += len;
        Some(s)
    }

    /// One property: name, type, status (ignored), value.
    fn property(&mut self, ctx: &DecodeContext<'_>, obj_format: u16) -> Option<Node> {
        let name = self.wstring()?;
        self.need(2)?;
        let prop_type = self.u8()?;
        let _status = self.u8()?;
        let value = decode_vmad_property(ctx, self.data, &mut self.pos, prop_type, obj_format);
        Some(Node::obj([
            ("name", Node::Str(name)),
            ("type", Node::int(prop_type)),
            ("value", value),
        ]))
    }

    /// `count` properties.
    fn properties(
        &mut self,
        ctx: &DecodeContext<'_>,
        count: usize,
        obj_format: u16,
    ) -> Option<Vec<Node>> {
        (0..count).map(|_| self.property(ctx, obj_format)).collect()
    }

    /// One script: name, status, u16-counted properties.
    fn script(&mut self, ctx: &DecodeContext<'_>, obj_format: u16) -> Option<Node> {
        let name = self.wstring()?;
        self.need(1)?;
        let status = self.u8()?;
        let count = self.u16()? as usize;
        let properties = self.properties(ctx, count, obj_format)?;
        Some(Node::obj([
            ("name", Node::Str(name)),
            ("status", Node::int(status)),
            ("properties", Node::Array(properties)),
        ]))
    }

    /// `count` scripts.
    fn scripts(
        &mut self,
        ctx: &DecodeContext<'_>,
        count: usize,
        obj_format: u16,
    ) -> Option<Vec<Node>> {
        (0..count).map(|_| self.script(ctx, obj_format)).collect()
    }
}

/// The header and scripts, then the fragment tail when the data continues.
fn read_vmad(
    ctx: &DecodeContext<'_>,
    cur: &mut VmadCursor<'_>,
    fragments: Option<VmadFragments>,
) -> Option<Node> {
    let version = cur.u16()?;
    let obj_format = cur.u16()?;
    let script_count = cur.u16()? as usize;
    let scripts = cur.scripts(ctx, script_count, obj_format)?;
    let mut out = Node::obj([
        ("version", Node::int(version)),
        ("scripts", Node::Array(scripts)),
    ]);
    // Records with a fragmented layout can still carry only the plain header.
    let Some(fragments) = fragments.filter(|_| !cur.at_end()) else {
        return Some(out);
    };
    let Node::Struct(fields) = &mut out else {
        unreachable!("Node::obj builds a struct");
    };
    match fragments {
        VmadFragments::Qust => {
            fields.insert(
                "script_fragments".into(),
                qust_fragments(ctx, cur, obj_format)?,
            );
            fields.insert(
                "aliases".into(),
                Node::Array(qust_aliases(ctx, cur, obj_format)?),
            );
        }
        VmadFragments::Info => {
            fields.insert(
                "script_fragments".into(),
                flag_fragments(ctx, cur, obj_format, 0x03, false)?,
            );
        }
        VmadFragments::Pack => {
            fields.insert(
                "script_fragments".into(),
                flag_fragments(ctx, cur, obj_format, 0x07, false)?,
            );
        }
        VmadFragments::Scen => {
            fields.insert(
                "script_fragments".into(),
                flag_fragments(ctx, cur, obj_format, 0x03, true)?,
            );
        }
        VmadFragments::Perk => {
            fields.insert(
                "script_fragments".into(),
                perk_fragments(ctx, cur, obj_format)?,
            );
        }
    }
    Some(out)
}

/// A fragment's script and fragment names.
fn fragment_names(cur: &mut VmadCursor<'_>) -> Option<(Node, Node)> {
    Some((Node::Str(cur.wstring()?), Node::Str(cur.wstring()?)))
}

/// `wbVMADFragmentedQUST`'s Script Fragments: extra bind data version,
/// fragment count, script name, optional script data, then quest-stage
/// fragments.
fn qust_fragments(
    ctx: &DecodeContext<'_>,
    cur: &mut VmadCursor<'_>,
    obj_format: u16,
) -> Option<Node> {
    cur.need(1)?;
    let extra_bind_data_version = cur.i8()?;
    let frag_count = cur.u16()? as usize;
    let script_name = cur.wstring()?;
    // Script union: an empty script name means no script data.
    let script_data = if script_name.is_empty() {
        Node::Null
    } else {
        cur.need(3)?; // flags u8 + prop_count u16
        let flags = cur.u8()?;
        let count = cur.u16()? as usize;
        let properties = cur.properties(ctx, count, obj_format)?;
        Node::obj([
            ("flags", Node::int(flags)),
            ("properties", Node::Array(properties)),
        ])
    };
    let fragments = (0..frag_count)
        .map(|_| {
            let quest_stage = cur.u32()?;
            let quest_stage_index = cur.u32()?;
            cur.u8()?; // unknown
            let (script_name, fragment_name) = fragment_names(cur)?;
            Some(Node::obj([
                ("quest_stage", Node::int(quest_stage)),
                ("quest_stage_index", Node::int(quest_stage_index)),
                ("script_name", script_name),
                ("fragment_name", fragment_name),
            ]))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Node::obj([
        (
            "extra_bind_data_version",
            Node::int(extra_bind_data_version),
        ),
        ("script_name", Node::Str(script_name)),
        ("script_data", script_data),
        ("fragments", Node::Array(fragments)),
    ]))
}

/// `wbVMADFragmentedQUST`'s u16-counted Aliases.
fn qust_aliases(
    ctx: &DecodeContext<'_>,
    cur: &mut VmadCursor<'_>,
    obj_format: u16,
) -> Option<Vec<Node>> {
    let alias_count = cur.u16()? as usize;
    (0..alias_count)
        .map(|_| {
            // ScriptPropertyObject, per xEdit (wbDefinitionsFO76.pas
            // `wbScriptPropertyObject`, the same union `decode_vmad_property`'s
            // object type reads): objFormat == 1 selects "Object v1" (FormID,
            // Alias, Unused — FormID first); anything else, including the
            // objFormat == 2 SeventySix.esm carries, selects "Object v2"
            // (Unused, Alias, FormID — FormID last). Either layout is 8 bytes.
            // `Alias` is itS16: -1 means "None" (a script attached to the
            // quest itself rather than to one of its aliases).
            cur.need(8)?;
            let (alias_id, form_id) = if obj_format == 1 {
                let form_id = cur.u32()?;
                let alias_id = cur.i16()?;
                cur.u16()?; // unused
                (alias_id, form_id)
            } else {
                cur.u16()?; // unused
                let alias_id = cur.i16()?;
                (alias_id, cur.u32()?)
            };
            cur.need(4)?;
            let _version = cur.i16()?;
            let alias_obj_format = cur.u16()?;
            let script_count = cur.u16()? as usize;
            let alias_scripts = cur.scripts(ctx, script_count, alias_obj_format)?;
            Some(Node::obj([
                ("alias_id", Node::int(alias_id)),
                (
                    "form_id",
                    Node::FormId {
                        id: FormId::new(form_id),
                        curve: false,
                    },
                ),
                ("alias_scripts", Node::Array(alias_scripts)),
            ]))
        })
        .collect()
}

/// The INFO/PACK/SCEN Script Fragments: extra bind data version, flags, one
/// script entry, then one fragment per set bit of `flags & flag_mask`
/// (OnBegin/OnEnd, plus OnChange for PACK); SCEN adds u16-counted phase
/// fragments.
fn flag_fragments(
    ctx: &DecodeContext<'_>,
    cur: &mut VmadCursor<'_>,
    obj_format: u16,
    flag_mask: u8,
    phases: bool,
) -> Option<Node> {
    let extra_bind_data_version = cur.i8()?;
    let flags = cur.u8()?;
    let script_entry = cur.script(ctx, obj_format)?;
    let fragments = (0..(flags & flag_mask).count_ones())
        .map(|_| {
            cur.u8()?; // unknown
            let (script_name, fragment_name) = fragment_names(cur)?;
            Some(Node::obj([
                (
                    "extra_bind_data_version",
                    Node::int(extra_bind_data_version),
                ),
                ("script_name", script_name),
                ("fragment_name", fragment_name),
            ]))
        })
        .collect::<Option<Vec<_>>>()?;
    let mut out = Node::obj([
        ("flags", Node::int(flags)),
        ("script_entry", script_entry),
        ("fragments", Node::Array(fragments)),
    ]);
    if phases {
        let count = cur.u16()? as usize;
        let phase_fragments = (0..count)
            .map(|_| {
                let phase_flag = cur.u8()?;
                let phase_index = cur.u32()?;
                cur.u8()?; // unknown
                let (script_name, fragment_name) = fragment_names(cur)?;
                Some(Node::obj([
                    ("phase_flag", Node::int(phase_flag)),
                    ("phase_index", Node::int(phase_index)),
                    ("script_name", script_name),
                    ("fragment_name", fragment_name),
                ]))
            })
            .collect::<Option<Vec<_>>>()?;
        if let Node::Struct(fields) = &mut out {
            fields.insert("phase_fragments".into(), Node::Array(phase_fragments));
        }
    }
    Some(out)
}

/// `wbVMADFragmentedPERK`'s Script Fragments (also TERM's): extra bind data
/// version, one script entry, then u16-counted fragments, each with a u32
/// fragment index. Trailing unknown bytes after the fragments are ignored.
fn perk_fragments(
    ctx: &DecodeContext<'_>,
    cur: &mut VmadCursor<'_>,
    obj_format: u16,
) -> Option<Node> {
    let extra_bind_data_version = cur.i8()?;
    let script_entry = cur.script(ctx, obj_format)?;
    let count = cur.u16()? as usize;
    let fragments = (0..count)
        .map(|_| {
            let fragment_index = cur.u32()?;
            cur.u8()?; // unknown
            let (script_name, fragment_name) = fragment_names(cur)?;
            Some(Node::obj([
                ("fragment_index", Node::int(fragment_index)),
                ("script_name", script_name),
                ("fragment_name", fragment_name),
            ]))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Node::obj([
        (
            "extra_bind_data_version",
            Node::int(extra_bind_data_version),
        ),
        ("script_entry", script_entry),
        ("fragments", Node::Array(fragments)),
    ]))
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
