# Documentation cliente

Voir le [README](../README.md) pour l’installation et les profils, la [publication signée](releases.md), et le [guide complet](https://github.com/alexbertho/mysyncfiles-server/blob/main/docs/install-client.md).

## Dépendances et certificat TPM

Dans un terminal, l’installateur propose les paquets manquants sur Debian 13 et Arch Linux, y compris les distributions dérivées comme CachyOS : `curl`, Python 3, OpenSSL, `flock` (`util-linux`) et `systemctl` (`systemd`). Les bibliothèques TPM sont proposées si le binaire signé ne peut pas démarrer. Les outils `tpm2-tools` sont proposés lorsqu’une récupération du certificat EK est nécessaire. Le refus laisse l’installation arrêtée avec une consigne de reprise. Sans terminal, le script affiche la commande du gestionnaire de paquets à exécuter et conserve le certificat dans la commande d’appairage proposée.

Un message « TPM reachable, but no readable … EK certificate » signifie que le TPM répond, mais que le client ne trouve pas de certificat constructeur utilisable dans ses index NV standards. Certains fTPM AMD sont dans ce cas. L’installateur :

1. Vérifie la présence des outils et lit le fabricant avec `tpm2_getcap`.
2. Propose, pour AMD et Intel, une récupération auprès du service HTTPS du fabricant. La question explique qu’un identifiant dérivé de la clé EK publique sera transmis au fabricant.
3. Après accord, utilise `tpm2_createek` et `tpm2_getekcertificate`, d’abord avec RSA-2048 puis avec P-256 si nécessaire. Chaque commande a un délai maximal. La validation TLS reste activée.
4. Vérifie avec le client signé que le certificat DER correspond à ce TPM. L’appairage vérifiera ensuite sa chaîne contre les racines constructeur configurées sur le serveur.
5. Enregistre le certificat vérifié dans `$XDG_CONFIG_HOME/mysync/ek.der` ou `~/.config/mysync/ek.der`, avec les permissions `0600`, et continue l’installation et l’appairage. Les relances réutilisent ce certificat après vérification.

Ces opérations ne réinitialisent pas le TPM, ne modifient pas ses index NV et ne changent pas les autorités de confiance du serveur. Un certificat déjà conservé n’est remplacé qu’après vérification du nouveau. Une récupération refusée, interrompue ou ne correspondant pas au TPM ne déclenche pas l’installation du client ni le démarrage du service.

Pour fournir un certificat DER déjà obtenu auprès du fabricant, y compris sans terminal :

```sh
MYSYNC_EK_CERT="$HOME/.config/mysync/ek.der" sh install.sh
```

Pour les autres fabricants, le script indique comment identifier le TPM et fournir ce certificat. Si AMD ou Intel ne renvoie aucun certificat correspondant, vérifier la connectivité vers le service indiqué et les informations de prise en charge du firmware auprès du constructeur du PC. Ne pas publier le certificat ou l’identifiant EK dans un ticket public.

## Server ID et reprise d'appairage

Le Server ID demandé par le client est la clé publique du serveur. Sur le serveur mis à jour, `make get` affiche cette clé, les appareils et le stockage sans créer d'invitation. La transmettre au client directement par un canal fiable. Si `make pair` est déjà ouvert pour lire cette clé, le conserver : le code affiché ensuite par le client se saisit dans ce même terminal, puis l'administrateur attend la preuve TPM et confirme l'empreinte.

Une reprise utilise le même nom et le même code. Si un nouveau code est utilisé pour un nom ayant une invitation en attente, le serveur mis à jour propose de remplacer cette invitation après confirmation ; l'ancien code est alors annulé. Un appareil déjà approuvé reste protégé : reprendre son profil existant ou choisir un autre nom pour un nouveau client.

## Refus HTTP 401 pendant l’appairage

Le nom demandé par `make pair` est un libellé choisi par l’administrateur pour reconnaître le PC, par exemple `pc-bureau`. Conserver le même nom et le même code pour reprendre une demande encore valide ; après expiration, relancer l’installateur et utiliser le code qu’il affiche.

Si `/v1/enroll/start` renvoie HTTP 401, consulter `make logs` côté serveur. Le contrôle `doctor` vérifie que le certificat correspond au TPM ; la chaîne constructeur n’est vérifiée par le serveur que pendant l’appairage. Un message serveur « untrusted or expired EK certificate » peut donc provenir d’un intermédiaire manquant même si le certificat est encore valide.

Le client complète automatiquement les intermédiaires AMD, notamment ceux de familles telles que `PRG-RPL`, et Intel, même sans chaîne NV embarquée. Il lit l’adresse CA Issuers du certificat, accepte seulement les dépôts constructeur attendus (`ftpm.amd.com/pki/aia/` et `tsci.intel.com/content/OnDieCA/certs/`), utilise HTTPS, limite la taille et le nombre de certificats et vérifie la signature de chaque lien. Il présente la chaîne au serveur sans modifier ses autorités de confiance. Une chaîne partielle sans adresse CA Issuers est transmise au serveur, qui décide si ses racines configurées permettent de la valider.

Avec un ancien client ou une chaîne constructeur obtenue séparément, fournir ses intermédiaires au format PEM :

```sh
MYSYNC_EK_CHAIN=/chemin/intermediaires.pem sh install.sh
```

Après réception de la preuve TPM, comparer l’empreinte complète affichée sur le PC et le serveur avant de confirmer l’approbation dans `make pair`.

## Compatibilité TPM et limites

Le fabricant du processeur n’est pas une preuve de compatibilité : un PC AMD ou Intel peut utiliser un fTPM/PTT ou un TPM discret d’un autre fabricant. Le client ne choisit pas son algorithme d’après le CPU. Le périphérique Linux `tpmrm` au numéro le plus bas est choisi pour les nouvelles identités, en conservant la préférence pour `/dev/tpmrm0`. Le transport est enregistré dans l’identité ; un échec d’accès n’entraîne jamais un basculement vers un autre TPM ou une clé logicielle.

Sur une machine avec plusieurs TPM, sélectionner explicitement le périphérique pendant le diagnostic et l’appairage manuel :

```sh
mysync doctor --tcti device:/dev/tpmrm1
mysync --profile bureau setup --tcti device:/dev/tpmrm1 \
    --server https://sync.example.org --dir "$HOME/Sync/bureau"
```

L’option existe aussi pour `enroll`. Le parcours automatique de récupération du certificat dans l’installateur utilise `/dev/tpmrm0` ; pour un autre périphérique, utiliser le parcours manuel avec un certificat constructeur correspondant. Le client doit être compilé avec le pilote TPM pour ces opérations.

| Fonction | Configuration prise en charge |
| --- | --- |
| Identité et signatures | Clé ECDSA P-256 non exportable, SHA-256 ; les paramètres sont interrogés avec `TPM2_TestParms` avant création. |
| Parent EK et certificat | RSA-2048 ou ECC P-256, avec SHA-256 PolicySecret et AES-128-CFB. Un échec RSA permet l’essai ECC, en conservant les deux diagnostics en cas d’échec. |
| Modèle EK constructeur | Modèle et nonce NV du profil TCG bas, lorsqu’ils existent. Les paramètres et la politique doivent correspondre au profil sécurisé pris en charge ; un modèle présent mais invalide n’est pas ignoré. |
| Lecture des certificats NV | Autorisation d’index vide préférée, puis autorisation propriétaire vide si l’index ne propose que celle-ci. La lecture est bornée et utilise la taille de tampon annoncée par le TPM. Aucun mot de passe de hiérarchie n’est modifié. |
| Chaîne constructeur | Chaîne NV TCG, découverte HTTPS AMD/Intel, ou intermédiaires PEM fournis par `--ek-chain`. La confiance reste exclusivement déterminée par les racines configurées sur le serveur. |
| TPM discret | Possible sur CPU AMD ou Intel avec les mêmes capacités. Si son fabricant n’a pas de dépôt automatique pris en charge, fournir son certificat et ses intermédiaires, et faire configurer sa racine par l’administrateur. |

Les profils EK RSA-3072/4096, ECC P-384/P-521 seuls, l’absence de P-256 pour l’identité, une hiérarchie endorsement protégée par un mot de passe inconnu, ou des modèles/politiques différents ne sont pas pris en charge par ce protocole. Un certificat absent chez le fabricant, expiré, non correspondant au TPM ou sans chaîne approuvée bloque l’appairage. Un TPM désactivé, verrouillé ou inaccessible demande une intervention sur les permissions ou le firmware ; le client ne le réinitialise pas.

Les index sont définis par le [profil EK TCG](https://trustedcomputinggroup.org/wp-content/uploads/TCG-EK-Credential-Profile-for-TPM-Family-2.0-Level-0-Version-2.6_pub.pdf). Le certificat doit correspondre à la clé EK recréée localement, puis la possession de cette clé est prouvée par ActivateCredential. Un certificat racine téléchargé par le client ne devient jamais une autorité de confiance du serveur.

## Compilation et tests de portabilité

Utiliser la version de Rust indiquée dans `rust-toolchain.toml`. Le mode `--build` du script de dépendances ajoute le compilateur C, `pkg-config` et les en-têtes OpenSSL/TSS sur Debian et ses dérivées, Arch, Fedora et openSUSE. Sans cette option, il installe les dépendances d’exécution. Après l’ajout à un groupe TPM, une nouvelle session utilisateur est nécessaire ; le script retourne un échec explicite pour empêcher un appairage prématuré.

```sh
./deploy/install-tpm-deps.sh --build
make test
make build
# Installer aussi swtpm et tpm2-tools avec le gestionnaire de paquets.
make test-tpm
```

Les tests de `tests/tpm_portability.rs` utilisent exclusivement des simulateurs éphémères : EK RSA et ECC avec modèle/nonce constructeur, identité rechargée et signature vérifiée, propriétaire protégé, lecture owner-only, certificat incorrect et modèle invalide. Les tests de chaînes AMD et Intel utilisent des certificats publics et des téléchargements simulés, avec refus des mauvais émetteurs. Ils ne démontrent pas une validation physique sur tous les TPM AMD ou Intel. Le certificat racine Intel de test provient de son [dépôt officiel](https://tsci.intel.com/content/OnDieCA/certs/OnDie_CA_RootCA_Certificate.cer) et n’est jamais ajouté aux racines de production.

Pour les tests d’intégration du dépôt serveur, placer le client à côté sous `mysyncfiles-client`, puis exécuter `MYSYNC_INTEGRATION_WORKTREE=1 make test-integration` pendant le développement coordonné. Ces tests utilisent notamment le port du pont navigateur : les exécuter dans un réseau isolé si un client réel l’occupe déjà, sans arrêter une synchronisation personnelle.
