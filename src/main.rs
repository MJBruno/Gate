//! Gate — gestionnaire de projet et de dépendances pour Kastel (l'équivalent de Cargo).

mod cmd;
mod manifest;
mod resolve;
mod toml;
mod util;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match cmd::dispatch(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("erreur : {e}");
            1
        }
    };
    std::process::exit(code);
}
