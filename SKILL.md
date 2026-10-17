---
name: morph-npu
description: Moteur d'inférence local sur NPU Hexagon et GPU Adreno (Snapdragon, Windows ARM64). Utiliser pour exécuter un modèle GGUF sur un PC Snapdragon avec le NPU.
---

# morph-npu

Lance `llama-server` avec le backend Hexagon (NPU) du fork Locaryn/llama-snapdragon.

- Fonctionne uniquement sur Windows ARM64 (Snapdragon). Ailleurs, le lanceur refuse de démarrer.
- Le NPU exige `activer-npu.ps1` (administrateur, une fois, puis redémarrage). Sans cela, le runtime se lance sans NPU.
- Le lanceur télécharge le runtime au premier usage et vérifie son empreinte SHA-256.
- Serveur lié à 127.0.0.1 uniquement.
