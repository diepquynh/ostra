use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Tier {
    Fast,
    Balanced,
    Advanced,
    Frontier,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Fast, Tier::Balanced, Tier::Advanced, Tier::Frontier];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Fast => "fast",
            Tier::Balanced => "balanced",
            Tier::Advanced => "advanced",
            Tier::Frontier => "frontier",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Tier {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Tier::ALL
            .into_iter()
            .find(|t| t.as_str() == s.trim())
            .ok_or_else(|| format!("unknown tier `{s}`"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

/// A plan phase's `**Complexity:**` value. Work with no phase file counts as `Low`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS, Default,
)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Complexity {
    #[default]
    Low,
    Medium,
    High,
}

impl Complexity {
    pub const ALL: [Complexity; 3] = [Complexity::Low, Complexity::Medium, Complexity::High];

    pub fn as_str(self) -> &'static str {
        match self {
            Complexity::Low => "low",
            Complexity::Medium => "medium",
            Complexity::High => "high",
        }
    }
}

impl FromStr for Complexity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Complexity::Low),
            "medium" => Ok(Complexity::Medium),
            "high" => Ok(Complexity::High),
            other => Err(format!("unknown complexity `{other}`")),
        }
    }
}

impl fmt::Display for Complexity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
