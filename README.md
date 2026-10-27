# MySyncFiles Client

Client Linux de synchronisation vers plusieurs serveurs MySyncFiles. Chaque connexion possède son dossier local, son identité TPM, ses révisions et ses autorisations web. Le serveur et l’interface Atlas vivent dans [mysyncfiles-server](https://github.com/alexbertho/mysyncfiles-server).

Les données restent en clair sur le serveur : ce logiciel n’est ni chiffré de bout en bout ni une sauvegarde. Les versions locales concurrentes sont préservées dans `.mysync-conflicts/`.

## Installer

Télécharger et lire le script avant la première exécution :

```sh
curl -fsS --proto '=https' --max-redirs 0 https://raw.githubusercontent.com/alexbertho/mysyncfiles-client/main/deploy/install.sh -o install.sh
sh install.sh
```

Le script demande l’URL HTTPS du serveur. Il télécharge une release signée depuis ce dépôt GitHub, vérifie signature et SHA-256, puis vérifie le TPM. La clé publique du serveur est reçue directement de l’administrateur. L’appairage doit être approuvé avec `make pair` côté serveur après comparaison des empreintes.

Sur Debian 13 et Arch Linux, y compris CachyOS, l’installateur détecte les outils manquants et propose les paquets correspondants avant d’utiliser `sudo`. Si le certificat EK est absent du TPM AMD ou Intel, il propose de le récupérer auprès du fabricant après avoir expliqué l’envoi d’un identifiant du TPM. Le certificat est vérifié contre ce TPM, conservé dans `~/.config/mysync/ek.der` (ou sous `$XDG_CONFIG_HOME`) et transmis à l’appairage. Voir le [parcours de dépannage](docs/install-client.md).

Prérequis : Linux x86-64 ou AArch64, TPM 2.0 avec certificat EK constructeur vérifiable. Debian 13 et Arch Linux sont les distributions de validation. `MYSYNC_SERVER_URL`, `MYSYNC_PROFILE`, `MYSYNC_WEB_STATUS`, `MYSYNC_EK_CERT` et `MYSYNC_EK_CHAIN` configurent l’installateur. Sans terminal, `MYSYNC_SERVER_URL` est requis et aucun service n’est démarré automatiquement.

Une release signée 0.4 doit être publiée séparément pour installer cette version. Tant qu’aucune release marquée « latest » ne contient le manifeste de votre architecture et son binaire, le téléchargement renvoie HTTP 404 : l’administrateur du dépôt doit suivre la [procédure de publication](docs/releases.md). Le protocole exige également un serveur 0.4 ; les profils et appairages TPM existants peuvent être conservés.

## Plusieurs serveurs

```sh
mysync --profile maison setup --server https://home.example.org --dir "$HOME/Sync/maison"
mysync --profile travail setup --server https://work.example.org --dir "$HOME/Sync/travail"
mysync profiles
mysync --profile maison status
mysync daemon --all
```

Chaque dossier doit être distinct et non imbriqué. Les profils nommés sont enregistrés dans `$XDG_CONFIG_HOME/mysync/profiles/NOM.json` (ou `~/.config/mysync/profiles`). Le profil `default` désigne l’ancien `mysync/config.json`. Les états, journaux et appairages en attente restent à côté du profil, hors du miroir. `--config` permet un chemin explicite ; le service commun charge uniquement les profils enregistrés.

Le service utilisateur lance `mysync daemon --all`. Redémarrer le service après l’ajout d’un profil ou un changement d’autorisation. Un pont loopback unique sélectionne la connexion par origine HTTPS exacte ; les autorisations `web-files` et `web-status` s’appliquent au profil sélectionné. Deux profils web sur la même origine sont refusés.

## Sécurité des sessions

Le TPM et l’approbation administrative restent nécessaires. Le TPM signe une clé Ed25519 temporaire en mémoire pour une session d’au plus 15 minutes ; cette clé signe les requêtes de synchronisation. Elle n’est jamais sauvegardée sur disque. Une capture de la mémoire peut permettre son usage jusqu’à expiration ou révocation. Les autorisations web et l’ouverture d’une nouvelle session exigent toujours une preuve TPM fraîche. Le jeton seul ne permet pas d’accéder aux fichiers.

Les réponses du serveur et les fichiers sont authentifiés contre sa clé épinglée, indépendamment du proxy HTTPS. Les téléchargements de releases acceptent au plus cinq redirections HTTPS et refusent HTTP ; les requêtes de synchronisation refusent toutes les redirections.

## Développer

```sh
make test             # sans pilote ni simulateur TPM
./deploy/install-tpm-deps.sh
make build            # client opérationnel avec TPM
MYSYNC_PROFILE=maison make install  # active le service utilisateur
```

`make test` comprend l’installateur, les profils, les transferts HTTP simulés, les mises à jour signées et les vérifications du miroir. Le mode `--no-default-features` sert aux tests et refuse les opérations TPM. Les builds habituels et `make build` activent le pilote matériel.

Le paquet `mysync-protocol` vient d’une révision Git précise du dépôt serveur. Avant publication, exécuter aussi `make test-integration` dans ce dépôt, avec le client voisin à la révision indiquée dans `integration/client-revision`, puis les scénarios Debian 13 et Arch. L’historique de ce dépôt a été extrait des fichiers clients de MySyncFiles ; la licence GPL-3.0-only et les auteurs sont conservés.

Voir [la publication signée](docs/releases.md) et [la documentation serveur](https://github.com/alexbertho/mysyncfiles-server/tree/main/docs).
