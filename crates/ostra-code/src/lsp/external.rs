//! Files outside the project that a language server points at: library sources under a package
//! cache (`file://`) and classes inside jars (jdtls's `jdt://`). Ostra shows them read-only.

use crate::lang;
use serde_json::{Value, json};
use std::path::PathBuf;
use url::Url;

/// How Ostra shows an outside URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    pub name: String,
    pub path: String,
    pub language: Option<&'static str>,
}

/// The local path of a `file://` URI.
pub fn file_path(uri: &str) -> Option<PathBuf> {
    let u = Url::parse(uri).ok()?;
    (u.scheme() == "file")
        .then(|| u.to_file_path().ok())
        .flatten()
}

/// Name, display path, and language of an outside URI; `None` for a scheme Ostra cannot read.
pub fn describe(uri: &str) -> Option<Described> {
    let u = Url::parse(uri).ok()?;
    match u.scheme() {
        "file" => {
            // Decode the path from the URI rather than `to_file_path`, which is OS-specific and
            // rejects a POSIX path on Windows. `/C:/x` from a Windows URI drops its leading slash.
            let decoded = unescape(u.path());
            let full = match decoded.strip_prefix('/') {
                Some(rest)
                    if rest.len() >= 2
                        && rest.as_bytes()[0].is_ascii_alphabetic()
                        && rest.as_bytes()[1] == b':' =>
                {
                    rest.replace('/', "\\")
                }
                _ => decoded.clone(),
            };
            let name = full
                .rsplit(['/', '\\'])
                .find(|s| !s.is_empty())?
                .to_string();
            let home = ostra_core::paths::home()
                .map(|h| h.to_string_lossy().into_owned())
                .unwrap_or_default();
            let path = match full.strip_prefix(&home) {
                Some(rest) if !home.is_empty() && rest.starts_with('/') => format!("~{rest}"),
                _ => full,
            };
            Some(Described {
                language: lang::for_path(&name).map(|l| l.id),
                name,
                path,
            })
        }
        // `jdt://contents/<jar>/<package>/<Class>.class?<project and jar path>`.
        "jdt" => {
            let segs: Vec<String> = u
                .path_segments()?
                .filter(|s| !s.is_empty())
                .map(unescape)
                .collect();
            let name = segs.last()?.clone();
            let path = match (u.host_str(), segs.as_slice()) {
                (Some("contents"), [jar, package, class]) => {
                    format!("{jar} › {}/{class}", package.replace('.', "/"))
                }
                _ => segs.join("/"),
            };
            Some(Described {
                name,
                path,
                language: Some("java"),
            })
        }
        _ => None,
    }
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|&c| hex(c)),
                b.get(i + 2).and_then(|&c| hex(c)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `initializationOptions` with the client extensions Ostra handles for the server's languages:
/// jdtls answers definitions in jars with `jdt://` URIs only when the client says it reads them.
pub fn with_extensions(languages: &[String], options: Option<Value>) -> Option<Value> {
    if !languages.iter().any(|l| l == "java") {
        return options;
    }
    let mut o = match options {
        Some(Value::Object(m)) => Value::Object(m),
        None | Some(Value::Null) => json!({}),
        other => return other,
    };
    let ext = &mut o["extendedClientCapabilities"];
    if !ext.is_object() {
        *ext = json!({});
    }
    if ext.get("classFileContentsSupport").is_none() {
        ext["classFileContentsSupport"] = Value::Bool(true);
    }
    Some(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jar_classes_show_the_jar_and_the_class_path() {
        let d = describe("jdt://contents/jackson-databind-2.15.2.jar/com.fasterxml.jackson.databind/ObjectMapper.class?%3Dbackend%2F%5C%2Fhome%2Fu%2F.m2").unwrap();
        assert_eq!(d.name, "ObjectMapper.class");
        assert_eq!(
            d.path,
            "jackson-databind-2.15.2.jar › com/fasterxml/jackson/databind/ObjectMapper.class"
        );
        assert_eq!(d.language, Some("java"));
    }

    #[test]
    fn files_name_their_language_and_other_schemes_are_left_out() {
        let d = describe("file:///opt/go/src/fmt/print.go").unwrap();
        assert_eq!((d.name.as_str(), d.language), ("print.go", Some("go")));
        assert_eq!(d.path, "/opt/go/src/fmt/print.go");
        assert!(describe("untitled:Untitled-1").is_none());
        assert!(describe("not a uri").is_none());
    }

    #[test]
    fn java_servers_get_class_file_contents_unless_set() {
        let java = vec!["java".to_string()];
        assert_eq!(
            with_extensions(&java, None),
            Some(json!({"extendedClientCapabilities": {"classFileContentsSupport": true}}))
        );
        let own = json!({"settings": {"x": 1}, "extendedClientCapabilities": {"classFileContentsSupport": false}});
        assert_eq!(with_extensions(&java, Some(own.clone())), Some(own));
        assert_eq!(with_extensions(&["go".to_string()], None), None);
    }
}
