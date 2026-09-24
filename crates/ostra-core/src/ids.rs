use serde::{Deserialize, Serialize};
use std::fmt;
use ts_rs::TS;

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(
            Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS,
        )]
        #[ts(export, type = "string")]
        pub struct $name(pub String);

        impl $name {
            /// Time-ordered id, so lexical order matches creation order.
            pub fn new() -> Self {
                Self(format!("{}{}", $prefix, uuid::Uuid::now_v7().simple()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }
    };
}

id_type!(WorkspaceId, "ws_");
id_type!(SessionId, "s_");
id_type!(ExecutionId, "x_");
id_type!(GateId, "g_");
id_type!(DecisionId, "d_");
