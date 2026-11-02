//! Stable tmux object identifiers.
//!
//! tmux assigns every session (`$N`), window (`@N`) and pane (`%N`) a unique
//! id that never changes for the lifetime of the server. Indices and names
//! do change, so everything in Muxscape is keyed by these ids.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

macro_rules! tmux_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(pub u32);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, IdError> {
                let digits = s
                    .strip_prefix($prefix)
                    .ok_or_else(|| IdError(s.to_string()))?;
                if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(IdError(s.to_string()));
                }
                digits.parse().map($name).map_err(|_| IdError(s.to_string()))
            }
        }

        impl $name {
            pub fn parse_bytes(b: &[u8]) -> Result<Self, IdError> {
                std::str::from_utf8(b)
                    .map_err(|_| IdError(String::from_utf8_lossy(b).into_owned()))?
                    .parse()
            }
        }

        // Serialized as the canonical tmux string ("$1", "@3", "%7") so the
        // frontend sees the same ids that tmux does.
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

tmux_id!(SessionId, "$");
tmux_id!(WindowId, "@");
tmux_id!(PaneId, "%");

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tmux id: {0:?}")]
pub struct IdError(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display_roundtrip() {
        assert_eq!("$12".parse::<SessionId>().unwrap(), SessionId(12));
        assert_eq!("@0".parse::<WindowId>().unwrap(), WindowId(0));
        assert_eq!("%7".parse::<PaneId>().unwrap(), PaneId(7));
        assert_eq!(PaneId(7).to_string(), "%7");
        assert_eq!(SessionId(3).to_string(), "$3");
    }

    #[test]
    fn rejects_wrong_prefix_and_garbage() {
        assert!("@1".parse::<PaneId>().is_err());
        assert!("%".parse::<PaneId>().is_err());
        assert!("%1a".parse::<PaneId>().is_err());
        assert!("%-1".parse::<PaneId>().is_err());
        assert!("%99999999999".parse::<PaneId>().is_err());
    }

    #[test]
    fn serde_uses_tmux_syntax() {
        assert_eq!(serde_json::to_string(&WindowId(4)).unwrap(), "\"@4\"");
        let p: PaneId = serde_json::from_str("\"%9\"").unwrap();
        assert_eq!(p, PaneId(9));
        assert!(serde_json::from_str::<PaneId>("\"@9\"").is_err());
    }
}
