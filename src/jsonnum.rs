//! f64 with explicit NaN/Infinity JSON tokens so unconvertible values are never
//! silently dropped during serialisation.

use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_nan() {
        s.serialize_str("NaN")
    } else if v.is_infinite() {
        s.serialize_str(if *v > 0.0 { "Infinity" } else { "-Infinity" })
    } else {
        s.serialize_f64(*v)
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Num(f64),
        Token(String),
    }
    match Repr::deserialize(d)? {
        Repr::Num(n) => Ok(n),
        Repr::Token(t) => match t.as_str() {
            "NaN" => Ok(f64::NAN),
            "Infinity" => Ok(f64::INFINITY),
            "-Infinity" => Ok(f64::NEG_INFINITY),
            other => Err(serde::de::Error::custom(format!("invalid numeric token '{other}'"))),
        },
    }
}

/// Convenience wrapper used in event payloads.
#[derive(Debug, Clone, Copy)]
pub struct Jf64(pub f64);

impl serde::Serialize for Jf64 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize(&self.0, s)
    }
}

impl<'de> Deserialize<'de> for Jf64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        deserialize(d).map(Jf64)
    }
}
