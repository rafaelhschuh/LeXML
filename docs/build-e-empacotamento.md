# Build e empacotamento

## Dependências

- **Rust** (edição 2021) e Cargo.
- **GTK4** com os pacotes de desenvolvimento (o app é **GTK4 puro**, sem libadwaita):
  - Fedora: `sudo dnf install gtk4-devel`
  - Debian/Ubuntu: `sudo apt install libgtk-4-dev`
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

## Controle de tema (Seguir o sistema / Claro / Escuro)

Por ser **GTK4 puro** (sem libadwaita), o app **segue o tema do sistema** — cores,
**cor de destaque (accent)** e variante clara/escura vêm do desktop (GNOME, Zorin
OS etc.). Em **Configurações → Geral → Esquema de cores** você escolhe entre
**Seguir o sistema** (padrão), **Claro** ou **Escuro**; a escolha é salva.

Também é possível forçar claro/escuro por variável de ambiente:

| Variável      | Valores                                | Efeito                          |
|---------------|----------------------------------------|---------------------------------|
| `LEXML_THEME` | `system` (padrão) \| `light` \| `dark` | força modo claro ou escuro      |

Exemplos:

```bash
LEXML_THEME=dark ./Lê-XML-x86_64.AppImage     # escuro
LEXML_THEME=light ./Lê-XML-x86_64.AppImage    # claro
```

> No **binário nativo** (fora do AppImage) a integração com o tema do sistema é
> total. Dentro do **AppImage**, a GTK empacotada tenta localizar o tema do
> sistema via `XDG_DATA_DIRS`; se o tema não estiver disponível, cai no tema GTK
> padrão. Para integração visual completa e portátil, o caminho recomendado é o
> **Flatpak** (planejado).

Observação: os botões da janela (minimizar/maximizar/fechar) e a borda são
desenhados pelo **gerenciador de janelas** do desktop, não pelo app — por isso
sempre têm a cara do seu sistema.
