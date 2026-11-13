# morph-npu

Moteur d'inférence Locaryn pour le **NPU des PC Snapdragon** (Windows 11 ARM64, Snapdragon X Elite / X Plus).

Deux moteurs au choix, réglage **Moteur** :

| | **foundry** (par défaut, recommandé) | **llamacpp** (avancé) |
|---|---|---|
| Ce qui tourne | [Microsoft Foundry Local](https://learn.microsoft.com/azure/foundry-local/) | llama.cpp Snapdragon ([Locaryn/llama-snapdragon](https://github.com/Locaryn/llama-snapdragon)) |
| Accès au NPU | Pilote QNN **signé par Qualcomm**, distribué par Microsoft | Bibliothèques Hexagon signées par Locaryn |
| Sécurité de Windows | **Rien à changer** : ni mode test, ni Secure Boot | Mode test-signing obligatoire (filigrane « Mode de test », Secure Boot parfois coupé) |
| Modèles | **Catalogue Foundry uniquement** (phi-4-mini, qwen2.5-1.5b…) | Vos fichiers **GGUF** |

## Mode Foundry Local : pour un ami qui a un PC Snapdragon

1. Installer le morph NPU dans Locaryn.
2. Dans **Réglages → Moteur**, lancer le moteur **NPU Snapdragon**.
   - Au premier lancement, le morph installe Foundry Local par `winget` (paquet Microsoft `Microsoft.FoundryLocal`, sans droits administrateur le plus souvent), télécharge le fournisseur NPU de Qualcomm puis le modèle. **Comptez plusieurs minutes** : rien n'est bloqué, Locaryn attend jusqu'à 30 minutes.
   - Si Windows refuse l'installation automatique : réglages du morph → **Installer Foundry Local**. Une fenêtre s'ouvre ; acceptez la demande Windows si elle apparaît, attendez « Terminé », puis relancez le moteur.
3. Discuter normalement. Le modèle choisi dans la liste de Locaryn n'a pas d'effet dans ce mode : c'est le réglage **Modèle Foundry** qui compte.
4. Réglages du morph → **Vérifier** : la réponse doit être « Le NPU est utilisé : … tourne sur le NPU ».

### Choisir le modèle

- **Modèle Foundry** = `auto` (défaut) : un petit modèle NPU (qwen2.5-1.5b, sinon phi-4-mini, phi-3.5-mini, qwen2.5-7b, sinon le plus léger).
- **Lister les modèles NPU** affiche ce que le catalogue propose pour le NPU de ce PC. Copiez un nom (ex. `phi-4-mini`) dans **Modèle Foundry**, enregistrez, relancez le moteur.
- Un fichier GGUF **n'est pas accepté** dans ce mode. Pour un GGUF, passez **Moteur** à `llamacpp`.

### En cas de problème

- « aucun modèle NPU » : le pilote du NPU est trop ancien (30.0.140.0 minimum). Faire Windows Update, redémarrer.
- Journal du moteur : Locaryn → Réglages → Moteur → journal. Journal de Foundry : `foundry server logs`.
- Vérification à la main dans un terminal : `foundry server status`, `foundry model list --loaded`.

## Mode llama.cpp (avancé)

Réglage **Moteur** = `llamacpp`. Le morph télécharge au premier usage le runtime llama.cpp Snapdragon (empreinte SHA-256 vérifiée) et lance `llama-server` en local (127.0.0.1). Le NPU Hexagon n'est utilisable qu'après **Activer le mode test** (administrateur, une fois, puis redémarrage). Pour tout annuler : `scripts/desactiver-npu.ps1` en administrateur, puis redémarrer.

## Fonctionnement

En mode Foundry, `locaryn-npu-launch serve --port 8189` :

1. trouve la CLI `foundry` (ou l'installe par winget) ;
2. démarre le service (`foundry server start`) et lit son adresse dans `foundry server status` (le port change à chaque démarrage) ;
3. lit le catalogue (`foundry model list --variants -o json`), choisit la variante NPU, la télécharge et la charge (`foundry model download/load`) ;
4. sert sur `127.0.0.1:8189` un relais compatible OpenAI : `/health`, `/v1/models` (le modèle chargé), et tout le reste transmis à Foundry, flux SSE compris, avec le champ `model` remplacé par le modèle chargé. Si Foundry s'arrête, le relais le relance.

Commandes du lanceur :

```
locaryn-npu-launch.exe status          # moteur choisi, état (JSON)
locaryn-npu-launch.exe install         # installe Foundry Local ou le runtime llama.cpp
locaryn-npu-launch.exe serve --port 8189 [--model M.gguf]
```

Réglages : `backend` (`foundry` par défaut, ou `llamacpp`), `foundry_model` (`auto`, un alias ou un identifiant de variante) ; pour llama.cpp : `context_size`, `parallel`, `gpu_layers`, `thinking`, `vision`, `runtime_tag`.

Outils MCP (boutons des réglages) : `npu_etat` (Vérifier), `npu_modeles`, `npu_foundry_installer`, `npu_activer`.

## Compatibilité NPU : ce que disent les étiquettes

La source de vérité est le catalogue de Foundry (`foundry model list --variants -o json`, champ `device`). La passerelle publie dans `/v1/models`, pour chaque modèle : `device` (`npu`), `offload` (`full`), `engine`, `execution_provider` (`QNNExecutionProvider`), `aliases` (l'alias Foundry et le nom long de la variante, par exemple `DeepSeek-R1-Distill-Qwen-7B` pour `deepseek-r1-7b`) et, pour celui qui tourne, `loaded` et `runtime` (appareil réel).

- **NPU** : le modèle a une variante NPU au catalogue Foundry de ce PC ; il tourne entièrement sur le NPU.
- **NPU partiel** : un GGUF jusqu'à 14 milliards de paramètres (`accelerated_files` du manifeste). En mode llama.cpp Hexagon, une partie des calculs seulement passe sur le NPU, et il faut le mode test-signing. La limite de 14B est un choix prudent du morph : elle n'a pas été mesurée sur un vrai NPU.
- **Aucune étiquette** : pas de variante NPU, et trop gros ou taille inconnue.

Foundry ne montre que les variantes que ce PC peut faire tourner : sans NPU Snapdragon vu, le catalogue ne contient **aucune** variante NPU (cas du PC x64 de développement : 101 variantes, 0 NPU). La passerelle le dit dans son journal et le bouton « Vérifier » aussi.

### Ce qui tourne vraiment

Foundry 0.11 rend `foundry model list --loaded` vide, même pendant qu'il sert un modèle (vérifié le 13/11/2026). La passerelle retient donc la variante qu'elle vient de faire servir (le champ `model` de la réponse). L'appareil vient de la fiche de cette variante au catalogue. Elle préfère `--loaded` quand une version de Foundry le remplit. L'application affiche cet appareil dans la barre de conversation, et signale un repli (annoncé NPU, servi CPU).

### Vérifier

« Vérifier » répond, ligne par ligne :
- Foundry est-il installé (chemin, version) ?
- Windows voit-il un NPU (classe `ComputeAccelerator`) ?
- Combien de variantes NPU le catalogue contient-il ?
- Quel modèle est chargé, et sur quel appareil ?
- Où est le journal de la passerelle (`gateways/npu/gateway.log` dans le dossier de données de Locaryn) ?

### Gros modèles (Qwen3.8 27B…)

Le catalogue Foundry vu d'un PC x64 contient `qwen3.8-27b`, mais seulement en variante GPU CUDA (18 Go, type « Unknown », non listée comme modèle de conversation). Les variantes NPU (QNN) publiées par Foundry sont de petits modèles. Sans variante QNN, un 27B ne tournera pas sur le NPU d'un Snapdragon X. En GGUF par llama.cpp Hexagon, il dépasse la limite de 14B : il reste le CPU, lent.

## Statut

Version `0.1.0-beta.8`. La mécanique Foundry (CLI 0.11 : démarrage, catalogue, chargement, relais d'une conversation en flux, appareil réel rapporté) est vérifiée sur un PC x64, avec un modèle CPU en mode essai (`LOCARYN_NPU_APPAREIL_ESSAI=cpu`). **Le chargement sur un vrai NPU Snapdragon reste à confirmer** sur matériel.

## Développement

```
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
LOCARYN_TEST_FOUNDRY_MODELE=qwen3-0.6b-generic-cpu:4 cargo test --test foundry_reel -- --ignored
```

La release est produite par `.github/workflows/release.yml` à chaque tag `v*` : un zip `windows-aarch64` (lanceur, serveur MCP, scripts, runtime Visual C++ ARM64, manifeste) et son `SHA256SUMS.txt`.

Licence : Apache-2.0.
