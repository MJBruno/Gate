//! Lecture de Gate.toml et édition textuelle (add / remove) qui préserve les commentaires.

use crate::toml::{self, Table, Value};
use crate::util::{ctx, is_ident, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "Gate.toml";
pub const LOCKFILE: &str = "Gate.lock";

#[derive(Debug, Clone, PartialEq)]
pub enum GitRef {
    Default,
    Branch(String),
    Tag(String),
    Rev(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Path(String),
    Git { url: String, reference: GitRef },
    Registry(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    pub name: String,
    pub source: Source,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Bin,
    Lib,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub main: Option<String>,
    pub kastel: Option<String>,
    pub dependencies: Vec<Dependency>,
    pub dev_dependencies: Vec<Dependency>,
    pub root: PathBuf,
}

impl Manifest {
    pub fn load(root: &Path) -> Result<Manifest> {
        let path = root.join(MANIFEST);
        let text = ctx(
            fs::read_to_string(&path),
            format!("lecture de {}", path.display()),
        )?;
        Self::parse(&text, root).map_err(|e| format!("{} : {}", path.display(), e))
    }

    pub fn parse(text: &str, root: &Path) -> Result<Manifest> {
        let t = toml::parse(text)?;
        let pkg = t
            .get("package")
            .and_then(Value::as_table)
            .ok_or("section [package] manquante")?;
        let name = req_str(pkg, "name")?;
        if !is_ident(&name) {
            return Err(format!(
                "nom de paquet invalide « {name} » (lettres, chiffres et « _ », sans commencer par un chiffre)"
            ));
        }
        let version = req_str(pkg, "version")?;
        check_version(&version)?;
        let description = opt_str(pkg, "description")?.unwrap_or_default();
        let main = opt_str(pkg, "main")?;
        let authors = match pkg.get("authors") {
            None => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .ok_or_else(|| "authors doit contenir des chaînes".to_string())
                })
                .collect::<Result<Vec<String>>>()?,
            Some(_) => return Err("authors doit être un tableau".into()),
        };
        let kastel = t
            .get("toolchain")
            .and_then(Value::as_table)
            .and_then(|tc| tc.get("kastel"))
            .and_then(Value::as_str)
            .map(String::from);
        Ok(Manifest {
            name,
            version,
            description,
            authors,
            main,
            kastel,
            dependencies: parse_deps(t.get("dependencies"))?,
            dev_dependencies: parse_deps(t.get("dev-dependencies"))?,
            root: root.to_path_buf(),
        })
    }

    /// Point d'entrée : `main` du manifeste, sinon src/main.ks (binaire), sinon src/lib.ks (bibliothèque).
    pub fn entry(&self) -> Result<(PathBuf, Kind)> {
        if let Some(m) = &self.main {
            let p = self.root.join(m);
            if !p.is_file() {
                return Err(format!("fichier principal introuvable : {}", p.display()));
            }
            let kind = if p.file_name().and_then(|f| f.to_str()) == Some("lib.ks") {
                Kind::Lib
            } else {
                Kind::Bin
            };
            return Ok((p, kind));
        }
        let main = self.root.join("src").join("main.ks");
        if main.is_file() {
            return Ok((main, Kind::Bin));
        }
        let lib = self.root.join("src").join("lib.ks");
        if lib.is_file() {
            return Ok((lib, Kind::Lib));
        }
        Err("aucun point d'entrée : créez src/main.ks (binaire) ou src/lib.ks (bibliothèque), ou définissez `main` dans [package]".into())
    }
}

fn req_str(t: &Table, key: &str) -> Result<String> {
    opt_str(t, key)?.ok_or_else(|| format!("champ « {key} » manquant dans [package]"))
}

fn opt_str(t: &Table, key: &str) -> Result<Option<String>> {
    match t.get(key) {
        None => Ok(None),
        Some(Value::Str(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("« {key} » doit être une chaîne")),
    }
}

pub fn check_version(v: &str) -> Result<()> {
    let parts: Vec<&str> = v.split('.').collect();
    let ok = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    if ok {
        Ok(())
    } else {
        Err(format!("version invalide « {v} » (format attendu : X.Y.Z)"))
    }
}

fn parse_deps(v: Option<&Value>) -> Result<Vec<Dependency>> {
    let mut out = Vec::new();
    let Some(v) = v else { return Ok(out) };
    let table = v
        .as_table()
        .ok_or("la section des dépendances doit être une table")?;
    for (name, spec) in table {
        if !is_ident(name) {
            return Err(format!("nom de dépendance invalide « {name} »"));
        }
        let source = match spec {
            Value::Str(ver) => Source::Registry(ver.clone()),
            Value::Table(t) => {
                let get = |k: &str| t.get(k).and_then(Value::as_str).map(String::from);
                if let Some(p) = get("path") {
                    Source::Path(p)
                } else if let Some(url) = get("git") {
                    let reference = match (get("tag"), get("branch"), get("rev")) {
                        (Some(x), None, None) => GitRef::Tag(x),
                        (None, Some(x), None) => GitRef::Branch(x),
                        (None, None, Some(x)) => GitRef::Rev(x),
                        (None, None, None) => GitRef::Default,
                        _ => return Err(format!(
                            "dépendance « {name} » : un seul parmi tag / branch / rev est permis"
                        )),
                    };
                    Source::Git { url, reference }
                } else if let Some(ver) = get("version") {
                    Source::Registry(ver)
                } else {
                    return Err(format!(
                        "dépendance « {name} » : `path` ou `git` est requis"
                    ));
                }
            }
            _ => return Err(format!("dépendance « {name} » : valeur invalide")),
        };
        out.push(Dependency {
            name: name.clone(),
            source,
        });
    }
    Ok(out)
}

/// Remonte les dossiers parents jusqu'à trouver un Gate.toml.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(MANIFEST).is_file() {
            return Ok(d.to_path_buf());
        }
        dir = d.parent();
    }
    Err(format!(
        "{MANIFEST} introuvable dans {} ni dans ses parents",
        start.display()
    ))
}

// ---------------------------------------------------------------------------
// Édition textuelle de Gate.toml
// ---------------------------------------------------------------------------

fn section_range(lines: &[String], header: &str) -> Option<(usize, usize)> {
    let target = format!("[{header}]");
    let start = lines.iter().position(|l| l.trim() == target)?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| l.trim_start().starts_with('['))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());
    Some((start, end))
}

fn is_key_line(line: &str, name: &str) -> bool {
    line.trim_start()
        .strip_prefix(name)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

fn finish(lines: Vec<String>) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Ajoute (ou remplace) `name = spec` dans `[section]`, en créant la section si besoin.
pub fn add_dependency_text(src: &str, section: &str, name: &str, spec: &str) -> String {
    let mut lines: Vec<String> = src.lines().map(String::from).collect();
    let new_line = format!("{name} = {spec}");
    match section_range(&lines, section) {
        Some((start, end)) => {
            let existing = (start + 1..end).find(|&i| is_key_line(&lines[i], name));
            if let Some(idx) = existing {
                lines[idx] = new_line;
            } else {
                let mut at = end;
                while at > start + 1 && lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                lines.insert(at, new_line);
            }
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{section}]"));
            lines.push(new_line);
        }
    }
    finish(lines)
}

/// Retire `name` de `[section]` (forme `name = ...` ou section `[section.name]`).
pub fn remove_dependency_text(src: &str, section: &str, name: &str) -> Option<String> {
    let mut lines: Vec<String> = src.lines().map(String::from).collect();
    if let Some((start, end)) = section_range(&lines, section) {
        let found = (start + 1..end).find(|&i| is_key_line(&lines[i], name));
        if let Some(idx) = found {
            lines.remove(idx);
            return Some(finish(lines));
        }
    }
    if let Some((start, end)) = section_range(&lines, &format!("{section}.{name}")) {
        lines.drain(start..end);
        return Some(finish(lines));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n# commentaire\n\n[dev-dependencies]\n";

    #[test]
    fn parses_manifest_with_all_dependency_kinds() {
        let m = Manifest::parse(
            "[package]\nname = \"app\"\nversion = \"1.2.3\"\nauthors = [\"A\"]\n\n[dependencies]\nlocal = { path = \"../local\" }\nremote = { git = \"https://x/y.git\", tag = \"v1\" }\nreg = \"1.0.0\"\n\n[toolchain]\nkastel = \"kastel-nightly\"\n",
            Path::new("."),
        )
        .unwrap();
        assert_eq!(m.dependencies.len(), 3);
        assert_eq!(m.kastel.as_deref(), Some("kastel-nightly"));
        let remote = m.dependencies.iter().find(|d| d.name == "remote").unwrap();
        assert_eq!(
            remote.source,
            Source::Git {
                url: "https://x/y.git".into(),
                reference: GitRef::Tag("v1".into())
            }
        );
    }

    #[test]
    fn rejects_bad_name_version_and_conflicting_refs() {
        let p = Path::new(".");
        assert!(
            Manifest::parse("[package]\nname = \"mon-app\"\nversion = \"0.1.0\"\n", p).is_err()
        );
        assert!(Manifest::parse("[package]\nname = \"app\"\nversion = \"1.0\"\n", p).is_err());
        assert!(Manifest::parse(
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\n[dependencies]\nx = { git = \"u\", tag = \"t\", rev = \"r\" }\n",
            p
        )
        .is_err());
    }

    #[test]
    fn add_inserts_after_last_content_of_section() {
        let out = add_dependency_text(BASE, "dependencies", "utils", "{ path = \"../utils\" }");
        let deps = out.split("[dependencies]").nth(1).unwrap();
        assert!(deps.contains("# commentaire\nutils = { path = \"../utils\" }\n"));
        assert!(Manifest::parse(&out, Path::new(".")).is_ok());
    }

    #[test]
    fn add_replaces_existing_and_creates_missing_section() {
        let once = add_dependency_text(BASE, "dependencies", "utils", "{ path = \"a\" }");
        let twice = add_dependency_text(&once, "dependencies", "utils", "{ path = \"b\" }");
        assert_eq!(twice.matches("utils =").count(), 1);
        assert!(twice.contains("path = \"b\""));
        let fresh = add_dependency_text(
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\n",
            "dev-dependencies",
            "t",
            "{ path = \"t\" }",
        );
        assert!(fresh.contains("\n[dev-dependencies]\nt = { path = \"t\" }\n"));
    }

    #[test]
    fn remove_handles_inline_and_table_forms() {
        let with = add_dependency_text(BASE, "dependencies", "utils", "{ path = \"a\" }");
        let without = remove_dependency_text(&with, "dependencies", "utils").unwrap();
        assert!(!without.contains("utils"));
        assert!(remove_dependency_text(&without, "dependencies", "utils").is_none());
        let tabled = "[package]\nname = \"a\"\nversion = \"0.1.0\"\n[dependencies.utils]\npath = \"x\"\n[toolchain]\nkastel = \"k\"\n";
        let out = remove_dependency_text(tabled, "dependencies", "utils").unwrap();
        assert!(!out.contains("utils") && out.contains("[toolchain]"));
    }
}
