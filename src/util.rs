//! Utilitaires partagés : erreurs, système de fichiers, affichage.

use std::fs;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

/// Ajoute un contexte lisible à une erreur d'entrée/sortie.
pub fn ctx<T>(r: std::io::Result<T>, msg: impl AsRef<str>) -> Result<T> {
    r.map_err(|e| format!("{} : {}", msg.as_ref(), e))
}

/// Ligne d'état à la façon de Cargo (sur stderr pour garder stdout propre).
pub fn status(verb: &str, msg: impl AsRef<str>) {
    eprintln!("{:>13} {}", verb, msg.as_ref());
}

/// Un identifiant Kastel valide (nom de paquet / de dépendance = nom de module).
pub fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Hachage FNV-1a 64 bits (noms stables pour le cache git).
pub fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// `~/.gate` (ou `GATE_HOME`).
pub fn gate_home() -> Result<PathBuf> {
    if let Some(h) = std::env::var_os("GATE_HOME") {
        return Ok(PathBuf::from(h));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| "dossier personnel introuvable (définissez GATE_HOME)".to_string())?;
    Ok(PathBuf::from(home).join(".gate"))
}

/// Chemin canonique, sans le préfixe `\\?\` de Windows.
pub fn canon(p: &Path) -> Result<PathBuf> {
    let c = ctx(fs::canonicalize(p), format!("chemin introuvable : {}", p.display()))?;
    let s = c.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if !rest.starts_with("UNC\\") {
            return Ok(PathBuf::from(rest));
        }
    }
    drop(s);
    Ok(c)
}

/// Copie récursive (ignore `.git` et `target`).
pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    ctx(fs::create_dir_all(dst), format!("création de {}", dst.display()))?;
    for entry in ctx(fs::read_dir(src), format!("lecture de {}", src.display()))? {
        let entry = ctx(entry, "lecture d'une entrée")?;
        let name = entry.file_name();
        let n = name.to_string_lossy();
        if n == ".git" || n == "target" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let ft = ctx(entry.file_type(), "type de fichier")?;
        if ft.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            ctx(fs::copy(&from, &to), format!("copie de {}", from.display()))?;
        }
    }
    Ok(())
}

/// Tous les fichiers `.ks` d'un dossier, récursivement, triés.
pub fn collect_ks(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = ctx(fs::read_dir(dir), format!("lecture de {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_ks(&p, out)?;
        } else if p.extension().and_then(|e| e.to_str()) == Some("ks") {
            out.push(p);
        }
    }
    Ok(())
}

/// Chemin relatif à la racine du projet, pour l'affichage.
pub fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}
