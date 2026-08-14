# ADR-0006 — Apply et rollback journalisés

- Statut : **accepté pour le MVP**
- Date : 2026-08-09

## Contexte

Un plan peut devenir périmé après son affichage et un crash peut survenir entre
un renommage NTFS et sa confirmation en base. Un rollback aveugle risquerait
alors de déplacer le mauvais fichier ou d’écraser une création récente.

## Décision

Chaque action d’un plan devient une opération immuable contenant l’identité et la
version attendues, la source, la destination, le volume, la raison et ses
dépendances. Les lots ne sont pas prétendus atomiques : chaque fichier possède
son propre journal durable.

La machine d’état minimale est :

`PLANNED → PREPARED → APPLIED → VERIFIED`

avec les sorties explicites `ABORTED`, `FAILED` et `RECOVERY_REQUIRED`. Avant
l’appel système, le writer persiste `PREPARED` et ses préconditions avec un
niveau de synchronisation durable. Après le renommage NTFS sans remplacement, il
enregistre l’identité réellement observée à destination puis `APPLIED` et
`VERIFIED`.

Le MVP accepte seulement un renommage/déplacement sur un même volume NTFS local,
vers une destination absente dont le parent existe déjà. Il n’exécute ni delete,
ni overwrite, ni copie inter-volume, ni cycle nécessitant un emplacement
temporaire.

Un rollback est une nouvelle opération inverse, elle aussi journalisée
(`ROLLBACK_PREPARED → ROLLED_BACK`). Elle est autorisée seulement si :

- l’opération initiale est confirmée comme appliquée ;
- la destination contient encore la même identité et la version attendue ;
- la source d’origine est absente ;
- aucune opération ultérieure n’en dépend ;
- les contraintes NTFS local et intra-volume restent vraies.

Sinon, le journal passe en `RECOVERY_REQUIRED` et le cas devient logiquement
`TO_REVIEW`. Aucun rollback n’est forcé.

## Reprise après interruption

Pour un état `PREPARED`, la reprise observe sans muter :

- identité attendue à la source seulement : l’action n’a pas eu lieu, elle peut
  être marquée `ABORTED` ;
- identité attendue à la destination seulement : le renommage a eu lieu, le
  journal peut être réconcilié vers `APPLIED`, puis vérifié ;
- présence aux deux emplacements, absence aux deux ou identité différente :
  `RECOVERY_REQUIRED`.

La même règle, avec source et destination inversées, s’applique à
`ROLLBACK_PREPARED`. Une reprise ne rejoue jamais une opération dont l’issue est
ambiguë.

## Invariants

- aucune mutation pendant analyse ou simulation ;
- préflight immédiatement avant chaque effet, pas seulement à la création du plan ;
- journal durable avant effet externe et résultat observé après effet ;
- jamais de suppression ni de remplacement d’une destination ;
- le succès partiel d’un lot est visible et récupérable ;
- toute divergence privilégie l’arrêt et la revue humaine.

## Conséquences

Le système peut expliquer chaque effet et converger après un crash sans supposer
le résultat d’un appel interrompu. L’utilisateur doit toutefois accepter qu’un
lot puisse être partiellement appliqué et qu’un rollback soit parfois refusé à
cause d’un changement externe légitime.

Cette stratégie dépend des garanties de renommage intra-volume NTFS et ne fournit
pas de transaction atomique couvrant plusieurs fichiers et SQLite.

## Limites et alternatives rejetées

- « best effort » sans journal préalable est rejeté ;
- sauvegarder puis écraser la destination est rejeté ;
- un rollback inconditionnel est rejeté ;
- création de parents, résolution de cycles, copies, contenus modifiés, volumes
  non NTFS et chemins synchronisés sont hors MVP.

## Critères de sortie

- des tests d’injection de panne couvrent chaque frontière d’état avant et après
  l’appel système et le commit SQLite ;
- une course créant la destination provoque un refus sans overwrite ;
- la reprise classe correctement les cas source seule, destination seule,
  les deux, aucune et identité divergente ;
- un rollback après modification externe est refusé et expliqué ;
- l’historique d’un lot partiel reste lisible et chaque action peut être
  réconciliée indépendamment.
