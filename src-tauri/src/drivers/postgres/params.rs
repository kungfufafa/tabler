//! `Value` → PostgreSQL bind parameters.
//!
//! Everything is sent as `TEXT` (bytes as `BYTEA`); the generated SQL adds `$n::<type>` casts so
//! the server performs the conversion with its own input functions. This keeps one code path for
//! every type, including ones tokio-postgres cannot encode natively.

use tokio_postgres::types::{ToSql, Type};

use crate::error::AppError;
use crate::types::value::{Value, pg_array_literal};

/// A boxed bind parameter.
pub type Param = Box<dyn ToSql + Send + Sync>;

/// Converts values into (declared parameter types, boxed parameters) for `prepare_typed` +
/// `query_raw`.
pub fn bind(values: &[Value]) -> Result<(Vec<Type>, Vec<Param>), AppError> {
    use base64::Engine as _;
    let mut types = Vec::with_capacity(values.len());
    let mut params: Vec<Param> = Vec::with_capacity(values.len());
    for value in values {
        match value {
            Value::Null | Value::Default => {
                types.push(Type::TEXT);
                params.push(Box::new(None::<String>));
            }
            Value::Bytes(b) => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&b.base64)
                    .map_err(|e| AppError::InvalidInput(format!("invalid base64: {e}")))?;
                types.push(Type::BYTEA);
                params.push(Box::new(bytes));
            }
            Value::Bool(b) => {
                types.push(Type::TEXT);
                params.push(Box::new(if *b { "true" } else { "false" }.to_string()));
            }
            Value::Array(items) => {
                types.push(Type::TEXT);
                params.push(Box::new(pg_array_literal(items)));
            }
            other => {
                types.push(Type::TEXT);
                params.push(Box::new(other.as_text().unwrap_or_default()));
            }
        }
    }
    Ok((types, params))
}

/// Borrows boxed parameters in the shape `query_raw` expects.
pub fn as_refs(params: &[Param]) -> Vec<&(dyn ToSql + Sync)> {
    params
        .iter()
        .map(|p| p.as_ref() as &(dyn ToSql + Sync))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::BytesValue;

    #[test]
    fn binds_text_and_bytea() {
        let values = vec![
            Value::Null,
            Value::Int(1),
            Value::Bool(true),
            Value::Bytes(BytesValue {
                base64: "AP8=".into(),
                len: 2,
                truncated: false,
            }),
            Value::Array(vec![Value::Text("a".into()), Value::Null]),
            Value::Default,
        ];
        let (types, params) = bind(&values).unwrap();
        assert_eq!(
            types,
            vec![
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::BYTEA,
                Type::TEXT,
                Type::TEXT
            ]
        );
        assert_eq!(params.len(), 6);
        assert_eq!(as_refs(&params).len(), 6);
    }

    #[test]
    fn rejects_bad_base64() {
        let values = vec![Value::Bytes(BytesValue {
            base64: "***".into(),
            len: 0,
            truncated: false,
        })];
        assert!(matches!(bind(&values), Err(AppError::InvalidInput(_))));
    }
}
