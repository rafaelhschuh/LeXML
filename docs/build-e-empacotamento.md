# Compilação e empacotamento

## Requisitos

- Linux em arquitetura compatível com as bibliotecas instaladas;
- Rust com Cargo, edição 2021;
- GTK 4.12 ou superior com arquivos de desenvolvimento;
- compilador C e ferramentas básicas de build;
- `curl`, `find` e utilitários GNU para gerar o AppImage.

O SQLite não precisa ser instalado separadamente: `rusqlite` utiliza a
funcionalidade `bundled`.

### Fedora

```bash
sudo dnf install rust cargo gtk4-devel gcc
```

### Debian e Ubuntu

O projeto habilita a API GTK 4.12. Em distribuições cuja versão estável fornece
um GTK anterior, será necessário usar uma versão mais recente da distribuição
ou outra forma de obter GTK 4.12+.

```bash
sudo apt install cargo rustc libgtk-4-dev build-essential
```

Confirme a versão disponível:

```bash
pkg-config --modversion gtk4
```

## Compilar

Para desenvolvimento:

```bash
cargo build
```

Para distribuição local:

```bash
cargo build --release
```

O executável é criado em:

```text
target/debug/lexml-gtk
target/release/lexml-gtk
```

## Executar

Sem documento inicial:

```bash
cargo run
```

Com um XML:

```bash
cargo run -- caminho/arquivo.xml
```

Também é possível abrir mais de um arquivo:

```bash
cargo run -- primeiro.xml segundo.xml
```

## Verificações

### Testes

```bash
cargo test
```

Os testes atuais cobrem:

- leitura, filtro, soma e salvamento;
- preservação de `DATAPACKET` e `PARAMS`;
- preservação de quebras de linha;
- nomes de coluna com aspas;
- clonagem e movimentação de linhas.

### Formatação

Verificar:

```bash
cargo fmt --all -- --check
```

Aplicar:

```bash
cargo fmt --all
```

### Lints

```bash
cargo clippy --all-targets
```

> Na versão 0.5.3, a árvore ainda possui avisos do Clippy, principalmente pelo
> uso da sintaxe antiga do macro `glib::clone!`, além de itens não utilizados.
> Portanto, `cargo clippy --all-targets -- -D warnings` ainda não passa.

## Perfil de release

O [Cargo.toml](../Cargo.toml) configura:

- otimização nível 3;
- LTO;
- uma unidade de geração de código;
- remoção de símbolos do binário.

Essas opções reduzem o tamanho e favorecem o desempenho do executável final,
com custo de compilação maior.

## Gerar o AppImage

Execute a partir da raiz:

```bash
./packaging/build-appimage.sh
```

O resultado esperado é:

```text
Lê-XML-x86_64.AppImage
```

Na primeira execução, o script baixa para `.appimage-tools/`:

- `linuxdeploy`;
- plugin GTK do `linuxdeploy`;
- `appimagetool`.

Depois ele:

1. compila o projeto em release;
2. cria `AppDir`;
3. reúne o executável e dependências GTK;
4. remove bibliotecas que devem vir do sistema;
5. empacota temas de ícone necessários;
6. configura os backends Wayland/X11 e o carregador SVG;
7. restaura binários afetados pelo `patchelf` em sistemas recentes;
8. gera o AppImage.

### Características do AppImage atual

- alvo `x86_64`;
- preferência por Wayland, com fallback para X11;
- GTK4 e ícones empacotados;
- dependência da glibc e de algumas bibliotecas do sistema de build;
- adequado principalmente para testes na mesma distribuição ou em sistema
  compatível.

O script remove e recria o diretório `AppDir`. Não guarde arquivos manuais
dentro dele.

Para distribuição ampla entre versões diferentes de Linux, um pacote Flatpak
seria mais apropriado; ele ainda não faz parte do repositório.

## Variáveis de ambiente

### `LEXML_THEME`

Substitui a preferência salva:

```bash
LEXML_THEME=dark cargo run
LEXML_THEME=light ./Lê-XML-x86_64.AppImage
LEXML_THEME=system ./target/release/lexml-gtk
```

Valores reconhecidos:

| Valor | Efeito |
| --- | --- |
| `system` | segue o tema do ambiente |
| `light` | solicita variante clara |
| `dark` | solicita variante escura |

### `LEXML_GDK_BACKEND`

É utilizada pelo AppImage para substituir a preferência padrão
`wayland,x11`:

```bash
LEXML_GDK_BACKEND=x11 ./Lê-XML-x86_64.AppImage
```

## Integração com o desktop

Os arquivos de integração ficam em `packaging/`:

- `br.dev.schuh.lexml.desktop`;
- `br.dev.schuh.lexml.png`;
- `br.dev.schuh.lexml.svg`.

O identificador `br.dev.schuh.lexml` deve permanecer consistente entre o
aplicativo, o arquivo `.desktop` e o nome do ícone. Essa correspondência permite
que o ambiente gráfico associe corretamente a janela ao lançador e ao ícone do
dock.

Voltar ao [índice da documentação](README.md).
