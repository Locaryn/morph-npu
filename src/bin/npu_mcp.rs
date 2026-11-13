//! `locaryn-npu-mcp` : les actions des réglages du morph, servies en MCP (stdio).
//!
//! - `npu_activer` lance `activer-npu.ps1` en administrateur : Windows affiche
//!   sa demande d'autorisation (UAC), la personne accepte ou refuse.
//! - `npu_etat` (« Vérifier ») dit si le NPU est utilisé : en mode Foundry
//!   Local, quel modèle est chargé et sur quel appareil ; en mode llama.cpp,
//!   si le mode test-signing est actif.
//! - `npu_modeles` liste les modèles NPU du catalogue Foundry Local.
//! - `npu_foundry_installer` installe Foundry Local (fenêtre visible, UAC
//!   seulement si Windows l'exige).

use locaryn_plugin_npu::backend::{self, Backend};
use locaryn_plugin_npu::{diagnostic, foundry, settings};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::Command;

const PROTOCOLE: &str = "2024-11-05";

fn main() {
    let entree = std::io::stdin();
    let mut sortie = std::io::stdout();
    for ligne in entree.lock().lines() {
        let ligne = match ligne {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[npu-mcp] lecture : {e}");
                break;
            }
        };
        if ligne.trim().is_empty() {
            continue;
        }
        let requete: Value = match serde_json::from_str(&ligne) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[npu-mcp] message illisible : {e}");
                continue;
            }
        };
        // Une notification (sans `id`) n'attend pas de réponse.
        let Some(id) = requete.get("id").cloned() else {
            continue;
        };
        let methode = requete["method"].as_str().unwrap_or("");
        let reponse = match repondre(methode, &requete["params"]) {
            Ok(resultat) => json!({ "jsonrpc": "2.0", "id": id, "result": resultat }),
            Err(message) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": message }
            }),
        };
        if writeln!(sortie, "{reponse}")
            .and_then(|()| sortie.flush())
            .is_err()
        {
            break;
        }
    }
}

fn repondre(methode: &str, params: &Value) -> Result<Value, String> {
    match methode {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOLE,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "morph-npu", "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": outils() })),
        "tools/call" => {
            let nom = params["name"].as_str().unwrap_or("");
            let (texte, erreur) = match nom {
                "npu_activer" => resultat(activer()),
                "npu_etat" => resultat(etat()),
                "npu_modeles" => resultat(modeles()),
                "npu_telecharger" => resultat(telecharger()),
                "npu_supprimer" => resultat(supprimer()),
                "npu_foundry_installer" => resultat(installer_foundry()),
                autre => (format!("outil inconnu : {autre}"), true),
            };
            Ok(json!({
                "content": [{ "type": "text", "text": texte }],
                "isError": erreur
            }))
        }
        autre => Err(format!("méthode non prise en charge : {autre}")),
    }
}

fn resultat(r: Result<Value, String>) -> (String, bool) {
    match r {
        Ok(v) => (v.to_string(), false),
        Err(e) => (json!({ "message": e }).to_string(), true),
    }
}

fn outils() -> Value {
    json!([
        {
            "name": "npu_activer",
            "description": "Autorise le NPU Snapdragon : lance activer-npu.ps1 en administrateur (demande Windows UAC). Il faut ensuite redémarrer le PC.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "npu_etat",
            "description": "Diagnostic du NPU : Foundry installé, NPU vu par Windows, nombre de modèles NPU au catalogue, modèle chargé et son appareil réel, chemin du journal de la passerelle.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "npu_modeles",
            "description": "Liste les modèles NPU du catalogue Foundry Local (alias à copier dans le réglage « Modèle Foundry »).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "npu_telecharger",
            "description": "Télécharge dans Foundry Local le modèle NPU nommé dans le réglage « Modèle Foundry », en arrière-plan.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "npu_supprimer",
            "description": "Supprime de Foundry Local le modèle nommé dans le réglage « Modèle Foundry ».",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "npu_foundry_installer",
            "description": "Installe Microsoft Foundry Local (winget) dans une fenêtre visible ; Windows demande l'autorisation seulement s'il l'exige.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

/// Le dossier `scripts/` livré avec le morph, à côté de `bin/`.
fn script(nom: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("emplacement du morph : {e}"))?;
    let racine = exe
        .parent()
        .and_then(|bin| bin.parent())
        .ok_or("dossier du morph introuvable")?;
    let chemin = racine.join("scripts").join(nom);
    if chemin.is_file() {
        Ok(chemin)
    } else {
        Err(format!(
            "{} manquant : réinstallez le morph.",
            chemin.display()
        ))
    }
}

fn sans_console(c: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = c;
}

fn activer() -> Result<Value, String> {
    if !cfg!(windows) {
        return Err("Le NPU Snapdragon ne s'active que sous Windows.".into());
    }
    let chemin = script("activer-npu.ps1")?;
    // Le PowerShell administrateur reste ouvert (-NoExit) : la personne lit
    // ce que le script a fait, ou pourquoi il a échoué (Secure Boot).
    let chemin_ps = chemin.display().to_string().replace('\'', "''");
    let commande = format!(
        "Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList \
         '-NoProfile','-ExecutionPolicy','Bypass','-NoExit','-File','\"{chemin_ps}\"'"
    );
    let mut c = Command::new("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-Command", &commande]);
    sans_console(&mut c);
    let sortie = c
        .output()
        .map_err(|e| format!("PowerShell introuvable : {e}"))?;
    if !sortie.status.success() {
        let detail = String::from_utf8_lossy(&sortie.stderr);
        eprintln!("[npu-mcp] élévation refusée ou impossible : {detail}");
        return Err(
            "Autorisation refusée : Windows n'a pas lancé le script. Recliquez et acceptez la demande."
                .into(),
        );
    }
    Ok(json!({
        "message": "Fenêtre administrateur ouverte : quand elle affiche « Terminé », redémarrez le PC. Le NPU sera alors utilisé."
    }))
}

/// Le moteur choisi dans les réglages du morph.
fn backend_choisi() -> Result<Backend, String> {
    let r = settings::Settings::load()?;
    backend::choose(&r.backend, "windows", "aarch64")
}

/// Le port de la passerelle, celui que `morph.json` déclare.
const PORT_PASSERELLE: u16 = 8190;

/// « Vérifier » : tout le chemin, de l'installation au modèle qui tourne.
fn etat() -> Result<Value, String> {
    if !cfg!(windows) {
        return Err("Le NPU Snapdragon n'existe que sous Windows.".into());
    }
    let test_signing = match backend_choisi()? {
        Backend::Foundry => None,
        Backend::Npu => etat_test_signing()?["npu_autorise"].as_bool(),
    };
    let exe = foundry::trouver();
    let foundry_vu = exe.as_deref().map(|e| {
        let version = foundry::executer(e, &["--version"])
            .map(|o| {
                foundry::sans_ansi(&String::from_utf8_lossy(&o.stdout))
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();
        format!(
            "{}, version {}",
            e.display(),
            if version.is_empty() { "?" } else { &version }
        )
    });
    let (catalogue, charges) = match &exe {
        Some(e) => catalogue_et_charges(e),
        None => (Err("Foundry absent".into()), Vec::new()),
    };
    let liste = passerelle_json("/v1/models");
    let execution = charges
        .first()
        .map(|m| diagnostic::Execution {
            variante: m.id.clone(),
            appareil: foundry::appareil_de(m),
            fournisseur: m.fournisseur.clone(),
            declare_par_le_moteur: true,
        })
        .or_else(|| {
            liste
                .as_ref()
                .and_then(diagnostic::lire_execution_passerelle)
        });
    let constat = diagnostic::Constat {
        foundry: foundry_vu,
        npu_windows: npu_windows(),
        catalogue,
        execution,
        passerelle: etat_passerelle(),
        test_signing,
    };
    let d = diagnostic::rediger(&constat);
    Ok(json!({ "npu_utilise": d.npu_utilise, "lignes": d.lignes, "message": d.message() }))
}

/// Le catalogue complet (pour compter les variantes NPU) et ce que Foundry
/// déclare avoir chargé. Démarre le service s'il dort.
fn catalogue_et_charges(
    exe: &std::path::Path,
) -> (
    Result<Vec<foundry::ModeleFoundry>, String>,
    Vec<foundry::ModeleFoundry>,
) {
    let base = match foundry::demarrer(exe) {
        Ok(b) => b,
        Err(e) => return (Err(e), Vec::new()),
    };
    let charges = foundry::charges_details(exe, &base).unwrap_or_else(|e| {
        eprintln!("[npu-mcp] modèles chargés illisibles : {e}");
        Vec::new()
    });
    (foundry::catalogue(exe, &base), charges)
}

/// Un GET sur la passerelle locale ; `None` si elle ne répond pas.
fn passerelle_json(chemin: &str) -> Option<Value> {
    let url = format!("http://127.0.0.1:{PORT_PASSERELLE}{chemin}");
    let reponse = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .get(&url)
        .call();
    match reponse {
        Ok(r) | Err(ureq::Error::Status(_, r)) => r.into_json().ok(),
        Err(e) => {
            eprintln!("[npu-mcp] passerelle muette ({url}) : {e}");
            None
        }
    }
}

fn etat_passerelle() -> diagnostic::Passerelle {
    let Some(v) = passerelle_json("/health") else {
        return diagnostic::Passerelle::Muette;
    };
    let journal = v["log"].as_str().unwrap_or("gateway.log").to_string();
    if v["status"].as_str() == Some("ok") {
        diagnostic::Passerelle::Prete { journal }
    } else {
        diagnostic::Passerelle::Demarre { journal }
    }
}

/// Les accélérateurs d'IA que Windows déclare (classe « ComputeAccelerator »
/// ou nom contenant NPU) ; `None` si PowerShell ne répond pas.
fn npu_windows() -> Option<Vec<String>> {
    let mut c = Command::new("powershell.exe");
    c.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Get-CimInstance Win32_PnPEntity | Where-Object { $_.PNPClass -eq 'ComputeAccelerator' \
         -or $_.Name -match '\\bNPU\\b' } | ForEach-Object { $_.Name }",
    ]);
    sans_console(&mut c);
    match c.output() {
        Ok(o) if o.status.success() => Some(diagnostic::lire_npu_windows(
            &String::from_utf8_lossy(&o.stdout),
        )),
        Ok(o) => {
            eprintln!(
                "[npu-mcp] accélérateurs illisibles : {}",
                String::from_utf8_lossy(&o.stderr)
            );
            None
        }
        Err(e) => {
            eprintln!("[npu-mcp] PowerShell introuvable : {e}");
            None
        }
    }
}

/// Foundry installé et démarré, ou un message qui dit quoi faire.
fn foundry_pret() -> Result<(PathBuf, String), String> {
    let exe = foundry::trouver()
        .ok_or("Foundry Local n'est pas installé : cliquez d'abord « Installer Foundry Local ».")?;
    if let Some(dossier) = foundry::modeles_locaryn() {
        foundry::ranger_dans(&exe, &dossier);
    }
    let url = foundry::demarrer(&exe)?;
    Ok((exe, url))
}

fn taille(m: &foundry::ModeleFoundry) -> String {
    m.taille_mo
        .map_or_else(String::new, |t| format!(" ({:.1} Go)", t as f64 / 1024.0))
}

fn modeles() -> Result<Value, String> {
    let (exe, url) = foundry_pret()?;
    let npu: Vec<foundry::ModeleFoundry> = foundry::catalogue(&exe, &url)?
        .into_iter()
        .filter(foundry::ModeleFoundry::est_npu)
        .collect();
    if npu.is_empty() {
        return Err("Aucun modèle NPU au catalogue : vérifiez que le pilote du NPU Snapdragon est à jour (Windows Update).".into());
    }
    let presents = foundry::installes(&exe);
    let (installes, autres): (Vec<_>, Vec<_>) = npu
        .iter()
        .partition(|m| foundry::est_installe(&presents, m));
    let liste = |l: &[&foundry::ModeleFoundry]| {
        l.iter()
            .map(|m| format!("{}{}", m.alias, taille(m)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let dossier = foundry::emplacement(&exe);
    let mut message = if installes.is_empty() {
        "Aucun modèle NPU téléchargé.".to_string()
    } else {
        format!("Téléchargés : {}.", liste(&installes))
    };
    if !autres.is_empty() {
        message.push_str(&format!(
            " À télécharger : {}. Copiez un nom dans « Modèle Foundry », enregistrez, puis « Télécharger le modèle ».",
            liste(&autres)
        ));
    }
    if let Some(d) = &dossier {
        message.push_str(&format!(" Dossier des modèles : {d}"));
    }
    Ok(json!({ "modeles": npu, "installes": presents, "dossier": dossier, "message": message }))
}

/// Le modèle nommé dans le réglage, trouvé au catalogue.
fn modele_du_reglage(exe: &std::path::Path, url: &str) -> Result<foundry::ModeleFoundry, String> {
    let demande = settings::Settings::load()?.foundry_model;
    if demande.trim().is_empty() || demande.trim().eq_ignore_ascii_case("auto") {
        return Err("Écrivez d'abord un nom dans « Modèle Foundry » (voir « Lister les modèles NPU »), puis enregistrez.".into());
    }
    let presents = foundry::installes(exe);
    foundry::choisir(&demande, &foundry::catalogue(exe, url)?, &presents)
}

fn telecharger() -> Result<Value, String> {
    let (exe, url) = foundry_pret()?;
    let m = modele_du_reglage(&exe, &url)?;
    if foundry::est_installe(&foundry::installes(&exe), &m) {
        return Ok(json!({ "message": format!("{} est déjà téléchargé.", m.alias) }));
    }
    // En arrière-plan : quelques minutes pour un gros modèle, le bouton rend
    // la main tout de suite.
    let mut c = Command::new(&exe);
    c.args(["model", "download", &m.id])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    sans_console(&mut c);
    c.spawn()
        .map_err(|e| format!("téléchargement non lancé : {e}"))?;
    Ok(json!({
        "message": format!(
            "Téléchargement de {}{} lancé. Il sera prêt à charger une fois fini.",
            m.alias,
            taille(&m)
        )
    }))
}

fn supprimer() -> Result<Value, String> {
    let (exe, url) = foundry_pret()?;
    let m = modele_du_reglage(&exe, &url)?;
    if !foundry::est_installe(&foundry::installes(&exe), &m) {
        return Ok(
            json!({ "message": format!("{} n'est pas téléchargé : rien à supprimer.", m.alias) }),
        );
    }
    foundry::supprimer(&exe, &m.id)?;
    Ok(json!({ "message": format!("{} supprimé.", m.alias) }))
}

fn installer_foundry() -> Result<Value, String> {
    if !cfg!(windows) {
        return Err("Foundry Local ne s'installe ici que sous Windows.".into());
    }
    let chemin = script("installer-foundry.ps1")?;
    let chemin_ps = chemin.display().to_string().replace('\'', "''");
    // Fenêtre visible, sans élévation : le script ne demande l'administrateur
    // que si winget échoue sans.
    let commande = format!(
        "Start-Process -FilePath powershell.exe -ArgumentList \
         '-NoProfile','-ExecutionPolicy','Bypass','-NoExit','-File','\"{chemin_ps}\"'"
    );
    let mut c = Command::new("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-Command", &commande]);
    sans_console(&mut c);
    let sortie = c
        .output()
        .map_err(|e| format!("PowerShell introuvable : {e}"))?;
    if !sortie.status.success() {
        eprintln!(
            "[npu-mcp] installation non lancée : {}",
            String::from_utf8_lossy(&sortie.stderr)
        );
        return Err("La fenêtre d'installation ne s'est pas ouverte : réessayez.".into());
    }
    Ok(json!({
        "message": "Installation de Foundry Local lancée dans une fenêtre : quand elle affiche « Terminé », lancez le moteur NPU."
    }))
}

/// Mode llama.cpp : le NPU n'est accessible qu'en mode test-signing.
fn etat_test_signing() -> Result<Value, String> {
    // Lisible sans droits administrateur : la ligne `testsigning` du
    // chargeur de démarrage actuel.
    let mut c = Command::new("powershell.exe");
    c.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "(Get-CimInstance Win32_OperatingSystem).Caption; \
         [bool](Get-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Control' -Name SystemStartOptions \
         -ErrorAction Stop).SystemStartOptions.ToUpper().Contains('TESTSIGNING')",
    ]);
    sans_console(&mut c);
    let sortie = c
        .output()
        .map_err(|e| format!("PowerShell introuvable : {e}"))?;
    let texte = String::from_utf8_lossy(&sortie.stdout);
    let actif = texte.lines().any(|l| l.trim().eq_ignore_ascii_case("true"));
    Ok(json!({
        "npu_autorise": actif,
        "message": if actif {
            "Le NPU est autorisé (mode test-signing actif)."
        } else {
            "Le NPU n'est pas encore autorisé : cliquez « Activer le NPU », acceptez la demande Windows, puis redémarrez."
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_outils_des_reglages_sont_declares() {
        let o = outils();
        let noms: Vec<&str> = o
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        for attendu in [
            "npu_activer",
            "npu_etat",
            "npu_modeles",
            "npu_telecharger",
            "npu_supprimer",
            "npu_foundry_installer",
        ] {
            assert!(noms.contains(&attendu), "{attendu}");
        }
    }
}
