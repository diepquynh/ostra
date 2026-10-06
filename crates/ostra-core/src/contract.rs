//! Rule CA5: result contracts. An agent declares the contract its submit follows (`returns`), and
//! the engine reads a result by its contract, never by which agent wrote it. A built-in stage runs
//! any agent whose contract it reads; a plugin defines contracts of its own and handles their
//! results (Rule PL5).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use ts_rs::TS;

/// What an agent's result is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, TS)]
#[ts(export, type = "string")]
pub enum Contract {
    /// A research document and its findings (the research stage).
    Research,
    /// A spec document (the spec stage).
    Spec,
    /// A fact-check verdict on a spec or plan.
    FactCheck,
    /// A master plan and its phases (the plan stage).
    Plan,
    /// A phase's code change and its report (the build stage).
    Implementation,
    /// Review findings for one review loop.
    Review,
    /// A test plan from the changed code's execution paths.
    PathAnalysis,
    /// Tests and their report.
    Tests,
    /// One project's part of the documentation book.
    Documentation,
    /// Instruction files and their report.
    Prompt,
    /// A project's setup step (init sessions).
    Setup,
    /// An answer to one question.
    Answer,
    /// Advice on a failed or stuck step.
    Advice,
    /// A workflow stage's verdict, summary, findings, and declared `data` (Rule CA3).
    Stage,
    /// A plugin's own contract, `<plugin>:<name>`, whose results the plugin handles.
    Plugin(PluginContract),
}

/// A plugin contract name, `<plugin>:<name>`, interned so [`Contract`] stays `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginContract(&'static str);

impl PluginContract {
    pub fn new(plugin: &str, name: &str) -> Result<PluginContract, String> {
        let ok = |s: &str| crate::plugin::valid_plugin_name(s);
        if !ok(plugin) || !ok(name) {
            return Err(format!(
                "Name the contract `<plugin>:<name>` in lowercase kebab-case: `{plugin}:{name}` is not."
            ));
        }
        Ok(PluginContract(crate::agent::intern(&format!(
            "{plugin}:{name}"
        ))))
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }

    pub fn plugin(self) -> &'static str {
        self.0.split_once(':').map(|(p, _)| p).unwrap_or(self.0)
    }

    pub fn name(self) -> &'static str {
        self.0.split_once(':').map(|(_, n)| n).unwrap_or(self.0)
    }
}

impl Contract {
    pub const BUILTIN: [Contract; 14] = [
        Contract::Research,
        Contract::Spec,
        Contract::FactCheck,
        Contract::Plan,
        Contract::Implementation,
        Contract::Review,
        Contract::PathAnalysis,
        Contract::Tests,
        Contract::Documentation,
        Contract::Prompt,
        Contract::Setup,
        Contract::Answer,
        Contract::Advice,
        Contract::Stage,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Contract::Research => "research",
            Contract::Spec => "spec",
            Contract::FactCheck => "fact-check",
            Contract::Plan => "plan",
            Contract::Implementation => "implementation",
            Contract::Review => "review",
            Contract::PathAnalysis => "path-analysis",
            Contract::Tests => "tests",
            Contract::Documentation => "documentation",
            Contract::Prompt => "prompt",
            Contract::Setup => "setup",
            Contract::Answer => "answer",
            Contract::Advice => "advice",
            Contract::Stage => "stage",
            Contract::Plugin(p) => p.as_str(),
        }
    }

    /// Rule CA5: the contract a run's report file belongs to is written before its submit, except
    /// a review's, whose ledger exists only when it found something.
    pub fn report_required(self) -> bool {
        self != Contract::Review
    }

    /// Rule CA5: contracts whose runs belong to one plan phase, so routes may differ by the
    /// phase's complexity.
    pub fn per_phase(self) -> bool {
        matches!(self, Contract::Implementation | Contract::Tests)
    }

    /// A contract a workflow's custom stage reads: the stage verdict, or a plugin's own.
    pub fn custom_stage(self) -> bool {
        matches!(self, Contract::Stage | Contract::Plugin(_))
    }
}

impl fmt::Display for Contract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Contract {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Some(c) = Contract::BUILTIN.into_iter().find(|c| c.as_str() == s) {
            return Ok(c);
        }
        match s.split_once(':') {
            Some((p, n)) => PluginContract::new(p, n).map(Contract::Plugin),
            None => Err(format!(
                "Set `returns` to one of {}, or `<plugin>:<contract>`: `{s}` is not one.",
                Contract::BUILTIN
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

impl Serialize for Contract {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Contract {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = std::borrow::Cow::<'de, str>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_round_trip() {
        for c in Contract::BUILTIN {
            assert_eq!(c.as_str().parse::<Contract>().unwrap(), c);
        }
        let p: Contract = "acme:release-note".parse().unwrap();
        assert!(
            matches!(p, Contract::Plugin(x) if x.plugin() == "acme" && x.name() == "release-note")
        );
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"acme:release-note\"");
        assert!("nope".parse::<Contract>().is_err());
        assert!("Acme:x".parse::<Contract>().is_err());
    }
}
