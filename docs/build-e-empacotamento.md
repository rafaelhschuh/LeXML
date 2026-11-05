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
cargo clippy --all-targets -- -D warnings
```

A árvore não tem avisos do Clippy nem trechos fora do `rustfmt`; o CI reprova
qualquer novo aviso.

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

O resultado esperado são dois arquivos:

```text
Le-XML-x86_64.AppImage
Le-XML-x86_64.AppImage.zsync
```

O nome é escrito sem acento de propósito — veja
[Atualização automática](#atualização-automática).

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
8. gera o AppImage com a update information embutida e o `.zsync` ao lado.

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

## Atualização automática

O AppImage carrega uma *update information* na seção `.upd_info` do binário,
que diz ao [AppImageUpdate](https://github.com/AppImage/AppImageUpdate) e ao
AppImageLauncher onde procurar uma versão nova:

```text
gh-releases-zsync|rafaelhschuh|LeXML|latest|Le-XML-x86_64.AppImage.zsync
```

Com ela, quem já tem o AppImage atualiza sem baixar os 40 MB de novo — o zsync
compara blocos e transfere apenas o que mudou:

```bash
AppImageUpdate Le-XML-x86_64.AppImage
```

No AppImageLauncher, o mesmo está no item **Update** do menu de contexto do
aplicativo.

### O nome do arquivo não pode ter acento

O GitHub renomeia anexos de release que tenham caracteres especiais, por um
algoritmo que ele não documenta. Nas releases v0.5.1 a v0.5.3, o arquivo local
`Lê-XML-x86_64.AppImage` foi guardado como `Le-XML-x86_64.AppImage`.

Isso quebraria a atualização de duas maneiras: o padrão da update information
não casaria com o nome guardado, e o cabeçalho `URL:` gravado dentro do
`.zsync` apontaria para um arquivo inexistente no servidor. Por isso o script
já gera com o nome final em ASCII, idêntico ao das releases anteriores.

### Publicar uma versão

O caminho normal é por tag — veja [Integração contínua](#integração-contínua).
Suba a versão no `Cargo.toml`, faça o commit e empurre a tag correspondente:

```bash
git tag v0.6.0
git push origin v0.6.0
```

O workflow compila, roda os testes, gera os dois arquivos e cria a release.

Para publicar à mão, os **dois** arquivos precisam ser anexados. Sem o `.zsync`
publicado, a update information embutida não leva a lugar nenhum:

```bash
gh release create "v$(cargo pkgid | sed 's/.*[#@]//')" \
  --repo rafaelhschuh/LeXML \
  --latest \
  Le-XML-x86_64.AppImage Le-XML-x86_64.AppImage.zsync
```

O `--latest` não é decorativo: o padrão da update information aponta para
`latest`, então uma release marcada como pré-lançamento ficaria invisível para
quem tenta atualizar.

### Desativar ou redirecionar

A variável `LEXML_UPDATE_INFO` substitui a update information — útil em um
fork. Definida como vazia, o AppImage é gerado sem atualização automática e sem
`.zsync`:

```bash
LEXML_UPDATE_INFO= ./packaging/build-appimage.sh
```

## Integração contínua

Os workflows ficam em `.github/workflows/`.

### `ci.yml`

Dispara em push na `main`, em pull request e manualmente.

| Job | Bloqueia? | O que faz |
| --- | --- | --- |
| `teste` | sim | `cargo test --all-targets` e `cargo build --release` |
| `qualidade` | sim | `cargo fmt --check` e `cargo clippy --all-targets -- -D warnings` |

Desde a 0.6.1 a árvore está sem passivo de formatação e de lints, então o job de
qualidade também reprova: rode `cargo fmt` e o Clippy antes de enviar.

A imagem é `ubuntu-24.04` fixa, não `ubuntu-latest`: o crate exige as APIs do
GTK 4.12 e a 22.04 traz apenas a 4.6. Trocar de imagem deve ser uma decisão
consciente.

### `release.yml`

Dispara em tags `v*` e manualmente.

1. roda os testes — a tag é imutável e os usuários atualizam sozinhos pelo
   zsync, então publicar um binário quebrado é pior do que não publicar;
2. confere que a tag corresponde à versão do `Cargo.toml`;
3. gera o AppImage e o `.zsync`;
4. valida os artefatos: lê a update information da seção `.upd_info` com
   `objcopy` e confere que o cabeçalho `URL:` do `.zsync` cita exatamente o
   nome que o GitHub guardará como anexo;
5. publica a release com `--latest`.

Executado manualmente pela aba **Actions**, com a opção *Publicar* desmarcada,
ele percorre todo o empacotamento sem criar release nenhuma e deixa os
artefatos para download na própria execução. É a forma de testar mudanças no
`build-appimage.sh` sem queimar uma tag.

A leitura da update information usa `objcopy` em vez de executar o AppImage
porque o runner não tem FUSE, e `APPIMAGE_EXTRACT_AND_RUN` faria o AppImage
abrir a janela GTK em vez de imprimir a informação.

## Variáveis de ambiente

### `LEXML_THEME`

Substitui a preferência salva:

```bash
LEXML_THEME=dark cargo run
LEXML_THEME=light ./Le-XML-x86_64.AppImage
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
LEXML_GDK_BACKEND=x11 ./Le-XML-x86_64.AppImage
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
