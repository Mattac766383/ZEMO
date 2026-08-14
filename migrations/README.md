# Migrations SQLite

`0001_initial.sql` crée le schéma local-first complet dans une transaction
`BEGIN IMMEDIATE` / `COMMIT`. Il requiert SQLite 3.37 ou plus récent, avec les
fonctions JSON et FTS5. Toutes les tables ordinaires sont `STRICT`; la table
virtuelle FTS5 et ses tables internes ne peuvent pas l’être.

## Source de vérité

Les enregistrements métier durables sont notamment :

- les principals, workspaces, volumes, roots, policies et scans ;
- les fichiers logiques, identités natives, localisations historisées,
  versions, contenus et digests ;
- le registre des processors/models, les jobs, événements, artifacts,
  extractions et leur provenance ;
- les taxonomies et les assertions de connaissance avec leur evidence ;
- les policies, revisions et décisions de revue d’organisation ;
- les plans d’opérations, approvals, executions, états filesystem et rollback ;
- les consentements cloud, disclosures, références de secrets et événements
  d’audit.

Une identité native ou une localisation « courante » est une ligne dont
`valid_to_scan_id IS NULL`. Des index uniques partiels garantissent qu’un
identifiant natif et un chemin courant ne désignent pas plusieurs fichiers.
Chaque clé étrangère possède un index couvrant côté enfant.

Les plans passent de `draft` à `sealed` en écrivant simultanément `plan_hash`
et `sealed_at`. Des triggers interdisent ensuite toute modification du plan,
de ses steps, preconditions et dependencies. Les approvals et executions sont
des enregistrements ultérieurs liés au hash scellé. Le journal d’exécution et
les événements d’audit sont append-only grâce à des triggers d’immutabilité.
Les `CHECK` des opérations n'autorisent que `create_directory`,
`remove_directory_if_empty`, `move` et `no_op`, toujours avec une stratégie de
collision `fail`. Le retrait d'un dossier ne peut viser qu'un dossier créé par
le plan et vide lors du rollback. `DELETE` de fichier, `OVERWRITE`, suffixage
automatique et fusion implicite sont impossibles dans le schéma initial.

## Projections et index dérivés

Les groupes de doublons, `search_documents`, `search_documents_fts`, les
embeddings et leurs vector generations sont des projections reconstruisibles.
Les classifications, entités, faits et relations sont des sorties persistées :
elles restent traçables vers jobs, artifacts, chunks/spans et evidence, mais
ne remplacent jamais les versions de fichiers comme source primaire.

`search_documents_fts` est un index FTS5 external-content adossé à
`search_documents`. Trois triggers maintiennent l’index après insertion,
modification ou retrait d’une projection. Pour le reconstruire :

```sql
INSERT INTO search_documents_fts(search_documents_fts) VALUES ('rebuild');
```

Le chemin produit Milestone 4 utilise la projection dédiée
`local_search_documents` et l’index `local_search_fts`. Le texte intégral reste
canonique dans `content_extraction_results` : une vue external-content l’expose
à FTS5 sans seconde copie métier. Les écritures de scan/extraction mettent la
projection à jour dans la même transaction et les suppressions du catalogue
retirent l’entrée FTS avant les cascades. `file_review_items` conserve les
raisons, états et tentatives de nouvelle extraction avec une unicité par
version de fichier et raison.

Milestone 5 ajoute des analyses sémantiques versionnées, leurs champs candidats,
entités et preuves, ainsi que des corrections utilisateur séparées des valeurs
machine. Une seule analyse réussie courante est désignée par version de fichier;
les anciennes analyses et les confirmations humaines sont conservées pour la
traçabilité et la réanalyse.

Milestone 6 ajoute les identités applicatives inter-fichiers, occurrences,
alias, rôles, signaux normalisés, candidats et preuves, contraintes de
séparation, décisions, historique de fusion, relations fichier/projet et
groupes de revue. Les décisions utilisateur restent séparées de l’état machine;
les fusions et séparations ne modifient que la base locale.

Milestone 9 ajoute `local_embedding_models`, `local_search_embeddings` et
`local_search_embedding_state` pour la recherche hybride. Les représentations
sont des vecteurs `int8` bornés, versionnés et remplaçables fichier par fichier;
les sources restent reliées à la version du fichier et à l’analyse sémantique.
Le fournisseur de hachage livré est explicitement réservé au développement,
non sémantique et sans téléchargement. Aucun enregistrement de modèle
n’autorise le réseau ou un fallback cloud.

Milestone 10 ajoute la restauration du dernier workspace, l’état de monitoring
`PRUDENT`, les réglages et exclusions par root, les jobs locaux durables et les
activités agrégées. `watch_registrations`, `watch_events` et
`watch_checkpoints` restent la source des hints bruts séquencés; un job
`monitoring_jobs` coalesce plusieurs hints pour un même chemin avant une
réconciliation. Les liens vers `scans` rendent le traitement redémarrable sans
transformer un hint de watcher en vérité catalogue.

Les chemins observés, exclusions, erreurs et résumés de monitoring restent
exclusivement dans la base SQLCipher locale. Cette couche n’ajoute ni réseau,
ni télémétrie, ni accès filesystem, ni capacité de mutation. Une activité est
écrite par batch, et non par fichier, afin de borner l’historique et les
notifications.

Milestone 11 ajoute `local_user_rules`, `local_learning_observations`,
`local_rule_suggestions` et `local_rule_file_matches`. Les règles sont des
instructions typées, ordonnées et explicitement gérées par l’utilisateur. Les
observations de correction restent locales et ne créent jamais une règle :
elles ne produisent qu’une suggestion après le seuil borné, puis une acceptation
explicite crée la règle dans la même transaction. Les correspondances
règle-fichier sont une projection reconstruisible utilisée pour un boost de
classement borné.

`0010_local_rules_learning.sql` complète aussi
`local_organization_preferences` avec les noms des racines Personal/Business,
un modèle de renommage sûr et un seuil de revue. La migration conserve le
schéma Phase 0 `organization_rules` inutilisé et suit la lignée active
`local_*`. Elle ne contient aucune opération filesystem, capacité réseau,
télémétrie ou donnée d’entraînement cloud.

Une vector generation est construite séparément puis marquée `active`; l’index
unique partiel garantit une seule génération active par workspace et usage.
Les vecteurs sont stockés en BLOB avec format et dimensions vérifiés.

## SQLCipher, WAL et connexions Rust

La migration ne contient volontairement ni clé, ni `journal_mode`. Le code
Rust doit, dans cet ordre :

1. ouvrir la connexion SQLCipher et fournir la clé avant toute lecture du
   schéma, sans la journaliser ;
2. appliquer les réglages SQLCipher retenus, par exemple
   `cipher_memory_security`, puis vérifier la clé et la compatibilité ;
3. activer `PRAGMA foreign_keys = ON` et un `busy_timeout` sur **chaque**
   connexion ;
4. configurer `PRAGMA journal_mode = WAL`, `synchronous` et la stratégie de
   checkpoint au niveau du pool/application ;
5. exécuter les migrations avec un seul writer, puis contrôler
   `foreign_key_check`.

Le choix de `synchronous = NORMAL` ou `FULL`, la fréquence des checkpoints et
la rotation de clé relèvent de la politique de durabilité Rust. Les fichiers
de base, WAL et SHM doivent conserver des permissions système restrictives.
Les valeurs secrètes restent dans le trousseau du système ; la table
`secret_references` ne stocke que leurs références.

Chaque migration enregistre sa version dans `schema_migrations`. Appliquée
directement après la version 8, `0009_continuous_monitoring.sql` positionne
`PRAGMA user_version = 9`; `0010_local_rules_learning.sql` positionne la version
10, avant le consentement d’exécution version 11 et la recherche hybride
version 12. La chaîne courante se termine avec
`0012_hybrid_semantic_search.sql` et `PRAGMA user_version = 12`. Les migrations
sont appliquées séquentiellement par le writer Rust, sous la connexion
sérialisée, puis validées avec `foreign_key_check`.
