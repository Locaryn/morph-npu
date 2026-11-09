//! Réglages de l'extension, lus dans le fichier que l'hôte désigne.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Version du runtime épinglée : deux machines installent la même chose.
pub const RUNTIME_TAG: &str = "b11003-locaryn.1";
pub const RUNTIME_REPO: &str = "Locaryn/llama-snapdragon";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// `foundry` (défaut, `auto` aussi) ou `llamacpp` (Hexagon, mode test).
    #[serde(default = "backend_auto")]
    pub backend: String,
    /// Modèle Foundry Local : `auto`, un alias du catalogue (`phi-4-mini`) ou
    /// l'identifiant exact d'une variante NPU. Ignoré par llama.cpp.
    #[serde(default = "foundry_auto")]
    pub foundry_model: String,
    /// Taille du contexte. 8192 laisse de la place au modèle sur une carte de 6 Go.
    #[serde(default = "contexte_defaut", deserialize_with = "nombre")]
    pub context_size: u32,
    /// Charger le projecteur d'images quand il est à côté du modèle. Coûte
    /// environ 0,6 Go de mémoire vidéo : coupé par défaut.
    #[serde(default, deserialize_with = "booleen")]
    pub vision: bool,
    /// Couches placées sur la carte. 0 (défaut) : llama.cpp mesure la mémoire
    /// libre et répartit lui-même — un modèle trop gros pour la carte passe en
    /// partie sur le processeur au lieu d'échouer. Un nombre impose le choix
    /// (999 : toutes, au risque d'un échec de mémoire).
    #[serde(default = "couches_defaut", deserialize_with = "nombre")]
    pub gpu_layers: u32,
    /// Requêtes traitées en parallèle. Chaque emplacement a son cache : 1 est
    /// ce qui tient dans une petite carte.
    #[serde(default = "parallele_defaut", deserialize_with = "nombre")]
    pub parallel: u32,
    /// Raisonnement du modèle : `off` (défaut), `on` ou `auto`. Un modèle de raisonnement
    /// réfléchit longuement avant de répondre ; sur une carte modeste cela multiplie par
    /// plusieurs fois le temps d'une réponse, d'où `off`.
    #[serde(default = "raisonnement_defaut")]
    pub thinking: String,
    /// Autre version du runtime que celle épinglée.
    #[serde(default)]
    pub runtime_tag: String,
}

fn backend_auto() -> String {
    "auto".into()
}
fn foundry_auto() -> String {
    "auto".into()
}
fn contexte_defaut() -> u32 {
    8192
}
fn couches_defaut() -> u32 {
    0
}
fn parallele_defaut() -> u32 {
    1
}
fn raisonnement_defaut() -> String {
    "off".into()
}

/// L'interface des réglages écrit parfois les nombres en chaînes.
fn nombre<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    use serde::de::Error;
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| D::Error::custom("nombre hors limites")),
        serde_json::Value::String(s) => s.trim().parse().map_err(D::Error::custom),
        autre => Err(D::Error::custom(format!("nombre attendu, reçu {autre}"))),
    }
}

fn booleen<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    use serde::de::Error;
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Bool(b) => Ok(b),
        serde_json::Value::String(s) => Ok(matches!(s.trim(), "true" | "1" | "yes" | "oui")),
        autre => Err(D::Error::custom(format!("booléen attendu, reçu {autre}"))),
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            backend: backend_auto(),
            foundry_model: foundry_auto(),
            context_size: contexte_defaut(),
            vision: false,
            gpu_layers: couches_defaut(),
            parallel: parallele_defaut(),
            thinking: raisonnement_defaut(),
            runtime_tag: String::new(),
        }
    }
}

impl Settings {
    /// Lit le fichier désigné par l'hôte ; les défauts s'il est absent, et une
    /// erreur dite si son contenu est invalide (jamais d'ignorance silencieuse).
    pub fn load() -> Result<Self, String> {
        let Some(chemin) = std::env::var_os("LOCARYN_EXTENSION_CONFIG_FILE")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
        else {
            return Ok(Self::default());
        };
        if !chemin.exists() {
            return Ok(Self::default());
        }
        let brut = std::fs::read_to_string(&chemin)
            .map_err(|e| format!("réglages illisibles ({}) : {e}", chemin.display()))?;
        serde_json::from_str(&brut)
            .map_err(|e| format!("réglages invalides ({}) : {e}", chemin.display()))
    }

    pub fn tag(&self) -> &str {
        if self.runtime_tag.trim().is_empty() {
            RUNTIME_TAG
        } else {
            self.runtime_tag.trim()
        }
    }
}

/// Dossier privé de l'extension, où vit le runtime téléchargé.
pub fn data_dir() -> PathBuf {
    for var in ["LOCARYN_EXTENSION_DATA_DIR", "LOCARYN_DATA_DIR"] {
        if let Some(d) = std::env::var_os(var).filter(|d| !d.is_empty()) {
            return PathBuf::from(d);
        }
    }
    std::env::temp_dir().join("locaryn-npu")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_defauts_tiennent_sur_une_petite_carte() {
        let s = Settings::default();
        assert_eq!(s.context_size, 8192);
        assert_eq!(s.parallel, 1);
        assert_eq!(s.gpu_layers, 0, "la répartition est laissée à llama.cpp");
        assert_eq!(s.thinking, "off");
        assert!(
            !s.vision,
            "le projecteur d'images coûte de la mémoire vidéo"
        );
        assert_eq!(s.tag(), RUNTIME_TAG);
    }

    #[test]
    fn l_interface_peut_ecrire_les_nombres_en_chaines() {
        let s: Settings =
            serde_json::from_str(r#"{"context_size":"16384","vision":"true","parallel":2}"#)
                .unwrap();
        assert_eq!(s.context_size, 16384);
        assert!(s.vision);
        assert_eq!(s.parallel, 2);
    }

    #[test]
    fn un_reglage_absurde_est_refuse() {
        assert!(serde_json::from_str::<Settings>(r#"{"context_size":"beaucoup"}"#).is_err());
    }

    #[test]
    fn un_tag_vide_retombe_sur_l_epingle() {
        let s: Settings = serde_json::from_str(r#"{"runtime_tag":"  "}"#).unwrap();
        assert_eq!(s.tag(), RUNTIME_TAG);
        let s: Settings = serde_json::from_str(r#"{"runtime_tag":"autre-tag"}"#).unwrap();
        assert_eq!(s.tag(), "autre-tag");
    }
}
