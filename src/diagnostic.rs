//! Le diagnostic du bouton « Vérifier » : où ça casse, dit en clair.
//!
//! Cinq questions, dans l'ordre où une panne les rend inutiles : Foundry
//! est-il installé, Windows voit-il un NPU, le catalogue contient-il des
//! modèles NPU, quel modèle tourne et sur quel appareil, où est le journal.
//! La collecte (CLI, PowerShell, HTTP) reste dans le binaire MCP ; ici, la
//! rédaction seule, testable avec de faux constats.

use crate::foundry::{bilan_catalogue, ModeleFoundry};
use serde_json::Value;

/// Ce qu'on sait du modèle qui tourne.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execution {
    pub variante: String,
    /// `NPU`, `GPU` ou `CPU`.
    pub appareil: String,
    pub fournisseur: Option<String>,
    /// Foundry l'a déclaré lui-même (`--loaded`), ou c'est la variante que la
    /// passerelle lui a fait servir.
    pub declare_par_le_moteur: bool,
}

/// L'état de la passerelle du morph (`/health`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Passerelle {
    Prete { journal: String },
    Demarre { journal: String },
    Muette,
}

/// Tout ce que « Vérifier » a pu constater.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constat {
    /// Chemin (et version) de la CLI `foundry`, `None` si absente.
    pub foundry: Option<String>,
    /// Les accélérateurs que Windows déclare ; `None` : non vérifiable.
    pub npu_windows: Option<Vec<String>>,
    /// Le catalogue de Foundry, ou pourquoi il n'a pas pu être lu.
    pub catalogue: Result<Vec<ModeleFoundry>, String>,
    pub execution: Option<Execution>,
    pub passerelle: Passerelle,
    /// Mode llama.cpp seulement : le mode test-signing est-il actif ?
    pub test_signing: Option<bool>,
}

/// Le verdict : le NPU sert-il vraiment, et les lignes à afficher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub npu_utilise: bool,
    pub lignes: Vec<String>,
}

impl Diagnostic {
    pub fn message(&self) -> String {
        self.lignes.join("\n")
    }
}

/// Les noms d'accélérateurs dans la sortie de la commande PowerShell (un par
/// ligne).
pub fn lire_npu_windows(texte: &str) -> Vec<String> {
    texte
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Le modèle qui tourne, d'après la liste `/v1/models` de la passerelle.
pub fn lire_execution_passerelle(v: &Value) -> Option<Execution> {
    let liste = v.get("data")?.as_array()?;
    let m = liste.iter().find(|m| m["loaded"].as_bool() == Some(true))?;
    let r = m.get("runtime")?;
    Some(Execution {
        variante: r["variant"].as_str().unwrap_or_default().to_string(),
        appareil: r["device"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_uppercase(),
        fournisseur: r["execution_provider"].as_str().map(str::to_string),
        declare_par_le_moteur: r["source"].as_str() == Some("engine"),
    })
}

fn ligne_foundry(c: &Constat) -> String {
    match &c.foundry {
        Some(f) => format!("Foundry Local : installé ({f})."),
        None => "Foundry Local : non installé. Il s'installe au premier lancement de la \
                 passerelle, ou cliquez « Installer Foundry Local »."
            .into(),
    }
}

fn ligne_npu(c: &Constat) -> String {
    match &c.npu_windows {
        Some(n) if !n.is_empty() => format!("NPU vu par Windows : {}.", n.join(", ")),
        Some(_) => "NPU vu par Windows : aucun. Ce PC n'a pas de NPU que Windows déclare \
                    (Snapdragon X attendu) ou son pilote manque : Windows Update."
            .into(),
        None => "NPU vu par Windows : non vérifiable sur ce système.".into(),
    }
}

fn ligne_catalogue(c: &Constat) -> String {
    match &c.catalogue {
        Ok(l) => format!("Catalogue : {}", bilan_catalogue(l)),
        Err(e) => format!("Catalogue : illisible ({e})."),
    }
}

fn ligne_execution(c: &Constat) -> String {
    let Some(e) = &c.execution else {
        return "Modèle chargé : aucun pour l'instant. Choisissez un modèle NPU dans la liste \
                des modèles et envoyez un message."
            .into();
    };
    let ep = e
        .fournisseur
        .as_deref()
        .map_or_else(String::new, |f| format!(", {f}"));
    let source = if e.declare_par_le_moteur {
        "déclaré par Foundry"
    } else {
        "variante servie par Foundry à la dernière réponse"
    };
    let ou = match e.appareil.as_str() {
        "NPU" => "sur le NPU".to_string(),
        "" => "sur un appareil que Foundry ne précise pas".to_string(),
        autre => format!("sur le {autre}, PAS sur le NPU"),
    };
    format!(
        "Modèle chargé : {} {ou} (Foundry Local{ep} ; {source}).",
        e.variante
    )
}

fn ligne_journal(c: &Constat) -> String {
    match &c.passerelle {
        Passerelle::Prete { journal } => format!("Passerelle : prête. Journal : {journal}"),
        Passerelle::Demarre { journal } => {
            format!("Passerelle : en cours de démarrage. Journal : {journal}")
        }
        Passerelle::Muette => "Passerelle : ne répond pas sur 127.0.0.1:8190. Journal : \
                               gateways/npu/gateway.log dans le dossier de données de Locaryn."
            .into(),
    }
}

/// Rédige le diagnostic. Le NPU n'est déclaré « utilisé » que si un modèle
/// tourne sur une variante NPU : un catalogue NPU ou un NPU vu ne suffisent
/// pas.
pub fn rediger(c: &Constat) -> Diagnostic {
    let mut lignes = vec![ligne_foundry(c), ligne_npu(c)];
    if c.foundry.is_some() {
        lignes.push(ligne_catalogue(c));
        lignes.push(ligne_execution(c));
    }
    if let Some(actif) = c.test_signing {
        lignes.push(if actif {
            "Mode llama.cpp : mode test-signing actif, le NPU Hexagon est accessible (une \
             partie des calculs seulement)."
                .into()
        } else {
            "Mode llama.cpp : mode test-signing inactif, le NPU Hexagon est inaccessible \
             (« Activer le mode test », puis redémarrer)."
                .into()
        });
    }
    lignes.push(ligne_journal(c));
    let npu_utilise = c.execution.as_ref().is_some_and(|e| e.appareil == "NPU");
    Diagnostic {
        npu_utilise,
        lignes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn variante(alias: &str, id: &str, appareil: &str) -> ModeleFoundry {
        ModeleFoundry {
            alias: alias.into(),
            id: id.into(),
            appareil: appareil.into(),
            ..Default::default()
        }
    }

    /// Le PC de développement : x64, Foundry installé, aucune variante NPU.
    fn pc_x64() -> Constat {
        Constat {
            foundry: Some("C:\\foundry.exe, 0.11.0".into()),
            npu_windows: Some(Vec::new()),
            catalogue: Ok(vec![
                variante("qwen2.5-0.5b", "qwen2.5-0.5b-instruct-generic-cpu:4", "CPU"),
                variante("qwen3.8-27b", "qwen3.8-27b-cuda-gpu:1", "GPU"),
            ]),
            execution: Some(Execution {
                variante: "qwen2.5-0.5b-instruct-generic-cpu:4".into(),
                appareil: "CPU".into(),
                fournisseur: Some("CPUExecutionProvider".into()),
                declare_par_le_moteur: false,
            }),
            passerelle: Passerelle::Prete {
                journal: "D:\\banc\\gateways\\npu\\gateway.log".into(),
            },
            test_signing: None,
        }
    }

    #[test]
    fn sans_variante_npu_le_diagnostic_le_dit_et_ne_ment_pas() {
        let d = rediger(&pc_x64());
        let m = d.message();
        assert!(!d.npu_utilise);
        assert!(m.contains("Aucune variante NPU sur 2"), "{m}");
        assert!(m.contains("NPU vu par Windows : aucun"), "{m}");
        assert!(m.contains("sur le CPU, PAS sur le NPU"), "{m}");
        assert!(m.contains("gateway.log"), "{m}");
    }

    #[test]
    fn un_snapdragon_qui_sert_une_variante_npu_est_reconnu() {
        let mut c = pc_x64();
        c.npu_windows = Some(vec![
            "Snapdragon(R) X Elite - Qualcomm(R) Hexagon(TM) NPU".into()
        ]);
        c.catalogue = Ok(vec![variante(
            "phi-4-mini",
            "Phi-4-mini-instruct-qnn-npu:2",
            "NPU",
        )]);
        c.execution = Some(Execution {
            variante: "Phi-4-mini-instruct-qnn-npu:2".into(),
            appareil: "NPU".into(),
            fournisseur: Some("QNNExecutionProvider".into()),
            declare_par_le_moteur: true,
        });
        let d = rediger(&c);
        assert!(d.npu_utilise);
        let m = d.message();
        assert!(m.contains("1 variante(s) NPU sur 1"), "{m}");
        assert!(
            m.contains("sur le NPU (Foundry Local, QNNExecutionProvider ; déclaré par Foundry)"),
            "{m}"
        );
    }

    #[test]
    fn foundry_absent_arrete_le_diagnostic_a_la_premiere_cause() {
        let mut c = pc_x64();
        c.foundry = None;
        c.passerelle = Passerelle::Muette;
        let m = rediger(&c).message();
        assert!(m.contains("non installé"), "{m}");
        assert!(!m.contains("Catalogue"), "{m}");
        assert!(m.contains("ne répond pas"), "{m}");
    }

    #[test]
    fn le_modele_qui_tourne_se_lit_dans_la_liste_de_la_passerelle() {
        let v = json!({"data": [
            {"id": "a", "loaded": false},
            {"id": "b", "loaded": true, "runtime": {"engine": "Foundry Local", "device": "npu",
             "execution_provider": "QNNExecutionProvider", "variant": "b:1", "source": "engine"}}
        ]});
        let e = lire_execution_passerelle(&v).unwrap();
        assert_eq!(e.appareil, "NPU");
        assert_eq!(e.variante, "b:1");
        assert!(e.declare_par_le_moteur);
        assert!(lire_execution_passerelle(&json!({"data": []})).is_none());
    }

    #[test]
    fn les_noms_d_accelerateurs_sont_lus_ligne_a_ligne() {
        assert_eq!(
            lire_npu_windows("\r\nQualcomm(R) Hexagon(TM) NPU\r\n  \r\n"),
            vec!["Qualcomm(R) Hexagon(TM) NPU".to_string()]
        );
        assert!(lire_npu_windows("").is_empty());
    }
}
