//! `locaryn-npu-launch` : ce que l'hôte exécute pour démarrer le moteur NPU.
//!
//!   serve --port N --model CHEMIN   installe le runtime au besoin, puis lance llama-server
//!   install                         installe le runtime sans rien lancer
//!   status                          dit ce qui serait utilisé, en JSON

use locaryn_plugin_npu::{args, backend, proc, runtime, settings};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("[npu] {e}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let commande = argv.first().map(String::as_str).unwrap_or("");
    let reglages = settings::Settings::load()?;
    let (os, arch) = (std::env::consts::OS, backend::host_arch());
    let choix = backend::choose(&reglages.backend, os, arch)?;
    let data = settings::data_dir();
    let tag = reglages.tag().to_string();

    match commande {
        "status" => {
            let dossier = runtime::install_dir(&data, &tag, choix, arch);
            let serveur =
                runtime::find_server(&dossier).filter(|_| dossier.join(".ready").exists());
            let etat = serde_json::json!({
                "runtime_tag": tag,
                "backend": choix.name(),
                "installed": serveur.is_some(),
                "server": serveur,
                "settings": reglages,
            });
            println!("{etat}");
            Ok(0)
        }
        "install" => {
            let serveur = runtime::ensure(&data, &tag, choix, os, arch)?;
            eprintln!("[npu] runtime prêt : {}", serveur.display());
            Ok(0)
        }
        "serve" => serve(&argv[1..], &reglages, choix, &data, &tag, os, arch),
        autre => Err(format!(
            "commande « {autre} » inconnue (serve, install, status)"
        )),
    }
}

fn valeur<'a>(argv: &'a [String], drapeau: &str) -> Option<&'a str> {
    argv.iter()
        .position(|a| a == drapeau)
        .and_then(|i| argv.get(i + 1))
        .map(String::as_str)
}

/// Ce que veut dire un code de sortie de Windows qui empêche le serveur de
/// démarrer, dit en clair plutôt qu'un nombre négatif.
fn cause_windows(code: i32) -> Option<&'static str> {
    match code as u32 {
        // STATUS_DLL_NOT_FOUND
        0xC000_0135 => Some(
            "llama-server n'a pas pu démarrer : une bibliothèque lui manque (le runtime \
             Visual C++ le plus souvent). Installez « Microsoft Visual C++ Redistributable » \
             ARM64, puis relancez le modèle.",
        ),
        // STATUS_INVALID_IMAGE_FORMAT
        0xC000_007B => Some(
            "llama-server n'a pas pu démarrer : une bibliothèque n'est pas de la bonne \
             architecture (x64 au lieu d'ARM64). Réinstallez le morph.",
        ),
        _ => None,
    }
}

fn serve(
    argv: &[String],
    reglages: &settings::Settings,
    choix: backend::Backend,
    data: &std::path::Path,
    tag: &str,
    os: &str,
    arch: &str,
) -> Result<u8, String> {
    let port: u16 = valeur(argv, "--port")
        .ok_or("--port requis")?
        .parse()
        .map_err(|_| "--port doit être un nombre")?;
    let modele = PathBuf::from(valeur(argv, "--model").ok_or("--model requis")?);
    if !modele.is_file() {
        return Err(format!("modèle introuvable : {}", modele.display()));
    }
    eprintln!(
        "[npu] backend {} · modèle {} · contexte {}",
        choix.name(),
        modele.display(),
        reglages.context_size
    );
    let serveur = runtime::ensure(data, tag, choix, os, arch)?;
    let couches = (reglages.gpu_layers > 0).then_some(reglages.gpu_layers);
    let ligne = args::build(&modele, port, reglages, None, couches);
    let mut enfant = proc::spawn(&serveur, &ligne)?;
    let statut = enfant
        .wait()
        .map_err(|e| format!("attente du serveur : {e}"))?;
    if let Some(explication) = statut.code().and_then(cause_windows) {
        return Err(explication.to_string());
    }
    Ok(statut.code().map_or(1, |c| c.clamp(0, 255) as u8))
}
