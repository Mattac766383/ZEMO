# Guide testeur — bêta privée macOS

Application : **ZEMO** (organisation locale de fichiers)  
Version : **0.1.0** · distribution **0.1.0-beta.5** · Apple Silicon (arm64)  
Public : 5 à 10 testeurs contrôlés. Ce n’est pas une version publique.

**À distribuer : le fichier `ZEMO-0.1.0-beta.5-arm64.dmg`.**  
Les builds antérieures (Working Name / beta.1–beta.4) sont obsolètes. Ne pas les envoyer.

Vous avez la bonne version si l’en-tête affiche **ZEMO · Bêta privée macOS · 0.1.0-beta.5**
et si la navigation principale est : Accueil, Organisation, Recherche, Surveillance.

Si **Working Name** est encore dans Applications, retirez-la puis installez **ZEMO**
depuis le DMG beta.5.

## Ce que fait cette bêta

L’application analyse des dossiers **que vous choisissez**, comprend vos documents
localement, puis **propose** une organisation. Vous pouvez aussi rechercher
des fichiers et voir les nouveaux fichiers détectés.

Tout reste sur votre Mac. Rien n’est envoyé dans le cloud, sauf si **vous**
demandez explicitement l’installation du modèle de recherche sémantique
(~118 Mo, téléchargement unique depuis une source figée).

## Ce que cette bêta ne fait pas

- Elle ne déplace des fichiers **qu’après** votre confirmation explicite
  (« Appliquer l’organisation »). Rien n’est supprimé ni écrasé.
- Elle n’organise pas automatiquement votre disque.
- Elle n’est pas signée / notariée Apple (avertissement Gatekeeper attendu).
- Elle n’est pas compatible Intel (cette build est **Apple Silicon uniquement**).
- La reconnaissance optique (OCR) et certains PDF scannés nécessitent des
  outils optionnels **non fournis**. Sans eux, l’application reste utilisable.

**Apply + Undo :** après application, « Annuler les changements » replace
les fichiers à leur emplacement précédent lorsque cela peut être fait
sans écraser de modifications récentes.

## Avant de commencer

1. Utilisez un **Mac Apple Silicon** (M1 / M2 / M3 / M4 / M5…).
2. Préparez un **dossier de test** : une **copie** d’un dossier un peu
   désordonné, pas vos seules archives irremplaçables — surtout si vous
   choisissez des dossiers manuellement.
3. Gardez ce guide et le fichier `README-FIRST.txt` à portée de main.

## Installation

1. Ouvrez le fichier `.dmg` **ZEMO-0.1.0-beta.5-arm64.dmg**.
2. Glissez **ZEMO** vers le dossier **Applications**.
3. Ouvrez **Applications**, puis **ZEMO**.
4. Si macOS bloque l’ouverture :
   - Dans le Finder, allez dans **Applications**.
   - **Clic droit** (ou Contrôle-clic) sur **ZEMO**.
   - Choisissez **Ouvrir**, puis confirmez **Ouvrir**.
5. Faites-le une fois. Les lancements suivants devraient être normaux.

Ne désactivez pas Gatekeeper, et n’exécutez pas de commande `sudo` pour
contourner la sécurité de macOS.

## Premiers pas

Au premier lancement, l’écran d’accueil propose deux chemins :

- **Organiser mon ordinateur** — recommandé. L’application prépare une
  analyse des dossiers personnels utiles (Bureau, Documents,
  Téléchargements, Images ; Vidéos et Musique s’ils existent). Ce n’est
  **pas** tout le disque : les fichiers système, les applications et les
  données internes de l’app restent exclus.
- **Choisir des dossiers** — pour analyser uniquement un dossier que vous
  sélectionnez (par exemple votre copie de test).

Dans les deux cas :

- Vos fichiers restent sur votre Mac.
- macOS peut demander l’accès **uniquement** aux emplacements concernés.
  Vous pouvez refuser un dossier inaccessible : l’application continue
  avec les autres.
- Commencez par un petit dossier de test avant d’appliquer
  l’organisation à un dossier personnel.

Enchaînement typique : accueil → explication d’accès → choix des
emplacements → **Commencer l’analyse**.

## Examiner l’organisation proposée

1. Après l’analyse, vous arrivez sur l’**Accueil** (tableau de bord).
2. Ouvrez **Organisation**.
3. Cliquez **Préparer l’organisation** si aucune proposition n’existe encore.
4. Comparez **emplacement actuel** et **destination proposée**.
5. Les fichiers incertains vont dans **À revoir**. C’est normal.
6. Corrigez ou marquez un exemple clairement faux avant d’appliquer.
7. Quand la proposition convient, cliquez **Appliquer l’organisation**,
   relisez l’avertissement, puis confirmez **Appliquer**.
8. Vérifiez que les fichiers de test ont bien bougé, puis essayez
   **Annuler les changements** si vous voulez les replacer.

## Rechercher

1. Ouvrez **Recherche**, ou utilisez la recherche rapide sur l’accueil.
2. Essayez d’abord une recherche simple (nom ou mot du document).
3. La recherche « par le sens » (modèle local) est **optionnelle** :
   - Sans modèle : la recherche lexicale fonctionne.
   - Avec modèle : cliquez **Activer**, acceptez le téléchargement
     (~118 Mo), puis réessayez une phrase naturelle
     (« facture Point P du projet Martin »).

Vous n’avez **pas** besoin de variable d’environnement ni d’outil
développeur.

## Surveillance

1. Laissez l’application ouverte, ou rouvrez-la plus tard.
2. Ajoutez un nouveau fichier **dans un dossier déjà analysé**.
3. Ouvrez **Surveillance** : un nouveau fichier devrait être détecté et
   une **proposition** mise à jour.
4. Vérifiez que le fichier n’a **pas** été déplacé sur le disque.

## Signaler un problème

Utilisez `docs/beta/feedback-template.md` (ou la copie fournie avec le DMG).

Indiquez :

- version affichée en haut de l’application
  (`ZEMO · Bêta privée macOS · 0.1.0-beta.5`)
- modèle de Mac et version de macOS
- ce que vous faisiez
- un **extrait** du message d’erreur affiché à l’écran

N’envoyez **pas** votre base de données, ni des fichiers personnels, ni
des captures contenant des noms de documents sensibles.

## Désinstallation

Voir `README-FIRST.txt` : retirer l’application **ne supprime pas** vos
documents. La suppression des données de l’application est une étape
**séparée** et optionnelle.
