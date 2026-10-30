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

O projeto requer as APIs GTK 4.12 e usa `rusqlite` com as funcionalidades
`bundled` (SQLite incorporado) e `limits` — esta última é usada para zerar
`SQLITE_LIMIT_ATTACHED` e impedir que uma consulta do usuário anexe arquivos de
banco externos.

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
- protege identificadores SQL (`quote_ident`);
- valida nomes de coluna como `Name` do XML (`is_valid_xml_name`);
- executa filtros e consultas, com as travas de somente leitura;
- atualiza células;
- altera linhas e colunas;
- soma valores;
- serializa XML e CSV;
- grava em disco de forma atômica (`write_atomic`, também usada por
  `textdoc.rs`).

As colunas de negócio usam `TEXT`. A tabela tem uma chave primária interna
explícita (`INTEGER PRIMARY KEY`, equivalente ao `rowid`) e uma coluna `REAL`
de ordenação. Os dois nomes, mais o apelido do identificador nas visões
editáveis, partem de `__rid`, `__ord` e `_rid` e são derivados com sufixo
numérico se algum `FIELD` do arquivo já usar esses nomes. A chave primária
explícita também evita que um campo chamado `rowid` encubra o identificador
interno.

### Somente leitura na consulta livre

`XmlDoc::query()` passa por `run_readonly()`, que aplica, em ordem:

1. `ensure_select_only()` — a instrução precisa começar por `SELECT`, `WITH`,
   `VALUES` ou `EXPLAIN`, desprezando comentários à esquerda;
2. `ReadOnlyGuard` — liga `PRAGMA query_only` e o desliga no `Drop`, de modo
   que um erro ou um pânico não deixem a conexão travada;
3. `Statement::readonly()` — rede de segurança do próprio SQLite, para o caso
   de uma instrução de leitura na aparência escrever de fato.

Somada ao `SQLITE_LIMIT_ATTACHED = 0` aplicado na abertura da conexão, a
combinação impede tanto a perda dos dados em memória quanto a escrita de
arquivos arbitrários no disco a partir da caixa de SQL.

`filter()` e `sum_column()` também passam por `run_readonly()`, já que
incorporam uma expressão `WHERE` escrita pelo usuário.

### `src/document.rs`

Constrói a interface tabular e coordena as ações do usuário com `XmlDoc`.

O resultado de uma consulta é convertido em objetos `RowObject` e inserido em
`gio::ListStore` em blocos de 2.000 linhas. Um contador de geração cancela uma
carga anterior quando outra visão é solicitada.

Visões criadas por filtro incluem o identificador de linha e são editáveis;
`XmlDoc::rid_alias()` informa o nome dessa coluna e `is_internal_col()` diz
quais colunas do resultado não devem ser exibidas nem exportadas. Resultados de
SQL livre são apresentados como somente leitura.

### `src/row_object.rs`

Objeto GLib usado por cada linha visível. Mantém:

- identificador da linha;
- vetor de valores;
- indicador de edição.

### `src/textdoc.rs`

Editor simples para arquivos que não entram no modo tabela. Controla o buffer,
estado de alteração, salvamento direto e **Salvar como**.

Recebe de `app.rs` um sinalizador `read_only` e, opcionalmente, o motivo da
recusa do modo tabela. Com `read_only`, a `TextView` e o botão de salvar ficam
inativos e `write_to()` recusa a gravação: o buffer não corresponde aos bytes
do arquivo (leitura não-UTF-8 ou cortada no teto de 64 MiB) e gravá-lo
destruiria o conteúdo.

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
7. Se a interpretação tabular falhar, o conteúdo segue para `TextDocView`,
   levando junto o erro para ser exibido no rodapé.

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
SELECT "__rid" AS "_rid", "col1", "col2", ...
FROM dados
WHERE <expressão>
ORDER BY "__ord"
```

As colunas são listadas explicitamente, em vez de `SELECT *`, para que nenhuma
coluna de controle vaze para a grade mesmo quando o arquivo traz um campo com
nome parecido. Sem expressão, o trecho `WHERE` é omitido.

A consulta livre prepara a instrução fornecida pelo usuário no SQLite em
memória, depois de passar pelas travas de somente leitura descritas acima. Seu
resultado não possui uma identidade de linha garantida e por isso a interface
desabilita edição e operações estruturais.

## Decisões de desempenho

- SQLite em memória com `journal_mode=OFF` e `synchronous=OFF`;
- transação única durante a importação, com as linhas inseridas conforme o
  parser as produz — nada de acumular o arquivo inteiro antes do primeiro
  `INSERT`;
- leitura do texto cru limitada a 1 MiB, só para recuperar a moldura do
  documento (`<DATAPACKET>` e `<PARAMS>`);
- parsing orientado a eventos com `quick-xml`;
- abertura e consulta inicial fora da thread GTK;
- carregamento visual em blocos;
- cancelamento por geração de cargas obsoletas;
- capturas fracas nos handlers das células para evitar ciclos de referência;
- liberação explícita da grade ao trocar o documento;
- atualização pontual da lista após operações de linha e coluna.

## Testes

Os testes unitários ficam ao final de `src/xmldoc.rs` e usam arquivos
temporários gerados em tempo de execução. Monte cada caminho com o auxiliar
`tmp_path()`: os testes rodam em paralelo e um nome fixo faz um teste ler o
arquivo enquanto outro o reescreve, produzindo falhas intermitentes.

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

Há também testes de regressão para as garantias de segurança e integridade:
recusa de escrita pela caixa de SQL, recusa de nomes de campo inválidos,
`<PARAMS>` escondido em comentário, campos com os nomes das colunas internas,
gravação atômica sem rastros e escape de valores. Ao mexer nessas áreas,
mantenha-os passando em vez de ajustá-los à nova saída.

## Pontos de atenção

- `document.rs` concentra grande parte da interface tabular; mudanças devem
  preservar o ciclo de vida dos handlers e capturas fracas.
- A exportação CSV consulta toda a tabela, não o filtro ou SQL visível.
- Nomes de coluna são nomes de atributo XML no arquivo salvo; qualquer caminho
  novo que crie ou renomeie colunas precisa passar por `check_column_name()`.
- Toda gravação em disco deve usar `write_atomic()`, nunca `fs::write` ou
  `File::create` diretamente. A exceção conhecida é `config.rs`, cujo
  `config.ini` volta ao padrão se corromper.
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
