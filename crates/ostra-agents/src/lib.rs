//! Agent assets (prompts, judges, references, skills), rendered per executor, plus the typed spawn
//! contract and the repo brief appended to every execution.

pub mod brief;
mod mapping;
pub mod spawn;

use ostra_core::{AgentName, Capability, Effort, ExecutorKind, Tier};
use rust_embed::RustEmbed;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use spawn::{SpawnParams, parse_block};

#[derive(RustEmbed)]
#[folder = "../../assets"]
struct Assets;

#[derive(Debug, thiserror::Error)]
pub enum AgentsError {
    #[error("asset `{0}` is missing")]
    Missing(String),
    #[error("parsing asset `{path}`: {message}")]
    Parse { path: String, message: String },
    #[error("rendering `{path}`: {source}")]
    Template {
        path: String,
        source: minijinja::Error,
    },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// The fields of Ultracode's `definition.json`, from `assets/agents/<name>/agent.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentDef {
    pub name: AgentName,
    pub description: String,
    pub default_tier: Tier,
    /// Reasoning effort per executor table (`native`, `claude`, `codex`, `grok`, `agy`).
    pub effort: BTreeMap<String, Effort>,
    pub capabilities: Vec<Capability>,
    pub timeout_secs: u64,
}

#[derive(Deserialize)]
struct AgentToml {
    description: String,
    default_tier: Tier,
    timeout_seconds: u64,
    capabilities: Vec<Capability>,
    effort: BTreeMap<String, Effort>,
}

pub(crate) fn asset_text(path: &str) -> Result<String, AgentsError> {
    let file = Assets::get(path).ok_or_else(|| AgentsError::Missing(path.to_string()))?;
    String::from_utf8(file.data.into_owned()).map_err(|e| AgentsError::Parse {
        path: path.to_string(),
        message: e.to_string(),
    })
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn load_defs() -> Result<BTreeMap<AgentName, AgentDef>, AgentsError> {
    let mut out = BTreeMap::new();
    for agent in AgentName::ALL {
        let path = format!("agents/{}/agent.toml", agent.as_str());
        let raw: AgentToml =
            toml::from_str(&asset_text(&path)?).map_err(|e| AgentsError::Parse {
                path: path.clone(),
                message: e.to_string(),
            })?;
        out.insert(
            agent,
            AgentDef {
                name: agent,
                description: raw.description,
                default_tier: raw.default_tier,
                effort: raw.effort,
                capabilities: raw.capabilities,
                timeout_secs: raw.timeout_seconds,
            },
        );
    }
    Ok(out)
}

fn defs() -> &'static BTreeMap<AgentName, AgentDef> {
    static DEFS: OnceLock<BTreeMap<AgentName, AgentDef>> = OnceLock::new();
    DEFS.get_or_init(|| {
        load_defs().unwrap_or_else(|e| panic!("embedded agent definitions are invalid: {e}"))
    })
}

/// The definition of one agent. Embedded assets are validated by this crate's tests, so a missing
/// or malformed `agent.toml` is a build defect, not a runtime condition.
pub fn agent_def(agent: AgentName) -> &'static AgentDef {
    &defs()[&agent]
}

/// Reasoning effort for an agent on an executor. Falls back to the native value.
pub fn effort_for(agent: AgentName, executor: ExecutorKind) -> Effort {
    let def = agent_def(agent);
    def.effort
        .get(executor.tier_table())
        .or_else(|| def.effort.get("native"))
        .copied()
        .unwrap_or(Effort::High)
}

// ---------------------------------------------------------------------------------------------
// Assets dir
// ---------------------------------------------------------------------------------------------

static ASSETS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Set where the embedded references and skills are materialized on disk. Call once at startup,
/// before any prompt is rendered. Returns false when it was already set.
pub fn set_assets_dir(dir: PathBuf) -> bool {
    ASSETS_DIR.set(dir).is_ok()
}

/// Directory agents read references and bundled skills from: `<data_dir>/assets` unless set.
pub fn assets_dir() -> PathBuf {
    ASSETS_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| ostra_core::paths::data_dir().join("assets"))
}

fn fill_assets_dir(text: &str, dir: &Path) -> String {
    let d = dir.display().to_string();
    text.replace("{{assets_dir}}", &d)
        .replace("{{ assets_dir }}", &d)
}

/// Write every embedded reference and bundled skill under `dir` (`refs/<name>.md`,
/// `skills/<name>/...`), rewriting only files whose content changed.
pub fn materialize_assets(dir: &Path) -> Result<Vec<PathBuf>, AgentsError> {
    let mut written = vec![];
    for name in Assets::iter() {
        if !(name.starts_with("refs/") || name.starts_with("skills/")) {
            continue;
        }
        let target = dir.join(name.as_ref());
        let content = fill_assets_dir(&asset_text(&name)?, dir);
        if std::fs::read_to_string(&target).ok().as_deref() == Some(content.as_str()) {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content)?;
        written.push(target);
    }
    Ok(written)
}

/// Absolute path of a materialized reference, for `Stack reference:` spawn lines.
pub fn reference_path(name: &str) -> PathBuf {
    assets_dir().join("refs").join(format!("{name}.md"))
}

// ---------------------------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------------------------

/// A judge's system prompt, from `assets/judges/<name>.md` (`classify`, `sufficiency`, `stakes`,
/// `route-answer`, `rescue`, `resolve-review`, `yolo-answer`, `completion`).
pub fn judge_prompt(name: &str) -> Option<&'static str> {
    static CACHE: OnceLock<BTreeMap<String, &'static str>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            Assets::iter()
                .filter_map(|p| {
                    let stem = p.strip_prefix("judges/")?.strip_suffix(".md")?.to_string();
                    Some((stem, leak(asset_text(&p).ok()?)))
                })
                .collect()
        })
        .get(name)
        .copied()
}

/// A stack reference (`assets/refs/<name>.md`), with `{{assets_dir}}` filled in.
pub fn reference(name: &str) -> Option<&'static str> {
    static CACHE: OnceLock<BTreeMap<String, &'static str>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let dir = assets_dir();
            Assets::iter()
                .filter_map(|p| {
                    let stem = p.strip_prefix("refs/")?.strip_suffix(".md")?.to_string();
                    Some((stem, leak(fill_assets_dir(&asset_text(&p).ok()?, &dir))))
                })
                .collect()
        })
        .get(name)
        .copied()
}

/// Names of every embedded stack reference.
pub fn reference_names() -> Vec<String> {
    Assets::iter()
        .filter_map(|p| Some(p.strip_prefix("refs/")?.strip_suffix(".md")?.to_string()))
        .collect()
}

/// Stacks with a seed reference, sorted: every reference headed `# Stack Reference:` except the
/// `_generic` fallback. An empty project stack lets the initializer detect it instead.
pub fn stack_names() -> Vec<String> {
    let mut names: Vec<String> = reference_names()
        .into_iter()
        .filter(|n| !n.starts_with('_'))
        .filter(|n| {
            asset_text(&format!("refs/{n}.md")).is_ok_and(|t| t.starts_with("# Stack Reference:"))
        })
        .collect();
    names.sort();
    names
}

/// A skill that ships with Ostra (`assets/skills/<name>/SKILL.md`), with `{{assets_dir}}` filled in.
pub fn embedded_skill(name: &str) -> Option<&'static str> {
    static CACHE: OnceLock<BTreeMap<String, &'static str>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let dir = assets_dir();
            Assets::iter()
                .filter_map(|p| {
                    let skill = p
                        .strip_prefix("skills/")?
                        .strip_suffix("/SKILL.md")?
                        .to_string();
                    Some((skill, leak(fill_assets_dir(&asset_text(&p).ok()?, &dir))))
                })
                .collect()
        })
        .get(name)
        .copied()
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// The tool name (or instruction) a capability maps to on an executor.
pub fn tool_name(capability: &str, agent: AgentName, executor: ExecutorKind) -> Option<String> {
    mapping::get().tool(capability, executor, agent)
}

/// The agent prompt rendered with the executor's tool names. Harness executors also get a tool
/// vocabulary section first, because their tools differ from the names the prompts were tuned on.
pub fn render_prompt(agent: AgentName, executor: ExecutorKind) -> Result<String, AgentsError> {
    let path = format!("agents/{}/prompt.md", agent.as_str());
    let source = asset_text(&path)?;
    let ctx = mapping::get().context(agent, executor, &assets_dir());
    let body = render_str(&path, &source, &ctx)?;
    match executor {
        ExecutorKind::Native => Ok(body),
        ExecutorKind::Harness(h) => {
            let vocab = mapping::get().vocabulary(agent, h, &agent_def(agent).capabilities);
            let vocab = render_str("tool vocabulary", &vocab, &ctx)?;
            Ok(format!("{vocab}\n{body}"))
        }
    }
}

fn render_str(
    path: &str,
    source: &str,
    ctx: &BTreeMap<&'static str, String>,
) -> Result<String, AgentsError> {
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.set_keep_trailing_newline(true);
    env.render_str(source, ctx)
        .map_err(|source| AgentsError::Template {
            path: path.to_string(),
            source,
        })
}

#[cfg(test)]
mod tests;
