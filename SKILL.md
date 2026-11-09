---
name: morph-npu
description: Moteur d'inférence local sur le NPU des PC Snapdragon (Windows ARM64). Par défaut Microsoft Foundry Local (modèles du catalogue Foundry, aucun mode test) ; en option llama.cpp Hexagon pour les fichiers GGUF.
---

# morph-npu

- Fonctionne uniquement sur Windows ARM64 (Snapdragon). Ailleurs, le lanceur refuse de démarrer.
- Moteur par défaut (`backend` = `foundry`) : Microsoft Foundry Local, installé au premier usage par winget. Le NPU passe par le pilote QNN signé par Qualcomm : ni mode test, ni Secure Boot à couper. Modèles du catalogue Foundry seulement (réglage `foundry_model`, `auto` par défaut) ; un GGUF n'est pas accepté.
- `backend` = `llamacpp` : llama.cpp Hexagon pour les GGUF ; exige le mode test-signing (bouton « Activer le mode test », administrateur, puis redémarrage).
- Outils : `npu_etat` (le NPU est-il utilisé ?), `npu_modeles` (modèles NPU du catalogue), `npu_foundry_installer`, `npu_activer`.
- Serveur lié à 127.0.0.1 uniquement.
