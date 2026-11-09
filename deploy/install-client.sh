#!/usr/bin/env bash
set -euo pipefail

project_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source_binary=${1:-"$project_dir/target/release/mysync"}
web_status=${MYSYNC_WEB_STATUS:-}
case "$web_status" in
    ''|true|false) ;;
    *) printf 'MYSYNC_WEB_STATUS must be true or false.\n' >&2; exit 1 ;;
esac
web_permissions=${MYSYNC_WEB_PERMISSIONS:-}
case "$web_permissions" in
    ''|true|false) ;;
    *) printf 'MYSYNC_WEB_PERMISSIONS must be true or false.\n' >&2; exit 1 ;;
esac
if [[ -n $web_status && -n $web_permissions ]]; then
    printf 'Choose either MYSYNC_WEB_PERMISSIONS or MYSYNC_WEB_STATUS.\n' >&2
    exit 1
fi

if [[ -z ${HOME:-} ]]; then
    printf 'HOME is required to install the client.\n' >&2
    exit 1
fi

config_dir="${XDG_CONFIG_HOME:-$HOME/.config}"
profile=${MYSYNC_PROFILE:-default}
case "$profile" in ''|*[!a-zA-Z0-9_-]*) printf 'Invalid MYSYNC_PROFILE.\n' >&2; exit 1 ;; esac
if [[ "$profile" == default ]]; then
    config_path="$config_dir/mysync/config.json"
else
    config_path="$config_dir/mysync/profiles/$profile.json"
fi
destination_dir="$HOME/.local/bin"
unit_dir="$config_dir/systemd/user"
account=$(id -un)

if [[ ! -f "$config_path" ]]; then
    printf 'Configure a synchronization folder before installing the background service: %s\n' "$config_path" >&2
    exit 1
fi
if [[ ! -f "$source_binary" || ! -x "$source_binary" ]]; then
    printf 'Client binary is missing or not executable: %s\n' "$source_binary" >&2
    exit 1
fi
if [[ -L "$source_binary" ]]; then
    printf 'Refusing symbolic-link source binary: %s\n' "$source_binary" >&2
    exit 1
fi
source_version=$("$source_binary" --version)
if [[ ! $source_version =~ ^mysync\ ([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
    printf 'Source binary reported an invalid version: %s\n' "$source_version" >&2
    exit 1
fi
source_major=${BASH_REMATCH[1]}
source_minor=${BASH_REMATCH[2]}
source_patch=${BASH_REMATCH[3]}

setup_help=$("$source_binary" setup --help)
if [[ $setup_help == *--web-permissions* && -z $web_status ]]; then
    if [[ -z $web_permissions && -t 1 && -r /dev/tty ]]; then
        saved_setting=$("$source_binary" --config "$config_path" web-permissions status)
        case "$saved_setting" in
            web_permissions_enabled=true) web_permissions=true; prompt='Y/n' ;;
            web_permissions_enabled=false) web_permissions=false; prompt='y/N' ;;
            web_permissions_enabled=custom) web_permissions=custom; prompt='y/n, Enter to keep saved permissions' ;;
            *) printf 'Cannot read the saved browser permissions.\n' >&2; exit 1 ;;
        esac
        while :; do
            printf 'Allow all browser permissions (status, files, uploads, rename/delete, editing and local code execution)? [%s] ' "$prompt" >/dev/tty
            IFS= read -r answer </dev/tty || { printf 'Cannot read the browser permissions choice.\n' >&2; exit 1; }
            case "$answer" in
                '') break ;;
                y|Y|yes|YES|o|O|oui|OUI) web_permissions=true; break ;;
                n|N|no|NO|non|NON) web_permissions=false; break ;;
                *) printf 'Please answer yes or no.\n' ;;
            esac
        done
    fi
elif [[ -n $web_permissions ]]; then
    printf 'The source client lacks --web-permissions; build a client with this option.\n' >&2
    exit 1
elif [[ $setup_help == *--web-status* ]]; then
    if [[ -z $web_status && -t 1 && -r /dev/tty ]]; then
        saved_setting=$("$source_binary" --config "$config_path" web-status status)
        case "$saved_setting" in
            web_status_enabled=true) web_status=true; prompt='Y/n' ;;
            web_status_enabled=false) web_status=false; prompt='y/N' ;;
            *) printf 'Cannot read the saved browser status setting.\n' >&2; exit 1 ;;
        esac
        while :; do
            printf 'Enable browser status on this PC? [%s] ' "$prompt" >/dev/tty
            IFS= read -r answer </dev/tty || { printf 'Cannot read the browser status choice.\n' >&2; exit 1; }
            case "$answer" in
                '') break ;;
                y|Y|yes|YES) web_status=true; break ;;
                n|N|no|NO) web_status=false; break ;;
                *) printf 'Please answer yes or no.\n' ;;
            esac
        done
    fi
elif [[ -n $web_status ]]; then
    printf 'The source client lacks --web-status; build MySyncFiles 0.3.8 or newer.\n' >&2
    exit 1
fi

for directory in "$destination_dir" "$unit_dir"; do
    if [[ -L "$directory" ]]; then
        printf 'Refusing symbolic-link installation directory: %s\n' "$directory" >&2
        exit 1
    fi
    install -d -m 755 "$directory"
done

command -v flock >/dev/null 2>&1 || { printf 'flock is required to serialize installation and updates.\n' >&2; exit 1; }
update_lock="$destination_dir/.mysync-update.lock"
[[ ! -L "$update_lock" ]] || { printf 'Refusing symbolic-link client update lock: %s\n' "$update_lock" >&2; exit 1; }
exec 9>"$update_lock"
chmod 600 "$update_lock"
flock -x 9
temporary_binary="$destination_dir/.mysync-install-$$"
trap 'if [[ -n ${temporary_binary:-} ]]; then rm -f -- "$temporary_binary"; fi' EXIT
install -m 755 "$source_binary" "$temporary_binary"
destination="$destination_dir/mysync"
if [[ -L "$destination" ]]; then
    printf 'Refusing symbolic-link client binary: %s\n' "$destination" >&2
    exit 1
fi
if [[ -e "$destination" ]]; then
    if [[ ! -f "$destination" || ! -x "$destination" ]]; then
        printf 'Existing client is not a regular executable: %s\n' "$destination" >&2
        exit 1
    fi
    if ! cmp -s "$source_binary" "$destination"; then
        installed_version=$("$destination" --version 2>/dev/null || true)
        if [[ $installed_version =~ ^mysync\ ([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
            installed_major=${BASH_REMATCH[1]}
            installed_minor=${BASH_REMATCH[2]}
            installed_patch=${BASH_REMATCH[3]}
            if (( installed_major > source_major ||
                  (installed_major == source_major && installed_minor > source_minor) ||
                  (installed_major == source_major && installed_minor == source_minor && installed_patch > source_patch) )); then
                printf 'Refusing to downgrade installed client %s to %s.\n' "$installed_version" "$source_version" >&2
                exit 1
            fi
        else
            printf 'Cannot compare the installed client version; existing client was left untouched.\n' >&2
            exit 1
        fi
        backup_dir=$(mktemp -d "$destination.backup.XXXXXXXX")
        ln "$destination" "$backup_dir/mysync"
        mv -f -T "$temporary_binary" "$destination"
        temporary_binary=''
        printf 'Installed %s; previous client retained at %s/mysync.\n' "$source_version" "$backup_dir"
    else
        rm -f "$temporary_binary"
        temporary_binary=''
        printf '%s is already installed.\n' "$source_version"
    fi
else
    mv -f -T "$temporary_binary" "$destination"
    temporary_binary=''
fi
unit_file="$unit_dir/mysync.service"
[[ ! -L "$unit_file" ]] || { printf 'Refusing symbolic-link service file: %s\n' "$unit_file" >&2; exit 1; }
install -m 644 "$project_dir/deploy/mysync.service" "$unit_file"

if [[ -n $web_permissions && $web_permissions != custom ]]; then
    if [[ $web_permissions == true ]]; then action=enable; else action=disable; fi
    "$destination" --config "$config_path" web-permissions "$action"
elif [[ -n $web_status ]]; then
    if [[ $web_status == true ]]; then action=enable; else action=disable; fi
    "$destination" --config "$config_path" web-status "$action"
fi

if [[ $(loginctl show-user "$account" --property=Linger --value) != yes ]]; then
    sudo loginctl enable-linger "$account"
fi
systemctl --user daemon-reload || {
    printf 'Could not reload user units. Retry: systemctl --user daemon-reload && systemctl --user restart mysync.service\n' >&2
    exit 1
}
systemctl --user enable mysync.service || {
    printf 'Could not enable the user service. Retry: systemctl --user enable mysync.service\n' >&2
    exit 1
}
systemctl --user restart mysync.service || {
    printf 'Could not restart the user service. Retry: systemctl --user restart mysync.service; inspect: systemctl --user status mysync.service\n' >&2
    exit 1
}
printf 'Installed %s; service enabled at boot for %s.\n' "$("$destination_dir/mysync" --version)" "$account"
