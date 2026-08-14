# Récupération et reprise — Phase 0

Statut : **normatif pour le MVP**. La récupération vise à établir ce qui s’est
réellement passé. Elle ne rejoue jamais une mutation ambiguë.

## Décision

Après un arrêt non propre, l’application démarre en mode récupération :

1. elle prend le verrou d’instance et désactive Apply/rollback ;
2. elle ouvre SQLCipher, vérifie le schéma et l’intégrité SQLite ;
3. elle lit le journal et recense les états non terminaux ;
4. elle observe le corpus en lecture seule via l’identité Windows ;
5. elle réconcilie uniquement les cas prouvés ;
6. elle laisse tout cas ambigu en `RECOVERY_REQUIRED` et `TO_REVIEW` logique.

Les écritures de réconciliation concernent le catalogue privé. Aucune étape
d’inspection ne crée, déplace, supprime ou écrase un objet du corpus.

## Reprise d’une opération

Pour une opération `PREPARED` :

- l’identité attendue existe seulement à la source : conclure non démarré /
  `ABORTED` selon l’état journalisé, **sans** exécuter le déplacement ;
- elle existe seulement à la destination prévue avec l’identité/empreinte
  attendues : conclure `APPLIED` / restauré selon le sens (forward ou rollback),
  puis revérifier ;
- elle existe aux deux emplacements, à aucun, ou une autre identité est
  observée : passer à récupération ambiguë / `RECOVERY_REQUIRED`.

Pour `ROLLBACK_PREPARED`, la même décision s’applique avec les emplacements
inversés. Les cas `FAILED`, ambigus et `RECOVERY_REQUIRED` ne sont jamais
retentés automatiquement en avant. La reprise avant (`forward resume`) n’est
**pas implémentée**.

Un lot partiel reste une suite d’opérations indépendantes : les succès restent
visibles, les actions non commencées restent non appliquées, et seules les
actions prouvées sont réconciliées.

## Milestone 8.1 — Coordination journal / exécuteur

Comportement actuel du coordinateur Rust et de l’exécuteur isolé :

- **`PREPARED` sans mutation prouvée** : au redémarrage, observation source
  seule → non démarré ; aucune mutation compensatoire.
- **Mutation effectuée mais accusé de réception perdu / réponse ambiguë** :
  l’exécuteur ou le canal peut laisser une unité en état ambigu. Le
  coordinateur force la récupération, compare source/destination réelles, et
  **n’autorise pas** un nouvel essai aveugle de la même unité.
- **Échec de commit DB / journal** : si l’intention durable ou la mise à jour
  d’événement ne peut pas être commitée, la mutation suivante est bloquée. Un
  flush journal impossible ne doit pas être suivi d’un effet filesystem.
- **Redémarrage de l’exécuteur** : une nouvelle session protocol-v2 est
  requise ; les nonces/séquences antérieurs ne sont pas rejoués. Les unités
  déjà journalisées restent sous récupération coordinateur, pas sous reprise
  automatique worker.
- **État ambigu** : source et destination présentes, absentes, ou identité
  inattendue → `RecoveryAmbiguous` / `RECOVERY_REQUIRED`, Apply globalement
  bloqué jusqu’à résolution humaine sûre (rollback conditionnel si prouvé).
- **Corruption de journal** : diagnostics authentifiés peuvent positionner
  `journal_locked` et désactiver Apply ; pas de purge/réparation automatique
  du corpus.
- **Rollback bloqué par modification externe** : préflight de rollback refuse
  l’overwrite si le fichier appliqué a changé, si le chemin d’origine est
  occupé, ou si un répertoire créé n’est plus vide → résultat partiel/bloqué
  honnête, sans destruction.

## Rollback conditionnel

Le bouton de rollback est disponible uniquement si un préflight actuel prouve :

- le même fichier et la version attendue sont à la destination ;
- la source d’origine est libre ;
- aucune opération plus récente ne dépend de ce déplacement ;
- les deux chemins sont sur le même volume NTFS local ;
- aucune destination ne sera remplacée.

Le rollback est journalisé avant le renommage inverse. Si une condition change
ou si l’appel échoue, il s’arrête en `RECOVERY_REQUIRED`. L’utilisateur reçoit
les identités, emplacements et raisons utiles à une résolution manuelle, sans
commande destructive proposée.

## Scénarios d’exploitation

### Worker interrompu

Ignorer sa sortie incomplète, libérer son budget et marquer l’élément à
réanalyser ou `TO_REVIEW`. Un retry reparcourt le fichier en lecture seule. Le
worker reste sans réseau.

### Writer ou application interrompus

Laisser SQLite/SQLCipher récupérer WAL et transactions, puis exécuter les
contrôles d’intégrité avant tout nouveau travail. Ne jamais supprimer un WAL ou
recréer la base comme première réponse.

### Base illisible, corrompue ou mauvaise clé

Bloquer Apply et préserver les artefacts. Ne jamais initialiser une base vide
par-dessus un fichier existant. Une restauration de sauvegarde est permise
seulement après validation de la clé et de l’intégrité ; ses journaux sont
considérés potentiellement périmés et doivent être réconciliés avec NTFS.

Si aucune restauration n’est possible, une réinitialisation explicitement
confirmée crée un nouveau catalogue puis réanalyse le corpus sans le muter.
L’ancien historique ne peut alors plus autoriser un rollback.

### Migration interrompue

Restaurer la sauvegarde SQLCipher créée avant migration ou reprendre la migration
transactionnelle selon sa version. Tant que le schéma n’est pas reconnu,
l’application reste en lecture seule et Apply est indisponible.

### Disque plein ou accès refusé

Arrêter les producteurs, conserver l’erreur et ne pas lancer de nettoyage du
corpus. La reprise exige l’espace minimal pour le journal et un test d’écriture
dans le stockage privé de l’application ; un Apply n’est jamais lancé si son
résultat ne pourra pas être journalisé.

### Clé SQLCipher perdue

Le catalogue est irrécupérable sans sauvegarde de clé prévue. Le corpus reste
intact. L’application propose seulement une réinitialisation locale explicite et
une nouvelle analyse non mutante, pas une récupération supposée des opérations.

## Sauvegardes

- sauvegarde cohérente via l’API SQLite, jamais par copie hasardeuse d’une base
  ouverte ;
- chiffrement conservé et clé jamais incluse en clair ;
- sauvegarde obligatoire avant migration de schéma ;
- rotation dans le stockage privé, séparée de toute opération sur le corpus ;
- test périodique de restauration sur un catalogue isolé.

Une sauvegarde restaure une connaissance passée, pas l’état actuel du système de
fichiers. Elle ne suffit donc jamais à autoriser Apply ou rollback.

## Invariants

- pas de mutation du corpus pendant diagnostic et réconciliation ;
- jamais de delete ou overwrite ;
- identité NTFS observée plutôt que chemin supposé ;
- journal durable avant tout nouvel effet ;
- rollback conditionnel seulement ;
- ambiguïté égale arrêt et revue humaine.

## Conséquences et limites

La reprise est déterministe et préserve les modifications externes, au prix de
blocages nécessitant parfois une décision humaine. Une opération multi-fichiers
n’est pas atomique. Si le journal et ses sauvegardes sont perdus, le produit peut
reconstruire le catalogue mais pas recréer une preuve fiable des anciens Apply.

Le MVP ne fournit aucune procédure automatique d’Apply ou de rollback sur volume
non NTFS, partage réseau, stockage cloud ou déplacement inter-volume.

## Critères de sortie

- injection de crash avant/après chaque commit et renommage, avec résultat
  déterministe au redémarrage ;
- tests de corruption, mauvaise clé, WAL présent, migration interrompue, disque
  plein et double instance ;
- restauration de sauvegarde testée sans autoriser un journal périmé ;
- lot partiel et rollback refusé expliqués clairement dans l’UI ;
- aucun scénario de récupération automatisé ne supprime, n’écrase ou ne déplace
  un fichier sans préflight et nouveau journal.
