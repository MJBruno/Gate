//! Les commandes de Gate : new, init, run, check, test, add, remove, fetch, update, tree, clean.

use crate::manifest::{self, Kind, Manifest, MANIFEST};
use crate::resolve::{self, Graph, Options};
use crate::toml::escape;
use crate::util::{collect_ks, ctx, is_ident, rel, status, Result};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Option passée à `kastel` pour vérifier un fichier sans l'exécuter (surchargeable par GATE_CHECK_FLAG).
const CHECK_FLAG: &str = "--check";
const TEST_TIMEOUT_SECS: u64 = 60;

pub fn dispatch(mut args: Vec<String>) -> Result<i32> {
    if args.is_empty() {
        print_help();
        return Ok(0);
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "new" => new_project(args, false),
        "init" => new_project(args, true),
        "run" | "r" => run(args),
        "check" | "c" | "build" | "b" => check(args),
        "test" | "t" => test(args),
        "add" => add(args),
        "remove" | "rm" => remove(args),
        "fetch" => fetch(args, false),
        "update" => fetch(args, true),
        "tree" => tree(args),
        "clean" => clean(args),
        "version" | "-V" | "--version" => {
            println!("gate {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        "help" | "-h" | "--help" => {
            print_help();
            Ok(0)
        }
        other => Err(format!(
            "commande inconnue « {other} » (essayez `gate help`)"
        )),
    }
}

fn print_help() {
    println!(
        "Gate {} — gestionnaire de projet et de dépendances pour Kastel

USAGE : gate <commande> [options]

COMMANDES
  new <nom> [--lib]     Crée un nouveau paquet dans ./<nom>
  init [--lib]          Initialise un paquet dans le dossier courant
  run [--example N] [-- args]
                        Exécute src/main.ks (ou examples/N.ks)
  check | build         Résout les dépendances et vérifie tous les fichiers de src/
  test [filtre]         Exécute chaque fichier de tests/ (--timeout SECONDES)
  add <nom> --path D | --git URL [--tag T|--branch B|--rev R] [--dev]
                        Ajoute une dépendance à Gate.toml
  remove <nom> [--dev]  Retire une dépendance
  fetch                 Récupère les dépendances et met à jour Gate.lock
  update                Comme fetch, mais ignore Gate.lock (dernières révisions git)
  tree                  Affiche l'arbre des dépendances
  clean                 Supprime le dossier target/
  version | help

OPTIONS COMMUNES : --offline (pas de réseau)  --locked (Gate.lock ne doit pas changer)

ENVIRONNEMENT
  GATE_KASTEL       exécutable kastel à utiliser (défaut : [toolchain] kastel, puis `kastel`)
  GATE_CHECK_FLAG   option de vérification passée à kastel (défaut : {})
  GATE_HOME         cache de Gate (défaut : ~/.gate)",
        env!("CARGO_PKG_VERSION"),
        CHECK_FLAG
    );
}

// ---------------------------------------------------------------------------
// Aides pour les arguments
// ---------------------------------------------------------------------------

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(i) = args.iter().position(|a| a == name) {
        args.remove(i);
        true
    } else {
        false
    }
}

fn take_opt(args: &mut Vec<String>, name: &str) -> Result<Option<String>> {
    if let Some(i) = args.iter().position(|a| a == name) {
        if i + 1 >= args.len() {
            return Err(format!("{name} attend une valeur"));
        }
        let v = args.remove(i + 1);
        args.remove(i);
        return Ok(Some(v));
    }
    let prefix = format!("{name}=");
    if let Some(i) = args.iter().position(|a| a.starts_with(&prefix)) {
        let v = args.remove(i)[prefix.len()..].to_string();
        return Ok(Some(v));
    }
    Ok(None)
}

fn split_dashdash(args: Vec<String>) -> (Vec<String>, Vec<String>) {
    let pos = args.iter().position(|a| a == "--");
    match pos {
        Some(i) => (args[..i].to_vec(), args[i + 1..].to_vec()),
        None => (args, Vec::new()),
    }
}

fn reject_extra(args: &[String]) -> Result<()> {
    match args.first() {
        Some(a) => Err(format!("argument inattendu « {a} »")),
        None => Ok(()),
    }
}

fn take_resolve_opts(args: &mut Vec<String>) -> Options {
    let offline = take_flag(args, "--offline");
    let locked = take_flag(args, "--locked");
    Options {
        offline,
        locked,
        update: false,
    }
}

// ---------------------------------------------------------------------------
// Contexte de projet et lancement de kastel
// ---------------------------------------------------------------------------

fn project() -> Result<(PathBuf, Manifest)> {
    let cwd = std::env::current_dir().map_err(|e| format!("dossier courant inaccessible : {e}"))?;
    let root = manifest::find_root(&cwd)?;
    let m = Manifest::load(&root)?;
    Ok((root, m))
}

fn prepare(root: &Path, with_dev: bool, opts: &Options) -> Result<(Graph, OsString)> {
    let graph = resolve::resolve(root, opts)?;
    let dirs = resolve::stage(&graph, with_dev)?;
    let path = resolve::kastel_path(&dirs)?;
    Ok((graph, path))
}

fn kastel_command(m: &Manifest, root: &Path, modules: &OsString) -> Command {
    let program = std::env::var("GATE_KASTEL")
        .ok()
        .or_else(|| m.kastel.clone())
        .unwrap_or_else(|| "kastel".to_string());
    let mut c = Command::new(program);
    c.current_dir(root)
        .env("KASTEL_PATH", modules)
        .env("GATE_PACKAGE_NAME", &m.name)
        .env("GATE_PACKAGE_VERSION", &m.version)
        .env("GATE_PROJECT_DIR", root);
    c
}

fn spawn_error(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        "kastel est introuvable : placez-le dans le PATH, ou définissez GATE_KASTEL / `kastel = \"...\"` dans [toolchain]".to_string()
    } else {
        format!("impossible de lancer kastel : {e}")
    }
}

// ---------------------------------------------------------------------------
// new / init
// ---------------------------------------------------------------------------

const MANIFEST_TEMPLATE: &str = r#"[package]
name = "__NAME__"
version = "0.1.0"
description = ""
authors = []

# Exemple : utils = { path = "../utils" }
#           json  = { git = "https://exemple.org/json.git", tag = "v1.0.0" }
[dependencies]

[dev-dependencies]
"#;

const MAIN_TEMPLATE: &str = "// Point d'entrée de __NAME__.\nprint(\"Bonjour depuis __NAME__ !\");\n";

const LIB_TEMPLATE: &str = "// Bibliothèque __NAME__ : les autres projets l'importent avec `import __NAME__.<module>`.\nexport func hello() -> str {\n    return \"Bonjour depuis __NAME__\";\n}\n";

const TEST_TEMPLATE: &str = "// Test de fumée généré par `gate new`.\n// `gate test` exécute chaque fichier de tests/ : code de sortie 0 = réussi.\n// `// expect: ...` compare la sortie standard ligne par ligne ;\n// `// expect-error: ...` attend un échec dont le message contient le texte.\n// expect: ok\nprint(\"ok\");\n";

fn write_new(path: &Path, content: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    ctx(fs::write(path, content), format!("écriture de {}", path.display()))
}

fn new_project(args: Vec<String>, init: bool) -> Result<i32> {
    let mut args = args;
    let lib = take_flag(&mut args, "--lib");
    let name_opt = take_opt(&mut args, "--name")?;
    let target: PathBuf = if init {
        reject_extra(&args)?;
        std::env::current_dir().map_err(|e| format!("dossier courant inaccessible : {e}"))?
    } else {
        if args.len() != 1 {
            return Err("usage : gate new <nom> [--lib] [--name <nom_du_paquet>]".into());
        }
        PathBuf::from(args.remove(0))
    };
    let raw_name = match name_opt {
        Some(n) => n,
        None => {
            let abs = if target.is_absolute() {
                target.clone()
            } else {
                std::env::current_dir()
                    .map_err(|e| format!("dossier courant inaccessible : {e}"))?
                    .join(&target)
            };
            abs.file_name()
                .and_then(|f| f.to_str())
                .ok_or("nom de projet illisible, utilisez --name")?
                .to_string()
        }
    };
    let name = raw_name.replace('-', "_");
    if !is_ident(&name) {
        return Err(format!(
            "nom de paquet invalide « {raw_name} » : lettres, chiffres et « _ » uniquement, sans commencer par un chiffre (--name permet d'en choisir un)"
        ));
    }
    if init {
        if target.join(MANIFEST).exists() {
            return Err(format!("{MANIFEST} existe déjà ici"));
        }
    } else if target.exists() {
        return Err(format!("« {} » existe déjà", target.display()));
    }

    ctx(fs::create_dir_all(target.join("src")), "création de src/")?;
    ctx(fs::create_dir_all(target.join("tests")), "création de tests/")?;
    write_new(
        &target.join(MANIFEST),
        &MANIFEST_TEMPLATE.replace("__NAME__", &name),
    )?;
    let (entry, body) = if lib {
        ("lib.ks", LIB_TEMPLATE)
    } else {
        ("main.ks", MAIN_TEMPLATE)
    };
    write_new(
        &target.join("src").join(entry),
        &body.replace("__NAME__", &name),
    )?;
    write_new(&target.join("tests").join("smoke.ks"), TEST_TEMPLATE)?;
    write_new(&target.join(".gitignore"), "target/\n")?;
    status(
        "Créé",
        format!(
            "paquet « {name} » ({})",
            if lib { "bibliothèque" } else { "binaire" }
        ),
    );
    Ok(0)
}

// ---------------------------------------------------------------------------
// run / check / test
// ---------------------------------------------------------------------------

fn run(args: Vec<String>) -> Result<i32> {
    let (mut head, program_args) = split_dashdash(args);
    let example = take_opt(&mut head, "--example")?;
    let opts = take_resolve_opts(&mut head);
    reject_extra(&head)?;
    let (root, m) = project()?;
    let file = match &example {
        Some(n) => {
            let p = root.join("examples").join(format!("{n}.ks"));
            if !p.is_file() {
                return Err(format!("exemple introuvable : {}", p.display()));
            }
            p
        }
        None => {
            let (p, kind) = m.entry()?;
            if kind == Kind::Lib {
                return Err("ce paquet est une bibliothèque (src/lib.ks) : rien à exécuter ; utilisez `gate test` ou `--example`".into());
            }
            p
        }
    };
    let (_graph, modules) = prepare(&root, example.is_some(), &opts)?;
    status("Exécution", rel(&root, &file));
    let mut c = kastel_command(&m, &root, &modules);
    c.arg(&file).args(&program_args);
    let st = c.status().map_err(spawn_error)?;
    Ok(st.code().unwrap_or(1))
}

fn check(args: Vec<String>) -> Result<i32> {
    let mut args = args;
    let opts = take_resolve_opts(&mut args);
    reject_extra(&args)?;
    let (root, m) = project()?;
    let (_g, modules) = prepare(&root, false, &opts)?;
    let mut files = Vec::new();
    collect_ks(&root.join("src"), &mut files)?;
    if files.is_empty() {
        return Err("aucun fichier .ks dans src/".into());
    }
    status("Vérification", format!("{} v{}", m.name, m.version));
    let flag = std::env::var("GATE_CHECK_FLAG").unwrap_or_else(|_| CHECK_FLAG.to_string());
    let mut failures = 0;
    for f in &files {
        let out = kastel_command(&m, &root, &modules)
            .arg(&flag)
            .arg(f)
            .output()
            .map_err(spawn_error)?;
        if !out.status.success() {
            failures += 1;
            eprintln!("--> {}", rel(&root, f));
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            eprint!("{}", String::from_utf8_lossy(&out.stdout));
        }
    }
    if failures > 0 {
        eprintln!("erreur : la vérification a échoué ({failures} fichier(s))");
        Ok(1)
    } else {
        status("Terminé", format!("{} fichier(s) sans erreur", files.len()));
        Ok(0)
    }
}

fn parse_expectations(src: &str) -> (Vec<String>, Option<String>) {
    let mut out = Vec::new();
    let mut err = None;
    for line in src.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("// expect-error:") {
            err = Some(rest.trim().to_string());
        } else if let Some(rest) = t.strip_prefix("// expect:") {
            out.push(rest.trim().to_string());
        }
    }
    (out, err)
}

/// `None` = test réussi, `Some(raison)` = échec.
fn run_test_file(
    m: &Manifest,
    root: &Path,
    modules: &OsString,
    file: &Path,
    timeout_s: u64,
) -> Result<Option<String>> {
    let source = fs::read_to_string(file).unwrap_or_default();
    let (expect_out, expect_err) = parse_expectations(&source);

    let mut c = kastel_command(m, root, modules);
    c.arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().map_err(spawn_error)?;
    let mut so = child.stdout.take().ok_or("sortie standard indisponible")?;
    let mut se = child.stderr.take().ok_or("sortie d'erreur indisponible")?;
    let h_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = so.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });
    let h_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = se.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let mut timed_out = false;
    let status = loop {
        let waited = child
            .try_wait()
            .map_err(|e| format!("attente du processus : {e}"))?;
        match waited {
            Some(s) => break s,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    timed_out = true;
                    break child.wait().map_err(|e| format!("attente du processus : {e}"))?;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        }
    };
    let out = h_out.join().unwrap_or_default();
    let err = h_err.join().unwrap_or_default();

    if timed_out {
        return Ok(Some(format!("délai dépassé ({timeout_s} s)")));
    }
    if let Some(needle) = expect_err {
        if status.success() {
            return Ok(Some(format!(
                "une erreur contenant « {needle} » était attendue, mais le programme a réussi"
            )));
        }
        let combined = format!("{out}\n{err}");
        if combined.contains(&needle) {
            return Ok(None);
        }
        return Ok(Some(format!(
            "message d'erreur attendu : « {needle} »\n--- sortie ---\n{}",
            combined.trim()
        )));
    }
    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "inconnu".to_string());
        return Ok(Some(format!(
            "code de sortie {code}\n{}\n{}",
            out.trim_end(),
            err.trim_end()
        )));
    }
    if !expect_out.is_empty() {
        let got: Vec<&str> = out.lines().map(|l| l.trim_end()).collect();
        let want: Vec<&str> = expect_out.iter().map(|s| s.as_str()).collect();
        if got != want {
            return Ok(Some(format!(
                "sortie différente\n--- attendu ---\n{}\n--- obtenu ---\n{}",
                want.join("\n"),
                got.join("\n")
            )));
        }
    }
    Ok(None)
}

fn test(args: Vec<String>) -> Result<i32> {
    let mut args = args;
    let timeout = match take_opt(&mut args, "--timeout")? {
        Some(s) => s
            .parse::<u64>()
            .map_err(|_| format!("--timeout invalide : « {s} »"))?,
        None => TEST_TIMEOUT_SECS,
    };
    let opts = take_resolve_opts(&mut args);
    if args.len() > 1 {
        return Err("usage : gate test [filtre] [--timeout SECONDES]".into());
    }
    let filter = args.first().cloned();
    let (root, m) = project()?;
    let (_g, modules) = prepare(&root, true, &opts)?;

    let mut files = Vec::new();
    collect_ks(&root.join("tests"), &mut files)?;
    if let Some(f) = &filter {
        files.retain(|p| rel(&root, p).contains(f.as_str()));
    }
    if files.is_empty() {
        eprintln!("aucun test trouvé");
        return Ok(0);
    }
    status(
        "Test",
        format!("{} v{} ({} fichier(s))", m.name, m.version, files.len()),
    );
    let start = Instant::now();
    let (mut passed, mut failed) = (0usize, 0usize);
    let mut failures: Vec<(String, String)> = Vec::new();
    for f in &files {
        let name = rel(&root, f);
        let t0 = Instant::now();
        let outcome = run_test_file(&m, &root, &modules, f, timeout)?;
        let ms = t0.elapsed().as_millis();
        match outcome {
            None => {
                println!("test {name} ... ok ({ms} ms)");
                passed += 1;
            }
            Some(why) => {
                println!("test {name} ... ÉCHEC ({ms} ms)");
                failed += 1;
                failures.push((name, why));
            }
        }
    }
    if !failures.is_empty() {
        println!("\néchecs :");
        for (n, w) in &failures {
            println!("\n---- {n} ----\n{w}");
        }
    }
    println!(
        "\nrésultat : {} ({passed} réussi(s), {failed} échoué(s)) en {:.2}s",
        if failed == 0 { "ok" } else { "ÉCHEC" },
        start.elapsed().as_secs_f64()
    );
    Ok(if failed == 0 { 0 } else { 1 })
}

// ---------------------------------------------------------------------------
// add / remove / fetch / update / tree / clean
// ---------------------------------------------------------------------------

fn add(args: Vec<String>) -> Result<i32> {
    const USAGE: &str = "usage : gate add <nom> (--path <dossier> | --git <url> [--tag T | --branch B | --rev R]) [--dev]";
    let mut args = args;
    let dev = take_flag(&mut args, "--dev");
    let path = take_opt(&mut args, "--path")?;
    let git = take_opt(&mut args, "--git")?;
    let tag = take_opt(&mut args, "--tag")?;
    let branch = take_opt(&mut args, "--branch")?;
    let rev = take_opt(&mut args, "--rev")?;
    if args.len() != 1 {
        return Err(USAGE.into());
    }
    let name = args.remove(0);
    if !is_ident(&name) {
        return Err(format!(
            "nom de dépendance invalide « {name} » (lettres, chiffres et « _ »)"
        ));
    }
    let refs: Vec<String> = [
        tag.map(|t| format!("tag = \"{}\"", escape(&t))),
        branch.map(|b| format!("branch = \"{}\"", escape(&b))),
        rev.map(|r| format!("rev = \"{}\"", escape(&r))),
    ]
    .into_iter()
    .flatten()
    .collect();

    let spec = match (&path, &git) {
        (Some(_), Some(_)) => return Err("--path et --git sont exclusifs".into()),
        (Some(p), None) => {
            if !refs.is_empty() {
                return Err("--tag / --branch / --rev ne s'appliquent qu'à --git".into());
            }
            format!("{{ path = \"{}\" }}", escape(&p.replace('\\', "/")))
        }
        (None, Some(url)) => {
            if refs.len() > 1 {
                return Err("un seul parmi --tag / --branch / --rev est permis".into());
            }
            let mut s = format!("{{ git = \"{}\"", escape(url));
            if let Some(r) = refs.first() {
                s.push_str(", ");
                s.push_str(r);
            }
            s.push_str(" }");
            s
        }
        (None, None) => return Err(USAGE.into()),
    };

    let (root, _m) = project()?;
    let file = root.join(MANIFEST);
    let old = ctx(fs::read_to_string(&file), "lecture de Gate.toml")?;
    let section = if dev { "dev-dependencies" } else { "dependencies" };
    let new = manifest::add_dependency_text(&old, section, &name, &spec);
    ctx(fs::write(&file, &new), "écriture de Gate.toml")?;
    status("Ajout", format!("{name} à [{section}]"));
    match resolve::resolve(&root, &Options::default()) {
        Ok(_) => Ok(0),
        Err(e) => {
            let _ = fs::write(&file, &old);
            Err(format!("dépendance refusée, Gate.toml restauré : {e}"))
        }
    }
}

fn remove(args: Vec<String>) -> Result<i32> {
    let mut args = args;
    let dev = take_flag(&mut args, "--dev");
    if args.len() != 1 {
        return Err("usage : gate remove <nom> [--dev]".into());
    }
    let name = args.remove(0);
    let (root, _m) = project()?;
    let file = root.join(MANIFEST);
    let old = ctx(fs::read_to_string(&file), "lecture de Gate.toml")?;
    let section = if dev { "dev-dependencies" } else { "dependencies" };
    match manifest::remove_dependency_text(&old, section, &name) {
        Some(new) => {
            ctx(fs::write(&file, new), "écriture de Gate.toml")?;
            status("Retrait", format!("{name} de [{section}]"));
            if let Err(e) = resolve::resolve(&root, &Options::default()) {
                eprintln!("avertissement : Gate.lock n'a pas pu être mis à jour : {e}");
            }
            Ok(0)
        }
        None => Err(format!("« {name} » n'est pas dans [{section}]")),
    }
}

fn fetch(args: Vec<String>, update: bool) -> Result<i32> {
    let mut args = args;
    let mut opts = take_resolve_opts(&mut args);
    reject_extra(&args)?;
    opts.update = update;
    let (root, _m) = project()?;
    let g = resolve::resolve(&root, &opts)?;
    status("Résolu", format!("{} dépendance(s)", g.packages.len()));
    Ok(0)
}

fn print_tree(g: &Graph, names: &[String], prefix: &str, seen: &mut BTreeSet<String>) {
    for (i, n) in names.iter().enumerate() {
        let last = i + 1 == names.len();
        let branch = if last { "└── " } else { "├── " };
        let Some(p) = g.packages.get(n) else { continue };
        let again = !seen.insert(n.clone());
        let mark = if again && !p.deps.is_empty() { " (*)" } else { "" };
        println!("{prefix}{branch}{} v{}{mark}", p.name, p.version);
        if !again {
            let next = format!("{prefix}{}", if last { "    " } else { "│   " });
            print_tree(g, &p.deps, &next, seen);
        }
    }
}

fn tree(args: Vec<String>) -> Result<i32> {
    let mut args = args;
    let opts = take_resolve_opts(&mut args);
    reject_extra(&args)?;
    let (root, m) = project()?;
    let g = resolve::resolve(&root, &opts)?;
    println!("{} v{}", m.name, m.version);
    let mut seen = BTreeSet::new();
    let direct = g.direct.clone();
    print_tree(&g, &direct, "", &mut seen);
    Ok(0)
}

fn clean(args: Vec<String>) -> Result<i32> {
    reject_extra(&args)?;
    let (root, _m) = project()?;
    let target = root.join("target");
    if target.exists() {
        ctx(fs::remove_dir_all(&target), "suppression de target/")?;
        status("Nettoyage", "target/ supprimé");
    } else {
        status("Nettoyage", "rien à supprimer");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn take_opt_supports_both_forms() {
        let mut a = v(&["x", "--path", "../u", "y"]);
        assert_eq!(take_opt(&mut a, "--path").unwrap(), Some("../u".into()));
        assert_eq!(a, v(&["x", "y"]));
        let mut b = v(&["--tag=v1"]);
        assert_eq!(take_opt(&mut b, "--tag").unwrap(), Some("v1".into()));
        assert!(b.is_empty());
        let mut c = v(&["--path"]);
        assert!(take_opt(&mut c, "--path").is_err());
    }

    #[test]
    fn dashdash_splits_program_arguments() {
        let (head, rest) = split_dashdash(v(&["--offline", "--", "a", "-b"]));
        assert_eq!(head, v(&["--offline"]));
        assert_eq!(rest, v(&["a", "-b"]));
        let (head, rest) = split_dashdash(v(&["x"]));
        assert_eq!(head, v(&["x"]));
        assert!(rest.is_empty());
    }

    #[test]
    fn expectations_are_collected() {
        let (out, err) = parse_expectations("// expect: 1\nprint(1);\n  // expect: deux\n");
        assert_eq!(out, v(&["1", "deux"]));
        assert_eq!(err, None);
        let (out, err) = parse_expectations("// expect-error: type mismatch\n");
        assert!(out.is_empty());
        assert_eq!(err.as_deref(), Some("type mismatch"));
    }

    #[test]
    fn new_and_init_templates_are_valid_manifests() {
        let text = MANIFEST_TEMPLATE.replace("__NAME__", "demo");
        let m = Manifest::parse(&text, Path::new(".")).unwrap();
        assert_eq!(m.name, "demo");
        assert!(m.dependencies.is_empty());
    }
}
