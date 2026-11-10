//! Le relais HTTP entre l'hôte et Foundry Local.
//!
//! L'hôte attend le moteur sur le port déclaré dans `morph.json` ; Foundry
//! choisit le sien à chaque démarrage. Le relais écoute sur 127.0.0.1:<port>,
//! répond lui-même à `/health` et `/v1/models`, remplace le champ `model` des
//! requêtes par le modèle chargé, et transmet le reste tel quel, flux SSE
//! compris.

use crate::foundry::Amont;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Taille maximale d'un corps de requête (une conversation longue, des images).
const CORPS_MAX: usize = 64 * 1024 * 1024;

/// Ce qui relance Foundry quand son service a disparu (arrêt après inactivité,
/// redémarrage de Windows Update…). Rend le nouvel amont.
pub type Relance = Box<dyn Fn() -> Result<Amont, String> + Send + Sync>;

/// Charge sur Foundry le modèle demandé par son nom ; rend le nouvel amont.
pub type Changement = Box<dyn Fn(&str) -> Result<Amont, String> + Send + Sync>;

/// L'état partagé du relais.
pub struct Relais {
    amont: Mutex<Option<Amont>>,
    relance: Relance,
    /// Mode passerelle : les modèles servis (déjà téléchargés), et comment
    /// passer de l'un à l'autre. Vide : un seul modèle, celui chargé.
    modeles: Mutex<Vec<String>>,
    changement: Option<Changement>,
}

impl Relais {
    pub fn new(relance: Relance) -> Arc<Self> {
        Arc::new(Self {
            amont: Mutex::new(None),
            relance,
            modeles: Mutex::new(Vec::new()),
            changement: None,
        })
    }

    /// Un relais qui sert plusieurs modèles et charge celui que la requête
    /// nomme.
    pub fn passerelle(relance: Relance, changement: Changement) -> Arc<Self> {
        Arc::new(Self {
            amont: Mutex::new(None),
            relance,
            modeles: Mutex::new(Vec::new()),
            changement: Some(changement),
        })
    }

    pub fn definir_modeles(&self, modeles: Vec<String>) {
        if let Ok(mut m) = self.modeles.lock() {
            *m = modeles;
        }
    }

    fn modeles(&self) -> Vec<String> {
        self.modeles.lock().map(|m| m.clone()).unwrap_or_default()
    }

    /// L'amont qui sert `demande` : le courant, ou celui où Foundry vient de
    /// charger ce modèle quand il fait partie des modèles servis.
    fn amont_pour(&self, demande: Option<&str>) -> Result<Option<Amont>, String> {
        let courant = self.courant();
        let (Some(changer), Some(d)) = (self.changement.as_ref(), demande) else {
            return Ok(courant);
        };
        if courant
            .as_ref()
            .is_some_and(|a| a.modele.eq_ignore_ascii_case(d))
        {
            return Ok(courant);
        }
        if !self.modeles().iter().any(|m| m.eq_ignore_ascii_case(d)) {
            return Ok(courant);
        }
        let mut garde = self
            .amont
            .lock()
            .map_err(|_| "état du relais corrompu".to_string())?;
        let nouveau = changer(d)?;
        *garde = Some(nouveau.clone());
        Ok(Some(nouveau))
    }

    pub fn definir(&self, amont: Amont) {
        if let Ok(mut a) = self.amont.lock() {
            *a = Some(amont);
        }
    }

    fn courant(&self) -> Option<Amont> {
        self.amont.lock().ok().and_then(|a| a.clone())
    }

    /// Relance Foundry, une seule à la fois.
    fn relancer(&self) -> Result<Amont, String> {
        let mut garde = self
            .amont
            .lock()
            .map_err(|_| "état du relais corrompu".to_string())?;
        let nouveau = (self.relance)()?;
        *garde = Some(nouveau.clone());
        Ok(nouveau)
    }
}

/// Une requête lue sur la connexion.
#[derive(Debug, PartialEq, Eq)]
pub struct Requete {
    pub methode: String,
    pub chemin: String,
    pub type_contenu: Option<String>,
    pub corps: Vec<u8>,
}

/// Lit la ligne de requête, les en-têtes et le corps (`Content-Length`).
pub fn lire_requete<R: BufRead>(r: &mut R) -> Result<Requete, String> {
    let mut ligne = String::new();
    r.read_line(&mut ligne).map_err(|e| e.to_string())?;
    let mut parts = ligne.split_whitespace();
    let methode = parts.next().ok_or("requête vide")?.to_string();
    let chemin = parts.next().ok_or("chemin manquant")?.to_string();
    let mut longueur = 0usize;
    let mut type_contenu = None;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).map_err(|e| e.to_string())? == 0 || h.trim().is_empty() {
            break;
        }
        let Some((nom, valeur)) = h.split_once(':') else {
            continue;
        };
        let valeur = valeur.trim();
        match nom.trim().to_ascii_lowercase().as_str() {
            "content-length" => longueur = valeur.parse().map_err(|_| "Content-Length invalide")?,
            "content-type" => type_contenu = Some(valeur.to_string()),
            "transfer-encoding" if valeur.eq_ignore_ascii_case("chunked") => {
                return Err("corps fragmenté non pris en charge".into());
            }
            _ => {}
        }
    }
    if longueur > CORPS_MAX {
        return Err("corps trop gros".into());
    }
    let mut corps = vec![0u8; longueur];
    r.read_exact(&mut corps).map_err(|e| e.to_string())?;
    Ok(Requete {
        methode,
        chemin,
        type_contenu,
        corps,
    })
}

/// Remplace le champ `model` d'un corps JSON par le modèle chargé. Un corps
/// qui n'est pas du JSON passe tel quel.
pub fn reecrire_modele(corps: &[u8], modele: &str) -> Vec<u8> {
    match serde_json::from_slice::<Value>(corps) {
        Ok(Value::Object(mut o)) if o.contains_key("model") => {
            o.insert("model".into(), Value::String(modele.into()));
            serde_json::to_vec(&Value::Object(o)).unwrap_or_else(|_| corps.to_vec())
        }
        _ => corps.to_vec(),
    }
}

/// La liste `/v1/models` que voit l'hôte.
pub fn liste_modeles(modeles: &[String]) -> Value {
    json!({
        "object": "list",
        "data": modeles
            .iter()
            .map(|m| json!({ "id": m, "object": "model", "owned_by": "foundry-local" }))
            .collect::<Vec<_>>()
    })
}

/// Le champ `model` d'un corps JSON.
fn modele_demande(corps: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(corps)
        .ok()?
        .get("model")?
        .as_str()
        .map(str::to_string)
}

fn raison(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

fn ecrire_entete(s: &mut TcpStream, code: u16, type_contenu: &str) -> std::io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {code} {}\r\nContent-Type: {type_contenu}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        raison(code)
    )
}

fn repondre_json(s: &mut TcpStream, code: u16, v: &Value) -> std::io::Result<()> {
    ecrire_entete(s, code, "application/json")?;
    s.write_all(v.to_string().as_bytes())?;
    s.flush()
}

fn agent() -> ureq::Agent {
    // Pas de délai de lecture : une longue réponse en flux dure des minutes.
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .build()
}

/// Envoie la requête au service Foundry ; `Ok(None)` quand il ne répond plus.
fn envoyer(amont: &Amont, req: &Requete) -> Result<Option<ureq::Response>, String> {
    let url = format!("{}{}", amont.base, req.chemin);
    let mut r = agent().request(&req.methode, &url);
    if let Some(t) = &req.type_contenu {
        r = r.set("Content-Type", t);
    }
    let resultat = if req.corps.is_empty() {
        r.call()
    } else {
        r.send_bytes(&reecrire_modele(&req.corps, &amont.modele))
    };
    match resultat {
        Ok(rep) | Err(ureq::Error::Status(_, rep)) => Ok(Some(rep)),
        Err(ureq::Error::Transport(t)) if t.kind() == ureq::ErrorKind::ConnectionFailed => Ok(None),
        Err(e) => Err(format!("Foundry Local : {e}")),
    }
}

fn transmettre(relais: &Relais, req: &Requete, s: &mut TcpStream) -> std::io::Result<()> {
    let amont = match relais.amont_pour(modele_demande(&req.corps).as_deref()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[npu] {e}");
            return repondre_json(s, 502, &json!({"error": {"message": e}}));
        }
    };
    let Some(amont) = amont else {
        return repondre_json(
            s,
            503,
            &json!({"error": {"message": "modèle en cours de chargement"}}),
        );
    };
    let reponse = match envoyer(&amont, req) {
        Ok(Some(r)) => Ok(r),
        Ok(None) => {
            eprintln!("[npu] Foundry Local ne répond plus : redémarrage…");
            relais
                .relancer()
                .and_then(|a| envoyer(&a, req))
                .and_then(|r| r.ok_or_else(|| "Foundry Local ne redémarre pas".into()))
        }
        Err(e) => Err(e),
    };
    match reponse {
        Ok(r) => {
            let code = r.status();
            let t = r.content_type().to_string();
            ecrire_entete(s, code, &t)?;
            std::io::copy(&mut r.into_reader(), s)?;
            s.flush()
        }
        Err(e) => {
            eprintln!("[npu] {e}");
            repondre_json(s, 502, &json!({"error": {"message": e}}))
        }
    }
}

fn servir_connexion(relais: &Relais, mut s: TcpStream) -> std::io::Result<()> {
    let mut lecteur = BufReader::new(s.try_clone()?);
    let req = match lire_requete(&mut lecteur) {
        Ok(r) => r,
        Err(e) => return repondre_json(&mut s, 400, &json!({"error": {"message": e}})),
    };
    let chemin = req.chemin.split('?').next().unwrap_or("");
    match (req.methode.as_str(), chemin) {
        ("GET", "/health") => match relais.courant() {
            Some(_) => repondre_json(&mut s, 200, &json!({"status": "ok"})),
            None => repondre_json(&mut s, 503, &json!({"status": "loading"})),
        },
        ("GET", "/v1/models") => {
            let servis = relais.modeles();
            match relais.courant() {
                _ if !servis.is_empty() => repondre_json(&mut s, 200, &liste_modeles(&servis)),
                Some(a) => repondre_json(&mut s, 200, &liste_modeles(&[a.modele])),
                None => repondre_json(&mut s, 503, &json!({"object": "list", "data": []})),
            }
        }
        _ => transmettre(relais, &req, &mut s),
    }
}

/// Écoute sur 127.0.0.1:`port`, une tâche par connexion. Ne rend la main
/// qu'en cas d'erreur d'écoute.
pub fn ecouter(port: u16, relais: Arc<Relais>) -> Result<(), String> {
    let ecoute = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("port {port} indisponible : {e}"))?;
    for connexion in ecoute.incoming() {
        let s = match connexion {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[npu] connexion refusée : {e}");
                continue;
            }
        };
        let r = Arc::clone(&relais);
        std::thread::spawn(move || {
            if let Err(e) = servir_connexion(&r, s) {
                eprintln!("[npu] relais : {e}");
            }
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn une_requete_est_lue_avec_son_corps() {
        let brut = "POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"model\":\"m\"}";
        let r = lire_requete(&mut Cursor::new(brut)).unwrap();
        assert_eq!(r.methode, "POST");
        assert_eq!(r.chemin, "/v1/chat/completions");
        assert_eq!(r.type_contenu.as_deref(), Some("application/json"));
        assert_eq!(r.corps, b"{\"model\":\"m\"}");
    }

    #[test]
    fn un_corps_fragmente_est_refuse() {
        let brut = "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(lire_requete(&mut Cursor::new(brut)).is_err());
    }

    #[test]
    fn le_modele_demande_est_remplace_par_le_modele_charge() {
        let c = reecrire_modele(
            br#"{"model":"Qwen3-4B.gguf","stream":true}"#,
            "Phi-4-mini-instruct-qnn-npu:2",
        );
        let v: Value = serde_json::from_slice(&c).unwrap();
        assert_eq!(v["model"], "Phi-4-mini-instruct-qnn-npu:2");
        assert_eq!(v["stream"], true);
        assert_eq!(reecrire_modele(b"pas du json", "m"), b"pas du json");
        assert_eq!(reecrire_modele(br#"{"a":1}"#, "m"), br#"{"a":1}"#);
    }

    #[test]
    fn le_relais_repond_sante_et_modeles() {
        let relais = Relais::new(Box::new(|| Err("pas de relance en test".into())));
        let ecoute = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = ecoute.local_addr().unwrap().port();
        drop(ecoute);
        let r = Arc::clone(&relais);
        std::thread::spawn(move || ecouter(port, r));
        std::thread::sleep(Duration::from_millis(200));
        let base = format!("http://127.0.0.1:{port}");
        let attente = ureq::get(&format!("{base}/health")).call();
        assert!(matches!(attente, Err(ureq::Error::Status(503, _))));
        relais.definir(Amont {
            base: "http://127.0.0.1:1".into(),
            modele: "phi-npu".into(),
        });
        assert_eq!(
            ureq::get(&format!("{base}/health"))
                .call()
                .unwrap()
                .status(),
            200
        );
        let v: Value = ureq::get(&format!("{base}/v1/models"))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(v["data"][0]["id"], "phi-npu");
    }

    /// Un faux service Foundry : renvoie le corps reçu, en flux SSE.
    fn faux_foundry() -> String {
        let ecoute = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", ecoute.local_addr().unwrap());
        std::thread::spawn(move || {
            for c in ecoute.incoming().flatten() {
                let mut s = c;
                let req = lire_requete(&mut BufReader::new(s.try_clone().unwrap())).unwrap();
                ecrire_entete(&mut s, 200, "text/event-stream").unwrap();
                s.write_all(b"data: ").unwrap();
                s.write_all(&req.corps).unwrap();
                s.write_all(
                    b"

data: [DONE]

",
                )
                .unwrap();
            }
        });
        base
    }

    #[test]
    fn une_conversation_est_relayee_avec_le_modele_charge() {
        let relais = Relais::new(Box::new(|| Err("pas de relance en test".into())));
        relais.definir(Amont {
            base: faux_foundry(),
            modele: "phi-npu:2".into(),
        });
        let ecoute = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = ecoute.local_addr().unwrap().port();
        drop(ecoute);
        let r = Arc::clone(&relais);
        std::thread::spawn(move || ecouter(port, r));
        std::thread::sleep(Duration::from_millis(200));
        let rep = ureq::post(&format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .set("Content-Type", "application/json")
            .send_string(r#"{"model":"x.gguf","stream":true}"#)
            .unwrap();
        assert_eq!(rep.content_type(), "text/event-stream");
        let texte = rep.into_string().unwrap();
        assert!(texte.contains(r#""model":"phi-npu:2""#), "{texte}");
        assert!(texte.contains("[DONE]"));
    }
}
