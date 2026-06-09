# Build e empacotamento

## Dependências

- **Rust** (edição 2021) e Cargo.
- **GTK4** e **libadwaita** com os pacotes de desenvolvimento:
  - Fedora: `sudo dnf install gtk4-devel libadwaita-devel`
  - Debian/Ubuntu: `sudo apt install libgtk-4-dev libadwaita-1-dev`
- O SQLite é **compilado junto** (feature `bundled` do `rusqlite`) — não precisa
  de SQLite do sistema.

## Compilar e rodar

```bash
cargo build --release
./target/release/lexml-gtk            # abre vazio
./target/release/lexml-gtk arquivo.xml
```

## Testes

```bash
cargo test --release
```

O teste usa um XML de exemplo gerado em tempo de execução (não depende de nenhum
arquivo externo) e valida parsing, filtro, soma e round-trip de salvamento.

---

## AppImage (binário portátil para testar)

Para distribuir um executável único que roda sem instalar dependências:

```bash
./packaging/build-appimage.sh
```

Isso gera `Lê-XML-x86_64.AppImage` na raiz do projeto. O script:

1. Baixa as ferramentas (`linuxdeploy`, plugin GTK e `appimagetool`) em
   `.appimage-tools/` na primeira vez.
2. Compila em release e monta o `AppDir` com o binário e as bibliotecas GTK.
3. Remove bibliotecas que devem vir do sistema (acopladas ao glibc).
4. **Restaura binário e bibliotecas com cópias íntegras do sistema.** Isso é
   necessário porque, no Fedora 43+, o `patchelf` (usado pelo linuxdeploy)
   corrompe a seção `.relr.dyn`, causando *segfault* na inicialização. A
   restauração é recursiva (cobre também os módulos em subpastas, como os
   *immodules* do GTK e os *loaders* de imagem).
5. Empacota com `appimagetool`.

> O AppImage gerado fica acoplado ao glibc do sistema onde foi construído
> (adequado para testes na mesma distro). Para portabilidade entre distros, o
> caminho recomendado é o **Flatpak** (planejado).

---

## Controle de tema (claro/escuro e cara de KDE/GNOME)

Por padrão o app **segue o sistema**: no KDE adota o visual do KDE (Breeze), no
GNOME adota o Adwaita, e acompanha o modo claro/escuro do desktop. Você pode
forçar via variáveis de ambiente:

| Variável      | Valores                       | Efeito                                                        |
|---------------|-------------------------------|--------------------------------------------------------------|
| `LEXML_LOOK`  | `system` (padrão) \| `gnome`  | `gnome` força a aparência Adwaita/GNOME em qualquer desktop   |
| `LEXML_THEME` | `system` (padrão) \| `light` \| `dark` | força modo claro ou escuro                          |

Exemplos:

```bash
LEXML_THEME=dark ./Lê-XML-x86_64.AppImage           # escuro, seguindo o desktop
LEXML_THEME=light ./Lê-XML-x86_64.AppImage          # claro
LEXML_LOOK=gnome ./Lê-XML-x86_64.AppImage           # cara de GNOME mesmo no KDE
LEXML_LOOK=gnome LEXML_THEME=light ./Lê-XML-x86_64.AppImage
```

No **binário nativo** (fora do AppImage), o `LEXML_THEME` controla claro/escuro;
o visual é sempre Adwaita (libadwaita), seguindo o claro/escuro do sistema.

Observação: os botões da janela (minimizar/maximizar/fechar) e a borda são
desenhados pelo **gerenciador de janelas** do desktop (ex.: KWin no KDE), não
pelo app — por isso eles sempre têm a cara do seu sistema.
