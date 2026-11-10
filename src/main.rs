//! `locaryn-npu-launch` : ce que l'hôte exécute pour démarrer le moteur NPU.
//!
//!   serve --port N [--model CHEMIN]   prépare le moteur choisi, puis le sert sur N
//!   install                           installe le moteur choisi sans rien lancer
//!   status                            dit ce qui serait utilisé, en JSON
//!   gateway --port N                  sert tous les modèles NPU déjà
//!                                     téléchargés dans Foundry, chacun chargé
//!                                     quand une conversation le demande
//!
//! Le moteur vient du réglage `backend` : Foundry Local (défaut) ou llama.cpp.

use locaryn_plugin_npu::backend::Backend;
use locaryn_plugin_npu::{args, backend, foundry, proc, proxy, runtime, settings};
use std::path::{Path, PathBuf};
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

/// Ce qui ne change pas d'une commande à l'autre.
struct Contexte {
    reglages: settings::Settings,
    choix: Backend,
    data: PathBuf,
    tag: String,
    os: &'static str,
    arch: &'static str,
}

fn run() -> Result<u8, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let commande = argv.first().map(String::as_str).unwrap_or("");
    let reglages = settings::Settings::load()?;
    let (os, arch) = (std::env::consts::OS, backend::host_arch());
    // La passerelle ne dépend pas du réglage « Moteur ». Hors Snapdragon elle
    // ne sert qu'aux essais (`LOCARYN_NPU_APPAREIL_ESSAI`).
    if commande == "gateway" {
        if std::env::var_os("LOCARYN_NPU_APPAREIL_ESSAI").is_none() {
            backend::choose("foundry", os, arch)?;
        }
        return passerelle(&argv[1..]);
    }
    let ctx = Contexte {
        choix: backend::choose(&reglages.backend, os, arch)?,
        tag: reglages.tag().to_string(),
        data: settings::data_dir(),
        reglages,
        os,
        arch,
    };
    match (commande, ctx.choix) {
        ("status", Backend::Foundry) => statut_foundry(&ctx),
        ("status", Backend::Npu) => statut_llama(&ctx),
        ("install", Backend::Foundry) => {
            let exe = foundry::trouver().map_or_else(foundry::installer, Ok)?;
            eprintln!("[npu] Foundry Local prêt : {}", exe.display());
            Ok(0)
        }
        ("install", Backend::Npu) => {
            let serveur = runtime::ensure(&ctx.data, &ctx.tag, ctx.choix, os, arch)?;
            eprintln!("[npu] runtime prêt : {}", serveur.display());
            Ok(0)
        }
        ("serve", Backend::Foundry) => serve_foundry(&argv[1..], &ctx),
        ("serve", Backend::Npu) => serve_llama(&argv[1..], &ctx),
        (autre, _) => Err(format!(
            "commande « {autre} » inconnue (serve, install, status)"
        )),
    }
}

fn statut_foundry(ctx: &Contexte) -> Result<u8, String> {
    let exe = foundry::trouver();
    let url = exe.as_deref().and_then(foundry::url_service);
    let charges = match (&exe, &url) {
        (Some(e), Some(u)) => foundry::modeles_charges(e, u)?,
        _ => Vec::new(),
    };
    let etat = serde_json::json!({
        "backend": ctx.choix.name(),
        "installed": exe.is_some(),
        "foundry": exe,
        "service": url,
        "loaded_models": charges,
        "settings": ctx.reglages,
    });
    println!("{etat}");
    Ok(0)
}

fn statut_llama(ctx: &Contexte) -> Result<u8, String> {
    let dossier = runtime::install_dir(&ctx.data, &ctx.tag, ctx.choix, ctx.arch);
    let serveur = runtime::find_server(&dossier).filter(|_| dossier.join(".ready").exists());
    let etat = serde_json::json!({
        "runtime_tag": ctx.tag,
        "backend": ctx.choix.name(),
        "installed": serveur.is_some(),
        "server": serveur,
        "settings": ctx.reglages,
    });
    println!("{etat}");
    Ok(0)
}

fn valeur<'a>(argv: &'a [String], drapeau: &str) -> Option<&'a str> {
    argv.iter()
        .position(|a| a == drapeau)
        .and_then(|i| argv.get(i + 1))
        .map(String::as_str)
        .filter(|v| !v.starts_with("--"))
}

fn port(argv: &[String]) -> Result<u16, String> {
    valeur(argv, "--port")
        .ok_or("--port requis")?
        .parse()
        .map_err(|_| "--port doit être un nombre".to_string())
}

/// Foundry Local : le relais écoute tout de suite (l'hôte voit « en cours de
/// chargement » pendant le téléchargement), puis le modèle NPU est chargé.
fn serve_foundry(argv: &[String], ctx: &Contexte) -> Result<u8, String> {
    let port = port(argv)?;
    if let Some(m) = valeur(argv, "--model") {
        eprintln!(
            "[npu] mode Foundry Local : le modèle « {m} » choisi dans Locaryn est ignoré ; \
             le modèle vient du réglage « Modèle Foundry » ({}).",
            ctx.reglages.foundry_model
        );
    }
    let demande = ctx.reglages.foundry_model.clone();
    let relance_demande = demande.clone();
    let relais = proxy::Relais::new(Box::new(move || foundry::preparer(&relance_demande)));
    let ecoute = {
        let r = std::sync::Arc::clone(&relais);
        std::thread::spawn(move || proxy::ecouter(port, r))
    };
    // Laisser le relais prendre le port avant le long chargement : s'il est
    // déjà pris, l'échec doit se voir tout de suite.
    std::thread::sleep(std::time::Duration::from_millis(300));
    if ecoute.is_finished() {
        return ecoute
            .join()
            .map_err(|_| "relais arrêté".to_string())?
            .map(|()| 1);
    }
    let amont = foundry::preparer(&demande)?;
    eprintln!(
        "[npu] prêt : {} sur 127.0.0.1:{port} (service Foundry {})",
        amont.modele, amont.base
    );
    relais.definir(amont);
    ecoute
        .join()
        .map_err(|_| "relais arrêté".to_string())?
        .map(|()| 0)
}

/// Les identifiants servis : tous les modèles NPU du catalogue.
fn servis(exe: &Path, base: &str) -> Result<Vec<(String, String)>, String> {
    Ok(foundry::npu_du_catalogue(exe, base)?
        .into_iter()
        .map(|m| (m.id, m.alias))
        .collect())
}

/// Téléchargements en cours, pour n'en lancer qu'un par modèle.
static TELECHARGEMENTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Charge le modèle demandé. S'il n'est pas encore téléchargé, le
/// téléchargement part en arrière-plan et la conversation reçoit un message
/// clair : une requête ne peut pas attendre plusieurs minutes.
fn charger_ou_telecharger(exe: &Path, demande: &str) -> Result<foundry::Amont, String> {
    let base = foundry::demarrer(exe)?;
    let modele = foundry::npu_du_catalogue(exe, &base)?
        .into_iter()
        .find(|m| m.id.eq_ignore_ascii_case(demande))
        .ok_or_else(|| format!("{demande} n'est pas un modèle NPU de Foundry Local"))?;
    if foundry::est_installe(&foundry::installes(exe), &modele) {
        eprintln!("[npu] chargement de {} sur le NPU…", modele.id);
        let nom = foundry::charger(exe, &base, &modele)?;
        return Ok(foundry::Amont { base, modele: nom });
    }
    let taille = modele
        .taille_mo
        .map_or_else(String::new, |t| format!(" ({:.1} Go)", t as f64 / 1024.0));
    let deja = {
        let mut en_cours = TELECHARGEMENTS
            .lock()
            .map_err(|_| "suivi des téléchargements corrompu".to_string())?;
        let deja = en_cours.contains(&modele.id);
        if !deja {
            en_cours.push(modele.id.clone());
        }
        deja
    };
    if !deja {
        let (exe, m) = (exe.to_path_buf(), modele.clone());
        std::thread::spawn(move || {
            if let Err(e) = foundry::telecharger(&exe, &m) {
                eprintln!("[npu] {e}");
            }
            if let Ok(mut en_cours) = TELECHARGEMENTS.lock() {
                en_cours.retain(|i| i != &m.id);
            }
        });
    }
    Err(format!(
        "{} n'était pas encore sur ce PC : son téléchargement{taille} est en cours.          Renvoyez votre message dans quelques minutes.",
        modele.alias
    ))
}

/// Le fournisseur « NPU » du sélecteur de modèles : tous les modèles NPU du
/// catalogue Foundry. Rien n'est téléchargé ni chargé avant qu'on en
/// choisisse un.
fn passerelle(argv: &[String]) -> Result<u8, String> {
    let port = port(argv)?;
    let exe = foundry::trouver().map_or_else(foundry::installer, Ok)?;
    if let Some(dossier) = foundry::modeles_locaryn() {
        foundry::ranger_dans(&exe, &dossier);
    }
    let base = foundry::demarrer(&exe)?;
    let exe_relance = exe.clone();
    let exe_change = exe.clone();
    let relais = proxy::Relais::passerelle(
        Box::new(move || {
            let base = foundry::demarrer(&exe_relance)?;
            Ok(foundry::Amont {
                base,
                modele: String::new(),
            })
        }),
        Box::new(move |demande| charger_ou_telecharger(&exe_change, demande)),
    );
    let modeles = servis(&exe, &base)?;
    eprintln!(
        "[npu] passerelle Foundry sur 127.0.0.1:{port} : {} modèle(s) NPU au catalogue",
        modeles.len()
    );
    relais.definir_noms(modeles.iter().cloned().collect());
    relais.definir_modeles(modeles.into_iter().map(|(id, _)| id).collect());
    relais.definir(foundry::Amont {
        base,
        modele: String::new(),
    });
    proxy::ecouter(port, relais).map(|()| 0)
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

fn serve_llama(argv: &[String], ctx: &Contexte) -> Result<u8, String> {
    let port = port(argv)?;
    let modele = PathBuf::from(valeur(argv, "--model").ok_or(
        "le mode llama.cpp a besoin d'un fichier GGUF : choisissez-en un dans Réglages → Moteur.",
    )?);
    if !modele.is_file() {
        return Err(format!("modèle introuvable : {}", modele.display()));
    }
    eprintln!(
        "[npu] backend {} · modèle {} · contexte {}",
        ctx.choix.name(),
        modele.display(),
        ctx.reglages.context_size
    );
    let serveur = runtime::ensure(&ctx.data, &ctx.tag, ctx.choix, ctx.os, ctx.arch)?;
    lancer_llama(&serveur, &modele, port, &ctx.reglages)
}

fn lancer_llama(
    serveur: &Path,
    modele: &Path,
    port: u16,
    reglages: &settings::Settings,
) -> Result<u8, String> {
    let couches = (reglages.gpu_layers > 0).then_some(reglages.gpu_layers);
    let ligne = args::build(modele, port, reglages, None, couches);
    let mut enfant = proc::spawn(serveur, &ligne)?;
    let statut = enfant
        .wait()
        .map_err(|e| format!("attente du serveur : {e}"))?;
    if let Some(explication) = statut.code().and_then(cause_windows) {
        return Err(explication.to_string());
    }
    Ok(statut.code().map_or(1, |c| c.clamp(0, 255) as u8))
}
