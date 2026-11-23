//! Text for tests, weighted toward what the switch's string passes treat specially.
//!
//! The property-test strategies serve only the C-oracle modules and build under the same cfg.

#[cfg(freeswitch_oracle)]
pub(crate) use strategies::{opens_with_a_non_ascii_head, text};

#[cfg(freeswitch_oracle)]
mod strategies {
    use proptest::collection::vec;
    use proptest::prelude::*;
    use proptest::sample::select;

    const HOSTILE: &[&str] = &[
        " ", "\t", "'", "\"", "\\", ",", "|", ":", "_", ":_:", "=", "{", "}", "[", "]", "<", ">",
        "^", "^^", "~", "!", ";", "$", "${", "(", ")", "&", r"\n", r"\r", r"\t", r"\s", "n", "r",
        "t", "s", "é", "😀", "\u{b}", "/", "@", "+", "%", "gateway", "app=",
    ];
    const ORDINARY: &[&str] = &["a", "Z", "0", "42", "bob", "x9"];
    const WHOLE: &[&str] = &["", "undef", "UNDEF", "Undef"];
    const EDGE: &[&str] = &["", "", " ", "  "];

    pub(crate) fn text() -> impl Strategy<Value = String> {
        let piece = prop_oneof![3 => select(HOSTILE), 2 => select(ORDINARY)];
        let body = vec(piece, 0..8).prop_map(|pieces| pieces.concat());
        prop_oneof![
            8 => (select(EDGE), body, select(EDGE))
                .prop_map(|(lead, body, trail)| format!("{lead}{body}{trail}")),
            1 => select(WHOLE).prop_map(str::to_owned),
        ]
    }

    /// `switch_separate_string` takes the first byte after `^^` whatever it is; the port takes
    /// no head naming a non-ASCII separator, which no char split mirrors.
    pub(crate) fn opens_with_a_non_ascii_head(input: &str) -> bool {
        matches!(input.as_bytes(), [b'^', b'^', picked, _, ..] if !picked.is_ascii())
    }
}

/// Quotes, escapes and separators whose tokens a split must lay end to end over the input.
#[cfg(feature = "esl")]
pub(crate) const TILING_INPUTS: &[&str] = &[
    "a,'b c',error/X",
    "'b c',a",
    "  a b  ",
    " 'x y' ",
    "a '' b",
    "''",
    "'",
    "x'y",
    r"a\,b,c",
    r"a\'b",
    r"'a\'b'",
    r"\\",
    r"trailing\",
    r"\s\n\t",
    r"a\',b",
    r"\'lead,x",
    r"\$${x}\$y",
    "${a}b,$${c}",
    "'é,ü' ,ß",
    "[v='x,y']loopback/9199/a,error/USER_BUSY",
    "a|'b|c' |d",
];
