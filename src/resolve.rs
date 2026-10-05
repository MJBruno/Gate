//! Résolution des dépendances (path + git), fichier Gate.lock et mise en place des modules.

use crate::manifest::{Dependency, GitRef, Manifest, Source, LOCKFILE, MANIFEST};
use crate::toml;
use crate::util::{canon, copy_dir, ctx, fnv, gate_home, status, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Ne jamais toucher au réseau.
    pub offline: bool,
    /// Échouer si Gate.lock devait changer.
    pub locked: bool,
    /// Ignorer Gate.lock et récupérer les dernières révisions (branches).
    pub update: bool,
}

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub root: PathBuf,
    /// `path+../utils` ou `git+URL#ref`.
    pub source: String,
    /// Révision exacte pour les dépendances git.
    pub rev: Option<String>,
    pub deps: Vec<String>,
}

pub struct Graph {
    pub root: Manifest,
    pub packages: BTreeMap<String, Package>,
    /// Dépendances directes (normales puis dev).
    pub direct: Vec<String>,
}

impl Graph {
    /// Paquets accessibles depuis [dependencies] (hors dev-dependencies pures).
    pub fn normal_reachable(&self) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut todo: Vec<String> = self
            .root
            .dependencies
            .iter()
            .map(|d| d.name.clone())
            .collect();
        while let Some(n) = todo.pop() {
            if !seen.insert(n.clone()) {
                continue;
            }
            if let Some(p) = self.packages.get(&n) {
                todo.extend(p.deps.iter().cloned());
            }
        }
        seen
    }
}

struct LockEntry {
    source: String,
    rev: Option<String>,
}

fn read_lock(root: &Path) -> BTreeMap<String, LockEntry> {
    let mut out = BTreeMap::new();
    let Ok(text) = fs::read_to_string(root.join(LOCKFILE)) else {
        return out;
    };
    let Ok(t) = toml::parse(&text) else {
        return out;
    };
    if let Some(pk) = t.get("package").and_then(toml::Value::as_table) {
        for (name, v) in pk {
            let Some(tb) = v.as_table() else { continue };
            if let Some(src) = tb.get("source").and_then(toml::Value::as_str) {
                let rev = tb
                    .get("rev")
                    .and_then(toml::Value::as_str)
                    .map(String::from);
                out.insert(
                    name.clone(),
                    LockEntry {
                        source: src.to_string(),
                        rev,
                    },
                );
            }
        }
    }
    out
}

fn git(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| format!("impossible de lancer git ({e}) — git est-il installé ?"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} a échoué : {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

struct Resolver<'a> {
    opts: &'a Options,
    lock: BTreeMap<String, LockEntry>,
    packages: BTreeMap<String, Package>,
    stack: Vec<String>,
}

type Located = (PathBuf, String, Option<String>);

impl Resolver<'_> {
    fn visit(&mut self, base: &Path, deps: &[Dependency]) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for dep in deps {
            if self.stack.contains(&dep.name) {
                let mut cycle = self.stack.clone();
                cycle.push(dep.name.clone());
                return Err(format!("dépendance cyclique : {}", cycle.join(" -> ")));
            }
            let (root, source, rev) = self.locate(base, dep)?;
            names.push(dep.name.clone());
            if let Some(existing) = self.packages.get(&dep.name) {
                if existing.root != root {
                    return Err(format!(
                        "conflit pour « {} » : {} et {} sont tous deux demandés",
                        dep.name, existing.source, source
                    ));
                }
                continue;
            }
            let m = Manifest::load(&root)?;
            if m.name != dep.name {
                return Err(format!(
                    "la dépendance « {} » pointe vers un paquet nommé « {} » ({})",
                    dep.name,
                    m.name,
                    root.display()
                ));
            }
            self.stack.push(dep.name.clone());
            let sub = self.visit(&root, &m.dependencies);
            self.stack.pop();
            let sub = sub?;
            self.packages.insert(
                dep.name.clone(),
                Package {
                    name: dep.name.clone(),
                    version: m.version.clone(),
                    root,
                    source,
                    rev,
                    deps: sub,
                },
            );
        }
        Ok(names)
    }

    fn locate(&self, base: &Path, dep: &Dependency) -> Result<Located> {
        match &dep.source {
            Source::Path(p) => {
                let dir = canon(&base.join(p))
                    .map_err(|e| format!("dépendance « {} » : {}", dep.name, e))?;
                if !dir.join(MANIFEST).is_file() {
                    return Err(format!(
                        "dépendance « {} » : {} ne contient pas de {}",
                        dep.name,
                        dir.display(),
                        MANIFEST
                    ));
                }
                Ok((dir, format!("path+{p}"), None))
            }
            Source::Git { url, reference } => self.locate_git(dep, url, reference),
            Source::Registry(v) => Err(format!(
                "dépendance « {} = \"{}\" » : le registre de paquets n'est pas encore disponible ; utilisez `path` ou `git`",
                dep.name, v
            )),
        }
    }

    fn locate_git(&self, dep: &Dependency, url: &str, reference: &GitRef) -> Result<Located> {
        let ref_key = match reference {
            GitRef::Default => "default".to_string(),
            GitRef::Branch(b) => format!("branch={b}"),
            GitRef::Tag(t) => format!("tag={t}"),
            GitRef::Rev(r) => format!("rev={r}"),
        };
        let source = format!("git+{url}#{ref_key}");
        let dir = gate_home()?
            .join("git")
            .join(format!("{}-{:016x}", dep.name, fnv(&source)));
        let dir_s = dir.to_string_lossy().to_string();
        let mut fetched = false;

        if !dir.join(".git").exists() {
            if self.opts.offline {
                return Err(format!(
                    "dépendance « {} » absente du cache et mode --offline actif",
                    dep.name
                ));
            }
            ctx(
                fs::create_dir_all(dir.parent().unwrap_or(&dir)),
                "création du cache Gate",
            )?;
            status("Clonage", format!("{} ({url})", dep.name));
            git(None, &["clone", "--quiet", url, dir_s.as_str()])?;
            fetched = true;
        } else if self.opts.update && !self.opts.offline {
            status("Mise à jour", &dep.name);
            git(
                Some(dir.as_path()),
                &["fetch", "--quiet", "--all", "--tags", "--prune"],
            )?;
            fetched = true;
        }

        let locked_rev = self
            .lock
            .get(&dep.name)
            .filter(|e| e.source == source && !self.opts.update)
            .and_then(|e| e.rev.clone());
        let target: String = match (&locked_rev, reference) {
            (Some(r), _) => r.clone(),
            (None, GitRef::Tag(t)) => t.clone(),
            (None, GitRef::Rev(r)) => r.clone(),
            (None, GitRef::Branch(b)) => format!("origin/{b}"),
            (None, GitRef::Default) => "origin/HEAD".to_string(),
        };

        let checkout = |t: &str| {
            git(
                Some(dir.as_path()),
                &["checkout", "--quiet", "--detach", t],
            )
        };
        if let Err(first) = checkout(target.as_str()) {
            if self.opts.offline || fetched {
                return Err(format!("dépendance « {} » : {first}", dep.name));
            }
            git(
                Some(dir.as_path()),
                &["fetch", "--quiet", "--all", "--tags", "--prune"],
            )?;
            checkout(target.as_str()).map_err(|e| format!("dépendance « {} » : {e}", dep.name))?;
        }
        let rev = git(Some(dir.as_path()), &["rev-parse", "HEAD"])?;
        Ok((canon(&dir)?, source, Some(rev)))
    }
}

/// Résout tout le graphe (dépendances normales + dev), met à jour Gate.lock.
pub fn resolve(root: &Path, opts: &Options) -> Result<Graph> {
    let manifest = Manifest::load(root)?;
    let mut deps = manifest.dependencies.clone();
    for d in &manifest.dev_dependencies {
        if !deps.iter().any(|x| x.name == d.name) {
            deps.push(d.clone());
        }
    }
    let mut r = Resolver {
        opts,
        lock: read_lock(root),
        packages: BTreeMap::new(),
        stack: vec![manifest.name.clone()],
    };
    let direct = r.visit(root, &deps)?;
    let graph = Graph {
        root: manifest,
        packages: r.packages,
        direct,
    };
    write_lock(&graph, opts)?;
    Ok(graph)
}

fn lock_text(g: &Graph) -> String {
    let mut s =
        String::from("# Fichier généré automatiquement par Gate. Ne pas modifier à la main.\n");
    for (name, p) in &g.packages {
        s.push_str(&format!(
            "\n[package.{name}]\nversion = \"{}\"\nsource = \"{}\"\n",
            toml::escape(&p.version),
            toml::escape(&p.source)
        ));
        if let Some(r) = &p.rev {
            s.push_str(&format!("rev = \"{}\"\n", toml::escape(r)));
        }
        if !p.deps.is_empty() {
            let list: Vec<String> = p.deps.iter().map(|d| format!("\"{d}\"")).collect();
            s.push_str(&format!("deps = [{}]\n", list.join(", ")));
        }
    }
    s
}

fn write_lock(g: &Graph, opts: &Options) -> Result<()> {
    let path = g.root.root.join(LOCKFILE);
    let text = lock_text(g);
    let existing = fs::read_to_string(&path).ok();
    if existing.is_none() && g.packages.is_empty() {
        return Ok(());
    }
    if existing.as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    if opts.locked {
        return Err(format!(
            "{LOCKFILE} doit être mis à jour, mais --locked est actif"
        ));
    }
    ctx(fs::write(&path, text), format!("écriture de {}", path.display()))
}

/// Copie `src/` de chaque dépendance dans `target/gate/modules/<nom>/`.
/// Retourne les dossiers de recherche de modules : `<projet>/src` puis `target/gate/modules`.
pub fn stage(g: &Graph, with_dev: bool) -> Result<Vec<PathBuf>> {
    let modules = g.root.root.join("target").join("gate").join("modules");
    if modules.exists() {
        ctx(fs::remove_dir_all(&modules), "nettoyage de target/gate/modules")?;
    }
    ctx(fs::create_dir_all(&modules), "création de target/gate/modules")?;
    let reachable = g.normal_reachable();
    for (name, p) in &g.packages {
        if !with_dev && !reachable.contains(name) {
            continue;
        }
        let src = p.root.join("src");
        if !src.is_dir() {
            return Err(format!("la dépendance « {name} » n'a pas de dossier src/"));
        }
        copy_dir(&src, &modules.join(name))?;
    }
    Ok(vec![g.root.root.join("src"), modules])
}

/// Valeur de la variable d'environnement KASTEL_PATH.
pub fn kastel_path(dirs: &[PathBuf]) -> Result<OsString> {
    std::env::join_paths(dirs).map_err(|e| format!("chemin de modules invalide : {e}"))
}
