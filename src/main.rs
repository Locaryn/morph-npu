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
        ("gateway", _) => passerelle(&argv[1..]),
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

/// Les identifiants servis : les modèles NPU téléchargés.
fn servis(exe: &Path, base: &str) -> Result<Vec<String>, String> {
    Ok(foundry::npu_installes(exe, base)?
        .into_iter()
        .map(|m| m.id)
        .collect())
}

/// Le fournisseur « NPU (Foundry Local) » du sélecteur de modèles : tous les
/// modèles NPU téléchargés, aucun chargé d'avance, aucun téléchargé ici.
fn passerelle(argv: &[String]) -> Result<u8, String> {
    let port = port(argv)?;
    let exe = foundry::trouver().map_or_else(foundry::installer, Ok)?;
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
        Box::new(move |demande| {
            let base = foundry::demarrer(&exe_change)?;
            let modele = foundry::npu_installes(&exe_change, &base)?
                .into_iter()
                .find(|m| m.id.eq_ignore_ascii_case(demande))
                .ok_or_else(|| format!("{demande} n'est pas téléchargé dans Foundry Local"))?;
            eprintln!("[npu] chargement de {} sur le NPU…", modele.id);
            let nom = foundry::charger(&exe_change, &base, &modele)?;
            Ok(foundry::Amont { base, modele: nom })
        }),
    );
    let modeles = servis(&exe, &base)?;
    eprintln!(
        "[npu] passerelle Foundry sur 127.0.0.1:{port} : {} modèle(s) NPU téléchargé(s)",
        modeles.len()
    );
    relais.definir_modeles(modeles);
    relais.definir(foundry::Amont {
        base: base.clone(),
        modele: String::new(),
    });
    // Un modèle téléchargé ou supprimé depuis les réglages apparaît ou
    // disparaît de la liste sans relancer la passerelle.
    let suivi = std::sync::Arc::clone(&relais);
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(30));
        match servis(&exe, &base) {
            Ok(m) => suivi.definir_modeles(m),
            Err(e) => eprintln!("[npu] liste des modèles non relue : {e}"),
        }
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
