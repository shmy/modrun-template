use std::{fmt, ops::Deref, str::FromStr};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};
use std::num::ParseIntError;
use std::result::Result;
use tsid::create_tsid;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Id(i64);

impl Id {
    pub fn new() -> Self {
        let id = create_tsid();
        // TSID 内部为 u64，映射到 i64 时裁剪到 i64::MAX，避免极端溢成负数。
        Self(id.number().min(i64::MAX as u64) as i64)
    }

    /// 从已信任来源（DB 行、已解码 JWT）直接构造；不做合法性校验。
    pub fn new_unchecked(id: i64) -> Self {
        Self(id)
    }
}

impl Default for Id {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for Id {
    type Target = i64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<i64> for Id {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl From<Id> for i64 {
    fn from(value: Id) -> Self {
        value.0
    }
}

impl FromStr for Id {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(i64::from_str(s)?))
    }
}

impl Serialize for Id {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

struct IdVisitor;

impl<'de> Visitor<'de> for IdVisitor {
    type Value = Id;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a positive string or integer id")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let id = v.parse::<i64>().map_err(E::custom)?;
        parse_positive_id(id).map_err(E::custom)
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        parse_positive_id(v).map_err(E::custom)
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let id = i64::try_from(v).map_err(E::custom)?;
        parse_positive_id(id).map_err(E::custom)
    }
}

/// ID 必须为正整数；负数/零只能是内部错误（`new_unchecked`），不接受外部输入。
fn parse_positive_id(id: i64) -> Result<Id, &'static str> {
    if id > 0 {
        Ok(Id(id))
    } else {
        Err("id must be a positive integer")
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(IdVisitor)
    }
}
