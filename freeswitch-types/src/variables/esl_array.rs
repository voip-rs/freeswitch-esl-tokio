use std::fmt;

const ARRAY_HEADER: &str = "ARRAY::";
const ARRAY_SEPARATOR: &str = "|:";

/// Maximum items an [`EslArray`] holds, however it is built.
///
/// Matches the index bound in FreeSWITCH `switch_event.c`
/// (`switch_event_base_add_header`, `if (index > -1 && index <= 4000)`).
pub const MAX_ARRAY_ITEMS: usize = 4000;

/// Errors from building an [`EslArray`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EslArrayError {
    /// Input did not start with the `ARRAY::` prefix.
    MissingPrefix,
    /// Item count exceeds [`MAX_ARRAY_ITEMS`].
    TooManyItems {
        /// Actual number of items found.
        count: usize,
        /// Configured maximum.
        max: usize,
    },
    /// An item holds the [`EslArray::SEPARATOR`], which the switch splits on
    /// with no escape (`switch_separate_string_string`).
    SeparatorInItem {
        /// Position of the item in the array.
        index: usize,
    },
}

impl fmt::Display for EslArrayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPrefix => f.write_str("missing ARRAY:: prefix"),
            Self::TooManyItems { count, max } => {
                write!(f, "array has {count} items, maximum is {max}")
            }
            Self::SeparatorInItem { index } => {
                write!(f, "array item {index} holds the |: separator")
            }
        }
    }
}

impl std::error::Error for EslArrayError {}

fn check_item_count(count: usize) -> Result<(), EslArrayError> {
    if count > MAX_ARRAY_ITEMS {
        return Err(EslArrayError::TooManyItems {
            count,
            max: MAX_ARRAY_ITEMS,
        });
    }
    Ok(())
}

fn check_item(index: usize, item: &str) -> Result<(), EslArrayError> {
    if item.contains(ARRAY_SEPARATOR) {
        return Err(EslArrayError::SeparatorInItem { index });
    }
    Ok(())
}

/// Parses FreeSWITCH `ARRAY::item1|:item2|:item3` format
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct EslArray(Vec<String>);

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for EslArray {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Vec::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl EslArray {
    /// Wire marker every `ARRAY::` value starts with.
    pub const PREFIX: &'static str = ARRAY_HEADER;

    /// Separator between items after the prefix.
    pub const SEPARATOR: &'static str = ARRAY_SEPARATOR;

    /// Parse an `ARRAY::` formatted string.
    ///
    /// Returns [`EslArrayError::MissingPrefix`] if the prefix is absent, or
    /// [`EslArrayError::TooManyItems`] if the item count exceeds
    /// [`MAX_ARRAY_ITEMS`].
    pub fn parse(s: &str) -> Result<Self, EslArrayError> {
        let body = s
            .strip_prefix(ARRAY_HEADER)
            .ok_or(EslArrayError::MissingPrefix)?;
        let items: Vec<String> = body
            .split(ARRAY_SEPARATOR)
            .map(String::from)
            .collect();
        Self::new(items)
    }

    /// Create a new array from a vec of items, refusing what
    /// [`parse`](Self::parse) could not read back: see [`EslArrayError`].
    pub fn new(items: Vec<String>) -> Result<Self, EslArrayError> {
        check_item_count(items.len())?;
        for (index, item) in items
            .iter()
            .enumerate()
        {
            check_item(index, item)?;
        }
        Ok(Self(items))
    }

    /// Append an item to the end; a refusal leaves the array as it was.
    pub fn push(&mut self, value: String) -> Result<(), EslArrayError> {
        check_item_count(self.len() + 1)?;
        check_item(self.len(), &value)?;
        self.0
            .push(value);
        Ok(())
    }

    /// Prepend an item to the front; a refusal leaves the array as it was.
    pub fn unshift(&mut self, value: String) -> Result<(), EslArrayError> {
        check_item_count(self.len() + 1)?;
        check_item(0, &value)?;
        self.0
            .insert(0, value);
        Ok(())
    }

    /// The parsed array items.
    pub fn items(&self) -> &[String] {
        &self.0
    }

    /// Number of items in the array.
    pub fn len(&self) -> usize {
        self.0
            .len()
    }

    /// Returns `true` if the array has no items.
    pub fn is_empty(&self) -> bool {
        self.0
            .is_empty()
    }
}

impl<'a> IntoIterator for &'a EslArray {
    type Item = &'a String;
    type IntoIter = std::slice::Iter<'a, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0
            .iter()
    }
}

impl IntoIterator for EslArray {
    type Item = String;
    type IntoIter = std::vec::IntoIter<String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0
            .into_iter()
    }
}

impl fmt::Display for EslArray {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(ARRAY_HEADER)?;
        for (i, item) in self
            .0
            .iter()
            .enumerate()
        {
            if i > 0 {
                f.write_str(ARRAY_SEPARATOR)?;
            }
            f.write_str(item)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Callers scanning prose for an embedded array need the marker without
    /// hardcoding it.
    #[test]
    fn prefix_is_the_wire_marker() {
        let input = format!("{}one|:two", EslArray::PREFIX);
        assert_eq!(
            EslArray::parse(&input)
                .unwrap()
                .items(),
            &["one", "two"]
        );
    }

    #[test]
    fn parse_single_item() {
        let arr = EslArray::parse("ARRAY::hello").unwrap();
        assert_eq!(arr.items(), &["hello"]);
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn parse_multiple_items() {
        let arr = EslArray::parse("ARRAY::one|:two|:three").unwrap();
        assert_eq!(arr.items(), &["one", "two", "three"]);
        assert_eq!(arr.len(), 3);
    }

    #[test]
    fn parse_non_array_returns_missing_prefix() {
        assert!(matches!(
            EslArray::parse("not an array"),
            Err(EslArrayError::MissingPrefix)
        ));
        assert!(matches!(
            EslArray::parse(""),
            Err(EslArrayError::MissingPrefix)
        ));
        assert!(matches!(
            EslArray::parse("ARRAY:"),
            Err(EslArrayError::MissingPrefix)
        ));
    }

    #[test]
    fn parse_at_max_items_succeeds() {
        let items: Vec<&str> = (0..MAX_ARRAY_ITEMS)
            .map(|_| "x")
            .collect();
        let input = format!("ARRAY::{}", items.join("|:"));
        let arr = EslArray::parse(&input).unwrap();
        assert_eq!(arr.len(), MAX_ARRAY_ITEMS);
    }

    #[test]
    fn parse_over_max_items_returns_error() {
        let items: Vec<&str> = (0..=MAX_ARRAY_ITEMS)
            .map(|_| "x")
            .collect();
        let input = format!("ARRAY::{}", items.join("|:"));
        assert_eq!(
            EslArray::parse(&input),
            Err(EslArrayError::TooManyItems {
                count: MAX_ARRAY_ITEMS + 1,
                max: MAX_ARRAY_ITEMS,
            })
        );
    }

    #[test]
    fn error_display_missing_prefix() {
        assert_eq!(
            EslArrayError::MissingPrefix.to_string(),
            "missing ARRAY:: prefix"
        );
    }

    #[test]
    fn error_display_too_many_items() {
        let err = EslArrayError::TooManyItems {
            count: 5000,
            max: 4000,
        };
        assert_eq!(err.to_string(), "array has 5000 items, maximum is 4000");
    }

    #[test]
    fn display_round_trip() {
        let input = "ARRAY::one|:two|:three";
        let arr = EslArray::parse(input).unwrap();
        assert_eq!(arr.to_string(), input);
    }

    #[test]
    fn display_single_item() {
        let arr = EslArray::parse("ARRAY::only").unwrap();
        assert_eq!(arr.to_string(), "ARRAY::only");
    }

    #[test]
    fn empty_items_in_array() {
        let arr = EslArray::parse("ARRAY::|:|:stuff").unwrap();
        assert_eq!(arr.items(), &["", "", "stuff"]);
    }

    #[test]
    fn new_holds_the_items_given() {
        let arr = EslArray::new(vec!["a".into(), "b".into(), "c".into()]).unwrap();
        assert_eq!(arr.items(), &["a", "b", "c"]);
        assert_eq!(arr.len(), 3);
    }

    #[test]
    fn push_appends() {
        let mut arr = EslArray::new(vec!["first".into()]).unwrap();
        arr.push("second".into())
            .unwrap();
        arr.push("third".into())
            .unwrap();
        assert_eq!(arr.items(), &["first", "second", "third"]);
        assert_eq!(arr.to_string(), "ARRAY::first|:second|:third");
    }

    #[test]
    fn unshift_prepends() {
        let mut arr = EslArray::new(vec!["last".into()]).unwrap();
        arr.unshift("middle".into())
            .unwrap();
        arr.unshift("first".into())
            .unwrap();
        assert_eq!(arr.items(), &["first", "middle", "last"]);
        assert_eq!(arr.to_string(), "ARRAY::first|:middle|:last");
    }

    fn full() -> Vec<String> {
        vec!["x".to_string(); MAX_ARRAY_ITEMS]
    }

    const OVER: EslArrayError = EslArrayError::TooManyItems {
        count: MAX_ARRAY_ITEMS + 1,
        max: MAX_ARRAY_ITEMS,
    };

    /// `new` refuses what `parse` refuses, so `Display` never emits an array `parse` rejects.
    #[test]
    fn new_refuses_more_than_max_items() {
        assert_eq!(
            EslArray::new(full())
                .unwrap()
                .len(),
            MAX_ARRAY_ITEMS
        );
        let mut over = full();
        over.push("y".into());
        assert_eq!(EslArray::new(over), Err(OVER));
    }

    #[test]
    fn push_and_unshift_refuse_past_max_items_and_leave_the_array_as_it_was() {
        let mut arr = EslArray::new(full()).unwrap();
        assert_eq!(arr.push("y".into()), Err(OVER));
        assert_eq!(arr.unshift("y".into()), Err(OVER));
        assert_eq!(arr.items(), full().as_slice());
    }

    /// Config load refuses what `parse` refuses.
    #[cfg(feature = "serde")]
    #[test]
    fn deserialize_refuses_more_than_max_items() {
        let mut over = full();
        over.push("y".into());
        let json = serde_json::to_string(&over).unwrap();
        assert!(serde_json::from_str::<EslArray>(&json).is_err());
        let json = serde_json::to_string(&full()).unwrap();
        assert_eq!(
            serde_json::from_str::<EslArray>(&json)
                .unwrap()
                .len(),
            MAX_ARRAY_ITEMS
        );
    }

    /// The switch splits on every `|:` with no escape, so an item holding one
    /// would come back as two.
    #[test]
    fn new_refuses_an_item_holding_the_separator() {
        let err = EslArray::new(vec!["a".into(), "b|:c".into()]).unwrap_err();
        assert_eq!(err, EslArrayError::SeparatorInItem { index: 1 });
        let shown = err.to_string();
        assert!(shown.contains('1'), "{shown}");
        assert!(!shown.contains("b|:c"), "{shown}");
    }

    #[test]
    fn push_and_unshift_refuse_an_item_holding_the_separator() {
        let mut arr = EslArray::new(vec!["a".into(), "b".into()]).unwrap();
        assert_eq!(
            arr.push("x|:y".into()),
            Err(EslArrayError::SeparatorInItem { index: 2 })
        );
        assert_eq!(
            arr.unshift("x|:y".into()),
            Err(EslArrayError::SeparatorInItem { index: 0 })
        );
        assert_eq!(arr.items(), &["a", "b"]);
    }

    /// A `|` ending one item and a `:` opening the next do not form a separator.
    #[test]
    fn separator_halves_at_item_edges_round_trip() {
        let arr = EslArray::new(vec!["a|".into(), ":b".into(), "|".into(), ":".into()]).unwrap();
        assert_eq!(EslArray::parse(&arr.to_string()), Ok(arr));
    }

    /// No items and one empty item both render a bare `ARRAY::`, which
    /// `switch_event_add_array` drops and the switch never emits.
    #[test]
    fn an_array_rendering_a_bare_prefix_is_refused() {
        assert_eq!(EslArray::new(vec![]), Err(EslArrayError::Empty));
        assert_eq!(
            EslArray::new(vec![String::new()]),
            Err(EslArrayError::Empty)
        );
        assert_eq!(EslArray::parse("ARRAY::"), Err(EslArrayError::Empty));
        let two_empty = EslArray::new(vec![String::new(), String::new()]).unwrap();
        assert_eq!(two_empty.to_string(), "ARRAY::|:");
        assert_eq!(EslArray::parse("ARRAY::|:"), Ok(two_empty));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn deserialize_refuses_an_empty_array() {
        assert!(serde_json::from_str::<EslArray>("[]").is_err());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn deserialize_refuses_a_separator_in_an_item() {
        assert!(serde_json::from_str::<EslArray>(r#"["a|:b"]"#).is_err());
    }

    #[test]
    fn parse_sip_uris_with_colons() {
        let input = "ARRAY::sip:alice@atlanta.example.com|:sip:bob@biloxi.example.com";
        let arr = EslArray::parse(input).unwrap();
        assert_eq!(
            arr.items(),
            &[
                "sip:alice@atlanta.example.com",
                "sip:bob@biloxi.example.com"
            ]
        );
        assert_eq!(arr.to_string(), input);
    }

    #[test]
    fn parse_sip_uris_with_angle_brackets_and_params() {
        let input = "ARRAY::<sip:+15551234567@gw.example.com;user=phone>|:<tel:+15559876543>";
        let arr = EslArray::parse(input).unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(
            arr.items()[0],
            "<sip:+15551234567@gw.example.com;user=phone>"
        );
        assert_eq!(arr.items()[1], "<tel:+15559876543>");
        assert_eq!(arr.to_string(), input);
    }
}
