//! Strongly typed UUID identifiers.

/// Defines a newtype over `uuid::Uuid` (v7 for new values).
#[macro_export]
macro_rules! define_id {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            PartialOrd,
            Ord,
            ::serde::Serialize,
            ::serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(::uuid::Uuid);

        impl $name {
            /// New time-ordered (v7) id.
            pub fn new() -> Self {
                Self(::uuid::Uuid::now_v7())
            }

            pub fn from_uuid(id: ::uuid::Uuid) -> Self {
                Self(id)
            }

            pub fn as_uuid(&self) -> ::uuid::Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                ::std::fmt::Display::fmt(&self.0, f)
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = ::uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                ::uuid::Uuid::parse_str(s).map(Self)
            }
        }

        impl From<::uuid::Uuid> for $name {
            fn from(id: ::uuid::Uuid) -> Self {
                Self(id)
            }
        }
    };
}

define_id!(UserId);
define_id!(WorkspaceId);
define_id!(MemberId);
define_id!(RoomId);
define_id!(ParticipantId);
define_id!(SessionId);
define_id!(SegmentId);
define_id!(MeetingRoomId);
define_id!(TranscriptId);

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn string_roundtrip() {
        let id = UserId::new();
        let parsed: UserId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
        assert!("nope".parse::<UserId>().is_err());
    }

    #[test]
    fn serde_roundtrip_is_transparent() {
        let id = RoomId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        let back: RoomId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
        assert_eq!(RoomId::from_uuid(id.as_uuid()), id);
    }

    #[test]
    fn new_is_v7() {
        assert_eq!(SessionId::new().as_uuid().get_version_num(), 7);
    }
}
