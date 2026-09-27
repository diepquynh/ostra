//! The finding record every check produces, and its TSV wire form between the confined role and
//! the host. TSV (tab separated, one finding per line) avoids JSON escaping over paths and error
//! text; newlines and tabs inside a field are flattened to spaces.

use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The design's assumption held (confinement worked, or the escape was blocked).
    Pass,
    /// The assumption was violated: a real gap Phase 3 must account for.
    Fail,
    /// Could not be decided on this VM (missing tool, ambiguous result).
    Inconclusive,
    /// Needs a Windows 11 client or the user's antivirus machine, not this headless VM.
    NeedsClient,
}

impl Status {
    pub fn tag(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Inconclusive => "INCONCLUSIVE",
            Status::NeedsClient => "NEEDS_CLIENT",
        }
    }
    pub fn from_tag(s: &str) -> Status {
        match s {
            "PASS" => Status::Pass,
            "FAIL" => Status::Fail,
            "NEEDS_CLIENT" => Status::NeedsClient,
            _ => Status::Inconclusive,
        }
    }
}

#[derive(Clone)]
pub struct Finding {
    pub verify: u8,
    pub name: String,
    pub status: Status,
    pub expected: String,
    pub observed: String,
}

fn flat(s: &str) -> String {
    s.replace(['\t', '\r', '\n'], " ")
}

impl Finding {
    pub fn to_tsv(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}",
            self.verify,
            self.status.tag(),
            flat(&self.name),
            flat(&self.expected),
            flat(&self.observed),
        )
    }
    pub fn from_tsv(line: &str) -> Option<Finding> {
        let mut it = line.splitn(5, '\t');
        let verify = it.next()?.trim().parse().ok()?;
        let status = Status::from_tag(it.next()?);
        let name = it.next()?.to_string();
        let expected = it.next()?.to_string();
        let observed = it.next().unwrap_or("").to_string();
        Some(Finding { verify, name, status, expected, observed })
    }
}

/// Collects findings for one confined-role invocation and writes them to a file the host reads.
#[derive(Default)]
pub struct Findings(pub Vec<Finding>);

impl Findings {
    pub fn push(&mut self, verify: u8, name: &str, status: Status, expected: &str, observed: String) {
        self.0.push(Finding {
            verify,
            name: name.to_string(),
            status,
            expected: expected.to_string(),
            observed,
        });
    }
    pub fn write(&self, path: &str) -> std::io::Result<()> {
        let mut out = String::new();
        for f in &self.0 {
            let _ = writeln!(out, "{}", f.to_tsv());
        }
        std::fs::write(path, out)
    }
    pub fn read(path: &str) -> Vec<Finding> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(Finding::from_tsv)
            .collect()
    }
}
