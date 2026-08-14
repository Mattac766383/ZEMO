# Architecture — Phase 0

Statut : **normatif pour le MVP**. Ce document décrit la cible de la refondation
Tauri 2 / React / TypeScript / Rust / SQLite. Les ADR liés priment sur les
préférences d’implémentation locales.

## Décision d’ensemble

Supremacy est une application de bureau **Windows-first** et **local-first** :

- React/TypeScript présente l’état et émet des intentions ;
- Tauri 2 expose une surface IPC réduite et typée ;
- Rust est l’unique autorité pour les règles métier, les permissions, la
  persistance et les opérations sur le système de fichiers ;
- SQLite chiffré par SQLCipher constitue le catalogue local, alimenté par un
  seul writer ;
- les workers d’extraction et d’exécution sont isolés et **sans accès réseau** ;
- une IA locale ou distante ne reçoit que des capacités explicites. Tout envoi
  vers le cloud exige un consentement ponctuel et contextualisé.

## Invariants non négociables

1. **Analyser ne mute jamais le corpus** : ni création, ni renommage, ni
   déplacement, ni métadonnée ajoutée aux fichiers observés.
2. Aucune opération produit ne supprime un fichier ni n’écrase une destination.
   Un déplacement conserve le contenu et échoue si la destination existe.
3. `TO_REVIEW` est un état logique du catalogue et de l’interface, jamais un
   dossier créé dans le corpus.
4. Le plan affiché est immuable. L’application recalcule ses préconditions juste
   avant chaque action.
5. Chaque Apply est journalisé avant mutation. Un rollback est proposé seulement
   si ses préconditions sont encore vraies ; il n’est jamais forcé.
6. L’Apply du MVP est limité à un volume **NTFS local**, source et destination
   sur le même volume. Partages réseau, cloud sync, supports amovibles et
   déplacements inter-volumes sont refusés.
7. Aucun worker ne possède de capacité réseau. Le seul composant pouvant joindre
   un fournisseur IA est la passerelle IA, après décision de politique.

## Composants et responsabilités

- **UI React** : vues, accessibilité, collecte du consentement, rendu des plans
  et journaux. Elle ne construit jamais de chemins d’opération faisant autorité.
- **Adaptateur Tauri** : composition, conversion des DTO IPC et émission
  d’événements. Il ne contient pas de règle métier.
- **Application Rust** : cas d’usage, contrôle d’accès, annulation, quotas et
  orchestration des domaines.
- **Domain** : identités de fichiers, états d’analyse, plans, préconditions,
  capacités IA et machine d’état Apply/rollback.
- **Platform Windows** : ouverture sûre des fichiers, identité Windows, détection
  de volume et primitives de renommage NTFS.
- **Extraction / Search / Knowledge** : lecture bornée, extraction non fiable,
  index local et provenance.
- **Persistence** : SQLCipher, migrations et unique file de commandes du writer.
- **Simulation / Organizer** : propositions déterministes, conflits et
  classement logique `TO_REVIEW`.
- **Operations** : préflight, journal durable, Apply et rollback conditionnel.
- **AI gateway** : redaction, budget, consentement ponctuel et appels réseau.

Les dépendances vont des adaptateurs vers l’application puis le domaine. Le
domaine ne dépend ni de Tauri, ni de SQLite, ni d’un fournisseur IA.

## Flux de référence

### Analyse

1. Rust valide les racines autorisées et ouvre le corpus en lecture seule.
2. La plateforme collecte l’identité stable et les métadonnées minimales.
3. Les workers sans réseau extraient des résultats bornés ; leur sortie est
   considérée comme non fiable.
4. L’application valide puis transmet les commandes au writer SQLite unique.
5. La simulation produit un plan et des raisons. Les cas ambigus deviennent
   logiquement `TO_REVIEW`.

Ce flux n’appelle jamais le module Operations.

### Apply

1. L’utilisateur confirme un plan figé et lisible.
2. Rust vérifie NTFS local, intra-volume, identité, empreinte pertinente,
   destination absente et absence de conflit.
3. L’intention et les préconditions sont synchronisées dans le journal.
4. L’opération de renommage/déplacement est exécutée sans remplacement.
5. Le résultat réel est synchronisé, puis le catalogue est réconcilié.

Une interruption laisse un journal récupérable. L’absence de preuve d’état
entraîne un blocage et une revue humaine, jamais une nouvelle mutation supposée.

## Frontières de confiance

Sont non fiables : UI WebView, contenu des fichiers, parseurs, sorties de
modèles, réponses cloud et chemins fournis par l’utilisateur. Sont autorités :
les validations Rust, les handles ouverts par la plateforme, la politique de
capacités et le journal durable. Le catalogue est une vue reconstruisible ; il
ne prévaut pas sur l’identité constatée du fichier au moment d’un Apply.

## Limites de Phase 0

- cible fonctionnelle initiale : Windows 10/11 et volumes NTFS locaux ;
- pas d’Apply sur macOS, Linux, ReFS, FAT/exFAT, SMB, OneDrive Files On-Demand
  ou autre système dont les garanties n’ont pas été qualifiées ;
- pas de suppression, fusion de fichiers, écriture dans le contenu ni
  remplacement de destination ;
- SQLCipher protège les données au repos, pas un poste déjà déverrouillé et
  compromis ;
- l’analyse de formats complexes reste best-effort et doit être isolée.

## Critères de sortie de Phase 0

- les six ADR sont acceptés et leurs invariants ont des propriétaires de tests ;
- les contrats IPC ne permettent aucune mutation hors du cas d’usage Apply ;
- un prototype prouve l’identité Windows et le renommage sans remplacement sur
  NTFS local intra-volume ;
- le writer unique, les migrations SQLCipher et la récupération après crash sont
  testés ;
- le threat model et les règles de traitement des données sont revus ;
- une matrice de formats fixe les limites, budgets et comportements d’échec ;
- les scénarios Apply, crash, reprise et rollback conditionnel disposent de
  tests d’acceptation avant activation en production.

## ADR et documents associés

- [ADR-0001 — Tauri 2 plutôt qu’Electron](adr/0001-tauri-plutot-que-electron.md)
- [ADR-0002 — Rust comme autorité](adr/0002-rust-comme-autorite.md)
- [ADR-0003 — SQLCipher et writer unique](adr/0003-sqlcipher-et-writer-unique.md)
- [ADR-0004 — Identité de fichier Windows](adr/0004-identite-fichier-windows.md)
- [ADR-0005 — IA par capacités et consentement cloud](adr/0005-ia-capacites-et-consentement-cloud.md)
- [ADR-0006 — Apply et rollback journalisés](adr/0006-apply-et-rollback-journalises.md)
- [Modèle de menace](../threat-model/README.md)
- [Traitement des données](../privacy/data-handling.md)
- [Récupération](../recovery/README.md)
- [Support des formats](format-support.md)
