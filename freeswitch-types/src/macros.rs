//! Internal declarative macros shared across the crate.
//!
//! These are `#[macro_use]`-imported from `lib.rs` and are not part of the
//! public API. They exist to reduce mechanical repetition in types that are
//! "wire-string ↔ enum" mappings (see `channel.rs`, `sofia/`).
//!
//! Every enum with a wire spelling goes through `wire_enum!`, `header_enum!`
//! or `esl_event_types!`, and each takes its serde from `serde_name`, so the
//! serialized form is the wire name everywhere. Each also requires a test
//! module, where that serde is checked over every variant.

/// Generate a single-field parse-error newtype.
///
/// Expands to `pub struct $Error(pub String)`, a `Display` impl emitting
/// `"unknown <label> (<n> bytes)"`, and `impl std::error::Error`. The rejected
/// input stays on the public field; interpolating it would put wire content in
/// every log line a consumer writes from `{e}`.
///
/// ```ignore
/// parse_error! { ParseFooError("foo"); }
/// ```
macro_rules! parse_error {
    ($Error:ident($label:literal);) => {
        #[doc = concat!("Error returned when parsing an invalid ", $label, " string.")]
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $Error(pub String);

        impl ::std::fmt::Display for $Error {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                write!(
                    f,
                    concat!("unknown ", $label, " ({} bytes)"),
                    self.0
                        .len()
                )
            }
        }

        impl ::std::error::Error for $Error {}
    };
}

/// The test every generated enum carries: serde over each `Variant => "wire"` row.
#[cfg(test)]
macro_rules! wire_serde_test {
    ($Enum:ident, [$($Variant:ident => $wire:literal),+ $(,)?]) => {
        #[cfg(feature = "serde")]
        #[test]
        fn serde_reads_wire_and_variant_names() {
            crate::serde_name::tests::assert_wire_names(&[
                $( ($Enum::$Variant, $wire, stringify!($Variant)), )+
            ]);
        }
    };
}

/// `Serialize` as the wire name and `Deserialize` through `serde_name`, from a
/// `Variant => "wire"` list; `$what` names the value in error messages.
macro_rules! wire_serde {
    ($Enum:ident, $what:expr, [$($Variant:ident => $wire:literal),+ $(,)?]) => {
        #[cfg(feature = "serde")]
        impl serde::Serialize for $Enum {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        #[cfg(feature = "serde")]
        impl<'de> serde::Deserialize<'de> for $Enum {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                const NAMES: &crate::serde_name::Names<$Enum> =
                    &[$(($Enum::$Variant, $wire, stringify!($Variant)),)+];
                crate::serde_name::deserialize(deserializer, $what, NAMES)
            }
        }
    };
}

/// [`sip_header_catalog::define_header_enum!`] with this crate's serde instead of
/// the catalog's, which reads back no 2.x variant name.
///
/// `serde_tests:` names the module holding the serde test, beside the catalog's
/// own `tests_mod:`.
macro_rules! header_enum {
    (
        tests_mod: $tests_mod:ident,
        serde_tests: $serde_tests:ident,
        error_type: $Err:ident => $err_msg:literal,
        $(#[$enum_meta:meta])*
        $vis:vis enum $Name:ident {
            $(
                $(#[$var_meta:meta])*
                $variant:ident => $wire:literal
            ),+ $(,)?
        }
    ) => {
        sip_header_catalog::define_header_enum! {
            tests_mod: $tests_mod,
            error_type: $Err => $err_msg,
            $(#[$enum_meta])*
            $vis enum $Name {
                $(
                    $(#[$var_meta])*
                    $variant => $wire
                ),+
            }
        }

        wire_serde! { $Name, concat!(stringify!($Name), " name"), [$($variant => $wire),+] }

        #[cfg(all(test, feature = "serde"))]
        mod $serde_tests {
            use super::*;

            wire_serde_test! { $Name, [$($variant => $wire),+] }
        }
    };
}

/// Generate a wire-string enum with `ALL`, `as_str`, `Display`, `FromStr` and a
/// `Parse<Name>Error`. The macro adds `#[non_exhaustive]`, `#[allow(missing_docs)]`
/// and serde; the caller supplies the rest, `Copy` among it.
///
/// ```ignore
/// wire_enum! {
///     /// Doc comment on the enum.
///     #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
///     pub enum CallDirection {
///         /// Optional per-variant attrs (docs, cfg_attr).
///         Inbound => "inbound",
///         Outbound => "outbound",
///     }
///     error ParseCallDirectionError("call direction");
///     tests: call_direction_wire_tests;
/// }
/// ```
///
/// `numeric: from_number(u8);` may precede `tests:` and needs a discriminant on
/// every row. `FromStr` is strict canonical case; the test module checks it
/// rejects any other.
macro_rules! wire_enum {
    // With numeric lookups.
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $Enum:ident {
            $(
                $(#[$var_meta:meta])*
                $Variant:ident = $disc:literal => $wire:literal
            ),+ $(,)?
        }
        error $Error:ident($label:literal);
        numeric: $numeric_fn:ident($repr:ty);
        tests: $tests_mod:ident;
    ) => {
        wire_enum! {
            @base
            $(#[$enum_meta])*
            $vis enum $Enum {
                $(
                    $(#[$var_meta])*
                    $Variant = $disc => $wire
                ),+
            }
            error $Error($label);
        }

        impl $Enum {
            /// Look up by numeric discriminant.
            pub fn $numeric_fn(n: $repr) -> Option<Self> {
                match n {
                    $( $disc => Some(Self::$Variant), )+
                    _ => None,
                }
            }

            /// Numeric discriminant value.
            pub fn as_number(&self) -> $repr {
                *self as $repr
            }
        }

        wire_enum! { @tests $Enum, $Error, $label, $tests_mod, $($Variant => $wire),+ }
    };

    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $Enum:ident {
            $(
                $(#[$var_meta:meta])*
                $Variant:ident $(= $disc:literal)? => $wire:literal
            ),+ $(,)?
        }
        error $Error:ident($label:literal);
        tests: $tests_mod:ident;
    ) => {
        wire_enum! {
            @base
            $(#[$enum_meta])*
            $vis enum $Enum {
                $(
                    $(#[$var_meta])*
                    $Variant $(= $disc)? => $wire
                ),+
            }
            error $Error($label);
        }

        wire_enum! { @tests $Enum, $Error, $label, $tests_mod, $($Variant => $wire),+ }
    };

    // Internal: the standard test module.
    (
        @tests $Enum:ident, $Error:ident, $label:literal, $tests_mod:ident,
        $($Variant:ident => $wire:literal),+
    ) => {
        #[cfg(test)]
        mod $tests_mod {
            use super::*;

            #[test]
            fn display_and_from_str_roundtrip() {
                $(
                    assert_eq!($Enum::$Variant.to_string(), $wire);
                    assert_eq!($wire.parse::<$Enum>(), Ok($Enum::$Variant));
                )+
            }

            #[test]
            fn from_str_rejects_wrong_case() {
                $({
                    let wire: &str = $wire;
                    let lower = wire.to_ascii_lowercase();
                    if lower != wire {
                        assert!(
                            lower.parse::<$Enum>().is_err(),
                            concat!(
                                stringify!($Enum),
                                " must reject lowercased \"",
                                $wire,
                                "\"",
                            ),
                        );
                    }
                    let upper = wire.to_ascii_uppercase();
                    if upper != wire {
                        assert!(
                            upper.parse::<$Enum>().is_err(),
                            concat!(
                                stringify!($Enum),
                                " must reject uppercased \"",
                                $wire,
                                "\"",
                            ),
                        );
                    }
                })+
            }

            #[test]
            fn from_str_rejects_unknown() {
                const BOGUS: &str = "__wire_enum_bogus_sentinel__";
                let err = BOGUS.parse::<$Enum>().unwrap_err();
                assert_eq!(err.0, BOGUS);
                assert_eq!(
                    err.to_string(),
                    format!("unknown {} ({} bytes)", $label, BOGUS.len()),
                );
            }

            wire_serde_test! { $Enum, [$($Variant => $wire),+] }
        }
    };

    // Internal: everything but the test module.
    (
        @base
        $(#[$enum_meta:meta])*
        $vis:vis enum $Enum:ident {
            $(
                $(#[$var_meta:meta])*
                $Variant:ident $(= $disc:literal)? => $wire:literal
            ),+ $(,)?
        }
        error $Error:ident($label:literal);
    ) => {
        $(#[$enum_meta])*
        #[non_exhaustive]
        #[allow(missing_docs)]
        $vis enum $Enum {
            $(
                $(#[$var_meta])*
                $Variant $(= $disc)?,
            )+
        }

        impl $Enum {
            /// Every variant, in declaration order.
            pub const ALL: &[Self] = &[ $( Self::$Variant, )+ ];

            /// Canonical wire-format string.
            pub const fn as_str(&self) -> &'static str {
                match self {
                    $( Self::$Variant => $wire, )+
                }
            }
        }

        impl ::std::fmt::Display for $Enum {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl AsRef<str> for $Enum {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl ::std::str::FromStr for $Enum {
            type Err = $Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $( $wire => Ok(Self::$Variant), )+
                    _ => Err($Error(s.to_string())),
                }
            }
        }

        wire_serde! { $Enum, $label, [$($Variant => $wire),+] }

        parse_error! { $Error($label); }
    };
}
