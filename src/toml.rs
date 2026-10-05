//! Mini-parseur TOML (sans dépendance) : le sous-ensemble utilisé par Gate.toml et Gate.lock.
//!
//! Pris en charge : commentaires, sections `[a.b]`, clés pointées, chaînes (`"..."` / `'...'`),
//! entiers, booléens, tableaux (y compris sur plusieurs lignes) et tables en ligne `{ k = v }`.
//! Non pris en charge : dates, flottants, chaînes multilignes, `[[tableaux de tables]]`.

use crate::util::Result;
use std::collections::BTreeMap;

pub type Table = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    Array(Vec<Value>),
    Table(Table),
}
#[allow(dead_code)]
impl Value {
    pub fn as_str(&self) -> Option<&str> {
        if let Value::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub fn as_table(&self) -> Option<&Table> {
        if let Value::Table(t) = self {
            Some(t)
        } else {
            None
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        if let Value::Array(a) = self {
            Some(a)
        } else {
            None
        }
    }
}

/// Échappe une chaîne pour l'écrire entre guillemets.
pub fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn parse(src: &str) -> Result<Table> {
    let mut root = Table::new();
    let mut current: Vec<String> = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let lineno = i + 1;
        let line = strip_comment(lines[i]);
        let line = line.trim().to_string();
        i += 1;
        if line.is_empty() {
            continue;
        }
        if line.starts_with("[[") {
            return Err(format!(
                "ligne {lineno} : les tableaux de tables [[...]] ne sont pas pris en charge"
            ));
        }
        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Err(format!("ligne {lineno} : en-tête de section invalide"));
            }
            let inner = &line[1..line.len() - 1];
            current = split_key(inner).map_err(|e| format!("ligne {lineno} : {e}"))?;
            table_at(&mut root, &current).map_err(|e| format!("ligne {lineno} : {e}"))?;
            continue;
        }
        let eq = find_eq(&line).ok_or_else(|| format!("ligne {lineno} : « = » attendu"))?;
        let key_path = split_key(line[..eq].trim()).map_err(|e| format!("ligne {lineno} : {e}"))?;
        let mut value_text = line[eq + 1..].trim().to_string();
        while depth(&value_text) > 0 {
            if i >= lines.len() {
                return Err(format!("ligne {lineno} : valeur non terminée"));
            }
            value_text.push(' ');
            value_text.push_str(strip_comment(lines[i]).trim());
            i += 1;
        }
        let value = parse_value(&value_text).map_err(|e| format!("ligne {lineno} : {e}"))?;
        let table = table_at(&mut root, &current).map_err(|e| format!("ligne {lineno} : {e}"))?;
        let (last, parents) = key_path.split_last().ok_or("clé vide")?;
        let target = table_at(table, parents).map_err(|e| format!("ligne {lineno} : {e}"))?;
        if target.insert(last.clone(), value).is_some() {
            return Err(format!("ligne {lineno} : clé « {last} » en double"));
        }
    }
    Ok(root)
}

fn table_at<'a>(mut t: &'a mut Table, path: &[String]) -> Result<&'a mut Table> {
    for seg in path {
        let e = t
            .entry(seg.clone())
            .or_insert_with(|| Value::Table(Table::new()));
        match e {
            Value::Table(inner) => t = inner,
            _ => return Err(format!("« {seg} » n'est pas une table")),
        }
    }
    Ok(t)
}

fn strip_comment(line: &str) -> String {
    let mut out = String::new();
    let mut q: Option<char> = None;
    let mut esc = false;
    for c in line.chars() {
        match q {
            Some(qc) => {
                out.push(c);
                if esc {
                    esc = false;
                } else if qc == '"' && c == '\\' {
                    esc = true;
                } else if c == qc {
                    q = None;
                }
            }
            None => {
                if c == '#' {
                    break;
                }
                if c == '"' || c == '\'' {
                    q = Some(c);
                }
                out.push(c);
            }
        }
    }
    out
}

fn find_eq(s: &str) -> Option<usize> {
    let mut q: Option<char> = None;
    for (i, c) in s.char_indices() {
        match q {
            Some(qc) => {
                if c == qc {
                    q = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    q = Some(c);
                } else if c == '=' {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Profondeur de crochets/accolades ouverts (hors chaînes) : > 0 = valeur non terminée.
fn depth(s: &str) -> i32 {
    let mut d = 0;
    let mut q: Option<char> = None;
    let mut esc = false;
    for c in s.chars() {
        match q {
            Some(qc) => {
                if esc {
                    esc = false;
                } else if qc == '"' && c == '\\' {
                    esc = true;
                } else if c == qc {
                    q = None;
                }
            }
            None => match c {
                '"' | '\'' => q = Some(c),
                '[' | '{' => d += 1,
                ']' | '}' => d -= 1,
                _ => {}
            },
        }
    }
    d
}

fn split_key(s: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut q: Option<char> = None;
    for c in s.chars() {
        match q {
            Some(qc) => {
                if c == qc {
                    q = None;
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '"' | '\'' => q = Some(c),
                '.' => {
                    parts.push(cur.trim().to_string());
                    cur.clear();
                }
                _ => cur.push(c),
            },
        }
    }
    if q.is_some() {
        return Err("guillemet non fermé dans une clé".into());
    }
    parts.push(cur.trim().to_string());
    if parts.iter().any(|p| p.is_empty()) {
        return Err(format!("clé vide dans « {s} »"));
    }
    Ok(parts)
}

fn parse_value(text: &str) -> Result<Value> {
    let mut p = P {
        s: text.chars().collect(),
        i: 0,
    };
    let v = p.value()?;
    p.ws();
    if p.peek().is_some() {
        return Err("texte inattendu après la valeur".into());
    }
    Ok(v)
}

struct P {
    s: Vec<char>,
    i: usize,
}

impl P {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t')) {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Result<Value> {
        self.ws();
        match self.peek() {
            None => Err("valeur attendue".into()),
            Some('"') => Ok(Value::Str(self.basic_string()?)),
            Some('\'') => Ok(Value::Str(self.literal_string()?)),
            Some('[') => self.array(),
            Some('{') => self.inline_table(),
            Some(_) => self.bare(),
        }
    }

    fn basic_string(&mut self) -> Result<String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let c = self.peek().ok_or("chaîne non terminée")?;
            self.i += 1;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let e = self.peek().ok_or("séquence d'échappement incomplète")?;
                    self.i += 1;
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        other => return Err(format!("échappement \\{other} non pris en charge")),
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn literal_string(&mut self) -> Result<String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let c = self.peek().ok_or("chaîne non terminée")?;
            self.i += 1;
            if c == '\'' {
                return Ok(out);
            }
            out.push(c);
        }
    }

    fn array(&mut self) -> Result<Value> {
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.ws();
            match self.peek() {
                None => return Err("tableau non terminé".into()),
                Some(']') => {
                    self.i += 1;
                    return Ok(Value::Array(items));
                }
                _ => {}
            }
            items.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(',') => self.i += 1,
                Some(']') => {}
                _ => return Err("« , » ou « ] » attendu dans le tableau".into()),
            }
        }
    }

    fn inline_table(&mut self) -> Result<Value> {
        self.i += 1;
        let mut t = Table::new();
        loop {
            self.ws();
            match self.peek() {
                None => return Err("table en ligne non terminée".into()),
                Some('}') => {
                    self.i += 1;
                    return Ok(Value::Table(t));
                }
                _ => {}
            }
            let key = self.key()?;
            self.ws();
            if self.peek() != Some('=') {
                return Err(format!("« = » attendu après « {key} »"));
            }
            self.i += 1;
            let v = self.value()?;
            if t.insert(key.clone(), v).is_some() {
                return Err(format!("clé « {key} » en double"));
            }
            self.ws();
            match self.peek() {
                Some(',') => self.i += 1,
                Some('}') => {}
                _ => return Err("« , » ou « } » attendu dans la table".into()),
            }
        }
    }

    fn key(&mut self) -> Result<String> {
        match self.peek() {
            Some('"') => self.basic_string(),
            Some('\'') => self.literal_string(),
            _ => {
                let start = self.i;
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    self.i += 1;
                }
                if start == self.i {
                    return Err("clé attendue".into());
                }
                Ok(self.s[start..self.i].iter().collect())
            }
        }
    }

    fn bare(&mut self) -> Result<Value> {
        let start = self.i;
        while matches!(self.peek(), Some(c) if !c.is_whitespace() && c != ',' && c != ']' && c != '}')
        {
            self.i += 1;
        }
        let word: String = self.s[start..self.i].iter().collect();
        match word.as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => word
                .replace('_', "")
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| format!("valeur non reconnue : « {word} »")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_and_inline_dependency() {
        let t = parse(
            "[package]\nname = \"a\" # commentaire\nversion = \"1.0.0\"\n\n[dependencies]\nutils = { path = \"../utils\" }\n",
        )
        .unwrap();
        assert_eq!(t["package"].as_table().unwrap()["name"].as_str(), Some("a"));
        let dep = t["dependencies"].as_table().unwrap()["utils"]
            .as_table()
            .unwrap();
        assert_eq!(dep["path"].as_str(), Some("../utils"));
    }

    #[test]
    fn parses_multiline_array_and_scalars() {
        let t = parse("a = [\n  \"x\",\n  \"y\", # fin\n]\nn = 1_000\nb = true\n").unwrap();
        let arr = t["a"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(t["n"], Value::Int(1000));
        assert_eq!(t["b"], Value::Bool(true));
    }

    #[test]
    fn parses_dotted_section_header() {
        let t = parse("[dependencies.utils]\npath = \"x\"\n").unwrap();
        let utils = t["dependencies"].as_table().unwrap()["utils"]
            .as_table()
            .unwrap();
        assert_eq!(utils["path"].as_str(), Some("x"));
    }

    #[test]
    fn hash_inside_string_is_not_a_comment() {
        let t = parse("s = \"a # b\"\n").unwrap();
        assert_eq!(t["s"].as_str(), Some("a # b"));
    }

    #[test]
    fn rejects_duplicate_keys_and_garbage() {
        assert!(parse("a = 1\na = 2\n").is_err());
        assert!(parse("a = \n").is_err());
        assert!(parse("a = [1, 2\n").is_err());
        assert!(parse("[[x]]\n").is_err());
    }

    #[test]
    fn escape_roundtrip() {
        let src = format!("s = \"{}\"\n", escape("a\\b\"c"));
        assert_eq!(parse(&src).unwrap()["s"].as_str(), Some("a\\b\"c"));
    }
}
