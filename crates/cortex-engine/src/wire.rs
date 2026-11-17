//! Bounded NDJSON; duplicate object keys are rejected at every depth.
use crate::{Error, Result};
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
// A 10 MiB prompt attachment expands to about 14 MiB in base64.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// `LATE` reports a duplicate key when its object closes (Python's
/// `object_pairs_hook` timing) instead of at the repeated key.
struct Unique<const LATE: bool = false>(Value);
impl<'de, const LATE: bool> Deserialize<'de> for Unique<LATE> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V<const LATE: bool>;
        impl<'de, const LATE: bool> Visitor<'de> for V<LATE> {
            type Value = Unique<LATE>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("unique-key JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Unique<LATE>, E> {
                Ok(Unique(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Unique<LATE>, E> {
                Ok(Unique(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Unique<LATE>, E> {
                Ok(Unique(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Unique<LATE>, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Unique(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Unique<LATE>, E> {
                Ok(Unique(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Unique<LATE>, E> {
                Ok(Unique(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique<LATE>, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Unique<LATE>, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique<LATE>, A::Error> {
                let mut out = Vec::new();
                while let Some(Unique::<LATE>(v)) = a.next_element()? {
                    out.push(v)
                }
                Ok(Unique(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique<LATE>, A::Error> {
                let mut out = Map::new();
                let mut duplicate = false;
                while let Some((k, Unique::<LATE>(v))) = a.next_entry::<String, Unique<LATE>>()? {
                    if out.insert(k, v).is_some() {
                        if !LATE {
                            return Err(de::Error::custom("duplicate_key"));
                        }
                        duplicate = true;
                    }
                }
                if duplicate {
                    return Err(de::Error::custom("duplicate_key"));
                }
                Ok(Unique(Value::Object(out)))
            }
        }
        d.deserialize_any(V::<LATE>)
    }
}
/// Why one incoming client line is not a JSON value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineFault {
    DuplicateKey,
    Syntax,
}
/// Parse one incoming client line like Python's `json.loads` with a
/// duplicate-rejecting `object_pairs_hook`: a repeated key is reported when its
/// object closes, so an earlier syntax error is reported instead. No size
/// bound here; the incoming transport applies its own line limit first.
pub fn parse_line(text: &str) -> std::result::Result<Value, LineFault> {
    let mut d = serde_json::Deserializer::from_str(text);
    let v = Unique::<true>::deserialize(&mut d).map_err(|e| {
        if e.is_data() {
            LineFault::DuplicateKey
        } else {
            LineFault::Syntax
        }
    })?;
    d.end().map_err(|_| LineFault::Syntax)?;
    Ok(v.0)
}
pub fn parse(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_FRAME {
        return Err(Error::FrameTooLarge);
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let v = Unique::<false>::deserialize(&mut d)
        .map_err(|_| Error::Protocol)?
        .0;
    d.end().map_err(|_| Error::Protocol)?;
    Ok(v)
}
pub async fn read_frame<R: AsyncBufRead + Unpin>(r: &mut R) -> Result<Option<Value>> {
    let mut out = Vec::new();
    loop {
        let buf = r.fill_buf().await?;
        if buf.is_empty() {
            return if out.is_empty() {
                Ok(None)
            } else {
                Err(Error::Protocol)
            };
        }
        let n = buf
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(buf.len());
        if out.len() + n > MAX_FRAME {
            return Err(Error::FrameTooLarge);
        }
        let done = buf[n - 1] == b'\n';
        out.extend_from_slice(&buf[..n]);
        r.consume(n);
        if done {
            return parse(&out).map(Some);
        }
    }
}
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, v: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(v).map_err(|_| Error::Protocol)?;
    if bytes.len() + 1 > MAX_FRAME {
        return Err(Error::FrameTooLarge);
    }
    bytes.push(b'\n');
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}
