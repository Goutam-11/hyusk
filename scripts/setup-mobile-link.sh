#!/usr/bin/env bash
set -euo pipefail

enable=false
if [[ "${1:-}" == "--enable" ]]; then
    enable=true
    shift
fi

if ! command -v openssl >/dev/null 2>&1; then
    printf 'openssl is required to create the local TLS identity.\n' >&2
    exit 1
fi

data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
link_dir="$data_home/hyusk/link"
host="${1:-$(hostname -I 2>/dev/null | awk '{print $1}')}"
if [[ -z "$host" || "$host" == "127.0.0.1" ]]; then
    printf 'No laptop LAN address found. Connect both devices to a network or pass the laptop IP.\n' >&2
    exit 1
fi
cert="$link_dir/server.crt"
key="$link_dir/server.key"

mkdir -p "$link_dir"
chmod 700 "$link_dir"

if [[ -e "$cert" || -e "$key" ]]; then
    printf 'Hyusk link identity already exists:\n  %s\n  %s\n' "$cert" "$key"
else
    if [[ "$host" =~ ^[0-9a-fA-F:.]+$ ]]; then
        host_san="IP:$host"
    else
        host_san="DNS:$host"
    fi
    openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days 825 \
        -subj '/CN=Hyusk Laptop Link' \
        -addext "subjectAltName=$host_san,IP:127.0.0.1,DNS:localhost" \
        -keyout "$key" -out "$cert"
    chmod 600 "$key" "$cert"
fi

if [[ "$enable" == true ]]; then
    project_root="$(cd "$(dirname "$0")/.." && pwd)"
    if [[ ! -f "$project_root/Cargo.toml" ]]; then
        project_root="$(systemctl --user show hyusk.service -p WorkingDirectory --value)"
    fi
    if [[ ! -f "$project_root/Cargo.toml" ]]; then
        printf 'Could not locate the Hyusk project; run this script from its checkout.\n' >&2
        exit 1
    fi
    environment_file="$project_root/.env"
    backup="$project_root/.env.hyusk-link-backup-$(date +%Y%m%d-%H%M%S)"
    if [[ -f "$environment_file" ]]; then
        cp -- "$environment_file" "$backup"
    fi
    temp_file="$(mktemp "$project_root/.env.hyusk-link.XXXXXX")"
    trap 'rm -f "$temp_file"' EXIT
    if [[ -f "$environment_file" ]]; then
        awk '!/^HYUSK_LINK_(ENABLED|BIND|ADVERTISE_HOST|TLS_CERT|TLS_KEY)=/' "$environment_file" > "$temp_file"
    fi
    printf '\nHYUSK_LINK_ENABLED=1\nHYUSK_LINK_BIND=0.0.0.0:4488\nHYUSK_LINK_ADVERTISE_HOST=%s\nHYUSK_LINK_TLS_CERT=%s\nHYUSK_LINK_TLS_KEY=%s\n' \
        "$host" "$cert" "$key" >> "$temp_file"
    chmod 600 "$temp_file"
    mv -- "$temp_file" "$environment_file"
    trap - EXIT
    if systemctl --user is-active --quiet hyusk.service; then
        systemctl --user restart hyusk.service
    fi
    printf 'Laptop link enabled. Open the Hyusk butterfly menu → Connect phone → Generate fresh code.\n'
    exit 0
fi

cat <<EOF

Add these values to .env:

HYUSK_LINK_ENABLED=1
HYUSK_LINK_BIND=0.0.0.0:4488
HYUSK_LINK_ADVERTISE_HOST=$host
HYUSK_LINK_TLS_CERT=$cert
HYUSK_LINK_TLS_KEY=$key

Or run this script with --enable to update .env and restart the service.
Then open the Hyusk butterfly menu → Connect phone → Generate fresh code.
The pairing secret expires after five minutes and works once.
EOF
