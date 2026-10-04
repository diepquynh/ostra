//! Workflow data (HANDOVER 10.11): references from one node to an earlier node's result,
//! conditions on them, and the typed transform functions Ostra ships. Everything here is a pure
//! function of JSON values, so the planner can evaluate conditions and the runner can run a
//! transform without either reading anything outside the session.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use ts_rs::TS;

/// Rule WB4: a reference names a node and a path into its result: `audit.data.risk`,
/// `count.output`, `audit.findings.0.file`, or a session fact such as `session.track`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub node: String,
    pub path: Vec<String>,
}

/// The reference roots that are not nodes.
pub const SESSION_REF: &str = "session";
pub const SCOPE_REF: &str = "scope";

pub fn parse_ref(s: &str) -> Result<Ref, String> {
    let s = s.trim();
    let mut parts = s.split('.');
    let node = parts.next().unwrap_or_default().to_string();
    let path: Vec<String> = parts.map(String::from).collect();
    let seg_ok = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    if node.is_empty() || !seg_ok(&node) || !path.iter().all(|p| seg_ok(p)) {
        return Err(format!(
            "Write a reference as `<node>.<field>`, such as `audit.data.risk`: `{s}` is not one."
        ));
    }
    Ok(Ref { node, path })
}

/// The value at `path` inside `v`, or null when any step is missing. A number step indexes an
/// array.
pub fn lookup(v: &Value, path: &[String]) -> Value {
    let mut cur = v;
    for p in path {
        let next = match cur {
            Value::Object(m) => m.get(p),
            Value::Array(a) => p.parse::<usize>().ok().and_then(|i| a.get(i)),
            _ => None,
        };
        match next {
            Some(n) => cur = n,
            None => return Value::Null,
        }
    }
    cur.clone()
}

/// Rule WB5: how a condition compares a referenced value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CondOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    /// A string holds the text, an array holds the value, or an object has the key.
    Contains,
    /// The value is one of an array's items.
    In,
    /// The value is present and not null.
    Exists,
    /// The value is null, `""`, `[]`, or `{}`.
    Empty,
    NotEmpty,
    /// Anything but null, false, 0, `""`, `[]`, and `{}`.
    Truthy,
    Falsy,
}

impl CondOp {
    pub const ALL: [CondOp; 13] = [
        CondOp::Eq,
        CondOp::Ne,
        CondOp::Gt,
        CondOp::Ge,
        CondOp::Lt,
        CondOp::Le,
        CondOp::Contains,
        CondOp::In,
        CondOp::Exists,
        CondOp::Empty,
        CondOp::NotEmpty,
        CondOp::Truthy,
        CondOp::Falsy,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CondOp::Eq => "eq",
            CondOp::Ne => "ne",
            CondOp::Gt => "gt",
            CondOp::Ge => "ge",
            CondOp::Lt => "lt",
            CondOp::Le => "le",
            CondOp::Contains => "contains",
            CondOp::In => "in",
            CondOp::Exists => "exists",
            CondOp::Empty => "empty",
            CondOp::NotEmpty => "not_empty",
            CondOp::Truthy => "truthy",
            CondOp::Falsy => "falsy",
        }
    }

    pub fn parse(s: &str) -> Option<CondOp> {
        CondOp::ALL.into_iter().find(|o| o.as_str() == s.trim())
    }

    /// Whether the operator compares against a value.
    pub fn takes_value(self) -> bool {
        !matches!(
            self,
            CondOp::Exists | CondOp::Empty | CondOp::NotEmpty | CondOp::Truthy | CondOp::Falsy
        )
    }

    pub fn eval(self, left: &Value, right: &Value) -> bool {
        use std::cmp::Ordering;
        let order = || -> Option<Ordering> {
            match (left, right) {
                (Value::Number(a), Value::Number(b)) => a.as_f64()?.partial_cmp(&b.as_f64()?),
                (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
                _ => None,
            }
        };
        match self {
            CondOp::Eq => same(left, right),
            CondOp::Ne => !same(left, right),
            CondOp::Gt => order() == Some(Ordering::Greater),
            CondOp::Ge => matches!(order(), Some(Ordering::Greater | Ordering::Equal)),
            CondOp::Lt => order() == Some(Ordering::Less),
            CondOp::Le => matches!(order(), Some(Ordering::Less | Ordering::Equal)),
            CondOp::Contains => match (left, right) {
                (Value::String(a), Value::String(b)) => a.contains(b.as_str()),
                (Value::Array(a), b) => a.iter().any(|x| same(x, b)),
                (Value::Object(m), Value::String(k)) => m.contains_key(k),
                _ => false,
            },
            CondOp::In => match right {
                Value::Array(a) => a.iter().any(|x| same(x, left)),
                Value::String(s) => left.as_str().is_some_and(|l| s.contains(l)),
                _ => false,
            },
            CondOp::Exists => !left.is_null(),
            CondOp::Empty => empty(left),
            CondOp::NotEmpty => !empty(left),
            CondOp::Truthy => truthy(left),
            CondOp::Falsy => !truthy(left),
        }
    }
}

/// JSON equality where `1` equals `1.0`.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

fn empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(m) => m.is_empty(),
        _ => false,
    }
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        other => !empty(other),
    }
}

/// Rule WB5: one condition of a node's `when`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Condition {
    /// The reference it reads, such as `audit.verdict`.
    #[serde(rename = "ref")]
    pub reference: String,
    pub op: CondOp,
    /// What `op` compares against, for the operators that take one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub value: Option<Value>,
}

/// Whether every condition must hold, or one is enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WhenMode {
    #[default]
    All,
    Any,
}

impl WhenMode {
    pub fn is_all(&self) -> bool {
        *self == WhenMode::All
    }
}

// ---------------------------------------------------------------------------------------------
// Transform functions (Rule WB2)
// ---------------------------------------------------------------------------------------------

/// The JSON type a transform's input, argument, or output has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ValueKind {
    Any,
    Bool,
    Number,
    String,
    Array,
    Object,
}

impl ValueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ValueKind::Any => "any",
            ValueKind::Bool => "bool",
            ValueKind::Number => "number",
            ValueKind::String => "string",
            ValueKind::Array => "array",
            ValueKind::Object => "object",
        }
    }

    pub fn matches(self, v: &Value) -> bool {
        match self {
            ValueKind::Any => true,
            ValueKind::Bool => v.is_boolean(),
            ValueKind::Number => v.is_number(),
            ValueKind::String => v.is_string(),
            ValueKind::Array => v.is_array(),
            ValueKind::Object => v.is_object(),
        }
    }
}

/// One named input or argument of a transform function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TransformParam {
    pub name: String,
    pub kind: ValueKind,
    #[serde(default = "yes")]
    pub required: bool,
    #[serde(default)]
    pub description: String,
}

fn yes() -> bool {
    true
}

/// A transform function as the browser lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TransformInfo {
    pub name: String,
    pub description: String,
    /// Values wired from earlier nodes (`inputs`).
    pub inputs: Vec<TransformParam>,
    /// True when the function takes any number of inputs under any names, in name order.
    pub variadic: bool,
    /// Fixed values set on the node (`args`).
    pub args: Vec<TransformParam>,
    pub output: ValueKind,
    /// Rule WB7: the workspace's own composite function, from `.ostra/transforms/`.
    #[serde(default)]
    pub custom: bool,
    /// Rule PL7: the plugin that runs it in code, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plugin: Option<String>,
}

fn param(name: &str, kind: ValueKind, required: bool, description: &str) -> TransformParam {
    TransformParam {
        name: name.into(),
        kind,
        required,
        description: description.into(),
    }
}

fn info(
    name: &str,
    description: &str,
    inputs: Vec<TransformParam>,
    variadic: bool,
    args: Vec<TransformParam>,
    output: ValueKind,
) -> TransformInfo {
    TransformInfo {
        name: name.into(),
        description: description.into(),
        inputs,
        variadic,
        args,
        output,
        custom: false,
        plugin: None,
    }
}

/// Every transform function Ostra ships, in the order the builder lists them.
pub fn transforms() -> Vec<TransformInfo> {
    use ValueKind::*;
    let items = || param("items", Array, true, "The list to work on.");
    let field = |required| {
        param(
            "field",
            String,
            required,
            "A dotted path into each item, such as `file` or `data.risk`.",
        )
    };
    let op = || {
        param(
            "op",
            String,
            true,
            "eq, ne, gt, ge, lt, le, contains, in, exists, empty, not_empty, truthy, or falsy.",
        )
    };
    let to = || {
        param(
            "to",
            Any,
            false,
            "The value `op` compares against, for the operators that take one.",
        )
    };
    vec![
        info(
            "pick",
            "Takes one field out of a value.",
            vec![param("value", Any, true, "The value to read.")],
            false,
            vec![param(
                "path",
                String,
                true,
                "A dotted path, such as `data.risk` or `findings.0`.",
            )],
            Any,
        ),
        info(
            "count",
            "Counts the items of a list, the keys of an object, or the characters of a text. Null counts 0.",
            vec![param("items", Any, true, "The value to count.")],
            false,
            vec![],
            Number,
        ),
        info(
            "filter",
            "Keeps the items of a list whose field passes a comparison.",
            vec![items()],
            false,
            vec![field(false), op(), to()],
            Array,
        ),
        info(
            "map",
            "Takes one field out of every item of a list.",
            vec![items()],
            false,
            vec![field(true)],
            Array,
        ),
        info(
            "concat",
            "Joins its inputs into one list, in input name order. An input that is not a list adds one item.",
            vec![],
            true,
            vec![],
            Array,
        ),
        info(
            "unique",
            "Drops repeated items from a list, keeping the first of each.",
            vec![items()],
            false,
            vec![],
            Array,
        ),
        info(
            "sort",
            "Sorts a list by its items or by a field of each item.",
            vec![items()],
            false,
            vec![
                field(false),
                param(
                    "descending",
                    Bool,
                    false,
                    "True sorts from largest to smallest.",
                ),
            ],
            Array,
        ),
        info(
            "group_count",
            "Counts the items of a list by the value of a field: `{\"high\": 2, \"low\": 5}`.",
            vec![items()],
            false,
            vec![field(true)],
            Object,
        ),
        info(
            "sum",
            "Adds up the numbers of a list, or a numeric field of each item.",
            vec![items()],
            false,
            vec![field(false)],
            Number,
        ),
        info(
            "compare",
            "Compares a value and returns true or false, for a branch that later nodes' conditions read.",
            vec![param("value", Any, true, "The value to compare.")],
            false,
            vec![op(), to()],
            Bool,
        ),
        info(
            "all",
            "True when every input is truthy.",
            vec![],
            true,
            vec![],
            Bool,
        ),
        info(
            "any",
            "True when at least one input is truthy.",
            vec![],
            true,
            vec![],
            Bool,
        ),
        info(
            "not",
            "True when its input is falsy.",
            vec![param("value", Any, true, "The value to negate.")],
            false,
            vec![],
            Bool,
        ),
        info(
            "merge",
            "Merges object inputs into one object, in input name order; a later key wins.",
            vec![],
            true,
            vec![],
            Object,
        ),
        info(
            "template",
            "Writes a text with `{{name}}` or `{{name.path}}` replaced by inputs. Text inputs go in as they are, others as JSON.",
            vec![],
            true,
            vec![param("text", String, true, "The text with placeholders.")],
            String,
        ),
        info(
            "constant",
            "Returns a fixed value, for a setting several nodes read.",
            vec![],
            false,
            vec![param("value", Any, true, "The value to return.")],
            Any,
        ),
        info(
            "findings",
            "Collects the `findings` of node results into one list. Inputs are node results or lists of them.",
            vec![],
            true,
            vec![],
            Array,
        ),
    ]
}

pub fn transform_info(name: &str) -> Option<TransformInfo> {
    transforms().into_iter().find(|t| t.name == name)
}

/// Rule WB2: the problems with a transform node's function, inputs, and arguments.
pub fn check_transform(
    node: &str,
    function: &str,
    inputs: &[String],
    args: &Map<String, Value>,
    functions: &Functions,
    plugin: &PluginTransforms,
) -> Vec<String> {
    let Some(f) = function_info_with(function, functions, plugin) else {
        return vec![format!(
            "Set `transform` of node `{node}` to a function Ostra or the workspace has, such as `filter` or `count`: `{function}` is not one."
        )];
    };
    let mut out = vec![];
    if !f.variadic {
        for p in f.inputs.iter().filter(|p| p.required) {
            if !inputs.contains(&p.name) {
                out.push(format!(
                    "Wire input `{}` of node `{node}`: `{function}` needs it.",
                    p.name
                ));
            }
        }
        for i in inputs {
            if !f.inputs.iter().any(|p| &p.name == i) {
                out.push(format!(
                    "Remove input `{i}` from node `{node}`: `{function}` takes {}.",
                    names(&f.inputs)
                ));
            }
        }
    } else if inputs.is_empty() {
        out.push(format!(
            "Wire at least one input into node `{node}`: `{function}` works on its inputs."
        ));
    }
    for p in f.args.iter().filter(|p| p.required) {
        if !args.contains_key(&p.name) {
            out.push(format!(
                "Set argument `{}` of node `{node}`: `{function}` needs it.",
                p.name
            ));
        }
    }
    for (k, v) in args {
        match f.args.iter().find(|p| &p.name == k) {
            None => out.push(format!(
                "Remove argument `{k}` from node `{node}`: `{function}` takes {}.",
                names(&f.args)
            )),
            Some(p) if !p.kind.matches(v) => out.push(format!(
                "Set argument `{k}` of node `{node}` to a {}.",
                p.kind.as_str()
            )),
            Some(_) => {}
        }
    }
    if !f.custom
        && f.plugin.is_none()
        && let Some(op) = args.get("op").and_then(Value::as_str)
        && CondOp::parse(op).is_none()
    {
        out.push(format!(
            "Set `op` of node `{node}` to one of {}: `{op}` is not one.",
            CondOp::ALL.map(|o| o.as_str()).join(", ")
        ));
    }
    out
}

fn names(ps: &[TransformParam]) -> String {
    if ps.is_empty() {
        return "none".into();
    }
    ps.iter()
        .map(|p| format!("`{}`", p.name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn path_of(args: &Map<String, Value>, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.split('.').map(String::from).collect())
        .unwrap_or_default()
}

fn list<'a>(inputs: &'a Map<String, Value>, key: &str) -> Result<&'a Vec<Value>, String> {
    match inputs.get(key) {
        Some(Value::Array(a)) => Ok(a),
        Some(Value::Null) | None => Err(format!("Input `{key}` is empty, and a list is needed.")),
        Some(_) => Err(format!("Input `{key}` is not a list.")),
    }
}

/// Rule WB2: run transform `function`. The same inputs and arguments give the same output.
pub fn run_transform(
    function: &str,
    inputs: &Map<String, Value>,
    args: &Map<String, Value>,
) -> Result<Value, String> {
    let ordered = || inputs.values();
    let cmp = |args: &Map<String, Value>| -> (CondOp, Value) {
        (
            args.get("op")
                .and_then(Value::as_str)
                .and_then(CondOp::parse)
                .unwrap_or(CondOp::Truthy),
            args.get("to").cloned().unwrap_or(Value::Null),
        )
    };
    Ok(match function {
        "pick" => lookup(
            inputs.get("value").unwrap_or(&Value::Null),
            &path_of(args, "path"),
        ),
        "count" => Value::from(match inputs.get("items").unwrap_or(&Value::Null) {
            Value::Null => 0,
            Value::Array(a) => a.len(),
            Value::Object(m) => m.len(),
            Value::String(s) => s.chars().count(),
            _ => 1,
        }),
        "filter" => {
            let field = path_of(args, "field");
            let (op, to) = cmp(args);
            Value::Array(
                list(inputs, "items")?
                    .iter()
                    .filter(|i| op.eval(&lookup(i, &field), &to))
                    .cloned()
                    .collect(),
            )
        }
        "map" => {
            let field = path_of(args, "field");
            Value::Array(
                list(inputs, "items")?
                    .iter()
                    .map(|i| lookup(i, &field))
                    .collect(),
            )
        }
        "concat" => {
            let mut out = vec![];
            for v in ordered() {
                match v {
                    Value::Array(a) => out.extend(a.iter().cloned()),
                    Value::Null => {}
                    other => out.push(other.clone()),
                }
            }
            Value::Array(out)
        }
        "unique" => {
            let mut out: Vec<Value> = vec![];
            for i in list(inputs, "items")? {
                if !out.iter().any(|o| same(o, i)) {
                    out.push(i.clone());
                }
            }
            Value::Array(out)
        }
        "sort" => {
            let field = path_of(args, "field");
            let mut out = list(inputs, "items")?.clone();
            out.sort_by(|a, b| order_values(&lookup(a, &field), &lookup(b, &field)));
            if args.get("descending").and_then(Value::as_bool) == Some(true) {
                out.reverse();
            }
            Value::Array(out)
        }
        "group_count" => {
            let field = path_of(args, "field");
            let mut out = Map::new();
            for i in list(inputs, "items")? {
                let key = match lookup(i, &field) {
                    Value::String(s) => s,
                    Value::Null => "null".into(),
                    other => other.to_string(),
                };
                let n = out.get(&key).and_then(Value::as_u64).unwrap_or(0) + 1;
                out.insert(key, Value::from(n));
            }
            Value::Object(out)
        }
        "sum" => {
            let field = path_of(args, "field");
            let total: f64 = list(inputs, "items")?
                .iter()
                .filter_map(|i| lookup(i, &field).as_f64())
                .sum();
            number(total)
        }
        "compare" => {
            let (op, to) = cmp(args);
            Value::Bool(op.eval(inputs.get("value").unwrap_or(&Value::Null), &to))
        }
        "all" => Value::Bool(ordered().all(truthy)),
        "any" => Value::Bool(ordered().any(truthy)),
        "not" => Value::Bool(!truthy(inputs.get("value").unwrap_or(&Value::Null))),
        "merge" => {
            let mut out = Map::new();
            for v in ordered() {
                match v {
                    Value::Object(m) => out.extend(m.clone()),
                    Value::Null => {}
                    _ => return Err("Every input of `merge` must be an object.".into()),
                }
            }
            Value::Object(out)
        }
        "template" => Value::String(template(
            args.get("text").and_then(Value::as_str).unwrap_or_default(),
            inputs,
        )),
        "constant" => args.get("value").cloned().unwrap_or(Value::Null),
        "findings" => {
            let mut out = vec![];
            for v in ordered() {
                collect_findings(v, &mut out);
            }
            Value::Array(out)
        }
        other => return Err(format!("Ostra has no transform function `{other}`.")),
    })
}

fn number(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9.0e15 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

fn order_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .partial_cmp(&y.as_f64())
            .unwrap_or(Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        _ => a.to_string().cmp(&b.to_string()),
    }
}

fn collect_findings(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| collect_findings(x, out)),
        Value::Object(m) => {
            if let Some(Value::Array(f)) = m.get("findings") {
                out.extend(f.iter().cloned());
            }
        }
        _ => {}
    }
}

/// `{{name}}` and `{{name.path}}` replaced by the input's value: text as it is, else JSON.
pub fn template(text: &str, inputs: &Map<String, Value>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let key = after[..end].trim();
        let mut parts = key.split('.');
        let name = parts.next().unwrap_or_default();
        let path: Vec<String> = parts.map(String::from).collect();
        match inputs.get(name) {
            Some(v) => match lookup(v, &path) {
                Value::String(s) => out.push_str(&s),
                Value::Null => {}
                other => out.push_str(&other.to_string()),
            },
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------------------------
// Composite functions (Rule WB7)
// ---------------------------------------------------------------------------------------------

/// The workspace's composite functions by name.
pub type Functions = std::collections::BTreeMap<String, FunctionFile>;

/// How deep composites may call composites, because each level is a nested run.
pub const MAX_FUNCTION_DEPTH: usize = 8;
/// The most steps one composite may hold.
pub const MAX_FUNCTION_STEPS: usize = 32;

/// Rule WB7: one `.ostra/transforms/<name>.toml`: a reusable transform built from other transform
/// functions. Its steps run in order; a step reads the function's inputs as `input.<name>` and an
/// earlier step as `<step>.output`, and an argument value `"$<name>"` takes the function's argument.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct FunctionFile {
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "input", skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<TransformParam>,
    #[serde(default, rename = "arg", skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<TransformParam>,
    /// The step whose output is the function's output.
    pub output: String,
    #[serde(default, rename = "step")]
    pub steps: Vec<FunctionStep>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct FunctionStep {
    pub id: String,
    pub transform: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub inputs: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    #[ts(type = "Record<string, unknown>")]
    pub args: Map<String, Value>,
}

/// The root a step uses to read the function's own inputs.
pub const INPUT_REF: &str = "input";

/// Rule PL7: plugin transform functions by their full name, `<plugin>:<name>`.
pub type PluginTransforms = std::collections::BTreeMap<String, TransformInfo>;

/// A transform function by name: Ostra's, a plugin's (`<plugin>:<name>`), or a composite.
pub fn function_info_with(
    name: &str,
    functions: &Functions,
    plugin: &PluginTransforms,
) -> Option<TransformInfo> {
    match plugin.get(name) {
        Some(t) => Some(TransformInfo {
            name: name.into(),
            ..t.clone()
        }),
        None => function_info(name, functions),
    }
}

/// A transform function by name: one of Ostra's, else one of `functions`.
pub fn function_info(name: &str, functions: &Functions) -> Option<TransformInfo> {
    transform_info(name).or_else(|| {
        let f = functions.get(name)?;
        Some(TransformInfo {
            name: name.into(),
            description: f.description.clone(),
            inputs: f.inputs.clone(),
            variadic: false,
            args: f.args.clone(),
            output: output_kind(name, functions, 0),
            custom: true,
            plugin: None,
        })
    })
}

fn output_kind(name: &str, functions: &Functions, depth: usize) -> ValueKind {
    if depth > MAX_FUNCTION_DEPTH {
        return ValueKind::Any;
    }
    if let Some(t) = transform_info(name) {
        return t.output;
    }
    functions
        .get(name)
        .and_then(|f| f.steps.iter().find(|s| s.id == f.output))
        .map(|s| output_kind(&s.transform, functions, depth + 1))
        .unwrap_or(ValueKind::Any)
}

/// Rule WB7: the composites `names` call, directly or through other composites.
pub fn functions_used<'a>(
    names: impl IntoIterator<Item = &'a str>,
    functions: &Functions,
) -> Functions {
    let mut out = Functions::new();
    let mut todo: Vec<String> = names.into_iter().map(String::from).collect();
    while let Some(n) = todo.pop() {
        if out.contains_key(&n) {
            continue;
        }
        if let Some(f) = functions.get(&n) {
            todo.extend(f.steps.iter().map(|s| s.transform.clone()));
            out.insert(n, f.clone());
        }
    }
    out
}

/// Rule WB7: the problems with composite `name`, checked against Ostra's functions and the
/// workspace's other composites.
pub fn check_function(name: &str, f: &FunctionFile, functions: &Functions) -> Vec<String> {
    let mut out = vec![];
    if transform_info(name).is_some() {
        out.push(format!(
            "Name the function something other than `{name}`, which is one of Ostra's transform functions."
        ));
    }
    let ident = |s: &str| {
        s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    for p in f.inputs.iter().chain(&f.args) {
        if !ident(&p.name) {
            out.push(format!(
                "Name `{}` with letters, digits, and `_`, starting with a letter.",
                p.name
            ));
        }
    }
    if f.steps.is_empty() {
        out.push("Add at least one step.".into());
    }
    if f.steps.len() > MAX_FUNCTION_STEPS {
        out.push(format!(
            "Keep at most {MAX_FUNCTION_STEPS} steps in one function."
        ));
    }
    let mut seen: Vec<&str> = vec![];
    for step in &f.steps {
        let id = step.id.as_str();
        if !ident(id) && !crate::workflow::valid_name(id) {
            out.push(format!("Name step `{id}` in lowercase kebab-case."));
        }
        if seen.contains(&id) || id == INPUT_REF {
            out.push(format!("Give step `{id}` its own id."));
        }
        if calls(&step.transform, name, functions, 0) {
            out.push(format!(
                "Step `{id}` calls `{}`, which leads back to `{name}`. A function cannot call itself.",
                step.transform
            ));
        } else {
            let mut args = step.args.clone();
            for (k, v) in step.args.iter() {
                if let Some(arg) = v.as_str().and_then(|v| v.strip_prefix('$')) {
                    match f.args.iter().find(|a| a.name == arg) {
                        // The argument's own kind stands in for the value it will get.
                        Some(a) => {
                            args.insert(k.clone(), sample(a.kind));
                        }
                        None => out.push(format!(
                            "Step `{id}` uses `${arg}`, which is not an argument of the function. Declare it under `[[arg]]`."
                        )),
                    }
                }
            }
            let inputs: Vec<String> = step.inputs.keys().cloned().collect();
            if step.transform.contains(':') {
                out.push(format!(
                    "Step `{id}` calls plugin function `{}`. A composite runs inside Ostra in one step, so it calls only Ostra's functions and the workspace's own.",
                    step.transform
                ));
            } else {
                out.extend(
                    check_transform(
                        id,
                        &step.transform,
                        &inputs,
                        &args,
                        functions,
                        &PluginTransforms::new(),
                    )
                    .into_iter()
                    .map(|e| e.replace("node `", "step `")),
                );
            }
        }
        for (k, r) in &step.inputs {
            match parse_ref(r) {
                Err(e) => out.push(format!("Input `{k}` of step `{id}`: {e}")),
                Ok(r) if r.node == INPUT_REF => {
                    if !r.path.first().is_some_and(|p| f.inputs.iter().any(|i| &i.name == p)) {
                        out.push(format!(
                            "Input `{k}` of step `{id}` reads `{}`, which is not an input of the function. Declare it under `[[input]]`.",
                            r.path.join(".")
                        ));
                    }
                }
                Ok(r) if !seen.contains(&r.node.as_str()) => out.push(format!(
                    "Input `{k}` of step `{id}` reads `{}`, which is not an earlier step. Steps run in order.",
                    r.node
                )),
                Ok(_) => {}
            }
        }
        seen.push(id);
    }
    if !f.steps.iter().any(|s| s.id == f.output) {
        out.push(format!(
            "Set `output` to one of the steps: `{}` is not one.",
            f.output
        ));
    }
    out
}

/// A value of `kind`, to check an argument the function passes on.
fn sample(kind: ValueKind) -> Value {
    match kind {
        ValueKind::Bool => Value::Bool(false),
        ValueKind::Number => Value::from(0),
        ValueKind::String | ValueKind::Any => Value::String(String::new()),
        ValueKind::Array => Value::Array(vec![]),
        ValueKind::Object => Value::Object(Map::new()),
    }
}

/// Whether `function` reaches `target` through composite calls.
fn calls(function: &str, target: &str, functions: &Functions, depth: usize) -> bool {
    if function == target {
        return true;
    }
    if depth > MAX_FUNCTION_DEPTH {
        return true;
    }
    functions.get(function).is_some_and(|f| {
        f.steps
            .iter()
            .any(|s| calls(&s.transform, target, functions, depth + 1))
    })
}

/// Rules WB2 and WB7: run a transform function, Ostra's or a composite of `functions`.
pub fn run_function(
    function: &str,
    inputs: &Map<String, Value>,
    args: &Map<String, Value>,
    functions: &Functions,
) -> Result<Value, String> {
    run_depth(function, inputs, args, functions, 0)
}

fn run_depth(
    function: &str,
    inputs: &Map<String, Value>,
    args: &Map<String, Value>,
    functions: &Functions,
    depth: usize,
) -> Result<Value, String> {
    let Some(f) = functions
        .get(function)
        .filter(|_| transform_info(function).is_none())
    else {
        return run_transform(function, inputs, args);
    };
    if depth >= MAX_FUNCTION_DEPTH {
        return Err(format!(
            "`{function}` calls functions deeper than {MAX_FUNCTION_DEPTH} levels."
        ));
    }
    let mut env = Map::new();
    env.insert(INPUT_REF.into(), Value::Object(inputs.clone()));
    for step in &f.steps {
        let step_inputs: Map<String, Value> = step
            .inputs
            .iter()
            .map(|(k, r)| {
                let v = parse_ref(r)
                    .map(|r| match env.get(&r.node) {
                        Some(root) => lookup(root, &r.path),
                        None => Value::Null,
                    })
                    .unwrap_or(Value::Null);
                (k.clone(), v)
            })
            .collect();
        let step_args: Map<String, Value> = step
            .args
            .iter()
            .map(|(k, v)| {
                let v = match v.as_str().and_then(|s| s.strip_prefix('$')) {
                    Some(a) => args.get(a).cloned().unwrap_or(Value::Null),
                    None => v.clone(),
                };
                (k.clone(), v)
            })
            .collect();
        let out = run_depth(
            &step.transform,
            &step_inputs,
            &step_args,
            functions,
            depth + 1,
        )
        .map_err(|e| format!("`{function}` step `{}`: {e}", step.id))?;
        let mut v = Map::new();
        v.insert("output".into(), out);
        env.insert(step.id.clone(), Value::Object(v));
    }
    Ok(env
        .get(&f.output)
        .map(|v| lookup(v, &["output".to_string()]))
        .unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    fn check_transform(
        node: &str,
        function: &str,
        inputs: &[String],
        args: &Map<String, Value>,
    ) -> Vec<String> {
        super::check_transform(
            node,
            function,
            inputs,
            args,
            &Functions::new(),
            &PluginTransforms::new(),
        )
    }

    #[test]
    fn references_parse_and_read_paths() {
        let r = parse_ref("audit.data.risk").unwrap();
        assert_eq!(r.node, "audit");
        assert_eq!(r.path, vec!["data", "risk"]);
        assert!(parse_ref("audit..x").is_err());
        assert!(parse_ref("a b").is_err());
        let v = json!({"findings": [{"file": "a.rs"}]});
        assert_eq!(
            lookup(&v, &["findings".into(), "0".into(), "file".into()]),
            json!("a.rs")
        );
        assert_eq!(lookup(&v, &["nope".into()]), Value::Null);
    }

    #[test]
    fn conditions_compare_json() {
        assert!(CondOp::Eq.eval(&json!(1), &json!(1.0)));
        assert!(CondOp::Gt.eval(&json!(3), &json!(2)));
        assert!(!CondOp::Gt.eval(&json!("3"), &json!(2)));
        assert!(CondOp::Contains.eval(&json!(["a", "b"]), &json!("b")));
        assert!(CondOp::In.eval(&json!("high"), &json!(["high", "medium"])));
        assert!(CondOp::Empty.eval(&json!([]), &Value::Null));
        assert!(CondOp::Truthy.eval(&json!(2), &Value::Null));
        assert!(CondOp::Falsy.eval(&json!(""), &Value::Null));
    }

    #[test]
    fn wb2_transforms_run_on_json() {
        let findings = json!([
            {"file": "a.rs", "risk": "high", "n": 2},
            {"file": "b.rs", "risk": "low", "n": 3},
            {"file": "a.rs", "risk": "high", "n": 1}
        ]);
        let items = m(json!({"items": findings}));
        let high = run_transform(
            "filter",
            &items,
            &m(json!({"field": "risk", "op": "eq", "to": "high"})),
        )
        .unwrap();
        assert_eq!(high.as_array().unwrap().len(), 2);
        let files = run_transform("map", &items, &m(json!({"field": "file"}))).unwrap();
        assert_eq!(
            run_transform("unique", &m(json!({"items": files})), &Map::new()).unwrap(),
            json!(["a.rs", "b.rs"])
        );
        assert_eq!(
            run_transform("group_count", &items, &m(json!({"field": "risk"}))).unwrap(),
            json!({"high": 2, "low": 1})
        );
        assert_eq!(
            run_transform("sum", &items, &m(json!({"field": "n"}))).unwrap(),
            json!(6)
        );
        let sorted = run_transform(
            "sort",
            &items,
            &m(json!({"field": "n", "descending": true})),
        )
        .unwrap();
        assert_eq!(sorted[0]["n"], json!(3));
        assert_eq!(
            run_transform("count", &items, &Map::new()).unwrap(),
            json!(3)
        );
        assert_eq!(
            run_transform(
                "compare",
                &m(json!({"value": 3})),
                &m(json!({"op": "ge", "to": 3}))
            )
            .unwrap(),
            json!(true)
        );
        assert_eq!(
            run_transform("concat", &m(json!({"a": [1], "b": 2})), &Map::new()).unwrap(),
            json!([1, 2])
        );
        assert_eq!(
            run_transform(
                "template",
                &m(json!({"n": 2, "who": {"name": "audit"}})),
                &m(json!({"text": "{{who.name}} found {{n}} {{missing}}"}))
            )
            .unwrap(),
            json!("audit found 2 {{missing}}")
        );
        assert_eq!(
            run_transform(
                "findings",
                &m(json!({"a": {"findings": [1]}, "b": [{"findings": [2]}]})),
                &Map::new()
            )
            .unwrap(),
            json!([1, 2])
        );
        assert!(run_transform("filter", &m(json!({"items": 3})), &Map::new()).is_err());
    }

    #[test]
    fn wb2_transform_nodes_are_checked() {
        assert!(check_transform("n", "count", &["items".into()], &Map::new()).is_empty());
        assert!(check_transform("n", "nope", &[], &Map::new())[0].contains("function"));
        let e = check_transform("n", "filter", &[], &m(json!({"op": "big", "x": 1})));
        assert!(e.iter().any(|x| x.contains("Wire input `items`")), "{e:?}");
        assert!(e.iter().any(|x| x.contains("Remove argument `x`")), "{e:?}");
        assert!(e.iter().any(|x| x.contains("`big` is not one")), "{e:?}");
        assert!(check_transform("n", "concat", &[], &Map::new())[0].contains("at least one"));
        assert!(
            check_transform(
                "n",
                "sort",
                &["items".into()],
                &m(json!({"descending": "y"}))
            )[0]
            .contains("bool")
        );
    }

    const HIGH_FILES: &str = r#"
description = "The files of findings at a risk."
output = "files"

[[input]]
name = "findings"
kind = "array"

[[arg]]
name = "risk"
kind = "string"

[[step]]
id = "high"
transform = "filter"
inputs = { items = "input.findings" }
args = { field = "risk", op = "eq", to = "$risk" }

[[step]]
id = "files"
transform = "map"
inputs = { items = "high.output" }
args = { field = "file" }
"#;

    fn funcs(files: &[(&str, &str)]) -> Functions {
        files
            .iter()
            .map(|(n, t)| (n.to_string(), toml::from_str(t).unwrap()))
            .collect()
    }

    /// Rule WB7: a composite runs its steps in order with its arguments substituted, and can be
    /// called by another composite.
    #[test]
    fn wb7_composites_run_and_nest() {
        let f = funcs(&[
            ("high-files", HIGH_FILES),
            (
                "high-count",
                "output = \"n\"\n[[input]]\nname = \"findings\"\nkind = \"array\"\n[[step]]\nid = \"files\"\ntransform = \"high-files\"\ninputs = { findings = \"input.findings\" }\nargs = { risk = \"high\" }\n[[step]]\nid = \"n\"\ntransform = \"count\"\ninputs = { items = \"files.output\" }\n",
            ),
        ]);
        for (n, file) in &f {
            assert!(
                check_function(n, file, &f).is_empty(),
                "{n}: {:?}",
                check_function(n, file, &f)
            );
        }
        let findings = m(json!({"findings": [
            {"file": "a.rs", "risk": "high"}, {"file": "b.rs", "risk": "low"}, {"file": "c.rs", "risk": "high"}
        ]}));
        assert_eq!(
            run_function("high-files", &findings, &m(json!({"risk": "high"})), &f).unwrap(),
            json!(["a.rs", "c.rs"])
        );
        assert_eq!(
            run_function("high-count", &findings, &Map::new(), &f).unwrap(),
            json!(2)
        );
        let info = function_info("high-count", &f).unwrap();
        assert!(info.custom);
        assert_eq!(
            info.output,
            ValueKind::Number,
            "the output kind follows the steps"
        );
        assert_eq!(
            functions_used(["high-count"], &f).len(),
            2,
            "a workflow records every composite it reaches"
        );
        let e = run_function(
            "high-files",
            &m(json!({"findings": 3})),
            &m(json!({"risk": "x"})),
            &f,
        )
        .unwrap_err();
        assert!(e.contains("step `high`"), "{e}");
    }

    #[test]
    fn wb7_broken_composites_are_refused_with_the_fix() {
        let check = |name: &str, text: &str| {
            let f = funcs(&[(name, text)]);
            check_function(name, &f[name], &f).join(" ")
        };
        let e = check(
            "loop",
            "output = \"a\"\n[[step]]\nid = \"a\"\ntransform = \"loop\"\n",
        );
        assert!(e.contains("cannot call itself"), "{e}");
        let e = check(
            "x",
            "output = \"a\"\n[[step]]\nid = \"a\"\ntransform = \"count\"\ninputs = { items = \"input.nope\" }\n",
        );
        assert!(e.contains("not an input of the function"), "{e}");
        let e = check(
            "x",
            "output = \"b\"\n[[step]]\nid = \"a\"\ntransform = \"count\"\ninputs = { items = \"b.output\" }\n[[step]]\nid = \"b\"\ntransform = \"constant\"\nargs = { value = 1 }\n",
        );
        assert!(e.contains("not an earlier step"), "{e}");
        let e = check(
            "x",
            "output = \"a\"\n[[step]]\nid = \"a\"\ntransform = \"constant\"\nargs = { value = \"$v\" }\n",
        );
        assert!(e.contains("`$v`"), "{e}");
        let e = check(
            "filter",
            "output = \"a\"\n[[step]]\nid = \"a\"\ntransform = \"constant\"\nargs = { value = 1 }\n",
        );
        assert!(e.contains("one of Ostra's"), "{e}");
        let e = check(
            "x",
            "output = \"zz\"\n[[step]]\nid = \"a\"\ntransform = \"constant\"\nargs = { value = 1 }\n",
        );
        assert!(e.contains("Set `output`"), "{e}");
        let e = check(
            "x",
            "output = \"a\"\n[[step]]\nid = \"a\"\ntransform = \"lib:double\"\n",
        );
        assert!(e.contains("calls plugin function `lib:double`"), "{e}");
    }
}
