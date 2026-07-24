# Arquitetura e desenvolvimento

## Visão geral

O Lê-XML é um binário Rust de processo único. A interface GTK trabalha com uma
representação SQLite em memória; o arquivo XML só é lido na abertura e reescrito
no salvamento.

```text
arquivo XML
    │
    ▼
quick-xml ──► XmlDoc + SQLite em memória
                         │
                         ▼
                DocumentView / GTK4
                         │
                edição, filtro e SQL
                         │
                         ▼
                   XML ou CSV
```

XMLs não reconhecidos como tabela seguem para `TextDocView`, sem passar pelo
SQLite.

## Dependências principais

| Crate | Finalidade |
| --- | --- |
| `gtk4` | interface GTK, GIO e GLib |
| `quick-xml` | leitura de eventos XML |
| `rusqlite` | tabela temporária e consultas |
| `anyhow` | propagação e contexto de erros |

O projeto requer as APIs GTK 4.12 e usa `rusqlite` com SQLite incorporado.

## Módulos

### `src/main.rs`

Declara os módulos e transfere a execução para `app::run()`.

### `src/app.rs`

Responsável por:

- criação do `gtk::Application`;
- janelas, cabeçalho e tela inicial;
- abertura de arquivos recebidos pela interface ou pelo sistema;
- carregamento em segundo plano;
- escolha entre modo tabela e modo texto;
- confirmação de alterações não salvas;
- configurações, tema, idioma e tela Sobre;
- CSS específico da grade e do autocompletar.

Cada janela mantém um `OpenDoc`, que pode conter `DocumentView` ou
`TextDocView`.

### `src/xmldoc.rs`

É a camada de dados do modo tabela:

- interpreta `FIELD` e `ROW`;
- cria a tabela SQLite `dados`;
- protege identificadores SQL;
- executa filtros e consultas;
- atualiza células;
- altera linhas e colunas;
- soma valores;
- serializa XML e CSV.

As colunas de negócio usam `TEXT`. `__ord` mantém a ordenação e o `rowid` do
SQLite é projetado como `_rid` nas visões editáveis.

### `src/document.rs`

Constrói a interface tabular e coordena as ações do usuário com `XmlDoc`.

O resultado de uma consulta é convertido em objetos `RowObject` e inserido em
`gio::ListStore` em blocos de 2.000 linhas. Um contador de geração cancela uma
carga anterior quando outra visão é solicitada.

Visões criadas por filtro incluem `_rid` e são editáveis. Resultados de SQL
livre são apresentados como somente leitura.

### `src/row_object.rs`

Objeto GLib usado por cada linha visível. Mantém:

- identificador da linha;
- vetor de valores;
- indicador de edição.

### `src/textdoc.rs`

Editor simples para arquivos que não entram no modo tabela. Controla o buffer,
estado de alteração, salvamento direto e **Salvar como**.

### `src/config.rs`

Lê e grava preferências simples em:

```text
<diretório de configuração do usuário>/lexml/config.ini
```

Exemplo:

```ini
theme=system
lang=pt
```

Falhas de leitura ou escrita das preferências são toleradas e não impedem a
execução.

### `src/i18n.rs`

Contém o catálogo interno em português e inglês. O idioma é definido uma vez no
início do processo; por isso sua alteração exige reinicialização.

### `src/dialog.rs`

Centraliza alertas e formulários modais feitos com GTK4 puro.

## Fluxo de abertura

1. O GTK recebe ativação normal ou uma lista de arquivos.
2. É criada uma janela para cada arquivo.
3. A janela mostra um indicador de carregamento.
4. `gio::spawn_blocking` executa `XmlDoc::open()` e a consulta inicial.
5. Em caso de sucesso, a thread principal cria `DocumentView`.
6. As linhas entram na grade em blocos.
7. Se a interpretação tabular falhar, o conteúdo segue para `TextDocView`.

O carregamento pesado fora da thread principal e a inserção em blocos evitam
congelar a interface com documentos grandes.

## Fluxo de edição e salvamento

No modo tabela:

1. uma edição atualiza o SQLite imediatamente;
2. o documento é marcado como alterado;
3. filtros e somas consultam a base em memória;
4. salvar consulta todas as colunas por `__ord`;
5. `XmlDoc::save()` reconstrói o `DATAPACKET`.

No modo texto, o `gtk::TextBuffer` é a fonte do conteúdo salvo.

O estado alterado é usado para pedir confirmação ao fechar a janela.

## Filtro e consulta

O filtro monta:

```sql
SELECT rowid AS _rid, *
FROM dados
WHERE <expressão>
ORDER BY __ord
```

Sem expressão, o trecho `WHERE` é omitido.

A consulta livre prepara a instrução fornecida pelo usuário diretamente no
SQLite em memória. Seu resultado não possui uma identidade de linha garantida e
por isso a interface desabilita edição e operações estruturais.

## Decisões de desempenho

- SQLite em memória com `journal_mode=OFF` e `synchronous=OFF`;
- transação única durante a importação;
- parsing orientado a eventos com `quick-xml`;
- abertura e consulta inicial fora da thread GTK;
- carregamento visual em blocos;
- cancelamento por geração de cargas obsoletas;
- capturas fracas nos handlers das células para evitar ciclos de referência;
- liberação explícita da grade ao trocar o documento;
- atualização pontual da lista após operações de linha e coluna.

## Testes

Os testes unitários ficam ao final de `src/xmldoc.rs` e usam arquivos temporários
gerados em tempo de execução.

Execute:

```bash
cargo test
```

Ao alterar parsing ou serialização, inclua um teste de round-trip:

1. gerar um XML mínimo;
2. abrir com `XmlDoc`;
3. executar a operação;
4. salvar em outro arquivo;
5. reabrir e comparar a semântica esperada.

## Pontos de atenção

- `document.rs` concentra grande parte da interface tabular; mudanças devem
  preservar o ciclo de vida dos handlers e capturas fracas.
- O fallback para modo texto atualmente também absorve erros de parsing ou
  leitura sem expor a causa ao usuário.
- A exportação CSV consulta toda a tabela, não o filtro ou SQL visível.
- A serialização preserva dados selecionados, não a formatação literal do XML.
- A sintaxe antiga de `glib::clone!` gera avisos nas versões atuais do crate.
- O catálogo de tradução retorna texto vazio para chaves ausentes em português;
  revise os dois idiomas ao adicionar uma chave.

## Rotina recomendada antes de entregar mudanças

```bash
cargo fmt --all
cargo test
cargo clippy --all-targets
cargo build --release
```

Para alterações de interface, execute também o aplicativo com:

- documento tabular vazio;
- documento pequeno editável;
- documento com muitas linhas;
- XML fora do formato tabular;
- tema claro e escuro.

Voltar ao [índice da documentação](README.md).
