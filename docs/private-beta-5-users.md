# ZEMO — protocole bêta privée (5 utilisateurs)

## Objectif

Valider que cinq personnes non techniques peuvent utiliser ZEMO seules, du premier lancement jusqu’au rangement et à la recherche, sans perdre confiance ni avoir besoin d’explications techniques.

Cette bêta ne sert pas à ajouter des fonctionnalités. Elle sert à trouver les blocages réels avant d’élargir l’accès.

## Profil des 5 testeurs

Choisir si possible des profils différents :

- 2 personnes peu à l’aise avec l’informatique ;
- 2 personnes avec beaucoup de fichiers personnels/professionnels ;
- 1 personne plus technique capable de décrire précisément les erreurs.

Éviter de commencer avec des machines contenant des données irremplaçables sans sauvegarde. Le premier test doit rester prudent et observable.

## Parcours à faire sans assistance

1. Installer et lancer ZEMO.
2. Comprendre en moins d’une minute ce que ZEMO va faire et ce qu’il ne va pas faire.
3. Choisir le périmètre à analyser ou utiliser le parcours de rangement proposé.
4. Laisser ZEMO analyser les fichiers.
5. Comprendre l’aperçu avant toute modification réelle.
6. Valider un rangement prudent.
7. Vérifier que les fichiers sont retrouvables et cohérents après rangement.
8. Tester Annuler/Undo sur une opération réelle mais limitée.
9. Faire trois recherches naturelles sans connaître le nom exact du fichier.
10. Fermer puis relancer ZEMO et vérifier que l’état utile est restauré.

Le testeur doit faire ce parcours seul. L’observateur note les hésitations mais n’explique pas l’interface sauf blocage complet.

## Ce qu’il faut mesurer

Pour chaque testeur, noter :

- installation réussie : oui/non ;
- onboarding compris sans aide : oui/non ;
- premier rangement terminé : oui/non ;
- nombre d’erreurs ou blocages ;
- nombre de classements manifestement faux ;
- nombre d’éléments envoyés à « À vérifier » ;
- confiance avant le rangement (1–5) ;
- confiance après le rangement (1–5) ;
- temps jusqu’au premier résultat utile ;
- recherches réussies sur 3 ;
- Undo compris et réussi : oui/non ;
- volonté de réutiliser ZEMO la semaine suivante : oui/non.

## Retour en cas de bug

Depuis l’accueil, ouvrir **Support bêta** puis copier le diagnostic local. Ce diagnostic doit rester limité à la version de ZEMO, à l’état de sécurité de l’application et à des compteurs de parcours. Il ne doit contenir aucun nom de fichier, chemin, contenu extrait, identité, client ou requête de recherche.

Avec le diagnostic, demander seulement :

1. ce que la personne voulait faire ;
2. ce qui s’est passé à la place ;
3. une capture d’écran si elle aide à comprendre.

Ne jamais demander au testeur d’envoyer ses documents pour diagnostiquer un bug d’interface ou de parcours.

## Critères avant de passer de 5 à ~100 testeurs

Ne pas élargir la bêta tant que l’un de ces points est fréquent :

- installation ou premier lancement bloquant ;
- utilisateur incapable de comprendre si ZEMO va déplacer des fichiers ;
- erreur de rangement grave ou difficile à annuler ;
- Undo peu fiable ;
- crash ou blocage reproductible sur un parcours principal ;
- recherche naturelle jugée inutile par la majorité ;
- besoin régulier d’une explication humaine pour terminer le parcours principal.

Une fois les problèmes critiques corrigés, refaire au moins un passage complet avec un testeur non technique avant d’ouvrir davantage.
