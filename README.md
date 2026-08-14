# ZEMO

Application desktop Windows-first et local-first qui comprend les fichiers,
construit un index sémantique, propose une organisation révisable et ne modifie
le filesystem qu'après approbation d'un plan scellé.

Le prototype Electron historique est conservé par le tag
`prototype-electron-v0.1`. Le produit courant utilise Tauri 2, React,
TypeScript, Rust et SQLite/SQLCipher.

## Garanties actuelles

- Le scan n'obtient qu'une capacité de lecture et ne mute jamais le corpus.
- Le renderer n'accède ni aux chemins absolus, ni au SQL, ni au filesystem.
- L'index, les artifacts et le journal de récupération sont chiffrés.
- Aucun modèle ne dispose d'outil fichier ou shell.
- Le cloud exige un consentement ponctuel exactement lié à la tâche.
- Une destination existante n'est jamais remplacée.
- Aucun fichier n'est supprimé automatiquement.
- Les cas ambigus ou non calibrés restent `TO_REVIEW`.
- Apply est verrouillé dans le processus UI. L'exécuteur Windows isolé exige
  un plan authentifié et reste feature-gated.
- Le transfert inter-volume est codé comme protocole de coffre, mais reste
  verrouillé tant que la préservation ACL/ADS/EFS n'est pas auditée.

## Développement

Prérequis : Node.js 22+, Rust stable et les prérequis Tauri de la plateforme.

```bash
npm install
npm run typecheck
npm test
cargo test --workspace
npm run tauri -- dev
```

Pour vérifier les primitives Windows depuis une autre plateforme :

```bash
rustup target add x86_64-pc-windows-msvc
cargo check -p platform-windows -p operations -p operation-executor \
  --target x86_64-pc-windows-msvc
```

Préparation / qualification Windows (M15-A) — ne revendique pas un PASS natif
hors machine Windows réelle :

```bash
npm run windows:qualification:prep   # packaging + rapport NOT RUN
npm run windows:qualification        # runtime complet sur Windows uniquement
```

Voir [docs/qualification/windows.md](docs/qualification/windows.md).

## Architecture et sécurité

- [Architecture normative](docs/architecture/README.md)
- [Décisions d'architecture](docs/architecture/adr/)
- [Modèle de menace](docs/threat-model/README.md)
- [Traitement des données](docs/privacy/data-handling.md)
- [Récupération](docs/recovery/README.md)
- [Support des formats](docs/architecture/format-support.md)

Le tag historique ne doit pas être remis dans le chemin d'exécution du produit.
