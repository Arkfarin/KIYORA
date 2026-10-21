use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use std::fmt;

struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, UniqueValue>()? {
                    if values.insert(key, value.0).is_some() {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        d.deserialize_any(UniqueVisitor)
    }
}

/// serde_json's recursion limit remains enabled, including for nested values.
pub fn strict_json(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<UniqueValue>(bytes).map(|v| v.0)
}

/// Sorted UTF-8 keys, compact JSON, integers only in protocol data; not RFC 8785.
pub fn canonical_json<T: Serialize>(input: &T) -> Result<Vec<u8>, serde_json::Error> {
    fn write(value: &Value, out: &mut Vec<u8>) -> Result<(), serde_json::Error> {
        match value {
            Value::Object(map) => {
                out.push(b'{');
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                for (index, key) in keys.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    serde_json::to_writer(&mut *out, key)?;
                    out.push(b':');
                    write(&map[*key], out)?;
                }
                out.push(b'}');
            }
            Value::Array(values) => {
                out.push(b'[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    write(value, out)?;
                }
                out.push(b']');
            }
            other => serde_json::to_writer(out, other)?,
        }
        Ok(())
    }
    let mut result = Vec::new();
    write(&serde_json::to_value(input)?, &mut result)?;
    Ok(result)
}
