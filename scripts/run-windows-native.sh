#!/usr/bin/env bash
set -euo pipefail

if ! command -v powershell.exe >/dev/null 2>&1; then
    echo "error: powershell.exe not found. Run this from WSL with Windows interop enabled." >&2
    exit 1
fi

if ! command -v wslpath >/dev/null 2>&1; then
    echo "error: wslpath not found. Use scripts/run-windows-native.ps1 from Windows PowerShell instead." >&2
    exit 1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ps_script="$(wslpath -w "$script_dir/run-windows-native.ps1")"

args=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --check)
            args+=("-Check")
            shift
            ;;
        --target)
            if [[ $# -lt 2 ]]; then
                echo "error: --target requires a Rust target triple." >&2
                exit 1
            fi
            args+=("-Target" "$2")
            shift 2
            ;;
        --toolchain)
            if [[ $# -lt 2 ]]; then
                echo "error: --toolchain requires a Rust toolchain name." >&2
                exit 1
            fi
            args+=("-Toolchain" "$2")
            shift 2
            ;;
        --)
            shift
            args+=("$@")
            break
            ;;
        *)
            args+=("$1")
            shift
            ;;
    esac
done

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$ps_script" "${args[@]}"
