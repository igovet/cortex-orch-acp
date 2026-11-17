//! Preserve JSON object insertion order where it becomes a wire array.
//! Python replaces duplicate values without changing their first position.
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::fmt;

#[derive(Default)]
pub(crate) struct Ordered {
    pub value: Value,
    pub members: Vec<(String, Ordered)>,
}
impl Ordered {
    pub fn member(&self, key: &str) -> Option<&Self> {
        self.members.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}
struct OrderedVisitor;
impl<'de> Visitor<'de> for OrderedVisitor {
    type Value = Ordered;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON")
    }
    fn visit_unit<E: de::Error>(self) -> Result<Ordered, E> {
        Ok(Ordered::default())
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Ordered, E> {
        Ok(Ordered {
            value: v.into(),
            members: vec![],
        })
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Ordered, E> {
        Ok(Ordered {
            value: v.into(),
            members: vec![],
        })
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Ordered, E> {
        Ok(Ordered {
            value: v.into(),
            members: vec![],
        })
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Ordered, E> {
        let number = serde_json::Number::from_f64(v).ok_or_else(|| E::custom("nonfinite JSON"))?;
        Ok(Ordered {
            value: number.into(),
            members: vec![],
        })
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Ordered, E> {
        self.visit_string(v.into())
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Ordered, E> {
        Ok(Ordered {
            value: v.into(),
            members: vec![],
        })
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Ordered, A::Error> {
        let mut values = vec![];
        while let Some(v) = seq.next_element::<Value>()? {
            values.push(v);
        }
        Ok(Ordered {
            value: values.into(),
            members: vec![],
        })
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
        let mut members: Vec<(String, Ordered)> = vec![];
        let mut value = serde_json::Map::new();
        let mut positions = std::collections::BTreeMap::<String, usize>::new();
        while let Some((key, child)) = map.next_entry::<String, Ordered>()? {
            value.insert(key.clone(), child.value.clone());
            if let Some(index) = positions.get(&key) {
                members[*index].1 = child;
            } else {
                positions.insert(key.clone(), members.len());
                members.push((key, child));
            }
        }
        Ok(Ordered {
            value: Value::Object(value),
            members,
        })
    }
}
impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(OrderedVisitor)
    }
}
