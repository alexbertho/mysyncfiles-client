# Publier une release cliente signée

La publication est distincte des commits, de la création du dépôt et du déploiement serveur. Elle nécessite la clé privée de signature existante, conservée hors du dépôt, hors de GitHub et hors des assets publics.

1. Augmenter la version dans `Cargo.toml` et actualiser `Cargo.lock`.
2. Exécuter `make test`, les tests croisés du dépôt serveur et les contrôles TPM Debian 13/Arch.
3. Construire les binaires de production avec `make build`, séparément pour les architectures supportées.
4. Signer chaque binaire avec la clé correspondant à `src/update_public_key.hex` :

```sh
./target/release/mysync-release publish --help
./target/release/mysync-release publish --secret-key /chemin/prive/release.key --binary target/release/mysync --version 0.4.0 --target linux-x86_64 --output-dir /chemin/prive/release-staging
```

L’outil écrit `linux-x86_64/latest-linux-x86_64.signed.json` et `linux-x86_64/mysync-0.4.0-linux-x86_64`. Publier uniquement ces deux fichiers comme assets d’une release GitHub du dépôt client ; faire de même pour AArch64 lorsqu’un binaire testé est disponible. Le nom du manifeste inclut la cible pour éviter les collisions entre architectures. Les fichiers de suivi local et les clés ne sont pas des assets.

Le client lit `https://github.com/alexbertho/mysyncfiles-client/releases/latest/download/latest-CIBLE.signed.json`, vérifie la signature et télécharge le nom exact inscrit au manifeste. Ne rendre la release « latest » qu’une fois tous ses assets présents et vérifiés. Il n’existe aucun repli vers des métadonnées non signées ni vers un serveur de synchronisation.

Une version ne peut pas être réutilisée pour un binaire différent. Les signatures de release n’attestent pas la sûreté du code ; les contrôles fonctionnels et TPM doivent précéder la signature. `mysync update` prend un verrou commun avec l’installateur et vérifie le binaire avant remplacement atomique.
