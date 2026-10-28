//! A collection's columns, inferred from a sample of its documents (`$sample`, or the first
//! documents where that is refused): `_id` first as the key, then the top-level fields by how
//! many documents have them, then by name. A column's type lists the BSON types seen, most
//! frequent first (`int|string|null`); its kind follows the most frequent type other than null,
//! and it is nullable when a document holds null or lacks the field. Embedded documents and
//! arrays are one Json column each.

use std::collections::HashMap;

use ::mongodb::bson::Document;

use super::values::{kind_of, type_label};
use crate::sqlgen::native::mongo::ID;
use crate::types::schema::{ColumnMeta, ValueKind};

/// Documents sampled to infer the columns (the profile can change it).
pub const DEFAULT_SAMPLE_SIZE: u32 = 100;
/// The most a profile may ask for.
pub const MAX_SAMPLE_SIZE: u32 = 10_000;

/// What the sample says about one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldStats {
    pub name: String,
    /// Documents that have the field.
    pub present: usize,
    /// Type labels with the number of documents holding each, most frequent first, then by
    /// label.
    pub types: Vec<(&'static str, usize)>,
}

impl FieldStats {
    /// The type labels as a column type (`int|string`).
    pub fn data_type(&self) -> String {
        self.types
            .iter()
            .map(|(t, _)| *t)
            .collect::<Vec<_>>()
            .join("|")
    }

    /// The most frequent type other than null (null when that is all there is).
    pub fn dominant(&self) -> &'static str {
        self.types
            .iter()
            .map(|(t, _)| *t)
            .find(|t| *t != "null")
            .unwrap_or("null")
    }

    pub fn has_null(&self) -> bool {
        self.types.iter().any(|(t, _)| *t == "null")
    }
}

/// The fields of a sample, `_id` first, then by frequency and name.
pub fn infer(sample: &[Document]) -> Vec<FieldStats> {
    let mut by_name: HashMap<&str, (usize, HashMap<&'static str, usize>)> = HashMap::new();
    for d in sample {
        for (k, v) in d {
            let entry = by_name.entry(k.as_str()).or_default();
            entry.0 += 1;
            *entry.1.entry(type_label(v)).or_default() += 1;
        }
    }
    let mut fields: Vec<FieldStats> = by_name
        .into_iter()
        .map(|(name, (present, types))| {
            let mut types: Vec<(&'static str, usize)> = types.into_iter().collect();
            types.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            FieldStats {
                name: name.to_string(),
                present,
                types,
            }
        })
        .collect();
    fields.sort_by(|a, b| {
        (b.name == ID)
            .cmp(&(a.name == ID))
            .then(b.present.cmp(&a.present))
            .then(a.name.cmp(&b.name))
    });
    fields
}

/// The grid's columns from the sample's fields. `sampled` is the number of documents read; an
/// empty sample still has `_id` (an ObjectId, as the server would add).
pub fn columns(fields: &[FieldStats], sampled: usize) -> Vec<ColumnMeta> {
    let mut out: Vec<ColumnMeta> = Vec::with_capacity(fields.len() + 1);
    if sampled == 0 {
        out.push(column(ID, "objectId", ValueKind::Other, false, 1, None));
    }
    for f in fields {
        let ordinal = out.len() as u32 + 1;
        let nullable = f.name != ID && (f.has_null() || f.present < sampled);
        out.push(column(
            &f.name,
            &f.data_type(),
            kind_of(f.dominant()),
            nullable,
            ordinal,
            Some(format!("in {} of {sampled} sampled documents", f.present)),
        ));
    }
    out
}

fn column(
    name: &str,
    data_type: &str,
    kind: ValueKind,
    nullable: bool,
    ordinal: u32,
    extra: Option<String>,
) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        ordinal,
        data_type: data_type.to_string(),
        kind,
        nullable,
        default_expr: None,
        is_pk: name == ID,
        pk_ordinal: (name == ID).then_some(1),
        is_auto_increment: false,
        is_generated: false,
        is_unsigned: false,
        comment: None,
        enum_values: None,
        char_max_len: None,
        numeric_precision: None,
        numeric_scale: None,
        collation: None,
        array_elem_type: None,
        extra,
        base_type: None,
    }
}

/// The field names for completion, as `infer` orders them.
pub fn field_names(fields: &[FieldStats]) -> Vec<String> {
    fields.iter().map(|f| f.name.clone()).collect()
}

#[cfg(test)]
mod tests {
    use ::mongodb::bson::{Binary, doc, oid::ObjectId, spec::BinarySubtype};

    use super::*;

    #[test]
    fn fields_are_ordered_id_first_then_by_frequency_and_name() {
        let sample = vec![
            doc! { "b": 1, "_id": ObjectId::new(), "a": "x", "z": null },
            doc! { "_id": ObjectId::new(), "a": 2, "b": 3 },
            doc! { "_id": "text id", "c": { "n": 1 }, "a": "y" },
        ];
        let fields = infer(&sample);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["_id", "a", "b", "c", "z"]);
        assert_eq!(fields[0].data_type(), "objectId|string");
        assert_eq!(fields[1].types, vec![("string", 2), ("int", 1)]);
        assert_eq!(fields[1].dominant(), "string");
        assert_eq!(fields[4].dominant(), "null");

        let cols = columns(&fields, sample.len());
        assert_eq!(cols[0].name, "_id");
        assert!(cols[0].is_pk && !cols[0].nullable);
        assert_eq!(cols[0].pk_ordinal, Some(1));
        assert_eq!(cols[1].kind, ValueKind::Text);
        assert!(!cols[1].nullable);
        // `b` is missing from one document, `z` holds null.
        assert!(cols[2].nullable && cols[4].nullable);
        assert_eq!(cols[2].kind, ValueKind::Int);
        assert_eq!(cols[3].kind, ValueKind::Json);
        assert_eq!(cols[4].kind, ValueKind::Text);
        assert_eq!(
            cols[3].extra.as_deref(),
            Some("in 1 of 3 sampled documents")
        );
        let ordinals: Vec<u32> = cols.iter().map(|c| c.ordinal).collect();
        assert_eq!(ordinals, [1, 2, 3, 4, 5]);
        assert_eq!(field_names(&fields), ["_id", "a", "b", "c", "z"]);
    }

    #[test]
    fn types_tie_break_by_label_and_uuids_are_named() {
        let u = Binary {
            subtype: BinarySubtype::Uuid,
            bytes: vec![0; 16],
        };
        let sample = vec![
            doc! { "_id": 1, "v": 1_i64, "u": u.clone() },
            doc! { "_id": 2, "v": 1.5, "u": u },
        ];
        let fields = infer(&sample);
        let v = fields.iter().find(|f| f.name == "v").unwrap();
        assert_eq!(v.data_type(), "double|long");
        let cols = columns(&fields, 2);
        let u = cols.iter().find(|c| c.name == "u").unwrap();
        assert_eq!((u.data_type.as_str(), u.kind), ("uuid", ValueKind::Uuid));
        assert_eq!(cols[0].data_type, "int");
    }

    #[test]
    fn an_empty_sample_has_an_object_id_key() {
        let cols = columns(&infer(&[]), 0);
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].name, "_id");
        assert_eq!(cols[0].data_type, "objectId");
        assert!(cols[0].is_pk);
    }

    #[test]
    fn a_sample_without_id_has_no_key() {
        // A view that projects `_id` away.
        let cols = columns(&infer(&[doc! { "a": 1 }]), 1);
        assert_eq!(cols.len(), 1);
        assert!(!cols[0].is_pk);
    }
}
