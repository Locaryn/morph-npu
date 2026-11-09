//! Quel build du runtime convient à cette machine.

/// Les deux chemins vers le NPU de ce morph, pour Windows ARM64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Microsoft Foundry Local (fournisseur QNN signé par Qualcomm) : aucun
    /// réglage de sécurité à toucher. Modèles du catalogue Foundry seulement.
    Foundry,
    /// llama.cpp Snapdragon (NPU Hexagon, GPU Adreno) : fichiers GGUF, mais
    /// exige le mode test-signing de Windows.
    Npu,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Foundry => "foundry",
            Backend::Npu => "npu",
        }
    }
}

/// Le paquet publié par Locaryn/llama-snapdragon.
pub const ASSET_SNAPDRAGON: &str = "llama-b11003-bin-win-snapdragon-arm64.zip";

/// L'architecture de la *machine*, pas celle de ce programme : un lanceur x64
/// émulé sur un Snapdragon doit quand même télécharger le runtime ARM64,
/// natif. Windows le dit dans `PROCESSOR_ARCHITEW6432` (processus émulé) ou
/// `PROCESSOR_ARCHITECTURE`.
pub fn host_arch() -> &'static str {
    if cfg!(windows) {
        for var in ["PROCESSOR_ARCHITEW6432", "PROCESSOR_ARCHITECTURE"] {
            if let Ok(v) = std::env::var(var) {
                if v.eq_ignore_ascii_case("ARM64") {
                    return "aarch64";
                }
            }
        }
    }
    std::env::consts::ARCH
}

/// Le backend à utiliser. `demande` vient des réglages (`auto` par défaut).
pub fn choose(demande: &str, os: &str, arch: &str) -> Result<Backend, String> {
    if os != "windows" || arch != "aarch64" {
        return Err(format!(
            "morph-npu ne fonctionne que sur Windows ARM64 (Snapdragon) ; cette machine est {os} {arch}."
        ));
    }
    match demande.trim() {
        "" | "auto" | "foundry" => Ok(Backend::Foundry),
        "llamacpp" | "llama.cpp" | "npu" | "hexagon" => Ok(Backend::Npu),
        autre => Err(format!("backend « {autre} » inconnu (foundry, llamacpp)")),
    }
}

/// Les archives à télécharger. `tag` est la release du dépôt llama-snapdragon.
pub fn assets(_tag: &str, backend: Backend, os: &str, arch: &str) -> Result<Vec<String>, String> {
    match (os, arch, backend) {
        ("windows", "aarch64", Backend::Npu) => Ok(vec![ASSET_SNAPDRAGON.to_string()]),
        _ => Err(format!(
            "pas de runtime NPU publié pour {os} {arch} avec le backend {}",
            backend.name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seul_le_snapdragon_est_accepte() {
        assert_eq!(choose("auto", "windows", "aarch64"), Ok(Backend::Foundry));
        assert_eq!(
            choose("foundry", "windows", "aarch64"),
            Ok(Backend::Foundry)
        );
        assert_eq!(choose("llamacpp", "windows", "aarch64"), Ok(Backend::Npu));
        assert_eq!(choose("npu", "windows", "aarch64"), Ok(Backend::Npu));
        assert!(choose("auto", "windows", "x86_64").is_err());
        assert!(choose("auto", "linux", "aarch64").is_err());
        assert!(choose("cuda", "windows", "aarch64").is_err());
    }

    #[test]
    fn le_paquet_snapdragon_est_nomme() {
        let a = assets("b11003-locaryn.1", Backend::Npu, "windows", "aarch64").unwrap();
        assert_eq!(
            a,
            vec!["llama-b11003-bin-win-snapdragon-arm64.zip".to_string()]
        );
        assert!(assets("t", Backend::Npu, "windows", "x86_64").is_err());
    }
}
