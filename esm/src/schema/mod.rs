use serde::Deserialize;
use serde_json::value::RawValue;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// The decoder's record definitions, keyed by signature.
///
/// Each definition stays raw JSON until first use: a query touches a handful
/// of record types, and parsing all ~180 definitions up front was most of
/// what opening a database cost. The embedded schema arrives already split
/// per record type, each definition parsed and validated by `build.rs`, so
/// an invalid one fails the build; a schema loaded from a file is parsed
/// and validated eagerly.
#[derive(Debug)]
pub struct Schema {
    records: HashMap<Cow<'static, str>, LazyRecord>,
}

#[derive(Debug)]
struct LazyRecord {
    raw: Cow<'static, str>,
    parsed: OnceLock<RecordDef>,
}

impl LazyRecord {
    fn new(raw: Cow<'static, str>) -> Self {
        LazyRecord {
            raw,
            parsed: OnceLock::new(),
        }
    }
}

impl LazyRecord {
    fn parse(&self, sig: &str) -> anyhow::Result<&RecordDef> {
        if let Some(def) = self.parsed.get() {
            return Ok(def);
        }
        let def: RecordDef = serde_json::from_str(&self.raw)
            .map_err(|e| anyhow::anyhow!("schema record {sig}: {e}"))?;
        def.validate(sig).map_err(anyhow::Error::msg)?;
        Ok(self.parsed.get_or_init(|| def))
    }
}

#[derive(Deserialize)]
struct RawSchema {
    records: HashMap<String, Box<RawValue>>,
}

mod defs;
pub use defs::*;

// `SCHEMA_DIGEST`: FNV-1a over the embedded `fo76.json`, `fo76.ctda.json` and
// `hardcoded_fo76.json`, computed by `build.rs`.
include!(concat!(env!("OUT_DIR"), "/schema_digest.rs"));

// `EMBEDDED_RECORDS`: `fo76.json`'s record definitions as raw JSON, one
// `(signature, definition)` pair per record type, split by `build.rs`.
include!(concat!(env!("OUT_DIR"), "/schema_records.rs"));

impl Schema {
    pub fn load_embedded() -> anyhow::Result<Self> {
        Ok(Schema {
            records: EMBEDDED_RECORDS
                .iter()
                .map(|&(sig, raw)| (Cow::Borrowed(sig), LazyRecord::new(Cow::Borrowed(raw))))
                .collect(),
        })
    }

    pub fn load_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text)
    }

    /// Parse and validate every record definition in `text`.
    pub fn from_json(text: &str) -> anyhow::Result<Self> {
        let raw: RawSchema = serde_json::from_str(text)?;
        let schema = Schema {
            records: raw
                .records
                .into_iter()
                .map(|(sig, raw)| {
                    let raw = Cow::Owned(raw.get().to_owned());
                    (Cow::Owned(sig), LazyRecord::new(raw))
                })
                .collect(),
        };
        for (sig, record) in &schema.records {
            record.parse(sig)?;
        }
        Ok(schema)
    }

    /// The definition for record type `sig`, parsed on first use.
    ///
    /// # Panics
    /// If the embedded schema holds a definition the decoder rejects — which
    /// `build.rs` makes a build failure, not a runtime one.
    pub fn record(&self, sig: &str) -> Option<&RecordDef> {
        let record = self.records.get(sig)?;
        Some(record.parse(sig).unwrap_or_else(|e| panic!("{e:#}")))
    }

    /// Every record type's signature and definition, parsing each on first use.
    pub fn records(&self) -> impl Iterator<Item = (&str, &RecordDef)> {
        self.records
            .keys()
            .filter_map(|sig| Some((sig.as_ref(), self.record(sig)?)))
    }
}

#[cfg(test)]
mod tests {
    use super::{RecordDef, Schema};

    fn one_array(count_path: &str) -> String {
        format!(
            r#"{{"records":{{"TEST":{{"name":"Test","members":[{{"kind":"array","name":"Things",
            "element":{{"kind":"integer","name":"Thing","width":"u8"}},
            "count":{{"count_path":{count_path}}}}}]}}}}}}"#
        )
    }

    /// Every record definition, through both the build-time split the
    /// binary embeds and a parse of the file itself.
    #[test]
    fn embedded_schema_parses_and_validates() {
        let text = include_str!("../../schema/fo76.json");
        let raw: serde_json::Value = serde_json::from_str(text).unwrap();
        let count = raw["records"].as_object().unwrap().len();
        assert_eq!(Schema::from_json(text).unwrap().records().count(), count);
        let embedded = Schema::load_embedded().unwrap();
        assert_eq!(embedded.records().count(), count);
        for (sig, def) in raw["records"].as_object().unwrap() {
            let parsed = serde_json::to_value(embedded.record(sig).unwrap()).unwrap();
            let reparsed: RecordDef = serde_json::from_value(def.clone()).unwrap();
            assert_eq!(parsed, serde_json::to_value(reparsed).unwrap(), "{sig}");
        }
    }

    /// One record with one member, `member`.
    fn one_member(member: &str) -> String {
        format!(r#"{{"records":{{"TEST":{{"name":"Test","members":[{member}]}}}}}}"#)
    }

    /// A union of two integer variants picked by `decider`.
    fn one_union(decider: &str) -> String {
        one_member(&format!(
            r#"{{"kind":"union","sig":"DATA","name":"U","decider":{decider},"variants":[
            {{"kind":"integer","name":"A","width":"u8"}},{{"kind":"integer","name":"B","width":"u8"}}]}}"#
        ))
    }

    #[test]
    fn rejects_an_unknown_key_but_not_a_comment() {
        let int = |extra: &str| {
            one_member(&format!(
                r#"{{"kind":"integer","sig":"DATA","name":"V","width":"u32"{extra}}}"#
            ))
        };
        Schema::from_json(&int(r#","_comment":"why""#)).unwrap();
        let err = Schema::from_json(&int(r#","from_verison":999"#)).unwrap_err();
        assert!(err.to_string().contains("from_verison"), "{err:#}");
    }

    #[test]
    fn rejects_a_version_gate_that_admits_nothing() {
        let gated = one_member(
            r#"{"kind":"integer","sig":"DATA","name":"V","width":"u8","from_version":90,"below_version":80}"#,
        );
        assert!(Schema::from_json(&gated).is_err());
    }

    #[test]
    fn a_union_decider_names_exactly_one_kind_and_only_its_keys() {
        Schema::from_json(&one_union(r#"{"field":"X","map":{"1":1}}"#)).unwrap();
        for bad in [
            r#"{"field":"X","byte_offset":0,"map":{}}"#,
            r#"{"feild":"X","map":{}}"#,
            r#"{"field":"X","mapp":{}}"#,
            r#"{"field":"X","width_bytes":2}"#,
            r#"{}"#,
        ] {
            assert!(Schema::from_json(&one_union(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_union_decider_picks_only_variants_the_union_has() {
        for bad in [
            r#"{"field":"X","map":{"1":2}}"#,
            r#"{"byte_offset":0,"map":{"x":0}}"#,
            r#"{"byte_offset":0,"width_bytes":3,"map":{}}"#,
            r#"{"form_version_thresholds":[10,20]}"#,
            r#"{"edid_prefix":{"ab":0}}"#,
        ] {
            assert!(Schema::from_json(&one_union(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn accepts_a_count_path_one_scope_up() {
        Schema::from_json(&one_array(r#"{"up":1,"path":["Counts","Things"]}"#)).unwrap();
    }

    #[test]
    fn rejects_a_count_path_the_decoder_cannot_resolve() {
        for bad in [
            r#"{"up":2,"path":["Count"]}"#,
            r#"{"up":0,"path":[]}"#,
            r#""..\\XCNT\\Count""#,
        ] {
            assert!(Schema::from_json(&one_array(bad)).is_err(), "{bad}");
        }
    }
}
