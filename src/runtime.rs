//! Télécharger, vérifier et installer le runtime. Rien n'est exécuté avant
//! que l'empreinte SHA-256 publiée par GitHub ne corresponde.

use crate::backend::{self, Backend};
use crate::settings::RUNTIME_REPO;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// Empreinte hexadécimale en minuscules, sans le préfixe `sha256:`.
    pub sha256: Option<String>,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent("locaryn-npu")
        .timeout_connect(std::time::Duration::from_secs(20))
        .build()
}

/// Les fichiers d'une release, avec leurs empreintes.
pub fn parse_assets(release: &Value) -> Vec<Asset> {
    release["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(Asset {
                        name: x["name"].as_str()?.to_string(),
                        url: x["browser_download_url"].as_str()?.to_string(),
                        sha256: x["digest"]
                            .as_str()
                            .and_then(|d| d.strip_prefix("sha256:"))
                            .map(str::to_lowercase),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn release_assets(tag: &str) -> Result<Vec<Asset>, String> {
    let url = format!("https://api.github.com/repos/{RUNTIME_REPO}/releases/tags/{tag}");
    let json: Value = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("release {tag} introuvable sur github.com/{RUNTIME_REPO} : {e}"))?
        .into_json()
        .map_err(|e| format!("réponse de GitHub illisible : {e}"))?;
    Ok(parse_assets(&json))
}

/// Télécharge `asset` dans `dest` et refuse tout fichier dont l'empreinte diffère.
fn download(asset: &Asset, dest: &Path) -> Result<(), String> {
    let attendue = asset.sha256.as_deref().ok_or_else(|| {
        format!(
            "GitHub ne publie pas d'empreinte pour {} : téléchargement refusé, rien ne permettrait de le vérifier.",
            asset.name
        )
    })?;
    eprintln!("[npu] téléchargement de {}", asset.name);
    let resp = agent()
        .get(&asset.url)
        .call()
        .map_err(|e| format!("téléchargement de {} impossible : {e}", asset.name))?;
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut lecteur = resp.into_reader();
    let mut fichier =
        File::create(dest).map_err(|e| format!("écriture de {} : {e}", dest.display()))?;
    let mut sha = Sha256::new();
    let mut tampon = vec![0u8; 256 * 1024];
    let (mut recu, mut dernier_palier) = (0u64, 0u64);
    loop {
        let n = lecteur
            .read(&mut tampon)
            .map_err(|e| format!("lecture interrompue pour {} : {e}", asset.name))?;
        if n == 0 {
            break;
        }
        sha.update(&tampon[..n]);
        fichier
            .write_all(&tampon[..n])
            .map_err(|e| format!("écriture de {} : {e}", dest.display()))?;
        recu += n as u64;
        if total > 0 && recu * 10 / total > dernier_palier {
            dernier_palier = recu * 10 / total;
            eprintln!("[npu]   {} %", dernier_palier * 10);
        }
    }
    let obtenue = format!("{:x}", sha.finalize());
    if obtenue != attendue {
        // Un fichier qui ne correspond pas ne doit pas rester sur le disque.
        if let Err(e) = std::fs::remove_file(dest) {
            eprintln!("[npu] suppression de {} : {e}", dest.display());
        }
        return Err(format!(
            "EMPREINTE DIFFÉRENTE pour {} : attendue {attendue}, reçue {obtenue}. Rien n'a été installé.",
            asset.name
        ));
    }
    Ok(())
}

fn extract(archive: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("dossier {} : {e}", dest.display()))?;
    let nom = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let fichier =
        File::open(archive).map_err(|e| format!("ouverture de {} : {e}", archive.display()))?;
    if nom.ends_with(".zip") {
        let mut z = zip::ZipArchive::new(fichier).map_err(|e| format!("{nom} illisible : {e}"))?;
        for i in 0..z.len() {
            let mut entree = z.by_index(i).map_err(|e| format!("{nom} : {e}"))?;
            // Un chemin qui sort du dossier est refusé, pas corrigé.
            let rel = entree
                .enclosed_name()
                .ok_or_else(|| format!("{nom} contient un chemin dangereux : {}", entree.name()))?;
            let cible = dest.join(rel);
            if entree.is_dir() {
                std::fs::create_dir_all(&cible).map_err(|e| format!("{}: {e}", cible.display()))?;
                continue;
            }
            if let Some(p) = cible.parent() {
                std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
            }
            let mut sortie =
                File::create(&cible).map_err(|e| format!("{}: {e}", cible.display()))?;
            std::io::copy(&mut entree, &mut sortie)
                .map_err(|e| format!("extraction de {} : {e}", cible.display()))?;
        }
        Ok(())
    } else if nom.ends_with(".tar.gz") {
        tar::Archive::new(flate2::read::GzDecoder::new(fichier))
            .unpack(dest)
            .map_err(|e| format!("extraction de {nom} : {e}"))
    } else {
        Err(format!("format d'archive inconnu : {nom}"))
    }
}

/// Le `llama-server` d'un dossier d'installation, quelle que soit sa profondeur.
pub fn find_server(dir: &Path) -> Option<PathBuf> {
    fn cherche(dir: &Path, reste: u32) -> Option<PathBuf> {
        for entree in std::fs::read_dir(dir).ok()?.flatten() {
            let chemin = entree.path();
            if chemin.is_dir() {
                if reste > 0 {
                    if let Some(trouve) = cherche(&chemin, reste - 1) {
                        return Some(trouve);
                    }
                }
            } else if matches!(
                chemin.file_name().and_then(|n| n.to_str()),
                Some("llama-server" | "llama-server.exe")
            ) {
                return Some(chemin);
            }
        }
        None
    }
    cherche(dir, 3)
}

/// Dossier d'une installation du runtime.
pub fn install_dir(data: &Path, tag: &str, backend: Backend, arch: &str) -> PathBuf {
    // Le runtime ARM64 a son propre dossier : un runtime x64 posé plus tôt
    // (lanceur émulé) ne doit pas être repris sur un Snapdragon. Le dossier
    // x64 garde son nom d'avant, pour ne rien retélécharger.
    let suffixe = if arch == "aarch64" { "-arm64" } else { "" };
    data.join("runtime")
        .join(format!("{tag}-{}{suffixe}", backend.name()))
}

/// Poser à côté de `llama-server` le runtime Visual C++ livré avec le lanceur
/// (`bin/vcrt/`), quand le système ne l'a pas.
///
/// Les runtimes Windows de llama.cpp Snapdragon dépendent de `VCRUNTIME140.dll` et
/// `MSVCP140.dll` sans les fournir. Sur un Snapdragon sans le « Visual C++
/// Redistributable ARM64 » — fréquent —, `llama-server` mourait au démarrage
/// (DLL introuvable) et le morph attendait un serveur qui ne viendrait jamais.
/// Microsoft autorise ce dépôt à côté de l'application ; aucune installation
/// ni droit d'administrateur n'est demandé. Rien ne change si les DLL sont
/// déjà dans le dossier.
pub fn poser_runtime_vc(serveur: &Path) {
    if !cfg!(windows) {
        return;
    }
    let Some(cible) = serveur.parent() else {
        return;
    };
    let Some(source) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("vcrt")))
    else {
        return;
    };
    let Ok(entrees) = std::fs::read_dir(&source) else {
        return;
    };
    for e in entrees.flatten() {
        let nom = e.file_name();
        let est_dll = nom.to_string_lossy().to_ascii_lowercase().ends_with(".dll");
        let dest = cible.join(&nom);
        if est_dll && !dest.exists() {
            match std::fs::copy(e.path(), &dest) {
                Ok(_) => eprintln!("[npu] runtime Visual C++ posé : {}", nom.to_string_lossy()),
                Err(err) => eprintln!(
                    "[npu] {} non copié à côté du serveur : {err}",
                    nom.to_string_lossy()
                ),
            }
        }
    }
}

/// Le chemin de `llama-server`, installé au besoin.
pub fn ensure(
    data: &Path,
    tag: &str,
    backend: Backend,
    os: &str,
    arch: &str,
) -> Result<PathBuf, String> {
    let dossier = install_dir(data, tag, backend, arch);
    let pret = dossier.join(".ready");
    if pret.exists() {
        if let Some(serveur) = find_server(&dossier) {
            // Aussi pour une installation d'avant ce correctif.
            poser_runtime_vc(&serveur);
            return Ok(serveur);
        }
    }
    let noms = backend::assets(tag, backend, os, arch)?;
    let disponibles = release_assets(tag)?;
    let tmp = data
        .join("runtime")
        .join(format!(".dl-{tag}-{}", backend.name()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("dossier {} : {e}", tmp.display()))?;
    // Un essai précédent interrompu laisse une installation à moitié écrite.
    if dossier.exists() {
        std::fs::remove_dir_all(&dossier)
            .map_err(|e| format!("nettoyage de {} : {e}", dossier.display()))?;
    }
    let resultat = (|| {
        for nom in &noms {
            let asset = disponibles
                .iter()
                .find(|a| &a.name == nom)
                .ok_or_else(|| format!("la release {tag} ne contient pas {nom}"))?;
            let archive = tmp.join(&asset.name);
            download(asset, &archive)?;
            eprintln!("[npu] extraction de {}", asset.name);
            extract(&archive, &dossier)?;
            if let Err(e) = std::fs::remove_file(&archive) {
                eprintln!("[npu] suppression de {} : {e}", archive.display());
            }
        }
        let serveur = find_server(&dossier)
            .ok_or_else(|| format!("llama-server absent de {}", dossier.display()))?;
        poser_runtime_vc(&serveur);
        std::fs::write(&pret, noms.join("\n")).map_err(|e| format!("{}: {e}", pret.display()))?;
        Ok(serveur)
    })();
    if let Err(e) = std::fs::remove_dir_all(&tmp) {
        eprintln!("[npu] nettoyage de {} : {e}", tmp.display());
    }
    resultat
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn les_empreintes_de_github_se_lisent() {
        let r = json!({"assets":[
            {"name":"a.zip","browser_download_url":"https://x/a.zip","digest":"sha256:ABCD"},
            {"name":"b.zip","browser_download_url":"https://x/b.zip","digest":null},
            {"name":"sans-url"}
        ]});
        let a = parse_assets(&r);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].sha256.as_deref(), Some("abcd"));
        assert_eq!(a[1].sha256, None);
    }

    #[test]
    fn un_fichier_sans_empreinte_n_est_pas_telecharge() {
        let a = Asset {
            name: "x.zip".into(),
            url: "http://127.0.0.1:1/x.zip".into(),
            sha256: None,
        };
        let e = download(&a, &std::env::temp_dir().join("jamais-ecrit.zip")).unwrap_err();
        assert!(e.contains("refusé"), "{e}");
    }

    #[test]
    fn le_serveur_se_trouve_dans_un_sous_dossier() {
        let d = std::env::temp_dir().join(format!("npu-test-{}", std::process::id()));
        let bin = d.join("llama-server").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        assert!(find_server(&d).is_none());
        std::fs::write(bin.join("llama-server.exe"), b"x").unwrap();
        assert_eq!(find_server(&d), Some(bin.join("llama-server.exe")));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn un_chemin_qui_sort_du_dossier_est_refuse() {
        use std::io::Write as _;
        let d = std::env::temp_dir().join(format!("npu-zip-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let archive = d.join("piege.zip");
        {
            let mut z = zip::ZipWriter::new(File::create(&archive).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("../evade.txt", opts).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        let e = extract(&archive, &d.join("sortie")).unwrap_err();
        assert!(e.contains("dangereux"), "{e}");
        assert!(!d.join("evade.txt").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
