use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid canonical Telegram identifier")]
pub struct InvalidId;

macro_rules! identifier {
    ($name:ident, $valid:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(i64);
        impl $name {
            pub fn new(value: i64) -> Result<Self, InvalidId> {
                if ($valid)(value) {
                    Ok(Self(value))
                } else {
                    Err(InvalidId)
                }
            }
            pub fn get(self) -> i64 {
                self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
        impl FromStr for $name {
            type Err = InvalidId;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let number: i64 = value.parse().map_err(|_| InvalidId)?;
                if number.to_string() != value {
                    return Err(InvalidId);
                }
                Self::new(number)
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                String::deserialize(deserializer)?
                    .parse()
                    .map_err(de::Error::custom)
            }
        }
    };
}

identifier!(MessageId, |n| n > 0);
identifier!(ChatId, |n| n != 0);
identifier!(UserId, |n| n > 0);
