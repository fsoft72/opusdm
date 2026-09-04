#!/bin/sh
# Install JetBrainsMono Nerd Font for the current user.
set -e
DIR="$HOME/.local/share/fonts"
mkdir -p "$DIR"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fL https://github.com/ryanoasis/nerd-fonts/releases/latest/download/JetBrainsMono.tar.xz -o "$tmp/font.tar.xz"
tar -xf "$tmp/font.tar.xz" -C "$DIR"
fc-cache -f "$DIR"
echo "Installed to $DIR (font name: 'JetBrainsMono Nerd Font')"
