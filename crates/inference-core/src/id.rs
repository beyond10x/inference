//! Identifiers shared across layers, each a newtype of [`uuid::Uuid`] serialized as the Uuid string
//! it wraps.
//!
//! The entities they identify arrive with their own crates; here they are values only, so that a
//! crate depending on `inference-core` alone can carry them.
//!
//! An identifier is written in the canonical hyphenated form and read only in that form, the one
//! spelling ESS accepts: eight, four, four, four and twelve hexadecimal digits in either case. The
//! `urn:uuid:`, brace-wrapped and unhyphenated spellings, and surrounding whitespace, are refused,
//! so that one value has one spelling.

use uuid::Uuid;

/// Parses `text` as a Uuid if it is in the canonical hyphenated form and nothing else. Public for
/// [`uuid_newtype!`](crate::uuid_newtype) only.
#[doc(hidden)]
#[must_use]
pub fn parse_canonical_uuid(text: &str) -> Option<Uuid> {
    let mut groups = text.split('-');
    for width in [8, 4, 4, 4, 12] {
        let group = groups.next()?;
        if group.len() != width || !group.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
    }
    if groups.next().is_some() {
        return None;
    }
    Uuid::parse_str(text).ok()
}

/// Declares an identifier newtype of [`uuid::Uuid`] as this module describes it: serialized as the
/// Uuid string it wraps, read only in the canonical hyphenated form, with `generate`, `as_uuid`,
/// both `From` conversions and `Display`.
///
/// It is exported so that the other crates of the workspace declare their identifiers the same
/// way, `inference.stream.SegmentId` of `inference-text` among them. It reaches `serde` and `uuid`
/// through `inference-core`, so the declaring crate needs neither as a dependency of its own.
#[doc(hidden)]
#[macro_export]
macro_rules! uuid_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name($crate::__private::uuid::Uuid);

        impl $crate::__private::serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error>
            where
                S: $crate::__private::serde::Serializer,
            {
                $crate::__private::serde::Serialize::serialize(&self.0, serializer)
            }
        }

        impl<'de> $crate::__private::serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
            where
                D: $crate::__private::serde::Deserializer<'de>,
            {
                let text = <::std::string::String as $crate::__private::serde::Deserialize>::deserialize(
                    deserializer,
                )?;
                $crate::id::parse_canonical_uuid(&text).map(Self).ok_or_else(|| {
                    <D::Error as $crate::__private::serde::de::Error>::invalid_value(
                        $crate::__private::serde::de::Unexpected::Str(&text),
                        &"a Uuid in the canonical hyphenated form 8-4-4-4-12",
                    )
                })
            }
        }

        impl $name {
            /// A new random (version 4) identifier.
            #[must_use]
            pub fn generate() -> Self {
                Self($crate::__private::uuid::Uuid::new_v4())
            }

            /// The Uuid this identifier wraps.
            #[must_use]
            pub const fn as_uuid(&self) -> $crate::__private::uuid::Uuid {
                self.0
            }
        }

        impl ::std::convert::From<$crate::__private::uuid::Uuid> for $name {
            fn from(uuid: $crate::__private::uuid::Uuid) -> Self {
                Self(uuid)
            }
        }

        impl ::std::convert::From<$name> for $crate::__private::uuid::Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                ::std::fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

pub(crate) use crate::uuid_newtype;

uuid_newtype!(
    /// `inference.graph.RunId`: identifies a run.
    RunId
);
uuid_newtype!(
    /// `inference.graph.NodeId`: identifies a node of a pipeline.
    NodeId
);
uuid_newtype!(
    /// `inference.stream.StreamId`: identifies a stream.
    StreamId
);
uuid_newtype!(
    /// `inference.stream.GenerationId`: identifies a generation, one model output epoch within a
    /// run; an interruption supersedes it.
    GenerationId
);
uuid_newtype!(
    /// `inference.session.SessionId`: identifies a session, stable while the runs below it come and go.
    SessionId
);

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::json;

    use super::{GenerationId, NodeId, RunId, SessionId, StreamId};
    use crate::model::{AdapterId, ModelId};

    const LOWER: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
    const UPPER: &str = "0A1B2C3D-4E5F-4A6B-8C7D-9E0F1A2B3C4D";

    fn assert_uppercase_accepted<T>(name: &str)
    where
        T: Debug + PartialEq + Serialize + DeserializeOwned,
    {
        let upper: T = serde_json::from_value(json!(UPPER))
            .unwrap_or_else(|error| panic!("{name} refused the uppercase canonical form: {error}"));
        let lower: T = serde_json::from_value(json!(LOWER)).expect("lowercase canonical form");
        assert_eq!(upper, lower, "{name}: either case is the same value");
        assert_eq!(
            serde_json::to_value(&upper).expect("serializes"),
            json!(LOWER),
            "{name} is written in lowercase"
        );
    }

    #[test]
    fn identifiers_accept_the_uppercase_canonical_spelling() {
        assert_uppercase_accepted::<ModelId>("ModelId");
        assert_uppercase_accepted::<AdapterId>("AdapterId");
        assert_uppercase_accepted::<RunId>("RunId");
        assert_uppercase_accepted::<NodeId>("NodeId");
        assert_uppercase_accepted::<StreamId>("StreamId");
        assert_uppercase_accepted::<GenerationId>("GenerationId");
        assert_uppercase_accepted::<SessionId>("SessionId");
    }
}
