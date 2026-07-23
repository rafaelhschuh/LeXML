#!/usr/bin/env bash
# Empacota o LeXML num AppImage standalone para testes.
# Uso: ./packaging/build-appimage.sh   (rode a partir da raiz do crate)
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$HERE"

APPID="br.dev.schuh.lexml"
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

# Ícones: o app usa ícones simbólicos (SVG) do tema Adwaita (media-floppy-symbolic,
# x-office-spreadsheet-symbolic, etc.). Duas coisas quebram no AppImage:
#  a) O loaders.cache do gdk-pixbuf lista os loaders por nome RELATIVO
#     ("libpixbufloader-svg.so") e aponta LoaderDir para o caminho do HOST de
#     compilação. Sem GDK_PIXBUF_MODULEDIR, a GTK procura o loader SVG no host,
#     não acha, e TODO ícone simbólico cai no glifo de "imagem quebrada".
#     O hook exporta GDK_PIXBUF_MODULE_FILE mas NÃO o MODULEDIR — corrigimos abaixo.
#  b) Máquinas sem o tema Adwaita instalado não teriam os SVGs. Empacotamos o
#     tema (só símbolos + escaláveis) para o app ser autossuficiente.
# Copiamos o gdk-pixbuf-query-loaders para regenerar o cache em tempo de execução
# (ver hook). O cache empacotado tem caminhos relativos + LoaderDir do host, que a
# GTK não resolve dentro do AppImage montado.
QL="$(find /usr/lib/x86_64-linux-gnu /usr/lib64 /usr/lib -name 'gdk-pixbuf-query-loaders' 2>/dev/null | grep -m1 -iE 'x86_64|lib64|/usr/lib/gdk' || find /usr/lib -name 'gdk-pixbuf-query-loaders' 2>/dev/null | head -1)"
if [ -n "$QL" ]; then
  cp -f "$QL" "$APPDIR/usr/lib/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders"
  chmod +x "$APPDIR/usr/lib/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders"
fi

echo ">> Empacotando tema de ícones Adwaita (símbolos)…"
for base in /usr/share/icons/Adwaita /usr/share/icons/hicolor; do
  name="$(basename "$base")"
  [ -d "$base" ] || continue
  mkdir -p "$APPDIR/usr/share/icons/$name"
  cp -a "$base/." "$APPDIR/usr/share/icons/$name/" 2>/dev/null || true
done

# Tema: o app é GTK4 puro e deve SEGUIR o tema da DE ONDE FOR EXECUTADO (não a
# de onde foi compilado). O plugin gtk, porém, força "GTK_THEME=Adwaita:variant"
# ("Custom themes are broken") — isso ignora o tema do desktop. Sobrescrevemos
# esse comportamento: o bloco abaixo é acrescentado ao FINAL do hook (roda depois
# do plugin, então vence) e detecta, em tempo de execução, o tema da DE atual.
HOOK="$APPDIR/apprun-hooks/linuxdeploy-plugin-gtk.sh"
if [ -f "$HOOK" ] && ! grep -q 'Lê-XML: tema da DE atual' "$HOOK"; then
cat >> "$HOOK" <<'EOF'

# ===== Lê-XML: backend GDK — preferir Wayland =====
# O plugin gtk força GDK_BACKEND=x11 (comentário padrão "Crash with Wayland
# backend on Wayland" — genérico do upstream, NÃO observado neste app). Sob
# XWayland qualquer travada momentânea do laço principal congela a entrada do
# servidor X para a sessão inteira (teclado preso dentro e fora do app). Como o
# build nativo (cargo run) roda liso no Wayland, preferimos Wayland aqui e só
# caímos para X11 quando não há sessão Wayland. Override manual: LEXML_GDK_BACKEND.
export GDK_BACKEND="${LEXML_GDK_BACKEND:-wayland,x11}"

# ===== Lê-XML: loader SVG do gdk-pixbuf (senão ícones simbólicos quebram) =====
# O loaders.cache empacotado tem caminhos relativos + LoaderDir do host de
# compilação; dentro do AppImage montado (/tmp/.mount_*) a GTK não acha o loader
# SVG e TODO ícone simbólico vira "imagem quebrada". Regeneramos o cache aqui, em
# tempo de execução, com os caminhos ABSOLUTOS do ponto de montagem atual.
_lx_qloaders="$APPDIR/usr/lib/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders"
_lx_moddir="$APPDIR/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders"
if [ -x "$_lx_qloaders" ] && [ -d "$_lx_moddir" ]; then
  _lx_cache="$(mktemp -t lexml-loaders.XXXXXX.cache)"
  if GDK_PIXBUF_MODULEDIR="$_lx_moddir" "$_lx_qloaders" > "$_lx_cache" 2>/dev/null; then
    export GDK_PIXBUF_MODULE_FILE="$_lx_cache"
  fi
fi
export GDK_PIXBUF_MODULEDIR="$_lx_moddir"

# ===== Lê-XML: tema da DE atual (executado, não compilado) =====
# Reaproveita $GTK_THEME_VARIANT (claro/escuro) já calculado pelo plugin acima a
# partir do portal/gsettings da máquina atual. Override manual: LEXML_THEME.
case "${LEXML_THEME:-}" in
  light) GTK_THEME_VARIANT="light";;
  dark)  GTK_THEME_VARIANT="dark";;
esac

# 1) Nome do tema GTK da DE atual (GNOME/Zorin, Cinnamon, MATE, XFCE, KDE…).
_lx_theme="$(gsettings get org.gnome.desktop.interface gtk-theme 2>/dev/null | tr -d "'\"")"
[ -z "$_lx_theme" ] && _lx_theme="${GTK_THEME_NAME:-}"
[ -z "$_lx_theme" ] && _lx_theme="Adwaita"

# 2) Só usa o tema da DE se existir uma versão gtk-4.0 dele em algum local padrão
#    (temas só-GTK3 fariam a GTK4 cair em Adwaita de qualquer forma).
_lx_found=""
for _d in "$HOME/.themes" "$HOME/.local/share/themes" /usr/share/themes /usr/local/share/themes; do
  if [ -d "$_d/$_lx_theme/gtk-4.0" ]; then _lx_found="$_lx_theme"; break; fi
done
[ -z "$_lx_found" ] && _lx_found="Adwaita"

# 3) Aplica o tema da DE (com a variante clara/escura). Independentemente disso, a
#    GTK ainda carrega ~/.config/gtk-4.0/gtk.css da DE por cima (ex.: cores do KDE).
export GTK_THEME="${_lx_found}${GTK_THEME_VARIANT:+:$GTK_THEME_VARIANT}"

# 4) Garante que os temas/ícones do sistema atual sejam encontrados pela GTK do
#    AppImage (acrescenta os diretórios de dados do host ao caminho de busca).
export XDG_DATA_DIRS="${XDG_DATA_DIRS}:$HOME/.local/share:/usr/share:/usr/local/share"
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
