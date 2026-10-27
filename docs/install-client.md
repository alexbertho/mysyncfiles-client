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
