use ostra_core::{AgentName, Capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

fn def(name: &str, description: &str, input_schema: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema,
    }
}

const READ: &str = "Reads a file from the local filesystem.

Usage:
- file_path should be an absolute path. A relative path resolves against the shell's current directory.
- By default it reads up to 2000 lines from the start of the file. For a long file, pass offset (the 1-based line number to start at) and limit (the number of lines).
- Lines longer than 2000 characters are truncated.
- Results are numbered like `cat -n`: the line number, a tab, then the line. The number and tab are not part of the file, so leave them out of any Edit old_string.
- Reading a directory is an error. Use Glob, or Bash with `ls`, to list one.
- You must Read an existing file before you Edit or Write it in this run, so you never overwrite content you have not seen.";

const WRITE: &str = "Writes a file to the local filesystem, replacing it if it exists.

Usage:
- If the file already exists, you must Read it first in this run. Otherwise the call fails.
- Parent directories are created as needed.
- Prefer Edit for changes to an existing file: it sends only the change and cannot drop unrelated content.
- Write the complete file content. Nothing is appended.";

const EDIT: &str = "Performs an exact string replacement in a file.

Usage:
- You must Read the file in this run before editing it.
- old_string must match the file exactly, including whitespace and indentation. Copy it from the Read output without the line-number prefix.
- The edit fails when old_string matches more than one place. Add surrounding lines until it is unique, or set replace_all to change every match (for example to rename a variable).
- new_string must differ from old_string.
- An empty old_string on a file that does not exist creates it with new_string as its content.";

const BASH: &str = "Executes a bash command and returns its combined stdout and stderr.

Usage:
- The working directory persists between calls: a `cd` carries over to the next command. The shell starts at the project root.
- Default timeout is 120000 ms (2 minutes). Pass timeout in milliseconds for longer commands, up to 600000 ms (10 minutes). A command that runs past its timeout is stopped along with every process it started.
- Output over 30000 characters is truncated, keeping the start and the end.
- A non-zero exit code is reported with the output.
- Processes left running in the background are stopped when the command returns, so do not start servers or watchers here.
- Quote paths that contain spaces.
- Prefer Read, Grep, and Glob over `cat`, `grep`, and `find`: they are faster and their output is structured for you.
- Run the project's own build and test commands exactly as your brief gives them.";

const GREP: &str = "Searches file contents with a regular expression (ripgrep syntax).

Usage:
- pattern is a Rust regex: `log.*Error`, `fn\\s+\\w+`. Escape literal braces, for example `interface\\{\\}`.
- path is the file or directory to search. It defaults to the shell's current directory.
- Filter files with glob (`*.js`, `**/*.{ts,tsx}`) or type (`rust`, `js`, `ts`, `py`, `go`, `java`).
- output_mode is `files_with_matches` (default: matching file paths, newest first), `content` (matching lines, with -n line numbers on by default and -A, -B, -C context), or `count` (matches per file).
- -i makes the search case-insensitive. head_limit caps the number of lines or files returned.
- By default a match is one line. Set multiline to let `.` match newlines and a pattern span lines.
- .gitignore rules are respected; hidden files such as `.ostra/` are searched.";

const GLOB: &str = "Finds files by name pattern.

Usage:
- Supports glob patterns such as `**/*.js` or `src/**/*.ts`. `*` does not cross a directory separator; `**` does.
- path is the directory to search from. It defaults to the shell's current directory.
- Returns matching file paths, newest first, at most 100.
- .gitignore rules are respected.";

const SKILL: &str = "Loads a skill: a SKILL.md file of instructions for a kind of task.

Usage:
- Pass name to load the project skill at `.ostra/skills/<name>/SKILL.md` or a skill Ostra ships. Pass path to load a SKILL.md at an exact location.
- Load each skill your prompt or brief names before you start the work it covers, then follow its instructions.
- Never guess a skill name or path. Use the ones your brief lists.";

const WEB_FETCH: &str = "Fetches a URL and returns its content as markdown.

Usage:
- url must be a full http or https URL.
- HTML pages are converted to markdown. Other text types are returned as they are. Binary content is refused.
- Content over 100000 characters is truncated.
- prompt is optional: say what you are looking for on the page, and it is shown above the content.
- Cite the URL and the page's date for every fact you take from it.";

const REPORT: &str = "Writes your report to the exact `Report file:` path your prompt declared.

Usage:
- Pass the complete report as content. The call replaces the file.
- The path is fixed by Ostra so later stages can find the report. You do not choose it.
- If the call is refused because a failure-to-recovery lesson is not recorded yet, record it with Memory first, or pass reason to say why no lesson applies.";

const DOCUMENT: &str = "Writes your document: the research document, the spec, or the plan, as typed JSON. Ostra renders the markdown the next stage reads from it and shows each part in the browser.

Usage:
- path is the absolute path of the document's markdown file in your session dir, named as your prompt says. Ostra writes it and a .json beside it. A plan also gets one `-phase-{N}.md` file per phase.
- Send document to write the whole document. It replaces what is there.
- Send update to revise in place: each top-level field you name replaces the stored one, except a list whose items carry an `id`, which is merged by id. Send only the items that changed. remove drops list items by id.
- A long document can be written in parts: a first call with document holding every required field, then update calls that add list items.
- The result lists what Ostra's checks found. Fix every error before you submit, because the submit call is refused while one remains.
- Never write these files with Write, Edit, or the shell. Ostra refuses it, because the markdown is rendered from the document.";

const MEMORY: &str = "Records a durable lesson in this project's memory, for future runs to recall.

Usage:
- Record only what a later reader would otherwise pay to rediscover: a constraint the code does not state, behavior that contradicts a name, a version-specific API detail, an invariant that spans files, or how a build failure was fixed.
- Do not record anything the code makes obvious, this run's task, or a lesson recall already returned.
- area scopes the lesson to a module, for example `orders` or `orders::Service`. lesson is one line.
- Recording the same area and lesson again updates it in place.";

const MEMORY_RECALL: &str = "Recalls lessons earlier runs recorded for this project.

Usage:
- Call it before you start work in an area, with query describing the task, and again with the error text as query when you hit a failure.
- area narrows recall to a module and its sub-scopes. Lessons from other areas that match the query fill the remaining slots.
- limit defaults to 8.";

fn read_def() -> ToolDefinition {
    def(
        "Read",
        READ,
        json!({"type": "object", "properties": {
            "file_path": {"type": "string", "description": "Absolute path of the file to read"},
            "offset": {"type": "integer", "minimum": 1, "description": "1-based line number to start reading from"},
            "limit": {"type": "integer", "minimum": 1, "description": "Number of lines to read"}
        }, "required": ["file_path"], "additionalProperties": false}),
    )
}

fn write_def() -> ToolDefinition {
    def(
        "Write",
        WRITE,
        json!({"type": "object", "properties": {
            "file_path": {"type": "string", "description": "Absolute path of the file to write"},
            "content": {"type": "string", "description": "The complete file content"}
        }, "required": ["file_path", "content"], "additionalProperties": false}),
    )
}

fn edit_def() -> ToolDefinition {
    def(
        "Edit",
        EDIT,
        json!({"type": "object", "properties": {
            "file_path": {"type": "string", "description": "Absolute path of the file to edit"},
            "old_string": {"type": "string", "description": "The exact text to replace"},
            "new_string": {"type": "string", "description": "The text to put in its place"},
            "replace_all": {"type": "boolean", "default": false, "description": "Replace every match of old_string"}
        }, "required": ["file_path", "old_string", "new_string"], "additionalProperties": false}),
    )
}

fn bash_def() -> ToolDefinition {
    def(
        "Bash",
        BASH,
        json!({"type": "object", "properties": {
            "command": {"type": "string", "description": "The command to run"},
            "timeout": {"type": "integer", "minimum": 1, "maximum": 600000, "description": "Timeout in milliseconds, up to 600000"},
            "description": {"type": "string", "description": "What the command does, in 5 to 10 words"}
        }, "required": ["command"], "additionalProperties": false}),
    )
}

fn grep_def() -> ToolDefinition {
    def(
        "Grep",
        GREP,
        json!({"type": "object", "properties": {
            "pattern": {"type": "string", "description": "Regular expression to search for"},
            "path": {"type": "string", "description": "File or directory to search. Defaults to the current directory"},
            "glob": {"type": "string", "description": "Glob that files must match, such as `*.js` or `**/*.{ts,tsx}`"},
            "type": {"type": "string", "description": "File type to search, such as `rust`, `js`, `py`"},
            "output_mode": {"type": "string", "enum": ["content", "files_with_matches", "count"], "description": "What to return. Defaults to files_with_matches"},
            "-i": {"type": "boolean", "description": "Case-insensitive search"},
            "-n": {"type": "boolean", "description": "Show line numbers in content mode. Defaults to true"},
            "-A": {"type": "integer", "minimum": 0, "description": "Lines of context after each match (content mode)"},
            "-B": {"type": "integer", "minimum": 0, "description": "Lines of context before each match (content mode)"},
            "-C": {"type": "integer", "minimum": 0, "description": "Lines of context before and after each match (content mode)"},
            "head_limit": {"type": "integer", "minimum": 1, "description": "Return at most this many lines or files"},
            "multiline": {"type": "boolean", "description": "Let `.` match newlines and patterns span lines"}
        }, "required": ["pattern"], "additionalProperties": false}),
    )
}

fn glob_def() -> ToolDefinition {
    def(
        "Glob",
        GLOB,
        json!({"type": "object", "properties": {
            "pattern": {"type": "string", "description": "Glob pattern to match file paths against"},
            "path": {"type": "string", "description": "Directory to search from. Defaults to the current directory"}
        }, "required": ["pattern"], "additionalProperties": false}),
    )
}

fn skill_def() -> ToolDefinition {
    def(
        "Skill",
        SKILL,
        json!({"type": "object", "properties": {
            "name": {"type": "string", "description": "Skill name, such as `convention`"},
            "path": {"type": "string", "description": "Exact path of a SKILL.md file"}
        }, "additionalProperties": false}),
    )
}

fn web_fetch_def() -> ToolDefinition {
    def(
        "WebFetch",
        WEB_FETCH,
        json!({"type": "object", "properties": {
            "url": {"type": "string", "description": "The http or https URL to fetch"},
            "prompt": {"type": "string", "description": "What you are looking for on the page"}
        }, "required": ["url"], "additionalProperties": false}),
    )
}

fn report_def() -> ToolDefinition {
    def(
        "Report",
        REPORT,
        json!({"type": "object", "properties": {
            "content": {"type": "string", "description": "The complete report, in markdown"},
            "reason": {"type": "string", "description": "Why no lesson applies, when the lesson gate refused the report"}
        }, "required": ["content"], "additionalProperties": false}),
    )
}

fn memory_def() -> ToolDefinition {
    def(
        "Memory",
        MEMORY,
        json!({"type": "object", "properties": {
            "area": {"type": "string", "description": "Module scope, such as `orders` or `orders::Service`"},
            "lesson": {"type": "string", "description": "The lesson, in one line"},
            "source": {"type": "string", "description": "Where the lesson came from. Defaults to this run"}
        }, "required": ["area", "lesson"], "additionalProperties": false}),
    )
}

fn memory_recall_def() -> ToolDefinition {
    def(
        "MemoryRecall",
        MEMORY_RECALL,
        json!({"type": "object", "properties": {
            "query": {"type": "string", "description": "The task, or the error text of a failure"},
            "area": {"type": "string", "description": "Module scope to recall from first"},
            "limit": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum lessons to return. Defaults to 8"}
        }, "required": ["query"], "additionalProperties": false}),
    )
}

/// The `Document` tool for an agent that writes a typed document, with that document's schema.
pub fn document_tool_definition(agent: AgentName) -> Option<ToolDefinition> {
    let kind = ostra_core::doc::DocKind::for_agent(agent)?;
    let mut schema = kind.schema();
    let defs = schema
        .as_object_mut()
        .and_then(|m| m.remove("$defs"))
        .unwrap_or_else(|| json!({}));
    if let Some(m) = schema.as_object_mut() {
        m.remove("$schema");
    }
    let mut defs = match defs {
        serde_json::Value::Object(m) => m,
        _ => Default::default(),
    };
    defs.insert("Document".into(), schema);
    Some(def(
        "Document",
        DOCUMENT,
        json!({"type": "object", "properties": {
            "path": {"type": "string", "description": "Absolute path of the document's markdown file in your session dir"},
            "document": {"$ref": "#/$defs/Document", "description": "The whole document. Replaces what is stored"},
            "update": {"type": "object", "description": "Top-level fields to change. Lists of items with an `id` merge by id"},
            "remove": {"type": "array", "items": {"type": "string"}, "description": "Ids of list items to remove, such as `R4` or a phase number"}
        }, "required": ["path"], "additionalProperties": false, "$defs": defs}),
    ))
}

/// Local tool definitions for these capabilities, in declaration order. WebSearch is omitted: it
/// runs on the provider's side (see [`wants_web_search`]).
pub fn definitions(capabilities: &[Capability]) -> Vec<ToolDefinition> {
    let mut seen = std::collections::HashSet::new();
    capabilities
        .iter()
        .filter(|c| seen.insert(**c))
        .filter_map(|c| match c {
            Capability::Read => Some(read_def()),
            Capability::Write => Some(write_def()),
            Capability::Edit => Some(edit_def()),
            Capability::Shell => Some(bash_def()),
            Capability::SearchText => Some(grep_def()),
            Capability::Glob => Some(glob_def()),
            Capability::Skill => Some(skill_def()),
            Capability::WebFetch => Some(web_fetch_def()),
            Capability::Report => Some(report_def()),
            // Agent-specific: see [`document_tool_definition`].
            Capability::Document => None,
            Capability::Memory => Some(memory_def()),
            Capability::MemoryRecall => Some(memory_recall_def()),
            Capability::WebSearch => None,
        })
        .collect()
}

/// True when the agent may search the web, so the loop enables the provider's search tool.
pub fn wants_web_search(capabilities: &[Capability]) -> bool {
    capabilities.contains(&Capability::WebSearch)
}

pub fn submit_tool_definition(agent: AgentName) -> ToolDefinition {
    ToolDefinition {
        name: agent.submit_tool_name(),
        description: ostra_core::submit::submit_description(agent),
        input_schema: ostra_core::submit::submit_schema(agent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_capabilities_and_skips_web_search() {
        let caps = [
            Capability::Read,
            Capability::Shell,
            Capability::WebSearch,
            Capability::Read,
            Capability::MemoryRecall,
        ];
        let names: Vec<String> = definitions(&caps).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["Read", "Bash", "MemoryRecall"]);
        assert!(wants_web_search(&caps));
        assert!(!wants_web_search(&[Capability::Read]));
    }

    #[test]
    fn every_capability_has_a_native_name_match() {
        for c in [
            Capability::Read,
            Capability::Write,
            Capability::Edit,
            Capability::Shell,
            Capability::SearchText,
            Capability::Glob,
            Capability::Skill,
            Capability::WebFetch,
            Capability::Report,
            Capability::Memory,
            Capability::MemoryRecall,
        ] {
            let d = definitions(&[c]);
            assert_eq!(d[0].name, c.native_tool());
            assert_eq!(d[0].input_schema["type"], "object");
        }
    }

    #[test]
    fn descriptions_follow_writing_rules() {
        let caps: Vec<Capability> = vec![
            Capability::Read,
            Capability::Write,
            Capability::Edit,
            Capability::Shell,
            Capability::SearchText,
            Capability::Glob,
            Capability::Skill,
            Capability::WebFetch,
            Capability::Report,
            Capability::Memory,
            Capability::MemoryRecall,
        ];
        for d in definitions(&caps) {
            assert!(
                !d.description.contains('\u{2014}'),
                "{} has an em dash",
                d.name
            );
        }
    }

    #[test]
    fn submit_definition() {
        let d = submit_tool_definition(AgentName::CodeReviewer);
        assert_eq!(d.name, "submit_code_reviewer");
        assert!(d.input_schema.get("properties").is_some());
    }
}

#[cfg(test)]
mod document_tests {
    use super::*;

    /// Every `$ref` in the Document schema points at a definition it carries.
    #[test]
    fn document_schema_refs_resolve() {
        for agent in [AgentName::Explore, AgentName::GenerateSpec, AgentName::Plan] {
            let d = document_tool_definition(agent).unwrap();
            let text = d.input_schema.to_string();
            let defs = d.input_schema["$defs"].as_object().unwrap();
            for part in text.split("\"#/$defs/").skip(1) {
                let name = part.split('"').next().unwrap();
                assert!(defs.contains_key(name), "{agent}: dangling ref {name}");
            }
            assert!(!d.description.contains('\u{2014}'));
        }
        assert!(document_tool_definition(AgentName::Implementer).is_none());
    }
}
