# morph-npu

Moteur d'inférence Locaryn pour le **NPU Hexagon** et le GPU Adreno des processeurs Snapdragon (Windows ARM64).

Ce morph remplace l'ancien dépôt `llama-snapdragon` côté Locaryn : il télécharge au premier usage le runtime [Locaryn/llama-snapdragon](https://github.com/Locaryn/llama-snapdragon) (llama.cpp avec le backend Hexagon), vérifie son empreinte SHA-256, puis lance `llama-server` en local (127.0.0.1 uniquement).

## Prérequis

- Windows 11 ARM64 sur un processeur Snapdragon avec NPU Hexagon.
- Sur une autre machine, le lanceur refuse de démarrer et l'indique clairement.
- Pour utiliser réellement le NPU : lancer une fois `activer-npu.ps1` (fourni dans le runtime) **en administrateur**, puis redémarrer le PC. Le script active le mode test-signing de Windows : un filigrane « Mode de test » reste affiché.

## Commandes du lanceur

```
locaryn-npu-launch.exe status                         # backend, runtime installé, réglages (JSON)
locaryn-npu-launch.exe install                        # télécharge et vérifie le runtime
locaryn-npu-launch.exe serve --port 8189 --model M.gguf
```

Les réglages (`settings.json` dans le dossier de données) : `backend` (`auto` ou `npu`), `context_size`, `parallel`, `gpu_layers`, `thinking` (`off`, `on`, `auto`), `vision`, `runtime_tag`.

## Statut

Version `0.1.0-beta.1`. Le lanceur est testé (tests unitaires, refus sur machine non Snapdragon). **Le chargement effectif sur NPU n'a pas encore été vérifié sur matériel Snapdragon** : à valider avant de passer en stable.

## Développement

```
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

La release est produite par `.github/workflows/release.yml` à chaque tag `v*` : un zip `windows-aarch64` (lanceur, runtime Visual C++ ARM64, manifeste) et son `SHA256SUMS.txt`.

Licence : Apache-2.0.
