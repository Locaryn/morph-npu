//! Microsoft Foundry Local : le NPU Snapdragon par le fournisseur QNN signé
//! par Qualcomm, sans mode test ni Secure Boot coupé.
//!
//! Tout passe par la CLI `foundry` : installation (winget), démarrage du
//! service, catalogue, téléchargement et chargement du modèle. Le port du
//! service change à chaque démarrage : il est lu dans `foundry server status`,
//! jamais supposé.

use crate::proc::masquer_console;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// Identifiant winget du paquet.
pub const PAQUET_WINGET: &str = "Microsoft.FoundryLocal";

/// Modèles essayés dans cet ordre quand le réglage vaut `auto` : petits,
/// rapides sur NPU, et avec une variante QNN au catalogue.
pub const PREFERES: [&str; 4] = ["qwen2.5-1.5b", "phi-4-mini", "phi-3.5-mini", "qwen2.5-7b"];

/// Une entrée du catalogue Foundry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModeleFoundry {
    /// Nom court (`phi-4-mini`), commun aux variantes CPU, GPU et NPU.
    pub alias: String,
    /// Identifiant de la variante (`Phi-4-mini-instruct-qnn-npu:1`). Vaut
    /// l'alias quand la CLI ne l'affiche pas ; Foundry choisit alors lui-même
    /// la variante NPU sur un Snapdragon.
    pub id: String,
    /// `NPU`, `GPU` ou `CPU`.
    pub appareil: String,
    /// Taille du téléchargement, en Mo, quand elle est connue.
    pub taille_mo: Option<u64>,
}

impl ModeleFoundry {
    pub fn est_npu(&self) -> bool {
        self.appareil.eq_ignore_ascii_case("npu")
    }
}

/// Retire les séquences de couleur ANSI qu'une CLI peut écrire.
pub fn sans_ansi(texte: &str) -> String {
    let mut out = String::with_capacity(texte.len());
    let mut car = texte.chars().peekable();
    while let Some(c) = car.next() {
        if c == '\u{1b}' {
            if car.peek() == Some(&'[') {
                car.next();
                for suite in car.by_ref() {
                    if suite.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// L'URL de base du service (`http://127.0.0.1:5273`) dans la sortie de
/// `foundry server status` (ou `service status` des anciennes versions).
pub fn lire_url_service(texte: &str) -> Option<String> {
    let propre = sans_ansi(texte);
    let debut = propre.find("http://")?;
    let reste = &propre[debut + "http://".len()..];
    let hote: String = reste
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, '/' | '"' | ',' | ')'))
        .collect();
    let (nom, port) = hote.rsplit_once(':')?;
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    let nom = if nom.eq_ignore_ascii_case("localhost") {
        "127.0.0.1"
    } else {
        nom
    };
    Some(format!("http://{nom}:{port}"))
}

/// Taille écrite « 2.13 GB » ou « 900 MB », en Mo.
fn taille_en_mo(nombre: &str, unite: &str) -> Option<u64> {
    let n: f64 = nombre.replace(',', ".").parse().ok()?;
    let facteur = match unite.to_ascii_uppercase().as_str() {
        "GB" | "GO" => 1024.0,
        "MB" | "MO" => 1.0,
        _ => return None,
    };
    Some((n * facteur).round() as u64)
}

fn est_appareil(mot: &str) -> bool {
    ["NPU", "GPU", "CPU"]
        .iter()
        .any(|a| mot.eq_ignore_ascii_case(a))
}

/// Un identifiant de variante porte le nom de son appareil (`…-qnn-npu:1`,
/// `…-generic-cpu`) ; un nom d'affichage ou une licence, non.
fn ressemble_a_un_id(mot: &str) -> bool {
    let m = mot.to_ascii_lowercase();
    m.contains('-')
        && [
            "npu", "qnn", "cpu", "gpu", "cuda", "webgpu", "openvino", "vitis",
        ]
        .iter()
        .any(|k| m.contains(k))
}

/// Une ligne du tableau de `foundry model list`, en tenant compte de l'alias
/// de la ligne précédente (les variantes d'un même alias ne le répètent pas).
fn lire_ligne(ligne: &str, alias_courant: &str) -> Option<ModeleFoundry> {
    let mots: Vec<&str> = ligne.split_whitespace().collect();
    let pos = mots.iter().position(|m| est_appareil(m))?;
    let continue_alias = ligne.starts_with(char::is_whitespace) || pos == 0;
    let alias = if continue_alias {
        alias_courant.to_string()
    } else {
        // Un alias n'a jamais d'espace ; ce qui suit peut être un nom
        // d'affichage (`--verbose`).
        mots[0].to_string()
    };
    if alias.is_empty() {
        return None;
    }
    let apres = &mots[pos + 1..];
    if apres
        .first()
        .is_some_and(|t| !t.to_ascii_lowercase().contains("chat"))
    {
        return None;
    }
    let taille_mo = apres.windows(2).find_map(|w| taille_en_mo(w[0], w[1]));
    let id = mots
        .last()
        .filter(|m| ressemble_a_un_id(m))
        .map_or_else(|| alias.clone(), |m| (*m).to_string());
    Some(ModeleFoundry {
        alias,
        id,
        appareil: mots[pos].to_ascii_uppercase(),
        taille_mo,
    })
}

/// Le tableau de `foundry model list` (texte), modèles de conversation seuls.
pub fn lire_liste_cli(texte: &str) -> Vec<ModeleFoundry> {
    let mut alias = String::new();
    let mut modeles = Vec::new();
    for ligne in sans_ansi(texte).lines() {
        let t = ligne.trim();
        if t.is_empty() || t.starts_with("Alias") || t.starts_with('-') || t.starts_with('─') {
            continue;
        }
        if let Some(m) = lire_ligne(ligne, &alias) {
            alias.clone_from(&m.alias);
            modeles.push(m);
        }
    }
    modeles
}

/// La première valeur JSON d'une sortie de CLI, après d'éventuelles lignes
/// de progression (téléchargement des fournisseurs d'exécution).
pub fn premier_json(texte: &str) -> Option<Value> {
    let propre = sans_ansi(texte);
    propre.match_indices(['{', '[']).find_map(|(i, _)| {
        serde_json::Deserializer::from_str(&propre[i..])
            .into_iter::<Value>()
            .next()
            .and_then(Result::ok)
    })
}

/// Le premier champ texte non vide parmi `cles`.
fn texte_de(v: &Value, cles: &[&str]) -> String {
    cles.iter()
        .filter_map(|c| v.get(*c).and_then(Value::as_str))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// Les entrées d'un catalogue JSON : `{ "variants": [...] }` (CLI 0.11+,
/// `--variants`), `{ "models": [...] }` (CLI, `GET /foundry/list`) ou un
/// tableau.
fn entrees(v: &Value) -> &[Value] {
    ["variants", "models"]
        .iter()
        .find_map(|c| v.get(*c).and_then(Value::as_array))
        .or_else(|| v.as_array())
        .map_or(&[], Vec::as_slice)
}

fn lire_entree(m: &Value) -> Option<ModeleFoundry> {
    let id = texte_de(m, &["variantId", "id", "name"]);
    if id.is_empty() {
        return None;
    }
    let alias = Some(texte_de(m, &["alias"]))
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| id.clone());
    let appareil = Some(texte_de(m, &["device"]))
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| texte_de(&m["runtime"], &["deviceType"]));
    Some(ModeleFoundry {
        alias,
        id,
        appareil: appareil.to_ascii_uppercase(),
        taille_mo: m.get("fileSizeMb").and_then(Value::as_u64),
    })
}

/// Un catalogue JSON (CLI `-o json` ou API), modèles de conversation seuls.
pub fn lire_catalogue_json(v: &Value) -> Vec<ModeleFoundry> {
    entrees(v)
        .iter()
        .filter(|m| {
            texte_de(m, &["type", "task"])
                .to_ascii_lowercase()
                .contains("chat")
        })
        .filter_map(lire_entree)
        .collect()
}

/// Les identifiants d'une liste JSON de modèles chargés.
pub fn lire_ids_json(v: &Value) -> Vec<String> {
    entrees(v)
        .iter()
        .filter_map(|m| match m {
            Value::String(s) => Some(s.clone()),
            autre => lire_entree(autre).map(|e| e.id),
        })
        .collect()
}

fn sans_version(id: &str) -> &str {
    id.split_once(':').map_or(id, |(nom, _)| nom)
}

/// `m` est-il dans le cache de Foundry (déjà téléchargé) ?
pub fn est_installe(installes: &[String], m: &ModeleFoundry) -> bool {
    let id = sans_version(&m.id).to_ascii_lowercase();
    installes.iter().any(|i| {
        let i = sans_version(i).to_ascii_lowercase();
        i == id || i.contains(&id)
    })
}

/// Les identifiants d'une sortie texte de `foundry cache list`.
pub fn lire_cache_texte(texte: &str) -> Vec<String> {
    sans_ansi(texte)
        .split_whitespace()
        .filter(|m| ressemble_a_un_id(m))
        .map(str::to_string)
        .collect()
}

fn noms(modeles: &[&ModeleFoundry]) -> String {
    let mut a: Vec<&str> = modeles.iter().map(|m| m.alias.as_str()).collect();
    a.dedup();
    a.join(", ")
}

/// Le modèle à charger. `demande` vient du réglage : `auto`, un alias du
/// catalogue, ou l'identifiant exact d'une variante.
pub fn choisir(
    demande: &str,
    catalogue: &[ModeleFoundry],
    installes: &[String],
) -> Result<ModeleFoundry, String> {
    let npu: Vec<&ModeleFoundry> = catalogue.iter().filter(|m| m.est_npu()).collect();
    if npu.is_empty() {
        return Err(
            "Foundry Local ne propose aucun modèle NPU sur ce PC : le pilote du NPU Snapdragon \
             (30.0.140.0 ou plus récent) est peut-être absent. Mettez Windows à jour."
                .into(),
        );
    }
    let d = demande.trim();
    if d.is_empty() || d.eq_ignore_ascii_case("auto") {
        // « auto » ne télécharge jamais : il prend un modèle NPU déjà présent.
        let presents: Vec<&ModeleFoundry> = npu
            .iter()
            .copied()
            .filter(|m| est_installe(installes, m))
            .collect();
        if presents.is_empty() {
            return Err(format!(
                "Aucun modèle NPU téléchargé. Choisissez-en un dans les réglages du morph                  (« Modèle Foundry », puis « Télécharger le modèle ») : {}.",
                noms(&npu)
            ));
        }
        let prefere = PREFERES
            .iter()
            .find_map(|p| presents.iter().find(|m| m.alias.eq_ignore_ascii_case(p)));
        let plus_petit = presents
            .iter()
            .min_by_key(|m| m.taille_mo.unwrap_or(u64::MAX));
        return Ok((*prefere.or(plus_petit).unwrap_or(&presents[0])).clone());
    }
    if let Some(m) = catalogue
        .iter()
        .find(|m| sans_version(&m.id).eq_ignore_ascii_case(sans_version(d)))
    {
        return Ok(m.clone());
    }
    if let Some(m) = npu.iter().find(|m| m.alias.eq_ignore_ascii_case(d)) {
        return Ok((*m).clone());
    }
    if catalogue.iter().any(|m| m.alias.eq_ignore_ascii_case(d)) {
        return Err(format!(
            "« {d} » n'a pas de variante NPU. Modèles NPU disponibles : {}.",
            noms(&npu)
        ));
    }
    Err(format!(
        "« {d} » n'est pas au catalogue Foundry (les fichiers GGUF ne sont pas acceptés dans \
         ce mode). Modèles NPU disponibles : {}.",
        noms(&npu)
    ))
}

/// La CLI `foundry`, sur le chemin ou dans les alias d'application de
/// Windows (un processus lancé avant l'installation n'a pas le PATH à jour).
pub fn trouver() -> Option<PathBuf> {
    let nom = if cfg!(windows) {
        "foundry.exe"
    } else {
        "foundry"
    };
    let mut candidats: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join(nom)).collect())
        .unwrap_or_default();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidats.push(
            PathBuf::from(local)
                .join("Microsoft")
                .join("WindowsApps")
                .join(nom),
        );
    }
    // Un alias d'application est un point d'analyse que `metadata` ne suit
    // pas : `symlink_metadata` le voit.
    candidats
        .into_iter()
        .find(|c| std::fs::symlink_metadata(c).is_ok())
}

/// La commande winget d'installation, sans fenêtre ni question.
pub fn arguments_winget() -> Vec<&'static str> {
    vec![
        "install",
        "--id",
        PAQUET_WINGET,
        "--exact",
        "--silent",
        "--accept-package-agreements",
        "--accept-source-agreements",
        "--disable-interactivity",
    ]
}

/// Installe Foundry Local par winget, sans droits administrateur (paquet
/// MSIX par utilisateur). Quand Windows exige l'administrateur, l'erreur dit
/// quel bouton utiliser.
pub fn installer() -> Result<PathBuf, String> {
    eprintln!("[npu] Foundry Local absent : installation par winget…");
    let mut c = Command::new("winget");
    c.args(arguments_winget()).stdin(Stdio::null());
    masquer_console(&mut c);
    let statut = c.status().map_err(|e| {
        format!(
            "winget introuvable ({e}). Installez Foundry Local depuis \
             https://aka.ms/foundry-local-installer, puis relancez le modèle."
        )
    })?;
    if !statut.success() {
        return Err(
            "L'installation de Foundry Local a échoué sans droits administrateur. Dans les \
             réglages du morph NPU, cliquez « Installer Foundry Local » et acceptez la demande \
             Windows, puis relancez le modèle."
                .into(),
        );
    }
    trouver().ok_or_else(|| {
        "Foundry Local est installé mais la commande « foundry » reste introuvable : \
         redémarrez Locaryn."
            .to_string()
    })
}

/// Lance la CLI et rend sa sortie, sans fenêtre.
pub fn executer(exe: &Path, args: &[&str]) -> Result<Output, String> {
    let mut c = Command::new(exe);
    c.args(args).stdin(Stdio::null());
    masquer_console(&mut c);
    c.output()
        .map_err(|e| format!("impossible de lancer {} : {e}", exe.display()))
}

/// Lance la CLI en laissant sa progression aller au journal du moteur.
fn executer_visible(exe: &Path, args: &[&str]) -> Result<bool, String> {
    let mut c = Command::new(exe);
    c.args(args).stdin(Stdio::null());
    masquer_console(&mut c);
    c.status()
        .map(|s| s.success())
        .map_err(|e| format!("impossible de lancer {} : {e}", exe.display()))
}

fn texte_sortie(o: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// L'URL du service s'il tourne. `server` est le nom actuel de la commande,
/// `service` celui des versions 0.x.
pub fn url_service(exe: &Path) -> Option<String> {
    ["server", "service"].iter().find_map(|groupe| {
        executer(exe, &[groupe, "status"])
            .ok()
            .and_then(|o| lire_url_service(&texte_sortie(&o)))
    })
}

/// Démarre le service et attend son URL (jusqu'à deux minutes).
pub fn demarrer(exe: &Path) -> Result<String, String> {
    if let Some(url) = url_service(exe) {
        return Ok(url);
    }
    let lance = executer(exe, &["server", "start"]).is_ok_and(|o| o.status.success());
    if !lance {
        executer(exe, &["service", "start"])?;
    }
    for _ in 0..60 {
        if let Some(url) = url_service(exe) {
            return Ok(url);
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Err("le service Foundry Local ne démarre pas (voir « foundry server logs »).".into())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(60))
        .build()
}

fn obtenir_json(url: &str) -> Result<Value, String> {
    agent()
        .get(url)
        .call()
        .map_err(|e| format!("{url} : {e}"))?
        .into_json()
        .map_err(|e| format!("{url} : réponse illisible ({e})"))
}

/// Le catalogue : la CLI d'abord (elle télécharge aussi, au premier appel, le
/// fournisseur QNN du NPU), l'API du service en secours.
pub fn catalogue(exe: &Path, base: &str) -> Result<Vec<ModeleFoundry>, String> {
    // JSON d'abord (CLI 0.11+), puis le tableau texte des versions 0.x.
    let json: [&[&str]; 2] = [
        &[
            "model",
            "list",
            "--variants",
            "--type",
            "chat",
            "-o",
            "json",
        ],
        &["model", "list", "--variants", "-o", "json"],
    ];
    for args in json {
        let liste = executer(exe, args)
            .ok()
            .and_then(|o| premier_json(&texte_sortie(&o)))
            .map(|v| lire_catalogue_json(&v))
            .unwrap_or_default();
        if !liste.is_empty() {
            return Ok(liste);
        }
    }
    if let Ok(o) = executer(exe, &["model", "list"]) {
        let liste = lire_liste_cli(&texte_sortie(&o));
        if !liste.is_empty() {
            return Ok(liste);
        }
    }
    let liste = lire_catalogue_json(&obtenir_json(&format!("{base}/foundry/list"))?);
    if liste.is_empty() {
        return Err("catalogue Foundry vide ou illisible".into());
    }
    Ok(liste)
}

/// Les modèles chargés par le service : la CLI (0.11+), sinon l'API (0.x).
pub fn modeles_charges(exe: &Path, base: &str) -> Result<Vec<String>, String> {
    let cli = executer(exe, &["model", "list", "--loaded", "-o", "json"])
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| premier_json(&texte_sortie(&o)));
    match cli {
        Some(v) => Ok(lire_ids_json(&v)),
        None => Ok(lire_ids_json(&obtenir_json(&format!(
            "{base}/openai/loadedmodels"
        ))?)),
    }
}

/// Les modèles déjà téléchargés (le cache de Foundry).
pub fn installes(exe: &Path) -> Vec<String> {
    let json = executer(exe, &["cache", "list", "-o", "json"])
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| premier_json(&texte_sortie(&o)))
        .map(|v| lire_ids_json(&v))
        .unwrap_or_default();
    if !json.is_empty() {
        return json;
    }
    executer(exe, &["cache", "list"])
        .map(|o| lire_cache_texte(&texte_sortie(&o)))
        .unwrap_or_default()
}

/// Le dossier où Foundry range ses modèles.
pub fn emplacement(exe: &Path) -> Option<String> {
    let o = executer(exe, &["cache", "location"]).ok()?;
    lire_chemin_windows(&texte_sortie(&o))
}

/// Le premier chemin Windows (`C:\…`) d'une sortie de CLI.
pub fn lire_chemin_windows(texte: &str) -> Option<String> {
    sans_ansi(texte).lines().find_map(|l| {
        let o = l.as_bytes();
        (0..o.len().saturating_sub(2))
            .find(|&i| o[i].is_ascii_alphabetic() && o[i + 1] == b':' && o[i + 2] == b'\\')
            .map(|i| l[i..].trim().trim_end_matches(['"', '\'']).to_string())
    })
}

/// Télécharge un modèle, seulement quand la personne l'a demandé.
pub fn telecharger(exe: &Path, m: &ModeleFoundry) -> Result<(), String> {
    eprintln!("[npu] Foundry : téléchargement de {}…", m.id);
    if executer_visible(exe, &["model", "download", &m.id])? {
        Ok(())
    } else {
        Err(format!("téléchargement de {} refusé par Foundry", m.id))
    }
}

/// Retire un modèle du cache de Foundry. La CLI peut demander confirmation :
/// on lui répond oui.
pub fn supprimer(exe: &Path, id: &str) -> Result<(), String> {
    use std::io::Write;
    let mut c = Command::new(exe);
    c.args(["cache", "remove", id])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    masquer_console(&mut c);
    let mut enfant = c
        .spawn()
        .map_err(|e| format!("impossible de lancer {} : {e}", exe.display()))?;
    if let Some(mut entree) = enfant.stdin.take() {
        if let Err(e) = entree.write_all(b"y\n") {
            eprintln!("[npu] confirmation non envoyée à Foundry : {e}");
        }
    }
    let o = enfant
        .wait_with_output()
        .map_err(|e| format!("suppression de {id} : {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Foundry refuse de supprimer {id} : {}",
            texte_sortie(&o).trim()
        ))
    }
}

/// Le nom sous lequel le service a chargé `m` (l'identifiant exact, version
/// comprise, même quand on a demandé un alias).
pub fn nom_charge(charges: &[String], m: &ModeleFoundry) -> Option<String> {
    let cherche = |cle: &str| {
        let cle = sans_version(cle).to_ascii_lowercase();
        charges
            .iter()
            .find(|c| sans_version(c).to_ascii_lowercase().contains(&cle))
            .cloned()
    };
    cherche(&m.id).or_else(|| cherche(&m.alias))
}

/// Charge un modèle déjà téléchargé ; rend son nom chargé.
pub fn charger(exe: &Path, base: &str, m: &ModeleFoundry) -> Result<String, String> {
    eprintln!("[npu] Foundry : chargement de {} sur le NPU…", m.id);
    if !executer_visible(exe, &["model", "load", &m.id])? {
        return Err(format!("chargement de {} refusé par Foundry", m.id));
    }
    let charges = modeles_charges(exe, base)?;
    Ok(nom_charge(&charges, m).unwrap_or_else(|| m.id.clone()))
}

/// Ce que le proxy sert : l'URL du service et le modèle chargé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amont {
    pub base: String,
    pub modele: String,
}

/// Les modèles NPU déjà téléchargés, tels que le catalogue les décrit.
pub fn npu_installes(exe: &Path, base: &str) -> Result<Vec<ModeleFoundry>, String> {
    let presents = installes(exe);
    Ok(catalogue(exe, base)?
        .into_iter()
        .filter(|m| m.est_npu() && est_installe(&presents, m))
        .collect())
}

/// Tout le chemin : installer au besoin, démarrer, choisir, charger.
pub fn preparer(demande: &str) -> Result<Amont, String> {
    let exe = match trouver() {
        Some(e) => e,
        None => installer()?,
    };
    let base = demarrer(&exe)?;
    eprintln!("[npu] Foundry Local en marche sur {base}");
    let presents = installes(&exe);
    let modele = choisir(demande, &catalogue(&exe, &base)?, &presents)?;
    eprintln!(
        "[npu] modèle {} ({}, {})",
        modele.alias, modele.id, modele.appareil
    );
    if !est_installe(&presents, &modele) {
        return Err(format!(
            "{} n'est pas téléchargé : cliquez « Télécharger le modèle » dans les réglages du morph.",
            modele.alias
        ));
    }
    let nom = charger(&exe, &base, &modele)?;
    Ok(Amont { base, modele: nom })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tout(l: &[ModeleFoundry]) -> Vec<String> {
        l.iter().map(|m| m.id.clone()).collect()
    }

    #[test]
    fn auto_ne_telecharge_rien_et_prend_un_modele_present() {
        let l = lire_liste_cli(LISTE_0X);
        let e = choisir("auto", &l, &[]).unwrap_err();
        assert!(e.contains("Aucun modèle NPU téléchargé"), "{e}");
        let present = vec!["Phi-4-mini-instruct-qnn-npu:2".to_string()];
        assert_eq!(choisir("auto", &l, &present).unwrap().alias, "phi-4-mini");
    }

    #[test]
    fn le_cache_et_son_dossier_se_lisent() {
        let ids = lire_cache_texte(
            "Models cached on device:\n  Alias  Model ID\n💾 phi-4-mini  Phi-4-mini-instruct-qnn-npu:2\n",
        );
        assert_eq!(ids, vec!["Phi-4-mini-instruct-qnn-npu:2"]);
        assert_eq!(
            lire_chemin_windows(
                r"💾 Cache directory path: C:\Users\a\.foundry\cache\models
"
            ),
            Some(r"C:\Users\a\.foundry\cache\models".into())
        );
    }

    const LISTE_0X: &str = "\
Alias                          Device     Task               File Size    License      Model ID
-----------------------------------------------------------------------------------------------
phi-4-mini                     GPU        chat-completion    3.72 GB      MIT          Phi-4-mini-instruct-generic-gpu
                               NPU        chat-completion    3.60 GB      MIT          Phi-4-mini-instruct-qnn-npu
                               CPU        chat-completion    4.80 GB      MIT          Phi-4-mini-instruct-generic-cpu
-----------------------------------------------------------------------------------------------
qwen2.5-1.5b                   NPU        chat-completion    2.78 GB      apache-2.0   qwen2.5-1.5b-instruct-qnn-npu
                               CPU        chat-completion    1.78 GB      apache-2.0   qwen2.5-1.5b-instruct-generic-cpu
-----------------------------------------------------------------------------------------------
whisper-tiny                   NPU        automatic-speech-recognition 0.18 GB MIT     openai-whisper-tiny-qnn-npu
";

    #[test]
    fn le_tableau_de_la_cli_est_lu_avec_ses_variantes() {
        let l = lire_liste_cli(LISTE_0X);
        assert_eq!(l.len(), 5, "whisper n'est pas un modèle de conversation");
        assert_eq!(l[1].alias, "phi-4-mini");
        assert_eq!(l[1].appareil, "NPU");
        assert_eq!(l[1].id, "Phi-4-mini-instruct-qnn-npu");
        assert_eq!(l[1].taille_mo, Some(3686));
        assert_eq!(l[3].alias, "qwen2.5-1.5b");
    }

    #[test]
    fn sans_colonne_d_identifiant_l_alias_sert_d_identifiant() {
        let t = "Alias        Device  Task  File Size\nphi-4-mini   NPU     chat  3.6 GB\n";
        let l = lire_liste_cli(t);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].id, "phi-4-mini");
    }

    #[test]
    fn les_couleurs_et_la_progression_sont_ignorees() {
        let t = "\u{1b}[32mDownloading execution providers 100%\u{1b}[0m\n\
                 \u{1b}[1mphi-4-mini\u{1b}[0m  NPU  chat-completion  3.60 GB  MIT  Phi-4-mini-instruct-qnn-npu:2\n";
        let l = lire_liste_cli(t);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].id, "Phi-4-mini-instruct-qnn-npu:2");
    }

    #[test]
    fn l_url_du_service_est_lue_quel_que_soit_le_format() {
        assert_eq!(
            lire_url_service(
                "🟢 Model management service is running on http://127.0.0.1:5273/openai/status"
            ),
            Some("http://127.0.0.1:5273".into())
        );
        assert_eq!(
            lire_url_service("State: running\nEndpoint: http://localhost:52341/v1\nPID: 4242"),
            Some("http://127.0.0.1:52341".into())
        );
        // `server status` et `server start` de la CLI 0.11 (sorties réelles).
        assert_eq!(
            lire_url_service("\u{1b}[1mState\u{1b}[0m    Ready\n\u{1b}[1mWeb URLs\u{1b}[0m http://127.0.0.1:50116   \n"),
            Some("http://127.0.0.1:50116".into())
        );
        assert_eq!(
            lire_url_service(
                "success: Server ready \u{1b}[38;2;107;114;128m(http://127.0.0.1:50116)\u{1b}[0m"
            ),
            Some("http://127.0.0.1:50116".into())
        );
        assert_eq!(lire_url_service("note: Server is not running."), None);
        assert_eq!(lire_url_service("http://127.0.0.1:0/foundry/list"), None);
    }

    #[test]
    fn le_catalogue_json_de_la_cli_0_11_est_lu() {
        // Forme réelle de `foundry model list --variants -o json` (0.11),
        // précédée d'une ligne de progression.
        let t = r#"QNNExecutionProvider: 100%
{"variants":[{"alias":"qwen2.5-1.5b","variantName":"qwen2.5-1.5b-instruct-qnn-npu",
"variantId":"qwen2.5-1.5b-instruct-qnn-npu:3","type":"Chat","device":"Npu",
"executionProvider":"QNNExecutionProvider","fileSizeMb":1700,"cached":false},
{"alias":"qwen2.5-1.5b","variantId":"qwen2.5-1.5b-instruct-generic-cpu:4",
"type":"Chat","device":"Cpu","fileSizeMb":1822}]}"#;
        let l = lire_catalogue_json(&premier_json(t).unwrap());
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].id, "qwen2.5-1.5b-instruct-qnn-npu:3");
        assert!(l[0].est_npu());
        assert!(!l[1].est_npu());
        assert_eq!(
            choisir("auto", &l, &tout(&l)).unwrap().id,
            "qwen2.5-1.5b-instruct-qnn-npu:3"
        );
    }

    #[test]
    fn les_modeles_charges_sont_lus_en_json() {
        let v = premier_json(
            r#"{"models":[{"alias":"phi-4-mini","id":"Phi-4-mini-instruct-qnn-npu:2","type":"Chat","device":"Npu"}]}"#,
        )
        .unwrap();
        assert_eq!(
            lire_ids_json(&v),
            vec!["Phi-4-mini-instruct-qnn-npu:2".to_string()]
        );
        assert!(lire_ids_json(&premier_json(r#"{"models":[]}"#).unwrap()).is_empty());
        let ancien = serde_json::json!(["Phi-4-mini-instruct-qnn-npu"]);
        assert_eq!(
            lire_ids_json(&ancien),
            vec!["Phi-4-mini-instruct-qnn-npu".to_string()]
        );
    }

    #[test]
    fn le_catalogue_json_est_lu() {
        let v = serde_json::json!({"models": [
            {"name": "Phi-4-mini-instruct-qnn-npu", "alias": "phi-4-mini", "task": "chat-completion",
             "runtime": {"deviceType": "NPU", "executionProvider": "QNNExecutionProvider"}, "fileSizeMb": 3686},
            {"name": "openai-whisper-tiny-qnn-npu", "alias": "whisper-tiny", "task": "automatic-speech-recognition",
             "runtime": {"deviceType": "NPU"}}
        ]});
        let l = lire_catalogue_json(&v);
        assert_eq!(l.len(), 1);
        assert!(l[0].est_npu());
        assert_eq!(l[0].taille_mo, Some(3686));
    }

    #[test]
    fn auto_prend_un_petit_modele_npu() {
        let l = lire_liste_cli(LISTE_0X);
        let m = choisir("auto", &l, &tout(&l)).unwrap();
        assert_eq!(m.id, "qwen2.5-1.5b-instruct-qnn-npu");
    }

    #[test]
    fn un_alias_donne_sa_variante_npu() {
        let l = lire_liste_cli(LISTE_0X);
        assert_eq!(
            choisir("Phi-4-Mini", &l, &[]).unwrap().id,
            "Phi-4-mini-instruct-qnn-npu"
        );
        assert_eq!(
            choisir("phi-4-mini-instruct-qnn-npu:3", &l, &[])
                .unwrap()
                .id,
            "Phi-4-mini-instruct-qnn-npu"
        );
    }

    #[test]
    fn un_gguf_ou_un_modele_sans_npu_est_refuse_clairement() {
        let l = lire_liste_cli(LISTE_0X);
        let e = choisir("Qwen3-4B-Q4_K_M.gguf", &l, &[]).unwrap_err();
        assert!(e.contains("GGUF"), "{e}");
        assert!(e.contains("phi-4-mini"), "{e}");
        let cpu_seul = vec![ModeleFoundry {
            alias: "phi-4".into(),
            id: "Phi-4-generic-cpu".into(),
            appareil: "CPU".into(),
            taille_mo: None,
        }];
        assert!(choisir("auto", &cpu_seul, &[])
            .unwrap_err()
            .contains("pilote"));
    }

    #[test]
    fn le_nom_charge_est_retrouve_avec_sa_version() {
        let m = ModeleFoundry {
            alias: "phi-4-mini".into(),
            id: "phi-4-mini".into(),
            appareil: "NPU".into(),
            taille_mo: None,
        };
        let charges = vec!["Phi-4-mini-instruct-qnn-npu:2".to_string()];
        assert_eq!(
            nom_charge(&charges, &m),
            Some("Phi-4-mini-instruct-qnn-npu:2".into())
        );
        assert_eq!(nom_charge(&[], &m), None);
    }

    #[test]
    fn winget_installe_sans_question() {
        let a = arguments_winget().join(" ");
        assert!(a.contains("--id Microsoft.FoundryLocal"));
        assert!(a.contains("--silent"));
        assert!(a.contains("--accept-package-agreements"));
    }
}
