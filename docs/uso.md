# Guia de uso

Este guia descreve o comportamento da versão 0.5.4 do Lê-XML.

## Conteúdo

- [Abrir ou criar documentos](#abrir-ou-criar-documentos)
- [Modos de visualização](#modos-de-visualização)
- [Trabalhar com a tabela](#trabalhar-com-a-tabela)
- [Selecionar, copiar e colar linhas](#selecionar-copiar-e-colar-linhas)
- [Desfazer e refazer](#desfazer-e-refazer)
- [Ordenar por uma coluna](#ordenar-por-uma-coluna)
- [Localizar valores](#localizar-valores)
- [Substituir valores](#substituir-valores)
- [Filtrar com SQL WHERE](#filtrar-com-sql-where)
- [Executar uma consulta SQL](#executar-uma-consulta-sql)
- [Somar uma coluna](#somar-uma-coluna)
- [Salvar](#salvar)
- [Backup e recuperação](#backup-e-recuperação)
- [Exportar CSV](#exportar-csv)
- [Configurações](#configurações)
- [Atalhos](#atalhos)
- [Cuidados e limitações](#cuidados-e-limitações)

## Abrir ou criar documentos

### Abrir um XML

Use uma destas opções:

- clique no botão **Abrir** do cabeçalho;
- clique em **Abrir arquivo** na tela inicial;
- abra um `.xml` associado ao Lê-XML pelo gerenciador de arquivos;
- informe o caminho na linha de comando:

```bash
lexml-gtk caminho/arquivo.xml
```

### Arquivos recentes

Os últimos 12 arquivos abertos ou salvos ficam guardados em
`~/.config/lexml/recentes.txt`. Para reabrir um deles:

- clique no arquivo na lista **Recentes** da tela inicial, que mostra os 8
  mais recentes;
- ou clique na seta ao lado do botão **Abrir**, no cabeçalho, para ver a
  lista completa.

Arquivos que foram movidos ou excluídos deixam de aparecer. **Limpar lista**
esvazia o histórico.

A primeira abertura reutiliza a janela inicial vazia. Se uma janela já contém
um documento, o próximo arquivo é aberto em outra janela. Também é possível
informar vários arquivos ao aplicativo; cada um recebe sua própria janela.

Enquanto um documento tabular é interpretado, a janela apresenta um indicador
de carregamento. As linhas são transferidas para a grade em blocos para manter a
interface responsiva.

### Criar um documento

Clique no botão **Novo** do cabeçalho. Um novo documento começa com:

- nome sugerido `novo.xml`;
- uma coluna chamada `coluna1`;
- nenhuma linha.

O primeiro salvamento abre o seletor de destino.

## Modos de visualização

### Modo tabela

Um XML com a estrutura `DATAPACKET`, `METADATA/FIELDS` e ao menos um `FIELD`
válido é aberto como tabela. Nesse modo ficam disponíveis edição, filtros,
consultas, operações estruturais, soma e CSV.

Consulte [Formato XML suportado](formato-xml.md) para ver a estrutura completa.

### Modo texto

Quando o arquivo não é reconhecido como tabela, o Lê-XML mostra seu conteúdo em
um editor monoespaçado. O rodapé exibe **Modo texto — XML sem estrutura de
tabela** e, quando há um motivo concreto para a recusa (um `FIELD` com nome
inválido, por exemplo), acrescenta a causa.

O modo texto oferece edição e salvamento, mas não disponibiliza filtros, SQL,
soma, operações de linha ou coluna e exportação CSV.

### Modo texto somente leitura

Dois casos abrem o editor com a edição e o salvamento desabilitados:

- o arquivo **não é UTF-8 válido** — por exemplo, um XML gravado em Latin-1;
- o arquivo **passa de 64 MiB**, o teto de leitura do editor.

Nos dois casos o texto exibido não representa fielmente os bytes do arquivo:
gravá-lo substituiria cada byte não reconhecido por `?` (U+FFFD) ou truncaria o
conteúdo. O rodapé explica a situação e o botão de salvar fica inativo.

Para editar um arquivo em Latin-1, converta-o antes:

```bash
iconv -f ISO-8859-1 -t UTF-8 entrada.xml > saida.xml
```

## Trabalhar com a tabela

### Selecionar e editar células

- O primeiro clique seleciona a linha e focaliza a célula.
- Clique novamente na célula focalizada para editá-la.
- Também é possível começar a digitar em uma célula focalizada; o conteúdo
  anterior é substituído pelo caractere digitado.
- Ao sair da edição, o valor é atualizado na base temporária em memória.

As alterações só chegam ao arquivo em disco quando o documento é salvo.

### Navegar pelo teclado

Com uma célula focalizada:

- use as setas para navegar entre células e linhas;
- durante a edição, `Tab` confirma e segue para a direita;
- `Shift+Tab` confirma e segue para a esquerda;
- as setas para cima e para baixo confirmam a edição e mudam de linha.

### Menu de contexto

Clique com o botão direito sobre uma célula. A linha e a coluna clicadas tornam-se
o alvo das ações.

No submenu **Linha**:

- **Copiar linha(s)** copia as linhas selecionadas (veja
  [Selecionar, copiar e colar linhas](#selecionar-copiar-e-colar-linhas));
- **Nova linha acima** ou **Nova linha abaixo** cria uma linha vazia;
- **Clonar para cima** ou **Clonar para baixo** copia a linha e a posiciona ao
  lado da original;
- **Mover para cima** ou **Mover para baixo** troca a linha com sua vizinha;
- **Excluir linha(s)** remove a linha — ou, se ela faz parte de uma seleção,
  todas as linhas selecionadas, num único passo de desfazer.

No submenu **Coluna**:

- **Somar coluna** soma a coluna clicada;
- **Renomear coluna…** abre o campo para trocar o nome da coluna;
- **Adicionar coluna** cria ao final uma coluna vazia com nome automático;
- **Excluir coluna** remove a coluna e seus valores.

O botão direito no título de uma coluna abre esse mesmo menu de coluna.

O documento sempre precisa manter pelo menos uma coluna.

### Renomear uma coluna

Clique com o botão direito numa célula ou no título da coluna, escolha
**Coluna → Renomear coluna…** (no título, **Renomear coluna…**), informe o novo
nome e pressione `Enter`.
A alteração atualiza tanto a tabela interna quanto o atributo `attrname` do
`FIELD` ao salvar.

O nome precisa ser um **nome válido de atributo XML**, porque é exatamente isso
que ele se torna no arquivo salvo. Valem letras (com acentos), dígitos, `_`,
`-`, `.` e `:`, começando por letra ou `_`. Não são aceitos nomes vazios,
repetidos, com espaços, com aspas ou iniciados por dígito — eles produziriam um
XML inválido na gravação. A mesma regra vale para **Adicionar coluna**.

Operações de edição e estrutura ficam desabilitadas enquanto uma consulta SQL
livre está sendo exibida.

## Selecionar, copiar e colar linhas

### Selecionar

- clique numa célula para selecionar só aquela linha;
- `Ctrl`+clique marca ou desmarca linhas soltas;
- `Shift`+clique seleciona o intervalo desde a última linha clicada;
- `Shift+↑` e `Shift+↓` estendem a seleção pelo teclado;
- `Ctrl+A` seleciona todas as linhas visíveis.

O rodapé mostra quantas linhas estão selecionadas. Clicar na área vazia
abaixo da última linha limpa a seleção.

### Copiar, recortar e colar

| Ação | Atalho | Menu |
| --- | --- | --- |
| Copiar as linhas selecionadas | `Ctrl+C` | **Linha → Copiar linha(s)** |
| Recortar (copiar e excluir) | `Ctrl+X` | — |
| Colar abaixo da linha atual | `Ctrl+V` | **Colar → Colar abaixo** |
| Colar acima da linha clicada | — | **Colar → Colar acima** |

As linhas vão para a área de transferência no mesmo formato do arquivo, um
elemento `<ROW .../>` por linha:

```xml
<ROW id="1" nome="carlos" valor="1.234,56" data="01/02/2024"/>
<ROW id="2" nome="Ana" valor="9" data="31/01/2024"/>
```

Por isso é possível colar:

- em outra janela do Lê-XML, inclusive com outro arquivo aberto;
- num editor de texto, onde as linhas aparecem como XML;
- de volta no Lê-XML a partir de um editor de texto ou de um trecho de
  arquivo `DATAPACKET`.

Ao colar, cada atributo vai para a coluna de mesmo nome. Atributos sem coluna
correspondente são ignorados e listados no rodapé; colunas que não aparecem na
linha colada ficam vazias. As linhas coladas ficam selecionadas, e um único
`Ctrl+Z` desfaz a colagem inteira.

O submenu **Colar** só aparece no botão direito quando a área de
transferência tem linhas `<ROW>`. Enquanto uma célula está em edição, `Ctrl+C`
e `Ctrl+V` copiam e colam apenas o texto dela. No resultado de uma consulta
SQL livre é possível copiar, mas não colar nem recortar.

## Desfazer e refazer

Use os botões de seta no início da barra de ferramentas ou os atalhos:

- `Ctrl+Z` desfaz a última alteração;
- `Ctrl+Shift+Z` ou `Ctrl+Y` refaz o que foi desfeito.

O histórico cobre edição de células, inclusão, clonagem, movimentação e
exclusão de linhas, e inclusão, renomeação e exclusão de colunas. Uma coluna
excluída volta na mesma posição, com seus atributos e valores.

- O histórico guarda as últimas 500 alterações do documento.
- Fazer uma alteração nova depois de desfazer descarta o que havia para refazer.
- Enquanto uma célula, o filtro ou o campo Localizar está em edição, os atalhos
  desfazem o texto digitado no próprio campo, e não a tabela.
- Desfazer e refazer ficam indisponíveis enquanto o resultado de uma consulta
  SQL livre está na tela; aplique o filtro novamente para voltar à visão
  editável.
- O histórico não é salvo junto com o arquivo e se perde ao fechar a janela.

## Ordenar por uma coluna

Clique no título de uma coluna para ordenar a grade por ela. Cada clique
avança um passo:

1. crescente — o título ganha `▲`;
2. decrescente — o título ganha `▼`;
3. ordem do documento — a seta some.

A comparação considera o conteúdo:

- números comparam pelo valor, inclusive no formato brasileiro
  (`9` vem antes de `10`, e `1.234,56` depois de `100.5`);
- datas `dd/mm/aaaa` comparam no tempo;
- o restante compara como texto, sem diferenciar maiúsculas;
- células vazias ficam sempre no fim.

A ordenação muda **só a exibição**. Salvar mantém a ordem das linhas do
documento, e a exportação CSV também. Ela continua ativa ao aplicar outro
filtro e é desfeita ao executar uma consulta SQL, que traz a própria ordem.

Com a grade ordenada, **Mover para cima/baixo** fica indisponível: a linha
vizinha na tela não é a vizinha no documento. Volte à ordem do documento para
mover linhas.

Para renomear a coluna, use o botão direito no título.

## Localizar valores

Use o campo **Localizar…**:

1. digite parte do valor procurado;
2. aguarde a seleção automática ou pressione `Enter`;
3. pressione `Enter` novamente para procurar a próxima linha correspondente.

A busca:

- verifica todas as colunas visíveis;
- não diferencia maiúsculas de minúsculas;
- procura texto contido na célula, não apenas correspondência exata;
- começa depois da linha selecionada e volta ao início ao chegar ao fim;
- apenas seleciona a ocorrência; não esconde outras linhas.

## Substituir valores

Clique no botão de localizar e substituir, ao lado do campo **Localizar…**, ou
pressione `Ctrl+H`. Uma segunda barra aparece abaixo da barra de ferramentas.

1. digite o texto procurado em **Localizar…**;
2. digite o novo texto em **Substituir por…**;
3. use **Substituir** para trocar na linha selecionada e ir para a próxima
   ocorrência, ou **Substituir todas** para trocar em todas as linhas visíveis.

Opções da barra:

- **Todas as colunas** — escolha uma coluna para limitar a busca e a troca a
  ela;
- **Maiúsc./minúsc.** — diferencia maiúsculas de minúsculas;
- **Célula inteira** — a célula precisa ser igual ao texto procurado, e não
  apenas contê-lo. Com **Localizar…** vazio, encontra as células vazias, o que
  serve para preencher valores em branco.

As opções também valem para o **Localizar…** enquanto estiverem marcadas.

**Substituir todas** respeita o filtro `WHERE` ativo: só troca nas linhas
visíveis. O rodapé informa quantas células mudaram, e um único `Ctrl+Z` desfaz
a substituição inteira. Substituir não está disponível no resultado de uma
consulta SQL livre.

`Esc` fecha a barra; `Ctrl+F` leva o cursor para **Localizar…**.

## Filtrar com SQL WHERE

O campo **Filtrar (SQL WHERE)** aceita somente a expressão que viria depois de
`WHERE`. Pressione `Enter` ou clique na lupa para aplicar.

```sql
valor <> '0.00'
nome LIKE 'A%'
id >= '2' AND valor <> ''
CAST(valor AS REAL) > 100
```

Os valores importados do XML são armazenados como texto. Use aspas para
comparações textuais e `CAST` quando precisar de comparação numérica.

Enquanto você digita o começo do nome de uma coluna, o aplicativo mostra até
oito sugestões. Use:

- `↑` e `↓` para escolher;
- `Tab` ou `Enter` para completar;
- `Esc` para fechar;
- clique para completar com o mouse.

Nomes de coluna que não são identificadores SQL simples são inseridos entre
aspas duplas automaticamente.

Para remover o filtro, apague o conteúdo do campo e aplique novamente. O rodapé
mostra a quantidade de linhas e a expressão ativa.

> A expressão é executada diretamente pelo SQLite sobre uma base temporária
> chamada `dados`. Um erro de sintaxe é mostrado em um diálogo e não altera a
> visão atual.

## Executar uma consulta SQL

Clique em **SQL…**, escreva uma instrução completa e selecione **Executar**.
A tabela interna se chama `dados`.

```sql
SELECT nome, valor
FROM dados
ORDER BY CAST(valor AS REAL) DESC
```

```sql
SELECT nome, COUNT(*) AS quantidade
FROM dados
GROUP BY nome
ORDER BY quantidade DESC
```

O resultado substitui temporariamente o conteúdo visível e é somente leitura na
interface. Para voltar à visão editável, aplique novamente o filtro — vazio para
todas as linhas ou com uma expressão `WHERE`.

As colunas internas de controle, quando presentes, não são exibidas.

### A consulta é somente leitura de verdade

Só são aceitas instruções iniciadas por `SELECT`, `WITH`, `VALUES` ou
`EXPLAIN`. Qualquer outra coisa é recusada com uma mensagem de erro, antes de
chegar ao banco:

```sql
DELETE FROM dados          -- recusado
UPDATE dados SET nome = '' -- recusado
DROP TABLE dados           -- recusado
ATTACH DATABASE '/tmp/x.db' AS ev  -- recusado
VACUUM INTO '/tmp/x.db'    -- recusado
```

A restrição não é apenas cosmética. Sem ela, um comando digitado por engano
apagaria os dados carregados sem marcar o documento como alterado, e um
`ATTACH` seguido de `CREATE TABLE ... AS SELECT` escreveria o conteúdo da
tabela em qualquer arquivo do sistema. Três travas independentes garantem a
restrição, e o banco é aberto sem a possibilidade de anexar outros arquivos.

Para alterar os dados, use a edição da tabela e o menu de contexto.

## Somar uma coluna

Clique em **Σ Somar**, escolha a coluna e confirme. Como alternativa, clique com
o botão direito em uma célula e use **Somar coluna**.

A soma respeita o filtro `WHERE` ativo e aceita:

- decimal com ponto: `1234.56`;
- formato brasileiro: `1.234,56`;
- números negativos.

Valores vazios ou não numéricos são ignorados. O resultado é apresentado no
formato brasileiro com duas casas decimais e informa quantos valores numéricos
foram considerados.

## Salvar

Use o botão com ícone de disquete ou `Ctrl+S`.

- Um arquivo aberto do disco é sobrescrito diretamente.
- Um documento novo abre **Salvar como** no primeiro salvamento.
- `Ctrl+Shift+S` sempre abre **Salvar como**.
- Uma edição de célula ainda ativa é confirmada antes da gravação.

Após salvar, o rodapé mostra o caminho e o horário. Se a janela for fechada com
alterações pendentes, o aplicativo oferece cancelar, fechar sem salvar ou
salvar.

No modo tabela, o XML é reconstruído a partir dos dados em memória. No modo
texto, o conteúdo do editor é gravado como está. Veja os detalhes de preservação
em [Formato XML suportado](formato-xml.md).

A gravação é atômica: o conteúdo novo é escrito em um arquivo temporário no
mesmo diretório e só então renomeado por cima do destino, preservando as
permissões do arquivo substituído. Se faltar espaço em disco ou o aplicativo
for interrompido no meio da escrita, o arquivo original continua intacto em vez
de ficar truncado. O mesmo vale para a exportação CSV.

## Backup e recuperação

O Lê-XML guarda backups fora da pasta do arquivo, em
`~/.cache/lexml/backup/`:

- **Cópia de recuperação** (`recuperacao/`) — cada alteração não salva num
  documento tabular é copiada logo em seguida (menos de um segundo depois da
  última edição; uma sequência rápida de edições vira uma gravação só). A
  cópia é gravada em segundo plano, sem travar a janela, e é apagada quando o
  documento é salvo ou fechado normalmente.
- **Versões** (`versoes/`) — antes de cada salvamento sobrescrever um arquivo,
  a versão que estava em disco é copiada como `nome_AAAAMMDD-HHMMSS.xml`.

### Recuperar depois de uma queda

Se o aplicativo fechar sem salvar — falta de energia, travamento, sistema
desligado —, a cópia de recuperação sobra no cache e o Lê-XML pergunta o que
fazer com ela, no estilo do Office:

- **ao abrir o aplicativo**, uma janela lista os documentos com cópia
  pendente;
- **ao abrir um arquivo** que tem cópia pendente (pelo seletor, pelos recentes
  ou pela linha de comando), a pergunta aparece antes de carregá-lo.

Para cada documento:

- **Restaurar** abre a cópia, com "(recuperado)" no título. O documento fica
  marcado como alterado e **Salvar** grava no arquivo de origem (ou abre
  **Salvar como**, se era um documento novo);
- **Descartar** apaga a cópia e, ao abrir um arquivo, carrega a versão do
  disco;
- **Ver diferenças** compara a cópia com o arquivo em disco, linha por linha:
  em vermelho (`−`) o que está só no disco, em verde (`+`) o que está só na
  cópia, e os trechos iguais recolhidos. A janela também tem **Restaurar** e
  **Descartar**. Fica indisponível quando o arquivo de origem não existe mais;
- **Decidir depois** mantém a cópia para a próxima abertura.

### Limpeza

Ao abrir o aplicativo e a cada hora, são apagados os backups com mais de 7
dias. Se a pasta ainda passar de 500 MB, os mais antigos saem primeiro. As
cópias de documentos abertos no momento nunca são apagadas.

### Configurações

Na aba **Backup** das configurações dá para ajustar os dias de retenção e o
tamanho máximo da pasta, além de abrir a pasta de backups.

Documentos abertos no modo texto não têm cópia de recuperação.

## Exportar CSV

Clique no botão com ícone de planilha, escolha o destino e confirme.

O arquivo gerado:

- usa UTF-8;
- separa colunas com ponto e vírgula (`;`);
- inclui uma linha de cabeçalho;
- coloca entre aspas campos com `;`, aspas ou quebra de linha;
- duplica aspas internas conforme a convenção CSV;
- não inclui as colunas internas de controle.

Na versão 0.5.4, a exportação inclui todas as linhas da tabela original, mesmo
que um filtro ou resultado SQL esteja visível.

## Configurações

Abra o botão **Configurações** no cabeçalho.

### Idioma

Escolha **Português** ou **English**. A preferência é salva imediatamente, mas
os textos da interface são atualizados somente após reiniciar o aplicativo.

### Esquema de cores

As opções são:

- **Seguir o sistema**;
- **Claro**;
- **Escuro**.

A mudança de tema é aplicada imediatamente. Por usar GTK4 puro, o Lê-XML herda
as cores e o destaque do tema GTK disponível no sistema.

A aba **Sobre** informa versão, autor e endereço do repositório.

## Atalhos

| Ação | Atalho |
| --- | --- |
| Salvar | `Ctrl+S` |
| Salvar como | `Ctrl+Shift+S` |
| Selecionar todas as linhas | `Ctrl+A` |
| Copiar / recortar / colar linhas | `Ctrl+C` / `Ctrl+X` / `Ctrl+V` |
| Estender a seleção | `Shift+↑` / `Shift+↓`, `Shift`+clique |
| Marcar linhas soltas | `Ctrl`+clique |
| Desfazer | `Ctrl+Z` |
| Refazer | `Ctrl+Shift+Z` ou `Ctrl+Y` |
| Ir para Localizar | `Ctrl+F` |
| Próxima ocorrência em Localizar | `Enter` |
| Abrir/fechar Substituir | `Ctrl+H` |
| Substituir e ir para a próxima | `Enter` em **Substituir por…** |
| Aplicar filtro | `Enter` no campo de filtro |
| Aceitar sugestão do filtro | `Tab` ou `Enter` |
| Navegar nas sugestões | `↑` e `↓` |
| Fechar sugestões | `Esc` |
| Navegar na grade | teclas de direção |
| Confirmar edição e avançar | `Tab` |
| Confirmar edição e voltar | `Shift+Tab` |
| Ordenar pela coluna | clique no cabeçalho |
| Renomear coluna | botão direito no cabeçalho → **Renomear coluna…** |
| Abrir operações da tabela | botão direito em uma célula |

## Cuidados e limitações

- O modo tabela é específico para o formato descrito em
  [Formato XML suportado](formato-xml.md).
- A consulta SQL aceita apenas leitura (`SELECT`, `WITH`, `VALUES`,
  `EXPLAIN`); comandos de alteração são recusados.
- Arquivos que não são UTF-8 válido, ou maiores que 64 MiB, abrem somente para
  leitura no modo texto.
- Nomes de coluna precisam ser nomes válidos de atributo XML.
- O histórico de desfazer/refazer guarda as últimas 500 alterações e não
  sobrevive ao fechamento da janela.
- O aplicativo não salva automaticamente no arquivo; ele guarda cópias de
  recuperação no cache (veja [Backup e recuperação](#backup-e-recuperação)).
- Salvar um documento tabular normaliza a formatação do XML; não preserva
  indentação e espaços do arquivo original.
- Células vazias são gravadas como atributos ausentes.
- A exportação CSV da versão atual não respeita o filtro visível.
- O aplicativo lê apenas arquivos em UTF-8.

Voltar ao [índice da documentação](README.md).
