use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum HarnessKind {
    Claude,
    Codex,
    Grok,
    Agy,
}

impl HarnessKind {
    pub const ALL: [HarnessKind; 4] = [
        HarnessKind::Claude,
        HarnessKind::Codex,
        HarnessKind::Grok,
        HarnessKind::Agy,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            HarnessKind::Claude => "claude",
            HarnessKind::Codex => "codex",
            HarnessKind::Grok => "grok",
            HarnessKind::Agy => "agy",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            HarnessKind::Claude => "Claude Code",
            HarnessKind::Codex => "Codex",
            HarnessKind::Grok => "Grok Build",
            HarnessKind::Agy => "Antigravity",
        }
    }
}

impl FromStr for HarnessKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "claude" => Ok(HarnessKind::Claude),
            "codex" => Ok(HarnessKind::Codex),
            "grok" => Ok(HarnessKind::Grok),
            "agy" | "antigravity" => Ok(HarnessKind::Agy),
            other => Err(format!("unknown harness `{other}`")),
        }
    }
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What runs an execution. Serialized as `native` or `harness:<name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, TS, Default)]
#[ts(export, type = "string")]
pub enum ExecutorKind {
    #[default]
    Native,
    Harness(HarnessKind),
}

impl ExecutorKind {
    /// Key into `[tiers.<table>]` of the global config.
    pub fn tier_table(self) -> &'static str {
        match self {
            ExecutorKind::Native => "native",
            ExecutorKind::Harness(h) => h.as_str(),
        }
    }

    pub fn all() -> Vec<ExecutorKind> {
        let mut v = vec![ExecutorKind::Native];
        v.extend(HarnessKind::ALL.into_iter().map(ExecutorKind::Harness));
        v
    }

    /// The live view an execution on this executor streams.
    pub fn stream(self) -> ExecStream {
        match self {
            ExecutorKind::Native => ExecStream::Activity,
            ExecutorKind::Harness(_) => ExecStream::Terminal,
        }
    }
}

/// What the browser shows for a running execution: the Activity feed or a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExecStream {
    Activity,
    Terminal,
}

impl fmt::Display for ExecutorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutorKind::Native => f.write_str("native"),
            ExecutorKind::Harness(h) => write!(f, "harness:{h}"),
        }
    }
}

impl FromStr for ExecutorKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s == "native" {
            return Ok(ExecutorKind::Native);
        }
        match s.strip_prefix("harness:") {
            Some(h) => Ok(ExecutorKind::Harness(h.parse()?)),
            None => Err(format!(
                "unknown executor `{s}`: use `native` or `harness:<claude|codex|grok|agy>`"
            )),
        }
    }
}

impl Serialize for ExecutorKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ExecutorKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for e in ExecutorKind::all() {
            assert_eq!(e.to_string().parse::<ExecutorKind>().unwrap(), e);
        }
        assert!("harness:vim".parse::<ExecutorKind>().is_err());
    }

    #[test]
    fn stream_follows_the_executor() {
        assert_eq!(ExecutorKind::Native.stream(), ExecStream::Activity);
        for h in HarnessKind::ALL {
            assert_eq!(ExecutorKind::Harness(h).stream(), ExecStream::Terminal);
        }
        assert_eq!(
            serde_json::to_string(&ExecStream::Terminal).unwrap(),
            "\"terminal\""
        );
    }
}
