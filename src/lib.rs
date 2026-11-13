//! Lanceur du moteur NPU (morph-npu).
//!
//! Deux chemins : Microsoft Foundry Local (défaut, aucun réglage de sécurité)
//! et llama.cpp Hexagon (GGUF, mode test-signing). Pour llama.cpp :
//! Le NPU Hexagon et le GPU Adreno ne sont accessibles qu'avec le fork
//! Snapdragon de llama.cpp (Locaryn/llama-snapdragon). L'hôte ne sait pas
//! installer un binaire, il lance ce que l'extension lui donne : ce lanceur
//! télécharge le runtime épinglé au premier usage (empreinte vérifiée), vérifie
//! que la machine est un Snapdragon ARM64, puis démarre `llama-server`.

pub mod args;
pub mod backend;
pub mod diagnostic;
pub mod foundry;
pub mod proc;
pub mod proxy;
pub mod runtime;
pub mod settings;
