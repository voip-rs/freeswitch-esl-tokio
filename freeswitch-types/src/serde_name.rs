//! Serde for the enums `wire_enum!`, `header_enum!` and `esl_event_types!` generate.
//!
//! A value serializes as its wire name, the spelling the switch and a config author write.
//! It reads back from the wire name in any ASCII case, or from the Rust variant name, which is
//! what 2.x serialized: data written before the switch to wire names keeps loading. The exact
//! wire name is tried first and the variant name before the case-folded wire name, so a variant
//! name that happens to equal another variant's wire name in some casing still reads as its own
//! variant; the generated tests check every variant both ways.
//!
//! Errors name the expected value and the input's length, never the input.

use std::cell::Cell;
use std::fmt;

use serde::de::{self, Deserializer, Visitor};

/// One row per variant: the value, its wire name, its Rust variant name.
pub(crate) type Names<T> = [(T, &'static str, &'static str)];

/// Read a value from its wire name in any ASCII case, or from its variant name.
pub(crate) fn deserialize<'de, D, T>(
    deserializer: D,
    what: &'static str,
    names: &'static Names<T>,
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Copy + 'static,
{
    let read = Cell::new(false);
    deserializer
        .deserialize_str(NameVisitor {
            what,
            names,
            read: &read,
        })
        .map_err(|e| {
            // Raised before a string was read, it is serde's, which quotes the value.
            if read.get() {
                e
            } else {
                de::Error::custom(format_args!("invalid {what}: expected a string"))
            }
        })
}

fn lookup<T: Copy>(names: &Names<T>, s: &str) -> Option<T> {
    names
        .iter()
        .find(|(_, wire, _)| *wire == s)
        .or_else(|| {
            names
                .iter()
                .find(|(_, _, variant)| *variant == s)
        })
        .or_else(|| {
            names
                .iter()
                .find(|(_, wire, _)| wire.eq_ignore_ascii_case(s))
        })
        .map(|(value, ..)| *value)
}

struct NameVisitor<'a, T: 'static> {
    what: &'static str,
    names: &'static Names<T>,
    read: &'a Cell<bool>,
}

impl<T: Copy + 'static> Visitor<'_> for NameVisitor<'_, T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a {}", self.what)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        self.read
            .set(true);
        lookup(self.names, v)
            .ok_or_else(|| E::custom(format_args!("unknown {} ({} bytes)", self.what, v.len())))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fmt::Debug;

    use serde::de::DeserializeOwned;
    use serde::Serialize;

    /// `wire` with the case of every other ASCII letter flipped.
    fn odd_case(wire: &str) -> String {
        wire.chars()
            .enumerate()
            .map(|(i, c)| match i % 2 {
                0 if c.is_ascii_lowercase() => c.to_ascii_uppercase(),
                0 => c.to_ascii_lowercase(),
                _ => c,
            })
            .collect()
    }

    fn read<T: DeserializeOwned>(s: &str) -> Result<T, serde_json::Error> {
        serde_json::from_str(&serde_json::to_string(s).unwrap())
    }

    /// Every `(value, wire, variant)` row: serializes as `wire`, and reads back from `wire`,
    /// from `wire` in odd, upper and lower case, and from the 2.x `variant` form.
    pub(crate) fn assert_wire_names<T>(rows: &[(T, &str, &str)])
    where
        T: Serialize + DeserializeOwned + PartialEq + Debug + Copy,
    {
        for &(value, wire, variant) in rows {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::Value::String(wire.into()),
                "{variant} serializes as its wire name"
            );
            for form in [
                wire.to_string(),
                odd_case(wire),
                wire.to_ascii_uppercase(),
                wire.to_ascii_lowercase(),
                variant.to_string(),
            ] {
                assert_eq!(
                    read::<T>(&form).ok(),
                    Some(value),
                    "{form:?} reads as {variant}"
                );
            }
        }
        let bogus = "__no_such_wire_name__";
        let err = read::<T>(bogus)
            .unwrap_err()
            .to_string();
        assert!(!err.contains(bogus), "{err}");
        let err = serde_json::from_str::<T>("7")
            .unwrap_err()
            .to_string();
        assert!(!err.contains('7'), "{err}");
    }
}
