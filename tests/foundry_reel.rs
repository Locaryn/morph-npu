//! Essai de bout en bout contre un vrai Foundry Local installé sur la machine.
//!
//! Ignoré par défaut (CI sans Foundry). À lancer à la main avec un modèle déjà
//! en cache, CPU accepté puisque le but est la mécanique, pas le NPU :
//!
//!   LOCARYN_TEST_FOUNDRY_MODELE=qwen3-0.6b-generic-cpu:4 cargo test --test foundry_reel -- --ignored

use locaryn_plugin_npu::foundry;
use locaryn_plugin_npu::proxy::{ecouter, Relais};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

#[test]
#[ignore = "exige Foundry Local et un modèle en cache"]
fn une_conversation_passe_par_le_relais_jusqu_a_foundry() {
    let demande =
        std::env::var("LOCARYN_TEST_FOUNDRY_MODELE").expect("LOCARYN_TEST_FOUNDRY_MODELE requis");
    let exe = foundry::trouver().expect("CLI foundry introuvable");
    let base = foundry::demarrer(&exe).expect("service Foundry");
    let catalogue = foundry::catalogue(&exe, &base).expect("catalogue");
    assert!(!catalogue.is_empty());
    let modele = catalogue
        .iter()
        .find(|m| m.id.eq_ignore_ascii_case(&demande))
        .unwrap_or_else(|| panic!("{demande} absent du catalogue"))
        .clone();
    let nom = foundry::charger(&exe, &base, &modele).expect("chargement");
    let charges = foundry::modeles_charges(&exe, &base).expect("modèles chargés");
    assert!(charges.contains(&nom), "{nom} absent de {charges:?}");

    let relais = Relais::new(Box::new(|| Err("pas de relance".into())));
    relais.definir(foundry::Amont { base, modele: nom });
    let ecoute = TcpListener::bind("127.0.0.1:0").expect("port libre");
    let port = ecoute.local_addr().expect("adresse").port();
    drop(ecoute);
    let r = Arc::clone(&relais);
    std::thread::spawn(move || ecouter(port, r));
    std::thread::sleep(Duration::from_millis(300));

    let corps = r#"{"model":"un-gguf-quelconque.gguf","stream":true,"max_tokens":16,
        "messages":[{"role":"user","content":"Dis bonjour."}]}"#;
    let rep = ureq::post(&format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .set("Content-Type", "application/json")
        .send_string(corps)
        .expect("réponse du relais");
    assert_eq!(rep.status(), 200);
    let texte = rep.into_string().expect("corps");
    assert!(texte.contains("data:"), "flux SSE attendu : {texte}");
    assert!(texte.contains("[DONE]"), "{texte}");
}
