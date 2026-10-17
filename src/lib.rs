//! Lanceur du moteur NPU (morph-npu).
//!
//! Le NPU Hexagon et le GPU Adreno ne sont accessibles qu'avec le fork
//! Snapdragon de llama.cpp (Locaryn/llama-snapdragon). L'hôte ne sait pas
//! installer un binaire, il lance ce que l'extension lui donne : ce lanceur
//! télécharge le runtime épinglé au premier usage (empreinte vérifiée), vérifie
//! que la machine est un Snapdragon ARM64, puis démarre `llama-server`.

pub mod args;
pub mod backend;
pub mod proc;
pub mod runtime;
pub mod settings;
