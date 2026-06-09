#!/usr/bin/env bash
# Empacota o LeXML num AppImage standalone para testes.
# Uso: ./packaging/build-appimage.sh   (rode a partir da raiz do crate)
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$HERE"

APPID="com.empresa.lexml"
BIN="lexml-gtk"
TOOLS="$HERE/.appimage-tools"
mkdir -p "$TOOLS"

LD="$TOOLS/linuxdeploy-x86_64.AppImage"
LDGTK="$TOOLS/linuxdeploy-plugin-gtk.sh"

if [ ! -f "$LD" ]; then
  echo ">> Baixando linuxdeploy…"
  curl -L -o "$LD" \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
  chmod +x "$LD"
fi
if [ ! -f "$LDGTK" ]; then
  echo ">> Baixando plugin GTK…"
  curl -L -o "$LDGTK" \
    https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh
  chmod +x "$LDGTK"
fi

AIT="$TOOLS/appimagetool-x86_64.AppImage"
if [ ! -f "$AIT" ]; then
  echo ">> Baixando appimagetool…"
  curl -L -o "$AIT" \
    https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
  chmod +x "$AIT"
fi

echo ">> Compilando release…"
cargo build --release

APPDIR="$HERE/AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
cp "target/release/$BIN" "$APPDIR/usr/bin/"

export PATH="$TOOLS:$PATH"
export DEPLOY_GTK_VERSION=4
# O strip embutido no linuxdeploy não entende a seção .relr.dyn das libs novas
# do Fedora 43+; pular o strip evita aborto (o binário já é stripado pelo cargo).
export NO_STRIP=1

# Fase 1: popular o AppDir (binário + libs GTK), SEM gerar o AppImage ainda.
"$LD" --appdir "$APPDIR" \
  -e "$APPDIR/usr/bin/$BIN" \
  -d "packaging/$APPID.desktop" \
  -i "packaging/$APPID.png" \
  --plugin gtk

# Fase 2: remover libs ABI-acopladas ao glibc/host. Empacotá-las quebra o init
# (segfault em _dl_init, p.ex. libcrypt). Devem vir do sistema. O plugin gtk
# ignora --exclude-library, por isso removemos aqui à mão.
REMOVE=(
  libcrypt.so.* libselinux.so.* libsystemd.so.* libmount.so.*
  libblkid.so.* liblzma.so.* libpcre2-8.so.* libcap.so.* libgcc_s.so.*
)
for pat in "${REMOVE[@]}"; do
  rm -fv "$APPDIR"/usr/lib/$pat
done

# Tema: por padrão o app cede ao tema do sistema (Breeze no KDE / Adwaita no
# GNOME) e segue claro/escuro. Acrescentamos controles de override ao final do
# hook gtk: LEXML_LOOK (system|gnome) e LEXML_THEME (system|light|dark).
HOOK="$APPDIR/apprun-hooks/linuxdeploy-plugin-gtk.sh"
if [ -f "$HOOK" ] && ! grep -q 'Lê-XML: controle de tema' "$HOOK"; then
cat >> "$HOOK" <<'EOF'

# ===== Lê-XML: controle de tema (override do usuário) =====
# LEXML_LOOK = system (padrão, cede ao tema do desktop: Breeze no KDE / Adwaita
#              no GNOME) | gnome (força aparência Adwaita/GNOME em qualquer DE)
# LEXML_THEME = system (padrão, segue claro/escuro do desktop) | light | dark
case "${LEXML_THEME:-system}" in
  light) _LX_VARIANT=light;;
  dark)  _LX_VARIANT=dark;;
  *)     _LX_VARIANT="${GTK_THEME_VARIANT:-light}";;
esac
if [ "${LEXML_LOOK:-system}" = "gnome" ]; then
  unset GTK_THEME            # libadwaita/Adwaita; claro-escuro via AdwStyleManager (código)
else
  export GTK_THEME="Adwaita:${_LX_VARIANT}"   # cede ao tema do sistema
fi
EOF
fi

# Fase 2.5: o linuxdeploy roda patchelf em TUDO (binário e libs) para gravar o
# RUNPATH $ORIGIN/../lib. No Fedora 43+, o patchelf corrompe a seção .relr.dyn
# (relocations relativas compactas) que o linker do sistema e o rustc geram —
# resultado: SIGSEGV no _dl_init/_init, antes do main. Como o AppRun (hook gtk)
# já exporta LD_LIBRARY_PATH, o RUNPATH é dispensável: sobrescrevemos cada
# arquivo com a cópia íntegra do sistema. Inerentemente acopla ao glibc do host
# (suficiente para testes; portabilidade real vem depois via Flatpak).
echo ">> Restaurando binário e libs íntegros (anti-corrupção do patchelf)…"
cp -f --remove-destination "target/release/$BIN" "$APPDIR/usr/bin/$BIN"
# Recursivo: além das libs no topo de usr/lib, cobre os MÓDULOS carregáveis em
# subpastas (gtk-4.0/immodules, gdk-pixbuf loaders, gio modules) — também são
# patchelf'd e corrompidos. Foi um deles (libim-ibus.so) que segfaultou no
# gtk_init → g_io_modules_scan_all. Casamos por basename na árvore do sistema.
while IFS= read -r -d '' lib; do
  base="$(basename "$lib")"
  src="$(find /usr/lib64 /lib64 -name "$base" 2>/dev/null | head -1)"
  [ -n "$src" ] && cp -f --remove-destination "$src" "$lib"
done < <(find "$APPDIR/usr/lib" -name '*.so*' -print0)

# Fase 3: empacotar o AppDir já limpo com appimagetool (apenas compacta, NÃO
# re-analisa dependências — por isso não recopia as libs removidas acima).
ARCH=x86_64 "$AIT" "$APPDIR" "Lê-XML-x86_64.AppImage"

echo ">> Pronto. AppImage gerado na raiz do projeto."
ls -1 *.AppImage
