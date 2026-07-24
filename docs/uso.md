# Guia de uso

Este guia descreve o comportamento da versão 0.5.3 do Lê-XML.

## Conteúdo

- [Abrir ou criar documentos](#abrir-ou-criar-documentos)
- [Modos de visualização](#modos-de-visualização)
- [Trabalhar com a tabela](#trabalhar-com-a-tabela)
- [Localizar valores](#localizar-valores)
- [Filtrar com SQL WHERE](#filtrar-com-sql-where)
- [Executar uma consulta SQL](#executar-uma-consulta-sql)
- [Somar uma coluna](#somar-uma-coluna)
- [Salvar](#salvar)
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
tabela**.

O modo texto oferece edição e salvamento, mas não disponibiliza filtros, SQL,
soma, operações de linha ou coluna e exportação CSV.

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

- **Nova linha acima** ou **Nova linha abaixo** cria uma linha vazia;
- **Clonar para cima** ou **Clonar para baixo** copia a linha e a posiciona ao
  lado da original;
- **Mover para cima** ou **Mover para baixo** troca a linha com sua vizinha;
- **Excluir linha** remove a linha.

No submenu **Coluna**:

- **Somar coluna** soma a coluna clicada;
- **Adicionar coluna** cria ao final uma coluna vazia com nome automático;
- **Excluir coluna** remove a coluna e seus valores.

O documento sempre precisa manter pelo menos uma coluna.

### Renomear uma coluna

Dê duplo clique no título da coluna, informe o novo nome e pressione `Enter`.
Nomes vazios ou repetidos não são aceitos. A alteração atualiza tanto a tabela
interna quanto o atributo `attrname` do `FIELD` ao salvar.

Operações de edição e estrutura ficam desabilitadas enquanto uma consulta SQL
livre está sendo exibida.

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

As colunas internas `_rid` e `__ord`, quando presentes, não são exibidas.

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

## Exportar CSV

Clique no botão com ícone de planilha, escolha o destino e confirme.

O arquivo gerado:

- usa UTF-8;
- separa colunas com ponto e vírgula (`;`);
- inclui uma linha de cabeçalho;
- coloca entre aspas campos com `;`, aspas ou quebra de linha;
- duplica aspas internas conforme a convenção CSV;
- não inclui as colunas internas `_rid` e `__ord`.

Na versão 0.5.3, a exportação inclui todas as linhas da tabela original, mesmo
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
| Próxima ocorrência em Localizar | `Enter` |
| Aplicar filtro | `Enter` no campo de filtro |
| Aceitar sugestão do filtro | `Tab` ou `Enter` |
| Navegar nas sugestões | `↑` e `↓` |
| Fechar sugestões | `Esc` |
| Navegar na grade | teclas de direção |
| Confirmar edição e avançar | `Tab` |
| Confirmar edição e voltar | `Shift+Tab` |
| Renomear coluna | duplo clique no cabeçalho |
| Abrir operações da tabela | botão direito em uma célula |

## Cuidados e limitações

- O modo tabela é específico para o formato descrito em
  [Formato XML suportado](formato-xml.md).
- A consulta SQL é destinada a consultas sobre a base temporária. Use
  preferencialmente instruções `SELECT`.
- O histórico de desfazer/refazer não é implementado.
- O aplicativo não salva automaticamente.
- Salvar um documento tabular normaliza a formatação do XML; não preserva
  indentação e espaços do arquivo original.
- Células vazias são gravadas como atributos ausentes.
- A exportação CSV da versão atual não respeita o filtro visível.

Voltar ao [índice da documentação](README.md).
