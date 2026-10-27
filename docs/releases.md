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

## Première publication sur GitHub

Un dépôt contenant le code et un workflow de compilation ne possède pas automatiquement de release. Si l’installateur reçoit HTTP 404 pour `latest-linux-x86_64.signed.json`, vérifier qu’une release publique, hors brouillon et hors préversion, est marquée « latest » et contient cet asset ainsi que le binaire signé correspondant. Cette erreur ne concerne ni l’URL du serveur de synchronisation ni le TPM du client.

Après les validations et la signature ci-dessus, créer d’abord un brouillon et y joindre uniquement les deux assets de production :

```sh
gh release create v0.4.0 \
  /chemin/prive/release-staging/linux-x86_64/latest-linux-x86_64.signed.json \
  /chemin/prive/release-staging/linux-x86_64/mysync-0.4.0-linux-x86_64 \
  --repo alexbertho/mysyncfiles-client --target COMMIT_VALIDE \
  --title 'MySyncFiles Client 0.4.0' --notes 'Client Linux avec profils indépendants et sessions TPM.' \
  --draft
```

Remplacer `COMMIT_VALIDE` par le SHA du commit testé. Ajouter les deux assets AArch64 au brouillon si cette architecture a aussi été construite et validée. Vérifier les assets dans le brouillon, puis publier :

```sh
gh release edit v0.4.0 --repo alexbertho/mysyncfiles-client --draft=false --latest
curl -fLsS --proto '=https' --proto-redir '=https' --max-redirs 5 \
  https://github.com/alexbertho/mysyncfiles-client/releases/latest/download/latest-linux-x86_64.signed.json \
  -o /tmp/latest-linux-x86_64.signed.json
```

La publication nécessite la clé privée correspondant à la clé publique déjà intégrée au client et à l’installateur. Ne pas générer une nouvelle clé pour contourner le 404 : une signature produite par une autre clé serait refusée. Relancer ensuite l’installation depuis le poste client.

Une version ne peut pas être réutilisée pour un binaire différent. Les signatures de release n’attestent pas la sûreté du code ; les contrôles fonctionnels et TPM doivent précéder la signature. `mysync update` prend un verrou commun avec l’installateur et vérifie le binaire avant remplacement atomique.
