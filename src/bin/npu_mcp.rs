//! `locaryn-npu-mcp` : les actions des réglages du morph, servies en MCP (stdio).
//!
//! - `npu_activer` lance `activer-npu.ps1` en administrateur : Windows affiche
//!   sa demande d'autorisation (UAC), la personne accepte ou refuse.
//! - `npu_etat` dit si le NPU est déjà autorisé (mode test-signing actif).

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
            "description": "Dit si le NPU est autorisé sur ce PC (mode test-signing de Windows actif).",
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

fn etat() -> Result<Value, String> {
    if !cfg!(windows) {
        return Err("Le NPU Snapdragon n'existe que sous Windows.".into());
    }
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
