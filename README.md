# Gate

Gestionnaire de projet et de dépendances pour **Kastel**, dans l'esprit de Cargo.
Écrit en Rust, **sans aucune dépendance externe** (`cargo build --release` suffit, hors-ligne).

``` text
cargo build --release          # produit target/release/gate(.exe)
cargo test                     # tests unitaires de Gate lui-même
```

## Démarrage rapide

``` text
gate new mon_app               # nouveau binaire   (gate new ma_lib --lib pour une bibliothèque)
cd mon_app
gate run                       # exécute src/main.ks
gate add utils --path ../utils # dépendance locale
gate add json --git https://exemple.org/json.git --tag v1.0.0
gate test                      # exécute tests/**/*.ks
gate tree                      # arbre des dépendances
```

## Structure d'un projet

``` text
mon_app/
├── Gate.toml        # manifeste
├── Gate.lock        # révisions exactes (généré)
├── src/main.ks      # point d'entrée (binaire)   — ou src/lib.ks (bibliothèque)
├── tests/*.ks       # tests exécutés par `gate test`
├── examples/*.ks    # exécutables avec `gate run --example nom`
└── target/          # fichiers générés (ignoré par git)
```

## Gate.toml

```toml
[package]
name = "mon_app"          # identifiant Kastel : lettres, chiffres, "_" (sert de nom de module)
version = "0.1.0"         # X.Y.Z
description = ""
authors = ["Moi"]
# main = "src/autre.ks"   # optionnel

[dependencies]
utils = { path = "../utils" }
json  = { git = "https://exemple.org/json.git", tag = "v1.0.0" }   # ou branch = "...", rev = "..."

[dev-dependencies]        # visibles seulement pour tests/ et examples/
helpers = { path = "../helpers" }

[toolchain]
kastel = "kastel"         # exécutable à lancer (GATE_KASTEL a priorité)
```

Le registre central (`nom = "1.2.3"`) n'existe pas encore : Gate le signale clairement
et propose `path` ou `git`.

## Commandes

| Commande | Rôle |
| --- | --- |
| `gate new <nom> [--lib]` / `gate init [--lib]` | crée / initialise un paquet |
| `gate run [--example N] [-- args]` | exécute le point d'entrée |
| `gate check` (alias `build`) | résout les dépendances, vérifie chaque `.ks` de `src/` |
| `gate test [filtre] [--timeout S]` | exécute les fichiers de `tests/` |
| `gate add <nom> --path D \| --git URL [--tag\|--branch\|--rev X] [--dev]` | édite Gate.toml (commentaires conservés), annule si la dépendance est invalide |
| `gate remove <nom> [--dev]` | retire une dépendance |
| `gate fetch` / `gate update` | récupère les dépendances / ignore Gate.lock |
| `gate tree` | arbre des dépendances (`(*)` = déjà affiché) |
| `gate clean` | supprime `target/` |

Options communes : `--offline`, `--locked`.
Gate cherche `Gate.toml` dans le dossier courant puis dans les parents, comme Cargo.

## Comment Kastel trouve les dépendances (contrat avec l'interpréteur)

À chaque `run`, `check` et `test`, Gate copie le dossier `src/` de chaque dépendance dans
`target/gate/modules/<nom>/` puis lance `kastel` avec la variable d'environnement
**`KASTEL_PATH`** = `<projet>/src` + `target/gate/modules` (séparateur du système : `;` sous Windows, `:` ailleurs).

Convention de résolution que le chargeur de modules de Kastel doit suivre :

| Instruction | Fichier cherché dans chaque dossier de `KASTEL_PATH` |
| --- | --- |
| `import utils` | `utils/lib.ks` |
| `import utils.maths` | `utils/maths.ks` |
| `import utils.a.b` | `utils/a/b.ks` |

Les dépendances transitives sont mises à plat dans `target/gate/modules/` (le nom du paquet est
l'espace de noms). Deux paquets de même nom mais de sources différentes provoquent une erreur de conflit,
un cycle aussi.

Gate exporte aussi `GATE_PACKAGE_NAME`, `GATE_PACKAGE_VERSION` et `GATE_PROJECT_DIR`.

## Tests

Chaque fichier de `tests/` est lancé avec `kastel` : code de sortie 0 = réussi. En commentaire d'en-tête :

``` text
// expect: 42            ← la sortie standard doit valoir exactement ces lignes (une directive par ligne)
// expect-error: texte   ← le programme doit échouer, avec un message contenant ce texte
```

## Dépendances git

Clonées dans `~/.gate/git/` (ou `$GATE_HOME/git/`) puis figées par révision dans `Gate.lock`.
`gate update` récupère les dernières révisions des branches ; `--offline` interdit tout accès réseau.

## Variables d'environnement

`GATE_KASTEL`, `GATE_CHECK_FLAG` (option de vérification passée à `kastel`, défaut `--check`), `GATE_HOME`.

## Limites actuelles

- pas de registre de paquets, ni de `publish` ;
- TOML : pas de dates, flottants, chaînes multilignes ni `[[tableaux de tables]]` ;
- `gate build` est un alias de `check` (Kastel est interprété, il n'y a pas d'artefact à produire).
