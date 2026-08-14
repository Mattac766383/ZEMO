# Support des formats — Phase 0

Statut : **cible normative du MVP**. « Supporté » signifie qu’un worker peut
extraire des observations bornées ; cela n’autorise jamais une modification du
contenu.

## Décision

Tous les fichiers réguliers peuvent être catalogués par métadonnées. L’extraction
de contenu est allowlistée par format, isolée hors processus et sans réseau.
L’extension est un indice : elle doit être cohérente avec la signature et la
structure détectées. En cas de doute, l’application conserve les métadonnées et
classe l’objet logiquement `TO_REVIEW`.

### Niveau 1 — texte déterministe

Contenu extrait pour :

- texte brut et Markdown : `.txt`, `.md` ;
- données textuelles : `.csv`, `.json`, `.jsonl`, `.yaml`, `.yml`, `.toml`,
  `.xml` ;
- code source explicitement allowlisté par le produit.

Encodages MVP : UTF-8 valide et UTF-16 LE/BE avec BOM. Aucun encodage n’est
deviné silencieusement. Un NUL inattendu, un encodage invalide ou une taille hors
budget arrête l’extraction et produit `TO_REVIEW`.

### Niveau 2 — documents structurés isolés

- `.pdf` : métadonnées et couche texte seulement ;
- `.docx`, `.xlsx`, `.pptx` : propriétés de base et texte visible du package
  OOXML.

JavaScript PDF, pièces jointes, formulaires, liens externes, macros, objets OLE,
formules et ressources distantes ne sont jamais exécutés ni chargés. Les formats
macro-enabled (`.docm`, `.xlsm`, `.pptm`), chiffrés ou protégés par mot de passe
restent au niveau métadonnées et `TO_REVIEW`.

### Niveau 3 — métadonnées seulement

- images (`.jpg`, `.jpeg`, `.png`, `.webp`, `.gif`, `.tiff`) : dimensions et
  métadonnées sûres, sans OCR ;
- audio/vidéo : conteneur, durée et codecs lorsque le parser isolé le permet,
  sans transcription ;
- archives générales : type et taille seulement, sans extraction ni récursion ;
- anciens formats Office binaires, exécutables, bibliothèques, installateurs,
  raccourcis et fichiers système : aucune interprétation de contenu.

Un type inconnu, corrompu, polyglotte, incohérent avec son extension ou contenant
une structure refusée est `TO_REVIEW`. Ce statut est une vue du catalogue, pas un
dossier créé sur disque.

## Budgets initiaux

Les dépassements arrêtent proprement le worker et ne dégradent jamais vers un
parser moins sûr :

- texte brut : 16 Mio d’entrée maximum ;
- PDF/OOXML : fichier de 64 Mio maximum ;
- package OOXML : 10 000 entrées, 128 Mio décompressés et ratio 100:1 maximum ;
- 10 secondes CPU/temps écoulé et 256 Mio de mémoire par fichier ;
- 2 Mio de sortie structurée maximum ;
- profondeur d’archive : une pour le package OOXML, zéro archive imbriquée.

Ces valeurs sont des plafonds MVP à valider sur le matériel Windows minimal. Les
budgets globaux de lot, l’espace libre et la file du writer sont également
bornés. Aucun dépassement ne déclenche un envoi cloud automatique.

## Contrat d’extraction

Le worker reçoit un handle ou flux limité en lecture, un type attendu et un
budget. Il ne reçoit ni secret, ni capacité d’écriture, ni réseau. Il renvoie un
schéma versionné contenant observations, avertissements, offsets et provenance.
Rust valide taille, encodage, bornes et type de chaque champ avant persistance.

Les chemins internes OOXML sont normalisés et toute traversée (`..`, chemin
absolu, périphérique Windows) invalide le package. Les URLs restent du texte.
Le crash, timeout ou résultat malformé du worker est traité comme un échec
réessayable ou `TO_REVIEW`, jamais comme un contenu vide fiable.

## Relation avec organisation et Apply

Une proposition cite les observations qui la motivent et son niveau de confiance.
Une extraction partielle ou absente ne peut pas être présentée comme complète.
Une IA locale peut aider sous capacité ; une IA cloud exige un consentement
ponctuel séparé.

Apply déplace le fichier entier sans réencoder ni écrire son contenu. Le format
n’assouplit jamais les préconditions : NTFS local intra-volume, identité
revérifiée, destination absente, journal préalable et rollback conditionnel.

## Invariants

- analyse strictement non mutante ;
- aucun parser n’exécute macro, script, binaire ou ressource distante ;
- tous les workers sont sans réseau ;
- aucune extraction illimitée ou récursive ;
- échec et ambiguïté deviennent `TO_REVIEW` logique ;
- aucune donnée extraite ne fait autorité pour l’identité du fichier.

## Conséquences

Le MVP couvre les usages courants tout en gardant une surface de parsing bornée.
Il n’offre ni aperçu parfait, ni OCR, ni transcription, ni compatibilité
universelle. Des fichiers légitimes seront envoyés en revue ; ce faux négatif est
préféré à l’exécution implicite ou à une classification trompeuse.

## Limites

- pas d’OCR, speech-to-text, archives générales, emails, formats CAO ou binaires
  Office historiques en Phase 0 ;
- pas de récupération de mot de passe ni de déchiffrement documentaire ;
- métadonnées EXIF et propriétés Office peuvent elles-mêmes être hostiles et
  restent soumises aux limites de sortie ;
- le simple déplacement d’un fichier ne garantit pas qu’une application tierce
  saura ensuite l’ouvrir.

## Critères de sortie

- corpus de fixtures valides, tronquées, chiffrées, polyglottes et malveillantes
  pour chaque famille ;
- fuzzing des parseurs PDF/OOXML et du schéma de sortie ;
- tests prouvant timeout, mémoire, taille, ratio et profondeur ;
- test réseau démontrant l’impossibilité pour chaque worker de charger une URL ;
- aucune macro, pièce jointe ou archive générale exécutée/extraite ;
- matrice visible dans l’UI avec niveau, limite et raison de `TO_REVIEW` ;
- budgets validés sur la configuration Windows minimale avant publication.
