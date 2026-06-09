# Guia de uso

Visão geral da janela:

```
┌───────────────────────────────────────────────────────────────────────┐
│  [Abrir]                    Lê-XML                          _  □  ✕     │  ← cabeçalho
├───────────────────────────────────────────────────────────────────────┤
│ [Localizar…] [Filtrar (SQL WHERE)        ] [🔍] [SQL…] [coluna ▾] [Σ] [💾] [CSV] │  ← barra
├───────────────────────────────────────────────────────────────────────┤
│  id    │ nome    │ valor                                                │
│  1     │ Alpha   │ 10.50                                                │  ← tabela
│  2     │ Beta    │ 4.25                                                 │
│  …                                                                      │
├───────────────────────────────────────────────────────────────────────┤
│  Total: 3                                                               │  ← rodapé
└───────────────────────────────────────────────────────────────────────┘
```

---

## Abrir arquivos

- Clique em **Abrir** (cabeçalho) ou em **Abrir arquivo** (tela inicial), ou
  passe o caminho na linha de comando: `lexml-gtk arquivo.xml`.
- **Cada arquivo abre em uma janela nova.** Abrir outro arquivo **não fecha** o
  que já estava aberto — você pode comparar vários lado a lado.
- A primeira janela (ainda vazia) é reaproveitada no primeiro arquivo; a partir
  daí, cada **Abrir** cria uma nova janela. O nome do arquivo aparece no título
  da janela.

---

## Localizar (pesquisa de texto)

Campo **Localizar…** à esquerda da barra.

- Digite um texto e pressione **Enter** (ou apenas digite) para ir até a próxima
  célula que **contém** aquele texto, em qualquer coluna.
- Pressione **Enter** de novo para pular para a próxima ocorrência (faz a volta
  ao chegar no fim).
- A pesquisa **não esconde linhas** — ela só seleciona e rola até a célula
  encontrada. Para esconder/reduzir linhas, use o **Filtrar**.
- Não diferencia maiúsculas de minúsculas.

---

## Filtrar (SQL WHERE)

Campo **Filtrar (SQL WHERE)** no centro da barra. Você escreve **apenas a
condição** (o que viria depois de `WHERE`), e a tabela passa a mostrar só as
linhas que casam.

Exemplos:

```sql
valor <> '0.00'
nome LIKE 'A%'
id >= '2' AND valor <> ''
```

- Pressione **Enter** ou clique na **lupa** para aplicar.
- Deixe o campo **vazio** e aplique para voltar à tabela completa.
- Erros de SQL aparecem numa caixa de diálogo.
- Como os dados são guardados internamente em SQLite, vale **qualquer expressão
  válida de WHERE**.
- A visão filtrada continua **editável** (veja abaixo).

> Observação: os valores são texto, por isso compare entre aspas simples
> (`'0.00'`). Para comparação numérica, use `CAST(valor AS REAL) > 100`.

---

## SQL… (consulta livre, somente leitura)

Botão **SQL…**. Abre uma caixa para você escrever uma **consulta SQL completa**
sobre a tabela interna chamada `dados`.

```sql
SELECT nome, valor FROM dados ORDER BY valor DESC
SELECT nome, COUNT(*) FROM dados GROUP BY nome
```

- Serve para agrupamentos, ordenações, agregações etc.
- O resultado é **somente leitura** (não dá para editar células de uma consulta
  livre, porque a linha pode não corresponder a um registro único).
- Para editar, use a tabela completa ou um **Filtrar (WHERE)**.

---

## Σ Somar coluna

1. Escolha a coluna no **menu suspenso** (à esquerda do botão Σ).
2. Clique em **Σ Somar**.

O app soma todos os valores **numéricos** daquela coluna e mostra o total
formatado em pt-BR (ex.: `1.234.567,89`). Entende tanto `1234.56` quanto o
formato brasileiro `1.234,56`. Células não numéricas são ignoradas, e o diálogo
informa quantos valores entraram na conta.

---

## Editar células

- Dê **duplo clique** numa célula para editar; pressione **Enter** para
  confirmar (ou **Esc** para cancelar).
- A edição é gravada na base interna na hora.
- É possível editar quando a visão é a **tabela completa** ou um **Filtrar
  (WHERE)**. Resultados de **SQL… livre** são somente leitura.
- As alterações só vão para o disco quando você **Salvar**.

---

## Salvar (.xml)

Botão **💾**. Abre um seletor para gravar um `.xml`.

- O arquivo é reescrito no **mesmo formato** do original: a seção
  `METADATA/FIELDS` é preservada idêntica e as linhas são regravadas com suas
  edições.
- Atributos vazios são omitidos na linha (igual ao arquivo original).
- Dica: salve com um nome novo para manter o original intacto enquanto testa.

---

## Exportar CSV

Botão **CSV**. Gera um arquivo `.csv` da visão atual.

- Separador **`;`** (padrão pt-BR, abre direto no LibreOffice/Excel em
  português).
- Codificação **UTF-8** (acentos preservados).
- Campos com `;`, aspas ou quebras de linha são colocados entre aspas.

---

## Rodapé

Mostra o **Total** de linhas exibidas e, quando há filtro ou SQL ativo, a
expressão usada. Também exibe confirmações ao salvar ou exportar.
