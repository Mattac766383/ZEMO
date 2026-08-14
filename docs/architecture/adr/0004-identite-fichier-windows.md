# ADR-0004 — Identité de fichier Windows

- Statut : **accepté pour le MVP**
- Date : 2026-08-09

## Contexte

Un chemin Windows n’est pas une identité : il peut changer, varier par la casse,
pointer vers un reparse point ou être remplacé entre l’analyse et l’Apply.
Décider à partir d’une chaîne de chemin créerait des risques de confusion et de
TOCTOU.

## Décision

Sur un volume NTFS local, l’identité stable d’un objet est le couple :

`(VolumeSerialNumber, FILE_ID_128)`

Ces valeurs sont lues depuis un handle avec `GetFileInformationByHandleEx`
(`FileIdInfo`). Le chemin normalisé reste un localisateur et une donnée
d’affichage. Pour une précondition de mutation, l’identité est complétée par une
version observée : type, taille, horodatage de dernière écriture et, lorsque le
contenu a été lu, empreinte cryptographique.

Avant Apply et rollback, Rust rouvre l’objet, récupère son identité et compare la
version requise. Il vérifie aussi le type de système de fichiers, le volume, le
parent de destination et l’absence de destination. La primitive de renommage ne
demande jamais le remplacement d’un objet existant.

Les reparse points ne sont pas suivis pendant l’Apply du MVP. Les objets ayant
plusieurs liens physiques, une identité incohérente ou un nom ambigu sont classés
logiquement `TO_REVIEW`.

## Invariants

- une chaîne de chemin seule ne permet jamais une mutation ;
- source et destination sont sur le même volume NTFS local ;
- le contrôle d’identité est refait au plus près de l’appel système ;
- une destination existante provoque un refus, même si elle semble identique ;
- aucune analyse ne crée de marqueur, ADS, fichier sidecar ou dossier
  `TO_REVIEW` dans le corpus ;
- une ambiguïté de lien, reparse point ou concurrence externe bloque l’action.

## Conséquences

Les renommages externes peuvent être reconnus tant que l’identité subsiste et les
remplacements au même chemin sont détectés. L’implémentation devient spécifique
à Windows et nécessite des fixtures NTFS couvrant la casse, les noms longs, les
hard links et les reparse points.

Un identifiant de fichier peut être réutilisé après suppression par un autre
processus ; c’est pourquoi il n’est pas suffisant sans version observée. Une
empreinte complète renforce la preuve mais a un coût et ne supprime pas la
fenêtre de concurrence.

## Limites et alternatives rejetées

- le MVP n’accorde aucune garantie d’Apply sur ReFS, FAT/exFAT, SMB, supports
  amovibles ou fournisseurs de fichiers cloud ;
- le suivi automatique des reparse points et hard links est reporté ;
- chemin canonique, taille + date ou empreinte seuls sont rejetés comme identité
  primaire ;
- l’USN Journal peut aider à détecter des changements, mais n’est pas une source
  d’autorisation et n’est pas requis pour le MVP.

## Critères de sortie

- des tests Windows démontrent stabilité après renommage et détection d’un
  remplacement au même chemin ;
- hard links, junctions, symlinks, changement de casse, noms réservés et chemins
  longs ont un comportement documenté et testé ;
- un changement entre simulation et préflight produit un refus sans mutation ;
- tout volume non local, non NTFS ou inter-volume est refusé avant journal
  `PREPARED` ;
- les appels de renommage sont vérifiés sans option de remplacement.
